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

/// Graphemes of `s` as (char offset, grapheme).
pub fn graphemes_with_char_offsets(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut chars = 0;
    for g in s.graphemes(true) {
        out.push((chars, g));
        chars += g.chars().count();
    }
    out
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
        }
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
        let off = self.cursor - start;
        let col = graphemes_with_char_offsets(&text)
            .iter()
            .take_while(|(o, _)| *o < off)
            .count();
        (line + 1, col + 1)
    }

    /// Text of line `l` without its trailing `\n` (a `\r` stays an ordinary character).
    pub fn line_text(&self, l: usize) -> String {
        let mut s = self.rope.line(l).to_string();
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }

    pub fn line_start(&self, l: usize) -> usize {
        self.rope.line_to_char(l)
    }

    /// Char index of the end of line `l` (before its newline).
    pub fn line_end(&self, l: usize) -> usize {
        self.line_start(l) + self.line_text(l).chars().count()
    }

    /// Display column of the cursor within its line.
    pub fn cursor_display_col(&self) -> usize {
        let line = self.cursor_line();
        self.display_col(line, self.cursor - self.line_start(line))
    }

    /// Display column of char offset `off` within line `l`.
    pub fn display_col(&self, l: usize, off: usize) -> usize {
        let text = self.line_text(l);
        let mut col = 0;
        for (o, g) in graphemes_with_char_offsets(&text) {
            if o >= off {
                break;
            }
            col += grapheme_width(g, col);
        }
        col
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
        self.raw_apply(&edit);
        self.cursor = cursor_after.min(self.rope.len_chars());
        self.dirty = true;
        self.redo.clear();
        self.pending_edits.push(edit.clone());
        let simple_typing =
            kind == GroupKind::Typing && edit.removed.is_empty() && !edit.inserted.contains('\n');
        let coalesce = simple_typing
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

    pub fn insert_str(&mut self, s: &str, kind: GroupKind) {
        if s.is_empty() {
            return;
        }
        self.apply(Edit::insert(self.cursor, s), kind);
    }

    /// Deletes the previous grapheme (joins with the previous line at a line start).
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
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
        if self.cursor >= self.rope.len_chars() {
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
        g.open = false;
        for e in g.edits.iter().rev() {
            let inv = e.inverse();
            self.raw_apply(&inv);
            self.pending_edits.push(inv);
        }
        self.cursor = g.cursor_before.min(self.rope.len_chars());
        self.want_col = self.cursor_display_col();
        self.dirty = true;
        self.redo.push(g);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(g) = self.redo.pop() else {
            return false;
        };
        for e in &g.edits {
            self.raw_apply(e);
            self.pending_edits.push(e.clone());
        }
        self.cursor = g.cursor_after.min(self.rope.len_chars());
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

    /// Previous grapheme boundary before `at` (crosses a newline as one step).
    pub fn prev_boundary(&self, at: usize) -> usize {
        if at == 0 {
            return 0;
        }
        let line = self.rope.char_to_line(at);
        let start = self.line_start(line);
        if at == start {
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

    /// Next grapheme boundary after `at` (crosses a newline as one step).
    pub fn next_boundary(&self, at: usize) -> usize {
        let len = self.rope.len_chars();
        if at >= len {
            return len;
        }
        let line = self.rope.char_to_line(at);
        let start = self.line_start(line);
        let text = self.line_text(line);
        let off = at - start;
        let line_len = text.chars().count();
        if off >= line_len {
            return (at + 1).min(len);
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
