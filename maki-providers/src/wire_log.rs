//! Binary API log format (`MLOG`).
//!
//! Chat APIs are stateless: every turn re-sends the entire conversation plus the
//! (identical) tool and system blocks. Logging each request verbatim is O(K²) in
//! turns and mostly duplicate bytes. This format stores each request/response
//! once and byte-exactly, so the raw wire message is always recoverable for
//! debugging, while repeated content is deduplicated.
//!
//! Two dedup strategies share one file:
//! - **Fragment interning** (providers that hand us [`Fragments`], e.g. Anthropic):
//!   the `messages` array is split at message boundaries, each fragment interned
//!   once as a `DEF` and referenced by id. Immune to Anthropic's sliding
//!   `cache_control` window — only the messages whose bytes actually changed get
//!   re-emitted.
//! - **Byte diff** (every other provider): each request is stored as a
//!   prefix/suffix patch against the previous request's raw bytes.
//!
//! Either way the reconstructed bytes equal exactly what went over the wire.
//!
//! ## Layout (little-endian)
//! ```text
//! File:   "MLOG" | u8 version
//! Record: u64 ts_ms | u8 type | u32 len | payload      (type & 0x80 => payload is zstd)
//!   REQ_FULL  (1)  u32 uri_len | uri | body
//!   REQ_PATCH (2)  u32 uri_len | uri | u32 prefix | u32 suffix | diff   (patch over body)
//!   RESPONSE  (3)  u16 status  | u32 ct_len | content_type | body
//!   DEF       (4)  u32 id | blob                                        (intern one fragment)
//!   REQ_REFS  (5)  u32 uri_len | uri | u32 pre_id | u32 n | n×u32 msg_id | u32 post_id
//! ```
//! A `REQ_REFS` body reconstructs as `pre + messages.join(",") + post`.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde_json::Value;

pub const MAGIC: &[u8; 4] = b"MLOG";
pub const VERSION: u8 = 1;

const REQ_FULL: u8 = 1;
const REQ_PATCH: u8 = 2;
const RESPONSE: u8 = 3;
const DEF: u8 = 4;
const REQ_REFS: u8 = 5;
const COMPRESSED: u8 = 0x80;
const TYPE_MASK: u8 = 0x7f;

/// Only bother compressing payloads above this size; below it zstd framing
/// overhead usually outweighs the gain.
const COMPRESS_THRESHOLD: usize = 256;
const ZSTD_LEVEL: i32 = 3;

/// Per-message wire fragments whose concatenation is byte-identical to the sent
/// body: `preamble + messages.join(",") + postamble`. Produced by
/// [`fragment_body`]; the caller verifies the reconstruction before use.
pub struct Fragments {
    pub preamble: Vec<u8>,
    pub messages: Vec<Vec<u8>>,
    pub postamble: Vec<u8>,
}

impl Fragments {
    /// Reconstruct the exact body bytes these fragments came from.
    pub fn reconstruct(&self) -> Vec<u8> {
        let sep = self.messages.len().saturating_sub(1);
        let cap = self.preamble.len()
            + self.postamble.len()
            + sep
            + self.messages.iter().map(Vec::len).sum::<usize>();
        let mut out = Vec::with_capacity(cap);
        out.extend_from_slice(&self.preamble);
        for (i, m) in self.messages.iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.extend_from_slice(m);
        }
        out.extend_from_slice(&self.postamble);
        out
    }
}

/// Split a request body into interned-per-message fragments.
///
/// `array_key` names the messages array (`"messages"`, `"contents"`, …). Returns
/// `None` if the body has no such array or the boundaries can't be located; the
/// caller then falls back to a full/patch record. The result is only trusted
/// after checking [`Fragments::reconstruct`] equals the real bytes, so a wrong
/// guess degrades compression but never fidelity.
pub fn fragment_body(body: &Value, array_key: &str) -> Option<Fragments> {
    let arr = body.get(array_key)?.as_array()?;
    let messages: Vec<Vec<u8>> = arr
        .iter()
        .map(serde_json::to_vec)
        .collect::<Result<_, _>>()
        .ok()?;

    // Serialize the same body with an empty array in place of the messages, so
    // everything outside the array keeps byte-identical serialization. The
    // empty array serializes as `[]`; split around those two bytes.
    let mut template = body.clone();
    template[array_key] = Value::Array(Vec::new());
    let tmpl = serde_json::to_vec(&template).ok()?;

    let needle = format!("\"{array_key}\":[]");
    let pos = find(&tmpl, needle.as_bytes())?;
    let bracket_open = pos + needle.len() - 2; // index of '['
    let preamble = tmpl[..=bracket_open].to_vec(); // ends with '['
    let postamble = tmpl[bracket_open + 1..].to_vec(); // starts with ']'

    Some(Fragments {
        preamble,
        messages,
        postamble,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SessionLog {
    /// Raw bytes of the previous request, for prefix/suffix diffing.
    last_body: Vec<u8>,
    /// Interned fragment bytes -> id.
    blobs: HashMap<Vec<u8>, u32>,
    next_id: u32,
}

/// Keyed by the session's current log file. Renaming the file (on session
/// rename, in the UI layer) orphans the old entry; the next request starts a
/// fresh interning table under the new path, which is byte-exact but re-DEFs
/// shared blobs once — an acceptable cost for a debug log.
static SESSIONS: LazyLock<Mutex<HashMap<PathBuf, SessionLog>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Longest common prefix/suffix diff of `new` against `old`.
pub fn compute_patch(old: &[u8], new: &[u8]) -> (u32, u32, Vec<u8>) {
    let min_len = old.len().min(new.len());
    let mut prefix = 0;
    while prefix < min_len && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let max_suffix = min_len - prefix;
    let mut suffix = 0;
    while suffix < max_suffix
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let diff = new[prefix..new.len() - suffix].to_vec();
    (prefix as u32, suffix as u32, diff)
}

fn open_append(path: &Path) -> std::io::Result<File> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if file.metadata()?.len() == 0 {
        file.write_all(MAGIC)?;
        file.write_all(&[VERSION])?;
    }
    Ok(file)
}

fn write_record(file: &mut File, ts_ms: u64, rtype: u8, payload: &[u8]) -> std::io::Result<()> {
    let compressed = if payload.len() > COMPRESS_THRESHOLD {
        zstd::stream::encode_all(payload, ZSTD_LEVEL)
            .ok()
            .filter(|c| c.len() < payload.len())
    } else {
        None
    };
    let (rtype, bytes) = match &compressed {
        Some(c) => (rtype | COMPRESSED, c.as_slice()),
        None => (rtype, payload),
    };
    let mut header = [0u8; 13];
    header[..8].copy_from_slice(&ts_ms.to_le_bytes());
    header[8] = rtype;
    header[9..].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    file.write_all(&header)?;
    file.write_all(bytes)
}

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(buf: &mut Vec<u8>, b: &[u8]) {
    put_u32(buf, b.len() as u32);
    buf.extend_from_slice(b);
}

/// Log a request. `fragments` (verified byte-exact by the caller) enables
/// message interning; otherwise the request is diffed against the previous one.
/// Errors are swallowed — logging must never break a request.
pub fn log_request(
    path: &Path,
    ts_ms: u64,
    uri: &str,
    raw_body: &[u8],
    fragments: Option<Fragments>,
) {
    // Never trust fragments that don't reproduce the exact bytes: fall back to
    // a byte-exact diff/full record instead.
    let fragments = fragments.filter(|f| f.reconstruct() == raw_body);

    let Ok(mut map) = SESSIONS.lock() else { return };
    let sess = map.entry(path.to_path_buf()).or_default();
    let Ok(mut file) = open_append(path) else { return };

    let ok = match fragments {
        Some(frags) => write_refs(&mut file, ts_ms, uri, &frags, sess),
        None => write_diff(&mut file, ts_ms, uri, raw_body, sess),
    };
    if ok.is_ok() {
        sess.last_body = raw_body.to_vec();
    }
}

fn write_refs(
    file: &mut File,
    ts_ms: u64,
    uri: &str,
    frags: &Fragments,
    sess: &mut SessionLog,
) -> std::io::Result<()> {
    let pre = intern(file, ts_ms, &frags.preamble, sess)?;
    let mut msg_ids = Vec::with_capacity(frags.messages.len());
    for m in &frags.messages {
        msg_ids.push(intern(file, ts_ms, m, sess)?);
    }
    let post = intern(file, ts_ms, &frags.postamble, sess)?;

    let mut payload = Vec::new();
    put_bytes(&mut payload, uri.as_bytes());
    put_u32(&mut payload, pre);
    put_u32(&mut payload, msg_ids.len() as u32);
    for id in msg_ids {
        put_u32(&mut payload, id);
    }
    put_u32(&mut payload, post);
    write_record(file, ts_ms, REQ_REFS, &payload)
}

/// Return the id for `bytes`, emitting a `DEF` record first if it's novel.
fn intern(file: &mut File, ts_ms: u64, bytes: &[u8], sess: &mut SessionLog) -> std::io::Result<u32> {
    if let Some(&id) = sess.blobs.get(bytes) {
        return Ok(id);
    }
    let id = sess.next_id;
    sess.next_id += 1;
    let mut payload = Vec::with_capacity(4 + bytes.len());
    put_u32(&mut payload, id);
    payload.extend_from_slice(bytes);
    write_record(file, ts_ms, DEF, &payload)?;
    sess.blobs.insert(bytes.to_vec(), id);
    Ok(id)
}

fn write_diff(
    file: &mut File,
    ts_ms: u64,
    uri: &str,
    raw_body: &[u8],
    sess: &mut SessionLog,
) -> std::io::Result<()> {
    let mut payload = Vec::new();
    put_bytes(&mut payload, uri.as_bytes());

    if sess.last_body.is_empty() {
        payload.extend_from_slice(raw_body);
        return write_record(file, ts_ms, REQ_FULL, &payload);
    }
    let (prefix, suffix, diff) = compute_patch(&sess.last_body, raw_body);
    // Keyframe when the patch saves little, so a corrupt patch can't poison a
    // long chain and reconstruction stays cheap.
    if diff.len() + 8 >= raw_body.len() {
        payload.extend_from_slice(raw_body);
        write_record(file, ts_ms, REQ_FULL, &payload)
    } else {
        put_u32(&mut payload, prefix);
        put_u32(&mut payload, suffix);
        payload.extend_from_slice(&diff);
        write_record(file, ts_ms, REQ_PATCH, &payload)
    }
}

/// Log a response. Stores the exact received bytes.
pub fn log_response(path: &Path, ts_ms: u64, status: u16, content_type: &str, raw_body: &[u8]) {
    let Ok(_guard) = SESSIONS.lock() else { return };
    let Ok(mut file) = open_append(path) else { return };
    let mut payload = Vec::with_capacity(6 + content_type.len() + raw_body.len());
    payload.extend_from_slice(&status.to_le_bytes());
    put_bytes(&mut payload, content_type.as_bytes());
    payload.extend_from_slice(raw_body);
    let _ = write_record(&mut file, ts_ms, RESPONSE, &payload);
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// A decoded record with its bytes reconstructed byte-exactly.
pub enum Record {
    Request {
        ts_ms: u64,
        uri: String,
        body: Vec<u8>,
    },
    Response {
        ts_ms: u64,
        status: u16,
        content_type: String,
        body: Vec<u8>,
    },
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn lp(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

/// Decode a whole `.mlog` file into ordered records.
pub fn read_file(path: &Path) -> std::io::Result<Vec<Record>> {
    let data = std::fs::read(path)?;
    read_bytes(&data)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed MLOG file"))
}

fn read_bytes(data: &[u8]) -> Option<Vec<Record>> {
    let mut cur = Cursor { buf: data, pos: 0 };
    if cur.take(4)? != MAGIC || cur.take(1)?[0] != VERSION {
        return None;
    }

    let mut blobs: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut last_body: Vec<u8> = Vec::new();
    let mut out = Vec::new();

    while cur.pos < data.len() {
        let ts_ms = cur.u64()?;
        let rtype = cur.take(1)?[0];
        let len = cur.u32()? as usize;
        let raw = cur.take(len)?;
        let payload = if rtype & COMPRESSED != 0 {
            zstd::stream::decode_all(raw).ok()?
        } else {
            raw.to_vec()
        };
        let mut p = Cursor {
            buf: &payload,
            pos: 0,
        };

        match rtype & TYPE_MASK {
            DEF => {
                let id = p.u32()?;
                blobs.insert(id, payload[p.pos..].to_vec());
            }
            REQ_FULL => {
                let uri = String::from_utf8_lossy(p.lp()?).into_owned();
                let body = payload[p.pos..].to_vec();
                last_body = body.clone();
                out.push(Record::Request { ts_ms, uri, body });
            }
            REQ_PATCH => {
                let uri = String::from_utf8_lossy(p.lp()?).into_owned();
                let prefix = p.u32()? as usize;
                let suffix = p.u32()? as usize;
                let diff = &payload[p.pos..];
                if prefix + suffix > last_body.len() {
                    return None;
                }
                let mut body = Vec::with_capacity(prefix + diff.len() + suffix);
                body.extend_from_slice(&last_body[..prefix]);
                body.extend_from_slice(diff);
                body.extend_from_slice(&last_body[last_body.len() - suffix..]);
                last_body = body.clone();
                out.push(Record::Request { ts_ms, uri, body });
            }
            REQ_REFS => {
                let uri = String::from_utf8_lossy(p.lp()?).into_owned();
                let pre = blobs.get(&p.u32()?)?.clone();
                let n = p.u32()? as usize;
                let mut msgs = Vec::with_capacity(n);
                for _ in 0..n {
                    msgs.push(blobs.get(&p.u32()?)?.clone());
                }
                let post = blobs.get(&p.u32()?)?.clone();
                let frags = Fragments {
                    preamble: pre,
                    messages: msgs,
                    postamble: post,
                };
                let body = frags.reconstruct();
                last_body = body.clone();
                out.push(Record::Request { ts_ms, uri, body });
            }
            RESPONSE => {
                let status = p.u16()?;
                let content_type = String::from_utf8_lossy(p.lp()?).into_owned();
                let body = payload[p.pos..].to_vec();
                out.push(Record::Response {
                    ts_ms,
                    status,
                    content_type,
                    body,
                });
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Collapse an SSE `event-stream` response into a compact reconstruction of the
/// assistant turn (text/reasoning/usage). Used by the viewer for readability;
/// non-SSE bodies are returned parsed as-is. Byte-exact recovery still lives in
/// the record itself.
pub fn clean_response_body(body_str: &str, content_type: Option<&str>) -> Value {
    let is_sse = content_type.is_some_and(|ct| ct.contains("event-stream"))
        || body_str.contains("data: ");
    if !is_sse {
        return serde_json::from_str(body_str)
            .unwrap_or_else(|_| Value::String(body_str.to_string()));
    }

    let mut assistant = String::new();
    let mut reasoning = String::new();
    let mut usage = Value::Null;
    let mut model = String::new();

    for line in body_str.lines() {
        let Some(data) = line.trim().strip_prefix("data: ") else {
            continue;
        };
        if data.trim() == "[DONE]" {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(m) = val["model"].as_str().filter(|_| model.is_empty()) {
            model = m.to_string();
        }
        // OpenAI-style streaming delta.
        if let Some(delta) = val["choices"][0].get("delta") {
            if let Some(c) = delta["content"].as_str() {
                assistant.push_str(c);
            }
            if let Some(r) = delta["reasoning"].as_str() {
                reasoning.push_str(r);
            } else if let Some(r) = delta["reasoning_content"].as_str() {
                reasoning.push_str(r);
            }
        }
        // Anthropic-style content_block_delta.
        if let Some(delta) = val.get("delta") {
            if let Some(t) = delta["text"].as_str() {
                assistant.push_str(t);
            }
            if let Some(t) = delta["thinking"].as_str() {
                reasoning.push_str(t);
            }
        }
        if val.get("usage").is_some() {
            usage = val["usage"].clone();
        }
    }

    let mut map = serde_json::Map::new();
    map.insert("stream_reconstructed".into(), Value::Bool(true));
    if !model.is_empty() {
        map.insert("model".into(), Value::String(model));
    }
    map.insert("content".into(), Value::String(assistant));
    if !reasoning.is_empty() {
        map.insert("reasoning".into(), Value::String(reasoning));
    }
    if !usage.is_null() {
        map.insert("usage".into(), usage);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn patch_roundtrip() {
        let old = b"the quick brown fox";
        let new = b"the very quick brown fox";
        let (prefix, suffix, diff) = compute_patch(old, new);
        let mut rebuilt = Vec::new();
        rebuilt.extend_from_slice(&old[..prefix as usize]);
        rebuilt.extend_from_slice(&diff);
        rebuilt.extend_from_slice(&old[old.len() - suffix as usize..]);
        assert_eq!(rebuilt, new);
    }

    #[test]
    fn fragments_are_byte_exact() {
        let body = json!({
            "max_tokens": 100,
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello"},
            ],
            "model": "claude",
            "tools": [{"name": "read"}],
        });
        let expected = serde_json::to_vec(&body).unwrap();
        let frags = fragment_body(&body, "messages").unwrap();
        assert_eq!(frags.reconstruct(), expected);
        assert_eq!(frags.messages.len(), 2);
    }

    #[test]
    fn fragments_empty_messages() {
        let body = json!({"messages": [], "model": "x"});
        let expected = serde_json::to_vec(&body).unwrap();
        let frags = fragment_body(&body, "messages").unwrap();
        assert_eq!(frags.reconstruct(), expected);
    }

    fn request_bodies(records: &[Record]) -> Vec<Vec<u8>> {
        records
            .iter()
            .filter_map(|r| match r {
                Record::Request { body, .. } => Some(body.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn file_roundtrip_dedups_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.mlog");
        SESSIONS.lock().unwrap().remove(&path);

        // A realistically large, identical tool block: the whole point is that
        // this is stored once, not re-sent whole every turn.
        let big_tool = json!({
            "name": "read_file",
            "description": "x".repeat(4096),
        });
        let body1 = json!({
            "messages": [{"role": "user", "content": "one"}],
            "tools": [big_tool.clone()],
        });
        let body2 = json!({
            "messages": [
                {"role": "user", "content": "one"},
                {"role": "assistant", "content": "two"},
            ],
            "tools": [big_tool],
        });
        let raw1 = serde_json::to_vec(&body1).unwrap();
        let raw2 = serde_json::to_vec(&body2).unwrap();

        let f1 = fragment_body(&body1, "messages").unwrap();
        assert_eq!(f1.reconstruct(), raw1);
        log_request(&path, 1000, "https://api/v1/messages", &raw1, Some(f1));

        let f2 = fragment_body(&body2, "messages").unwrap();
        assert_eq!(f2.reconstruct(), raw2);
        log_request(&path, 2000, "https://api/v1/messages", &raw2, Some(f2));

        log_response(&path, 2500, 200, "text/event-stream", b"data: {\"x\":1}\n");

        let requests = request_bodies(&read_file(&path).unwrap());
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], raw1);
        assert_eq!(requests[1], raw2);

        // The shared user message + tools blob must be interned once: the file
        // is far smaller than storing both bodies whole.
        let file_len = std::fs::metadata(&path).unwrap().len() as usize;
        assert!(file_len < raw1.len() + raw2.len());

        SESSIONS.lock().unwrap().remove(&path);
    }

    #[test]
    fn diff_path_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diff.mlog");
        SESSIONS.lock().unwrap().remove(&path);

        let raw1 = br#"{"messages":[{"role":"user","content":"aaaa"}],"model":"gpt"}"#.to_vec();
        let raw2 =
            br#"{"messages":[{"role":"user","content":"aaaa"},{"role":"assistant","content":"b"}],"model":"gpt"}"#
                .to_vec();
        log_request(&path, 1, "https://api/chat", &raw1, None);
        log_request(&path, 2, "https://api/chat", &raw2, None);

        let requests = request_bodies(&read_file(&path).unwrap());
        assert_eq!(requests, vec![raw1, raw2]);
        SESSIONS.lock().unwrap().remove(&path);
    }
}
