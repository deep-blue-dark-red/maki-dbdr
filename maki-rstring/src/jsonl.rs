//! JSONL columnar fast lane: if the input is a run of JSON objects with a
//! shared key signature, render one header + TSV rows (values volatile-masked).
//! JSON keys are the token-expensive part of JSONL; stating them once removes
//! that tax with zero information loss.

use crate::{cluster, mask};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn try_columnar(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 4 {
        return None;
    }
    let mut objects: Vec<BTreeMap<String, String>> = Vec::new();
    let mut parsed = 0usize;
    for l in &lines {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(l) {
            parsed += 1;
            if parsed * 4 < lines.len() * 3 {
                // still plausible; keep collecting
            }
            let flat: BTreeMap<String, String> = map
                .iter()
                .map(|(k, v)| {
                    let vs = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), vs)
                })
                .collect();
            objects.push(flat);
        }
    }
    if parsed * 4 < lines.len() * 3 {
        return None; // <75% JSONL — not this shape
    }
    // dominant key signature
    let mut sig_count: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    for o in &objects {
        let sig: Vec<String> = o.keys().cloned().collect();
        *sig_count.entry(sig).or_insert(0) += 1;
    }
    let (sig, n) = sig_count.into_iter().max_by_key(|(_, n)| *n)?;
    if n * 4 < objects.len() * 3 {
        return None; // no dominant uniform signature
    }
    let rows: Vec<&BTreeMap<String, String>> =
        objects.iter().filter(|o| o.len() == sig.len()).collect();
    let mut body = String::new();
    body.push_str(&format!("<rec {}>\n", sig.join(",")));
    for r in &rows {
        let vals: Vec<String> = sig
            .iter()
            .map(|k| {
                let v = r.get(k).map(String::as_str).unwrap_or("");
                // display form: volatile values (timestamps/UUIDs) masked to class
                // markers; load-bearing text kept verbatim.
                mask::merge_key(&v.replace('\t', "\\t").replace('\n', "\\n"))
            })
            .collect();
        body.push_str(&vals.join("\t"));
        body.push('\n');
    }
    // volatile-merge over rows: identical modulo timestamps/UUIDs -> [xN]
    Some(cluster::run(&body))
}
