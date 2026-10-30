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
    let mut col = 0;
    for (_, g) in graphemes_with_char_offsets(line) {
        let w = grapheme_width(g, col);
        let end = col + w;
        if end <= left {
            col = end;
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
    let buf = &app.buffer;
    let (top, left) = app.scroll();
    let gw = gutter_width(buf.line_count(), app.show_line_numbers);
    let text_w = (area.width as usize).saturating_sub(gw);
    cells.set_style(area, theme.text);
    for row in 0..area.height as usize {
        let y = area.y + row as u16;
        let line = top + row;
        if line < buf.line_count() {
            if gw > 0 {
                let num = format!("{:>w$} │ ", line + 1, w = gw - 3);
                cells.set_stringn(area.x, y, num, gw, theme.gutter);
            }
            render_line(
                cells,
                area.x + gw as u16,
                y,
                &buf.line_text(line),
                left,
                text_w,
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
}
