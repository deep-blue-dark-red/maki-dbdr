//! The producer side of the session feed: a background thread that owns this
//! process's ring and republishes the newest [`SessionSnapshot`] per session
//! every [`BROADCAST_PERIOD`] until the [`Broadcaster`] handle drops.
//!
//! Consumers attach at head and never see history, so the latest state has to
//! keep landing after they join; republishing the whole session map each tick
//! also recovers readers that were lapped. The UI hands over snapshots
//! losslessly on an unbounded channel and stays free of shmem concerns.

use std::{
    collections::HashMap,
    sync::{mpsc::{self, RecvTimeoutError}, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::{
    ChatLine, Chunk, Hello, MakiAck, MakiStatus,
    auth::AuthKey,
    control::{AckResult, Command, ControlLinks, Incoming},
    ring::{self, Ring, RingError},
    text,
};

const BROADCAST_PERIOD: Duration = Duration::from_millis(500);
const SENDER_NAME: &str = "maki";
/// [`ChatLine::text`] capacity; shorter texts go out as one line record.
const CHAT_TEXT_BYTES: usize = 456;
/// [`Chunk::payload`] capacity; longer texts split into these pieces.
const CHUNK_PAYLOAD_BYTES: usize = 480;

/// What a session is doing, on the wire as `MakiStatus.state`. The values are
/// the contract haunt decodes; do not renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Working = 1,
    Idle = 2,
    Waiting = 3,
}

/// Who said a chat line. Wire values are the low byte of [`Chunk::reserved_14`]
/// and the `sender_name` text of a [`ChatLine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    User = 1,
    Assistant = 2,
}

impl Speaker {
    pub const fn label(self) -> &'static str {
        match self {
            Speaker::User => "user",
            Speaker::Assistant => "assistant",
        }
    }

    const fn wire(self) -> u16 {
        self as u16
    }
}

/// `stream_key` of a chunked chat stream: FNV-1a 64 of the session id. A
/// consumer attributes chunks without a session field on [`Chunk`] by hashing
/// the ids it knows from `MakiStatus` records the same way.
pub fn stream_key(session_id: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in session_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// One session's publishable state, latest-wins per `session_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub state: SessionState,
    pub context_used: u32,
    pub context_limit: u32,
    pub cost_usd: f64,
    pub session_id: String,
    pub title: String,
    pub last_tool: String,
}

enum Cmd {
    Snapshot(SessionSnapshot),
    Remove(String),
    Ack {
        session_id: String,
        request_id: u64,
        result: AckResult,
    },
    Chat {
        session_id: String,
        speaker: Speaker,
        text: String,
    },
}

/// A command the UI must apply, handed over by the broadcaster thread.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingCommand {
    pub request_id: u64,
    pub session_id: String,
    pub command: Command,
}

/// Handle to this process's session ring. Dropping the last handle parks the
/// ring (`Status::Stopped`) and unlinks its file; a killed process instead
/// leaves the file behind for consumers to age out on the heartbeat.
#[derive(Debug)]
pub struct Broadcaster {
    tx: mpsc::Sender<Cmd>,
    commands: Mutex<mpsc::Receiver<PendingCommand>>,
}

impl Broadcaster {
    /// Creates this process's ring at `ring::ring_path`.
    pub fn start() -> Result<Self, RingError> {
        let pid = std::process::id();
        Self::start_at(&ring::ring_path(pid), pid)
    }

    fn start_at(path: &str, pid: u32) -> Result<Self, RingError> {
        let ring = Ring::create(path, ring::MAKI_NODE_ID, 0)?;
        let (tx, rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        thread::spawn(move || run(ring, rx, cmd_tx, pid));
        Ok(Self {
            tx,
            commands: Mutex::new(cmd_rx),
        })
    }

    /// Refreshes `snapshot.session_id`'s entry; never blocks, never fails.
    pub fn update(&self, snapshot: SessionSnapshot) {
        let _ = self.tx.send(Cmd::Snapshot(snapshot));
    }

    /// Drops a session from the feed, e.g. a tab that was deleted.
    pub fn remove(&self, session_id: &str) {
        let _ = self.tx.send(Cmd::Remove(session_id.to_owned()));
    }

    /// Replies to a control command on the status ring.
    pub fn ack(&self, session_id: &str, request_id: u64, result: AckResult) {
        let _ = self.tx.send(Cmd::Ack {
            session_id: session_id.to_owned(),
            request_id,
            result,
        });
    }

    /// Streams one chat entry: a single [`ChatLine`] when the text fits, a
    /// [`Chunk`] sequence keyed by [`stream_key`] when it does not. Published
    /// immediately, off the status period.
    pub fn chat(&self, session_id: &str, speaker: Speaker, text: &str) {
        if !text.is_empty() {
            let _ = self.tx.send(Cmd::Chat {
                session_id: session_id.to_owned(),
                speaker,
                text: text.to_owned(),
            });
        }
    }

    /// Commands waiting to be applied by the UI; drains without blocking.
    pub fn drain_commands(&self) -> Vec<PendingCommand> {
        let rx = self.commands.lock().expect("command channel poisoned");
        rx.try_iter().collect()
    }
}

fn run(ring: Ring, rx: mpsc::Receiver<Cmd>, cmd_tx: mpsc::Sender<PendingCommand>, pid: u32) {
    let key = AuthKey::load_or_create();
    let mut producer = ring::Producer::new(&ring);
    ring.update_status(ring::Status::Running);

    let mut hello = Hello {
        magic: 0x4D414B49,
        version: 1,
        role: 0,
        reserved_7: 0,
        pid,
        reserved_12: [0; 4],
        timestamp_ns: ring::monotonic_nanos() as u64,
        sender_name: [0; 64],
        reserved_88: [0; 8],
    };
    text::encode(&mut hello.sender_name, SENDER_NAME);
    producer.publish(Hello::MSG_ID.get(), &hello);
    producer.flush();
    ring.update_heartbeat(ring::monotonic_nanos());

    let mut sessions: HashMap<String, SessionSnapshot> = HashMap::new();
    let mut controls = ControlLinks::default();
    let mut last_publish = Instant::now();
    loop {
        let remaining = BROADCAST_PERIOD.saturating_sub(last_publish.elapsed());
        match rx.recv_timeout(remaining) {
            Ok(Cmd::Snapshot(snapshot)) => {
                sessions.insert(snapshot.session_id.clone(), snapshot);
            }
            Ok(Cmd::Remove(id)) => {
                sessions.remove(&id);
            }
            Ok(Cmd::Ack {
                session_id,
                request_id,
                result,
            }) => publish_ack(&mut producer, &session_id, request_id, result),
            Ok(Cmd::Chat {
                session_id,
                speaker,
                text,
            }) => publish_chat(&mut producer, &session_id, speaker, &text),
            Err(RecvTimeoutError::Timeout) => {
                let now = ring::monotonic_nanos() as u64;
                let polled = key.as_ref().map(|k| controls.poll(k)).unwrap_or_default();
                handle_commands(
                    &polled,
                    &sessions,
                    &cmd_tx,
                    &mut producer,
                    pid,
                    now,
                    key.as_ref(),
                );
                publish_statuses(&mut producer, &sessions, pid, now, key.as_ref());
                ring.update_heartbeat(ring::monotonic_nanos());
                last_publish = Instant::now();
            }
            Err(RecvTimeoutError::Disconnected) => {
                ring.update_status(ring::Status::Stopped);
                break;
            }
        }
    }
}

/// Targeted commands only reach the UI when this process owns the session;
/// a broadcast refresh republishes everyone. Targeted refreshes also
/// republish straight away, which is the whole point of a refresh.
fn handle_commands(
    incoming: &[Incoming],
    sessions: &HashMap<String, SessionSnapshot>,
    cmd_tx: &mpsc::Sender<PendingCommand>,
    producer: &mut ring::Producer,
    pid: u32,
    now: u64,
    key: Option<&AuthKey>,
) {
    for event in incoming {
        let owned = event.target.is_empty() || sessions.contains_key(&event.target);
        if !owned {
            continue;
        }
        match &event.command {
            Command::Refresh => {
                publish_statuses(producer, sessions, pid, now, key);
            }
            command => {
                let _ = cmd_tx.send(PendingCommand {
                    request_id: event.request_id,
                    session_id: event.target.clone(),
                    command: command.clone(),
                });
            }
        }
    }
}

fn publish_statuses(
    producer: &mut ring::Producer,
    sessions: &HashMap<String, SessionSnapshot>,
    pid: u32,
    now: u64,
    key: Option<&AuthKey>,
) {
    for snapshot in sessions.values() {
        let mut status = status_record(pid, snapshot, now);
        if let Some(key) = key {
            key.sign_status(&mut status);
        }
        producer.publish(MakiStatus::MSG_ID.get(), &status);
    }
    producer.flush();
}

fn publish_ack(producer: &mut ring::Producer, session_id: &str, request_id: u64, result: AckResult) {
    let mut ack = MakiAck {
        request_id,
        result: result as u8,
        reserved_9: [0; 7],
        timestamp_ns: ring::monotonic_nanos() as u64,
        session_id: [0; 32],
        mac: [0; 16],
        reserved_72: [0; 16],
    };
    text::encode(&mut ack.session_id, session_id);
    if let Some(key) = AuthKey::load_or_create() {
        key.sign_ack(&mut ack);
    }
    producer.publish(MakiAck::MSG_ID.get(), &ack);
    producer.flush();
}

fn publish_chat(producer: &mut ring::Producer, session_id: &str, speaker: Speaker, text: &str) {
    if text.len() <= CHAT_TEXT_BYTES {
        let mut line = ChatLine {
            timestamp_ns: ring::monotonic_nanos() as u64,
            sender_name: [0; 32],
            text: [0; 456],
        };
        text::encode(&mut line.sender_name, speaker.label());
        text::encode(&mut line.text, text);
        producer.publish(ChatLine::MSG_ID.get(), &line);
    } else {
        let key = stream_key(session_id);
        let bytes = text.as_bytes();
        let count = bytes.len().div_ceil(CHUNK_PAYLOAD_BYTES).min(u16::MAX as usize) as u16;
        let pieces: Vec<&[u8]> = chunk_pieces(text, count as usize).collect();
        for (index, piece) in pieces.into_iter().enumerate() {
            let mut chunk = Chunk {
                stream_key: key,
                chunk_index: index as u16,
                chunk_count: count,
                payload_len: piece.len() as u16,
                reserved_14: speaker.wire(),
                payload: [0; 480],
            };
            chunk.payload[..piece.len()].copy_from_slice(piece);
            producer.publish(Chunk::MSG_ID.get(), &chunk);
        }
    }
    producer.flush();
}

/// Splits `text` into `count` pieces of at most [`CHUNK_PAYLOAD_BYTES`],
/// backing off to char boundaries so reassembly stays valid utf8.
fn chunk_pieces(text: &str, count: usize) -> impl Iterator<Item = &[u8]> {
    let mut start = 0;
    (0..count).map(move |_| {
        let mut end = (start + CHUNK_PAYLOAD_BYTES).min(text.len());
        while end < text.len() && !text.is_char_boundary(end) {
            end -= 1;
        }
        let piece = &text.as_bytes()[start..end];
        start = end;
        piece
    })
}

fn status_record(pid: u32, snapshot: &SessionSnapshot, timestamp_ns: u64) -> MakiStatus {
    let mut status = MakiStatus {
        pid,
        state: snapshot.state as u8,
        flags: 0,
        reserved_6: 0,
        timestamp_ns,
        context_used: snapshot.context_used,
        context_limit: snapshot.context_limit,
        cost_usd: snapshot.cost_usd,
        session_id: [0; 32],
        session_title: [0; 64],
        last_tool: [0; 32],
        mac: [0; 16],
        reserved_176: [0; 16],
    };
    text::encode(&mut status.session_id, &snapshot.session_id);
    text::encode(&mut status.session_title, &snapshot.title);
    text::encode(&mut status.last_tool, &snapshot.last_tool);
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ring::{ring_dir, Consumer, Poll, StartAt},
    };

    fn unique_path(tag: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("{}/maki-test-{}-{tag}.brng", ring_dir(), n)
    }

    fn snapshot(session_id: &str, state: SessionState) -> SessionSnapshot {
        SessionSnapshot {
            state,
            context_used: 51_000,
            context_limit: 200_000,
            cost_usd: 0.51,
            session_id: session_id.to_owned(),
            title: "ring test".to_owned(),
            last_tool: "bash".to_owned(),
        }
    }

    #[test]
    fn status_record_maps_fields() {
        for (state, wire) in [
            (SessionState::Working, 1u8),
            (SessionState::Idle, 2),
            (SessionState::Waiting, 3),
        ] {
            let record = status_record(4242, &snapshot("abcd1234", state), 77);
            assert_eq!(record.state, wire);
            assert_eq!(record.pid, 4242);
            assert_eq!(record.timestamp_ns, 77);
            assert_eq!(record.context_used, 51_000);
            assert_eq!(record.context_limit, 200_000);
            assert_eq!(record.cost_usd, 0.51);
            assert_eq!(text::decode(&record.session_id), "abcd1234");
            assert_eq!(text::decode(&record.session_title), "ring test");
            assert_eq!(text::decode(&record.last_tool), "bash");
        }
    }

    #[test]
    fn status_record_truncates_long_text() {
        let mut long = snapshot(&"s".repeat(64), SessionState::Idle);
        long.title = "t".repeat(200);
        long.last_tool = "l".repeat(64);
        let record = status_record(1, &long, 0);
        assert_eq!(text::decode(&record.session_id).len(), 32);
        assert_eq!(text::decode(&record.session_title).len(), 64);
        assert_eq!(text::decode(&record.last_tool).len(), 32);
    }

    /// `StartAt::Head` only sees records published after the consumer exists,
    /// so wait for the tick rather than asserting on any single poll.
    fn await_status(ring: &Ring, consumer: &mut Consumer, deadline: Instant) -> Option<MakiStatus> {
        await_record(ring, consumer, MakiStatus::MSG_ID.get(), deadline)
            .map(|body| MakiStatus::copy_from_bytes(&body).expect("status"))
    }

    #[test]
    fn broadcaster_publishes_latest_snapshot() {
        let path = unique_path("broadcast");
        let pid = std::process::id();
        let broadcaster = Broadcaster::start_at(&path, pid).expect("start broadcaster");
        let ring = Ring::open(&path).expect("open");
        let mut consumer = Consumer::new(&ring, StartAt::Head);

        broadcaster.update(snapshot("session-a", SessionState::Working));
        let record = await_status(&ring, &mut consumer, Instant::now() + BROADCAST_PERIOD * 4)
            .expect("a published status");
        assert_eq!(text::decode(&record.session_id), "session-a");
        assert_eq!(record.pid, pid);
        assert_eq!(record.state, SessionState::Working as u8);
        assert_eq!(record.context_used, 51_000);

        broadcaster.update(snapshot("session-a", SessionState::Waiting));
        let record = await_status(&ring, &mut consumer, Instant::now() + BROADCAST_PERIOD * 4)
            .expect("a republished status");
        assert_eq!(record.state, SessionState::Waiting as u8);
    }

    /// Waits for the next `msg_id` record; `Head` consumers only see what is
    /// published after they exist, so poll until the deadline.
    fn await_record(
        ring: &Ring,
        consumer: &mut Consumer,
        msg_id: u16,
        deadline: Instant,
    ) -> Option<Vec<u8>> {
        while Instant::now() < deadline {
            match consumer.poll(ring) {
                Poll::Received { msg_id: id, .. } if id == msg_id => {
                    return Some(consumer.last_body(ring).to_vec());
                }
                Poll::Received { .. } => {}
                Poll::Waiting => thread::sleep(Duration::from_millis(10)),
                _ => return None,
            }
        }
        None
    }

    fn deadline() -> Instant {
        Instant::now() + BROADCAST_PERIOD * 4
    }

    #[test]
    fn short_chat_text_publishes_one_chat_line() {
        let path = unique_path("chat-short");
        let broadcaster = Broadcaster::start_at(&path, 7).expect("start broadcaster");
        let ring = Ring::open(&path).expect("open");
        let mut consumer = Consumer::new(&ring, StartAt::Head);

        broadcaster.chat("sess", Speaker::User, "hello rings");
        let line = ChatLine::copy_from_bytes(&await_record(
            &ring,
            &mut consumer,
            ChatLine::MSG_ID.get(),
            deadline(),
        )
        .expect("a chat line"))
        .expect("line");
        assert_eq!(text::decode(&line.sender_name), "user");
        assert_eq!(text::decode(&line.text), "hello rings");
    }

    #[test]
    fn chat_line_boundary_uses_one_record_and_past_it_chunks() {
        let path = unique_path("chat-boundary");
        let broadcaster = Broadcaster::start_at(&path, 7).expect("start broadcaster");
        let count_of = |ring: &Ring, text: &str, msg_id: u16| {
            let mut consumer = Consumer::new(ring, StartAt::Head);
            broadcaster.chat("sess", Speaker::Assistant, text);
            let deadline = deadline();
            let mut seen = 0;
            while Instant::now() < deadline {
                match consumer.poll(ring) {
                    Poll::Received { msg_id: id, .. } if id == msg_id => seen += 1,
                    Poll::Waiting => thread::sleep(Duration::from_millis(5)),
                    _ => break,
                }
            }
            seen
        };
        let ring = Ring::open(&path).expect("open");
        let fits = "a".repeat(CHAT_TEXT_BYTES);
        assert_eq!(count_of(&ring, &fits, ChatLine::MSG_ID.get()), 1);
        let spans = "a".repeat(CHUNK_PAYLOAD_BYTES + 1);
        assert_eq!(count_of(&ring, &spans, Chunk::MSG_ID.get()), 2);
    }

    #[test]
    fn chunked_chat_reassembles_to_the_original() {
        let path = unique_path("chat-chunks");
        let broadcaster = Broadcaster::start_at(&path, 7).expect("start broadcaster");
        let ring = Ring::open(&path).expect("open");
        let mut consumer = Consumer::new(&ring, StartAt::Head);

        // Multi-byte chars straddle the 480-byte piece boundary.
        let text = format!("{}tail", "é".repeat(CHUNK_PAYLOAD_BYTES));
        broadcaster.chat("session-x", Speaker::Assistant, &text);
        let deadline = deadline();
        let mut assembled = Vec::new();
        let mut count = 0;
        while assembled.len() < text.len() && Instant::now() < deadline {
            if let Some(body) = await_record(&ring, &mut consumer, Chunk::MSG_ID.get(), deadline) {
                let chunk = Chunk::copy_from_bytes(&body).expect("chunk");
                assert_eq!(chunk.stream_key, stream_key("session-x"));
                assert_eq!(chunk.reserved_14, Speaker::Assistant.wire());
                count = chunk.chunk_count;
                assembled.extend_from_slice(&chunk.payload[..chunk.payload_len as usize]);
            }
        }
        assert_eq!(assembled, text.as_bytes());
        assert_eq!(count as usize, text.len().div_ceil(CHUNK_PAYLOAD_BYTES));
    }

    #[test]
    fn empty_chat_text_publishes_nothing() {
        let path = unique_path("chat-empty");
        let broadcaster = Broadcaster::start_at(&path, 7).expect("start broadcaster");
        let ring = Ring::open(&path).expect("open");
        let mut consumer = Consumer::new(&ring, StartAt::Head);

        broadcaster.chat("sess", Speaker::User, "");
        thread::sleep(Duration::from_millis(50));
        assert_eq!(consumer.poll(&ring), Poll::Waiting);
    }

    #[test]
    fn dropping_the_broadcaster_unlinks_the_ring() {
        let path = unique_path("drop");
        let broadcaster = Broadcaster::start_at(&path, 1).expect("start broadcaster");
        broadcaster.update(snapshot("session-b", SessionState::Idle));
        drop(broadcaster);
        let deadline = Instant::now() + BROADCAST_PERIOD * 4;
        while std::path::Path::new(&path).exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!std::path::Path::new(&path).exists(), "ring file should be unlinked");
    }

    /// The full two-way trip: a haunt-side control ring delivers a targeted
    /// command, the UI side sees it via `drain_commands`, its `ack` lands as
    /// a `MakiAck` on the status ring.
    #[test]
    fn commands_round_trip_with_acks() {
        use crate::control::{self, AckResult, Command};

        let status_path = unique_path("cmd-status");
        let control_path = format!("{}/haunt-test-cmd-roundtrip.brng", crate::ring::ring_dir());
        let _ = std::fs::remove_file(&control_path);

        let broadcaster =
            Broadcaster::start_at(&status_path, std::process::id()).expect("start broadcaster");
        broadcaster.update(snapshot("session-a", SessionState::Working));

        let haunt = crate::ring::Ring::create(&control_path, crate::ring::HAUNT_NODE_ID, 0)
            .expect("create control ring");
        let mut producer = crate::ring::Producer::new(&haunt);
        haunt.update_status(crate::ring::Status::Running);

        // One tick to attach (`StartAt::Head` never replays), then publish.
        thread::sleep(BROADCAST_PERIOD * 2);
        let key = crate::auth::AuthKey::load_or_create().expect("auth key");
        let mut cmd = control::command_record(
            42,
            "session-a",
            &Command::Prompt("hello from haunt".into()),
            0,
        );
        key.sign_command(&mut cmd);
        producer.publish(crate::MakiCommand::MSG_ID.get(), &cmd);
        producer.flush();
        haunt.update_heartbeat(crate::ring::monotonic_nanos());

        let deadline = Instant::now() + BROADCAST_PERIOD * 6;
        let pending = loop {
            if let Some(p) = broadcaster
                .drain_commands()
                .into_iter()
                .find(|p| p.request_id == 42)
            {
                break p;
            }
            assert!(Instant::now() < deadline, "command never forwarded");
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            pending,
            PendingCommand {
                request_id: 42,
                session_id: "session-a".into(),
                command: Command::Prompt("hello from haunt".into()),
            }
        );

        let ring = Ring::open(&status_path).expect("open status ring");
        let mut consumer = Consumer::new(&ring, StartAt::Head);
        broadcaster.ack("session-a", 42, AckResult::Ok);
        let deadline = Instant::now() + BROADCAST_PERIOD * 6;
        let ack = loop {
            match consumer.poll(&ring) {
                Poll::Received { msg_id, .. } if msg_id == crate::MakiAck::MSG_ID.get() => {
                    let body = consumer.last_body(&ring);
                    break *unsafe { crate::schema::blit_runtime::from_bytes::<crate::MakiAck>(body) }
                        .expect("ack body");
                }
                Poll::Received { .. } => {}
                Poll::Waiting => thread::sleep(Duration::from_millis(20)),
                _ => panic!("unexpected poll outcome while awaiting ack"),
            }
            assert!(Instant::now() < deadline, "ack never published");
        };
        assert_eq!(ack.request_id, 42);
        assert_eq!(ack.result, AckResult::Ok as u8);
        assert_eq!(text::decode(&ack.session_id), "session-a");

        let _ = std::fs::remove_file(&control_path);
    }
}
