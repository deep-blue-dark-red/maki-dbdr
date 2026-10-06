//! Cooperative cancellation with parent-to-child propagation.
//!
//! `CancelTrigger` fires on Drop, so cleanup happens even if the trigger is forgotten.
//! `cancelled()` uses a double-check around the listener to close the TOCTOU window between flag read and listener registration.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use event_listener::Event;

struct Shared {
    /// A stop aimed at the run in flight. [`rearm`](Shared::rearm) clears it
    /// for the next run rather than a counter moving past it, so a stop raised
    /// with no run in flight goes stale instead of killing the next one.
    stopped: AtomicBool,
    /// A stop that outranks every rearm: a parent's cancel, a trigger's own
    /// drop, or an explicit [`fire`](CancelToken::fire). Never cleared, so it
    /// is a kill switch rather than a switch a run can flip back.
    ended: AtomicBool,
    event: Event,
}

impl Shared {
    fn fresh() -> Arc<Self> {
        Arc::new(Self {
            stopped: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            event: Event::new(),
        })
    }

    fn fire(&self) {
        self.ended.store(true, Ordering::Release);
        self.event.notify(usize::MAX);
    }

    fn interrupt(&self) {
        self.stopped.store(true, Ordering::Release);
        self.event.notify(usize::MAX);
    }

    fn rearm(&self) {
        self.stopped.store(false, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.ended.load(Ordering::Acquire) || self.stopped.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct CancelToken(Arc<Shared>);

pub struct CancelTrigger(Arc<Shared>);

impl CancelToken {
    pub fn new() -> (CancelTrigger, Self) {
        let shared = Shared::fresh();
        (CancelTrigger(Arc::clone(&shared)), Self(shared))
    }

    pub fn none() -> Self {
        Self(Shared::fresh())
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Fire from any clone of the token, any number of times. Unlike a
    /// `CancelTrigger`, holding or dropping the token never fires by itself,
    /// so it is safe to hand out as a kill switch.
    ///
    /// Sticky: no [`rearm`](Self::rearm) takes it back, so a cancel can never
    /// be lost to the run that happened to be opening.
    pub fn fire(&self) {
        self.0.fire();
    }

    /// Stops the run in flight and nothing else. The next
    /// [`rearm`](Self::rearm) opens past it, so an interrupt raised with no
    /// run in flight is dropped rather than killing the run after it.
    ///
    /// One store, so an interrupt and a rearm cannot interleave into a stop
    /// aimed at a run that has already ended.
    pub fn interrupt(&self) {
        self.0.interrupt();
    }

    /// Opens the next run: a stop aimed at an earlier run stops counting,
    /// while a stop from above — which a new run must not clear — keeps the
    /// token cancelled.
    pub fn rearm(&self) {
        self.0.rearm();
    }

    pub async fn race<T>(&self, future: impl Future<Output = T>) -> Result<T, String> {
        if self.is_cancelled() {
            return Err("cancelled".into());
        }
        futures_lite::future::race(async { Ok(future.await) }, async {
            self.cancelled().await;
            Err("cancelled".into())
        })
        .await
    }

    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let listener = self.0.event.listen();
            if self.is_cancelled() {
                return;
            }
            listener.await;
        }
    }

    pub fn child(&self) -> (CancelTrigger, Self) {
        let (child_trigger, child_token) = Self::new();
        let parent = self.clone();
        let child_shared = Arc::clone(&child_token.0);
        smol::spawn(async move {
            parent.cancelled().await;
            child_shared.fire();
        })
        .detach();
        (child_trigger, child_token)
    }
}

/// A frontend's handle on the run a session has in flight, kept next to the
/// session's own lifetime token so one value answers both questions the pane
/// asks: can this task still read an interrupt, and does it have a run to stop?
#[derive(Clone)]
pub struct RunInterrupt {
    session: CancelToken,
    run: CancelToken,
}

impl RunInterrupt {
    pub fn new(session: CancelToken, run: CancelToken) -> Self {
        Self { session, run }
    }

    /// Stops the run in flight, leaving the session free to run again.
    pub fn interrupt(&self) {
        self.run.interrupt();
    }

    /// Whether the session is still there to answer. A session that has closed,
    /// been cancelled from above, or belonged to a run that ended without
    /// parking it is gone; one the user is still talking to is not.
    pub fn is_live(&self) -> bool {
        !self.session.is_cancelled()
    }
}

impl std::fmt::Debug for RunInterrupt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunInterrupt")
            .field("live", &self.is_live())
            .finish()
    }
}

impl CancelTrigger {
    pub fn cancel(self) {
        self.0.fire();
    }

    /// Whether dropping this trigger is what fires {token}. A registry keeps
    /// the triggers and hands out the tokens, so this is how an owner finds its
    /// own row again without an id that somebody else could reuse.
    pub fn fires(&self, token: &CancelToken) -> bool {
        Arc::ptr_eq(&self.0, &token.0)
    }
}

impl Drop for CancelTrigger {
    fn drop(&mut self) {
        self.0.fire();
    }
}

/// Names one registration inside a key's list so its owner can retire it
/// without disturbing the others registered under the same key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CancelSlot(u64);

/// One session's registration under a key: the slot its owner retires by,
/// the trigger whose drop fires that session's token, and whether the work
/// outlives the run that opened it.
struct Registration {
    slot: CancelSlot,
    /// Held, never read. Dropping it is what fires the token of the session
    /// that registered it.
    _trigger: CancelTrigger,
    /// Work that outlives its run, which [`CancelMap::cancel_all`] honours.
    parked: bool,
}

#[derive(Default)]
struct Entry {
    registrations: Vec<Registration>,
    cancelled: bool,
}

/// Triggers grouped under an id a user can name and stop from the outside. One
/// subagent tool call can open several sessions under a single `tool_use_id`,
/// and cancelling that id has to reach all of them, even the ones opened after
/// the cancel.
///
/// So the mark lives on the id until the whole map is drained by
/// [`cancel_all`](Self::cancel_all). Clearing it when the last sibling retires
/// would lose the cancels that land while the id sits empty, which it does
/// before the first session registers and again between two sessions one tool
/// call opens back to back.
pub struct CancelMap<K> {
    entries: Mutex<HashMap<K, Entry>>,
    next_slot: AtomicU64,
}

impl<K: Eq + std::hash::Hash> Default for CancelMap<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + std::hash::Hash> CancelMap<K> {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            next_slot: AtomicU64::new(0),
        }
    }

    /// Registers {trigger} under {id}, alongside any already there, and
    /// returns the slot to hand back to [`retire`](Self::retire).
    pub fn insert(&self, id: K, trigger: CancelTrigger) -> CancelSlot {
        let mut map = self.lock();
        let slot = CancelSlot(self.next_slot.fetch_add(1, Ordering::Relaxed));
        let entry = map.entry(id).or_default();
        // Under a cancelled id the trigger is dropped instead of stored, and
        // that drop is what fires the token, so the session is born cancelled.
        if !entry.cancelled {
            entry.registrations.push(Registration {
                slot,
                _trigger: trigger,
                parked: false,
            });
        }
        slot
    }

    /// Retires one registration and drops its trigger. Siblings and the id's
    /// cancelled mark stay.
    pub fn retire(&self, id: &K, slot: CancelSlot) {
        let mut map = self.lock();
        let Some(entry) = map.get_mut(id) else {
            return;
        };
        entry.registrations.retain(|reg| reg.slot != slot);
    }

    /// Marks one registration as work that outlives the run that opened it, so
    /// [`cancel_all`](Self::cancel_all) leaves it be. Cancelling the id still
    /// reaches it, and retiring it still stops it.
    pub fn park(&self, id: &K, slot: CancelSlot) {
        let mut map = self.lock();
        let Some(entry) = map.get_mut(id) else {
            return;
        };
        if let Some(registration) = entry.registrations.iter_mut().find(|reg| reg.slot == slot) {
            registration.parked = true;
        }
    }

    /// Cancels every registration under {id} and marks the id, so a session
    /// registering under it later is born cancelled too.
    pub fn cancel(&self, id: K) {
        let mut map = self.lock();
        let entry = map.entry(id).or_default();
        entry.cancelled = true;
        entry.registrations.clear();
    }

    /// The run that owned these is over: stop what it still has registered and
    /// drop the marks with it, so no cancel of one run leaks into the next.
    /// Parked work outlives its run and stays registered under its id.
    pub fn cancel_all(&self) {
        let mut map = self.lock();
        for entry in map.values_mut() {
            entry.registrations.retain(|reg| reg.parked);
        }
        map.retain(|_, entry| !entry.registrations.is_empty());
    }

    /// Stops everything, parked work included. Respawn or shutdown: this map is
    /// done, whatever it was in the middle of.
    pub fn release_all(&self) {
        self.lock().clear();
    }

    #[cfg(test)]
    fn has_key(&self, id: &K) -> bool {
        self.lock().contains_key(id)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<K, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test]
    fn trigger_wakes_token() {
        smol::block_on(async {
            let (trigger, token) = CancelToken::new();
            assert!(!token.is_cancelled());
            trigger.cancel();
            token.cancelled().await;
            assert!(token.is_cancelled());
        });
    }

    #[test]
    fn child_cancelled_by_parent() {
        smol::block_on(async {
            let (parent_trigger, parent_token) = CancelToken::new();
            let (_child_trigger, child_token) = parent_token.child();
            parent_trigger.cancel();
            child_token.cancelled().await;
            assert!(child_token.is_cancelled());
        });
    }

    #[test]
    fn child_cancelled_by_own_trigger() {
        smol::block_on(async {
            let (_parent_trigger, parent_token) = CancelToken::new();
            let (child_trigger, child_token) = parent_token.child();
            child_trigger.cancel();
            child_token.cancelled().await;
            assert!(child_token.is_cancelled());
            assert!(!parent_token.is_cancelled());
        });
    }

    #[test]
    fn drop_trigger_also_cancels() {
        smol::block_on(async {
            let (trigger, token) = CancelToken::new();
            drop(trigger);
            token.cancelled().await;
            assert!(token.is_cancelled());
        });
    }

    #[test]
    fn race_returns_value_when_not_cancelled() {
        smol::block_on(async {
            let (_trigger, token) = CancelToken::new();
            let result = token.race(async { 42 }).await;
            assert_eq!(result.unwrap(), 42);
        });
    }

    #[test]
    fn race_returns_error_when_already_cancelled() {
        smol::block_on(async {
            let (trigger, token) = CancelToken::new();
            trigger.cancel();
            let result = token.race(std::future::pending::<()>()).await;
            assert!(result.unwrap_err().contains("cancelled"));
        });
    }

    #[test]
    fn race_interrupted_by_concurrent_cancel() {
        smol::block_on(async {
            let (trigger, token) = CancelToken::new();
            smol::spawn(async move { trigger.cancel() }).detach();
            let result = token.race(std::future::pending::<()>()).await;
            assert!(result.is_err());
        });
    }

    #[test]
    fn token_fire_works_from_a_clone_and_stays_safe_to_drop() {
        smol::block_on(async {
            let (_trigger, token) = CancelToken::new();
            let handle = token.clone();
            handle.fire();
            assert!(token.is_cancelled(), "fire from a clone reaches the token");
            handle.fire();
            // Dropping the handle must not be a kill: only an explicit fire is.
            drop(handle);
            assert!(token.is_cancelled());
            assert!(token.race(std::future::pending::<()>()).await.is_err());
        });
    }

    #[test]
    fn trigger_identity_tells_registrations_apart() {
        let (trigger, token) = CancelToken::new();
        let (_other_trigger, other_token) = CancelToken::new();
        assert!(trigger.fires(&token));
        assert!(!trigger.fires(&other_token));
    }

    const STALE_STOP: &str = "a stop aimed at a finished run must not kill the next one";
    const REARM_TOOK_THE_CANCEL: &str = "rearm must not take back a cancel that outranks it";
    const LOST_INTERRUPT: &str = "an interrupt of the run in flight did not stop it";

    /// An interrupt raised with no run in flight goes stale at the next
    /// `rearm` instead of killing the run that opens after it.
    #[test]
    fn interrupt_raised_between_runs_is_dropped_by_rearm() {
        let (_trigger, token) = CancelToken::new();
        token.interrupt();
        token.rearm();
        assert!(!token.is_cancelled(), "{STALE_STOP}");
    }

    #[test]
    fn interrupt_stops_the_run_it_lands_in() {
        smol::block_on(async {
            let (_trigger, token) = CancelToken::new();
            token.rearm();
            token.interrupt();
            token.cancelled().await;
            assert!(token.is_cancelled(), "{LOST_INTERRUPT}");
        });
    }

    /// `fire` is the kill switch its doc promises: no rearm takes it back, so a
    /// cancel cannot be lost to the run that happened to be opening.
    #[test]
    fn fire_outranks_rearm() {
        let (_trigger, token) = CancelToken::new();
        token.fire();
        token.rearm();
        assert!(token.is_cancelled(), "{REARM_TOOK_THE_CANCEL}");
    }

    /// A run's stop token sits two levels below the owner that can cancel it
    /// from the outside, and the propagation down is asynchronous: once it has
    /// landed, no rearm of any descendant may forget it again.
    #[test]
    fn parent_cancel_survives_the_descendants_rearm() {
        smol::block_on(async {
            let (parent_trigger, parent) = CancelToken::new();
            let (_child_trigger, child) = parent.child();
            let (_grand_trigger, grandchild) = child.child();

            parent_trigger.cancel();
            grandchild.cancelled().await;
            child.rearm();
            grandchild.rearm();

            assert!(grandchild.is_cancelled(), "{LOST_CANCEL}");
        });
    }

    /// What a task pane reads: the session outlives a run of its own, so a
    /// still-listening session is live and an interrupted one stays live too.
    #[test]
    fn run_interrupt_reports_the_session_not_the_run() {
        let (session_trigger, session) = CancelToken::new();
        let (_run_trigger, run) = session.child();
        let handle = RunInterrupt::new(session.clone(), run.clone());

        run.interrupt();
        assert!(handle.is_live(), "an interrupted run leaves the session");
        assert!(run.is_cancelled());

        session_trigger.cancel();
        assert!(!handle.is_live(), "a cancelled session is gone for good");
    }

    const KEY: &str = "x";
    const OTHER_KEY: &str = "y";
    const LOST_CANCEL: &str = "the cancel left no mark for the session after it";

    fn key() -> String {
        KEY.to_owned()
    }

    /// What the id looks like when the cancel lands.
    enum Shape {
        /// Nothing registered yet, so the cancel and the first session race.
        Empty,
        Occupied,
        /// One session retired and the next has yet to register, the gap a tool
        /// call leaves between two it opens back to back.
        Hole,
    }

    /// Whatever shape the id is in, the cancel has to reach the sessions the
    /// tool call has not opened yet. Losing it left the next session running
    /// with its pane already marked cancelled.
    #[test_case(Shape::Empty    ; "the_cancel_beat_the_first_session")]
    #[test_case(Shape::Occupied ; "a_sibling_is_still_running")]
    #[test_case(Shape::Hole     ; "between_two_sessions_of_one_tool_call")]
    fn cancel_map_cancel_catches_the_session_that_registers_after_it(shape: Shape) {
        let map: CancelMap<String> = CancelMap::new();
        let (earlier, _token) = CancelToken::new();
        match shape {
            Shape::Empty => {}
            Shape::Occupied => {
                map.insert(key(), earlier);
            }
            Shape::Hole => {
                let slot = map.insert(key(), earlier);
                map.retire(&key(), slot);
            }
        }

        map.cancel(key());

        let (trigger, token) = CancelToken::new();
        map.insert(key(), trigger);
        assert!(token.is_cancelled(), "{LOST_CANCEL}");
    }

    #[test]
    fn cancel_map_cancel_all_stops_everything_and_forgets_the_marks() {
        let map = CancelMap::new();
        let (t1, tok1) = CancelToken::new();
        let (t2, tok2) = CancelToken::new();
        map.insert(key(), t1);
        map.insert(OTHER_KEY.to_owned(), t2);
        map.cancel(key());

        map.cancel_all();
        assert!(tok1.is_cancelled());
        assert!(tok2.is_cancelled());
        assert!(!map.has_key(&key()));

        let (trigger, token) = CancelToken::new();
        map.insert(key(), trigger);
        assert!(!token.is_cancelled());
    }

    /// One tool call can open several subagents. They used to evict each
    /// other, so the first died the moment the second registered.
    #[test]
    fn cancel_map_keeps_siblings_under_one_key() {
        let map = CancelMap::new();
        let (t1, tok1) = CancelToken::new();
        let (t2, tok2) = CancelToken::new();
        map.insert(key(), t1);
        map.insert(key(), t2);
        assert!(!tok1.is_cancelled(), "a sibling must not evict the first");
        assert!(!tok2.is_cancelled());

        map.cancel(key());
        assert!(tok1.is_cancelled(), "cancelling the key stops them all");
        assert!(tok2.is_cancelled());
    }

    #[test]
    fn cancel_map_retire_leaves_siblings_running() {
        let map = CancelMap::new();
        let (t1, tok1) = CancelToken::new();
        let (t2, tok2) = CancelToken::new();
        let slot1 = map.insert(key(), t1);
        map.insert(key(), t2);

        map.retire(&key(), slot1);
        assert!(tok1.is_cancelled(), "retiring drops that trigger");
        assert!(!tok2.is_cancelled(), "the sibling keeps running");

        map.cancel(key());
        assert!(tok2.is_cancelled());
    }

    const LOST_PARK: &str = "parked work died with the run that opened it";
    const HELD_PARK: &str = "parked work outlived the release that should stop it";
    const LOST_PARK_CANCEL: &str = "cancelling the id did not reach parked work";

    /// A session the user is still talking to outlives the run that opened it,
    /// so ending that run must spare it while stopping its siblings.
    #[test]
    fn cancel_map_parked_work_outlives_the_run_that_opened_it() {
        let map = CancelMap::new();
        let (parked, parked_token) = CancelToken::new();
        let (sibling, sibling_token) = CancelToken::new();
        let slot = map.insert(key(), parked);
        map.insert(OTHER_KEY.to_owned(), sibling);
        map.park(&key(), slot);

        map.cancel_all();
        assert!(!parked_token.is_cancelled(), "{LOST_PARK}");
        assert!(sibling_token.is_cancelled());
    }

    #[test]
    fn cancel_map_cancel_still_reaches_parked_work() {
        let map = CancelMap::new();
        let (parked, token) = CancelToken::new();
        let slot = map.insert(key(), parked);
        map.park(&key(), slot);

        map.cancel(key());
        assert!(token.is_cancelled(), "{LOST_PARK_CANCEL}");
    }

    /// How a parked session ends itself: retiring its registration drops the
    /// trigger, and that drop is what fires its token.
    #[test]
    fn cancel_map_retire_stops_parked_work() {
        let map = CancelMap::new();
        let (parked, token) = CancelToken::new();
        let slot = map.insert(key(), parked);
        map.park(&key(), slot);

        map.retire(&key(), slot);
        assert!(token.is_cancelled());
    }

    /// Respawn and shutdown end the whole map, parked work included.
    #[test]
    fn cancel_map_release_all_stops_parked_work_too() {
        let map = CancelMap::new();
        let (parked, token) = CancelToken::new();
        let slot = map.insert(key(), parked);
        map.park(&key(), slot);

        map.release_all();
        assert!(token.is_cancelled(), "{HELD_PARK}");
        assert!(!map.has_key(&key()));
    }
}
