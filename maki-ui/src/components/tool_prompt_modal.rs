use std::sync::Arc;

use arc_swap::ArcSwap;
use crossterm::event::{KeyCode, KeyEvent};
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

const TITLE: &str = " Tool prompt — enabled tools ";
pub(crate) const WIDTH_PERCENT: u16 = 90;
const MAX_HEIGHT_PERCENT: u16 = 90;
const HINT_PAIRS: &[(&str, &str)] = &[("esc", "close")];

/// Live view of the tool instructions the model is offered, one section per
/// enabled tool. Like [`crate::components::SystemPromptModal`], it reads the
/// agent's published snapshot each frame, so a model switch or config change
/// is reflected without reopening.
pub struct ToolPromptModal {
    open: bool,
    scroll: ModalScroll,
    source: Option<Arc<ArcSwap<String>>>,
}

impl ToolPromptModal {
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

    pub fn handle_key(&mut self, key_event: KeyEvent) {
        if key_event.code == KeyCode::Esc || key::QUIT.matches(key_event) {
            self.close();
            return;
        }
        self.scroll.handle_key(key_event);
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
