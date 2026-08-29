# IDEAS HEAP

Features missing vs a fully featured harness (Claude Code, opencode, aider, codex class).
Ranked in tiers. Verified against the current tree, 2026-08-21.

## Already have (for reference)

index (tree-sitter), code_execution (monty sandbox), task (model tiers, research/general), batch,
memory, skill, todo_write, question, view_image, webfetch (SSRF guard), websearch, MCP (stdio/HTTP/OAuth,
prompts), plan mode, concurrent sessions, queue, /btw, /compact, /checkpoint, rewind (chat only),
export, custom slash commands, Lua plugins, 12+ providers incl. local, dynamic providers, headless
stream-json, ACP, permissions with bash tree-sitter scoping, compaction, usage modal, themes,
auto-update.

## Tier 1: high value, contained effort

### Code rewind (file checkpoints)
README says "no code rewind yet, only chat history". Snapshot file state per turn (path, hash,
content or git-reflog based) and restore on `/rewind`. `file_tracker` already tracks mtimes for
stale reads; extend it to keep content. Biggest gap vs Claude Code.

### Lifecycle hooks
User-configurable hooks: PreToolUse, PostToolUse, Stop, SessionStart/End, Notification. Run a shell
command or Lua with JSON on stdin/stdout; exit code blocks the tool. Lua plugin host already has
tool start hooks, so the plumbing exists. Unlocks post-edit diagnostics (below) without a new tool.

### Post-edit diagnostics
After edit/write, run the project check (cargo check, tsc, ruff, luac) on touched files and feed
errors back to the agent. Implement as a PostToolUse hook or a native step in the edit/write path.

### OS notifications + terminal bell
Desktop notification (notify-rust) and/or terminal bell when a background session finishes or needs
input. Today only the status bar flashes.

### @-mentions in input
`@file`, `@url`, `@selection` with completion, injected into the prompt as context. File picker
already exists to back completion.

### Custom subagents
User-defined subagents: `agents/*.md` with frontmatter (model tier, tool allowlist, system prompt).
`task` today only exposes built-in research/general types.

### Persistent shell / background processes
bash is one-shot with a timeout. Add background runs (returns an id), attach/kill, tail output, and
keep dev servers alive across turns.

### Model fallback
retry.rs retries the same provider. On 5xx or timeout, fall back to a configured alternate
model/provider.

### Spend caps
Max tokens or cost per session, hard stop. Usage is visible in the modal but nothing enforces a
limit.

## Tier 2: high value, larger effort

### LSP integration
Language servers for diagnostics, go-to-def, find references, hover. Tree-sitter covers skeletons;
LSP covers semantics. Start with a `diagnostics` tool aggregating LSP output for edited files.

### Structured git tools
git status/diff/log/blame as first-class tools with parsed output, plus commit and PR workflows and
a pre-commit secret scan. Bash covers it, but structured tools save tokens and give the permission
system real targets.

### Test runner integration
Detect the project runner (nextest, pytest, go test), run it, parse failures into a compact report,
feed back. Pairs with post-edit diagnostics into a "fix until green" loop.

### Browser automation
Headless browser tool (playwright) for interactive pages, forms, JS-heavy docs. webfetch is static
HTML only.

### General file watching
notify crate is used for theme reload only. Watch files the agent read or edited; warn on external
change. stale_read_check is mtime-at-read only.

### Sandboxed bash
Run untrusted commands in a namespace/container (bubblewrap on Linux, sandbox-exec on macOS) with
configurable mounts. Today bash runs with the user's full permissions; the permission system gates
the prompt, not the process.

### MCP resources
prompts/get is already wired in transport.rs. Add resource listing and reads into context.

### Session full-text search
Session picker filters by name only. Index message bodies (SQLite FTS) to find "the session where we
fixed X".

## Tier 3: filler

- Semantic search (embeddings) as opt-in plugin. semble and ast-grep exist externally; bundle them.
- Per-hunk diff review: accept/reject individual hunks in the diff UI.
- Path-scoped rules: `.maki/rules/*.md` with `globs:` frontmatter (path-conditional instructions).
- Session sharing: share a session as a URL or zip. Export exists, no share.
- Voice input (push-to-talk, local STT).
- PDF and image OCR in `read`.
- Remote execution: bash over SSH with local file sync.
- Multi-root: more than one working dir in a session.
- Status bar customization (segments, custom Lua).
- Structured fix loop: run tests, fix, re-run until green or budget exhausted.
  max_continuation_turns exists, no test-aware loop.
- Prompt templates with variables (extend /system_prompt editing).
- Token budget per subagent.
- Cost estimate before running a plan.
- Prompt injection heuristics for webfetch/websearch content (SSRF guard exists, no content guard).
- Vim mode.
- Telemetry (opt-in).
- Ghost text / inline completion (separate product category, listed for completeness).

## Probably out of scope

- IDE plugins beyond ACP (VS Code extension).
- Multi-user collaboration.
