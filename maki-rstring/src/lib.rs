//! rstring — token-efficient, semantically-preserving compression for agent
//! sessions and tool output. Deterministic, synchronous, no models.
//!
//! Tiers (auto-detected, see [`detect`]):
//! - **stream** — bash-output fast lane: JSONL columnar + merge-iff-volatile
//!   line clustering (`[xN]` counts) + lossless repeated-span factoring
//!   (`<n>` refs resolved by a lead legend).
//! - **session-md** — markdown session exports: thinking → extractive
//!   decisions, stale tool outputs → resolvable refs.
//! - **session-json** — maki-style JSON exports: compact transcript rendering.
//!
//! Every elision lands in a content-addressed [`side::SideTable`] behind a
//! `<r:hash>` stub — `rstring expand <hash>` recovers the original.

pub mod cluster;
pub mod jsonl;
pub mod mask;
pub mod repeat;
pub mod session_json;
pub mod session_md;
pub mod side;
pub mod surp;
pub mod thin;
pub mod tokens;

/// Threshold below which a caller should skip compression entirely: the
/// markers a merge emits cost more context than they save on a short string.
/// Not enforced here — [`compress_if_useful`] costs nothing on prose anyway.
pub const MIN_BYTES: usize = 399;

#[derive(Debug, Clone)]
pub struct Cfg {
    pub keep_thinking: bool,
    pub surp_on: bool,
    /// Static thinning rate: when set, replaces per-doc surp with the
    /// corpus-table tier (token-exact budget, `rstring thin-table`).
    pub thin_rate: Option<f64>,
    pub evict_last: usize,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            keep_thinking: false,
            surp_on: true,
            thin_rate: None,
            evict_last: 3,
        }
    }
}

/// Auto-detect the input shape.
pub fn detect(text: &str) -> &'static str {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if v.get("messages").is_some() {
            return "session-json";
        }
        // bare top-level array of turns — the same session without an envelope
        if v.as_array().is_some_and(|a| {
            a.iter().all(|t| {
                t.as_object()
                    .is_some_and(|o| o.contains_key("type") || o.contains_key("role"))
            })
        }) {
            return "session-json";
        }
    }
    // JSONL of message records (claude-code style): most lines wrap a
    // {role, content} message in per-line metadata
    {
        let (mut msg, mut tot) = (0usize, 0usize);
        for line in text.lines().take(50) {
            let l = line.trim();
            if l.is_empty() {
                continue;
            }
            tot += 1;
            let wraps = serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|o| o.get("message").and_then(|m| m.get("role")).cloned())
                .is_some();
            msg += wraps as usize;
        }
        if tot > 0 && msg * 10 >= tot * 3 {
            return "session-json";
        }
    }
    if text.contains("<details") && text.contains("**Output:**") {
        return "session-md";
    }
    "stream"
}

/// bash-output fast lane: JSONL columnar when uniform, else volatile-merge
/// clustering followed by lossless repeated-span factoring.
///
/// `--thin` applies to the factored output — raw bash output is tool-output
/// bulk, the tier's intended target, and refs/legends are structured lines
/// its guards already skip. Columnar rows are never touched: a table's fields
/// are load-bearing by construction, and cell drops would break rows.
pub fn stream(text: &str, table: &mut side::SideTable, cfg: &Cfg) -> String {
    if let Some(col) = jsonl::try_columnar(text, table) {
        return col;
    }
    let clustered = cluster::run(text, table);
    let out = repeat::run(&clustered);
    match cfg.thin_rate {
        Some(r) => thin::run(&out, r),
        None => out,
    }
}

/// Compress with auto-detected tier. Returns (output, mode).
pub fn compress_auto(text: &str, table: &mut side::SideTable, cfg: &Cfg) -> (String, &'static str) {
    let mode = detect(text);
    let out = match mode {
        "session-md" => session_md::run(text, table, cfg),
        "session-json" => session_json::run(text, table, cfg),
        _ => stream(text, table, cfg),
    };
    (out, mode)
}

/// `stream` tier for a one-shot caller: ANSI escapes stripped, a fresh
/// [`side::SideTable`] per call — stubs dedupe within the returned text but
/// are not recoverable afterwards (`rstring expand` needs the table).
/// Returns the input unchanged unless the compressed form is really shorter,
/// so a caller may hand it everything and gate on [`MIN_BYTES`] only to skip
/// the work on short prose.
pub fn compress_if_useful(text: String) -> String {
    let stripped = mask::strip_ansi(&text);
    let mut table = side::SideTable::default();
    let out = stream(&stripped, &mut table, &Cfg::default());
    if out.len() < text.len() {
        out
    } else {
        text
    }
}

#[cfg(test)]
mod compress_tests {
    use super::*;

    const TS_LINE: &str = "2026-09-29T10:22:02.831781Z level=WARN provider=anthropic";

    fn repetitive() -> String {
        let later = TS_LINE.replace("10:22:02", "18:44:59");
        let mut s = String::new();
        while s.len() <= MIN_BYTES * 4 {
            s.push_str(TS_LINE);
            s.push('\n');
            s.push_str(&later);
            s.push('\n');
        }
        s
    }

    #[test]
    fn shrinks_repetitive_output() {
        let input = repetitive();
        let out = compress_if_useful(input.clone());
        assert!(out.contains(" [x"), "{out}");
        assert!(out.contains("10:22:02"), "{out}");
        assert!(out.len() < input.len() / 10, "{}", out.len());
    }

    #[test]
    fn incompressible_output_stays_byte_identical() {
        // Lines share a 4-byte prefix only, below repeat::run's MIN_CAND, so
        // the span-factoring pass cannot make them compressible either.
        let input: String = (0..MIN_BYTES / 8)
            .map(|i| format!("row {i} key {}\n", i * 7 + 3))
            .collect();
        assert_eq!(compress_if_useful(input.clone()), input);
    }

    #[test]
    fn ansi_escapes_stripped() {
        let input = format!("\u{1b}[31m{TS_LINE}\u{1b}[0m\n");
        let out = compress_if_useful(input);
        assert!(!out.contains('\u{1b}'), "{out}");
    }
}
