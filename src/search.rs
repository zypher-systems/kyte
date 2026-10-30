//! Incremental, case-sensitive substring search over a rope, forward with wrap.

use ropey::Rope;

/// First match of `query` at or after char index `from`, wrapping to the start.
/// `None` for an empty query or no match.
pub fn find_from(rope: &Rope, query: &str, from: usize) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    let text = rope.to_string();
    let from = from.min(rope.len_chars());
    let from_byte = rope.char_to_byte(from);
    if let Some(i) = text[from_byte..].find(query) {
        return Some(rope.byte_to_char(from_byte + i));
    }
    text.find(query).map(|i| rope.byte_to_char(i))
}

/// First match strictly after the match at `current`, wrapping (Enter in the prompt).
pub fn find_next(rope: &Rope, query: &str, current: usize) -> Option<usize> {
    let len = rope.len_chars();
    let from = if current >= len { 0 } else { current + 1 };
    find_from(rope, query, from)
}

/// Whether `query` occurs exactly at char index `at`.
pub fn matches_at(rope: &Rope, query: &str, at: usize) -> bool {
    if query.is_empty() {
        return false;
    }
    let n = query.chars().count();
    at + n <= rope.len_chars() && rope.slice(at..at + n) == query
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
    fn wraps_from_the_end() {
        assert_eq!(find_from(&r("abc abc"), "abc", 5), Some(0));
    }

    #[test]
    fn empty_and_missing() {
        assert_eq!(find_from(&r("abc"), "", 0), None);
        assert_eq!(find_from(&r("abc"), "x", 0), None);
        assert_eq!(find_next(&r(""), "x", 0), None);
    }

    #[test]
    fn case_sensitive_and_unicode_char_indices() {
        let x = r("Ünï cödé CÖDÉ");
        assert_eq!(find_from(&x, "cödé", 0), Some(4));
        assert_eq!(find_from(&x, "CÖDÉ", 0), Some(9));
        assert_eq!(find_from(&x, "cöDé", 0), None);
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
    }
}
