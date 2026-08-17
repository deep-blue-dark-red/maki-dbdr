use crate::components::keybindings::{get_configured_bind, key_event_to_string, update_bind};
use crate::components::settings_picker::UserSettings;
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
    let parent = config_dir
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "No parent config dir"))?;
    Ok(parent.join("user.config"))
}

/// `user.config` is a flat one-line-per-setting format, so a value that
/// needs an embedded newline (e.g. a multi-line `user_prompt_prefix`) is
/// written as a literal `\n` escape. Unescapes `\n`, `\t`, and `\\`;
/// anything else after a backslash is left as-is.
fn unescape_config_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Inverse of [`unescape_config_value`], applied when writing a value back
/// out so a raw newline in memory doesn't break the one-line-per-setting
/// file format.
fn escape_config_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// Extracts a `key = value` value, honoring surrounding `"..."` quotes.
/// `raw_val` is the unmodified text after `=` (its trailing whitespace was
/// already removed by the caller's line-level `trim()`, but leading
/// whitespace right after `=` has not been). Quoted values keep their inner
/// whitespace exactly (so a trailing space, e.g. in `user_prompt_prefix`,
/// survives); unquoted values are fully trimmed, matching every other
/// setting in this file.
fn parse_string_value(raw_val: &str) -> &str {
    let s = raw_val.trim_start();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s.trim_end()
    }
}

pub fn default_skills_dirs() -> Vec<String> {
    let mut dirs = Vec::new();
    if let Ok(config_dir) = maki_storage::paths::config_dir() {
        dirs.push(config_dir.join("skills").to_string_lossy().into_owned());
    }
    if let Some(home) = maki_storage::paths::home() {
        dirs.push(home.join(".agents/skills").to_string_lossy().into_owned());
        dirs.push(home.join(".claude/skills").to_string_lossy().into_owned());
        dirs.push(
            home.join(".config/opencode/skills")
                .to_string_lossy()
                .into_owned(),
        );
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
        let mut settings = UserSettings::default();
        if let Some(parent) = path.parent() {
            // Migrate old maki.config → user.config
            let old_maki_config = parent.join("maki.config");
            if old_maki_config.exists() {
                let _ = fs::rename(&old_maki_config, &path);
                // Re-check after migration
                if path.exists() {
                    return load_config();
                }
            }
        }
        // Migrate legacy settings.json
        if let Ok(config_dir) = maki_storage::paths::config_dir() {
            let old_path = config_dir.join("settings.json");
            if old_path.exists() {
                if let Ok(data) = fs::read(&old_path)
                    && let Ok(parsed) = serde_json::from_slice::<UserSettings>(&data)
                {
                    settings = parsed;
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
        if let Some((key, raw_val)) = line.split_once('=') {
            let key = key.trim();
            let val = raw_val.trim();
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
                "spinner_enabled" => settings.spinner_enabled = val.parse().unwrap_or(true),
                "spinner_style" => settings.spinner_style = val.to_string(),
                "override_expand_string" => {
                    settings.override_expand_string =
                        Some(unescape_config_value(parse_string_value(raw_val)));
                }
                "user_prompt_prefix" => {
                    settings.user_prompt_prefix =
                        Some(unescape_config_value(parse_string_value(raw_val)));
                }
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
    lines.push(format!(
        "show_system_prompt = {}",
        settings.show_system_prompt
    ));
    lines.push(format!("api_logging = {}", settings.api_logging));
    lines.push(format!("show_reasoning = {}", settings.show_reasoning));
    lines.push(format!("show_token_stats = {}", settings.show_token_stats));
    lines.push(format!("global_sessions = {}", settings.global_sessions));
    lines.push(format!("spinner_enabled = {}", settings.spinner_enabled));
    lines.push(format!("spinner_style = {}", settings.spinner_style));
    if let Some(ref cmd) = settings.log_command {
        lines.push(format!("log_command = {}", cmd));
    }
    if let Some(tokens) = settings.compact_tokens {
        lines.push(format!("compact_tokens = {}", tokens));
    }
    if let Some(ref expand_str) = settings.override_expand_string {
        lines.push(format!(
            "override_expand_string = \"{}\"",
            escape_config_value(expand_str)
        ));
    }
    if let Some(ref prefix) = settings.user_prompt_prefix {
        lines.push(format!(
            "user_prompt_prefix = \"{}\"",
            escape_config_value(prefix)
        ));
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
            lines.push(format!(
                "keybind = {}={}",
                key_event_to_string(&bind.to_key_event()),
                action
            ));
        }
    }

    let _ = fs::write(&path, lines.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::settings_picker::UserSettings;

    #[test]
    fn escape_unescape_round_trips_newlines_and_backslashes() {
        let original = "---\n{n}> \\literal";
        let escaped = escape_config_value(original);
        assert_eq!(escaped, "---\\n{n}> \\\\literal");
        assert_eq!(unescape_config_value(&escaped), original);
    }

    #[test]
    fn unescape_leaves_unknown_escapes_untouched() {
        assert_eq!(unescape_config_value(r"\q"), r"\q");
    }

    #[test]
    fn save_then_load_round_trips_multiline_user_prompt_prefix() {
        let settings = UserSettings {
            user_prompt_prefix: Some("---\n{n}> ".to_string()),
            ..Default::default()
        };
        settings.save();

        let loaded = load_config();
        assert_eq!(loaded.user_prompt_prefix.as_deref(), Some("---\n{n}> "));

        // Reset so other tests sharing this thread's config file (test mode
        // pins one file per OS thread) don't see this value.
        UserSettings::default().save();
    }
}
