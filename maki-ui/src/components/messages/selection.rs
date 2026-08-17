use super::segment::{Segment, SegmentCache};
use crate::selection::{self, LineBreaks, ScreenSelection, Selection};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Paragraph, Widget, Wrap};

/// What a selection is scraped from: painted cells, plus the line-break
/// bitmap that decides where newlines and re-inserted wrap spaces land in the
/// copied text, plus the buffer row that bitmap's bit 0 describes.
struct Scrape {
    buf: Buffer,
    area: Rect,
    breaks: LineBreaks,
    breaks_origin: u16,
}

/// Paints just the rows a selection touches into a buffer addressed at the
/// segment's own row numbers.
///
/// The area starts at `skip` rather than 0, so the buffer is only `rows` tall
/// while its cells still answer to absolute segment rows. Copying three rows
/// out of a 5000-line tool output otherwise meant allocating the whole
/// segment's worth of cells and deep-cloning every `Line` to paint rows nobody
/// asked for — ~10ms per segment per copy against ~10us for the window.
///
/// The breaks are cut to the same line range for the same reason: built over
/// the whole segment they cost as much as the discarded paint did. `residual`
/// is how far into the range's first line the window starts, so the range
/// begins at display row `skip - residual` and that is where bit 0 sits.
fn scrape_window(seg: &Segment, skip: u16, rows: u16, width: u16) -> Scrape {
    let area = Rect::new(0, skip, width, rows);
    let mut buf = Buffer::empty(area);
    let (range, residual) = seg.window(skip, rows, width);
    let Some(visible) = seg.lines().get(range) else {
        return Scrape {
            buf,
            area,
            breaks: LineBreaks::default(),
            breaks_origin: skip,
        };
    };
    let mut p = Paragraph::new(visible).wrap(Wrap { trim: false });
    if residual > 0 {
        p = p.scroll((residual, 0));
    }
    p.render(area, &mut buf);
    Scrape {
        buf,
        area,
        breaks: LineBreaks::from_lines(visible, width),
        breaks_origin: skip.saturating_sub(residual),
    }
}

pub(super) fn extract_selection_text(
    cache: &SegmentCache,
    viewport_width: u16,
    sel: &Selection,
    msg_area: Rect,
) -> String {
    extract_with(cache, viewport_width, sel, msg_area, scrape_window)
}

/// `scrape` is a parameter only so tests can swap in the whole-segment scrape
/// and check the window against it; production always uses `scrape_window`.
fn extract_with(
    cache: &SegmentCache,
    viewport_width: u16,
    sel: &Selection,
    msg_area: Rect,
    scrape: impl Fn(&Segment, u16, u16, u16) -> Scrape,
) -> String {
    let (doc_start, doc_end) = sel.normalized();
    let width = viewport_width;

    let heights: Vec<u16> = cache.segments().iter().map(|s| s.height(width)).collect();

    let mut out = String::new();
    let mut doc_row: u32 = 0;

    for (i, &h) in heights.iter().enumerate() {
        let seg_start = doc_row;
        let seg_end = doc_row + h as u32;
        doc_row = seg_end;

        if seg_end <= doc_start.row || seg_start > doc_end.row {
            continue;
        }

        if !out.is_empty() {
            out.push('\n');
        }

        let Some(seg) = cache.get(i) else { continue };

        if seg.lines().is_empty() {
            continue;
        }

        let rel_start = doc_start.row.saturating_sub(seg_start) as u16;
        let rel_end = ((doc_end.row + 1).saturating_sub(seg_start) as u16).min(h);
        let rows = rel_end.saturating_sub(rel_start);
        if rows == 0 {
            continue;
        }

        let start_col = if seg_start > doc_start.row {
            0
        } else {
            doc_start.col.saturating_sub(msg_area.x)
        };
        let end_col = if seg_end < doc_end.row + 1 {
            width.saturating_sub(1)
        } else {
            doc_end.col.saturating_sub(msg_area.x)
        };

        let ss = ScreenSelection {
            start_row: rel_start,
            start_col,
            end_row: rel_end.saturating_sub(1),
            end_col,
        };

        let s = scrape(seg, rel_start, rows, width);
        selection::append_rows(
            &s.buf,
            s.area,
            &ss,
            rel_start..rel_end,
            &mut out,
            &s.breaks,
            s.breaks_origin,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::SelectionZone;
    use ratatui::text::Line;
    use test_case::test_case;

    /// The pre-windowing scrape: render every row of the segment into a buffer
    /// rooted at row 0 and index breaks over every line. Correct but
    /// O(segment) per copy, which is what `scrape_window` exists to avoid.
    fn scrape_whole(seg: &Segment, _skip: u16, _rows: u16, width: u16) -> Scrape {
        let area = Rect::new(0, 0, width, seg.height(width));
        let mut buf = Buffer::empty(area);
        Paragraph::new(seg.lines().to_vec())
            .wrap(Wrap { trim: false })
            .render(area, &mut buf);
        Scrape {
            buf,
            area,
            breaks: LineBreaks::from_lines(seg.lines(), width),
            breaks_origin: 0,
        }
    }

    fn cache_of(segments: &[&[&str]]) -> SegmentCache {
        let mut cache = SegmentCache::new();
        for lines in segments {
            let ls: Vec<Line<'static>> = lines.iter().map(|t| Line::raw(t.to_string())).collect();
            cache.push(Segment::with_lines(ls, lines.join("\n"), None));
        }
        cache
    }

    /// A doc-space selection from `start` to `end` inclusive. The area is one
    /// row tall so both endpoints come straight from the scroll offset.
    fn selection_over(start: u32, end: u32, width: u16, cols: (u16, u16)) -> Selection {
        let area = Rect::new(0, 0, width, 1);
        let mut sel = Selection::start(0, cols.0, area, SelectionZone::Messages, start);
        sel.update(0, cols.1, end);
        sel
    }

    /// Windowing is only a speed-up if it is invisible: whatever the whole-
    /// segment paint put on the clipboard, the windowed paint must put there
    /// too — at every start row, every end row, and across segment borders.
    #[test_case(&[&["alpha beta gamma delta", "solo"], &["one two three four five"]] ; "wrapping_across_segments")]
    #[test_case(&[&["a", "b", "c"], &["d", "e"], &["f"]] ; "many_short_segments")]
    #[test_case(&[&["", "text", ""], &["more"]] ; "blank_lines")]
    #[test_case(&[&["a single line that wraps a great many times over indeed"]] ; "one_long_line")]
    #[test_case(&[&["你好世界 mixed 漢字"], &["ascii tail"]] ; "wide_chars")]
    #[test_case(&[&["hello world"], &["ok"], &["foo bar baz qux"]] ; "word_wraps_needing_spaces")]
    // Char wraps then word wraps inside one line, so the row that does and
    // does not take a re-inserted space differ — a shifted breaks origin
    // moves the space rather than just the newlines.
    #[test_case(&[&["abcdefghijklmnopqrs tuv wxyz abc", "tail"]] ; "char_wrap_then_word_wrap")]
    fn windowed_copy_matches_whole_segment_copy(segments: &[&[&str]]) {
        const WIDTH: u16 = 12;
        let cache = cache_of(segments);
        let total = cache.total_height(WIDTH);
        let area = Rect::new(0, 0, WIDTH, total as u16);

        for start in 0..total {
            for end in start..total {
                for cols in [(0, WIDTH - 1), (2, WIDTH - 3), (0, 4)] {
                    let sel = selection_over(start, end, WIDTH, cols);
                    assert_eq!(
                        extract_selection_text(&cache, WIDTH, &sel, area),
                        extract_with(&cache, WIDTH, &sel, area, scrape_whole),
                        "rows {start}..={end}, cols {cols:?}"
                    );
                }
            }
        }
    }
    /// A tall segment is the case the window is for: the copied text must not
    /// depend on how much unselected content sits above it.
    #[test]
    fn copy_from_a_tall_segment_reads_the_selected_rows() {
        const WIDTH: u16 = 20;
        let lines: Vec<String> = (0..2000).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let cache = cache_of(&[&refs]);
        let area = Rect::new(0, 0, WIDTH, 24);

        let sel = selection_over(1997, 1999, WIDTH, (0, WIDTH - 1));
        assert_eq!(
            extract_selection_text(&cache, WIDTH, &sel, area),
            "line 1997\nline 1998\nline 1999"
        );
    }
}
