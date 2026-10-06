//! Static token-surprisal thinning (lossy semantic tier).
//!
//! Unlike [`crate::surp`] — which counts bigrams *per document* and therefore
//! has noisy statistics on the short prose lines that dominate kept tool
//! output — scores here come from a table of unigram token counts built once
//! offline over a domain corpus (`rstring thin-table`). Runtime is pure
//! lookup: no counting pass, deterministic, stable on any line length.
//!
//! Budget is token-exact, not word-count-exact: a line is thinned until its
//! *measured* BPE count (re-encoded, because merges shift at the seams) is
//! at or below `ceil(n_tokens * keep_rate)`. Scoring, budgeting, and the
//! table are all in the deployed tokenizer's id space — swap the tokenizer
//! (e.g. a Qwen `tokenizer.json`) and `RSTRING_PRETOK=qwen2`, rebuild the
//! table, and thinning prices every drop in Qwen3 tokens.
//!
//! Safety rails are surp's plus two: non-ASCII spans are never dropped
//! (byte-level BPE tokens can be *incomplete* UTF-8 sequences — a mid-
//! character drop would corrupt text), and drops stop at the measured
//! token target instead of a word-count rate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::tokens;

/// Add-α smoothing mass. Unseen tokens get p = α/(N + α·V) ≈ 3e-7 on the
/// bench corpus — a floor high enough that unknown spans (identifiers,
/// paths, hashes) sort after every stopword and are never reached by the
/// drop loop at sane keep rates.
const ALPHA: f64 = 0.5;

const MAGIC: &[u8; 4] = b"rthn";
const VERSION: u8 = 1;

pub struct Table {
    counts: HashMap<u32, u32>,
    /// Total token occurrences the table was built from.
    pub total: u64,
    /// Distinct token ids observed.
    pub distinct: usize,
    /// Vocab size of the tokenizer (smoothing denominator).
    vocab_size: u32,
    /// sha256(tokenizer file bytes) ++ pretok name — tables are only valid
    /// for the tokenizer they were counted with.
    fingerprint: Vec<u8>,
}

fn fingerprint() -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    if let Ok(b) = std::fs::read(tokens::vocab_path()) {
        h.update(&b);
    }
    h.update(tokens::pretok_name().as_bytes());
    let mut fp = h.finalize().to_vec();
    fp.truncate(16);
    fp
}

impl Table {
    /// Count unigram token ids over one in-memory text.
    pub fn from_text(text: &str) -> Table {
        let mut t = Table::empty();
        t.count_into(text);
        t
    }

    fn empty() -> Table {
        Table {
            counts: HashMap::new(),
            total: 0,
            distinct: 0,
            vocab_size: tokens::vocab_size(),
            fingerprint: fingerprint(),
        }
    }

    fn count_into(&mut self, text: &str) {
        for id in tokens::ids(text) {
            *self.counts.entry(id).or_insert(0) += 1;
            self.total += 1;
        }
        self.distinct = self.counts.len();
    }

    /// Count unigram token ids over the given files (ANSI-stripped).
    pub fn build(files: &[String]) -> std::io::Result<Table> {
        let mut t = Table::empty();
        for f in files {
            let raw = std::fs::read_to_string(f)?;
            t.count_into(&crate::mask::strip_ansi(&raw));
        }
        if t.total == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "corpus yielded zero tokens",
            ));
        }
        Ok(t)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.push(VERSION);
        buf.extend_from_slice(&self.fingerprint);
        buf.extend_from_slice(&self.total.to_le_bytes());
        buf.extend_from_slice(&(self.distinct as u32).to_le_bytes());
        buf.extend_from_slice(&self.vocab_size.to_le_bytes());
        let mut ids: Vec<u32> = self.counts.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            buf.extend_from_slice(&id.to_le_bytes());
            buf.extend_from_slice(&self.counts[&id].to_le_bytes());
        }
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        std::fs::write(path, buf)
    }

    pub fn load(path: &Path) -> Result<Table, String> {
        let buf =
            std::fs::read(path).map_err(|e| format!("thin table {}: {}", path.display(), e))?;
        if buf.len() < 4 + 1 + 16 + 8 + 4 + 4 || &buf[0..4] != MAGIC {
            return Err(format!(
                "thin table {}: not an rstring table",
                path.display()
            ));
        }
        if buf[4] != VERSION {
            return Err(format!(
                "thin table {}: unsupported version {}",
                path.display(),
                buf[4]
            ));
        }
        let fp = buf[5..21].to_vec();
        if fp != fingerprint() {
            return Err(format!(
                "thin table {} was built for a different tokenizer (ranks/pretokenizer changed) — rebuild with `rstring thin-table`",
                path.display()
            ));
        }
        let total = u64::from_le_bytes(buf[21..29].try_into().unwrap());
        let distinct = u32::from_le_bytes(buf[29..33].try_into().unwrap()) as usize;
        let vocab_size = u32::from_le_bytes(buf[33..37].try_into().unwrap());
        let mut counts = HashMap::with_capacity(distinct);
        let mut off = 37;
        for _ in 0..distinct {
            let id = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
            let c = u32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap());
            counts.insert(id, c);
            off += 8;
        }
        Ok(Table {
            counts,
            total,
            distinct,
            vocab_size,
            fingerprint: fp,
        })
    }

    /// −log p under add-α smoothing; the higher, the more informative.
    fn surprisal(&self, id: u32) -> f64 {
        let c = self.counts.get(&id).copied().unwrap_or(0) as f64 + ALPHA;
        let denom = self.total as f64 + ALPHA * self.vocab_size.max(1) as f64;
        -(c / denom).ln()
    }

    /// A span is only thinning fodder if the corpus priced every one of
    /// its tokens. Unseen content — identifiers, paths, hashes, names — is
    /// load-bearing by definition: budget pressure must never reach it
    /// (the estimate loop would otherwise walk past stopwords into it at
    /// aggressive rates).
    fn priced(&self, ids: &[u32]) -> bool {
        ids.iter().all(|id| self.counts.contains_key(id))
    }
}

pub fn default_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("RSTRING_THIN") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".cache/rstring/thin-table.bin")
}

static TABLE: OnceLock<Mutex<Option<Table>>> = OnceLock::new();

/// Lazily load the process-wide static table. Missing or stale tables are
/// a hard error with a rebuild pointer, mirroring tokens.rs's stance on
/// missing ranks.
fn table() -> &'static Mutex<Option<Table>> {
    TABLE.get_or_init(|| {
        Mutex::new(match Table::load(&default_path()) {
            Ok(t) => Some(t),
            Err(e) => panic!(
                "{} — build one with `rstring thin-table <corpus files...>`",
                e
            ),
        })
    })
}

/// Guard rails shared with surp, plus thin's two additions. Returns false
/// when a span must never be dropped.
fn droppable(w: &str) -> bool {
    if !w.is_ascii() {
        return false; // byte-BPE tokens may split UTF-8 characters
    }
    if w.chars().any(|c| c.is_uppercase()) {
        return false; // CamelCase / proper nouns read as identifiers
    }
    if w.chars().all(|c| c.is_ascii_punctuation()) {
        return false;
    }
    !w.ends_with(['.', '?', ':', ';'])
        && !w
            .chars()
            .any(|c| c.is_ascii_digit() || c == '`' || c == '_')
}

pub fn run(text: &str, keep_rate: f64) -> String {
    let guard = table().lock().unwrap();
    let tbl = guard.as_ref().expect("thin table");
    run_with(text, keep_rate, tbl)
}

/// The thin pass proper, against an explicit table (tests and callers
/// that keep per-domain tables).
pub fn run_with(text: &str, keep_rate: f64, tbl: &Table) -> String {
    let tok = tokens::tok();

    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if crate::surp::structured(line) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let n_tok = tok.encode_ids(line, false).len();
        let target = (n_tok as f64 * keep_rate).ceil() as usize;
        let ws: Vec<&str> = line.split(' ').filter(|w| !w.is_empty()).collect();
        if target >= n_tok || ws.len() < 4 {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Per-span token ids: spans re-join with single spaces, so a
        // non-first span contributes exactly `" " + span` — pretokens never
        // merge across whitespace, making per-span counts context-free.
        // Index 0 is never droppable, so "first" stays first.
        let mut span_ids: Vec<Vec<u32>> = Vec::with_capacity(ws.len());
        for (i, w) in ws.iter().enumerate() {
            let ids = if i == 0 {
                tok.encode_ids(w, false)
            } else {
                let mut b = String::with_capacity(w.len() + 1);
                b.push(' ');
                b.push_str(w);
                tok.encode_ids(&b, false)
            };
            span_ids.push(ids);
        }
        // Candidates: (score, index, token estimate), most predictable first.
        let mut cands: Vec<(f64, usize, usize)> = (1..ws.len())
            .filter(|&i| droppable(ws[i]) && i + 1 != ws.len() && tbl.priced(&span_ids[i]))
            .map(|i| {
                let s: f64 = span_ids[i].iter().map(|&id| tbl.surprisal(id)).sum();
                (s, i, span_ids[i].len().max(1))
            })
            .collect();
        cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

        let mut drop = vec![false; ws.len()];
        let mut ci = 0;
        let mut saved = 0usize;
        while saved < n_tok - target && ci < cands.len() {
            let (_, i, t) = cands[ci];
            drop[i] = true;
            saved += t;
            ci += 1;
        }
        // Verify on the rebuilt line — merges shift at seams, so only the
        // re-encoded count is truth — and correct by dropping one more.
        loop {
            let mut rebuilt = String::with_capacity(line.len());
            let mut first = true;
            for (i, w) in ws.iter().enumerate() {
                if drop[i] {
                    continue;
                }
                if !first {
                    rebuilt.push(' ');
                }
                first = false;
                rebuilt.push_str(w);
            }
            let rebuilt_ids = tok.encode_ids(&rebuilt, false);
            if rebuilt_ids.len() <= target || ci >= cands.len() {
                out.push_str(&rebuilt);
                break;
            }
            let (_, i, _) = cands[ci];
            drop[i] = true;
            ci += 1;
        }
        out.push('\n');
    }
    out
}
