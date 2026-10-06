//! JSON session tier (maki-style export) v2: render a compact, model-readable
//! transcript instead of re-serializing JSON.
//!
//! Savings come from three structural facts:
//! 1. `tool_outputs` duplicates the inline `tool_result` blocks in `messages`
//!    (verified byte-identical for ~94% of entries, remainder empty-vs-filled)
//!    — dropped, zero information loss.
//! 2. JSON keys/punctuation (`{"type":"text","text":"…"}` × per block) are the
//!    token-expensive part — replaced by one-word role/block markers.
//! 3. `thinking` blocks are extractive-decision stubs; originals go to the
//!    side table (`rstring expand <hash>`).
//!
//! Kept verbatim: tool names + full inputs (commands are load-bearing), error
//! outputs, the last `evict_last` outputs. Structured text is never word-dropped.

use crate::{cluster, side::SideTable, surp, tokens, Cfg};
use serde_json::{json, Value};

const ERR_SIG: &[&str] = &[
    "error", "Error", "ERROR", "CS0", "CS1", "panic", "Panic", "fail", "Fail", "denied",
];

fn is_error(s: &str) -> bool {
    ERR_SIG.iter().any(|e| s.contains(e))
}

/// Escape a value for a one-line `v="…"` preview.
fn preview(s: &str, n: usize) -> String {
    s.chars()
        .take(n)
        .map(|c| match c {
            '"' => '\u{201c}'.to_string(), // typographic quotes: never break the stub
            '\\' => '\\'.to_string(),
            _ => c.to_string(),
        })
        .collect::<String>()
        .replace('\n', " ")
}

fn ref_stub(body: &str, table: &mut SideTable) -> String {
    let hash = table.put(body);
    let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    format!(
        "<r:{} n={} v=\"{}\">",
        hash,
        tokens::count(body),
        preview(first, 70)
    )
}

fn decisions(block: &str) -> String {
    // Reuse the md tier's cue-phrase extraction.
    crate::session_md::decisions(block)
}

fn compact_input(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut parts: Vec<String> =
                m.iter().map(|(k, val)| format!("{}={}", k, val)).collect();
            parts.sort();
            parts.join(" ")
        }
        other => other.to_string(),
    }
}

pub fn run(text: &str, table: &mut SideTable, cfg: &Cfg) -> String {
    // Three envelopes are the same session: a whole-document object (native
    // export / API request), a bare array of flat turns, or JSONL lines each
    // wrapping a message in per-line metadata (claude-code style).
    let (v, pre) = match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(a)) => (normalize_turns(Value::Array(a)), String::new()),
        Ok(v) => (v, String::new()),
        Err(_) => normalize_jsonl(text, table),
    };
    let mut out = String::with_capacity(text.len() / 3);
    out.push_str(&pre);

    // Header: short top-level scalars only; big containers handled below.
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            match val {
                Value::Array(_) | Value::Object(_) => {
                    let s = val.to_string();
                    if s.len() <= 120 {
                        out.push_str(&format!("{}: {}\n", k, s));
                    } else if k == "tool_outputs" {
                        out.push_str(
                            "tool_outputs: <dropped — mirrors inline tool_result content>\n",
                        );
                    }
                }
                Value::String(s) if s.len() <= 200 => out.push_str(&format!("{}: {}\n", k, s)),
                other => out.push_str(&format!("{}: {}\n", k, other)),
            }
        }
    }

    let empty: Vec<Value> = Vec::new();
    let messages = v
        .get("messages")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    // Pass 1: tool_result keep/evict plan (errors + last N stay).
    let mut n_results = 0usize;
    for msg in messages {
        if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
            n_results += blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                .count();
        }
    }
    let mut seen_results = 0usize;

    // Pass 2: render.
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("?");
        out.push_str(&format!("\n<{}>\n", role));
        let content = msg.get("content");
        let blocks = match content {
            Some(Value::Array(a)) => a,
            Some(Value::String(s)) => {
                out.push_str(s);
                out.push('\n');
                continue;
            }
            _ => continue,
        };
        for b in blocks {
            let btype = b.get("type").and_then(Value::as_str).unwrap_or("");
            match btype {
                "text" => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        out.push_str(t);
                        out.push('\n');
                    }
                }
                "thinking" => {
                    let th = b.get("thinking").and_then(Value::as_str).unwrap_or("");
                    if th.trim().is_empty() {
                        continue;
                    }
                    if cfg.keep_thinking {
                        out.push_str(th);
                        out.push('\n');
                    } else {
                        let hash = table.put(th);
                        out.push_str(&format!(
                            "<think r={} n={}>\n{}\n</think>\n",
                            hash,
                            tokens::count(th),
                            decisions(th)
                        ));
                    }
                }
                "tool_use" => {
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                    let input = b.get("input").map(compact_input).unwrap_or_default();
                    out.push_str(&format!("> {} {}\n", name, input));
                }
                "tool_result" => {
                    seen_results += 1;
                    let body = result_text(b);
                    let keep = seen_results + cfg.evict_last > n_results || is_error(&body);
                    if keep {
                        let mut s = cluster::run(&body, table);
                        if let Some(r) = cfg.thin_rate {
                            // static-table thin, token-exact budget
                            s = crate::thin::run(&s, r);
                        } else if cfg.surp_on {
                            // word-drop scoped to machine-generated bulk only
                            s = surp::run(&s, 0.85);
                        }
                        out.push_str(s.trim_end());
                        out.push('\n');
                    } else {
                        out.push_str(&ref_stub(&body, table));
                        out.push('\n');
                    }
                }
                other => {
                    out.push_str(&format!("[{} {}]\n", other, b));
                }
            }
        }
    }

    // Lossless final pass.
    cluster::run(&out, table)
}

/// Collect a JSONL session (claude-code style: one record per line, each
/// wrapping a {role, content} message in metadata) into the message envelope.
/// Non-message records and unparseable lines are elided to the side table —
/// nothing is dropped silently.
fn normalize_jsonl(text: &str, table: &mut SideTable) -> (Value, String) {
    let mut messages = Vec::new();
    let mut pre = String::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(l) {
            Ok(obj) if obj.get("message").is_some() => {
                if let Some(m) = obj.get("message") {
                    messages.push(m.clone());
                }
            }
            Ok(obj) => {
                let ty = obj.get("type").and_then(Value::as_str).unwrap_or("record");
                pre.push_str(&format!(
                    "[{} r={} n={}]\n",
                    ty,
                    table.put(l),
                    tokens::count(l)
                ));
            }
            Err(_) => {
                pre.push_str(&format!(
                    "[line r={} n={}]\n",
                    table.put(l),
                    tokens::count(l)
                ));
            }
        }
    }
    (json!({"messages": messages}), pre)
}

/// Fold a bare array of flat turns ({type, text[, name, args]}) into the
/// {messages: [{role, content: [blocks]}]} envelope the renderer expects.
fn normalize_turns(v: Value) -> Value {
    let turns = match v {
        Value::Array(a) => a,
        _ => unreachable!(),
    };
    let mut messages = Vec::with_capacity(turns.len());
    for t in turns {
        let ty = t
            .get("type")
            .or_else(|| t.get("role"))
            .and_then(Value::as_str)
            .unwrap_or("user");
        let text = t.get("text").and_then(Value::as_str).unwrap_or("");
        let msg = match ty {
            "tool" => {
                let name = t.get("name").and_then(Value::as_str).unwrap_or("?");
                let input: Value = t
                    .get("args")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| json!({}));
                json!({"role": "assistant", "content": [
                    {"type": "tool_use", "name": name, "input": input},
                    {"type": "tool_result", "content": text},
                ]})
            }
            role => json!({"role": role, "content": [{"type": "text", "text": text}]}),
        };
        messages.push(msg);
    }
    json!({"messages": messages})
}

/// maki wraps tool_result content as {"Plain": {"text": …}} sometimes.
fn result_text(b: &Value) -> String {
    match b.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(wrapped @ Value::Object(_)) => {
            if let Some(t) = wrapped
                .get("Plain")
                .and_then(|p| p.get("text"))
                .and_then(Value::as_str)
            {
                return t.to_string();
            }
            wrapped.to_string()
        }
        Some(other) => other.to_string(),
        None => String::new(),
    }
}
