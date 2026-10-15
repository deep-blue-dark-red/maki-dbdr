//! What the supervisor reads of a worker. Its input is the bill, so both
//! views stay compact.
//!
//! The transcript keeps what the worker wrote, ran and got wrong, and shrinks
//! what it merely looked at to a line. The action list is narrower still:
//! each call that changes something, the one-line `why` the worker gave with
//! it, and how it came out, with lookups and reasoning left out.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;

use maki_config::FILE_WRITE_TOOLS;
use maki_providers::{ContentBlock, Message, Role};
use serde_json::{Value, json};

use super::TOOL_NAME;
use crate::mcp::TOOL_SEARCH_TOOL_NAME;

pub(super) const WHY_FIELD: &str = "why";
const WHY_DESCRIPTION: &str = "One line: why this action, in terms of your plan";
/// Tools that only look or talk. They are not actions and need no reason.
const PASSIVE_TOOLS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "outline",
    "list",
    "ast_grep",
    "tool_help",
    "view_image",
    "webfetch",
    "websearch",
    "todo_write",
    "question",
    "skill",
    TOOL_SEARCH_TOOL_NAME,
    TOOL_NAME,
];
const INPUT_CAP: usize = 300;
const WHY_CAP: usize = 200;
const ERROR_CAP: usize = 400;
const TEXT_CAP: usize = 2_000;
/// Enough to catch a misread task, far short of the whole chain.
const THINKING_CAP: usize = 300;
const TRANSCRIPT_INPUT_CAP: usize = 400;
/// Diffs are where correctness lives, so writes get room.
const WRITE_INPUT_CAP: usize = 4_000;
const TRANSCRIPT_ERROR_CAP: usize = 600;
const EDGE_LINES: usize = 6;
const LINE_CAP: usize = 200;
/// Results only sized, since their call already says what was looked at.
const LOOKUP_TOOLS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "outline",
    "list",
    "ast_grep",
    "tool_help",
    "view_image",
];
const UNKNOWN_TOOL: &str = "?";
const NO_WHY: &str = "(none given)";
const NO_RESULT: &str = "no result";
const ELLIPSIS: &str = "…";

fn is_action(name: &str) -> bool {
    !PASSIVE_TOOLS.contains(&name)
}

/// Makes a `why` required on every action tool, so each action arrives with
/// its explanation.
pub(super) fn require_why(tools: &mut Value) {
    let Value::Array(definitions) = tools else {
        return;
    };
    for definition in definitions {
        if !definition["name"].as_str().is_some_and(is_action) {
            continue;
        }
        let Some(schema) = definition
            .get_mut("input_schema")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        if let Some(properties) = schema
            .entry("properties")
            .or_insert_with(|| json!({}))
            .as_object_mut()
        {
            properties.insert(
                WHY_FIELD.to_owned(),
                json!({ "type": "string", "description": WHY_DESCRIPTION }),
            );
        }
        if let Some(required) = schema
            .entry("required")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            && !required.iter().any(|name| name == WHY_FIELD)
        {
            required.push(json!(WHY_FIELD));
        }
    }
}

/// The tool never sees the `why`, which is for the supervisor alone.
pub(crate) fn strip_why(input: &mut Value) {
    if let Some(fields) = input.as_object_mut() {
        fields.remove(WHY_FIELD);
    }
}

/// Appends the transcript of `messages`. Messages to the supervisor are left
/// out, since they reach it on their own.
pub(super) fn render_transcript(messages: &[Message], out: &mut String) {
    let mut names: HashMap<&str, &str> = HashMap::new();
    for message in messages {
        let speaker = speaker(message);
        for block in &message.content {
            match block {
                ContentBlock::Text { text } if !text.trim().is_empty() => {
                    let _ = writeln!(out, "{speaker}: {}", clip(text.trim(), TEXT_CAP));
                }
                ContentBlock::Thinking { thinking, .. } if !thinking.trim().is_empty() => {
                    let _ = writeln!(out, "THINKING: {}", clip(thinking.trim(), THINKING_CAP));
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    names.insert(id, name);
                    if name != TOOL_NAME {
                        let _ = writeln!(out, "CALL {name} {}", transcript_input(name, input));
                    }
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let name = names
                        .get(tool_use_id.as_str())
                        .copied()
                        .unwrap_or(UNKNOWN_TOOL);
                    if name != TOOL_NAME {
                        transcript_result(name, content, *is_error, out);
                    }
                }
                ContentBlock::Image { .. } => {
                    let _ = writeln!(out, "{speaker}: [image]");
                }
                _ => {}
            }
        }
    }
}

fn speaker(message: &Message) -> &'static str {
    match message.role {
        Role::Assistant => "ASSISTANT",
        _ if message.is_observation() || message.display_text.as_deref() == Some("") => "HOST",
        _ => "USER",
    }
}

fn transcript_input(name: &str, input: &Value) -> String {
    let cap = if FILE_WRITE_TOOLS.contains(&name) {
        WRITE_INPUT_CAP
    } else {
        TRANSCRIPT_INPUT_CAP
    };
    clip(&input.to_string(), cap).into_owned()
}

fn transcript_result(name: &str, content: &str, is_error: bool, out: &mut String) {
    let lines = content.lines().count();
    if is_error {
        let _ = writeln!(
            out,
            "RESULT {name} ERROR: {}",
            clip(content.trim(), TRANSCRIPT_ERROR_CAP)
        );
        return;
    }
    if LOOKUP_TOOLS.contains(&name) || lines == 0 {
        let _ = writeln!(out, "RESULT {name} ok, {lines} lines");
        return;
    }
    let _ = writeln!(out, "RESULT {name} ok, {lines} lines:");
    let mut push = |line: &str| {
        let _ = writeln!(out, "  {}", clip(line, LINE_CAP));
    };
    if lines <= EDGE_LINES * 2 {
        content.lines().for_each(&mut push);
        return;
    }
    content.lines().take(EDGE_LINES).for_each(&mut push);
    push(&format!("[{} lines]", lines - EDGE_LINES * 2));
    content.lines().skip(lines - EDGE_LINES).for_each(push);
}

/// Appends the actions in `messages`, all from `turn`, with their outcomes,
/// and with the `why` given for each when `why` is asked for.
pub(super) fn render_actions(messages: &[Message], turn: u32, why: bool, out: &mut String) {
    let results: HashMap<&str, (&str, bool)> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id.as_str(), (content.as_str(), *is_error))),
            _ => None,
        })
        .collect();
    for (id, name, input) in messages.iter().flat_map(Message::tool_uses) {
        if !is_action(name) {
            continue;
        }
        let reason = input
            .get(WHY_FIELD)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|why| !why.is_empty())
            .unwrap_or(NO_WHY);
        let mut input = input.clone();
        strip_why(&mut input);
        let outcome = match results.get(id) {
            Some((_, false)) => Cow::Borrowed("ok"),
            Some((content, true)) => {
                format!("ERROR {}", one_line(&clip(content.trim(), ERROR_CAP))).into()
            }
            None => Cow::Borrowed(NO_RESULT),
        };
        let _ = writeln!(
            out,
            "t{turn} {name} {}",
            clip(&input.to_string(), INPUT_CAP)
        );
        if why {
            let _ = writeln!(out, "  why: {}", one_line(&clip(reason, WHY_CAP)));
        }
        let _ = writeln!(out, "  -> {outcome}");
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn clip(text: &str, cap: usize) -> Cow<'_, str> {
    if text.len() <= cap {
        return text.into();
    }
    format!("{}{ELLIPSIS}", &text[..text.floor_char_boundary(cap)]).into()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use test_case::test_case;

    use super::*;

    const WHY: &str = "step 2 of the plan: fix the off-by-one";

    fn call(id: &str, name: &str, input: Value) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
                thought_signature: None,
            }],
            ..Default::default()
        }
    }

    fn result(id: &str, content: &str, is_error: bool) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: content.into(),
                is_error,
            }],
            display_text: Some(String::new()),
            ..Default::default()
        }
    }

    fn rendered(messages: &[Message]) -> String {
        let mut out = String::new();
        render_actions(messages, 3, true, &mut out);
        out
    }

    fn transcript(messages: &[Message]) -> String {
        let mut out = String::new();
        render_transcript(messages, &mut out);
        out
    }

    fn numbered(count: usize) -> String {
        (1..=count).map(|n| format!("line{n}\n")).collect()
    }

    #[test]
    fn actions_without_why_leave_the_line_out() {
        let mut out = String::new();
        render_actions(
            &[
                call("t1", "bash", json!({"command": "ls"})),
                result("t1", "a", false),
            ],
            3,
            false,
            &mut out,
        );
        assert_eq!(out, "t3 bash {\"command\":\"ls\"}\n  -> ok\n");
    }

    #[test]
    fn lookup_results_shrink_to_a_size() {
        let out = transcript(&[
            call("t1", "read", json!({"path": "src/lib.rs"})),
            result("t1", &numbered(240), false),
        ]);
        assert_eq!(
            out,
            "CALL read {\"path\":\"src/lib.rs\"}\nRESULT read ok, 240 lines\n"
        );
    }

    #[test]
    fn long_output_keeps_its_edges() {
        let out = transcript(&[
            call("t1", "bash", json!({"command": "cargo test"})),
            result("t1", &numbered(40), false),
        ]);
        assert!(out.contains("  line1\n"));
        assert!(out.contains("  line40\n"));
        assert!(out.contains(&format!("[{} lines]", 40 - EDGE_LINES * 2)));
        assert!(!out.contains("  line20\n"));
    }

    #[test]
    fn errors_render_whatever_the_tool() {
        let out = transcript(&[
            call("t1", "read", json!({"path": "missing"})),
            result("t1", "file not found", true),
        ]);
        assert!(out.ends_with("RESULT read ERROR: file not found\n"));
    }

    #[test_case("edit", WRITE_INPUT_CAP ; "write_tool_gets_room")]
    #[test_case("bash", TRANSCRIPT_INPUT_CAP ; "other_tool_is_clipped")]
    fn input_cap_follows_the_tool(name: &str, cap: usize) {
        let input = json!({"body": "x".repeat(WRITE_INPUT_CAP * 2)});
        let out = transcript_input(name, &input);
        assert_eq!(out.len(), cap + ELLIPSIS.len());
    }

    #[test]
    fn messages_to_the_supervisor_stay_out_of_the_transcript() {
        let out = transcript(&[
            call("t1", TOOL_NAME, json!({"message": "plan"})),
            result("t1", "Sent.", false),
        ]);
        assert!(out.is_empty());
    }

    #[test]
    fn thinking_is_clipped_and_text_kept() {
        let message = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: "t".repeat(THINKING_CAP * 3),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "done".into(),
                },
            ],
            ..Default::default()
        };
        let out = transcript(&[message]);
        let thinking = out.lines().next().unwrap();
        assert_eq!(
            thinking.len(),
            "THINKING: ".len() + THINKING_CAP + ELLIPSIS.len()
        );
        assert!(out.ends_with("ASSISTANT: done\n"));
    }

    #[test_case(Message::user("fix it".into()), "USER: fix it\n" ; "user_speaks")]
    #[test_case(Message::observation("build ok".into()), "HOST: build ok\n" ; "observation_is_host")]
    #[test_case(Message::synthetic("keep going".into()), "HOST: keep going\n" ; "synthetic_is_host")]
    fn speaker_labels(message: Message, expected: &str) {
        assert_eq!(transcript(&[message]), expected);
    }

    #[test]
    fn action_shows_its_why_and_outcome_without_the_why_in_the_input() {
        let out = rendered(&[
            call("t1", "edit", json!({"path": "src/lib.rs", WHY_FIELD: WHY})),
            result("t1", "applied", false),
        ]);
        assert_eq!(
            out,
            format!("t3 edit {{\"path\":\"src/lib.rs\"}}\n  why: {WHY}\n  -> ok\n")
        );
    }

    #[test]
    fn failed_action_keeps_its_error_on_one_line() {
        let out = rendered(&[
            call(
                "t1",
                "bash",
                json!({"command": "cargo test", WHY_FIELD: WHY}),
            ),
            result("t1", "test parser ... FAILED\n\nexit 101", true),
        ]);
        assert!(out.ends_with("  -> ERROR test parser ... FAILED exit 101\n"));
    }

    #[test_case(json!({"command": "ls"}) ; "missing")]
    #[test_case(json!({"command": "ls", WHY_FIELD: "  "}) ; "blank")]
    fn action_without_a_why_says_so(input: Value) {
        let out = rendered(&[call("t1", "bash", input)]);
        assert!(out.contains(&format!("  why: {NO_WHY}\n")));
        assert!(out.ends_with(&format!("  -> {NO_RESULT}\n")));
    }

    #[test_case("read" ; "lookup")]
    #[test_case(TOOL_NAME ; "supervisor_tool")]
    fn passive_calls_are_not_actions(name: &str) {
        let out = rendered(&[call("t1", name, json!({})), result("t1", "body", false)]);
        assert!(out.is_empty());
    }

    #[test]
    fn why_is_required_on_actions_only() {
        let mut tools = json!([
            {"name": "bash", "input_schema": {"type": "object", "properties": {"command": {}}, "required": ["command"]}},
            {"name": "mcp__db__query", "input_schema": {"type": "object"}},
            {"name": "read", "input_schema": {"type": "object", "properties": {}}},
        ]);
        require_why(&mut tools);
        require_why(&mut tools);
        assert_eq!(
            tools[0]["input_schema"]["required"],
            json!(["command", WHY_FIELD])
        );
        assert!(tools[1]["input_schema"]["properties"][WHY_FIELD].is_object());
        assert_eq!(tools[1]["input_schema"]["required"], json!([WHY_FIELD]));
        assert!(tools[2]["input_schema"]["properties"][WHY_FIELD].is_null());
    }

    #[test]
    fn strip_leaves_the_rest_of_the_input() {
        let mut input = json!({"command": "ls", WHY_FIELD: WHY});
        strip_why(&mut input);
        assert_eq!(input, json!({"command": "ls"}));
    }
}
