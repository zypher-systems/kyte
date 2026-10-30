//! Status bar, message line, legend, palette popup and modals.

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;

use super::theme::Theme;
use ratatui::layout::Rect as R;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::app::{App, Mode, Prompt, CONFIRM_QUIT_TEXT, LEGEND};
use crate::command::KEY_HELP;
use crate::palette::VISIBLE_ROWS;

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
        Mode::Prompt(Prompt::GotoLine { text }) => {
            let shown = format!(" Go to line: {text}");
            let n = shown.chars().count();
            match &app.message {
                Some(m) => {
                    cells.set_stringn(area.x, area.y, &shown, w, theme.message);
                    if n + 3 < w {
                        cells.set_stringn(area.x + n as u16 + 3, area.y, m, w - n - 3, theme.error);
                    }
                }
                None => {
                    cells.set_stringn(area.x, area.y, &shown, w, theme.message);
                }
            }
            return Some((
                area.x + (n as u16).min(area.width.saturating_sub(1)),
                area.y,
            ));
        }
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
    let rows = state.results.len().clamp(1, VISIBLE_ROWS) as u16;
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
    let iw = inner.width as usize;
    cells.set_stringn(inner.x, inner.y, &input, iw, theme.palette);
    if state.results.is_empty() {
        if inner.height > 1 {
            cells.set_stringn(
                inner.x,
                inner.y + 1,
                "  No matching command",
                iw,
                theme.palette,
            );
        }
    } else {
        let off = state.scroll_offset();
        for (i, cmd) in state
            .results
            .iter()
            .enumerate()
            .skip(off)
            .take(VISIBLE_ROWS)
        {
            let y = inner.y + 1 + (i - off) as u16;
            if y >= inner.y + inner.height {
                break;
            }
            let sel = i == state.selected;
            let style = if sel { theme.selected } else { theme.palette };
            let marker = if sel { "▸ " } else { "  " };
            let label = format!("{marker}{}", cmd.label());
            let hint = cmd.hint();
            let pad = iw.saturating_sub(label.chars().count() + hint.len() + 1);
            let line = format!("{label}{}{hint} ", " ".repeat(pad));
            cells.set_style(Rect::new(inner.x, y, inner.width, 1), style);
            cells.set_stringn(inner.x, y, line, iw, style);
        }
    }
    let cx = inner.x + (input.chars().count() as u16).min(inner.width.saturating_sub(1));
    Some((cx, inner.y))
}

/// The "Show key help" modal.
pub fn help(app: &App, text_area: Rect, cells: &mut Cells, theme: &Theme) {
    if app.mode != Mode::Help {
        return;
    }
    let w = 64.min(text_area.width.saturating_sub(2)).max(10);
    let h = (KEY_HELP.len() as u16 + 4).min(text_area.height);
    let area = centered(text_area, w, h);
    let inner = modal_box(
        area,
        cells,
        "Kyte keys (Esc closes)",
        theme.modal,
        theme.palette_border,
    );
    for (i, (k, v)) in KEY_HELP.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let line = format!(" {k:<26}{v}");
        cells.set_stringn(inner.x, y, line, inner.width as usize, theme.modal);
    }
}

/// The Recover (or Stale) modal shown at startup.
pub fn recover(app: &App, text_area: Rect, cells: &mut Cells, theme: &Theme) {
    let Mode::Recover(info) = &app.mode else {
        return;
    };
    let path = info.target.display().to_string();
    let lines: Vec<String> = if info.recovered.is_some() {
        vec![
            "Kyte found unsaved changes for:".to_string(),
            format!("  {path}"),
            format!("Swap file written {}", info.modified),
            String::new(),
            "r  Recover the unsaved changes".to_string(),
            "d  Discard them and open the file as it is on disk".to_string(),
            "q  Quit without changing anything (also Esc)".to_string(),
        ]
    } else {
        vec![
            "The swap file is stale for:".to_string(),
            format!("  {path}"),
            "The file changed on disk after the swap was written.".to_string(),
            format!("Swap file written {}", info.modified),
            String::new(),
            "d  Discard the stale swap and open the file".to_string(),
            "q  Quit without changing anything (also Esc)".to_string(),
        ]
    };
    let title = if info.recovered.is_some() {
        "Recover unsaved changes"
    } else {
        "Stale swap file"
    };
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let w = (longest + 4).max(40).min(text_area.width);
    let h = (lines.len() as u16 + 2).min(text_area.height);
    let area = centered(text_area, w, h);
    let inner = modal_box(area, cells, title, theme.modal, theme.palette_border);
    for (i, l) in lines.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        cells.set_stringn(
            inner.x + 1,
            y,
            l,
            inner.width.saturating_sub(1) as usize,
            theme.modal,
        );
    }
}
