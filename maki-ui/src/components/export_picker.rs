use crate::components::Overlay;
use crate::components::list_picker::{ListPicker, PickerAction, PickerItem};
use crate::components::settings_picker::UserSettings;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportType {
    MarkdownClipboard,
    MarkdownSave,
    JsonClipboard,
    JsonSave,
}

#[derive(Debug, Clone)]
pub struct ExportEntry {
    pub export_type: ExportType,
    pub label_text: String,
}

impl PickerItem for ExportEntry {
    fn label(&self) -> &str {
        &self.label_text
    }
}

pub enum ExportPickerAction {
    Consumed,
    Select(ExportEntry),
    Close,
}

pub struct ExportPicker {
    picker: ListPicker<ExportEntry>,
}

impl ExportPicker {
    pub fn new() -> Self {
        Self {
            picker: ListPicker::new(),
        }
    }

    pub fn open(&mut self, cwd: &std::path::Path) {
        let settings = UserSettings::load();
        let path_display = settings.export_path.as_deref().unwrap_or("cwd");
        let resolved_display = if path_display == "cwd" {
            cwd.to_string_lossy().into_owned()
        } else {
            path_display.to_string()
        };

        let entries = vec![
            ExportEntry {
                export_type: ExportType::MarkdownClipboard,
                label_text: "markdown : copy to clipboard".to_string(),
            },
            ExportEntry {
                export_type: ExportType::MarkdownSave,
                label_text: format!("markdown : save to {}", resolved_display),
            },
            ExportEntry {
                export_type: ExportType::JsonClipboard,
                label_text: "json     : copy to clipboard".to_string(),
            },
            ExportEntry {
                export_type: ExportType::JsonSave,
                label_text: format!("json     : copy to {}", resolved_display),
            },
        ];

        self.picker.open(entries, " Export Options ");
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ExportPickerAction {
        match self.picker.handle_key(key) {
            PickerAction::Consumed | PickerAction::Toggle(_, _) => ExportPickerAction::Consumed,
            PickerAction::Select(entry) => {
                self.picker.close();
                ExportPickerAction::Select(entry)
            }
            PickerAction::Close => {
                self.picker.close();
                ExportPickerAction::Close
            }
        }
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        self.picker.view(frame, area)
    }
}

impl Overlay for ExportPicker {
    fn is_open(&self) -> bool {
        self.picker.is_open()
    }

    fn close(&mut self) {
        self.picker.close();
    }
}
