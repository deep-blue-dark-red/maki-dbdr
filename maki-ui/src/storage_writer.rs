//! Coalescing write-behind cache with incremental JSONL persistence.
//!
//! Apps post session snapshots keyed by session id; the writer thread drains
//! the newest snapshot of every session per wake and performs O(delta)
//! appends. Deletes travel through the same per-session slot as saves, so
//! whichever the app asked for last is what reaches disk.
//!
//! Every entry carries the claim it is written under, and the writer holds
//! none of its own. A session stays locked exactly while a tab has it open or
//! a write for it is still queued, so a tab that lets go frees it the moment
//! its last snapshot lands. There is no release message that could overtake
//! that snapshot.

use std::collections::{HashMap, HashSet};
use std::io;
use std::mem;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maki_storage::id::MakiId;
use maki_storage::sessions::{SessionClaim, SessionError};
use maki_storage::{StateDir, StorageError};
use tracing::warn;

use crate::AppSession;

const SAVE_FAILED_PREFIX: &str = "Session save failed";
const SAVE_RECOVERED: &str = "Session save recovered";
/// The writer thread is how `pending` drains; without it every save would be
/// reported and never land.
#[cfg(test)]
pub(crate) const STORAGE_THREAD_MSG: &str = "spawn storage writer thread";

type Pending = Arc<Mutex<PendingWrites>>;

#[derive(Default)]
struct PendingWrites {
    entries: HashMap<MakiId, Entry>,
    in_flight: HashSet<MakiId>,
}

impl PendingWrites {
    fn take_batch(&mut self) -> HashMap<MakiId, Entry> {
        let batch = mem::take(&mut self.entries);
        self.in_flight.extend(
            batch
                .iter()
                .filter_map(|(id, entry)| matches!(entry, Entry::Save(..)).then_some(*id)),
        );
        batch
    }

    fn outstanding(&self) -> Vec<MakiId> {
        self.in_flight
            .iter()
            .copied()
            .chain(
                self.entries
                    .iter()
                    .filter_map(|(id, entry)| matches!(entry, Entry::Save(..)).then_some(*id)),
            )
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }
}

type DeleteCallback = Box<dyn FnOnce(Result<(), SessionError>) + Send>;

/// One slot per session, holding whatever the app asked for last. Deletes
/// used to ride a side channel, where a flush queued before a delete could
/// drain a save enqueued after it, so the delete unlinked a session the app
/// had just saved.
enum Entry {
    Save(Arc<AppSession>, SessionClaim),
    /// The claim when the caller holds one. Acquiring it again here would
    /// conflict with that holder, since a lock is per open file.
    Delete(Option<SessionClaim>, DeleteCallback),
}

pub struct StorageWriter {
    pending: Pending,
    wake: flume::Sender<()>,
    done_rx: flume::Receiver<Vec<MakiId>>,
}

impl StorageWriter {
    pub fn new(dir: StateDir, warn_tx: flume::Sender<String>) -> std::io::Result<Self> {
        let pending: Pending = Arc::default();
        let writer_pending = Arc::clone(&pending);
        let (wake, wake_rx) = flume::unbounded::<()>();
        let (done_tx, done_rx) = flume::bounded::<Vec<MakiId>>(1);

        std::thread::Builder::new()
            .name("storage-writer".into())
            .spawn(move || {
                let mut writer = Writer {
                    dir,
                    warn_tx,
                    failing: HashSet::new(),
                };
                while wake_rx.recv().is_ok() {
                    writer.flush(&writer_pending);
                }
                writer.flush(&writer_pending);
                let _ = done_tx.send(writer.failing.into_iter().collect());
            })?;

        // Without the thread there is nothing to drain `pending`, so sessions
        // would be reported saved and never written. Fail the start instead.
        Ok(Self {
            pending,
            wake,
            done_rx,
        })
    }

    pub fn send(&self, session: Arc<AppSession>, claim: SessionClaim) {
        self.enqueue(session.id, Entry::Save(session, claim));
    }

    /// Delete a session's files on the writer thread; `done` fires there, so
    /// callers never block on disk. Deleting a session that was never written
    /// reports success, and a save enqueued afterwards supersedes the delete.
    /// A caller with the session open passes its claim. Any other session is
    /// claimed here and refused if another process has it.
    pub fn delete(
        &self,
        id: MakiId,
        claim: Option<SessionClaim>,
        done: impl FnOnce(Result<(), SessionError>) + Send + 'static,
    ) {
        self.enqueue(id, Entry::Delete(claim, Box::new(done)));
    }

    fn enqueue(&self, id: MakiId, entry: Entry) {
        lock(&self.pending).entries.insert(id, entry);
        if self.wake.send(()).is_err() {
            let mut pending = lock(&self.pending);
            if matches!(pending.entries.get(&id), Some(Entry::Delete(..)))
                && let Some(Entry::Delete(_, done)) = pending.entries.remove(&id)
            {
                drop(pending);
                done(Err(writer_gone()));
            }
        }
    }

    /// Returns the sessions whose last write never reached disk. The caller
    /// reports them, since the screen is still up here.
    #[must_use]
    pub fn shutdown(self, timeout: Duration) -> Vec<MakiId> {
        drop(self.wake);
        self.done_rx.recv_timeout(timeout).unwrap_or_else(|_| {
            warn!("storage writer did not drain within {timeout:?}");
            lock(&self.pending).outstanding()
        })
    }
}

fn lock(pending: &Pending) -> std::sync::MutexGuard<'_, PendingWrites> {
    pending.lock().unwrap_or_else(|e| e.into_inner())
}

fn writer_gone() -> SessionError {
    StorageError::Io(io::Error::other("storage writer unavailable")).into()
}

/// Everything the writer thread owns. It never leaves that thread, so nothing
/// here needs a lock.
struct Writer {
    dir: StateDir,
    warn_tx: flume::Sender<String>,
    /// Sessions whose last write failed, so a sick disk warns once instead of
    /// once per frame.
    /// Whatever is still in here when the thread stops never reached disk.
    failing: HashSet<MakiId>,
}

impl Writer {
    fn flush(&mut self, pending: &Pending) {
        let batch = lock(pending).take_batch();
        for (id, entry) in batch {
            match entry {
                Entry::Save(session, claim) => {
                    let result = claim.persist(&*session);
                    let mut pending = lock(pending);
                    pending.in_flight.remove(&id);
                    if result.is_err() {
                        // `checkpoint` never resends an unchanged revision, so
                        // a dropped snapshot would miss disk for good.
                        // `or_insert` lets a newer op win; the shutdown flush
                        // is the last retry. The claim stays queued with it, so
                        // the session stays locked while a write is owed.
                        pending
                            .entries
                            .entry(id)
                            .or_insert(Entry::Save(session, claim));
                    }
                    drop(pending);
                    self.report(id, result);
                }
                Entry::Delete(claim, done) => {
                    self.failing.remove(&id);
                    done(self.remove(id, claim));
                }
            }
        }
    }

    fn remove(&self, id: MakiId, claim: Option<SessionClaim>) -> Result<(), SessionError> {
        let claim = match claim {
            Some(claim) => claim,
            None => SessionClaim::acquire(id, &self.dir)?,
        };
        match AppSession::delete(&claim, &self.dir) {
            Err(e) if e.is_not_found() => Ok(()),
            result => result,
        }
    }

    fn report(&mut self, id: MakiId, result: Result<(), SessionError>) {
        match result {
            Ok(()) => {
                if self.failing.remove(&id) {
                    let _ = self.warn_tx.send(SAVE_RECOVERED.to_string());
                }
            }
            Err(e) => {
                warn!(error = %e, %id, "session write failed");
                if self.failing.insert(id) {
                    let _ = self.warn_tx.send(format!("{SAVE_FAILED_PREFIX}: {e}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use maki_storage::sessions::SESSIONS_DIR;
    use tempfile::TempDir;
    use test_case::test_case;

    use super::*;
    use crate::OpenSession;

    const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
    const MODEL: &str = "test-model";
    const CWD: &str = "/tmp/writer";
    const MSG_PREFIX: &str = "msg-";
    const RESUMED_MSG: &str = "resumed";
    const TOOL_ID: &str = "tool-1";
    const TOOL_TEXT: &str = "tool output";
    const TITLE: &str = "renamed after reload";
    const OWED_WRITE_HOLDS: &str = "a session with a write still owed must stay claimed";
    const LANDED_RELEASES: &str = "a session let go is free once its last snapshot landed";
    const ALL_LANDED: &str = "a drain that wrote everything reports nothing unsaved";
    const REPORTED_UNSAVED: &str = "a transcript that never landed must be named on the way out";

    #[test]
    fn a_snapshot_sent_after_the_writer_stops_is_named_at_shutdown() {
        let (_tmp, dir) = state_dir();
        let (session, claim) = fresh(&dir);
        let id = session.id;
        let (wake, wake_rx) = flume::unbounded();
        let (done_tx, done_rx) = flume::bounded(1);
        drop(wake_rx);
        drop(done_tx);
        let writer = StorageWriter {
            pending: Arc::default(),
            wake,
            done_rx,
        };
        writer.send(Arc::new(session), claim);
        assert_eq!(
            writer.shutdown(Duration::ZERO),
            vec![id],
            "{REPORTED_UNSAVED}"
        );
    }

    #[test_case(false, false, false; "queued_timeout")]
    #[test_case(true, false, false; "in_flight_timeout")]
    #[test_case(true, true, false; "newer_snapshot_timeout")]
    #[test_case(false, false, true; "queued_disconnected")]
    #[test_case(true, false, true; "in_flight_disconnected")]
    fn unfinished_writes_are_named_when_shutdown_cannot_drain(
        in_flight: bool,
        newer_snapshot: bool,
        disconnected: bool,
    ) {
        let (_tmp, dir) = state_dir();
        let (session, claim) = fresh(&dir);
        let id = session.id;
        let pending: Pending = Arc::default();
        lock(&pending)
            .entries
            .insert(id, Entry::Save(Arc::new(session.clone()), claim.clone()));
        let _batch = in_flight.then(|| lock(&pending).take_batch());
        if newer_snapshot {
            lock(&pending)
                .entries
                .insert(id, Entry::Save(Arc::new(session), claim));
        }
        let (wake, _wake_rx) = flume::unbounded();
        let (done_tx, done_rx) = flume::bounded(1);
        let _done_tx = (!disconnected).then_some(done_tx);
        let writer = StorageWriter {
            pending,
            wake,
            done_rx,
        };
        assert_eq!(
            writer.shutdown(Duration::ZERO),
            vec![id],
            "{REPORTED_UNSAVED}"
        );
    }

    fn state_dir() -> (TempDir, StateDir) {
        let tmp = TempDir::new().unwrap();
        let dir = StateDir::from_path(tmp.path().to_path_buf());
        (tmp, dir)
    }

    fn writer(dir: &StateDir) -> (StorageWriter, flume::Receiver<String>) {
        let (warn_tx, warn_rx) = flume::unbounded();
        (
            StorageWriter::new(dir.clone(), warn_tx).expect(STORAGE_THREAD_MSG),
            warn_rx,
        )
    }

    fn drain(writer: StorageWriter) {
        assert!(writer.shutdown(DRAIN_TIMEOUT).is_empty(), "{ALL_LANDED}");
    }

    fn fresh(dir: &StateDir) -> (AppSession, SessionClaim) {
        let OpenSession { session, claim } = OpenSession::fresh(MODEL, CWD, dir);
        (session, claim)
    }

    fn sessions_dir(dir: &StateDir) -> PathBuf {
        dir.path().join(SESSIONS_DIR)
    }

    fn message_texts(session: &AppSession) -> Vec<String> {
        session
            .messages()
            .iter()
            .map(|m| m.user_text().unwrap_or_default().to_string())
            .collect()
    }

    fn msg_text(n: usize) -> String {
        format!("{MSG_PREFIX}{n}")
    }

    fn user_message(n: usize) -> maki_providers::Message {
        maki_providers::Message::user(msg_text(n))
    }

    /// A plain file where the sessions dir should be. `create_dir_all` cannot
    /// turn that into a directory, so every flush fails until it is removed.
    fn block_sessions_dir(dir: &StateDir) {
        std::fs::write(sessions_dir(dir), "").unwrap();
    }

    /// A directory where one session's rewrite puts its temp file, so that
    /// session fails to write while its lock and every other session work.
    fn block_log(dir: &StateDir, id: MakiId) -> PathBuf {
        let blocker = sessions_dir(dir).join(format!("{id}.jsonl.tmp"));
        std::fs::create_dir_all(&blocker).unwrap();
        blocker
    }

    /// Snapshots must coalesce per session id, not into one `latest` slot:
    /// two racing sessions used to silently drop one.
    #[test]
    fn shutdown_drains_newest_snapshot_of_every_session() {
        let (_tmp, dir) = state_dir();
        let (writer, _warn_rx) = writer(&dir);
        let (a, a_claim) = fresh(&dir);
        let (mut b, b_claim) = fresh(&dir);
        let (a_id, b_id) = (a.id, b.id);
        writer.send(Arc::new(a), a_claim);
        writer.send(Arc::new(b.clone()), b_claim.clone());
        b.set_title("renamed".into());
        writer.send(Arc::new(b), b_claim);
        drain(writer);

        assert!(AppSession::load(a_id, &dir).is_ok());
        assert_eq!(AppSession::load(b_id, &dir).unwrap().title, "renamed");
    }

    #[test]
    fn delete_discards_pending_snapshot() {
        let (_tmp, dir) = state_dir();
        let (writer, _warn_rx) = writer(&dir);
        let (session, claim) = fresh(&dir);
        let id = session.id;
        writer.send(Arc::new(session), claim.clone());
        let (done_tx, done_rx) = flume::bounded(1);
        writer.delete(id, Some(claim), move |res| {
            let _ = done_tx.send(res);
        });
        drain(writer);

        assert!(done_rx.recv().unwrap().is_ok());
        assert!(AppSession::load(id, &dir).is_err());
    }

    /// A later run over an existing file holds a new claim and so no cursor.
    /// Its first write must start the file over instead of landing on offsets
    /// that describe the session as the earlier run left it.
    #[test]
    fn a_new_claim_rewrites_the_file_instead_of_appending() {
        let (_tmp, dir) = state_dir();
        let (mut session, claim) = fresh(&dir);
        let id = session.id;
        for i in 0..5 {
            session.push_message(user_message(i));
        }
        let (first, _first_warn_rx) = writer(&dir);
        first.send(Arc::new(session.clone()), claim);
        drain(first);

        session.truncate_messages(2);
        session.push_message(maki_providers::Message::user(RESUMED_MSG.into()));
        session.insert_tool_output(
            TOOL_ID.into(),
            Arc::new(maki_agent::ToolOutput::Plain(TOOL_TEXT.to_string().into())),
        );
        session.set_title(TITLE.into());

        let (second, second_warn_rx) = writer(&dir);
        let claim = SessionClaim::acquire(id, &dir).expect("the first run let go");
        second.send(Arc::new(session.clone()), claim);
        drain(second);

        let loaded = AppSession::load(id, &dir).unwrap();
        assert_eq!(
            message_texts(&loaded),
            [msg_text(0), msg_text(1), RESUMED_MSG.to_string()]
        );
        assert_eq!(loaded.title, TITLE);
        match loaded.tool_outputs().get(TOOL_ID).map(Arc::as_ref) {
            Some(maki_agent::ToolOutput::Plain(out)) => assert_eq!(out.text, TOOL_TEXT),
            other => panic!("tool output lost: {other:?}"),
        }
        assert!(second_warn_rx.is_empty());
    }

    /// A disk that keeps failing warns once, not once per frame, and says so
    /// exactly once when writes start working again.
    #[test]
    fn failing_flush_warns_once_and_reports_recovery() {
        let (_tmp, dir) = state_dir();
        block_sessions_dir(&dir);
        let (writer, warn_rx) = writer(&dir);
        let (session, claim) = fresh(&dir);
        let session = Arc::new(session);
        let id = session.id;

        writer.send(Arc::clone(&session), claim.clone());
        let warning = warn_rx.recv_timeout(DRAIN_TIMEOUT).unwrap();
        assert!(warning.starts_with(SAVE_FAILED_PREFIX), "{warning}");

        // The save is enqueued before the delete, so the flush that runs the
        // delete has already drained it; a repeat failure must stay silent.
        writer.send(Arc::clone(&session), claim.clone());
        let (done_tx, done_rx) = flume::bounded(1);
        writer.delete(MakiId::generate(), None, move |res| {
            let _ = done_tx.send(res);
        });
        assert!(done_rx.recv_timeout(DRAIN_TIMEOUT).unwrap().is_err());
        assert!(warn_rx.is_empty(), "second failure warned again");

        std::fs::remove_file(sessions_dir(&dir)).unwrap();
        writer.send(session, claim);
        let recovered = warn_rx.recv_timeout(DRAIN_TIMEOUT).unwrap();
        assert_eq!(recovered, SAVE_RECOVERED);
        drain(writer);

        assert!(warn_rx.is_empty());
        assert!(AppSession::load(id, &dir).is_ok());
    }

    /// A tab that swaps its session out drops its claim right after queueing
    /// the last snapshot. The session must stay locked until that snapshot is
    /// on disk and be free right after, without anyone telling the writer.
    #[test]
    fn a_session_let_go_stays_claimed_until_its_last_snapshot_lands() {
        let (_tmp, dir) = state_dir();
        let (writer, warn_rx) = writer(&dir);
        let (mut session, claim) = fresh(&dir);
        session.push_message(user_message(0));
        let id = session.id;
        let blocker = block_log(&dir, id);

        writer.send(Arc::new(session), claim);
        let warning = warn_rx.recv_timeout(DRAIN_TIMEOUT).unwrap();
        assert!(warning.starts_with(SAVE_FAILED_PREFIX), "{warning}");
        assert!(
            SessionClaim::acquire(id, &dir).is_err(),
            "{OWED_WRITE_HOLDS}"
        );

        std::fs::remove_dir(blocker).unwrap();
        drain(writer);

        SessionClaim::acquire(id, &dir).expect(LANDED_RELEASES);
        assert_eq!(
            message_texts(&AppSession::load(id, &dir).unwrap()),
            [msg_text(0)]
        );
    }

    /// The final flush runs after the status bar is gone, so the return value
    /// is the only way a lost transcript gets reported.
    #[test]
    fn a_transcript_that_never_landed_is_named_at_shutdown() {
        let (_tmp, dir) = state_dir();
        let (mut session, claim) = fresh(&dir);
        session.push_message(user_message(0));
        let id = session.id;
        let _blocker = block_log(&dir, id);

        let (writer, _warn_rx) = writer(&dir);
        writer.send(Arc::new(session), claim);

        assert_eq!(
            writer.shutdown(DRAIN_TIMEOUT),
            vec![id],
            "{REPORTED_UNSAVED}"
        );
    }

    /// A save enqueued after a delete must win: clear a draft and retype it
    /// fast enough, and the queued delete used to unlink the file the retype
    /// had just saved.
    #[test]
    fn save_enqueued_after_delete_survives() {
        let (_tmp, dir) = state_dir();
        let (writer, warn_rx) = writer(&dir);
        let (mut session, claim) = fresh(&dir);
        let id = session.id;
        session.push_message(user_message(0));
        writer.send(Arc::new(session.clone()), claim.clone());
        writer.delete(id, Some(claim.clone()), |_| {});
        session.push_message(maki_providers::Message::user(RESUMED_MSG.into()));
        writer.send(Arc::new(session), claim);
        drain(writer);

        let loaded = AppSession::load(id, &dir).unwrap();
        assert_eq!(
            message_texts(&loaded),
            [msg_text(0), RESUMED_MSG.to_string()]
        );
        assert!(warn_rx.is_empty());
    }

    /// A failed write stays queued: `checkpoint` never resends an unchanged
    /// revision, so the writer owns the retry, and the shutdown flush is the
    /// last one.
    #[test]
    fn failed_write_is_retried_by_a_later_flush() {
        let (_tmp, dir) = state_dir();
        block_sessions_dir(&dir);
        let (writer, warn_rx) = writer(&dir);
        let (session, claim) = fresh(&dir);
        let id = session.id;

        writer.send(Arc::new(session), claim);
        let warning = warn_rx.recv_timeout(DRAIN_TIMEOUT).unwrap();
        assert!(warning.starts_with(SAVE_FAILED_PREFIX), "{warning}");

        std::fs::remove_file(sessions_dir(&dir)).unwrap();
        drain(writer);

        assert!(AppSession::load(id, &dir).is_ok());
        assert_eq!(warn_rx.recv_timeout(DRAIN_TIMEOUT).unwrap(), SAVE_RECOVERED);
    }

    /// After a delete the cursor still holds an open handle to the unlinked
    /// file, which still looks unchanged, so an append would write the session
    /// into nothing. The delete voids the claim's cursor, so the next snapshot
    /// writes a whole file.
    #[test]
    fn session_recreated_after_delete_is_written_in_full() {
        let (_tmp, dir) = state_dir();
        let (writer, warn_rx) = writer(&dir);
        let (mut session, claim) = fresh(&dir);
        let id = session.id;
        session.push_message(user_message(0));
        writer.send(Arc::new(session.clone()), claim.clone());

        let (done_tx, done_rx) = flume::bounded(1);
        writer.delete(id, Some(claim.clone()), move |res| {
            let _ = done_tx.send(res);
        });
        done_rx.recv_timeout(DRAIN_TIMEOUT).unwrap().unwrap();
        assert!(AppSession::load(id, &dir).is_err());

        session.push_message(maki_providers::Message::user(RESUMED_MSG.into()));
        writer.send(Arc::new(session), claim);
        drain(writer);

        let loaded = AppSession::load(id, &dir).unwrap();
        assert_eq!(
            message_texts(&loaded),
            [msg_text(0), RESUMED_MSG.to_string()]
        );
        assert!(warn_rx.is_empty());
    }
}
