use crate::components::Overlay;
use crate::components::list_picker::{ListPicker, PickerAction};
use crate::components::rewind_picker::{RewindEntry, display_msg_index_for_turn, NO_TURNS_MSG};

use crossterm::event::KeyEvent;
use maki_providers::Message;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};

const TITLE: &str = " Go To ";
const PREVIEW_MAX_LEN: usize = 80;

pub enum GotoPickerAction {
    Consumed,
    Select(RewindEntry),
    Close,
}

pub struct GotoPicker {
    picker: ListPicker<RewindEntry>,
}

impl GotoPicker {
    pub fn new() -> Self {
        Self {
            picker: ListPicker::new(),
        }
    }

    pub fn open(&mut self, messages: &[Message]) -> Result<(), String> {
        let mut turn_num = 0usize;
        let mut entries: Vec<RewindEntry> = Vec::new();
        for (msg_idx, msg) in messages.iter().enumerate() {
            if !msg.is_user_turn() {
                continue;
            }
            let Some(full_text) = msg.user_text() else {
                continue;
            };
            turn_num += 1;
            let first_line = full_text.lines().next().unwrap_or("");
            let preview = if first_line.len() > PREVIEW_MAX_LEN {
                format!(
                    "{turn_num}: {}...",
                    &first_line[..first_line.floor_char_boundary(PREVIEW_MAX_LEN)]
                )
            } else {
                format!("{turn_num}: {first_line}")
            };
            entries.push(RewindEntry {
                turn_index: msg_idx,
                segment_index: display_msg_index_for_turn(messages, msg_idx),
                prompt_preview: preview,
                prompt_text: full_text.to_owned(),
            });
        }
        if entries.is_empty() {
            return Err(NO_TURNS_MSG.into());
        }
        entries.reverse();
        self.picker.open(entries, TITLE);
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.picker.is_open()
    }

    pub fn close(&mut self) {
        self.picker.close();
    }

    pub fn contains(&self, pos: Position) -> bool {
        self.picker.contains(pos)
    }

    pub fn scroll(&mut self, delta: i32) {
        self.picker.scroll(delta);
    }

    pub fn handle_paste(&mut self, text: &str) -> bool {
        self.picker.handle_paste(text)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> GotoPickerAction {
        match self.picker.handle_key(key) {
            PickerAction::Consumed => GotoPickerAction::Consumed,
            PickerAction::Select(entry) => GotoPickerAction::Select(entry),
            PickerAction::Close => GotoPickerAction::Close,
            PickerAction::Toggle(..) => GotoPickerAction::Consumed,
        }
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        self.picker.view(frame, area)
    }
}

impl Overlay for GotoPicker {
    fn is_open(&self) -> bool {
        self.is_open()
    }

    fn close(&mut self) {
        self.close()
    }
}
