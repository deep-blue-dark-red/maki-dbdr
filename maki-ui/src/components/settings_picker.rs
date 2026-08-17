use crate::components::Overlay;
use crate::components::list_picker::{ListPicker, PickerAction, PickerItem};

use crate::theme;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwap;

const TITLE: &str = " Settings ";
const MAX_VISIBLE: u16 = 10;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserSettings {
    #[serde(default)]
    pub show_system_prompt: bool,
    #[serde(default = "default_true")]
    pub api_logging: bool,
    #[serde(default = "default_true")]
    pub show_reasoning: bool,
    #[serde(default = "default_true")]
    pub show_token_stats: bool,
    #[serde(default = "default_log_command")]
    pub log_command: Option<String>,
    #[serde(default)]
    pub compact_tokens: Option<usize>,
    #[serde(default)]
    pub skills_dirs: Vec<String>,
    #[serde(default)]
    pub export_path: Option<String>,
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
    #[serde(default)]
    pub global_sessions: bool,
    #[serde(default)]
    pub override_expand_string: Option<String>,
    #[serde(default = "default_true")]
    pub spinner_enabled: bool,
    #[serde(default = "default_spinner_style")]
    pub spinner_style: String,
    #[serde(default)]
    pub user_prompt_prefix: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_log_command() -> Option<String> {
    Some("tail -n 30 alog | jlf -c | less -R".to_string())
}

fn default_spinner_style() -> String {
    "braille".to_string()
}

/// Default template for the user-turn prefix; `{n}` is replaced with the
/// 1-based turn number. See [`UserSettings::user_prompt_prefix_template`].
const DEFAULT_USER_PROMPT_PREFIX: &str = "{n}‧ you ∙ ";

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            show_system_prompt: false,
            api_logging: true,
            show_reasoning: true,
            show_token_stats: true,
            log_command: Some("tail -n 30 alog | jlf -c | less -R".to_string()),
            compact_tokens: None,
            skills_dirs: Vec::new(),
            export_path: None,
            disabled_plugins: Vec::new(),
            global_sessions: false,
            override_expand_string: None,
            spinner_enabled: true,
            spinner_style: default_spinner_style(),
            user_prompt_prefix: None,
        }
    }
}

/// In-memory cache of `user.config`, mirroring the `theme::current()` /
/// `animation::spinner_config()` pattern: reading settings happens on hot
/// render paths (e.g. the user-prompt prefix, truncation hints), so `load()`
/// must not hit disk every call. Populated lazily, kept current by `save()`,
/// and force-refreshed by `reload()` after the file is hand-edited.
static SETTINGS_CACHE: LazyLock<ArcSwap<UserSettings>> =
    LazyLock::new(|| ArcSwap::from_pointee(crate::config::load_config()));

impl UserSettings {
    pub fn load() -> Self {
        if cfg!(test) {
            // `config::config_path()` returns a distinct file per test
            // thread under `cfg!(test)`; the process-wide cache below would
            // leak settings between concurrently-running tests, so bypass
            // it and always hit disk here, same as before caching existed.
            return crate::config::load_config();
        }
        (**SETTINGS_CACHE.load()).clone()
    }

    /// Re-reads `user.config` from disk and refreshes the cache. Call after
    /// the file may have been edited outside `save()` (e.g. in `$EDITOR`).
    pub fn reload() -> Self {
        let fresh = crate::config::load_config();
        if !cfg!(test) {
            SETTINGS_CACHE.store(Arc::new(fresh.clone()));
        }
        fresh
    }

    pub fn save(&self) {
        crate::config::save_config(self);
        if !cfg!(test) {
            SETTINGS_CACHE.store(Arc::new(self.clone()));
        }
    }

    /// The hint text shown next to collapsed/truncated content ("click to
    /// expand" by default, or the user's `override_expand_string`).
    pub fn expand_hint(&self) -> String {
        self.override_expand_string
            .clone()
            .unwrap_or_else(|| "click to expand".to_string())
    }

    /// The template for the prefix shown before each user turn (default
    /// `"{n}‧ you ∙ "`, or the user's `override`). `{n}` is replaced with the
    /// 1-based turn number.
    pub fn user_prompt_prefix_template(&self) -> String {
        self.user_prompt_prefix
            .clone()
            .unwrap_or_else(|| DEFAULT_USER_PROMPT_PREFIX.to_string())
    }

    pub fn resolved_export_path(&self, cwd: &std::path::Path) -> std::path::PathBuf {
        let raw = self.export_path.as_deref().unwrap_or("cwd");
        if raw == "cwd" {
            cwd.to_path_buf()
        } else {
            let path_str = raw.to_string();
            if let Some(stripped) = path_str.strip_prefix("~/") {
                if let Some(home) = maki_storage::paths::home() {
                    home.join(stripped)
                } else {
                    std::path::PathBuf::from(path_str)
                }
            } else {
                std::path::PathBuf::from(path_str)
            }
        }
    }

    pub fn load_legacy_json() -> Self {
        if let Ok(config_dir) = maki_storage::paths::config_dir() {
            let path = config_dir.join("settings.json");
            let file_data = std::fs::read(&path).ok();
            if let Some(settings) =
                file_data.and_then(|data| serde_json::from_slice::<Self>(&data).ok())
            {
                return settings;
            }
        }
        Self::default()
    }
}

pub enum SettingsPickerAction {
    Consumed,
    ToggleShowSystemPrompt(bool),
    ToggleApiLogging(bool),
    ToggleShowReasoning(bool),
    ToggleShowTokenStats(bool),
    EditLogCommand,
    OpenUserConfig,
    OpenSystemConfig,
    Closed,
    ToggleGlobalSessions(bool),
    ToggleSpinnerEnabled(bool),
}

#[derive(Clone)]
struct SettingItem {
    name: String,
}

impl PickerItem for SettingItem {
    fn label(&self) -> &str {
        &self.name
    }
}

pub struct SettingsPicker {
    picker: ListPicker<SettingItem>,
}

impl SettingsPicker {
    pub fn new() -> Self {
        Self {
            picker: ListPicker::new().with_max_visible(MAX_VISIBLE),
        }
    }

    pub fn open(&mut self, settings: &UserSettings) {
        let items = vec![
            SettingItem {
                name: "show-system-prompt".to_string(),
            },
            SettingItem {
                name: "api-logging".to_string(),
            },
            SettingItem {
                name: "show-reasoning".to_string(),
            },
            SettingItem {
                name: "show-token-stats".to_string(),
            },
            SettingItem {
                name: "global-sessions".to_string(),
            },
            SettingItem {
                name: "spinner-enabled".to_string(),
            },
            SettingItem {
                name: format!(
                    "log-command: {}",
                    settings.log_command.as_deref().unwrap_or("less +G {}")
                ),
            },
            SettingItem {
                name: format!(
                    "compact-tokens: {}",
                    settings
                        .compact_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "none".to_string())
                ),
            },
            SettingItem {
                name: format!("spinner-style: {}", settings.spinner_style),
            },
            SettingItem {
                name: format!(
                    "user-prompt-prefix: {}",
                    settings.user_prompt_prefix_template()
                ),
            },
            SettingItem {
                name: "open user.config in editor".to_string(),
            },
            SettingItem {
                name: "open maki init.lua in editor".to_string(),
            },
        ];
        let enabled = vec![
            settings.show_system_prompt,
            settings.api_logging,
            settings.show_reasoning,
            settings.show_token_stats,
            settings.global_sessions,
            settings.spinner_enabled,
            false,
            false,
            false,
            false,
            false,
            false,
        ];

        let path_str = if let Ok(path) = crate::config::config_path() {
            path.to_string_lossy().to_string()
        } else {
            "user.config".to_string()
        };
        let t = theme::current();
        let mut spans = crate::components::hint_line(&[("Enter", "toggle/edit")]).spans;
        spans.push(Span::styled(", user.config at ", t.tool_dim));
        spans.push(Span::styled(path_str, t.item_desc));
        self.picker.set_static_footer(Line::from(spans));

        self.picker.open_toggleable(items, enabled, TITLE);
    }

    pub fn is_open(&self) -> bool {
        self.picker.is_open()
    }

    pub fn close(&mut self) {
        self.picker.close();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SettingsPickerAction {
        match self.picker.handle_key(key) {
            PickerAction::Toggle(idx, val) => match idx {
                0 => SettingsPickerAction::ToggleShowSystemPrompt(val),
                1 => SettingsPickerAction::ToggleApiLogging(val),
                2 => SettingsPickerAction::ToggleShowReasoning(val),
                3 => SettingsPickerAction::ToggleShowTokenStats(val),
                4 => SettingsPickerAction::ToggleGlobalSessions(val),
                5 => SettingsPickerAction::ToggleSpinnerEnabled(val),
                6 => SettingsPickerAction::EditLogCommand,
                7 => SettingsPickerAction::EditLogCommand,
                8 => SettingsPickerAction::EditLogCommand,
                9 => SettingsPickerAction::EditLogCommand,
                10 => SettingsPickerAction::OpenUserConfig,
                11 => SettingsPickerAction::OpenSystemConfig,
                _ => SettingsPickerAction::Consumed,
            },
            PickerAction::Close => SettingsPickerAction::Closed,
            PickerAction::Consumed => SettingsPickerAction::Consumed,
            PickerAction::Select(..) => SettingsPickerAction::Consumed,
        }
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        self.picker.view(frame, area)
    }

    pub fn handle_paste(&mut self, text: &str) -> bool {
        self.picker.handle_paste(text)
    }
}

impl Overlay for SettingsPicker {
    fn is_open(&self) -> bool {
        self.is_open()
    }

    fn close(&mut self) {
        self.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // ── maki-mcp fork tests ───────────────────────────────────────────────────────

    #[test]
    fn test_resolved_export_path_cwd() {
        let cwd = Path::new("/my/project");
        let mut settings = UserSettings::default();
        assert_eq!(settings.resolved_export_path(cwd), cwd);

        settings.export_path = Some("cwd".to_string());
        assert_eq!(settings.resolved_export_path(cwd), cwd);
    }

    #[test]
    fn test_resolved_export_path_absolute() {
        let settings = UserSettings {
            export_path: Some("/tmp/export".to_string()),
            ..Default::default()
        };
        let cwd = Path::new("/my/project");
        assert_eq!(settings.resolved_export_path(cwd), Path::new("/tmp/export"));
    }
}
