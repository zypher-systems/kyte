//! Operator implementations: pure functions over a `&Rope` (section 6).

use std::ops::Range;

use ropey::Rope;

use crate::buffer::graphemes_with_char_offsets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Punct,
    Space,
}

fn class_of(g: &str) -> Class {
    let c = g.chars().next().unwrap_or(' ');
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// Calls `f(char_index, grapheme)` for each grapheme starting at or after
/// `from`, across lines (newlines included), until `f` returns false.
fn for_each_grapheme_from(rope: &Rope, from: usize, mut f: impl FnMut(usize, &str) -> bool) {
    if from >= rope.len_chars() {
        return;
    }
    let mut line = rope.char_to_line(from);
    while line < rope.len_lines() {
        let start = rope.line_to_char(line);
        let text = rope.line(line).to_string();
        for (off, g) in graphemes_with_char_offsets(&text) {
            if start + off < from {
                continue;
            }
            if !f(start + off, g) {
                return;
            }
        }
        line += 1;
    }
}

/// End of the `w` deletion: skip the rest of the current class run (word
/// chars, punctuation, or whitespace), then any following whitespace.
/// Returns `from` at the end of the buffer.
pub fn word_end(rope: &Rope, from: usize) -> usize {
    let mut first: Option<Class> = None;
    let mut in_trailing_space = false;
    let mut end = from;
    for_each_grapheme_from(rope, from, |at, g| {
        let c = class_of(g);
        let next = at + g.chars().count();
        match first {
            None => {
                first = Some(c);
                end = next;
                true
            }
            Some(f) => {
                if !in_trailing_space && c == f {
                    end = next;
                    true
                } else if f != Class::Space && c == Class::Space {
                    in_trailing_space = true;
                    end = next;
                    true
                } else {
                    false
                }
            }
        }
    });
    end
}

/// Range deleted by `d`: the whole line at `at` including its newline. On a
/// last line without a trailing newline the preceding newline goes too
/// (unless it is the only line, which just becomes empty).
pub fn line_range(rope: &Rope, at: usize) -> Range<usize> {
    let len = rope.len_chars();
    let l = rope.char_to_line(at.min(len));
    let mut start = rope.line_to_char(l);
    let end = if l + 1 < rope.len_lines() {
        rope.line_to_char(l + 1)
    } else {
        len
    };
    let has_newline = end > start && rope.char(end - 1) == '\n';
    if !has_newline && l > 0 {
        start -= 1;
        if start > 0 && rope.char(start - 1) == '\r' {
            start -= 1;
        }
    }
    start..end
}

/// End of the line at `at`, before its LF or CRLF (for `k`).
pub fn eol(rope: &Rope, at: usize) -> usize {
    let len = rope.len_chars();
    let l = rope.char_to_line(at.min(len));
    let start = rope.line_to_char(l);
    let line = rope.line(l);
    let mut n = line.len_chars();
    if n > 0 && line.char(n - 1) == '\n' {
        n -= 1;
        if n > 0 && line.char(n - 1) == '\r' {
            n -= 1;
        }
    }
    start + n
}

/// Range deleted by `k`: cursor to end of line, or the newline itself when
/// already at the end of a line. Empty at the end of the buffer.
pub fn kill_range(rope: &Rope, at: usize) -> Range<usize> {
    let e = eol(rope, at);
    if at < e {
        at..e
    } else if at < rope.len_chars() {
        let crlf = rope.char(at) == '\r' && rope.get_char(at + 1) == Some('\n');
        at..at + if crlf { 2 } else { 1 }
    } else {
        at..at
    }
}

fn partner(c: char) -> Option<(char, char, bool)> {
    match c {
        '(' => Some(('(', ')', true)),
        '[' => Some(('[', ']', true)),
        '{' => Some(('{', '}', true)),
        ')' => Some(('(', ')', false)),
        ']' => Some(('[', ']', false)),
        '}' => Some(('{', '}', false)),
        _ => None,
    }
}

fn find_partner(rope: &Rope, i: usize) -> Option<usize> {
    let (open, close, forward) = partner(rope.char(i))?;
    let mut depth: i64 = 0;
    if forward {
        for (j, c) in rope.chars_at(i).enumerate() {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i + j);
                }
            }
        }
    } else {
        let mut j = i + 1;
        while j > 0 {
            j -= 1;
            let c = rope.char(j);
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
        }
    }
    None
}

/// Target of `%`: the partner of the bracket under the cursor, counting
/// nesting; when not on a bracket, the partner of the first bracket after the
/// cursor on the same line. No string or comment awareness. `None` when
/// unmatched or no bracket.
pub fn match_bracket(rope: &Rope, at: usize) -> Option<usize> {
    let len = rope.len_chars();
    if at >= len {
        return None;
    }
    if partner(rope.char(at)).is_some() {
        return find_partner(rope, at);
    }
    let end = eol(rope, at);
    (at..end)
        .find(|&j| partner(rope.char(j)).is_some())
        .and_then(|j| find_partner(rope, j))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn crlf_is_kept_whole_by_the_operators() {
        // k stops before the line ending, then joins by removing all of it
        let x = r("ab\r\ncd\r\n");
        assert_eq!(eol(&x, 0), 2);
        assert_eq!(kill_range(&x, 0), 0..2);
        assert_eq!(kill_range(&x, 2), 2..4);
        // d takes the line with its ending
        assert_eq!(line_range(&x, 0), 0..4);
        // on a last line with no ending, the previous line's ending goes
        let y = r("ab\r\ncd");
        assert_eq!(line_range(&y, 5), 2..6);
        // w treats the ending as one piece of whitespace
        let z = r("ab\r\n  cd");
        assert_eq!(word_end(&z, 2), 6);
    }

    fn del(s: &str, range: Range<usize>) -> String {
        let mut x = r(s);
        x.remove(range);
        x.to_string()
    }

    // ----- w -----

    #[test]
    fn w_on_space_after_word_deletes_whitespace_only() {
        let s = "foo bar baz";
        assert_eq!(word_end(&r(s), 3), 4);
        assert_eq!(del(s, 3..4), "foobar baz");
    }

    #[test]
    fn w_at_word_start_deletes_word_and_following_space() {
        let s = "foo bar";
        assert_eq!(word_end(&r(s), 0), 4);
        assert_eq!(del(s, 0..4), "bar");
    }

    #[test]
    fn w_mid_word_and_punctuation_classes() {
        assert_eq!(word_end(&r("foobar baz"), 3), 7);
        assert_eq!(word_end(&r("a.b"), 0), 1);
        assert_eq!(word_end(&r("..b c"), 0), 2);
        assert_eq!(word_end(&r("snake_case x"), 0), 11);
        assert_eq!(word_end(&r("foo   \n  bar"), 0), 9);
    }

    #[test]
    fn w_at_end_of_buffer_does_nothing() {
        assert_eq!(word_end(&r("abc"), 3), 3);
        assert_eq!(word_end(&r(""), 0), 0);
    }

    #[test]
    fn w_keeps_graphemes_whole() {
        // é as e + combining acute belongs to the word
        assert_eq!(word_end(&r("e\u{301}tude x"), 0), 7);
    }

    // ----- d -----

    #[test]
    fn d_middle_line() {
        let s = "a\nbb\nc";
        assert_eq!(line_range(&r(s), 3), 2..5);
        assert_eq!(del(s, 2..5), "a\nc");
    }

    #[test]
    fn d_last_line_without_newline_takes_preceding_newline() {
        let s = "a\nb\nc";
        assert_eq!(line_range(&r(s), 4), 3..5);
        assert_eq!(del(s, 3..5), "a\nb");
    }

    #[test]
    fn d_only_line_becomes_empty() {
        assert_eq!(line_range(&r("abc"), 1), 0..3);
        assert_eq!(line_range(&r(""), 0), 0..0);
    }

    #[test]
    fn d_first_line_and_line_with_newline() {
        assert_eq!(del("a\nb\n", line_range(&r("a\nb\n"), 0)), "b\n");
        assert_eq!(del("a\nb\n", line_range(&r("a\nb\n"), 2)), "a\n");
    }

    // ----- k -----

    #[test]
    fn k_mid_line_deletes_to_eol() {
        let s = "abcd\nef";
        assert_eq!(kill_range(&r(s), 2), 2..4);
        assert_eq!(del(s, 2..4), "ab\nef");
    }

    #[test]
    fn k_at_eol_joins_lines() {
        let s = "ab\nef";
        assert_eq!(kill_range(&r(s), 2), 2..3);
        assert_eq!(del(s, 2..3), "abef");
    }

    #[test]
    fn k_at_end_of_buffer_is_empty() {
        assert_eq!(kill_range(&r("ab"), 2), 2..2);
        assert_eq!(eol(&r("ab\ncd"), 0), 2);
        assert_eq!(eol(&r("ab\ncd"), 4), 5);
    }

    // ----- % -----

    #[test]
    fn percent_nested_brackets() {
        let s = "(a[b]c)";
        assert_eq!(match_bracket(&r(s), 0), Some(6));
        assert_eq!(match_bracket(&r(s), 6), Some(0));
        assert_eq!(match_bracket(&r(s), 2), Some(4));
        assert_eq!(match_bracket(&r(s), 4), Some(2));
        assert_eq!(match_bracket(&r("{ ( ) { } }"), 0), Some(10));
    }

    #[test]
    fn percent_not_on_bracket_searches_forward_on_line() {
        assert_eq!(match_bracket(&r("x(a[b]c)y"), 0), Some(7));
        assert_eq!(match_bracket(&r("fn f(x) {\n}"), 0), Some(6));
        // only the current line is searched
        assert_eq!(match_bracket(&r("abc\n(x)"), 0), None);
    }

    #[test]
    fn percent_unmatched_or_none() {
        assert_eq!(match_bracket(&r("(ab"), 0), None);
        assert_eq!(match_bracket(&r("ab)"), 2), None);
        assert_eq!(match_bracket(&r("abc"), 0), None);
        assert_eq!(match_bracket(&r("abc"), 3), None);
    }

    #[test]
    fn percent_across_lines() {
        assert_eq!(match_bracket(&r("{\n  x\n}"), 0), Some(6));
        assert_eq!(match_bracket(&r("{\n  x\n}"), 6), Some(0));
    }
}
