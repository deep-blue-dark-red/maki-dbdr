//! Stream-tier compression for tool output, vendored from
//! [rstring](https://github.com/tontinton/rstring) (`mask`, `cluster`,
//! `jsonl`, `side`). Deterministic, synchronous: two lines collapse into one
//! `[xN]` entry iff they differ only in volatile values (timestamps, UUIDs,
//! epoch runs, temp paths); semver, error codes, repo paths and short numbers
//! are load-bearing and never masked; code-looking lines (indented, or ending
//! in `;`, `{`, `}`) pass through verbatim: never merged, never entropy-
//! masked. Long token-alphabet runs (base64 bodies, digests, random bytes)
//! elide inline to `<r:hash n>` stubs. The side table is per call: stubs
//! dedupe within one output but are not recoverable across calls — a
//! persistent table can be threaded later without changing this API.

pub mod cluster;
pub mod jsonl;
pub mod mask;
pub mod side;

/// Inputs at or below this stay verbatim: merging a handful of lines costs
/// more context in markers than it saves.
pub const MIN_BYTES: usize = 399;

/// Compress bash-style tool output: JSONL columnar when the input is a run of
/// uniform JSON objects, else volatile-merge clustering over lines.
pub fn stream(text: &str) -> String {
    let text = mask::strip_ansi(text);
    let mut table = side::SideTable::default();
    if let Some(col) = jsonl::try_columnar(&text, &mut table) {
        return col;
    }
    cluster::run(&text, &mut table)
}

/// `stream`, kept only when it actually shrinks the input.
pub fn compress_if_useful(text: String) -> String {
    let compressed = stream(&text);
    if compressed.len() < text.len() {
        compressed
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    const TS_LINE: &str = "2026-09-29T10:22:02.831781Z level=WARN provider=anthropic";
    const TS_LINE_LATER: &str = "2026-09-29T18:44:59.000001Z level=WARN provider=anthropic";

    #[test_case(TS_LINE, TS_LINE_LATER, " [x2]")]
    #[test_case(
        "req 67e55044-10b1-426f-9247-bb680e138921 done",
        "req 00000000-0000-0000-0000-000000000000 done",
        " [x2]"
    )]
    fn volatile_lines_merge(a: &str, b: &str, marker: &str) {
        let out = stream(&format!("{a}\n{b}\n"));
        assert_eq!(out.matches(marker).count(), 1, "{out}");
        assert!(
            out.contains("provider=anthropic") || out.contains("done"),
            "{out}"
        );
    }

    #[test_case("bumped dep 1.2.0", "bumped dep 1.2.1")]
    #[test_case("port 8080", "port 8081")]
    #[test_case("error CS0246 in src/PinHttp", "error CS5001 in src/PinHttp")]
    fn load_bearing_lines_never_merge(a: &str, b: &str) {
        let out = stream(&format!("{a}\n{b}\n"));
        assert!(out.contains(a) && out.contains(b), "{out}");
        assert!(!out.contains("[x"), "{out}");
    }

    #[test]
    fn columnar_jsonl_states_keys_once() {
        let input = (0..6)
            .map(|i| {
                format!(
                    "{{\"ts\": \"2026-09-29T10:22:0{i}.000Z\", \"level\": \"INFO\", \"msg\": \"tick\"}}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let out = stream(&input);
        assert!(out.contains("<rec level,msg,ts>"), "{out}");
        assert_eq!(out.matches("level").count(), 1, "{out}");
    }

    #[test]
    fn ansi_escapes_stripped_before_clustering() {
        let out = stream(&format!("\u{1b}[31m{TS_LINE}\u{1b}[0m\n{TS_LINE_LATER}\n"));
        assert!(out.contains(" [x2]"), "{out}");
        assert!(!out.contains('\u{1b}'), "{out}");
    }

    #[test]
    fn compress_if_useful_keeps_incompressible_byte_identical() {
        let distinct = (0..MIN_BYTES / 8)
            .map(|i| format!("distinct line {i}\n"))
            .collect::<String>();
        assert_eq!(compress_if_useful(distinct.clone()), distinct);
    }

    #[test]
    fn compress_if_useful_shrinks_repetitive_input() {
        let mut repetitive = String::new();
        while repetitive.len() <= MIN_BYTES * 4 {
            repetitive.push_str(TS_LINE);
            repetitive.push('\n');
            repetitive.push_str(TS_LINE_LATER);
            repetitive.push('\n');
        }
        let compressed = compress_if_useful(repetitive.clone());
        assert!(compressed.contains(" [x"), "{compressed}");
        assert!(compressed.len() < repetitive.len() / 10, "{compressed}");
    }

    const BLOB_CHUNK: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ+/=";

    #[test]
    fn random_blob_line_elides_to_stub() {
        let blob = BLOB_CHUNK.repeat(9);
        let out = stream(&format!("{blob}\n"));
        assert!(out.starts_with("<r:"), "{out}");
        assert!(out.contains(&format!(" n={}>", blob.len())), "{out}");
    }

    #[test]
    fn duplicate_blobs_collapse_to_one_stub() {
        let blob = BLOB_CHUNK.repeat(8);
        let out = stream(&format!("{blob}\n{blob}\n"));
        assert_eq!(out.matches("<r:").count(), 1, "{out}");
        assert!(out.contains(" [x2]"), "{out}");
    }

    #[test]
    fn prose_and_git_sha_stay_verbatim() {
        let prose = "word ".repeat(120);
        let sha = "a".repeat(40);
        let out = stream(&format!("{prose}\ncommit {sha}\n"));
        assert!(out.contains(&prose) && out.contains(&sha), "{out}");
        assert!(!out.contains("<r:"), "{out}");
    }

    #[test]
    fn entropy_segment_elides_inline_context_kept() {
        let sha256_hex = "0123456789abcdef".repeat(4);
        let out = stream(&format!("sha256 {sha256_hex} file.tgz\n"));
        let out = out.trim_end();
        assert!(out.starts_with("sha256 <r:"), "{out}");
        assert!(out.ends_with(" file.tgz"), "{out}");
    }

    #[test]
    fn pem_body_elides() {
        let b64 = "TWFuIGlzIGRpc3Rpbmd1aXNoZWQsIG5vdCBvbmx5IGJ5IGhpcyByZWFzb24sIGJ1dCBieSB0aGlz";
        let out = stream(&format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{b64}\n-----END OPENSSH PRIVATE KEY-----\n"
        ));
        assert!(out.contains("<r:"), "{out}");
        assert!(!out.contains("TWFu"), "{out}");
    }
}
