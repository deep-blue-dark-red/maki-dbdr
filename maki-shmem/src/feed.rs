//! The client side of the status feed: every live maki process publishes a
//! `maki-<pid>.brng`; [`Feed`] attaches to all of them and folds the newest
//! verified [`MakiStatus`] per session, plus the [`MakiAck`] round trips.

use crate::{
    MakiAck, MakiStatus,
    auth::AuthKey,
    broadcast::SessionState,
    control::AckResult,
    ring::{self, Consumer, Poll, Ring, StartAt},
    text,
};

/// A session whose producer stopped beating this long ago is considered ended.
const STALE_HEARTBEAT_NS: i64 = 15_000_000_000;

/// One session's newest state, decoded off its latest `MakiStatus`.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedSession {
    pub pid: u32,
    pub state: SessionState,
    pub context_used: u32,
    pub context_limit: u32,
    pub cost_usd: f64,
    pub session_id: String,
    pub title: String,
    pub last_tool: String,
}

/// An ack a maki process published for a command this client sent.
#[derive(Debug, Clone, PartialEq)]
pub struct AckInfo {
    pub request_id: u64,
    pub result: AckResult,
    pub session_id: String,
}

struct Attached {
    path: String,
    ring: Ring,
    consumer: Consumer,
    last: Option<FeedSession>,
}

/// Every live session ring, newest verified status per session.
#[derive(Default)]
pub struct Feed {
    attached: Vec<Attached>,
    pub sessions: Vec<FeedSession>,
    /// Acks drained since the last poll; one-shot, like haunt's.
    pub acks: Vec<AckInfo>,
}

impl Feed {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches to rings that appeared, drops vanished or silent ones, and
    /// drains everything published. Returns true when the session table or
    /// ack set changed.
    pub fn poll(&mut self) -> bool {
        let now = ring::monotonic_nanos();
        let live = status_ring_paths();
        self.attached.retain(|attached| {
            let (_, beat) = attached.ring.peer_health();
            live.contains(&attached.path) && (beat == 0 || now - beat < STALE_HEARTBEAT_NS)
        });

        for path in &live {
            if self.attached.iter().any(|a| &a.path == path) {
                continue;
            }
            if let Ok(ring) = Ring::attach(path, 0) {
                ring.register_consumer(ring::HAUNT_NODE_ID);
                // Earliest: statuses republish every period, so a fresh
                // reader still learns every session without waiting a tick.
                let consumer = Consumer::new(&ring, StartAt::Earliest);
                self.attached.push(Attached {
                    path: path.clone(),
                    ring,
                    consumer,
                    last: None,
                });
            }
        }

        let key = AuthKey::load_or_create();
        let mut acks = Vec::new();
        for attached in self.attached.iter_mut() {
            loop {
                match attached.consumer.poll(&attached.ring) {
                    Poll::Received { msg_id, .. } => {
                        let body = *attached.consumer.last_body(&attached.ring);
                        if msg_id == MakiStatus::MSG_ID.get() {
                            attached.last = decode_status(&body, key.as_ref());
                        } else if msg_id == MakiAck::MSG_ID.get()
                            && let Some(ack) = decode_ack(&body, key.as_ref())
                        {
                            acks.push(ack);
                        }
                    }
                    Poll::Waiting | Poll::Finished => break,
                    Poll::Lapped | Poll::Unsupported => {}
                }
            }
        }

        let mut sessions: Vec<FeedSession> =
            self.attached.iter().filter_map(|a| a.last.clone()).collect();
        sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        sessions.dedup_by(|a, b| a.session_id == b.session_id);

        let changed = sessions != self.sessions || !acks.is_empty();
        self.sessions = sessions;
        self.acks = acks;
        changed
    }
}

fn decode_status(body: &[u8], key: Option<&AuthKey>) -> Option<FeedSession> {
    let status = MakiStatus::copy_from_bytes(body)?;
    if key.is_some_and(|key| !key.verify_status(&status)) {
        return None;
    }
    let session_id = text::decode(&status.session_id).into_owned();
    if session_id.is_empty() {
        return None;
    }
    Some(FeedSession {
        pid: status.pid,
        state: SessionState::from_wire(status.state),
        context_used: status.context_used,
        context_limit: status.context_limit,
        cost_usd: status.cost_usd,
        session_id,
        title: text::decode(&status.session_title).into_owned(),
        last_tool: text::decode(&status.last_tool).into_owned(),
    })
}

fn decode_ack(body: &[u8], key: Option<&AuthKey>) -> Option<AckInfo> {
    let ack = MakiAck::copy_from_bytes(body)?;
    if key.is_some_and(|key| !key.verify_ack(&ack)) {
        return None;
    }
    Some(AckInfo {
        request_id: ack.request_id,
        result: AckResult::from_wire(ack.result)?,
        session_id: text::decode(&ack.session_id).into_owned(),
    })
}

/// Every maki status ring in the ring directory, sorted by name.
pub fn status_ring_paths() -> Vec<String> {
    let dir = ring::ring_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("maki-") && name.ends_with(".brng")
        })
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcast::{Broadcaster, SessionSnapshot};

    fn snapshot(session_id: &str) -> SessionSnapshot {
        SessionSnapshot {
            state: SessionState::Working,
            context_used: 12_000,
            context_limit: 200_000,
            cost_usd: 0.25,
            session_id: session_id.to_owned(),
            title: "feed test".to_owned(),
            last_tool: "read".to_owned(),
        }
    }

    #[test]
    fn feed_folds_statuses_and_acks_from_live_rings() {
        use crate::control::AckResult;

        let mut feed = Feed::new();
        feed.poll();
        // One Broadcaster per process: its ring path is pid-derived, so two
        // live at once would truncate each other.
        let broadcaster = Broadcaster::start().expect("start broadcaster");
        broadcaster.update(snapshot("feed-a"));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while feed
            .sessions
            .iter()
            .all(|s| s.session_id != "feed-a")
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
            feed.poll();
        }
        let session = feed
            .sessions
            .iter()
            .find(|s| s.session_id == "feed-a")
            .expect("session discovered");
        assert_eq!(session.context_used, 12_000);
        assert_eq!(session.title, "feed test");
        assert_eq!(session.state, SessionState::Working);

        broadcaster.ack("feed-a", 41, AckResult::Ok);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while feed
            .acks
            .iter()
            .all(|a| a.session_id != "feed-a")
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
            feed.poll();
        }
        let ack = feed
            .acks
            .iter()
            .find(|a| a.session_id == "feed-a")
            .expect("ack drained");
        assert_eq!(ack.request_id, 41);
        assert_eq!(ack.result, AckResult::Ok);
        drop(broadcaster);
    }

    #[test]
    fn tampered_status_is_refused() {
        let key = AuthKey::load_or_create().expect("key");
        let mut record = MakiStatus {
            pid: 7,
            state: SessionState::Working as u8,
            flags: 0,
            reserved_6: 0,
            timestamp_ns: 1,
            context_used: 5,
            context_limit: 100,
            cost_usd: 0.0,
            session_id: [0; 32],
            session_title: [0; 64],
            last_tool: [0; 32],
            mac: [0; 16],
            reserved_176: [0; 16],
        };
        text::encode(&mut record.session_id, "tampered");
        key.sign_status(&mut record);
        record.mac[0] ^= 0xff;
        assert!(decode_status(record.as_bytes(), Some(&key)).is_none());
    }
}
