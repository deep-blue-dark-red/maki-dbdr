# Tutorial: rstring on agent sessions

A hands-on walkthrough: count tokens, compress a session export, recover what
was elided, score a batch, and verify that nothing is lost. All example
numbers below are from a current build on the committed bench assets.

Replace `rstring` with the built binary if you haven't installed it:

```sh
cargo build --release            # then: ./target/release/rstring
cargo install --path .           # or: put `rstring` on PATH
```

If you set `CARGO_TARGET_DIR`, the binary is at
`$CARGO_TARGET_DIR/release/rstring`.

## 0. Provision the tokenizer

Counting and the compression gates are exact o200k and need a
`tokenizer.json` (deliberately not vendored):

```sh
mkdir -p ~/.cache/rstring
curl -fsSL https://huggingface.co/Xenova/gpt-4o/resolve/main/tokenizer.json \
  -o ~/.cache/rstring/o200k_tokenizer.json
```

The first run compiles `o200k_tokenizer.tkz` beside it (one-off ~410 ms);
after that loads are ~17 ms. `RSTRING_O200K=/path` overrides the location,
and `./bench/reproduce.sh` also provisions it (and fetches the competitor
clones for the full sweep).

## 1. Count first

Tokens are the unit everything is scored in:

```console
$ rstring tokens bench/bench_assets/log.log bench/bench_assets/session-CfZnPneytdphBS8Xd4Xpt.md
bench/bench_assets/log.log	4196	1202
bench/bench_assets/session-CfZnPneytdphBS8Xd4Xpt.md	488004	136496
```

Columns: path, bytes, exact o200k tokens.

## 2. Compress a session export

Point it at the JSON session export (not a raw shell transcript):

```console
$ rstring compress --side /tmp/side.json \
    < bench/bench_assets/session-CfZnPneytdphBS8Xd4Xpt.json > /tmp/compact.txt
rstring: mode=session-json tokens 188413 -> 120146 (36.2% less) | side-table 61 entries
```

The file shrank from 648 KB to 417,854 bytes of readable transcript: a header
of short scalars,
`<user>` / `<assistant>` markers, tool calls as `> name key="…"`
one-liners, thinking as extractive `<think r=…>` summaries, and JSON key
syntax gone. `tool_outputs` shows `<dropped — mirrors inline tool_result
content>` — that mirror duplication is dropped, not re-encoded.

The same session as its markdown export:

```console
$ rstring compress --side /tmp/side-md.json \
    < bench/bench_assets/session-CfZnPneytdphBS8Xd4Xpt.md > /tmp/compact-md.txt
rstring: mode=session-md tokens 136496 -> 120092 (12.0% less) | side-table 61 entries
```

And a plain log stream (auto-detected, no tokenizer needed — see §7):

```console
$ rstring compress < bench/bench_assets/log.log > /tmp/compact-log.txt
rstring: mode=stream bytes 4196 -> 1366 | side-table 0 entries (token counts: rstring tokens)

$ rstring tokens /tmp/compact-log.txt
/tmp/compact-log.txt	1366	382
```

```text
<rec fields,level,target,timestamp>
{"message":"failed to create provider, skipping","provider":"anthropic"}	WARN	maki_providers::provider	<TS> [x2]
{"message":"failed to create provider, skipping","provider":"openai"}	WARN	maki_providers::provider	<TS> [x2]
```

Uniform JSONL becomes a columnar table: keys stated once in the `<rec>`
header, rows as TSV, volatile values as `<TS>`, duplicates collapsed to
`[xN]`. Marker reference: README → *Reading compressed output*.

## 3. Recover an elided body

Every `<r:hash16 …>` stub resolves through the side table written next to the
run:

```console
$ grep -o '<r:421054622c7b8c1e[^>]*>' /tmp/compact.txt
<r:421054622c7b8c1e n=281 v="  218173 /tmp/maki/rstring/llmtrim-turns.json">

$ rstring expand 421054622c7b8c1e /tmp/side.json | head -6
  218173 /tmp/maki/rstring/llmtrim-turns.json
keys: ['model', 'max_tokens', 'system', 'messages']
n system: 1
n messages: 129
Counter({'user': 65, 'assistant': 64})
```

`expand` writes the original bytes verbatim (no added newline). To check that
*every* ref created by a run resolves:

```sh
out=/tmp/compact.txt; side=/tmp/side.json
grep -o '<r:[0-9a-f]\{16\}' "$out" | sort -u | cut -c4- |
while read -r h; do
  rstring expand "$h" "$side" >/dev/null || echo "unresolved: $h"
done
```

Caveat: if you compress material that already contains `<r:…>` stubs as
*literal text* (e.g. a transcript of an earlier rstring run), those
pre-existing hashes are not in your side table and won't expand — they were
not created by this run.

## 4. Score a batch

```console
$ rstring bench bench/bench_assets/log.log \
      bench/bench_assets/session-CfZnPneytdphBS8Xd4Xpt.json
file                              mode           bytes bytes_out   tok_in  tok_out   saved        ms
log.log                           stream          4196      1366     1202      382   68.2%       0.2
session-CfZnPneytdphBS8Xd4Xpt.json session-json  648275    417854   188413   120146   36.2%      72.1
```

`ms` is compression time only (tokenizer init is extra on a cold process —
~17 ms from the compiled `.tkz`, ~410 ms the first run parses the JSON;
session tiers already pay it for their gates). For the canonical,
gated version of this over the fixed corpus, use `./bench/standard.py` (§10).

## 5. Choosing a mode

`auto` (the default) sniffs the input:

| input | detects as |
|---|---|
| whole-document JSON with `messages` (maki export, API request) | `session-json` |
| JSON array of flat turns (`type` or `role` per element) | `session-json` |
| claude-code-style JSONL (≥30% of the first 50 lines wrap `message.role`) | `session-json` |
| markdown containing `<details` and `**Output:**` | `session-md` |
| anything else (logs, bash output, prose) | `stream` |

Caveats:

* maki's **on-disk session records** (`{"t":"msg","d":{…}}` per line) are not
  an export envelope and none of the rules match — they fall back to
  `stream`. Export the session (or wrap the messages in a
  `{"messages":[…]}` envelope) if you want the `session-json` tier.
* Forcing `--mode session-json` on unknown JSONL folds every line into an
  opaque `[record r=hash n=…]` stub: ~all content goes to the side table
  (recoverable, but the visible transcript is gone). Use it only for shapes
  the renderer knows.
* `[xN]` counts are informational: identical (after masking) lines collapse
  to their first occurrence, so session renders show a single
  `<user> [xN]` / `<assistant> [xN]` marker rather than one per turn.

## 6. Knobs

```console
$ rstring compress --keep-thinking   # don't stub thinking; keep the full text
$ rstring compress --no-surp         # no word-drop on kept tool-output bulk
$ rstring compress --evict-last 8    # keep the last 8 tool outputs inline (default 3)
$ rstring compress --side out.json   # where the recoverable side table lands
```

Errors are always kept inline regardless of `--evict-last`. `--side` defaults
to `.rstring-side.json` in the current directory; it is loaded and saved on
every run, so it accumulates across runs and lets you `expand` older hashes
as long as you keep the file. Use a fresh path per corpus when you want
isolation.

## 7. Static thin tier (optional)

`--thin RATE` drops the most predictable *tokens* of kept tool-output bulk
using a static table built from a conversation corpus:

```console
$ rstring thin-table --out /tmp/thin.bin bench/bench_assets/session-CfZnPneytdphBS8Xd4Xpt.json
/tmp/thin.bin: 188413 tokens, 7929 distinct ids, tokenizer o200k (/Users/mcp/.cache/rstring/o200k_tokenizer.json)

$ rstring compress --mode session-json --thin 0.85 --side /tmp/side-thin.json \
    < bench/bench_assets/session-CfZnPneytdphBS8Xd4Xpt.json > /tmp/compact-thin.txt
rstring: mode=session-json tokens 188413 -> 120585 (36.0% less) | side-table 61 entries
```

Notes:

* Default table path is `~/.cache/rstring/thin-table.bin`.
* Tables are fingerprinted to their tokenizer file + pretokenizer; a mismatched
  table is rejected rather than silently mispricing tokens.
* Guards never drop identifiers (CamelCase/underscores/backticks), non-ASCII
  spans, unseen-in-corpus tokens, or line boundaries — so on low-redundancy
  or code-heavy input the effect is small, and on this session it is
  currently *negative*: 188,413 → 120,585 with `--thin 0.85` vs 120,146
  without (+439 tok, the same delta on the md shape). Thin is budget-exact
  per line (`tests/thin.rs`), so the extra tokens come from the line rebuild
  rather than the budget — treat thin as a win only where the table has
  real prose bulk to drop.
* On the stream path only clustered bash output is thinned; columnar JSONL
  rows are never touched.

## 8. Qwen token space

Qwen1–3 share one byte-BPE lineage; switch counting and the thin tier with a
`tokenizer.json` plus a pretokenizer flag:

```sh
RSTRING_O200K=~/models/qwen_tokenizer.json RSTRING_PRETOK=qwen2 \
  rstring tokens session.jsonl
```

The compiled sidecar is per-scheme (`qwen_tokenizer.qwen2.tkz`), so flipping
`RSTRING_PRETOK` against the same source file rebuilds instead of silently
counting in the other id space.

## 9. Re-compressing: is it a fixed point?

Shape-dependent, and worth knowing before you double-compress:

| input | pass 2 | measured |
|---|---|---|
| non-columnar stream output (174/200 random samples) | byte-identical | exact fixed point (verified `cmp`) |
| repeated-span factoring function alone | byte-identical | tested fixed point (`tests/repeat.rs::repeat_pass_is_a_fixed_point`) |
| stream output, columnar JSONL (`<rec>` table, 26/200) | unstable both ways | 15/26 shrink further (up to −27.8%: log.log 382 → 289 tok); 11/26 grow (up to +4.5%: 0054-jsonl 469 → 490 tok, bytes still 1648 → 1501) |
| session-json output (24 export envelopes) | median −2.7% | −0.6% … −12% |
| session-md output (10 windows) | median −3.4% | −0.7% … −5.6% |

"−x%" = pass 2 finds that much more reduction. Non-columnar stream output
is an exact fixed point, and the factoring function alone is too. The
columnar path is the exception: a pass-1 `<rec>` table is not recognized as
JSONL on a second pass, so cluster+factoring runs over the table text — that
can shrink further (repeated cell text becomes a legend) or grow in tokens
even when bytes shrink, because `<n>` refs fragment BPE tokens. Session
renderers are near fixed points (README's −0.8% figure is this class on its
corpus). If you re-compress, stop when the delta is 0 — and expect slightly
different results after a `<rec>` table.

## 10. Verify a run

* Standardized scoreboard + gates: `./bench/standard.py` — fixed corpus in
  `bench/cases.tsv`, auto tier, exact o200k counts, and per-case gates
  (expected tier, every emitted ref expands byte-exact, side keys integral,
  no orphans, md bodies faithful, deterministic output). Writes
  `bench/standard/results.tsv` / `results.json`, exits non-zero on failure;
  `--check` gates without writing files. Token columns are comparable across
  machines; `ms` moves with load.
* Exact token counts for any two files (`rstring tokens in out`).
* Ref resolution: the loop in §3, or spot-check one stub with `expand`.
* Invariants and tokenizer parity: `cargo test` (37 tests; includes
  tiktoken-rs parity over the bench corpus and adversarial edge cases).
* Benchmark harness tests: `python3 bench/test_standard.py` (27 tests).
* Full method × shape sweep against the competitors: `./bench/reproduce.sh`
  (writes `/tmp/maki/rstring-repro`, tables land in `bench/RESULTS.md` format).
* End-to-end latency: `./bench/latency.sh`. Throughput: `cargo bench`.

## 11. Troubleshooting

| symptom | cause / fix |
|---|---|
| `tokenizer unavailable at …` | provision the tokenizer (§0) or set `RSTRING_O200K` |
| stderr reports bytes, not tokens | cold process + `stream` tier never loaded the tokenizer; use `rstring tokens` for counts |
| `unknown flag` | flags are: `--mode`, `--side`, `--keep-thinking`, `--no-surp`, `--thin`, `--evict-last` |
| `expand` can't find a hash | it's a stub that was already literal in the input, or you're using the wrong `--side` file |
| `--thin` rejected | table fingerprint (tokenizer / pretokenizer) mismatch — rebuild with `thin-table` |
| no `session-json` wins on a maki session file | raw on-disk JSONL is not an export envelope (§5) |
| first run after provisioning is slow (~0.4 s) | it parses `tokenizer.json` and compiles the `.tkz` sidecar; later runs load it in ~17 ms. Delete `~/.cache/rstring/<stem>.<pretok>.tkz` to force a recompile |

## 12. Where things live

| path | what |
|---|---|
| `.rstring-side.json` (cwd) | default side table: `hash16 → original body` |
| `~/.cache/rstring/o200k_tokenizer.json` (+ `.o200k.tkz`) | o200k tokenizer, and its compiled form (name carries the pretokenizer, so `RSTRING_PRETOK` switches never share one) |
| `~/.cache/rstring/thin-table.bin` | default static thin table |
| `bench/RESULTS.md` | full benchmark tables (sweep record) |
| `bench/standard.py` + `bench/cases.tsv` | standardized scoreboard + gates |
| `bench/standard/results.tsv` | canonical results (regen with `standard.py`) |
| `bench/reproduce.sh` | end-to-end reproduction sweep (competitors) |
