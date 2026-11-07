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
pub mod export_md;
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

/// [`compress_if_useful`] keeps its output only when it cuts at least this
/// many bytes *and* at least 1/[`MIN_CUT_DIV`] of the input. Below that the
/// markers cost the reader more than the bytes saved: a prompt with one
/// folded line or a log shaved by 3% is better left alone.
pub const MIN_CUT_BYTES: usize = 256;
pub const MIN_CUT_DIV: usize = 5;

#[derive(Debug, Clone)]
pub struct Cfg {
    pub keep_thinking: bool,
    pub surp_on: bool,
    /// Static thinning rate: when set, replaces per-doc surp with the
    /// corpus-table tier (token-exact budget, `rstring thin-table`).
    pub thin_rate: Option<f64>,
    pub evict_last: usize,
    /// Where stream-tier entropy runs go: the side table (CLI) or an
    /// unrecoverable inline marker (one-shot callers).
    pub elide: mask::Elide,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            keep_thinking: false,
            surp_on: true,
            thin_rate: None,
            evict_last: 3,
            elide: mask::Elide::Side,
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
    if let Some(col) = jsonl::try_columnar_with(text, table, cfg.elide) {
        return col;
    }
    let clustered = cluster::run_with(text, table, cfg.elide);
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

/// `stream` tier for a one-shot caller: ANSI escapes stripped, no side
/// table survives the call, so entropy runs elide inline
/// ([`mask::Elide::Inline`], [`mask::INLINE_RUN_MIN`]+ only) instead of as
/// stubs nobody could expand. Returns the (ANSI-stripped) input unless the
/// cut clears [`MIN_CUT_BYTES`] and 1/[`MIN_CUT_DIV`] of it, so a caller
/// may hand it everything and gate on [`MIN_BYTES`] only to skip the work on
/// short prose.
pub fn compress_if_useful(text: String) -> String {
    let stripped = mask::strip_ansi(&text);
    let mut table = side::SideTable::default();
    let cfg = Cfg {
        elide: mask::Elide::Inline,
        ..Cfg::default()
    };
    let out = stream(&stripped, &mut table, &cfg);
    let cut = stripped.len().saturating_sub(out.len());
    if cut >= MIN_CUT_BYTES && cut * MIN_CUT_DIV >= stripped.len() {
        out
    } else if stripped.len() < text.len() {
        stripped // escapes are pure noise: dropping them needs no marker
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

    #[test]
    fn small_cut_returns_input_untouched() {
        // One folded pair inside a long prose prompt: shorter, but not by
        // MIN_CUT_BYTES / 1/MIN_CUT_DIV, so no marker reaches the model.
        let mut input = String::from("Please look at these two lines and the notes below.\n");
        input.push_str(&format!(
            "{TS_LINE}\n{}\n",
            TS_LINE.replace("10:22", "11:22")
        ));
        for i in 0..20 {
            input.push_str(&format!(
                "note {i}: a distinct sentence about part {i} of the plan\n"
            ));
        }
        assert_eq!(compress_if_useful(input.clone()), input);
    }

    #[test]
    fn one_shot_masks_only_long_blobs_inline() {
        let digest = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        let jwt: String = (0..4)
            .map(|_| "eyJhbGciOiJIUzI1NiJ9xQ2LmF5dGhvcml6YXRpb24iOiJCZWFyZXIgYWJj1Z9q")
            .collect();
        let mut input = format!("sha {digest}\ntoken {jwt}\n");
        input.push_str(&repetitive());
        let out = compress_if_useful(input);
        assert!(out.contains(digest), "digests stay quotable: {out}");
        assert!(
            out.contains(&format!("token [masked n={}]", jwt.len())),
            "{out}"
        );
        assert!(!out.contains("<r:"), "no unexpandable stubs: {out}");
    }
}
