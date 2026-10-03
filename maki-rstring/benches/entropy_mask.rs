//! Throughput of the SWAR entropy-run scanner (`mask_entropy`) against a
//! scalar byte-loop reference with the identical predicate and stub output,
//! including side-table registration cost on both sides.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use maki_rstring::mask::{ENTROPY_RUN_MIN, mask_entropy};
use maki_rstring::side::SideTable;

const SIZE: usize = 64 * 1024;
const TOKEN_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz+/=";

/// Deterministic noise source, so benches need no rand dep.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }
}

fn token_line(rng: &mut Lcg, len: usize) -> String {
    (0..len)
        .map(|_| TOKEN_ALPHABET[(rng.next() % TOKEN_ALPHABET.len() as u64) as usize] as char)
        .collect()
}

fn pem_block(total: usize) -> String {
    let mut rng = Lcg(7);
    let mut out = String::with_capacity(total);
    while out.len() < total {
        writeln!(out, "{}", token_line(&mut rng, 70)).unwrap();
        writeln!(out, "{}", token_line(&mut rng, 64)).unwrap();
    }
    out
}

fn log_lines(total: usize) -> String {
    let mut out = String::with_capacity(total);
    let mut i = 0u64;
    while out.len() < total {
        writeln!(
            out,
            "2026-09-29T10:{:02}:{:02}.{:06}Z level=WARN provider=anthropic req={:016x} tokens=1287",
            (i / 60) % 60,
            i % 60,
            i % 1_000_000,
            i * 0x9E3779B97F4A7C15
        )
        .unwrap();
        i += 1;
    }
    out
}

fn one_blob(total: usize) -> String {
    token_line(&mut Lcg(1), total)
}

// --- scalar reference: same predicate, same stubs, byte-at-a-time ---

fn flush(
    mut out: Option<String>,
    copied: &mut usize,
    line: &str,
    table: &mut SideTable,
    start: usize,
    len: usize,
) -> String {
    let mut s = out
        .take()
        .unwrap_or_else(|| String::with_capacity(line.len()));
    s.push_str(&line[*copied..start]);
    let hash = table.put(&line[start..start + len]);
    write!(s, "<r:{hash} n={len}>").unwrap();
    *copied = start + len;
    s
}

fn mask_scalar<'a>(line: &'a str, table: &mut SideTable) -> Cow<'a, str> {
    let is_token =
        |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'/' | b'=' | b'_');
    let mut copied = 0;
    let mut out: Option<String> = None;
    let mut start = 0;
    let mut len = 0usize;
    for (i, &c) in line.as_bytes().iter().enumerate() {
        if is_token(c) {
            if len == 0 {
                start = i;
            }
            len += 1;
        } else {
            if len >= ENTROPY_RUN_MIN {
                out = Some(flush(out, &mut copied, line, table, start, len));
            }
            len = 0;
        }
    }
    if len >= ENTROPY_RUN_MIN {
        out = Some(flush(out, &mut copied, line, table, start, len));
    }
    match out {
        Some(mut s) => {
            s.push_str(&line[copied..]);
            Cow::Owned(s)
        }
        None => Cow::Borrowed(line),
    }
}

fn mask_all(text: &str, table: &mut SideTable, swar: bool) -> usize {
    text.lines()
        .map(|l| {
            if swar {
                mask_entropy(l, table).len()
            } else {
                mask_scalar(l, table).len()
            }
        })
        .sum()
}

fn bench_entropy(c: &mut Criterion) {
    let inputs: [(&str, String); 3] = [
        ("log_lines", log_lines(SIZE)),
        ("pem_block", pem_block(SIZE)),
        ("one_blob", one_blob(SIZE)),
    ];

    let mut group = c.benchmark_group("mask_entropy");
    for (name, text) in &inputs {
        group.bench_with_input(BenchmarkId::new("swar", name), text, |b, t| {
            let mut table = SideTable::default();
            b.iter(|| mask_all(t, &mut table, true))
        });
        group.bench_with_input(BenchmarkId::new("scalar", name), text, |b, t| {
            let mut table = SideTable::default();
            b.iter(|| mask_all(t, &mut table, false))
        });
    }
    group.finish();

    // the real per-tool-call floor: a 411-byte throwaway SSH key shape
    let key = pem_block(411);
    let mut group = c.benchmark_group("key_411b");
    group.bench_function("swar", |b| {
        let mut table = SideTable::default();
        b.iter(|| mask_entropy(&key, &mut table).len())
    });
    group.bench_function("scalar", |b| {
        let mut table = SideTable::default();
        b.iter(|| mask_scalar(&key, &mut table).len())
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_millis(700));
    targets = bench_entropy
}
criterion_main!(benches);
