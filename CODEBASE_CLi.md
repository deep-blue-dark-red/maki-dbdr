# Codebase: CLI and Bootstrap

## Entry Point

`src/main.rs` — installs `color_eyre` error handling, runs `cmd::dispatch(Cli::parse())`.

The CLI is split into a few paths:
1. Subcommand (`Auth`, `Models`, `Index`, `Mcp`, `Update`, `Rollback`, `Acp`, `Prompt`, `Migrate`)
2. No subcommand → TUI/SDK/Print bootstrap

### CLI Definition

`src/cli.rs` — `Cli` struct via `clap::Parser`.

**Top-level flags:**
| Flag | Description |
|---|---|
| `-p / --print` | Non-interactive mode, runs prompt and exits |
| `-m / --model` | Model spec (`provider/model-id`) |
| `-v / --verbose` | Include full turn-by-turn messages in `--print` output |
| `-c / --continue` | Resume most recent session in this directory |
| `-s / --session` | Resume specific session by ID |
| `--output-format` | Output format for `--print` (`text` or `stream-json`) |
| `--input-format` | Input format (`text` or `stream-json` for SDK) |
| `--allowed-tools` | Pre-approve tools (comma-separated) |
| `--disallowed-tools` | Disallowed tools |
| `--yolo` | Skip all permission prompts |
| `--exit-on-done` | Exit after agent completes |
| `--max-turns` | Max agent turns |
| `--system-prompt` | System prompt override |
| `--append-system-prompt` | Append to system prompt |
| `--no-commands` | Skip custom commands |
| `--no-plugins` | Disable Lua plugin system |
| `--session-id` | Session ID for SDK |
| `--fork-session` | Fork loaded session under new ID |
| `--permission-mode` | `default`, `acceptEdits`, `plan`, `bypassPermissions` |

Claude Code SDK compatibility flags are accepted but ignored: `--fallback-model`, `--settings`, `--add-dir`, `--mcp-config`, `--tools`, `--betas`, `--max-thinking-tokens`, `--effort`, `--json-schema`, `--max-budget-usd`, `--thinking`, `--thinking-display`.

### Command Dispatch

`src/cmd/mod.rs` — `dispatch(cli: Cli) -> Result<()>`

Routes based on `cli.command`:
- `Auth` → `subcmd::auth_login/logout/status`
- `Models` → `subcmd::models`
- `Index` → `subcmd::index` (runs the index tool on a file)
- `Mcp` → `subcmd::mcp_auth/logout`
- `Update` → `update::update`
- `Rollback` → `update::rollback`
- `Acp` → `acp::run`
- `Prompt` → `subcmd::prompt`
- `Migrate` → `migrate::migrate`
- `None` → `tui::run`

### TUI Bootstrap

`src/cmd/tui.rs` — `run(cli: Cli) -> Result<()>`

Sequence:
1. Resolve model: `setup::resolve_model()`
2. Load global + project config: `Config::load()`
3. Resolve storage: `StateDir::new()`
4. Discover custom commands from `.maki/commands`
5. Init Lua plugin host with native tool registry
6. Load or create session (resolved by `--session`, `--continue`, or new)
7. Start MCP servers
8. **Route to execution mode:**
   - `--print --input-format stream-json` → `sdk_mode::run()` (Claude Code SDK)
   - `--print` → `print::run()` (text mode)
   - default → `maki_ui::run(EventLoopParams)` (TUI)

### Model Resolution

`src/setup.rs` — `resolve_model()`

1. Explicit `--model` spec: parse `Model::from_spec()`
2. Saved last-used model from `StateDir`
3. `default_model` in config
4. Auto-detect by scanning `PROVIDER_PRIORITY` by tier:

```
Provider priority:
  1. Anthropic
  2. OpenAI
  3. Copilot
  4. Z.AI
  5. Synthetic
  6. DeepSeek
For each: ModelTier::Strong first, ModelTier::Medium second
```

Error if no provider has a valid API key.

### Panic and Logging Hooks

`setup::install_panic_log_hook()` — replaces default panic hook to log payload and location via `tracing::error!()`.

`setup::init_logging(storage, config)` — sets up rotating JSON log files via `tracing_subscriber::fmt().json()` with `EnvFilter` for `RUST_LOG` override.

### SDK Mode

`src/sdk_mode.rs` — Claude Code SDK wire protocol compatible mode.

Activated by `--print --input-format stream-json`.

**Key structures:**
- `StreamSynth` — synthesizes Anthropic SSE events (block starts, deltas, stops)
- `SdkWriter` — emits wire messages to stdout
- `EventPump` — maps `AgentEvent`s to wire protocol messages
- `TOOL_NAME_MAP` — maps maki snake_case tool names to PascalCase (Claude expects `Bash`, `Read`, etc.)

**Inbound wire messages** (from stdin):
- `user` — send a prompt
- `control_request` — `initialize`, `interrupt`, `set_permission_mode`, `set_model`
- `control_response` — permission prompt response
- `control_cancel_request` — cancel pending permission prompt

**Outbound wire messages** (to stdout):
- `system` (subtype `init`) — startup info (cwd, tools, model, permission mode)
- `assistant` — LLM response
- `user` — tool results submitted
- `result` — turn completion with timing, usage, cost
- `stream_event` — live Anthropic SSE events (when `--include-partial-messages`)
- `control_request` — permission prompt sent to orchestrator
- `control_response` — ACK to control messages
- `system` (subtype `api_retry`) — retry info

**Permission modes:** `default`, `acceptEdits`, `plan`, `bypassPermissions`

### ACP Mode

`src/cmd/acp.rs` — bootstraps the ACP server (`maki-acp`).

`maki-acp/src/` is an ndjson stdio server implementing the Agent Client Protocol. Files:
- `lib.rs` — entry
- `server.rs` — main server loop
- `methods.rs` — method handlers
- `permissions.rs` — permission handling
- `translate.rs` — protocol translation

### Print Mode

`src/print.rs` — non-interactive text mode. Runs the agent and prints output as plain text.

### Self-Update

`src/update.rs` — download and replace the binary from GitHub releases. Supports rollback.
