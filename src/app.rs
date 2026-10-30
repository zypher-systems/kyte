//! Application state and the mode state machine (sections 5 and 8).
//! `handle_key` takes `now` so tests control time.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::buffer::{Buffer, GroupKind};
use crate::cli::Options;
use crate::command::{self, Command};
use crate::fileio::{self, DiskState};
use crate::input::{normalize, Input, Key};
use crate::ops;
use crate::palette::PaletteState;
use crate::search;
use crate::swap::{self, RecoverInfo, SwapStatus, SwapWriter};

/// How soon after Esc a `[` or `O` counts as the rest of a split key sequence.
/// A person can't reach those keys this fast, and they do nothing in the layer.
const ESC_TAIL_WINDOW: Duration = Duration::from_millis(150);

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
    /// The "Show key help" modal; Esc closes it.
    Help,
    /// Startup modal for a recoverable (or stale) swap.
    Recover(RecoverInfo),
}

/// One-line prompts on the message line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// `origin` is the cursor to restore on Esc when nothing matched.
    Search {
        query: String,
        origin: usize,
    },
    GotoLine {
        text: String,
    },
    ConfirmQuit,
    /// Asks for a file name. `quit_after` quits once the save succeeds.
    SaveAs {
        text: String,
        quit_after: bool,
    },
    /// A save that needs a yes first. `quit_after` quits once it succeeds.
    ConfirmSave {
        risk: SaveRisk,
        path: std::path::PathBuf,
        quit_after: bool,
    },
}

/// Why a save stopped to ask, in the order the questions are asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveRisk {
    ChangedOnDisk,
    Exists,
    ReadOnly,
}

impl SaveRisk {
    pub fn question(self) -> &'static str {
        match self {
            SaveRisk::ChangedOnDisk => {
                "File changed on disk! y overwrites those changes, n or Esc cancels"
            }
            SaveRisk::Exists => "File exists! y overwrites it, n or Esc cancels",
            SaveRisk::ReadOnly => "File is read-only! y replaces it anyway, n or Esc cancels",
        }
    }
}

/// The rest of a key sequence whose Esc arrived alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscTail {
    /// `[`, then parameters, then one final character.
    Csi,
    /// `O`, then one character.
    Ss3,
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
    /// Leave the swap on disk at exit (Recover modal `q`).
    pub keep_swap: bool,
    swap: Option<SwapWriter>,
    /// Canonical path the swap is keyed by.
    swap_target: Option<std::path::PathBuf>,
    /// The file as last read or written; `None` means it's not known.
    disk: Option<DiskState>,
    /// A swap write failed, so the swap no longer holds every edit.
    swap_failed: bool,
    /// Characters still to discard from a split key sequence.
    esc_tail: Option<(EscTail, Instant)>,
    last_fsync: Option<Instant>,
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

pub const CONFIRM_QUIT_TEXT: &str = "Unsaved changes! y saves and quits, n discards, Esc cancels";

impl App {
    pub fn new(buffer: Buffer, swap: Option<SwapWriter>, opts: &Options) -> App {
        let swap_target = buffer.path().and_then(|p| fileio::resolve_target(p).ok());
        let disk = Some(DiskState::of_rope(buffer.rope()));
        App {
            keep_swap: false,
            swap,
            swap_target,
            disk,
            swap_failed: false,
            esc_tail: None,
            last_fsync: None,
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

    /// Handles one key, then appends the resulting edits to the swap.
    pub fn handle_key(&mut self, key: Key, now: Instant) -> Result<()> {
        if self.swallow_esc_tail(&key, now) {
            return Ok(());
        }
        if let Key::Alt(_) = key {
            if self.mode == Mode::Insert {
                for k in normalize(key, true) {
                    self.handle_key(k, now)?;
                }
                return Ok(());
            }
        }
        self.key_inner(key, now);
        self.flush_edits();
        Ok(())
    }

    /// Discards the rest of a split key sequence, so its characters aren't typed.
    fn swallow_esc_tail(&mut self, key: &Key, now: Instant) -> bool {
        let Some((tail, at)) = self.esc_tail.take() else {
            return false;
        };
        let Key::Char(c) = *key else {
            return false;
        };
        if now.saturating_duration_since(at) > ESC_TAIL_WINDOW {
            return false;
        }
        let more = tail == EscTail::Csi && matches!(c, ' '..='?');
        if more {
            self.esc_tail = Some((tail, now));
        }
        true
    }

    /// Write-ahead: every edit reaches `write()` before anything else happens.
    fn flush_edits(&mut self) {
        let edits = self.buffer.take_pending_edits();
        if edits.is_empty() || self.swap_failed {
            return;
        }
        if let Some(w) = self.swap.as_mut() {
            if let Err(e) = w.append(&edits) {
                self.swap_failed = true;
                self.set_error(format!("Swap write failed: {e}"));
            }
        }
    }

    /// Whether a crash now could lose edits. Stays true until a save starts a
    /// working swap, so it can't scroll away like a message.
    pub fn recovery_off(&self) -> bool {
        let recovering = matches!(self.mode, Mode::Recover(_));
        !recovering && (self.swap.is_none() || self.swap_failed)
    }

    /// Whether a swap file is being written.
    pub fn swap_path(&self) -> Option<&std::path::Path> {
        self.swap.as_ref().map(|w| w.path())
    }

    /// Called once after the event loop: removes the swap unless it must be kept.
    pub fn finish(&mut self) {
        if let Some(w) = self.swap.take() {
            if self.keep_swap {
                let _ = { w }.fsync_if_pending();
            } else {
                let _ = w.remove();
            }
        }
    }

    fn key_inner(&mut self, key: Key, now: Instant) {
        self.message = None;
        self.message_is_error = false;
        let mode = self.mode.clone();
        match mode {
            Mode::Insert => self.insert_key(key, now),
            Mode::Overlay { opened_at } => self.overlay_key(key, opened_at, now),
            Mode::Palette(state) => self.palette_key(key, state),
            Mode::Prompt(Prompt::Search { query, origin }) => self.search_key(key, query, origin),
            Mode::Prompt(Prompt::GotoLine { text }) => self.goto_key(key, text),
            Mode::Recover(info) => self.recover_key(key, info),
            Mode::Help => {
                if matches!(key, Key::Esc | Key::Enter | Key::Char('q')) {
                    self.mode = Mode::Insert;
                }
            }
            Mode::Prompt(Prompt::SaveAs { text, quit_after }) => {
                self.save_as_key(key, text, quit_after)
            }
            Mode::Prompt(Prompt::ConfirmQuit) => match key {
                Key::Char('y') => {
                    self.mode = Mode::Insert;
                    self.start_save(true);
                }
                Key::Char('n') => self.discard_and_quit(),
                Key::Esc => self.mode = Mode::Insert,
                _ => {}
            },
            Mode::Prompt(Prompt::ConfirmSave {
                risk,
                path,
                quit_after,
            }) => match key {
                Key::Char('y') => {
                    self.mode = Mode::Insert;
                    self.save_after(path, Some(risk), quit_after);
                }
                Key::Char('n') | Key::Esc => {
                    self.mode = Mode::Insert;
                    self.set_message("Save cancelled");
                }
                _ => {}
            },
        }
    }

    /// Recover modal: `r` replays the swap, `d` discards it, `q`/Esc quit
    /// without touching anything. A stale swap offers only `d` and `q`.
    fn recover_key(&mut self, key: Key, info: RecoverInfo) {
        match key {
            Key::Char('r') if info.recovered.is_some() => {
                let text = info.recovered.clone().unwrap_or_default();
                let path = self.buffer.path().map(|p| p.to_path_buf());
                self.buffer = Buffer::from_str(&text, path);
                self.buffer.set_dirty();
                self.mode = Mode::Insert;
                if info.file_changed {
                    // Saving this version replaces what's on disk, so ask first.
                    self.disk = None;
                }
                match SwapWriter::adopt(&info.swap_path) {
                    Ok(w) => {
                        self.swap = Some(w);
                        self.set_message("Recovered unsaved changes; Ctrl-S saves them");
                    }
                    Err(e) => {
                        self.set_error(format!("Recovered, but the swap could not be reused: {e}"))
                    }
                }
            }
            Key::Char('d') => {
                let _ = std::fs::remove_file(&info.swap_path);
                self.mode = Mode::Insert;
                let fresh = if info.target.as_os_str().is_empty() {
                    SwapWriter::create_untitled()
                } else {
                    SwapWriter::create(&info.target, self.buffer.rope())
                };
                match fresh {
                    Ok(w) => {
                        self.swap = Some(w);
                        self.set_message("Discarded the swap file");
                    }
                    Err(e) => self.set_error(format!("Discarded the swap; new swap failed: {e}")),
                }
            }
            Key::Char('q') | Key::Esc => {
                self.keep_swap = true;
                self.quit = true;
            }
            _ => {}
        }
    }

    fn insert_key(&mut self, key: Key, now: Instant) {
        let b = &mut self.buffer;
        match key {
            Key::Esc | Key::Ctrl(' ') => self.mode = Mode::Overlay { opened_at: now },
            Key::Ctrl('p') => self.open_palette(),
            Key::Ctrl('f') => self.open_search(),
            Key::Char(c) => b.insert_str(&c.to_string(), GroupKind::Typing),
            Key::Enter => b.insert_newline(),
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
        let split = now.saturating_duration_since(opened_at) <= ESC_TAIL_WINDOW;
        match key {
            Key::Char('[') if split => self.esc_tail = Some((EscTail::Csi, now)),
            Key::Char('O') if split => self.esc_tail = Some((EscTail::Ss3, now)),
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
            Key::Esc => {
                self.mode = Mode::Insert;
                return;
            }
            Key::Enter => {
                if let Some(cmd) = state.selected_command() {
                    self.mode = Mode::Insert;
                    self.run_command(cmd);
                    return;
                }
            }
            Key::Char(c) => state.push(c),
            Key::Backspace => state.pop(),
            Key::Up => state.up(),
            Key::Down => state.down(),
            _ => {}
        }
        self.mode = Mode::Palette(state);
    }

    /// Runs a palette command (the palette is already closed).
    pub fn run_command(&mut self, cmd: Command) {
        match cmd {
            Command::Save => self.save(),
            Command::SaveAs => self.open_save_as(false),
            Command::Quit => self.request_quit(),
            Command::QuitWithoutSaving => self.discard_and_quit(),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Find => self.open_search(),
            Command::GotoLine => {
                self.mode = Mode::Prompt(Prompt::GotoLine {
                    text: String::new(),
                })
            }
            Command::ToggleLineNumbers => self.show_line_numbers = !self.show_line_numbers,
            Command::ShowKeyHelp => self.mode = Mode::Help,
        }
    }

    fn goto_key(&mut self, key: Key, mut text: String) {
        match key {
            Key::Esc => {
                self.mode = Mode::Insert;
                return;
            }
            Key::Enter => match command::parse_line_number(&text) {
                Ok(n) => {
                    let last = self.buffer.line_count();
                    let line = n.min(last) - 1;
                    let start = self.buffer.line_start(line);
                    self.buffer.move_to(start);
                    self.mode = Mode::Insert;
                    return;
                }
                Err(e) => self.set_error(e),
            },
            Key::Char(c) => text.push(c),
            Key::Backspace => {
                text.pop();
            }
            _ => {}
        }
        self.mode = Mode::Prompt(Prompt::GotoLine { text });
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
        let off = crate::buffer::graphemes_with_char_offsets(&text)
            .take(col)
            .last()
            .map_or(0, |(o, _)| o);
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
                let text = text.replace('\n', self.buffer.eol());
                self.buffer.insert_str(&text, GroupKind::Paste);
            }
            Mode::Palette(state) => {
                let q = format!("{}{}", state.query, text.replace('\n', " "));
                state.set_query(q);
            }
            Mode::Prompt(Prompt::Search { query, origin }) => {
                query.push_str(&text.replace('\n', self.buffer.eol()));
                let (query, origin) = (query.clone(), *origin);
                self.incremental(&query, origin);
            }
            Mode::Prompt(Prompt::GotoLine { text: typed })
            | Mode::Prompt(Prompt::SaveAs { text: typed, .. }) => {
                typed.push_str(text.lines().next().unwrap_or(""));
            }
            Mode::Prompt(_) | Mode::Help | Mode::Recover(_) => {}
        }
        self.flush_edits();
        Ok(())
    }

    /// Called when the poll timeout elapses with no input.
    /// fsyncs a dirty swap at most once per second.
    pub fn tick(&mut self, now: Instant) -> Result<()> {
        let due = self
            .last_fsync
            .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(1));
        if due {
            if let Some(w) = self.swap.as_mut() {
                if let Err(e) = w.fsync_if_pending() {
                    self.swap_failed = true;
                    self.set_error(format!("Swap fsync failed: {e}"));
                }
            }
            self.last_fsync = Some(now);
        }
        Ok(())
    }

    /// Ctrl-S: atomic save, asking first if it would lose something.
    pub fn save(&mut self) {
        self.start_save(false);
    }

    /// Saves to the buffer's file, or asks for a name if it has none.
    fn start_save(&mut self, quit_after: bool) {
        match self.buffer.path().map(|p| p.to_path_buf()) {
            Some(path) => self.save_after(path, None, quit_after),
            None => self.open_save_as(quit_after),
        }
    }

    /// Opens the Save As prompt, starting from the current file name.
    fn open_save_as(&mut self, quit_after: bool) {
        let text = self
            .buffer
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.mode = Mode::Prompt(Prompt::SaveAs { text, quit_after });
    }

    fn save_as_key(&mut self, key: Key, mut text: String, quit_after: bool) {
        match key {
            Key::Esc => {
                self.mode = Mode::Insert;
                self.set_message("Save cancelled");
                return;
            }
            Key::Enter => match self.submit_save_as(&text, quit_after) {
                Ok(()) => return,
                Err(e) => self.set_error(e),
            },
            Key::Char(c) => text.push(c),
            Key::Backspace => {
                text.pop();
            }
            _ => {}
        }
        self.mode = Mode::Prompt(Prompt::SaveAs { text, quit_after });
    }

    /// Checks the typed name and starts the save; `Err` keeps the prompt open.
    fn submit_save_as(&mut self, text: &str, quit_after: bool) -> Result<(), String> {
        if text.is_empty() {
            return Err("Enter a file name".to_string());
        }
        let path = fileio::absolute(&fileio::expand_tilde(text)).map_err(|e| e.to_string())?;
        let meta = std::fs::metadata(&path).ok();
        if meta.as_ref().is_some_and(|m| !m.is_file()) {
            return Err("Not a regular file".to_string());
        }
        let target = fileio::resolve_target(&path).map_err(|e| e.to_string())?;
        if Some(&target) == self.swap_target.as_ref() {
            self.mode = Mode::Insert;
            self.save_after(path, None, quit_after);
            return Ok(());
        }
        if let Ok(SwapStatus::LiveLock { pid }) = swap::inspect(&target, b"") {
            return Err(format!("Open in another kyte (PID {pid})"));
        }
        self.mode = Mode::Insert;
        if meta.is_some() {
            self.mode = Mode::Prompt(Prompt::ConfirmSave {
                risk: SaveRisk::Exists,
                path,
                quit_after,
            });
        } else {
            self.write_file(&path, quit_after);
        }
        Ok(())
    }

    /// The next thing to ask about once `passed` has been answered.
    fn next_risk(&self, path: &std::path::Path, passed: Option<SaveRisk>) -> Option<SaveRisk> {
        if passed.is_none() {
            // An unreadable file is left for the write itself to report.
            let changed = matches!(fileio::disk_state(path), Ok(Some(s)) if Some(s) != self.disk);
            if changed {
                return Some(SaveRisk::ChangedOnDisk);
            }
        }
        (passed != Some(SaveRisk::ReadOnly) && fileio::is_read_only(path))
            .then_some(SaveRisk::ReadOnly)
    }

    /// Asks the next question after `passed`, or writes the file.
    fn save_after(&mut self, path: std::path::PathBuf, passed: Option<SaveRisk>, quit_after: bool) {
        match self.next_risk(&path, passed) {
            Some(risk) => {
                self.mode = Mode::Prompt(Prompt::ConfirmSave {
                    risk,
                    path,
                    quit_after,
                })
            }
            None => self.write_file(&path, quit_after),
        }
    }

    fn write_file(&mut self, path: &std::path::Path, quit_after: bool) {
        let text = self.buffer.to_string();
        match fileio::save_atomic(path, &text) {
            Ok(()) => {
                self.disk = Some(DiskState::of_bytes(text.as_bytes()));
                self.quit = quit_after;
                if self.buffer.path() != Some(path) {
                    self.buffer.set_path(path.to_path_buf());
                    self.swap_target = fileio::resolve_target(path).ok();
                }
                self.buffer.mark_clean();
                let n = self.buffer.saved_line_count();
                self.set_message(format!("Saved {n} line{}", if n == 1 { "" } else { "s" }));
                // The saved file is the new base: replace the swap with an empty one.
                if let Some(old) = self.swap.take() {
                    let _ = old.remove();
                }
                if let Some(target) = self.swap_target.clone() {
                    match SwapWriter::create(&target, self.buffer.rope()) {
                        Ok(w) => {
                            self.swap = Some(w);
                            self.swap_failed = false;
                        }
                        Err(e) => self.set_error(format!("Saved {n} lines, but no swap: {e}")),
                    }
                }
            }
            Err(e) => self.set_error(format!(
                "Save failed: {e}. Your changes are safe; Save as in the palette writes them elsewhere"
            )),
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

    /// Quits discarding changes (ConfirmQuit `y`, "Quit without saving").
    pub fn discard_and_quit(&mut self) {
        self.quit = true;
    }

    /// Label for the status bar mode field.
    pub fn mode_label(&self) -> &'static str {
        match &self.mode {
            Mode::Insert => "INSERT",
            Mode::Overlay { .. } => "ESC",
            Mode::Palette(_) => "PALETTE",
            Mode::Prompt(Prompt::Search { .. }) => "SEARCH",
            Mode::Prompt(Prompt::GotoLine { .. }) => "GOTO",
            Mode::Prompt(Prompt::ConfirmQuit) => "QUIT?",
            Mode::Prompt(Prompt::SaveAs { .. }) => "SAVE AS",
            Mode::Prompt(Prompt::ConfirmSave { .. }) => "SAVE?",
            Mode::Help => "HELP",
            Mode::Recover(_) => "RECOVER",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(text: &str) -> App {
        App::new(Buffer::from_str(text, None), None, &Options::default())
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

    /// A swap-backed app whose swap has started failing.
    fn failing_swap_app() -> App {
        App::new(
            Buffer::from_str("", None),
            Some(SwapWriter::failing()),
            &Options::default(),
        )
    }

    #[test]
    fn swap_failure_stays_flagged_after_the_message_clears() {
        let t = Instant::now();
        let mut a = failing_swap_app();
        assert!(!a.recovery_off());
        a.handle_key(Key::Char('x'), t).unwrap();
        assert!(a.message.as_deref().unwrap().contains("Swap write failed"));
        assert!(a.recovery_off());
        a.handle_key(Key::Char('y'), t).unwrap();
        assert_eq!(a.message, None, "the error isn't repeated on every key");
        assert!(a.recovery_off(), "the flag outlives the message");
        assert_eq!(a.buffer.to_string(), "xy", "typing keeps working");
    }

    #[test]
    fn no_swap_at_all_counts_as_recovery_off() {
        assert!(app("x").recovery_off());
    }

    #[test]
    fn split_csi_sequence_types_nothing() {
        for seq in ["[A", "[3~", "[1;5C", "OA"] {
            let t = Instant::now();
            let mut a = app("abc");
            a.handle_key(Key::Esc, t).unwrap();
            for c in seq.chars() {
                a.handle_key(Key::Char(c), ms(t, 30)).unwrap();
            }
            assert_eq!(a.buffer.to_string(), "abc", "{seq}");
            assert_eq!(a.mode, Mode::Insert);
            a.handle_key(Key::Char('z'), ms(t, 40)).unwrap();
            assert_eq!(a.buffer.to_string(), "zabc", "{seq}: typing resumes");
        }
    }

    #[test]
    fn a_slow_bracket_after_esc_is_just_an_unknown_key() {
        let t = Instant::now();
        let mut a = app("abc");
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('['), ms(t, 400)).unwrap();
        a.handle_key(Key::Char('A'), ms(t, 410)).unwrap();
        assert_eq!(a.buffer.to_string(), "Aabc");
    }

    #[test]
    fn typing_soon_after_the_window_is_not_swallowed() {
        let t = Instant::now();
        let mut a = app("abc");
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char('['), ms(t, 10)).unwrap();
        a.handle_key(Key::Char('A'), ms(t, 500)).unwrap();
        assert_eq!(a.buffer.to_string(), "Aabc");
    }

    #[test]
    fn paste_reaches_the_prompts() {
        let t = Instant::now();
        let mut a = search_app();
        a.handle_key(Key::Ctrl('f'), t).unwrap();
        a.handle_paste("baz".to_string()).unwrap();
        assert_eq!(a.buffer.cursor(), 8, "pasting searches like typing");
        a.handle_key(Key::Esc, t).unwrap();
        a.run_command(Command::GotoLine);
        a.handle_paste("1\nignored".to_string()).unwrap();
        assert_eq!(
            a.mode,
            Mode::Prompt(Prompt::GotoLine {
                text: "1".to_string()
            })
        );
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
        let mut a = App::new(Buffer::from_str("", None), None, &opts);
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
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert!(!a.quit);
        a.handle_key(Key::Ctrl('q'), t).unwrap();
        for ignored in [Key::Ctrl('q'), Key::Char('z'), Key::Enter] {
            a.handle_key(ignored, t).unwrap();
            assert_eq!(a.mode, Mode::Prompt(Prompt::ConfirmQuit));
        }
        assert!(!a.quit);
        a.handle_key(Key::Char('n'), t).unwrap();
        assert!(a.quit);
    }

    #[test]
    fn y_on_an_untitled_buffer_asks_for_a_name_and_esc_keeps_editing() {
        let t = Instant::now();
        let mut a = app("");
        a.handle_key(Key::Char('x'), t).unwrap();
        a.handle_key(Key::Ctrl('q'), t).unwrap();
        a.handle_key(Key::Char('y'), t).unwrap();
        assert!(matches!(
            a.mode,
            Mode::Prompt(Prompt::SaveAs {
                quit_after: true,
                ..
            })
        ));
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert!(!a.quit);
        assert_eq!(a.buffer.to_string(), "x");
    }

    #[test]
    fn save_as_needs_a_name() {
        let t = Instant::now();
        let mut a = app("x");
        a.handle_key(Key::Ctrl('s'), t).unwrap();
        a.handle_key(Key::Enter, t).unwrap();
        assert!(matches!(a.mode, Mode::Prompt(Prompt::SaveAs { .. })));
        assert_eq!(a.message.as_deref(), Some("Enter a file name"));
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
    fn palette_filter_and_run_save_untitled() {
        let t = Instant::now();
        let mut a = app("x");
        a.handle_key(Key::Ctrl('p'), t).unwrap();
        typ(&mut a, "sav");
        match &a.mode {
            Mode::Palette(s) => assert_eq!(s.selected_command(), Some(Command::Save)),
            m => panic!("{m:?}"),
        }
        a.handle_key(Key::Enter, t).unwrap();
        assert!(matches!(a.mode, Mode::Prompt(Prompt::SaveAs { .. })));
    }

    #[test]
    fn palette_goto_line_prompt() {
        let t = Instant::now();
        let mut a = app("1\n2\n3\n4\n5");
        a.handle_key(Key::Ctrl('p'), t).unwrap();
        typ(&mut a, "goto");
        a.handle_key(Key::Enter, t).unwrap();
        assert!(matches!(a.mode, Mode::Prompt(Prompt::GotoLine { .. })));
        typ(&mut a, "3");
        a.handle_key(Key::Enter, t).unwrap();
        assert_eq!(a.buffer.line_col(), (3, 1));
        assert_eq!(a.mode, Mode::Insert);
        a.run_command(Command::GotoLine);
        typ(&mut a, "999");
        a.handle_key(Key::Enter, t).unwrap();
        assert_eq!(a.buffer.line_col(), (5, 1));
        a.run_command(Command::GotoLine);
        typ(&mut a, "abc");
        a.handle_key(Key::Enter, t).unwrap();
        assert!(matches!(a.mode, Mode::Prompt(Prompt::GotoLine { .. })));
        assert!(a.message_is_error);
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert_eq!(a.buffer.line_col(), (5, 1));
    }

    #[test]
    fn palette_toggle_numbers_help_and_esc() {
        let t = Instant::now();
        let mut a = app("x");
        a.run_command(Command::ToggleLineNumbers);
        assert!(!a.show_line_numbers);
        a.run_command(Command::ToggleLineNumbers);
        assert!(a.show_line_numbers);
        a.run_command(Command::ShowKeyHelp);
        assert_eq!(a.mode, Mode::Help);
        a.handle_key(Key::Char('z'), t).unwrap();
        assert_eq!(a.mode, Mode::Help);
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        a.handle_key(Key::Ctrl('p'), t).unwrap();
        typ(&mut a, "und");
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert_eq!(a.buffer.to_string(), "x");
    }

    #[test]
    fn clean_quit_is_immediate() {
        let mut a = app("");
        a.handle_key(Key::Ctrl('x'), Instant::now()).unwrap();
        assert!(a.quit);
    }
}
