//! Content-addressed side table: every elided/merged original is recoverable
//! via `rstring expand <hash16>`.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Default)]
pub struct SideTable {
    map: HashMap<String, String>,
}

impl SideTable {
    pub fn load(path: &str) -> Self {
        let map = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| {
                v.as_object().map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
            })
            .unwrap_or_default();
        SideTable { map }
    }

    pub fn save(&self, path: &str) {
        if self.map.is_empty() {
            return;
        }
        let v: Value = self
            .map
            .iter()
            .map(|(k, s)| (k.clone(), json!(s)))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        std::fs::write(path, serde_json::to_string(&v).unwrap()).unwrap();
    }

    pub fn put(&mut self, text: &str) -> String {
        let mut h = Sha256::digest(text.as_bytes());
        let mut hex = String::new();
        for b in h.iter_mut().take(8) {
            hex.push_str(&format!("{:02x}", b));
        }
        self.map
            .entry(hex.clone())
            .or_insert_with(|| text.to_string());
        hex
    }

    pub fn get(&self, hash: &str) -> Option<&String> {
        self.map.get(hash)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    /// Read-only view, for tests and tooling.
    pub fn map_ref(&self) -> &HashMap<String, String> {
        &self.map
    }
}
