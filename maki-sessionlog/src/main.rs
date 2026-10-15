use std::fs;
use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use color_eyre::eyre::{Context, Result, bail};
use maki_sessionlog::blame::{BlameOpts, BlameView};
use maki_sessionlog::effects::{DiffView, EffectsOpts};
use maki_sessionlog::events::build;
use maki_sessionlog::filter::Filter;
use maki_sessionlog::fs::{collect_logs, par_map};
use maki_sessionlog::history::HistoryOpts;
use maki_sessionlog::model::{LogError, ParseOpts, Role, Session};
use maki_sessionlog::render::{Format, RenderOpts, Thin};
use maki_sessionlog::stats::{Stats, approx_tokens, summary_line};
use maki_sessionlog::{blame_log, effects_log, history_log, render_log};
use regex::Regex;

const OUTPUT_SEPARATOR: &str = "\n---\n";
const DEFAULT_DIFF_CONTEXT: usize = 2;
const DEFAULT_EFFECT_EXAMPLES: usize = 5;

#[derive(Parser)]
#[command(
    version,
    about = "Extract, filter and thin maki session logs into token-efficient text"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render sessions as markdown or plain text
    Render(Box<RenderArgs>),
    /// Per-file edit history rebuilt from the stored edit diffs
    History(HistoryArgs),
    /// What the tool calls changed: files, commands, network, state, failures
    Effects(EffectsArgs),
    /// Effects plus the net git diff of every changed file
    Summary(SummaryArgs),
    /// Which edit introduced each line of a file's final state
    Blame(BlameArgs),
    /// Token cost by message kind and tool
    Stats(PathsArgs),
    /// One tab-separated line per session
    Ls(PathsArgs),
}

#[derive(Args)]
struct PathsArgs {
    /// Session files, or directories searched recursively for *.jsonl
    #[arg(required = true)]
    paths: Vec<PathBuf>,
}

#[derive(Args)]
struct HistoryArgs {
    #[command(flatten)]
    paths: PathsArgs,
    /// Only files whose path contains this text (repeatable)
    #[arg(long)]
    file: Vec<String>,
    /// Unchanged lines shown around each change
    #[arg(long, default_value_t = DEFAULT_DIFF_CONTEXT)]
    context: usize,
    /// List each step with its +/- line counts but no diff bodies
    #[arg(long)]
    stat: bool,
    /// Write one file per session into this directory instead of stdout
    #[arg(short, long)]
    out_dir: Option<PathBuf>,
}

#[derive(Args)]
struct EffectsArgs {
    #[command(flatten)]
    paths: PathsArgs,
    /// Distinct examples listed per command, target or failure
    #[arg(long, default_value_t = DEFAULT_EFFECT_EXAMPLES)]
    limit: usize,
    /// Write one file per session into this directory instead of stdout
    #[arg(short, long)]
    out_dir: Option<PathBuf>,
}

#[derive(Args)]
struct SummaryArgs {
    #[command(flatten)]
    paths: PathsArgs,
    /// Distinct examples listed per command, target or failure
    #[arg(long, default_value_t = DEFAULT_EFFECT_EXAMPLES)]
    limit: usize,
    /// Unchanged lines shown around each change in the git diff
    #[arg(long, default_value_t = DEFAULT_DIFF_CONTEXT)]
    context: usize,
    /// Cap each file's diff at this many lines (head and tail kept)
    #[arg(long)]
    diff_lines: Option<usize>,
    /// Leave out the trailing git diff section
    #[arg(long)]
    no_diff: bool,
    /// Replace the commands list with the model's replies interleaved with every command
    #[arg(long)]
    transcript: bool,
    /// Write one file per session into this directory instead of stdout
    #[arg(short, long)]
    out_dir: Option<PathBuf>,
}

#[derive(Args)]
struct BlameArgs {
    #[command(flatten)]
    paths: PathsArgs,
    /// Only files whose path contains this text (repeatable)
    #[arg(long)]
    file: Vec<String>,
    /// Only these lines of the final file, as N or START-END
    #[arg(long, value_parser = parse_line_range)]
    lines: Option<RangeInclusive<usize>>,
    /// lines: text grouped by origin; ranges: origin ranges only; summary: lines per origin
    #[arg(long, value_enum, default_value_t)]
    view: BlameView,
    /// Write one file per session into this directory instead of stdout
    #[arg(short, long)]
    out_dir: Option<PathBuf>,
}

fn parse_line_range(text: &str) -> Result<RangeInclusive<usize>, String> {
    let (start, end) = text.split_once('-').unwrap_or((text, text));
    let number = |part: &str| {
        part.parse::<usize>()
            .ok()
            .filter(|line| *line > 0)
            .ok_or_else(|| format!("{part:?} is not a line number"))
    };
    let (start, end) = (number(start)?, number(end)?);
    if start > end {
        return Err(format!("range {start}-{end} runs backwards"));
    }
    Ok(start..=end)
}

#[derive(Clone, Copy, ValueEnum)]
enum RoleArg {
    User,
    Assistant,
}

impl From<RoleArg> for Role {
    fn from(role: RoleArg) -> Self {
        match role {
            RoleArg::User => Self::User,
            RoleArg::Assistant => Self::Assistant,
        }
    }
}

#[derive(Args)]
struct RenderArgs {
    #[command(flatten)]
    paths: PathsArgs,
    /// Write one file per session into this directory instead of stdout
    #[arg(short, long)]
    out_dir: Option<PathBuf>,
    #[arg(short, long, value_enum, default_value_t = Format::Md)]
    format: Format,
    /// Skip the title and model/cwd/date line
    #[arg(long)]
    no_header: bool,
    /// Show edits as unified diffs, read from the stored tool outputs
    #[arg(long)]
    diffs: bool,
    /// Print input/output size to stderr
    #[arg(long)]
    report: bool,
    #[command(flatten)]
    select: SelectArgs,
    #[command(flatten)]
    thin: ThinArgs,
}

#[derive(Args)]
struct SelectArgs {
    /// Keep only prompts, or only the assistant's activity
    #[arg(long, value_enum)]
    role: Option<RoleArg>,
    /// Keep only calls to this tool (repeatable)
    #[arg(long)]
    tool: Vec<String>,
    /// Keep only failed tool calls
    #[arg(long)]
    errors: bool,
    /// Keep only prompts, replies, thinking and calls matching this regex
    #[arg(long)]
    grep: Option<Regex>,
    /// First message index to keep
    #[arg(long)]
    from: Option<usize>,
    /// Last message index to keep
    #[arg(long)]
    to: Option<usize>,
    /// Keep only the final N messages
    #[arg(long)]
    last: Option<usize>,
    #[arg(long)]
    no_thinking: bool,
    #[arg(long)]
    no_tools: bool,
    #[arg(long)]
    no_subagents: bool,
}

#[derive(Args)]
struct ThinArgs {
    /// Preset: clip outputs and inputs, drop old outputs, dedupe, squeeze
    #[arg(long)]
    thin: bool,
    /// Keep at most N lines per tool output (head and tail)
    #[arg(long)]
    output_lines: Option<usize>,
    /// Keep at most N chars per tool input value
    #[arg(long)]
    input_chars: Option<usize>,
    /// Keep at most N chars of each thinking block, 0 drops it
    #[arg(long)]
    thinking_chars: Option<usize>,
    /// Elide successful outputs older than the last N calls
    #[arg(long)]
    evict_after: Option<usize>,
    /// Collapse identical consecutive calls
    #[arg(long)]
    dedupe: bool,
    /// Cluster repetitive output lines
    #[arg(long)]
    squeeze: bool,
    /// Drop the description argument tools carry for the UI
    #[arg(long)]
    drop_descriptions: bool,
}

impl From<ThinArgs> for Thin {
    fn from(args: ThinArgs) -> Self {
        let mut thin = if args.thin {
            Self::preset()
        } else {
            Self::default()
        };
        thin.output_lines = args.output_lines.or(thin.output_lines);
        thin.input_chars = args.input_chars.or(thin.input_chars);
        thin.thinking_chars = args.thinking_chars.or(thin.thinking_chars);
        thin.evict_after = args.evict_after.or(thin.evict_after);
        thin.dedupe |= args.dedupe;
        thin.squeeze |= args.squeeze;
        thin.drop_descriptions |= args.drop_descriptions;
        thin
    }
}

impl From<SelectArgs> for Filter {
    fn from(args: SelectArgs) -> Self {
        Self {
            role: args.role.map(Role::from),
            tools: args.tool,
            errors_only: args.errors,
            grep: args.grep,
            from: args.from,
            to: args.to,
            last: args.last,
            no_thinking: args.no_thinking,
            no_tools: args.no_tools,
            no_subagents: args.no_subagents,
        }
    }
}

fn main() -> ExitCode {
    color_eyre::install().ok();
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

/// `false` when some files failed but the rest were still processed.
fn run(cli: Cli) -> Result<bool> {
    match cli.command {
        Command::Render(args) => render_command(*args),
        Command::History(args) => history_command(args),
        Command::Blame(args) => blame_command(args),
        Command::Effects(args) => effects_command(args),
        Command::Summary(args) => summary_command(args),
        Command::Stats(args) => stats_command(&args),
        Command::Ls(args) => ls_command(&args),
    }
}

fn logs_of(args: &PathsArgs) -> Result<Vec<PathBuf>> {
    let logs = collect_logs(&args.paths).wrap_err("cannot list session logs")?;
    if logs.is_empty() {
        bail!("no .jsonl session logs found");
    }
    Ok(logs)
}

fn read_log(path: &Path) -> Result<String> {
    fs::read_to_string(path).wrap_err_with(|| format!("cannot read {}", path.display()))
}

/// Not every `.jsonl` under the sessions dir is a session log (e.g. turnstats).
fn parse_session<'a>(path: &Path, text: &'a str) -> Result<Option<Session<'a>>> {
    match Session::parse(text, ParseOpts::default()) {
        Ok(session) => Ok(Some(session)),
        Err(LogError::MissingHeader) => Ok(None),
        Err(error) => Err(error).wrap_err_with(|| format!("cannot parse {}", path.display())),
    }
}

fn report_skipped(skipped: usize) {
    if skipped > 0 {
        eprintln!("skipped {skipped} files that are not session logs");
    }
}

enum Outcome {
    NotSession,
    NothingToShow,
    Rendered(Rendered),
}

struct Rendered {
    input_bytes: usize,
    output_bytes: usize,
    text: Option<String>,
}

fn render_command(args: RenderArgs) -> Result<bool> {
    let logs = logs_of(&args.paths)?;
    let filter = Filter::from(args.select);
    let opts = RenderOpts {
        format: args.format,
        thin: args.thin.into(),
        header: !args.no_header,
        diffs: args.diffs,
    };
    let extension = match args.format {
        Format::Md => "md",
        Format::Text => "txt",
    };
    let target = Target {
        out_dir: args.out_dir.as_deref(),
        extension,
        report: args.report,
    };
    emit(&logs, &target, |text| render_log(text, &filter, &opts))
}

fn history_command(args: HistoryArgs) -> Result<bool> {
    let logs = logs_of(&args.paths)?;
    let opts = HistoryOpts {
        context: args.context,
        stat_only: args.stat,
    };
    let target = Target {
        out_dir: args.out_dir.as_deref(),
        extension: "txt",
        report: false,
    };
    emit(&logs, &target, |text| history_log(text, &args.file, opts))
}

fn effects_command(args: EffectsArgs) -> Result<bool> {
    let logs = logs_of(&args.paths)?;
    let opts = EffectsOpts {
        limit: args.limit,
        diff: None,
        transcript: false,
    };
    let target = Target {
        out_dir: args.out_dir.as_deref(),
        extension: "txt",
        report: false,
    };
    emit(&logs, &target, |text| effects_log(text, opts))
}

fn summary_command(args: SummaryArgs) -> Result<bool> {
    let logs = logs_of(&args.paths)?;
    let opts = EffectsOpts {
        limit: args.limit,
        diff: (!args.no_diff).then_some(DiffView {
            context: args.context,
            max_lines: args.diff_lines,
        }),
        transcript: args.transcript,
    };
    let target = Target {
        out_dir: args.out_dir.as_deref(),
        extension: "txt",
        report: false,
    };
    emit(&logs, &target, |text| effects_log(text, opts))
}

fn blame_command(args: BlameArgs) -> Result<bool> {
    let logs = logs_of(&args.paths)?;
    let opts = BlameOpts {
        view: args.view,
        lines: args.lines,
    };
    let target = Target {
        out_dir: args.out_dir.as_deref(),
        extension: "txt",
        report: false,
    };
    emit(&logs, &target, |text| blame_log(text, &args.file, &opts))
}

struct Target<'a> {
    out_dir: Option<&'a Path>,
    extension: &'a str,
    report: bool,
}

fn emit(
    logs: &[PathBuf],
    target: &Target,
    produce: impl Fn(&str) -> Result<Option<String>, LogError> + Sync,
) -> Result<bool> {
    if let Some(dir) = target.out_dir {
        fs::create_dir_all(dir).wrap_err_with(|| format!("cannot create {}", dir.display()))?;
    }

    let results = par_map(logs, |path| -> Result<Outcome> {
        let input = read_log(path)?;
        let output = match produce(&input) {
            Ok(Some(output)) => output,
            Ok(None) => return Ok(Outcome::NothingToShow),
            Err(LogError::MissingHeader) => return Ok(Outcome::NotSession),
            Err(error) => {
                return Err(error).wrap_err_with(|| format!("cannot render {}", path.display()));
            }
        };
        let output_bytes = output.len();
        let text = match target.out_dir {
            Some(dir) => {
                let file = dir
                    .join(path.file_stem().unwrap_or_default())
                    .with_extension(target.extension);
                fs::write(&file, output)
                    .wrap_err_with(|| format!("cannot write {}", file.display()))?;
                None
            }
            None => Some(output),
        };
        Ok(Outcome::Rendered(Rendered {
            input_bytes: input.len(),
            output_bytes,
            text,
        }))
    });

    let mut stdout = io::stdout().lock();
    let (mut input_bytes, mut output_bytes, mut failures) = (0, 0, 0);
    let (mut not_sessions, mut empty, mut rendered_count) = (0, 0, 0);
    for result in results {
        match result {
            Ok(Outcome::NotSession) => not_sessions += 1,
            Ok(Outcome::NothingToShow) => empty += 1,
            Ok(Outcome::Rendered(rendered)) => {
                input_bytes += rendered.input_bytes;
                output_bytes += rendered.output_bytes;
                if let Some(text) = rendered.text {
                    if rendered_count > 0 {
                        stdout.write_all(OUTPUT_SEPARATOR.as_bytes())?;
                    }
                    stdout.write_all(text.as_bytes())?;
                }
                rendered_count += 1;
            }
            Err(error) => {
                failures += 1;
                eprintln!("{error:#}");
            }
        }
    }
    report_skipped(not_sessions);
    if target.report {
        eprintln!(
            "{rendered_count} rendered, {empty} empty after filtering: {input_bytes} -> {output_bytes} bytes (~{} -> ~{} tokens)",
            approx_tokens(input_bytes),
            approx_tokens(output_bytes)
        );
    }
    Ok(failures == 0)
}

fn stats_command(args: &PathsArgs) -> Result<bool> {
    let logs = logs_of(args)?;
    let results = par_map(&logs, |path| -> Result<Option<Stats>> {
        let text = read_log(path)?;
        let Some(session) = parse_session(path, &text)? else {
            return Ok(None);
        };
        let mut stats = Stats::default();
        stats.add(&session, &build(&session));
        Ok(Some(stats))
    });

    let mut total = Stats::default();
    let (mut ok, mut skipped) = (true, 0);
    for result in results {
        match result {
            Ok(None) => skipped += 1,
            Ok(Some(stats)) => total.merge(stats),
            Err(error) => {
                ok = false;
                eprintln!("{error:#}");
            }
        }
    }
    report_skipped(skipped);
    print!("{total}");
    Ok(ok)
}

fn ls_command(args: &PathsArgs) -> Result<bool> {
    let logs = logs_of(args)?;
    let lines = par_map(&logs, |path| -> Result<Option<String>> {
        let text = read_log(path)?;
        let Some(session) = parse_session(path, &text)? else {
            return Ok(None);
        };
        Ok(Some(summary_line(&session, &build(&session), text.len())))
    });

    let mut stdout = io::stdout().lock();
    let (mut ok, mut skipped) = (true, 0);
    for line in lines {
        match line {
            Ok(None) => skipped += 1,
            Ok(Some(line)) => writeln!(stdout, "{line}")?,
            Err(error) => {
                ok = false;
                eprintln!("{error:#}");
            }
        }
    }
    report_skipped(skipped);
    Ok(ok)
}
