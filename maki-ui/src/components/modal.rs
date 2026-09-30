use crate::components::ModalScroll;
use crate::components::scrollbar::render_vertical_scrollbar;
use crate::theme;

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

pub const CHROME_LINES: u16 = 2;

pub struct Modal<'a> {
    pub title: &'a str,
    pub width_percent: u16,
    pub max_height_percent: u16,
}

impl Modal<'_> {
    pub fn render(&self, frame: &mut Frame, area: Rect, content_height: u16) -> (Rect, Rect) {
        let max_h = (area.height as u32 * self.max_height_percent as u32 / 100) as u16;
        let total_h = (content_height + CHROME_LINES)
            .min(max_h)
            .max(CHROME_LINES + 1);

        let [popup] = Layout::vertical([Constraint::Length(total_h)])
            .flex(Flex::Center)
            .areas(area);
        let [popup] = Layout::horizontal([Constraint::Percentage(self.width_percent)])
            .flex(Flex::Center)
            .areas(popup);

        frame.render_widget(Clear, popup);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::current().panel_border)
            .title(self.title)
            .title_style(theme::current().panel_title)
            .style(Style::new().bg(theme::current().background));

        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        (popup, inner)
    }

    /// Draws the scrollable modal in one pass: chrome sized to the content,
    /// scroll state synced to what fits, body, then the scrollbar if the
    /// body overflows. Returns `(popup, inner)` like [`Self::render`].
    pub fn render_lines(
        &self,
        frame: &mut Frame,
        area: Rect,
        lines: Vec<Line<'static>>,
        scroll: &mut ModalScroll,
    ) -> (Rect, Rect) {
        let total = lines.len() as u16;
        let (popup, inner) = self.render(frame, area, total);
        let viewport_h = inner.height;
        scroll.update_dimensions(total, viewport_h);
        let offset = scroll.offset();
        frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), inner);
        if total > viewport_h {
            render_vertical_scrollbar(frame, inner, u32::from(total), u32::from(offset));
        }
        (popup, inner)
    }
}
