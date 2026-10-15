//! String payloads for `MakiCommand.payload`: raw utf8 when it fits, zstd
//! when compression buys room (flagged in `MakiCommand::flags`), hard
//! truncation only as the last resort for incompressible text.

use crate::MakiCommand;

/// `MakiCommand.flags` bit for a zstd-compressed payload.
pub const FLAG_ZSTD: u8 = 1;

const PAYLOAD_BYTES: usize = 256;
const ZSTD_LEVEL: i32 = 3;
/// Decompression cap: a prompt orders of magnitude larger than the field is
/// hostile, not useful.
const DECOMPRESS_CAP: usize = 64 * 1024;

/// Fills `flags`/`payload`/`payload_len` from `text`.
pub fn encode(record: &mut MakiCommand, text: &str) {
    record.payload = [0; PAYLOAD_BYTES];
    record.flags = 0;
    if text.len() <= PAYLOAD_BYTES {
        record.payload[..text.len()].copy_from_slice(text.as_bytes());
        record.payload_len = text.len() as u16;
        return;
    }
    if let Ok(compressed) = zstd::bulk::compress(text.as_bytes(), ZSTD_LEVEL)
        && compressed.len() <= PAYLOAD_BYTES
    {
        record.flags = FLAG_ZSTD;
        record.payload[..compressed.len()].copy_from_slice(&compressed);
        record.payload_len = compressed.len() as u16;
        return;
    }
    let mut end = PAYLOAD_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    record.payload[..end].copy_from_slice(&text.as_bytes()[..end]);
    record.payload_len = end as u16;
}

/// The text a command carries; empty when the payload cannot be decoded.
pub fn decode(record: &MakiCommand) -> String {
    let len = (record.payload_len as usize).min(PAYLOAD_BYTES);
    if record.flags & FLAG_ZSTD != 0 {
        return zstd::bulk::decompress(&record.payload[..len], DECOMPRESS_CAP)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default();
    }
    String::from_utf8_lossy(&record.payload[..len]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_with(text: &str) -> MakiCommand {
        let mut record = MakiCommand {
            request_id: 0,
            cmd: 3,
            flags: 0,
            payload_len: 0,
            reserved_12: [0; 4],
            timestamp_ns: 0,
            target_session: [0; 32],
            payload: [0; PAYLOAD_BYTES],
            mac: [0; 16],
            reserved_328: [0; 8],
        };
        encode(&mut record, text);
        record
    }

    #[test]
    fn short_text_stays_raw() {
        let record = record_with("continue");
        assert_eq!(record.flags, 0);
        assert_eq!(decode(&record), "continue");
    }

    #[test]
    fn long_compressible_text_goes_zstd() {
        let text = "please continue the refactor. ".repeat(64);
        assert!(text.len() > PAYLOAD_BYTES);
        let record = record_with(&text);
        assert_eq!(record.flags, FLAG_ZSTD);
        assert_eq!(decode(&record), text);
    }

    #[test]
    fn incompressible_text_truncates() {
        let text: String = (0..1024).map(|i| char::from_u32(0x4E00 + i % 500).unwrap()).collect();
        let record = record_with(&text);
        assert_eq!(record.flags & FLAG_ZSTD, 0, "random-ish text should not compress");
        assert_eq!(decode(&record), text[..decode(&record).len()]);
        assert!(!decode(&record).is_empty());
    }

    #[test]
    fn exact_fit_round_trips() {
        let text = "a".repeat(PAYLOAD_BYTES);
        let record = record_with(&text);
        assert_eq!(record.flags, 0);
        assert_eq!(decode(&record), text);
    }
}
