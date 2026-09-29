//! Intelligence scores the `aa_scores` plugin caches from Artificial Analysis,
//! so a picker row can quote one instead of a tier word.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::StorageError;
use crate::paths::state_dir;

const CACHE_NAME: &str = "aa_scores.json";

#[derive(Debug, Deserialize)]
struct CacheFile {
    scores: HashMap<String, f64>,
    #[serde(default)]
    estimated: HashSet<String>,
}

/// Scores for the models Artificial Analysis tracks, keyed by slug.
#[derive(Debug, Default)]
pub struct AaScores {
    scores: HashMap<String, f64>,
    estimated: HashSet<String>,
}

impl AaScores {
    /// Reads the plugin's cache. A missing or unreadable file scores nothing:
    /// the picker simply shows no score until the plugin writes one.
    pub fn load() -> Self {
        let Ok(dir) = state_dir() else {
            return Self::default();
        };
        std::fs::read_to_string(dir.join(CACHE_NAME))
            .ok()
            .and_then(|text| Self::parse(&text).ok())
            .unwrap_or_default()
    }

    pub fn parse(text: &str) -> Result<Self, StorageError> {
        let file: CacheFile = serde_json::from_str(text)?;
        Ok(Self {
            scores: file
                .scores
                .into_iter()
                .map(|(slug, score)| (canonical(&slug), score))
                .collect(),
            estimated: file.estimated.into_iter().map(|s| canonical(&s)).collect(),
        })
    }

    /// The index for {model_id} and whether Artificial Analysis estimated it.
    /// An id the leaderboard spells differently matches its longest published
    /// prefix at a dash boundary, so `gemini-3.8-flash-high` answers with
    /// `gemini-3-8-flash`.
    pub fn score(&self, model_id: &str) -> Option<(f64, bool)> {
        let id = canonical(model_id);
        if let Some(&score) = self.scores.get(&id) {
            return Some((score, self.estimated.contains(&id)));
        }
        let (slug, &score) = self
            .scores
            .iter()
            .filter(|(slug, _)| {
                id.len() > slug.len()
                    && id.starts_with(slug.as_str())
                    && id.as_bytes()[slug.len()] == b'-'
            })
            .max_by_key(|(slug, _)| slug.len())?;
        Some((score, self.estimated.contains(slug)))
    }
}

/// Slugs are lowercase and dashed already, so this only normalises the ids
/// maki spells differently: a dotted version, an underscore, or the path a
/// local model is filed under.
fn canonical(id: &str) -> String {
    id.rsplit('/')
        .next()
        .unwrap_or(id)
        .to_lowercase()
        .replace(['.', '_'], "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CACHE: &str = r#"{"scores":{"glm-5-3":44.8,"glm-5":27.9,"gemini-3-8-flash":40.9},"estimated":["glm-5-3","glm-5"]}"#;

    #[test]
    fn exact_slug_answers_with_its_estimated_flag() {
        let scores = AaScores::parse(CACHE).unwrap();
        assert_eq!(scores.score("glm-5.3"), Some((44.8, true)));
        assert_eq!(scores.score("gemini-3.8-flash"), Some((40.9, false)));
    }

    #[test]
    fn a_longer_id_falls_back_to_its_base_model() {
        let scores = AaScores::parse(CACHE).unwrap();
        assert_eq!(
            scores.score("gemini-3.8-flash-high"),
            Some((40.9, false)),
            "an effort variant should quote the measured base model"
        );
    }

    #[test]
    fn a_local_model_matches_by_file_name() {
        let scores = AaScores::parse(CACHE).unwrap();
        assert_eq!(scores.score("/models/GLM-5.3.gguf"), Some((44.8, true)));
    }

    #[test]
    fn an_unknown_model_scores_nothing() {
        let scores = AaScores::parse(CACHE).unwrap();
        assert_eq!(scores.score("deepseek-v4-pro"), None);
        assert_eq!(
            scores.score("glm-50"),
            None,
            "a prefix needs a dash after it"
        );
    }

    #[test]
    fn a_broken_cache_is_an_error_not_a_panic() {
        assert!(AaScores::parse("not json").is_err());
    }
}
