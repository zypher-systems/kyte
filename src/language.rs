//! What language a text is when its name doesn't say. A language is carried as
//! the file extension that stands for it, because everything else that looks
//! at a file's kind already goes by its name.
//!
//! [`guess`] reads only the text, and it's used for a buffer with no file name.
//! It stays quiet unless a line can hardly be anything else: a wrong guess puts
//! colors on someone's prose, while no guess only leaves code plain until it's
//! saved or a language is chosen.

/// Every language that can be chosen, by name, with the extension it goes by.
pub const LANGUAGES: [(&str, &str); 34] = [
    ("Bash", "sh"),
    ("BibTeX", "bib"),
    ("C", "c"),
    ("C#", "cs"),
    ("C++", "cpp"),
    ("CMake", "cmake"),
    ("CSS", "css"),
    ("Diff", "diff"),
    ("Dockerfile", "dockerfile"),
    ("Fish", "fish"),
    ("Go", "go"),
    ("HTML", "html"),
    ("INI", "ini"),
    ("Java", "java"),
    ("JavaScript", "js"),
    ("JSON", "json"),
    ("LaTeX", "tex"),
    ("Lua", "lua"),
    ("Make", "mk"),
    ("Markdown", "md"),
    ("Nix", "nix"),
    ("PHP", "php"),
    ("Plain text", "txt"),
    ("Python", "py"),
    ("Ruby", "rb"),
    ("Rust", "rs"),
    ("SQL", "sql"),
    ("Swift", "swift"),
    ("TOML", "toml"),
    ("TSX", "tsx"),
    ("TypeScript", "ts"),
    ("XML", "xml"),
    ("YAML", "yaml"),
    ("Zig", "zig"),
];

/// How far into a text [`guess`] looks, in lines that hold something.
const LINES: usize = 8;
/// How much of a text [`guess`] needs, in characters.
pub const OPENING: usize = 4096;

/// A name that gives `extension`'s language.
pub fn file_name(extension: &str) -> String {
    format!("x.{extension}")
}

fn words(line: &str) -> Vec<&str> {
    line.split_whitespace().collect()
}

/// Letters, digits and `_`, starting with a letter or `_`.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// A dotted name like `os.path`.
fn is_dotted(s: &str) -> bool {
    s.split('.').all(is_name)
}

fn is_capitalized(s: &str) -> bool {
    is_name(s) && s.starts_with(char::is_uppercase)
}

/// `name(`, as in a call or a definition.
fn opens_call(s: &str) -> bool {
    s.split_once('(').is_some_and(|(name, _)| is_name(name))
}

/// `[name]` alone on a line.
fn is_section(line: &str) -> bool {
    line.strip_prefix('[')
        .and_then(|l| l.strip_suffix(']'))
        .is_some_and(|n| !n.is_empty() && n.split(['.', '-']).all(is_name))
}

/// `key:` alone or with a value, as YAML has it.
fn is_yaml_key(line: &str) -> bool {
    line.split_once(':').is_some_and(|(key, value)| {
        (value.is_empty() || value.starts_with(' '))
            && key
                .split(['-', '_'])
                .all(|k| k.chars().all(char::is_alphanumeric))
            && !key.is_empty()
    })
}

/// The language of `import ...`, which half the languages here spell the same.
fn import_line(line: &str, parts: &[&str]) -> Option<&'static str> {
    let rest = line.strip_prefix("import ")?.trim();
    if rest.starts_with('{') || rest.starts_with("* as ") || rest.contains(" from ") {
        return Some("js");
    }
    if rest.starts_with('(') || (rest.starts_with('"') && !rest.ends_with(';')) {
        return Some("go");
    }
    if rest.starts_with('\'') {
        return Some("js");
    }
    if let Some(class) = rest.strip_suffix(';') {
        return (class.contains('.') && !class.contains(' ')).then_some("java");
    }
    match parts.get(1..)? {
        [one]
            if matches!(
                *one,
                "Foundation" | "SwiftUI" | "UIKit" | "AppKit" | "Combine"
            ) =>
        {
            Some("swift")
        }
        [one] if is_dotted(one) => Some("py"),
        [one, "as", two] if is_dotted(one) && is_name(two) => Some("py"),
        _ => None,
    }
}

/// `func` is Go's and Swift's, and only Swift annotates parameters and returns.
fn func_line(line: &str) -> &'static str {
    let params = line.split_once('(').map_or("", |(_, rest)| rest);
    if line.contains("->") || params.split(')').next().unwrap_or("").contains(": ") {
        "swift"
    } else {
        "go"
    }
}

/// What one line shows. Lines that show nothing return `None`.
fn line_kind(line: &str) -> Option<&'static str> {
    let lower = line.to_lowercase();
    let parts = words(line);
    let first = parts.first().copied().unwrap_or("");
    let second = parts.get(1).copied().unwrap_or("");

    // Markup and data, which say what they are in their first characters.
    if line.starts_with("<?php") {
        return Some("php");
    }
    if line.starts_with("<?xml") || line.starts_with("<svg") {
        return Some("xml");
    }
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
        return Some("html");
    }
    const TAGS: [&str; 10] = [
        "<head", "<body", "<div", "<span", "<table", "<script", "<style", "<ul", "<h1", "<p>",
    ];
    if TAGS.iter().any(|t| lower.starts_with(t)) {
        return Some("html");
    }
    if line.starts_with("diff --git ") || line.starts_with("--- a/") || line.starts_with("+++ b/") {
        return Some("diff");
    }
    const TEX: [&str; 6] = [
        "\\documentclass",
        "\\usepackage",
        "\\begin{",
        "\\section{",
        "\\chapter{",
        "\\title{",
    ];
    if TEX.iter().any(|t| line.starts_with(t)) {
        return Some("tex");
    }
    const BIB: [&str; 8] = [
        "@article{",
        "@book{",
        "@inproceedings{",
        "@incollection{",
        "@misc{",
        "@online{",
        "@phdthesis{",
        "@techreport{",
    ];
    if BIB.iter().any(|t| lower.starts_with(t)) {
        return Some("bib");
    }
    if line.starts_with('{') && line.ends_with("}:") {
        return Some("nix");
    }
    if matches!(line, "{" | "[") || line.starts_with("{\"") || line.starts_with("[{") {
        return Some("json");
    }
    if is_section(line) {
        return Some("toml");
    }
    if line.starts_with("```") || line.starts_with("- [ ] ") {
        return Some("md");
    }

    // Lines one language owns.
    if line.contains("@import(") {
        return Some("zig");
    }
    if line.starts_with("cmake_minimum_required(") {
        return Some("cmake");
    }
    if line.starts_with(".PHONY:") {
        return Some("mk");
    }
    if first == "FROM" && parts.len() >= 2 {
        return Some("dockerfile");
    }
    const SQL: [&str; 6] = [
        "create table ",
        "insert into ",
        "alter table ",
        "drop table ",
        "delete from ",
        "select * ",
    ];
    if line.starts_with("SELECT ") || SQL.iter().any(|t| lower.starts_with(t)) {
        return Some("sql");
    }
    if first == "#include" {
        // C's headers end in `.h`, and most of C++'s don't.
        let cpp = second.starts_with('<') && !second.ends_with(".h>");
        return Some(if cpp { "cpp" } else { "c" });
    }
    if matches!(first, "#define" | "#ifndef" | "#ifdef" | "typedef") {
        return Some("c");
    }
    if line.starts_with("template <")
        || line.starts_with("template<")
        || line.starts_with("using namespace ")
    {
        return Some("cpp");
    }

    // Rust.
    if line.starts_with("#![") || line.starts_with("#[") {
        return Some("rs");
    }
    let definition = if first == "pub" { second } else { first };
    if matches!(definition, "fn" | "struct" | "enum" | "impl" | "trait")
        && (first == "pub" || first == "fn" || first == "impl")
    {
        return Some("rs");
    }
    if line.starts_with("use std::")
        || line.starts_with("use crate::")
        || line.starts_with("let mut ")
    {
        return Some("rs");
    }

    // Python, Ruby and Lua.
    if let Some(kind) = import_line(line, &parts) {
        return Some(kind);
    }
    if first == "from" && parts.get(2) == Some(&"import") && is_dotted(second) {
        return Some("py");
    }
    if (first == "def" || (first == "async" && second == "def")) && line.ends_with(':') {
        return Some("py");
    }
    if first == "def" && (parts.len() == 2 || opens_call(second)) {
        return Some("rb");
    }
    if first == "class" && line.ends_with(':') && second.starts_with(char::is_uppercase) {
        return Some("py");
    }
    if line.starts_with("if __name__") {
        return Some("py");
    }
    if first == "require" && (second.starts_with('\'') || second.starts_with('"')) {
        return Some("rb");
    }
    if first == "class" && parts.get(2) == Some(&"<") {
        return Some("rb");
    }
    if first == "module" && parts.len() == 2 && is_capitalized(second) {
        return Some("rb");
    }
    if first == "local" && (second == "function" || parts.get(2) == Some(&"=")) {
        return Some("lua");
    }

    // Go, Swift, and languages with packages.
    if first == "func" {
        return Some(func_line(line));
    }
    if first == "package" && parts.len() == 2 {
        if second.ends_with(';') {
            return Some("java");
        }
        if is_name(second) {
            return Some("go");
        }
    }
    if first == "using" && line.ends_with(';') {
        return Some("cs");
    }
    if first == "public" && matches!(second, "class" | "interface" | "enum") {
        return Some("java");
    }

    // TypeScript, then JavaScript.
    let declared = if first == "export" { second } else { first };
    let named = parts
        .get(if first == "export" { 2 } else { 1 })
        .copied()
        .unwrap_or("");
    if (declared == "interface" && is_capitalized(named))
        || (declared == "type" && is_capitalized(named) && line.contains('='))
    {
        return Some("ts");
    }
    if matches!(first, "const" | "let" | "var") && line.contains(" = ") {
        return Some(if second.ends_with(':') { "ts" } else { "js" });
    }
    if first == "function" || line.starts_with("async function ") {
        if line.ends_with('{') {
            return Some("js");
        }
        if opens_call(second) {
            return Some("lua");
        }
    }
    if line.starts_with("export default ") || line.starts_with("console.log(") {
        return Some("js");
    }
    if line.starts_with("module.exports") {
        return Some("js");
    }

    // Shell, which has few lines of its own.
    if line.starts_with("set -e") || line.starts_with("set -u") {
        return Some("sh");
    }
    if line.starts_with("if [ ") || line.starts_with("if [[ ") {
        return Some("sh");
    }
    if first == "export" && second.contains('=') {
        return Some("sh");
    }

    // CSS.
    if line.starts_with("@media ") || line.starts_with(":root") {
        return Some("css");
    }
    if line.ends_with('{')
        && parts.len() == 2
        && (first.starts_with('.') || matches!(first, "html" | "body"))
    {
        return Some("css");
    }
    None
}

/// Whether `line` is only a comment, which says little about the language.
fn is_comment(line: &str) -> bool {
    const DIRECTIVES: [&str; 6] = ["#[", "#![", "#include", "#define", "#ifndef", "#ifdef"];
    const COMMENTS: [&str; 5] = ["//", "/*", "*/", "* ", "-- "];
    COMMENTS.iter().any(|c| line.starts_with(c))
        || (line.starts_with('#') && !DIRECTIVES.iter().any(|d| line.starts_with(d)))
}

/// The extension of the language `text` looks like. A `#!` line is left to
/// what reads those. YAML needs two lines in a row, and Markdown's heading
/// loses to anything else, because it's also a comment in half the languages.
pub fn guess(text: &str) -> Option<&'static str> {
    if text.starts_with("#!") && !text.starts_with("#![") {
        return None;
    }
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(LINES)
        .collect();
    let code: Vec<&str> = lines.iter().copied().filter(|l| !is_comment(l)).collect();
    if let Some(kind) = code.iter().find_map(|l| line_kind(l)) {
        return Some(kind);
    }
    if let [a, b, ..] = code[..] {
        if is_yaml_key(a) && is_yaml_key(b) {
            return Some("yaml");
        }
    }
    let first = lines.first()?;
    let heading = first.trim_start_matches('#');
    let hashes = first.len() - heading.len();
    ((1..=6).contains(&hashes) && heading.starts_with(' ')).then_some("md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_says_what_it_is() {
        for (text, want) in [
            ("fn main() {}", "rs"),
            ("pub fn run() {", "rs"),
            ("impl Foo {", "rs"),
            ("use std::io;\n", "rs"),
            ("#[derive(Debug)]\nstruct A;", "rs"),
            ("// a tool\n\nuse crate::app;\n", "rs"),
            ("import os\n", "py"),
            ("import numpy as np\n", "py"),
            ("from a.b import c\n", "py"),
            ("def go(x):\n    pass\n", "py"),
            ("# setup\nimport sys\n", "py"),
            ("class Foo(Bar):\n", "py"),
            ("def go\n  1\nend\n", "rb"),
            ("require 'json'\n", "rb"),
            ("class Foo < Bar\n", "rb"),
            ("package main\n", "go"),
            ("import (\n\t\"fmt\"\n)\n", "go"),
            ("import \"fmt\"\n", "go"),
            ("func main() {\n", "go"),
            ("func (s *Server) Run() error {\n", "go"),
            ("func go(x: Int) -> Int {\n", "swift"),
            ("import SwiftUI\n", "swift"),
            ("#include <stdio.h>\n", "c"),
            ("#include \"a.h\"\n", "c"),
            ("#define MAX 3\n", "c"),
            ("#include <vector>\n", "cpp"),
            ("using namespace std;\n", "cpp"),
            ("package com.acme;\n", "java"),
            ("import java.util.List;\n", "java"),
            ("public class Main {\n", "java"),
            ("using System;\n", "cs"),
            ("const x = 1;\n", "js"),
            ("function go() {\n", "js"),
            ("import { a } from 'b';\n", "js"),
            ("import a from \"b\"\n", "js"),
            ("const x: number = 1;\n", "ts"),
            ("interface User {\n", "ts"),
            ("export type Id = string;\n", "ts"),
            ("function go()\n  return 1\nend\n", "lua"),
            ("local x = 1\n", "lua"),
            ("const std = @import(\"std\");\n", "zig"),
            ("set -euo pipefail\n", "sh"),
            ("export PATH=/bin\n", "sh"),
            ("SELECT a FROM b;\n", "sql"),
            ("create table a (b int);\n", "sql"),
            ("FROM ubuntu:24.04\n", "dockerfile"),
            (".PHONY: all\n", "mk"),
            ("cmake_minimum_required(VERSION 3.20)\n", "cmake"),
            ("{ pkgs, ... }:\n", "nix"),
        ] {
            assert_eq!(guess(text), Some(want), "{text:?}");
        }
    }

    #[test]
    fn markup_and_data_say_what_they_are() {
        for (text, want) in [
            ("<?php\n", "php"),
            ("<?xml version=\"1.0\"?>\n", "xml"),
            ("<!DOCTYPE html>\n", "html"),
            ("<div class=\"a\">\n", "html"),
            ("\\documentclass{article}\n", "tex"),
            ("% notes\n\\section{One}\n", "tex"),
            ("@article{knuth,\n", "bib"),
            ("diff --git a/x b/x\n", "diff"),
            ("{\n  \"a\": 1\n}\n", "json"),
            ("{\"a\": 1}\n", "json"),
            ("[package]\nname = \"a\"\n", "toml"),
            ("name: ci\non: push\n", "yaml"),
            ("services:\n  web:\n", "yaml"),
            ("# Title\n\nSome words.\n", "md"),
            ("## Title\n", "md"),
            ("```rust\nfn a() {}\n```\n", "md"),
            (".box {\n  color: red;\n}\n", "css"),
            ("body {\n", "css"),
        ] {
            assert_eq!(guess(text), Some(want), "{text:?}");
        }
    }

    #[test]
    fn prose_gets_no_guess() {
        for text in [
            "",
            "\n\n",
            "Dear reader,\n\nThis is a letter.\n",
            "import this module and then carry on",
            "from here to there",
            "class notes for Tuesday",
            "package arrived today, thanks",
            "module 3 covers grammar",
            "Note: buy milk\n\nThen go home.\n",
            "let me know when you can\n",
            "select the best of them from the list",
            "function of the heart",
            "define the terms first",
            "export the data to a sheet",
            "#hashtag",
            "####### not a heading",
            "template for the letter",
            "def not going",
            "class notes:",
        ] {
            assert_eq!(guess(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_shebang_is_left_alone() {
        assert_eq!(guess("#!/bin/sh\nimport os\n"), None);
        assert_eq!(guess("#![allow(dead_code)]\n"), Some("rs"));
    }

    #[test]
    fn only_the_opening_lines_count() {
        let mut text = "words\n".repeat(LINES);
        text.push_str("fn main() {}\n");
        assert_eq!(guess(&text), None);
    }

    #[test]
    fn every_language_has_one_name_and_one_extension() {
        let names: std::collections::HashSet<_> = LANGUAGES.iter().map(|l| l.0).collect();
        let exts: std::collections::HashSet<_> = LANGUAGES.iter().map(|l| l.1).collect();
        assert_eq!(names.len(), LANGUAGES.len());
        assert_eq!(exts.len(), LANGUAGES.len());
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn every_language_can_be_chosen_and_stands_for_itself() {
        use crate::highlight::Lang;
        use std::path::Path;
        let named: Vec<_> = LANGUAGES
            .iter()
            .filter_map(|(_, ext)| Lang::from_path(Path::new(&file_name(ext))))
            .collect();
        assert_eq!(
            named.len(),
            Lang::ALL.len(),
            "only Plain text has no grammar"
        );
        for lang in Lang::ALL {
            assert!(named.contains(&lang), "{lang:?} can't be chosen");
        }
    }
}
