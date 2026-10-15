use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::events::{Call, Event, EventKind};
use crate::model::{Diff, Session};
use crate::render::{session_header, unified_diff};

const UNATTRIBUTED_LABEL: &str = "unattributed";
const WRITE_TOOL: &str = "write";
const PATH_KEY: &str = "path";
const CONTENT_KEY: &str = "content";
const APPEND_KEY: &str = "append";
const UNNAMED_SUBAGENT: &str = "subagent";

#[derive(Debug)]
pub enum Change<'a> {
    Edit(&'a Diff),
    Write { before: &'a str, after: &'a str },
    Unattributed { before: &'a str, after: &'a str },
}

impl<'a> Change<'a> {
    pub(crate) fn sides(&self) -> (&'a str, &'a str) {
        match self {
            Self::Edit(diff) => (&diff.before, &diff.after),
            Self::Write { before, after } | Self::Unattributed { before, after } => (before, after),
        }
    }
}

#[derive(Debug)]
pub struct Step<'a> {
    pub at: usize,
    pub tool: &'a str,
    pub via: Option<&'a str>,
    pub change: Change<'a>,
}

#[derive(Debug)]
pub struct FileHistory<'a> {
    pub path: &'a str,
    pub steps: Vec<Step<'a>>,
}

impl FileHistory<'_> {
    pub(crate) fn edit_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| !matches!(step.change, Change::Unattributed { .. }))
            .count()
    }

    fn lines_at_start(&self) -> usize {
        self.steps
            .first()
            .map_or(0, |step| step.change.sides().0.lines().count())
    }

    fn lines_at_end(&self) -> usize {
        self.steps
            .last()
            .map_or(0, |step| step.change.sides().1.lines().count())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HistoryOpts {
    pub context: usize,
    pub stat_only: bool,
}

/// An edit whose `before` differs from the previous edit's `after` means the
/// file changed in a way the log never saw, so that gap is recorded as its own
/// unattributed step instead of being silently stitched over.
pub fn collect<'a>(events: &[Event<'a>], only_paths: &[String]) -> Vec<FileHistory<'a>> {
    let mut pending = Vec::new();
    gather(events, None, &mut pending);
    let aliases = relative_aliases(&pending);
    let mut histories = fold(&pending, &aliases);
    histories.retain(|history| {
        only_paths.is_empty()
            || only_paths
                .iter()
                .any(|wanted| history.path.contains(wanted))
    });
    histories
}

#[derive(Clone)]
enum Recorded<'a> {
    Diff(&'a Diff),
    Write { path: &'a str, content: &'a str },
}

impl<'a> Recorded<'a> {
    fn path(&self) -> &'a str {
        match self {
            Self::Diff(diff) => &diff.path,
            Self::Write { path, .. } => path,
        }
    }

    fn before(&self) -> Option<&'a str> {
        match self {
            Self::Diff(diff) => Some(&diff.before),
            Self::Write { .. } => None,
        }
    }

    fn after(&self) -> &'a str {
        match self {
            Self::Diff(diff) => &diff.after,
            Self::Write { content, .. } => content,
        }
    }
}

struct Pending<'a> {
    at: usize,
    tool: &'a str,
    via: Option<&'a str>,
    recorded: Recorded<'a>,
}

/// `write` leaves no diff, but unless it appends, its input is the whole new
/// file. Appends would need the previous content, which the log may not hold.
fn recorded<'a>(call: &Call<'a>) -> Option<Recorded<'a>> {
    if let Some(diff) = call.diff {
        return Some(Recorded::Diff(diff));
    }
    let succeeded = call.output.is_some() && !call.is_error;
    let appends = call.input.get(APPEND_KEY).and_then(Value::as_bool) == Some(true);
    if call.name != WRITE_TOOL || !succeeded || appends {
        return None;
    }
    Some(Recorded::Write {
        path: call.input.get(PATH_KEY)?.as_str()?,
        content: call.input.get(CONTENT_KEY)?.as_str()?,
    })
}

pub(crate) fn is_snapshot(call: &Call) -> bool {
    recorded(call).is_some()
}

fn gather<'a>(events: &[Event<'a>], via: Option<&'a str>, pending: &mut Vec<Pending<'a>>) {
    for event in events {
        let call = match &event.kind {
            EventKind::Call(call) => call,
            EventKind::Detached(subagent) => {
                let label = subagent.name.or(via).unwrap_or(UNNAMED_SUBAGENT);
                gather(&subagent.events, Some(label), pending);
                continue;
            }
            _ => continue,
        };
        if let Some(recorded) = recorded(call) {
            pending.push(Pending {
                at: event.at,
                tool: call.name,
                via,
                recorded,
            });
        }
        if let Some(subagent) = &call.subagent {
            let label = subagent.name.or(via).unwrap_or(UNNAMED_SUBAGENT);
            gather(&subagent.events, Some(label), pending);
        }
    }
}

/// Tools are free to name a file relatively, and the working directory they
/// resolved it against is not recorded. A relative path is therefore folded
/// into an absolute one only when the absolute path is the only one ending with
/// it and the later of the two starts from exactly the content the earlier left.
fn relative_aliases<'a>(pending: &[Pending<'a>]) -> HashMap<&'a str, &'a str> {
    let paths: HashSet<&str> = pending.iter().map(|item| item.recorded.path()).collect();
    let mut aliases = HashMap::new();
    for relative in paths.iter().filter(|path| !path.starts_with('/')) {
        let suffix = format!("/{relative}");
        let mut candidates = paths.iter().filter(|path| path.ends_with(&suffix));
        if let (Some(absolute), None) = (candidates.next(), candidates.next())
            && continues_across(pending, relative, absolute)
        {
            aliases.insert(*relative, *absolute);
        }
    }
    aliases
}

fn continues_across(pending: &[Pending], relative: &str, absolute: &str) -> bool {
    let first_index = |path: &str| pending.iter().position(|item| item.recorded.path() == path);
    let (Some(first_relative), Some(first_absolute)) =
        (first_index(relative), first_index(absolute))
    else {
        return false;
    };
    let (earlier, later_start) = if first_relative < first_absolute {
        (relative, first_absolute)
    } else {
        (absolute, first_relative)
    };
    let last_earlier = pending[..later_start]
        .iter()
        .rev()
        .find(|item| item.recorded.path() == earlier);
    last_earlier
        .zip(pending[later_start].recorded.before())
        .is_some_and(|(earlier_item, later_before)| earlier_item.recorded.after() == later_before)
}

fn fold<'a>(pending: &[Pending<'a>], aliases: &HashMap<&'a str, &'a str>) -> Vec<FileHistory<'a>> {
    let mut histories: Vec<FileHistory> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for item in pending {
        let raw_path = item.recorded.path();
        let path = aliases.get(raw_path).copied().unwrap_or(raw_path);
        let slot = *index.entry(path).or_insert_with(|| {
            histories.push(FileHistory {
                path,
                steps: Vec::new(),
            });
            histories.len() - 1
        });
        let history = &mut histories[slot];
        let previous_after = history.steps.last().map(|step| step.change.sides().1);
        let change = match &item.recorded {
            Recorded::Diff(diff) => {
                if let Some(previous_after) = previous_after.filter(|after| *after != diff.before) {
                    history.steps.push(Step {
                        at: item.at,
                        tool: UNATTRIBUTED_LABEL,
                        via: item.via,
                        change: Change::Unattributed {
                            before: previous_after,
                            after: &diff.before,
                        },
                    });
                }
                Change::Edit(diff)
            }
            Recorded::Write { content, .. } => Change::Write {
                before: previous_after.unwrap_or_default(),
                after: content,
            },
        };
        history.steps.push(Step {
            at: item.at,
            tool: item.tool,
            via: item.via,
            change,
        });
    }
    histories
}

pub fn render(session: &Session, histories: &[FileHistory], opts: HistoryOpts) -> String {
    let mut out = session_header(session, "# ");
    for history in histories {
        let gaps = history.steps.len() - history.edit_count();
        let _ = writeln!(
            out,
            "\n## {} ({} edits, {gaps} unattributed, {} -> {} lines)",
            history.path,
            history.edit_count(),
            history.lines_at_start(),
            history.lines_at_end(),
        );
        for step in &history.steps {
            let (before, after) = step.change.sides();
            let (added, removed) = line_delta(before, after);
            let via = step
                .via
                .map_or(String::new(), |name| format!(" via {name}"));
            let _ = writeln!(
                out,
                "[msg {} {}{via} +{added} -{removed}]",
                step.at, step.tool
            );
            if !opts.stat_only {
                out.push_str(&unified_diff(before, after, opts.context));
            }
        }
    }
    out
}

pub(crate) fn line_delta(before: &str, after: &str) -> (usize, usize) {
    TextDiff::from_lines(before, after).iter_all_changes().fold(
        (0, 0),
        |(added, removed), change| match change.tag() {
            ChangeTag::Insert => (added + 1, removed),
            ChangeTag::Delete => (added, removed + 1),
            ChangeTag::Equal => (added, removed),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::build;
    use crate::model::ParseOpts;
    use crate::testutil::{call, call_with_input, diff_out, log as parse, result};

    const CONTEXT: usize = 1;

    fn histories_of<'a>(session: &'a Session<'a>, only: &[String]) -> Vec<FileHistory<'a>> {
        collect(&build(session), only)
    }

    #[test]
    fn chained_edits_have_no_unattributed_step() {
        let text = parse(&[
            call("c1", "edit"),
            diff_out("c1", "a.rs", "x\\ny\\n", "x\\nz\\n"),
            call("c2", "multiedit"),
            diff_out("c2", "a.rs", "x\\nz\\n", "x\\nz\\nw\\n"),
        ]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let histories = histories_of(&session, &[]);

        assert_eq!(histories.len(), 1);
        let tools: Vec<_> = histories[0].steps.iter().map(|step| step.tool).collect();
        assert_eq!(tools, ["edit", "multiedit"]);
    }

    #[test]
    fn a_changed_file_between_edits_becomes_an_unattributed_step() {
        let text = parse(&[
            call("c1", "edit"),
            diff_out("c1", "a.rs", "x\\n", "y\\n"),
            call("c2", "edit"),
            diff_out("c2", "a.rs", "Y\\n", "Z\\n"),
        ]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let histories = histories_of(&session, &[]);

        let steps = &histories[0].steps;
        assert_eq!(steps.len(), 3);
        assert!(matches!(
            steps[1].change,
            Change::Unattributed {
                before: "y\n",
                after: "Y\n"
            }
        ));
    }

    #[test]
    fn path_filter_matches_substrings() {
        let text = parse(&[
            call("c1", "edit"),
            diff_out("c1", "src/a.rs", "1\\n", "2\\n"),
            call("c2", "edit"),
            diff_out("c2", "docs/b.md", "1\\n", "2\\n"),
        ]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let histories = histories_of(&session, &["docs/".to_owned()]);

        assert_eq!(histories.len(), 1);
        assert_eq!(histories[0].path, "docs/b.md");
    }

    #[test]
    fn subagent_edits_join_the_same_file_history() {
        let sub_call = r#"{"t":"sub_msg","sub":"t1","d":{"role":"assistant","content":[{"type":"tool_use","id":"s1","name":"edit","input":{}}]}}"#;
        let text = parse(&[
            call("t1", "task"),
            sub_call.to_owned(),
            diff_out("s1", "a.rs", "1\\n", "2\\n"),
            r#"{"t":"meta","title":"t","updated_at":0,"subagents":[{"tool_use_id":"t1","name":"scout"}]}"#.to_owned(),
        ]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let histories = histories_of(&session, &[]);

        assert_eq!(histories[0].steps[0].via, Some("scout"));
    }

    #[test]
    fn render_shows_stats_and_diff_bodies() {
        let text = parse(&[
            call("c1", "edit"),
            diff_out("c1", "a.rs", "x\\ny\\n", "x\\nz\\nw\\n"),
        ]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let histories = histories_of(&session, &[]);

        let full = render(
            &session,
            &histories,
            HistoryOpts {
                context: CONTEXT,
                stat_only: false,
            },
        );
        assert!(
            full.contains("## a.rs (1 edits, 0 unattributed, 2 -> 3 lines)"),
            "{full}"
        );
        assert!(full.contains("[msg 0 edit +2 -1]"), "{full}");
        assert!(full.contains("-y\n+z\n+w"), "{full}");

        let stat = render(
            &session,
            &histories,
            HistoryOpts {
                context: CONTEXT,
                stat_only: true,
            },
        );
        assert!(
            stat.contains("[msg 0 edit +2 -1]") && !stat.contains("-y"),
            "{stat}"
        );
    }

    fn write_call(id: &str, content: &str, extra: &str) -> String {
        call_with_input(
            id,
            "write",
            &format!(r#"{{"path":"a.rs","content":"{content}"{extra}}}"#),
        )
    }

    #[test]
    fn a_write_replaces_the_file_without_opening_a_gap() {
        let text = parse(&[
            call("c1", "edit"),
            diff_out("c1", "a.rs", "x\\n", "y\\n"),
            write_call("w1", "fresh\\n", ""),
            result("w1", "ok", false),
            call("c2", "edit"),
            diff_out("c2", "a.rs", "fresh\\n", "fresh\\nmore\\n"),
        ]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let histories = histories_of(&session, &[]);

        let tools: Vec<_> = histories[0].steps.iter().map(|step| step.tool).collect();
        assert_eq!(tools, ["edit", "write", "edit"]);
        assert!(matches!(
            histories[0].steps[1].change,
            Change::Write {
                before: "y\n",
                after: "fresh\n"
            }
        ));
    }

    #[test]
    fn a_first_write_is_a_creation_from_nothing() {
        let text = parse(&[write_call("w1", "a\\nb\\n", ""), result("w1", "ok", false)]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let histories = histories_of(&session, &[]);

        assert!(matches!(
            histories[0].steps[0].change,
            Change::Write {
                before: "",
                after: "a\nb\n"
            }
        ));
    }

    #[test]
    fn failed_and_appending_writes_are_not_snapshots() {
        let text = parse(&[
            write_call("w1", "lost\\n", ""),
            result("w1", "denied", true),
            write_call("w2", "tail\\n", r#","append":true"#),
            result("w2", "ok", false),
        ]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();

        assert!(histories_of(&session, &[]).is_empty());
    }

    fn write_to(id: &str, path: &str, content: &str) -> [String; 2] {
        let input = format!(r#"{{"path":"{path}","content":"{content}"}}"#);
        [
            call_with_input(id, "write", &input),
            result(id, "ok", false),
        ]
    }

    fn paths_of(text: &str) -> Vec<String> {
        let session = Session::parse(text, ParseOpts { diffs: true }).unwrap();
        histories_of(&session, &[])
            .iter()
            .map(|history| history.path.to_owned())
            .collect()
    }

    #[test]
    fn a_relative_path_joins_the_absolute_one_that_continues_its_content() {
        let mut lines = write_to("w", "src/a.rs", "1\\n2\\n").to_vec();
        lines.push(call("e", "edit"));
        lines.push(diff_out("e", "/proj/src/a.rs", "1\\n2\\n", "1\\nX\\n2\\n"));
        let text = parse(&lines);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let histories = histories_of(&session, &[]);

        assert_eq!(histories.len(), 1);
        assert_eq!(histories[0].path, "/proj/src/a.rs");
        let tools: Vec<_> = histories[0].steps.iter().map(|step| step.tool).collect();
        assert_eq!(tools, ["write", "edit"]);
        assert!(matches!(
            histories[0].steps[0].change,
            Change::Write { before: "", .. }
        ));
    }

    #[test]
    fn paths_stay_separate_when_the_content_does_not_continue() {
        let mut lines = write_to("w", "src/a.rs", "1\\n").to_vec();
        lines.push(call("e", "edit"));
        lines.push(diff_out(
            "e",
            "/proj/src/a.rs",
            "unrelated\\n",
            "changed\\n",
        ));

        assert_eq!(paths_of(&parse(&lines)), ["src/a.rs", "/proj/src/a.rs"]);
    }

    #[test]
    fn paths_stay_separate_when_two_absolute_paths_could_match() {
        let mut lines = write_to("w", "src/a.rs", "1\\n").to_vec();
        for (id, root) in [("e1", "/one"), ("e2", "/two")] {
            lines.push(call(id, "edit"));
            lines.push(diff_out(id, &format!("{root}/src/a.rs"), "1\\n", "2\\n"));
        }

        assert_eq!(paths_of(&parse(&lines)).len(), 3);
    }
}
