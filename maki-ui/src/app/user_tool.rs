use std::sync::Arc;
use std::time::Duration;

use maki_agent::tools::registry::plain_tool_args;
use maki_agent::tools::{Deadline, ToolRegistry, cli_tool_ctx};
use maki_agent::{ToolDoneEvent, ToolInput, ToolOutput, ToolStartEvent};
use maki_providers::Message;
use serde_json::{Map, Value, json};

use super::App;

const USER_TOOL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserToolPrefix {
    pub name: String,
    pub args: String,
    pub visible: bool,
}

/// Parses `$tool args` (`$$tool` hides the call and its output from the
/// agent). The name must look like a tool name: anything else falls back to
/// being a chat message, so prose like `$100` is never hijacked.
pub(crate) fn parse_user_tool_prefix(text: &str) -> Option<UserToolPrefix> {
    let rest = text.strip_prefix('$')?;
    let (visible, name_start) = if rest.starts_with('$') {
        (false, 2)
    } else {
        (true, 1)
    };
    let rest = &text[name_start..];
    if rest.starts_with(' ') {
        return None;
    }
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (rest, ""),
    };
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':'));
    if !valid {
        return None;
    }
    Some(UserToolPrefix {
        name: name.to_owned(),
        args: args.to_owned(),
        visible,
    })
}

/// Builds tool input from the text after `$tool`.
///
/// Three forms, one per call:
/// - empty: `{}`
/// - a JSON object: passed through as-is
/// - shorthand tokens: `key=value` pairs (value parsed as JSON when valid,
///   else a string) plus bare tokens filling the schema's string properties,
///   required ones first.
pub(crate) fn build_tool_input(args: &str, schema: &Value) -> Result<Value, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    if trimmed.starts_with('{') {
        let value: Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("invalid JSON input: {e}"))?;
        if !value.is_object() {
            return Err("JSON input must be an object".into());
        }
        return Ok(value);
    }
    let mut map = Map::new();
    let mut positional: Vec<String> = Vec::new();
    for token in trimmed.split_whitespace() {
        match token.split_once('=') {
            Some((key, raw)) if is_param_name(key) => {
                map.insert(key.to_owned(), parse_value(raw));
            }
            _ => positional.push(unquote(token)),
        }
    }
    if !positional.is_empty() {
        let slots: Vec<String> = string_properties(schema)
            .into_iter()
            .filter(|key| !map.contains_key(key))
            .collect();
        let mut next = slots.iter();
        for token in positional {
            match next.next() {
                Some(key) => {
                    map.insert(key.clone(), Value::String(token));
                }
                None => return Err(format!("no unused string parameter for {token:?}")),
            }
        }
    }
    Ok(Value::Object(map))
}

/// A `key=value` token's key half; shared with the tool palette, which
/// excludes keys already typed from its candidates.
pub(crate) fn is_param_name(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

fn parse_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(unquote(raw)))
}

fn unquote(token: &str) -> String {
    let bytes = token.as_bytes();
    if bytes.len() >= 2 && bytes.first() == bytes.last() && bytes[0] == b'"' {
        token[1..token.len() - 1].to_owned()
    } else {
        token.to_owned()
    }
}

/// One `properties` entry of a tool's input schema, in the order completion
/// and positional fill should offer them: `required` entries first (in the
/// schema's own declaration order), then the rest.
pub(crate) struct SchemaProp {
    pub name: String,
    pub type_name: String,
    pub description: String,
    pub variants: Vec<String>,
    /// A bare positional token can fill it: string-typed or untyped.
    pub accepts_positional: bool,
}

pub(crate) fn schema_props(schema: &Value) -> Vec<SchemaProp> {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut out: Vec<SchemaProp> = props
        .iter()
        .map(|(name, prop)| {
            let type_value = prop.get("type");
            let accepts_positional = match type_value {
                None => true,
                Some(Value::String(t)) => t == "string",
                Some(Value::Array(types)) => types
                    .iter()
                    .any(|t| matches!(t, Value::String(s) if s == "string")),
                Some(_) => false,
            };
            SchemaProp {
                name: name.clone(),
                type_name: type_value
                    .and_then(Value::as_str)
                    .unwrap_or("any")
                    .to_owned(),
                description: prop
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                variants: prop
                    .get("enum")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                accepts_positional,
            }
        })
        .collect();
    let rank = |prop: &SchemaProp| {
        required
            .iter()
            .position(|r| *r == prop.name.as_str())
            .unwrap_or(usize::MAX)
    };
    out.sort_by_key(rank);
    out
}

/// Schema property names that accept a bare string, `required` entries first
/// so `$read file.rs` lands on `path` rather than an incidental default.
fn string_properties(schema: &Value) -> Vec<String> {
    schema_props(schema)
        .into_iter()
        .filter(|prop| prop.accepts_positional)
        .map(|prop| prop.name)
        .collect()
}

pub(crate) enum UserToolEvent {
    Start {
        id: String,
        name: String,
        summary: String,
        input: String,
    },
    Done {
        id: String,
        name: String,
        display: String,
        output: String,
        is_error: bool,
        visible: bool,
    },
}

pub(crate) fn spawn_user_tool(
    name: String,
    input: Value,
    id: String,
    visible: bool,
    tx: flume::Sender<UserToolEvent>,
) {
    smol::spawn(async move {
        let shorthand = input_shorthand(&input);
        let _ = tx.send(UserToolEvent::Start {
            id: id.clone(),
            summary: plain_tool_args(&input),
            input: serde_json::to_string_pretty(&input).unwrap_or_default(),
            name: name.clone(),
        });
        let (output, is_error) = match run_tool(&name, &input).await {
            Ok(output) => (output, false),
            Err(err) => (err, true),
        };
        let display = format!("$ {name} {shorthand}");
        let _ = tx.send(UserToolEvent::Done {
            id,
            display: display.trim_end().to_owned(),
            name,
            output,
            is_error,
            visible,
        });
    })
    .detach();
}

/// The compact one-line form of the call for the shared message, mirroring
/// what the model sees in tool headers.
fn input_shorthand(input: &Value) -> String {
    match input {
        Value::Object(map) if map.is_empty() => String::new(),
        _ => plain_tool_args(input),
    }
}

async fn run_tool(name: &str, input: &Value) -> Result<String, String> {
    let entry = ToolRegistry::global()
        .get(name)
        .ok_or_else(|| format!("unknown tool: {name}"))?;
    let invocation = entry.tool.parse(input).map_err(|e| e.to_string())?;
    let mut ctx = cli_tool_ctx();
    ctx.deadline = Deadline::after(USER_TOOL_TIMEOUT);
    let result = invocation.execute(&ctx).await;
    match result.output {
        Ok(output) => Ok(output.as_text()),
        Err(err) => Err(err),
    }
}

impl App {
    pub(crate) fn handle_user_tool_event(&mut self, event: UserToolEvent) {
        match event {
            UserToolEvent::Start {
                id,
                name,
                summary,
                input,
            } => {
                self.main_chat().shell_tool_start(ToolStartEvent {
                    id,
                    tool: name.into(),
                    summary,
                    annotation: None,
                    input: Some(ToolInput::Code {
                        language: "json".into(),
                        code: input,
                    }),
                    raw_input: None,
                    output: None,
                    render_header: None,
                });
            }
            UserToolEvent::Done {
                id,
                name,
                display,
                output,
                is_error,
                visible,
            } => {
                if visible {
                    let label = if is_error { "Error" } else { "Output" };
                    let msg = Message::user(format!("I ran: {display}\n\n{label}:\n{output}"));
                    self.shell.push_result(msg);
                }
                self.main_chat().shell_tool_done(ToolDoneEvent {
                    call: None,
                    id: id.clone(),
                    tool: name.into(),
                    output: Arc::new(ToolOutput::Plain(output.into())),
                    is_error,
                    annotation: None,
                    written_path: None,
                });
                self.shell.release_id(&id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("$read" ; "bare_name")]
    #[test_case("$read src/main.rs" ; "name_and_args")]
    fn parse_user_tool_prefix_valid(input: &str) {
        let parsed = parse_user_tool_prefix(input).unwrap();
        assert_eq!(parsed.name, "read");
        assert!(parsed.visible);
    }

    #[test_case("" ; "empty")]
    #[test_case("!cmd" ; "shell_sigil")]
    #[test_case("$" ; "sigil_only")]
    #[test_case("$$" ; "double_sigil_only")]
    #[test_case("$ read src/main.rs" ; "space_after_sigil")]
    #[test_case("$100 dollars" ; "prose_digits")]
    #[test_case("$with!bang x" ; "punctuation_in_name")]
    #[test_case("runs $read" ; "not_at_start")]
    fn parse_user_tool_prefix_invalid(input: &str) {
        assert_eq!(parse_user_tool_prefix(input), None);
    }

    #[test]
    fn parse_hidden_double_sigil() {
        let parsed = parse_user_tool_prefix("$$read src/main.rs").unwrap();
        assert_eq!(parsed.name, "read");
        assert!(!parsed.visible);
    }

    #[test]
    fn build_input_empty_is_empty_object() {
        assert_eq!(build_tool_input("", &json!({})).unwrap(), json!({}));
    }

    #[test]
    fn build_input_json_object_passthrough() {
        let input = build_tool_input(r#"{"path": "a.rs", "limit": 5}"#, &json!({})).unwrap();
        assert_eq!(input, json!({"path": "a.rs", "limit": 5}));
    }

    #[test_case("{\"path\"" ; "truncated")]
    #[test_case("[1, 2]" ; "not_object")]
    fn build_input_json_rejected(input: &str) {
        assert!(build_tool_input(input, &json!({})).is_err());
    }

    #[test]
    fn build_input_key_value_pairs() {
        let schema = json!({"type": "object", "properties": {
            "limit": {"type": "integer"},
            "path": {"type": "string"},
            "raw": {"type": "string"}
        }});
        let input = build_tool_input("path=a.rs limit=5 raw=text", &schema).unwrap();
        assert_eq!(input, json!({"path": "a.rs", "limit": 5, "raw": "text"}));
    }

    #[test]
    fn build_input_positional_fills_required_first() {
        let schema = json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "limit": {"type": "integer"},
                "offset": {"type": "integer"},
                "path": {"type": "string"}
            }
        });
        let input = build_tool_input("src/main.rs", &schema).unwrap();
        assert_eq!(input, json!({"path": "src/main.rs"}));
    }

    #[test]
    fn build_input_positional_respects_explicit_keys() {
        let schema = json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string"}, "tag": {"type": "string"}}
        });
        let input = build_tool_input("tag=x src/main.rs", &schema).unwrap();
        assert_eq!(input, json!({"path": "src/main.rs", "tag": "x"}));
    }

    #[test]
    fn build_input_positional_without_slot_errors() {
        let schema = json!({"type": "object", "properties": {"limit": {"type": "integer"}}});
        assert!(build_tool_input("stray", &schema).is_err());
    }

    #[test]
    fn build_input_quotes_stripped_from_values() {
        let schema = json!({"type": "object", "properties": {"path": {"type": "string"}}});
        let input = build_tool_input(r#""src/main.rs""#, &schema).unwrap();
        assert_eq!(input, json!({"path": "src/main.rs"}));
    }

    /// The write tool's real shape: two required strings whose emitted
    /// `properties` come out alphabetical while `required` keeps declaration
    /// order. Positional fill must follow `required`, not the alphabet, or
    /// `$write file text` swaps path and content.
    #[test]
    fn build_input_positional_follows_required_not_alphabet() {
        let schema = json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "append": {"type": "boolean"},
                "content": {"type": "string"},
                "path": {"type": "string"}
            }
        });
        let input = build_tool_input("/tmp/a.txt hello", &schema).unwrap();
        assert_eq!(
            input,
            json!({"path": "/tmp/a.txt", "content": "hello"})
        );
    }
}
