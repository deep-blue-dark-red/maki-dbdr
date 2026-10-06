# rstring

Token-efficient, semantically-preserving compression for agent sessions and
tool output. Deterministic, synchronous, no models, no async.

* **Recoverable by construction** — every elided body lands in a
  content-addressed side table; `rstring expand <hash16>` returns it
  byte-exact.
* **Merge-iff-volatile** — lines collapse only when they differ *solely* in
  volatile values (timestamps, UUIDs, epoch runs, temp paths). Semver, error
  codes, repo paths and short numbers are load-bearing and never masked.
* **Tiered by shape** — a log stream, a markdown session and a JSON session
  each get a renderer for their structure instead of a generic re-encoder.
* **Exact o200k pricing** — every gate and scoreboard is counted with the
  same BPE, and that counter is gated token-for-token against tiktoken-rs
  (`tests/tokens_parity.rs`); Qwen token spaces are supported via a
  tokenizer switch.

Deps: `tokie` (token counting, stable Rust), `serde_json`, `sha2`;
`tiktoken-rs` stays as a dev-dependency, the parity oracle.

Hands-on walkthrough: [TUTORIAL.md](TUTORIAL.md). Full method × shape sweep:
[bench/RESULTS.md](bench/RESULTS.md).

## Quick start

```sh
# stable toolchain (rust-toolchain.toml pins it)
cargo build --release
cargo install --path .          # optional: puts `rstring` on PATH

# tokenizer (not vendored): an o200k tokenizer.json — first run compiles a
# .tkz beside it, after which loads take ~17 ms instead of ~410 ms
curl -fsSL https://huggingface.co/Xenova/gpt-4o/resolve/main/tokenizer.json \
  -o ~/.cache/rstring/o200k_tokenizer.json
# or: ./bench/reproduce.sh        (also fetches competitor clones for the sweep)

rstring compress --side side.json < session.json > compact.txt
rstring expand <hash16> side.json
rstring bench file1 file2 ...
```

## Usage

```sh
rstring compress [--mode auto|stream|session|session-md|session-json] \
                 [--side P] [--keep-thinking] [--no-surp] \
                 [--thin RATE] [--evict-last N] < in > out
rstring expand <hash16> [side-path]      # default side: .rstring-side.json
rstring bench <files...>                 # per-file scoreboard
rstring tokens <files...>                # o200k token counts
rstring thin-table [--out P] <files...>  # build the static thin table
rstring -V | --version
```

| mode | input shape | what it does |
|---|---|---|
| `auto` | any | [`detect`](src/lib.rs) then dispatch (default) |
| `stream` | bash output, logs, JSONL | uniform JSONL → columnar table (`<rec>` header, never thinned); else volatile-merge clustering ([xN]) + entropy masking + lossless repeated-span factoring (`<n>` legend; non-columnar output re-compresses byte-identically, columnar output does not — see TUTORIAL.md §9). `--thin` applies to the clustered bulk |
| `session-md` | maki markdown exports | thinking → extractive decisions (+ side table), tool outputs → volatile-merge clustering, stale non-errors → `<r:>` refs, prose → gated surprisal drop |
| `session-json` | maki JSON exports, claude-code JSONL, parsed turn files | compact transcript renderer: drops the duplicate `tool_outputs` mirror, replaces JSON key syntax with one-word markers, evicts stale thinking/tool outputs to `<r:>` stubs |

`--mode session` is an alias for `session-json`.

Knobs: `--keep-thinking` (don't stub thinking blocks), `--no-surp` (no
word-drop on machine-generated bulk), `--evict-last N` (default 3: the last
N tool outputs plus every error stay inline), `--thin RATE` (static thin
tier, see below), `--side P` (side-table path).

## Reading compressed output

| marker | meaning |
|---|---|
| `[xN]` | N lines collapsed because they differ only in volatile values (the count is informational; the original lines are not individually re-referenced) |
| `<r:hash16 n=… v="preview">` | elided body; full original in the side table — `rstring expand hash16` |
| `<n>` in a lead legend | repeated-span factoring: `<0> /path/prefix` defines `<0>`, used as `<0>-138- …` |
| `<rec field,…>` | columnar JSONL: header states keys once, rows are TSV |
| `<think r=hash16 n=…>` | extractive decision summary of a thinking block; original via `expand` |
| `<TS>` | masked volatile value inside a retained line |

## Design rules

1. **Merge-iff-volatile.** Two lines collapse into one `[xN]` entry iff their
   masked keys are equal — i.e. they differ *only* in volatile values (ISO
   timestamps, UUIDs, epoch runs, temp paths). Semver, error codes, repo paths
   and short numbers are load-bearing and never masked, so the over-merge
   cliff (ltk@0.7 destroying 11 of 12 provider names) is structurally
   impossible, not threshold-tuned away.
2. **Word-drop only on machine-generated bulk.** Surprisal-ranked word pruning
   is scoped to kept tool-output bodies. Narration, decisions, user turns and
   parse-valid structured lines are always verbatim.
3. **Every elision is recoverable.** Evicted thinking blocks and stale tool
   outputs become `<r:hash n=tokens v="first line">` stubs backed by a
   content-addressed side table; `expand` returns the original byte-exact.
   Errors and the last `evict_last` outputs stay inline.
4. **Duplication is dropped, not re-encoded.** maki JSON exports carry every
   tool result twice (inline `tool_result` + `tool_outputs` map) plus full JSON
   syntax; the transcript renderer states keys once and drops the mirror map.

## Benchmarks

### Standardized benchmark (`./bench/standard.py`)

The canonical rstring-only scoreboard: the fixed corpus in `bench/cases.tsv`,
auto tier, fresh side table per case, exact o200k counts. Every case is
gated on the recoverability contract — detected tier as expected, every
emitted ref expands byte-exact, every side key is sha256(text)[:16], no
orphan entries, md bodies faithful, and output bytes identical across
repeated compressions. Results are written to `bench/standard/results.tsv`
and `results.json`; exit code is non-zero on any gate failure (`--check`
runs the gates without writing files). Token columns are the comparable
unit; `ms` is in-process compression time (median of `--runs`, default 3)
and moves with machine load.

Current baseline (11 cases): **2,194,795 → 581,882 tok, 73.5% less**.

### Real inputs (exact o200k tokens, Apple silicon, single-threaded)

| input | mode | tokens | saved | ms |
|---|---|---|---|---|
| session-*.json (648 KB maki session export) | session-json | 188,413 → 120,146 | **36.2%** | 79.9 |
| session-*.md (488 KB md export, same session) | session-md | 136,496 → 120,092 | **12.0%** | 81.2 |
| log.log (4 KB JSONL warns) | stream | 1,202 → 382 | **68.2%** | 0.2 |
| turns.json (445 KB parsed agent request) | session-json | 132,708 → 116,508 | **12.2%** | 78.7 |
| claude-zig.jsonl (1.0 MB claude-code session) | session-json | 339,253 → 59,101 | **82.6%** | 58.5 |

Four more claude-code shapes (maki harness coding, indexer engineering, web
research, dense subagent): **81.7–92.1%** — full table in `bench/RESULTS.md`.
Two non-JSON structured shapes (TOML lock records, YAML endpoint records):
**61.6% / 91.3%** (`bench/standard.py`) with every record kept
distinct — ltk's higher numbers there come from masking the load-bearing IPs
and package names away. Head-to-head on
identical agent-request turn files (`bench/claude_to_turns.py`): ogham's
full-eviction contract leads at 77–95% (nothing visible without a retrieve);
rstring beats llmtrim on 4 of 6 — table in `bench/RESULTS.md`.

The maki-export rows are a low-redundancy session — transcript review and
benchmark work, no duplicate outputs — which is the hard case for
dedup-driven compression (the previous repetition-heavy session: 75.8% /
56.2%, preserved in `bench/RESULTS.md` history); `claude-session.jsonl` is
the counterweight, a real redundancy-heavy coding session at 82.5%. Semantic
capture on that previous corpus: 21/21 probe facts (entity names, versions,
paths, error codes) present verbatim, 20/20 json-native; re-compressing
rstring's own session output moved −0.8% on that corpus (session renderers
are near-idempotent — idempotence is shape-dependent; see the fixed-point
section of [TUTORIAL.md](TUTORIAL.md#9-re-compressing-is-it-a-fixed-point)).

### Criterion (`cargo bench`, deterministic synthetic corpora)

| bench | time | throughput |
|---|---|---|
| tiers/stream_jsonl_columnar (2 MB JSONL) | 62.3 ms | 39.0 MiB/s |
| tiers/stream_cluster (1 MB log spam) | 12.1 ms | 75.8 MiB/s |
| tiers/session_md (1.2 MB md session) | 5.1 ms | 73.0 MiB/s |
| tiers/session_json (614 KB json session) | 4.4 ms | 42.6 MiB/s |
| tiers/surp_prose (400 KB prose) | 7.1 ms | 35.5 MiB/s |
| micro/mask_merge_key (70 B line) | 885 ns | 85.1 MiB/s |
| micro/tokens_count (64 B) | 180 ns | 340 MiB/s |

Counting is tokie's: a SIMD pretokenizer plus backtracking BPE that builds
on **stable** Rust, with no git dependency and none of the previous
backend's weight (gigatoken needed nightly `portable_simd` and pulled
pyo3/arrow/parquet into the graph). It encodes session text roughly 10×
faster than tiktoken-rs (11.7 → 75–190 MB/s on 425 KB), and one-shot CLI
runs pay a one-time tokenizer load instead of a BPE build: ~17 ms from the
compiled `.tkz`, ~410 ms on the first run that parses `tokenizer.json`
(see `bench/RESULTS.md`, Latency).

Token reduction on the same synthetic corpora (printed by `cargo bench`):

| case | tok_in | tok_out | saved |
|---|---|---|---|
| jsonl_columnar | 880,080 | 330,955 † | 62.4% |
| stream_cluster | 408,000 | 11,400 | 97.2% |
| session_md | 144,813 | 22,313 | 84.6% |
| session_json | 74,923 | 19,221 | 74.3% |

† tokie prices a tab-then-`<` boundary one token higher than tiktoken
(`"\t<ID>"` → 4 ids vs 3), and that synthetic corpus has exactly 10,000
such lines. tiktoken-rs prices the same bytes at 320,955 (63.5%); the
divergence is pinned by name in `tests/tokens_parity.rs`.

Note: lines whose load-bearing digits make them unique (distinct job ids,
latencies) correctly do *not* merge — numbers under 9 digits are treated as
meaningful. `stream_cluster`'s 97% applies to log spam where payloads repeat
and only timestamps vary.

### Context (uniform o200k scoring — reproduce with `bench/reproduce.sh`, details in `bench/RESULTS.md`)

| tool | session md | session json | load-bearing facts | elided content recoverable |
|---|---|---|---|---|
| **rstring** | −12.0% | −36.2% | 100% verbatim* | yes — side table |
| ltk @0.95 | −21.3% | −29.9% | versions/paths masked away | no |
| llmtrim aggressive | −86.5%* | — | 100%* | §-handles |
| ogham agent | −39.1%* | — | 77% visible* | yes — CCR store |
| sqz auto | −92.3%* | −0.0% (inflates) | presence only, relations gone | no |
| zstd −12 pasted | −31.0% | −49.3% | unreadable to the model | n/a |

\* qualitative columns were recorded on the previous repetition-heavy corpus
(contracts unchanged — see `bench/RESULTS.md`). Not like-for-like rows:
llmtrim and ogham are fed `turns.json` (132,708 tok parsed from the
136,496-tok md export) — against the full export they read −86.8% and −40.8%
end-to-end (like-for-like table in `bench/RESULTS.md`). This corpus is
low-redundancy, so bulk pruners (llmtrim, sqz) lead on raw tokens while
recoverable methods keep every fact — the ranking inverts with session shape.
llmtrim prunes lossy-in-context; ogham evicts behind a host-side store
(visible-vs-retrievable is a different contract); sqz's number is truncation
wearing a compression costume. Competitor harnesses and prototypes live under
`bench/harnesses/`.

## Static thin tier

`--thin RATE` (default off; surp stays default) drops the most predictable
*tokens* of kept tool-output bulk, scored by a static unigram table instead of
per-document bigrams, budgeted on the measured (re-encoded) token count.
Build the table once from a conversation corpus:

```sh
rstring thin-table <agent-session files...>   # ~/.cache/rstring/thin-table.bin
rstring compress --mode session-json --thin 0.85 < in > out
```

The session renderers thin kept tool-output bodies; on the stream/auto path
`--thin` thins the clustered bash output (columnar JSONL rows are never
thinned — table fields are load-bearing). Tables are fingerprinted to their
tokenizer file + pretokenizer and rejected on mismatch.

Guards: lowercase-ASCII prose words only — identifiers
(CamelCase/digits/backticks/underscores), non-ASCII spans, unseen-in-corpus
tokens, and line boundaries are never dropped.

## Qwen-family token space

Qwen1–3 share one byte-BPE lineage. Point `RSTRING_O200K` at a Qwen
`tokenizer.json` (the Hub exports one, e.g. `Qwen/Qwen-7B`) and set
`RSTRING_PRETOK=qwen2` — counting and the thin tier then price everything
in Qwen ids:

```sh
RSTRING_O200K=~/models/qwen_tokenizer.json RSTRING_PRETOK=qwen2 rstring tokens file.jsonl
```

(tokie has no Qwen2 scheme of its own — Qwen's ByteLevel split is GPT-2's,
so `qwen2` maps to it.)

The o200k tokenizer lives at `~/.cache/rstring/o200k_tokenizer.json` unless
`RSTRING_O200K` overrides it; it is deliberately not vendored, and
`bench/reproduce.sh` provisions it.

## Tests

`cargo test` — 37 tests (33 integration + 4 unit). Includes the two that must
never break: lines differing in load-bearing text never merge, and every
`<r:>` ref resolves byte-exact from the side table. Token counting is checked
against tiktoken-rs over the full bench corpus and adversarial edge cases
(CJK, ZWJ emoji, special-token literals) as a *divergence tripwire*: any
input not pinned in `tests/tokens_parity.rs` must match tiktoken exactly,
and the handful that are pinned disagree by name there.

The benchmark harness has its own stdlib-unittest suite:
`python3 bench/test_standard.py` (28 tests: gate logic, case parsing, bench
output parsing, plus end-to-end gate checks against the real binary).

`cargo bench` runs the criterion scoreboard on deterministic synthetic
corpora.

## License

Apache-2.0.
