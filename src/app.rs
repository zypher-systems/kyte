//! Application state and the mode state machine (sections 5 and 8).
//! `handle_key` takes `now` so tests control time.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::buffer::{Buffer, GroupKind};
use crate::cli::Options;
use crate::fileio;
use crate::input::{normalize, Input, Key};

/// Text-area rows when the terminal size is not known yet.
const DEFAULT_TEXT_HEIGHT: usize = 28;

/// Which layer has the keyboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Insert,
    Prompt(Prompt),
}

/// One-line prompts on the message line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
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
            Input::Key(k) => {
                for key in normalize(k, self.mode == Mode::Insert) {
                    self.handle_key(key, now)?;
                }
            }
            Input::Paste(text) => self.handle_paste(text)?,
            Input::Resize(..) => {}
        }
        Ok(())
    }

    pub fn handle_key(&mut self, key: Key, now: Instant) -> Result<()> {
        let _ = now;
        self.message = None;
        self.message_is_error = false;
        let mode = self.mode.clone();
        match mode {
            Mode::Insert => self.insert_key(key),
            Mode::Prompt(Prompt::ConfirmQuit) => {
                self.mode = Mode::Insert;
                if matches!(key, Key::Ctrl('q') | Key::Char('y')) {
                    self.quit = true;
                }
            }
        }
        Ok(())
    }

    fn insert_key(&mut self, key: Key) {
        let b = &mut self.buffer;
        match key {
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
        if self.mode == Mode::Insert {
            self.buffer.insert_str(&text, GroupKind::Paste);
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
            Mode::Prompt(_) => "PROMPT",
        }
    }
}
