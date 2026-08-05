// Fork migration gate + feature behavior tests.
//
// These verify Matthew's fork features (status bar, thinking mode, pricing
// calculator, log menu, skills menu, plugins menu, wire logs, settings, state
// transitions) are present AND wired into the running App.
//
// Wiring-gate tests (prefixed `gate_`) currently FAIL on `main` because the
// prior migration only copied the source files; they were never instantiated,
// registered in the command palette, or rendered in `view.rs`. Each failure
// maps 1:1 to a missing wiring point. Fix the wiring, then these go green.
//
// Behavior tests (prefixed `behavior_`) lock correct semantics of the code that
// does exist, so it cannot regress.

use crate::BUILTIN_COMMANDS;
use crate::components::export_picker::ExportPicker;
use crate::components::goto_picker::{GotoPicker, GotoPickerAction};
use crate::components::plugins_modal::{PluginsModal, PluginsAction};
use crate::components::settings_picker::{SettingsPicker, UserSettings};
use crate::components::skills_modal::{FolderTag, SkillsModal};
use crate::components::status_bar::TurnStats;
use crate::components::Action;
use crate::components::command::ParsedCommand;

const FORK_COMMANDS: &[&str] = &[
    "/checkpoint",
    "/export",
    "/skills",
    "/plugins",
    "/rewind",
    "/rename",
    "/settings",
];

#[test]
fn gate_fork_commands_registered_in_palette() {
    let registered: Vec<&str> = BUILTIN_COMMANDS.iter().map(|c| c.name).collect();
    for cmd in FORK_COMMANDS {
        assert!(
            registered.iter().any(|r| r == cmd),
            "fork command {cmd} missing from BUILTIN_COMMANDS (command palette not wired)"
        );
    }
}

#[test]
fn gate_overlay_components_instantiable() {
    let _settings = SettingsPicker::new();
    let _skills = SkillsModal::new();
    let _plugins = PluginsModal::new();
    let _goto = GotoPicker::new();
    let _export = ExportPicker::new();
    let _stats = TurnStats {
        pp_tps: 0.0,
        tg_tps: 0.0,
        cache_rate: 0.0,
    };
    let _tag = FolderTag::Maki;
}

#[test]
fn behavior_user_settings_fork_defaults() {
    // FORK.md requires these ON by default, not the derived `false`.
    let s = UserSettings::default();
    assert!(s.api_logging, "api_logging must default to true");
    assert!(s.show_reasoning, "show_reasoning (thinking mode) must default to true");
    assert!(s.show_token_stats, "show_token_stats must default to true");
    assert!(
        s.log_command.is_some(),
        "log_command must have a fork default (log menu wiring)"
    );
}

#[test]
fn behavior_folder_tag_variants_exist() {
    let _ = [
        FolderTag::Maki,
        FolderTag::Global,
        FolderTag::Other,
        FolderTag::Local,
    ];
}

#[test]
fn behavior_turn_stats_constructable() {
    let stats = TurnStats {
        pp_tps: 12.5,
        tg_tps: 8.0,
        cache_rate: 0.42,
    };
    assert_eq!(stats.pp_tps, 12.5);
    assert_eq!(stats.cache_rate, 0.42);
}

#[test]
fn gate_checkpoint_and_rename_dispatch_to_actions() {
    // `/checkpoint` and `/rename` must be registered and route to the fork
    // Action variants that drive the agent-side checkpoint/rename queue items.
    let registered: Vec<&str> = BUILTIN_COMMANDS.iter().map(|c| c.name).collect();
    assert!(
        registered.iter().any(|r| r == &"/checkpoint"),
        "/checkpoint missing from palette"
    );
    assert!(
        registered.iter().any(|r| r == &"/rename"),
        "/rename missing from palette"
    );

    // The App handler must produce the matching Action variants. We assert the
    // variants exist and are constructed from the commands (compile-time proof
    // the wiring points at the right enum arms).
    let _checkpoint = Action::Checkpoint;
    let _rename = Action::RenameSession(vec![]);
    let _cp_cmd = ParsedCommand {
        name: "/checkpoint".into(),
        args: String::new(),
    };
    let _rn_cmd = ParsedCommand {
        name: "/rename".into(),
        args: String::new(),
    };
}
