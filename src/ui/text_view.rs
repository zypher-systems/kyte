//! The text area: gutter, vertical and horizontal scrolling, tabs, wide
//! characters, and the terminal cursor position. Per-line styling goes through
//! `render_line` so later phases (reader mode, highlighting) can override styles.

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;

use super::theme::Theme;
use crate::app::App;
use crate::buffer::{grapheme_width, graphemes_with_char_offsets};

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

/// Draws the text area into `area` and returns the terminal cursor position.
pub fn render(app: &App, area: Rect, cells: &mut Cells, theme: &Theme) -> Option<(u16, u16)> {
    let buf = app.view_buffer();
    let (top, left) = app.scroll();
    let gw = gutter_width(buf.line_count(), app.show_line_numbers);
    let text_w = (area.width as usize).saturating_sub(gw);
    cells.set_style(area, theme.text);
    #[cfg(feature = "highlight")]
    let spans = line_spans(app, top, area.height as usize, theme);
    for row in 0..area.height as usize {
        let y = area.y + row as u16;
        let line = top + row;
        if line < buf.line_count() {
            if gw > 0 {
                let num = format!("{:>w$} │ ", line + 1, w = gw - 3);
                cells.set_stringn(area.x, y, num, gw, theme.gutter);
            }
            let text = buf.line_text(line);
            #[cfg(feature = "highlight")]
            let styled = spans.get(row).filter(|s| !s.is_empty()).map(|sp| {
                render_line_styled(cells, area.x + gw as u16, y, &text, left, text_w, theme, sp)
            });
            #[cfg(not(feature = "highlight"))]
            let styled: Option<()> = None;
            if styled.is_none() {
                render_line(cells, area.x + gw as u16, y, &text, left, text_w, theme);
            }
            let row = Rect::new(area.x + gw as u16, y, text_w as u16, 1);
            paint_matches(app, line, row, cells, theme);
            paint_selection(
                app,
                line,
                Rect::new(area.x + gw as u16, y, text_w as u16, 1),
                cells,
                theme,
            );
        } else {
            cells.set_stringn(area.x, y, "~", 1, theme.tilde);
        }
    }
    let line = buf.cursor_line();
    let col = buf.cursor_display_col();
    if line >= top && line < top + area.height as usize && col >= left && col - left < text_w.max(1)
    {
        Some((
            area.x + (gw + col - left) as u16,
            area.y + (line - top) as u16,
        ))
    } else {
        None
    }
}

/// Highlights search matches on line `l`. Only the cells on screen are
/// looked at, so a long line full of matches costs no more than a short one.
fn paint_matches(app: &App, l: usize, row: Rect, cells: &mut Cells, theme: &Theme) {
    let Some(query) = app.search_query() else {
        return;
    };
    let buf = app.view_buffer();
    let line = buf.line_text(l);
    let matches = crate::search::line_matches(&line, query);
    if matches.is_empty() {
        return;
    }
    let (_, left) = app.scroll();
    let right = left + row.width as usize;
    let cursor = buf.cursor().wrapping_sub(buf.line_start(l));
    let mut walk = graphemes_with_char_offsets(&line);
    let mut col = walk.skip_plain(left);
    let mut next = 0;
    while let Some((off, g)) = walk.next() {
        if col >= right {
            break;
        }
        let w = grapheme_width(g, col);
        while matches.get(next).is_some_and(|m| m.end <= off) {
            next += 1;
        }
        let Some(m) = matches.get(next) else {
            break;
        };
        if m.contains(&off) && col + w > left {
            let style = if m.start == cursor {
                theme.search_current
            } else {
                theme.search_match
            };
            let from = col.max(left);
            let to = (col + w).min(right);
            let cell = Rect::new(row.x + (from - left) as u16, row.y, (to - from) as u16, 1);
            cells.set_style(cell, style);
        }
        col += w;
        if col < left {
            col += walk.skip_plain(left - col);
        }
    }
}

/// Styles the selected part of line `l` in `row`. A selection running past
/// the end of the line takes one more cell, so an empty line shows as selected.
fn paint_selection(app: &App, l: usize, row: Rect, cells: &mut Cells, theme: &Theme) {
    let buf = app.view_buffer();
    let Some(sel) = buf.selection() else {
        return;
    };
    let (start, end) = (buf.line_start(l), buf.line_end(l));
    if sel.end <= start || sel.start > end {
        return;
    }
    let (_, left) = app.scroll();
    let from = buf.display_col(l, sel.start.max(start) - start);
    let to = buf.display_col(l, sel.end.min(end) - start) + usize::from(sel.end > end);
    let right = left + row.width as usize;
    let (from, to) = (from.max(left), to.min(right));
    if from < to {
        let x = row.x + (from - left) as u16;
        let area = Rect::new(x, row.y, (to - from) as u16, 1);
        cells.set_style(area, theme.selection);
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
    let Some(lang) = buf.path().and_then(Lang::from_path) else {
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
                .find(|(r, _)| r.contains(&i))
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
