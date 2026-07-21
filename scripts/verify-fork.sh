#!/usr/bin/env bash
# Verify that all maki-mcp fork features survived the last merge.
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
file_exists "tool-macro crate" maki-tool-macro/src/lib.rs
file_exists "settings_picker"  maki-ui/src/components/settings_picker.rs
file_exists "export_picker"    maki-ui/src/components/export_picker.rs
file_exists "goto_picker"      maki-ui/src/components/goto_picker.rs
file_exists "skills_modal"     maki-ui/src/components/skills_modal.rs
file_exists "render_hints"     maki-ui/src/components/render_hints.rs
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
check "/rewind in palette"     "$F" '"/rewind"'
check "/rename in palette"     "$F" '"/rename"'

# ── app/mod.rs struct fields + handlers ─────────────────────────────────────

F=maki-ui/src/app/mod.rs
check "settings_picker field"    "$F" "pub(super) settings_picker: SettingsPicker"
check "export_picker field"      "$F" "pub(super) export_picker: ExportPicker"
check "plugins_modal field"      "$F" "pub(super) plugins_modal: PluginsModal"
check "skills_modal field"       "$F" "pub(super) skills_modal: SkillsModal"
check "goto_picker field"        "$F" "pub(super) goto_picker: GotoPicker"
check "/settings opens picker"   "$F" "self.settings_picker.open"
check "/export opens picker"     "$F" "self.export_picker.open"
check "/plugins opens modal"     "$F" "self.plugins_modal.open"
check "/skills opens modal"      "$F" "self.skills_modal.open"
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

F=maki-ui/src/components/messages/mod.rs
check "turn number prefix"   "$F" 'format!("{turn_num}‧ you ∙ ")'

# ── session picker ctx tokens ────────────────────────────────────────────────

F=maki-ui/src/components/session_picker.rs
check "format_context_size fn"  "$F" "fn format_context_size"
check "ctx display format"      "$F" '"ctx: {} · {}"'

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
check "active run duration field" "maki-ui/src/app/mod.rs" "let elapsed = start.elapsed().as_secs_f64()"

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
check "format_context_size in session picker" "maki-ui/src/components/session_picker.rs" "fn format_context_size"

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
check "dismiss keys plan form" "maki-ui/src/components/plan_form.rs" "DISMISS_KEYS"

# 21. 481b9885: chore: commit plugin tool usage prompt hints
check "bash tool usage hint" "plugins/bash/init.lua" "Reserve bash for system commands"
check "todo_write tool usage hint" "plugins/todo_write/init.lua" "Use todo_write to plan and track"

# 22. 4c394b86: feat: implement config reload command in TUI
check "reload_config function" "maki-ui/src/app/mod.rs" "pub fn reload_config"

# 23. fe67cc40: feat: render user turn prefix as # user ∙ and assistant prefix as └ maki ∙
check "assistant turn prefix" "maki-ui/src/components/tool_display.rs" "prefix: \"maki> \""

# 24. a9712b42: feat: add dot after turn number in user prefix to match {#}. user ∙ format
check "turn prefix dynamic formatting" "maki-ui/src/components/messages/mod.rs" "dynamic_prefix"

# 25. acb6e0fa: feat: rename user back to you in dynamic turn prefix
check "you turn prefix format" "maki-ui/src/components/messages/mod.rs" "you ∙"

# 26. 7f019da6: feat: use hyphenation point (‧) instead of dot in user prefix
check "hyphenation point turn prefix" "maki-ui/src/components/messages/mod.rs" "‧ you"

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
check "skills.json exclude hackernews" ".agents/skills.json" "\"hackernews\""

# 40-41. 75a1d1a2: feat: refine Ctrl+Shift+D shortcut to delete session and open sessions list popup directly
check "delete current session keybinding" "maki-ui/src/components/keybindings.rs" "delete_current_session"
check "delete current session logic in app" "maki-ui/src/app/mod.rs" "self.session_picker.remove_entry"

# 42. fbd0316e: fix: update no session message to match user preference
check "no session message session_picker" "maki-ui/src/components/session_picker.rs" "No previous sessions"

# 43. a41f4392: feat: add global_sessions setting and Ctrl+Shift+M shortcut to toggle it
check "global_sessions setting in Config" "maki-ui/src/components/settings_picker.rs" "pub global_sessions: bool"
check "toggle_global_sessions keybind in keybindings" "maki-ui/src/components/keybindings.rs" "toggle_global_sessions"

# 44. d9597d12: fix: resolve clippy warnings throughout workspace
check "clippy fixed let chains" "maki-storage/src/sessions.rs" "&& header.cwd != c"

# 45. 11555dfb: fix: skip excluded/disabled skills during plugin discovery
check "skip excluded skills check" "plugins/skill/init.lua" "if not excluded[folder_name] then"

# 46. 611de9a6: Abbreviate status bar tokens text to t
check "abbreviated token stats status bar" "maki-ui/src/components/status_bar.rs" "t"

# 47. 907624df: before merge
file_exists "SKILL_TESTING.md file" "SKILL_TESTING.md"
file_exists "create-plugin skill test script" "tests/agent/skill-test-create-plugin.sh"
file_exists "ssh skill test script" "tests/agent/skill-test-ssh.sh"

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
