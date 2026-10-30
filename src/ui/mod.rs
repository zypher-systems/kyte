//! Rendering: `render(&App, &mut Frame)`. The bottom two rows are always the
//! status bar and the message line, so the legend never shifts the text.

pub mod chrome;
pub mod text_view;
pub mod theme;

use ratatui::layout::{Position, Rect};
use ratatui::Frame;

use crate::app::App;
use theme::{Theme, DARK};

/// Smallest supported terminal; below it Kyte draws a notice instead.
pub const MIN_WIDTH: u16 = 20;
pub const MIN_HEIGHT: u16 = 5;

pub fn render(app: &App, frame: &mut Frame) {
    render_with(app, frame, &DARK);
}

/// Draws one frame in `theme`.
pub fn render_with(app: &App, frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let cells = frame.buffer_mut();
        cells.set_style(area, theme.text);
        cells.set_stringn(
            area.x,
            area.y,
            "terminal too small",
            area.width as usize,
            theme.text,
        );
        return;
    }
    let text_area = Rect::new(area.x, area.y, area.width, area.height - 2);
    let status_area = Rect::new(area.x, area.y + area.height - 2, area.width, 1);
    let msg_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    let cells = frame.buffer_mut();
    let text_cursor = if matches!(app.mode, crate::app::Mode::Recover(_)) {
        // The modal sits over an empty text view.
        cells.set_style(text_area, theme.text);
        for y in text_area.y..text_area.y + text_area.height {
            cells.set_stringn(text_area.x, y, "~", 1, theme.tilde);
        }
        None
    } else {
        text_view::render(app, text_area, cells, theme)
    };
    chrome::status_bar(app, status_area, cells, theme);
    let prompt_cursor = chrome::message_line(app, msg_area, cells, theme);
    let popup_cursor = chrome::palette(app, text_area, cells, theme);
    let help_box = chrome::help(app, text_area, cells, theme);
    let menu_box = chrome::menu(app, text_area, text_cursor, cells, theme);
    chrome::completions(app, text_area, cells, theme);
    chrome::recover(app, text_area, cells, theme);
    // A cursor under a box would blink through its text.
    let text_cursor = text_cursor.filter(|&c| {
        ![help_box, menu_box]
            .iter()
            .flatten()
            .any(|b| b.contains(Position::from(c)))
    });
    if let Some((x, y)) = popup_cursor.or(prompt_cursor).or(text_cursor) {
        frame.set_cursor_position((x, y));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::backend::TestBackend;
    use ratatui::layout::{Position, Rect};
    use ratatui::Terminal;

    use super::chrome;
    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::cli::Options;
    use crate::input::Key;

    fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        app.layout(w, h);
        t.draw(|f| super::render(app, f)).unwrap();
        t
    }

    fn row(t: &Terminal<TestBackend>, y: u16) -> String {
        let b = t.backend().buffer();
        let mut s = String::new();
        for x in 0..b.area.width {
            s.push_str(b[(x, y)].symbol());
        }
        s.trim_end().to_string()
    }

    fn app(text: &str) -> App {
        App::new(
            Buffer::from_str(text, Some("/tmp/demo.txt".into())),
            None,
            &Options::default(),
        )
    }

    #[test]
    fn layout_has_text_status_and_message_rows() {
        let mut a = app("fn main() {\n    hi\n}");
        let t = draw(&mut a, 40, 8);
        assert_eq!(row(&t, 0), "  1 │ fn main() {");
        assert_eq!(row(&t, 1), "  2 │     hi");
        assert_eq!(row(&t, 3), "~");
        assert!(
            row(&t, 6).starts_with(" INSERT  demo.txt"),
            "{}",
            row(&t, 6)
        );
        assert!(row(&t, 6).ends_with("Ln 1, Col 1"), "{}", row(&t, 6));
    }

    #[test]
    fn the_status_bar_keeps_position_and_warning_when_tight() {
        let mut a = app("abc");
        let wide = draw(&mut a, 100, 8);
        assert!(row(&wide, 6).contains("no crash recovery"));
        assert!(row(&wide, 6).ends_with("Ln 1, Col 1"), "{}", row(&wide, 6));
        let narrow = draw(&mut a, 40, 8);
        assert!(row(&narrow, 6).contains("no cr"), "{}", row(&narrow, 6));
        assert!(
            row(&narrow, 6).ends_with("Ln 1, Col 1"),
            "{}",
            row(&narrow, 6)
        );
        assert!(row(&narrow, 6).contains(" Ln"), "{}", row(&narrow, 6));
    }

    #[test]
    fn cursor_moves_show_in_status_bar() {
        let mut a = app("abc\ndef");
        let now = Instant::now();
        a.handle_key(Key::Down, now).unwrap();
        a.handle_key(Key::Right, now).unwrap();
        let mut t = draw(&mut a, 40, 8);
        assert!(row(&t, 6).contains("Ln 2, Col 2"));
        assert_eq!(t.get_cursor_position().unwrap(), (7, 1).into());
        a.handle_key(Key::Char('x'), now).unwrap();
        t = draw(&mut a, 40, 8);
        assert!(row(&t, 6).contains("demo.txt [+]"));
    }

    #[test]
    fn tabs_and_wide_chars_render_at_right_columns() {
        let mut a = app("a\tb\n世界x");
        let now = Instant::now();
        a.handle_key(Key::Down, now).unwrap();
        a.handle_key(Key::End, now).unwrap();
        let mut t = draw(&mut a, 40, 8);
        let b = t.backend().buffer().clone();
        // gutter is 6 wide; tab expands a to col 4, so b is at 6 + 4
        assert_eq!(b[(10, 0)].symbol(), "b");
        assert_eq!(b[(6, 1)].symbol(), "世");
        assert_eq!(b[(8, 1)].symbol(), "界");
        assert_eq!(b[(10, 1)].symbol(), "x");
        assert_eq!(t.get_cursor_position().unwrap(), (11, 1).into());
    }

    #[test]
    fn scrolls_to_keep_cursor_visible() {
        let text: String = (1..=50).map(|i| format!("line {i}\n")).collect();
        let mut a = app(&text);
        a.handle_key(Key::CtrlEnd, Instant::now()).unwrap();
        let t = draw(&mut a, 40, 10);
        assert!(row(&t, 7).contains("51 │"), "{}", row(&t, 7));
        assert!(row(&t, 6).contains("50 │ line 50"));
    }

    #[test]
    fn horizontal_scroll_keeps_cursor_visible() {
        let mut a = app(&"x".repeat(100));
        a.handle_key(Key::End, Instant::now()).unwrap();
        let mut t = draw(&mut a, 30, 6);
        let x = t.get_cursor_position().unwrap().x;
        assert!(x < 30);
    }

    fn screen(t: &Terminal<TestBackend>, h: u16) -> String {
        (0..h).map(|y| row(t, y)).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn esc_layer_is_a_floating_menu_not_a_footer() {
        let mut a = app("abc");
        a.handle_key(Key::Esc, Instant::now()).unwrap();
        let t = draw(&mut a, 80, 24);
        assert!(!row(&t, 23).contains("del word"), "the footer stays clear");
        let all = screen(&t, 22);
        for (k, label) in a.layer_rows() {
            assert!(all.contains(&format!("{k} {label}")), "{k} missing\n{all}");
        }
    }

    #[test]
    fn the_cursor_line_is_picked_out_and_its_bar_stays_quiet() {
        let t = super::theme::DARK;
        let mut a = app("one
two");
        let term = draw(&mut a, 40, 8);
        let b = term.backend().buffer();
        assert_eq!(b[(12, 0)].bg, t.current_line.bg.unwrap());
        assert_eq!(b[(12, 1)].bg, t.text.bg.unwrap());
        assert_eq!(b[(2, 0)].fg, t.gutter_current.fg.unwrap());
        assert_eq!(b[(4, 0)].symbol(), "│");
        assert_eq!(b[(4, 0)].fg, t.gutter.fg.unwrap());
    }

    #[test]
    fn a_box_casts_a_shadow_only_on_the_text() {
        let t = super::theme::DARK;
        let mut a = app("abc");
        a.handle_key(Key::Esc, Instant::now()).unwrap();
        let term = draw(&mut a, 80, 24);
        let r = box_of(&term);
        let b = term.backend().buffer();
        let shadow = t.shadow.bg.unwrap();
        assert_eq!(b[(r.right(), r.y + 1)].bg, shadow, "right of the box");
        assert_eq!(b[(r.right() + 1, r.y + 1)].bg, shadow, "two cells wide");
        assert_eq!(b[(r.x + 2, r.bottom())].bg, shadow, "under the box");
        assert_ne!(b[(r.right(), r.y)].bg, shadow, "the corner stays clear");
        let (w, h) = (20, 7);
        let mut a = app("abc");
        a.run_command(crate::command::Command::ShowKeyHelp);
        let term = draw(&mut a, w, h);
        let b = term.backend().buffer();
        for x in 0..w {
            assert_ne!(b[(x, h - 2)].bg, shadow, "the status bar stays clear");
        }
    }

    #[test]
    fn a_long_menu_splits_into_two_columns() {
        let mut a = app("fn main() {}");
        a.handle_key(Key::Esc, Instant::now()).unwrap();
        let rows = a.layer_rows().len() as u16;
        assert!(rows > 10, "the test needs a long menu, got {rows}");
        let r = box_of(&draw(&mut a, 80, 24));
        assert!(r.height <= rows.div_ceil(2) + 4, "{r:?} for {rows} rows");
    }

    #[test]
    fn every_menu_key_stays_visible_on_a_short_terminal() {
        let mut a = app("abc");
        a.handle_key(Key::Esc, Instant::now()).unwrap();
        let t = draw(&mut a, 80, 8);
        let all = screen(&t, 6);
        for (k, _) in a.layer_rows() {
            assert!(all.contains(k), "{k} missing\n{all}");
        }
    }

    /// The box's edges, found by the popup background.
    fn box_of(t: &Terminal<TestBackend>) -> Rect {
        let b = t.backend().buffer();
        let bg = super::theme::DARK.palette.bg.unwrap();
        let area = b.area;
        let hit: Vec<(u16, u16)> = (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .filter(|&(x, y)| b[(x, y)].bg == bg)
            .collect();
        let (x, y) = (
            hit.iter().map(|p| p.0).min().unwrap(),
            hit.iter().map(|p| p.1).min().unwrap(),
        );
        let (right, bottom) = (
            hit.iter().map(|p| p.0).max().unwrap(),
            hit.iter().map(|p| p.1).max().unwrap(),
        );
        Rect::new(x, y, right - x + 1, bottom - y + 1)
    }

    fn assert_centered(r: Rect, w: u16, text_h: u16) {
        let (left, right) = (r.x, w - r.x - r.width);
        let (top, bottom) = (r.y, text_h - r.y - r.height);
        assert!(left.abs_diff(right) <= 1, "{r:?} is off centre across");
        assert!(top.abs_diff(bottom) <= 1, "{r:?} is off centre down");
    }

    #[test]
    fn the_esc_layer_sits_in_the_middle_wherever_the_cursor_is() {
        let text: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let now = Instant::now();
        for lines in [0, 10, 18, 40] {
            let mut a = app(&text);
            for _ in 0..lines {
                a.handle_key(Key::Down, now).unwrap();
            }
            a.handle_key(Key::Esc, now).unwrap();
            assert_centered(box_of(&draw(&mut a, 80, 24)), 80, 22);
        }
    }

    #[test]
    fn a_question_sits_in_the_middle() {
        let now = Instant::now();
        let mut a = app("abc");
        a.handle_key(Key::Char('x'), now).unwrap();
        a.run_command(crate::command::Command::Quit);
        assert_centered(box_of(&draw(&mut a, 80, 24)), 80, 22);
    }

    #[test]
    fn the_cursor_never_blinks_through_a_box() {
        let text = format!("{}\n", "x".repeat(70)).repeat(40);
        let now = Instant::now();
        let mut a = app(&text);
        for _ in 0..11 {
            a.handle_key(Key::Down, now).unwrap();
        }
        for _ in 0..34 {
            a.handle_key(Key::Right, now).unwrap();
        }
        let t = draw(&mut a, 80, 24);
        assert!(t.backend().cursor_visible());
        let at = t.backend().cursor_position();
        a.handle_key(Key::Esc, now).unwrap();
        let t = draw(&mut a, 80, 24);
        assert!(box_of(&t).contains(Position::new(at.x, at.y)));
        assert!(!t.backend().cursor_visible());
    }

    #[test]
    fn replace_keeps_the_line_in_question_showing() {
        let text: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let now = Instant::now();
        for lines in [0, 9, 10, 11, 12, 13, 39] {
            let mut a = app(&text);
            for _ in 0..lines {
                a.handle_key(Key::Down, now).unwrap();
            }
            a.run_command(crate::command::Command::Replace);
            for c in "line".chars() {
                a.handle_key(Key::Char(c), now).unwrap();
            }
            a.handle_key(Key::Enter, now).unwrap();
            for c in "row".chars() {
                a.handle_key(Key::Char(c), now).unwrap();
            }
            a.handle_key(Key::Enter, now).unwrap();
            let mut t = draw(&mut a, 80, 24);
            let pos = t.get_cursor_position().unwrap();
            let r = box_of(&t);
            assert!(
                pos.y < r.y || pos.y >= r.y + r.height,
                "the box covers line {lines}: {r:?} over row {}",
                pos.y
            );
            assert!(r.x.abs_diff(80 - r.x - r.width) <= 1, "{r:?} is off centre");
        }
    }

    #[test]
    fn a_box_with_no_room_on_either_side_stays_put() {
        let area = Rect::new(0, 0, 40, 6);
        let mid = chrome::centered(area, 10, 5);
        let got = chrome::centered_off_row(area, 10, 5, Some((3, 2)));
        assert_eq!(got, mid);
    }

    #[test]
    fn help_scrolls_to_every_row_at_any_height() {
        for (w, h) in [(80, 24), (40, 10), (100, 30), (20, 5)] {
            let mut a = app("abc");
            a.run_command(crate::command::Command::ShowKeyHelp);
            let now = Instant::now();
            draw(&mut a, w, h);
            let mut seen = screen(&draw(&mut a, w, h), h);
            for _ in 0..crate::command::KEY_HELP.len() {
                a.handle_key(Key::Down, now).unwrap();
                seen.push_str(&screen(&draw(&mut a, w, h), h));
            }
            if w >= 80 {
                for (k, _) in crate::command::KEY_HELP {
                    assert!(seen.contains(k.trim_end()), "{k} missing at {w}x{h}");
                }
            }
            for _ in 0..crate::command::KEY_HELP.len() {
                a.handle_key(Key::Up, now).unwrap();
            }
            assert_eq!(a.mode, crate::app::Mode::Help { top: 0 });
        }
    }

    #[test]
    fn too_small_terminal() {
        let mut a = app("abc");
        let t = draw(&mut a, 19, 4);
        assert!(row(&t, 0).starts_with("terminal too"));
    }

    #[test]
    fn untitled_save_shows_message() {
        let mut a = App::new(Buffer::from_str("", None), None, &Options::default());
        a.handle_key(Key::Ctrl('s'), Instant::now()).unwrap();
        let t = draw(&mut a, 60, 6);
        assert!(row(&t, 4).contains("Untitled"));
        assert!(row(&t, 5).contains("Save as:"));
    }
}
