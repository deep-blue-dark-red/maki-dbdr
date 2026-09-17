use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use maki_agent::{CancelToken, CancelTrigger};

/// Esc and teardown for one agent loop, kept apart.
///
/// The loop runs one thing at a time and run ids only grow, so esc is a
/// watermark. `min_run_id` is the oldest run still allowed to proceed, and
/// stopping the run in flight, refusing a run esc beat to the start and
/// dropping what is queued behind it all come off that one number. No second
/// copy of "the user pressed esc" to keep in step.
///
/// Teardown is a separate signal and never a run id, because the two mean
/// opposite things: esc ends a run and the loop moves on, teardown ends the
/// loop. Startup work like instructions and the MCP handshake belongs to the
/// loop, so it watches [`teardown`](Self::teardown). Filed as a run it would
/// have been the oldest one there is, and every esc would have killed the loop
/// mid-handshake.
pub(super) struct RunCancels {
    state: Mutex<State>,
    teardown: CancelToken,
}

struct State {
    min_run_id: u64,
    /// The run in flight, held by its [`LiveRun`]. The id tells a late esc for
    /// a run that already finished from one meant for this one.
    live: Option<(u64, CancelTrigger)>,
    /// Dropped by [`cancel_all`](RunCancels::cancel_all), which is what fires
    /// the token.
    teardown: Option<CancelTrigger>,
}

impl State {
    /// Moves the line queued work has to clear, and stops the run in flight if
    /// that leaves it below the line.
    fn raise(&mut self, min_run_id: u64) {
        self.min_run_id = self.min_run_id.max(min_run_id);
        let min_run_id = self.min_run_id;
        // Taking it out drops the trigger, which fires the run's token.
        self.live.take_if(|&mut (id, _)| id < min_run_id);
    }
}

impl RunCancels {
    pub(super) fn new() -> Arc<Self> {
        let (trigger, teardown) = CancelToken::new();
        Arc::new(Self {
            state: Mutex::new(State {
                min_run_id: 0,
                live: None,
                teardown: Some(trigger),
            }),
            teardown,
        })
    }

    /// Fires when the loop is done for good. Work that belongs to no single run
    /// watches this instead of esc.
    pub(super) fn teardown(&self) -> &CancelToken {
        &self.teardown
    }

    /// The oldest run id that may still start. Queued work older than this was
    /// cancelled before the loop reached it.
    pub(super) fn min_run_id(&self) -> u64 {
        self.lock().min_run_id
    }

    /// Hands {run_id} the token that stops it. If esc got here first the token
    /// comes back already cancelled, which closes the gap between queueing a run
    /// and starting it.
    pub(super) fn start(self: &Arc<Self>, run_id: u64) -> LiveRun {
        let (trigger, token) = CancelToken::new();
        let mut state = self.lock();
        if run_id < state.min_run_id {
            // Dropped instead of stored, so the run is born cancelled.
            drop(trigger);
        } else {
            state.live = Some((run_id, trigger));
        }
        drop(state);
        LiveRun {
            cancels: Arc::clone(self),
            token,
        }
    }

    /// Cancels {run_id} and everything older, in flight or still queued. The
    /// loop lives on and takes the next run.
    pub(super) fn cancel(&self, run_id: u64) {
        self.lock().raise(run_id.saturating_add(1));
    }

    /// The loop is being torn down (respawn, shutdown), so nothing may start
    /// under it again.
    pub(super) fn cancel_all(&self) {
        let mut state = self.lock();
        state.raise(u64::MAX);
        drop(state.teardown.take());
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The run in flight. Dropping it fires the run's token, so work holding a
/// clone stops when the run does, and clears the registration, so an esc aimed
/// at this run cannot reach whatever starts after it.
pub(super) struct LiveRun {
    cancels: Arc<RunCancels>,
    token: CancelToken,
}

impl LiveRun {
    pub(super) fn token(&self) -> &CancelToken {
        &self.token
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        // By trigger identity, not by run id. A queued run carries the id it
        // was typed under, so ids repeat, and matching on one would let a
        // finished run retire the registration of the one running now.
        self.cancels
            .lock()
            .live
            .take_if(|(_, trigger)| trigger.fires(&self.token));
    }
}

#[cfg(test)]
mod tests {
    use super::RunCancels;

    const FIRST_RUN: u64 = 1;
    const SECOND_RUN: u64 = 2;
    /// The id a restored queue and a prompt typed while maki wakes up both
    /// carry, before the app bumps its counter for a run of its own.
    const EARLIEST_RUN: u64 = 0;

    #[test]
    fn esc_moves_the_line_queued_work_has_to_clear_and_it_never_moves_back() {
        let cancels = RunCancels::new();
        assert_eq!(cancels.min_run_id(), EARLIEST_RUN);

        cancels.cancel(SECOND_RUN);
        assert_eq!(cancels.min_run_id(), SECOND_RUN + 1);

        cancels.cancel(FIRST_RUN);
        assert_eq!(
            cancels.min_run_id(),
            SECOND_RUN + 1,
            "a late esc for an older run must not let newer work back in"
        );
        assert!(
            cancels.start(SECOND_RUN).token().is_cancelled(),
            "a run the loop reaches after esc must not start"
        );
    }

    /// Queued runs carry the id they were typed under, so the run that just
    /// ended and the one running now can share an id.
    #[test]
    fn esc_aimed_at_a_finished_run_leaves_the_next_one_alone() {
        let cancels = RunCancels::new();
        drop(cancels.start(FIRST_RUN));
        let next = cancels.start(FIRST_RUN);

        cancels.cancel(EARLIEST_RUN);

        assert!(!next.token().is_cancelled());
    }

    #[test]
    fn a_run_ending_fires_its_own_token() {
        let cancels = RunCancels::new();
        let run = cancels.start(FIRST_RUN);
        let token = run.token().clone();

        drop(run);

        assert!(
            token.is_cancelled(),
            "work outliving its run has to hear that the run ended"
        );
    }

    #[test]
    fn esc_ends_the_run_in_flight_but_not_the_loop() {
        let cancels = RunCancels::new();
        let run = cancels.start(EARLIEST_RUN);

        cancels.cancel(EARLIEST_RUN);

        assert!(run.token().is_cancelled());
        assert!(!cancels.teardown().is_cancelled());
        assert!(
            !cancels.start(FIRST_RUN).token().is_cancelled(),
            "the loop has to go on to take the next run"
        );
    }

    #[test]
    fn a_torn_down_loop_starts_nothing_else() {
        let cancels = RunCancels::new();
        let run = cancels.start(FIRST_RUN);

        cancels.cancel_all();

        assert!(run.token().is_cancelled());
        assert!(cancels.teardown().is_cancelled());
        assert!(cancels.start(SECOND_RUN).token().is_cancelled());
    }
}
