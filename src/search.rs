//! Incremental search over a rope, forward and backward with wrap. Smart case:
//! a query with no capitals matches any case, and any capital makes it exact.

use std::ops::Range;

use ropey::Rope;

/// One-to-one case folding, so a match keeps its length in chars.
fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn fold(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

fn ignores_case(query: &str) -> bool {
    !query.chars().any(char::is_uppercase)
}

/// The text in the form a query is matched against, ready for repeated use.
struct Prepared {
    text: String,
    query: String,
    /// Byte offsets equal char offsets.
    ascii: bool,
}

impl Prepared {
    fn new(rope: &Rope, query: &str) -> Prepared {
        let ascii = rope.len_bytes() == rope.len_chars();
        let mut text = rope.to_string();
        let mut needle = query.to_string();
        if ignores_case(query) {
            needle = fold(query);
            if ascii {
                text.make_ascii_lowercase();
            } else {
                text = fold(&text);
            }
        }
        Prepared {
            text,
            query: needle,
            ascii,
        }
    }

    fn byte_of(&self, char_idx: usize) -> usize {
        if self.ascii {
            return char_idx.min(self.text.len());
        }
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn char_of(&self, byte: usize) -> usize {
        if self.ascii {
            byte
        } else {
            self.text[..byte].chars().count()
        }
    }

    /// First match at or after char index `from`.
    fn forward(&self, from: usize) -> Option<usize> {
        let start = self.byte_of(from);
        let i = self.text[start..].find(&self.query)?;
        Some(from + self.char_of_range(start, start + i))
    }

    fn char_of_range(&self, from: usize, to: usize) -> usize {
        if self.ascii {
            to - from
        } else {
            self.text[from..to].chars().count()
        }
    }

    /// Last match starting before char index `before`.
    fn backward(&self, before: usize) -> Option<usize> {
        let end = self.byte_of(before);
        // One byte short of fitting a match that starts at `end` itself.
        let mut cut = (end + self.query.len() - 1).min(self.text.len());
        while !self.text.is_char_boundary(cut) {
            cut -= 1;
        }
        self.text[..cut].rfind(&self.query).map(|i| self.char_of(i))
    }

    /// Matches inside a range of char indices, without overlaps.
    fn within(&self, range: Range<usize>) -> Vec<usize> {
        let (start, end) = (self.byte_of(range.start), self.byte_of(range.end));
        let mut out = Vec::new();
        let (mut byte, mut chars) = (start, range.start);
        for (i, _) in self.text[start..end].match_indices(&self.query) {
            chars += self.char_of_range(byte, start + i);
            byte = start + i;
            out.push(chars);
        }
        out
    }
}

/// First match of `query` at or after char index `from`, wrapping to the start.
/// `None` for an empty query or no match.
pub fn find_from(rope: &Rope, query: &str, from: usize) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    let p = Prepared::new(rope, query);
    p.forward(from.min(rope.len_chars()))
        .or_else(|| p.forward(0))
}

/// First match at or after `from`, without wrapping.
pub fn find_forward(rope: &Rope, query: &str, from: usize) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    Prepared::new(rope, query).forward(from.min(rope.len_chars()))
}

/// First match strictly after the match at `current`, wrapping (Enter in the prompt).
pub fn find_next(rope: &Rope, query: &str, current: usize) -> Option<usize> {
    let len = rope.len_chars();
    let from = if current >= len { 0 } else { current + 1 };
    find_from(rope, query, from)
}

/// Last match strictly before `current`, wrapping to the end.
pub fn find_prev(rope: &Rope, query: &str, current: usize) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    let p = Prepared::new(rope, query);
    p.backward(current).or_else(|| p.backward(rope.len_chars()))
}

/// Every match lying wholly inside `range`, without overlaps, as char indices.
pub fn find_within(rope: &Rope, query: &str, range: Range<usize>) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    Prepared::new(rope, query).within(range)
}

/// Whether `query` occurs exactly at char index `at`.
pub fn matches_at(rope: &Rope, query: &str, at: usize) -> bool {
    if query.is_empty() {
        return false;
    }
    let n = query.chars().count();
    if at + n > rope.len_chars() {
        return false;
    }
    let slice = rope.slice(at..at + n);
    if ignores_case(query) {
        slice
            .chars()
            .map(fold_char)
            .eq(query.chars().map(fold_char))
    } else {
        slice == query
    }
}

/// Matches of `query` within one line, as char ranges (for highlighting).
pub fn line_matches(line: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let n = query.chars().count();
    let (text, needle);
    let (hay, q): (&str, &str) = if ignores_case(query) {
        text = fold(line);
        needle = fold(query);
        (&text, &needle)
    } else {
        (line, query)
    };
    let mut out = Vec::new();
    let (mut byte, mut chars) = (0, 0);
    for (i, _) in hay.match_indices(q) {
        chars += hay[byte..i].chars().count();
        byte = i;
        out.push(chars..chars + n);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn finds_at_or_after_origin() {
        let x = r("foo bar baz");
        assert_eq!(find_from(&x, "ba", 0), Some(4));
        assert_eq!(find_from(&x, "ba", 4), Some(4));
        assert_eq!(find_from(&x, "ba", 5), Some(8));
    }

    #[test]
    fn find_next_wraps() {
        let x = r("foo bar baz");
        assert_eq!(find_next(&x, "ba", 4), Some(8));
        assert_eq!(find_next(&x, "ba", 8), Some(4));
        assert_eq!(find_next(&x, "foo", 0), Some(0));
        assert_eq!(find_next(&x, "ba", 11), Some(4));
    }

    #[test]
    fn find_prev_wraps() {
        let x = r("foo bar baz");
        assert_eq!(find_prev(&x, "ba", 8), Some(4));
        assert_eq!(find_prev(&x, "ba", 4), Some(8));
        assert_eq!(find_prev(&x, "foo", 0), Some(0));
        assert_eq!(find_prev(&x, "zz", 3), None);
        assert_eq!(find_prev(&r("世界 世界"), "世", 3), Some(0));
        assert_eq!(find_prev(&r("aXa"), "x", 2), Some(1), "folded");
    }

    #[test]
    fn wraps_from_the_end() {
        assert_eq!(find_from(&r("abc abc"), "abc", 5), Some(0));
    }

    #[test]
    fn empty_and_missing() {
        assert_eq!(find_from(&r("abc"), "", 0), None);
        assert_eq!(find_from(&r("abc"), "x", 0), None);
        assert_eq!(find_next(&r(""), "x", 0), None);
        assert_eq!(find_forward(&r("abc"), "", 0), None);
        assert!(find_within(&r("abc"), "", 0..3).is_empty());
    }

    #[test]
    fn smart_case() {
        let x = r("Hello hello HELLO");
        assert_eq!(find_from(&x, "hello", 1), Some(6), "lowercase matches any");
        assert_eq!(find_from(&x, "hello", 7), Some(12));
        assert_eq!(find_from(&x, "Hello", 1), Some(0), "a capital is exact");
        assert_eq!(find_within(&x, "hello", 0..17), vec![0, 6, 12]);
        assert_eq!(find_within(&x, "HELLO", 0..17), vec![12]);
    }

    #[test]
    fn exact_case_with_unicode_char_indices() {
        let x = r("Ünï cödé CÖDÉ");
        assert_eq!(find_from(&x, "cödé", 0), Some(4));
        assert_eq!(find_from(&x, "cödé", 5), Some(9), "folded across non-ASCII");
        assert_eq!(find_from(&x, "CÖDÉ", 0), Some(9));
        assert_eq!(find_from(&x, "cöDé", 0), None);
    }

    #[test]
    fn folding_that_changes_byte_length_keeps_char_indices() {
        // U+212A KELVIN SIGN folds to a shorter k, and İ to a plain i.
        let x = r("\u{212a}\u{212a}x İy");
        assert_eq!(find_from(&x, "x", 0), Some(2));
        assert_eq!(find_within(&x, "y", 0..6), vec![5]);
        assert_eq!(find_from(&x, "k", 1), Some(1));
        assert!(matches_at(&x, "kk", 0));
    }

    #[test]
    fn find_within_takes_only_whole_matches_inside_the_range() {
        let x = r("aaaa");
        assert_eq!(find_within(&x, "aa", 1..4), vec![1]);
        assert_eq!(find_within(&x, "aa", 0..1), Vec::<usize>::new());
        assert_eq!(find_within(&x, "aa", 0..4), vec![0, 2]);
        assert_eq!(
            find_within(&r("世x世x"), "x", 2..4),
            vec![3],
            "char indices"
        );
        assert_eq!(find_within(&x, "a", 2..2), Vec::<usize>::new());
    }

    #[test]
    fn across_lines() {
        assert_eq!(find_from(&r("ab\ncd"), "b\nc", 0), Some(1));
    }

    #[test]
    fn matches_at_position() {
        let x = r("foo bar");
        assert!(matches_at(&x, "bar", 4));
        assert!(!matches_at(&x, "bar", 3));
        assert!(!matches_at(&x, "bar", 6));
        assert!(!matches_at(&x, "", 0));
        assert!(matches_at(&r("FOO"), "foo", 0));
        assert!(!matches_at(&r("foo"), "Foo", 0));
    }

    #[test]
    fn line_matches_are_char_ranges_without_overlap() {
        assert_eq!(line_matches("aXaXa", "x"), vec![1..2, 3..4]);
        assert_eq!(line_matches("世a世A", "a"), vec![1..2, 3..4]);
        assert_eq!(line_matches("aaaa", "aa"), vec![0..2, 2..4]);
        assert!(line_matches("abc", "").is_empty());
        assert!(line_matches("abc", "A").is_empty());
    }
}
