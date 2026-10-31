//! Colors. Each theme comes in 24-bit color and in its nearest match in indexed
//! colors, and terminals that say they take 24-bit color get the first. A new
//! theme is a pair of `Colors` and a `ThemeName`.

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
    /// A misspelled word, laid over its text.
    pub misspelled: Style,
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
    on_matched: Color,
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

fn build(c: Colors) -> Theme {
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
        search_match: on(c.on_matched, c.matched),
        search_current: on(c.on_accent, c.warm).add_modifier(bold),
        modal: on(c.fg, c.popup),
        error: on(c.error, c.status).add_modifier(bold),
        current_line: Style::new().bg(c.line),
        gutter_current: on(c.fg, c.line),
        popup_title: on(c.accent, c.popup).add_modifier(bold),
        popup_hint: on(c.dim, c.popup),
        shadow: on(c.faint, c.shadow),
        misspelled: Style::new().fg(c.error).add_modifier(Modifier::UNDERLINED),
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

const SIGNAL_24BIT: Colors = Colors {
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
    on_matched: rgb(0x020a10),
    diff_old: rgb(0x5e1a2a),
    diff_new: rgb(0x0c4029),
    error: rgb(0xff6b7a),
};

const SIGNAL_256: Colors = Colors {
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
    on_matched: ix(232),
    diff_old: ix(52),
    diff_new: ix(22),
    error: ix(204),
};

const HARBOR_24BIT: Colors = Colors {
    bg: rgb(0x0f1f2c),
    line: rgb(0x162a3a),
    popup: rgb(0x1b3345),
    status: rgb(0x1f3c50),
    message: rgb(0x0b1822),
    shadow: rgb(0x08121a),
    fg: rgb(0xdce8ee),
    dim: rgb(0x8ba6b5),
    faint: rgb(0x738a99),
    accent: rgb(0x4fd0c4),
    on_accent: rgb(0x08201f),
    warm: rgb(0xffc36b),
    selection: rgb(0x1c3f54),
    keyword: rgb(0x6cc4f5),
    string: rgb(0xa6e3a1),
    comment: rgb(0x94abb8),
    function: rgb(0x5fe0cf),
    kind: rgb(0xffd58a),
    constant: rgb(0xff9e80),
    matched: rgb(0xb9d27a),
    on_matched: rgb(0x08201f),
    diff_old: rgb(0x47222c),
    diff_new: rgb(0x0e382e),
    error: rgb(0xff8a8a),
};

const HARBOR_256: Colors = Colors {
    bg: ix(234),
    line: ix(235),
    popup: ix(236),
    status: ix(237),
    message: ix(233),
    shadow: ix(232),
    fg: ix(254),
    dim: ix(109),
    faint: ix(102),
    accent: ix(80),
    on_accent: ix(232),
    warm: ix(215),
    selection: ix(18),
    keyword: ix(117),
    string: ix(150),
    comment: ix(109),
    function: ix(80),
    kind: ix(222),
    constant: ix(216),
    matched: ix(150),
    on_matched: ix(232),
    diff_old: ix(52),
    diff_new: ix(22),
    error: ix(210),
};

const DUSK_24BIT: Colors = Colors {
    bg: rgb(0x1b1d2b),
    line: rgb(0x24273a),
    popup: rgb(0x2a2d43),
    status: rgb(0x2f3350),
    message: rgb(0x171926),
    shadow: rgb(0x12131d),
    fg: rgb(0xd5d8ea),
    dim: rgb(0x9196b4),
    faint: rgb(0x8186a0),
    accent: rgb(0x8fa8f5),
    on_accent: rgb(0x171926),
    warm: rgb(0xf3b86b),
    selection: rgb(0x34395f),
    keyword: rgb(0xc29df7),
    string: rgb(0x9fd8a4),
    comment: rgb(0xa2a6c2),
    function: rgb(0x82b6f7),
    kind: rgb(0xf3cc8f),
    constant: rgb(0xf59b8a),
    matched: rgb(0xcfc27a),
    on_matched: rgb(0x171926),
    diff_old: rgb(0x4a2333),
    diff_new: rgb(0x1b372d),
    error: rgb(0xff8f9e),
};

const DUSK_256: Colors = Colors {
    bg: ix(234),
    line: ix(235),
    popup: ix(236),
    status: ix(237),
    message: ix(233),
    shadow: ix(232),
    fg: ix(253),
    dim: ix(146),
    faint: ix(103),
    accent: ix(111),
    on_accent: ix(232),
    warm: ix(179),
    selection: ix(18),
    keyword: ix(183),
    string: ix(151),
    comment: ix(146),
    function: ix(111),
    kind: ix(222),
    constant: ix(216),
    matched: ix(186),
    on_matched: ix(232),
    diff_old: ix(52),
    diff_new: ix(22),
    error: ix(210),
};

const EMBER_24BIT: Colors = Colors {
    bg: rgb(0x211c1a),
    line: rgb(0x2b2522),
    popup: rgb(0x332c28),
    status: rgb(0x3b322d),
    message: rgb(0x1b1715),
    shadow: rgb(0x151210),
    fg: rgb(0xeadfd2),
    dim: rgb(0xa89a8b),
    faint: rgb(0x908479),
    accent: rgb(0xf2994a),
    on_accent: rgb(0x211509),
    warm: rgb(0xf7c95f),
    selection: rgb(0x493729),
    keyword: rgb(0xee8c6a),
    string: rgb(0xc3cf7e),
    comment: rgb(0xb1a497),
    function: rgb(0xf0b866),
    kind: rgb(0x8ec7b0),
    constant: rgb(0xd9a0d0),
    matched: rgb(0xc9b458),
    on_matched: rgb(0x211509),
    diff_old: rgb(0x4d2621),
    diff_new: rgb(0x293421),
    error: rgb(0xff9180),
};

const EMBER_256: Colors = Colors {
    bg: ix(234),
    line: ix(235),
    popup: ix(236),
    status: ix(237),
    message: ix(233),
    shadow: ix(232),
    fg: ix(254),
    dim: ix(247),
    faint: ix(102),
    accent: ix(215),
    on_accent: ix(232),
    warm: ix(221),
    selection: ix(237),
    keyword: ix(209),
    string: ix(186),
    comment: ix(248),
    function: ix(179),
    kind: ix(151),
    constant: ix(182),
    matched: ix(179),
    on_matched: ix(232),
    diff_old: ix(52),
    diff_new: ix(22),
    error: ix(210),
};

const MOSS_24BIT: Colors = Colors {
    bg: rgb(0x1a211d),
    line: rgb(0x222b26),
    popup: rgb(0x28332d),
    status: rgb(0x2e3a33),
    message: rgb(0x161c19),
    shadow: rgb(0x111613),
    fg: rgb(0xd9e0d6),
    dim: rgb(0x98a798),
    faint: rgb(0x7e8b80),
    accent: rgb(0x9cc48a),
    on_accent: rgb(0x141c12),
    warm: rgb(0xdcc27c),
    selection: rgb(0x304134),
    keyword: rgb(0xa8c68f),
    string: rgb(0xd5c78f),
    comment: rgb(0xa0aca0),
    function: rgb(0x8fc6b0),
    kind: rgb(0xc7d0a0),
    constant: rgb(0xdba88c),
    matched: rgb(0xb5b86a),
    on_matched: rgb(0x141c12),
    diff_old: rgb(0x492a29),
    diff_new: rgb(0x213827),
    error: rgb(0xf09a8c),
};

const MOSS_256: Colors = Colors {
    bg: ix(234),
    line: ix(235),
    popup: ix(236),
    status: ix(237),
    message: ix(233),
    shadow: ix(232),
    fg: ix(253),
    dim: ix(247),
    faint: ix(102),
    accent: ix(108),
    on_accent: ix(232),
    warm: ix(179),
    selection: ix(237),
    keyword: ix(150),
    string: ix(187),
    comment: ix(248),
    function: ix(116),
    kind: ix(186),
    constant: ix(180),
    matched: ix(143),
    on_matched: ix(232),
    diff_old: ix(52),
    diff_new: ix(22),
    error: ix(217),
};

const DAYBREAK_24BIT: Colors = Colors {
    bg: rgb(0xf7f8fa),
    line: rgb(0xeceff4),
    popup: rgb(0xffffff),
    status: rgb(0xdfe4ec),
    message: rgb(0xeef0f4),
    shadow: rgb(0xc9cfda),
    fg: rgb(0x2a3140),
    dim: rgb(0x5f6980),
    faint: rgb(0x697182),
    accent: rgb(0x2f62d9),
    on_accent: rgb(0xffffff),
    warm: rgb(0xb35c00),
    selection: rgb(0xd1dff9),
    keyword: rgb(0x8835c6),
    string: rgb(0x216e3d),
    comment: rgb(0x586174),
    function: rgb(0x1d5ebe),
    kind: rgb(0x8a5509),
    constant: rgb(0xab3b2d),
    matched: rgb(0xf4d35e),
    on_matched: rgb(0x2a3140),
    diff_old: rgb(0xfce3e3),
    diff_new: rgb(0xcef1d7),
    error: rgb(0xc12336),
};

const DAYBREAK_256: Colors = Colors {
    bg: ix(231),
    line: ix(255),
    popup: ix(255),
    status: ix(253),
    message: ix(255),
    shadow: ix(250),
    fg: ix(236),
    dim: ix(60),
    faint: ix(242),
    accent: ix(26),
    on_accent: ix(231),
    warm: ix(94),
    selection: ix(189),
    keyword: ix(91),
    string: ix(22),
    comment: ix(59),
    function: ix(25),
    kind: ix(58),
    constant: ix(88),
    matched: ix(221),
    on_matched: ix(236),
    diff_old: ix(224),
    diff_new: ix(194),
    error: ix(124),
};

/// The themes people can choose. `Signal` is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemeName {
    #[default]
    Signal,
    Harbor,
    Dusk,
    Ember,
    Moss,
    Daybreak,
}

impl ThemeName {
    /// The order the picker lists them in.
    pub const ALL: [ThemeName; 6] = [
        ThemeName::Signal,
        ThemeName::Harbor,
        ThemeName::Dusk,
        ThemeName::Ember,
        ThemeName::Moss,
        ThemeName::Daybreak,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Signal => "Signal",
            ThemeName::Harbor => "Harbor",
            ThemeName::Dusk => "Dusk",
            ThemeName::Ember => "Ember",
            ThemeName::Moss => "Moss",
            ThemeName::Daybreak => "Daybreak",
        }
    }

    pub fn parse(value: &str) -> Option<ThemeName> {
        Self::ALL
            .into_iter()
            .find(|t| t.label().eq_ignore_ascii_case(value))
    }

    pub fn is_light(self) -> bool {
        self == ThemeName::Daybreak
    }

    fn colors(self, truecolor: bool) -> Colors {
        let (full, indexed) = match self {
            ThemeName::Signal => (SIGNAL_24BIT, SIGNAL_256),
            ThemeName::Harbor => (HARBOR_24BIT, HARBOR_256),
            ThemeName::Dusk => (DUSK_24BIT, DUSK_256),
            ThemeName::Ember => (EMBER_24BIT, EMBER_256),
            ThemeName::Moss => (MOSS_24BIT, MOSS_256),
            ThemeName::Daybreak => (DAYBREAK_24BIT, DAYBREAK_256),
        };
        if truecolor {
            full
        } else {
            indexed
        }
    }

    /// This theme's styles for a terminal that does or doesn't take 24-bit color.
    pub fn styles(self, truecolor: bool) -> Theme {
        build(self.colors(truecolor))
    }
}

/// Whether a terminal with this `COLORTERM` takes 24-bit color. It's the usual
/// way to say so, and it isn't set over SSH or sudo unless passed along, so a
/// terminal that says nothing gets indexed colors, which always work.
pub fn takes_24_bit(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some("truecolor" | "24bit"))
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

    fn all(mut check: impl FnMut(&Colors)) {
        for t in ThemeName::ALL {
            check(&t.colors(true));
            check(&t.colors(false));
        }
    }

    #[test]
    fn code_is_readable_on_every_background_it_lands_on() {
        all(|c| {
            let bgs = [c.bg, c.line, c.selection, c.diff_old];
            assert_readable("code", &code(c), &bgs);
        });
        for t in ThemeName::ALL {
            let c = t.colors(true);
            assert_readable("added lines", &code(&c), &[c.diff_new]);
        }
    }

    /// The darkest green in 256 colors still leaves code colors under the
    /// minimum on added lines, so there only plain text has to reach it.
    #[test]
    fn text_is_readable_on_added_lines_in_256_colors() {
        for t in ThemeName::ALL {
            let c = t.colors(false);
            assert_readable("added lines", &[c.fg], &[c.diff_new]);
        }
    }

    #[test]
    fn chrome_is_readable() {
        all(|c| {
            assert_readable("line numbers", &[c.faint], &[c.bg]);
            assert_readable("misspellings", &[c.error], &[c.bg, c.line]);
            assert_readable("text", &[c.fg], &[c.popup, c.status]);
            assert_readable("hints", &[c.dim], &[c.popup, c.message]);
            assert_readable("box", &[c.accent, c.warm], &[c.popup]);
            assert_readable("errors", &[c.error], &[c.status, c.message]);
            assert_readable("marks", &[c.on_accent], &[c.accent, c.warm]);
            assert_readable("matches", &[c.on_matched], &[c.matched]);
        });
    }

    #[test]
    fn the_selection_and_cursor_line_stand_out() {
        all(|c| {
            assert_ne!(c.line, c.bg);
            assert_ne!(c.selection, c.line);
            assert_ne!(c.selection, c.bg);
            assert_ne!(c.diff_old, c.diff_new);
        });
    }

    #[test]
    fn names_are_read_whatever_their_case() {
        for t in ThemeName::ALL {
            assert_eq!(ThemeName::parse(t.label()), Some(t));
            assert_eq!(ThemeName::parse(&t.label().to_lowercase()), Some(t));
        }
        assert_eq!(ThemeName::parse("sepia"), None);
    }

    #[test]
    fn only_daybreak_is_light() {
        for t in ThemeName::ALL {
            let c = t.colors(true);
            assert_eq!(t.is_light(), luminance(c.bg) > luminance(c.fg), "{t:?}");
        }
    }

    #[test]
    fn a_terminal_gets_24_bit_color_only_if_it_says_so() {
        assert!(takes_24_bit(Some("truecolor")));
        assert!(takes_24_bit(Some("24bit")));
        assert!(!takes_24_bit(None));
        assert!(!takes_24_bit(Some("")));
        assert!(!takes_24_bit(Some("yes")));
    }
}
