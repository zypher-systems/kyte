//! Application state and the mode state machine (sections 5 and 8).
//! `handle_key` takes `now` so tests control time.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::buffer::{Buffer, Edit, GroupKind, Indent};
use crate::cli::Options;
use crate::command::{self, Command};
use crate::fileio::{self, DiskState};
use crate::history::{self, Change, FileHistory, SharedTimeline, SnapshotKind, Timeline};
use crate::input::{normalize, Input, Key, Motion};
use crate::ops;
use crate::palette::{Entry, PaletteState};
use crate::reader::{self, Reader};
use crate::review::{self, Change as ReviewChange};
use crate::search;
use crate::session::{self, Opened};
use crate::swap::{self, RecoverInfo, SwapStatus, SwapWriter};
use crate::syntax;

/// How soon after Esc a `[` or `O` counts as the rest of a split key sequence.
/// A person can't reach those keys this fast, and they do nothing in the layer.
const ESC_TAIL_WINDOW: Duration = Duration::from_millis(150);

/// Lines kept above a section when reader mode brings it into view.
const READER_LINES_ABOVE: usize = 2;

/// How many files the universal palette lists, and how long it may look.
const PALETTE_MAX_FILES: usize = 20_000;
const PALETTE_SCAN_BUDGET: Duration = Duration::from_millis(100);

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
    /// Read-only, with everything outside the current section dimmed.
    Reader,
    /// Stepping through changes another program made to the file.
    Review(ReviewRun),
    /// Browsing the file's past, at version `at`.
    History {
        timeline: SharedTimeline,
        at: usize,
    },
    /// The "Show key help" modal, scrolled to row `top`; Esc closes it.
    Help {
        top: usize,
    },
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
    /// Unsaved changes stand in the way of `then`.
    ConfirmLeave {
        then: After,
    },
    /// Asks for a file to open.
    Open {
        text: String,
    },
    /// Asks before deleting the file's history.
    ConfirmDeleteHistory,
    /// Asks what to replace.
    ReplaceFind {
        query: String,
    },
    /// Asks what to replace it with.
    ReplaceWith {
        query: String,
        with: String,
    },
    /// Stops at each match and asks.
    ReplaceConfirm(ReplaceRun),
    /// Asks for a file name. `then` happens once the save succeeds.
    SaveAs {
        text: String,
        then: After,
    },
    /// A save that needs a yes first. `then` happens once it succeeds.
    ConfirmSave {
        risk: SaveRisk,
        path: std::path::PathBuf,
        then: After,
    },
}

/// What a save was for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum After {
    Stay,
    Quit,
    Open(std::path::PathBuf),
}

/// A review of outside changes in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRun {
    changes: std::sync::Arc<Vec<ReviewChange>>,
    /// The file as it is now.
    theirs: std::sync::Arc<String>,
    /// The change on screen.
    at: usize,
    /// How far earlier accepted changes moved the buffer's later lines.
    shift: isize,
    accepted: usize,
}

impl ReviewRun {
    /// (1-based number of the change on screen, how many there are).
    pub fn position(&self) -> (usize, usize) {
        (self.at + 1, self.changes.len())
    }

    /// Whether the change on screen overlaps the user's own edits.
    pub fn is_conflict(&self) -> bool {
        self.changes[self.at].conflict
    }
}

/// A replace in progress. It runs from where it started to the end of the
/// file, then from the top back to where it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceRun {
    query: String,
    with: String,
    /// Where it started, moved along by replacements made before that point.
    origin: usize,
    wrapped: bool,
    count: usize,
}

/// Why a save stopped to ask, in the order the questions are asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveRisk {
    ChangedOnDisk,
    Exists,
    ReadOnly,
}

impl SaveRisk {
    /// The question's title and what `y` does.
    pub fn question(self) -> (&'static str, &'static str) {
        match self {
            SaveRisk::ChangedOnDisk => ("File changed on disk!", "Overwrite those changes"),
            SaveRisk::Exists => ("File exists!", "Overwrite it"),
            SaveRisk::ReadOnly => ("File is read-only!", "Replace it anyway"),
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
    clipboard: String,
    /// The clipboard holds a whole line, so it pastes above the current one.
    clipboard_whole_line: bool,
    /// Bytes for the terminal itself, written after the next key.
    terminal_output: Option<String>,
    /// Names Tab found in a file prompt, shown until the next key.
    pub completions: Vec<String>,
    /// File history is wanted this session.
    history_on: bool,
    history: Option<FileHistory>,
    /// A history write failed, so no more are being kept.
    history_failed: bool,
    /// The version being browsed, while `Mode::History` is open.
    version_view: Option<Buffer>,
    /// The file's text as last read or written, what a review compares with.
    disk_text: Option<ropey::Rope>,
    /// The file's size and time when last checked, to spot a change cheaply.
    disk_stamp: Option<(u64, std::time::SystemTime)>,
    /// The file on disk has moved on and the change hasn't been reviewed.
    outside_change: bool,
    /// Something changed on screen with no key pressed.
    needs_redraw: bool,
    /// The view lines of the change on screen: (yours, theirs).
    pub review_marks: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    /// Reader mode's state, while it's open.
    pub reader: Option<Reader>,
    last_fsync: Option<Instant>,
    scroll_top: usize,
    scroll_left: usize,
    view: (u16, u16),
}

/// Shown on the message line only while the overlay is open (section 5.4).
/// The Esc layer's core keys, shown on top of whatever the context adds.
pub const LEGEND: &[(&str, &str)] = &[
    ("w", "del word"),
    ("d", "del line"),
    ("k", "del to end"),
    ("%", "bracket"),
    ("/", "search"),
    ("u", "undo"),
    ("r", "redo"),
    ("s", "save"),
    ("h", "history"),
    ("v", "reader mode"),
];

/// The double-tap key, listed last.
const PALETTE_KEY: (&str, &str) = ("Esc Esc", "palette");

impl App {
    pub fn new(buffer: Buffer, swap: Option<SwapWriter>, opts: &Options) -> App {
        let swap_target = buffer.path().and_then(|p| fileio::resolve_target(p).ok());
        let disk = Some(DiskState::of_rope(buffer.rope()));
        let disk_text = Some(buffer.rope().clone());
        App {
            keep_swap: false,
            swap,
            swap_target,
            disk,
            swap_failed: false,
            esc_tail: None,
            clipboard: String::new(),
            clipboard_whole_line: false,
            terminal_output: None,
            completions: Vec::new(),
            history_on: false,
            history: None,
            history_failed: false,
            version_view: None,
            disk_text,
            disk_stamp: None,
            outside_change: false,
            needs_redraw: false,
            review_marks: None,
            reader: None,
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

    /// The buffer on screen: the version being browsed, or the file.
    pub fn view_buffer(&self) -> &Buffer {
        self.version_view.as_ref().unwrap_or(&self.buffer)
    }

    /// Starts keeping history for this session.
    pub fn enable_history(&mut self) {
        self.history_on = true;
        self.start_history();
    }

    /// Points the history at the current file and text.
    fn start_history(&mut self) {
        self.history = None;
        self.history_failed = false;
        if self.history_on {
            self.history = self
                .swap_target
                .as_ref()
                .map(|t| FileHistory::new(t, self.buffer.rope()));
        }
    }

    /// Runs one history write. A failure stops history but never the editing.
    fn with_history(&mut self, f: impl FnOnce(&mut FileHistory) -> std::io::Result<()>) {
        let Some(h) = self.history.as_mut() else {
            return;
        };
        if let Err(e) = f(h) {
            self.history = None;
            self.history_failed = true;
            self.set_error(format!("History write failed: {e}"));
        }
    }

    /// Whether edits have stopped reaching the history.
    pub fn history_lost(&self) -> bool {
        self.history_failed
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
        let buf = self.view_buffer();
        let gw = crate::ui::text_view::gutter_width(buf.line_count(), self.show_line_numbers);
        let w = (width as usize).saturating_sub(gw).max(1);
        let line = buf.cursor_line();
        let lines = buf.line_count();
        let col = buf.cursor_display_col();
        if line < self.scroll_top {
            self.scroll_top = line;
        } else if line >= self.scroll_top + h {
            self.scroll_top = line + 1 - h;
        }
        let max_top = lines.saturating_sub(1);
        self.scroll_top = self.scroll_top.min(max_top);
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
        if edits.is_empty() {
            return;
        }
        self.with_history(|h| h.append(&edits));
        if self.swap_failed {
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

    /// Bytes waiting to go to the terminal, such as a clipboard copy.
    pub fn take_terminal_output(&mut self) -> Option<String> {
        self.terminal_output.take()
    }

    /// Whether a swap file is being written.
    pub fn swap_path(&self) -> Option<&std::path::Path> {
        self.swap.as_ref().map(|w| w.path())
    }

    /// Called once after the event loop: removes the swap unless it must be kept.
    pub fn finish(&mut self) {
        let text = self.buffer.rope().clone();
        self.with_history(|h| h.close(&text));
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
        self.completions.clear();
        let mode = self.mode.clone();
        match mode {
            Mode::Insert => self.insert_key(key, now),
            Mode::Overlay { opened_at } => self.overlay_key(key, opened_at, now),
            Mode::Palette(state) => self.palette_key(key, state),
            Mode::Prompt(Prompt::Search { query, origin }) => self.search_key(key, query, origin),
            Mode::Prompt(Prompt::GotoLine { text }) => self.goto_key(key, text),
            Mode::Prompt(Prompt::ReplaceFind { query }) => self.replace_find_key(key, query),
            Mode::Prompt(Prompt::ReplaceWith { query, with }) => {
                self.replace_with_key(key, query, with)
            }
            Mode::Prompt(Prompt::ReplaceConfirm(run)) => self.replace_confirm_key(key, run),
            Mode::Recover(info) => self.recover_key(key, info),
            Mode::History { timeline, at } => self.history_key(key, timeline, at),
            Mode::Reader => self.reader_key(key),
            Mode::Review(run) => self.review_key(key, run),
            Mode::Prompt(Prompt::ConfirmDeleteHistory) => match key {
                Key::Char('y') => {
                    self.mode = Mode::Insert;
                    self.delete_history();
                }
                Key::Char('n') | Key::Esc => self.mode = Mode::Insert,
                _ => {}
            },
            Mode::Help { top } => {
                let last = command::help_max_top(self.text_height());
                match key {
                    Key::Esc | Key::Enter | Key::Char('q') => self.mode = Mode::Insert,
                    Key::Up => {
                        self.mode = Mode::Help {
                            top: top.saturating_sub(1),
                        }
                    }
                    Key::Down => {
                        self.mode = Mode::Help {
                            top: (top + 1).min(last),
                        }
                    }
                    _ => {}
                }
            }
            Mode::Prompt(Prompt::SaveAs { text, then }) => self.save_as_key(key, text, then),
            Mode::Prompt(Prompt::Open { text }) => self.open_key(key, text),
            Mode::Prompt(Prompt::ConfirmLeave { then }) => match key {
                Key::Char('y') => {
                    self.mode = Mode::Insert;
                    self.start_save(then);
                }
                Key::Char('n') => {
                    self.mode = Mode::Insert;
                    self.after_save(then);
                }
                Key::Esc => self.mode = Mode::Insert,
                _ => {}
            },
            Mode::Prompt(Prompt::ConfirmSave { risk, path, then }) => match key {
                Key::Char('r') if risk == SaveRisk::ChangedOnDisk => {
                    self.mode = Mode::Insert;
                    self.open_review();
                }
                Key::Char('y') => {
                    self.mode = Mode::Insert;
                    self.save_after(path, Some(risk), then);
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
                    self.disk_text = None;
                }
                self.start_history();
                let recovered = self.buffer.rope().clone();
                self.with_history(|h| h.snapshot(&recovered, SnapshotKind::Recovered));
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
            Key::Ctrl('k') => self.open_universal(),
            Key::Ctrl('f') => self.open_search(),
            Key::Ctrl('r') => self.open_replace(),
            Key::Char(c) => b.insert_str(&c.to_string(), GroupKind::Typing),
            Key::Enter => b.insert_newline(),
            Key::Tab => b.insert_tab(),
            Key::BackTab => b.shift_lines(true),
            Key::Backspace => b.backspace(),
            Key::Delete => b.delete_forward(),
            Key::Left => self.motion(Motion::Left),
            Key::Right => self.motion(Motion::Right),
            Key::Up => self.motion(Motion::Up),
            Key::Down => self.motion(Motion::Down),
            Key::Home => self.motion(Motion::Home),
            Key::End => self.motion(Motion::End),
            Key::PageUp => self.motion(Motion::PageUp),
            Key::PageDown => self.motion(Motion::PageDown),
            Key::CtrlHome => self.motion(Motion::FileStart),
            Key::CtrlEnd => self.motion(Motion::FileEnd),
            Key::WordLeft => self.motion(Motion::WordLeft),
            Key::WordRight => self.motion(Motion::WordRight),
            Key::Select(m) => {
                let anchor = self.buffer.selection_anchor();
                self.motion(m);
                self.buffer.set_selection_anchor(anchor);
            }
            Key::WordBackspace => {
                if !b.delete_selection() {
                    let at = b.cursor();
                    let start = ops::word_start(b.rope(), at);
                    self.delete_range(start..at, start);
                }
            }
            Key::WordDelete => {
                if !b.delete_selection() {
                    self.op_word();
                }
            }
            Key::Ctrl('a') => b.select_all(),
            Key::Ctrl('x') => self.cut(),
            Key::Ctrl('c') => self.copy(),
            Key::Ctrl('v') => self.paste(),
            Key::Ctrl('s') => self.save(),
            Key::Ctrl('o') => self.open_prompt(),
            Key::Ctrl('q') => self.request_quit(),
            Key::Ctrl('z') => self.undo(),
            Key::Ctrl('y') => self.redo(),
            _ => {}
        }
    }

    /// Moves the cursor. A plain move also ends any selection.
    fn motion(&mut self, m: Motion) {
        let page = self.text_height() as isize;
        let b = &mut self.buffer;
        match m {
            Motion::Left => b.move_left(),
            Motion::Right => b.move_right(),
            Motion::Up => b.move_up(),
            Motion::Down => b.move_down(),
            Motion::Home => b.move_home(),
            Motion::End => b.move_end(),
            Motion::PageUp => b.move_lines(-page),
            Motion::PageDown => b.move_lines(page),
            Motion::FileStart => b.move_to(0),
            Motion::FileEnd => b.move_to(b.len_chars()),
            Motion::WordLeft => b.move_to(ops::word_start(b.rope(), b.cursor())),
            Motion::WordRight => b.move_to(ops::word_end(b.rope(), b.cursor())),
        }
    }

    /// The text Ctrl-C or Ctrl-X takes: the selection, or the whole line with
    /// its line ending. `None` when there is nothing to take.
    fn clip_text(&self) -> Option<(String, bool)> {
        let b = &self.buffer;
        if let Some(r) = b.selection() {
            return Some((b.slice_string(r), false));
        }
        if b.len_chars() == 0 {
            return None;
        }
        let mut line = b.rope().line(b.cursor_line()).to_string();
        if !line.ends_with('\n') {
            line.push_str(b.eol());
        }
        Some((line, true))
    }

    /// Puts `text` in Kyte's clipboard and queues it for the system clipboard.
    fn set_clipboard(&mut self, text: String, whole_line: bool) {
        self.terminal_output = crate::clipboard::osc52(&text);
        let lines = text.matches('\n').count();
        let what = if lines > 0 {
            format!("{lines} line{}", if lines == 1 { "" } else { "s" })
        } else {
            let n = text.chars().count();
            format!("{n} character{}", if n == 1 { "" } else { "s" })
        };
        if self.terminal_output.is_some() {
            self.set_message(format!("Copied {what}"));
        } else {
            self.set_message(format!(
                "Copied {what} inside Kyte; too large for the system clipboard"
            ));
        }
        self.clipboard = text;
        self.clipboard_whole_line = whole_line;
    }

    fn copy(&mut self) {
        match self.clip_text() {
            Some((text, whole_line)) => self.set_clipboard(text, whole_line),
            None => self.set_message("Nothing to copy"),
        }
    }

    fn cut(&mut self) {
        let Some((text, whole_line)) = self.clip_text() else {
            self.set_message("Nothing to cut");
            return;
        };
        if whole_line {
            self.op_line();
        } else {
            self.buffer.delete_selection();
        }
        self.set_clipboard(text, whole_line);
    }

    /// Pastes Kyte's clipboard. A whole line goes above the current line.
    fn paste(&mut self) {
        if self.clipboard.is_empty() {
            self.set_message("Clipboard is empty; your terminal's paste key reads the system one");
            return;
        }
        let text = self
            .clipboard
            .replace("\r\n", "\n")
            .replace('\n', self.buffer.eol());
        if self.clipboard_whole_line && self.buffer.selection().is_none() {
            let start = self.buffer.line_start(self.buffer.cursor_line());
            self.buffer.move_to(start);
        }
        self.buffer.insert_str(&text, GroupKind::Paste);
    }

    fn overlay_key(&mut self, key: Key, opened_at: Instant, now: Instant) {
        self.mode = Mode::Insert;
        let split = now.saturating_duration_since(opened_at) <= ESC_TAIL_WINDOW;
        let selecting = self.buffer.selection().is_some();
        match key {
            Key::Char('[') if split => self.esc_tail = Some((EscTail::Csi, now)),
            Key::Char('O') if split => self.esc_tail = Some((EscTail::Ss3, now)),
            Key::Esc if selecting => self.buffer.clear_selection(),
            Key::Esc => {
                if now.saturating_duration_since(opened_at) <= self.esc_window {
                    self.open_palette();
                }
            }
            // Any key that isn't an action closes the layer and is discarded.
            Key::Char(c) if selecting => self.selection_action(c),
            Key::Char(c) => self.layer_action(c),
            _ => {}
        }
    }

    /// The file's path, when its language has code actions.
    fn code_path(&self) -> Option<&std::path::Path> {
        self.buffer.path().filter(|p| syntax::supported(p))
    }

    fn comment_prefix(&self) -> Option<&'static str> {
        self.buffer.path().and_then(syntax::comment_prefix)
    }

    /// Whether the cursor is on a bracket that has a partner.
    fn on_bracket(&self) -> bool {
        let at = self.buffer.cursor();
        self.buffer
            .rope()
            .get_char(at)
            .is_some_and(|c| "()[]{}".contains(c))
            && ops::match_bracket(self.buffer.rope(), at).is_some()
    }

    /// The Esc layer's keys, for where the cursor is now. Each one here needs
    /// a matching arm in `layer_action` or `selection_action`.
    pub fn layer_rows(&self) -> Vec<(&'static str, &'static str)> {
        let code = self.code_path().is_some();
        let comment = self.comment_prefix().is_some();
        let mut rows = Vec::new();
        if self.buffer.selection().is_some() {
            rows.extend([("x", "cut"), ("c", "copy")]);
            rows.extend([("/", "find this"), ("R", "replace this")]);
            if comment {
                rows.push(("#", "comment"));
            }
            rows.extend([("U", "uppercase"), ("L", "lowercase"), ("S", "sort lines")]);
            if code {
                rows.push(("e", "expand selection"));
            }
            rows.push(("Esc", "clear selection"));
            return rows;
        }
        rows.extend_from_slice(LEGEND);
        if self.outside_change {
            rows.push(("c", "review outside changes"));
        }
        if self.on_bracket() {
            rows.push(("i", "select inside"));
        }
        if comment {
            rows.push(("#", "comment line"));
        }
        if code {
            rows.extend([
                ("f", "select function"),
                ("e", "select syntax"),
                ("n", "next function"),
                ("p", "prev function"),
            ]);
        }
        rows.push(PALETTE_KEY);
        rows
    }

    fn layer_action(&mut self, c: char) {
        match c {
            'w' => self.op_word(),
            'd' => self.op_line(),
            'k' => self.op_kill(),
            '%' => self.op_bracket(),
            '/' => self.open_search(),
            'u' => self.undo(),
            'r' => self.redo(),
            's' => self.save(),
            'h' => self.open_history(),
            'v' => self.open_reader(),
            'c' if self.outside_change => self.open_review(),
            'i' if self.on_bracket() => self.select_inside_brackets(),
            '#' => self.toggle_comment(),
            'f' if self.code_path().is_some() => self.select_function(),
            'e' if self.code_path().is_some() => self.expand_selection(),
            'n' if self.code_path().is_some() => self.go_to_function(true),
            'p' if self.code_path().is_some() => self.go_to_function(false),
            _ => {}
        }
    }

    fn selection_action(&mut self, c: char) {
        match c {
            'x' => self.cut(),
            'c' => self.copy(),
            '/' => self.open_search(),
            'R' => self.open_replace(),
            '#' => self.toggle_comment(),
            'U' => self.buffer.transform_selection(|s| s.to_uppercase()),
            'L' => self.buffer.transform_selection(|s| s.to_lowercase()),
            'S' => self.buffer.sort_lines(),
            'e' if self.code_path().is_some() => self.expand_selection(),
            _ => {}
        }
    }

    fn toggle_comment(&mut self) {
        if let Some(prefix) = self.comment_prefix() {
            self.buffer.toggle_comment(prefix);
        }
    }

    fn select_inside_brackets(&mut self) {
        let at = self.buffer.cursor();
        if let Some(other) = ops::match_bracket(self.buffer.rope(), at) {
            self.buffer.select_range(at.min(other) + 1..at.max(other));
        }
    }

    /// Runs `f` on the file's path, text and the cursor's byte offset.
    fn with_code<T>(
        &self,
        f: impl FnOnce(&std::path::Path, &str, usize) -> Option<T>,
    ) -> Option<T> {
        let path = self.code_path()?;
        let at = self.buffer.rope().char_to_byte(self.buffer.cursor());
        f(path, &self.buffer.to_string(), at)
    }

    /// Selects a byte range of the text.
    fn select_bytes(&mut self, r: std::ops::Range<usize>) {
        let rope = self.buffer.rope();
        let range = rope.byte_to_char(r.start)..rope.byte_to_char(r.end);
        self.buffer.select_range(range);
    }

    fn select_function(&mut self) {
        match self.with_code(syntax::function_at) {
            Some(r) => self.select_bytes(r),
            None => self.set_message("Not inside a function"),
        }
    }

    /// Selects the piece of syntax around the selection or cursor.
    fn expand_selection(&mut self) {
        let rope = self.buffer.rope();
        let chars = self
            .buffer
            .selection()
            .unwrap_or(self.buffer.cursor()..self.buffer.cursor());
        let bytes = rope.char_to_byte(chars.start)..rope.char_to_byte(chars.end);
        match self.with_code(|p, t, _| syntax::expand(p, t, bytes)) {
            Some(r) => self.select_bytes(r),
            None => self.set_message("Nothing larger to select"),
        }
    }

    fn go_to_function(&mut self, forward: bool) {
        let found = self.with_code(if forward {
            syntax::next_function
        } else {
            syntax::prev_function
        });
        match found {
            Some(b) => {
                let at = self.buffer.rope().byte_to_char(b);
                self.buffer.move_to(at);
            }
            None => self.set_message("No more functions"),
        }
    }

    /// Opens the search prompt (Ctrl-F, Esc /, or the palette's Find).
    pub fn open_search(&mut self) {
        let query = self.selected_query();
        let origin = self
            .buffer
            .selection()
            .map_or(self.buffer.cursor(), |r| r.start);
        self.buffer.move_to(origin);
        self.mode = Mode::Prompt(Prompt::Search { query, origin });
    }

    /// The selection as a starting query, if it fits on one line.
    fn selected_query(&self) -> String {
        self.buffer
            .selection()
            .map(|r| self.buffer.slice_string(r))
            .filter(|s| !s.contains('\n'))
            .unwrap_or_default()
    }

    /// The text to highlight, while a prompt is searching for something.
    pub fn search_query(&self) -> Option<&str> {
        match &self.mode {
            Mode::Prompt(Prompt::Search { query, .. })
            | Mode::Prompt(Prompt::ReplaceFind { query })
            | Mode::Prompt(Prompt::ReplaceWith { query, .. }) => Some(query),
            Mode::Prompt(Prompt::ReplaceConfirm(run)) => Some(&run.query),
            _ => None,
        }
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
            Key::Enter | Key::Down | Key::Ctrl('f') => {
                if !query.is_empty() {
                    let at = self.buffer.cursor();
                    match search::find_next(self.buffer.rope(), &query, at) {
                        Some(i) => self.buffer.move_to(i),
                        None => self.set_error("No match"),
                    }
                }
            }
            Key::Up => {
                if !query.is_empty() {
                    let at = self.buffer.cursor();
                    match search::find_prev(self.buffer.rope(), &query, at) {
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

    /// Opens the replace prompt, starting from the selection if there is one.
    fn open_replace(&mut self) {
        let query = self.selected_query();
        if let Some(r) = self.buffer.selection() {
            self.buffer.move_to(r.start);
        }
        self.mode = Mode::Prompt(Prompt::ReplaceFind { query });
    }

    fn replace_find_key(&mut self, key: Key, mut query: String) {
        match key {
            Key::Esc => {
                self.mode = Mode::Insert;
                return;
            }
            Key::Enter if !query.is_empty() => {
                self.mode = Mode::Prompt(Prompt::ReplaceWith {
                    query,
                    with: String::new(),
                });
                return;
            }
            Key::Char(c) => query.push(c),
            Key::Backspace => {
                query.pop();
            }
            _ => {}
        }
        self.mode = Mode::Prompt(Prompt::ReplaceFind { query });
    }

    fn replace_with_key(&mut self, key: Key, query: String, mut with: String) {
        match key {
            Key::Esc => {
                self.mode = Mode::Insert;
                return;
            }
            Key::Enter => {
                let from = self.buffer.cursor();
                let run = ReplaceRun {
                    query,
                    with,
                    origin: from,
                    wrapped: false,
                    count: 0,
                };
                self.replace_advance(run, from);
                return;
            }
            Key::Char(c) => with.push(c),
            Key::Backspace => {
                with.pop();
            }
            _ => {}
        }
        self.mode = Mode::Prompt(Prompt::ReplaceWith { query, with });
    }

    /// Stops at the next match from `from`, or finishes.
    fn replace_advance(&mut self, mut run: ReplaceRun, from: usize) {
        let rope = self.buffer.rope();
        let mut hit = search::find_forward(rope, &run.query, from);
        if hit.is_none() && !run.wrapped {
            run.wrapped = true;
            hit = search::find_forward(rope, &run.query, 0);
        }
        match hit.filter(|i| !run.wrapped || *i < run.origin) {
            Some(i) => {
                self.buffer.move_to(i);
                self.mode = Mode::Prompt(Prompt::ReplaceConfirm(run));
            }
            None => self.finish_replace(run.count),
        }
    }

    fn finish_replace(&mut self, count: usize) {
        self.mode = Mode::Insert;
        match count {
            0 => self.set_message("No matches replaced"),
            1 => self.set_message("Replaced 1 match"),
            n => self.set_message(format!("Replaced {n} matches")),
        }
    }

    fn replace_confirm_key(&mut self, key: Key, mut run: ReplaceRun) {
        let at = self.buffer.cursor();
        match key {
            Key::Char('y') => {
                let edit = self.replacement_at(at, &run);
                let inserted = edit.inserted.chars().count();
                let removed = edit.removed.chars().count();
                self.buffer
                    .apply_with_cursor(edit, GroupKind::Operator, at + inserted);
                run.count += 1;
                if run.wrapped {
                    run.origin = run.origin + inserted - removed;
                }
                self.replace_advance(run, at + inserted);
            }
            Key::Char('n') => self.replace_advance(run, at + 1),
            Key::Char('a') => {
                let (origin, wrapped) = (run.origin, run.wrapped);
                let hits = search::find_all(self.buffer.rope(), &run.query);
                let mut edits: Vec<Edit> = hits
                    .into_iter()
                    .filter(|i| {
                        if wrapped {
                            (at..origin).contains(i)
                        } else {
                            *i >= at || *i < origin
                        }
                    })
                    .map(|i| self.replacement_at(i, &run))
                    .collect();
                // Last first, so earlier positions stay right.
                edits.reverse();
                let count = run.count + edits.len();
                self.buffer.apply_group(edits, at);
                self.finish_replace(count);
            }
            Key::Char('q') | Key::Esc => self.finish_replace(run.count),
            _ => self.mode = Mode::Prompt(Prompt::ReplaceConfirm(run)),
        }
    }

    fn replacement_at(&self, at: usize, run: &ReplaceRun) -> Edit {
        let n = run.query.chars().count();
        Edit {
            at,
            removed: self.buffer.slice_string(at..at + n),
            inserted: run.with.clone(),
        }
    }

    fn open_palette(&mut self) {
        self.mode = Mode::Palette(PaletteState::default());
    }

    /// Ctrl-K: commands, files under the working directory, `:line` and `/text`.
    fn open_universal(&mut self) {
        let files = std::env::current_dir()
            .map(|d| fileio::list_files(&d, PALETTE_MAX_FILES, PALETTE_SCAN_BUDGET))
            .unwrap_or_default();
        self.mode = Mode::Palette(PaletteState::universal(files));
    }

    /// Runs the palette's selected row (the palette is already closed).
    fn run_entry(&mut self, entry: Entry) {
        match entry {
            Entry::Command(c) => self.run_command(c),
            Entry::File(path) => {
                if let Err(e) = self.submit_open(&path) {
                    self.set_error(e);
                }
            }
            Entry::Line(n) => self.go_to_line(n),
            Entry::Find(query) => {
                self.buffer.clear_selection();
                let origin = self.buffer.cursor();
                self.incremental(&query, origin);
                self.mode = Mode::Prompt(Prompt::Search { query, origin });
            }
        }
    }

    fn go_to_line(&mut self, n: usize) {
        let last = self.buffer.line_count();
        let start = self.buffer.line_start(n.min(last) - 1);
        self.buffer.move_to(start);
    }

    fn palette_key(&mut self, key: Key, mut state: PaletteState) {
        match key {
            Key::Esc => {
                self.mode = Mode::Insert;
                return;
            }
            Key::Enter => {
                if let Some(entry) = state.selected_entry().cloned() {
                    self.mode = Mode::Insert;
                    self.run_entry(entry);
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
            Command::SaveAs => self.open_save_as(After::Stay),
            Command::Open => self.open_prompt(),
            Command::Quit => self.request_quit(),
            Command::QuitWithoutSaving => self.discard_and_quit(),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Cut => self.cut(),
            Command::Copy => self.copy(),
            Command::Paste => self.paste(),
            Command::SelectAll => self.buffer.select_all(),
            Command::Find => self.open_search(),
            Command::Replace => self.open_replace(),
            Command::GotoLine => {
                self.mode = Mode::Prompt(Prompt::GotoLine {
                    text: String::new(),
                })
            }
            Command::ToggleLineNumbers => self.show_line_numbers = !self.show_line_numbers,
            Command::ToggleIndent => self.toggle_indent(),
            Command::History => self.open_history(),
            Command::Reader => self.open_reader(),
            Command::DeleteHistory => {
                self.mode = Mode::Prompt(Prompt::ConfirmDeleteHistory);
            }
            Command::ShowKeyHelp => self.mode = Mode::Help { top: 0 },
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
                    self.go_to_line(n);
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
            Edit {
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
            | Mode::Prompt(Prompt::Open { text: typed })
            | Mode::Prompt(Prompt::ReplaceFind { query: typed })
            | Mode::Prompt(Prompt::ReplaceWith { with: typed, .. })
            | Mode::Prompt(Prompt::SaveAs { text: typed, .. }) => {
                typed.push_str(text.lines().next().unwrap_or(""));
            }
            Mode::Prompt(_)
            | Mode::Help { .. }
            | Mode::Recover(_)
            | Mode::History { .. }
            | Mode::Review(_)
            | Mode::Reader => {}
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
            self.with_history(|h| h.fsync_if_pending());
            self.check_disk();
            self.last_fsync = Some(now);
        }
        Ok(())
    }

    /// Ctrl-S: atomic save, asking first if it would lose something.
    pub fn save(&mut self) {
        self.start_save(After::Stay);
    }

    /// Saves to the buffer's file, or asks for a name if it has none.
    fn start_save(&mut self, then: After) {
        match self.buffer.path().map(|p| p.to_path_buf()) {
            Some(path) => self.save_after(path, None, then),
            None => self.open_save_as(then),
        }
    }

    /// Opens the Save As prompt, starting from the current file name.
    fn open_save_as(&mut self, then: After) {
        let text = self
            .buffer
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.mode = Mode::Prompt(Prompt::SaveAs { text, then });
    }

    fn save_as_key(&mut self, key: Key, mut text: String, then: After) {
        match key {
            Key::Esc => {
                self.mode = Mode::Insert;
                self.set_message("Save cancelled");
                return;
            }
            Key::Enter => match self.submit_save_as(&text, then.clone()) {
                Ok(()) => return,
                Err(e) => self.set_error(e),
            },
            Key::Tab => self.complete_into(&mut text),
            Key::Char(c) => text.push(c),
            Key::Backspace => {
                text.pop();
            }
            _ => {}
        }
        self.mode = Mode::Prompt(Prompt::SaveAs { text, then });
    }

    /// Tab in a file prompt: completes the path as far as it can.
    fn complete_into(&mut self, text: &mut String) {
        match fileio::complete_path(text) {
            Some(c) => {
                *text = c.text;
                if c.choices.len() > 1 {
                    self.completions = c.choices;
                }
            }
            None => self.set_error("No match"),
        }
    }

    /// Opens the file prompt, starting in the current file's directory.
    fn open_prompt(&mut self) {
        let text = self
            .buffer
            .path()
            .and_then(|p| p.parent())
            .map(|d| format!("{}/", d.display()))
            .unwrap_or_default();
        self.mode = Mode::Prompt(Prompt::Open { text });
    }

    fn open_key(&mut self, key: Key, mut text: String) {
        match key {
            Key::Esc => {
                self.mode = Mode::Insert;
                return;
            }
            Key::Enter => match self.submit_open(&text) {
                Ok(()) => return,
                Err(e) => self.set_error(e),
            },
            Key::Tab => self.complete_into(&mut text),
            Key::Char(c) => text.push(c),
            Key::Backspace => {
                text.pop();
            }
            _ => {}
        }
        self.mode = Mode::Prompt(Prompt::Open { text });
    }

    /// Checks the typed file and opens it; `Err` keeps the prompt open.
    fn submit_open(&mut self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Err("Enter a file name".to_string());
        }
        let path = fileio::absolute(&fileio::expand_tilde(text)).map_err(|e| e.to_string())?;
        let target = fileio::resolve_target(&path).map_err(|e| e.to_string())?;
        if Some(&target) != self.swap_target.as_ref() {
            session::check(&path)?;
        }
        self.mode = Mode::Insert;
        if self.buffer.is_dirty() {
            self.mode = Mode::Prompt(Prompt::ConfirmLeave {
                then: After::Open(path),
            });
        } else {
            self.switch_to(&path);
        }
        Ok(())
    }

    /// Leaves the current buffer for `path`. Anything unsaved in it is gone.
    fn switch_to(&mut self, path: &std::path::Path) {
        let same =
            fileio::resolve_target(path).ok() == self.swap_target && self.swap_target.is_some();
        if same {
            // Its swap holds what was just discarded.
            if let Some(old) = self.swap.take() {
                let _ = old.remove();
            }
        }
        let opened = match session::open(path, false) {
            Ok(o) => o,
            Err(e) => {
                self.set_error(format!("Can't open {}: {e}", path.display()));
                return;
            }
        };
        let (buffer, swap, recover, warning) = match opened {
            Opened::Ready {
                buffer,
                swap,
                warning,
            } => (buffer, swap, None, warning),
            Opened::Recover { buffer, info } => (buffer, None, Some(info), None),
            Opened::Locked { pid } => {
                self.set_error(format!("Open in another kyte (PID {pid})"));
                return;
            }
        };
        if let Some(old) = self.swap.take() {
            let _ = old.remove();
        }
        self.swap_target = buffer.path().and_then(|p| fileio::resolve_target(p).ok());
        self.disk = Some(DiskState::of_rope(buffer.rope()));
        self.disk_text = Some(buffer.rope().clone());
        self.disk_stamp = None;
        self.outside_change = false;
        let closing = self.buffer.rope().clone();
        self.with_history(|h| h.close(&closing));
        self.buffer = buffer;
        self.swap = swap;
        self.swap_failed = false;
        self.start_history();
        self.scroll_top = 0;
        self.scroll_left = 0;
        self.mode = recover.map_or(Mode::Insert, Mode::Recover);
        if let Some(w) = warning {
            self.set_error(w);
        } else if self.mode == Mode::Insert {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            self.set_message(format!("Opened {name}"));
        }
    }

    fn open_reader(&mut self) {
        self.buffer.clear_selection();
        let (focus, crumbs) = reader::section_at(&self.buffer);
        self.bring_into_view(&focus);
        self.reader = Some(Reader {
            focus,
            crumbs,
            show_keys: true,
        });
        self.mode = Mode::Reader;
    }

    /// Scrolls to show `lines`, unless they're already on screen. Moving the
    /// page while someone reads it loses their place.
    fn bring_into_view(&mut self, lines: &std::ops::Range<usize>) {
        let bottom = self.scroll_top + self.text_height();
        if lines.start < self.scroll_top || lines.end > bottom {
            self.scroll_top = lines.start.saturating_sub(READER_LINES_ABOVE);
        }
    }

    fn reader_key(&mut self, key: Key) {
        let mut show_keys = false;
        match key {
            Key::Esc | Key::Char('q') => {
                self.reader = None;
                self.mode = Mode::Insert;
                return;
            }
            Key::Char('?') => {
                show_keys = !self.reader.as_ref().is_some_and(|r| r.show_keys);
            }
            Key::Up => self.motion(Motion::Up),
            Key::Down => self.motion(Motion::Down),
            Key::PageUp => self.motion(Motion::PageUp),
            Key::PageDown => self.motion(Motion::PageDown),
            Key::Char('n') | Key::Char('p') => {
                match reader::step(&self.buffer, key == Key::Char('n')) {
                    Some(at) => self.buffer.move_to(at),
                    None => self.set_message("No more sections"),
                }
            }
            _ => {}
        }
        let (focus, crumbs) = reader::section_at(&self.buffer);
        if self.reader.as_ref().is_none_or(|r| r.focus != focus) {
            self.bring_into_view(&focus);
        }
        self.reader = Some(Reader {
            focus,
            crumbs,
            show_keys,
        });
    }

    /// Whether the file on disk has changes that haven't been reviewed.
    pub fn has_outside_change(&self) -> bool {
        self.outside_change
    }

    /// Whether something changed with no key pressed.
    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.needs_redraw)
    }

    /// Looks for outside changes. The size and time are checked first, so an
    /// unchanged file costs one stat.
    fn check_disk(&mut self) {
        if matches!(self.mode, Mode::Recover(_) | Mode::Review(_)) {
            return;
        }
        let Some(path) = self.buffer.path().map(|p| p.to_path_buf()) else {
            return;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return;
        };
        let stamp = meta.modified().ok().map(|t| (meta.len(), t));
        if stamp == self.disk_stamp {
            return;
        }
        self.disk_stamp = stamp;
        let changed = matches!(fileio::disk_state(&path), Ok(Some(s)) if Some(s) != self.disk);
        if changed != self.outside_change {
            self.outside_change = changed;
            self.needs_redraw = true;
        }
    }

    /// Starts reviewing what changed on disk.
    fn open_review(&mut self) {
        let Some(path) = self.buffer.path().map(|p| p.to_path_buf()) else {
            return;
        };
        let theirs = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.set_error(format!("Can't read the file: {e}"));
                return;
            }
        };
        let ours = self.buffer.to_string();
        let base = self
            .disk_text
            .as_ref()
            .map_or(ours.clone(), |r| r.to_string());
        let changes = review::changes(&base, &ours, &theirs);
        // The version goes in the history before any choice can lose it.
        let (outside, current) = (ropey::Rope::from_str(&theirs), self.buffer.rope().clone());
        self.with_history(|h| h.note_outside(&outside, &current));
        let run = ReviewRun {
            changes: std::sync::Arc::new(changes),
            theirs: std::sync::Arc::new(theirs),
            at: 0,
            shift: 0,
            accepted: 0,
        };
        if run.changes.is_empty() {
            self.set_message("Nothing to review; you already have those changes");
            self.finish_review(&run);
        } else {
            self.show_change(run);
        }
    }

    /// The buffer's lines for a change, allowing for changes already taken.
    fn change_lines(&self, run: &ReviewRun, i: usize) -> std::ops::Range<usize> {
        let c = &run.changes[i];
        let at = |line: usize| (line as isize + run.shift) as usize;
        at(c.ours.start)..at(c.ours.end)
    }

    fn theirs_text(run: &ReviewRun, i: usize) -> String {
        review::lines(&run.theirs)[run.changes[i].theirs.clone()].concat()
    }

    /// Shows the change at `run.at`: the buffer, with the incoming lines
    /// placed under the lines they'd replace.
    fn show_change(&mut self, run: ReviewRun) {
        let lines = self.change_lines(&run, run.at);
        let incoming = Self::theirs_text(&run, run.at);
        let added = run.changes[run.at].theirs.len();
        let mut rope = self.buffer.rope().clone();
        let at = rope.line_to_char(lines.end.min(rope.len_lines()));
        let needs_break = at > 0 && rope.char(at - 1) != '\n' && !incoming.is_empty();
        if needs_break {
            rope.insert(at, self.buffer.eol());
        }
        let at = rope.line_to_char(lines.end.min(rope.len_lines()));
        rope.insert(at, &incoming);
        let mut view = self.buffer.view_of(rope);
        let first = lines.start.min(view.line_count() - 1);
        view.move_to(view.line_start(first));
        self.review_marks = Some((lines.clone(), lines.end..lines.end + added));
        self.version_view = Some(view);
        self.bring_into_view(&(lines.start..lines.end + added));
        self.mode = Mode::Review(run);
    }

    /// The edit that takes change `i`.
    fn change_edit(&self, run: &ReviewRun, i: usize) -> Edit {
        let lines = self.change_lines(run, i);
        let rope = self.buffer.rope();
        let start = rope.line_to_char(lines.start);
        let end = rope.line_to_char(lines.end.min(rope.len_lines()));
        Edit {
            at: start,
            removed: self.buffer.slice_string(start..end),
            inserted: Self::theirs_text(run, i),
        }
    }

    fn review_key(&mut self, key: Key, mut run: ReviewRun) {
        let i = run.at;
        match key {
            Key::Char('y') => {
                let edit = self.change_edit(&run, i);
                let end = edit.at + edit.inserted.chars().count();
                self.buffer
                    .apply_with_cursor(edit, GroupKind::Operator, end);
                let c = &run.changes[i];
                run.shift += c.theirs.len() as isize - c.ours.len() as isize;
                run.accepted += 1;
            }
            Key::Char('n') => {}
            Key::Char('a') => {
                let mut edits: Vec<Edit> = (i..run.changes.len())
                    .map(|j| self.change_edit(&run, j))
                    .collect();
                let at = edits[0].at;
                run.accepted += edits.len();
                // Last first, so earlier positions stay right.
                edits.reverse();
                self.buffer.apply_group(edits, at);
                self.finish_review(&run);
                return;
            }
            Key::Esc | Key::Char('q') => {
                self.close_review();
                self.set_message("Review stopped; the file still has changes you haven't seen");
                return;
            }
            _ => {
                self.show_change(run);
                return;
            }
        }
        run.at += 1;
        if run.at == run.changes.len() {
            self.finish_review(&run);
        } else {
            self.show_change(run);
        }
    }

    fn close_review(&mut self) {
        self.version_view = None;
        self.review_marks = None;
        self.mode = Mode::Insert;
    }

    /// Every change has an answer, so the file on disk is now the known one.
    fn finish_review(&mut self, run: &ReviewRun) {
        self.close_review();
        self.disk = Some(DiskState::of_bytes(run.theirs.as_bytes()));
        self.disk_text = Some(ropey::Rope::from_str(&run.theirs));
        self.outside_change = false;
        if self.buffer.to_string() == *run.theirs {
            self.buffer.mark_clean();
        }
        if !run.changes.is_empty() {
            let (took, all) = (run.accepted, run.changes.len());
            self.set_message(format!("Took {took} of {all} changes"));
        }
    }

    /// Opens the timeline at the newest version.
    fn open_history(&mut self) {
        let Some(h) = self.history.as_ref() else {
            let why = if !self.history_on {
                "File history is off this session"
            } else {
                "This buffer has no file yet, so it has no history"
            };
            self.set_message(why);
            return;
        };
        match Timeline::load(h.path()) {
            Ok(t) if t.is_empty() => {
                self.set_message("No history yet; it starts with your first edit")
            }
            Ok(t) => {
                let at = t.len() - 1;
                self.show_version(std::sync::Arc::new(t), at);
            }
            Err(e) => self.set_error(format!("Can't read the history: {e}")),
        }
    }

    /// Shows version `at`, with what it changed selected.
    fn show_version(&mut self, timeline: SharedTimeline, at: usize) {
        let mut view = self.buffer.view_of(timeline.text_at(at));
        if let Change::Edit { at: pos, ins, .. } = &timeline.version(at).change {
            let end = pos + ins.chars().count();
            view.move_to(end);
            view.set_selection_anchor(*pos);
        }
        self.version_view = Some(view);
        self.mode = Mode::History { timeline, at };
    }

    fn close_history(&mut self) {
        self.version_view = None;
        self.mode = Mode::Insert;
    }

    fn history_key(&mut self, key: Key, timeline: SharedTimeline, at: usize) {
        let target = match key {
            Key::Left => at.saturating_sub(1),
            Key::Right => (at + 1).min(timeline.len() - 1),
            Key::Up => timeline.prev_burst(at),
            Key::Down => timeline.next_burst(at),
            Key::PageUp => timeline.prev_saved(at).unwrap_or(at),
            Key::PageDown => timeline.next_saved(at).unwrap_or(at),
            Key::Home => 0,
            Key::End => timeline.len() - 1,
            Key::Enter => {
                let text = timeline.text_at(at).to_string();
                let when = history::ago_from_now(timeline.version(at).time_ms);
                self.close_history();
                if self.buffer.replace_all(&text) {
                    self.set_message(format!(
                        "Restored the version from {when}; Ctrl-Z undoes it"
                    ));
                } else {
                    self.set_message("That's the version you have now");
                }
                return;
            }
            Key::Esc | Key::Char('q') => {
                self.close_history();
                return;
            }
            _ => at,
        };
        self.show_version(timeline, target);
    }

    fn delete_history(&mut self) {
        let text = self.buffer.rope().clone();
        self.with_history(|h| h.delete(&text));
        if self.history.is_some() {
            self.set_message("Deleted this file's history");
        }
    }

    /// Switches this file between tabs and spaces for new indent.
    fn toggle_indent(&mut self) {
        let next = match self.buffer.indent() {
            Indent::Tabs => Indent::Spaces(4),
            Indent::Spaces(_) => Indent::Tabs,
        };
        self.buffer.set_indent(next);
        self.set_message(match next {
            Indent::Tabs => "Indenting with tabs".to_string(),
            Indent::Spaces(n) => format!("Indenting with {n} spaces"),
        });
    }

    /// Does what a save, or a decision to discard, was for.
    fn after_save(&mut self, then: After) {
        match then {
            After::Stay => {}
            After::Quit => self.quit = true,
            After::Open(path) => self.switch_to(&path),
        }
    }

    /// Checks the typed name and starts the save; `Err` keeps the prompt open.
    fn submit_save_as(&mut self, text: &str, then: After) -> Result<(), String> {
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
            self.save_after(path, None, then);
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
                then,
            });
        } else {
            self.write_file(&path, then);
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
    fn save_after(&mut self, path: std::path::PathBuf, passed: Option<SaveRisk>, then: After) {
        match self.next_risk(&path, passed) {
            Some(risk) => self.mode = Mode::Prompt(Prompt::ConfirmSave { risk, path, then }),
            None => self.write_file(&path, then),
        }
    }

    fn write_file(&mut self, path: &std::path::Path, then: After) {
        let text = self.buffer.to_string();
        let overwritten = self.outside_version(path);
        match fileio::save_atomic(path, &text) {
            Ok(()) => {
                self.disk = Some(DiskState::of_bytes(text.as_bytes()));
                self.disk_text = Some(self.buffer.rope().clone());
                self.disk_stamp = None;
                self.outside_change = false;
                self.needs_redraw = true;
                if self.buffer.path() != Some(path) {
                    self.buffer.set_path(path.to_path_buf());
                    self.swap_target = fileio::resolve_target(path).ok();
                }
                self.buffer.mark_clean();
                self.note_save(path, overwritten);
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
                self.after_save(then);
            }
            Err(e) => self.set_error(format!("Save failed: {e}. Use Save as in the palette")),
        }
    }

    /// The file's content, if it isn't what Kyte last read or wrote. A save is
    /// about to replace it, so the history should keep it.
    fn outside_version(&self, path: &std::path::Path) -> Option<String> {
        if self.buffer.path() != Some(path) || self.history.is_none() {
            return None;
        }
        let state = fileio::disk_state(path).ok()??;
        (Some(state) != self.disk)
            .then(|| std::fs::read_to_string(path).ok())
            .flatten()
    }

    /// Records a finished save in the history, for the file it went to.
    fn note_save(&mut self, path: &std::path::Path, overwritten: Option<String>) {
        let moved = self.history.as_ref().map(|h| h.target());
        if moved != fileio::resolve_target(path).ok().as_deref() {
            self.swap_target = fileio::resolve_target(path).ok();
            self.start_history();
        }
        let text = self.buffer.rope().clone();
        self.with_history(|h| {
            if let Some(old) = overwritten {
                h.snapshot(&ropey::Rope::from_str(&old), SnapshotKind::Outside)?;
            }
            h.mark_saved(&text)
        });
    }

    /// Ctrl-Q: quit now when clean, otherwise ask.
    pub fn request_quit(&mut self) {
        if self.buffer.is_dirty() {
            self.mode = Mode::Prompt(Prompt::ConfirmLeave { then: After::Quit });
        } else {
            self.quit = true;
        }
    }

    /// Quits discarding changes ("Quit without saving").
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
            Mode::Prompt(Prompt::ReplaceFind { .. })
            | Mode::Prompt(Prompt::ReplaceWith { .. })
            | Mode::Prompt(Prompt::ReplaceConfirm(_)) => "REPLACE",
            Mode::Prompt(Prompt::ConfirmLeave { .. }) => "SAVE?",
            Mode::Prompt(Prompt::Open { .. }) => "OPEN",
            Mode::Prompt(Prompt::SaveAs { .. }) => "SAVE AS",
            Mode::Prompt(Prompt::ConfirmSave { .. }) => "SAVE?",
            Mode::History { .. } => "HISTORY",
            Mode::Reader => "READER",
            Mode::Review(_) => "REVIEW",
            Mode::Prompt(Prompt::ConfirmDeleteHistory) => "DELETE?",
            Mode::Help { .. } => "HELP",
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
    fn indent_keys_and_the_toggle() {
        let t = Instant::now();
        let mut a = app("x\n");
        a.handle_key(Key::Tab, t).unwrap();
        assert_eq!(a.buffer.to_string(), "\tx\n");
        a.handle_key(Key::BackTab, t).unwrap();
        assert_eq!(a.buffer.to_string(), "x\n");
        a.run_command(Command::ToggleIndent);
        assert_eq!(a.message.as_deref(), Some("Indenting with 4 spaces"));
        a.handle_key(Key::Tab, t).unwrap();
        assert_eq!(a.buffer.to_string(), "    x\n");
        a.run_command(Command::ToggleIndent);
        assert_eq!(a.message.as_deref(), Some("Indenting with tabs"));
    }

    #[test]
    fn word_keys_move_and_delete() {
        let t = Instant::now();
        let mut a = app("foo bar baz");
        a.handle_key(Key::WordRight, t).unwrap();
        assert_eq!(a.buffer.cursor(), 4);
        a.handle_key(Key::WordRight, t).unwrap();
        assert_eq!(a.buffer.cursor(), 8);
        a.handle_key(Key::WordLeft, t).unwrap();
        assert_eq!(a.buffer.cursor(), 4);
        a.handle_key(Key::WordDelete, t).unwrap();
        assert_eq!(a.buffer.to_string(), "foo baz");
        a.handle_key(Key::WordBackspace, t).unwrap();
        assert_eq!(a.buffer.to_string(), "baz");
        assert_eq!(a.buffer.cursor(), 0);
        a.handle_key(Key::Ctrl('z'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "foo baz", "one undo step");
        assert_eq!(a.buffer.cursor(), 4);
    }

    #[test]
    fn word_backspace_at_the_start_does_nothing() {
        let mut a = app("abc");
        a.handle_key(Key::WordBackspace, Instant::now()).unwrap();
        assert!(!a.buffer.is_dirty());
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
        assert_eq!(
            a.mode,
            Mode::Prompt(Prompt::ConfirmLeave { then: After::Quit })
        );
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert!(!a.quit);
        a.handle_key(Key::Ctrl('q'), t).unwrap();
        for ignored in [Key::Ctrl('q'), Key::Char('z'), Key::Enter] {
            a.handle_key(ignored, t).unwrap();
            assert_eq!(a.mode_label(), "SAVE?");
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
                then: After::Quit,
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
    fn search_up_goes_back_and_ctrl_f_goes_on() {
        let t = Instant::now();
        let mut a = search_app();
        a.handle_key(Key::Ctrl('f'), t).unwrap();
        typ(&mut a, "ba");
        a.handle_key(Key::Ctrl('f'), t).unwrap();
        assert_eq!(a.buffer.cursor(), 8);
        a.handle_key(Key::Up, t).unwrap();
        assert_eq!(a.buffer.cursor(), 4);
        a.handle_key(Key::Up, t).unwrap();
        assert_eq!(a.buffer.cursor(), 8, "wraps to the end");
    }

    #[test]
    fn search_starts_from_a_selected_word() {
        let t = Instant::now();
        let mut a = search_app();
        a.buffer.move_to(7);
        a.buffer.set_selection_anchor(4);
        a.handle_key(Key::Ctrl('f'), t).unwrap();
        assert_eq!(a.search_query(), Some("bar"));
        assert_eq!(a.buffer.selection(), None);
        assert_eq!(a.buffer.cursor(), 4);
    }

    /// Runs Ctrl-R with `query` and `with`, stopping at the first question.
    fn start_replace(a: &mut App, query: &str, with: &str) {
        let t = Instant::now();
        a.handle_key(Key::Ctrl('r'), t).unwrap();
        typ(a, query);
        a.handle_key(Key::Enter, t).unwrap();
        typ(a, with);
        a.handle_key(Key::Enter, t).unwrap();
    }

    #[test]
    fn replace_one_by_one() {
        let mut a = app("cat cat cat");
        start_replace(&mut a, "cat", "dog");
        assert_eq!(a.mode_label(), "REPLACE");
        assert_eq!(a.buffer.cursor(), 0);
        typ(&mut a, "y");
        assert_eq!(a.buffer.to_string(), "dog cat cat");
        typ(&mut a, "n");
        typ(&mut a, "y");
        assert_eq!(a.buffer.to_string(), "dog cat dog");
        assert_eq!(a.mode, Mode::Insert);
        assert_eq!(a.message.as_deref(), Some("Replaced 2 matches"));
    }

    #[test]
    fn replace_all_is_one_undo_step_and_wraps_around() {
        let t = Instant::now();
        let mut a = app("a x b x c x");
        a.buffer.move_to(6);
        start_replace(&mut a, "x", "yy");
        assert_eq!(a.buffer.cursor(), 6);
        typ(&mut a, "a");
        assert_eq!(a.buffer.to_string(), "a yy b yy c yy");
        assert_eq!(a.message.as_deref(), Some("Replaced 3 matches"));
        a.handle_key(Key::Ctrl('z'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "a x b x c x");
    }

    #[test]
    fn replacing_never_rereads_its_own_output() {
        let mut a = app("aa aa");
        start_replace(&mut a, "a", "aa");
        typ(&mut a, "yyyy");
        assert_eq!(a.buffer.to_string(), "aaaa aaaa");
        assert_eq!(a.mode, Mode::Insert);
    }

    #[test]
    fn replace_wraps_and_stops_where_it_started_after_growing_the_text() {
        let mut a = app("x-x-x");
        a.buffer.move_to(2);
        start_replace(&mut a, "x", "long");
        typ(&mut a, "y"); // middle
        typ(&mut a, "y"); // last
        typ(&mut a, "y"); // wraps to the first
        assert_eq!(a.buffer.to_string(), "long-long-long");
        assert_eq!(a.message.as_deref(), Some("Replaced 3 matches"));
    }

    #[test]
    fn replace_with_nothing_deletes_and_stop_keeps_the_rest() {
        let mut a = app("ab ab ab");
        start_replace(&mut a, "ab ", "");
        typ(&mut a, "y");
        assert_eq!(a.buffer.to_string(), "ab ab");
        typ(&mut a, "q");
        assert_eq!(a.mode, Mode::Insert);
        assert_eq!(a.buffer.to_string(), "ab ab");
        assert_eq!(a.message.as_deref(), Some("Replaced 1 match"));
    }

    #[test]
    fn replace_with_no_match_says_so_and_changes_nothing() {
        let mut a = app("abc");
        start_replace(&mut a, "zzz", "y");
        assert_eq!(a.mode, Mode::Insert);
        assert_eq!(a.message.as_deref(), Some("No matches replaced"));
        assert!(!a.buffer.is_dirty());
    }

    #[test]
    fn replace_matches_any_case_like_search() {
        let mut a = app("Foo foo FOO");
        start_replace(&mut a, "foo", "bar");
        typ(&mut a, "a");
        assert_eq!(a.buffer.to_string(), "bar bar bar");
    }

    #[test]
    fn replace_empty_find_does_not_advance() {
        let t = Instant::now();
        let mut a = app("abc");
        a.handle_key(Key::Ctrl('r'), t).unwrap();
        a.handle_key(Key::Enter, t).unwrap();
        assert!(matches!(a.mode, Mode::Prompt(Prompt::ReplaceFind { .. })));
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
    fn ctrl_k_jumps_to_a_line_and_searches() {
        let t = Instant::now();
        let mut a = app("one\ntwo\nthree\n");
        a.handle_key(Key::Ctrl('k'), t).unwrap();
        assert!(matches!(&a.mode, Mode::Palette(s) if s.is_universal()));
        typ(&mut a, ":3");
        a.handle_key(Key::Enter, t).unwrap();
        assert_eq!(a.buffer.line_col(), (3, 1));
        assert_eq!(a.mode, Mode::Insert);
        a.handle_key(Key::Ctrl('k'), t).unwrap();
        typ(&mut a, "/two");
        a.handle_key(Key::Enter, t).unwrap();
        assert_eq!(a.buffer.line_col(), (2, 1), "wraps to find it");
        assert_eq!(a.search_query(), Some("two"));
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
    }

    #[test]
    fn ctrl_k_runs_commands_and_esc_closes_it() {
        let t = Instant::now();
        let mut a = app("x");
        a.handle_key(Key::Ctrl('k'), t).unwrap();
        typ(&mut a, "toggle line");
        a.handle_key(Key::Enter, t).unwrap();
        assert!(!a.show_line_numbers);
        a.handle_key(Key::Ctrl('k'), t).unwrap();
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
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
        assert_eq!(a.mode, Mode::Help { top: 0 });
        a.handle_key(Key::Char('z'), t).unwrap();
        assert_eq!(a.mode, Mode::Help { top: 0 });
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
        a.handle_key(Key::Ctrl('q'), Instant::now()).unwrap();
        assert!(a.quit);
    }

    fn select_right(a: &mut App, n: usize) {
        for _ in 0..n {
            a.handle_key(Key::Select(Motion::Right), Instant::now())
                .unwrap();
        }
    }

    #[test]
    fn shift_motion_extends_and_plain_motion_ends_the_selection() {
        let t = Instant::now();
        let mut a = app("hello world");
        select_right(&mut a, 3);
        assert_eq!(a.buffer.selection(), Some(0..3));
        a.handle_key(Key::Select(Motion::End), t).unwrap();
        assert_eq!(a.buffer.selection(), Some(0..11));
        a.handle_key(Key::Select(Motion::Home), t).unwrap();
        assert_eq!(a.buffer.selection(), None, "back at the anchor");
        select_right(&mut a, 2);
        a.handle_key(Key::Right, t).unwrap();
        assert_eq!(a.buffer.selection(), None);
    }

    #[test]
    fn selection_by_word_and_across_lines() {
        let t = Instant::now();
        let mut a = app("foo bar\nbaz\n");
        a.handle_key(Key::Select(Motion::WordRight), t).unwrap();
        assert_eq!(a.buffer.selection(), Some(0..4));
        a.handle_key(Key::Select(Motion::Down), t).unwrap();
        assert_eq!(a.buffer.selection(), Some(0..11));
        a.handle_key(Key::Select(Motion::FileEnd), t).unwrap();
        assert_eq!(a.buffer.selection(), Some(0..12));
    }

    #[test]
    fn esc_with_a_selection_opens_its_menu_and_a_second_esc_clears_it() {
        let t = Instant::now();
        let mut a = app("hello");
        select_right(&mut a, 2);
        a.handle_key(Key::Esc, t).unwrap();
        assert!(matches!(a.mode, Mode::Overlay { .. }));
        assert_eq!(a.buffer.selection(), Some(0..2), "still selected");
        assert!(a.layer_rows().contains(&("x", "cut")));
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.buffer.selection(), None);
        assert_eq!(a.mode, Mode::Insert, "not the palette");
    }

    fn code_app(path: &str, text: &str) -> App {
        App::new(
            Buffer::from_str(text, Some(path.into())),
            None,
            &Options::default(),
        )
    }

    fn esc_then(a: &mut App, c: char) {
        let t = Instant::now();
        a.handle_key(Key::Esc, t).unwrap();
        a.handle_key(Key::Char(c), t).unwrap();
    }

    #[test]
    fn selection_actions_change_case_sort_and_cut() {
        let mut a = app("hello world");
        select_right(&mut a, 5);
        esc_then(&mut a, 'U');
        assert_eq!(a.buffer.to_string(), "HELLO world");
        assert_eq!(
            a.buffer.selection(),
            Some(0..5),
            "ready for the next action"
        );
        esc_then(&mut a, 'L');
        assert_eq!(a.buffer.to_string(), "hello world");
        esc_then(&mut a, 'x');
        assert_eq!(a.buffer.to_string(), " world");
        let mut b = app("b\na\n");
        b.buffer.select_all();
        esc_then(&mut b, 'S');
        assert_eq!(b.buffer.to_string(), "a\nb\n");
    }

    #[test]
    fn find_and_replace_from_a_selection_start_with_its_text() {
        let mut a = app("cat dog cat");
        select_right(&mut a, 3);
        esc_then(&mut a, '/');
        assert_eq!(a.search_query(), Some("cat"));
        a.handle_key(Key::Esc, Instant::now()).unwrap();
        a.buffer.move_to(0);
        select_right(&mut a, 3);
        esc_then(&mut a, 'R');
        assert_eq!(a.search_query(), Some("cat"));
    }

    #[test]
    fn unknown_key_keeps_the_selection_and_types_nothing() {
        let mut a = app("hello");
        select_right(&mut a, 2);
        esc_then(&mut a, 'z');
        assert_eq!(a.buffer.selection(), Some(0..2));
        assert_eq!(a.buffer.to_string(), "hello");
        assert_eq!(a.mode, Mode::Insert);
    }

    #[test]
    fn layer_rows_follow_the_context() {
        let rows = |a: &App| a.layer_rows().into_iter().map(|r| r.0).collect::<Vec<_>>();
        let plain = code_app("/x/notes.txt", "(a)");
        let r = rows(&plain);
        assert!(r.contains(&"w") && r.contains(&"i"), "on a bracket: {r:?}");
        assert!(!r.contains(&"#"), "no comment syntax for .txt");
        assert_eq!(r.last(), Some(&"Esc Esc"));
        let mut off = code_app("/x/notes.txt", "(a) b");
        off.buffer.move_to(4);
        assert!(!rows(&off).contains(&"i"), "not on a bracket");
        let mut unmatched = code_app("/x/notes.txt", "(a");
        unmatched.buffer.move_to(0);
        assert!(!rows(&unmatched).contains(&"i"), "no partner");
        let toml = code_app("/x/a.toml", "a = 1\n");
        assert!(rows(&toml).contains(&"#"));
    }

    #[test]
    fn inside_brackets_selects_between_them_from_either_end() {
        let mut a = code_app("/x/a.txt", "f(abc)");
        a.buffer.move_to(1);
        esc_then(&mut a, 'i');
        assert_eq!(a.buffer.selection(), Some(2..5));
        a.buffer.move_to(5);
        esc_then(&mut a, 'i');
        assert_eq!(a.buffer.selection(), Some(2..5));
    }

    #[test]
    fn comment_key_toggles_the_line() {
        let mut a = code_app("/x/a.py", "x = 1\n");
        esc_then(&mut a, '#');
        assert_eq!(a.buffer.to_string(), "# x = 1\n");
        esc_then(&mut a, '#');
        assert_eq!(a.buffer.to_string(), "x = 1\n");
        let mut plain = code_app("/x/a.txt", "x\n");
        esc_then(&mut plain, '#');
        assert!(!plain.buffer.is_dirty());
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn code_keys_select_and_move_by_function() {
        let text = "fn one() {\n    let a = 1;\n}\n\nfn two() {}\n";
        let mut a = code_app("/x/a.rs", text);
        assert!(a.layer_rows().contains(&("f", "select function")));
        a.buffer.move_to(16);
        esc_then(&mut a, 'f');
        assert_eq!(a.buffer.selection(), Some(0..27));
        // then act on it, the way a user would: Esc f, Esc x
        esc_then(&mut a, 'x');
        assert_eq!(a.buffer.to_string(), "\n\nfn two() {}\n");
        a.handle_key(Key::Ctrl('z'), Instant::now()).unwrap();
        a.buffer.move_to(0);
        esc_then(&mut a, 'n');
        assert_eq!(a.buffer.cursor(), 29);
        esc_then(&mut a, 'p');
        assert_eq!(a.buffer.cursor(), 0);
        esc_then(&mut a, 'p');
        assert_eq!(a.message.as_deref(), Some("No more functions"));
        a.buffer.move_to(28);
        esc_then(&mut a, 'f');
        assert_eq!(a.message.as_deref(), Some("Not inside a function"));
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn expand_grows_the_selection_one_level_at_a_time() {
        let mut a = code_app("/x/a.rs", "fn f() { g(1 + 2); }\n");
        a.buffer.move_to(11);
        esc_then(&mut a, 'e');
        assert_eq!(a.buffer.selection(), Some(11..12));
        esc_then(&mut a, 'e');
        assert_eq!(a.buffer.selection(), Some(11..16));
        esc_then(&mut a, 'e');
        assert_eq!(a.buffer.selection(), Some(10..17));
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn code_keys_keep_char_positions_with_non_ascii_text() {
        let mut a = code_app("/x/a.rs", "// 世界\nfn f() {}\n");
        a.buffer.move_to(7);
        esc_then(&mut a, 'f');
        assert_eq!(a.buffer.selection(), Some(6..15));
    }

    #[test]
    fn reader_is_read_only_and_esc_returns_to_editing() {
        let t = Instant::now();
        let mut a = code_app("/x/a.txt", "one\ntwo\n\nthree\n");
        esc_then(&mut a, 'v');
        assert_eq!(a.mode_label(), "READER");
        assert_eq!(a.reader.as_ref().unwrap().focus, 0..2);
        for key in [Key::Char('x'), Key::Backspace, Key::Enter, Key::Ctrl('v')] {
            a.handle_key(key, t).unwrap();
        }
        a.handle_paste("pasted".to_string()).unwrap();
        assert!(!a.buffer.is_dirty());
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
        assert_eq!(a.reader, None);
    }

    #[test]
    fn reader_focus_follows_the_cursor() {
        let t = Instant::now();
        let mut a = code_app("/x/a.txt", "one\n\ntwo\n");
        a.run_command(Command::Reader);
        a.handle_key(Key::Down, t).unwrap();
        assert_eq!(a.reader.as_ref().unwrap().focus, 2..3);
        a.handle_key(Key::Char('p'), t).unwrap();
        assert_eq!(a.reader.as_ref().unwrap().focus, 0..1);
        a.handle_key(Key::Char('p'), t).unwrap();
        assert_eq!(a.message.as_deref(), Some("No more sections"));
    }

    #[test]
    fn reader_keys_show_at_first_then_on_request() {
        let t = Instant::now();
        let mut a = code_app("/x/a.txt", "one\n");
        a.run_command(Command::Reader);
        let shown = |a: &App| a.reader.as_ref().unwrap().show_keys;
        assert!(shown(&a));
        a.handle_key(Key::Down, t).unwrap();
        assert!(!shown(&a));
        a.handle_key(Key::Char('?'), t).unwrap();
        assert!(shown(&a));
        a.handle_key(Key::Char('?'), t).unwrap();
        assert!(!shown(&a));
    }

    #[test]
    fn reader_scrolls_only_when_a_section_is_off_screen() {
        let t = Instant::now();
        let text = format!("one\n\ntwo\n{}\n\nfar\n", "x\n".repeat(60));
        let mut a = code_app("/x/a.txt", &text);
        a.layout(40, 20);
        a.run_command(Command::Reader);
        a.handle_key(Key::Char('n'), t).unwrap();
        assert_eq!(a.scroll().0, 0, "the next section was already showing");
        a.handle_key(Key::Char('n'), t).unwrap();
        assert_eq!(a.scroll().0, 65 - 2, "the far one wasn't");
    }

    #[test]
    fn reader_clears_a_selection_it_replaces() {
        let t = Instant::now();
        let mut a = code_app("/x/a.txt", "one\n");
        a.buffer.select_all();
        a.run_command(Command::Reader);
        assert_eq!(a.buffer.selection(), None);
        a.handle_key(Key::Esc, t).unwrap();
        assert_eq!(a.mode, Mode::Insert);
    }

    #[test]
    fn code_keys_do_nothing_outside_code() {
        let mut a = code_app("/x/a.txt", "abc");
        esc_then(&mut a, 'f');
        esc_then(&mut a, 'n');
        assert_eq!(a.buffer.selection(), None);
        assert_eq!(a.buffer.cursor(), 0);
        assert_eq!(a.message, None);
    }

    #[test]
    fn alt_chord_with_a_selection_runs_the_selection_action() {
        let t = Instant::now();
        let mut a = app("one\ntwo\n");
        select_right(&mut a, 2);
        a.handle_key(Key::Alt('x'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "e\ntwo\n");
        assert_eq!(a.mode, Mode::Insert);
    }

    #[test]
    fn word_keys_delete_a_selection_when_there_is_one() {
        let t = Instant::now();
        let mut a = app("foo bar baz");
        a.buffer.move_to(4);
        select_right(&mut a, 3);
        a.handle_key(Key::WordBackspace, t).unwrap();
        assert_eq!(a.buffer.to_string(), "foo  baz");
        let mut b = app("foo bar baz");
        select_right(&mut b, 1);
        b.handle_key(Key::WordDelete, t).unwrap();
        assert_eq!(b.buffer.to_string(), "oo bar baz");
    }

    #[test]
    fn copy_and_paste_a_selection() {
        let t = Instant::now();
        let mut a = app("hello world");
        select_right(&mut a, 5);
        a.handle_key(Key::Ctrl('c'), t).unwrap();
        assert_eq!(a.message.as_deref(), Some("Copied 5 characters"));
        assert_eq!(
            a.take_terminal_output().as_deref(),
            Some("\x1b]52;c;aGVsbG8=\x07")
        );
        assert_eq!(a.take_terminal_output(), None, "sent once");
        assert_eq!(a.buffer.selection(), Some(0..5), "copy keeps the selection");
        a.handle_key(Key::CtrlEnd, t).unwrap();
        a.handle_key(Key::Ctrl('v'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "hello worldhello");
        a.handle_key(Key::Ctrl('z'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "hello world");
    }

    #[test]
    fn cut_a_selection_is_one_undo_step() {
        let t = Instant::now();
        let mut a = app("hello world");
        a.buffer.move_to(5);
        select_right(&mut a, 6);
        a.handle_key(Key::Ctrl('x'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "hello");
        a.handle_key(Key::Ctrl('z'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "hello world");
        a.handle_key(Key::CtrlHome, t).unwrap();
        a.handle_key(Key::Ctrl('v'), t).unwrap();
        assert_eq!(a.buffer.to_string(), " worldhello world");
    }

    #[test]
    fn with_no_selection_cut_and_copy_take_the_line() {
        let t = Instant::now();
        let mut a = app("one\ntwo\nthree");
        a.buffer.move_to(5);
        a.handle_key(Key::Ctrl('x'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "one\nthree");
        assert_eq!(a.message.as_deref(), Some("Copied 1 line"));
        a.buffer.move_to(1);
        a.handle_key(Key::Ctrl('v'), t).unwrap();
        assert_eq!(
            a.buffer.to_string(),
            "two\none\nthree",
            "a whole line pastes above, wherever the cursor is in the line"
        );
    }

    #[test]
    fn copying_a_last_line_with_no_ending_still_pastes_a_whole_line() {
        let t = Instant::now();
        let mut a = app("one\ntwo");
        a.handle_key(Key::CtrlEnd, t).unwrap();
        a.handle_key(Key::Ctrl('c'), t).unwrap();
        a.handle_key(Key::Home, t).unwrap();
        a.handle_key(Key::Ctrl('v'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "one\ntwo\ntwo");
    }

    #[test]
    fn pasting_over_a_selection_replaces_it() {
        let t = Instant::now();
        let mut a = app("abc xyz");
        select_right(&mut a, 3);
        a.handle_key(Key::Ctrl('c'), t).unwrap();
        a.handle_key(Key::End, t).unwrap();
        a.handle_key(Key::Select(Motion::WordLeft), t).unwrap();
        a.handle_key(Key::Ctrl('v'), t).unwrap();
        assert_eq!(a.buffer.to_string(), "abc abc");
    }

    #[test]
    fn empty_buffer_and_empty_clipboard_say_so() {
        let t = Instant::now();
        let mut a = app("");
        a.handle_key(Key::Ctrl('c'), t).unwrap();
        assert_eq!(a.message.as_deref(), Some("Nothing to copy"));
        a.handle_key(Key::Ctrl('x'), t).unwrap();
        assert_eq!(a.message.as_deref(), Some("Nothing to cut"));
        a.handle_key(Key::Ctrl('v'), t).unwrap();
        assert!(a.message.as_deref().unwrap().contains("Clipboard is empty"));
        assert!(!a.buffer.is_dirty());
    }

    #[test]
    fn paste_uses_the_file_line_ending() {
        let t = Instant::now();
        let mut a = app("a\nb\n");
        a.handle_key(Key::Select(Motion::FileEnd), t).unwrap();
        a.handle_key(Key::Ctrl('c'), t).unwrap();
        let mut dos = app("x\r\ny\r\n");
        dos.clipboard = a.clipboard.clone();
        dos.handle_key(Key::Ctrl('v'), t).unwrap();
        assert_eq!(dos.buffer.to_string(), "a\r\nb\r\nx\r\ny\r\n");
    }

    #[test]
    fn huge_copy_stays_inside_kyte() {
        let t = Instant::now();
        let mut a = app(&"x".repeat(2 << 20));
        a.handle_key(Key::Ctrl('c'), t).unwrap();
        assert!(a.message.as_deref().unwrap().contains("inside Kyte"));
        assert_eq!(a.take_terminal_output(), None);
    }
}
