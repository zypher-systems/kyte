//! What the code under the cursor is, for the Esc layer's code actions. Parsing
//! happens only when an action runs, so opening the layer stays instant. All
//! positions are byte offsets into the text.

use std::ops::Range;
use std::path::Path;

/// The line comment for a file, by extension.
pub fn comment_prefix(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "go" | "java" | "js" | "jsx" | "ts" | "tsx"
        | "swift" | "kt" | "scala" | "cs" | "zig" => Some("//"),
        "py" | "pyi" | "sh" | "bash" | "zsh" | "fish" | "rb" | "pl" | "toml" | "yaml" | "yml"
        | "conf" | "ini" | "mk" | "r" | "ex" | "exs" => Some("#"),
        "lua" | "sql" | "hs" => Some("--"),
        "vim" => Some("\""),
        _ => None,
    }
}

#[cfg(feature = "highlight")]
mod code {
    use super::*;
    use crate::highlight::Lang;
    use tree_sitter::{Node, Parser, Tree};

    fn parse(lang: Lang, text: &str) -> Option<Tree> {
        let mut parser = Parser::new();
        parser.set_language(&lang.language()).ok()?;
        parser.parse(text, None)
    }

    fn is_function(lang: Lang, node: &Node) -> bool {
        match lang {
            Lang::Rust => node.kind() == "function_item",
            Lang::Python | Lang::Bash => node.kind() == "function_definition",
        }
    }

    /// Whether this file's language has code actions.
    pub fn supported(path: &Path) -> bool {
        Lang::from_path(path).is_some()
    }

    /// The function around `at`. A cursor just past its end still counts.
    pub fn function_at(path: &Path, text: &str, at: usize) -> Option<Range<usize>> {
        let lang = Lang::from_path(path)?;
        let tree = parse(lang, text)?;
        let root = tree.root_node();
        [at, at.saturating_sub(1)].into_iter().find_map(|pos| {
            let mut node = root.descendant_for_byte_range(pos, pos);
            while let Some(n) = node {
                if is_function(lang, &n) {
                    return Some(n.byte_range());
                }
                node = n.parent();
            }
            None
        })
    }

    /// The next larger piece of syntax around `range`.
    pub fn expand(path: &Path, text: &str, range: Range<usize>) -> Option<Range<usize>> {
        let lang = Lang::from_path(path)?;
        let tree = parse(lang, text)?;
        let mut node = tree
            .root_node()
            .named_descendant_for_byte_range(range.start, range.end);
        while let Some(n) = node {
            let r = n.byte_range();
            let larger = r.start <= range.start && r.end >= range.end && r != range;
            if larger {
                return Some(r);
            }
            node = n.parent();
        }
        None
    }

    /// Every function in the file, in the order they start.
    fn functions(lang: Lang, tree: &Tree) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut cursor = tree.walk();
        loop {
            if is_function(lang, &cursor.node()) {
                out.push(cursor.node().byte_range());
            }
            if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return out;
                }
            }
        }
    }

    /// The start of the first function after `at`.
    pub fn next_function(path: &Path, text: &str, at: usize) -> Option<usize> {
        let lang = Lang::from_path(path)?;
        let tree = parse(lang, text)?;
        functions(lang, &tree)
            .into_iter()
            .map(|r| r.start)
            .find(|s| *s > at)
    }

    /// The start of the last function before `at`.
    pub fn prev_function(path: &Path, text: &str, at: usize) -> Option<usize> {
        let lang = Lang::from_path(path)?;
        let tree = parse(lang, text)?;
        functions(lang, &tree)
            .into_iter()
            .map(|r| r.start)
            .rfind(|s| *s < at)
    }
}

#[cfg(not(feature = "highlight"))]
mod code {
    use super::*;

    pub fn supported(_: &Path) -> bool {
        false
    }

    pub fn function_at(_: &Path, _: &str, _: usize) -> Option<Range<usize>> {
        None
    }

    pub fn expand(_: &Path, _: &str, _: Range<usize>) -> Option<Range<usize>> {
        None
    }

    pub fn next_function(_: &Path, _: &str, _: usize) -> Option<usize> {
        None
    }

    pub fn prev_function(_: &Path, _: &str, _: usize) -> Option<usize> {
        None
    }
}

pub use code::{expand, function_at, next_function, prev_function, supported};

#[cfg(all(test, feature = "highlight"))]
mod tests {
    use super::*;

    const RUST: &str = "fn one() {\n    let a = (1 + 2);\n}\n\nfn two() {}\n";
    const PY: &str = "def one():\n    return 1\n\ndef two():\n    return 2\n";
    const SH: &str = "one() {\n  echo hi\n}\n\ntwo() { :; }\n";

    #[test]
    fn functions_are_found_in_each_language() {
        let at = RUST.find("let").unwrap();
        let r = function_at(Path::new("a.rs"), RUST, at).unwrap();
        assert_eq!(&RUST[r], "fn one() {\n    let a = (1 + 2);\n}");
        let at = PY.find("return 2").unwrap();
        let r = function_at(Path::new("a.py"), PY, at).unwrap();
        assert_eq!(&PY[r.clone()][..7], "def two");
        let at = SH.find("echo").unwrap();
        let r = function_at(Path::new("a.sh"), SH, at).unwrap();
        assert!(SH[r].starts_with("one()"));
    }

    #[test]
    fn outside_a_function_is_none() {
        let at = RUST.find("\n\nfn two").unwrap() + 1;
        assert_eq!(function_at(Path::new("a.rs"), RUST, at), None);
        assert_eq!(function_at(Path::new("notes.txt"), RUST, 3), None);
    }

    #[test]
    fn just_past_the_end_still_counts() {
        let end = RUST.find("}\n").unwrap() + 1;
        let r = function_at(Path::new("a.rs"), RUST, end).unwrap();
        assert_eq!(r.start, 0);
    }

    #[test]
    fn expand_grows_outward_until_the_file() {
        let at = RUST.find('1').unwrap();
        let path = Path::new("a.rs");
        let a = expand(path, RUST, at..at).unwrap();
        assert_eq!(&RUST[a.clone()], "1");
        let b = expand(path, RUST, a).unwrap();
        assert_eq!(&RUST[b.clone()], "1 + 2");
        let c = expand(path, RUST, b).unwrap();
        assert_eq!(&RUST[c], "(1 + 2)");
        let whole = 0..RUST.len();
        assert_eq!(expand(path, RUST, whole), None);
    }

    #[test]
    fn stepping_between_functions() {
        let path = Path::new("a.rs");
        let two = RUST.find("fn two").unwrap();
        assert_eq!(next_function(path, RUST, 0), Some(two));
        assert_eq!(next_function(path, RUST, two), None);
        assert_eq!(prev_function(path, RUST, two), Some(0));
        assert_eq!(prev_function(path, RUST, 0), None);
    }

    #[test]
    fn nested_functions_count() {
        let src = "fn outer() {\n    fn inner() {}\n}\n";
        let path = Path::new("a.rs");
        let inner = src.find("fn inner").unwrap();
        assert_eq!(next_function(path, src, 0), Some(inner));
        let r = function_at(path, src, inner + 3).unwrap();
        assert_eq!(&src[r], "fn inner() {}");
    }

    #[test]
    fn broken_code_doesnt_panic() {
        let src = "fn broken( {\n  let = ;;; }}}\n";
        let path = Path::new("a.rs");
        let _ = function_at(path, src, 5);
        let _ = expand(path, src, 5..5);
        let _ = next_function(path, src, 0);
    }
}

#[cfg(test)]
mod prefix_tests {
    use super::*;

    #[test]
    fn comment_prefixes_by_extension() {
        assert_eq!(comment_prefix(Path::new("a.rs")), Some("//"));
        assert_eq!(comment_prefix(Path::new("a.py")), Some("#"));
        assert_eq!(comment_prefix(Path::new("a.sql")), Some("--"));
        assert_eq!(comment_prefix(Path::new("notes.txt")), None);
        assert_eq!(comment_prefix(Path::new("Makefile")), None);
    }
}
