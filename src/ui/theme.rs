//! Colors. One theme, Signal: near black with vivid color. Terminals that say
//! they take 24-bit color get `SIGNAL`, and the rest get `SIGNAL_256`, its
//! nearest match in indexed colors. Later themes are more `Colors` constants.

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
    pub diff_ours: Style,
    pub diff_theirs: Style,
    pub search_match: Style,
    pub search_current: Style,
    pub modal: Style,
    pub error: Style,
    /// The cursor's line, as a background.
    pub current_line: Style,
    pub gutter_current: Style,
    pub popup_title: Style,
    /// Quiet text in a box: hints and rules.
    pub popup_hint: Style,
    /// The cells a box casts its shadow on.
    pub shadow: Style,
    pub syntax: Syntax,
}

/// Code colors, laid over `Theme::text`.
#[derive(Debug, Clone, Copy)]
pub struct Syntax {
    pub keyword: Style,
    pub string: Style,
    pub comment: Style,
    pub function: Style,
    pub kind: Style,
    pub constant: Style,
}

/// The colors a theme is made from.
#[derive(Debug, Clone, Copy)]
struct Colors {
    bg: Color,
    line: Color,
    popup: Color,
    status: Color,
    message: Color,
    shadow: Color,
    fg: Color,
    dim: Color,
    faint: Color,
    accent: Color,
    on_accent: Color,
    warm: Color,
    selection: Color,
    keyword: Color,
    string: Color,
    comment: Color,
    function: Color,
    kind: Color,
    constant: Color,
    matched: Color,
    diff_old: Color,
    diff_new: Color,
    error: Color,
}

const fn rgb(n: u32) -> Color {
    Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
}

const fn ix(n: u8) -> Color {
    Color::Indexed(n)
}

const fn on(fg: Color, bg: Color) -> Style {
    Style::new().fg(fg).bg(bg)
}

const fn theme(c: Colors) -> Theme {
    let bold = Modifier::BOLD;
    Theme {
        text: on(c.fg, c.bg),
        gutter: on(c.faint, c.bg),
        tilde: on(c.faint, c.bg),
        status: on(c.fg, c.status),
        status_mode: on(c.on_accent, c.accent).add_modifier(bold),
        status_mode_overlay: on(c.on_accent, c.warm).add_modifier(bold),
        message: on(c.dim, c.message),
        palette: on(c.fg, c.popup),
        palette_border: on(c.accent, c.popup),
        menu_key: on(c.warm, c.popup).add_modifier(bold),
        selected: on(c.on_accent, c.accent).add_modifier(bold),
        selection: Style::new().bg(c.selection),
        reader_dim: Style::new().fg(c.faint),
        diff_ours: Style::new().bg(c.diff_old),
        diff_theirs: Style::new().bg(c.diff_new),
        search_match: on(c.on_accent, c.matched),
        search_current: on(c.on_accent, c.warm).add_modifier(bold),
        modal: on(c.fg, c.popup),
        error: on(c.error, c.status).add_modifier(bold),
        current_line: Style::new().bg(c.line),
        gutter_current: on(c.fg, c.line),
        popup_title: on(c.accent, c.popup).add_modifier(bold),
        popup_hint: on(c.dim, c.popup),
        shadow: on(c.faint, c.shadow),
        syntax: Syntax {
            keyword: Style::new().fg(c.keyword).add_modifier(bold),
            string: Style::new().fg(c.string),
            comment: Style::new().fg(c.comment).add_modifier(Modifier::ITALIC),
            function: Style::new().fg(c.function),
            kind: Style::new().fg(c.kind),
            constant: Style::new().fg(c.constant),
        },
    }
}

const COLORS_24BIT: Colors = Colors {
    bg: rgb(0x0b0c10),
    line: rgb(0x181a22),
    popup: rgb(0x1c1f29),
    status: rgb(0x262a38),
    message: rgb(0x050608),
    shadow: rgb(0x000000),
    fg: rgb(0xf4f5f8),
    dim: rgb(0xb4b9c7),
    faint: rgb(0x747a8e),
    accent: rgb(0x38b6ff),
    on_accent: rgb(0x020a10),
    warm: rgb(0xffcc33),
    selection: rgb(0x1a3759),
    keyword: rgb(0xff6fb5),
    string: rgb(0x7dff9b),
    comment: rgb(0x9aa1b5),
    function: rgb(0x5cc8ff),
    kind: rgb(0xffd75f),
    constant: rgb(0xff9b4f),
    matched: rgb(0xd8e04a),
    diff_old: rgb(0x5e1a2a),
    diff_new: rgb(0x0c4029),
    error: rgb(0xff6b7a),
};

const COLORS_256: Colors = Colors {
    bg: ix(232),
    line: ix(234),
    popup: ix(235),
    status: ix(236),
    message: ix(16),
    shadow: ix(16),
    fg: ix(255),
    dim: ix(250),
    faint: ix(244),
    accent: ix(39),
    on_accent: ix(232),
    warm: ix(220),
    selection: ix(18),
    keyword: ix(205),
    string: ix(120),
    comment: ix(247),
    function: ix(81),
    kind: ix(221),
    constant: ix(215),
    matched: ix(185),
    diff_old: ix(52),
    diff_new: ix(22),
    error: ix(204),
};

pub const SIGNAL: Theme = theme(COLORS_24BIT);
pub const SIGNAL_256: Theme = theme(COLORS_256);

/// The theme for a terminal with this `COLORTERM`. It's the usual way to say
/// 24-bit color works, and it isn't set over SSH or sudo unless passed along,
/// so a terminal that says nothing gets indexed colors, which always work.
pub fn for_terminal(colorterm: Option<&str>) -> &'static Theme {
    match colorterm {
        Some("truecolor" | "24bit") => &SIGNAL,
        _ => &SIGNAL_256,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMUM: f64 = 4.5;

    fn channels(c: Color) -> [u8; 3] {
        let cube = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        match c {
            Color::Rgb(r, g, b) => [r, g, b],
            Color::Indexed(n @ 232..) => [8 + (n - 232) * 10; 3],
            Color::Indexed(n @ 16..) => {
                let n = n - 16;
                [cube(n / 36), cube(n / 6 % 6), cube(n % 6)]
            }
            other => panic!("not a theme color: {other:?}"),
        }
    }

    fn luminance(c: Color) -> f64 {
        let [r, g, b] = channels(c).map(|v| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        });
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn code(c: &Colors) -> [Color; 7] {
        [
            c.fg, c.keyword, c.string, c.comment, c.function, c.kind, c.constant,
        ]
    }

    fn assert_readable(what: &str, fgs: &[Color], bgs: &[Color]) {
        for &fg in fgs {
            for &bg in bgs {
                let r = contrast(fg, bg);
                assert!(r >= MINIMUM, "{what}: {fg:?} on {bg:?} is {r:.1}");
            }
        }
    }

    #[test]
    fn code_is_readable_on_every_background_it_lands_on() {
        for c in [COLORS_24BIT, COLORS_256] {
            let bgs = [c.bg, c.line, c.selection, c.diff_old];
            assert_readable("code", &code(&c), &bgs);
        }
        let c = COLORS_24BIT;
        assert_readable("added lines", &code(&c), &[c.diff_new]);
    }

    /// The darkest green in 256 colors still leaves code colors under the
    /// minimum on added lines, so there only plain text has to reach it.
    #[test]
    fn text_is_readable_on_added_lines_in_256_colors() {
        let c = COLORS_256;
        assert_readable("added lines", &[c.fg], &[c.diff_new]);
    }

    #[test]
    fn chrome_is_readable() {
        for c in [COLORS_24BIT, COLORS_256] {
            assert_readable("line numbers", &[c.faint], &[c.bg]);
            assert_readable("text", &[c.fg], &[c.popup, c.status]);
            assert_readable("hints", &[c.dim], &[c.popup, c.message]);
            assert_readable("box", &[c.accent, c.warm], &[c.popup]);
            assert_readable("errors", &[c.error], &[c.status, c.message]);
            assert_readable("marks", &[c.on_accent], &[c.accent, c.warm, c.matched]);
        }
    }

    #[test]
    fn a_terminal_gets_24_bit_color_only_if_it_says_so() {
        let is_24bit = |v| for_terminal(v).text.bg == SIGNAL.text.bg;
        assert!(is_24bit(Some("truecolor")));
        assert!(is_24bit(Some("24bit")));
        assert!(!is_24bit(None));
        assert!(!is_24bit(Some("")));
        assert!(!is_24bit(Some("yes")));
    }
}
