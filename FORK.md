# maki-mcp fork features

This file is the source of truth for what the fork adds on top of upstream maki.
Run `scripts/verify-fork.sh` after every upstream merge to catch regressions.

## New files (pure additions — never conflict)

### Providers
- `maki-providers/src/wire_log.rs` — network wire logger
- `maki-providers/src/bin/mlog.rs` — `mlog` binary for reading wire logs
- `maki-providers/src/providers/tensorx.rs` — tensorx provider

### Infrastructure
- `maki-tool-macro/` crate — proc macro for tool definitions

### UI components
- `maki-ui/src/components/settings_picker.rs` — UserSettings + interactive settings menu
- `maki-ui/src/components/export_picker.rs` — `/export` transcript picker
- `maki-ui/src/components/goto_picker.rs` — `/goto` turn navigation
- `maki-ui/src/components/skills_modal.rs` — `/skills` TUI skills manager
- `maki-ui/src/components/render_hints.rs` — render hints component
- `maki-ui/src/config.rs` — flat Ghostty-style config loader (`user.config`)

### Plugins
- `plugins/hackernews/init.lua` — HackerNews reader

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
- `/rename` — AI-generate session name

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
"/rename"   => self.start_rename(),
```

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

Turn numbering must be present in the message render loop:
```rust
} else if msg.role == DisplayRole::User {
    let turn_num = self.messages[..=i]
        .iter()
        .filter(|m| m.role == DisplayRole::User)
        .count();
    dynamic_prefix = format!("{turn_num}‧ you ∙ ");
    &dynamic_prefix
```

---

### 6. `maki-ui/src/components/session_picker.rs`

Session list must show context size:
```
devise-a-brilliant ...  ctx: 135.8K · 1d ago
```

Requires `format_context_size()` helper and `context_size` field in `SessionEntry`.

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

- Config file path: `user.config` (parent of `maki_storage::paths::config_dir()`)
- Migration: `maki.config` → `user.config` on first load
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

## Merge checklist

Run after every `git merge upstream/main`:

```bash
scripts/verify-fork.sh       # grep-based checks, exits non-zero on failure
cargo check --workspace      # compilation
cargo test -p maki-ui -p maki-storage  # unit tests
```

If `verify-fork.sh` fails, the specific check output tells you exactly which
file needs patching and what to add.
