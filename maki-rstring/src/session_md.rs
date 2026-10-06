//! Markdown session tier: thinking -> extractive decisions (+ side-table),
//! tool outputs -> volatile-merge clustering, stale non-errors -> resolvable
//! refs, prose -> gated surprisal drop. Single walk, deterministic.

use crate::{cluster, side::SideTable, surp, tokens, Cfg};
use std::collections::HashSet;

const CUES: &[&str] = &[
    "so ",
    "therefore",
    "decision",
    "decided",
    "chose",
    "chosen",
    "fix",
    "fixed",
    "error",
    "fail",
    "problem",
    "issue",
    "must",
    "need to",
    "next step",
    "goal",
    "conclusion",
    "that means",
    "which means",
    "the key",
    "important",
    "instead",
    "root cause",
    "turns out",
    "actually the",
    "hypothesis",
];

const ERR_SIG: &[&str] = &[
    "error", "Error", "ERROR", "CS0", "CS1", "panic", "Panic", "fail", "Fail", "denied",
];

fn is_error(s: &str) -> bool {
    ERR_SIG.iter().any(|e| s.contains(e))
}

fn sentences(block: &str) -> Vec<String> {
    let mut s = Vec::new();
    let mut buf = String::new();
    for c in block.chars() {
        buf.push(c);
        if c == '.' || c == '\n' {
            if !buf.trim().is_empty() {
                s.push(buf.trim().to_string());
            }
            buf.clear();
        }
    }
    if !buf.trim().is_empty() {
        s.push(buf.trim().to_string());
    }
    s
}

pub fn decisions(block: &str) -> String {
    let s = sentences(block);
    let kept: Vec<String> = s
        .iter()
        .enumerate()
        .filter(|(i, sent)| {
            let low = format!("{} ", sent.to_lowercase());
            CUES.iter().any(|c| low.contains(c)) || *i == 0 || i + 1 == s.len()
        })
        .map(|(_, x)| x.clone())
        .collect();
    kept.join(" ")
}

fn strip_fmt_line(line: &str) -> Option<String> {
    let t = line.trim_start();
    if t.starts_with("```")
        || t == "---"
        || t.starts_with("<details")
        || t == "</details>"
        || t.starts_with("<summary>")
    {
        return None;
    }
    let mut l = line.replace("**", "");
    if t.starts_with('#') {
        l = t.trim_start_matches('#').trim_start().to_string();
    }
    Some(l)
}

pub fn run(text: &str, table: &mut SideTable, cfg: &Cfg) -> String {
    // Pass 1: collect tool output bodies to decide keep/evict indices.
    let mut bodies: Vec<String> = Vec::new();
    {
        let mut in_output = false;
        let mut opened = false;
        let mut buf = String::new();
        for line in text.lines() {
            let t = line.trim_start();
            if t == "**Output:**" {
                in_output = true;
                opened = false;
                buf.clear();
            } else if in_output && t.starts_with("```") {
                if !opened {
                    opened = true;
                } else {
                    bodies.push(std::mem::take(&mut buf));
                    in_output = false;
                }
            } else if in_output {
                buf.push_str(line);
                buf.push('\n');
            }
        }
    }
    let keep: HashSet<usize> =
        (bodies.len().saturating_sub(cfg.evict_last)..bodies.len()).collect();

    // Pass 2: emit.
    let mut out = String::with_capacity(text.len() / 2);
    let mut in_think = false;
    let mut think_buf = String::new();
    let mut in_output = false;
    let mut output_opened = false;
    let mut out_buf = String::new();
    let mut idx = 0usize;

    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("<details") {
            in_think = true;
            continue;
        }
        if t == "</details>" {
            in_think = false;
            let block = std::mem::take(&mut think_buf);
            if !block.trim().is_empty() {
                if cfg.keep_thinking {
                    out.push_str(&block);
                } else {
                    let d = decisions(&block);
                    let hash = table.put(&block);
                    out.push_str(&format!(
                        "<think r={} n={}>\n{}\n</think>\n",
                        hash,
                        tokens::count(&block),
                        d
                    ));
                }
            }
            continue;
        }
        if in_think {
            if !t.starts_with("<summary") {
                think_buf.push_str(line);
                think_buf.push('\n');
            }
            continue;
        }
        if t == "**Output:**" {
            in_output = true;
            output_opened = false; // next ``` OPENS the fenced body
            out_buf.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_output {
            if t.starts_with("```") {
                if !output_opened {
                    output_opened = true;
                    continue;
                }
                let body = std::mem::take(&mut out_buf);
                if keep.contains(&idx) || is_error(&body) {
                    let mut b = cluster::run(&body, table);
                    if let Some(r) = cfg.thin_rate {
                        // static-table thin, token-exact budget
                        b = crate::thin::run(&b, r);
                    } else if cfg.surp_on {
                        // word-drop scoped to tool-output bulk only
                        b = surp::run(&b, 0.85);
                    }
                    out.push_str(b.trim_end());
                } else {
                    let hash = table.put(&body);
                    let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                    let v: String = first
                        .chars()
                        .take(70)
                        .map(|c| if c == '"' { '\u{201c}' } else { c })
                        .collect::<String>()
                        .replace('\n', " ");
                    out.push_str(&format!(
                        "<r:{} n={} v=\"{}\">",
                        hash,
                        tokens::count(&body),
                        v
                    ));
                }
                out.push('\n');
                idx += 1;
                in_output = false;
                continue;
            }
            out_buf.push_str(line);
            out_buf.push('\n');
            continue;
        }
        if let Some(l) = strip_fmt_line(line) {
            out.push_str(&l);
            out.push('\n');
        }
    }

    // Lossless final pass only: merge-iff-volatile clustering. Word-drop (surp)
    // is NEVER applied to narration, decisions, or user turns — prose stays
    // verbatim; surp (if enabled) is scoped to kept tool-output bodies above.
    cluster::run(&out, table)
}
