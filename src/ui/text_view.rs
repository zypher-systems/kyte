//! The text area: gutter, vertical and horizontal scrolling, tabs, wide
//! characters, and the terminal cursor position. Per-line styling goes through
//! `render_line` so later phases (reader mode, highlighting) can override styles.

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;

use super::theme::Theme;
use crate::app::App;
use crate::buffer::{grapheme_width, graphemes_with_char_offsets, measure};
use crate::wrap::{self, Segment};

/// Width of the gutter for `line_count` lines (`NNN │ `), 0 when hidden.
pub fn gutter_width(line_count: usize, show: bool) -> usize {
    if !show {
        return 0;
    }
    line_count.to_string().len().max(3) + 3
}

/// What a grapheme looks like on screen (control characters as control pictures).
fn display_symbol(g: &str, w: usize) -> String {
    if g == "\t" {
        return " ".repeat(w);
    }
    if let Some(c) = g.chars().find(|c| c.is_control()) {
        let pic = match c as u32 {
            n @ 0..=0x1f => char::from_u32(0x2400 + n).unwrap_or('?'),
            0x7f => '\u{2421}',
            _ => '\u{fffd}',
        };
        return pic.to_string();
    }
    if unicode_width::UnicodeWidthStr::width(g) == 0 {
        return "\u{fffd}".to_string();
    }
    g.to_string()
}

/// The visible part of `line` from display column `left`, at most `width` cells.
pub fn visible_text(line: &str, left: usize, width: usize) -> String {
    let mut out = String::new();
    let right = left + width;
    let mut walk = graphemes_with_char_offsets(line);
    let mut col = walk.skip_plain(left);
    while let Some((_, g)) = walk.next() {
        let w = grapheme_width(g, col);
        let end = col + w;
        if end <= left {
            col = end + walk.skip_plain(left - end);
            continue;
        }
        if col >= right {
            break;
        }
        if col < left || end > right {
            // Partly visible wide character or tab: blank the visible cells.
            let from = col.max(left);
            let to = end.min(right);
            out.push_str(&" ".repeat(to - from));
        } else {
            out.push_str(&display_symbol(g, w));
        }
        col = end;
    }
    out
}

/// One row of the text area.
struct ScreenRow {
    line: usize,
    segment: Segment,
    /// The last row of its line, where the line ending is.
    last: bool,
}

/// The rows on screen. Without wrap each line is one row. Its char range runs
/// on past the line's end, so working out its length isn't needed.
fn screen_rows(app: &App, height: usize, width: usize) -> Vec<ScreenRow> {
    let buf = app.view_buffer();
    let (top, _) = app.scroll();
    let count = buf.line_count();
    if !app.soft_wrap {
        let rows = top..(top + height).min(count);
        return rows
            .map(|line| ScreenRow {
                line,
                segment: Segment {
                    chars: 0..usize::MAX / 2,
                    bytes: 0..usize::MAX / 2,
                },
                last: true,
            })
            .collect();
    }
    let mut out = Vec::with_capacity(height);
    let mut skip = app.scroll_row();
    for line in top..count {
        let segments = wrap::segments(&buf.line_text(line), width);
        let n = segments.len();
        for (i, segment) in segments.into_iter().enumerate().skip(skip) {
            if out.len() == height {
                return out;
            }
            let last = i + 1 == n;
            out.push(ScreenRow {
                line,
                segment,
                last,
            });
        }
        skip = 0;
    }
    out
}

/// Draws the text area into `area` and returns the terminal cursor position.
pub fn render(app: &App, area: Rect, cells: &mut Cells, theme: &Theme) -> Option<(u16, u16)> {
    let buf = app.view_buffer();
    let (_, left) = app.scroll();
    let gw = gutter_width(buf.line_count(), app.show_line_numbers);
    let text_w = (area.width as usize).saturating_sub(gw);
    cells.set_style(area, theme.text);
    let rows = screen_rows(app, area.height as usize, text_w);
    #[cfg(feature = "highlight")]
    let first_line = rows.first().map_or(0, |r| r.line);
    #[cfg(feature = "highlight")]
    let spans = line_spans(
        app,
        first_line,
        rows.last().map_or(0, |r| r.line + 1 - first_line),
        theme,
    );
    let mut cursor = None;
    let mut full = (usize::MAX, String::new());
    for (i, row) in rows.iter().enumerate() {
        let y = area.y + i as u16;
        let line = row.line;
        if full.0 != line {
            full = (line, buf.line_text(line));
        }
        let text = &full.1;
        let bytes = row.segment.bytes.start.min(text.len())..row.segment.bytes.end.min(text.len());
        let piece = &text[bytes.clone()];
        if gw > 0 {
            let first = row.segment.chars.start == 0;
            let label = if first {
                (line + 1).to_string()
            } else {
                String::new()
            };
            let num = format!("{label:>w$} │ ", w = gw - 3);
            cells.set_stringn(area.x, y, num, gw, theme.gutter);
        }
        let x = area.x + gw as u16;
        #[cfg(feature = "highlight")]
        let styled = spans
            .get(line - first_line)
            .filter(|s| !s.is_empty())
            .map(|sp| render_line_styled(cells, x, y, piece, bytes.start, left, text_w, theme, sp));
        #[cfg(not(feature = "highlight"))]
        let styled: Option<()> = None;
        if styled.is_none() {
            render_line(cells, x, y, piece, left, text_w, theme);
        }
        let area_row = Rect::new(x, y, text_w as u16, 1);
        paint_matches(app, text, row, piece, area_row, cells, theme);
        paint_selection(app, row, piece, area_row, cells, theme);
        if let Some((ours, theirs)) = &app.review_marks {
            if ours.contains(&line) {
                cells.set_style(area_row, theme.diff_ours);
            } else if theirs.contains(&line) {
                cells.set_style(area_row, theme.diff_theirs);
            }
        }
        if app
            .reader
            .as_ref()
            .is_some_and(|r| !r.focus.contains(&line))
        {
            cells.set_style(Rect::new(area.x, y, area.width, 1), theme.reader_dim);
        }
        if line == buf.cursor_line() {
            let off = buf.cursor() - buf.line_start(line);
            let here = if app.soft_wrap {
                off >= row.segment.chars.start && (off < row.segment.chars.end || row.last)
            } else {
                true
            };
            if here {
                let col = measure(piece, off - row.segment.chars.start).0;
                if col >= left && col - left < text_w.max(1) {
                    cursor = Some((area.x + (gw + col - left) as u16, y));
                }
            }
        }
    }
    for i in rows.len()..area.height as usize {
        cells.set_stringn(area.x, area.y + i as u16, "~", 1, theme.tilde);
    }
    cursor
}

/// Highlights search matches on a row. Only the cells on screen are looked at,
/// so a long line full of matches costs no more than a short one.
fn paint_matches(
    app: &App,
    line: &str,
    row: &ScreenRow,
    piece: &str,
    area: Rect,
    cells: &mut Cells,
    theme: &Theme,
) {
    let Some(query) = app.search_query() else {
        return;
    };
    let buf = app.view_buffer();
    let matches = crate::search::line_matches(line, query);
    if matches.is_empty() {
        return;
    }
    let (_, left) = app.scroll();
    let right = left + area.width as usize;
    let cursor = buf.cursor().wrapping_sub(buf.line_start(row.line));
    let start = row.segment.chars.start;
    let mut walk = graphemes_with_char_offsets(piece);
    let mut col = walk.skip_plain(left);
    let mut next = 0;
    while let Some((off, g)) = walk.next() {
        if col >= right {
            break;
        }
        let w = grapheme_width(g, col);
        let at = start + off;
        while matches.get(next).is_some_and(|m| m.end <= at) {
            next += 1;
        }
        let Some(m) = matches.get(next) else {
            break;
        };
        if m.contains(&at) && col + w > left {
            let style = if m.start == cursor {
                theme.search_current
            } else {
                theme.search_match
            };
            let from = col.max(left);
            let to = (col + w).min(right);
            let cell = Rect::new(area.x + (from - left) as u16, area.y, (to - from) as u16, 1);
            cells.set_style(cell, style);
        }
        col += w;
        if col < left {
            col += walk.skip_plain(left - col);
        }
    }
}

/// Styles the selected part of a row. A selection running past the end of the
/// line takes one more cell, so an empty line shows as selected.
fn paint_selection(
    app: &App,
    row: &ScreenRow,
    piece: &str,
    area: Rect,
    cells: &mut Cells,
    theme: &Theme,
) {
    let buf = app.view_buffer();
    let Some(sel) = buf.selection() else {
        return;
    };
    let line_start = buf.line_start(row.line);
    let line_end = buf.line_end(row.line);
    let start = line_start + row.segment.chars.start;
    let end = (line_start + row.segment.chars.end).min(line_end);
    let past = sel.end > line_end;
    if sel.end <= start || sel.start > end || (sel.start == end && !row.last) {
        return;
    }
    let (_, left) = app.scroll();
    let from = measure(piece, sel.start.max(start) - start).0;
    let to = measure(piece, sel.end.min(end) - start).0 + usize::from(row.last && past);
    let right = left + area.width as usize;
    let (from, to) = (from.max(left), to.min(right));
    if from < to {
        let x = area.x + (from - left) as u16;
        cells.set_style(Rect::new(x, area.y, (to - from) as u16, 1), theme.selection);
    }
}

/// Draws one line of text (the single place per-line styles are applied).
pub fn render_line(
    cells: &mut Cells,
    x: u16,
    y: u16,
    line: &str,
    left: usize,
    width: usize,
    theme: &Theme,
) {
    let vis = visible_text(line, left, width);
    cells.set_stringn(x, y, vis, width, theme.text);
}

/// Highlight styles for each visible row, as byte ranges within the line.
#[cfg(feature = "highlight")]
fn line_spans(
    app: &App,
    top: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<Vec<(std::ops::Range<usize>, ratatui::style::Style)>> {
    use crate::highlight::{highlight, Lang};
    let mut per = vec![Vec::new(); rows];
    let buf = app.view_buffer();
    let head = buf.head();
    let Some(lang) = buf.path().and_then(|p| Lang::detect(p, &head)) else {
        return per;
    };
    let end = (top + rows).min(buf.line_count());
    if top >= end {
        return per;
    }
    let text = buf.to_string();
    let rope = buf.rope();
    for (r, kind) in highlight(lang, &text, top..end) {
        if r.is_empty() {
            continue;
        }
        let style = kind.style(theme.text);
        for l in rope.byte_to_line(r.start)..=rope.byte_to_line(r.end - 1) {
            if l < top || l >= end {
                continue;
            }
            let ls = rope.line_to_byte(l);
            let le = rope.line_to_byte(l + 1);
            let (a, b) = (r.start.max(ls) - ls, r.end.min(le) - ls);
            if a < b {
                per[l - top].push((a..b, style));
            }
        }
    }
    per
}

/// `render_line` with per-byte-range styles (feature `highlight`).
#[cfg(feature = "highlight")]
#[allow(clippy::too_many_arguments)]
fn render_line_styled(
    cells: &mut Cells,
    x: u16,
    y: u16,
    line: &str,
    base: usize,
    left: usize,
    width: usize,
    theme: &Theme,
    spans: &[(std::ops::Range<usize>, ratatui::style::Style)],
) {
    use unicode_segmentation::UnicodeSegmentation;
    let right = left + width;
    let mut col = 0;
    for (i, g) in line.grapheme_indices(true) {
        let w = grapheme_width(g, col);
        let end = col + w;
        if end <= left {
            col = end;
            continue;
        }
        if col >= right {
            break;
        }
        let at = (x as usize + col.max(left) - left) as u16;
        if col < left || end > right {
            let n = end.min(right) - col.max(left);
            cells.set_stringn(at, y, " ".repeat(n), n, theme.text);
        } else {
            let style = spans
                .iter()
                .find(|(r, _)| r.contains(&(base + i)))
                .map_or(theme.text, |(_, s)| *s);
            cells.set_stringn(at, y, display_symbol(g, w), w, style);
        }
        col = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_widths() {
        assert_eq!(gutter_width(5, true), 6);
        assert_eq!(gutter_width(12345, true), 8);
        assert_eq!(gutter_width(5, false), 0);
    }

    #[test]
    fn tabs_and_wide_chars() {
        assert_eq!(visible_text("a\tb", 0, 10), "a   b");
        assert_eq!(visible_text("世界x", 0, 10), "世界x");
        // left edge cuts 世 in half
        assert_eq!(visible_text("世界x", 1, 10), " 界x");
        // right edge cuts 界
        assert_eq!(visible_text("世界x", 0, 3), "世 ");
    }

    #[test]
    fn control_chars_are_pictures() {
        assert_eq!(visible_text("a\rb", 0, 10), "a\u{240d}b");
    }

    #[test]
    fn horizontal_scroll() {
        assert_eq!(visible_text("abcdefgh", 3, 3), "def");
    }

    #[test]
    fn horizontal_scroll_past_mixed_text() {
        assert_eq!(visible_text("ab世cdef", 4, 2), "cd");
        assert_eq!(visible_text("ab\tcdef", 5, 2), "de");
        assert_eq!(visible_text("abcd世ef", 5, 3), " ef");
        assert_eq!(visible_text("abc", 10, 3), "");
    }
}
