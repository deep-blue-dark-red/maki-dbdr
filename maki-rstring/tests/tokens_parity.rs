//! Divergence tripwire: rstring's tokie-backed counter vs tiktoken-rs.
//!
//! tokie re-implements o200k (hand-written scanner + backtracking BPE)
//! rather than wrapping tiktoken, so agreement with
//! `o200k_base().encode_with_special_tokens()` — the oracle every figure in
//! bench/RESULTS.md is scored in — is *gated*, not assumed.
//!
//! Contract: anything not pinned below must match exactly. A divergence on
//! an unpinned input fails this test, so a new tokie regression cannot land
//! silently. Inputs that disagree today are pinned by name: they may differ
//! but not by more than [`within_envelope`], so a pin cannot absorb a growing
//! divergence. If upstream fixes one, the note printed here is how we notice
//! the list has gone stale.

use std::path::PathBuf;

fn reference(s: &str) -> usize {
    let bpe = tiktoken_rs::o200k_base().expect("o200k vocab");
    bpe.encode_with_special_tokens(s).len()
}

/// Known-divergent edge cases, keyed by the label used in `edge_cases`.
///
/// tokie's backtracking BPE and tiktoken's rank-ordered merges part ways on
/// long uniform runs and on some newline/tab seams; these three disagree
/// today. Anything else in `edge_cases` must match exactly — pinning a
/// label is a last resort, and one that starts agreeing again should be
/// removed rather than left to rot into a blanket excuse.
const KNOWN_DIVERGENT_CASES: &[&str] = &[
    // 10k run of one byte: tokie 1261 vs tiktoken 1250.
    "long_x",
    // Newline/space seam around a word run.
    "ssippi_newline",
    // The `\t<ID>` case behind the jsonl_columnar footnote in README: 4 ids
    // vs tiktoken's 3, and every tab-separated log line carries one.
    "tab_angle_bracket",
];

/// Bench assets whose totals already disagree with the oracle. All three
/// are agent-session JSONL (tabs, JSON, newline seams) and each runs exactly
/// one token over tiktoken; the rest of the corpus — log stream, session
/// json/md, turns, toml/yaml records — matches to the token.
const KNOWN_DIVERGENT_FILES: &[&str] = &[
    "claude-maki.jsonl",
    "claude-zig.jsonl",
    "claude-search.jsonl",
];

/// A pin freezes a divergence, it does not excuse growth: today the worst is
/// +11 tokens (the single-byte run) and every corpus asset is +1. Anything
/// past this envelope is a tokie regression to re-review, not to absorb.
fn within_envelope(got: usize, want: usize) -> bool {
    let d = got.abs_diff(want);
    d <= 4 || d * 100 <= want
}

fn edge_cases() -> Vec<(&'static str, String)> {
    let long_x = "x".repeat(10_000);
    let long_ab = "a b ".repeat(5_000);
    vec![
        ("empty", String::new()),
        ("hello", "hello world".into()),
        ("ws_edges", "  leading and trailing   ".into()),
        ("mixed_ws", "\n\r\n\t\tmixed whitespace\r\n".into()),
        (
            "camel",
            "camelCase and HTTPServer and iOSApp and Müllerstraße".into(),
        ),
        (
            "contractions",
            "don't we'll it's I'm you've they're can't".into(),
        ),
        (
            "digits",
            "digits 123 1234 12345 123456 007 3.14159 1,000,000".into(),
        ),
        (
            "cjk",
            "日本語のテキスト中文한국어 mixed with English".into(),
        ),
        (
            "emoji",
            "emoji 👨‍👩‍👧‍👦 family ZWJ and 🇺🇸 flags and ☃︎ variants".into(),
        ),
        (
            "accents",
            "naïve café résumé Ægyptus Ἑλληνικός русский".into(),
        ),
        ("special_endoftext", "<|endoftext|>".into()),
        ("special_endofprompt", "<|endofprompt|>".into()),
        ("special_mid", "before <|endoftext|> after".into()),
        (
            "fim_not_special",
            "<|fim_prefix|><|fim_middle|><|fim_suffix|>".into(),
        ),
        (
            "near_specials",
            "<|not_a_special|> <|endoftex <|endofprompts|>".into(),
        ),
        (
            "ansi",
            "ANSI \x1b[31mred\x1b[0m and \x1b[1;32mbold\x1b[0m codes".into(),
        ),
        (
            "json_escapes",
            "{\"role\":\"user\",\"content\":\"json with \\\"escapes\\\" and \\n newlines\"}".into(),
        ),
        ("long_x", long_x),
        ("long_ab", long_ab),
        (
            "snake",
            "long_unbroken_snake_case_identifier_name_for_tool_output_parsing".into(),
        ),
        (
            "path",
            "/Users/mcp/git/rstring/src/session_json.rs:142:30".into(),
        ),
        (
            "sha256",
            "sha256: a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2".into(),
        ),
        (
            "symbols",
            "100% $99.99 ±0.5 §3 ¶4 — em dash, 'curly quotes', «guillemets»".into(),
        ),
        ("ctrl", "tabs\tand\x0bvertical\x0cform\x1effeed".into()),
        (
            "json_log_line",
            "maki_providers::provider\t<TS> [x2]\n{\"message\":\"failed to create provider\"}"
                .into(),
        ),
        ("ssippi_newline", "issippi\n \n All construction".into()),
        // Minimal repro for the +1/line divergence that moves README's
        // jsonl_columnar figure: tiktoken merges `<ID` (3 ids), tokie doesn't (4).
        ("tab_angle_bracket", "\t<ID>".into()),
    ]
}

#[test]
fn edge_cases_match_tiktoken() {
    for (name, c) in edge_cases() {
        let (got, want) = (maki_rstring::tokens::count(&c), reference(&c));
        if got == want {
            continue;
        }
        if KNOWN_DIVERGENT_CASES.contains(&name) {
            assert!(
                within_envelope(got, want),
                "pinned divergence [{name}] widened: tokie {got} vs tiktoken {want} ({c:?})"
            );
            eprintln!("known divergence [{name}]: tokie {got} vs tiktoken {want}");
            continue;
        }
        panic!("new divergence on {name}: tokie {got} vs tiktoken {want} ({c:?})");
    }
}

#[test]
fn corpus_files_match_tiktoken() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bench/bench_assets");
    // The corpus is 6.6 MB of real agent transcripts, kept in the crate's own
    // repository rather than vendored here. Run it there; nothing to compare
    // against means nothing to fail.
    if !dir.is_dir() {
        return;
    }
    let mut checked = 0;
    let mut diverged: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("bench_assets present") {
        let p = entry.unwrap().path();
        if !p.is_file() {
            continue;
        }
        let raw = std::fs::read(&p).unwrap();
        let text = String::from_utf8_lossy(&raw);
        let (got, want) = (maki_rstring::tokens::count(&text), reference(&text));
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        checked += 1;
        if got == want {
            continue;
        }
        if KNOWN_DIVERGENT_FILES.contains(&name.as_str()) {
            assert!(
                within_envelope(got, want),
                "pinned divergence [{name}] widened: tokie {got} vs tiktoken {want}"
            );
            eprintln!("known divergence [{name}]: tokie {got} vs tiktoken {want}");
            continue;
        }
        diverged.push(format!("{name}: tokie {got} vs tiktoken {want}"));
    }
    assert!(
        checked >= 10,
        "expected the full corpus, got {checked} files"
    );
    if !diverged.is_empty() {
        panic!(
            "new divergence(s) on {} file(s):\n{}",
            diverged.len(),
            diverged.join("\n")
        );
    }
}
