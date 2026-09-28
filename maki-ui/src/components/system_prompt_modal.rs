use std::sync::Arc;

use arc_swap::ArcSwap;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::components::ModalScroll;
use crate::components::Overlay;
use crate::components::hint_line;
use crate::components::keybindings::key;
use crate::components::modal::Modal;
use crate::components::scrollbar::render_vertical_scrollbar;
use crate::markdown::text_to_lines;
use crate::theme;

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
pub struct SystemPromptModal {
    open: bool,
    scroll: ModalScroll,
    source: Option<Arc<ArcSwap<String>>>,
}

impl SystemPromptModal {
    pub fn new() -> Self {
        Self {
            open: false,
            scroll: ModalScroll::new_top(),
            source: None,
        }
    }

    pub fn open(&mut self, source: Arc<ArcSwap<String>>) {
        self.source = Some(source);
        self.open = true;
        self.scroll = ModalScroll::new_top();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
        self.scroll.reset();
    }

    pub fn scroll(&mut self, delta: i32) {
        self.scroll.scroll(delta);
    }

    /// The currently published resolved prompt, if any.
    pub fn resolved_text(&self) -> Option<String> {
        self.source.as_ref().map(|source| source.load().to_string())
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
                self.scroll.handle_key(key_event);
                return SystemPromptAction::None;
            }
        };
        self.close();
        action
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        if !self.open {
            return Rect::default();
        }
        let Some(source) = self.source.as_ref() else {
            return Rect::default();
        };
        let text = source.load();

        let modal = Modal {
            title: TITLE,
            width_percent: WIDTH_PERCENT,
            max_height_percent: MAX_HEIGHT_PERCENT,
        };
        // The chrome height is the wrapped line count, and the wrap width is
        // what the chrome leaves. Probe once for the width, then draw: the
        // real frame is taller than the probe, so its `Clear` covers it.
        let (_, probe) = modal.render(frame, area, 0);
        let lines = self.build_lines(&text, probe.width);
        let total = lines.len() as u16;
        let (popup, inner) = modal.render(frame, area, total);

        let viewport_h = inner.height;
        self.scroll.update_dimensions(total, viewport_h);
        let scroll = self.scroll.offset();

        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);

        if total > viewport_h {
            render_vertical_scrollbar(frame, inner, u32::from(total), u32::from(scroll));
        }

        popup
    }

    fn build_lines(&self, prompt: &str, width: u16) -> Vec<Line<'static>> {
        let t = theme::current();
        let mut lines = vec![hint_line(HINT_PAIRS), Line::default()];
        lines.extend(text_to_lines(prompt, "", t.item, t.item, width, None));
        lines
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
