#!/usr/bin/env bash
# Verify that all maki-dbdr fork features survived the last merge.
# Run after every `git merge upstream/main`.
# Exits 0 if all checks pass, 1 if any fail.

set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

PASS=0
FAIL=0
FAILURES=()

check() {
    local desc="$1"
    local file="$2"
    local pattern="$3"
    if grep -qF "$pattern" "$file" 2>/dev/null; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("FAIL [$file] $desc")
    fi
}

file_exists() {
    local desc="$1"
    local file="$2"
    if [ -e "$REPO/$file" ]; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("FAIL [missing] $file — $desc")
    fi
}

check_not() {
    local desc="$1"
    local file="$2"
    local pattern="$3"
    if ! grep -qF "$pattern" "$file" 2>/dev/null; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("FAIL [$file] $desc (pattern should not exist: '$pattern')")
    fi
}


# ── New files ────────────────────────────────────────────────────────────────

file_exists "wire logger"      maki-providers/src/wire_log.rs
file_exists "mlog binary"      maki-providers/src/bin/mlog.rs
file_exists "tensorx provider" maki-providers/src/providers/tensorx.rs
# maki-tool-macro and render_hints.rs were deleted on purpose in d952f8aa
# ("plugins: batch: move the last native tool to Lua") — the macro existed to
# declare native Rust tools and nothing declares those any more. Likewise
# plugins/hackernews was dropped in efaefdb8. No checks here on purpose.
file_exists "settings_picker"  maki-ui/src/components/settings_picker.rs
file_exists "export_picker"    maki-ui/src/components/export_picker.rs
file_exists "goto_picker"      maki-ui/src/components/goto_picker.rs
file_exists "skills_modal"     maki-ui/src/components/skills_modal.rs
file_exists "ui config.rs"     maki-ui/src/config.rs
file_exists "kanagawa theme"   maki-ui/src/themes/kanagawa_maki.toml
file_exists "rose pine maki"   maki-ui/src/themes/rose_pine_maki.toml
file_exists "dark daltonized"  maki-ui/src/themes/dark_daltonized.toml

# ── view.rs render wiring (THE critical merge failure point) ─────────────────

F=maki-ui/src/app/view.rs
check "goto_picker rendered"     "$F" "render_if_open!(self.goto_picker)"
check "settings_picker rendered" "$F" "render_if_open!(self.settings_picker)"
check "export_picker rendered"   "$F" "render_if_open!(self.export_picker)"
check "plugins_modal rendered"   "$F" "self.plugins_modal.view(frame"
check "skills_modal rendered"    "$F" "self.skills_modal.view(frame"

# ── command palette ──────────────────────────────────────────────────────────

F=maki-ui/src/components/command.rs
check "/checkpoint in palette" "$F" '"/checkpoint"'
check "/export in palette"     "$F" '"/export"'
check "/skills in palette"     "$F" '"/skills"'
check "/plugins in palette"    "$F" '"/plugins"'
check "/rewind in palette"    "$F" '"/rewind"'
# /rename is owned by the sessions Lua plugin (upstream), not a builtin: the
# fork's AI rename survives as the auto-name of a New session.
check_not "no builtin /rename" "$F" 'name: "/rename"'
check "plugin owns /rename"    "plugins/sessions/init.lua" 'name = "/rename"'

# ── app/mod.rs struct fields + handlers ─────────────────────────────────────

F=maki-ui/src/app/mod.rs
check "settings_picker field"    "$F" "pub(super) settings_picker: SettingsPicker"
check "export_picker field"      "$F" "pub(super) export_picker: ExportPicker"
check "plugins_modal field"      "$F" "pub(super) plugins_modal: PluginsModal"
check "skills_modal field"       "$F" "pub(super) skills_modal: SkillsModal"
check "goto_picker field"        "$F" "pub(super) goto_picker: GotoPicker"
check "/settings opens picker"   "$F" "self.settings_picker.open"
# `/export` and `/skills` wrap their open() onto the next line, so match the
# receiver rather than the call.
check "/export opens picker"     "$F" "self.export_picker"
check "/plugins opens modal"     "$F" "self.plugins_modal.open"
check "/skills opens modal"      "$F" "self.skills_modal"
check "settings_picker overlay"  "$F" "&self.settings_picker,"
check "export_picker overlay"    "$F" "&self.export_picker,"
check "goto_picker overlay"      "$F" "&self.goto_picker,"

# ── keybindings ─────────────────────────────────────────────────────────────

F=maki-ui/src/components/keybindings.rs
check "ConfiguredKeybindings struct"     "$F" "pub struct ConfiguredKeybindings"
check "sessions bind field"              "$F" "pub sessions: Bind"
check "shift_session_down bind field"    "$F" "pub shift_session_down: Bind"
check "shift_session_up bind field"      "$F" "pub shift_session_up: Bind"
check "delete_current_session bind"      "$F" "pub delete_current_session: Bind"
check "toggle_global_sessions bind"      "$F" "pub toggle_global_sessions: Bind"
check "sessions in get_configured_bind"  "$F" '"sessions" =>'
check "shift_session_down in registry"   "$F" '"shift_session_down" =>'
check "toggle_global in registry"        "$F" '"toggle_global_sessions" =>'
check "plan_toggle in registry"          "$F" '"plan_toggle" =>'

# ── turn numbering ───────────────────────────────────────────────────────────

# The hardcoded `format!("{turn_num}‧ you ∙ ")` became a configurable
# template: settings_picker owns the default, messages/mod.rs substitutes {n}.
check "turn prefix default"  maki-ui/src/components/settings_picker.rs 'DEFAULT_USER_PROMPT_PREFIX: &str = "{n}‧ you ∙ "'
check "turn prefix template applied" maki-ui/src/components/messages/mod.rs 'template.replace("{n}"'
F=maki-ui/src/components/messages/mod.rs

# ── session picker ctx tokens ────────────────────────────────────────────────

# The Rust session_picker.rs component was replaced by the Lua /sessions
# plugin; the ctx-size column moved with it. maki-storage still supplies the
# number and event_loop.rs still exposes it to Lua, both checked below.
F=plugins/sessions/init.lua
check "format_context_size fn"  "$F" "local function format_context_size"
check "ctx display format"      "$F" 'local ctx = "ctx: "'
check "context_size exposed to lua" maki-ui/src/event_loop.rs '"context_size": rt.app.state.context_size'
# Upstream binds sessions to <C-p>, which is this fork's prev_chat. Lua
# keymaps dispatch before native binds, so this must not come back.
check_not "sessions must not steal ctrl+p" "$F" 'maki.keymap.set("n", "<C-p>"'

# ── session storage context_size ─────────────────────────────────────────────

F=maki-storage/src/sessions.rs
check "context_size in SessionSummary"  "$F" "pub context_size: u32"
check "context_size in ScanRecord"      "$F" "context_size: u32"

# ── /logs runs user command ──────────────────────────────────────────────────

F=maki-ui/src/event_loop.rs
check "/logs uses log_command"    "$F" "settings.log_command"
check "/logs resolves alog alias" "$F" '.replace("alog"'

# ── terminal run_shell_command ───────────────────────────────────────────────

check "run_shell_command fn" maki-ui/src/terminal.rs "pub(crate) fn run_shell_command"

# ── config file path ─────────────────────────────────────────────────────────

check "user.config filename"     maki-ui/src/config.rs '"user.config"'
check "maki.config migration"    maki-ui/src/config.rs '"maki.config"'

# ── UserSettings defaults ────────────────────────────────────────────────────

F=maki-ui/src/components/settings_picker.rs
check "api_logging default true"     "$F" "api_logging: true"
check "show_reasoning default true"  "$F" "show_reasoning: true"
check "show_token_stats default true" "$F" "show_token_stats: true"
check "log_command default set"      "$F" "tail -n 30 alog"

# ── Commit-level verification (range: 8ea1fcfd..907624df) ────────────────────

# 1. 86955e5e: feat: add interactive settings picker and API logging support
check "SettingsPicker struct definition" "maki-ui/src/components/settings_picker.rs" "pub struct SettingsPicker"

# 2. 9429e05c: feat: replace rotating TUI spinners with static star symbol and dynamic API stats
check "active run start tracking field" "maki-ui/src/app/mod.rs" "turn_start: Option<Instant>"

# 3. 140751b9: fix: remove fallback spinners and resolve clippy warnings
check "teardrop/asterisk spinner rendering" "maki-ui/src/components/tool_display.rs" "Indicator::InProgress =>"

# 4. cc3c527e: feat: implement real-time decaying activity tracker and glowing input borders
check "last api send tracking field" "maki-ui/src/app/mod.rs" "pub(super) last_turn_stats: Option<crate::components::status_bar::TurnStats>"

# 5. 2c499433: feat: implement sliding activity event timeline and persistent status stats
check "active run duration field" "maki-ui/src/app/mod.rs" ".map(|start| start.elapsed().as_secs_f64())"

# 6. 33a7c5d9: feat: make visual history timeline period configurable under settings
check "UserSettings load in settings picker" "maki-ui/src/components/settings_picker.rs" "pub fn load() -> Self"

# 7. 15e4250d: feat: implement full-width top border event history timeline
check "turn api sent at field" "maki-ui/src/app/mod.rs" "turn_start: Option<Instant>"

# 8. fae21874: many changes
check "PromptId::Research template" "maki-agent/src/prompt.rs" "PromptId::Research"

# 9. 09352b12: many changes
check_not "starved removed from view" "maki-ui/src/app/view.rs" "starved"

# 10. 3021e8f9: many changes
check_not "tick_timeline call removed" "maki-ui/src/event_loop.rs" "tick_timeline"

# 11. 0980778f: feat: replace activity timeline with per-turn token stats (PP/TG/CR)
check "turn_first_token_at field" "maki-ui/src/app/mod.rs" "self.last_turn_stats = Some("
check "TurnStats struct definition" "maki-ui/src/components/status_bar.rs" "pub struct TurnStats"

# 12. 4ba71dbc: Implement copy_transcript, logs settings command, command palette exact match priority, and skills menu
check "copy_transcript handler function" "maki-ui/src/app/session.rs" "fn export_session_to_markdown"
check "exact match command palette priority" "maki-ui/src/components/command.rs" "exact_match_takes_precedence"

# 13. 9b299fa0: Add target compaction token configuration and support passing value to /compact
check "target_tokens option in compact" "maki-agent/src/agent/compaction.rs" "target_tokens: Option<usize>"
check "QueueItem Compact variant target_tokens" "maki-ui/src/agent/shared_queue.rs" "Compact {"

# 14. d431749e: Refactor conversation history compaction prompts and constraints
check "compaction CRITICAL LENGTH CONSTRAINT" "maki-agent/src/agent/compaction.rs" "LENGTH CONSTRAINT:"

# 15. 910ad267: feat: show context length in sessions list and refactor compaction logic
check "Compaction attempt info log" "maki-agent/src/agent/compaction.rs" "summary succeeded after truncating oldest rounds"
check "format_context_size in session picker" "plugins/sessions/init.lua" "local function format_context_size"

# 16. ec107910: feat: add rewind and goto commands to command palette and implement GotoPicker
check "GotoPicker file exists" "maki-ui/src/components/goto_picker.rs" "pub struct GotoPicker"
check "rewind command handler" "maki-ui/src/app/mod.rs" "\"/rewind\" =>"

# 17. 16dfda53: fix: remove segment highlight calls from goto and rewind selections
check_not "segment highlight from session" "maki-ui/src/app/session.rs" "segment_highlight"

# 18. f1625960: feat: implement alt+s, alt+shift+a, and alt+shift+s session keybinds
check "SESSIONS matches keybinding" "maki-ui/src/app/mod.rs" "key::SESSIONS.matches"
check "shift_session fn definition" "maki-ui/src/app/session.rs" "pub(super) fn shift_session"

# 19. 767fb9f7: feat: migrate settings to flat Ghostty-style config format and support fully configurable keybindings
check "keybinding label formatting" "maki-ui/src/components/keybindings.rs" "pub fn get_bind_label"
check "config_path in config.rs" "maki-ui/src/config.rs" "pub fn config_path"

# 20. 79c54dae: feat: render plan form dismiss key label dynamically
check "dismiss keys plan form" "maki-ui/src/components/plan_form.rs" "fn dismiss_keys()"

# 21. 481b9885: chore: commit plugin tool usage prompt hints
check "bash tool usage hint" "plugins/bash/init.lua" "Reserve bash for system commands"
# Upstream 0aa0d8b4 reworded this hint ("Use todo_write for multi-step tasks
# ... update **after EACH step**"). What the fork cares about is that the
# per-step instruction survives in the prompt, so match that, not the phrasing.
check "todo_write tool usage hint" "plugins/todo_write/init.lua" "after EACH step"

# 22. 4c394b86: feat: implement config reload command in TUI
check "reload_config function" "maki-ui/src/app/mod.rs" "pub fn reload_config"

# 23. fe67cc40: feat: render user turn prefix as # user ∙ and assistant prefix as └ maki ∙
check "assistant turn prefix" "maki-ui/src/components/tool_display.rs" "prefix: \"maki> \""

# 24. a9712b42: feat: add dot after turn number in user prefix to match {#}. user ∙ format
check "turn prefix dynamic formatting" "maki-ui/src/components/messages/mod.rs" "dynamic_prefix"

# 25. acb6e0fa: feat: rename user back to you in dynamic turn prefix
check "you turn prefix format" "maki-ui/src/components/settings_picker.rs" "you ∙"

# 26. 7f019da6: feat: use hyphenation point (‧) instead of dot in user prefix
check "hyphenation point turn prefix" "maki-ui/src/components/settings_picker.rs" "‧ you"

# 27-29. 5c749436: feat: align TUI skills manager folders with backend Lua discovery
check "skills manager FolderInfo display path" "maki-ui/src/components/skills_modal.rs" "is_enabled: true,"

# 30. a83134fe: feat: resolve project workspace ancestors for skills modal
check "find_project_ancestors fn" "maki-ui/src/components/skills_modal.rs" "fn find_project_ancestors"

# 31. 81d6752d: feat: implement adding/removing skill folders with custom classifications persisted in config
check "FolderTag enum definitions" "maki-ui/src/components/skills_modal.rs" "enum FolderTag"

# 32. d6f97cf2: style: dynamically resize Folders and Skills panes in skills manager
check "skills manager folder height constraint" "maki-ui/src/components/skills_modal.rs" "Constraint::Length(folder_height)"

# 33. ed0ae69a: fix: make 'e' open the selected skill file when focused on Skills
check "SkillsAction EditSkill" "maki-ui/src/components/skills_modal.rs" "EditSkill(std::path::PathBuf)"

# 34. 9975c837: fix: make 'e' open the selected folder in Folders, and update shortcut description text
check "SkillsAction EditSkillsJson" "maki-ui/src/components/skills_modal.rs" "EditSkillsJson(std::path::PathBuf)"

# 35. 4b70bfa2: fix: resolve local project root tagging and load only existing workspace folders
check "project_root starts_with Local tag" "maki-ui/src/components/skills_modal.rs" "starts_with(&project_root)"

# 36. 32bdf2a4: feat: rename /copy_transcript to /export and implement Export Options popup selector
check "ExportPicker struct definition" "maki-ui/src/components/export_picker.rs" "pub struct ExportPicker"
check_not "copy_transcript in palette" "maki-ui/src/components/command.rs" "\"/copy_transcript\""

# 37. ad8fd763: feat: add hackernews plugin and json support for webfetch
check "webfetch json format support" "plugins/webfetch/init.lua" "fmt == \"json\""

# 38. 0f9f9cc1: feat: add /plugins interactive menu with runtime enable/disable
check "PluginsModal struct definition" "maki-ui/src/components/plugins_modal.rs" "pub struct PluginsModal"
check "load_builtin function" "maki-lua/src/loader.rs" "pub fn load_builtin"

# 39. 75288f0e: minor
# The hackernews plugin was removed in efaefdb8, so nothing excludes it now.

# 40-41. 75a1d1a2: feat: refine Ctrl+Shift+D shortcut to delete session and open sessions list popup directly
check "delete current session keybinding" "maki-ui/src/components/keybindings.rs" "delete_current_session"
# NOTE: key::DELETE_CURRENT_SESSION (Ctrl+Shift+D) is still declared and
# configurable but no longer has a handler — the Rust session_picker that
# owned remove_entry is gone. Verify the bind is at least still declared;
# restoring the behaviour against the Lua /sessions plugin is open work.
check "delete current session bind declared" "maki-ui/src/components/keybindings.rs" "pub const DELETE_CURRENT_SESSION"

# 42. fbd0316e: fix: update no session message to match user preference
check "no session message session_picker" "plugins/sessions/init.lua" "No sessions yet"

# 43. a41f4392: feat: add global_sessions setting and Ctrl+Shift+M shortcut to toggle it
check "global_sessions setting in Config" "maki-ui/src/components/settings_picker.rs" "pub global_sessions: bool"
check "toggle_global_sessions keybind in keybindings" "maki-ui/src/components/keybindings.rs" "toggle_global_sessions"

# 44. d9597d12: fix: resolve clippy warnings throughout workspace
# The cwd filter was restructured upstream; the fork just needs cwd carried
# through the scanned header.
check "scanned header carries cwd" "maki-storage/src/sessions.rs" "cwd: header.cwd"

# 45. 11555dfb: fix: skip excluded/disabled skills during plugin discovery
check "skip excluded skills check" "plugins/skill/init.lua" "if not excluded[folder_name] then"

# 46. 611de9a6: Abbreviate status bar tokens text to t
check "abbreviated token stats status bar" "maki-ui/src/components/status_bar.rs" "t"

# 47. 907624df: before merge
file_exists "SKILL_TESTING.md file" "SKILL_TESTING.md"
file_exists "create-plugin skill test script" "tests/agent/skill-test-create-plugin.sh"
file_exists "ssh skill test script" "tests/agent/skill-test-ssh.sh"

# ── User-editable system prompt (/system_prompt is read, not just written) ────

F=maki-agent/src/prompt.rs
check "system.md override loader"    "$F" "pub fn load_user_system_prompt"
check "override consulted by template" "$F" "USER_SYSTEM_PROMPT.load_full()"
check "user prompt relaxes slot check" "$F" "pub fn is_user_supplied"
check "override loaded at startup"   "src/cmd/mod.rs" "load_user_system_prompt()"
check "override reloaded on /reload" "src/cmd/tui.rs" "load_user_system_prompt()"
check "missing slot warns not fails"  "maki-lua/src/api/tool.rs" "is_user_supplied(pid)"

# ── Thinking tokens in the status bar ────────────────────────────────────────

check "reasoning field on TokenUsage" "maki-providers/src/model.rs" "pub reasoning: u32"
check "reasoning parsed (responses)" "maki-providers/src/providers/openai/responses.rs" '"output_tokens_details"'
check "reasoning parsed (compat)"    "maki-providers/src/providers/openai_compat.rs" "completion_tokens_details"
check "TurnStats carries thinking"   "maki-ui/src/components/status_bar.rs" "pub thinking_tokens: u32"
check "status bar renders TH"        "maki-ui/src/components/status_bar.rs" '" | TH {}"'
check "thinking wired from turn"     "maki-ui/src/app/mod.rs" "thinking_tokens: tc.usage.reasoning"

# ── Results ──────────────────────────────────────────────────────────────────

echo ""
echo "╔══════════════════════════════════════════╗"
printf "║  verify-fork: %3d passed, %3d failed     ║\n" "$PASS" "$FAIL"
echo "╚══════════════════════════════════════════╝"

if [ ${#FAILURES[@]} -gt 0 ]; then
    echo ""
    for f in "${FAILURES[@]}"; do
        echo "  $f"
    done
    echo ""
    echo "See FORK.md for what each check expects and how to restore it."
    exit 1
fi

echo ""
echo "All fork features verified. ✓"
