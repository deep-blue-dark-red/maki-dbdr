use crate::components::settings_picker::UserSettings;
use crate::components::keybindings::{update_bind, get_configured_bind, key_event_to_string};
use std::fs;
use std::path::PathBuf;

thread_local! {
    static TEST_CONFIG_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub fn config_path() -> Result<PathBuf, std::io::Error> {
    if cfg!(test) {
        return Ok(TEST_CONFIG_PATH.with(|p| {
            let mut cell = p.borrow_mut();
            if cell.is_none() {
                use std::hash::{BuildHasher, Hasher};
                let hasher_builder = std::collections::hash_map::RandomState::new();
                let mut hasher = hasher_builder.build_hasher();
                hasher.write_u64(42);
                let rand_val = hasher.finish();
                *cell = Some(std::env::temp_dir().join(format!("maki-test-{rand_val}.config")));
            }
            cell.as_ref().unwrap().clone()
        }));
    }
    let config_dir = maki_storage::paths::config_dir()?;
    let parent = config_dir.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "No parent config dir")
    })?;
    Ok(parent.join("maki.config"))
}

pub fn default_skills_dirs() -> Vec<String> {
    let mut dirs = Vec::new();
    if let Ok(config_dir) = maki_storage::paths::config_dir() {
        dirs.push(config_dir.join("skills").to_string_lossy().into_owned());
    }
    if let Some(home) = maki_storage::paths::home() {
        dirs.push(home.join(".agents/skills").to_string_lossy().into_owned());
        dirs.push(home.join(".claude/skills").to_string_lossy().into_owned());
        dirs.push(home.join(".config/opencode/skills").to_string_lossy().into_owned());
    }
    dirs.push("/Users/mcp/.gemini/config/skills".to_string());
    dirs
}

pub fn load_config() -> UserSettings {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return UserSettings::default(),
    };

    if !path.exists() {
        // Migration logic: if settings.json exists, load it, delete it, and save it in maki.config
        let mut settings = UserSettings::default();
        if let Ok(config_dir) = maki_storage::paths::config_dir() {
            let old_path = config_dir.join("settings.json");
            if old_path.exists() {
                if let Ok(data) = fs::read(&old_path) {
                    if let Ok(parsed) = serde_json::from_slice::<UserSettings>(&data) {
                        settings = parsed;
                    }
                }
                let _ = fs::remove_file(&old_path);
            }
        }
        if settings.skills_dirs.is_empty() {
            settings.skills_dirs = default_skills_dirs();
        }
        save_config(&settings);
        return settings;
    }

    let mut settings = UserSettings::default();
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            settings.skills_dirs = default_skills_dirs();
            return settings;
        }
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim();
            match key {
                "show_system_prompt" => settings.show_system_prompt = val.parse().unwrap_or(false),
                "api_logging" => settings.api_logging = val.parse().unwrap_or(false),
                "show_reasoning" => settings.show_reasoning = val.parse().unwrap_or(false),
                "show_token_stats" => settings.show_token_stats = val.parse().unwrap_or(false),
                "log_command" => settings.log_command = Some(val.to_string()),
                "compact_tokens" => settings.compact_tokens = val.parse().ok(),
                "skills_dir" => settings.skills_dirs.push(val.to_string()),
                "export_path" => settings.export_path = Some(val.to_string()),
                "disabled_plugin" => settings.disabled_plugins.push(val.to_string()),
                "global_sessions" => settings.global_sessions = val.parse().unwrap_or(false),
                "keybind" => {
                    if let Some((shortcut, action)) = val.split_once('=') {
                        let shortcut = shortcut.trim();
                        let action = action.trim();
                        update_bind(action, shortcut);
                    }
                }
                _ => {}
            }
        }
    }

    if settings.skills_dirs.is_empty() {
        settings.skills_dirs = default_skills_dirs();
        save_config(&settings);
    }

    settings
}

pub fn save_config(settings: &UserSettings) {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return,
    };

    let mut lines = Vec::new();
    lines.push("# Maki UI Configuration".to_string());
    lines.push(format!("show_system_prompt = {}", settings.show_system_prompt));
    lines.push(format!("api_logging = {}", settings.api_logging));
    lines.push(format!("show_reasoning = {}", settings.show_reasoning));
    lines.push(format!("show_token_stats = {}", settings.show_token_stats));
    lines.push(format!("global_sessions = {}", settings.global_sessions));
    if let Some(ref cmd) = settings.log_command {
        lines.push(format!("log_command = {}", cmd));
    }
    if let Some(tokens) = settings.compact_tokens {
        lines.push(format!("compact_tokens = {}", tokens));
    }
    for dir in &settings.skills_dirs {
        lines.push(format!("skills_dir = {}", dir));
    }
    if let Some(ref path) = settings.export_path {
        lines.push(format!("export_path = {}", path));
    }
    for plugin in &settings.disabled_plugins {
        lines.push(format!("disabled_plugin = {}", plugin));
    }

    lines.push("".to_string());
    lines.push("# Keybindings".to_string());

    let actions = &[
        "quit",
        "help",
        "prev_chat",
        "next_chat",
        "scroll_half_up",
        "scroll_half_down",
        "scroll_line_up",
        "scroll_line_down",
        "scroll_top",
        "scroll_bottom",
        "pop_queue",
        "delete_word",
        "search",
        "file_picker",
        "toggle_verbose",
        "open_editor",
        "edit_system_prompt",
        "plan_toggle",
        "tasks",
        "suspend",
        "delete",
        "kill_line",
        "line_start",
        "line_end",
        "edit_input",
        "sessions",
        "shift_session_down",
        "shift_session_up",
        "delete_current_session",
        "toggle_global_sessions",
    ];

    for &action in actions {
        if let Some(bind) = get_configured_bind(action) {
            lines.push(format!("keybind = {}={}", key_event_to_string(&bind.to_key_event()), action));
        }
    }

    let _ = fs::write(&path, lines.join("\n"));
}
