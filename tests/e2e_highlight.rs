//! Syntax highlighting is visible in the terminal, and doesn't hide selection
//! or search highlighting (feature `highlight`, on by default).
#![cfg(feature = "highlight")]

mod common;

use std::path::Path;

use common::{Pty, TIMEOUT};

/// Screen column of the first `pat` in row `r` (the gutter has multi-byte `│`).
fn col_of(p: &Pty, r: usize, pat: &str) -> u16 {
    let row = p.row(r);
    let i = row.find(pat).unwrap();
    row[..i].chars().count() as u16
}

/// Kyte on a Rust file holding `text`.
fn open_rust(text: &str) -> (tempfile::TempDir, Pty) {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let file = dir.path().join("main.rs");
    std::fs::write(&file, text).unwrap();
    let p = Pty::kyte(
        Path::new(env!("CARGO_BIN_EXE_kyte")),
        &state,
        &[file.to_str().unwrap()],
    );
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    (dir, p)
}

#[test]
fn selection_shows_on_a_highlighted_line() {
    let (_dir, mut p) = open_rust("fn main() {}\n");
    p.wait_for("fn main", TIMEOUT);
    let col = col_of(&p, 0, "fn");
    let plain = p.bg(0, col);
    p.send(b"\x1b[1;2C");
    p.send(b"\x1b[1;2C");
    p.wait_for("2 selected", TIMEOUT);
    assert_ne!(p.bg(0, col), plain, "selected text looks selected");
    assert_eq!(
        p.fg(0, col).unwrap(),
        vt100::Color::Idx(176),
        "and keeps its color"
    );
    assert_eq!(p.bg(0, col + 3), plain, "the rest doesn't");
    p.send_keys(&["Esc"]);
    p.wait_for("x cut", TIMEOUT);
    std::thread::sleep(std::time::Duration::from_millis(30));
    p.send_keys(&["Esc"]);
    p.wait_gone("2 selected", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_exit(TIMEOUT);
}

#[test]
fn search_matches_show_on_a_highlighted_line() {
    let (_dir, mut p) = open_rust("fn main() { main() }\n");
    p.wait_for("fn main", TIMEOUT);
    let col = col_of(&p, 0, "main");
    let plain = p.bg(0, col);
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    p.send_str("main");
    p.wait_for("Search: main", TIMEOUT);
    assert_ne!(p.bg(0, col), plain, "the match shows");
    assert_ne!(p.bg(0, col + 12), plain, "and so does the other one");
    p.send_keys(&["Esc"]);
    p.wait_gone("Search: main", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_exit(TIMEOUT);
}

#[test]
fn esc_offers_code_actions_and_they_compose() {
    let (dir, mut p) = open_rust("fn one() {\n    let a = 1;\n}\n\nfn two() {}\n");
    p.wait_for("fn two", TIMEOUT);
    p.send_keys(&["Down"]);
    p.wait_for("Ln 2", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("select function", TIMEOUT);
    p.send_str("f");
    p.wait_for("27 selected", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("x cut", TIMEOUT);
    p.send_str("x");
    p.wait_gone("fn one", TIMEOUT);
    p.send_keys(&["C-s"]);
    p.wait_for("Saved", TIMEOUT);
    let saved = std::fs::read_to_string(dir.path().join("main.rs")).unwrap();
    assert_eq!(saved, "\n\nfn two() {}\n");
    p.send_keys(&["C-q"]);
    p.wait_exit(TIMEOUT);
}

#[test]
fn reader_focuses_on_a_function_and_shows_where_you_are() {
    let (_dir, mut p) =
        open_rust("impl Foo {\n    fn bar() {\n        x();\n    }\n}\n\nfn other() {}\n");
    p.wait_for("fn other", TIMEOUT);
    for _ in 0..2 {
        p.send_keys(&["Down"]);
    }
    p.wait_for("Ln 3", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("reader mode", TIMEOUT);
    p.send_str("v");
    p.wait_for("impl Foo › fn bar", TIMEOUT);
    let col = col_of(&p, 6, "other");
    assert_eq!(
        p.fg(6, col),
        Some(vt100::Color::Idx(240)),
        "other functions are dimmed, syntax colors included"
    );
    p.send_str("n");
    p.wait_for("fn other", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone(" READER ", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_exit(TIMEOUT);
}

#[test]
fn t13_rust_keyword_has_non_default_foreground() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let file = dir.path().join("main.rs");
    std::fs::write(&file, "fn main() {\n    let x = 1;\n}\n").unwrap();
    let mut p = Pty::kyte(
        Path::new(env!("CARGO_BIN_EXE_kyte")),
        &state,
        &[file.to_str().unwrap()],
    );
    p.wait_for("let x = 1;", TIMEOUT);
    // Row 0 is `  1 │ fn main() {`.
    let col = col_of(&p, 0, "fn");
    let kw = p.fg(0, col).unwrap();
    let space = p.fg(0, col + 2).unwrap();
    assert_ne!(kw, space, "keyword `fn` should be colored, got {kw:?}");
    assert_eq!(kw, vt100::Color::Idx(176), "keyword color");
    p.send_keys(&["C-q"]);
    p.wait_exit(TIMEOUT);
}

#[test]
fn t13_plain_text_file_is_not_highlighted() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let mut p = Pty::kyte(
        Path::new(env!("CARGO_BIN_EXE_kyte")),
        &state,
        &[file.to_str().unwrap()],
    );
    p.wait_for("fn main", TIMEOUT);
    let col = col_of(&p, 0, "fn");
    assert_eq!(p.fg(0, col).unwrap(), vt100::Color::Idx(252));
    p.send_keys(&["C-q"]);
    p.wait_exit(TIMEOUT);
}
