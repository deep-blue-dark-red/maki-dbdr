//! Bigram-surprisal word drop (deterministic semantic tier). Drops the most
//! predictable words on prose lines; parse-verified structured lines and
//! anchors (digits, code ticks, sentence starts/ends) are never dropped.

use std::collections::HashMap;

pub(crate) fn structured(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return true;
    }
    if serde_json::from_str::<serde_json::Value>(line).is_ok() {
        return true;
    }
    if t.starts_with(['{', '[', '<', '`', '|', '-', '*', '#', '"']) || t.starts_with("<rec") {
        return true;
    }
    let b = t.as_bytes();
    let mut hard = 0;
    for &c in b {
        if c.is_ascii_digit() || b"{}[]<>=:;,|/\\\"'()".contains(&c) {
            hard += 1;
        }
    }
    hard * 4 > b.len()
}

pub fn run(text: &str, keep_rate: f64) -> String {
    // N-gram counts over interned word ids: the (prev, cur) tuple packs into
    // one u64 key and unigrams sit in a dense vec, so counting and lookup
    // allocate nothing per token (vs two Strings per window before).
    let mut ids: HashMap<&str, u32> = HashMap::new();
    let mut bigram: HashMap<u64, u32> = HashMap::new();
    let mut uni: Vec<u32> = Vec::new();
    let mut total = 0usize;
    let mut cur: Vec<u32> = Vec::new();
    for line in text.lines() {
        if structured(line) {
            continue;
        }
        cur.clear();
        for w in line.split(' ').filter(|w| !w.is_empty()) {
            let next = ids.len() as u32;
            cur.push(*ids.entry(w).or_insert(next));
        }
        uni.resize(ids.len(), 0);
        for p in cur.windows(2) {
            *bigram.entry((p[0] as u64) << 32 | p[1] as u64).or_insert(0) += 1;
            uni[p[1] as usize] += 1;
            total += 1;
        }
    }
    let v = total.max(1) as f64;
    let mut out = String::new();
    for line in text.lines() {
        if structured(line) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let ws: Vec<&str> = line.split(' ').filter(|w| !w.is_empty()).collect();
        let n_drop = ((ws.len() as f64 * (1.0 - keep_rate)) + 0.5) as usize;
        if n_drop == 0 || ws.len() < 4 {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        cur.clear();
        cur.extend(ws.iter().map(|w| ids.get(*w).copied().unwrap_or(u32::MAX)));
        let surp = |i: usize| -> f64 {
            let c = bigram
                .get(&((cur[i - 1] as u64) << 32 | cur[i] as u64))
                .copied()
                .unwrap_or(0) as f64
                + 1.0;
            let u = uni.get(cur[i] as usize).copied().unwrap_or(0) as f64 + v * 1e-4;
            -(c / u).ln()
        };
        // Score each candidate once, then stable-sort: ties keep ascending
        // index order, matching a stable sort on surp(i) itself.
        let mut idxs: Vec<(f64, usize)> = (1..ws.len())
            .filter(|&i| {
                let w = ws[i];
                !(i + 1 == ws.len()
                    || w.ends_with(['.', '?', ':', ';'])
                    || w.chars()
                        .any(|c| c.is_ascii_digit() || c == '`' || c == '_')
                    || w.chars().all(|c| c.is_ascii_punctuation()))
            })
            .map(|i| (surp(i), i))
            .collect();
        idxs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut drop = vec![false; ws.len()];
        for &(_, i) in idxs.iter().take(n_drop) {
            drop[i] = true;
        }
        let mut first = true;
        for (i, w) in ws.iter().enumerate() {
            if drop[i] {
                continue;
            }
            if !first {
                out.push(' ');
            }
            first = false;
            out.push_str(w);
        }
        out.push('\n');
    }
    out
}
