//! CLI: stdin → compressed stdout, plus `expand` and `bench`.

use maki_rstring::{self as rs, side::SideTable, tokens, Cfg};
use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-V") | Some("--version") | Some("version") => {
            println!("rstring {}", env!("CARGO_PKG_VERSION"));
        }
        Some("expand") => {
            let hash = args
                .get(1)
                .expect("usage: rstring expand <hash16> [side-path]");
            let side_path = args.get(2).cloned().unwrap_or_else(default_side_path);
            let table = SideTable::load(&side_path);
            match table.get(hash) {
                Some(text) => print!("{}", text),
                None => {
                    eprintln!("no side-table entry for {}", hash);
                    std::process::exit(1);
                }
            }
        }
        Some("compress") => compress(&args[1..]),
        Some("export-md") => export_md_cmd(&args[1..]),
        Some("bench") => bench(&args[1..]),
        Some("tokens") => tokens_cmd(&args[1..]),
        Some("thin-table") => thin_table(&args[1..]),
        _ => {
            eprintln!(
                "usage: rstring compress [--mode auto|stream|session|session-md|session-json] [--side P] [--keep-thinking] [--no-surp] [--thin RATE] [--evict-last N] < in > out"
            );
            eprintln!(
                "       rstring export-md [--no-thinking] [--no-tools] [--name S] [--path P] [--index N] [--date SECS] [--model S] [file] < session > markdown"
            );
            eprintln!("       rstring expand <hash16> [side-path]");
            eprintln!("       rstring bench <files...>   # end-to-end scoreboard");
            eprintln!("       rstring tokens <files...>  # o200k token counts");
            eprintln!(
                "       rstring thin-table [--out P] <files...>  # static thin scores from a corpus"
            );
            eprintln!("       rstring -V | --version");
            std::process::exit(2);
        }
    }
}

fn default_side_path() -> String {
    ".rstring-side.json".into()
}

/// /export as a filter: transcript jsonl or export JSON in, markdown out.
fn export_md_cmd(args: &[String]) {
    let mut o = rs::export_md::Opts::default();
    let mut file: Option<&String> = None;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--name" => {
                o.name = Some(flag_value(args, i, flag));
                i += 2;
            }
            "--path" => {
                o.path = Some(flag_value(args, i, flag));
                i += 2;
            }
            "--index" => {
                o.index = Some(args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("--index wants an integer");
                    std::process::exit(2);
                }));
                i += 2;
            }
            "--date" => {
                o.date = Some(args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("--date wants unix seconds");
                    std::process::exit(2);
                }));
                i += 2;
            }
            "--model" => {
                o.model = Some(flag_value(args, i, flag));
                i += 2;
            }
            "--no-thinking" => {
                o.thinking = false;
                i += 1;
            }
            "--no-tools" => {
                o.tools = false;
                i += 1;
            }
            other if other.starts_with("--") => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
            _ => {
                file = args.get(i);
                i += 1;
            }
        }
    }

    let input = match file {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("read {p}: {e}");
            std::process::exit(1);
        }),
        None => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .expect("stdin utf-8");
            s
        }
    };
    match rs::export_md::run(&input, &o) {
        Ok(out) => print!("{out}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn flag_value(args: &[String], i: usize, flag: &str) -> String {
    args.get(i + 1).cloned().unwrap_or_else(|| {
        eprintln!("{flag} wants a value");
        std::process::exit(2);
    })
}

fn compress(args: &[String]) {
    let mut mode = "auto".to_string();
    let mut side_path = default_side_path();
    let mut cfg = Cfg::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                mode = args[i + 1].clone();
                i += 2;
            }
            "--side" => {
                side_path = args[i + 1].clone();
                i += 2;
            }
            "--keep-thinking" => {
                cfg.keep_thinking = true;
                i += 1;
            }
            "--no-surp" => {
                cfg.surp_on = false;
                i += 1;
            }
            "--thin" => {
                let r: f64 = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("--thin wants a rate in (0, 1], e.g. 0.85");
                    std::process::exit(2);
                });
                if !(0.0..=1.0).contains(&r) || r == 0.0 {
                    eprintln!("--thin wants a rate in (0, 1], got {}", r);
                    std::process::exit(2);
                }
                cfg.thin_rate = Some(r);
                i += 2;
            }
            "--evict-last" => {
                cfg.evict_last = args[i + 1].parse().unwrap();
                i += 2;
            }
            other => {
                eprintln!("unknown flag {}", other);
                std::process::exit(2);
            }
        }
    }

    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("stdin utf-8");
    let text = rs::mask::strip_ansi(&input);

    let resolved = match mode.as_str() {
        "auto" => rs::detect(&text).to_string(),
        "session" => "session-json".to_string(), // alias: the json renderer takes all envelopes
        "stream" | "session-md" | "session-json" => mode.clone(),
        other => {
            eprintln!("unknown --mode {other:?} (auto|stream|session|session-md|session-json)");
            std::process::exit(2);
        }
    };
    let mut table = SideTable::load(&side_path);

    let out = match resolved.as_str() {
        "session-md" => rs::session_md::run(&text, &mut table, &cfg),
        "session-json" => rs::session_json::run(&text, &mut table, &cfg),
        _ => rs::stream(&text, &mut table, &cfg),
    };
    table.save(&side_path);

    let mut stdout = std::io::stdout().lock();
    stdout.write_all(out.as_bytes()).unwrap();
    if tokens::warm() {
        // session tiers already initialized the BPE for their gates; the
        // summary counts are nearly free then. On a cold process (stream
        // tier) they would force a tokenizer init this run never needed
        // (~17 ms from the compiled .tkz, ~410 ms from tokenizer.json).
        let (t0, t1) = (tokens::count(&text), tokens::count(&out));
        eprintln!(
            "rstring: mode={} tokens {} -> {} ({:.1}% less) | side-table {} entries",
            resolved,
            t0,
            t1,
            100.0 * (1.0 - t1 as f64 / t0.max(1) as f64),
            table.len()
        );
    } else {
        eprintln!(
            "rstring: mode={} bytes {} -> {} | side-table {} entries (token counts: rstring tokens)",
            resolved,
            text.len(),
            out.len(),
            table.len()
        );
    }
}

/// Exact o200k token counts — the scoring unit for reproduction runs.
fn tokens_cmd(files: &[String]) {
    if files.is_empty() {
        eprintln!("usage: rstring tokens <files...>");
        std::process::exit(2);
    }
    for f in files {
        let raw = std::fs::read(f).unwrap_or_else(|e| {
            eprintln!("cannot read {}: {}", f, e);
            std::process::exit(1);
        });
        let text = String::from_utf8_lossy(&raw);
        println!("{}\t{}\t{}", f, raw.len(), tokens::count(&text));
    }
}

/// Build the static thin table from a corpus of agent conversations.
fn thin_table(args: &[String]) {
    let mut out = None;
    let mut files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                out = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                files.push(other.to_string());
                i += 1;
            }
        }
    }
    if files.is_empty() {
        eprintln!("usage: rstring thin-table [--out P] <files...>");
        std::process::exit(2);
    }
    let t = maki_rstring::thin::Table::build(&files).unwrap_or_else(|e| {
        eprintln!("thin-table: {}", e);
        std::process::exit(1);
    });
    let path = out
        .map(std::path::PathBuf::from)
        .unwrap_or_else(maki_rstring::thin::default_path);
    t.save(&path).unwrap_or_else(|e| {
        eprintln!("thin-table: save {}: {}", path.display(), e);
        std::process::exit(1);
    });
    eprintln!(
        "{}: {} tokens, {} distinct ids, tokenizer {} ({})",
        path.display(),
        t.total,
        t.distinct,
        maki_rstring::tokens::pretok_name(),
        maki_rstring::tokens::vocab_path().display()
    );
}

/// End-to-end scoreboard: per file — mode, bytes, tokens, % saved, wall time.
fn bench(files: &[String]) {
    if files.is_empty() {
        eprintln!("usage: rstring bench <files...>");
        std::process::exit(2);
    }
    println!(
        "{:<28} {:<12} {:>9} {:>9} {:>8} {:>8} {:>7} {:>9}",
        "file", "mode", "bytes", "bytes_out", "tok_in", "tok_out", "saved", "ms"
    );
    for (side_n, f) in files.iter().enumerate() {
        let input = std::fs::read_to_string(f).unwrap_or_else(|e| {
            eprintln!("cannot read {}: {}", f, e);
            std::process::exit(1);
        });
        let text = rs::mask::strip_ansi(&input);
        let mode = rs::detect(&text);
        let side_path = format!("/tmp/rstring-bench-side-{side_n}.json");
        let mut table = SideTable::load(&side_path);
        let cfg = Cfg::default();
        let t0 = std::time::Instant::now();
        let out = match mode {
            "session-md" => rs::session_md::run(&text, &mut table, &cfg),
            "session-json" => rs::session_json::run(&text, &mut table, &cfg),
            _ => rs::stream(&text, &mut table, &cfg),
        };
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        let _ = std::fs::remove_file(&side_path);
        let (ti, to) = (tokens::count(&text), tokens::count(&out));
        println!(
            "{:<28} {:<12} {:>9} {:>9} {:>8} {:>8} {:>6.1}% {:>9.1}",
            f.rsplit('/').next().unwrap_or(f),
            mode,
            text.len(),
            out.len(),
            ti,
            to,
            100.0 * (1.0 - to as f64 / ti.max(1) as f64),
            ms
        );
    }
}
