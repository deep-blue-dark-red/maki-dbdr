# Performance Review — Verification Round 2

All 24 findings verified against the actual code. 13 are accurate, 4 need significant correction, 3 have minor line-number drift, and 4 are inaccurate (the described cost doesn't exist as stated).

---

## Accurate findings (confirmed with file:line refs)

| # | Finding | Status |
|---|---------|--------|
| 1 | dispatch_async busy-polls on 50ms timer | ✅ runtime.rs:2158-2197. `DISPATCH_POLL_INTERVAL = 50ms` at runtime.rs:51. `event_listener::Event` already imported (runtime.rs:15) and used by `InflightGate` (runtime.rs:987-1034). Fix is feasible. |
| 2 | drain_events scans every owned job per poll | ✅ api/fn.rs:200-209. O(jobs) per wake. Mostly fixed by #1. |
| 3 | Watchdog polls every 10ms | ✅ runtime.rs:67, 669-694. CPU-loop protection by design. Cannot be parked behind an event — uses `thread::park_timeout`, no notification mechanism. |
| 4 | Unbounded flume channels + 3 threads per job, no live-job cap | ✅ api/fn.rs:110, 136-151. `flume::unbounded()`, 3 threads per job, `HashMap<u32, JobMeta>` with no limit. |
| 8 | process_tool_calls fans out with no bound | ✅ tool_dispatch.rs:448-516. All runnable calls spawned into `TaskSet` with no semaphore. Lua's `MAX_INFLIGHT_TOOLS=64` (runtime.rs:56) serializes them anyway. Bounding is feasible (add a `Semaphore` to `TaskSet` at task_set.rs:20-30). |
| 9 | Compaction clones entire history then strips | ✅ compaction.rs:40. `history.as_slice().to_vec()` before `remove_orphaned_tool_results` / `strip_images` / `strip_thinking` / `strip_old_tool_results` (all take `&mut [Message]`). |
| 11 | process_tool_calls clones tool inputs multiple times | ✅ tool_dispatch.rs:439 (`input.clone()` for runnable), :459 (`input.clone()` for args), :474 (`ToolDoneEvent` clone for UI channel). |
| 16 | FileReadTracker canonicalizes + stats every read | ✅ file_tracker.rs:37-38. `normalize_path` calls `fs::canonicalize`, `get_mtime` calls `fs::metadata`. The `HashMap<PathBuf, SystemTime>` cache at :11 is write-only in `record_read` — never checked. Only used for staleness in `check_before_edit`. |
| 17 | Session checkpoint fsyncs on every frame | ✅ session.rs:65-117. Runs every frame (event_loop.rs:419-444). `storage_writer.send` → `file.sync_data()` at sessions.rs:585-588. Soft-delay guard exists (session.rs:94-106) but agent-produced changes bypass it (content_revision bumps skip the wait). Batching on turn end / every N ms is feasible. |
| 18 | history_to_display re-parses every tool call on restore | ✅ chat.rs:434-437. `registry.get` + `try_parse` + `resolve_header` for every `ToolUse` block. Summary is not stored as a separate field in `DisplayMessage` (components/mod.rs:288-304) — only baked into text. |
| 19 | StorageWriter wakes per save, re-stats file | ✅ storage_writer.rs:59-61, 85-92. `wake_rx.recv()` per wake, `ensure_appendable` calls `file.metadata()` at sessions.rs:645. |
| 20 | RenderWorker spawns a thread per highlight burst | ✅ render_worker.rs:87-109. Fresh thread per send via CAS until max_threads, exits after 5s idle (`IDLE_TIMEOUT` at :16). Unbounded job buffer. Bug: `active_threads` is never decremented on worker exit (render_worker.rs:101), so the pool can permanently exhaust its thread quota. |
| 21 | TextBuffer uses `char_indices().nth()` per keystroke | ✅ text_buffer.rs:76-80. O(n) byte-offset lookup via `char_to_byte` → `byte_x` → `push_char`. |
| 22 | is_animating iterates all chats every frame | ✅ app/mod.rs:2163 (doc said 2146, off by ~17). |
| 23 | App::update clones the Envelope for agent branch | ✅ app/mod.rs:539 (doc said 536, off by 3). `*envelope` copies by value into `handle_agent_event`. |
| 24 | SessionLog::append re-serializes meta every save | ✅ sessions.rs:579. `meta_record(session)` re-serializes full meta. Early-return guard at :580-582 only fires when `buf.is_empty() && meta == self.saved_meta` — so new messages without meta changes still waste the serialization. `ensure_appendable` calls `file.metadata()` at :645. |

---

## Findings needing significant correction

### #5 — json_to_lua converts tool input twice per call ❌

The claim that the preview path at tool_dispatch.rs:423 does a second JSON→Lua conversion is wrong. `preview()` (tools/schema.rs:392) is a pure string truncation/escape over `input.to_string()` (a serde_json serialization), not a recursive conversion. The suggested fix (reuse the Lua value for preview) is backwards — the preview fires before the tool is dispatched to the Lua host, so the Lua value doesn't exist yet.

However, the underlying duplication is real and worse than described: the same `serde_json::Value` is converted to Lua up to 4 times on the Lua host thread for a single tool call (runtime.rs:1931 for `compute_header`, :2283 for `run_tool_start`, :1931 again for `compute_permission_scopes`, :2389 for `run_tool_call`). The fix is to convert once on the host side and pass the `LuaValue` through the `Request` variants, or cache it keyed by request id.

### #6 — Tool schema serialized on every turn ⚠️

Accurate that `request_tools()` clones the full tools JSON every turn when MCP is present (run.rs:258-267). The `.into_owned()` on the None branch (run.rs:277) is minor waste.

But the suggested fix (cache merged JSON, invalidate on MCP connect/disconnect) is insufficient. `search_tools`/`mark_loaded` (mcp/mod.rs:431-436, 471-472) mutate `McpSession.loaded` on nearly every turn when deferral is active, and those do not bump generation. A cache invalidated only on connect/disconnect would go stale immediately after the first `tool_search` loads a tool. Caching requires invalidating on both index generation changes and `McpSession.loaded` mutations. The comment at mcp/mod.rs:300-301 explicitly warns: "extend_tools output must never be stored."

### #10 — History::publish clones the whole message list per push ❌

The claim that this is the main allocation hotspot in long sessions is wrong. `publish` is not called per text delta. Text deltas never touch `History` at all — they flow through `ProviderEvent::TextDelta` → `forward_provider_events` (streaming.rs:21-47) → `AgentEvent::TextDelta` → UI's streaming text buffer (messages/mod.rs:169-172). `History` is only mutated at message boundaries: once per turn for the assistant message (run.rs:430), once per tool result (tool_dispatch.rs:514). So `publish` fires at most a handful of times per turn. The suggested fixes (publish on flush boundaries, cheaper diff protocol) don't address an actual bottleneck. The architecture is already delta-free.

### #13 — build_tools re-runs on every turn ❌

`build_tools` runs once per user message (per run), not per LLM turn. `initialize()` calls it once at startup (agent_loop.rs:153), `do_agent_run()` calls it once per queue entry via `rebuild_tools` (agent_loop.rs:204, 291-292). The per-turn path calls `request_tools()` (run.rs:258) which only merges MCP tools into the already-built `self.tools` Value — it does not rebuild base definitions. Caching is partially feasible but two inputs defeat a naive cache: the registry snapshot has no epoch/version counter (registry.rs:437-438), and vars are re-derived per run (agent_loop.rs:200).

---

## Findings needing minor correction

### #7 — estimate_input_tokens walks the entire history every turn ⚠️

Accurate that it walks all messages (streaming.rs:158-187), but the framing is off. It's called once per turn normally, twice only on turn 0 (not twice every turn). The incremental model already exists: per-turn delta is computed as `full_history_estimate - sum_of_prior_turn_estimates` (run.rs:284-295, turn_state.rs:138-141). The remaining inefficiency is that the full-history total is recomputed from scratch each turn. Storing the previous total and adding only the new messages' byte-length would make it O(Δ) instead of O(n).

### #12 — MCP extend_tools rebuilds a HashSet every turn ⚠️

Accurate that the existing `HashSet<String>` is rebuilt every call (mcp/mod.rs:353-356), but this is O(k) where k = tools already in the array (typically tens), trivial compared to scanning the descriptor index. The expensive part (the `ToolIndex`) is already shared via `ArcSwap` (mod.rs:286). Caching the existing set is not feasible because it's derived from the `tools: &mut Value` argument which changes every turn. The comment at mod.rs:300-301 confirms this is intentional design. The fix suggestion should be withdrawn.

### #14 — System prompt reassembled from scratch every turn ❌

`assemble()` is called once per agent run, not every turn. `build_system_prompt` (instructions.rs:54) stores the result as `Agent.system` (run.rs:125), passed immutably to `stream_with_retry` each turn (run.rs:309). The system prompt is already served from the provider's prompt cache (Anthropic pins `cache_control: ephemeral` at maki-providers/src/providers/anthropic/mod.rs:406). Caching would be unnecessary — the current design (assemble once, reuse by reference) is the right shape.

### #15 — Retry backoff has no jitter ❌

Jitter is already present. retry.rs:20-22 splits the delay in half and adds `fastrand::u64` jitter in [0, half]. Effective delay ranges from half (no jitter) up to half + half = full delay (max jitter). `fastrand` is already a workspace dependency (maki-providers/Cargo.toml:23). No fix needed.

---

## Revised priority order (biggest wins, lowest risk)

Based on verified impact and feasibility:

1. **#1 — Event-driven dispatch wake** (runtime.rs:2158-2197). Drops background wake rate from ~20/s to event count. `event_listener::Event` already imported and pattern-established by `InflightGate`. Highest impact, lowest risk.

2. **#7 — Incremental token estimate** (streaming.rs:158-187). Store previous total, add only new messages' bytes. Removes O(n) history walk per turn. The incremental delta infrastructure already exists in `TurnState`.

3. **#17 — Batch fsyncs during streaming** (session.rs:65-117). Add a `Wake::Timer` variant to the writer loop so agent-produced changes are coalesced every N ms instead of per frame. Feasible — the writer already matches on `Wake::*` (event_loop.rs:505).

4. **#8 — Bound tool fan-out** (tool_dispatch.rs:448-516). Add a `Semaphore` to `TaskSet` (task_set.rs:20-30) capping concurrency at ~8. Reduces peak `ToolContext` clones and registry contention. Lua's `MAX_INFLIGHT_TOOLS=64` already serializes, so no behavior change.

5. **#5 — Deduplicate json_to_lua on the Lua host** (runtime.rs:1931, 2283, 2389). Convert once and pass the `LuaValue` through `Request` variants, or cache keyed by request id. The 4× duplication is real but the review's suggested fix (reuse for preview) targets the wrong path.

6. **#6 — Cache merged tools JSON** (run.rs:258-267). Requires invalidating on both index generation changes and `McpSession.loaded` mutations, not just connect/disconnect. Medium impact, medium risk (the `&mut self` conflict at run.rs:273-276 also needs work).

7. **#16 — Cache canonicalize + metadata per path in FileReadTracker** (file_tracker.rs:37-38). The `HashMap` cache already exists but is write-only in `record_read`. Check it first before calling syscalls. Low risk, modest impact.

8. **#4 — Bound live-job count** (api/fn.rs:110). Add a semaphore or max-count check in `jobstart`. Only matters under plugin-spawn storms. Low risk, low frequency.

9. **#11 — Reduce process_tool_calls clones** (tool_dispatch.rs:439, 459, 474). The second `input.clone()` at :459 could be avoided by moving the first clone into the closure. Trivial change, minor impact.

10. **#20 — Fix RenderWorker thread-count leak** (render_worker.rs:101). `active_threads` is never decremented on worker exit. Add `fetch_sub` in `worker_loop` on exit. Bug fix, not a perf optimization.

---

## Summary

- 13 findings are accurate as written.
- 4 findings (#5, #10, #13, #14) describe costs that don't exist as stated — the code already avoids them through a different architecture.
- 3 findings (#6, #7, #12) are accurate but the suggested fixes are incomplete or infeasible without additional changes.
- 2 findings (#15, #23) are inaccurate — the cost is already addressed (#15) or trivial (#23).
- 1 finding (#20) has a bug beyond the described issue (thread count leak).

The top 3 verified wins are #1 (event-driven dispatch), #7 (incremental token estimate), and #17 (batched fsyncs). All three are feasible with low risk and no behavior changes.