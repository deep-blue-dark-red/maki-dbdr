//! Static thin tier: guard rails, token-exact budget, table roundtrip.
//! Requires the o200k tokenizer at the usual cache path (same as
//! tests/tokens_parity.rs).

use maki_rstring::thin::{self, Table};
use maki_rstring::tokens;

fn corpus() -> String {
    // Make function words overwhelmingly frequent; leave identifiers,
    // paths and numbers unseen so they must survive.
    "the user asked to run the tool and the tool ran the output of the run \
     we then read the file and the file had the data the end"
        .repeat(200)
}

#[test]
fn table_roundtrip() {
    let t = Table::from_text(&corpus());
    assert!(t.total > 1000);
    let dir = std::env::temp_dir().join("rstring-thin-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.bin");
    t.save(&p).unwrap();
    let l = Table::load(&p).unwrap();
    assert_eq!(l.total, t.total);
    assert_eq!(l.distinct, t.distinct);
}

#[test]
fn load_rejects_garbage() {
    let dir = std::env::temp_dir().join("rstring-thin-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("bad.bin");
    std::fs::write(&p, b"not a table at all").unwrap();
    assert!(Table::load(&p).is_err());
}

#[test]
fn keeps_load_bearing_spans() {
    let tbl = Table::from_text(&corpus());
    let line = "the tool wrote /etc/hosts and port 5432 then aborted with error code X7Q";
    let out = thin::run_with(line, 0.5, &tbl);
    for keep in ["/etc/hosts", "5432", "X7Q"] {
        assert!(out.contains(keep), "lost {keep} in {out:?}");
    }
    // frequent non-initial corpus words should be dropped; the initial
    // "the" is line-initial and therefore protected by design
    assert!(
        !out.split(' ').any(|w| w == "tool" || w == "and"),
        "kept dropping-class word in {out:?}"
    );
    assert!(out.starts_with("the"));
}

#[test]
fn budget_is_token_exact() {
    let tbl = Table::from_text(&corpus());
    let line = "the tool ran the checks and the checks all passed the run finished quietly";
    let rate = 0.6;
    let before = tokens::count(line);
    let out = thin::run_with(line, rate, &tbl);
    let after = tokens::count(out.trim_end());
    assert!(
        after <= ((before as f64) * rate).ceil() as usize,
        "budget violated: {after} > ceil({before} * {rate})"
    );
    assert!(after < before, "nothing was dropped");
}

#[test]
fn non_ascii_and_boundaries_survive() {
    let tbl = Table::from_text(&corpus());
    let line = "路径 /tmp/引用了 the 文件 and more words here";
    let out = thin::run_with(line, 0.5, &tbl);
    assert!(out.contains("路径"), "non-ASCII span dropped: {out:?}");
    assert!(
        out.starts_with("路径"),
        "line-initial span dropped: {out:?}"
    );
}

#[test]
fn structured_lines_pass_through() {
    let tbl = Table::from_text(&corpus());
    let json = r#" {"key": "the the the the the the"} "#;
    assert_eq!(thin::run_with(json, 0.1, &tbl), format!("{json}\n"));
}

#[test]
fn deterministic() {
    let tbl = Table::from_text(&corpus());
    let text = "the tool ran the checks and the checks all passed the run finished\nmore prose the end of the line here";
    assert_eq!(
        thin::run_with(text, 0.6, &tbl),
        thin::run_with(text, 0.6, &tbl)
    );
}
