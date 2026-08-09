use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Tokens of expected-vs-actual `cache_read` deviation that flags a
/// prompt-cache miss (see [`TurnState::record_cache_check`]).
pub const CACHE_MISS_DEVIANCE: u32 = 10_000;

/// One tool call dispatched during a turn: the args it was invoked with, how
/// long it ran, and whether it errored. Built by
/// `tool_dispatch::process_tool_calls` and folded into the owning `Turn` once
/// all of a turn's tool calls finish. Serialized (as `duration_ms`) so the
/// per-turn stats event and on-disk log can carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub tool: String,
    pub args: serde_json::Value,
    pub duration_ms: u64,
    pub is_error: bool,
}

/// Everything known about one internal round-trip to the model: our local
/// token estimate vs. what the API actually reported, cache/cost figures,
/// timing, and any tool calls it triggered. Turns are 0-indexed; turn 0 is
/// a synthetic accounting bucket for the system-prompt/tools overhead (see
/// `Agent::turn`'s `num_turns == 0` special case) and never has its own
/// timing, cost, or tool calls — those fields simply stay at their
/// defaults for it.
#[derive(Debug, Clone)]
pub struct Turn {
    pub id: usize,
    /// Local byte-based estimate of this turn's input size, computed
    /// before the request is sent.
    pub input_estimate: u32,
    /// Cumulative input tokens the API actually reported for this turn,
    /// once the response lands.
    pub input_exact: Option<u32>,
    /// `input_exact - input_estimate`, once both are known.
    pub input_delta: Option<i32>,
    pub cache_read: u32,
    pub cache_creation: u32,
    pub output: u32,
    pub cost: Option<f64>,
    /// Whether this turn's response looked like a prompt-cache miss: the
    /// server reported far fewer `cache_read` tokens than expected given
    /// prior turns' recorded sizes. Always `false` for turns 0 and 1 —
    /// there's nothing to have cached yet on the first real request.
    pub is_cache_miss: bool,
    /// Number of retried API errors before this turn's request ultimately
    /// succeeded. A turn whose request fails outright (retries exhausted,
    /// or a non-retryable error) aborts the whole run instead of
    /// completing, so it never shows up here as a finished `Turn`.
    pub api_error_count: u32,
    pub started_at: Instant,
    /// When the first streamed byte of the response arrived, if the turn
    /// actually made a request (not the synthetic turn 0).
    pub first_byte_at: Option<Instant>,
    /// Wall-clock time from `started_at` to the response finishing.
    pub duration: Option<Duration>,
    pub tool_calls: Vec<ToolCallRecord>,
}

impl Turn {
    fn new(id: usize, started_at: Instant) -> Self {
        Self {
            id,
            input_estimate: 0,
            input_exact: None,
            input_delta: None,
            cache_read: 0,
            cache_creation: 0,
            output: 0,
            cost: None,
            is_cache_miss: false,
            api_error_count: 0,
            started_at,
            first_byte_at: None,
            duration: None,
            tool_calls: Vec::new(),
        }
    }

    /// Number of tool calls in this turn that errored.
    pub fn tool_error_count(&self) -> usize {
        self.tool_calls.iter().filter(|t| t.is_error).count()
    }

    /// Total time spent executing this turn's tool calls (they run
    /// concurrently, so this is a sum of individual durations, not a
    /// wall-clock span).
    pub fn tool_duration(&self) -> Duration {
        Duration::from_millis(self.tool_calls.iter().map(|t| t.duration_ms).sum())
    }

    /// Time to the first streamed byte of the response, if the turn made
    /// a request and it hasn't errored out before streaming anything.
    pub fn ttfb(&self) -> Option<Duration> {
        self.first_byte_at.map(|at| at.duration_since(self.started_at))
    }
}

/// Per-turn history for the current run. In-memory only — rebuilt fresh
/// each run, not persisted to session storage.
#[derive(Debug, Default)]
pub struct TurnState {
    pub turns: Vec<Turn>,
}

impl TurnState {
    pub fn new() -> Self {
        Self { turns: Vec::new() }
    }

    fn ensure_turn(&mut self, turn: usize) -> &mut Turn {
        if turn >= self.turns.len() {
            let now = Instant::now();
            while self.turns.len() <= turn {
                let id = self.turns.len();
                self.turns.push(Turn::new(id, now));
            }
        }
        &mut self.turns[turn]
    }

    pub fn record_estimate(&mut self, turn: usize, estimate: u32) {
        self.ensure_turn(turn).input_estimate = estimate;
    }

    pub fn record_exact(&mut self, turn: usize, exact: u32) {
        let info = self.ensure_turn(turn);
        info.input_exact = Some(exact);
        info.input_delta = Some(exact as i32 - info.input_estimate as i32);
    }

    /// Sum of recorded estimates for turns strictly before `turn`.
    pub fn sum_estimate_before(&self, turn: usize) -> u32 {
        let end = turn.min(self.turns.len());
        self.turns[..end].iter().map(|t| t.input_estimate).sum()
    }

    /// Sum of exact (falling back to estimate when not yet known) token
    /// counts for turns strictly before `turn`. This is the cumulative
    /// input size that should now be served from the provider's prompt
    /// cache.
    pub fn sum_exact_before(&self, turn: usize) -> u32 {
        let end = turn.min(self.turns.len());
        self.turns[..end]
            .iter()
            .map(|t| t.input_exact.unwrap_or(t.input_estimate))
            .sum()
    }

    /// Records the cache/output/cost figures the API reported for a turn's
    /// response.
    pub fn record_response(
        &mut self,
        turn: usize,
        cache_read: u32,
        cache_creation: u32,
        output: u32,
        cost: Option<f64>,
    ) {
        let t = self.ensure_turn(turn);
        t.cache_read = cache_read;
        t.cache_creation = cache_creation;
        t.output = output;
        t.cost = cost;
    }

    /// Compares `expected` (what should now be served from cache, per
    /// prior turns' recorded sizes) against `actual` (the API's reported
    /// `cache_read`), and records whether the deviation crosses
    /// `CACHE_MISS_DEVIANCE`. Returns the computed flag.
    pub fn record_cache_check(&mut self, turn: usize, expected: u32, actual: u32) -> bool {
        let is_miss = expected.saturating_sub(actual) >= CACHE_MISS_DEVIANCE;
        self.ensure_turn(turn).is_cache_miss = is_miss;
        is_miss
    }

    /// Records how many retried API errors happened before this turn's
    /// request ultimately succeeded.
    pub fn record_api_errors(&mut self, turn: usize, count: u32) {
        self.ensure_turn(turn).api_error_count = count;
    }

    /// Records when the first streamed byte of a turn's response arrived,
    /// if it hasn't been recorded already.
    pub fn record_first_byte(&mut self, turn: usize, at: Instant) {
        let t = self.ensure_turn(turn);
        if t.first_byte_at.is_none() {
            t.first_byte_at = Some(at);
        }
    }

    /// Records total elapsed time for a turn, measured from when it was
    /// first created (i.e. when its input estimate was recorded, just
    /// before dispatching the request) to now.
    pub fn record_turn_complete(&mut self, turn: usize) {
        let t = self.ensure_turn(turn);
        t.duration = Some(t.started_at.elapsed());
    }

    /// Folds in the tool calls a turn triggered.
    pub fn record_tool_calls(&mut self, turn: usize, calls: Vec<ToolCallRecord>) {
        self.ensure_turn(turn).tool_calls.extend(calls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(4000, 4010, 10; "positive delta")]
    #[test_case(500, 490, -10; "negative delta")]
    fn test_record_exact(estimate: u32, exact: u32, expected_delta: i32) {
        let mut state = TurnState::new();
        state.record_estimate(0, estimate);
        state.record_exact(0, exact);

        assert_eq!(state.turns[0].input_estimate, estimate);
        assert_eq!(state.turns[0].input_exact, Some(exact));
        assert_eq!(state.turns[0].input_delta, Some(expected_delta));
    }

    #[test]
    fn record_response_sets_cache_output_and_cost() {
        let mut state = TurnState::new();
        state.record_response(1, 100, 20, 55, Some(0.012));

        let turn = &state.turns[1];
        assert_eq!(turn.cache_read, 100);
        assert_eq!(turn.cache_creation, 20);
        assert_eq!(turn.output, 55);
        assert_eq!(turn.cost, Some(0.012));
    }

    #[test]
    fn record_api_errors_sets_count() {
        let mut state = TurnState::new();
        state.record_api_errors(1, 2);
        assert_eq!(state.turns[1].api_error_count, 2);
    }

    #[test_case(50_000, 41_000, false; "deviation_below_threshold_not_a_miss")]
    #[test_case(50_000, 40_000, true; "deviation_at_threshold_is_a_miss")]
    #[test_case(40_000, 50_000, false; "actual_exceeding_expected_not_a_miss")]
    fn record_cache_check_flags_large_deviation(expected: u32, actual: u32, want_miss: bool) {
        let mut state = TurnState::new();
        let got = state.record_cache_check(2, expected, actual);
        assert_eq!(got, want_miss);
        assert_eq!(state.turns[2].is_cache_miss, want_miss);
    }

    #[test]
    fn record_turn_complete_sets_duration() {
        let mut state = TurnState::new();
        state.record_estimate(0, 10);
        std::thread::sleep(Duration::from_millis(5));
        state.record_turn_complete(0);

        assert!(state.turns[0].duration.unwrap() >= Duration::from_millis(5));
    }

    #[test]
    fn record_first_byte_does_not_overwrite() {
        let mut state = TurnState::new();
        let first = Instant::now();
        state.record_first_byte(0, first);
        state.record_first_byte(0, Instant::now());

        assert_eq!(state.turns[0].first_byte_at, Some(first));
    }

    #[test]
    fn ttfb_is_none_until_first_byte_recorded() {
        let mut state = TurnState::new();
        state.record_estimate(0, 10);
        assert_eq!(state.turns[0].ttfb(), None);

        std::thread::sleep(Duration::from_millis(5));
        state.record_first_byte(0, Instant::now());
        assert!(state.turns[0].ttfb().unwrap() >= Duration::from_millis(5));
    }

    #[test]
    fn tool_error_count_and_duration_aggregate_across_calls() {
        let mut state = TurnState::new();
        state.record_tool_calls(
            0,
            vec![
                ToolCallRecord {
                    id: "a".into(),
                    tool: "bash".into(),
                    args: serde_json::json!({}),
                    duration_ms: 100,
                    is_error: false,
                },
                ToolCallRecord {
                    id: "b".into(),
                    tool: "read".into(),
                    args: serde_json::json!({}),
                    duration_ms: 50,
                    is_error: true,
                },
            ],
        );

        let turn = &state.turns[0];
        assert_eq!(turn.tool_error_count(), 1);
        assert_eq!(turn.tool_duration(), Duration::from_millis(150));
    }
}
