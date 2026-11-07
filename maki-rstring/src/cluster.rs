//! Order-preserving merge-iff-volatile clustering: lines with equal
//! [`mask::merge_key`] collapse to the first occurrence + `[xN]` count.
//! By construction this never merges lines that differ in load-bearing text.
//! Code-looking lines ([`mask::looks_like_code`]) keep their position and
//! bytes: never masked, never merged.

use crate::mask::{self, Elide};
use std::collections::HashMap;

pub fn run(text: &str, table: &mut crate::side::SideTable) -> String {
    run_with(text, table, Elide::Side)
}

pub fn run_with(text: &str, table: &mut crate::side::SideTable, elide: Elide) -> String {
    enum Entry {
        Verbatim(String),
        Merged(String, String), // (line, key)
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<Entry> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if mask::looks_like_code(line) {
            order.push(Entry::Verbatim(line.to_string()));
            continue;
        }
        let masked = mask::mask_entropy_with(line, table, elide);
        let key = mask::merge_key(&masked);
        match counts.get_mut(&key) {
            Some(n) => *n += 1,
            None => {
                counts.insert(key.clone(), 1);
                order.push(Entry::Merged(masked.into_owned(), key));
            }
        }
    }
    let mut out = String::with_capacity(text.len());
    for e in order {
        match e {
            Entry::Verbatim(line) => out.push_str(&line),
            Entry::Merged(line, key) => {
                out.push_str(&line);
                let n = counts[&key];
                if n > 1 {
                    out.push_str(&format!(" [x{}]", n));
                }
            }
        }
        out.push('\n');
    }
    out
}
