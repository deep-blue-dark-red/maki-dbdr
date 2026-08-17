use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use std::ops::Range;

pub(super) struct RenderCursor {
    skip: u16,
    y: u16,
    bottom: u16,
    viewport: Rect,
}

impl RenderCursor {
    pub fn new(scroll_top: u16, viewport: Rect) -> Self {
        Self {
            skip: scroll_top,
            y: viewport.y,
            bottom: viewport.y + viewport.height,
            viewport,
        }
    }

    pub fn past_bottom(&self) -> bool {
        self.y >= self.bottom
    }

    /// Paints one block. `window` reports which source lines cover the rows
    /// about to be drawn, and how far into the first of them to start.
    ///
    /// Only those lines reach `Paragraph`, because ratatui's `&[Line]` ->
    /// `Text` conversion clones every line and span it is given. Handing it
    /// the whole block would deep-copy a segment's worth of strings per frame
    /// to paint a viewport's worth of rows.
    pub fn render(
        &mut self,
        lines: &[Line<'static>],
        h: u16,
        window: impl FnOnce(u16, u16) -> (Range<usize>, u16),
        style: Option<Style>,
        highlight: bool,
        frame: &mut Frame,
    ) {
        if self.skip >= h {
            self.skip -= h;
            return;
        }
        if self.y >= self.bottom {
            return;
        }
        let visible_h = h
            .saturating_sub(self.skip)
            .min(self.bottom.saturating_sub(self.y));
        let seg_area = Rect::new(self.viewport.x, self.y, self.viewport.width, visible_h);
        let (range, residual) = window(self.skip, visible_h);
        self.skip = 0;
        self.y += visible_h;
        let Some(visible) = lines.get(range) else {
            return;
        };
        let mut p = Paragraph::new(visible).wrap(Wrap { trim: false });
        let mut base = style.unwrap_or_default();
        if highlight {
            base = base.add_modifier(Modifier::REVERSED);
        }
        p = p.style(base);
        if residual > 0 {
            p = p.scroll((residual, 0));
        }
        frame.render_widget(p, seg_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::wrap::{WrapIndex, wrapped_line_count};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use test_case::test_case;

    const WIDTH: u16 = 12;
    const HEIGHT: u16 = 6;

    /// The pre-slicing renderer: hand ratatui every line and let it skip
    /// `skip` wrapped rows itself. Correct but O(block) per frame, which is
    /// what the window logic exists to avoid.
    fn paint_whole(lines: &[Line<'static>], skip: u16, h: u16) -> Buffer {
        paint(lines, skip, h, |_, _| (0..lines.len(), skip))
    }

    fn paint_windowed(lines: &[Line<'static>], skip: u16, h: u16) -> Buffer {
        let index = WrapIndex::build(lines, WIDTH);
        paint(lines, skip, h, |s, rows| index.window(s, rows))
    }

    fn paint(
        lines: &[Line<'static>],
        skip: u16,
        h: u16,
        window: impl FnOnce(u16, u16) -> (Range<usize>, u16),
    ) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
        terminal
            .draw(|frame| {
                let viewport = Rect::new(0, 0, WIDTH, HEIGHT);
                RenderCursor::new(skip, viewport).render(lines, h, window, None, false, frame);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn lines_of(texts: &[&str]) -> Vec<Line<'static>> {
        texts.iter().map(|t| Line::raw(t.to_string())).collect()
    }

    /// Slicing to the visible window must be invisible: whatever ratatui
    /// would have drawn from the whole block, it still draws from the slice.
    #[test_case(&["alpha beta gamma delta", "solo", "one two three four"] ; "wrapping_lines")]
    #[test_case(&["a", "b", "c", "d", "e", "f", "g", "h"] ; "many_short_lines")]
    #[test_case(&["", "text", "", "more"] ; "blank_lines")]
    #[test_case(&["a very long single line that wraps several times over"] ; "one_long_line")]
    fn windowed_paint_matches_whole_paint_at_every_scroll(texts: &[&str]) {
        let lines = lines_of(texts);
        let h = wrapped_line_count(&lines, WIDTH);
        for skip in 0..=h {
            assert_eq!(
                paint_windowed(&lines, skip, h),
                paint_whole(&lines, skip, h),
                "scroll offset {skip} of {h}"
            );
        }
    }

    #[test]
    fn empty_block_paints_nothing() {
        assert_eq!(paint_windowed(&[], 0, 0), paint_whole(&[], 0, 0));
    }
}
