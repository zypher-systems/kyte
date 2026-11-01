//! Reader mode: read-only, with everything outside the current section dimmed.
//! A section is a function in code Kyte can parse, otherwise a paragraph.

use std::ops::Range;

use crate::buffer::Buffer;
use crate::syntax;

/// Reader mode's state while it's open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    /// The lines in focus.
    pub focus: Range<usize>,
    /// Where the cursor is in the code, such as `impl Foo › fn bar`.
    pub crumbs: String,
    pub show_keys: bool,
}

fn blank(b: &Buffer, line: usize) -> bool {
    b.line_text(line).trim().is_empty()
}

/// The paragraph at `line`: the lines around it up to blank lines. On a blank
/// line it's the next paragraph, or the last one if none follows.
fn paragraph_at(b: &Buffer, line: usize) -> Range<usize> {
    let count = b.line_count();
    let mut at = line;
    while at < count && blank(b, at) {
        at += 1;
    }
    if at == count {
        at = line;
        while at > 0 && blank(b, at) {
            at -= 1;
        }
        if blank(b, at) {
            return line..line + 1;
        }
    }
    let mut start = at;
    while start > 0 && !blank(b, start - 1) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < count && !blank(b, end) {
        end += 1;
    }
    start..end
}

/// The file's path and text, when it's code small enough to parse on each key.
fn code(b: &Buffer) -> Option<(std::borrow::Cow<'_, std::path::Path>, String)> {
    let path = b.kind_path();
    let parsed =
        syntax::supported(&path, &b.head()) && b.rope().len_bytes() <= syntax::MAX_LIVE_PARSE_BYTES;
    parsed.then(|| (path, b.to_string()))
}

/// Whether a line belongs to the function below it: a comment or an attribute.
fn leads_in(text: &str, prefix: Option<&str>) -> bool {
    let t = text.trim_start();
    t.starts_with("#[") || t.starts_with('@') || prefix.is_some_and(|p| t.starts_with(p))
}

/// The lines to focus on at the cursor, and the breadcrumb.
pub fn section_at(b: &Buffer) -> (Range<usize>, String) {
    let line = b.cursor_line();
    let Some((path, text)) = code(b) else {
        return (paragraph_at(b, line), String::new());
    };
    let rope = b.rope();
    let at = rope.char_to_byte(b.cursor());
    let crumbs = syntax::breadcrumb(&path, &text, at).join(" › ");
    let Some(r) = syntax::function_at(&path, &text, at) else {
        return (paragraph_at(b, line), crumbs);
    };
    let mut start = rope.byte_to_line(r.start);
    let prefix = syntax::comment_prefix(&path, &b.head());
    while start > 0 && leads_in(&b.line_text(start - 1), prefix) {
        start -= 1;
    }
    (
        start..rope.byte_to_line(r.end.saturating_sub(1)) + 1,
        crumbs,
    )
}

/// The char index of the next or previous section's start.
pub fn step(b: &Buffer, forward: bool) -> Option<usize> {
    let rope = b.rope();
    if let Some((path, text)) = code(b) {
        let at = rope.char_to_byte(b.cursor());
        let hit = if forward {
            syntax::next_function(&path, &text, at)
        } else {
            syntax::prev_function(&path, &text, at)
        };
        if let Some(byte) = hit {
            return Some(rope.byte_to_char(byte));
        }
    }
    let here = paragraph_at(b, b.cursor_line());
    let target = if forward {
        (here.end..b.line_count()).find(|l| !blank(b, *l))?
    } else {
        let last = (0..here.start).rev().find(|l| !blank(b, *l))?;
        paragraph_at(b, last).start
    };
    Some(b.line_start(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, path: &str, cursor: usize) -> Buffer {
        let mut b = Buffer::from_str(text, Some(path.into()));
        b.move_to(cursor);
        b
    }

    #[test]
    fn paragraphs_stop_at_blank_lines() {
        let text = "one\ntwo\n\n\nthree\n";
        assert_eq!(section_at(&at(text, "/x/a.txt", 0)).0, 0..2);
        assert_eq!(section_at(&at(text, "/x/a.txt", 5)).0, 0..2);
        assert_eq!(
            section_at(&at(text, "/x/a.txt", 8)).0,
            4..5,
            "blank goes on"
        );
        assert_eq!(
            section_at(&at(text, "/x/a.txt", 15)).0,
            4..5,
            "the end goes back"
        );
        assert_eq!(section_at(&at("\n\n", "/x/a.txt", 0)).0, 0..1);
        assert_eq!(section_at(&at("", "/x/a.txt", 0)).0, 0..1);
    }

    #[test]
    fn stepping_through_paragraphs() {
        let text = "a\nb\n\nc\n\nd\n";
        let mut b = at(text, "/x/a.txt", 0);
        for want in [5, 8] {
            let to = step(&b, true).unwrap();
            assert_eq!(to, want);
            b.move_to(to);
        }
        assert_eq!(step(&b, true), None);
        assert_eq!(step(&b, false), Some(5));
        b.move_to(0);
        assert_eq!(step(&b, false), None);
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn a_function_is_a_section_with_its_lead_in() {
        let text = "use x;\n\n/// Doc.\n#[inline]\nfn f() {\n    a();\n}\n\nfn g() {}\n";
        let b = at(text, "/x/a.rs", text.find("a()").unwrap());
        let (focus, crumbs) = section_at(&b);
        assert_eq!(focus, 2..7);
        assert_eq!(crumbs, "fn f");
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn code_steps_by_function_then_falls_back_to_paragraphs() {
        let text = "fn f() {}\n\nfn g() {}\n\nconst X: u8 = 1;\n";
        let mut b = at(text, "/x/a.rs", 0);
        let to = step(&b, true).unwrap();
        assert_eq!(to, 11);
        b.move_to(to);
        let to = step(&b, true).unwrap();
        assert_eq!(to, 22, "past the last function, by paragraph");
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn large_code_reads_by_paragraph_without_parsing() {
        let text = "fn f() {}\n".repeat(60_000);
        let b = at(&text, "/x/a.rs", 0);
        let (focus, crumbs) = section_at(&b);
        assert_eq!(crumbs, "");
        assert_eq!(focus.len(), b.line_count() - 1);
    }
}
