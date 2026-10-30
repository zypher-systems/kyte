//! Status bar, message line, legend, palette popup and modals.

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;

use super::theme::Theme;
use ratatui::layout::Rect as R;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::app::{App, Mode, Prompt, CONFIRM_QUIT_TEXT, LEGEND};

/// Status bar: mode, file name, dirty marker, position (one full row).
pub fn status_bar(app: &App, area: Rect, cells: &mut Cells, theme: &Theme) {
    cells.set_style(area, theme.status);
    let mode = format!(" {} ", app.mode_label());
    let mode_style = match app.mode {
        Mode::Insert => theme.status_mode,
        _ => theme.status_mode_overlay,
    };
    let w = area.width as usize;
    cells.set_stringn(area.x, area.y, &mode, w, mode_style);
    let name = app
        .buffer
        .path()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_string());
    let dirty = if app.buffer.is_dirty() { " [+]" } else { "" };
    let (l, c) = app.buffer.line_col();
    let rest = format!("│ {name}{dirty} │ Ln {l}, Col {c}");
    let x = mode.chars().count();
    if x < w {
        cells.set_stringn(area.x + x as u16, area.y, rest, w - x, theme.status);
    }
}

/// Message line. Returns the cursor position when a prompt owns the cursor.
pub fn message_line(app: &App, area: Rect, cells: &mut Cells, theme: &Theme) -> Option<(u16, u16)> {
    cells.set_style(area, theme.message);
    let w = area.width as usize;
    if let Mode::Overlay { .. } = app.mode {
        legend(area, cells, theme);
        return None;
    }
    let text = match &app.mode {
        Mode::Prompt(Prompt::ConfirmQuit) => CONFIRM_QUIT_TEXT.to_string(),
        Mode::Prompt(Prompt::Search { query, .. }) => match &app.message {
            Some(m) => format!("Search: {query}   ({m})"),
            None => format!("Search: {query}"),
        },
        _ => app.message.clone().unwrap_or_default(),
    };
    let style = if app.message_is_error && matches!(app.mode, Mode::Insert) {
        theme.error
    } else {
        theme.message
    };
    cells.set_stringn(area.x, area.y, format!(" {text}"), w, style);
    None
}

/// The overlay legend: each key letter and its label.
fn legend(area: Rect, cells: &mut Cells, theme: &Theme) {
    cells.set_style(area, theme.legend);
    let mut x = area.x + 1;
    let right = area.x + area.width;
    for (key, label) in LEGEND {
        for (text, style) in [
            (*key, theme.legend_key),
            (" ", theme.legend),
            (*label, theme.legend),
            ("  ", theme.legend),
        ] {
            if x >= right {
                return;
            }
            let (nx, _) = cells.set_stringn(x, area.y, text, (right - x) as usize, style);
            x = nx;
        }
    }
}

/// A centered box of `w` x `h` cells inside `area` (clamped).
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    R::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Draws a bordered, titled box and returns its inner area.
pub fn modal_box(
    area: Rect,
    cells: &mut Cells,
    title: &str,
    style: ratatui::style::Style,
    border: ratatui::style::Style,
) -> Rect {
    Clear.render(area, cells);
    cells.set_style(area, style);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(style)
        .title(format!(" {title} "));
    let inner = block.inner(area);
    block.render(area, cells);
    inner
}

/// The command palette popup. Returns the cursor position in its input line.
pub fn palette(app: &App, text_area: Rect, cells: &mut Cells, theme: &Theme) -> Option<(u16, u16)> {
    let Mode::Palette(state) = &app.mode else {
        return None;
    };
    let rows = 1u16;
    let w = 54.min(text_area.width.saturating_sub(2)).max(10);
    let area = centered(text_area, w, rows + 3);
    let inner = modal_box(
        area,
        cells,
        "Command palette",
        theme.palette,
        theme.palette_border,
    );
    let input = format!("> {}", state.query);
    cells.set_stringn(
        inner.x,
        inner.y,
        &input,
        inner.width as usize,
        theme.palette,
    );
    let cx = inner.x + (input.chars().count() as u16).min(inner.width.saturating_sub(1));
    Some((cx, inner.y))
}
