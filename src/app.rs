//! Application state and the mode state machine (sections 5 and 8).
//! `handle_key` takes `now` so tests control time.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::buffer::{Buffer, GroupKind};
use crate::cli::Options;
use crate::fileio;
use crate::input::{normalize, Input, Key};
use crate::ops;
use crate::palette::PaletteState;
use crate::search;

/// Text-area rows when the terminal size is not known yet.
const DEFAULT_TEXT_HEIGHT: usize = 28;

/// Which layer has the keyboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Insert,
    /// The one-shot operator layer, opened by Esc, Ctrl-Space or an Alt chord.
    Overlay {
        opened_at: Instant,
    },
    Palette(PaletteState),
    Prompt(Prompt),
}

/// One-line prompts on the message line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// `origin` is the cursor to restore on Esc when nothing matched.
    Search {
        query: String,
        origin: usize,
    },
    ConfirmQuit,
}

pub struct App {
    pub buffer: Buffer,
    pub mode: Mode,
    /// Shown on the message line until the next key.
    pub message: Option<String>,
    /// The message is an error (drawn in the error style).
    pub message_is_error: bool,
    pub show_line_numbers: bool,
    pub esc_window: Duration,
    pub quit: bool,
    scroll_top: usize,
    scroll_left: usize,
    view: (u16, u16),
}

/// Shown on the message line only while the overlay is open (section 5.4).
pub const LEGEND: &[(&str, &str)] = &[
    ("w", "del word"),
    ("d", "del line"),
    ("k", "del to end"),
    ("%", "bracket"),
    ("/", "search"),
    ("u", "undo"),
    ("r", "redo"),
    ("s", "save"),
    ("Esc Esc", "palette"),
];

pub const CONFIRM_QUIT_TEXT: &str =
    "Unsaved changes! Quit without saving? Ctrl-Q or y quits, any other key cancels";

impl App {
    pub fn new(buffer: Buffer, opts: &Options) -> App {
        App {
            buffer,
            mode: Mode::Insert,
            message: None,
            message_is_error: false,
            show_line_numbers: true,
            esc_window: Duration::from_millis(opts.esc_ms),
            quit: false,
            scroll_top: 0,
            scroll_left: 0,
            view: (0, 0),
        }
    }

    /// (top line, left display column) of the text view.
    pub fn scroll(&self) -> (usize, usize) {
        (self.scroll_top, self.scroll_left)
    }

    fn text_height(&self) -> usize {
        if self.view.1 >= 3 {
            self.view.1 as usize - 2
        } else {
            DEFAULT_TEXT_HEIGHT
        }
    }

    /// Records the terminal size and scrolls so the cursor stays visible.
    /// Call before every render.
    pub fn layout(&mut self, width: u16, height: u16) {
        self.view = (width, height);
        let h = (height as usize).saturating_sub(2).max(1);
        let gw =
            crate::ui::text_view::gutter_width(self.buffer.line_count(), self.show_line_numbers);
        let w = (width as usize).saturating_sub(gw).max(1);
        let line = self.buffer.cursor_line();
        if line < self.scroll_top {
            self.scroll_top = line;
        } else if line >= self.scroll_top + h {
            self.scroll_top = line + 1 - h;
        }
        let max_top = self.buffer.line_count().saturating_sub(1);
        self.scroll_top = self.scroll_top.min(max_top);
        let col = self.buffer.cursor_display_col();
        if col < self.scroll_left {
            self.scroll_left = col;
        } else if col >= self.scroll_left + w {
            self.scroll_left = col + 1 - w;
        }
    }

    fn set_message(&mut self, m: impl Into<String>) {
        self.message = Some(m.into());
        self.message_is_error = false;
    }

    fn set_error(&mut self, m: impl Into<String>) {
        self.message = Some(m.into());
        self.message_is_error = true;
    }

    /// Dispatches one input.
    pub fn handle_input(&mut self, input: Input, now: Instant) -> Result<()> {
        match input {
            Input::Key(k) => self.handle_key(k, now)?,
            Input::Paste(text) => self.handle_paste(text)?,
            Input::Resize(..) => {}
        }
        Ok(())
    }

    pub fn handle_key(&mut self, key: Key, now: Instant) -> Result<()> {
        if let Key::Alt(_) = key {
            if self.mode == Mode::Insert {
                for k in normalize(key, true) {
                    self.handle_key(k, now)?;
                }
                return Ok(());
            }
        }
        self.message = None;
        self.message_is_error = false;
        let mode = self.mode.clone();
        match mode {
            Mode::Insert => self.insert_key(key, now),
            Mode::Overlay { opened_at } => self.overlay_key(key, opened_at, now),
            Mode::Palette(state) => self.palette_key(key, state),
            Mode::Prompt(Prompt::Search { query, origin }) => self.search_key(key, query, origin),
            Mode::Prompt(Prompt::ConfirmQuit) => {
                self.mode = Mode::Insert;
                if matches!(key, Key::Ctrl('q') | Key::Char('y')) {
                    self.quit = true;
                }
            }
        }
        Ok(())
    }

    fn insert_key(&mut self, key: Key, now: Instant) {
        let b = &mut self.buffer;
        match key {
            Key::Esc | Key::Ctrl(' ') => self.mode = Mode::Overlay { opened_at: now },
            Key::Ctrl('p') => self.open_palette(),
            Key::Ctrl('f') => self.open_search(),
            Key::Char(c) => b.insert_str(&c.to_string(), GroupKind::Typing),
            Key::Enter => b.insert_str("\n", GroupKind::Typing),
            Key::Tab => b.insert_str("\t", GroupKind::Typing),
            Key::Backspace => b.backspace(),
            Key::Delete => b.delete_forward(),
            Key::Left => b.move_left(),
            Key::Right => b.move_right(),
            Key::Up => b.move_up(),
            Key::Down => b.move_down(),
            Key::Home => b.move_home(),
            Key::End => b.move_end(),
            Key::PageUp => {
                let h = self.text_height() as isize;
                self.buffer.move_lines(-h);
            }
            Key::PageDown => {
                let h = self.text_height() as isize;
                self.buffer.move_lines(h);
            }
            Key::CtrlHome => b.move_to(0),
            Key::CtrlEnd => {
                let n = b.len_chars();
                b.move_to(n);
            }
            Key::Ctrl('s') | Key::Ctrl('o') => self.save(),
            Key::Ctrl('q') | Key::Ctrl('x') => self.request_quit(),
            Key::Ctrl('z') => self.undo(),
            Key::Ctrl('y') => self.redo(),
            _ => {}
        }
    }

    fn overlay_key(&mut self, key: Key, opened_at: Instant, now: Instant) {
        self.mode = Mode::Insert;
        match key {
            Key::Esc => {
                if now.saturating_duration_since(opened_at) <= self.esc_window {
                    self.open_palette();
                }
            }
            Key::Char('w') => self.op_word(),
            Key::Char('d') => self.op_line(),
            Key::Char('k') => self.op_kill(),
            Key::Char('%') => self.op_bracket(),
            Key::Char('/') => self.open_search(),
            Key::Char('u') => self.undo(),
            Key::Char('r') => self.redo(),
            Key::Char('s') => self.save(),
            // Any other key closes the overlay and is discarded.
            _ => {}
        }
    }

    /// Opens the search prompt (Ctrl-F, Esc /, or the palette's Find).
    pub fn open_search(&mut self) {
        self.mode = Mode::Prompt(Prompt::Search {
            query: String::new(),
            origin: self.buffer.cursor(),
        });
    }

    fn search_key(&mut self, key: Key, mut query: String, origin: usize) {
        match key {
            Key::Esc => {
                let at = self.buffer.cursor();
                if !search::matches_at(self.buffer.rope(), &query, at) {
                    self.buffer.move_to(origin);
                }
                self.mode = Mode::Insert;
                return;
            }
            Key::Enter => {
                if !query.is_empty() {
                    let at = self.buffer.cursor();
                    match search::find_next(self.buffer.rope(), &query, at) {
                        Some(i) => self.buffer.move_to(i),
                        None => self.set_error("No match"),
                    }
                }
            }
            Key::Char(c) => {
                query.push(c);
                self.incremental(&query, origin);
            }
            Key::Backspace => {
                query.pop();
                self.incremental(&query, origin);
            }
            _ => {}
        }
        self.mode = Mode::Prompt(Prompt::Search { query, origin });
    }

    /// Jumps to the first match at or after `origin`, or back to `origin`.
    fn incremental(&mut self, query: &str, origin: usize) {
        if query.is_empty() {
            self.buffer.move_to(origin);
            return;
        }
        match search::find_from(self.buffer.rope(), query, origin) {
            Some(i) => self.buffer.move_to(i),
            None => {
                self.buffer.move_to(origin);
                self.set_error("No match");
            }
        }
    }

    fn open_palette(&mut self) {
        self.mode = Mode::Palette(PaletteState::default());
    }

    fn palette_key(&mut self, key: Key, mut state: PaletteState) {
        match key {
            Key::Esc => self.mode = Mode::Insert,
            Key::Char(c) => {
                state.query.push(c);
                self.mode = Mode::Palette(state);
            }
            Key::Backspace => {
                state.query.pop();
                self.mode = Mode::Palette(state);
            }
            _ => self.mode = Mode::Palette(state),
        }
    }

    fn delete_range(&mut self, r: std::ops::Range<usize>, cursor_after: usize) {
        if r.is_empty() {
            return;
        }
        let removed = self.buffer.slice_string(r.clone());
        self.buffer.apply_with_cursor(
            crate::buffer::Edit {
                at: r.start,
                removed,
                inserted: String::new(),
            },
            GroupKind::Operator,
            cursor_after,
        );
    }

    /// `w`: delete to the start of the next word.
    fn op_word(&mut self) {
        let at = self.buffer.cursor();
        let end = ops::word_end(self.buffer.rope(), at);
        self.delete_range(at..end, at);
    }

    /// `d`: delete the current line; the cursor keeps its column on the line
    /// that takes its place, clamped to that line's last character.
    fn op_line(&mut self) {
        let at = self.buffer.cursor();
        let (_, col) = self.buffer.line_col();
        let r = ops::line_range(self.buffer.rope(), at);
        if r.is_empty() {
            return;
        }
        let start = r.start;
        self.delete_range(r, start);
        let line = self
            .buffer
            .rope()
            .char_to_line(start.min(self.buffer.len_chars()));
        let text = self.buffer.line_text(line);
        let graphemes = crate::buffer::graphemes_with_char_offsets(&text);
        let idx = (col - 1).min(graphemes.len().saturating_sub(1));
        let off = graphemes.get(idx).map(|(o, _)| *o).unwrap_or(0);
        let target = self.buffer.line_start(line) + off;
        self.buffer.move_to(target);
    }

    /// `k`: delete to end of line, or join with the next line at the end.
    fn op_kill(&mut self) {
        let at = self.buffer.cursor();
        let r = ops::kill_range(self.buffer.rope(), at);
        self.delete_range(r, at);
    }

    /// `%`: jump to the matching bracket.
    fn op_bracket(&mut self) {
        match ops::match_bracket(self.buffer.rope(), self.buffer.cursor()) {
            Some(t) => self.buffer.move_to(t),
            None => self.set_error("No matching bracket"),
        }
    }

    fn undo(&mut self) {
        if !self.buffer.undo() {
            self.set_message("Nothing to undo");
        }
    }

    fn redo(&mut self) {
        if !self.buffer.redo() {
            self.set_message("Nothing to redo");
        }
    }

    pub fn handle_paste(&mut self, text: String) -> Result<()> {
        self.message = None;
        match &mut self.mode {
            Mode::Insert | Mode::Overlay { .. } => {
                self.mode = Mode::Insert;
                self.buffer.insert_str(&text, GroupKind::Paste);
            }
            Mode::Palette(state) => {
                state.query.push_str(&text.replace('\n', " "));
            }
            Mode::Prompt(_) => {}
        }
        Ok(())
    }

    /// Called when the poll timeout elapses with no input.
    pub fn tick(&mut self, now: Instant) -> Result<()> {
        let _ = now;
        Ok(())
    }

    /// Ctrl-S: atomic save.
    pub fn save(&mut self) {
        let Some(path) = self.buffer.path().map(|p| p.to_path_buf()) else {
            self.set_error("No file name; start Kyte with a file path");
            return;
        };
        let text = self.buffer.to_string();
        match fileio::save_atomic(&path, &text) {
            Ok(()) => {
                self.buffer.mark_clean();
                let n = self.buffer.saved_line_count();
                self.set_message(format!("Saved {n} line{}", if n == 1 { "" } else { "s" }));
            }
            Err(e) => self.set_error(format!("Save failed: {e}")),
        }
    }

    /// Ctrl-Q: quit now when clean, otherwise ask.
    pub fn request_quit(&mut self) {
        if self.buffer.is_dirty() {
            self.mode = Mode::Prompt(Prompt::ConfirmQuit);
        } else {
            self.quit = true;
        }
    }

    /// Label for the status bar mode field.
    pub fn mode_label(&self) -> &'static str {
        match &self.mode {
            Mode::Insert => "INSERT",
            Mode::Overlay { .. } => "ESC",
            Mode::Palette(_) => "PALETTE",
            Mode::Prompt(Prompt::Search { .. }) => "SEARCH",
            Mode::Prompt(_) => "PROMPT",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(text: &str) -> App {
        App::new(Buffer::from_str(text, None), &Options::default())
    }

    fn ms(t: Instant, n: u64) -> Instant {
        t + Duration::from_millis(n)
    }

    #[test]
    fn insert_esc_opens_overlay_instantly() {
        let mut a = app("x");
        let t = Instant::now();
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Overlay { opened_at: t });
        assert_eq!(a.mode_label(), "ESC");
    }

    #[test]
    fn ctrl_space_opens_overlay() {
        let mut a = app("x");
        let t = Instant::now();
        a.handle_key(Key::Ctrl(' '), t).unwrap();
        assert!(matches!(a.mode, Mode::Overlay { .. }));
    }

    #[test]
    fn esc_within_window_opens_palette() {
        let mut a = app("x");
        let t = Instant::now();
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Esc, ms(t, 299)).unwrap();
        assert!(matches!(a.mode, Mode::Palette(_)));
        assert_eq!(a.buffer.to_string(), "x");
    }

    #[test]
    fn esc_after_window_closes_overlay() {
        let mut a = app("x");
        let t = Instant::now();
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Esc, ms(t, 500)).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert!(!a.buffer.is_dirty());
    }

    #[test]
    fn custom_window_800ms() {
        let opts = Options {
            esc_ms: 800,
            ..Options::default()
        };
        let mut a = App::new(Buffer::from_str("", None), &opts);
        let t = Instant::now();
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Esc, ms(t, 500)).unwrap();
        assert!(matches!(a.mode, Mode::Palette(_)));
    }

    #[test]
    fn palette_esc_returns_to_insert() {
        let mut a = app("");
        let t = Instant::now();
        a.handle_key(Key::Ctrl('p'), t).unwrap();
        assert!(matches!(a.mode, Mode::Palette(_)));
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
    }

    #[test]
    fn unknown_overlay_key_closes_and_is_discarded() {
        let mut a = app("abc");
        let t = Instant::now();
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('z'), t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert_eq!(a.buffer.to_string(), "abc");
        assert!(!a.buffer.is_dirty());
    }

    #[test]
    fn alt_key_in_insert_is_esc_then_key() {
        let mut a = app("one\ntwo\n");
        a.handle_key(Key::Alt('d'), Instant::now()).unwrap();
        assert_eq!(a.buffer.to_string(), "two\n");
        assert_eq!(a.mode, Mode::Insert);
    }

    #[test]
    fn operators_are_one_undo_step_and_return_to_insert() {
        let t = Instant::now();
        let mut a = app("foo bar baz");
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('w'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "bar baz");
        assert_eq!(a.mode, Mode::Insert);
        a.handle_key(Key::Ctrl('z'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "foo bar baz");
    }

    #[test]
    fn d_cursor_lands_on_following_line_clamped() {
        let t = Instant::now();
        let mut a = app("a\nbb\nc");
        a.buffer.move_to(3); // b|b
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('d'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "a\nc");
        assert_eq!(a.buffer.cursor(), 2);
        let mut b = app("a\nlong\nxyz");
        b.buffer.move_to(4); // lo|ng
        b.handle_key(Key::Esc, t).unwrap();
        b.handle_key(Key::Char('d'), t).unwrap();
        assert_eq!(b.buffer.to_string(), "a\nxyz");
        assert_eq!(b.buffer.line_col(), (2, 3));
    }

    #[test]
    fn d_on_last_line_moves_to_previous_line() {
        let t = Instant::now();
        let mut a = app("a\nb\nc");
        a.buffer.move_to(4);
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('d'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "a\nb");
        assert_eq!(a.buffer.line_col(), (2, 1));
    }

    #[test]
    fn k_and_percent_through_overlay() {
        let t = Instant::now();
        let mut a = app("ab(cd)\nef");
        a.buffer.move_to(2);
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('%'), t).unwrap();
        assert_eq!(a.buffer.cursor(), 5);
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('k'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "ab(cd\nef");
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('k'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "ab(cdef");
    }

    #[test]
    fn percent_unmatched_shows_message() {
        let t = Instant::now();
        let mut a = app("(ab");
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('%'), t).unwrap();
        assert_eq!(a.message.as_deref(), Some("No matching bracket"));
        assert_eq!(a.buffer.cursor(), 0);
    }

    #[test]
    fn overlay_undo_redo() {
        let t = Instant::now();
        let mut a = app("");
        for c in "abc".chars() {
            a.handle_key(Key::Char(c), t).unwrap();
        }
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('u'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "");
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('r'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "abc");
    }

    #[test]
    fn confirm_quit_transitions() {
        let t = Instant::now();
        let mut a = app("");
        a.handle_key(Key::Char('x'), t).unwrap();
        a.handle_key(Key::Ctrl('q'), t).unwrap();
        assert_eq!(a.mode, Mode::Prompt(Prompt::ConfirmQuit));
        a.handle_key(Key::Char('n'), t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert!(!a.quit);
        a.handle_key(Key::Ctrl('q'), t).unwrap();
        a.handle_key(Key::Char('y'), t).unwrap();
        assert!(a.quit);
    }

    fn search_app() -> App {
        app("foo bar baz")
    }

    fn typ(a: &mut App, s: &str) {
        for c in s.chars() {
            a.handle_key(Key::Char(c), Instant::now()).unwrap();
        }
    }

    #[test]
    fn search_incremental_enter_wrap_and_esc() {
        let t = Instant::now();
        let mut a = search_app();
        a.handle_key(Key::Ctrl('f'), t).unwrap();
        typ(&mut a, "ba");
        assert_eq!(a.buffer.cursor(), 4);
        a.handle_key(Key::Enter, t).unwrap();
        assert_eq!(a.buffer.cursor(), 8);
        a.handle_key(Key::Enter, t).unwrap();
        assert_eq!(a.buffer.cursor(), 4);
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert_eq!(a.buffer.cursor(), 4);
    }

    #[test]
    fn search_no_match_restores_origin() {
        let t = Instant::now();
        let mut a = search_app();
        a.buffer.move_to(2);
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('/'), t).unwrap();
        assert!(matches!(a.mode, Mode::Prompt(Prompt::Search { .. })));
        typ(&mut a, "zz");
        assert_eq!(a.buffer.cursor(), 2);
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.buffer.cursor(), 2);
    }

    #[test]
    fn search_empty_query_does_nothing() {
        let t = Instant::now();
        let mut a = search_app();
        a.handle_key(Key::Ctrl('f'), t).unwrap();
        a.handle_key(Key::Enter, t).unwrap();
        assert_eq!(a.buffer.cursor(), 0);
        assert!(matches!(a.mode, Mode::Prompt(Prompt::Search { .. })));
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.buffer.cursor(), 0);
        assert!(!a.buffer.is_dirty());
    }

    #[test]
    fn clean_quit_is_immediate() {
        let mut a = app("");
        a.handle_key(Key::Ctrl('x'), Instant::now()).unwrap();
        assert!(a.quit);
    }
}
