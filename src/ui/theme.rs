//! Colors. One dark theme, 256-color indexed values only (no truecolor), so it
//! looks the same in tmux and in the pty demo. Later themes are more constants.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub text: Style,
    pub gutter: Style,
    pub tilde: Style,
    pub status: Style,
    pub status_mode: Style,
    pub status_mode_overlay: Style,
    pub message: Style,
    pub palette: Style,
    pub palette_border: Style,
    pub menu_key: Style,
    pub selected: Style,
    pub selection: Style,
    pub reader_dim: Style,
    pub search_match: Style,
    pub search_current: Style,
    pub modal: Style,
    pub error: Style,
}

const fn ix(n: u8) -> Color {
    Color::Indexed(n)
}

pub const DARK: Theme = Theme {
    text: Style::new().fg(ix(252)).bg(ix(235)),
    gutter: Style::new().fg(ix(243)).bg(ix(235)),
    tilde: Style::new().fg(ix(239)).bg(ix(235)),
    status: Style::new().fg(ix(252)).bg(ix(238)),
    status_mode: Style::new()
        .fg(ix(235))
        .bg(ix(110))
        .add_modifier(Modifier::BOLD),
    status_mode_overlay: Style::new()
        .fg(ix(235))
        .bg(ix(214))
        .add_modifier(Modifier::BOLD),
    message: Style::new().fg(ix(250)).bg(ix(234)),
    palette: Style::new().fg(ix(252)).bg(ix(237)),
    palette_border: Style::new().fg(ix(110)).bg(ix(237)),
    menu_key: Style::new()
        .fg(ix(214))
        .bg(ix(237))
        .add_modifier(Modifier::BOLD),
    selected: Style::new()
        .fg(ix(235))
        .bg(ix(110))
        .add_modifier(Modifier::BOLD),
    selection: Style::new().bg(ix(60)),
    reader_dim: Style::new().fg(ix(240)),
    search_match: Style::new().fg(ix(235)).bg(ix(143)),
    search_current: Style::new()
        .fg(ix(235))
        .bg(ix(214))
        .add_modifier(Modifier::BOLD),
    modal: Style::new().fg(ix(252)).bg(ix(237)),
    error: Style::new()
        .fg(ix(203))
        .bg(ix(234))
        .add_modifier(Modifier::BOLD),
};
