# Performance Review — Round 3

Consolidation of five parallel audits (render path, checkpoint/meta path, storage
writer, Lua runtime, agent loop) against the code, plus measurements.

Every claim below was checked against the source. Where a number appears, it was
measured on this machine with a scratch harness, not estimated. Several findings
from the audits do not survive that check, and one recommendation would
reintroduce a bug fixed on this branch — those are recorded in full, because the
same claims have now survived three rounds of review by being plausible.

---

## Measurement first

The audits rank by *where allocations appear in the call graph*. That ordering
inverts once frequency is applied: a per-turn cost competes with a multi-second
network round trip, while a per-frame cost competes with a 16 ms budget.

| Path | Frequency | Budget |
|---|---|---|
| Per frame (animating) | 60 Hz | 16 ms |
| Per keystroke | ~10 Hz | human latency |
| Per turn | once per model response | seconds (API latency) |
| Per session load | once | startup |

A 1 ms/turn cost is 0.05 % of a 2 s turn. A 1 ms/frame cost is 6 % of a frame.
They are not comparable, and no audit round has weighted them.

---

## Findings that hold

### 0. Copying a selection renders the entire segment — `maki-ui/src/components/messages/selection.rs:35-70`

The largest item this round, and not previously quantified.

`extract_selection_text` loops over the segments a selection touches and, for
each, does:

```rust
let tmp_area = Rect::new(0, 0, width, h);   // h = the WHOLE segment height
let mut tmp = Buffer::empty(tmp_area);
Paragraph::new(seg.lines().to_vec())        // clone of the WHOLE segment
    .wrap(Wrap { trim: false })
    .render(tmp_area, &mut tmp);
```

then reads only rows `rel_start..rel_end`. So selecting three lines out of a
5000-line tool output deep-clones 5000 lines, allocates a 5000-row cell buffer
(≈ 500 000 `Cell`s at width 100), wraps and paints every row, and discards all
but three. It repeats per segment in the selection.

Measured, per segment, selecting 3 rows near the bottom:

| segment | current | windowed |
|---|---|---|
| 200 lines | 673 µs | 9.8 µs |
| 1000 lines | 2.54 ms | 6.9 µs |
| 5000 lines | 11.96 ms | 7.3 µs |

12 ms is a visible hitch on a single copy, and a selection spanning several
large outputs multiplies it.

This is the same defect `RenderCursor::render` had before `0aff7023`, in the one
place that fix did not reach, and `WrapIndex::window` already supplies what it
needs. The fix is to allocate `Rect::new(0, rel_start, width, rows)` — a buffer
addressed at absolute rows but only `rows` tall — and render the windowed line
range with the residual scroll.

One subtlety to get right: `append_rows` computes `rel_row = row - area.y` for
the `LineBreaks::is_line_start` lookup that decides newline placement in the
copied text, while `breaks` is built over the whole segment. Moving `area.y` off
zero desynchronises those. The clean version passes the breaks origin explicitly;
there are two production call sites (`selection.rs:626`,
`messages/selection.rs:69`).

**Status:** open, quantified, fix path known.

### 1. `request_tools` clones the full tools JSON every turn — `agent/run.rs:258-267`

With MCP connected, every turn deep-clones the base tools array and re-runs
`extend_tools`, which rebuilds a `HashSet<String>` of existing names
(`mcp/mod.rs:353-356`) and clones each loaded descriptor's `definition` Value.
Realistically 50–100 KB allocated and freed per turn.

Caching is harder than it looks: `search_tools`/`mark_loaded`
(`mcp/mod.rs:431-436, 471-472`) mutate `McpSession.loaded` without bumping any
generation, so a cache keyed on connect/disconnect goes stale after the first
`tool_search`. The comment at `mcp/mod.rs:300-301` — "extend_tools output must
never be stored" — is there for this reason. Any fix needs invalidation on both
index generation *and* `loaded` mutation.

**Status:** open. Real, bounded, medium risk.

### 2. `estimate_input_tokens` walks the whole history twice per turn — `agent/streaming.rs:158-187`

Called from `run.rs:284` and again from `streaming.rs:69` inside
`stream_with_retry` (`run.rs:305`), which runs every turn. Round 2 claimed this
was "once per turn, twice only on turn 0" — that missed the call inside
`stream_with_retry`, which Round 1 had correctly named. It is twice per turn.

Measured, 60 KB tools schema:

| history | per call |
|---|---|
| 50 messages | 51 µs |
| 200 messages | 72 µs |
| 800 messages | 192 µs |

So ≤ 0.4 ms/turn at 800 messages. Genuinely O(n) per turn and worth fixing for
tidiness, but it is not the second-highest-impact item in the codebase.

Note the two call sites estimate different things: `run.rs` walks raw history,
`stream_with_retry` walks image-adapted messages. Sharing one value changes
behaviour for image-bearing histories.

**Status:** open. Real, small.

### 3. `History::publish` clones the whole message list per push — `agent/history.rs:96-102`

Every `push`/`edit`/`rewrite` does `Arc::new(self.messages.clone())`.

Measured per push:

| history size | cost |
|---|---|
| 200 KB | 46 µs |
| 800 KB | 76 µs |
| 3.2 MB | 179 µs |

Round 1 called this "the main allocation hotspot… for a streaming turn with many
text deltas". Round 2 correctly refuted the delta part: text deltas never touch
`History` at all — they flow `ProviderEvent::TextDelta` → `forward_provider_events`
→ `AgentEvent::TextDelta` → the UI's streaming buffer. `History` is mutated only
at message boundaries. So this fires a handful of times per turn: **~1 ms/turn**.

Fixing it properly means `Vec<Arc<Message>>` threaded through `as_slice()` across
maki-agent, maki-ui and maki-acp. That is a poor trade for 1 ms/turn.

**Status:** open by decision, not oversight.

### 4. Lua dispatch polls on a 50 ms timer — `maki-lua/src/runtime.rs:2160-2199, 803-814, 2541-2562`

Three loops park on `Timer::after(DISPATCH_POLL_INTERVAL)`. The plugin-events
loop at `:2541` is the notable one: it runs for the entire lifetime of the Lua
thread, 20 wakes/sec, scanning the jobs map even with zero plugins and zero jobs.

`event_listener::Event` is already imported (`:15`) and the pattern is
established by `InflightGate` (`:983-1034`).

For calibration, the audit's own #3 — the watchdog at `WATCHDOG_POLL_INTERVAL =
10ms` — is 100 wakes/sec, unconditionally, and is correctly identified as
by-design and unfixable. Any claim that the 50 ms loop is "the single largest
background-wake source" is contradicted by that.

**Status:** open, explicitly deferred by the owner.

### 5. `drain_events` / `is_empty` / `kill_owner` are O(total jobs) — `maki-lua/src/api/fn.rs:168-243`

They filter the whole jobs map by owner rather than indexing by it.
`kill_owner` additionally allocates a `Vec` per task-scope drop, i.e. per tool
call completion. Indexing `JobStore` by `JobOwner` fixes all three.

**Status:** open. Low risk, scales with job count.

### 6. `json_to_lua` runs 3× per tool call — `maki-lua/src/runtime.rs:1931, 2283, 2389`

`compute_header`, `compute_permission_scopes` and `run_tool_call` each convert
the same logical input into Lua tables and strings.

One correction to the proposed fix: these arrive as **separate requests at
different times**, each carrying its own owned `Value`. "Convert once and pass
the `LuaValue` through the `Request` variants" has the direction backwards —
requests originate on non-Lua threads that have no `Lua` to convert with. The
request-id cache is the workable half.

**Status:** open. Real, medium risk.

### 7. `create_dir_all` on every session save — `maki-ui/src/storage_writer.rs:157` → `maki-storage/src/lib.rs:50-54`

`Writer::write` calls `ensure_subdir` per session per flush, so every save
re-runs `create_dir_all` on a directory that already exists.

Correction to the audit's framing: this is on the **storage writer thread**, not
the render path, and only runs when a save actually happens (gated by revision
change). It is not "the hottest cost". It is a free fix — hoist it to writer
construction.

**Status:** open. Trivial.

### 8. `SessionLog::append` re-serializes meta on every append — `maki-storage/src/sessions.rs:582-586`

`meta_record` clones title, subagents, usage_by_model and the whole `SessionMeta`,
then serializes, then byte-compares against `saved_meta`.

The short-circuit at `:583` essentially never fires, because `touch_soft`
(`:1251-1254`) sets `updated_at = now_epoch()` on every mutation, so the
serialized bytes always differ. Making it fire is a persistence design question
(what `updated_at` means on disk), not a cleanup.

**Status:** open, non-trivial.

### 9. Minor per-frame costs in `build_meta` — `maki-ui/src/app/session.rs:173-182`

`session_rules_snapshot()` clones the rule Vec and `text_messages()` locks the
queue mutex and clones every queued String — both every frame, both discarded by
the `SessionMeta` equality check when nothing changed. `plan_path` allocates a
`String` from a `Cow` per frame.

Idle cost is near zero: empty `Vec`s do not allocate. The cost appears only with
a non-empty queue or rule list. The `draft_mirror` pattern
(`session.rs:151-161`) is the template if it becomes worth doing.

**Status:** open, low.

### 10. `char_to_byte` is O(line) per keystroke — `maki-ui/src/text_buffer.rs:90-94`

`char_indices().nth()` consumes from the start. Called from `push_char`,
`remove_char`, `delete_char`, both word-delete paths and `kill_to_start_of_line`.
`find_prev_word_boundary`/`find_next_word_boundary` additionally allocate a
`Vec<char>` of the whole line per call.

Real, but the input is a prompt line — a few hundred chars at most, at human
typing speed. Sub-microsecond.

**Status:** open, below the noise floor.

---

## Findings that do not hold

### `load_full()` deep-clones the history every frame — **wrong**

Ranked #2 in the consolidated list and described as "a deep clone of the entire
conversation history on every frame for every session" and "the single largest
per-frame cost in the checkpoint path".

`ArcSwap::load_full` (arc-swap 1.9.2, `lib.rs:422-424`) is
`Guard::into_inner(self.load())` — it returns the stored `Arc<HistorySnapshot>`.
`HistorySnapshot { epoch: u64, messages: Arc<Vec<M>> }`
(`maki-storage/src/sessions.rs:139-142`) holds the messages behind a second
`Arc`.

So `session.rs:66` costs **two atomic increments**. Nothing is deep-copied.
The whole point of the type is that the UI can take a snapshot per frame for free.

### Excluding `revision` from the input `RenderKey` — **would reintroduce a fixed bug**

Offered as the "key architectural issue" of the text-buffer audit, on the premise
that `revision` bumps per keystroke and therefore "`render_lines` runs fully every
frame during typing".

Two errors. First, the frequency: a keystroke invalidates the cache *once*, not
once per frame — between keystrokes the key is stable and the cache hits. The
checkpoint audit got this right ("clone of styled-line tree every frame", i.e. the
hit path), and contradicts the text-buffer audit on the same code.

Second, and more seriously: `revision` is what makes the cache correct. Commit
`2df5202e` on this branch fixed a stale-render bug in exactly this cache —
`set_input` rebuilt the buffer, restarting `revision` at zero, so two history
entries of equal length shared a key and recalling the second painted the first.
Removing `revision` from the key generalises that bug to every content change.

### `render_worker` leaks its thread count — **wrong** (already corrected)

Round 2 claimed `active_threads` is never decremented on worker exit. It is, at
`render_worker.rs:130`, and on the spawn-failure path at `:106`. This round
correctly retracts it.

### Per-frame fsync during streaming — **wrong** (already corrected)

Rounds 1 and 2 claimed a streaming turn fsyncs per text-delta frame. It does not:
`content_revision` moves only via `touch()` (message boundaries, tool outputs,
token usage), while `set_meta` uses `touch_soft()` and is throttled by
`SOFT_SAVE_DELAY`. This round correctly retracts it, and its own `History`
finding supplies the reason.

---

## Already addressed on this branch

| Finding | Commit |
|---|---|
| `Paragraph::new` deep-cloning every visible segment per frame — 6.33 ms → 112 µs at 5000 lines | `0aff7023` |
| Streaming block re-measured from scratch every frame | `0aff7023` |
| `update_api_log_symlink` filesystem syscalls every frame; stale symlink on rename | `a433664f` |
| O(n²) turn numbering; `UserSettings::load` per user message | `1707ca8d` |
| Tool inputs deep-copied twice on session load | `1707ca8d` |
| `ToolDoneEvent` cloned whole to feed the UI channel — 115 µs per 5000-line read | `de4dfdde` |
| Two redundant tool-input clones in dispatch | `de4dfdde` |
| Input render cache serving stale text after `set_input` | `2df5202e` |
| Draft re-joined and re-compared every frame | `2df5202e` |

The remaining `Paragraph::new(visible)` clone noted by the render audit is the
intended, bounded behaviour: `WrapIndex::window` restricts it to the lines
covering the viewport. `render.rs` documents this and tests that the windowed
paint is byte-identical to the unwindowed one at every scroll offset.

`WrapIndex::build` for the spacer and collapsed-thinking lines
(`messages/mod.rs:819-820`) does run per frame. Both are one to two lines; the
spacer is constant and could be hoisted. Genuinely trivial.

---

## Priority

Ranked by measured or bounded impact, not by call-graph position.

1. **Selection copy renders whole segments** (`messages/selection.rs:35-70`) —
   12 ms per large segment per copy, measured. Reuses machinery already on the
   branch. Highest measured impact of anything still open.
2. **Lua plugin-events loop** (`runtime.rs:2541`) — the only unconditional
   background cost; 20 wakes/sec forever, at idle. *(Deferred by owner.)*
3. **`JobStore` owner index** (`api/fn.rs:168-243`) — removes three O(n) scans
   and a per-tool-call allocation. Low risk.
4. **`json_to_lua` request cache** (`runtime.rs:1931, 2283, 2389`) — 3× tree walk
   per tool call, scales with input size.
5. **`create_dir_all` hoist** (`storage_writer.rs:157`) — free.
6. **`request_tools` cache** (`run.rs:258`) — needs correct invalidation.
7. **`estimate_input_tokens`** — ≤ 0.4 ms/turn; tidy, not urgent.
8. **`History::publish`** — ~1 ms/turn; not worth threading `Arc<Message>`
   through three crates.

Below the line: `char_to_byte`, `build_meta`'s rule/queue clones, the spacer
`WrapIndex`, `is_animating`, `SessionLog` meta serialization. All real, all
under 0.1 % of their respective budgets.

---

## On the process

Three rounds have now produced findings at roughly this rate: about half hold as
written, a quarter are real with the magnitude or the fix wrong, and a quarter
describe costs that are not there. The failure mode is consistent — reading a
`.clone()` and inferring cost without checking what is behind it (`load_full`),
how often it runs (`build_tools`, `assemble`), or whether it was already done
(`retry.rs` jitter, `render_worker` decrement).

Two rounds also missed the render path entirely, which is where every measured
win on this branch came from. A `.clone()` in a 60 Hz paint loop and a `.clone()`
in a once-per-turn setup path read identically in a grep; they differ by four
orders of magnitude in cost.

The cheapest correction is to attach a frequency and a measurement to each
finding before ranking it.
