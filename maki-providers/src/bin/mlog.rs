//! Viewer for `.mlog` binary API logs (see `maki_providers::wire_log`).
//!
//! ```text
//! mlog <file.mlog>              pretty request/response, SSE responses reconstructed
//! mlog --raw <file.mlog>        exact bytes as sent/received (byte-for-byte)
//! mlog --transcript <file>      linear conversation: only each turn's new messages
//! ```

use std::process::ExitCode;

use maki_providers::wire_log::{self, Record};
use serde_json::Value;

fn main() -> ExitCode {
    let mut raw = false;
    let mut transcript = false;
    let mut path: Option<String> = None;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--raw" => raw = true,
            "--transcript" => transcript = true,
            "-h" | "--help" => {
                eprintln!("usage: mlog [--raw|--transcript] <file.mlog>");
                return ExitCode::SUCCESS;
            }
            other => path = Some(other.to_string()),
        }
    }

    let Some(path) = path else {
        eprintln!("usage: mlog [--raw|--transcript] <file.mlog>");
        return ExitCode::FAILURE;
    };

    let records = match wire_log::read_file(std::path::Path::new(&path)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("mlog: {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    if transcript {
        print_transcript(&records);
    } else {
        print_records(&records, raw);
    }
    ExitCode::SUCCESS
}

fn ts(ms: u64) -> String {
    jiff::Timestamp::from_millisecond(ms as i64)
        .map(|t| t.to_string()[..19].replace('T', " "))
        .unwrap_or_else(|_| ms.to_string())
}

fn pretty(bytes: &[u8]) -> String {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| lossy(bytes)),
        Err(_) => lossy(bytes),
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn print_records(records: &[Record], raw: bool) {
    for rec in records {
        match rec {
            Record::Request { ts_ms, uri, body } => {
                println!("\n\x1b[1;34m[{}] REQUEST\x1b[0m {uri}", ts(*ts_ms));
                if raw {
                    println!("{}", lossy(body));
                } else {
                    println!("{}", pretty(body));
                }
            }
            Record::Response {
                ts_ms,
                status,
                content_type,
                body,
            } => {
                println!(
                    "\n\x1b[1;32m[{}] RESPONSE\x1b[0m {status} {content_type}",
                    ts(*ts_ms)
                );
                if raw {
                    println!("{}", lossy(body));
                } else {
                    let cleaned = wire_log::clean_response_body(&lossy(body), Some(content_type));
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&cleaned).unwrap_or_else(|_| lossy(body))
                    );
                }
            }
        }
    }
}

/// Linear conversation: for each request print only the messages that weren't in
/// the previous request, so the growing history isn't reprinted every turn.
fn print_transcript(records: &[Record]) {
    let mut prev_len = 0usize;
    for rec in records {
        let Record::Request { ts_ms, body, .. } = rec else {
            continue;
        };
        let Ok(val) = serde_json::from_slice::<Value>(body) else {
            continue;
        };
        let messages = val
            .get("messages")
            .or_else(|| val.get("contents"))
            .and_then(Value::as_array);
        let Some(messages) = messages else {
            continue;
        };

        for msg in messages.iter().skip(prev_len) {
            let role = msg
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("message");
            println!("\n\x1b[1;35m[{}] {role}\x1b[0m", ts(*ts_ms));
            println!("{}", render_content(msg));
        }
        prev_len = messages.len();
    }
}

/// Best-effort text of a message across provider shapes (string or block array).
fn render_content(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    t.to_string()
                } else if let Some(t) = b.get("thinking").and_then(Value::as_str) {
                    format!("[thinking] {t}")
                } else {
                    serde_json::to_string(b).unwrap_or_default()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => serde_json::to_string(msg).unwrap_or_default(),
    }
}
