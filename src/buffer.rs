//! Text buffer: a rope, a grapheme-aware cursor, and grouped undo/redo
//! (section 6). Every change goes through `apply` (or undo/redo, which use
//! the same bookkeeping) and is queued for the swap log.

use std::ops::Range;
use std::path::PathBuf;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Tabs render to the next multiple of this many columns.
pub const TAB_WIDTH: usize = 4;

/// A single replace operation in char indices. The inverse swaps `removed` and `inserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
}

impl Edit {
    pub fn insert(at: usize, s: &str) -> Edit {
        Edit {
            at,
            removed: String::new(),
            inserted: s.to_string(),
        }
    }

    pub fn inverse(&self) -> Edit {
        Edit {
            at: self.at,
            removed: self.inserted.clone(),
            inserted: self.removed.clone(),
        }
    }
}

/// What kind of change an undo group holds; only `Typing` coalesces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Typing,
    Operator,
    Paste,
    Other,
}

#[derive(Debug, Clone)]
struct Group {
    edits: Vec<Edit>,
    cursor_before: usize,
    cursor_after: usize,
    kind: GroupKind,
    /// Typing may still be appended to this group.
    open: bool,
}

/// Display width of one grapheme starting at display column `col`.
pub fn grapheme_width(g: &str, col: usize) -> usize {
    if g == "\t" {
        TAB_WIDTH - col % TAB_WIDTH
    } else if g.chars().any(|c| c.is_control()) {
        1
    } else {
        UnicodeWidthStr::width(g).max(1)
    }
}

/// Printable ASCII: one byte, one cell.
fn is_plain(b: u8) -> bool {
    (0x20..0x7f).contains(&b)
}

fn is_plain_char(c: char) -> bool {
    c.is_ascii() && is_plain(c as u8)
}

/// Graphemes of a string as (char offset, grapheme), found as they're needed
/// so a very long line costs only as much of it as gets walked. An ASCII byte
/// followed by another is its own grapheme unless it's the CR of a CRLF, which
/// skips the costly segmentation for most text.
#[derive(Clone)]
pub struct GraphemeWalk<'a> {
    text: &'a str,
    byte: usize,
    chars: usize,
}

impl<'a> GraphemeWalk<'a> {
    /// Skips up to `cols` cells of plain ASCII and returns how many it skipped.
    /// The last byte of a run stays, as it may join what follows it.
    pub fn skip_plain(&mut self, cols: usize) -> usize {
        let rest = &self.text.as_bytes()[self.byte..];
        let run = rest
            .iter()
            .take(cols + 1)
            .take_while(|b| is_plain(**b))
            .count();
        let n = run.saturating_sub(1).min(cols);
        self.byte += n;
        self.chars += n;
        n
    }
}

impl<'a> Iterator for GraphemeWalk<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = &self.text[self.byte..];
        let bytes = rest.as_bytes();
        let first = *bytes.first()?;
        let single =
            first.is_ascii() && first != b'\r' && bytes.get(1).is_none_or(|b| b.is_ascii());
        let g = if single {
            &rest[..1]
        } else {
            rest.graphemes(true).next()?
        };
        let at = self.chars;
        self.byte += g.len();
        self.chars += if single { 1 } else { g.chars().count() };
        Some((at, g))
    }
}

/// Graphemes of `s` as (char offset, grapheme).
pub fn graphemes_with_char_offsets(s: &str) -> GraphemeWalk<'_> {
    GraphemeWalk {
        text: s,
        byte: 0,
        chars: 0,
    }
}

/// Display column and grapheme count before char offset `off` in `text`.
fn measure(text: &str, off: usize) -> (usize, usize) {
    let mut walk = graphemes_with_char_offsets(text);
    let (mut col, mut count) = (0, 0);
    loop {
        let n = walk.skip_plain(off.saturating_sub(walk.chars));
        col += n;
        count += n;
        if walk.chars >= off {
            return (col, count);
        }
        let Some((_, g)) = walk.next() else {
            return (col, count);
        };
        col += grapheme_width(g, col);
        count += 1;
    }
}

/// One open UTF-8 document.
pub struct Buffer {
    rope: Rope,
    path: Option<PathBuf>,
    cursor: usize,
    want_col: usize,
    dirty: bool,
    undo: Vec<Group>,
    redo: Vec<Group>,
    pending_edits: Vec<Edit>,
    /// What Enter inserts: the line ending most of the file uses.
    eol: &'static str,
    /// The other end of the selection. Every motion and edit clears it, so
    /// only code that means to extend a selection has to think about it.
    anchor: Option<usize>,
}

/// The line ending most of `text` uses (LF on a tie).
fn dominant_eol(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

impl Buffer {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str, path: Option<PathBuf>) -> Buffer {
        Buffer {
            rope: Rope::from_str(text),
            path,
            cursor: 0,
            want_col: 0,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            pending_edits: Vec::new(),
            eol: dominant_eol(text),
            anchor: None,
        }
    }

    /// The selected char range, if any.
    pub fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor?;
        Some(a.min(self.cursor)..a.max(self.cursor))
    }

    /// Where a selection starting now would be anchored.
    pub fn selection_anchor(&self) -> usize {
        self.anchor.unwrap_or(self.cursor)
    }

    /// Anchors the selection at `at`; an empty one is no selection.
    pub fn set_selection_anchor(&mut self, at: usize) {
        self.anchor = (at != self.cursor).then_some(at);
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.close_group();
        self.cursor = self.rope.len_chars();
        self.want_col = self.cursor_display_col();
        self.set_selection_anchor(0);
    }

    /// Deletes the selection as one undo step; false when there is none.
    pub fn delete_selection(&mut self) -> bool {
        let Some(r) = self.selection() else {
            return false;
        };
        let edit = Edit {
            at: r.start,
            removed: self.slice_string(r.clone()),
            inserted: String::new(),
        };
        self.apply_with_cursor(edit, GroupKind::Operator, r.start);
        true
    }

    /// The line ending new lines get.
    pub fn eol(&self) -> &'static str {
        self.eol
    }

    /// Enter: inserts the file's line ending.
    pub fn insert_newline(&mut self) {
        self.insert_str(self.eol, GroupKind::Typing);
    }

    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        self.rope.to_string()
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// Gives the buffer a new file name (after Save As).
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Number of displayed lines (an empty last line after a trailing newline counts).
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Lines as editors count them for "Saved N lines": a trailing newline does
    /// not start a new line.
    pub fn saved_line_count(&self) -> usize {
        let n = self.rope.len_lines();
        let len = self.rope.len_chars();
        if len > 0 && self.rope.char(len - 1) == '\n' {
            n - 1
        } else {
            n
        }
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Marks the buffer as differing from disk (used after crash recovery).
    pub fn set_dirty(&mut self) {
        self.dirty = true;
    }

    /// Zero-based line of the cursor.
    pub fn cursor_line(&self) -> usize {
        self.rope.char_to_line(self.cursor)
    }

    /// 1-based (line, grapheme column), for the status bar.
    pub fn line_col(&self) -> (usize, usize) {
        let line = self.cursor_line();
        let start = self.rope.line_to_char(line);
        let text = self.line_text(line);
        let (_, col) = measure(&text, self.cursor - start);
        (line + 1, col + 1)
    }

    /// Text of line `l` without its line ending, LF or CRLF. A `\r` anywhere
    /// else stays an ordinary character.
    pub fn line_text(&self, l: usize) -> String {
        let mut s = self.rope.line(l).to_string();
        if s.ends_with('\n') {
            s.pop();
            if s.ends_with('\r') {
                s.pop();
            }
        }
        s
    }

    /// Whether char index `at` is the `\n` of a CRLF.
    fn inside_crlf(&self, at: usize) -> bool {
        at > 0
            && at < self.rope.len_chars()
            && self.rope.char(at) == '\n'
            && self.rope.char(at - 1) == '\r'
    }

    /// `at`, moved off the middle of a CRLF.
    fn outside_crlf(&self, at: usize) -> usize {
        if self.inside_crlf(at) {
            at - 1
        } else {
            at
        }
    }

    pub fn line_start(&self, l: usize) -> usize {
        self.rope.line_to_char(l)
    }

    /// Char index of the end of line `l` (before its newline).
    pub fn line_end(&self, l: usize) -> usize {
        crate::ops::eol(&self.rope, self.line_start(l))
    }

    /// Display column of the cursor within its line.
    pub fn cursor_display_col(&self) -> usize {
        let line = self.cursor_line();
        self.display_col(line, self.cursor - self.line_start(line))
    }

    /// Display column of char offset `off` within line `l`.
    pub fn display_col(&self, l: usize, off: usize) -> usize {
        measure(&self.line_text(l), off).0
    }

    pub fn slice_string(&self, r: Range<usize>) -> String {
        self.rope.slice(r).to_string()
    }

    // ----- editing -----

    fn raw_apply(&mut self, e: &Edit) {
        let n = e.removed.chars().count();
        if n > 0 {
            self.rope.remove(e.at..e.at + n);
        }
        if !e.inserted.is_empty() {
            self.rope.insert(e.at, &e.inserted);
        }
    }

    fn close_group(&mut self) {
        if let Some(g) = self.undo.last_mut() {
            g.open = false;
        }
    }

    /// Applies `edit`, records undo, marks dirty, and queues it for the swap.
    /// The cursor lands after the inserted text.
    pub fn apply(&mut self, edit: Edit, kind: GroupKind) {
        let after = edit.at + edit.inserted.chars().count();
        self.apply_with_cursor(edit, kind, after);
    }

    /// Like [`apply`](Self::apply) but places the cursor at `cursor_after`.
    pub fn apply_with_cursor(&mut self, edit: Edit, kind: GroupKind, cursor_after: usize) {
        let before = self.cursor;
        self.anchor = None;
        self.raw_apply(&edit);
        self.cursor = self.outside_crlf(cursor_after.min(self.rope.len_chars()));
        self.dirty = true;
        self.redo.clear();
        self.pending_edits.push(edit.clone());
        // Typing over a selection opens a group too, so one undo restores it.
        let simple_typing = kind == GroupKind::Typing && !edit.inserted.contains('\n');
        let coalesce = simple_typing
            && edit.removed.is_empty()
            && matches!(self.undo.last(), Some(g) if g.open && g.kind == GroupKind::Typing && g.cursor_after == edit.at);
        if coalesce {
            let g = self.undo.last_mut().unwrap();
            g.edits.push(edit);
            g.cursor_after = self.cursor;
        } else {
            self.close_group();
            self.undo.push(Group {
                edits: vec![edit],
                cursor_before: before,
                cursor_after: self.cursor,
                kind,
                open: simple_typing,
            });
        }
        self.want_col = self.cursor_display_col();
    }

    /// Applies `edits`, in order, as one undo step and leaves the cursor at
    /// `cursor_after`.
    pub fn apply_group(&mut self, edits: Vec<Edit>, cursor_after: usize) {
        if edits.is_empty() {
            return;
        }
        let before = self.cursor;
        self.anchor = None;
        for e in &edits {
            self.raw_apply(e);
            self.pending_edits.push(e.clone());
        }
        self.cursor = self.outside_crlf(cursor_after.min(self.rope.len_chars()));
        self.dirty = true;
        self.redo.clear();
        self.close_group();
        self.undo.push(Group {
            edits,
            cursor_before: before,
            cursor_after: self.cursor,
            kind: GroupKind::Operator,
            open: false,
        });
        self.want_col = self.cursor_display_col();
    }

    /// Inserts at the cursor, replacing the selection if there is one.
    pub fn insert_str(&mut self, s: &str, kind: GroupKind) {
        if s.is_empty() {
            return;
        }
        let edit = match self.selection() {
            Some(r) => Edit {
                at: r.start,
                removed: self.slice_string(r),
                inserted: s.to_string(),
            },
            None => Edit::insert(self.cursor, s),
        };
        self.apply(edit, kind);
    }

    /// Deletes the previous grapheme (joins with the previous line at a line start).
    pub fn backspace(&mut self) {
        if self.delete_selection() || self.cursor == 0 {
            return;
        }
        let prev = self.prev_boundary(self.cursor);
        let removed = self.slice_string(prev..self.cursor);
        self.apply(
            Edit {
                at: prev,
                removed,
                inserted: String::new(),
            },
            GroupKind::Other,
        );
    }

    /// Deletes the next grapheme (joins with the next line at a line end).
    pub fn delete_forward(&mut self) {
        if self.delete_selection() || self.cursor >= self.rope.len_chars() {
            return;
        }
        let next = self.next_boundary(self.cursor);
        let removed = self.slice_string(self.cursor..next);
        let at = self.cursor;
        self.apply_with_cursor(
            Edit {
                at,
                removed,
                inserted: String::new(),
            },
            GroupKind::Other,
            at,
        );
    }

    pub fn undo(&mut self) -> bool {
        let Some(mut g) = self.undo.pop() else {
            return false;
        };
        self.anchor = None;
        g.open = false;
        for e in g.edits.iter().rev() {
            let inv = e.inverse();
            self.raw_apply(&inv);
            self.pending_edits.push(inv);
        }
        self.cursor = self.outside_crlf(g.cursor_before.min(self.rope.len_chars()));
        self.want_col = self.cursor_display_col();
        self.dirty = true;
        self.redo.push(g);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(g) = self.redo.pop() else {
            return false;
        };
        self.anchor = None;
        for e in &g.edits {
            self.raw_apply(e);
            self.pending_edits.push(e.clone());
        }
        self.cursor = self.outside_crlf(g.cursor_after.min(self.rope.len_chars()));
        self.want_col = self.cursor_display_col();
        self.dirty = true;
        self.close_group();
        self.undo.push(g);
        true
    }

    pub fn take_pending_edits(&mut self) -> Vec<Edit> {
        std::mem::take(&mut self.pending_edits)
    }

    /// After a successful save.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    // ----- motion -----

    /// Previous grapheme boundary before `at` (crosses a line ending as one step).
    pub fn prev_boundary(&self, at: usize) -> usize {
        if at == 0 {
            return 0;
        }
        let line = self.rope.char_to_line(at);
        let start = self.line_start(line);
        if at == start {
            return self.outside_crlf(at - 1);
        }
        let plain = |i: usize| is_plain_char(self.rope.char(i));
        if plain(at - 1) && (at - 1 == start || plain(at - 2)) {
            return at - 1;
        }
        let text = self.line_text(line);
        let off = at - start;
        let mut best = 0;
        for (o, _) in graphemes_with_char_offsets(&text) {
            if o < off {
                best = o;
            } else {
                break;
            }
        }
        start + best
    }

    /// Next grapheme boundary after `at` (crosses a line ending as one step).
    pub fn next_boundary(&self, at: usize) -> usize {
        let len = self.rope.len_chars();
        if at >= len {
            return len;
        }
        let plain = |c: Option<char>| c.is_some_and(is_plain_char);
        if plain(self.rope.get_char(at)) && (at + 1 == len || plain(self.rope.get_char(at + 1))) {
            return at + 1;
        }
        let line = self.rope.char_to_line(at);
        let start = self.line_start(line);
        let text = self.line_text(line);
        let off = at - start;
        let line_len = text.chars().count();
        if off >= line_len {
            let crlf = self.inside_crlf(at + 1);
            return (at + if crlf { 2 } else { 1 }).min(len);
        }
        for (o, g) in graphemes_with_char_offsets(&text) {
            let end = o + g.chars().count();
            if end > off {
                return start + end;
            }
        }
        start + line_len
    }

    fn set_cursor_h(&mut self, c: usize) {
        self.close_group();
        self.anchor = None;
        self.cursor = c.min(self.rope.len_chars());
        self.want_col = self.cursor_display_col();
    }

    pub fn move_left(&mut self) {
        let c = self.prev_boundary(self.cursor);
        self.set_cursor_h(c);
    }

    pub fn move_right(&mut self) {
        let c = self.next_boundary(self.cursor);
        self.set_cursor_h(c);
    }

    /// Char index on line `l` closest to display column `col` without passing it.
    pub fn char_at_col(&self, l: usize, col: usize) -> usize {
        let start = self.line_start(l);
        let text = self.line_text(l);
        let mut c = 0;
        let mut last = text.chars().count();
        for (o, g) in graphemes_with_char_offsets(&text) {
            let w = grapheme_width(g, c);
            if c + w > col {
                last = o;
                break;
            }
            c += w;
        }
        start + last
    }

    /// Moves by `delta` lines keeping the remembered column (Up/Down/PageUp/PageDown).
    pub fn move_lines(&mut self, delta: isize) {
        self.close_group();
        self.anchor = None;
        let line = self.cursor_line() as isize;
        let max = self.line_count() as isize - 1;
        let target = (line + delta).clamp(0, max.max(0)) as usize;
        if target as isize == line {
            return;
        }
        self.cursor = self.char_at_col(target, self.want_col);
    }

    pub fn move_up(&mut self) {
        self.move_lines(-1);
    }

    pub fn move_down(&mut self) {
        self.move_lines(1);
    }

    pub fn move_home(&mut self) {
        let s = self.line_start(self.cursor_line());
        self.set_cursor_h(s);
    }

    pub fn move_end(&mut self) {
        let e = self.line_end(self.cursor_line());
        self.set_cursor_h(e);
    }

    /// Moves to `char_idx` (clamped), snapping back to a grapheme boundary.
    pub fn move_to(&mut self, char_idx: usize) {
        let idx = char_idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        let start = self.line_start(line);
        let text = self.line_text(line);
        let off = idx - start;
        let mut snapped = text.chars().count().min(off);
        for (o, g) in graphemes_with_char_offsets(&text) {
            if o <= off && off < o + g.chars().count() {
                snapped = o;
                break;
            }
        }
        self.set_cursor_h(start + snapped);
    }

    /// Moves to display column `col` on line `l` (clamped), updating the remembered column.
    pub fn move_to_line_col(&mut self, l: usize, col: usize) {
        let l = l.min(self.line_count().saturating_sub(1));
        let c = self.char_at_col(l, col);
        self.set_cursor_h(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Buffer {
        Buffer::from_str(s, None)
    }

    fn typed(buf: &mut Buffer, s: &str) {
        for c in s.chars() {
            let kind = GroupKind::Typing;
            buf.insert_str(&c.to_string(), kind);
        }
    }

    #[test]
    fn from_str_to_string_roundtrip_keeps_bytes() {
        for s in ["", "a", "a\n", "a\r\nb", "no newline", "x\n\n"] {
            assert_eq!(b(s).to_string(), s);
        }
    }

    #[test]
    fn counts() {
        let x = b("ab\ncd\n");
        assert_eq!(x.len_chars(), 6);
        assert_eq!(x.line_count(), 3);
        assert_eq!(x.saved_line_count(), 2);
        assert_eq!(b("ab").saved_line_count(), 1);
        assert_eq!(b("").saved_line_count(), 1);
    }

    #[test]
    fn cr_is_an_ordinary_character() {
        let x = b("a\rb\nc");
        assert_eq!(x.line_count(), 2);
        assert_eq!(x.line_text(0), "a\rb");
    }

    const FRAGMENTS: &[&str] = &[
        "a",
        "b",
        " ",
        "~",
        "\t",
        "\r",
        "\r\n",
        "\u{7f}",
        "é",
        "e\u{301}",
        "a\u{301}",
        "世",
        "🙂",
        "👨\u{200d}👩\u{200d}👧",
        "🇺🇸",
        "🇺",
        "\u{200d}",
        "\u{301}",
        "\u{1100}\u{1161}",
    ];

    proptest::proptest! {
        #[test]
        fn walk_matches_full_segmentation(
            parts in proptest::collection::vec(proptest::sample::select(FRAGMENTS), 0..40)
        ) {
            let s = parts.concat();
            let mut chars = 0;
            let mut expected = Vec::new();
            for g in s.graphemes(true) {
                expected.push((chars, g));
                chars += g.chars().count();
            }
            let walked: Vec<_> = graphemes_with_char_offsets(&s).collect();
            proptest::prop_assert_eq!(&walked, &expected);
        }

        #[test]
        fn measure_matches_a_full_walk(
            parts in proptest::collection::vec(proptest::sample::select(FRAGMENTS), 0..40),
            pick in 0usize..120,
        ) {
            let s = parts.concat();
            let off = pick.min(s.chars().count());
            let (mut col, mut count) = (0, 0);
            for (o, g) in s.graphemes(true).scan(0, |at, g| {
                let o = *at;
                *at += g.chars().count();
                Some((o, g))
            }) {
                if o >= off {
                    break;
                }
                col += grapheme_width(g, col);
                count += 1;
            }
            proptest::prop_assert_eq!(measure(&s, off), (col, count));
        }

        #[test]
        fn fast_boundaries_match_the_full_walk(
            parts in proptest::collection::vec(proptest::sample::select(FRAGMENTS), 0..30)
        ) {
            let s = parts.concat().replace("\r\n", "\n");
            let x = b(&s);
            let mut at = 0;
            while at < x.len_chars() {
                let next = x.next_boundary(at);
                proptest::prop_assert!(next > at);
                proptest::prop_assert_eq!(x.prev_boundary(next), at, "at {} in {:?}", at, s);
                at = next;
            }
        }
    }

    #[test]
    fn long_ascii_line_measures_with_marks_and_tabs() {
        let s = format!("{}e\u{301}\tx", "a".repeat(5000));
        // 5000 cells, one for e + mark, a tab to the next multiple of four
        assert_eq!(measure(&s, 5000), (5000, 5000));
        assert_eq!(measure(&s, 5002), (5001, 5001));
        assert_eq!(measure(&s, 5003), (5004, 5002));
    }

    fn select(x: &mut Buffer, from: usize, to: usize) {
        x.move_to(to);
        x.set_selection_anchor(from);
    }

    #[test]
    fn selection_range_is_ordered_and_an_empty_one_is_none() {
        let mut x = b("hello");
        assert_eq!(x.selection(), None);
        select(&mut x, 1, 4);
        assert_eq!(x.selection(), Some(1..4));
        select(&mut x, 4, 1);
        assert_eq!(x.selection(), Some(1..4));
        select(&mut x, 2, 2);
        assert_eq!(x.selection(), None);
    }

    #[test]
    fn motion_and_edits_clear_the_selection() {
        let mut x = b("hello\nworld");
        select(&mut x, 0, 3);
        x.move_right();
        assert_eq!(x.selection(), None);
        select(&mut x, 0, 3);
        x.move_down();
        assert_eq!(x.selection(), None);
        select(&mut x, 0, 3);
        x.insert_str("X", GroupKind::Typing);
        assert_eq!(x.selection(), None);
        select(&mut x, 0, 1);
        assert!(x.undo());
        assert_eq!(x.selection(), None);
    }

    #[test]
    fn typing_replaces_the_selection_in_one_undo_step() {
        let mut x = b("hello world");
        select(&mut x, 0, 5);
        typed(&mut x, "bye");
        assert_eq!(x.to_string(), "bye world");
        assert!(x.undo());
        assert_eq!(x.to_string(), "hello world", "the replaced text returns");
        assert!(!x.undo());
    }

    #[test]
    fn backspace_and_delete_remove_the_selection_only() {
        let mut x = b("abcdef");
        select(&mut x, 1, 3);
        x.backspace();
        assert_eq!(x.to_string(), "adef");
        assert_eq!(x.cursor(), 1);
        let mut y = b("abcdef");
        select(&mut y, 4, 2);
        y.delete_forward();
        assert_eq!(y.to_string(), "abef");
    }

    #[test]
    fn apply_group_is_one_undo_step() {
        let mut x = b("a-b-c");
        x.apply_group(
            vec![
                Edit {
                    at: 3,
                    removed: "-".into(),
                    inserted: "++".into(),
                },
                Edit {
                    at: 1,
                    removed: "-".into(),
                    inserted: "++".into(),
                },
            ],
            0,
        );
        assert_eq!(x.to_string(), "a++b++c");
        assert_eq!(x.take_pending_edits().len(), 2);
        assert!(x.undo());
        assert_eq!(x.to_string(), "a-b-c");
        assert!(x.redo());
        assert_eq!(x.to_string(), "a++b++c");
    }

    #[test]
    fn select_all_covers_everything() {
        let mut x = b("a\nb\n");
        x.select_all();
        assert_eq!(x.selection(), Some(0..4));
        assert_eq!(b("").selection(), None);
        let mut e = b("");
        e.select_all();
        assert_eq!(e.selection(), None);
    }

    #[test]
    fn dominant_line_ending_decides_what_enter_inserts() {
        assert_eq!(b("").eol(), "\n");
        assert_eq!(b("a\nb\n").eol(), "\n");
        assert_eq!(b("a\r\nb\r\n").eol(), "\r\n");
        assert_eq!(b("a\r\nb\r\nc\n").eol(), "\r\n");
        assert_eq!(b("a\r\nb\n").eol(), "\n", "a tie goes to LF");
        let mut x = b("one\r\n");
        x.move_end();
        x.insert_newline();
        assert_eq!(x.to_string(), "one\r\n\r\n");
        assert_eq!(x.line_col(), (2, 1));
    }

    #[test]
    fn crlf_is_one_line_ending_for_every_motion() {
        let mut x = b("ab\r\ncd\r\n");
        assert_eq!(x.line_text(0), "ab");
        x.move_end();
        assert_eq!(x.cursor(), 2);
        assert_eq!(x.line_col(), (1, 3));
        x.move_right();
        assert_eq!(x.cursor(), 4, "right steps over the whole line ending");
        assert_eq!(x.line_col(), (2, 1));
        x.move_left();
        assert_eq!(x.cursor(), 2, "left does too");
        x.move_down();
        x.move_end();
        assert_eq!(x.cursor(), 6);
    }

    #[test]
    fn crlf_is_one_character_for_backspace_and_delete() {
        let mut x = b("ab\r\ncd\r\n");
        x.move_down();
        x.backspace();
        assert_eq!(x.to_string(), "abcd\r\n");
        assert!(x.undo());
        assert_eq!(x.to_string(), "ab\r\ncd\r\n");
        let mut y = b("ab\r\ncd\r\n");
        y.move_end();
        y.delete_forward();
        assert_eq!(y.to_string(), "abcd\r\n");
    }

    #[test]
    fn typing_at_a_crlf_line_end_stays_before_the_ending() {
        let mut x = b("one\r\ntwo\r\n");
        x.move_end();
        typed(&mut x, "X");
        x.insert_newline();
        typed(&mut x, "Y");
        assert_eq!(x.to_string(), "oneX\r\nY\r\ntwo\r\n");
    }

    #[test]
    fn deleting_between_cr_and_lf_never_strands_the_cursor_inside_a_crlf() {
        let mut x = b("a\rb\nc");
        x.move_to(2);
        x.delete_forward();
        assert_eq!(x.to_string(), "a\r\nc");
        assert_eq!(x.cursor(), 1);
        assert!(x.undo());
        assert!(x.redo());
        assert_eq!(x.cursor(), 1);
    }

    #[test]
    fn lone_cr_stays_visible_text() {
        let mut x = b("a\rb\n");
        assert_eq!(x.line_text(0), "a\rb");
        x.move_end();
        assert_eq!(x.cursor(), 3);
    }

    #[test]
    fn insert_and_line_col() {
        let mut x = b("");
        typed(&mut x, "hello");
        x.insert_str("\n", GroupKind::Typing);
        typed(&mut x, "wo");
        assert_eq!(x.to_string(), "hello\nwo");
        assert_eq!(x.line_col(), (2, 3));
        assert!(x.is_dirty());
        x.mark_clean();
        assert!(!x.is_dirty());
    }

    #[test]
    fn typing_coalesces_into_one_undo_step() {
        let mut x = b("");
        typed(&mut x, "abc");
        assert!(x.undo());
        assert_eq!(x.to_string(), "");
        assert!(!x.undo());
        assert!(x.redo());
        assert_eq!(x.to_string(), "abc");
        assert_eq!(x.cursor(), 3);
    }

    #[test]
    fn cursor_move_and_newline_split_groups() {
        let mut x = b("");
        typed(&mut x, "ab");
        x.move_left();
        x.move_right();
        typed(&mut x, "cd");
        x.insert_str("\n", GroupKind::Typing);
        typed(&mut x, "ef");
        assert!(x.undo());
        assert_eq!(x.to_string(), "abcd\n");
        assert!(x.undo());
        assert_eq!(x.to_string(), "abcd");
        assert!(x.undo());
        assert_eq!(x.to_string(), "ab");
        assert!(x.undo());
        assert_eq!(x.to_string(), "");
    }

    #[test]
    fn operator_and_paste_are_single_groups() {
        let mut x = b("one\ntwo\n");
        x.apply(
            Edit {
                at: 0,
                removed: "one\n".into(),
                inserted: String::new(),
            },
            GroupKind::Operator,
        );
        x.insert_str("p1\np2\n", GroupKind::Paste);
        assert_eq!(x.to_string(), "p1\np2\ntwo\n");
        assert!(x.undo());
        assert_eq!(x.to_string(), "two\n");
        assert!(x.undo());
        assert_eq!(x.to_string(), "one\ntwo\n");
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut x = b("");
        typed(&mut x, "a");
        x.undo();
        typed(&mut x, "b");
        assert!(!x.redo());
        assert_eq!(x.to_string(), "b");
    }

    #[test]
    fn backspace_and_delete_join_lines() {
        let mut x = b("ab\ncd");
        x.move_to(3);
        x.backspace();
        assert_eq!(x.to_string(), "abcd");
        assert_eq!(x.cursor(), 2);
        x.delete_forward();
        assert_eq!(x.to_string(), "abd");
        assert_eq!(x.cursor(), 2);
        x.move_to(0);
        x.backspace();
        assert_eq!(x.to_string(), "abd");
        x.move_to(3);
        x.delete_forward();
        assert_eq!(x.to_string(), "abd");
    }

    #[test]
    fn pending_edits_are_a_forward_log() {
        let mut x = b("");
        typed(&mut x, "ab");
        x.undo();
        x.redo();
        let edits = x.take_pending_edits();
        let mut r = Rope::new();
        for e in &edits {
            let n = e.removed.chars().count();
            r.remove(e.at..e.at + n);
            r.insert(e.at, &e.inserted);
        }
        assert_eq!(r.to_string(), "ab");
        assert!(x.take_pending_edits().is_empty());
    }

    #[test]
    fn unicode_backspace_removes_one_grapheme() {
        let s = "héllo 世界 🙂";
        let mut x = b(s);
        x.move_end();
        x.backspace();
        assert_eq!(x.to_string(), "héllo 世界 ");
        // a decomposed e + combining acute is one grapheme
        let mut y = b("e\u{301}");
        y.move_end();
        y.backspace();
        assert_eq!(y.to_string(), "");
    }

    #[test]
    fn unicode_left_right_by_grapheme_and_display_col() {
        let mut x = b("héllo 世界 🙂");
        x.move_end();
        assert_eq!(x.line_col(), (1, 11));
        // h é l l o ␠ 世(2) 界(2) ␠ 🙂(2) => 6 + 2 + 2 + 1 + 2 = 13 cells
        assert_eq!(x.cursor_display_col(), 13);
        x.move_left();
        assert_eq!(x.line_col(), (1, 10));
        assert_eq!(x.cursor_display_col(), 11);
        x.move_left();
        x.move_left();
        assert_eq!(x.cursor_display_col(), 8);
        x.move_right();
        assert_eq!(x.cursor_display_col(), 10);
    }

    #[test]
    fn tabs_advance_to_next_multiple_of_4() {
        let mut x = b("a\tb");
        x.move_end();
        assert_eq!(x.cursor_display_col(), 5);
        assert_eq!(grapheme_width("\t", 0), 4);
        assert_eq!(grapheme_width("\t", 3), 1);
    }

    #[test]
    fn up_down_keep_remembered_column() {
        let mut x = b("abcdef\nab\nabcdef");
        x.move_to(5);
        x.move_down();
        assert_eq!(x.line_col(), (2, 3));
        x.move_down();
        assert_eq!(x.line_col(), (3, 6));
        x.move_up();
        x.move_up();
        assert_eq!(x.line_col(), (1, 6));
        x.move_up();
        assert_eq!(x.line_col(), (1, 6));
    }

    #[test]
    fn up_down_over_wide_chars() {
        let mut x = b("世界x\nabcde");
        x.move_to(2); // after 世界, display col 4
        x.move_down();
        assert_eq!(x.line_col(), (2, 5));
        x.move_left(); // col 3
        x.move_up();
        // col 3 falls inside 界 (cols 2-3); land before it
        assert_eq!(x.line_col(), (1, 2));
    }

    #[test]
    fn home_end_and_move_lines() {
        let mut x = b("one\ntwo\nthree\nfour");
        x.move_to(5);
        x.move_end();
        assert_eq!(x.line_col(), (2, 4));
        x.move_home();
        assert_eq!(x.line_col(), (2, 1));
        x.move_lines(10);
        assert_eq!(x.line_col(), (4, 1));
        x.move_lines(-10);
        assert_eq!(x.line_col(), (1, 1));
    }

    #[test]
    fn move_to_snaps_and_clamps() {
        let mut x = b("e\u{301}x");
        x.move_to(1);
        assert_eq!(x.cursor(), 0);
        x.move_to(99);
        assert_eq!(x.cursor(), 3);
        x.move_to_line_col(5, 1);
        assert_eq!(x.cursor(), 2);
    }

    #[test]
    fn apply_with_cursor_sets_cursor() {
        let mut x = b("abc");
        x.apply_with_cursor(
            Edit {
                at: 0,
                removed: "a".into(),
                inserted: String::new(),
            },
            GroupKind::Operator,
            1,
        );
        assert_eq!(x.cursor(), 1);
        x.undo();
        x.redo();
        assert_eq!(x.cursor(), 1);
    }
}
