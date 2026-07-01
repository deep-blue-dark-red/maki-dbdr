use crate::components::Overlay;
use crate::components::list_picker::{ListPicker, PickerAction, PickerItem};

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

const TITLE: &str = " Settings ";
const MAX_VISIBLE: u16 = 10;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserSettings {
    #[serde(default)]
    pub show_system_prompt: bool,
    #[serde(default)]
    pub api_logging: bool,
    #[serde(default)]
    pub show_reasoning: bool,
    #[serde(default)]
    pub show_token_stats: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            show_system_prompt: false,
            api_logging: false,
            show_reasoning: false,
            show_token_stats: false,
        }
    }
}

impl UserSettings {
    pub fn load() -> Self {
        if let Ok(config_dir) = maki_storage::paths::config_dir() {
            let path = config_dir.join("settings.json");
            let file_data = std::fs::read(&path).ok();
            if let Some(settings) = file_data.and_then(|data| serde_json::from_slice::<Self>(&data).ok()) {
                return settings;
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        if let Ok(config_dir) = maki_storage::paths::config_dir() {
            let path = config_dir.join("settings.json");
            if let Ok(data) = serde_json::to_vec_pretty(self) {
                let _ = std::fs::write(&path, data);
            }
        }
    }
}

pub enum SettingsPickerAction {
    Consumed,
    ToggleShowSystemPrompt(bool),
    ToggleApiLogging(bool),
    ToggleShowReasoning(bool),
    ToggleShowTokenStats(bool),
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

    pub fn open(&mut self, show_system_prompt: bool, api_logging: bool, show_reasoning: bool, show_token_stats: bool) {
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
        ];
        let enabled = vec![show_system_prompt, api_logging, show_reasoning, show_token_stats];
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
