//! Syntax highlighting (cargo feature `highlight`, on by default).
//!
//! tree-sitter with a grammar per language and each grammar's bundled
//! `highlights.scm` query. `highlight` returns byte ranges of the whole text
//! that overlap the requested line range, each with a [`HighlightKind`]; the
//! text view maps the kinds to theme colors.

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::Style;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::ui::theme::Theme;

/// Documents larger than this are not highlighted (every redraw re-parses).
pub const MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
    Bash,
    JavaScript,
    TypeScript,
    Tsx,
    Json,
    C,
    Cpp,
    Go,
    Html,
    Css,
    Toml,
    Yaml,
    Markdown,
}

impl Lang {
    /// Every language, in the order of the `CONFIGS` table.
    pub const ALL: [Lang; 15] = [
        Lang::Rust,
        Lang::Python,
        Lang::Bash,
        Lang::JavaScript,
        Lang::TypeScript,
        Lang::Tsx,
        Lang::Json,
        Lang::C,
        Lang::Cpp,
        Lang::Go,
        Lang::Html,
        Lang::Css,
        Lang::Toml,
        Lang::Yaml,
        Lang::Markdown,
    ];

    /// Picks the grammar from the file's name, or failing that the `#!` line
    /// at the start of `head`.
    pub fn detect(path: &Path, head: &str) -> Option<Lang> {
        Lang::from_path(path).or_else(|| Lang::from_shebang(head))
    }

    fn from_shebang(head: &str) -> Option<Lang> {
        match crate::syntax::shebang_program(head)? {
            "sh" | "bash" | "zsh" | "dash" | "ksh" => Some(Lang::Bash),
            "node" | "nodejs" | "deno" | "bun" => Some(Lang::JavaScript),
            name if name.starts_with("python") => Some(Lang::Python),
            _ => None,
        }
    }

    /// Picks the grammar from the file name, then its extension.
    pub fn from_path(path: &Path) -> Option<Lang> {
        match path.file_name()?.to_str()? {
            ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" | "PKGBUILD" => {
                return Some(Lang::Bash)
            }
            "Cargo.lock" | "uv.lock" | "poetry.lock" => return Some(Lang::Toml),
            _ => {}
        }
        Some(match path.extension()?.to_str()? {
            "rs" => Lang::Rust,
            "py" | "pyi" => Lang::Python,
            "sh" | "bash" | "zsh" => Lang::Bash,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "json" | "jsonc" => Lang::Json,
            "c" | "h" => Lang::C,
            "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Lang::Cpp,
            "go" => Lang::Go,
            "html" | "htm" => Lang::Html,
            "css" => Lang::Css,
            "toml" => Lang::Toml,
            "yaml" | "yml" => Lang::Yaml,
            "md" | "markdown" => Lang::Markdown,
            _ => return None,
        })
    }

    /// The tree-sitter grammar.
    pub fn language(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
            Lang::C => tree_sitter_c::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Html => tree_sitter_html::LANGUAGE.into(),
            Lang::Css => tree_sitter_css::LANGUAGE.into(),
            Lang::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Lang::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }

    /// The highlight query. TypeScript builds on JavaScript's, and C++ on C's.
    fn highlights(self) -> String {
        let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
        let jsx = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
        let ts = tree_sitter_typescript::HIGHLIGHTS_QUERY;
        match self {
            Lang::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
            Lang::Python => tree_sitter_python::HIGHLIGHTS_QUERY.to_string(),
            Lang::Bash => tree_sitter_bash::HIGHLIGHT_QUERY.to_string(),
            Lang::JavaScript => format!("{js}\n{jsx}"),
            Lang::TypeScript => format!("{ts}\n{js}"),
            Lang::Tsx => format!("{ts}\n{js}\n{jsx}"),
            Lang::Json => tree_sitter_json::HIGHLIGHTS_QUERY.to_string(),
            Lang::C => tree_sitter_c::HIGHLIGHT_QUERY.to_string(),
            Lang::Cpp => format!(
                "{}\n{}",
                tree_sitter_cpp::HIGHLIGHT_QUERY,
                tree_sitter_c::HIGHLIGHT_QUERY
            ),
            Lang::Go => tree_sitter_go::HIGHLIGHTS_QUERY.to_string(),
            Lang::Html => tree_sitter_html::HIGHLIGHTS_QUERY.to_string(),
            Lang::Css => tree_sitter_css::HIGHLIGHTS_QUERY.to_string(),
            Lang::Toml => tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_string(),
            Lang::Yaml => tree_sitter_yaml::HIGHLIGHTS_QUERY.to_string(),
            Lang::Markdown => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_string(),
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
const NAMES: [&str; 13] = [
    "keyword",
    "string",
    "comment",
    "function",
    "type",
    "constant",
    "number",
    // Names the markup and data grammars use, mapped onto the same colors.
    "tag",
    "attribute",
    "property",
    "text.title",
    "text.literal",
    "text.uri",
];
const KINDS: [HighlightKind; 13] = [
    HighlightKind::Keyword,
    HighlightKind::String,
    HighlightKind::Comment,
    HighlightKind::Function,
    HighlightKind::Type,
    HighlightKind::Constant,
    HighlightKind::Number,
    HighlightKind::Keyword,
    HighlightKind::Type,
    HighlightKind::Function,
    HighlightKind::Keyword,
    HighlightKind::String,
    HighlightKind::Constant,
];

impl HighlightKind {
    /// This kind's look in `theme`.
    pub fn style(self, theme: &Theme) -> Style {
        let s = &theme.syntax;
        theme.text.patch(match self {
            HighlightKind::Keyword => s.keyword,
            HighlightKind::String => s.string,
            HighlightKind::Comment => s.comment,
            HighlightKind::Function => s.function,
            HighlightKind::Type => s.kind,
            HighlightKind::Constant | HighlightKind::Number => s.constant,
        })
    }
}

fn config(lang: Lang) -> Option<&'static HighlightConfiguration> {
    static CONFIGS: [OnceLock<Option<HighlightConfiguration>>; Lang::ALL.len()] =
        [const { OnceLock::new() }; Lang::ALL.len()];
    CONFIGS[lang as usize]
        .get_or_init(|| {
            let name = format!("{lang:?}");
            let mut c =
                HighlightConfiguration::new(lang.language(), name, &lang.highlights(), "", "")
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
    fn every_grammar_loads_and_its_query_compiles() {
        for (i, lang) in Lang::ALL.iter().enumerate() {
            assert_eq!(*lang as usize, i, "CONFIGS is indexed by this order");
            assert!(config(*lang).is_some(), "{lang:?}");
        }
    }

    #[test]
    fn each_language_colors_something_real() {
        use HighlightKind as K;
        let cases: &[(Lang, &str, &str, HighlightKind)] = &[
            (Lang::JavaScript, "const a = 'hi';\n", "const", K::Keyword),
            (
                Lang::JavaScript,
                "const b = <div a=\"1\"></div>;\n",
                "div",
                K::Keyword,
            ),
            (
                Lang::TypeScript,
                "interface A { b: string }\n",
                "interface",
                K::Keyword,
            ),
            (Lang::Tsx, "const a: number = 1;\n", "const", K::Keyword),
            (Lang::Json, "{\"a\": 1}\n", "1", K::Number),
            (
                Lang::C,
                "int main(void) { return 0; }\n",
                "return",
                K::Keyword,
            ),
            (
                Lang::Cpp,
                "class A { public: int b; };\n",
                "class",
                K::Keyword,
            ),
            (Lang::Go, "func main() {}\n", "func", K::Keyword),
            (Lang::Html, "<div class=\"a\"></div>\n", "div", K::Keyword),
            (Lang::Css, "a { color: red; }\n", "color", K::Function),
            (Lang::Toml, "name = \"kyte\"\n", "name", K::Function),
            (Lang::Yaml, "name: kyte\n", "name", K::Function),
            (Lang::Markdown, "# Title\n\ntext\n", "Title", K::Keyword),
        ];
        for (lang, src, token, kind) in cases {
            let spans = highlight(*lang, src, 0..2);
            let k = kinds_of(src, &spans);
            assert!(
                k.iter().any(|(t, x)| t.contains(token) && x == kind),
                "{lang:?}: {k:?}"
            );
        }
    }

    #[test]
    fn names_with_no_extension_are_recognized() {
        assert_eq!(
            Lang::from_path(Path::new("/home/a/.bashrc")),
            Some(Lang::Bash)
        );
        assert_eq!(Lang::from_path(Path::new("Cargo.lock")), Some(Lang::Toml));
    }

    #[test]
    fn shebang_picks_the_language_when_the_name_does_not() {
        let d = |head: &str| Lang::detect(Path::new("/usr/local/bin/tool"), head);
        assert_eq!(d("#!/bin/sh\necho hi\n"), Some(Lang::Bash));
        assert_eq!(d("#!/usr/bin/env bash\n"), Some(Lang::Bash));
        assert_eq!(d("#!/usr/bin/python3.12\n"), Some(Lang::Python));
        assert_eq!(
            d("#!/usr/bin/env -S FOO=1 python3 -u\n"),
            Some(Lang::Python)
        );
        assert_eq!(d("#!/usr/bin/env node\n"), Some(Lang::JavaScript));
        assert_eq!(d("#!/usr/bin/perl\n"), None);
        assert_eq!(d("just text\n"), None);
        assert_eq!(d(""), None);
        assert_eq!(
            Lang::detect(Path::new("a.rs"), "#!/bin/sh\n"),
            Some(Lang::Rust),
            "the name wins"
        );
    }

    #[test]
    fn languages_from_extension() {
        assert_eq!(Lang::from_path(Path::new("a/b.rs")), Some(Lang::Rust));
        assert_eq!(Lang::from_path(Path::new("x.py")), Some(Lang::Python));
        assert_eq!(Lang::from_path(Path::new("run.sh")), Some(Lang::Bash));
        assert_eq!(Lang::from_path(Path::new("notes.txt")), None);
        assert_eq!(Lang::from_path(Path::new("a.tsx")), Some(Lang::Tsx));
        assert_eq!(Lang::from_path(Path::new("a.hpp")), Some(Lang::Cpp));
        assert_eq!(Lang::from_path(Path::new("a.yml")), Some(Lang::Yaml));
        assert_eq!(Lang::from_path(Path::new("Makefile")), None);
    }
}
