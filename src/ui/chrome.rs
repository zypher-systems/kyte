//! Status bar, message line, legend, palette popup and modals.

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;

use super::theme::Theme;
use ratatui::layout::Rect as R;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::app::{After, App, Mode, Prompt};
use crate::buffer::Indent;
use crate::command::{help_max_top, KEY_HELP};
use crate::history;
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
    let (l, c) = app.view_buffer().line_col();
    let eol = if app.buffer.eol() == "\r\n" {
        " │ CRLF"
    } else {
        ""
    };
    let sel = app
        .buffer
        .selection()
        .map(|r| format!(" │ {} selected", r.len()))
        .unwrap_or_default();
    let indent = match app.buffer.indent() {
        Indent::Tabs => String::new(),
        Indent::Spaces(n) => format!(" │ Spaces {n}"),
    };
    let rest = match &app.mode {
        Mode::History { timeline, at } => format!(
            "│ {name} │ {} of {} │ {} │ {}",
            at + 1,
            timeline.len(),
            history::ago_from_now(timeline.version(*at).time_ms),
            timeline.describe(*at),
        ),
        _ => format!("│ {name}{dirty} │ Ln {l}, Col {c}{sel}{indent}{eol}"),
    };
    let x = mode.chars().count();
    if x < w {
        let (end, _) = cells.set_stringn(area.x + x as u16, area.y, rest, w - x, theme.status);
        let left = (area.x + area.width).saturating_sub(end) as usize;
        if app.recovery_off() {
            cells.set_stringn(end, area.y, " │ no crash recovery", left, theme.error);
        } else if app.history_lost() {
            cells.set_stringn(end, area.y, " │ no history", left, theme.error);
        }
    }
}

/// Message line. Returns the cursor position when a prompt owns the cursor.
pub fn message_line(app: &App, area: Rect, cells: &mut Cells, theme: &Theme) -> Option<(u16, u16)> {
    cells.set_style(area, theme.message);
    let w = area.width as usize;
    let text = match &app.mode {
        Mode::Prompt(Prompt::Open { text }) => {
            return input_prompt(app, "Open", text, area, cells, theme)
        }
        Mode::Prompt(Prompt::ReplaceFind { query }) => {
            return input_prompt(app, "Replace", query, area, cells, theme)
        }
        Mode::Prompt(Prompt::ReplaceWith { with, .. }) => {
            return input_prompt(app, "With", with, area, cells, theme)
        }
        Mode::Prompt(Prompt::GotoLine { text }) => {
            return input_prompt(app, "Go to line", text, area, cells, theme)
        }
        Mode::Prompt(Prompt::SaveAs { text, .. }) => {
            return input_prompt(app, "Save as", text, area, cells, theme)
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

/// A typed prompt with any error after it. Returns the cursor position. Text
/// too long for the row shows its end, where the typing happens.
fn input_prompt(
    app: &App,
    label: &str,
    text: &str,
    area: Rect,
    cells: &mut Cells,
    theme: &Theme,
) -> Option<(u16, u16)> {
    let w = area.width as usize;
    let shown = format!(" {label}: {text}");
    let n = shown.chars().count();
    let skip = (n + 1).saturating_sub(w);
    let shown: String = shown.chars().skip(skip).collect();
    let n = n - skip;
    cells.set_stringn(area.x, area.y, &shown, w, theme.message);
    if let Some(m) = &app.message {
        if n + 3 < w {
            cells.set_stringn(area.x + n as u16 + 3, area.y, m, w - n - 3, theme.error);
        }
    }
    Some((area.x + n as u16, area.y))
}

/// A floating list of keys and what they do.
struct Menu {
    title: &'static str,
    rows: Vec<(&'static str, &'static str)>,
    /// Keep clear of the cursor, because what's under it matters. Otherwise
    /// the menu is a question and takes the middle of the screen.
    beside_text: bool,
}

/// The menu the current mode wants, if any.
fn menu_for(app: &App) -> Option<Menu> {
    let question = |title, rows| Menu {
        title,
        rows,
        beside_text: false,
    };
    let beside = |title, rows| Menu {
        title,
        rows,
        beside_text: true,
    };
    Some(match &app.mode {
        Mode::Overlay { .. } => {
            let title = if app.buffer.selection().is_some() {
                "Selection"
            } else {
                "Esc"
            };
            beside(title, app.layer_rows())
        }
        Mode::Prompt(Prompt::ReplaceConfirm(_)) => beside(
            "Replace this one?",
            vec![
                ("y", "Replace"),
                ("n", "Skip"),
                ("a", "Replace all the rest"),
                ("Esc", "Stop"),
            ],
        ),
        Mode::History { .. } => beside(
            "History",
            vec![
                ("Left Right", "one edit"),
                ("Up Down", "one burst"),
                ("PgUp PgDn", "saved versions"),
                ("Home End", "first, newest"),
                ("Enter", "restore this"),
                ("Esc", "close"),
            ],
        ),
        Mode::Prompt(Prompt::ConfirmLeave { then: After::Quit }) => question(
            "Unsaved changes",
            vec![
                ("y", "Save and quit"),
                ("n", "Discard them and quit"),
                ("Esc", "Keep editing"),
            ],
        ),
        Mode::Prompt(Prompt::ConfirmLeave { .. }) => question(
            "Unsaved changes",
            vec![
                ("y", "Save, then open the file"),
                ("n", "Discard them and open it"),
                ("Esc", "Keep editing"),
            ],
        ),
        Mode::Prompt(Prompt::ConfirmSave { risk, .. }) => {
            let (title, yes) = risk.question();
            question(
                title,
                vec![("y", yes), ("n", "Don't save"), ("Esc", "Don't save")],
            )
        }
        Mode::Prompt(Prompt::ConfirmDeleteHistory) => question(
            "Delete all history for this file?",
            vec![("y", "Delete it"), ("n", "Keep it"), ("Esc", "Keep it")],
        ),
        _ => return None,
    })
}

/// Bottom right of `area`, moved out of the way if `cursor` is there.
fn clear_of_cursor(area: Rect, w: u16, h: u16, cursor: Option<(u16, u16)>) -> Rect {
    let (w, h) = (w.min(area.width), h.min(area.height));
    let right = area.x + area.width - w;
    let bottom = area.y + area.height - h;
    let hits =
        |r: Rect| cursor.is_some_and(|(x, y)| x >= r.x && x < r.x + w && y >= r.y && y < r.y + h);
    [
        R::new(right, bottom, w, h),
        R::new(right, area.y, w, h),
        R::new(area.x, bottom, w, h),
        R::new(area.x, area.y, w, h),
    ]
    .into_iter()
    .find(|r| !hits(*r))
    .unwrap_or(R::new(right, bottom, w, h))
}

/// Draws the current mode's menu. Rows that don't fit the height flow into
/// more columns, so every key stays visible on a short terminal.
pub fn menu(
    app: &App,
    text_area: Rect,
    cursor: Option<(u16, u16)>,
    cells: &mut Cells,
    theme: &Theme,
) {
    let Some(menu) = menu_for(app) else {
        return;
    };
    let key_w = menu.rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let label_w = menu.rows.iter().map(|(_, l)| l.len()).max().unwrap_or(0);
    let cell_w = key_w + 1 + label_w + 2;
    let room = (text_area.height as usize).saturating_sub(2).max(1);
    let per_col = menu.rows.len().min(room);
    let cols = menu.rows.len().div_ceil(per_col);
    let w = (cell_w * cols + 3).max(menu.title.len() + 6) as u16;
    let h = per_col as u16 + 2;
    let area = if menu.beside_text {
        clear_of_cursor(text_area, w, h, cursor)
    } else {
        centered(text_area, w, h)
    };
    let inner = modal_box(area, cells, menu.title, theme.palette, theme.palette_border);
    for (i, (key, label)) in menu.rows.iter().enumerate() {
        let x = inner.x + 1 + (i / per_col * cell_w) as u16;
        let y = inner.y + (i % per_col) as u16;
        if x >= inner.x + inner.width || y >= inner.y + inner.height {
            continue;
        }
        let room = (inner.x + inner.width - x) as usize;
        let shown = format!("{key:>key_w$}");
        let (next, _) = cells.set_stringn(x, y, &shown, room, theme.menu_key);
        let room = (inner.x + inner.width).saturating_sub(next + 1) as usize;
        cells.set_stringn(next + 1, y, label, room, theme.palette);
    }
}

/// Names Tab found in a file prompt, floating above the message line.
pub fn completions(app: &App, text_area: Rect, cells: &mut Cells, theme: &Theme) {
    const MAX_ROWS: usize = 10;
    if app.completions.is_empty() {
        return;
    }
    let more = app.completions.len().saturating_sub(MAX_ROWS);
    let mut rows: Vec<String> = app.completions.iter().take(MAX_ROWS).cloned().collect();
    if more > 0 {
        rows.push(format!("and {more} more"));
    }
    let widest = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as u16;
    let w = (widest + 4).max(16).min(text_area.width);
    let h = (rows.len() as u16 + 2).min(text_area.height);
    let bottom = text_area.y + text_area.height - h;
    let area = R::new(text_area.x, bottom, w, h);
    let inner = modal_box(area, cells, "Matches", theme.palette, theme.palette_border);
    for (i, row) in rows.iter().enumerate() {
        let y = inner.y + i as u16;
        if y < inner.y + inner.height {
            let room = inner.width.saturating_sub(1) as usize;
            cells.set_stringn(inner.x + 1, y, row, room, theme.palette);
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
    let title = if state.is_universal() {
        "Commands, files, :line, /text"
    } else {
        "Command palette"
    };
    let inner = modal_box(area, cells, title, theme.palette, theme.palette_border);
    let input = format!("> {}", state.query);
    let iw = inner.width as usize;
    cells.set_stringn(inner.x, inner.y, &input, iw, theme.palette);
    if state.results.is_empty() {
        if inner.height > 1 {
            cells.set_stringn(inner.x, inner.y + 1, "  No match", iw, theme.palette);
        }
    } else {
        let off = state.scroll_offset();
        for (i, entry) in state
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
            let hint = entry.hint();
            let label = fit_label(&entry.label(), iw.saturating_sub(hint.len() + 4));
            let label = format!("{marker}{label}");
            let pad = iw.saturating_sub(label.chars().count() + hint.len() + 1);
            let line = format!("{label}{}{hint} ", " ".repeat(pad));
            cells.set_style(Rect::new(inner.x, y, inner.width, 1), style);
            cells.set_stringn(inner.x, y, line, iw, style);
        }
    }
    let cx = inner.x + (input.chars().count() as u16).min(inner.width.saturating_sub(1));
    Some((cx, inner.y))
}

/// `text` within `width` cells. A long path keeps its end, where the file
/// name is.
fn fit_label(text: &str, width: usize) -> String {
    let n = text.chars().count();
    if n <= width || width < 2 {
        return text.to_string();
    }
    let tail: String = text.chars().skip(n + 1 - width).collect();
    format!("…{tail}")
}

/// The "Show key help" modal.
pub fn help(app: &App, text_area: Rect, cells: &mut Cells, theme: &Theme) {
    let Mode::Help { top } = app.mode else {
        return;
    };
    let w = 64.min(text_area.width.saturating_sub(2)).max(10);
    let h = (KEY_HELP.len() as u16 + 4).min(text_area.height);
    let area = centered(text_area, w, h);
    let max_top = help_max_top(text_area.height as usize);
    let title = if max_top > 0 {
        "Kyte keys (Up, Down scroll; Esc closes)"
    } else {
        "Kyte keys (Esc closes)"
    };
    let inner = modal_box(area, cells, title, theme.modal, theme.palette_border);
    for (i, (k, v)) in KEY_HELP.iter().skip(top.min(max_top)).enumerate() {
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
    let path = if info.target.as_os_str().is_empty() {
        "(untitled)".to_string()
    } else {
        info.target.display().to_string()
    };
    let lines: Vec<String> = if info.recovered.is_some() {
        let mut l = vec![
            "Kyte found unsaved changes for:".to_string(),
            format!("  {path}"),
        ];
        if info.file_changed {
            l.push("The file changed on disk after these edits were made.".to_string());
        }
        l.extend([
            format!("Swap file written {}", info.modified),
            String::new(),
            "r  Recover the unsaved changes".to_string(),
            "d  Discard them and open the file as it is on disk".to_string(),
            "q  Quit without changing anything (also Esc)".to_string(),
        ]);
        l
    } else {
        vec![
            "The swap file is stale for:".to_string(),
            format!("  {path}"),
            "It can't be read, so its edits can't be recovered.".to_string(),
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
