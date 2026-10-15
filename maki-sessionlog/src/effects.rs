use std::collections::BTreeMap;
use std::fmt::Write;

use serde_json::Value;

use crate::events::{Call, Event, EventKind};
use crate::history::{self, Change, FileHistory, is_snapshot, line_delta};
use crate::model::Session;
use crate::render::{clip_lines, session_header, unified_diff};
use crate::shell;

const SHELL_TOOLS: &[&str] = &["bash", "exec"];
const FILE_TOOLS: &[&str] = &["edit", "multiedit", "multiEdit", "edit_lines", "write"];
const SEMANTIC_EDIT_TOOLS: &[&str] = &[
    "compilerbrain__ReplaceMember",
    "compilerbrain__AddMember",
    "compilerbrain__DeleteMember",
    "compilerbrain__RenameSymbol",
    "compilerbrain__AddOrReplaceFile",
    "compilerbrain__BatchEdit",
    "compilerbrain__AddAttribute",
    "compilerbrain__SaveChangedCodeToDisc",
    "ast_grep_replace",
    "ast-grep__search_and_replace",
];
const MUTATING_MEMORY_COMMANDS: &[&str] = &["write", "edit", "delete"];
const MUTATING_CRON_ACTIONS: &[&str] = &["add", "edit", "delete"];
const TARGET_KEYS: &[&str] = &[
    "command",
    "path",
    "fileFullPath",
    "paths",
    "symbolId",
    "url",
    "query",
    "pattern",
    "name",
    "description",
];
const MAX_TEXT_CHARS: usize = 120;
const UNNAMED_SUBAGENT: &str = "subagent";
const NO_RESULT: &str = "no result recorded";
const NEW_FILE_SIDE: &str = "/dev/null";
const REASON_ELLIPSIS: &str = " … ";
const TRANSCRIPT_TITLE: &str = "model output and commands";
const COMMAND_PREFIX: &str = "$ ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Unmeasured,
    Commands,
    Network,
    Subagents,
    State,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Self::Unmeasured => "edits without a snapshot",
            Self::Commands => "commands",
            Self::Network => "network",
            Self::Subagents => "subagents",
            Self::State => "state",
        }
    }
}

#[derive(Debug)]
struct Action {
    group: Group,
    label: String,
    text: String,
    via: Option<String>,
}

#[derive(Debug)]
enum Moment<'a> {
    Reply { text: &'a str, via: Option<String> },
    Command(usize),
}

#[derive(Debug)]
struct Failure {
    tool: String,
    target: String,
    reason: String,
}

#[derive(Debug, Default)]
struct Counts {
    with_effects: usize,
    inspection_only: usize,
    failed: usize,
}

#[derive(Debug)]
pub struct Report<'a> {
    files: Vec<FileHistory<'a>>,
    actions: Vec<Action>,
    timeline: Vec<Moment<'a>>,
    failures: Vec<Failure>,
    counts: Counts,
}

#[derive(Debug, Clone, Copy)]
pub struct DiffView {
    pub context: usize,
    pub max_lines: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct EffectsOpts {
    pub limit: usize,
    pub diff: Option<DiffView>,
    pub transcript: bool,
}

pub fn collect<'a>(events: &[Event<'a>]) -> Report<'a> {
    let mut report = Report {
        files: history::collect(events, &[]),
        actions: Vec::new(),
        timeline: Vec::new(),
        failures: Vec::new(),
        counts: Counts::default(),
    };
    report.walk(events, None);
    report
}

impl<'a> Report<'a> {
    pub fn call_count(&self) -> usize {
        self.counts.with_effects + self.counts.inspection_only + self.counts.failed
    }

    fn walk(&mut self, events: &[Event<'a>], via: Option<&str>) {
        for event in events {
            let call = match &event.kind {
                EventKind::Call(call) => call,
                EventKind::Reply(text) => {
                    self.timeline.push(Moment::Reply {
                        text,
                        via: via.map(str::to_owned),
                    });
                    continue;
                }
                EventKind::Detached(subagent) => {
                    let label = subagent.name.or(via).unwrap_or(UNNAMED_SUBAGENT);
                    self.walk(&subagent.events, Some(label));
                    continue;
                }
                _ => continue,
            };
            let before = self.actions.len();
            self.record(call, via);
            for index in before..self.actions.len() {
                if self.actions[index].group == Group::Commands {
                    self.timeline.push(Moment::Command(index));
                }
            }
            if let Some(subagent) = &call.subagent {
                let label = subagent.name.or(via).unwrap_or(UNNAMED_SUBAGENT);
                self.walk(&subagent.events, Some(label));
            }
        }
    }

    fn record(&mut self, call: &Call, via: Option<&str>) {
        let failure = match (call.output, call.is_error) {
            (None, _) => Some(NO_RESULT.to_owned()),
            (Some(output), true) => Some(failure_reason(output)),
            (Some(_), false) => None,
        };
        if let Some(reason) = failure {
            self.counts.failed += 1;
            self.failures.push(Failure {
                tool: call.name.to_owned(),
                target: target_of(call.input).to_owned(),
                reason,
            });
            return;
        }

        let before = self.actions.len();
        let mut sink = Sink {
            actions: &mut self.actions,
            via,
        };
        sink.classify(call.name, call.input, is_snapshot(call));
        if self.actions.len() > before || is_snapshot(call) {
            self.counts.with_effects += 1;
        } else {
            self.counts.inspection_only += 1;
        }
    }
}

struct Sink<'a, 'v> {
    actions: &'a mut Vec<Action>,
    via: Option<&'v str>,
}

impl Sink<'_, '_> {
    fn push(&mut self, group: Group, label: &str, text: String) {
        self.actions.push(Action {
            group,
            label: label.to_owned(),
            text,
            via: self.via.map(str::to_owned),
        });
    }

    fn classify(&mut self, name: &str, input: &Value, covered_by_snapshot: bool) {
        match name {
            _ if SHELL_TOOLS.contains(&name) => {
                for invocation in shell::analyze(str_field(input, "command")).effects {
                    self.push(Group::Commands, &invocation.label, invocation.text);
                }
            }
            "code_execution" => {
                let lines = str_field(input, "code").lines().count();
                self.push(Group::Commands, name, format!("{name} ({lines} lines)"));
            }
            "webfetch" => self.push(
                Group::Network,
                name,
                format!("{name} {}", str_field(input, "url")),
            ),
            "websearch" => self.push(
                Group::Network,
                name,
                format!("{name} {}", str_field(input, "query")),
            ),
            "task" | "subtask" => self.push(
                Group::Subagents,
                name,
                format!(
                    "{name}[{}] {}",
                    str_field(input, "subagent_type"),
                    str_field(input, "description")
                ),
            ),
            "memory" if MUTATING_MEMORY_COMMANDS.contains(&str_field(input, "command")) => self
                .push(
                    Group::State,
                    name,
                    format!(
                        "memory {} {}",
                        str_field(input, "command"),
                        str_field(input, "path")
                    ),
                ),
            "cronjob" if MUTATING_CRON_ACTIONS.contains(&str_field(input, "action")) => self.push(
                Group::State,
                name,
                format!("cronjob {}", str_field(input, "action")),
            ),
            "create_plugin" => self.push(
                Group::State,
                name,
                format!("create_plugin {}", str_field(input, "name")),
            ),
            "async" if str_field(input, "action") == "spawn" => {
                for job in input
                    .get("jobs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let tool = str_field(job, "tool");
                    if let Some(parameters) = job.get("parameters") {
                        self.classify(tool, parameters, false);
                    }
                }
            }
            "batch" => {
                for entry in batch_entries(input) {
                    self.classify(str_field(&entry, "tool"), &entry, false);
                }
            }
            _ if (FILE_TOOLS.contains(&name) && !covered_by_snapshot)
                || SEMANTIC_EDIT_TOOLS.contains(&name) =>
            {
                self.push(
                    Group::Unmeasured,
                    name,
                    format!("{name} {}", target_of(input)),
                );
            }
            _ => {}
        }
    }
}

/// `batch` carries its calls as an array, or as that array encoded in a string.
fn batch_entries(input: &Value) -> Vec<Value> {
    match input.get("tool_calls") {
        Some(Value::Array(entries)) => entries.clone(),
        Some(Value::String(encoded)) => serde_json::from_str(encoded).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn str_field<'v>(input: &'v Value, key: &str) -> &'v str {
    input.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn target_of(input: &Value) -> &str {
    TARGET_KEYS
        .iter()
        .map(|key| str_field(input, key))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

/// The first line says what was attempted, the last usually carries the verdict
/// (exit code, exception), so a leading success message can't mask a failure.
fn failure_reason(output: &str) -> String {
    let mut lines = output.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or_default();
    match lines.next_back() {
        Some(last) => format!("{first}{REASON_ELLIPSIS}{last}"),
        None => first.to_owned(),
    }
}

fn clip(text: &str) -> String {
    let single_line = text.replace('\n', " ");
    match single_line.char_indices().nth(MAX_TEXT_CHARS) {
        Some((cut, _)) => format!("{}…", &single_line[..cut]),
        None => single_line,
    }
}

pub fn render(session: &Session, report: &Report, opts: EffectsOpts) -> String {
    let mut out = session_header(session, "# ");
    let counts = &report.counts;
    let _ = writeln!(
        out,
        "{} tool calls: {} with effects, {} inspection-only, {} failed",
        report.call_count(),
        counts.with_effects,
        counts.inspection_only,
        counts.failed
    );
    write_files(&mut out, &report.files);

    let mut by_group: BTreeMap<Group, Vec<&Action>> = BTreeMap::new();
    for action in &report.actions {
        by_group.entry(action.group).or_default().push(action);
    }
    if opts.transcript {
        by_group.entry(Group::Commands).or_default();
    }
    for (group, actions) in by_group {
        if opts.transcript && group == Group::Commands {
            write_transcript(&mut out, report);
            continue;
        }
        let _ = writeln!(out, "\n## {}", group.title());
        write_grouped(&mut out, &actions, opts.limit);
    }
    write_failures(&mut out, &report.failures, opts.limit);
    if let Some(view) = opts.diff {
        write_git_diff(&mut out, &report.files, view);
    }
    out
}

/// Replies and commands in the order they happened with none collapsed, so
/// the model's reasoning reads next to what it ran.
fn write_transcript(out: &mut String, report: &Report) {
    let _ = writeln!(out, "\n## {TRANSCRIPT_TITLE}");
    for moment in &report.timeline {
        match moment {
            Moment::Reply { text, via } => {
                if let Some(label) = via {
                    let _ = write!(out, "({label}) ");
                }
                let _ = writeln!(out, "{}", text.trim());
            }
            Moment::Command(index) => {
                let _ = writeln!(out, "{COMMAND_PREFIX}{}", describe(&report.actions[*index]));
            }
        }
    }
}

fn write_files(out: &mut String, files: &[FileHistory]) {
    if files.is_empty() {
        return;
    }
    let totals = files
        .iter()
        .map(file_delta)
        .fold((0, 0), |(a, r), (fa, fr)| (a + fa, r + fr));
    let _ = writeln!(
        out,
        "\n## files changed ({}, +{} -{})",
        files.len(),
        totals.0,
        totals.1
    );
    for file in files {
        let (added, removed) = file_delta(file);
        let mut tools: Vec<(&str, usize)> = Vec::new();
        for step in &file.steps {
            match tools.iter_mut().find(|(tool, _)| *tool == step.tool) {
                Some((_, count)) => *count += 1,
                None => tools.push((step.tool, 1)),
            }
        }
        let tools: Vec<_> = tools
            .iter()
            .map(|(tool, count)| format!("{tool}×{count}"))
            .collect();
        let _ = writeln!(
            out,
            "{} {} +{added} -{removed} {}",
            if was_created(file) { 'A' } else { 'M' },
            file.path,
            tools.join(" ")
        );
    }
}

fn was_created(file: &FileHistory) -> bool {
    matches!(
        file.steps.first().map(|step| &step.change),
        Some(Change::Write { before: "", .. })
    )
}

/// Each file's journey in one diff, from its first recorded state to its last,
/// so edits that undo each other cancel out.
fn write_git_diff(out: &mut String, files: &[FileHistory], view: DiffView) {
    let heading_at = out.len();
    for file in files {
        let (Some(first), Some(last)) = (file.steps.first(), file.steps.last()) else {
            continue;
        };
        let diff = unified_diff(first.change.sides().0, last.change.sides().1, view.context);
        if diff.is_empty() {
            continue;
        }
        if out.len() == heading_at {
            out.push_str("\n## git diff\n");
        }
        let old_side = if was_created(file) {
            NEW_FILE_SIDE
        } else {
            file.path
        };
        let _ = writeln!(out, "--- {old_side}\n+++ {}", file.path);
        let body = view
            .max_lines
            .and_then(|max| clip_lines(&diff, max))
            .unwrap_or(diff);
        out.push_str(&body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
}

fn file_delta(file: &FileHistory) -> (usize, usize) {
    match (file.steps.first(), file.steps.last()) {
        (Some(first), Some(last)) => line_delta(first.change.sides().0, last.change.sides().1),
        _ => (0, 0),
    }
}

fn write_grouped(out: &mut String, actions: &[&Action], limit: usize) {
    let mut labels: Vec<&str> = Vec::new();
    for action in actions {
        if !labels.contains(&action.label.as_str()) {
            labels.push(&action.label);
        }
    }
    for label in labels {
        let members: Vec<&&Action> = actions
            .iter()
            .filter(|action| action.label == label)
            .collect();
        if let [only] = members.as_slice() {
            let _ = writeln!(out, "{}", describe(only));
            continue;
        }
        let _ = writeln!(out, "{label} ×{}", members.len());
        let mut shown: Vec<String> = Vec::new();
        for member in &members {
            let line = describe(member);
            if !shown.contains(&line) {
                shown.push(line);
            }
        }
        for line in shown.iter().take(limit) {
            let _ = writeln!(out, "  {line}");
        }
        if shown.len() > limit {
            let _ = writeln!(out, "  … {} more distinct", shown.len() - limit);
        }
    }
}

fn describe(action: &Action) -> String {
    let via = action
        .via
        .as_deref()
        .map_or(String::new(), |name| format!(" (via {name})"));
    format!("{}{via}", clip(&action.text))
}

fn write_failures(out: &mut String, failures: &[Failure], limit: usize) {
    if failures.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n## failed, no effect recorded ({})", failures.len());
    let mut lines: Vec<(String, usize)> = Vec::new();
    for failure in failures {
        let line = format!(
            "{} {} → {}",
            failure.tool,
            clip(&failure.target),
            clip(&failure.reason)
        );
        match lines.iter_mut().find(|(existing, _)| *existing == line) {
            Some((_, count)) => *count += 1,
            None => lines.push((line, 1)),
        }
    }
    for (line, count) in lines.iter().take(limit) {
        let suffix = if *count > 1 {
            format!(" ×{count}")
        } else {
            String::new()
        };
        let _ = writeln!(out, "{line}{suffix}");
    }
    if lines.len() > limit {
        let _ = writeln!(out, "… {} more distinct", lines.len() - limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::build;
    use crate::model::ParseOpts;
    use crate::testutil::{call_with_input, diff_out, log, reply, result, thought};

    const LIMIT: usize = 3;

    fn report_of(lines: &[String]) -> String {
        let text = log(lines);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let events = build(&session);
        render(
            &session,
            &collect(&events),
            EffectsOpts {
                limit: LIMIT,
                diff: None,
                transcript: false,
            },
        )
    }

    fn report_with_diff(lines: &[String], view: DiffView) -> String {
        let text = log(lines);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let events = build(&session);
        let opts = EffectsOpts {
            limit: LIMIT,
            diff: Some(view),
            transcript: false,
        };
        render(&session, &collect(&events), opts)
    }

    fn two_edits_to_one_file() -> [String; 5] {
        [
            call_with_input("w", "write", r#"{"path":"a.rs","content":"1\n2\n3\n"}"#),
            result("w", "ok", false),
            call_with_input("e", "edit", r#"{"path":"a.rs"}"#),
            diff_out("e", "a.rs", "1\\n2\\n3\\n", "1\\nTWO\\n3\\n4\\n"),
            result("e", "ok", false),
        ]
    }

    fn bash(id: &str, command: &str, output: &str, is_error: bool) -> [String; 2] {
        let input = serde_json::json!({ "command": command }).to_string();
        [
            call_with_input(id, "bash", &input),
            result(id, output, is_error),
        ]
    }

    #[test]
    fn only_effectful_commands_are_listed_and_inspection_is_counted() {
        let mut lines = Vec::new();
        lines.extend(bash("1", "ls && cat a | wc -l", "ok", false));
        lines.extend(bash("2", "git commit -m done", "ok", false));
        let out = report_of(&lines);

        assert!(
            out.contains("2 tool calls: 1 with effects, 1 inspection-only, 0 failed"),
            "{out}"
        );
        assert!(out.contains("## commands\ngit commit -m done\n"), "{out}");
        assert!(!out.contains("cat"), "{out}");
    }

    #[test]
    fn failed_calls_report_their_tool_output_and_no_effect() {
        let lines = bash("1", "git push", "Exit code: 1", true);
        let out = report_of(&lines);

        assert!(out.contains("1 failed"), "{out}");
        assert!(
            out.contains("## failed, no effect recorded (1)\nbash git push → Exit code: 1\n"),
            "{out}"
        );
        assert!(!out.contains("## commands"), "{out}");
    }

    #[test]
    fn the_git_diff_section_shows_one_net_diff_per_file_after_everything_else() {
        let view = DiffView {
            context: 1,
            max_lines: None,
        };
        let out = report_with_diff(&two_edits_to_one_file(), view);

        assert!(out.contains("A a.rs +4 -0 write×1 edit×1\n"), "{out}");
        assert!(
            out.ends_with(
                "## git diff\n--- /dev/null\n+++ a.rs\n@@ -0,0 +1,4 @@\n+1\n+TWO\n+3\n+4\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn diff_view_clips_long_diffs_to_the_line_cap() {
        let view = DiffView {
            context: 1,
            max_lines: Some(3),
        };
        let out = report_with_diff(&two_edits_to_one_file(), view);

        assert!(out.contains("[2 lines omitted]"), "{out}");
        assert!(!out.contains("+TWO"), "{out}");
    }

    fn transcript_of(lines: &[String]) -> String {
        let text = log(lines);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let events = build(&session);
        let opts = EffectsOpts {
            limit: LIMIT,
            diff: None,
            transcript: true,
        };
        render(&session, &collect(&events), opts)
    }

    #[test]
    fn transcript_interleaves_replies_with_commands_and_drops_thinking() {
        let mut lines = vec![thought("secret plan"), reply("Building first.")];
        lines.extend(bash("1", "cargo build", "ok", false));
        lines.push(reply("Now testing."));
        lines.extend(bash("2", "cargo build", "ok", false));
        let out = transcript_of(&lines);

        assert!(
            out.contains(
                "## model output and commands\nBuilding first.\n$ cargo build\nNow testing.\n$ cargo build\n"
            ),
            "{out}"
        );
        assert!(!out.contains("secret plan"), "{out}");
        assert!(!out.contains("## commands"), "{out}");
    }

    #[test]
    fn transcript_section_appears_even_without_commands() {
        let lines = [
            reply("Reading only."),
            call_with_input("1", "read", r#"{"path":"a.rs"}"#),
            result("1", "text", false),
        ];
        let out = transcript_of(&lines);

        assert!(
            out.contains("## model output and commands\nReading only.\n"),
            "{out}"
        );
    }

    #[test]
    fn the_git_diff_heading_is_omitted_when_nothing_changed() {
        let lines = [
            call_with_input("1", "read", r#"{"path":"a.rs"}"#),
            result("1", "text", false),
        ];
        let view = DiffView {
            context: 1,
            max_lines: None,
        };
        let out = report_with_diff(&lines, view);

        assert!(!out.contains("## git diff"), "{out}");
    }

    #[test]
    fn without_a_diff_view_only_counts_are_shown() {
        let out = report_of(&two_edits_to_one_file());

        assert!(!out.contains("@@"), "{out}");
    }

    #[test]
    fn a_failure_keeps_its_verdict_when_the_output_starts_with_success() {
        let lines = bash(
            "1",
            "python check.py",
            "decay ok\\nTraceback\\nExit code: 1",
            true,
        );
        let out = report_of(&lines);

        assert!(
            out.contains("bash python check.py → decay ok … Exit code: 1\n"),
            "{out}"
        );
    }

    #[test]
    fn a_long_command_does_not_hide_the_failure_reason() {
        let command = "x".repeat(MAX_TEXT_CHARS * 2);
        let lines = bash("1", &command, "Exit code: 2", true);
        let out = report_of(&lines);

        assert!(out.contains("… → Exit code: 2\n"), "{out}");
    }

    #[test]
    fn multi_line_commands_are_reported_on_one_line() {
        let lines = bash(
            "1",
            "python3 - <<'EOF'\nprint(1)\nEOF",
            "Exit code: 1",
            true,
        );
        let out = report_of(&lines);

        assert!(
            out.contains("bash python3 - <<'EOF' print(1) EOF → Exit code: 1\n"),
            "{out}"
        );
    }

    #[test]
    fn a_call_without_a_result_is_not_counted_as_an_effect() {
        let lines = [call_with_input("1", "bash", r#"{"command":"rm -rf x"}"#)];
        let out = report_of(&lines);

        assert!(out.contains("bash rm -rf x → no result recorded"), "{out}");
        assert!(!out.contains("## commands"), "{out}");
    }

    #[test]
    fn files_come_from_snapshots_with_net_line_counts() {
        let lines = [
            call_with_input("w", "write", r#"{"path":"a.rs","content":"1\n2\n"}"#),
            result("w", "ok", false),
            call_with_input("e", "edit", r#"{"path":"a.rs"}"#),
            diff_out("e", "a.rs", "1\\n2\\n", "1\\n2\\n3\\n"),
            result("e", "ok", false),
        ];
        let out = report_of(&lines);

        assert!(
            out.contains("## files changed (1, +3 -0)\nA a.rs +3 -0 write×1 edit×1\n"),
            "{out}"
        );
        assert!(!out.contains("without a snapshot"), "{out}");
    }

    #[test]
    fn batch_and_async_inner_calls_are_expanded() {
        let batch = serde_json::json!({
            "tool_calls": serde_json::json!([{"tool": "websearch", "query": "rust lexers"}]).to_string()
        })
        .to_string();
        let spawn = serde_json::json!({
            "action": "spawn",
            "jobs": [{"name": "j", "tool": "bash", "parameters": {"command": "cargo test"}}]
        })
        .to_string();
        let lines = [
            call_with_input("b", "batch", &batch),
            result("b", "ok", false),
            call_with_input("a", "async", &spawn),
            result("a", "ok", false),
        ];
        let out = report_of(&lines);

        assert!(out.contains("## network\nwebsearch rust lexers\n"), "{out}");
        assert!(out.contains("## commands\ncargo test\n"), "{out}");
    }

    #[test]
    fn memory_reads_are_inspection_and_writes_are_state() {
        let lines = [
            call_with_input("r", "memory", r#"{"command":"read","path":"a.md"}"#),
            result("r", "text", false),
            call_with_input("w", "memory", r#"{"command":"write","path":"b.md"}"#),
            result("w", "ok", false),
        ];
        let out = report_of(&lines);

        assert!(out.contains("## state\nmemory write b.md\n"), "{out}");
        assert!(out.contains("1 with effects, 1 inspection-only"), "{out}");
    }

    #[test]
    fn semantic_edits_without_snapshots_are_flagged_as_unmeasured() {
        let lines = [
            call_with_input("s", "compilerbrain__ReplaceMember", r#"{"symbolId":"A.B"}"#),
            result("s", "ok", false),
        ];
        let out = report_of(&lines);

        assert!(
            out.contains("## edits without a snapshot\ncompilerbrain__ReplaceMember A.B\n"),
            "{out}"
        );
    }

    #[test]
    fn repeated_labels_collapse_with_distinct_examples_up_to_the_limit() {
        let mut lines = Vec::new();
        for (index, command) in ["rm a", "rm b", "rm c", "rm d", "rm a"].iter().enumerate() {
            lines.extend(bash(&index.to_string(), command, "ok", false));
        }
        let out = report_of(&lines);

        assert!(
            out.contains("rm ×5\n  rm a\n  rm b\n  rm c\n  … 1 more distinct\n"),
            "{out}"
        );
    }
}
