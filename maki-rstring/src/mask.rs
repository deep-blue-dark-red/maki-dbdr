//! Volatile-vs-load-bearing classification. The core safety rule:
//! two lines may merge **iff** their masked keys are equal — i.e. they differ
//! only in volatile values (timestamps, UUIDs, temp paths). Semver, error
//! codes, repo paths and identifiers are load-bearing and never masked.

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
