//! What the supervisor reads of the worker's transcript. Its input is the bill,
//! so everything the worker merely looked at shrinks to a line, while what it
//! wrote and what failed stays legible.

use std::collections::HashMap;
use std::fmt::Write;

use maki_config::FILE_WRITE_TOOLS;
use maki_providers::{ContentBlock, Message, Role};
use serde_json::Value;

use super::{TOOL_FIELD, TOOL_NAME};

const TEXT_CAP: usize = 2_000;
/// Enough to catch a misread task, far short of the whole chain.
const THINKING_CAP: usize = 300;
const INPUT_CAP: usize = 400;
/// Diffs are where correctness lives, so writes get room.
const WRITE_INPUT_CAP: usize = 4_000;
const ERROR_CAP: usize = 600;
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
const ELLIPSIS: &str = "…";

pub(super) fn render(messages: &[Message], out: &mut String) {
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
                    match input.get(TOOL_FIELD).and_then(Value::as_str) {
                        Some(message) if name == TOOL_NAME => {
                            let _ =
                                writeln!(out, "TO SUPERVISOR: {}", clip(message.trim(), TEXT_CAP));
                        }
                        _ => {
                            let _ = writeln!(out, "CALL {name} {}", render_input(name, input));
                        }
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
                        render_result(name, content, *is_error, out);
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

fn render_input(name: &str, input: &Value) -> String {
    let cap = if FILE_WRITE_TOOLS.contains(&name) {
        WRITE_INPUT_CAP
    } else {
        INPUT_CAP
    };
    clip(&input.to_string(), cap).into_owned()
}

fn render_result(name: &str, content: &str, is_error: bool, out: &mut String) {
    let lines = content.lines().count();
    if is_error {
        let _ = writeln!(
            out,
            "RESULT {name} ERROR: {}",
            clip(content.trim(), ERROR_CAP)
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

fn clip(text: &str, cap: usize) -> std::borrow::Cow<'_, str> {
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
        render(messages, &mut out);
        out
    }

    fn numbered(count: usize) -> String {
        (1..=count).map(|n| format!("line{n}\n")).collect()
    }

    #[test]
    fn lookup_results_shrink_to_a_size() {
        let out = rendered(&[
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
        let out = rendered(&[
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
        let out = rendered(&[
            call("t1", "read", json!({"path": "missing"})),
            result("t1", "file not found", true),
        ]);
        assert!(out.ends_with("RESULT read ERROR: file not found\n"));
    }

    #[test_case("edit", WRITE_INPUT_CAP ; "write_tool_gets_room")]
    #[test_case("bash", INPUT_CAP ; "other_tool_is_clipped")]
    fn input_cap_follows_the_tool(name: &str, cap: usize) {
        let input = json!({"body": "x".repeat(WRITE_INPUT_CAP * 2)});
        let out = render_input(name, &input);
        assert_eq!(out.len(), cap + ELLIPSIS.len());
    }

    #[test]
    fn worker_message_reads_as_dialogue() {
        let out = rendered(&[
            call(
                "t1",
                TOOL_NAME,
                json!({TOOL_FIELD: "plan: fix parser, then tests"}),
            ),
            result("t1", "Sent.", false),
        ]);
        assert_eq!(out, "TO SUPERVISOR: plan: fix parser, then tests\n");
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
        let out = rendered(&[message]);
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
        assert_eq!(rendered(&[message]), expected);
    }
}
