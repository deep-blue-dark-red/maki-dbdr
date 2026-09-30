use std::sync::Arc;

use arc_swap::ArcSwap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::components::Overlay;
use crate::components::keybindings::key;
use crate::components::modal::Modal;
use crate::components::prompt_view::PromptViewModal;

const TITLE: &str = " System prompt — resolved ";
pub(crate) const WIDTH_PERCENT: u16 = 90;
const MAX_HEIGHT_PERCENT: u16 = 90;
const HINT_PAIRS: &[(&str, &str)] = &[
    ("e", "template"),
    ("i", "identity"),
    ("t", "tone"),
    ("p", "instructions"),
    ("a", "sources"),
    ("esc", "close"),
];

pub enum SystemPromptAction {
    None,
    Edit,
    EditIdentity,
    EditTone,
    PickInstructions,
    OpenAfterSources,
}

/// Live view of the prompt as the agent assembled it: template slots filled,
/// plugin hints inlined, environment and instructions appended. It reads the
/// agent's published snapshot each frame, so a rebuild (next turn, or after
/// editing an override file) is reflected without reopening. Editing hands
/// each part to `$EDITOR`: `e` the `system.md` template, `i`/`t` the
/// identity/tone overrides, `p` a contributing instructions file, `a` the
/// plugin source behind `{{after_instructions}}`.
pub struct SystemPromptModal(PromptViewModal);

impl SystemPromptModal {
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

    /// The currently published resolved prompt, if any.
    #[cfg(test)]
    pub fn resolved_text(&self) -> Option<String> {
        self.0.resolved_text()
    }

    pub fn handle_key(&mut self, key_event: KeyEvent) -> SystemPromptAction {
        // Checked before the scroll keys it shadows: Ctrl+E is line-down
        // everywhere else, and here it means edit.
        if key_event.code == KeyCode::Char('e') {
            let ctrl = key_event.modifiers == KeyModifiers::CONTROL;
            self.close();
            return if ctrl || key_event.modifiers.is_empty() {
                SystemPromptAction::Edit
            } else {
                SystemPromptAction::None
            };
        }
        let action = match key_event.code {
            KeyCode::Char('i') => SystemPromptAction::EditIdentity,
            KeyCode::Char('t') => SystemPromptAction::EditTone,
            KeyCode::Char('p') => SystemPromptAction::PickInstructions,
            KeyCode::Char('a') => SystemPromptAction::OpenAfterSources,
            KeyCode::Esc => SystemPromptAction::None,
            _ if key::QUIT.matches(key_event) => SystemPromptAction::None,
            _ => {
                self.0.handle_scroll_key(key_event);
                return SystemPromptAction::None;
            }
        };
        self.close();
        action
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        self.0.view(frame, area)
    }
}

impl Overlay for SystemPromptModal {
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

    const PROMPT: &str = "You are Maki.\n\n# Tone\nBe concise.";

    fn modal() -> SystemPromptModal {
        let mut modal = SystemPromptModal::new();
        modal.open(Arc::new(ArcSwap::from_pointee(PROMPT.to_string())));
        modal
    }

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn ctrl_e_requests_edit() {
        let mut modal = modal();
        let edit = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert!(matches!(modal.handle_key(edit), SystemPromptAction::Edit));
        assert!(!modal.is_open());
    }

    #[test]
    fn edit_keys_map_to_actions_and_close() {
        for (c, expected) in [
            ('e', SystemPromptAction::Edit),
            ('i', SystemPromptAction::EditIdentity),
            ('t', SystemPromptAction::EditTone),
            ('p', SystemPromptAction::PickInstructions),
            ('a', SystemPromptAction::OpenAfterSources),
        ] {
            let mut modal = modal();
            assert!(
                std::mem::discriminant(&modal.handle_key(plain(c)))
                    == std::mem::discriminant(&expected),
                "key {c}"
            );
            assert!(!modal.is_open(), "key {c} must close");
        }
    }

    #[test]
    fn esc_closes_without_editing() {
        let mut modal = modal();
        assert!(matches!(
            modal.handle_key(key_ev(KeyCode::Esc)),
            SystemPromptAction::None
        ));
        assert!(!modal.is_open());
    }

    #[test]
    fn view_reflects_snapshot_updates() {
        let source = Arc::new(ArcSwap::from_pointee(PROMPT.to_string()));
        let mut modal = SystemPromptModal::new();
        modal.open(Arc::clone(&source));
        source.store(Arc::new("updated".to_string()));
        assert_eq!(modal.resolved_text().as_deref(), Some("updated"));
    }

    /// Ctrl+E is line-down for the rest of the app; only here must it edit.
    #[test]
    fn plain_ctrl_e_still_scrolls_elsewhere_is_overridden_here() {
        let mut modal = SystemPromptModal::new();
        let long = format!("{}\n{}", PROMPT, "line\n".repeat(50));
        modal.open(Arc::new(ArcSwap::from_pointee(long)));
        let edit = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert!(matches!(modal.handle_key(edit), SystemPromptAction::Edit));
    }
}
