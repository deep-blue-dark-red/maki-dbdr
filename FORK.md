# maki-dbdr fork features

This file is the source of truth for what the fork adds on top of upstream maki.
Run `scripts/verify-fork.sh` after every upstream merge to catch regressions.

## New files (pure additions — never conflict)

### Providers
- `maki-providers/src/wire_log.rs` — network wire logger
- `maki-providers/src/bin/mlog.rs` — `mlog` binary for reading wire logs
- `maki-providers/src/providers/tensorx.rs` — tensorx provider

### UI components
- `maki-ui/src/components/settings_picker.rs` — UserSettings + interactive settings menu
- `maki-ui/src/components/export_picker.rs` — `/export` transcript picker
- `maki-ui/src/components/goto_picker.rs` — `/goto` turn navigation
- `maki-ui/src/components/skills_modal.rs` — `/skills` TUI skills manager
- `maki-ui/src/config.rs` — flat Ghostty-style config loader (`user.config`)

### Removed on purpose (do not restore on merge)
- `maki-tool-macro/` and `maki-ui/src/components/render_hints.rs` — deleted in
  `d952f8aa` when the last native Rust tool moved to Lua. The macro declared
  native tools; nothing declares those any more.
- `plugins/hackernews/` — deleted in `efaefdb8`.
- `maki-ui/src/components/session_picker.rs` — the session list is now the Lua
  `plugins/sessions/init.lua` plugin. See conflict zone 6.

### Themes
- `maki-ui/src/themes/kanagawa_maki.toml` + 5 variants (ink, lotus, slate, storm, wave)
- `maki-ui/src/themes/rose_pine_maki.toml` + 5 variants (bloom, dusk, haze, midnight, slate)
- `maki-ui/src/themes/dark_daltonized.toml` + `dark_daltonized_v2.toml`

### Bench / tooling
- `bench/` — Python benchmarking suite (runner, mutator, reporter, 20+ tasks)
- `scripts/agent-test.sh`
- `tests/agent/skill-test-*.sh`

---

## Conflict zones (files changed in both fork and upstream)

These are the files that break silently on merge. Each section describes what the
fork requires and how to verify it survived.

---

### 1. `maki-ui/src/app/view.rs` ← HIGHEST RISK

The dirty merge missed this entirely. Fork overlays must appear in the render loop.

**Required in `render_picker_overlays`:**
```rust
render_if_open!(self.goto_picker);
render_if_open!(self.settings_picker);
render_if_open!(self.export_picker);
```

**Required in `render_top_modals`:**
```rust
let r = self.plugins_modal.view(frame, full);
if r.width > 0 { overlay_rect = r; }
let r = self.skills_modal.view(frame, full);
if r.width > 0 { overlay_rect = r; }
```

---

### 2. `maki-ui/src/components/command.rs`

Fork commands that must be in `BUILTIN_COMMANDS`:
- `/checkpoint` — summary checkpoint without compacting
- `/export` — transcript export picker
- `/skills` — skills manager
- `/plugins` — plugin enable/disable
- `/rewind` — delete turns menu

`/rename` is deliberately NOT a builtin any more: upstream's
`plugins/sessions/init.lua` registers `/rename <title>`, so the fork's AI-naming
stays only as the automatic rename of a `New session` (see `start_rename`).

---

### 3. `maki-ui/src/app/mod.rs`

#### Struct fields (must survive merge)
```rust
pub(super) settings_picker: SettingsPicker,
pub(super) export_picker: ExportPicker,
pub(super) plugins_modal: PluginsModal,
pub(super) skills_modal: SkillsModal,
pub(super) goto_picker: GotoPicker,
pub show_token_stats: bool,
```

#### Command handlers
```rust
"/settings" => { self.settings_picker.open(&settings); vec![] }
"/export"   => { self.export_picker.open(...); vec![] }
"/plugins"  => { self.plugins_modal.open(...); vec![] }
"/skills"   => { self.skills_modal.open(...); vec![] }
"/checkpoint" => { ... }
"/rewind"   => { ... }
```

(`/rename` is upstream's Lua plugin command now; the fork's auto-name of a
`New session` still goes through `start_rename`.)

#### `overlays()` / `overlays_mut()` arrays
All five fork components must be present.

---

### 4. `maki-ui/src/components/keybindings.rs`

Fork uses a runtime-configurable `ConfiguredKeybindings` struct (not just static consts).

**Required fields in `ConfiguredKeybindings`:**
```rust
pub sessions: Bind,
pub shift_session_down: Bind,
pub shift_session_up: Bind,
pub delete_current_session: Bind,
pub toggle_global_sessions: Bind,
```

**Required named actions in `get_configured_bind()`:**
`sessions`, `shift_session_down`, `shift_session_up`, `delete_current_session`,
`toggle_global_sessions`, `toggle_verbose`, `open_editor`, `edit_system_prompt`, `plan_toggle`

**Required in `update_bind()` match arm:**
All of the above names must route to the right field.

---

### 5. `maki-ui/src/components/messages/mod.rs`

Turn numbering. The hardcoded prefix became a configurable template, so the
requirement now spans two files:

- `settings_picker.rs` must keep
  `const DEFAULT_USER_PROMPT_PREFIX: &str = "{n}‧ you ∙ ";`
- `messages/mod.rs` must substitute the turn number into it:
  `template.replace("{n}", &user_turns.to_string())`

---

### 6. `plugins/sessions/init.lua`

The session list moved out of Rust into this Lua plugin. It must show context
size:
```
devise-a-brilliant ...  ctx: 135.8K · 1d ago
```
which needs the plugin's `format_context_size()` helper, `context_size` on
`SessionSummary` (conflict zone 7), and `event_loop.rs` exposing
`"context_size": rt.app.state.context_size` to Lua.

**Upstream re-adds `maki.keymap.set("n", "<C-p>", open, ...)` here.** Ctrl+P is
this fork's `prev_chat`, and Lua keymaps are dispatched *before* native binds
(`app/mod.rs`, `dispatch_override` runs ahead of `handle_main_chat_key`), so
that line silently shadows `prev_chat`. Sessions are reached through the
configurable `sessions` bind (Alt+S) instead. `verify-fork.sh` guards this.

---

### 7. `maki-storage/src/sessions.rs`

```rust
pub struct SessionSummary {
    pub context_size: u32,  // ← fork addition
    ...
}
```

`ScanRecord::Meta` must include `#[serde(default)] context_size: u32`.
`read_last_meta()` must return `(String, u64, u32)` not `(String, u64)`.

---

### 8. `maki-ui/src/event_loop.rs`

`Action::RunLogsCommand` must run the user's `log_command` setting against
`storage.path()/maki.log`, replacing `alog`/`{}`/`<path>` placeholders.

Must NOT open the logs directory in an editor (upstream default).

---

### 9. `maki-ui/src/terminal.rs`

Must have `run_shell_command(cmd: &str, terminal: &mut DefaultTerminal)` for
tearing down TUI, running a shell command, and restoring.

---

### 10. `maki-ui/src/config.rs`

- Config file path: `user.config` inside `maki_storage::paths::config_dir()` (e.g. `~/.config/maki/user.config`)
- Migration: legacy `user.config`/`maki.config` (beside the config dir) and in-dir `maki.config` → `user.config` on first load
- `default_skills_dirs()` returns 5 paths including `~/.gemini/config/skills`

---

### 11. UserSettings defaults (`maki-ui/src/components/settings_picker.rs`)

```rust
impl Default for UserSettings {
    fn default() -> Self {
        Self {
            api_logging: true,       // NOT false
            show_reasoning: true,    // NOT false
            show_token_stats: true,  // NOT false
            log_command: Some("tail -n 30 alog | jlf -c | less -R".to_string()),
            ...
        }
    }
}
```

---

---

### 12. User-editable system prompt (`maki-agent/src/prompt.rs`)

`/system_prompt` seeds and opens `<config_dir>/system.md`. That file must
actually be **read back**, not just written:

- `load_user_system_prompt()` stores it in the `USER_SYSTEM_PROMPT` `ArcSwap`.
- `PromptId::template()` returns it for `PromptId::System` when set.
- Called from `src/cmd/mod.rs::dispatch` (startup, every subcommand) and from
  the `RunOutcome::Reload` arm in `src/cmd/tui.rs` (so `/reload` re-reads it).
- A blank or missing file falls back to the built-in prompt.

Because `PromptId::has_slot()` tests the *live* template, a user prompt that
drops a `{{slot}}` marker would otherwise abort plugin loading. `is_user_supplied()`
downgrades that to a warning in `maki-lua/src/api/tool.rs` — without it, editing
system.md can brick startup.

---

### 13. Thinking tokens in the status bar

`TokenUsage::reasoning` carries provider-reported thinking tokens. It is a
**subset of `output`**, never added to it, so it must stay out of
`total_input()` / `context_tokens()` / cost math.

- Parsed from `output_tokens_details.reasoning_tokens` (OpenAI Responses) and
  `completion_tokens_details.reasoning_tokens` (openai_compat: OpenRouter,
  DeepSeek, …). Anthropic folds thinking into `output_tokens` and reports 0.
- Not persisted to `StoredTokenUsage` — it is a live per-round readout.
- `TurnStats::thinking_tokens` renders as ` | TH 1.2k` next to `TG`, and is
  omitted entirely when zero.

---

### 14. `plugins/skill/init.lua` — skill_test failure diagnostics

The `skill_test` agent tool runs behavioral smoke tests from a SKILL.md `tests:`
frontmatter (architecture in `SKILL_TESTING.md`). The fork hardened its failure
reporting; `run_one_test()` must keep:

- **stderr capture** — an `on_stderr` handler collects subprocess stderr into
  `stderr_parts`; it is no longer dropped.
- **Per-test `timeout_ms` override** — `local deadline = tonumber(tc.timeout_ms) or 60000`
  (default 60s per test).
- **Elapsed timing** — `os.time()` delta returned as `elapsed` on every result.
- **Diagnostic tails on failure** — the `tail_oneline()` helper collapses
  whitespace and keeps the last 300 chars; the timeout, JSON-parse and
  `is_error` paths return elapsed time, exit code, stderr tail and stdout tail.
  The old bare `return nil, "timeout after 60s"` must not come back.
- **Report rendering** — ERROR rows render the diagnostics in a fenced block;
  PASS/FAIL rows include duration; FAIL rows append the first 800 chars of the
  LLM response.

Plugins are embedded at compile time (`include_dir!`, see `maki-lua/src/loader.rs`),
so Lua edits require `cargo build --release` to take effect. `find_maki_bin()`
prefers `maki` on PATH over the repo build, so the installed binary must be
refreshed too or the old plugin keeps running.

Verify:
```bash
grep -n "on_stderr\|timeout_ms\|tail_oneline" plugins/skill/init.lua
cargo test -p maki-lua --test spec skill_plugin_spec
```

---

## Upstream v0.5.5 merge notes (2026-09-17)

Resolved on `merge/upstream-v0.5.5` (upstream `5da1b2a2`, v0.5.5; 49 conflicted
files). Decisions taken, so the next merge does not re-litigate them:

- `/rename` is upstream's Lua plugin command now. The fork's builtin is gone;
  the AI naming survives as the automatic rename of a `New session`
  (`App::start_rename`). `verify-fork.sh` checks the plugin owns the name.
- MCP tools in plan mode are permission-gated (upstream), not hard-blocked.
- `Segment::height()` keeps the fork's semantics (old width while stale,
  `drawn_height()` is the truth); upstream's contradicting test was dropped.
- The in-transcript search corpus uses the rendered `{n}‧ you ∙ ` prefix, so a
  query for a turn number hits what the reader saw.
- Both output knobs stay: the fork's `agent.max_output_tokens` (model cap
  clamp) and upstream's `agent.max_turn_output` (per-turn budget).
- Terminal focus uses upstream's `Focus` tri-state with the fork's
  background-frame throttle ported onto it.
- Late instruction segments keep the fork's anchor-based cache.

Ported from upstream onto the fork's structures:

- `ToolOutput::filterable_text_mut` + its test (upstream's output hooks need it).
- `InflightGate::acquire_before_abandoned`: a tool call still queued for a slot
  now ends its wait at cancel/deadline instead of parking until one frees.
- `deliver_pending_job_events`: a finished task makes one bounded (256 event)
  final drain under its scope, so events landing after the pump stops are not
  lost with the scope.

Not ported (upstream's parallel implementations of areas the fork replaced):
upstream's `RowWalk`/`next_chunk` message renderer, its projection-based
selection/model, and their tests.

Known test failures inherited from upstream on macOS (verified on pristine
`5da1b2a2`, not merge regressions): the `maki-pack`/`maki-lua` pack lock tests
under parallel load, `trusted_folders::non_utf8_paths_are_refused`, and the
`markdown::the_code_memo_follows_the_theme…` flake under a full-workspace run.

## Known gaps (not regressions, but broken)

- **`Ctrl+Shift+D` (delete current session) has no handler.** The bind is
  declared in `keybindings.rs`, is user-configurable via `config.rs`, and is
  advertised — but nothing matches `key::DELETE_CURRENT_SESSION`. The handler
  lived on the deleted Rust `session_picker` (`remove_entry`) and was not
  reimplemented when the session list moved to Lua. Pressing it does nothing.

## Merge checklist

Run after every `git merge upstream/main`:

```bash
scripts/verify-fork.sh       # grep-based checks, exits non-zero on failure
cargo check --workspace      # compilation
cargo test -p maki-ui -p maki-storage  # unit tests
```

If `verify-fork.sh` fails, the specific check output tells you exactly which
file needs patching and what to add.
