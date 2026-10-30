//! T13: syntax highlighting is visible in the terminal (feature `highlight`).
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
