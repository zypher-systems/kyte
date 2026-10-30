//! Status bar, message line, legend, palette popup and modals.

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;

use super::theme::Theme;
use crate::app::{App, Mode, Prompt, CONFIRM_QUIT_TEXT};

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
    let text = match &app.mode {
        Mode::Prompt(Prompt::ConfirmQuit) => CONFIRM_QUIT_TEXT.to_string(),
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
