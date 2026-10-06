use maki_rstring::{repeat, side::SideTable, stream, Cfg};

fn grepish() -> String {
    let p = "/Users/mcp/git/blitgen-zig/src/lower.zig";
    let mut s = String::new();
    for i in 0..39 {
        s.push_str(&format!("{p}-{i}-        return foo_{i}();\n"));
    }
    s
}

#[test]
fn repeats_factor_losslessly() {
    let text = grepish();
    let out = repeat::run(&text);
    assert!(
        out.len() < text.len(),
        "must shrink: {} vs {}",
        out.len(),
        text.len()
    );
    assert!(
        out.starts_with("<0> /Users/mcp/git/blitgen-zig/src/lower.zig"),
        "{out}"
    );
    assert_eq!(decode(&out), text, "legend substitution must be byte-exact");
}

#[test]
fn repeat_pass_is_a_fixed_point() {
    let once = repeat::run(&grepish());
    assert_eq!(repeat::run(&once), once);
}

#[test]
fn unique_lines_pass_through() {
    let text = "alpha beta gamma\ndelta epsilon zeta\neta theta iota\n";
    assert_eq!(repeat::run(text), text);
}

#[test]
fn stream_factors_repeats() {
    let text = grepish();
    let out = stream(&text, &mut SideTable::default(), &Cfg::default());
    assert!(
        out.contains("<0> /Users/mcp/git/blitgen-zig/src/lower.zig"),
        "{out}"
    );
    assert_eq!(decode(&out), text);
}

#[test]
fn repeat_layer_is_byte_exact_on_synthetic_corpus() {
    let mut any_factored = false;
    for (name, src) in synthetic_corpus() {
        let mut table = SideTable::default();
        let clustered = maki_rstring::cluster::run(&src, &mut table);
        let factored = repeat::run(&clustered);
        assert!(factored.len() <= clustered.len(), "must never grow: {name}");
        assert_eq!(
            decode(&factored),
            clustered,
            "repeat layer must invert on {name}"
        );
        any_factored |= factored.len() < clustered.len();
    }
    assert!(
        any_factored,
        "synthetic corpus must exercise the factoring path"
    );
}

/// Synthetic stand-ins for the untracked bench corpus: log/toml/yaml shapes
/// with repeated prefixes so the ref-factoring pass actually fires.
fn synthetic_corpus() -> Vec<(&'static str, String)> {
    let log = (0..80)
        .map(|i| {
            format!(
                "2024-01-01T00:00:{i:02}Z INFO  serve::routes  handled path=/api/v1/items status=200 dur_ms={i}\n"
            )
        })
        .collect();
    let toml = (0..60)
        .map(|i| {
            format!(
                "[[package]]\nname = \"crate-{i}\"\nversion = \"1.{i}.0\"\nedition = \"2021\"\n\n"
            )
        })
        .collect();
    let yaml = (0..60)
        .map(|i| format!("- apiVersion: apps/v1\n  kind: Deployment\n  metadata:\n    name: app-{i}\n    namespace: prod\n"))
        .collect();
    vec![("log", log), ("toml", toml), ("yaml", yaml)]
}

#[test]
fn repeat_layer_is_byte_exact_on_real_corpus_when_present() {
    for f in [
        "bench/bench_assets/log.log",
        "bench/bench_assets/toml-records.toml",
        "bench/bench_assets/yaml-records.yaml",
    ] {
        let path = std::path::Path::new(f);
        if !path.exists() {
            continue;
        }
        let src = std::fs::read_to_string(path).unwrap();
        let mut table = SideTable::default();
        let clustered = maki_rstring::cluster::run(&src, &mut table);
        let factored = repeat::run(&clustered);
        assert!(factored.len() <= clustered.len(), "must never grow: {f}");
        assert_eq!(
            decode(&factored),
            clustered,
            "repeat layer must invert on {f}"
        );
    }
}

/// Reproducible xorshift so the property test needs no new dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x % n as u64) as usize
    }
}

const ALPHABET: &[&str] = &[
    "a",
    "b",
    "z",
    "/",
    "-",
    "=",
    " ",
    "  ",
    "\n",
    "path/to/file",
    "é",
    "你",
    "🙂",
];

#[test]
fn repeat_layer_inverts_random_input() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..2000 {
        let mut s = String::new();
        for _ in 0..rng.below(400) {
            s.push_str(ALPHABET[rng.below(ALPHABET.len())]);
        }
        let out = repeat::run(&s);
        assert!(out.len() <= s.len(), "must never grow: {s:?}");
        if legend_shaped(&s) {
            // A text already carrying a legend is a documented fixed point.
            assert_eq!(out, s);
        } else {
            assert_eq!(decode(&out), s, "must invert: {s:?}");
        }
    }
}

fn legend_shaped(text: &str) -> bool {
    let b = text.split('\n').next().unwrap_or("").as_bytes();
    if b.first() != Some(&b'<') {
        return false;
    }
    let mut i = 1;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    i > 1 && b.get(i) == Some(&b'>')
}

/// Resolve `<n>` refs using the lead legend (test-only decoder).
fn decode(out: &str) -> String {
    let mut rest = out;
    let mut legend: Vec<&str> = Vec::new();
    loop {
        let line_end = rest.find('\n').map_or(rest.len(), |p| p + 1);
        let line = rest[..line_end].trim_end_matches('\n');
        match legend_value(line) {
            Some(v) => {
                legend.push(v);
                rest = &rest[line_end..];
            }
            None => break,
        }
    }
    let mut res = String::with_capacity(rest.len());
    let mut i = 0;
    while i < rest.len() {
        let ch = rest[i..].chars().next().unwrap();
        if ch == '<' {
            let after = &rest[i + 1..];
            let nd = after.bytes().take_while(|b| b.is_ascii_digit()).count();
            if nd > 0 && after.as_bytes().get(nd) == Some(&b'>') {
                if let Ok(n) = after[..nd].parse::<usize>() {
                    if let Some(v) = legend.get(n) {
                        res.push_str(v);
                        i += 1 + nd + 1;
                        continue;
                    }
                }
            }
        }
        res.push(ch);
        i += ch.len_utf8();
    }
    res
}

fn legend_value(line: &str) -> Option<&str> {
    let b = line.as_bytes();
    if b.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    (i > 1 && b.get(i) == Some(&b'>') && b.get(i + 1) == Some(&b' ')).then(|| &line[i + 2..])
}
