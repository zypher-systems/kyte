//! Syntax highlighting (cargo feature `highlight`, on by default).
//!
//! tree-sitter with the Rust, Python and Bash grammars and their bundled
//! `highlights.scm` queries. `highlight` returns byte ranges of the whole text
//! that overlap the requested line range, each with a [`HighlightKind`]; the
//! text view maps the kinds to theme colors.

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Documents larger than this are not highlighted (every redraw re-parses).
pub const MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
    Bash,
}

impl Lang {
    /// Picks the grammar from the file extension.
    pub fn from_path(path: &Path) -> Option<Lang> {
        match path.extension()?.to_str()? {
            "rs" => Some(Lang::Rust),
            "py" | "pyi" => Some(Lang::Python),
            "sh" | "bash" => Some(Lang::Bash),
            _ => None,
        }
    }

    /// The tree-sitter grammar.
    pub fn language(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Constant,
    Number,
}

/// Capture names Kyte colors, in the order of [`HighlightKind`].
const NAMES: [&str; 7] = [
    "keyword", "string", "comment", "function", "type", "constant", "number",
];
const KINDS: [HighlightKind; 7] = [
    HighlightKind::Keyword,
    HighlightKind::String,
    HighlightKind::Comment,
    HighlightKind::Function,
    HighlightKind::Type,
    HighlightKind::Constant,
    HighlightKind::Number,
];

impl HighlightKind {
    /// The foreground for this kind on top of `base` (the text style).
    pub fn style(self, base: Style) -> Style {
        let fg = |n| base.fg(Color::Indexed(n));
        match self {
            HighlightKind::Keyword => fg(176).add_modifier(Modifier::BOLD),
            HighlightKind::String => fg(150),
            HighlightKind::Comment => fg(245).add_modifier(Modifier::ITALIC),
            HighlightKind::Function => fg(111),
            HighlightKind::Type => fg(180),
            HighlightKind::Constant | HighlightKind::Number => fg(209),
        }
    }
}

fn config(lang: Lang) -> Option<&'static HighlightConfiguration> {
    static RUST: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    static PYTHON: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    static BASH: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    let (cell, language, query) = match lang {
        Lang::Rust => (
            &RUST,
            tree_sitter_rust::LANGUAGE,
            tree_sitter_rust::HIGHLIGHTS_QUERY,
        ),
        Lang::Python => (
            &PYTHON,
            tree_sitter_python::LANGUAGE,
            tree_sitter_python::HIGHLIGHTS_QUERY,
        ),
        Lang::Bash => (
            &BASH,
            tree_sitter_bash::LANGUAGE,
            tree_sitter_bash::HIGHLIGHT_QUERY,
        ),
    };
    cell.get_or_init(|| {
        let mut c =
            HighlightConfiguration::new(language.into(), format!("{lang:?}"), query, "", "")
                .ok()?;
        c.configure(&NAMES);
        Some(c)
    })
    .as_ref()
}

/// Highlighted byte ranges of `text` that overlap lines `lines` (0-based,
/// end exclusive). Ranges never overlap each other; innermost captures win.
pub fn highlight(
    lang: Lang,
    text: &str,
    lines: Range<usize>,
) -> Vec<(Range<usize>, HighlightKind)> {
    let mut out = Vec::new();
    if text.len() > MAX_BYTES {
        return out;
    }
    let Some(cfg) = config(lang) else {
        return out;
    };
    // Byte window of the requested lines.
    let mut starts = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1));
    let lo = starts.nth(lines.start).unwrap_or(text.len());
    let hi = if lines.end > lines.start {
        starts
            .nth(lines.end - lines.start - 1)
            .unwrap_or(text.len())
    } else {
        lo
    };
    let mut hl = Highlighter::new();
    let Ok(events) = hl.highlight(cfg, text.as_bytes(), None, None, |_| None) else {
        return out;
    };
    let mut stack: Vec<HighlightKind> = Vec::new();
    for ev in events {
        match ev {
            Ok(HighlightEvent::HighlightStart(h)) => stack.push(KINDS[h.0]),
            Ok(HighlightEvent::HighlightEnd) => {
                stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                if end <= lo {
                    continue;
                }
                if start >= hi {
                    break;
                }
                if let Some(&k) = stack.last() {
                    out.push((start..end, k));
                }
            }
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_of<'a>(
        text: &'a str,
        spans: &[(Range<usize>, HighlightKind)],
    ) -> Vec<(&'a str, HighlightKind)> {
        spans.iter().map(|(r, k)| (&text[r.clone()], *k)).collect()
    }

    #[test]
    fn tokenizes_a_rust_snippet() {
        let src = "// note\nfn main() {\n    let s = \"hi\";\n}\n";
        let spans = highlight(Lang::Rust, src, 0..4);
        let k = kinds_of(src, &spans);
        assert!(k.contains(&("// note", HighlightKind::Comment)), "{k:?}");
        assert!(k.contains(&("fn", HighlightKind::Keyword)), "{k:?}");
        assert!(k.contains(&("let", HighlightKind::Keyword)), "{k:?}");
        assert!(k.contains(&("main", HighlightKind::Function)), "{k:?}");
        assert!(
            k.iter()
                .any(|(t, kind)| t.contains("hi") && *kind == HighlightKind::String),
            "{k:?}"
        );
    }

    #[test]
    fn only_requested_lines() {
        let src = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let spans = highlight(Lang::Rust, src, 1..2);
        assert!(!spans.is_empty());
        assert!(
            spans.iter().all(|(r, _)| r.start >= 10 && r.end <= 20),
            "{spans:?}"
        );
    }

    #[test]
    fn python_and_bash_grammars_load() {
        let py = "def f():\n    return 1\n";
        assert!(kinds_of(py, &highlight(Lang::Python, py, 0..2))
            .contains(&("def", HighlightKind::Keyword)));
        let sh = "if true; then echo hi; fi\n";
        assert!(kinds_of(sh, &highlight(Lang::Bash, sh, 0..1))
            .contains(&("if", HighlightKind::Keyword)));
    }

    #[test]
    fn languages_from_extension() {
        assert_eq!(Lang::from_path(Path::new("a/b.rs")), Some(Lang::Rust));
        assert_eq!(Lang::from_path(Path::new("x.py")), Some(Lang::Python));
        assert_eq!(Lang::from_path(Path::new("run.sh")), Some(Lang::Bash));
        assert_eq!(Lang::from_path(Path::new("notes.txt")), None);
        assert_eq!(Lang::from_path(Path::new("Makefile")), None);
    }
}
