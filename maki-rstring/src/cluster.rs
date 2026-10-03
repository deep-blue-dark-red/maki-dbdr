//! Order-preserving merge-iff-volatile clustering: lines with equal
//! [`mask::merge_key`] collapse to the first occurrence + `[xN]` count.
//! By construction this never merges lines that differ in load-bearing text,
//! and code-looking lines never merge at all: dropping lines from a listing
//! loses structure no marker can restore.

use crate::mask;
use crate::side::SideTable;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

pub fn run(text: &str, table: &mut SideTable) -> String {
    let mut out = String::with_capacity(text.len());
    // key -> count, insertion order tracked separately. Mergeable first
    // occurrences land in `order`; code lines are never mergeable.
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<(String, bool)> = Vec::new(); // (line, mergeable)
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if code_like(line) {
            order.push((line.to_string(), false));
            continue;
        }
        let masked = mask::mask_entropy(line, table);
        match counts.entry(mask::merge_key(&masked)) {
            Entry::Vacant(slot) => {
                slot.insert(1);
                order.push((masked.into_owned(), true));
            }
            Entry::Occupied(mut slot) => *slot.get_mut() += 1,
        }
    }
    for (line, mergeable) in order {
        out.push_str(&line);
        if mergeable {
            let n = counts[&mask::merge_key(&line)];
            if n > 1 {
                out.push_str(&format!(" [x{n}]"));
            }
        }
        out.push('\n');
    }
    out
}

/// Indented lines and `;`/`{`/`}`-terminated lines read as source code:
/// near-identical code lines (epoch-sized literals, ids) would otherwise
/// collapse, and dropped lines are unrecoverable. Checked before entropy
/// masking so code passes through verbatim.
fn code_like(line: &str) -> bool {
    line.starts_with([' ', '\t']) || line.ends_with([';', '{', '}'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indented_code_lines_stay_verbatim() {
        let a = "    let seed = 1735689600;";
        let b = "    let seed = 1735689601;";
        let out = run(&format!("{a}\n{b}\n"), &mut SideTable::default());
        assert!(out.contains(a) && out.contains(b), "{out}");
        assert!(!out.contains("[x"), "{out}");
    }

    #[test]
    fn duplicate_code_lines_stay_verbatim() {
        let out = run("    body();\n    body();\n", &mut SideTable::default());
        assert_eq!(out.matches("    body();").count(), 2, "{out}");
        assert!(!out.contains("[x"), "{out}");
    }

    #[test]
    fn code_lines_never_entropy_masked() {
        let mut table = SideTable::default();
        let hex = "a".repeat(64);
        let line = format!("    data = \"{hex}\";");
        let out = run(&format!("{line}\n"), &mut table);
        assert!(out.contains(&line), "{out}");
        assert!(!out.contains("<r:"), "{out}");
    }

    #[test]
    fn log_lines_still_merge() {
        let a = "2026-09-29T10:22:02Z level=WARN provider=anthropic";
        let b = "2026-09-29T18:44:59Z level=WARN provider=anthropic";
        let out = run(&format!("{a}\n{b}\n"), &mut SideTable::default());
        assert!(
            out.matches("provider=anthropic").count() == 1 && out.contains(" [x2]"),
            "{out}"
        );
    }
}
