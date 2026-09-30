# Agents Guide — Qwen Edition

Maki is an AI coding agent (like Claude Code and opencode) built in Rust to optimize costs and token usage without sacrificing performance.

## Overview

- **Language:** Rust 2024 edition, rust-version 1.88+
- **Total source:** ~92k lines of Rust (14 crates) + ~18k lines of Lua (built-in plugins)
- **Runtime:** smol (async), single-threaded UI, separate threads for agent loop and Lua VM
- **Entry point:** `src/main.rs` → `cli::Cli` → `cmd::dispatch()`

## Workspace Structure

```
/Users/mcp/git/maki/
├── Cargo.toml              ← workspace root + dependencies
├── src/                    ← binary crate (maki binary)
│   ├── main.rs             ← entry: install color_eyre, parse CLI, dispatch
│   ├── cli.rs              ← clap definitions, subcommands, flag handling
│   ├── cmd/mod.rs          ← command router
│   ├── cmd/tui.rs          ← TUI/SDK/print bootstrap
│   ├── cmd/acp.rs          ← ACP server bootstrap
│   ├── cmd/subcmd.rs       ← subcommand implementations
│   ├── cmd/migrate.rs      ← data migrations
│   ├── sdk_mode.rs         ← Claude Code SDK wire protocol compatible mode
│   ├── print.rs            ← non-interactive print mode
│   ├── setup.rs            ← model resolution, logging init
│   └── update.rs           ← self-update logic
│
├── maki-agent/             ← core agent loop
│   └── src/
│       ├── lib.rs           ← re-exports
│       ├── agent/           ← agent execution
│       │   ├── mod.rs       ← submodule re-exports
│       │   ├── run.rs       ← Agent, AgentParams, run_loop()
│       │   ├── streaming.rs ← SSE event parsing from providers
│       │   ├── compaction.rs ← context compaction when hitting limits
│       │   ├── history.rs   ← message history management
│       │   ├── instructions.rs ← AGENTS.md loading, prompt assembly
│       │   └── tool_dispatch.rs ← tool call execution pipeline
│       ├── tools/           ← tool system
│       │   ├── mod.rs       ← ToolFilter, Deadline, register_tools!
│       │   ├── registry.rs  ← ToolRegistry (ArcSwap, native/MCP/Lua sources)
│       │   ├── schema.rs    ← ParamSchema (JSON schema single source of truth)
│       │   └── [tool].rs    ← individual tool implementations
│       ├── mcp/             ← MCP support
│       │   ├── mod.rs       ← start(), McpHandle
│       │   ├── config.rs    ← MCP config file parsing
│       │   ├── protocol.rs  ← JSON-RPC protocol
│       │   ├── stdio.rs     ← stdio transport
│       │   ├── http.rs      ← SSE HTTP transport
│       │   ├── transport.rs ← unified transport
│       │   └── oauth/       ← MCP OAuth flow
│       ├── prompts/         ← embedded MD prompts
│       │   ├── system.md    ← system prompt
│       │   ├── general.md   ← general task instructions
│       │   ├── plan.md      ← plan mode
│       │   ├── research.md  ← research mode
│       │   ├── compaction.md ← compaction user prompt
│       │   └── compaction_user.md ← compaction assistant prompt
│       ├── permissions.rs   ← PermissionManager, scope matching, yolo
│       ├── headless.rs      ← headless agent spawning for SDK
│       ├── task_set.rs      ← parallel task execution
│       ├── types.rs         ← AgentEvent, ToolOutput, Envelope, SharedBuf
│       ├── prompt.rs        ← ResolvedSlots, prompt rendering
│       ├── cancel.rs        ← cancel tokens, cancel map
│       ├── child_guard.rs   ← child process management
│       └── diff.rs          ← diff generation utilities
│
├── maki-ui/                ← terminal UI (ratatui, Elm-like)
│   └── src/
│       ├── lib.rs           ← run(EventLoopParams)
│       ├── app/mod.rs       ← App, Msg, update(), overlay routing
│       ├── app/tests.rs     ← 2611-lines of UI tests
│       ├── update.rs        ← update side effects, Action dispatch
│       ├── components/      ← UI components
│       │   ├── messages/mod.rs ← chat messages rendering
│       │   ├── messages/render.rs
│       │   ├── messages/segment.rs
│       │   ├── messages/selection.rs
│       │   ├── messages/tests.rs
│       │   ├── tool_display.rs ← tool output rendering (2024 lines)
│       │   ├── input.rs     ← input box component
│       │   ├── lua_float.rs ← Lua popup rendering (1939 lines)
│       │   ├── command.rs   ← command palette
│       │   ├── help_modal.rs
│       │   ├── search_modal.rs
│       │   ├── permission_prompt.rs
│       │   ├── split_layout.rs
│       │   └── ... (pickers, modals, status bar)
│       ├── agent/           ← UI-side agent coordination
│       │   ├── agent_loop.rs ← QueueReceiver → Agent.run()
│       │   ├── command_router.rs
│       │   ├── cancel_map.rs
│       │   └── shared_queue.rs
│       ├── event_loop.rs    ← main loop: tick → drain → draw
│       ├── terminal.rs      ← crossterm setup
│       ├── render_worker.rs ← background render tasks
│       ├── markdown.rs      ← markdown rendering in UI
│       ├── highlight.rs     ← syntax highlighting
│       ├── selection.rs     ← text selection
│       ├── themes/          ← 22 theme TOML files
│       └── ... (clipboard, image, splash, etc.)
│
├── maki-providers/         ← LLM provider integrations
│   └── src/
│       ├── lib.rs           ← public API re-exports
│       ├── model.rs         ← Model (prefix lookup, pricing, capabilities)
│       ├── provider.rs      ← Provider trait, ProviderKind (12 providers)
│       ├── providers/       ← per-provider implementations
│       │   ├── mod.rs       ← shared HTTP, KeyPool, SSE parsing
│       │   ├── anthropic/   ← Anthropic API (including Bedrock)
│       │   ├── openai/      ← OpenAI API
│       │   ├── google.rs    ← Google Gemini
│       │   ├── copilot/     ← GitHub Copilot
│       │   ├── mistral.rs   ← Mistral
│       │   ├── zai/         ← Z.AI
│       │   ├── deepseek.rs  ← DeepSeek
│       │   ├── ollama.rs    ← Ollama
│       │   ├── llama_cpp.rs ← llama.cpp
│       │   ├── openrouter.rs
│       │   ├── openai_compat.rs
│       │   ├── dynamic.rs   ← user-configured custom providers
│       │   ├── custom.rs    ← providers.toml entries
│       │   ├── local.rs     ← local providers (Ollama, LlamaCpp)
│       │   └── synthetic.rs ← test/fake provider
│       ├── types.rs         ← Message, ContentBlock, ProviderEvent
│       ├── error.rs         ← AgentError
│       └── retry.rs         ← exponential backoff
│
├── maki-storage/           ← persistent state
│   └── src/
│       ├── lib.rs           ← StateDir, SessionLog, Session, SessionMeta
│       ├── paths.rs         ← XDG dir resolution, fallback to ~/.maki
│       ├── sessions.rs      ← JSONL sessions, O(delta) saves, CWD index
│       ├── auth.rs          ← OAuth, MCP auth, API key storage (0600 perms)
│       ├── log.rs           ← rotating log files (zstd compressed)
│       ├── model.rs         ← last-used model persistence
│       ├── theme.rs         ← theme selection
│       ├── plans.rs         ← plan mode persistence
│       ├── input_history.rs ← readline-style history
│       └── words/          ← adjective/noun lists for session names
│
├── maki-config/            ← config system
│   └── src/
│       ├── lib.rs           ← Config, UiConfig, AgentConfig, etc.
│       └── providers.rs     ← providers.toml model resolution
│
├── maki-config-macro/      ← #[derive(ConfigSection)] proc macro
│   └── src/lib.rs           ← generates FIELDS, validate(), Default
│
├── maki-tool-macro/        ← #[derive(Tool)] proc macro
│   └── src/lib.rs           ← generates SCHEMA, parse_input() from struct
│
├── maki-lua/               ← Lua plugin system (Luau via mlua)
│   └── src/
│       ├── lib.rs
│       ├── runtime.rs       ← Lua thread, PluginHost, event loop (512MB limit)
│       ├── loader.rs        ← builtin loading, user plugin loading
│       ├── error.rs
│       ├── language.rs
│       ├── plugin_permissions.rs
│       └── api/             ← API surface for plugins
│           ├── mod.rs       ← maki global table: api, env, fs, log, etc.
│           ├── tool.rs      ← register_tool(), register_command()
│           ├── fs.rs        ← filesystem ops
│           ├── env.rs       ← environment variables
│           ├── net.rs       ← HTTP requests
│           ├── uv.rs        ← process/job spawning
│           ├── treesitter/  ← tree-sitter parsing
│           ├── ui/          ← UI buffers, clicks, floats
│           ├── async.rs     ← async.run() background coroutines
│           ├── agent.rs     ← agent state
│           ├── autocmd.rs   ← event-driven callbacks
│           ├── keymap.rs    ← keybinding registration
│           ├── json.rs, yaml.rs, text.rs, fn.rs, log.rs
│           └── util/        ← convert, ctx, command, setup
│
├── maki-interpreter/       ← code_execution tool (Python sandbox)
│   └── src/
│       ├── lib.rs           ← runner via monty (Pydantic sandbox)
│       ├── runner.rs
│       ├── convert.rs
│       └── error.rs
│
├── maki-acp/               ← ACP (Agent Client Protocol) stdio server
│   └── src/
│       ├── lib.rs
│       ├── server.rs
│       ├── methods.rs
│       ├── permissions.rs
│       └── translate.rs
│
├── maki-highlight/         ← syntax highlighting (syntect-based)
│   └── src/lib.rs
│
├── maki-markdown/          ← markdown rendering
│   └── src/
│       ├── lib.rs
│       └── render.rs
│
├── maki-docgen/            ← documentation generator
│   └── src/
│       ├── main.rs
│       ├── gen_tools.rs     ← generates tool docs
│       ├── gen_providers.rs
│       ├── gen_config.rs
│       ├── gen_keybindings.rs
│       ├── gen_commands.rs
│       └── lua_util.rs
│
├── plugins/                ← built-in Lua plugins (embedded via include_dir!)
│   ├── lib/                ← shared Lua modules
│   ├── bash/init.lua       ← bash tool
│   ├── edit/init.lua       ← edit tool
│   ├── glob/init.lua       ← glob tool
│   ├── grep/init.lua       ← grep tool
│   ├── index/init.lua      ← index tool (tree-sitter file skeletons)
│   ├── memory/init.lua     ← memory tool (project scratchpad)
│   ├── question/init.lua   ← question tool (user clarification)
│   ├── read/init.lua       ← read tool
│   ├── write/init.lua      ← write tool
│   ├── task/init.lua       ← task tool (sub-agents)
│   ├── skill/init.lua      ← skill tool (playbook loading)
│   ├── webfetch/init.lua   ← webfetch tool
│   ├── websearch/init.lua  ← websearch tool
│   └── todo_write/init.lua ← todo_write tool
│
├── scripts/                ← Python utility scripts
├── site/                   ← docs site source
└── justfile                ← just recipes
```

## Architecture

### Execution Flow

```
CLI (clap)
  └── cmd::dispatch()
        ├── Auth / Models / Update / Rollback / Migrate / Prompt / Index
        ├── Acp → maki-acp::server::run()
        └── No subcommand → cmd::tui::run()
              ├── Config::load()  ← merge global + project
              ├── StateDir::new() ← XDG paths
              ├── Model::from_spec() ← resolve model
              ├── Lua plugin host ← load builtins
              ├── Session load/Create ← JSONL in data dir
              ├── MCP servers start
              └── Mode dispatch:
                    ├── --print --input-format stream-json → SDK mode
                    ├── --print → print mode
                    └── default → TUI (maki_ui::run())
```

### TUI Mode — Elm Architecture

Single-threaded event loop on main thread. Agent runs on a separate smol executor. Lua runs on its own OS thread.

```
EventLoop (main thread)
  └── tick() → drain_channels() → terminal.draw() → poll_input()
        │                                        │
        │                        Msg::Key / Mouse / Paste / Scroll
        │                                        │
        └── Agent events via flume channels ←─── Agent thread (smol)
                                                      │
                                                      └── Lua thread (mlua)
```

**Msg** (events into App::update):
- `Key(KeyEvent)` — keyboard input
- `Paste(String)` — clipboard paste
- `Mouse(MouseEvent)` — mouse click/drag
- `Scroll { column, row, delta }` — scroll events (coalesced)
- `Agent(Box<Envelope>)` — agent events from the async agent loop

**Action** (side effects dispatched by EventLoop):
- `SendMessage(Box<AgentInput>)` — send user message to agent
- `CancelAgent { run_id }` — cancel current agent run
- `ChangeModel(String)` — switch LLM model
- `Compact` — trigger context compaction
- `OpenEditor(PathBuf)` — open external editor

`App::update(Msg) -> Vec<Action>` is the pure update function.
`EventLoop::handle_action()` processes the side effects.

**Key detail:** `run_id: u64` filters stale events. Stale events have `envelope.run_id != app.current_run_id`.

### Agent Loop

`maki_agent::Agent` in a separate smol task on its own thread.

```
Agent::run(AgentInput)
  └── run_loop()
        └── turn()
          ├── stream_with_retry() ← call LLM provider, parse SSE
          ├── has_tool_calls? → process_tool_calls()
          │     ├── doom-loop detection (3+ identical consecutive calls rejected)
          │     ├── ToolRegistry::get() → resolve tool
          │     ├── Tool::parse() → validate input
          │     ├── ToolInvocation::start_header() → compute header
          │     ├── PermissionManager::enforce() → permission prompt (async)
          │     ├── ToolInvocation::execute() → run tool (parallel via TaskSet)
          │     └── tool_results() → build User message with results
          ├── try_auto_compact() ← check context size
          └── handle_queued_command() ← poll for cancel/compact/interrupt
```

Communication:
- **UI → Agent:** `QueueReceiver<QueueItem>` channel
- **Agent → UI:** `flume::Sender<Envelope>` (AgentEvent + run_id + subagent info)

### Tool System

Three tool sources, unified in `ToolRegistry` (ArcSwap for lock-free reads):

| Source | Registration | Runtime |
|---|---|---|
| **Native** | `#[derive(Tool)]` macro, `register_tools!` | direct Rust async |
| **Lua** | `maki.api.register_tool()` in `init.lua` | sandboxed Luau thread |
| **MCP** | MCP JSON-RPC `tools/list` | stdio or HTTP transport |

**Dispatch:** `tool_dispatch::process_tool_calls()` parses the LLM's tool_use blocks, checks for doom loops, runs non-looped calls in parallel, and collects results into a User message.

**Tool definitions:**
- `#[derive(Tool)]` generates `SCHEMA` (from `#[param(description = "...")]` on fields)
- `schema.rs` is the single source of truth: `ParamSchema` generates both JSON Schema and `validate()`
- `jsonrepair` fixes malformed LLM JSON before validation

**Built-in tools:** `bash`, `read`, `write`, `edit`, `glob`, `grep`, `index`, `memory`, `webfetch`, `websearch`, `task`, `skill`, `question`, `todo_write`, `code_execution`, `batch`

### Permission System

`PermissionManager` at `maki-agent/src/permissions.rs`:
- Three-layer rule resolution: session → config → builtin
- **Deny rules always checked before allow rules** (security-first)
- Scoped patterns: `cargo *`, `/path/**`, `*`, `**`
- `physical_boundary_check()` prevents symlink-based path escapes
- `yolo` bypasses prompts but respects deny rules

### Sessions

- **Format:** Append-only JSONL, crash-tolerant (unparseable trailing lines skipped)
- **Location:** `data_dir/sessions/{uuid}.jsonl`
- **CWD index:** `cwd_latest.json` maps working directory → latest session ID
- **Cursor tracking:** `SessionLog` tracks `saved_msg_count` and `saved_tool_ids` for O(delta) saves

### Config

Two-layer merge: `RawConfig::merge()` overlays project config on top of global.

| Layer | Location |
|---|---|
| Global | `~/.config/maki/config.toml` |
| Project | `.maki/config.toml` |
| Permissions | `~/.config/maki/permissions.toml`, `.maci/permissions.toml` |
| Env | `~/.config/maki/.env`, `.maki/.env` |

`#[derive(ConfigSection)]` proc macro generates a `FIELDS` array with description, type, default, min, and a `validate()` method.

### Storage Paths

| Function | Default | Fallback |
|---|---|---|
| `config_dir()` | `~/.config/maki` | `~/.maki` |
| `data_dir()` | `~/.local/share/maki` | `~/.maki` |
| `state_dir()` | `~/.local/state/maki` | `~/.maki` |
| `logs_dir()` | `~/.local/logs/maki` | `~/.maki` |
| `cache_dir()` | `~/.cache/maki` | `~/.maki` |

Fallback triggers when `~/.maki` exists as a directory (backward compatibility).

### SDK Mode

`--print --input-format stream-json` speaks a Claude Code-compatible wire protocol.
- `StreamSynth` synthesizes Anthropic SSE events (block starts/deltas/stops)
- Tool names mapped: `bash` → `Bash`, `read` → `Read`, etc. (`TOOL_NAME_MAP`)
- `EventPump` transforms agent events into wire messages on stdout
- Reads commands from stdin as JSON

### ACP Mode

`maki acp` runs as an Agent Client Protocol server over stdio (ndjson). Implemented in `maki-acp`.

## Coding Conventions

**Style is enforced by `just ci` (fmt-check + lint + pylint + test + gen-docs-check + machete).**

### Rust Conventions

- Rust 2024 edition, latest features
- No wildcard imports — explicit use statements
- Import types at top: `use std::sync::Arc;` then `Arc<T>`, never inline paths
- Descriptive variable and function names
- `Result<T, E>` over panics, `color_eyre` when error type isn't important
- Custom error types via `thiserror` for domain errors
- Unit tests in same file under `#[cfg(test)]` modules
- `#[test_case]` for parameterized tests, snake_case test names
- `#[derive(Copy)]` only on structs with 1 primitive field
- Consts at top of file after imports
- No trivial comments

### Config

- Add deps to global `Cargo.toml`, then `workspace = true` in package
- Prefer existing dependencies before adding new ones
- `deny_unknown_fields` on all config TOML structs

### Logging

- Structured logging via `tracing` with wide log entries
- JSON logs to `logs_dir`, zstd-compressed, rotating by size/file count

### Tests

- `cargo clippy --all --tests -- -D warnings`
- `cargo nextest run --workspace`
- Use `#[test_case]` from `test-case` crate
- Const error/status messages in tests, assert against shared constants
- No tautology tests

## Key Files to Know

| File | Purpose | Lines |
|---|---|---|
| `maki-agent/src/permissions.rs` | Permission system, scope matching | ~1100 |
| `maki-agent/src/types.rs` | AgentEvent, ToolOutput, SharedBuf | ~1240 |
| `maki-lua/src/runtime.rs` | Lua thread, plugin event loop | ~2170 |
| `maki-ui/src/app/mod.rs` | App state, Msg/update, overlays | ~1740 |
| `maki-config/src/lib.rs` | Config structs, merge logic | ~1900 |
| `maki-storage/src/sessions.rs` | JSONL session I/O | ~1450 |
| `maki-ui/src/components/messages/mod.rs` | Chat message rendering | ~1550 |
| `maki-ui/src/components/tool_display.rs` | Tool output display | ~2020 |
| `maki-ui/src/components/lua_float.rs` | Lua popup rendering | ~1940 |
| `maki-ui/src/app/tests.rs` | UI integration tests | ~2610 |

## Build and Test

```
just ci         # Full CI (fmt, lint, py-lint, test, gen-docs-check, machete)
just test       # Run tests (cargo nextest)
just lint       # clippy --all --tests -- -D warnings
just fmt        # rustfmt + stylua (Lua)
just gen-docs   # Regenerate documentation
```

## Documentation

Site at `./site/`. User docs at `./site/docs/`. Generated by `maki-docgen`.

Run `just gen-docs` to regenerate.

Tone: warm, simple, concise, easy for non-native English. No em-dashes, no emojis, no AI tone.
