//! Order-preserving merge-iff-volatile clustering: lines with equal
//! [`mask::merge_key`] collapse to the first occurrence + `[xN]` count.
//! By construction this never merges lines that differ in load-bearing text.

use crate::mask;
use std::collections::HashMap;

pub fn run(text: &str, table: &mut crate::side::SideTable) -> String {
    let mut out = String::with_capacity(text.len());
    // key -> (first occurrence line, count). Insertion order preserved by Vec.
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<(String, usize)> = Vec::new(); // (line, key slot)
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let masked = mask::mask_entropy(line, table);
        let key = mask::merge_key(&masked);
        match counts.get_mut(&key) {
            Some(n) => *n += 1,
            None => {
                counts.insert(key.clone(), 1);
                order.push((masked.into_owned(), order.len()));
            }
        }
    }
    for (line, _) in order {
        let key = mask::merge_key(&line);
        let n = counts[&key];
        out.push_str(&line);
        if n > 1 {
            out.push_str(&format!(" [x{}]", n));
        }
        out.push('\n');
    }
    out
}
