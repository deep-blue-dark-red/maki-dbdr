# MAKI fork merge — analysis & pain points

Forensic + repair log for migrating Matthew's fork features (`forkv2`) into the
current `main` of `/Users/mcp/git/maki` (a Ratatui Rust TUI workspace). Written
2026-07-22.

## Repo layout (gotcha: not where you'd guess)
- Working repo: `/Users/mcp/git/maki`. There is NO `forkv3` branch and NO
  `marki` dir (`marki` was a typo). `~/git/maki` is the right path.
- Remotes: `upstream` = `/Users/mcp/git/maki-main`.
- Branches: `main` (HEAD = `16041b9f "Matthew fork merge"`), `forkv2`
  (Matthew's original features, `fb8527f3`), `maki-mcp` (a prior BOTCHED
  migration attempt).
- `forkv2` and `maki-mcp` are NOT ancestors of `main`. The "Matthew fork
  merge" commit re-applied features by hand and dropped most wiring.

## The core problem
The migration copied feature SOURCE FILES but never wired them. Smoking gun:
`cargo check -p maki-ui` showed **38 dead-code warnings** (every fork
component compiled but was never instantiated/rendered/registered). After
wiring, warnings dropped to ~9 (pre-existing list_picker/skills_modal helpers).

## Authoritative spec
`maki-mcp`'s `FORK.md` + `scripts/verify-fork.sh` is the source of truth for
what each feature requires. The grep-based `verify-fork.sh` is the migration
gate. Ported it into `main` as `scripts/verify-fork.sh`. Baseline: 63 passed /
55 failed. After wiring the 6 overlays: 89 passed / 29 failed.

## Feature status (post-repair)
| Feature | Status |
|---|---|
| Settings menu (`/settings`) | WIRED — palette, handler, field, key handling, render, overlays[] |
| Skills menu (`/skills`) | WIRED (same) |
| Plugins menu (`/plugins`) | WIRED (same) |
| Export (`/export`) | WIRED (was partially wired) |
| Rewind (`/rewind`) | WIRED |
| Goto (`/goto`) | WIRED — `goto_turn()` reuses `rewind_to` |
| Checkpoint (`/checkpoint`) | WIRED — added Action/QueueItem/agent fn + event |
| Rename (`/rename`) | WIRED — added Action/QueueItem/agent fn + RenameResult event |
| Thinking mode defaults | FIXED — `UserSettings` now derives fork defaults (api_logging/show_reasoning/show_token_stats=true, log_command set) |
| Pricing calculator (OpenRouter) | ALREADY CORRECT — `TokenUsage::cost` + per-token→$/1M scaling; added openrouter tests |
| Wire logs / mlog / tensorx | Present (never broken) |
| Log menu (`/logs`) | ALREADY WIRED in main via `Action::RunLogsCommand` + `run_view_log_command` (forkv2 used `run_shell_command` — different name, same effect) |
| Status bar / TurnStats transitions | PARTIAL — `TurnStats` struct + status_bar render exist; live tracking fields (turn_start, last_turn_stats, turn_first_token_at) NOT driven from app/mod.rs yet |
| Session-list context size | DIVERGENT — main refactored the session picker; `format_context_size` lives in a different module than forkv2's `session_picker.rs` |
| Compaction target_tokens + LENGTH CONSTRAINT | DIVERGENT — main's `compact()` takes no target_tokens; agent compaction.rs differs from forkv2 |

## Pain points / gotchas for future work
1. **Two parallel agent loops.** `QueueItem` is consumed by
   `maki-ui/src/agent/agent_loop.rs::process_entry` (the active path), AND
   `into_extracted_command()` maps it to `ExtractedCommand` consumed by
   `maki-agent/src/agent/run.rs::handle_queued_command`. Adding a `QueueItem`
   variant requires updating BOTH the `QueueItem` match (agent_loop) AND the
   `ExtractedCommand` enum + its match (run.rs). Easy to miss → non-exhaustive
   match compile errors.
2. **`maki-agent::agent` module is private** (`mod compaction;` in
   `agent/mod.rs`). Public fns must be re-exported there
   (`pub use compaction::{compact, checkpoint, rename_session};`). Adding a
   `pub fn` in `compaction.rs` is NOT enough — also add the re-export or
   external crates can't see it (error: "cannot find function in module agent").
3. **`AgentEvent` changes ripple.** Adding a variant (e.g. `RenameResult`,
   `CompactionStart`) must be handled in `maki-ui/src/chat.rs::handle_event`
   AND any exhaustive match (tests.rs). Route rename back to app via a new
   `ChatEventResult::RenameResult(String)` variant, handled in
   `maki-ui/src/app/mod.rs`.
4. **`UserSettings` is the fork's config; main kept `maki.config`** (forkv2
   migrated to `user.config`). Don't "fix" the config filename — it's an
   intentional upstream divergence, not a regression.
5. **`SessionMeta` in main has no `show_system_prompt`/`show_reasoning`
   fields** (forkv2 did). The settings toggles save to `UserSettings` config
   only; don't assign to `self.state.session.meta.*` or it won't compile.
6. **Theme test `theme::tests::set_installs_theme_before_generation_observed`
   is PRE-EXISTING broken** (from the "Matthew fork merge" themes commit, before
   any of my work). Do not treat it as a regression from wiring changes.
7. **`verify-fork.sh` is grep-based**, so it lags reality: it fails on
   functionally-correct code that uses different identifiers (e.g. it wanted
   `run_shell_command` but main has `run_view_log_command`; it wanted
   `self.export_picker.open` on one line). Reconcile the script's patterns when
   the underlying feature is actually wired, or the gate will keep reporting
   false failures.
8. **Main already had exact-match command priority** (forkv2's
   `exact_match_takes_precedence` check is satisfied by `eq_ignore_ascii_case`
   in command.rs).

## Test additions (this session)
- `maki-ui/src/app/fork_features_test.rs` — migration gate tests (6 pass):
  palette registration, settings defaults, overlay constructability,
  FolderTag/TurnStats, checkpoint/rename Action dispatch.
- `maki-providers/src/model.rs` — `openrouter_pricing_mapping_is_correct` +
  `openrouter_zero_pricing_yields_zero_cost` (OpenRouter pricing correctness).
- `scripts/verify-fork.sh` — committed grep gate (baseline 63/55 → 89/29).

## Still TODO (29 verify-fork failures remaining)
- Turn/activity tracking fields in app/mod.rs + event_loop (status bar
  transitions fully live).
- Session-list context size (`format_context_size` in main's refactored picker).
- Compaction `target_tokens` + LENGTH CONSTRAINT logs (agent compaction.rs).
- `config user.config` vs `maki.config` (intentional divergence — reconcile FORK.md).
- `run_shell_command` vs `run_view_log_command` (log menu works — reconcile script).
- 5 missing forkv2-only files: `maki-tool-macro`, `render_hints.rs`,
  `SKILL_TESTING.md`, `tests/agent/skill-test-create-plugin.sh`,
  `tests/agent/skill-test-ssh.sh`.
- Plugin-script tweaks: `webfetch` json support, skill exclusion, `.agents/skills.json` hackernews.
- Minor render: assistant turn prefix, plan_form dismiss keys, 1 clippy let-chain in sessions.rs.

# MAKI fork merge — STRATEGY (updated 2026-07-22)

Decision rule: **less divergence from main is better.** Do NOT force-merge
forkv2 features that conflict with main's architecture. Per-item verdict:
MERGE (cheap + main-compatible), SKIP (fork-only / main already does it /
would require a hackjob), or REIMPLEMENT (valuable but build it natively on
main's structures, not by porting forkv2 code).

## Verdicts on the 29 remaining verify-fork failures

### Already satisfied — FALSE failures in verify-fork.sh (reconcile the gate, don't touch code)
- **assistant turn prefix** (`tool_display.rs`): main ALREADY has `└ maki ∙ `
  and `you ∙ ` prefixes. Fork check wanted the literal `maki> ` — different
  string, same feature. FALSE FAILURE.
- **plan_form dismiss keys**: main already has `dismiss_keys()`. Fork check
  wanted `DISMISS_KEYS` const. FALSE FAILURE.
- **reload_config function**: main's `/reload` quits with `ExitRequest::Reload`
  (native restart-based config reload). Fork's in-place `reload_config()` is a
  fork divergence; main deliberately reloads by restart. NOT NEEDED.
- **run_shell_command fn** (`terminal.rs`): main has `suspend` +
  `open_in_editor` + `run_view_log_command`. The log menu already works via
  `Action::RunLogsCommand`. Fork's `run_shell_command` is a redundant helper.
  NOT NEEDED.
- **log menu**: already wired in main (covered earlier). NOT a gap.

### SKIP — do not merge (fork-only infra / conflicts with main's architecture)
- **maki-tool-macro crate**: does not exist in main and is referenced nowhere.
  Forkv2-only proc-macro crate for tools. Adding it = new crate + converting
  main's tools. Pure divergence, zero benefit. SKIP.
- **render_hints.rs**: unused anywhere in main. Forkv2-only dead code. SKIP.
- **user.config filename** (`config.rs`): main uses `maki.config` intentionally
  (upstream kept it; forkv2 migrated to `user.config`). Changing it ripples
  through config loading/migration for no user benefit. SKIP — keep `maki.config`.
- **session_picker.rs recreation** (`format_context_size`, `ctx display`,
  `no session message`): `maki-ui/src/components/session_picker.rs` does NOT
  exist in main — main refactored sessions into a tabs model
  (`event_loop.tabs`, `AppSession`). Recreating forkv2's whole file = large
  divergence. SKIP the file; if ctx-size-in-session-list is wanted, REIMPLEMENT
  on main's actual session-list UI.
- **compaction target_tokens + CRITICAL LENGTH CONSTRAINT + attempt info log**
  (`compaction.rs`): forkv2 added `target_tokens` to `compact()` and a
  constraint/log. Main's `compact()` takes no target_tokens and has its own
  overflow handling. Porting changes the agent compaction contract. SKIP unless
  a real need arises; main's compaction works.
- **shift_session fn / SESSIONS keybind / delete current session logic in app**:
  main already has `delete_current_session` (Ctrl+Shift+D) and a tabs-based
  session switcher. Forkv2's `shift_session`/`SESSIONS` were built for a
  different session model. SKIP — main's mechanism covers it.
- **5 missing forkv2-only files**: `SKILL_TESTING.md`,
  `tests/agent/skill-test-create-plugin.sh`, `tests/agent/skill-test-ssh.sh` —
  forkv2 testing infra, not product features. SKIP.
- **clippy fixed let chains** (`sessions.rs`): a forkv2 clippy cleanup that is
  not present in main's code; not a feature. SKIP (cosmetic).

### REIMPLEMENT-if-wanted (optional, build natively on main — do NOT port forkv2 code)
- **turn/activity tracking fields** (`turn_start`, `last_api_send`,
  `turn_first_token_at`, `turn api sent at`, `active run start/duration`):
  main's status bar ALREADY renders `TurnStats` (pp/tg/cr) when
  `last_turn_stats` is `Some` — but `app/mod.rs` never populates it, so it's
  dead. This is ADDITIVE: feed `TurnStats` from agent events into the existing
  `StatusBarContext.last_turn_stats`. Low risk, no architectural change. Only
  do it if the live activity display is actually wanted.
- **session ctx size** (if desired): reimplement on main's actual session-list
  UI, not forkv2's deleted `session_picker.rs`.
- **plugin-script tweaks** (`webfetch` json, skill exclusion, `.agents/skills.json`
  hackernews): small, fork-specific Lua features. Low value, low risk. Merge only
  if those plugins are used; otherwise SKIP.

## Bottom line
- The valuable, main-compatible features (settings/skills/plugins/export/rewind/
  goto menus, checkpoint, rename, thinking defaults, OpenRouter pricing) are
  DONE and wired with minimal divergence.
- ~14 of the 29 "failures" are FALSE (main already does it with different
  identifiers) or NOT-NEEDED (main has a native alternative). Reconcile
  `verify-fork.sh` so the gate stops reporting them.
- The genuinely-skippable ones are fork-only infra (tool-macro, render_hints,
  user.config, recreated session_picker, target_tokens compaction) that would
  require architectural hackjobs to port. Leave them out.
- Only one optional ADDITIVE item remains (TurnStats feeding) and it's safe.

## Recommended next action
1. Reconcile `scripts/verify-fork.sh` to mark the false/non-needed checks as
   passing (adjust patterns or convert to `check_not`), so the gate reflects
   reality: expect ~14 fewer "failures" with zero code changes.
2. Do NOT port the SKIP items.
3. Optionally implement TurnStats feeding as the single clean additive feature.
