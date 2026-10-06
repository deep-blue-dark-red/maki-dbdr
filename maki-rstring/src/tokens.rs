//! o200k token counting (the pricing unit for every gate).
//!
//! Backed by tokie's SIMD pretokenizer + backtracking BPE over a
//! HuggingFace `tokenizer.json`: the same 200000-entry vocab and the same
//! two special tokens as `tiktoken-rs::o200k_base()`. tokie is a
//! reimplementation, so exactness is *gated*, not assumed —
//! `tests/tokens_parity.rs` is a divergence tripwire against tiktoken-rs
//! (the oracle every figure in bench/RESULTS.md is scored in) with the
//! currently-disagreeing inputs pinned in `KNOWN_DIVERGENT_CASES`.
//!
//! tokie builds and runs on stable Rust (gigatoken, the previous backend,
//! needed nightly `portable_simd`). The tokenizer file is deliberately not
//! vendored — it is resolved from `$RSTRING_O200K` or
//! `~/.cache/rstring/o200k_tokenizer.json` (`bench/reproduce.sh` provisions
//! it), and a compiled `.tkz` is written beside it on first load so later
//! runs deserialize in ~17 ms instead of parsing the 9.7 MB JSON (~410 ms).
//! The sidecar's name carries the pretokenizer it was built with
//! (`o200k_tokenizer.o200k.tkz`), because a compiled tokenizer bakes its
//! scheme in — reusing one across `RSTRING_PRETOK` values would count in the
//! wrong id space without a word of complaint.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokie::{hf, PretokType, Tokenizer};

/// Pretokenizer scheme: o200k by default; `RSTRING_PRETOK=qwen2` pairs with
/// a Qwen `tokenizer.json` so counts and thin-table scores live in the
/// deployed model's id space. tokie has no Qwen2 scheme — Qwen1–3's
/// ByteLevel split is GPT-2's, so that name maps to `Gpt2`.
fn pretok() -> PretokType {
    match std::env::var("RSTRING_PRETOK").as_deref() {
        Ok("qwen2") => PretokType::Gpt2,
        Ok("cl100k") => PretokType::Cl100k,
        _ => PretokType::O200k,
    }
}

pub fn pretok_name() -> &'static str {
    match pretok() {
        PretokType::Gpt2 => "qwen2",
        PretokType::Cl100k => "cl100k",
        _ => "o200k",
    }
}

/// The configured tokenizer file — a `tokenizer.json`, or a compiled
/// `.tkz` when `RSTRING_O200K` names one. The thin table fingerprints on
/// its bytes, so it must be the file the user points at, not the sidecar.
pub fn vocab_path() -> PathBuf {
    if let Ok(p) = std::env::var("RSTRING_O200K") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cache/rstring/o200k_tokenizer.json")
}

/// Compiled sidecar for `src`: same stem, this process's pretokenizer in the
/// middle, `.tkz` extension. Keyed by scheme so switching `RSTRING_PRETOK`
/// cannot pick up another one's compiled form.
fn tkz_path(src: &Path) -> PathBuf {
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tokenizer");
    src.with_file_name(format!("{stem}.{}.tkz", pretok_name()))
}

/// Load a compiled tokenizer, refusing one built for another pretokenizer.
fn compiled(p: &Path) -> Option<Tokenizer> {
    let tok = Tokenizer::from_file(p).ok()?;
    (tok.pretokenizer_type() == pretok()).then_some(tok)
}

fn load() -> Tokenizer {
    let src = vocab_path();
    let sidecar = tkz_path(&src);

    if src.extension().and_then(|e| e.to_str()) == Some("tkz") {
        return compiled(&src).unwrap_or_else(|| {
            panic!(
                "compiled tokenizer at {} is not a {} tokenizer — point RSTRING_O200K at the matching .tkz, or re-provision with bench/reproduce.sh",
                src.display(),
                pretok_name()
            )
        });
    }

    // A compiled sidecar newer than its source skips the ~410 ms JSON parse
    // for a ~17 ms deserialize; an older one is rebuilt below.
    if src.is_file() && !older(&sidecar, &src) {
        if let Some(tok) = compiled(&sidecar) {
            return tok;
        }
    }

    if src.is_file() {
        let tok = hf::from_json_with_pretokenizer(&src, pretok()).unwrap_or_else(|e| {
            panic!(
                "tokenizer unavailable at {}: {} — set RSTRING_O200K or run bench/reproduce.sh to provision ~/.cache/rstring/",
                src.display(),
                e
            )
        });
        let _ = tok.to_file(&sidecar); // best effort: read-only caches just stay slow
        return tok;
    }

    if sidecar.is_file() {
        return compiled(&sidecar).unwrap_or_else(|| {
            panic!(
                "compiled tokenizer at {} does not load as {} — delete it to force a rebuild",
                sidecar.display(),
                pretok_name()
            )
        });
    }

    panic!(
        "tokenizer unavailable at {} — set RSTRING_O200K or run bench/reproduce.sh to provision ~/.cache/rstring/",
        src.display()
    );
}

/// Missing or stale sidecars must not shadow their source tokenizer.
fn older(sidecar: &Path, src: &Path) -> bool {
    match (
        std::fs::metadata(sidecar).and_then(|m| m.modified()),
        std::fs::metadata(src).and_then(|m| m.modified()),
    ) {
        (Ok(a), Ok(b)) => a < b,
        _ => true,
    }
}

static TOK: OnceLock<Tokenizer> = OnceLock::new();

/// Shared tokenizer. tokie's `Tokenizer` has no interior mutability on the
/// encode path, so callers borrow it directly instead of locking.
pub fn tok() -> &'static Tokenizer {
    TOK.get_or_init(load)
}

pub fn vocab_size() -> u32 {
    tok().vocab_size() as u32
}

/// Token ids for one string — the thin table's counting unit. Added/special
/// tokens are matched in the text (like tiktoken's allow-all), never
/// appended, matching `encode_with_special_tokens` semantics.
pub fn ids(s: &str) -> Vec<u32> {
    tok().encode_ids(s, false)
}

pub fn count(s: &str) -> usize {
    tok().count_tokens(s)
}

/// Whether the tokenizer has been constructed in this process. Constructing
/// it costs ~410 ms cold (JSON parse + matcher build) or ~17 ms from a
/// compiled `.tkz`, so callers that only *report* counts — the compress
/// CLI's stderr summary — skip themselves on cold processes instead of
/// forcing an init the compression didn't need (the stream tier never
/// counts tokens).
pub fn warm() -> bool {
    TOK.get().is_some()
}
