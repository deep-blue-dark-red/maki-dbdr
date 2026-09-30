use std::sync::Arc;

use arc_swap::ArcSwap;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::components::Overlay;
use crate::components::keybindings::key;
use crate::components::modal::Modal;
use crate::components::prompt_view::PromptViewModal;

const TITLE: &str = " Tool prompt — enabled tools ";
pub(crate) const WIDTH_PERCENT: u16 = 90;
const MAX_HEIGHT_PERCENT: u16 = 90;
const HINT_PAIRS: &[(&str, &str)] = &[("esc", "close")];

/// Live view of the tool instructions the model is offered, one section per
/// enabled tool. Like [`crate::components::SystemPromptModal`], it reads the
/// agent's published snapshot each frame, so a model switch or config change
/// is reflected without reopening.
pub struct ToolPromptModal(PromptViewModal);

impl ToolPromptModal {
    pub fn new() -> Self {
        Self(PromptViewModal::new(
            Modal {
                title: TITLE,
                width_percent: WIDTH_PERCENT,
                max_height_percent: MAX_HEIGHT_PERCENT,
            },
            HINT_PAIRS,
        ))
    }

    pub fn open(&mut self, source: Arc<ArcSwap<String>>) {
        self.0.open(source);
    }

    pub fn is_open(&self) -> bool {
        self.0.is_open()
    }

    pub fn close(&mut self) {
        self.0.close();
    }

    pub fn scroll(&mut self, delta: i32) {
        self.0.scroll(delta);
    }

    pub fn handle_key(&mut self, key_event: KeyEvent) {
        if key_event.code == KeyCode::Esc || key::QUIT.matches(key_event) {
            self.close();
            return;
        }
        self.0.handle_scroll_key(key_event);
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        self.0.view(frame, area)
    }
}

impl Overlay for ToolPromptModal {
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
    use crate::components::key as key_ev;
    use crossterm::event::KeyCode;

    fn modal() -> ToolPromptModal {
        let mut modal = ToolPromptModal::new();
        modal.open(Arc::new(ArcSwap::from_pointee(String::from("body"))));
        modal
    }

    #[test]
    fn esc_closes() {
        let mut modal = modal();
        modal.handle_key(key_ev(KeyCode::Esc));
        assert!(!modal.is_open());
    }

    #[test]
    fn scroll_keys_do_not_close() {
        let mut modal = modal();
        modal.handle_key(key_ev(KeyCode::Down));
        assert!(modal.is_open());
    }
}
