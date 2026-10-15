//! The control half of the feed: haunt publishes command rings
//! (`haunt-<pid>.brng`) and every maki process polls all of them, keeping the
//! commands addressed to sessions it owns. Acks travel the other way as
//! [`MakiAck`] records on maki's own status ring, so a command's round trip
//! never opens a second channel.

use crate::{
    MakiCommand,
    auth::AuthKey,
    payload,
    ring::{self, Consumer, Poll, Ring, StartAt},
    schema::blit_runtime::from_bytes,
    text,
};

/// How long a control ring whose producer stopped beating is kept attached.
/// haunt beats on every poll (250ms), so this only trips on a dead haunt.
const CONTROL_STALE_NS: i64 = 5_000_000_000;

const CMD_REFRESH: u8 = 1;
const CMD_CANCEL: u8 = 2;
const CMD_PROMPT: u8 = 3;

/// A command haunt issued, decoded off the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct Incoming {
    pub request_id: u64,
    /// Owning-session filter: empty means "every session in this process"
    /// and is only valid for [`Command::Refresh`].
    pub target: String,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Republish the current status immediately.
    Refresh,
    /// Cancel the target session's current run.
    Cancel,
    /// Queue `payload` as a user message in the target session.
    Prompt(String),
}

/// What a maki process replies to a command, on the wire as `MakiAck.result`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckResult {
    Ok = 1,
    /// The target session is not owned by this process.
    NoSession = 2,
    /// The command was understood but refused by the session.
    Rejected = 3,
}

/// Every `haunt-<pid>.brng` this process is attached to.
#[derive(Default)]
pub struct ControlLinks {
    links: Vec<ControlLink>,
}

struct ControlLink {
    path: String,
    ring: Ring,
    consumer: Consumer,
    /// Highest verified request id from this ring; lower ids are replays.
    last_request_id: u64,
}

impl ControlLinks {
    /// Attaches to new command rings, drops dead ones, and returns every
    /// command published since the last poll. Rings replaced on disk (a
    /// restarted haunt) are re-attached by path. Records failing the HMAC
    /// check or the request-id watermark are dropped.
    pub fn poll(&mut self, key: &AuthKey) -> Vec<Incoming> {
        let live: Vec<String> = control_ring_paths();
        self.links
            .retain(|link| live.contains(&link.path) && !link.is_stale());

        let attached: Vec<String> = self.links.iter().map(|l| l.path.clone()).collect();
        for path in &live {
            if attached.contains(path) {
                continue;
            }
            if let Ok(ring) = Ring::attach(path, 0) {
                let consumer = Consumer::new(&ring, StartAt::Head);
                self.links.push(ControlLink {
                    path: path.clone(),
                    ring,
                    consumer,
                    last_request_id: 0,
                });
            }
        }

        let mut out = Vec::new();
        for link in &mut self.links {
            link.drain(key, &mut out);
        }
        out
    }
}

impl ControlLink {
    fn is_stale(&self) -> bool {
        let (status, beat) = self.ring.peer_health();
        (status == ring::Status::Stopped)
            || (beat != 0 && ring::monotonic_nanos() - beat > CONTROL_STALE_NS)
    }

    fn drain(&mut self, key: &AuthKey, out: &mut Vec<Incoming>) {
        loop {
            match self.consumer.poll(&self.ring) {
                Poll::Received { msg_id, .. }
                    if msg_id == MakiCommand::MSG_ID.get()
                        && let Some(record) = (unsafe { from_bytes(self.consumer.last_body(&self.ring)) })
                        && key.verify_command(record)
                        && record.request_id > self.last_request_id
                        && let Some(incoming) = decode(record) =>
                {
                    self.last_request_id = record.request_id;
                    out.push(incoming)
                }
                Poll::Received { .. } => {}
                Poll::Waiting | Poll::Finished => return,
                Poll::Lapped | Poll::Unsupported => {}
            }
        }
    }
}

fn decode(record: &MakiCommand) -> Option<Incoming> {
    let target = text::decode(&record.target_session).into_owned();
    let command = match record.cmd {
        CMD_REFRESH => Command::Refresh,
        CMD_CANCEL if !target.is_empty() => Command::Cancel,
        CMD_PROMPT if !target.is_empty() => Command::Prompt(payload::decode(record)),
        _ => return None,
    };
    Some(Incoming {
        request_id: record.request_id,
        target,
        command,
    })
}

/// A command to publish on a control ring, from the haunt side. The caller
/// signs it (`AuthKey::sign_command`) before it reaches the wire.
pub fn command_record(
    request_id: u64,
    target: &str,
    command: &Command,
    timestamp_ns: u64,
) -> MakiCommand {
    let mut record = MakiCommand {
        request_id,
        cmd: match command {
            Command::Refresh => CMD_REFRESH,
            Command::Cancel => CMD_CANCEL,
            Command::Prompt(_) => CMD_PROMPT,
        },
        flags: 0,
        payload_len: 0,
        reserved_12: [0; 4],
        timestamp_ns,
        target_session: [0; 32],
        payload: [0; 256],
        mac: [0; 16],
        reserved_328: [0; 8],
    };
    text::encode(&mut record.target_session, target);
    if let Command::Prompt(text) = command {
        payload::encode(&mut record, text);
    }
    record
}

/// Every command ring in the ring directory, sorted by name.
pub fn control_ring_paths() -> Vec<String> {
    let dir = ring::ring_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("haunt-") && name.ends_with(".brng")
        })
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::Hello;

    fn unique_path(tag: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("{}/haunt-test-{}-{tag}.brng", ring::ring_dir(), n)
    }

    /// Ring-scanning tests share the directory (and this key), so they
    /// must not observe each other's rings: serialize them.
    static SCAN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn test_key() -> crate::auth::AuthKey {
        crate::auth::AuthKey::from_bytes([9; crate::auth::KEY_BYTES])
    }

    fn signed(request_id: u64, target: &str, command: &Command, key: &crate::auth::AuthKey) -> MakiCommand {
        let mut record = command_record(request_id, target, command, 0);
        key.sign_command(&mut record);
        record
    }

    fn haunt_ring(path: &str) -> Ring {
        let ring = Ring::create(path, ring::HAUNT_NODE_ID, 0).expect("create control ring");
        let mut producer = ring::Producer::new(&ring);
        let mut hello = Hello {
            magic: 0x4D414B49,
            version: 1,
            role: 0,
            reserved_7: 0,
            pid: std::process::id(),
            reserved_12: [0; 4],
            timestamp_ns: 0,
            sender_name: [0; 64],
            reserved_88: [0; 8],
        };
        text::encode(&mut hello.sender_name, "haunt-test");
        producer.publish(Hello::MSG_ID.get(), &hello);
        producer.flush();
        ring.update_status(ring::Status::Running);
        ring.update_heartbeat(ring::monotonic_nanos());
        ring
    }

    #[test]
    fn command_record_roundtrips_through_decode() {
        for command in [
            Command::Refresh,
            Command::Cancel,
            Command::Prompt("continue the refactor".into()),
        ] {
            let record = command_record(7, "sess-1", &command, 42);
            assert_eq!(record.request_id, 7);
            assert_eq!(record.timestamp_ns, 42);
            let incoming = decode(&record).expect("decodable");
            assert_eq!(incoming.request_id, 7);
            assert_eq!(incoming.target, "sess-1");
            assert_eq!(incoming.command, command);
        }
    }

    #[test]
    fn broadcast_cancel_and_prompt_are_refused() {
        assert!(decode(&command_record(1, "", &Command::Cancel, 0)).is_none());
        assert!(decode(&command_record(1, "", &Command::Prompt("x".into()), 0)).is_none());
        assert!(decode(&command_record(1, "", &Command::Refresh, 0)).is_some());
    }

    #[test]
    fn unknown_command_code_is_refused() {
        let mut record = command_record(1, "s", &Command::Refresh, 0);
        record.cmd = 99;
        assert!(decode(&record).is_none());
    }

    #[test]
    fn unsigned_tampered_and_replayed_commands_are_dropped() {
        let _scan = SCAN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = unique_path("auth");
        let ring = haunt_ring(&path);
        let mut producer = ring::Producer::new(&ring);
        let key = test_key();
        let mut links = ControlLinks::default();
        assert!(links.poll(&key).is_empty(), "attach only");
        ring.update_heartbeat(ring::monotonic_nanos());

        // Unsigned: refused.
        producer.publish(
            crate::MakiCommand::MSG_ID.get(),
            &command_record(1, "sess-a", &Command::Cancel, 0),
        );
        // Tampered after signing: refused.
        let mut tampered = signed(2, "sess-a", &Command::Cancel, &key);
        tampered.cmd = CMD_REFRESH;
        producer.publish(crate::MakiCommand::MSG_ID.get(), &tampered);
        // Genuine.
        producer.publish(
            crate::MakiCommand::MSG_ID.get(),
            &signed(3, "sess-a", &Command::Cancel, &key),
        );
        // Replay of 3 and anything older: refused.
        producer.publish(
            crate::MakiCommand::MSG_ID.get(),
            &signed(3, "sess-a", &Command::Cancel, &key),
        );
        producer.publish(
            crate::MakiCommand::MSG_ID.get(),
            &signed(2, "sess-a", &Command::Prompt("late".into()), &key),
        );
        producer.flush();

        let got = links.poll(&key);
        assert_eq!(
            got.as_slice(),
            [Incoming {
                request_id: 3,
                target: "sess-a".into(),
                command: Command::Cancel,
            }],
            "only the genuine, newest command survives"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn links_attach_drain_and_drop() {
        let _scan = SCAN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = unique_path("links");
        let ring = haunt_ring(&path);
        let mut producer = ring::Producer::new(&ring);

        let mut links = ControlLinks::default();
        let key = test_key();
        assert!(links.poll(&key).is_empty(), "nothing attached yet");

        // `StartAt::Head` only sees records published after the attach, so
        // the command has to land once the link exists.
        producer.publish(
            crate::MakiCommand::MSG_ID.get(),
            &signed(11, "sess-a", &Command::Cancel, &key),
        );
        producer.flush();
        ring.update_heartbeat(ring::monotonic_nanos());

        let got = links.poll(&key);
        assert_eq!(
            got,
            vec![Incoming {
                request_id: 11,
                target: "sess-a".into(),
                command: Command::Cancel,
            }]
        );
        assert!(links.poll(&key).is_empty(), "already drained");

        // Dead producer: the link goes stale, but the path stays live, so a
        // replaced haunt ring — same path, new inode — is still picked up.
        ring.update_heartbeat(ring::monotonic_nanos() - CONTROL_STALE_NS * 2);
        assert!(links.poll(&key).is_empty());

        let tmp = unique_path("replacement");
        let replacement = haunt_ring(&tmp);
        std::fs::rename(&tmp, &path).expect("replace control ring");
        let mut producer = ring::Producer::new(&replacement);

        // One tick drops the stale link and re-attaches the new file; a
        // command published during that swap tick is missed (documented),
        // anything after it is delivered.
        thread::sleep(Duration::from_millis(600));
        assert!(links.poll(&key).is_empty(), "swap tick re-attaches only");

        producer.publish(
            crate::MakiCommand::MSG_ID.get(),
            &signed(13, "sess-a", &Command::Cancel, &key),
        );
        producer.flush();
        let got = links.poll(&key);
        assert_eq!(got.len(), 1, "replacement ring picked up");
        assert_eq!(got[0].request_id, 13);
        let _ = std::fs::remove_file(&path);
    }
}
