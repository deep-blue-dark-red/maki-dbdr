use crate::components::Overlay;
use crate::components::list_picker::{ListPicker, PickerAction, PickerItem};

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use crate::theme;

const TITLE: &str = " Settings ";
const MAX_VISIBLE: u16 = 10;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UserSettings {
    #[serde(default)]
    pub show_system_prompt: bool,
    #[serde(default)]
    pub api_logging: bool,
    #[serde(default)]
    pub show_reasoning: bool,
    #[serde(default)]
    pub show_token_stats: bool,
    #[serde(default)]
    pub log_command: Option<String>,
    #[serde(default)]
    pub compact_tokens: Option<usize>,
    #[serde(default)]
    pub skills_dirs: Vec<String>,
    #[serde(default)]
    pub export_path: Option<String>,
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
}

impl UserSettings {
    pub fn load() -> Self {
        crate::config::load_config()
    }

    pub fn save(&self) {
        crate::config::save_config(self);
    }

    pub fn resolved_export_path(&self, cwd: &std::path::Path) -> std::path::PathBuf {
        let raw = self.export_path.as_deref().unwrap_or("cwd");
        if raw == "cwd" {
            cwd.to_path_buf()
        } else {
            let path_str = raw.to_string();
            if path_str.starts_with("~/") {
                if let Some(home) = maki_storage::paths::home() {
                    home.join(&path_str[2..])
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
            if let Some(settings) = file_data.and_then(|data| serde_json::from_slice::<Self>(&data).ok()) {
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
    Closed,
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

    pub fn open(
        &mut self,
        show_system_prompt: bool,
        api_logging: bool,
        show_reasoning: bool,
        show_token_stats: bool,
        log_command: Option<String>,
        compact_tokens: Option<usize>,
    ) {
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
                name: format!("log-command: {}", log_command.as_deref().unwrap_or("less +G {}")),
            },
            SettingItem {
                name: format!(
                    "compact-tokens: {}",
                    compact_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "none".to_string())
                ),
            },
        ];
        let enabled = vec![
            show_system_prompt,
            api_logging,
            show_reasoning,
            show_token_stats,
            false,
            false,
        ];

        let path_str = if let Ok(path) = crate::config::config_path() {
            path.to_string_lossy().to_string()
        } else {
            "maki.config".to_string()
        };
        let t = theme::current();
        let mut spans = crate::components::hint_line(&[("Enter", "toggle/edit")]).spans;
        spans.push(Span::styled(", maki.config at ", t.tool_dim));
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
                4 => SettingsPickerAction::EditLogCommand,
                5 => SettingsPickerAction::EditLogCommand,
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
