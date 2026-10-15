# maki-dbdr fork features

This file is the source of truth for what the fork adds on top of upstream maki.
Run `scripts/verify-fork.sh` after every upstream merge to catch regressions.

## New files (pure additions — never conflict)

### Providers
- `maki-providers/src/wire_log.rs` — network wire logger (owns the `.mlog` format; the old `bin/mlog.rs` viewer binary is gone, `e4a42ed8`)
- `maki-providers/src/providers/tensorx.rs` — tensorx provider

### UI components
- `maki-ui/src/components/settings_picker.rs` — UserSettings + interactive settings menu
- `maki-ui/src/components/export_picker.rs` — `/export` transcript picker
- `maki-ui/src/components/goto_picker.rs` — `/goto` turn navigation
- `maki-ui/src/components/skills_modal.rs` — `/skills` TUI skills manager
- `maki-ui/src/components/plugins_modal.rs` — `/plugins` enable/disable + per-plugin tool info
- `maki-ui/src/components/stats_modal.rs` — `/stats` token/cost readout (o200k counts)
- `maki-ui/src/components/system_prompt_modal.rs` — `/system_prompt` resolved-prompt viewer
- `maki-ui/src/components/tool_prompt_modal.rs` — `/tool_prompt` per-plugin tool toggles
- `maki-ui/src/components/prompt_view.rs` — resolved-prompt rendering shared by the prompt modals
- `maki-ui/src/components/cache_miss_prompt.rs` — cache-miss warm-ping warning
- `maki-ui/src/components/list_picker.rs` — generic list picker (sessions, rewind, menus)
- `maki-ui/src/config.rs` — flat Ghostty-style config loader (`user.config`)

### Agent / runtime
- `maki-ui/src/agent/keepwarm.rs` — idle provider prompt-cache warmer (zone 15)
- `maki-agent/src/prompts/checkpoint_user.md`, `session_name.md` — `/checkpoint` and auto session-naming templates
- `maki-lua/src/api/util/loaded.rs` — shared "is plugin loaded" helper
- `maki-storage/src/aa_scores.rs` — Artificial Analysis intelligence-score cache for the model picker

### rstring crate
- `maki-rstring/` — output compression + the `rstring` CLI: tiered collapsing of lines that differ only in volatile values (`line [xN]`), uniform JSONL → one TSV header row, entropy-run masking, o200k token counting, semantic thinning, `export-md` transcript rendering. Wired at two boundaries (zone 16).

### Plugins (fork-only)
- `plugins/outline/` — upstream's `plugins/index`, renamed; do not restore `index`
- `plugins/cd/` — `/cd-pick`, the folder picker over `maki.dir_picker`
- `plugins/status/` — NDJSON status feed for external programs
- `plugins/async/` — background job queue + jobs panel
- `plugins/ast_grep/`, `plugins/tool_help/`, `plugins/create_plugin/`, `plugins/aa_scores/`, `plugins/cronjob/`, `plugins/compilerbrain/` (tool views for the foreign `compilerbrain.*` tools)

### Removed on purpose (do not restore on merge)
- `maki-tool-macro/` and `maki-ui/src/components/render_hints.rs` — deleted in
  `d952f8aa` when the last native Rust tool moved to Lua. The macro declared
  native tools; nothing declares those any more.
- `plugins/hackernews/` — deleted in `efaefdb8`.
- `maki-ui/src/components/session_picker.rs` — the session list is now the Lua
  `plugins/sessions/init.lua` plugin. See conflict zone 6.
- `plugins/index/` — upstream's plugin, renamed to `plugins/outline` on the
  fork (`b0242269`); restoring it double-registers the outline tool.

### Themes
- `maki-ui/src/themes/kanagawa_maki.toml` + 5 variants (ink, lotus, slate, storm, wave)
- `maki-ui/src/themes/rose_pine_maki.toml` + 5 variants (bloom, dusk, haze, midnight, slate)
- `maki-ui/src/themes/dark_daltonized.toml` + `dark_daltonized_v2.toml`

### Bench / tooling
- `skill-benchmarking/` — Python benchmarking suite (runner, mutator, reporter,
  `tasks/` fixtures, `variants/` baselines)
- `scripts/agent-test.sh`, `scripts/merge-audit.sh`
- `tests/agent/skill-test-*.sh` (incl. `skill-test-create-plugin.sh`,
  `skill-test-ssh.sh`)

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

**Required in `render_top_modals`** (fork modals; `usage_modal` between them
is upstream's):
```rust
let r = self.system_prompt_modal.view(frame, full);
if r.width > 0 { overlay_rect = r; }
let r = self.tool_prompt_modal.view(frame, full);
if r.width > 0 { overlay_rect = r; }
let r = self.plugins_modal.view(frame, full);
if r.width > 0 { overlay_rect = r; }
let r = self.skills_modal.view(frame, full);
if r.width > 0 { overlay_rect = r; }
```

`stats_modal` renders further down on `is_open()`, and `cache_miss_prompt`
reserves bottom layout height and renders there — both must survive too.

---

### 2. `maki-ui/src/components/command.rs`

Fork commands that must be in `BUILTIN_COMMANDS`:
- `/checkpoint` — summary checkpoint without compacting
- `/export` — transcript export picker
- `/skills` — skills manager
- `/plugins` — plugin enable/disable
- `/rewind` — delete turns menu
- `/config` — alias of `/settings`
- `/resume` — alias of `/sessions` (dispatches to the sessions Lua plugin)
- `/stats` — token/cost stats modal
- `/system_prompt` — resolved-prompt viewer + editor seeding (zone 12)
- `/tool_prompt` — per-plugin tool toggle modal

`/rename` is deliberately NOT a builtin any more: upstream's
`plugins/sessions/init.lua` registers `/rename <title>`, so the fork's AI-naming
stays only as the automatic rename of a `New session` (see `start_rename`).

Upstream ships `plugins/completion/` (a Lua `@` mention popup driven by a
`completion.sources` slot). The fork drops both on merge: the native
`file_picker` is the only `@` popup, and it opens without printing anything,
with the `@` printed only if the picker closes without a pick
(`mention_pending`), so a selected path reaches the message bare. Keeping
upstream's plugin would put two popups on the same `@`.

`/cd` exists upstream, but the fork reworked it: a bare `/cd` opens the dir
picker, the `change_cwd` action is bound to Ctrl+L, and `plugins/cd` adds
`/cd-pick` on top.

---

### 3. `maki-ui/src/app/mod.rs`

#### Struct fields (must survive merge)
```rust
pub(super) settings_picker: SettingsPicker,
pub(super) export_picker: ExportPicker,
pub(super) plugins_modal: PluginsModal,
pub(super) skills_modal: SkillsModal,
pub(super) goto_picker: GotoPicker,
pub(super) system_prompt_modal: SystemPromptModal,
pub(super) tool_prompt_modal: ToolPromptModal,
pub(super) stats_modal: StatsModal,
pub(super) cache_warm: Option<CacheWarmPing>,
pub show_token_stats: bool,
```

#### Command handlers
```rust
"/settings" => { self.settings_picker.open(&settings); vec![] }
"/export"   => { self.export_picker.open(...); vec![] }
"/plugins"  => { self.plugins_modal.open(...); vec![] }
"/skills"   => { self.skills_modal.open(...); vec![] }
"/stats"       => { self.stats_modal.toggle(); vec![] }
"/cd"          => self.cmd_cd(&cmd.args),
"/tool_prompt" => { ... }
"/checkpoint" => { ... }
"/rewind"   => { ... }
```

(`/rename` is upstream's Lua plugin command now; the fork's auto-name of a
`New session` still goes through `start_rename`.)

#### `overlays()` / `overlays_mut()` arrays
All fork overlays must be present: the five components above plus
`system_prompt_modal`, `tool_prompt_modal` and `stats_modal` (the arrays claim
23 entries).

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

Two more fork-only behaviors live in this file now:

- **Per-folder thinking level** (`d2e68bfa`): `set_cwd_thinking` records the
  level last set via `/thinking` in a folder's meta; a fresh session there
  starts on it instead of the default.
- **By-path session links** (`9dffeaef`): sessions are linked as
  `by-path/<folder>/<ts>-<alias>.jsonl` beside the canonical id-named logs;
  `--session-name` names a directory's links, and the cwd index keeps the
  newest session per directory plus its alias.

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

- Config file path: `user.config` inside `maki_storage::paths::config_dir()` (e.g. `~/.maki/user.config`)
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

### 15. Idle cache warming (`maki-ui/src/agent/keepwarm.rs`)

While the UI sits idle, `keepwarm` sends bounded warm pings (`WARM_MAX_PINGS`)
so the provider prompt cache stays hot; `app/mod.rs` owns the `cache_warm:
Option<CacheWarmPing>` field and posts the `kv-cache-warm ping sent (n/N)`
status. A ping that misses the cache warns (`⚠ cache warm miss`) through
`cache_miss_prompt.rs`, which also reserves bottom layout height in `view.rs`.
Upstream has neither file — a merge reworking the idle path must re-wire the
pump, not drop it.

---

### 16. rstring compression (two boundaries)

`maki-rstring` compresses exactly two edges, both gated by the `rstring`
config knob:

- tool output at the lua→agent edge — `maki-lua/src/api/tool.rs` (bash output
  above `maki_rstring::MIN_BYTES` collapses; errors pass through)
- user messages after input hooks — `maki-agent/src/agent/run.rs`
  (`compress_if_useful` above `MIN_BYTES`; tests pin the disabled path)

`plugins/bash/init.lua` documents the resulting format for the model. A merge
that rewrites either call site silently re-inflates every big tool result.

---

### 17. `plugins/memory/init.lua` — fork extensions on an upstream plugin

`memory` is upstream's plugin; the fork added `/memory move`, the Ctrl+M
project picker and `move_to_project` (notes re-keyed onto another project).
They depend on fork-only Lua API: `maki.fs.rename`, `maki.dir_picker` and
`maki.known_paths` (which owns the project-id hash and the known-roots scan).
An upstream rewrite of the plugin silently drops the move UI; the API surface
itself is pure addition and safe.

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
- Router-reported cost (`bdd5dfef`): `TokenUsage::cost` reads OpenRouter's
  `usage.cost` and `billed_cost` prefers it, so a turn on a router is billed at
  what the router charged instead of the table's rate for the model we asked
  for. The pricing helper it displaced is now `TokenUsage::estimate`.
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

- Follow-up audit (2026-09-18) of `621a95a4`: all zones re-checked semantically
  (verify-fork 131/131, `cargo check --workspace --tests` clean). Three
  pre-existing gaps found and fixed in the follow-up commit: the stale `bench/`
  path above, missing `update_bind` arms for `delete_current_session` /
  `toggle_global_sessions`, and a hardcoded `~/.gemini` skills path.
  `verify-fork.sh` now guards all three (145 checks).

## Upstream v0.5.6 merge notes (2026-09-28)

Resolved on `main` (upstream `49fda0e5`; 36 conflicted files, 85 commits past
the `5da1b2a2` fork point). verify-fork 150/150, `cargo clippy --all --tests
-- -D warnings` clean, `cargo fmt --check` clean. Decisions taken, so the next
merge does not re-litigate them:

- **Permissions.** `is_universal_scope` reads a `/**` prefix both ways
  (canonical `canonical_key` *and* lexical `normalize_path`), while
  `scope_matches` grants by the canonical reading only. The fork's
  `universal_prefix_root`, which answered lexically in both places, is gone:
  it made a lexically-rooted pattern match every scope, which upstream's
  `lexical_root_through_symlink_is_refused_but_grants_only_its_subtree`
  refuses. `maki-agent/src/permissions.rs` is now byte-identical to upstream.
- **History marks its writes.** `push` and `rewrite` go through `edit`, which
  sets `unsaved`, so `SessionTurn::drop` persists again. Without that, seven
  `session::tests` found no file on disk.
- **Compaction.** `compact_history` keeps the fork's `target_tokens` and
  upstream's `hooks` + `(usage, summary)` return; `summarize` stays on
  `cancel`/`session_id` and is handed them from `hooks` at the call.
- **Turn completion.** `emit_turn_complete` keeps the fork's `cost`/`turn`
  parameters and the caller's `ledger.add`; upstream's `subsidised_list_cost`
  rides along in `TurnCompleteEvent`.
- **Copilot** calls upstream's `responses::apply_responses_reasoning` again,
  so the Responses body carries `summary: "auto"`. The fork's local copy is
  gone.
- **`@` completion dropped, not shipped disabled.** Upstream's
  `plugins/completion/`, its `completion.sources` slot and the loader entry
  are removed: the native `file_picker` is the only `@` popup (conflict zone
  2). `OPTIONAL_BUILTINS` is now empty and its test went with it; the hooks
  and context docs lost the matching section and `gen-docs` regenerated the
  rest.
- **Two upstream tests retargeted.** `a_key_an_unfocused_popup_claimed_...`
  types `'a'` and `moving_the_cursor_alone_fires_...` types `"src"`, because
  on this fork `@` opens the file picker without typing and never reaches the
  input. Both still assert what they were written for.

Inherited failures, verified on pristine `49fda0e5` in a scratch worktree and
therefore not merge regressions: the 13 `file_index::tests` (parallel-load
sensitive), `maki-pack`'s `deletion_preflight_..._one_revision_is_live` lock,
`markdown::the_code_memo_follows_the_theme...`, and
`child_env::child_sees_only_keys_set_after_strip`.

## Fork development since v0.5.6 (2026-09-29 → 2026-10-14)

No upstream merge in this window — main advanced on its own (69 commits).
Decisions and features a future merge must not trample:

- **Agent loop** (`9a2eef65`): stop-vs-end cancel semantics — a stop with no
  run in flight goes stale instead of killing the next run; the Lua agent api
  gained interrupt sources and an input channel (a session's registration,
  not its opening run, ends it), so listening sessions can be paused, fed
  text and continued. The task plugin surfaces `paused` results.
- **Default system prompt rewritten** (`0296256b`); zone 12's user
  `system.md` still overrides it wholesale.
- **Z.AI plan-gated 429s** are retried inside the rate-limit budget
  (`bd998fcc`), not surfaced as failures.
- **Hardening to keep**: MCP tool content decoding — content types,
  structured content, list pagination (`f1061ab8`); ACP bounded, cancel-safe
  stdin framing (`709cd601`); bedrock framing, SSE deadlines, openai tool
  accumulation (`b7303e2b`); API keys masked on char boundaries (`5eeaf07d`);
  config-macro errors are spanned, not panics (`4e6607f9`).
- **Tool surface**: `tool_help` plugin and `get_tool` returning the tool
  description (`4b4b0349`); input examples rendered for providers without
  native support (`b359d48b`); argument values in tool headers (`62cf50fd`);
  `register_tool_view` renders tools a plugin does not own (`1ca156c6`).
- **Keybindings**: the thinking picker sits on `plan_toggle` (Ctrl+T), the
  todo panel took Alt+T (`d2e68bfa`).
- **`/logs`** falls back to `less` when `jlf` is not on PATH (`6b54ac37`).
- **`create_plugin`** scaffolds personal plugins into the config dir
  (`2d25721b`), with an agent-level test
  (`tests/agent/skill-test-create-plugin.sh`).
- The **upstream v0.6.3 merge** is being resolved on branch
  `merge/upstream-v0.6.3`; its decisions (Rust providers → Lua declarations,
  `openrouter_pins.rs`, `tensorx` as a Lua plugin, `RowWalk` re-homed into
  `messages/segment.rs`) are recorded in that branch's FORK.md and land with
  the merge.

## Known gaps (not regressions, but broken)

- ~~`Ctrl+Shift+D` (delete current session) has no handler.~~ Fixed on the
  `merge/upstream-v0.6.3` branch (`26bebc6b`): the bind dispatches the
  sessions plugin's `/delete_session`. Still a no-op on `main` until that
  merge lands.

## Merge checklist

Run after every `git merge upstream/main`:

```bash
scripts/verify-fork.sh       # grep-based checks, exits non-zero on failure
cargo check --workspace      # compilation
cargo test -p maki-ui -p maki-storage  # unit tests
```

If `verify-fork.sh` fails, the specific check output tells you exactly which
file needs patching and what to add.
