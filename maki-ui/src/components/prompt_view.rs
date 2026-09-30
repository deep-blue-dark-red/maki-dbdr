use std::sync::Arc;

use arc_swap::ArcSwap;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::components::ModalScroll;
use crate::components::hint_line;
use crate::components::modal::Modal;
use crate::markdown::text_to_lines;
use crate::theme;

pub(crate) struct PromptViewModal {
    config: Modal<'static>,
    hints: &'static [(&'static str, &'static str)],
    open: bool,
    scroll: ModalScroll,
    source: Option<Arc<ArcSwap<String>>>,
}

impl PromptViewModal {
    pub(crate) fn new(
        config: Modal<'static>,
        hints: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self {
            config,
            hints,
            open: false,
            scroll: ModalScroll::new_top(),
            source: None,
        }
    }

    pub(crate) fn open(&mut self, source: Arc<ArcSwap<String>>) {
        self.source = Some(source);
        self.open = true;
        self.scroll = ModalScroll::new_top();
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.scroll.reset();
    }

    pub(crate) fn scroll(&mut self, delta: i32) {
        self.scroll.scroll(delta);
    }

    pub(crate) fn handle_scroll_key(&mut self, key_event: KeyEvent) {
        self.scroll.handle_key(key_event);
    }

    /// The currently published resolved prompt, if any.
    #[cfg(test)]
    pub(crate) fn resolved_text(&self) -> Option<String> {
        self.source.as_ref().map(|source| source.load().to_string())
    }

    pub(crate) fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        if !self.open {
            return Rect::default();
        }
        let Some(source) = self.source.as_ref() else {
            return Rect::default();
        };
        let text = source.load();

        // The chrome height is the wrapped line count, and the wrap width is
        // what the chrome leaves. Probe once for the width, then draw: the
        // real frame is taller than the probe, so its `Clear` covers it.
        let (_, probe) = self.config.render(frame, area, 0);
        let lines = self.build_lines(&text, probe.width);
        self.config
            .render_lines(frame, area, lines, &mut self.scroll)
            .0
    }

    fn build_lines(&self, prompt: &str, width: u16) -> Vec<Line<'static>> {
        let t = theme::current();
        let mut lines = vec![hint_line(self.hints), Line::default()];
        lines.extend(text_to_lines(prompt, "", t.item, t.item, width, None));
        lines
    }
}
