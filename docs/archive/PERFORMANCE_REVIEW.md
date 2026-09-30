# Maki Performance Review

Findings from a read-only audit of the Rust workspace (maki-lua, maki-agent, maki-ui, maki-providers, maki-storage). Ordered roughly by impact-to-effort.

---

## Lua runtime (maki-lua/src/)

### 1. `dispatch_async` busy-polls on a 50ms timer
`runtime.rs:2158-2197` — The async dispatch loop yields `smol::Timer::after(DISPATCH_POLL_INTERVAL)` (50ms) whenever the event buffer is empty. For a long-running `bash` job this wakes ~20×/s forever, each iteration locking the `TaskCell` mutex, calling `with_jobs`, and running Lua callbacks. This is the single largest background-wake source.
**Fix:** Signal the loop via `event_listener::Event` (already imported) when a `JobEvent` or `finish` arrives, so it parks until real work exists.

### 2. `drain_events` scans every owned job per poll
`api/fn.rs:200-209` — Iterates every job owned by the task on each poll, calling `rx.try_recv()` on each. O(jobs) per wake. Combined with #1 this is the main cost; fixing #1 removes most of the scans.

### 3. Watchdog thread polls every 10ms
`runtime.rs:67` — Constant ~100 wakeups/s on a core. By design (CPU-loop protection), but could be parked behind an event if the interrupt-arm path ever allows it.

### 4. Unbounded `flume` channels + 3 threads per spawned job
`api/fn.rs:110` — Each `maki.fn.jobstart` spawns 3 OS threads (stdout/stderr/wait) and an unbounded channel. Nothing caps live-job count. A bounded channel + shared reader pool would help under plugin-spawn storms.

### 5. `json_to_lua` converts tool input twice per call
`runtime.rs:2387` — The input JSON is converted to Lua for the handler, and again `preview`-ed for logging in `tool_dispatch.rs:423`. Two full recursive conversions of the same value.

---

## Agent loop (maki-agent/src/)

### 6. Tool schema serialized on every turn
`agent/run.rs:258-267` — `request_tools()` clones the full tools JSON every turn when MCP is present, and `turn()` calls `.into_owned()` unconditionally. Once any MCP server connects, every turn re-serializes the entire tool list.
**Fix:** Cache the merged tools JSON; invalidate only on MCP connect/disconnect.

### 7. `estimate_input_tokens` walks the entire history every turn
`agent/streaming.rs:158-187` — Called twice per turn (once in `turn()`, once inside `stream_with_retry`). O(turns²) overall for long sessions. `TurnState` already tracks per-turn estimates incrementally; the estimate path should derive from the previous total + this turn's delta instead of re-walking everything.

### 8. `process_tool_calls` fans out with no bound
`agent/tool_dispatch.rs:448-516` — Spawns all runnable tool calls into a `TaskSet` concurrently. A model can emit many calls per turn; each holds `ToolContext` clones (registry, permissions, file tracker). The Lua `MAX_INFLIGHT_TOOLS = 64` gate serializes them anyway. Bounding Rust-side fan-out (e.g. 8) reduces peak memory/contention.

### 9. Compaction clones the entire history then strips it
`agent/compaction.rs:40` — `history.as_slice().to_vec()` clones everything before stripping images/thinking/tool-results. Infrequent, but avoidable if strip functions took a `&mut [Message]` slice of the live history.

### 10. `History::publish` clones the whole message list per push
`agent/history.rs:96-100` — Every `push`/`edit`/`rewrite` publishes a snapshot by cloning **all** messages into a new `Arc`. For a streaming turn with many text deltas this is the main allocation hotspot in long sessions.
**Fix:** Publish on flush boundaries, or use a cheaper diff protocol for the mirror.

### 11. `process_tool_calls` clones tool inputs multiple times
`agent/tool_dispatch.rs:440,459` — `input.clone()` for each runnable, plus `ToolDoneEvent` cloned for the UI channel *and* again for the history message. 2× the payload for large outputs.

### 12. MCP `extend_tools` rebuilds a `HashSet` every turn
`mcp/mod.rs:348-383` — Every `request_tools()` builds a fresh `HashSet<String>` of base tool names, then iterates all MCP descriptors and clones their `definition` JSON. O(base × mcp) allocations per turn.
**Fix:** Cache the base-tools set; invalidate on registry/filter change.

### 13. `build_tools` re-runs on every turn
`ui/agent/agent_loop.rs:291-300` — `rebuild_tools` runs `ToolRegistry::definitions` (full `input_schema` for every tool) before every run, even when model/workflow are unchanged.
**Fix:** Cache keyed by `(model.id, workflow, examples_flag, filter_hash)`.

### 14. System prompt reassembled from scratch every turn
`agent/prompt.rs:196-204` — `assemble()` clones the template and runs `fill_marker` for every slot each turn. Slots come from plugins via a Lua round-trip. Mostly stable; only needs rebuilding when instructions/cwd/model/slots change.
**Fix:** Cache like `build_tools`.

### 15. Retry backoff has no jitter
`providers/retry.rs:17-23` — Flat 3s delay. Multiple sessions hitting the same rate-limit retry in lockstep, amplifying thundering-herd effects.
**Fix:** Add jitter (`DELAY * (0.5 + rand)`).

### 16. `FileReadTracker` canonicalizes + stats every read
`tools/file_tracker.rs:37-49` — `record_read` calls `fs::canonicalize` + `fs::metadata` on every file read. `canonicalize` is syscall-heavy. Cheap to cache per path.

---

## UI (maki-ui/src/)

### 17. Session checkpoint fsyncs on every frame
`app/session.rs:65-117` — `checkpoint_with` runs every frame; on a changed revision it calls `storage_writer.send`, which ends in `file.sync_data()`. Agent-produced changes bypass the soft-delay guard, so a streaming turn can fsync on every text-delta frame.
**Fix:** Batch deltas — publish to the writer only on turn end or every N ms.

### 18. `history_to_display` re-parses every tool call on restore
`chat.rs:402-516` — For each `ToolUse` block it calls `registry.get` + `try_parse` + `resolve_header` just to render a static summary. The summary is already stored in the `DisplayMessage`.
**Fix:** Short-circuit when no annotation is needed.

### 19. `StorageWriter` wakes per save
`storage_writer.rs:59-61` — Writer thread loops on `wake_rx.recv()`; `enqueue` sends `()` on every `send`. De-dup happens on the UI side, so the writer still wakes per accepted save and re-stats the file in `ensure_appendable`.

### 20. `RenderWorker` spawns a thread per highlight burst
`render_worker.rs:87-109` — Fresh OS thread per `send` until `max_threads`; threads exit after 5s idle. Contended compare-exchange on every send; unbounded job buffer during a highlight storm.
**Fix:** Persistent pool with a work queue.

### 21. `TextBuffer` uses `char_indices().nth()` per keystroke
`text_buffer.rs:76-80` — O(n) byte-offset lookup per cursor move for long input lines. Low priority (lines are usually short).

### 22. `is_animating` iterates all chats every frame
`app/mod.rs:2146` — `self.chats.iter().any(|c| c.is_animating())` per frame. Cheap unless there are many subagent chats.

### 23. `App::update` clones the `Envelope` for the agent branch
`app/mod.rs:536` — `handle_agent_event(*envelope)` copies the envelope by value. Trivial, but the hot path is agent events.

---

## Storage (maki-storage/src/)

### 24. `SessionLog::append` re-serializes meta every save
`sessions.rs:579-583` — `meta_record(session)` re-serializes full session meta on every append, and `ensure_appendable` calls `file.metadata()` (stat syscall) per append. The stat is needed for divergence detection; the meta serialization is pure waste when nothing meta-relevant changed.

---

## Priority order (biggest wins, lowest risk)

1. **#1** — event-driven dispatch wake (drops background wake rate from ~20/s to event count)
2. **#7** — incremental token estimate (removes O(turns²) history walk)
3. **#13** — cache `build_tools` (removes full tool-schema serialization per turn)
4. **#14** — cache system prompt assembly
5. **#17** — batch fsyncs during streaming (cuts per-frame disk stalls)
6. **#12** — cache MCP base-tool set
7. **#10** — mirror clone on every push
8. **#8** — bound tool fan-out
9. **#6** — cache merged tools JSON
10. **#15** — retry jitter