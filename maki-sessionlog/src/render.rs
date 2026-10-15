use std::borrow::Cow;
use std::fmt::Write;
use std::sync::LazyLock;

use clap::ValueEnum;
use jiff::Timestamp;
use regex::Regex;
use serde_json::Value;
use similar::TextDiff;

use crate::events::{Call, Event, EventKind, Subagent};
use crate::model::{Role, Session};

const DATE_FORMAT: &str = "%Y-%m-%d %H:%M";
const HEADER_SEPARATOR: &str = " | ";
const BASH_TOOL: &str = "bash";
const BASH_COMMAND_KEY: &str = "command";
const DESCRIPTION_KEY: &str = "description";
const DIFF_CONTEXT_LINES: usize = 2;
const MD_FENCE_CHAR: char = '`';
const MD_MIN_FENCE: usize = 3;
const CLIP_ELLIPSIS: &str = "…";
const ANSI_PATTERN: &str = r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)";
const ESCAPE: char = '\u{1b}';
const MIN_COLLAPSED_RUN: usize = 3;

static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ANSI_PATTERN).expect("ANSI_PATTERN is a valid regex"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Md,
    Text,
}

#[derive(Debug, Clone, Default)]
pub struct Thin {
    pub output_lines: Option<usize>,
    pub input_chars: Option<usize>,
    pub thinking_chars: Option<usize>,
    pub evict_after: Option<usize>,
    pub dedupe: bool,
    pub squeeze: bool,
    pub drop_descriptions: bool,
}

impl Thin {
    pub const PRESET_OUTPUT_LINES: usize = 40;
    pub const PRESET_INPUT_CHARS: usize = 300;
    pub const PRESET_THINKING_CHARS: usize = 400;
    pub const PRESET_EVICT_AFTER: usize = 20;

    pub fn preset() -> Self {
        Self {
            output_lines: Some(Self::PRESET_OUTPUT_LINES),
            input_chars: Some(Self::PRESET_INPUT_CHARS),
            thinking_chars: Some(Self::PRESET_THINKING_CHARS),
            evict_after: Some(Self::PRESET_EVICT_AFTER),
            dedupe: true,
            squeeze: true,
            drop_descriptions: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderOpts {
    pub format: Format,
    pub thin: Thin,
    pub header: bool,
    pub diffs: bool,
}

pub fn render(session: &Session, events: &[Event], opts: &RenderOpts) -> String {
    let mut writer = Writer {
        opts,
        out: String::new(),
        last_role: None,
    };
    if opts.header {
        writer.header(session);
    }
    writer.events(events);
    writer.out
}

struct Writer<'o> {
    opts: &'o RenderOpts,
    out: String,
    last_role: Option<Role>,
}

impl Writer<'_> {
    fn is_md(&self) -> bool {
        self.opts.format == Format::Md
    }

    fn header(&mut self, session: &Session) {
        let marker = if self.is_md() { "# " } else { "" };
        self.out.push_str(&session_header(session, marker));
    }

    fn events(&mut self, events: &[Event]) {
        let total_calls = events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::Call(_)))
            .count();
        let mut calls_seen = 0;
        let mut index = 0;
        while index < events.len() {
            let event = &events[index];
            let repeats = self.repeats(events, index);
            index += repeats;
            self.role(event.kind.role());
            match &event.kind {
                EventKind::Prompt(text) | EventKind::Reply(text) => {
                    let _ = writeln!(self.out, "{text}");
                }
                EventKind::Thinking(text) => self.thinking(text),
                EventKind::Image => {
                    let _ = writeln!(self.out, "[image]");
                }
                EventKind::Detached(subagent) => self.subagent(subagent),
                EventKind::Call(call) => {
                    let evict = self
                        .opts
                        .thin
                        .evict_after
                        .is_some_and(|keep| calls_seen + keep < total_calls);
                    self.call(call, repeats, evict);
                    calls_seen += repeats;
                }
            }
        }
    }

    fn repeats(&self, events: &[Event], index: usize) -> usize {
        let EventKind::Call(first) = &events[index].kind else {
            return 1;
        };
        if !self.opts.thin.dedupe {
            return 1;
        }
        1 + events[index + 1..]
            .iter()
            .take_while(
                |next| matches!(&next.kind, EventKind::Call(call) if same_call(first, call)),
            )
            .count()
    }

    fn role(&mut self, role: Role) {
        if self.last_role == Some(role) {
            return;
        }
        self.last_role = Some(role);
        let name = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let _ = if self.is_md() {
            writeln!(self.out, "\n## {name}")
        } else {
            writeln!(self.out, "\n[{name}]")
        };
    }

    fn thinking(&mut self, text: &str) {
        let limit = self.opts.thin.thinking_chars;
        if limit == Some(0) {
            return;
        }
        let text = limit.map_or(Cow::Borrowed(text), |max| clip_chars(text, max));
        let _ = if self.is_md() {
            writeln!(self.out, "<thinking>\n{text}\n</thinking>")
        } else {
            writeln!(self.out, "[think] {text}")
        };
    }

    fn call(&mut self, call: &Call, repeats: usize, evict: bool) {
        let head = self.call_head(call);
        let repeat_suffix = if repeats > 1 {
            format!(" (x{repeats})")
        } else {
            String::new()
        };
        if self.is_md() && !head.contains('\n') {
            let _ = writeln!(self.out, "`{head}`{repeat_suffix}");
        } else if self.is_md() {
            let _ = writeln!(self.out, "{}{repeat_suffix}", fenced(&head));
        } else {
            let _ = writeln!(self.out, "[call] {head}{repeat_suffix}");
        }

        if let Some(output) = self
            .call_output(call, evict)
            .filter(|text| !text.is_empty())
        {
            self.output(&output, call.is_error);
        }
        if let Some(subagent) = &call.subagent {
            self.subagent(subagent);
        }
    }

    fn output(&mut self, text: &str, is_error: bool) {
        match (self.is_md(), is_error) {
            (true, true) => {
                let _ = writeln!(self.out, "error:\n{}", fenced(text));
            }
            (true, false) => {
                let _ = writeln!(self.out, "{}", fenced(text));
            }
            (false, true) => {
                let _ = writeln!(self.out, "[err]\n{text}");
            }
            (false, false) => {
                let _ = writeln!(self.out, "[out]\n{text}");
            }
        }
    }

    fn subagent(&mut self, subagent: &Subagent) {
        let label = [subagent.name, subagent.model]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(self.out, "<subagent {label}>");
        let outer_role = self.last_role.take();
        self.events(&subagent.events);
        self.last_role = outer_role;
        let _ = writeln!(self.out, "</subagent>");
    }

    fn call_output<'c>(&self, call: &'c Call, evict: bool) -> Option<Cow<'c, str>> {
        let thin = &self.opts.thin;
        let raw: Cow<str> = match call.diff.filter(|_| self.opts.diffs) {
            Some(diff) => Cow::Owned(unified_diff(&diff.before, &diff.after, DIFF_CONTEXT_LINES)),
            None => Cow::Borrowed(call.output?),
        };
        if evict && !call.is_error {
            return Some(Cow::Owned(format!(
                "[output elided: {} lines]",
                raw.lines().count()
            )));
        }
        let mut text = raw;
        if text.contains(ESCAPE) {
            text = Cow::Owned(ANSI.replace_all(&text, "").into_owned());
        }
        if thin.squeeze
            && let Some(collapsed) = collapse_repeated_lines(&text)
        {
            text = Cow::Owned(collapsed);
        }
        if let Some(max) = thin.output_lines
            && let Some(clipped) = clip_lines(&text, max)
        {
            text = Cow::Owned(clipped);
        }
        Some(text)
    }

    fn call_head(&self, call: &Call) -> String {
        let thin = &self.opts.thin;
        if let Some(diff) = call.diff.filter(|_| self.opts.diffs) {
            return format!("{} {}", call.name, diff.path);
        }
        let Value::Object(fields) = call.input else {
            return format!("{} {}", call.name, self.clip_input(&call.input.to_string()));
        };
        let fields: Vec<_> = fields
            .iter()
            .filter(|(key, _)| !(thin.drop_descriptions && *key == DESCRIPTION_KEY))
            .collect();
        match fields.as_slice() {
            [] => call.name.to_owned(),
            [(key, Value::String(text))] if call.name == BASH_TOOL && *key == BASH_COMMAND_KEY => {
                format!("$ {}", self.clip_input(text))
            }
            [(_, Value::String(text))] => format!("{}: {}", call.name, self.clip_input(text)),
            _ => {
                let mut head = call.name.to_owned();
                for (key, value) in fields {
                    let _ = write!(head, " {key}={}", self.clip_input(&field_text(value)));
                }
                head
            }
        }
    }

    fn clip_input<'t>(&self, text: &'t str) -> Cow<'t, str> {
        match self.opts.thin.input_chars {
            Some(max) => clip_chars(text, max),
            None => Cow::Borrowed(text),
        }
    }
}

fn same_call(a: &Call, b: &Call) -> bool {
    a.name == b.name && a.input == b.input && a.output == b.output
}

fn field_text(value: &Value) -> String {
    match value {
        Value::String(text) if is_bare(text) => text.clone(),
        other => other.to_string(),
    }
}

fn is_bare(text: &str) -> bool {
    !text.is_empty() && !text.contains(|c: char| c.is_whitespace() || c == '"')
}

pub(crate) fn session_header(session: &Session, title_marker: &str) -> String {
    let mut header = String::new();
    let title = session.title();
    if !title.is_empty() {
        let _ = writeln!(header, "{title_marker}{title}");
    }
    let fields = &session.header;
    let date = Timestamp::from_second(fields.created_at)
        .map(|timestamp| timestamp.strftime(DATE_FORMAT).to_string())
        .unwrap_or_default();
    let line = [&fields.model, &fields.cwd, date.as_str(), &fields.id];
    let _ = writeln!(header, "{}", line.join(HEADER_SEPARATOR));
    header
}

pub(crate) fn unified_diff(before: &str, after: &str, context: usize) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(context)
        .to_string()
}

fn fenced(text: &str) -> String {
    let longest_run = text
        .split(|c| c != MD_FENCE_CHAR)
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = MD_FENCE_CHAR
        .to_string()
        .repeat((longest_run + 1).max(MD_MIN_FENCE));
    format!("{fence}\n{}\n{fence}", text.trim_end_matches('\n'))
}

fn clip_chars(text: &str, max: usize) -> Cow<'_, str> {
    match text.char_indices().nth(max) {
        None => Cow::Borrowed(text),
        Some((cut, _)) => {
            let dropped = text[cut..].chars().count();
            Cow::Owned(format!("{}{CLIP_ELLIPSIS}(+{dropped} chars)", &text[..cut]))
        }
    }
}

pub(crate) fn clip_lines(text: &str, max: usize) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max {
        return None;
    }
    let head = max.div_ceil(2);
    let tail = max - head;
    let omitted = lines.len() - max;
    let mut clipped = lines[..head].join("\n");
    let _ = write!(clipped, "\n[{omitted} lines omitted]\n");
    clipped.push_str(&lines[lines.len() - tail..].join("\n"));
    Some(clipped)
}

fn collapse_repeated_lines(text: &str) -> Option<String> {
    let mut collapsed = String::new();
    let mut changed = false;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let mut run = 1;
        while lines.next_if_eq(&line).is_some() {
            run += 1;
        }
        collapsed.push_str(line);
        if run >= MIN_COLLAPSED_RUN {
            let _ = write!(collapsed, " [x{run}]");
            changed = true;
        } else {
            for _ in 1..run {
                collapsed.push('\n');
                collapsed.push_str(line);
            }
        }
        collapsed.push('\n');
    }
    changed.then_some(collapsed)
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::events::build;
    use crate::model::ParseOpts;

    const LOG: &str = concat!(
        r#"{"t":"header","v":2,"id":"sid","model":"mod","cwd":"/w","created_at":1790625106}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"user","content":[{"type":"text","text":"list files"}]}}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"assistant","content":[{"type":"thinking","thinking":"plan it"},{"type":"tool_use","id":"c1","name":"bash","input":{"command":"ls","description":"List"}}]}}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"a\nb\nc\nd\ne\nf"}]}}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
        "\n",
        r#"{"t":"meta","title":"Listing","updated_at":0}"#,
    );

    fn opts(format: Format, thin: Thin) -> RenderOpts {
        RenderOpts {
            format,
            thin,
            header: true,
            diffs: false,
        }
    }

    fn rendered(opts: &RenderOpts) -> String {
        let session = Session::parse(LOG, ParseOpts::default()).unwrap();
        render(&session, &build(&session), opts)
    }

    #[test]
    fn md_renders_roles_calls_and_fenced_output() {
        let text = rendered(&opts(Format::Md, Thin::default()));
        assert!(
            text.starts_with("# Listing\nmod | /w | 2026-09-28 "),
            "{text}"
        );
        assert!(text.contains("\n## user\nlist files\n"), "{text}");
        assert!(text.contains("<thinking>\nplan it\n</thinking>"), "{text}");
        assert!(
            text.contains("`bash command=ls description=List`"),
            "{text}"
        );
        assert!(text.contains("```\na\nb\nc\nd\ne\nf\n```"), "{text}");
        assert_eq!(text.matches("## assistant").count(), 1, "{text}");
    }

    #[test]
    fn text_format_has_no_markdown_syntax() {
        let text = rendered(&opts(Format::Text, Thin::default()));
        assert!(text.contains("[user]\nlist files"), "{text}");
        assert!(text.contains("[think] plan it"), "{text}");
        assert!(text.contains("[out]\na\nb"), "{text}");
        assert!(!text.contains("```") && !text.contains("##"), "{text}");
    }

    #[test]
    fn preset_thins_bash_and_clips_output() {
        let thin = Thin {
            output_lines: Some(4),
            drop_descriptions: true,
            ..Thin::default()
        };
        let text = rendered(&opts(Format::Text, thin));
        assert!(text.contains("[call] $ ls\n"), "{text}");
        assert!(text.contains("a\nb\n[2 lines omitted]\ne\nf"), "{text}");
    }

    #[test]
    fn zero_thinking_budget_drops_thinking() {
        let thin = Thin {
            thinking_chars: Some(0),
            ..Thin::default()
        };
        assert!(!rendered(&opts(Format::Md, thin)).contains("plan it"));
    }

    #[test_case("abcdef", 3, "abc…(+3 chars)"; "clipped")]
    #[test_case("abc", 3, "abc"; "exact")]
    #[test_case("héllo wörld", 5, "héllo…(+6 chars)"; "multibyte")]
    fn clips_chars_on_boundaries(text: &str, max: usize, expected: &str) {
        assert_eq!(clip_chars(text, max), expected);
    }

    #[test_case("a```b", "````\na```b\n````"; "wraps_longer_than_inner_run")]
    #[test_case("plain\n", "```\nplain\n```"; "min_fence")]
    fn fences_outgrow_content(text: &str, expected: &str) {
        assert_eq!(fenced(text), expected);
    }

    #[test]
    fn identical_consecutive_calls_collapse() {
        let call = |id: &str| {
            format!(
                r#"{{"t":"msg","d":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"bash","input":{{"command":"ls"}}}}]}}}}"#
            )
        };
        let result = |id: &str| {
            format!(
                r#"{{"t":"msg","d":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"x"}}]}}}}"#
            )
        };
        let text = [
            r#"{"t":"header","v":2,"id":"s","model":"m","cwd":"/","created_at":0}"#.to_owned(),
            call("1"),
            result("1"),
            call("2"),
            result("2"),
        ]
        .join("\n");
        let session = Session::parse(&text, ParseOpts::default()).unwrap();
        let thin = Thin {
            dedupe: true,
            ..Thin::default()
        };
        let out = render(&session, &build(&session), &opts(Format::Text, thin));
        assert_eq!(out.matches("[call]").count(), 1, "{out}");
        assert!(out.contains("(x2)"), "{out}");
    }

    #[test]
    fn old_outputs_are_evicted_but_errors_survive() {
        let mut lines = vec![
            r#"{"t":"header","v":2,"id":"s","model":"m","cwd":"/","created_at":0}"#.to_owned(),
        ];
        for (id, is_error) in [("1", true), ("2", false), ("3", false)] {
            lines.push(format!(
                r#"{{"t":"msg","d":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"read","input":{{"path":"f{id}"}}}}]}}}}"#
            ));
            lines.push(format!(
                r#"{{"t":"msg","d":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"body{id}\nmore","is_error":{is_error}}}]}}}}"#
            ));
        }
        let text = lines.join("\n");
        let session = Session::parse(&text, ParseOpts::default()).unwrap();
        let thin = Thin {
            evict_after: Some(1),
            ..Thin::default()
        };
        let out = render(&session, &build(&session), &opts(Format::Text, thin));
        assert!(out.contains("body1"), "{out}");
        assert!(out.contains("[output elided: 2 lines]"), "{out}");
        assert!(!out.contains("body2"), "{out}");
        assert!(out.contains("body3"), "{out}");
    }

    #[test]
    fn squeeze_collapses_only_long_runs_of_identical_lines() {
        let input = "a\nb\nb\nc\nc\nc\nc\nd";
        assert_eq!(
            collapse_repeated_lines(input).as_deref(),
            Some("a\nb\nb\nc [x4]\nd\n")
        );
        assert_eq!(collapse_repeated_lines("a\nb\nb\nc"), None);
    }

    #[test]
    fn ansi_escapes_are_stripped_from_outputs() {
        let log = LOG.replace(
            "a\\nb\\nc\\nd\\ne\\nf",
            "\\u001b[31mred\\u001b[0m\\u001b]0;title\\u0007ok",
        );
        let session = Session::parse(&log, ParseOpts::default()).unwrap();
        let out = render(
            &session,
            &build(&session),
            &opts(Format::Text, Thin::default()),
        );
        assert!(out.contains("redok"), "{out}");
        assert!(!out.contains('\u{1b}'), "{out}");
    }
}
