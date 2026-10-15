//! One supervised run as a record: the policy it ran under, every review with
//! its cost and ruling, the evidence the harness took at each report, and how
//! much the run cost in all. Appended to `episodes.ndjson` when the run's
//! thread ends, so policies can be compared, and later tuned, against real
//! outcomes.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use maki_providers::TokenUsage;
use serde::Serialize;
use tracing::warn;

use super::Policy;
use crate::RunLedger;

pub(super) const EPISODES_DIR: &str = "supervisor";
pub(super) const EPISODES_FILE: &str = "episodes.ndjson";

#[derive(Serialize)]
pub(super) struct Episode {
    started_at: u64,
    duration_ms: u64,
    session: Option<String>,
    worker: String,
    worker_model: String,
    supervisor_model: String,
    policy: Arc<Policy>,
    pub(super) reviews: Vec<Review>,
    pub(super) reports: Vec<Evidence>,
    pub(super) ended: bool,
    /// Worker and supervisor together, subagents included.
    total_usage: TokenUsage,
    total_cost: Option<f64>,
    #[serde(skip)]
    started: Instant,
    #[serde(skip)]
    ledger: Arc<RunLedger>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

/// What a review was about.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Message,
    Batch,
    Exit,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Verdict {
    Ok,
    Note,
    Stop,
    Failed,
    Offline,
}

#[derive(Serialize)]
pub(super) struct Review {
    pub kind: Kind,
    pub verdict: Verdict,
    pub turn: u32,
    pub usage: TokenUsage,
    pub cost: Option<f64>,
    pub elapsed_ms: u64,
}

impl Review {
    pub(super) fn new(kind: Kind, verdict: Verdict, turn: u32, started: Instant) -> Self {
        Self {
            kind,
            verdict,
            turn,
            usage: TokenUsage::default(),
            cost: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    }
}

/// The harness's own view at a report: the third-party signal a reward can
/// be computed from without trusting the worker.
#[derive(Serialize)]
pub(super) struct Evidence {
    pub turn: u32,
    /// `None` outside a git repository.
    pub changed: Option<bool>,
    pub check_exits: Vec<String>,
}

pub(super) struct Origin {
    pub session: Option<String>,
    pub worker: String,
    pub worker_model: String,
    pub supervisor_model: String,
}

impl Episode {
    pub(super) fn new(
        origin: Origin,
        policy: Arc<Policy>,
        ledger: Arc<RunLedger>,
        path: Option<PathBuf>,
    ) -> Self {
        Self {
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs()),
            duration_ms: 0,
            session: origin.session,
            worker: origin.worker,
            worker_model: origin.worker_model,
            supervisor_model: origin.supervisor_model,
            policy,
            reviews: Vec::new(),
            reports: Vec::new(),
            ended: false,
            total_usage: TokenUsage::default(),
            total_cost: None,
            started: Instant::now(),
            ledger,
            path,
        }
    }

    fn line(&mut self) -> serde_json::Result<String> {
        let totals = self.ledger.totals();
        self.duration_ms = self.started.elapsed().as_millis() as u64;
        self.total_usage = totals.usage;
        self.total_cost = totals.cost;
        serde_json::to_string(self)
    }
}

/// The thread is dropped whichever way it ends, a stop, the run moving on,
/// or the session closing, so this is the one place the record is written.
impl Drop for Episode {
    fn drop(&mut self) {
        let Some(path) = self.path.take() else {
            return;
        };
        if self.reviews.is_empty() && !self.ended {
            return;
        }
        let written = self.line().map_err(|e| e.to_string()).and_then(|line| {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| e.to_string())?;
            writeln!(file, "{line}").map_err(|e| e.to_string())
        });
        if let Err(error) = written {
            warn!(path = %path.display(), %error, "supervisor episode not recorded");
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;

    fn episode(path: Option<PathBuf>) -> Episode {
        let origin = Origin {
            session: None,
            worker: "main".into(),
            worker_model: "weak".into(),
            supervisor_model: "strong".into(),
        };
        Episode::new(origin, Arc::default(), Arc::default(), path)
    }

    fn review() -> Review {
        Review::new(Kind::Exit, Verdict::Note, 3, Instant::now())
    }

    #[test]
    fn dropped_episode_appends_one_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(EPISODES_DIR).join(EPISODES_FILE);
        for _ in 0..2 {
            let mut episode = episode(Some(path.clone()));
            episode.reviews.push(review());
            episode.reports.push(Evidence {
                turn: 3,
                changed: Some(true),
                check_exits: vec!["0".into()],
            });
        }
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["reviews"][0]["verdict"], "note");
        assert_eq!(lines[0]["reviews"][0]["kind"], "exit");
        assert_eq!(lines[0]["reports"][0]["check_exits"][0], "0");
        assert!(lines[0]["policy"].is_object());
    }

    #[test]
    fn episode_with_nothing_to_tell_is_not_written() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(EPISODES_FILE);
        drop(episode(Some(path.clone())));
        assert!(!path.exists());
    }
}
