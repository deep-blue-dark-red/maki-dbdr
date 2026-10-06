//! Invariant tests for every tier. The two that must never break:
//! 1. **Merge-iff-volatile**: lines differing in load-bearing text never merge.
//! 2. **Recoverability**: every elided body is in the side table, byte-exact.

use maki_rstring::{
    cluster, jsonl, mask, session_json, session_md, side::SideTable, surp, tokens, Cfg,
};
use serde_json::json;

#[test]
fn merge_key_masks_timestamps_not_semver() {
    let a = mask::merge_key("2026-09-29T10:22:02.831781Z level=WARN provider=anthropic");
    let b = mask::merge_key("2026-09-29T18:44:59.000001Z level=WARN provider=anthropic");
    assert_eq!(
        a, b,
        "lines differing only in ISO timestamps must share a key"
    );
    assert!(a.contains("<TS>"));

    // semver is load-bearing: never masked, never merges
    let s1 = mask::merge_key("bumped BlitGen.Structs 1.2.0");
    let s2 = mask::merge_key("bumped BlitGen.Structs 1.2.1");
    assert_ne!(s1, s2);
    assert!(s1.contains("1.2.0"), "semver must survive masking verbatim");

    // error codes and repo paths survive
    assert!(mask::merge_key("error CS0246 in src/PinHttp").contains("CS0246"));
    assert!(mask::merge_key("error CS0246 in src/PinHttp").contains("src/PinHttp"));
}

#[test]
fn merge_key_uuid_and_epoch_masked() {
    let a = mask::merge_key("req 67e55044-10b1-426f-9247-bb680e138921 done");
    let b = mask::merge_key("req 00000000-0000-0000-0000-000000000000 done");
    assert_eq!(a, b, "UUIDs are volatile");
    assert!(a.contains("<ID>"));

    let e1 = mask::merge_key("at 1790770189 ms");
    let e2 = mask::merge_key("at 9999999999 ms");
    assert_eq!(e1, e2, "epoch runs are volatile");

    // short numbers are load-bearing (counts, ports, versions)
    assert_ne!(
        mask::merge_key("port 8080"),
        mask::merge_key("port 8081"),
        "short numbers must not be masked"
    );
}

#[test]
fn merge_key_temp_paths_masked_repo_paths_kept() {
    let a = mask::merge_key("wrote /tmp/maki/xyz/out.txt ok");
    let b = mask::merge_key("wrote /tmp/maki/other/out.txt ok");
    assert_eq!(a, b, "temp paths are volatile");
    assert_ne!(
        mask::merge_key("edit src/PinHttp/a.cs"),
        mask::merge_key("edit src/PinHttp/b.cs"),
        "repo paths are load-bearing"
    );
}

#[test]
fn cluster_never_merges_load_bearing_diffs() {
    // 12 distinct provider names — the exact case where ltk@0.7 destroyed data
    let mut lines = String::new();
    for burst in 0..2 {
        for p in [
            "anthropic",
            "openai",
            "google",
            "copilot",
            "ollama",
            "mistral",
            "requesty",
            "synthetic",
            "regolo",
            "tensorx",
            "xai",
            "aperture",
        ] {
            lines.push_str(&format!(
                "{{\"timestamp\":\"2026-09-29T1{}:34:5{}.15065{}Z\",\"level\":\"WARN\",\"provider\":\"{}\"}}\n",
                burst, burst, burst, p
            ));
        }
    }
    let out = cluster::run(&lines, &mut SideTable::default());
    for p in [
        "anthropic",
        "openai",
        "google",
        "copilot",
        "ollama",
        "mistral",
        "requesty",
        "synthetic",
        "regolo",
        "tensorx",
        "xai",
        "aperture",
    ] {
        assert!(out.contains(p), "provider {} must survive clustering", p);
    }
    assert_eq!(
        out.lines().count(),
        12,
        "24 lines in 12 volatile-equivalent pairs -> 12"
    );
    assert!(out.contains("[x2]"), "each pair folds with a count");
}

#[test]
fn cluster_is_idempotent_and_count_preserving() {
    let text = "a\nb\na\na\nc\nb\n";
    let mut table = SideTable::default();
    let once = cluster::run(text, &mut table);
    let twice = cluster::run(&once, &mut table);
    assert_eq!(once, twice, "clustering must be a fixed point");
    // occurrence mass is conserved: 1+2+3 = 6
    let total: usize = once
        .lines()
        .filter_map(|l| {
            l.rsplit_once(" [x")
                .and_then(|(_, n)| n.trim_end_matches(']').parse().ok())
                .or(Some(1))
        })
        .sum();
    assert_eq!(total, text.lines().count());
}

#[test]
fn jsonl_columnar_states_keys_once() {
    let mut input = String::new();
    for i in 0..20 {
        input.push_str(&format!(
            "{{\"timestamp\":\"2026-09-29T10:22:0{}.8317{}Z\",\"level\":\"WARN\",\"provider\":\"p{}\",\"target\":\"t::x\"}}\n",
            i % 10, i % 10, i % 4
        ));
    }
    let out =
        jsonl::try_columnar(&input, &mut SideTable::default()).expect("uniform JSONL is columnar");
    assert!(
        out.starts_with("<rec level,provider,target,timestamp>"),
        "header states keys once: {}",
        out.lines().next().unwrap()
    );
    assert_eq!(out.lines().count(), 1 + 4, "20 rows in 4 provider groups");
    assert!(!out.contains("\"timestamp\""), "JSON key syntax is gone");
}

#[test]
fn jsonl_columnar_rejects_prose() {
    assert!(jsonl::try_columnar("hello\nworld\nfoo\nbar\n", &mut SideTable::default()).is_none());
}

#[test]
fn surp_never_touches_structured_lines() {
    let json_line =
        "{\"message\":\"failed to create provider, skipping\",\"provider\":\"anthropic\"}";
    let out = surp::run(&format!("{}\nsome prose line with words\n", json_line), 0.5);
    assert!(
        out.contains(json_line),
        "parse-valid JSON lines pass through verbatim"
    );
}

#[test]
fn surp_keeps_numbers_and_anchors() {
    let out = surp::run("bumped the version from 1.2.0 to 1.2.1 today", 0.3);
    assert!(
        out.contains("1.2.0") && out.contains("1.2.1"),
        "digit-bearing words are anchors"
    );
}

#[test]
fn side_table_round_trips_byte_exact() {
    let mut t = SideTable::load("/tmp/rstring-test-side.json");
    let _ = std::fs::remove_file("/tmp/rstring-test-side.json");
    let body = "line1\nline2 \"quoted\" \u{1f600}\n";
    let h1 = t.put(body);
    let h2 = t.put(body);
    assert_eq!(h1, h2, "content-addressed: same bytes -> same hash");
    assert_eq!(t.get(&h1).unwrap(), body);
    t.save("/tmp/rstring-test-side.json");
    let re = SideTable::load("/tmp/rstring-test-side.json");
    assert_eq!(re.get(&h1).unwrap(), body, "persists across save/load");
    let _ = std::fs::remove_file("/tmp/rstring-test-side.json");
}

fn sample_md_session() -> String {
    let mut s = String::from("# Session: test\n\n### User\nfix the bug\n\n### Assistant\n");
    s.push_str("<details>\n<summary>Thinking</summary>\nI need to find the error. So the root cause is the stale cache. Therefore I will clear it.\nAn unrelated filler sentence stays too.\n</details>\n");
    s.push_str("**Output:**\n```\nerror CS0246: type not found in Program.cs\n```\n");
    s.push_str("Some narration that must stay verbatim, exactly as written.\n\n");
    for i in 0..5 {
        s.push_str(&format!(
            "**Output:**\n```\nstale output number {} with padding padding padding\n```\n",
            i
        ));
    }
    s
}

#[test]
fn session_md_keeps_errors_narration_and_refs() {
    let mut table = SideTable::load("/tmp/rstring-test-md.json");
    let _ = std::fs::remove_file("/tmp/rstring-test-md.json");
    let out = session_md::run(&sample_md_session(), &mut table, &Cfg::default());

    assert!(out.contains("error CS0246"), "error outputs are kept");
    assert!(
        out.contains("Some narration that must stay verbatim, exactly as written."),
        "narration is never word-dropped"
    );
    assert!(
        out.contains("root cause"),
        "decision-cue sentences survive thinking extraction"
    );
    assert!(
        out.contains("<think r="),
        "thinking becomes a resolvable stub"
    );
    assert!(out.contains("<r:"), "stale outputs become refs");

    // every ref resolves byte-exact
    for h in out.split_whitespace().filter_map(|w| w.strip_prefix("<r:")) {
        let hash = h.split(&[' ', '>'][..]).next().unwrap();
        let body = table.get(hash).expect("ref resolves");
        assert!(
            body.contains("stale output number"),
            "side-table holds the original body"
        );
    }
    let _ = std::fs::remove_file("/tmp/rstring-test-md.json");
}

fn sample_json_session() -> String {
    let mut msgs = Vec::new();
    msgs.push(json!({"role": "user", "content": [{"type": "text", "text": "find the bug"}]}));
    msgs.push(json!({"role": "assistant", "content": [
        {"type": "thinking", "thinking": "The error is a stale cache. So I will clear it. Filler sentence here."},
        {"type": "text", "text": "Let me look."},
        {"type": "tool_use", "id": "call_1", "name": "bash", "input": {"command": "grep -r CS0246 src/"}},
    ]}));
    msgs.push(json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "call_1", "content": "error CS0246 in src/Program.cs at 1.2.0"},
    ]}));
    for i in 0..5 {
        let id = format!("call_{}", i + 2);
        msgs.push(json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": "bash", "input": {"command": format!("echo {}", i)}},
        ]}));
        msgs.push(json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": format!("stale bulk output {} padding padding padding", i)},
        ]}));
    }
    let mut tool_outputs = serde_json::Map::new();
    for i in 0..5 {
        tool_outputs.insert(
            format!("call_{}", i + 2),
            json!({"Plain": {"text": format!("stale bulk output {} padding padding padding", i)}}),
        );
    }
    json!({
        "version": 1, "cwd": "/x/y", "model": "m",
        "messages": msgs,
        "tool_outputs": tool_outputs,
        "usage": {"input": 5},
    })
    .to_string()
}

#[test]
fn session_json_transcript_drops_dup_and_refs_resolve() {
    let mut table = SideTable::load("/tmp/rstring-test-sj.json");
    let _ = std::fs::remove_file("/tmp/rstring-test-sj.json");
    let src = sample_json_session();
    let out = session_json::run(&src, &mut table, &Cfg::default());

    assert!(
        out.contains("<user>") && out.contains("<assistant>"),
        "role markers render"
    );
    assert!(out.contains("error CS0246"), "error results stay inline");
    assert!(out.contains("1.2.0"), "semver survives");
    assert!(
        out.contains("> bash command=\"grep -r CS0246 src/\""),
        "tool commands stay verbatim: {}",
        out.lines()
            .filter(|l| l.starts_with("> "))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        out.contains("mirrors inline"),
        "tool_outputs map is dropped with a note"
    );
    assert!(out.contains("<think r="), "thinking stubbed");
    assert!(out.contains("<r:"), "stale results become refs");
    assert!(
        !out.contains("\"type\":\"tool_result\""),
        "JSON block syntax is gone"
    );

    for h in out.split_whitespace().filter_map(|w| w.strip_prefix("<r:")) {
        let hash = h.split(&[' ', '>'][..]).next().unwrap();
        assert!(table.get(hash).is_some(), "every ref resolves");
    }
    let _ = std::fs::remove_file("/tmp/rstring-test-sj.json");
}

#[test]
fn session_json_side_table_recovers_evicted_bodies() {
    let mut table = SideTable::load("/tmp/rstring-test-sj2.json");
    let _ = std::fs::remove_file("/tmp/rstring-test-sj2.json");
    let out = session_json::run(&sample_json_session(), &mut table, &Cfg::default());
    // evicted bodies appear ONLY inside <r:... v="..."> preview lines, never inline
    let leaked: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("stale bulk output 0") && !l.trim_start().starts_with("<r:"))
        .collect();
    assert!(
        leaked.is_empty(),
        "evicted bodies must not render inline: {:?}",
        leaked
    );
    let any = table
        .map_ref()
        .values()
        .any(|v| v.contains("stale bulk output 0"));
    assert!(any, "…and recoverable from the side table");
    let _ = std::fs::remove_file("/tmp/rstring-test-sj2.json");
}

#[test]
fn compress_auto_detects_all_tiers() {
    assert_eq!(maki_rstring::detect(&sample_json_session()), "session-json");
    assert_eq!(maki_rstring::detect(&sample_md_session()), "session-md");
    assert_eq!(maki_rstring::detect("plain\nlines\nonly\n"), "stream");
}

#[test]
fn tokens_count_is_sane() {
    assert!(tokens::count("hello world") > 0);
    assert!(tokens::count("hello world") < 5);
}

#[test]
fn entropy_runs_elided_and_recoverable() {
    let mut table = SideTable::default();
    let b64 = "TWFuIGlzIGRpc3Rpbmd1aXNoZWQsIG5vdCBvbmx5IGJ5IGhpcyByZWFzb24sIGJ1dCBieSB0aGlz";
    let out = cluster::run(
        &format!("-----BEGIN KEY-----\n{b64}\n-----END KEY-----\n"),
        &mut table,
    );
    assert!(out.contains("<r:"), "run elided to a stub: {out}");
    assert!(!out.contains("TWFu"), "raw entropy must not survive: {out}");
    let hash = out
        .split("<r:")
        .nth(1)
        .unwrap()
        .split([' ', '>'])
        .next()
        .unwrap();
    assert_eq!(
        table.get(hash).map(String::as_str),
        Some(b64),
        "expand must recover the run byte-exact"
    );
}

#[test]
fn entropy_mask_preserves_inline_context() {
    let mut table = SideTable::default();
    let sha256_hex = "0123456789abcdef".repeat(4);
    let out = cluster::run(&format!("sha256 {sha256_hex} file.tgz\n"), &mut table);
    let out = out.trim_end();
    assert!(out.starts_with("sha256 <r:"), "{out}");
    assert!(out.ends_with(" file.tgz"), "{out}");
}

#[test]
fn uuid_and_git_sha_stay_verbatim() {
    let mut table = SideTable::default();
    let line = "commit 67e5504410b1426f9247bb680e5c8b42 done";
    let out = cluster::run(&format!("{line}\n"), &mut table);
    assert!(out.contains("67e5504410b1426f9247bb680e5c8b42"), "{out}");
    assert!(!out.contains("<r:"), "{out}");
}

#[test]
fn identical_runs_dedupe_through_stubs() {
    let mut table = SideTable::default();
    let run = "a".repeat(64);
    let out = cluster::run(&format!("{run}\n{run}\n"), &mut table);
    assert_eq!(
        out.matches("<r:").count(),
        1,
        "identical runs share a stub: {out}"
    );
    assert!(out.contains(" [x2]"), "{out}");
    assert_eq!(table.len(), 1);
}
