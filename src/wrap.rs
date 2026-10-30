//! Soft wrap: splits a line into the rows it takes on screen, at spaces where
//! it can. Nothing is stored; the file's text and line numbers don't change.

use std::ops::Range;

use crate::buffer::{grapheme_width, graphemes_with_char_offsets, measure};

/// One row of a wrapped line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The row's chars, as offsets into the line.
    pub chars: Range<usize>,
    /// The same text as byte offsets.
    pub bytes: Range<usize>,
}

/// Display width of `text` drawn from the start of a row.
fn width_of(text: &str) -> usize {
    measure(text, usize::MAX).0
}

/// The rows of `line` at `width` columns. There's always at least one, and a
/// row that fills the width exactly is followed by an empty one, so the cursor
/// has somewhere to sit at the end of the line. A grapheme wider than the
/// width gets a row of its own.
pub fn segments(line: &str, width: usize) -> Vec<Segment> {
    let width = width.max(1);
    let mut out = Vec::new();
    let (mut start_char, mut start_byte) = (0, 0);
    let mut col = 0;
    // Just after the last space on this row: (char, byte).
    let mut gap = None;
    let mut byte = 0;
    for (off, g) in graphemes_with_char_offsets(line) {
        // A break at a space can still leave this grapheme too wide for the
        // new row, so keep breaking until it fits or starts a row.
        while col + grapheme_width(g, col) > width && off > start_char {
            let (cut_char, cut_byte) = gap.unwrap_or((off, byte));
            out.push(Segment {
                chars: start_char..cut_char,
                bytes: start_byte..cut_byte,
            });
            (start_char, start_byte) = (cut_char, cut_byte);
            gap = None;
            col = width_of(&line[start_byte..byte]);
        }
        col += grapheme_width(g, col);
        byte += g.len();
        if g == " " || g == "\t" {
            gap = Some((off + 1, byte));
        }
    }
    let end_char = start_char + line[start_byte..].chars().count();
    out.push(Segment {
        chars: start_char..end_char,
        bytes: start_byte..line.len(),
    });
    if col >= width && !line.is_empty() {
        out.push(Segment {
            chars: end_char..end_char,
            bytes: line.len()..line.len(),
        });
    }
    out
}

/// Which row char offset `off` is on. A row's end belongs to the next row,
/// except at the end of the line.
pub fn row_of(segments: &[Segment], off: usize) -> usize {
    segments
        .iter()
        .position(|s| off < s.chars.end)
        .unwrap_or(segments.len().saturating_sub(1))
}

/// The char offset in `text` nearest to column `col`, not going past it.
pub fn offset_at_col(text: &str, col: usize) -> usize {
    let mut c = 0;
    for (off, g) in graphemes_with_char_offsets(text) {
        let w = grapheme_width(g, c);
        if c + w > col {
            return off;
        }
        c += w;
    }
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rows(line: &str, width: usize) -> Vec<&str> {
        segments(line, width)
            .into_iter()
            .map(|s| &line[s.bytes])
            .collect()
    }

    #[test]
    fn short_and_empty_lines_are_one_row() {
        assert_eq!(rows("hello", 10), vec!["hello"]);
        assert_eq!(rows("", 10), vec![""]);
    }

    #[test]
    fn it_breaks_after_spaces_when_it_can() {
        assert_eq!(rows("hello big world", 10), vec!["hello big ", "world"]);
    }

    #[test]
    fn a_long_word_breaks_where_it_has_to() {
        assert_eq!(rows("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_full_last_row_leaves_room_for_the_cursor() {
        assert_eq!(rows("abcd", 4), vec!["abcd", ""]);
        assert_eq!(rows("abcdefgh", 4), vec!["abcd", "efgh", ""]);
    }

    #[test]
    fn wide_characters_stay_whole() {
        assert_eq!(rows("世界世界", 5), vec!["世界", "世界"]);
        assert_eq!(rows("世", 1), vec!["世", ""]);
    }

    #[test]
    fn tabs_count_from_the_start_of_their_row() {
        assert_eq!(rows("ab\tcd", 5), vec!["ab\t", "cd"]);
    }

    #[test]
    fn row_ends_belong_to_the_next_row() {
        let s = segments("abcdefgh", 4);
        assert_eq!(row_of(&s, 0), 0);
        assert_eq!(row_of(&s, 3), 0);
        assert_eq!(row_of(&s, 4), 1);
        assert_eq!(row_of(&s, 8), 2);
        let t = segments("abc", 10);
        assert_eq!(row_of(&t, 3), 0, "the end of the line stays on its row");
    }

    #[test]
    fn offsets_from_columns() {
        assert_eq!(offset_at_col("abc", 1), 1);
        assert_eq!(offset_at_col("abc", 9), 3);
        assert_eq!(
            offset_at_col("世界", 3),
            1,
            "not into the middle of a wide char"
        );
    }

    const PIECES: &[&str] = &[
        "a", "b", " ", "\t", "é", "e\u{301}", "世", "🙂", "\r", "word ", "  ",
    ];

    proptest! {
        #[test]
        fn rows_cover_the_line_in_order_and_fit(
            parts in proptest::collection::vec(proptest::sample::select(PIECES), 0..60),
            width in 1usize..24,
        ) {
            let line = parts.concat();
            let segs = segments(&line, width);
            let mut chars = 0;
            let mut bytes = 0;
            let mut joined = String::new();
            for s in &segs {
                prop_assert_eq!(s.chars.start, chars);
                prop_assert_eq!(s.bytes.start, bytes);
                chars = s.chars.end;
                bytes = s.bytes.end;
                let text = &line[s.bytes.clone()];
                prop_assert_eq!(text.chars().count(), s.chars.len());
                joined.push_str(text);
                // Only a single grapheme may be wider than the row.
                let graphemes = graphemes_with_char_offsets(text).count();
                prop_assert!(width_of(text) <= width || graphemes == 1);
            }
            prop_assert_eq!(joined, line.clone());
            prop_assert_eq!(chars, line.chars().count());
        }

        #[test]
        fn every_offset_has_a_row(
            parts in proptest::collection::vec(proptest::sample::select(PIECES), 0..40),
            width in 1usize..24,
        ) {
            let line = parts.concat();
            let segs = segments(&line, width);
            for off in 0..=line.chars().count() {
                let row = row_of(&segs, off);
                prop_assert!(segs[row].chars.contains(&off) || off == segs[row].chars.end);
            }
        }
    }
}
