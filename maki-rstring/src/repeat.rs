//! Lossless repeated-span factoring for the stream tier.
//!
//! Spans that recur across lines — the same path on every grep hit, a fixed
//! log prefix — are hoisted into a lead legend and replaced inline by `<n>`
//! refs (`<0> /Users/me/src/lower.zig` + `<0>-138- ...`). The output stays
//! readable and recovers under one-pass legend substitution; unlike
//! [`crate::thin`] nothing is dropped and no side table is involved.
//!
//! Candidates are the longest common prefixes of adjacent *sorted* lines,
//! plus adjacent sorted whitespace spans under [`SPAN_CAP`] — no suffix
//! array and no dependencies. Selection is byte-scored on purpose: a
//! token-exact budget would force tokenizer init on every cold stream run
//! (~410 ms from tokenizer.json, ~17 ms even from the compiled `.tkz`),
//! which blows the stream tier's latency envelope. Per-run candidate scanning is bounded by
//! [`SCAN_BUDGET`], so the pass stays linear-ish at any input size.
//!
//! Refs are `<n>` (digit-bearing, so [`crate::thin`]'s guards already keep
//! them). Candidates containing `<` + digit are rejected, which makes it
//! impossible for a replacement to alias an earlier ref: a bare ref token is
//! 5 bytes at most, and candidates are ≥ [`MIN_CAND`] bytes.

const MIN_CAND: usize = 12; // shorter repeats can't pay for a ref
const MAX_CAND: usize = 1024; // cap: useful repeats are prefixes, not bodies
const MIN_NET: isize = 8; // net bytes required before a ref is kept
const SCAN_BUDGET: usize = 24 << 20; // per-run candidate-scan byte budget
const MAX_CANDS: usize = 64;
const PRE_RANK: usize = 256; // candidates counted before the true ranking
const COUNT_BUDGET: usize = 1 << 16; // sorted-array visits per run
const SPAN_CAP: usize = 1 << 20; // span-pass size cap (sort cost)

/// Factor repeated spans of `text` into a legend. Returns `text` unchanged
/// when nothing pays; a text already carrying a legend is a fixed point.
pub fn run(text: &str) -> String {
    if text.is_empty() || factored(text) {
        return text.to_string();
    }
    let cands = candidates(text);
    let k = (SCAN_BUDGET / text.len().max(1)).clamp(8, MAX_CANDS);

    let mut work = text.to_string();
    let mut legend: Vec<&str> = Vec::new();
    for (c, _) in cands.iter().take(k) {
        let occ = work.matches(c.as_str()).count();
        if occ < 2 {
            continue;
        }
        let ref_ = format!("<{}>", legend.len());
        let net = (occ * (c.len() - ref_.len())) as isize - (ref_.len() + 1 + c.len() + 1) as isize;
        if net <= MIN_NET {
            continue;
        }
        work = work.replace(c.as_str(), &ref_);
        legend.push(c.as_str());
    }
    if legend.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(work.len() + 2 * legend.len() + 64);
    for (i, c) in legend.iter().enumerate() {
        out.push_str(&format!("<{}> {}\n", i, c));
    }
    out.push_str(&work);
    out
}

/// True when the first line already is a `<n> value` legend entry.
fn factored(text: &str) -> bool {
    let line = text.split('\n').next().unwrap_or("");
    let b = line.as_bytes();
    if b.first() != Some(&b'<') {
        return false;
    }
    let mut i = 1;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    i > 1 && b.get(i) == Some(&b'>')
}

/// Sorted-adjacent prefixes of lines (always) and whitespace spans (small
/// inputs only — sorting every span of a huge log is the one superlinear
/// step), ranked by estimated byte saving.
fn candidates(text: &str) -> Vec<(String, usize)> {
    let mut lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines.sort_by(|a, b| key(a).cmp(key(b)).then_with(|| a.len().cmp(&b.len())));
    let spans: Vec<&str> = if text.len() <= SPAN_CAP {
        let mut v: Vec<&str> = text.lines().flat_map(str::split_whitespace).collect();
        v.sort_by(|a, b| key(a).cmp(key(b)).then_with(|| a.len().cmp(&b.len())));
        v.dedup();
        v
    } else {
        Vec::new()
    };
    let mut raw: Vec<String> = Vec::new();
    push_lcps(&lines, &mut raw);
    push_lcps(&spans, &mut raw);
    raw.sort_unstable();
    raw.dedup();
    raw.retain(|c| !contains_ref(c) && !head_ref(text, c));
    // Longest first is a cheap prefilter for the count pass; the real ranking
    // uses the estimated saving, so a dominant short prefix (`/p/a-`) beats a
    // niche longer one (`/p/a-1`) instead of fragmenting it.
    raw.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    raw.truncate(PRE_RANK);
    let mut budget = COUNT_BUDGET;
    let mut out: Vec<(String, usize)> = Vec::with_capacity(raw.len());
    for c in raw {
        let mut n = pref_count(&lines, &c, &mut budget);
        if !spans.is_empty() {
            n += pref_count(&spans, &c, &mut budget);
        }
        out.push((c, n));
    }
    out.sort_by(|a, b| {
        estimate(&b.0, b.1)
            .cmp(&estimate(&a.0, a.1))
            .then_with(|| b.0.len().cmp(&a.0.len()))
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}

fn estimate(c: &str, count: usize) -> isize {
    count.saturating_sub(1) as isize * (c.len() as isize - 4) - (c.len() as isize + 6)
}

/// Number of sorted items starting with `c`; prefix ranges are contiguous.
fn pref_count(items: &[&str], c: &str, budget: &mut usize) -> usize {
    let start = items.partition_point(|s| key(s) < c.as_bytes());
    let mut n = 0;
    for s in &items[start..] {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        if s.starts_with(c) {
            n += 1;
        } else {
            break;
        }
    }
    n
}

fn push_lcps(items: &[&str], out: &mut Vec<String>) {
    for w in items.windows(2) {
        let mut k = common_prefix(w[0].as_bytes(), w[1].as_bytes()).min(MAX_CAND);
        while k > 0 && !w[0].is_char_boundary(k) {
            k -= 1;
        }
        if k >= MIN_CAND {
            out.push(w[0][..k].to_string());
        }
    }
}

/// Ordering key: the first [`MAX_CAND`] bytes, so sorts and prefix scans stay
/// cheap even when lines share multi-KB JSONL heads.
fn key(s: &str) -> &[u8] {
    let b = s.as_bytes();
    &b[..b.len().min(MAX_CAND)]
}

fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

/// Candidate text must never contain `<` + digit: refs are `<n>`, and this
/// exclusion keeps a candidate from matching across (or inside) one.
fn contains_ref(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(1)).any(|i| b[i] == b'<' && b[i + 1].is_ascii_digit())
}

/// A candidate that starts the text and is followed by a space would land a
/// ref at column 0 of the body as `<n> …`, which a one-pass reader cannot tell
/// apart from a legend line. Reject it so the body's first line stays literal.
fn head_ref(text: &str, cand: &str) -> bool {
    text.starts_with(cand) && text.as_bytes().get(cand.len()) == Some(&b' ')
}
