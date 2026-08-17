//! Cumulative wrapped-row offsets for a block of lines.
//!
//! Ratatui has no way to paint a borrowed slice of lines: `Paragraph::new`
//! takes `impl Into<Text>`, and the `&[Line]` impl clones every line, span
//! and owned string. Painting a segment therefore costs a deep copy of the
//! whole segment, however little of it is on screen, once per frame.
//!
//! Knowing where each source line lands after wrapping turns that into a
//! copy of just the visible window. The offsets only change when the lines
//! or the width do, so they are built once and reused across frames.

use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

/// Display rows a block of lines occupies once wrapped to `width`.
pub(crate) fn wrapped_line_count(lines: &[Line<'_>], width: u16) -> u16 {
    if width == 0 {
        return lines.len() as u16;
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .line_count(width) as u16
}

/// Where each source line ends up after wrapping.
///
/// `ends[i]` is the display row just past source line `i`, so `ends` is
/// non-decreasing and its last entry is the block's total height. Ratatui
/// wraps each `Line` independently, so per-line counts sum to the count of
/// the whole slice; `wrap_index_total_matches_whole_slice` pins that down.
pub(crate) struct WrapIndex {
    at_width: u16,
    line_count: usize,
    ends: Vec<u16>,
}

impl WrapIndex {
    pub fn build(lines: &[Line<'_>], width: u16) -> Self {
        let mut ends = Vec::with_capacity(lines.len());
        let mut acc = 0u16;
        for line in lines {
            acc = acc.saturating_add(wrapped_line_count(std::slice::from_ref(line), width));
            ends.push(acc);
        }
        Self {
            at_width: width,
            line_count: lines.len(),
            ends,
        }
    }

    /// Whether this index still describes `lines` at `width`. Line count is
    /// a guard against a stale index, not a content hash: every mutator that
    /// can change wrapping without changing the count rebuilds explicitly.
    pub fn describes(&self, lines: &[Line<'_>], width: u16) -> bool {
        self.at_width == width && self.line_count == lines.len()
    }

    pub fn total(&self) -> u16 {
        self.ends.last().copied().unwrap_or(0)
    }

    /// Source line containing display row `rel_row`, or `None` past the end.
    pub fn source_line_at(&self, rel_row: u16) -> Option<usize> {
        let i = self.ends.partition_point(|&end| end <= rel_row);
        (i < self.ends.len()).then_some(i)
    }

    /// The source lines a painter must hand to ratatui to fill `rows` display
    /// rows starting `skip` rows into the block, and how far into the first of
    /// them the paint begins.
    ///
    /// The residual is why the range cannot simply start at `skip`: a source
    /// line that wraps may be entered partway, and ratatui only skips whole
    /// wrapped rows via `Paragraph::scroll`.
    pub fn window(&self, skip: u16, rows: u16) -> (std::ops::Range<usize>, u16) {
        if self.ends.is_empty() || rows == 0 {
            return (0..0, 0);
        }
        let start = self.ends.partition_point(|&end| end <= skip);
        if start >= self.ends.len() {
            return (0..0, 0);
        }
        let consumed = if start == 0 { 0 } else { self.ends[start - 1] };
        let residual = skip.saturating_sub(consumed);
        let last_row = skip.saturating_add(rows.saturating_sub(1));
        let end = self
            .ends
            .partition_point(|&e| e <= last_row)
            .saturating_add(1)
            .min(self.ends.len());
        (start..end, residual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    fn lines(texts: &[&str]) -> Vec<Line<'static>> {
        texts.iter().map(|t| Line::raw(t.to_string())).collect()
    }

    /// The whole point of building the index per line: if per-line counts did
    /// not sum to the whole-slice count, every scroll offset derived from the
    /// index would drift against the height ratatui actually paints.
    #[test_case(&["short", "also short"], 40 ; "no_wrapping")]
    #[test_case(&["a b c d e f g h i j k l", "m n o p"], 8 ; "word_wrapped")]
    #[test_case(&["", "text", ""], 10 ; "blank_lines")]
    #[test_case(&["one two three four five six seven"], 5 ; "single_long_line")]
    fn wrap_index_total_matches_whole_slice(texts: &[&str], width: u16) {
        let ls = lines(texts);
        assert_eq!(
            WrapIndex::build(&ls, width).total(),
            wrapped_line_count(&ls, width)
        );
    }

    #[test]
    fn window_covers_exactly_the_requested_rows() {
        // 4 rows, 1 row, 4 rows.
        let ls = lines(&["a b c d", "x", "e f g h"]);
        let idx = WrapIndex::build(&ls, 2);
        assert_eq!(idx.total(), 9);

        // Starting inside the first line keeps it, with a residual.
        assert_eq!(idx.window(2, 3), (0..2, 2));
        // Starting exactly at a boundary takes no residual.
        assert_eq!(idx.window(4, 1), (1..2, 0));
        // A window spanning all three lines asks for all three.
        assert_eq!(idx.window(3, 6), (0..3, 3));
        // Past the end paints nothing.
        assert_eq!(idx.window(9, 4), (0..0, 0));
    }

    #[test]
    fn window_of_zero_rows_is_empty() {
        let ls = lines(&["a", "b"]);
        assert_eq!(WrapIndex::build(&ls, 10).window(0, 0), (0..0, 0));
    }

    #[test]
    fn source_line_at_maps_wrapped_rows_back() {
        let ls = lines(&["a b c d", "x"]);
        let idx = WrapIndex::build(&ls, 2);
        assert_eq!(idx.source_line_at(0), Some(0));
        assert_eq!(idx.source_line_at(3), Some(0));
        assert_eq!(idx.source_line_at(4), Some(1));
        assert_eq!(idx.source_line_at(5), None);
    }

    #[test]
    fn describes_rejects_a_different_width_or_line_count() {
        let ls = lines(&["a", "b"]);
        let idx = WrapIndex::build(&ls, 10);
        assert!(idx.describes(&ls, 10));
        assert!(!idx.describes(&ls, 11));
        assert!(!idx.describes(&lines(&["a"]), 10));
    }
}
