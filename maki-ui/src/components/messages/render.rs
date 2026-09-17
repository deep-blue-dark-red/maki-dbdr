use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui_image::{
    Image,
    picker::Picker,
    sliced::{SignedPosition, SlicedImage, SlicedProtocol},
};
use std::ops::Range;

use crate::terminal_image::InlineImage;

pub(super) struct RenderCursor {
    skip: u16,
    y: u16,
    bottom: u16,
    viewport: Rect,
}

impl RenderCursor {
    /// `skip` is the number of rows to drop from the first segment drawn, not
    /// a document offset.
    pub fn new(skip: u16, viewport: Rect) -> Self {
        Self {
            skip,
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
    /// `visible` is false while an overlay covers the transcript. The encoded
    /// protocol is kept either way: releasing it here would re-decode and
    /// re-transmit every image each time a permission prompt opens and closes.
    pub fn render_image(
        &mut self,
        image: &mut InlineImage,
        picker: Option<&Picker>,
        visible: bool,
        frame: &mut Frame,
    ) {
        if self.past_bottom() {
            return;
        }
        // Ask for the pixels before measuring: an image with no fallback row is
        // zero rows tall until its protocol lands, and the check below reads
        // zero rows as scrolled past, so it would never get around to asking.
        if let Some(picker) = picker.filter(|_| visible) {
            image.prepare(picker, self.viewport.width);
        }
        let height = image.height();
        if self.skip >= height {
            self.skip -= height;
            return;
        }
        let Some(protocol) = image.protocol(self.viewport.width) else {
            let fallback = image.fallback().map(Line::from);
            let lines: &[Line<'static>] = fallback.as_slice();
            self.render(
                lines,
                height,
                |skip, _| (0..lines.len(), skip),
                None,
                false,
                frame,
            );
            return;
        };
        let visible_rows = height
            .saturating_sub(self.skip)
            .min(self.bottom.saturating_sub(self.y));
        let area = Rect::new(self.viewport.x, self.y, self.viewport.width, visible_rows);
        if let SlicedProtocol::Sliced(rows) = protocol {
            // Not `SlicedImage::new`: upstream renders `.skip(skip).take(len - drop)`
            // rows into `area`, which is `skip` rows too many when an image is
            // clipped at the top and the bottom at once, so it draws past `area`
            // into the segments below. Placing each row ourselves cannot overdraw.
            for (offset, row) in rows
                .iter()
                .skip(self.skip as usize)
                .take(visible_rows as usize)
                .enumerate()
            {
                frame.render_widget(
                    Image::new(row),
                    Rect::new(area.x, area.y + offset as u16, area.width, 1),
                );
            }
        } else {
            let position = SignedPosition::from((0, -(self.skip as i16)));
            frame.render_widget(SlicedImage::new(protocol, position), area);
        }
        self.skip = 0;
        self.y += visible_rows;
    }

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
