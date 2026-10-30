//! Rendering: `render(&App, &mut Frame)`. The bottom two rows are always the
//! status bar and the message line, so the legend never shifts the text.

pub mod chrome;
pub mod text_view;
pub mod theme;

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::app::App;
use theme::DARK;

/// Smallest supported terminal; below it Kyte draws a notice instead.
pub const MIN_WIDTH: u16 = 20;
pub const MIN_HEIGHT: u16 = 5;

pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let theme = &DARK;
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
    let text_cursor = text_view::render(app, text_area, cells, theme);
    chrome::status_bar(app, status_area, cells, theme);
    let prompt_cursor = chrome::message_line(app, msg_area, cells, theme);
    let popup_cursor = chrome::palette(app, text_area, cells, theme);
    chrome::help(app, text_area, cells, theme);
    if let Some((x, y)) = popup_cursor.or(prompt_cursor).or(text_cursor) {
        frame.set_cursor_position((x, y));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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
            row(&t, 6).contains("INSERT │ demo.txt │ Ln 1, Col 1"),
            "{}",
            row(&t, 6)
        );
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

    #[test]
    fn too_small_terminal() {
        let mut a = app("abc");
        let t = draw(&mut a, 19, 4);
        assert!(row(&t, 0).starts_with("terminal too"));
    }

    #[test]
    fn untitled_save_shows_message() {
        let mut a = App::new(Buffer::from_str("", None), &Options::default());
        a.handle_key(Key::Ctrl('s'), Instant::now()).unwrap();
        let t = draw(&mut a, 60, 6);
        assert!(row(&t, 4).contains("Untitled"));
        assert!(row(&t, 5).contains("No file name; start Kyte with a file path"));
    }
}
