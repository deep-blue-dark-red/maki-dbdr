//! Stream-tier compression for tool output, vendored from
//! [rstring](https://github.com/tontinton/rstring) (`mask`, `cluster`,
//! `jsonl`). Deterministic, synchronous, side-table free: two lines collapse
//! into one `[xN]` entry iff they differ only in volatile values (timestamps,
//! UUIDs, epoch runs, temp paths); semver, error codes, repo paths and short
//! numbers are load-bearing and never masked.

pub mod cluster;
pub mod jsonl;
pub mod mask;

/// Inputs at or below this stay verbatim: merging a handful of lines costs
/// more context in markers than it saves.
pub const MIN_BYTES: usize = 2048;

/// Compress bash-style tool output: JSONL columnar when the input is a run of
/// uniform JSON objects, else volatile-merge clustering over lines.
pub fn stream(text: &str) -> String {
    let text = mask::strip_ansi(text);
    if let Some(col) = jsonl::try_columnar(&text) {
        return col;
    }
    cluster::run(&text)
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
        while repetitive.len() <= MIN_BYTES {
            repetitive.push_str(TS_LINE);
            repetitive.push('\n');
            repetitive.push_str(TS_LINE_LATER);
            repetitive.push('\n');
        }
        let compressed = compress_if_useful(repetitive);
        assert!(compressed.contains(" [x"), "{compressed}");
        assert!(compressed.len() < MIN_BYTES / 10, "{compressed}");
    }
}
