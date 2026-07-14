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

# ── New files ────────────────────────────────────────────────────────────────

file_exists "wire logger"      maki-providers/src/wire_log.rs
file_exists "mlog binary"      maki-providers/src/bin/mlog.rs
file_exists "tensorx provider" maki-providers/src/providers/tensorx.rs
file_exists "tool-macro crate" maki-tool-macro/src/lib.rs
file_exists "hackernews plugin" plugins/hackernews/init.lua
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
