//! Volatile-vs-load-bearing classification. The core safety rule:
//! two lines may merge **iff** their masked keys are equal — i.e. they differ
//! only in volatile values (timestamps, UUIDs, temp paths). Semver, error
//! codes, repo paths and identifiers are load-bearing and never masked.
//!
//! Also hosts the SWAR entropy-run scanner: a run of >= [`ENTROPY_RUN_MIN`]
//! base64/hex-alphabet bytes that also *reads* random ([`looks_random`]) is
//! bulk entropy (PEM bodies, digests, tokens, random blobs) and is elided —
//! to an `<r:hash n>` side-table stub ([`Elide::Side`], recoverable via
//! `rstring expand <hash>`) or, for one-shot callers that drop the table, to
//! a plain `[masked n=<len>]` ([`Elide::Inline`], longer runs only). Long
//! paths, identifiers, test names and separator rules share the alphabet but
//! not the shape, and stay verbatim.

use crate::side::SideTable;
use std::borrow::Cow;
use std::fmt::Write as _;

/// A run of at least this many token-alphabet bytes is entropy noise: 48 hex
/// chars is 192 bits, 48 base64 chars is 288 — far above any identifier.
/// UUIDs (32) and git SHAs (40) stay under it and remain verbatim.
pub const ENTROPY_RUN_MIN: usize = 48;

/// [`Elide::Inline`] floor. Nothing behind the marker can be recovered, so
/// only bodies go: hex sha512 (128) and anything shorter is a value a model
/// may need to quote back; JWTs, keys and data URIs run longer.
pub const INLINE_RUN_MIN: usize = 160;

/// Where an elided entropy run goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elide {
    /// `<r:hash n=len>` backed by the side table (`rstring expand`).
    Side,
    /// `[masked n=len]`; the caller drops the table, so no hash is shown
    /// (a hash nobody can expand is one more random string in the context).
    Inline,
}

/// Minimum class-switch rate (adjacent alnum pairs changing between lower /
/// upper / digit) for a mixed-case run to count as random. Uniform base64
/// switches ~65% of the time; CamelCase and snake_case identifiers, even
/// digit-laden ones like `Tile4x8_Unroll2`, stay near 30%.
const SWITCH_MIN: f64 = 0.45;
/// A hex run needs this many contiguous hex digits (with both digits and
/// letters among them) to read as a digest rather than a word.
const HEX_SPAN_MIN: usize = 32;
const HIGH: u64 = 0x8080_8080_8080_8080;
const LO7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
const ONES: u64 = 0x0101_0101_0101_0101;

fn splat(c: u8) -> u64 {
    ONES * u64::from(c)
}

/// Per-lane `x >= y` over 7-bit lanes; the guard bit marks true lanes.
/// Lane arithmetic can't carry: `x + 0x80 - y` stays in `1..=255`.
fn ge7(x: u64, y: u64) -> u64 {
    x.wrapping_add(HIGH).wrapping_sub(y) & HIGH
}

/// Per-lane `lo <= byte <= hi`, guard bit set where true.
fn in_range(w: u64, lo: u8, hi: u8) -> u64 {
    ge7(w, splat(lo)) & !ge7(w, splat(hi + 1))
}

/// Guard-bit mask of base64/hex-alphabet bytes: `A-Za-z0-9+-/=_`.
/// Non-ASCII lanes are forced clear so UTF-8 breaks runs instead of
/// aliasing into it through the 7-bit truncation.
fn token_mask(raw: u64) -> u64 {
    let w = raw & LO7;
    (in_range(w, b'0', b'9')
        | in_range(w, b'A', b'Z')
        | in_range(w, b'a', b'z')
        | in_range(w, b'+', b'+')
        | in_range(w, b'-', b'-')
        | in_range(w, b'/', b'/')
        | in_range(w, b'=', b'=')
        | in_range(w, b'_', b'_'))
        & !(raw & HIGH)
}

fn is_token_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'/' | b'=' | b'_')
}

/// Mask every random-looking run of >= [`ENTROPY_RUN_MIN`] token bytes,
/// wherever it sits in the line, with an `<r:hash n>` side-table stub.
pub fn mask_entropy<'a>(line: &'a str, table: &mut SideTable) -> Cow<'a, str> {
    mask_entropy_with(line, table, Elide::Side)
}

/// [`mask_entropy`] with the elision target chosen. SWAR scan: 8 bytes
/// classified per u64, branchless; borrows when nothing is masked.
pub fn mask_entropy_with<'a>(line: &'a str, table: &mut SideTable, elide: Elide) -> Cow<'a, str> {
    let b = line.as_bytes();
    let (chunks, tail) = b.as_chunks::<8>();
    let mut copied = 0;
    let mut out: Option<String> = None;
    let mut run_start = 0;
    let mut run_len = 0;

    for (n, chunk) in chunks.iter().enumerate() {
        let w = u64::from_le_bytes(*chunk);
        let mask = token_mask(w);
        if mask == HIGH {
            if run_len == 0 {
                run_start = n * 8;
            }
            run_len += 8;
            continue;
        }
        for j in 0..8 {
            if mask & (0x80 << (8 * j)) != 0 {
                if run_len == 0 {
                    run_start = n * 8 + j;
                }
                run_len += 1;
            } else {
                flush_run(&mut out, &mut copied, line, table, elide, run_start, run_len);
                run_len = 0;
            }
        }
    }
    for (i, &c) in tail.iter().enumerate() {
        if is_token_byte(c) {
            if run_len == 0 {
                run_start = chunks.len() * 8 + i;
            }
            run_len += 1;
        } else {
            flush_run(&mut out, &mut copied, line, table, elide, run_start, run_len);
            run_len = 0;
        }
    }
    flush_run(&mut out, &mut copied, line, table, elide, run_start, run_len);

    match out {
        Some(mut s) => {
            s.push_str(&line[copied..]);
            Cow::Owned(s)
        }
        None => Cow::Borrowed(line),
    }
}

/// `copied` trails the last emitted byte; run boundaries are ASCII positions,
/// so `&line[a..b]` slicing is always char-boundary safe.
fn flush_run(
    out: &mut Option<String>,
    copied: &mut usize,
    line: &str,
    table: &mut SideTable,
    elide: Elide,
    start: usize,
    len: usize,
) {
    let min = match elide {
        Elide::Side => ENTROPY_RUN_MIN,
        Elide::Inline => INLINE_RUN_MIN,
    };
    let run = &line[start..start + len];
    if len < min || !looks_random(run.as_bytes()) {
        return;
    }
    let s = out.get_or_insert_with(|| String::with_capacity(line.len()));
    s.push_str(&line[*copied..start]);
    match elide {
        Elide::Side => {
            let hash = table.put(run);
            let _ = write!(s, "<r:{} n={}>", hash, len);
        }
        Elide::Inline => {
            let _ = write!(s, "[masked n={}]", len);
        }
    }
    *copied = start + len;
}

/// True when a token-alphabet run is shaped like random bytes rather than
/// text: a hex digest (a [`HEX_SPAN_MIN`]+ span of hex digits mixing digits
/// and letters), or mixed-case alphanumerics with digits whose character
/// class flips at least [`SWITCH_MIN`] of the time. Paths, identifiers,
/// `====` rules and digit-only numbers all fail both tests.
pub fn looks_random(run: &[u8]) -> bool {
    let (mut span, mut digits, mut letters) = (0usize, false, false);
    for &c in run {
        if c.is_ascii_hexdigit() {
            span += 1;
            digits |= c.is_ascii_digit();
            letters |= c.is_ascii_alphabetic();
            if span >= HEX_SPAN_MIN && digits && letters {
                return true;
            }
        } else {
            (span, digits, letters) = (0, false, false);
        }
    }
    let class = |c: u8| match c {
        b'a'..=b'z' => 1u8,
        b'A'..=b'Z' => 2,
        b'0'..=b'9' => 3,
        _ => 0,
    };
    let mut seen = [false; 4];
    let (mut pairs, mut switches) = (0usize, 0usize);
    for w in run.windows(2) {
        let (a, b) = (class(w[0]), class(w[1]));
        seen[a as usize] = true;
        seen[b as usize] = true;
        if a != 0 && b != 0 {
            pairs += 1;
            switches += (a != b) as usize;
        }
    }
    seen[1] && seen[2] && seen[3] && pairs > 0 && switches as f64 >= SWITCH_MIN * pairs as f64
}

/// Lines that look like source code: indented, or ending in `;`, `{` or `}`.
/// They pass through clustering verbatim — never masked, never merged —
/// because a model quotes them back into edits. A line opening with `{` is a
/// JSON record (a log line, the bulk worth folding), not code.
pub fn looks_like_code(line: &str) -> bool {
    let t = line.trim_end();
    if t.starts_with('{') {
        return false;
    }
    line.starts_with("  ") || line.starts_with('\t') || t.ends_with([';', '{', '}'])
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn is_hex_run(s: &[u8]) -> bool {
    s.iter().all(|c| c.is_ascii_hexdigit())
}

/// Byte-span of a volatile token starting at `from`, if any.
/// Returns (end, class) where class is "TS" or "ID". Tokens only start at a
/// word boundary, so digits inside identifiers and digests never match.
fn volatile_at(b: &[u8], from: usize) -> Option<(usize, &'static str)> {
    if from > 0 && b[from - 1].is_ascii_alphanumeric() {
        return None;
    }
    // ISO timestamp: 2026-09-29T10:22:02.831781Z. The time part is required:
    // a bare date is a name (`backup-2026-11-01.tar`), not a clock reading.
    if from + 16 <= b.len()
        && b[from + 4] == b'-'
        && b[from + 7] == b'-'
        && b[from..from + 4].iter().all(u8::is_ascii_digit)
        && b[from + 5..from + 7].iter().all(u8::is_ascii_digit)
        && b[from + 8..from + 10].iter().all(u8::is_ascii_digit)
        && (b[from + 10] == b'T' || b[from + 10] == b' ')
        && b[from + 11..from + 13].iter().all(u8::is_ascii_digit)
        && b[from + 13] == b':'
        && b[from + 14..from + 16].iter().all(u8::is_ascii_digit)
    {
        let mut j = from + 16;
        while j < b.len()
            && (b[j].is_ascii_digit()
                || b[j] == b':'
                || b[j] == b'.'
                || b[j] == b'Z'
                || b[j] == b'+'
                || b[j] == b'-')
        {
            j += 1;
        }
        return Some((j, "TS"));
    }
    // UUID: 8-4-4-4-12 hex
    if from + 36 <= b.len() {
        let lens = [8, 4, 4, 4, 12];
        let mut p = from;
        let mut ok = true;
        for (k, l) in lens.iter().enumerate() {
            if !is_hex_run(&b[p..p + l]) {
                ok = false;
                break;
            }
            p += l;
            if k < 4 && (p >= b.len() || b[p] != b'-') {
                ok = false;
                break;
            }
            p += 1;
        }
        if ok && (from + 36 == b.len() || !b[from + 36].is_ascii_alphanumeric()) {
            return Some((from + 36, "ID"));
        }
    }
    epoch_at(b, from).map(|end| (end, "TS"))
}

/// Epoch seconds 2008..2039 (`1.2e9..2.2e9`) at second, milli, micro or nano
/// precision, optionally with a `.fraction` (`time.time()`). Any other long
/// number — byte counts, rss, offsets — is load-bearing and stays put.
fn epoch_at(b: &[u8], from: usize) -> Option<usize> {
    let mut j = from;
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    if !matches!(j - from, 10 | 13 | 16 | 19) {
        return None;
    }
    let secs: u64 = std::str::from_utf8(&b[from..from + 10]).ok()?.parse().ok()?;
    if !(1_200_000_000..2_200_000_000).contains(&secs) {
        return None;
    }
    if j - from == 10 && j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
        j += 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
    }
    if j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.') {
        return None;
    }
    Some(j)
}

/// Markers a path must start with to be temp/cache (volatile).
const TEMP_ROOTS: &[&str] = &[
    "/tmp/",
    "/private/tmp/",
    "/private/var/folders/",
    "/var/folders/",
    "/proc/",
];
/// Markers that may sit anywhere inside an absolute path.
const TEMP_INNER: &[&str] = &["/Library/Caches/", "/Temp/", "\\Temp\\", "/.cache/"];

/// True for path-shaped runs that live in temp/cache locations (volatile).
/// The path must start at a boundary and be absolute (`/`, `~` or `C:\`),
/// so a repo's own `build/tmp/` never reads as temp.
fn temp_path_at(b: &[u8], from: usize) -> Option<usize> {
    if from > 0 && !matches!(b[from - 1], b' ' | b'\t' | b'"' | b'\'' | b'=' | b'(' | b'[' | b':' | b',') {
        return None;
    }
    let absolute = match b[from] {
        b'/' | b'~' => true,
        c => c.is_ascii_alphabetic() && b.get(from + 1) == Some(&b':') && b.get(from + 2) == Some(&b'\\'),
    };
    if !absolute {
        return None;
    }
    let s = std::str::from_utf8(&b[from..]).ok()?;
    let end = s
        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ')' || c == ',')
        .unwrap_or(s.len());
    let path = &s[..end];
    let temp = TEMP_ROOTS.iter().any(|m| path.starts_with(m))
        || TEMP_INNER.iter().any(|m| path.contains(m));
    temp.then_some(from + end)
}

/// Canonical merge key: volatile values replaced by class markers.
/// Lines with equal keys differ ONLY in volatile values — merging is lossless
/// w.r.t. load-bearing content (originals remain recoverable from the side table).
pub fn merge_key(line: &str) -> String {
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < b.len() {
        if let Some((end, class)) = volatile_at(b, i) {
            out.push_str(if class == "TS" { "<TS>" } else { "<ID>" });
            i = end;
        } else if let Some(end) = temp_path_at(b, i) {
            out.push_str("<TMP>");
            i = end;
        } else {
            // copy one utf-8 char
            let ch_len = utf8_len(b[i]);
            out.push_str(&String::from_utf8_lossy(&b[i..i + ch_len]));
            i += ch_len;
        }
    }
    out
}

fn utf8_len(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte >> 5 == 0b110 {
        2
    } else if byte >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swar_token_mask_matches_scalar_predicate_all_bytes_all_lanes() {
        for c in 0u8..=255 {
            let mask = token_mask(splat(c));
            for lane in 0..8 {
                assert_eq!(
                    mask & (0x80 << (8 * lane)) != 0,
                    is_token_byte(c),
                    "byte {c:#04x} lane {lane}"
                );
            }
        }
    }

    #[test]
    fn mixed_width_words_classify_per_lane() {
        let w = u64::from_le_bytes(*b"aG9 =/-_");
        let mask = token_mask(w);
        for (lane, expected) in b"aG9 =/-_".iter().enumerate() {
            assert_eq!(
                mask & (0x80 << (8 * lane)) != 0,
                is_token_byte(*expected),
                "lane {lane} byte {expected:#04x}"
            );
        }
    }

    #[test]
    fn clean_line_borrows_without_allocating() {
        let mut table = SideTable::default();
        assert!(matches!(
            mask_entropy("plain log line, 123 [ok] (done)", &mut table),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn entropy_run_lands_in_side_table() {
        let mut table = SideTable::default();
        let sha256_hex = "0123456789abcdef".repeat(4);
        let line = format!("sha256 {sha256_hex} file.tgz");
        let out = mask_entropy(&line, &mut table);
        let s = out.as_ref();
        assert!(s.starts_with("sha256 <r:"), "{s}");
        assert!(s.ends_with(" file.tgz"), "{s}");
        let hash = s.split("<r:").nth(1).unwrap().split(' ').next().unwrap();
        assert_eq!(
            table.get(hash).map(String::as_str),
            Some(sha256_hex.as_str())
        );
    }
}
