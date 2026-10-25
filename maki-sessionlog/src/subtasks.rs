//! Subtask (subagent) views over one stored transcript: what ran, what it
//! said, what it changed, and where a pattern matches.

use std::collections::HashSet;
use std::fmt::Write;

use regex::Regex;
use serde::Serialize;
use thiserror::Error;

use crate::effects::{self, DiffView, EffectsOpts};
use crate::events::{self, Event, EventKind, Subagent};
use crate::history;
use crate::model::{Block, LogError, ParseOpts, Role, Session, Turn};
use crate::render::clip_chars;

const UNNAMED_SUBTASK: &str = "subagent";
const TASK_MAX_CHARS: usize = 500;
const NAME_MAX_CHARS: usize = 80;
const MESSAGE_MAX_CHARS: usize = 2000;
const MAX_LINES: usize = 200;
const MAX_BYTES: usize = 64 * 1024;
const DEFAULT_EXAMPLES: usize = 5;
const QUERY_SNIPPET_BEFORE: usize = 80;
const QUERY_SNIPPET_AFTER: usize = 140;
pub const DEFAULT_QUERY_LIMIT: usize = 20;
const ELLIPSIS: char = '…';

#[derive(Debug, Error)]
pub enum SubtaskError {
    #[error(transparent)]
    Log(#[from] LogError),
    #[error("session has no subtasks")]
    NoSubtasks,
    #[error("subtask {index} is out of range 0..{count}")]
    OutOfRange { index: usize, count: usize },
    #[error("subtask {index} has no recorded messages")]
    Empty { index: usize },
    #[error("invalid query pattern: {0}")]
    BadPattern(#[from] regex::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtaskFormat {
    Conversation,
    Outline,
    Summary,
}

#[derive(Debug, Clone, Copy)]
pub struct ViewOpts {
    pub format: SubtaskFormat,
    pub diff: Option<DiffView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubtaskInfo {
    pub id: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub messages: usize,
    pub files: usize,
    pub added: usize,
    pub removed: usize,
    pub task: Option<String>,
}

struct Entry<'a> {
    id: &'a str,
    name: Option<&'a str>,
    model: Option<&'a str>,
    turns: Option<&'a [Turn<'a>]>,
}

pub fn list(text: &str) -> Result<Vec<SubtaskInfo>, LogError> {
    let session = Session::parse(text, ParseOpts { diffs: true })?;
    let events = events::build(&session);
    Ok(entries(&session)
        .into_iter()
        .map(|entry| info(&entry, &events))
        .collect())
}

pub fn view(text: &str, index: usize, opts: ViewOpts) -> Result<String, SubtaskError> {
    let diffs = opts.diff.is_some() || opts.format != SubtaskFormat::Conversation;
    let session = Session::parse(text, ParseOpts { diffs })?;
    let entries = entries(&session);
    let entry = pick(&entries, index)?;
    let turns = entry.turns.ok_or(SubtaskError::Empty { index })?;
    if opts.format == SubtaskFormat::Conversation {
        return Ok(conversation(index, entry, turns));
    }

    let events = events::build(&session);
    let subagent = find_subagent(&events, entry.id).ok_or(SubtaskError::Empty { index })?;
    let report = effects::collect(&subagent.events);
    if report.call_count() == 0 {
        return Ok(conversation(index, entry, turns));
    }
    let header = format!("{}\n", header(index, entry, turns.len()));
    Ok(effects::render_titled(
        &header,
        &report,
        EffectsOpts {
            limit: DEFAULT_EXAMPLES,
            diff: opts.diff,
            transcript: opts.format == SubtaskFormat::Outline,
        },
    ))
}

/// Matches {pattern} against every text, tool call and tool result of one
/// subtask, or of all of them when {index} is absent.
pub fn query(
    text: &str,
    index: Option<usize>,
    pattern: &str,
    limit: usize,
) -> Result<String, SubtaskError> {
    let regex = Regex::new(pattern)?;
    let limit = limit.max(1);
    let session = Session::parse(text, ParseOpts::default())?;
    let entries = entries(&session);
    if entries.is_empty() {
        return Err(SubtaskError::NoSubtasks);
    }
    let selected: Vec<(usize, &Entry)> = match index {
        Some(index) => vec![(index, pick(&entries, index)?)],
        None => entries.iter().enumerate().collect(),
    };

    let mut out = String::new();
    let mut matched = 0;
    for (index, entry) in selected {
        let (hits, truncated) = hits(entry, &regex, limit);
        if hits.is_empty() {
            continue;
        }
        matched += 1;
        let _ = writeln!(
            out,
            "subtask {index}: {} | {} hit(s)",
            display_name(entry),
            hits.len()
        );
        for hit in &hits {
            let _ = writeln!(out, "  {hit}");
        }
        if truncated {
            let _ = writeln!(out, "  [{ELLIPSIS} more hits not shown]");
        }
    }
    if matched == 0 {
        return Ok(match index {
            Some(index) => format!("no matches in subtask {index}"),
            None => format!("no matches in {} subtasks", entries.len()),
        });
    }
    Ok(cap_bytes(out))
}

fn entries<'a>(session: &'a Session<'a>) -> Vec<Entry<'a>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if let Some(meta) = &session.meta {
        for item in &meta.subagents {
            let id: &'a str = &item.tool_use_id;
            if seen.insert(id) {
                out.push(Entry {
                    id,
                    name: Some(&item.name),
                    model: item.model.as_deref(),
                    turns: session.subagent_turns.get(id).map(Vec::as_slice),
                });
            }
        }
    }
    for id in &session.subagent_order {
        let id: &'a str = id;
        if seen.insert(id) {
            out.push(Entry {
                id,
                name: None,
                model: None,
                turns: session.subagent_turns.get(id).map(Vec::as_slice),
            });
        }
    }
    out
}

fn pick<'a>(entries: &'a [Entry<'a>], index: usize) -> Result<&'a Entry<'a>, SubtaskError> {
    if entries.is_empty() {
        return Err(SubtaskError::NoSubtasks);
    }
    entries.get(index).ok_or(SubtaskError::OutOfRange {
        index,
        count: entries.len(),
    })
}

fn info(entry: &Entry, events: &[Event]) -> SubtaskInfo {
    let turns = entry.turns.unwrap_or_default();
    let (files, added, removed) = find_subagent(events, entry.id)
        .map(file_totals)
        .unwrap_or_default();
    SubtaskInfo {
        id: entry.id.to_owned(),
        name: non_empty(entry.name).map(str::to_owned),
        model: entry.model.map(str::to_owned),
        messages: turns.len(),
        files,
        added,
        removed,
        task: first_user_text(turns)
            .map(|text| clip_chars(&one_line(&text), TASK_MAX_CHARS).into_owned()),
    }
}

/// Net moved lines per file the subtask touched, folded from its first
/// recorded state to its last.
fn file_totals(subagent: &Subagent) -> (usize, usize, usize) {
    let histories = history::collect(&subagent.events, &[]);
    let (mut added, mut removed) = (0, 0);
    for history in &histories {
        if let (Some(first), Some(last)) = (history.steps.first(), history.steps.last()) {
            let (plus, minus) = history::line_delta(first.change.sides().0, last.change.sides().1);
            added += plus;
            removed += minus;
        }
    }
    (histories.len(), added, removed)
}

fn find_subagent<'a>(events: &'a [Event<'a>], id: &str) -> Option<&'a Subagent<'a>> {
    for event in events {
        let subagent = match &event.kind {
            EventKind::Call(call) => call.subagent.as_ref(),
            EventKind::Detached(subagent) => Some(subagent),
            _ => None,
        };
        if let Some(subagent) = subagent {
            if subagent.id == id {
                return Some(subagent);
            }
            if let Some(found) = find_subagent(&subagent.events, id) {
                return Some(found);
            }
        }
    }
    None
}

fn conversation(index: usize, entry: &Entry, turns: &[Turn]) -> String {
    let mut messages: Vec<(Role, String)> = turns
        .iter()
        .filter_map(|turn| {
            let text = one_line(&text_of(&turn.content));
            (!text.is_empty()).then_some((turn.role, text))
        })
        .collect();
    let mut out = header(index, entry, turns.len());
    out.push('\n');

    let line_budget = MAX_LINES - 2;
    let mut note = None;
    if messages.len() > line_budget {
        let head_budget = line_budget - line_budget.div_ceil(3);
        let tail_budget = line_budget - head_budget;
        let skipped = messages.len() - line_budget;
        note = Some(format!(
            "[{ELLIPSIS} {skipped} message(s) skipped; the most recent ones follow below {ELLIPSIS}]"
        ));
        let head: Vec<_> = messages.drain(..head_budget).collect();
        let tail_start = messages.len() - tail_budget;
        let tail: Vec<_> = messages.drain(tail_start..).collect();
        messages = head;
        messages.extend(tail);
    }

    let overhead = 128 + note.as_ref().map_or(0, |note| note.len() + 1);
    let per_message =
        MESSAGE_MAX_CHARS.min(MAX_BYTES.saturating_sub(overhead) / messages.len().max(1));
    if let Some(note) = note {
        out.push_str(&note);
        out.push('\n');
    }
    for (role, text) in &messages {
        let _ = writeln!(
            out,
            "{}: {}",
            role_name(*role),
            clip_chars(text, per_message)
        );
    }
    out
}

fn header(index: usize, entry: &Entry, messages: usize) -> String {
    let mut out = format!("subtask {index}: {}", display_name(entry));
    if let Some(model) = entry.model {
        let _ = write!(out, " | {model}");
    }
    let _ = write!(out, " | {messages} messages");
    out
}

fn display_name(entry: &Entry) -> String {
    non_empty(entry.name)
        .map(str::to_owned)
        .or_else(|| {
            first_user_text(entry.turns.unwrap_or_default())
                .map(|text| clip_chars(&one_line(&text), NAME_MAX_CHARS).into_owned())
        })
        .unwrap_or_else(|| UNNAMED_SUBTASK.to_owned())
}

fn hits(entry: &Entry, regex: &Regex, limit: usize) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    for (message, turn) in entry.turns.unwrap_or_default().iter().enumerate() {
        for block in &turn.content {
            let haystack = match block {
                Block::Text { text } => text.to_string(),
                Block::ToolResult { content, .. } => content.to_string(),
                Block::ToolUse { name, input, .. } => format!("{name} {input}"),
                _ => continue,
            };
            let Some(found) = regex.find(&haystack) else {
                continue;
            };
            if out.len() == limit {
                return (out, true);
            }
            out.push(snippet(
                message,
                turn.role,
                &haystack,
                found.start(),
                found.end(),
            ));
        }
    }
    (out, false)
}

fn snippet(message: usize, role: Role, haystack: &str, start: usize, end: usize) -> String {
    let before: String = haystack[..start]
        .chars()
        .rev()
        .take(QUERY_SNIPPET_BEFORE)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = haystack[end..].chars().take(QUERY_SNIPPET_AFTER).collect();
    let mut text = one_line(&format!("{before}{}{after}", &haystack[start..end]));
    if start > 0 && haystack[..start].chars().count() > QUERY_SNIPPET_BEFORE {
        text.insert(0, ELLIPSIS);
    }
    if haystack[end..].chars().count() > QUERY_SNIPPET_AFTER {
        text.push(ELLIPSIS);
    }
    format!("#{message} {}: {text}", role_name(role))
}

fn cap_bytes(mut text: String) -> String {
    if text.len() <= MAX_BYTES {
        return text;
    }
    let mut cut = MAX_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text.push_str("\n[output truncated]");
    text
}

fn text_of(blocks: &[Block]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(&**text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn first_user_text(turns: &[Turn]) -> Option<String> {
    turns
        .iter()
        .filter(|turn| turn.role == Role::User)
        .find_map(|turn| {
            let text = text_of(&turn.content);
            (!text.trim().is_empty()).then_some(text)
        })
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn non_empty(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.is_empty())
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{call, diff_out, log, reply};

    const META: &str = r#"{"t":"meta","title":"t","updated_at":0,"subagents":[{"tool_use_id":"t1","name":"scout","model":"fast"}]}"#;

    fn sub(id: &str, role: &str, content: &str) -> String {
        format!(r#"{{"t":"sub_msg","sub":"{id}","d":{{"role":"{role}","content":[{content}]}}}}"#)
    }

    fn text(value: &str) -> String {
        format!(r#"{{"type":"text","text":"{value}"}}"#)
    }

    fn tool_use(id: &str, name: &str, input: &str) -> String {
        format!(r#"{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}"#)
    }

    fn tool_result(id: &str, content: &str) -> String {
        format!(r#"{{"type":"tool_result","tool_use_id":"{id}","content":"{content}"}}"#)
    }

    fn sub_log(lines: &[String]) -> String {
        let mut all = vec![call("t1", "subtask")];
        all.extend(lines.iter().cloned());
        all.push(META.to_owned());
        log(&all)
    }

    #[test]
    fn list_reports_name_model_messages_and_line_counts() {
        let text = sub_log(&[
            sub("t1", "user", &text("check the logs")),
            sub(
                "t1",
                "assistant",
                &tool_use("s1", "edit", r#"{"path":"a.rs"}"#),
            ),
            sub("t1", "user", &tool_result("s1", "ok")),
            diff_out("s1", "a.rs", "x\\n", "x\\ny\\n"),
        ]);

        let infos = list(&text).unwrap();

        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, "t1");
        assert_eq!(infos[0].name.as_deref(), Some("scout"));
        assert_eq!(infos[0].model.as_deref(), Some("fast"));
        assert_eq!(infos[0].messages, 3);
        assert_eq!(
            (infos[0].files, infos[0].added, infos[0].removed),
            (1, 1, 0)
        );
        assert_eq!(infos[0].task.as_deref(), Some("check the logs"));
    }

    #[test]
    fn list_keeps_meta_order_and_appends_untracked_transcripts() {
        let meta = r#"{"t":"meta","title":"t","updated_at":0,"subagents":[{"tool_use_id":"t2","name":"second"},{"tool_use_id":"t1","name":"first"}]}"#;
        let text = log(&[
            call("t2", "subtask"),
            sub("t1", "user", &text("one")),
            sub("t2", "user", &text("two")),
            meta.to_owned(),
        ]);

        let infos = list(&text).unwrap();

        let names: Vec<_> = infos.iter().map(|info| info.name.as_deref()).collect();
        assert_eq!(names, [Some("second"), Some("first")]);
    }

    #[test]
    fn conversation_keeps_spoken_text_and_drops_tool_traffic() {
        let text = sub_log(&[
            sub("t1", "user", &text("monitor\\nthe run")),
            sub(
                "t1",
                "assistant",
                &format!(
                    "{},{}",
                    text("checking"),
                    tool_use("s1", "bash", r#"{"command":"ls"}"#)
                ),
            ),
        ]);

        let rendered = view(
            &text,
            0,
            ViewOpts {
                format: SubtaskFormat::Conversation,
                diff: None,
            },
        )
        .unwrap();

        assert!(
            rendered.starts_with("subtask 0: scout | fast | 2 messages\n"),
            "{rendered}"
        );
        assert!(rendered.contains("\nuser: monitor the run\n"), "{rendered}");
        assert!(rendered.contains("\nassistant: checking\n"), "{rendered}");
        assert!(!rendered.contains("bash"), "{rendered}");
    }

    #[test]
    fn outline_names_the_subtask_and_its_files() {
        let text = sub_log(&[
            sub("t1", "user", &text("go")),
            sub(
                "t1",
                "assistant",
                &format!(
                    "{},{}",
                    text("editing"),
                    tool_use("s1", "edit", r#"{"path":"a.rs"}"#)
                ),
            ),
            sub("t1", "user", &tool_result("s1", "ok")),
            diff_out("s1", "a.rs", "x\\n", "x\\ny\\n"),
        ]);

        let rendered = view(
            &text,
            0,
            ViewOpts {
                format: SubtaskFormat::Outline,
                diff: None,
            },
        )
        .unwrap();

        assert!(
            rendered.starts_with("subtask 0: scout | fast | 3 messages\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains("## files changed (1, +1 -0)"),
            "{rendered}"
        );
        assert!(rendered.contains("a.rs"), "{rendered}");
        assert!(
            rendered.contains("## model output and commands"),
            "{rendered}"
        );
        assert!(rendered.contains("editing"), "{rendered}");
    }

    #[test]
    fn outline_can_show_the_subtask_diff() {
        let text = sub_log(&[
            sub(
                "t1",
                "assistant",
                &tool_use("s1", "edit", r#"{"path":"a.rs"}"#),
            ),
            sub("t1", "user", &tool_result("s1", "ok")),
            diff_out("s1", "a.rs", "x\\n", "x\\ny\\n"),
        ]);

        let rendered = view(
            &text,
            0,
            ViewOpts {
                format: SubtaskFormat::Outline,
                diff: Some(DiffView {
                    context: 1,
                    max_lines: None,
                }),
            },
        )
        .unwrap();

        assert!(rendered.contains("## git diff"), "{rendered}");
        assert!(rendered.contains("+++ a.rs"), "{rendered}");
    }

    #[test]
    fn summary_omits_the_transcript_section() {
        let text = sub_log(&[
            sub("t1", "assistant", &text("thinking out loud")),
            sub(
                "t1",
                "assistant",
                &tool_use("s1", "bash", r#"{"command":"cargo build"}"#),
            ),
            sub("t1", "user", &tool_result("s1", "a\\nb")),
        ]);

        let rendered = view(
            &text,
            0,
            ViewOpts {
                format: SubtaskFormat::Summary,
                diff: None,
            },
        )
        .unwrap();

        assert!(
            !rendered.contains("model output and commands"),
            "{rendered}"
        );
        assert!(rendered.contains("## commands"), "{rendered}");
        assert!(rendered.contains("cargo"), "{rendered}");
    }

    #[test]
    fn query_matches_text_and_tool_results() {
        let text = sub_log(&[
            sub("t1", "user", &text("watch the zfs run")),
            sub(
                "t1",
                "assistant",
                &tool_use("s1", "bash", r#"{"command":"zfs list"}"#),
            ),
            sub("t1", "user", &tool_result("s1", "zfs is fine")),
        ]);

        let rendered = query(&text, Some(0), "zfs", DEFAULT_QUERY_LIMIT).unwrap();

        assert!(
            rendered.starts_with("subtask 0: scout | 3 hit(s)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("#0 user: watch the zfs run"),
            "{rendered}"
        );
        assert!(rendered.contains("zfs list"), "{rendered}");
        assert!(rendered.contains("zfs is fine"), "{rendered}");
    }

    #[test]
    fn query_spans_all_subtasks_when_no_index_is_given() {
        let meta = r#"{"t":"meta","title":"t","updated_at":0,"subagents":[{"tool_use_id":"t1","name":"scout"},{"tool_use_id":"t2","name":"builder"}]}"#;
        let text = log(&[
            call("t1", "subtask"),
            call("t2", "subtask"),
            sub("t1", "user", &text("needle one")),
            sub("t2", "user", &text("needle two")),
            meta.to_owned(),
        ]);

        let rendered = query(&text, None, "needle", DEFAULT_QUERY_LIMIT).unwrap();

        assert!(
            rendered.contains("subtask 0: scout | 1 hit(s)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("subtask 1: builder | 1 hit(s)"),
            "{rendered}"
        );
    }

    #[test]
    fn query_reports_clean_misses() {
        let text = sub_log(&[sub("t1", "user", &text("hello"))]);

        assert_eq!(
            query(&text, Some(0), "absent", DEFAULT_QUERY_LIMIT).unwrap(),
            "no matches in subtask 0"
        );
        assert_eq!(
            query(&text, None, "absent", DEFAULT_QUERY_LIMIT).unwrap(),
            "no matches in 1 subtasks"
        );
    }

    #[test]
    fn out_of_range_names_the_count() {
        let text = sub_log(&[sub("t1", "user", &text("hello"))]);

        let err = view(
            &text,
            3,
            ViewOpts {
                format: SubtaskFormat::Conversation,
                diff: None,
            },
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "subtask 3 is out of range 0..1");
    }

    #[test]
    fn missing_transcript_is_reported() {
        let text = log(&[META.to_owned()]);

        let err = view(
            &text,
            0,
            ViewOpts {
                format: SubtaskFormat::Conversation,
                diff: None,
            },
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "subtask 0 has no recorded messages");
    }

    #[test]
    fn no_subtasks_is_reported() {
        let text = log(&[reply("done")]);

        assert!(list(&text).unwrap().is_empty());
        assert!(matches!(
            query(&text, None, "x", DEFAULT_QUERY_LIMIT),
            Err(SubtaskError::NoSubtasks)
        ));
    }

    #[test]
    fn missing_header_is_a_log_error() {
        assert!(matches!(list(""), Err(LogError::MissingHeader)));
    }

    #[test]
    fn invalid_pattern_is_reported() {
        let text = sub_log(&[sub("t1", "user", &text("hello"))]);

        let err = query(&text, None, "(", DEFAULT_QUERY_LIMIT).unwrap_err();

        assert!(
            err.to_string().starts_with("invalid query pattern"),
            "{err}"
        );
    }
}
