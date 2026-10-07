//! Faithful session export to markdown — the `/export` shape, rebuilt over
//! file envelopes instead of in-memory types. Robust across harnesses: maki
//! transcript JSONL (`{"t":"header"|"msg"|"meta",...}`), maki export JSON
//! (`{title, model, messages, ...}`), claude-code style JSONL (`message`
//! wrapping `{role, content}`, `summary` title lines, ISO timestamps), codex
//! rollouts (`payload` response items), and bare `{role, content}` lines.
//! Unrecognized lines are skipped, never fatal.
//!
//! Nothing is compressed or side-tabled; the only losses are opt-in
//! (`--no-thinking`, `--no-tools`). Tool calls stay, paired with the
//! `tool_result`/`function_call_output` that follows, mirroring how `/export`
//! joins a call to its `tool_outputs` entry.

use jiff::Timestamp;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write;

/// UTC render of a session date, matching the by-path link naming but with
/// the time joined by underscores.
const DATE_FMT: &str = "%Y-%m-%d_%H-%M-%S";

/// Cap on a first-user-prompt standing in for a missing session title.
const TITLE_FALLBACK_CHARS: usize = 90;

/// Caller-known metadata and content filters. `Some` metadata beats whatever
/// the envelope itself carries; `thinking: false` drops `## Thinking`
/// sections, `tools: false` drops tool calls and their outputs entirely.
/// The default keeps everything.
pub struct Opts {
    pub name: Option<String>,
    pub path: Option<String>,
    pub index: Option<i64>,
    pub date: Option<i64>,
    pub model: Option<String>,
    pub thinking: bool,
    pub tools: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            name: None,
            path: None,
            index: None,
            date: None,
            model: None,
            thinking: true,
            tools: true,
        }
    }
}

/// What the envelopes agree on: session metadata plus the message list, each
/// message `{role, content: [blocks]}` with string content normalized to one
/// text block.
#[derive(Default)]
struct Envelope {
    name: Option<String>,
    model: Option<String>,
    cwd: Option<String>,
    date: Option<i64>,
    messages: Vec<Value>,
}

pub fn run(text: &str, o: &Opts) -> Result<String, String> {
    let env =
        parse(text).ok_or("not a session: want maki transcript jsonl ({\"t\":\"msg\",...} lines) or export JSON ({\"messages\":[...]})")?;
    Ok(render(&env, o))
}

fn parse(text: &str) -> Option<Envelope> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        // A single jsonl record also parses as whole-document JSON, so only
        // an export envelope or a bare turn array takes this path.
        match v {
            Value::Array(_) => {
                return Some(from_export(serde_json::json!({ "messages": v })));
            }
            Value::Object(_) if v.get("messages").and_then(Value::as_array).is_some() => {
                return Some(from_export(v));
            }
            _ => {}
        }
    }
    parse_jsonl(text)
}

fn from_export(v: Value) -> Envelope {
    Envelope {
        name: str_field(&v, "title"),
        model: str_field(&v, "model"),
        cwd: str_field(&v, "cwd"),
        date: num_field(&v, "updated_at").or_else(|| num_field(&v, "created_at")),
        messages: v["messages"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(normalize)
            .collect(),
    }
}

fn parse_jsonl(text: &str) -> Option<Envelope> {
    let mut env = Envelope::default();
    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;
    for line in text.lines() {
        let Ok(rec) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match rec.get("t").and_then(Value::as_str) {
            Some("header") => {
                if env.model.is_none() {
                    env.model = str_field(&rec, "model");
                }
                if env.cwd.is_none() {
                    env.cwd = str_field(&rec, "cwd");
                }
                if env.date.is_none() {
                    env.date = num_field(&rec, "created_at");
                }
            }
            // Meta lines are appended as the session runs; the last one wins.
            Some("meta") => {
                if let Some(t) = str_field(&rec, "title").filter(|t| !t.is_empty()) {
                    env.name = Some(t);
                }
                if let Some(u) = num_field(&rec, "updated_at") {
                    env.date = Some(u);
                }
            }
            Some("msg") => {
                if let Some(d) = rec.get("d") {
                    env.messages.push(normalize(d));
                }
            }
            _ => absorb_harness(&rec, &mut env, &mut first_ts, &mut last_ts),
        }
    }
    env.date = env.date.or(last_ts).or(first_ts);
    (!env.messages.is_empty()).then_some(env)
}

// Everything that is not maki's envelope: claude-code style records
// (`message` wrapping a role/content message, `summary` title lines), codex
// rollouts (`payload` holding response items), and bare {role, content}
// lines. Timestamps trail the session: first seen, last seen.
fn absorb_harness(
    rec: &Value,
    env: &mut Envelope,
    first_ts: &mut Option<i64>,
    last_ts: &mut Option<i64>,
) {
    let mut stamp = |v: Option<&Value>| {
        if let Some(secs) = parse_ts(v) {
            if first_ts.is_none() {
                *first_ts = Some(secs);
            }
            *last_ts = Some(secs);
        }
    };
    if env.cwd.is_none() {
        env.cwd = str_field(rec, "cwd")
            .or_else(|| rec.get("payload").and_then(|p| str_field(p, "cwd")));
    }
    if rec.get("type").and_then(Value::as_str) == Some("summary") {
        if env.name.is_none() {
            env.name = str_field(rec, "summary").filter(|t| !t.trim().is_empty());
        }
        return;
    }
    // Meta prompts (claude-code's injected context) are not conversation.
    if rec.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return;
    }
    if let Some(m) = rec.get("message") {
        if m.get("role").is_some() {
            stamp(rec.get("timestamp"));
            if let Some(model) = str_field(m, "model") {
                env.model = Some(model);
            }
            env.messages.push(harness_message(m));
            return;
        }
    }
    // codex spells its turn metadata on the outer record.
    if rec.get("type").and_then(Value::as_str) == Some("turn_context") {
        if let Some(p) = rec.get("payload") {
            if let Some(model) = str_field(p, "model") {
                env.model = Some(model);
            }
        }
        return;
    }
    if let Some(p) = rec.get("payload") {
        stamp(rec.get("timestamp"));
        match p.get("type").and_then(Value::as_str) {
            Some("message") if p.get("role").is_some() => env.messages.push(harness_message(p)),
            Some("reasoning") => {
                let text = p
                    .get("summary")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| str_field(s, "text"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    env.messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": [{ "type": "thinking", "thinking": text }],
                    }));
                }
            }
            Some("function_call") => {
                let input = p
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or_else(|| p.get("arguments").cloned().unwrap_or(Value::Null));
                env.messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": p.get("call_id"),
                        "name": p.get("name"),
                        "input": input,
                    }],
                }));
            }
            Some("function_call_output") => {
                env.messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": p.get("call_id"),
                        "content": p.get("output"),
                    }],
                }));
            }
            Some("turn_context") => {
                if let Some(model) = str_field(p, "model") {
                    env.model = Some(model);
                }
            }
            _ => {}
        }
        return;
    }
    if rec.get("role").is_some() {
        env.messages.push(harness_message(rec));
    }
}

fn normalize(msg: &Value) -> Value {
    let mut m = msg.clone();
    if let Some(s) = msg.get("content").and_then(Value::as_str) {
        m["content"] = serde_json::json!([{ "type": "text", "text": s }]);
    }
    m
}

// Harness messages normalize like normalize(), plus codex's text flavors.
fn harness_message(m: &Value) -> Value {
    let mut out = m.clone();
    out["content"] = match m.get("content") {
        Some(Value::Array(a)) => Value::Array(a.iter().map(harness_block).collect()),
        Some(Value::String(s)) => serde_json::json!([{ "type": "text", "text": s }]),
        _ => serde_json::json!([]),
    };
    out
}

fn harness_block(b: &Value) -> Value {
    match b.get("type").and_then(Value::as_str) {
        Some("input_text") | Some("output_text") => {
            serde_json::json!({ "type": "text", "text": b.get("text") })
        }
        _ => b.clone(),
    }
}

// Harness timestamps arrive as unix seconds or ISO-8601 strings.
fn parse_ts(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.parse::<Timestamp>().ok().map(|t| t.as_second()),
        _ => None,
    }
}

// A harness with no title line still has its first spoken user prompt.
fn first_user_title(messages: &[Value]) -> Option<String> {
    for msg in messages {
        if role(msg) != "user" {
            continue;
        }
        for b in blocks(msg) {
            if btype(b) != Some("text") {
                continue;
            }
            let text = str_field(b, "text").unwrap_or_default();
            let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty()) else {
                continue;
            };
            if line.is_empty() {
                continue;
            }
            let mut title: String = line.chars().take(TITLE_FALLBACK_CHARS).collect();
            if line.chars().count() > TITLE_FALLBACK_CHARS {
                title.push_str("...");
            }
            return Some(title);
        }
    }
    None
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn num_field(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

fn render(env: &Envelope, o: &Opts) -> String {
    let mut out = String::with_capacity(16 * 1024);
    let _ = writeln!(out, "---\n---");
    let name = o
        .name
        .clone()
        .or_else(|| env.name.clone())
        .or_else(|| first_user_title(&env.messages))
        .unwrap_or_else(|| "session".into());
    let _ = writeln!(out, "# {name}");
    let _ = writeln!(
        out,
        "- {}",
        o.path.as_deref().or(env.cwd.as_deref()).unwrap_or("-")
    );
    let _ = writeln!(out, "- Index {}", o.index.unwrap_or(0));
    let _ = writeln!(out, "- Date {}", date_str(o.date.or(env.date)));
    let _ = writeln!(out, "- Model {}", o.model.as_deref().or(env.model.as_deref()).unwrap_or("-"));

    // A call's answer lives in the user turn that follows it; index the
    // results first so each call renders joined to its output.
    let mut outputs: HashMap<String, String> = HashMap::new();
    if o.tools {
        for msg in &env.messages {
            if role(msg) != "user" {
                continue;
            }
            for b in blocks(msg) {
                if btype(b) == Some("tool_result") {
                    if let Some(id) = str_field(b, "tool_use_id") {
                        outputs.insert(id, result_text(b));
                    }
                }
            }
        }
    }

    let mut user_n = 0usize;
    let mut model_n = 0usize;
    for msg in &env.messages {
        match role(msg) {
            "user" => {
                // Tool-result-only turns carry no spoken content; with tools
                // excluded their results have nowhere to show anyway.
                if !blocks(msg).iter().any(|b| btype(b) != Some("tool_result")) {
                    continue;
                }
                section(&mut out, "User", user_n);
                user_n += 1;
                for b in blocks(msg) {
                    match btype(b) {
                        Some("text") => push_text(&mut out, b),
                        Some("image") => {
                            let _ = writeln!(out, "![Image]\n");
                        }
                        _ => {}
                    }
                }
            }
            "assistant" => {
                let mut body = String::new();
                for b in blocks(msg) {
                    match btype(b) {
                        Some("text") => push_text(&mut body, b),
                        Some("thinking") if o.thinking => {
                            let th = str_field(b, "thinking").unwrap_or_default();
                            if !th.trim().is_empty() {
                                let _ = writeln!(body, "## Thinking\n");
                                let _ = writeln!(body, "{}\n", th.trim());
                            }
                        }
                        Some("redacted_thinking") if o.thinking => {
                            let _ = writeln!(body, "## Thinking\n\n[redacted]\n");
                        }
                        Some("tool_use") if o.tools => {
                            tool_call(&mut body, b, &outputs);
                        }
                        _ => {}
                    }
                }
                if body.trim().is_empty() {
                    continue;
                }
                section(&mut out, "Model", model_n);
                model_n += 1;
                out.push_str(&body);
            }
            _ => {}
        }
    }
    out
}

fn section(out: &mut String, role: &str, n: usize) {
    let _ = writeln!(out, "\n## {role} {n}\n");
}

fn push_text(out: &mut String, b: &Value) {
    if let Some(t) = str_field(b, "text").filter(|t| !t.is_empty()) {
        let _ = writeln!(out, "{}\n", t.trim_end());
    }
}

fn tool_call(out: &mut String, b: &Value, outputs: &HashMap<String, String>) {
    let name = str_field(b, "name").unwrap_or_else(|| "?".into());
    let id = str_field(b, "id").unwrap_or_default();
    let _ = writeln!(out, "**Tool Call:** `{name}`");
    if let Some(input) = b.get("input") {
        let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
        let _ = writeln!(out, "```json\n{}\n```\n", pretty.trim());
    }
    match outputs.get(id.as_str()) {
        Some(text) if !text.trim().is_empty() => {
            let _ = writeln!(out, "**Output:**");
            let _ = writeln!(out, "```\n{}\n```\n", text.trim_end());
        }
        _ => {
            let _ = writeln!(out);
        }
    }
}

fn result_text(b: &Value) -> String {
    match b.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|p| str_field(p, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn role(msg: &Value) -> &str {
    msg.get("role").and_then(Value::as_str).unwrap_or("")
}

fn blocks(msg: &Value) -> &[Value] {
    match msg.get("content").and_then(Value::as_array) {
        Some(a) => a,
        None => &[],
    }
}

fn btype(b: &Value) -> Option<&str> {
    b.get("type").and_then(Value::as_str)
}

fn date_str(secs: Option<i64>) -> String {
    secs.and_then(|s| Timestamp::from_second(s).ok())
        .map(|t| t.strftime(DATE_FMT).to_string())
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const JSONL: &str = concat!(
        r#"{"t":"header","v":2,"id":"s1","model":"zhipuai/glm","created_at":0}"#, "\n",
        r#"{"t":"msg","d":{"role":"user","content":[{"type":"text","text":"abc xzy"}]}}"#, "\n",
        r#"{"t":"msg","d":{"role":"assistant","content":[{"type":"thinking","thinking":"why"},{"type":"text","text":"hello"}]}}"#, "\n",
        r#"{"t":"msg","d":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"read","input":{"path":"a.rs"}}]}}"#, "\n",
        r#"{"t":"msg","d":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"fn main() {}"}]}}"#, "\n",
        r#"{"t":"msg","d":{"role":"user","content":"plain string"}}"#, "\n",
        r#"{"t":"meta","title":"my session","updated_at":86466}"#, "\n",
    );

    #[test]
    fn jsonl_renders_numbered_markdown() {
        let out = run(JSONL, &Opts::default()).unwrap();
        assert!(out.starts_with("---\n---\n# my session\n"));
        assert!(out.contains("- Date 1970-01-02_00-01-06\n"));
        assert!(out.contains("- Model zhipuai/glm\n"));
        assert!(out.contains("## User 0\n\nabc xzy\n"));
        assert!(out.contains("## Model 0\n\n## Thinking\n\nwhy\n\nhello\n"));
        assert!(out.contains("**Tool Call:** `read`"));
        assert!(out.contains("```json\n{\n  \"path\": \"a.rs\"\n}\n```"));
        assert!(out.contains("**Output:**\n```\nfn main() {}\n```"));
        // The tool-result turn and the plain-string turn both number.
        assert!(out.contains("## User 1\n\nplain string\n"));
        assert!(!out.contains("## User 2"));
    }

    #[test]
    fn export_json_body_matches_jsonl_body() {
        let json = json!({
            "title": "my session",
            "model": "zhipuai/glm",
            "updated_at": 86466,
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "abc xzy"}]},
                {"role": "assistant", "content": [{"type": "thinking", "thinking": "why"}, {"type": "text", "text": "hello"}]},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "read", "input": {"path": "a.rs"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "fn main() {}"}]},
                {"role": "user", "content": [{"type": "text", "text": "plain string"}]}
            ]
        })
        .to_string();
        let from_json = run(&json, &Opts::default()).unwrap();
        let from_jsonl = run(JSONL, &Opts::default()).unwrap();
        fn body(s: &str) -> &str {
            s.split_once("\n- Model").unwrap().1
        }
        assert_eq!(body(&from_json), body(&from_jsonl));
    }

    #[test]
    fn no_thinking_drops_thinking_sections() {
        let out = run(JSONL, &Opts { thinking: false, ..Opts::default() }).unwrap();
        assert!(!out.contains("## Thinking"));
        assert!(!out.contains("why\n"));
        assert!(out.contains("hello"));
    }

    #[test]
    fn no_tools_drops_calls_and_outputs() {
        let out = run(JSONL, &Opts { tools: false, ..Opts::default() }).unwrap();
        assert!(!out.contains("Tool Call"));
        assert!(!out.contains("fn main() {}"));
        assert!(out.contains("## Model 0\n\n## Thinking\n\nwhy\n\nhello\n"));
    }

    #[test]
    fn opts_override_envelope_metadata() {
        let o = Opts {
            name: Some("named".into()),
            path: Some("/tmp/s.jsonl".into()),
            index: Some(3),
            date: Some(0),
            model: Some("other".into()),
            ..Opts::default()
        };
        let out = run(JSONL, &o).unwrap();
        assert!(out.contains("# named\n- /tmp/s.jsonl\n- Index 3\n- Date 1970-01-01_00-00-00\n- Model other\n"));
    }

    #[test]
    fn garbage_is_an_error_not_empty_output() {
        assert!(run("hello world", &Opts::default()).is_err());
    }

    const CLAUDE_JSONL: &str = concat!(
        r#"{"type":"summary","summary":"Fix the parser","leafUuid":"u1"}"#,
        "\n",
        r#"{"parentUuid":null,"type":"user","isMeta":true,"message":{"role":"user","content":"context blob"},"timestamp":"2026-10-07T00:15:00Z"}"#,
        "\n",
        r#"{"sessionId":"s","type":"user","message":{"role":"user","content":"read SPEC.md"},"timestamp":"2026-10-07T00:15:29Z","cwd":"/repo"}"#,
        "\n",
        r#"{"type":"assistant","message":{"model":"claude-x","role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"On it"},{"type":"tool_use","id":"t1","name":"read","input":{"path":"SPEC.md"}}]},"timestamp":"2026-10-07T00:15:30Z"}"#,
        "\n",
        r##"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"# SPEC"}]},"timestamp":"2026-10-07T00:15:31Z"}"##,
        "\n",
    );

    #[test]
    fn claude_code_jsonl_renders_with_summary_title() {
        let out = run(CLAUDE_JSONL, &Opts::default()).unwrap();
        assert!(out.contains("# Fix the parser\n- /repo\n"));
        assert!(out.contains("- Model claude-x\n"));
        // last seen timestamp, not the first
        assert!(out.contains("- Date 2026-10-07_00-15-31\n"));
        assert!(out.contains("## User 0\n\nread SPEC.md\n"));
        assert!(out.contains("## Thinking\n\nhmm\n"));
        assert!(out.contains("**Tool Call:** `read`"));
        assert!(out.contains("**Output:**\n```\n# SPEC\n```"));
        // isMeta context never becomes a section
        assert!(!out.contains("context blob"));
    }

    const CODEX_JSONL: &str = concat!(
        r#"{"timestamp":"2026-10-07T01:00:00Z","type":"session_meta","payload":{"id":"x","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-10-07T01:00:01Z","type":"turn_context","payload":{"model":"gpt-5-codex","cwd":"/repo"}}"#,
        "\n",
        r#"{"timestamp":"2026-10-07T01:00:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-10-07T01:00:03Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"plan"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-10-07T01:00:04Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"ls\"}","call_id":"c1"}}"#,
        "\n",
        r#"{"timestamp":"2026-10-07T01:00:05Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"a.txt"}}"#,
        "\n",
        r#"{"timestamp":"2026-10-07T01:00:06Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}"#,
        "\n",
    );

    #[test]
    fn codex_rollout_renders_reasoning_and_calls() {
        let out = run(CODEX_JSONL, &Opts::default()).unwrap();
        assert!(out.contains("- Model gpt-5-codex\n"));
        assert!(out.contains("- Date 2026-10-07_01-00-06\n"));
        assert!(out.contains("## User 0\n\ngo\n"));
        assert!(out.contains("## Model 0\n\n## Thinking\n\nplan\n"));
        assert!(out.contains("**Tool Call:** `shell`"));
        assert!(out.contains("```json\n{\n  \"cmd\": \"ls\"\n}\n```"));
        assert!(out.contains("**Output:**\n```\na.txt\n```"));
        assert!(out.contains("## Model 2\n\ndone\n"));
    }

    #[test]
    fn bare_role_lines_and_arrays_render_with_prompt_title() {
        let lines = concat!(
            "{\"role\":\"user\",\"content\":\"first prompt\"}\n",
            "{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}\n",
        );
        let out = run(lines, &Opts::default()).unwrap();
        assert!(out.contains("# first prompt\n"));
        assert!(out.contains("## Model 0\n\nhi\n"));

        let arr = r#"[{"role":"user","content":"array prompt"},{"role":"assistant","content":"hello"}]"#;
        let out = run(arr, &Opts::default()).unwrap();
        assert!(out.contains("# array prompt\n"));
        assert!(out.contains("## Model 0\n\nhello\n"));
    }
}
