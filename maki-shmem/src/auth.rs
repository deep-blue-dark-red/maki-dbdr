//! Record authentication: every `MakiCommand`, `MakiStatus` and `MakiAck`
//! carries an HMAC-SHA256 tag over its bytes with the tag field zeroed,
//! keyed by one per-user secret in `maki-auth.key` (0600) beside the rings.
//!
//! The trust boundary stays "same user" — anyone who can read the key file
//! can forge records — but tampered, corrupted or foreign-writer records are
//! refused instead of decoded, and captured commands cannot be replayed
//! (see `control::ControlLink`'s request-id watermark).

use std::{fs, path::PathBuf};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{
    MakiAck, MakiCommand, MakiStatus,
    ring,
    schema::blit_runtime::as_bytes,
};

pub const KEY_BYTES: usize = 32;
pub const TAG_BYTES: usize = 16;

const KEY_FILE: &str = "maki-auth.key";
const KEY_FILE_MODE: u32 = 0o600;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct AuthKey([u8; KEY_BYTES]);

impl std::fmt::Debug for AuthKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthKey").finish_non_exhaustive()
    }
}

impl AuthKey {
    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    /// Loads the shared key, creating it (atomically, 0600) when absent.
    /// `None` when the ring directory is unusable — callers degrade to an
    /// unsigned feed that the other side refuses.
    pub fn load_or_create() -> Option<AuthKey> {
        let path = key_path();
        if let Ok(bytes) = fs::read(&path) {
            return Some(AuthKey(bytes.as_slice().try_into().ok()?));
        }
        let mut key = [0u8; KEY_BYTES];
        fill_random(&mut key)?;
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, key).is_err() || set_private(&tmp).is_none() {
            let _ = fs::remove_file(&tmp);
            return None;
        }
        // Losing the race is fine: the survivor's key is the one in force.
        if fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
            let bytes = fs::read(&path).ok()?;
            return Some(AuthKey(bytes.as_slice().try_into().ok()?));
        }
        Some(AuthKey(key))
    }

    /// The first 16 bytes of the HMAC-SHA256 output: NIST SP 800-107
    /// permits truncation to half the hash length, and 128 bits dwarfs
    /// anything else in this threat model.
    fn tag(&self, message: &[u8]) -> [u8; TAG_BYTES] {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("hmac takes any key length");
        mac.update(message);
        let full = mac.finalize().into_bytes();
        let mut tag = [0u8; TAG_BYTES];
        tag.copy_from_slice(&full[..TAG_BYTES]);
        tag
    }

    fn verify_tag(&self, message: &[u8], tag: &[u8]) -> bool {
        constant_time_eq(&self.tag(message), tag)
    }

    pub fn sign_command(&self, record: &mut MakiCommand) {
        record.mac = self.tag(&message(record, MakiCommand::OFFSET_mac));
    }

    pub fn verify_command(&self, record: &MakiCommand) -> bool {
        self.verify_tag(&message(record, MakiCommand::OFFSET_mac), &record.mac)
    }

    pub fn sign_status(&self, record: &mut MakiStatus) {
        record.mac = self.tag(&message(record, MakiStatus::OFFSET_mac));
    }

    pub fn verify_status(&self, record: &MakiStatus) -> bool {
        self.verify_tag(&message(record, MakiStatus::OFFSET_mac), &record.mac)
    }

    pub fn sign_ack(&self, record: &mut MakiAck) {
        record.mac = self.tag(&message(record, MakiAck::OFFSET_mac));
    }

    pub fn verify_ack(&self, record: &MakiAck) -> bool {
        self.verify_tag(&message(record, MakiAck::OFFSET_mac), &record.mac)
    }
}

/// The record's bytes with the tag field zeroed: exactly what is signed.
fn message<T: Copy>(record: &T, tag_offset: usize) -> Vec<u8> {
    let mut bytes = unsafe { as_bytes(record) }.to_vec();
    bytes[tag_offset..tag_offset + TAG_BYTES].fill(0);
    bytes
}

/// Branch-free byte comparison, so tag mismatches do not leak timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn key_path() -> PathBuf {
    PathBuf::from(ring::ring_dir()).join(KEY_FILE)
}

fn fill_random(out: &mut [u8]) -> Option<()> {
    for chunk in out.chunks_mut(256) {
        if unsafe { libc::getentropy(chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) } != 0 {
            return None;
        }
    }
    Some(())
}

#[cfg(unix)]
fn set_private(path: &PathBuf) -> Option<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(KEY_FILE_MODE)).ok()
}

#[cfg(not(unix))]
fn set_private(_path: &PathBuf) -> Option<()> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_then_verify_round_trips() {
        let key = AuthKey::from_bytes([7; KEY_BYTES]);
        let mut command = MakiCommand {
            request_id: 1,
            cmd: 3,
            flags: 0,
            payload_len: 3,
            reserved_12: [0; 4],
            timestamp_ns: 9,
            target_session: [0; 32],
            payload: [0; 256],
            mac: [0; 16],
            reserved_328: [0; 8],
        };
        key.sign_command(&mut command);
        assert!(key.verify_command(&command));

        command.cmd = 2;
        assert!(!key.verify_command(&command), "tampered cmd rejected");

        let mut status = MakiStatus {
            pid: 1,
            state: 1,
            flags: 0,
            reserved_6: 0,
            timestamp_ns: 2,
            context_used: 3,
            context_limit: 4,
            cost_usd: 0.5,
            session_id: [0; 32],
            session_title: [0; 64],
            last_tool: [0; 32],
            mac: [0; 16],
            reserved_176: [0; 16],
        };
        key.sign_status(&mut status);
        assert!(key.verify_status(&status));
        status.context_used = 99;
        assert!(!key.verify_status(&status));

        let mut ack = MakiAck {
            request_id: 5,
            result: 1,
            reserved_9: [0; 7],
            timestamp_ns: 6,
            session_id: [0; 32],
            mac: [0; 16],
            reserved_72: [0; 16],
        };
        key.sign_ack(&mut ack);
        assert!(key.verify_ack(&ack));
        ack.result = 3;
        assert!(!key.verify_ack(&ack));
    }

    #[test]
    fn wrong_key_is_refused() {
        let signer = AuthKey::from_bytes([1; KEY_BYTES]);
        let verifier = AuthKey::from_bytes([2; KEY_BYTES]);
        let mut command = MakiCommand {
            request_id: 1,
            cmd: 1,
            flags: 0,
            payload_len: 0,
            reserved_12: [0; 4],
            timestamp_ns: 0,
            target_session: [0; 32],
            payload: [0; 256],
            mac: [0; 16],
            reserved_328: [0; 8],
        };
        signer.sign_command(&mut command);
        assert!(!verifier.verify_command(&command));
    }

    #[test]
    fn load_or_create_is_stable_across_calls() {
        let first = AuthKey::load_or_create().expect("key");
        let second = AuthKey::load_or_create().expect("key");
        assert!(same_bytes(&first, &second));
    }

    fn same_bytes(a: &AuthKey, b: &AuthKey) -> bool {
        a.0 == b.0
    }
}
