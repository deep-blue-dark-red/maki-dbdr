//! Volatile-vs-load-bearing classification. The core safety rule:
//! two lines may merge **iff** their masked keys are equal — i.e. they differ
//! only in volatile values (timestamps, UUIDs, temp paths). Semver, error
//! codes, repo paths and identifiers are load-bearing and never masked.
//!
//! Also hosts the SWAR entropy-run scanner: a run of >= [`ENTROPY_RUN_MIN`]
//! base64/hex-alphabet bytes anywhere in a line is bulk entropy (PEM bodies,
//! digests, tokens, random blobs) and is elided to an `<r:hash n>` stub in
//! the side table — recoverable via `rstring expand <hash>`.

use crate::side::SideTable;
use std::borrow::Cow;
use std::fmt::Write as _;

/// A run of at least this many token-alphabet bytes is entropy noise: 48 hex
/// chars is 192 bits, 48 base64 chars is 288 — far above any identifier.
/// UUIDs (32) and git SHAs (40) stay under it and remain verbatim.
pub const ENTROPY_RUN_MIN: usize = 48;

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

/// Mask every run of >= [`ENTROPY_RUN_MIN`] token bytes, wherever it sits in
/// the line, with an `<r:hash n>` side-table stub. SWAR scan: 8 bytes
/// classified per u64, branchless; borrows when nothing is masked.
pub fn mask_entropy<'a>(line: &'a str, table: &mut SideTable) -> Cow<'a, str> {
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
                flush_run(&mut out, &mut copied, line, table, run_start, run_len);
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
            flush_run(&mut out, &mut copied, line, table, run_start, run_len);
            run_len = 0;
        }
    }
    flush_run(&mut out, &mut copied, line, table, run_start, run_len);

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
    start: usize,
    len: usize,
) {
    if len < ENTROPY_RUN_MIN {
        return;
    }
    let s = out.get_or_insert_with(|| String::with_capacity(line.len()));
    s.push_str(&line[*copied..start]);
    let hash = table.put(&line[start..start + len]);
    let _ = write!(s, "<r:{} n={}>", hash, len);
    *copied = start + len;
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
/// Returns (end, class) where class is "TS" or "ID".
fn volatile_at(b: &[u8], from: usize) -> Option<(usize, &'static str)> {
    // ISO timestamp: 2026-09-29T10:22:02.831781Z (date part required)
    if from + 10 <= b.len()
        && b[from + 4] == b'-'
        && b[from + 7] == b'-'
        && b[from..from + 4].iter().all(u8::is_ascii_digit)
        && b[from + 5..from + 7].iter().all(u8::is_ascii_digit)
        && b[from + 8..from + 10].iter().all(u8::is_ascii_digit)
    {
        let mut j = from + 10;
        if j < b.len() && (b[j] == b'T' || b[j] == b' ') {
            j += 1;
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
        return Some((from + 10, "TS"));
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
        if ok {
            return Some((from + 36, "ID"));
        }
    }
    // long digit run (epoch seconds/millis/nanos) — but NOT semver-like x.y.z
    if b[from].is_ascii_digit() {
        let mut j = from;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j - from >= 9 && (j >= b.len() || b[j] != b'.') {
            return Some((j, "TS"));
        }
    }
    None
}

const TEMP_MARKERS: &[&str] = &[
    "/tmp/",
    "/private/tmp/",
    "/var/folders/",
    "Library/Caches",
    "/Temp/",
    "\\Temp\\",
    "/dev/null",
    "/proc/",
    "/sys/",
];

/// True for path-shaped runs that live in temp/cache locations (volatile).
fn temp_path_at(b: &[u8], from: usize) -> Option<usize> {
    let s = std::str::from_utf8(&b[from..]).ok()?;
    // find the end of the path-ish run
    let end = s
        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ')' || c == ',')
        .unwrap_or(s.len());
    let path = &s[..end];
    if path.contains('/') && TEMP_MARKERS.iter().any(|m| path.contains(m)) {
        Some(from + end)
    } else {
        None
    }
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
