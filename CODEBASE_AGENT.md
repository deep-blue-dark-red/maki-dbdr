# Codebase: Agent Loop, Tools, Permissions, Types

## Agent Loop

**`maki-agent/src/agent/run.rs`** — the main agent execution loop.

**Key types:**
- `AgentParams` — immutable config: provider, model, permissions, timeouts, system prompt, etc.
- `AgentRunParams<'h>` — per-run mutable state: history, tools JSON, compaction info
- `Agent<'h>` — state machine with ~20 fields (turn count, usage, cancel token, MCP handle, user response channel, interrupt source)
- `TurnOutcome` — `Continue` or `Done(Option<StopReason>)`

**Builder chain:**
```
Agent::new(AgentParams, AgentRunParams)
  .with_mcp(McpHandle)
  .with_user_response_rx(rx)
  .with_interrupt_source(source)
  .with_cancel(token)
```

**Main entry — `Agent::run(AgentInput)`:**
1. Push user message to history
2. `run_loop()` — `loop { turn() }` until Done or max turns
   - `stream_with_retry()` — call LLM provider, parse SSE events into a `StreamResponse`
   - If `has_tool_calls` → `process_tool_calls()` → loop
   - If no tools → check `max_tokens` stop → continue or Done
   - `try_auto_compact()` — check context size, trigger compaction
   - `handle_queued_command()` — poll `InterruptSource` for cancel/compact/interrupt

**`AgentMode`:** `Build` or `Plan(PathBuf)` — plan mode blocks writes to non-plan files.

**`AgentInput`:** message, mode, images, preamble, thinking config, fast flag, prompt.

**`InterruptSource`**: polled each turn for `ExtractedCommand` — `Interrupt(AgentInput, u64)` or `Compact(u64)`.

### Streaming

**`maki-agent/src/agent/streaming.rs`** — parses SSE events from providers into `StreamResponse`. Accumulates text deltas, thinking blocks, and tool_use blocks.

### Compaction

**`maki-agent/src/agent/compaction.rs`** — `compact()` calls a model with `compaction.md` + `compaction_user.md` prompts. Resets history to fit within context window. Triggered when context size exceeds threshold.

### History

**`maki-agent/src/agent/history.rs`** — `History` manages the message list. Tracks rollback point for undo after compaction. `SharedMessages` — Arc-protected message list.

### Instructions

**`maki-agent/src/agent/instructions.rs`** — loads `AGENTS.md`, `CLAUDE.md`, `.mcli/`, `.mcli/`, `.mcli/` etc. `load_instructions()` finds instruction files in directory tree. `find_subdirectory_instructions()` locates per-dir instructions. `build_system_prompt()` assembles the final system prompt from `prompts/*.md` + loaded instructions.

## Permission System

**`maki-agent/src/permissions.rs`**

`PermissionManager` checks allowed/denied/prompt before tool execution.

**Three-layer rule resolution** (session → config → builtin):
1. **Session rules** — added at runtime via user decisions (Mutex)
2. **Config rules** — from `permissions.toml`
3. **Builtins** — write/edit/multiedit within cwd, task always allowed

**Check order:** Deny rules checked before allow rules.

**Rules:** `(tool: &str, scope: Option<&str>, effect: Allow|Deny)`

**Scope matching:**
- `*` → match everything
- `**` → match everything
- `<path>/**` → match path prefix (component-aware, symlink-safe)
- `<cmd> *` → match command prefix with args
- `<prefix>*` → match string prefix
- exact string equality

`physical_boundary_check(parent, child)` — follows symlinks to verify child is inside parent. Prevents symlink-based boundary escapes.

**Permission answers:** `AllowOnce`, `AllowSession`, `AllowAlwaysLocal`, `AllowAlwaysGlobal`, `Deny`, `DenyWithGuidance`, `DenyAlwaysLocal`, `DenyAlwaysGlobal`.

**Scope generalization** (for "Allow always" decisions):
- `bash "cargo test"` → `bash "cargo *"`
- `write "/project/src/main.rs"` → `write "/project/src/**"`
- `mcp:fetch` args → `mcp:fetch *`
- Denies stay exact (no generalization)

**Async enforcement:** `enforce()` sends `PermissionRequest` event to UI, waits on receiver for answer. `yolo` bypasses prompts but NOT deny rules.

## Types

**`maki-agent/src/types.rs`**

**`ToolOutput`** — enum of all possible tool output shapes:
- `Plain(TextOutput)` — plain text
- `Markdown(TextOutput)` — markdown text
- `ReadCode` — path, start_line, lines, total_lines, instructions
- `ReadDir(TextOutput)` — directory listing
- `Diff` — path, before, after, summary
- `TodoList(Vec<TodoItem>)` — todo items with status/priority
- `WriteCode` — path, byte_count, lines (legacy)
- `GrepResult` — entries with matched context lines
- `Batch` — entries with status tracking per tool
- `Instructions` — instruction blocks appended to tool output

**`TextOutput`** — `{text: String, instructions: Option<Vec<InstructionBlock>>}`. Tools can append instruction blocks (e.g. AGENTS.md content) to their output.

**`ToolStartEvent`** — emitted when a tool call begins. Contains id, tool name, summary, render_header, input, raw_input, output (pre-computed).

**`ToolDoneEvent`** — emitted when a tool call ends. Contains id, tool name, output, is_error, annotation, written_path.

**`AgentEvent`** — enum of all events from agent to UI:
- `TextDelta { text }` / `ThinkingDelta { text }`
- `ToolStart(Box<ToolStartEvent>)` / `ToolDone(Box<ToolDoneEvent>)`
- `ToolOutput { id, content }` — full accumulated output (NOT a delta)
- `BatchProgress(Box<BatchProgressEvent>)`
- `TurnComplete(Box<TurnCompleteEvent>)`
- `ToolResultsSubmitted { message }`
- `QueueItemConsumed { text, image_count }`
- `Done { usage, num_turns, stop_reason }`
- `AutoCompacting`
- `Retry { attempt, message, delay_ms }`
- `SystemPrompt { text }`
- `Error { message }`
- `PermissionRequest { id, tool, scopes }`
- `AuthRequired`
- `SubagentHistory { tool_use_id, messages }`
- `ToolSnapshot { id, snapshot, theme_gen }` — pre-rendered snapshot for theme switching
- `ToolHeaderSnapshot { id, snapshot, theme_gen }`
- `LiveToolBuf { id, body: Arc<SharedBuf> }` — streaming tool output

**`Envelope`** — wraps `AgentEvent` + `Option<SubagentInfo>` + `run_id: u64`.

**`SharedBuf`** — append-only buffer for streaming tool output. Writers append under a Mutex, readers get a cheap Arc clone via `read_if_dirty()` (dirty flag + atomic swap). Poisoned mutex recovery.

**`BufferSnapshot`** — `Arc<Vec<SnapshotLine>>` with spans and styles. `SnapshotSpan` — text + `SpanStyle` (Default, Named, Inline with fg/bg/bold/italic/underline/dim/strikethrough/reversed).

**`EventSender`** — wraps `Sender<Envelope>` + `run_id`. `send()` / `try_send()` to channel.

**`TodoItem`** — `{content, status: TodoStatus, priority: TodoPriority}`.

## Tool Definition System

**`maki-tool-macro/src/lib.rs`** — `#[derive(Tool)]` proc macro.

Generates for each tool struct:
- `SCHEMA: &'static ParamSchema` — from struct fields, reading `#[param(description)]` and `#[param(alias)]` attributes. `Option<T>` or `#[serde(default)]` marks params as optional.
- `parse_input(input_json)` — sanitizes LLM JSON (strips stray quotes, converts camelCase keys) then validates against schema before serde deserialization.

Supporting derives: `#[derive(Args)]` for sub-objects, `#[derive(ArgEnum)]` for enum parameters.

**`maki-agent/src/tools/schema.rs`** — `ParamSchema` enum (Primitive, Enum, Array, Object, Any). Generates both JSON Schema (sent to LLM) and `validate()` (input validation). Single source of truth prevents schema drift.

Validation: type checking with coercions, `jsonrepair` fallback for malformed JSON, structured `ToolInputError` with JSON paths.

**`maki-agent/src/tools/registry.rs`** — `ToolRegistry`.

- Uses `ArcSwap` for lock-free reads, atomic swaps on writes
- Sources: `Native`, `Mcp { server }`, `Lua { plugin }`
- `Tool` trait: `name()`, `description()`, `schema()`, `examples()`, `audience()`, `parse()` → `ToolInvocation`
- Lua plugins can **replace** native tools (original saved as `native_fallbacks`)
- `register_many()` — all-or-nothing for MCP server registration
- Runtime lifecycle: `clear_mcp_server()`, `replace_plugin()`, `clear_plugin()`

`register_tools!` macro in `mod.rs` does compile-time dedup checking, generates `native_tools()` and `NATIVE_TOOL_NAMES`.

**`maki-agent/src/agent/tool_dispatch.rs`** — tool call execution pipeline.

`run()` (single tool call):
1. Look up in `ToolRegistry`
2. Parse & validate input
3. Plan mode: block writes to non-plan files
4. Send `ToolStart` event (header from `start_header()`)
5. `PermissionManager::enforce()` — permission check/prompt
6. `ToolInvocation::execute()` — async execution
7. Return `ToolDoneEvent` with output, timing, written_path

MCP fallback: if not in registry, check MCP handle. MCP tools skip native parsing/permissions.

`process_tool_calls()` — batch of tool calls from one LLM response:
- **Doom loop detection:** tracks last N calls by input hash. Rejects 3+ identical consecutive calls.
- Runs non-looped calls in parallel via `TaskSet`
- Sends events, collects results, builds tool results message via `tool_results()`
