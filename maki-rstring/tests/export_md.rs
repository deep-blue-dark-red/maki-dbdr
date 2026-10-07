//! The `rstring export-md` CLI end to end: file input, filtering flags,
//! error exit on garbage.

use std::path::PathBuf;
use std::process::Command;

const EXE: &str = env!("CARGO_BIN_EXE_rstring");

const JSONL: &str = concat!(
    r#"{"t":"header","v":2,"id":"s1","model":"m","created_at":0}"#,
    "\n",
    r#"{"t":"msg","d":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#,
    "\n",
    r#"{"t":"msg","d":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"read","input":{}}]}}"#,
    "\n",
    r#"{"t":"msg","d":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"out"}]}}"#,
    "\n",
);

fn tmp_jsonl(name: &str, body: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("rstring-export-md-{name}-{}.jsonl", std::process::id()));
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn export_md_renders_header_and_tool_pairing() {
    let path = tmp_jsonl("render", JSONL);
    let out = Command::new(EXE)
        .args(["export-md", "--index", "1", "--path", "/p/x.jsonl"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("---\n---\n# hi\n- /p/x.jsonl\n- Index 1\n- Date 1970-01-01_00-00-00\n- Model m\n"));
    assert!(text.contains("## User 0"));
    assert!(text.contains("**Tool Call:** `read`"));
    assert!(text.contains("**Output:**\n```\nout\n```"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn export_md_no_tools_drops_calls_and_outputs() {
    let path = tmp_jsonl("no_tools", JSONL);
    let out = Command::new(EXE)
        .args(["export-md", "--no-tools"])
        .arg(&path)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("Tool Call"));
    assert!(!text.contains("out\n"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn export_md_garbage_exits_nonzero() {
    let path = tmp_jsonl("garbage", "not a session at all\n");
    let out = Command::new(EXE).arg("export-md").arg(&path).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    std::fs::remove_file(&path).ok();
}
