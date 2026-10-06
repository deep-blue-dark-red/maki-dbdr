//! Throughput benchmarks per tier, on deterministic synthetic corpora.
//! Run: `cargo bench` (writes target/criterion/report/index.html).

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use maki_rstring::{
    cluster, jsonl, mask, session_json, session_md, side::SideTable, surp, thin, tokens, Cfg,
};
use std::time::Instant;

/// Small deterministic PRNG (xorshift64*) so corpora are reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const LEVELS: [&str; 5] = ["INFO", "WARN", "ERROR", "DEBUG", "TRACE"];
const PROVIDERS: [&str; 12] = [
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
];

/// ~2 MB of JSONL log records: 10k lines, 10 fields, mostly-volatile values.
fn synth_jsonl() -> String {
    let mut r = Rng(0xC0FFEE);
    let mut s = String::with_capacity(2 << 20);
    for i in 0..10_000u64 {
        let (lvl, p) = (LEVELS[r.below(5) as usize], PROVIDERS[r.below(12) as usize]);
        s.push_str(&format!(
            "{{\"timestamp\":\"2026-09-{:02}T{:02}:{:02}:{:02}.{:06}Z\",\"seq\":{},\"level\":\"{}\",\"service\":\"api-{}\",\"provider\":\"{}\",\"request_id\":\"{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}\",\"latency_ms\":{},\"status\":{},\"target\":\"svc::handlers::v{}\",\"message\":\"request processed {}\"}}\n",
            1 + r.below(28), r.below(24), r.below(60), r.below(60), r.below(1_000_000),
            i, lvl, r.below(8), p,
            r.below(16), r.below(16), r.below(16), r.below(16), r.below(16),
            r.below(500), 200 + 4 * r.below(50), r.below(3), i
        ));
    }
    s
}

/// Mixed log-shaped lines that defeat the columnar path (not valid JSON).
/// Payloads repeat from a pool; only timestamps (volatile) differ.
fn synth_stream() -> String {
    let mut r = Rng(0xBADC0DE);
    let pool: Vec<String> = (0..300)
        .map(|_| {
            format!(
                "{} worker-{} job {} finished in {}ms (attempt {})",
                LEVELS[r.below(5) as usize],
                r.below(32),
                r.below(500),
                r.below(900),
                1 + r.below(3)
            )
        })
        .collect();
    let mut s = String::with_capacity(1 << 20);
    for _ in 0..12_000u64 {
        let p = &pool[r.below(pool.len() as u64) as usize];
        s.push_str(&format!(
            "[2026-09-29T10:{:02}:{:02}.{:03}Z] {}\n",
            r.below(60),
            r.below(60),
            r.below(1000),
            p
        ));
    }
    s
}

/// Markdown session: 200 turns, each with thinking + a tool output.
fn synth_session_md() -> String {
    let mut r = Rng(0x5E55_1007);
    let mut s = String::from("# Session: synthetic bench\n\n### User\ndo the thing\n\n");
    for i in 0..200u64 {
        s.push_str(&format!("### Assistant\n<details>\n<summary>Thinking</summary>\nI need to check step {}. So the data suggests approach {}. Therefore I will implement it now. This sentence is filler that carries no decision. The error was in module {}.\n</details>\n", i, r.below(4), r.below(16)));
        s.push_str("**Output:**\n```\n");
        for j in 0..20u64 {
            s.push_str(&format!(
                "[2026-09-29T11:{:02}:{:02}.{:03}Z] INFO handler-{} processed record {} status=ok latency={}ms\n",
                r.below(60), r.below(60), r.below(1000), r.below(8), j, r.below(250)
            ));
        }
        s.push_str("```\n\n");
    }
    s
}

/// maki-style JSON session: 300 messages, tool_use/tool_result pairs, thinking.
fn synth_session_json() -> String {
    let mut r = Rng(0xABCD_EF01);
    let mut msgs = Vec::new();
    msgs.push(
        serde_json::json!({"role": "user", "content": [{"type": "text", "text": "run the job"}]}),
    );
    for i in 0..150u64 {
        let id = format!("call_{:024x}", r.next());
        msgs.push(serde_json::json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": format!("Step {}. So the approach is {}. Therefore proceed. Filler sentence with no decision value whatsoever appears here to add mass. The error was ec-{}.", i, r.below(4), r.below(64))},
            {"type": "text", "text": format!("Running step {}.", i)},
            {"type": "tool_use", "id": id, "name": "bash", "input": {"command": format!("process --step {} --mode {}", i, r.below(3)), "timeout": 300}},
        ]}));
        let mut body = String::new();
        for j in 0..12u64 {
            body.push_str(&format!(
                "[2026-09-30T09:{:02}:{:02}.{:03}Z] INFO worker-{} item {} done latency={}ms\n",
                r.below(60),
                r.below(60),
                r.below(1000),
                r.below(8),
                j,
                r.below(180)
            ));
        }
        msgs.push(serde_json::json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": body},
        ]}));
    }
    serde_json::json!({
        "version": 1, "cwd": "/bench", "model": "bench-model",
        "messages": msgs,
        "usage": {"input": 1},
    })
    .to_string()
}

fn proverbs() -> String {
    // Prose for the surp tier: repeated sentence frames with varied fillers.
    let mut r = Rng(0xFACE_B00C);
    let frames = [
        "the system processed the request and returned a response to the caller",
        "we need to check the configuration before the service starts up",
        "this is a sentence that exists to give the bigram model something to learn",
    ];
    let mut s = String::with_capacity(400_000);
    for i in 0..3_000u64 {
        s.push_str(&format!(
            "{} item {} batch {}\n",
            frames[r.below(3) as usize],
            i,
            r.below(50)
        ));
    }
    s
}

fn bench_tiers(c: &mut Criterion) {
    ratios();
    let jsonl_body = synth_jsonl();
    let stream_body = synth_stream();
    let md_body = synth_session_md();
    let sj_body = synth_session_json();
    let prose = proverbs();

    let mut g = c.benchmark_group("tiers");
    g.throughput(Throughput::Bytes(jsonl_body.len() as u64));
    g.bench_function("stream_jsonl_columnar", |b| {
        b.iter(|| jsonl::try_columnar(&jsonl_body, &mut SideTable::default()))
    });

    g.throughput(Throughput::Bytes(stream_body.len() as u64));
    g.bench_function("stream_cluster", |b| {
        b.iter(|| cluster::run(&stream_body, &mut SideTable::default()))
    });

    g.throughput(Throughput::Bytes(md_body.len() as u64));
    g.bench_function("session_md", |b| {
        b.iter(|| {
            let mut t = SideTable::load("/tmp/rstring-bench-md.json");
            let out = session_md::run(&md_body, &mut t, &Cfg::default());
            let _ = std::fs::remove_file("/tmp/rstring-bench-md.json");
            out
        })
    });

    g.throughput(Throughput::Bytes(sj_body.len() as u64));
    g.bench_function("session_json", |b| {
        b.iter(|| {
            let mut t = SideTable::load("/tmp/rstring-bench-sj.json");
            let out = session_json::run(&sj_body, &mut t, &Cfg::default());
            let _ = std::fs::remove_file("/tmp/rstring-bench-sj.json");
            out
        })
    });

    g.throughput(Throughput::Bytes(prose.len() as u64));
    g.bench_function("surp_prose", |b| b.iter(|| surp::run(&prose, 0.85)));

    // Static thin needs a table priced for the bench prose: build one from
    // the same generator so scores are warm, then measure pure lookup cost
    // (the surp bench above pays its counting pass inside the measurement).
    let thin_tbl = thin::Table::from_text(&prose);
    g.bench_function("thin_prose", |b| {
        b.iter(|| thin::run_with(&prose, 0.85, &thin_tbl))
    });
    g.finish();

    let mut m = c.benchmark_group("micro");
    let line = "[2026-09-29T10:22:02.831781Z] WARN worker-3 job 42 finished in 12ms (attempt 1)";
    m.throughput(Throughput::Bytes(line.len() as u64));
    m.bench_function("mask_merge_key_line", |b| b.iter(|| mask::merge_key(line)));
    m.throughput(Throughput::Bytes(64));
    m.bench_function("tokens_count_64b", |b| {
        b.iter(|| tokens::count("typical english prose line of about sixty four bytes in len"))
    });
    m.finish();
}

/// Compression-ratio scoreboard printed once per `cargo bench` run.
fn ratios() {
    let cases: [(&str, &str); 4] = [
        ("jsonl_columnar", &synth_jsonl()),
        ("stream_cluster", &synth_stream()),
        ("session_md", &synth_session_md()),
        ("session_json", &synth_session_json()),
    ];
    let oracle = tiktoken_rs::o200k_base().expect("o200k vocab");
    eprintln!(
        "\n{:<18} {:>10} {:>10} {:>8} {:>8}",
        "case", "tok_in", "tok_out", "saved", "ms"
    );
    for (name, body) in cases {
        let mut t = SideTable::load("/tmp/rstring-ratio.json");
        let t0 = Instant::now();
        let out = match name {
            "jsonl_columnar" => jsonl::try_columnar(body, &mut SideTable::default()).unwrap(),
            "stream_cluster" => cluster::run(body, &mut SideTable::default()),
            "session_md" => session_md::run(body, &mut t, &Cfg::default()),
            _ => session_json::run(body, &mut t, &Cfg::default()),
        };
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        let _ = std::fs::remove_file("/tmp/rstring-ratio.json");
        let (a, b) = (tokens::count(body), tokens::count(&out));
        eprintln!(
            "{:<18} {:>10} {:>10} {:>7.1}% {:>8.1}",
            name,
            a,
            b,
            100.0 * (1.0 - b as f64 / a as f64),
            ms
        );
        // Tokie vs the tiktoken-rs oracle on the same bytes: non-zero deltas
        // are the divergence tests/tokens_parity.rs is there to catch.
        let (ra, rb) = (
            oracle.encode_with_special_tokens(body).len(),
            oracle.encode_with_special_tokens(&out).len(),
        );
        if a != ra || b != rb {
            eprintln!(
                "    tiktoken-rs: tok_in {ra} tok_out {rb} (tokie delta in {} out {})",
                a as i64 - ra as i64,
                b as i64 - rb as i64
            );
        }
    }
}

criterion_group!(benches, bench_tiers);
criterion_main!(benches);
