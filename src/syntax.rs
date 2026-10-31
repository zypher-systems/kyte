//! What the code under the cursor is, for the Esc layer's code actions. Parsing
//! happens only when an action runs, so opening the layer stays instant. All
//! positions are byte offsets into the text.

use std::ops::Range;
use std::path::Path;

/// Code larger than this isn't parsed on every key, because that would lag.
pub const MAX_LIVE_PARSE_BYTES: usize = 512 * 1024;

/// The program a `#!` line runs, without its directory. `env` and its own
/// flags are looked past. `#![...]` is a Rust attribute, not a `#!` line.
pub fn shebang_program(head: &str) -> Option<&str> {
    let line = head.lines().next()?.strip_prefix("#!")?;
    if line.starts_with('[') {
        return None;
    }
    let mut words = line.split_whitespace();
    let first = words.next()?;
    let program = if first.ends_with("/env") {
        // As in `env -S FOO=1 python3`.
        words.find(|w| !w.starts_with('-') && !w.contains('='))?
    } else {
        first
    };
    program.rsplit('/').next()
}

fn extension_prefix(extension: &str) -> Option<&'static str> {
    match extension {
        "rs" | "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "go" | "java" | "js"
        | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "jsonc" | "swift" | "kt"
        | "scala" | "cs" | "zig" | "php" | "phtml" => Some("//"),
        "py" | "pyi" | "sh" | "bash" | "zsh" | "fish" | "rb" | "pl" | "toml" | "yaml" | "yml"
        | "conf" | "ini" | "cfg" | "mk" | "r" | "ex" | "exs" | "rake" | "gemspec" | "nix"
        | "cmake" | "dockerfile" | "desktop" | "service" | "timer" | "socket" => Some("#"),
        "lua" | "sql" | "hs" => Some("--"),
        "vim" => Some("\""),
        _ => None,
    }
}

/// The line comment for a file: by its name or extension, or failing that by
/// the program in its `#!` line.
pub fn comment_prefix(path: &Path, head: &str) -> Option<&'static str> {
    let by_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| match n {
            ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" | "PKGBUILD" => Some("#"),
            "Makefile" | "makefile" | "GNUmakefile" | "Dockerfile" | "Containerfile"
            | "Gemfile" | "Rakefile" | "Vagrantfile" | "Brewfile" | "CMakeLists.txt"
            | ".gitconfig" | ".gitmodules" | ".editorconfig" => Some("#"),
            n if n.starts_with("Dockerfile.") => Some("#"),
            _ => None,
        });
    let by_extension = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(extension_prefix);
    by_name.or(by_extension).or_else(|| {
        shebang_program(head).map(|program| match program {
            "node" | "nodejs" | "deno" | "bun" | "php" => "//",
            "lua" => "--",
            _ => "#",
        })
    })
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
        let kind = node.kind();
        match lang {
            Lang::Rust => kind == "function_item",
            Lang::Python | Lang::Bash | Lang::C | Lang::Cpp | Lang::Fish => {
                kind == "function_definition"
            }
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx => matches!(
                kind,
                "function_declaration"
                    | "generator_function_declaration"
                    | "method_definition"
                    | "function_expression"
                    | "arrow_function"
            ),
            Lang::Go => matches!(kind, "function_declaration" | "method_declaration"),
            Lang::Java | Lang::CSharp => {
                matches!(kind, "method_declaration" | "constructor_declaration")
            }
            Lang::Php => matches!(kind, "function_definition" | "method_declaration"),
            Lang::Ruby => matches!(kind, "method" | "singleton_method"),
            Lang::Lua | Lang::Zig => kind == "function_declaration",
            _ => false,
        }
    }

    /// Whether this file's language can be parsed.
    pub fn supported(path: &Path, head: &str) -> bool {
        Lang::detect(path, head).is_some()
    }

    /// Whether this file's language has functions to move between. Data and
    /// markup formats don't.
    pub fn has_functions(path: &Path, head: &str) -> bool {
        matches!(
            Lang::detect(path, head),
            Some(
                Lang::Rust
                    | Lang::Python
                    | Lang::Bash
                    | Lang::JavaScript
                    | Lang::TypeScript
                    | Lang::Tsx
                    | Lang::C
                    | Lang::Cpp
                    | Lang::Go
                    | Lang::Java
                    | Lang::CSharp
                    | Lang::Php
                    | Lang::Ruby
                    | Lang::Lua
                    | Lang::Fish
                    | Lang::Zig
            )
        )
    }

    /// The function around `at`. A cursor just past its end still counts.
    pub fn function_at(path: &Path, text: &str, at: usize) -> Option<Range<usize>> {
        let lang = Lang::detect(path, text)?;
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

    /// A named place in the code, if `node` is one.
    fn crumb(lang: Lang, node: &Node, text: &str) -> Option<String> {
        let name = |field: &str| {
            node.child_by_field_name(field)
                .and_then(|n| n.utf8_text(text.as_bytes()).ok())
        };
        if matches!(lang, Lang::C | Lang::Cpp) && node.kind() == "function_definition" {
            // The name sits inside any pointer and parameter wrapping.
            let mut declarator = node.child_by_field_name("declarator")?;
            while declarator.kind().ends_with("_declarator") {
                declarator = declarator
                    .child_by_field_name("declarator")
                    .or_else(|| declarator.named_child(0))?;
            }
            return declarator
                .utf8_text(text.as_bytes())
                .ok()
                .map(str::to_string);
        }
        let (word, field) = match (lang, node.kind()) {
            (Lang::Rust, "function_item") => ("fn", "name"),
            (Lang::Rust, "impl_item") => ("impl", "type"),
            (Lang::Rust, "trait_item") => ("trait", "name"),
            (Lang::Rust, "mod_item") => ("mod", "name"),
            (Lang::Python, "class_definition") => ("class", "name"),
            (Lang::Python, "function_definition") => ("def", "name"),
            (Lang::Bash, "function_definition") => ("function", "name"),
            (
                Lang::JavaScript | Lang::TypeScript | Lang::Tsx,
                "function_declaration" | "generator_function_declaration",
            ) => ("function", "name"),
            (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, "class_declaration") => {
                ("class", "name")
            }
            (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, "method_definition") => {
                ("method", "name")
            }
            (Lang::TypeScript | Lang::Tsx, "interface_declaration") => ("interface", "name"),
            (Lang::Cpp, "class_specifier") => ("class", "name"),
            (Lang::Cpp, "namespace_definition") => ("namespace", "name"),
            (Lang::Go, "function_declaration") => ("func", "name"),
            (Lang::Go, "method_declaration") => ("func", "name"),
            (Lang::Java | Lang::CSharp | Lang::Php, "class_declaration") => ("class", "name"),
            (Lang::Java | Lang::CSharp, "interface_declaration") => ("interface", "name"),
            (Lang::Java | Lang::CSharp | Lang::Php, "method_declaration") => ("method", "name"),
            (Lang::Java | Lang::CSharp, "constructor_declaration") => ("new", "name"),
            (Lang::CSharp, "namespace_declaration") => ("namespace", "name"),
            (Lang::Php, "function_definition") => ("function", "name"),
            (Lang::Ruby, "class") => ("class", "name"),
            (Lang::Ruby, "module") => ("module", "name"),
            (Lang::Ruby, "method" | "singleton_method") => ("def", "name"),
            (Lang::Lua, "function_declaration") => ("function", "name"),
            (Lang::Fish, "function_definition") => ("function", "name"),
            (Lang::Zig, "function_declaration") => ("fn", "name"),
            _ => return None,
        };
        Some(format!("{word} {}", name(field)?))
    }

    /// The named places around `at`, outermost first.
    pub fn breadcrumb(path: &Path, text: &str, at: usize) -> Vec<String> {
        let Some(lang) = Lang::detect(path, text) else {
            return Vec::new();
        };
        let Some(tree) = parse(lang, text) else {
            return Vec::new();
        };
        let root = tree.root_node();
        let mut out = Vec::new();
        let mut node = root.descendant_for_byte_range(at, at);
        while let Some(n) = node {
            out.extend(crumb(lang, &n, text));
            node = n.parent();
        }
        out.reverse();
        out
    }

    /// The next larger piece of syntax around `range`.
    pub fn expand(path: &Path, text: &str, range: Range<usize>) -> Option<Range<usize>> {
        let lang = Lang::detect(path, text)?;
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
        let lang = Lang::detect(path, text)?;
        let tree = parse(lang, text)?;
        functions(lang, &tree)
            .into_iter()
            .map(|r| r.start)
            .find(|s| *s > at)
    }

    /// The start of the last function before `at`.
    pub fn prev_function(path: &Path, text: &str, at: usize) -> Option<usize> {
        let lang = Lang::detect(path, text)?;
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

    pub fn supported(_: &Path, _: &str) -> bool {
        false
    }

    pub fn has_functions(_: &Path, _: &str) -> bool {
        false
    }

    pub fn function_at(_: &Path, _: &str, _: usize) -> Option<Range<usize>> {
        None
    }

    pub fn breadcrumb(_: &Path, _: &str, _: usize) -> Vec<String> {
        Vec::new()
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

pub use code::{
    breadcrumb, expand, function_at, has_functions, next_function, prev_function, supported,
};

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

    /// `(file, text, a place inside, what the function starts with, its crumb)`.
    const LATER: &[(&str, &str, &str, &str, &str)] = &[
        (
            "A.java",
            "class A {\n  int one() {\n    return 1;\n  }\n}\n",
            "return",
            "int one",
            "method one",
        ),
        (
            "A.cs",
            "class A {\n  int One() {\n    return 1;\n  }\n}\n",
            "return",
            "int One",
            "method One",
        ),
        (
            "a.php",
            "<?php\nclass A {\n  function one() {\n    return 1;\n  }\n}\n",
            "return",
            "function one",
            "method one",
        ),
        (
            "a.php",
            "<?php\nfunction one() {\n  return 1;\n}\n",
            "return",
            "function one",
            "function one",
        ),
        (
            "a.rb",
            "class A\n  def one\n    1\n  end\nend\n",
            "1",
            "def one",
            "def one",
        ),
        (
            "a.lua",
            "function one()\n  return 1\nend\n",
            "return",
            "function one",
            "function one",
        ),
        (
            "a.fish",
            "function one\n  echo hi\nend\n",
            "echo",
            "function one",
            "function one",
        ),
        (
            "a.zig",
            "fn one() u8 {\n    return 1;\n}\n",
            "return",
            "fn one",
            "fn one",
        ),
    ];

    #[test]
    fn functions_are_found_in_the_later_languages() {
        for (file, text, inside, start, crumb) in LATER {
            let path = Path::new(file);
            assert!(has_functions(path, text), "{file}");
            let at = text.find(inside).unwrap();
            let r = function_at(path, text, at).unwrap_or_else(|| panic!("{file}"));
            assert!(text[r.clone()].starts_with(start), "{file}: {:?}", &text[r]);
            let crumbs = breadcrumb(path, text, at);
            assert_eq!(crumbs.last().map(String::as_str), Some(*crumb), "{file}");
            let found = prev_function(path, text, text.len());
            assert_eq!(found, Some(text.find(start).unwrap()), "{file}");
        }
    }

    #[test]
    fn languages_without_functions_keep_those_keys_off_the_menu() {
        for file in [
            "a.sql",
            "a.xml",
            "Dockerfile",
            "Makefile",
            "a.ini",
            "a.diff",
        ] {
            assert!(!has_functions(Path::new(file), ""), "{file}");
            assert!(supported(Path::new(file), ""), "{file}");
        }
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
    fn functions_are_found_in_the_new_languages() {
        let cases = [
            (
                "a.js",
                "function one() {\n  x();\n}\n",
                "x()",
                "function one",
            ),
            (
                "a.ts",
                "class A {\n  run(): void {\n    x();\n  }\n}\n",
                "x()",
                "method run",
            ),
            (
                "a.tsx",
                "function App() {\n  return x();\n}\n",
                "x()",
                "function App",
            ),
            ("a.go", "func main() {\n\tx()\n}\n", "x()", "func main"),
            ("a.c", "int main(void) {\n  x();\n}\n", "x()", "main"),
            (
                "a.cpp",
                "namespace n {\nint f() {\n  x();\n}\n}\n",
                "x()",
                "f",
            ),
        ];
        for (name, src, token, crumb) in cases {
            let path = Path::new(name);
            let at = src.find(token).unwrap();
            assert!(function_at(path, src, at).is_some(), "{name}");
            let crumbs = breadcrumb(path, src, at);
            assert_eq!(
                crumbs.last().map(String::as_str),
                Some(crumb),
                "{name}: {crumbs:?}"
            );
        }
    }

    #[test]
    fn arrow_functions_count_in_javascript() {
        let src = "const f = (a) => {\n  return a;\n};\n";
        let at = src.find("return").unwrap();
        let r = function_at(Path::new("a.js"), src, at).unwrap();
        assert!(src[r].starts_with("(a) =>"));
    }

    #[test]
    fn data_formats_can_expand_but_have_no_functions() {
        let path = Path::new("a.json");
        assert!(supported(path, ""));
        assert!(!has_functions(path, ""));
        assert!(has_functions(Path::new("a.go"), ""));
        let src = "{\"a\": [1, 2]}\n";
        let at = src.find('1').unwrap();
        let r = expand(path, src, at..at).unwrap();
        assert_eq!(&src[r.clone()], "1");
        let r = expand(path, src, r).unwrap();
        assert_eq!(&src[r], "[1, 2]");
    }

    #[test]
    fn breadcrumbs_name_the_places_around_the_cursor() {
        let src =
            "mod m {\n    impl Foo {\n        fn bar() {\n            x();\n        }\n    }\n}\n";
        let at = src.find("x()").unwrap();
        assert_eq!(
            breadcrumb(Path::new("a.rs"), src, at),
            vec!["mod m", "impl Foo", "fn bar"]
        );
        let py = "class A:\n    def b(self):\n        pass\n";
        let at = py.find("pass").unwrap();
        assert_eq!(
            breadcrumb(Path::new("a.py"), py, at),
            vec!["class A", "def b"]
        );
        assert!(breadcrumb(Path::new("a.txt"), py, 0).is_empty());
        assert!(breadcrumb(Path::new("a.rs"), "", 0).is_empty());
    }

    #[test]
    fn c_breadcrumbs_name_the_function_not_its_pointer() {
        let name = |src: &str| {
            let at = src.find("x()").unwrap();
            breadcrumb(Path::new("a.c"), src, at).pop()
        };
        assert_eq!(
            name("int *foo(int a) {\n  x();\n}\n").as_deref(),
            Some("foo")
        );
        assert_eq!(
            name("char **bar(void) {\n  x();\n}\n").as_deref(),
            Some("bar")
        );
        assert_eq!(
            name("int (*baz(int a))(void) {\n  x();\n}\n").as_deref(),
            Some("baz")
        );
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
        assert_eq!(comment_prefix(Path::new("a.rs"), ""), Some("//"));
        assert_eq!(comment_prefix(Path::new("a.py"), ""), Some("#"));
        assert_eq!(comment_prefix(Path::new("a.sql"), ""), Some("--"));
        assert_eq!(comment_prefix(Path::new("notes.txt"), ""), None);
        assert_eq!(comment_prefix(Path::new("Makefile"), ""), Some("#"));
        assert_eq!(comment_prefix(Path::new("Dockerfile.dev"), ""), Some("#"));
        assert_eq!(comment_prefix(Path::new("a.php"), ""), Some("//"));
        assert_eq!(
            comment_prefix(Path::new("tool"), "#!/usr/bin/lua\n"),
            Some("--")
        );
        assert_eq!(comment_prefix(Path::new("tool"), "#!/bin/sh\n"), Some("#"));
    }

    #[test]
    fn a_comment_comes_from_the_language_not_just_a_shebang() {
        let node = "#!/usr/bin/env node\n";
        assert_eq!(comment_prefix(Path::new("script.js"), node), Some("//"));
        assert_eq!(comment_prefix(Path::new("script.ts"), node), Some("//"));
        assert_eq!(comment_prefix(Path::new("tool"), node), Some("//"));
        assert_eq!(
            comment_prefix(Path::new("tool"), "#!/usr/bin/deno\n"),
            Some("//")
        );
        assert_eq!(
            comment_prefix(Path::new("tool"), "#!/usr/bin/python3\n"),
            Some("#")
        );
        let attr = "#![deny(warnings)]\n";
        assert_eq!(comment_prefix(Path::new("lib.rs"), attr), Some("//"));
        assert_eq!(
            comment_prefix(Path::new("tool"), attr),
            None,
            "not a shebang"
        );
        assert_eq!(comment_prefix(Path::new(".bashrc"), ""), Some("#"));
    }
}
