//! End-to-end tests: the real `kyte` binary in a pty at 100x30 with
//! TERM=xterm-256color, KYTE_STATE_DIR in a temp dir and --no-kitty, read back
//! through a vt100 emulator. Tests are named after the acceptance criteria.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{Pty, Spawn, TIMEOUT};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kyte"))
}

struct Env {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        Env { dir, state }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.path(name);
        fs::write(&p, text).unwrap();
        p
    }

    /// Starts kyte on `file` and waits for the first frame.
    fn open(&self, file: &Path) -> Pty {
        self.open_args(file, &[])
    }

    fn open_args(&self, file: &Path, extra: &[&str]) -> Pty {
        let mut args: Vec<&str> = extra.to_vec();
        let f = file.to_str().unwrap();
        args.push(f);
        let pty = Pty::kyte(&bin(), &self.state, &args);
        pty.wait_for("Ln 1, Col 1", TIMEOUT);
        pty
    }
}

/// The status bar row (second to last) of a 30-row screen.
fn status(p: &Pty) -> String {
    p.row(28)
}

fn wait_status(p: &Pty, text: &str) {
    let what = format!("status bar to contain {text:?}");
    p.wait_until(&what, TIMEOUT, |_| status(p).contains(text));
}

fn save_and_wait(p: &mut Pty) {
    p.send_keys(&["C-s"]);
    p.wait_for("Saved ", TIMEOUT);
}

fn quit_clean(p: &mut Pty) {
    p.send_keys(&["C-q"]);
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

// ---------------------------------------------------------------- AC-03

#[test]
fn ac03_not_a_tty_exits_2() {
    let out = Command::new(bin())
        .arg("x.txt")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.trim_end().lines().count(), 1, "one-line message: {err}");
    assert!(err.contains("terminal"));
}

#[test]
fn ac03_stdout_not_a_tty_exits_2() {
    let env = Env::new();
    let script = format!(
        "{} --no-kitty {} > /dev/null; echo \"EXIT=$?\"",
        bin().display(),
        env.path("f.txt").display()
    );
    let mut p = Pty::spawn(Spawn::new("sh").arg("-c").arg(script));
    p.wait_for("EXIT=2", TIMEOUT);
    p.wait_exit(TIMEOUT);
}

#[test]
fn ac03_directory_exits_2() {
    let env = Env::new();
    let mut p = Pty::kyte(&bin(), &env.state, &[env.dir.path().to_str().unwrap()]);
    assert_eq!(p.wait_exit(TIMEOUT), 2);
    assert!(p.screen().contains("is a directory"), "{}", p.dump());
}

#[test]
fn ac03_non_utf8_exits_2_and_file_unchanged() {
    let env = Env::new();
    let f = env.path("bin.dat");
    let bytes = vec![b'o', b'k', 0xff, 0xfe, b'\n', 0xc3];
    fs::write(&f, &bytes).unwrap();
    let mut p = Pty::kyte(&bin(), &env.state, &[f.to_str().unwrap()]);
    assert_eq!(p.wait_exit(TIMEOUT), 2);
    assert!(p.screen().contains("not valid UTF-8"), "{}", p.dump());
    assert_eq!(fs::read(&f).unwrap(), bytes);
}

// ---------------------------------------------------------------- AC-04

#[test]
fn ac04_new_file_type_save_quit() {
    let env = Env::new();
    let f = env.path("new.txt");
    let mut p = env.open(&f);
    p.send_str("hello");
    p.send_keys(&["Enter"]);
    p.send_str("world");
    p.wait_for("world", TIMEOUT);
    save_and_wait(&mut p);
    assert!(p.screen().contains("Saved 2 lines"));
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "hello\nworld");
}

// ---------------------------------------------------------------- AC-05

#[test]
fn ac05_save_preserves_bytes_newline_mode_no_temp() {
    let env = Env::new();
    let original = "alpha\nbeta\ngamma\n";
    let f = env.write("keep.txt", original);
    fs::set_permissions(&f, fs::Permissions::from_mode(0o640)).unwrap();
    let mut p = env.open(&f);
    p.send_keys(&["Down", "X"]);
    wait_status(&p, "[+]");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "alpha\nXbeta\ngamma\n");
    assert_eq!(
        fs::metadata(&f).unwrap().permissions().mode() & 0o7777,
        0o640
    );
    let names: Vec<String> = fs::read_dir(env.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let mut names = names;
    names.sort();
    assert_eq!(names, vec!["keep.txt".to_string(), "state".to_string()]);
}

// ---------------------------------------------------------------- AC-06

#[test]
fn ac06_cursor_motion_and_status_bar() {
    let env = Env::new();
    let mut text = String::from("abcdef\nab\nabcdef\n");
    for i in 4..=60 {
        text.push_str(&format!("line {i}\n"));
    }
    let f = env.write("move.txt", &text);
    let mut p = env.open(&f);
    p.send_keys(&["Right", "Right", "Right"]);
    wait_status(&p, "Ln 1, Col 4");
    p.send_keys(&["End"]);
    wait_status(&p, "Ln 1, Col 7");
    // remembered column across a shorter line
    p.send_keys(&["Down"]);
    wait_status(&p, "Ln 2, Col 3");
    p.send_keys(&["Down"]);
    wait_status(&p, "Ln 3, Col 7");
    p.send_keys(&["Up"]);
    wait_status(&p, "Ln 2, Col 3");
    p.send_keys(&["Home"]);
    wait_status(&p, "Ln 2, Col 1");
    p.send_keys(&["Left"]);
    wait_status(&p, "Ln 1, Col 7");
    // PageDown moves by the 28-row text area
    p.send_keys(&["Home", "PageDown"]);
    wait_status(&p, "Ln 29, Col 1");
    p.send_keys(&["PageUp"]);
    wait_status(&p, "Ln 1, Col 1");
    p.send_keys(&["C-End"]);
    wait_status(&p, "Ln 61, Col 1");
    p.wait_for("60 │ line 60", TIMEOUT);
    p.send_keys(&["C-Home"]);
    wait_status(&p, "Ln 1, Col 1");
    p.wait_for("  1 │ abcdef", TIMEOUT);
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- AC-07

#[test]
fn ac07_unicode_graphemes_and_wide_cursor() {
    let env = Env::new();
    let f = env.write("uni.txt", "héllo 世界 🙂");
    let mut p = env.open(&f);
    p.wait_for("héllo 世界 🙂", TIMEOUT);
    p.send_keys(&["End"]);
    wait_status(&p, "Ln 1, Col 11");
    // gutter is 6 cells; "héllo " = 6, 世界 = 4, space = 1, 🙂 = 2 => col 6 + 13
    assert_eq!(p.cursor(), (0, 19));
    p.send_keys(&["Left"]);
    wait_status(&p, "Ln 1, Col 10");
    assert_eq!(p.cursor(), (0, 17));
    p.send_keys(&["Left"]);
    wait_status(&p, "Ln 1, Col 9");
    assert_eq!(p.cursor(), (0, 16));
    p.send_keys(&["Left"]);
    wait_status(&p, "Ln 1, Col 8");
    assert_eq!(p.cursor(), (0, 14));
    p.send_keys(&["Right", "Right", "Right"]);
    wait_status(&p, "Ln 1, Col 11");
    p.send_keys(&["Backspace"]);
    wait_status(&p, "Ln 1, Col 10");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "héllo 世界 ");
}

// ---------------------------------------------------------------- AC-08

#[test]
fn ac08_bracketed_paste_is_one_undo_step() {
    let env = Env::new();
    let f = env.write("paste.txt", "");
    let mut p = env.open(&f);
    p.paste("one\rtwo\rthree\rfour\rfive");
    p.wait_for("5 │ five", TIMEOUT);
    for (i, w) in ["one", "two", "three", "four", "five"].iter().enumerate() {
        assert!(p.row(i).contains(w), "{}", p.dump());
    }
    p.send_keys(&["C-z"]);
    p.wait_gone("five", TIMEOUT);
    wait_status(&p, "Ln 1, Col 1");
    assert!(!p.screen().contains("one"));
    p.send_keys(&["C-y"]);
    p.wait_for("5 │ five", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "one\ntwo\nthree\nfour\nfive"
    );
}

// ---------------------------------------------------------------- AC-10

#[test]
fn ac10_resize_keeps_bars_at_bottom_and_cursor_visible() {
    let env = Env::new();
    let text: String = (1..=40).map(|i| format!("row {i}\n")).collect();
    let f = env.write("resize.txt", &text);
    let mut p = env.open(&f);
    for _ in 0..20 {
        p.send_keys(&["Down"]);
    }
    wait_status(&p, "Ln 21, Col 1");
    p.resize(10, 40);
    p.wait_until("status bar on row 8 of 10", TIMEOUT, |_| {
        p.row(8).contains("INSERT") && p.row(8).contains("Ln 21")
    });
    let (r, c) = p.cursor();
    assert!(
        r < 8 && c < 40,
        "cursor {r},{c} must be in the text area\n{}",
        p.dump()
    );
    assert!(p.row(r as usize).contains("21 │ row 21"), "{}", p.dump());
    p.resize(30, 100);
    p.wait_until("status bar on row 28 of 30", TIMEOUT, |_| {
        p.row(28).contains("INSERT") && p.row(28).contains("Ln 21")
    });
    let (r, _) = p.cursor();
    assert!(r < 28);
    assert!(p.row(r as usize).contains("21 │ row 21"), "{}", p.dump());
    // tiny terminal: a notice, no crash
    p.resize(4, 15);
    p.wait_for("terminal too", TIMEOUT);
    p.resize(30, 100);
    wait_status(&p, "Ln 21, Col 1");
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- AC-11

#[test]
fn ac11_large_file_ctrl_end_and_typing_within_1s() {
    let env = Env::new();
    let mut text = String::with_capacity(1_300_000);
    for i in 1..=100_000 {
        text.push_str(&format!("line {i}\n"));
    }
    let f = env.write("big.txt", &text);
    let start = Instant::now();
    let mut p = env.open(&f);
    let opened = start.elapsed();
    p.send_keys(&["C-End"]);
    wait_status(&p, "Ln 100001, Col 1");
    let t = Instant::now();
    p.send_str("Z");
    p.wait_for("100001 │ Z", Duration::from_secs(1));
    let typed = t.elapsed();
    assert!(typed < Duration::from_secs(1), "typing took {typed:?}");
    eprintln!("AC-11: opened in {opened:?}, keystroke visible in {typed:?}");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_keys(&["C-q"]);
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

// ---------------------------------------------------------------- AC-18

#[test]
fn ac18_quit_dirty_asks_then_discards() {
    let env = Env::new();
    let f = env.write("q.txt", "keep me\n");
    let mut p = env.open(&f);
    p.send_str("junk");
    wait_status(&p, "[+]");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(p.try_exit(), None, "must not exit on the first Ctrl-Q");
    // any other key cancels
    p.send_keys(&["Right"]);
    p.wait_gone("Unsaved changes", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_keys(&["C-q"]);
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), "keep me\n");
}

#[test]
fn ac18_quit_dirty_y_discards() {
    let env = Env::new();
    let f = env.write("q.txt", "keep me\n");
    let mut p = env.open(&f);
    p.send_str("junk");
    wait_status(&p, "[+]");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("y");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), "keep me\n");
}

#[test]
fn ac18_quit_clean_exits_and_restores_terminal() {
    let env = Env::new();
    let f = env.write("q.txt", "hi\n");
    let script = format!(
        "echo shell-before; {} --no-kitty {}; echo shell-after-$?; sleep 2",
        bin().display(),
        f.display()
    );
    let mut p = Pty::spawn(
        Spawn::new("sh")
            .arg("-c")
            .arg(script)
            .env("KYTE_STATE_DIR", env.state.to_string_lossy()),
    );
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    assert!(p.alternate_screen());
    p.send_keys(&["C-q"]);
    p.wait_for("shell-after-0", TIMEOUT);
    assert!(!p.alternate_screen(), "alternate screen must be left");
    assert!(!p.cursor_hidden(), "cursor must be visible");
    assert!(p.screen().contains("shell-before"), "{}", p.dump());
    assert!(!p.screen().contains("INSERT"));
}

// ---------------------------------------------------------------- Esc layer helpers

const LEGEND_PART: &str = "w del word";
const LEGEND_END: &str = "Esc Esc palette";

fn has_legend(p: &Pty) -> bool {
    let s = p.screen();
    s.contains(LEGEND_PART) || s.contains(LEGEND_END)
}

fn status_has_esc(p: &Pty) -> bool {
    status(p).contains(" ESC ")
}

/// Esc, then wait until the overlay is visible.
fn open_overlay(p: &mut Pty) {
    p.send_keys(&["Esc"]);
    p.wait_for(LEGEND_PART, TIMEOUT);
}

fn wait_insert(p: &Pty) {
    p.wait_until("Insert mode (no legend)", TIMEOUT, |s| {
        !s.contains(LEGEND_PART) && s.contains(" INSERT ")
    });
}

/// Saves, quits and returns the file content.
fn save_quit_read(p: &mut Pty, f: &Path) -> String {
    save_and_wait(p);
    quit_clean(p);
    fs::read_to_string(f).unwrap()
}

// ---------------------------------------------------------------- AC-09

#[test]
fn ac09_undo_redo_typing_and_operator() {
    let env = Env::new();
    let f = env.write("undo.txt", "first\nsecond\n");
    let mut p = env.open(&f);
    p.send_keys(&["End"]);
    p.send_str("abc");
    p.wait_for("firstabc", TIMEOUT);
    p.send_keys(&["C-z"]);
    p.wait_gone("firstabc", TIMEOUT);
    p.wait_for("1 │ first", TIMEOUT);
    p.send_keys(&["C-y"]);
    p.wait_for("firstabc", TIMEOUT);
    // operator undone by exactly one Ctrl-Z
    open_overlay(&mut p);
    p.send_keys(&["d"]);
    p.wait_gone("firstabc", TIMEOUT);
    p.send_keys(&["C-z"]);
    p.wait_for("firstabc", TIMEOUT);
    assert!(p.screen().contains("2 │ second"));
    // the same through the overlay: Esc u, Esc r
    open_overlay(&mut p);
    p.send_keys(&["u"]);
    p.wait_gone("firstabc", TIMEOUT); // undo of the redone typing group
    wait_insert(&p);
    open_overlay(&mut p);
    p.send_keys(&["r"]);
    p.wait_for("firstabc", TIMEOUT);
    wait_insert(&p);
    open_overlay(&mut p);
    p.send_keys(&["d"]);
    p.wait_gone("firstabc", TIMEOUT);
    open_overlay(&mut p);
    p.send_keys(&["u"]);
    p.wait_for("firstabc", TIMEOUT);
    assert_eq!(save_quit_read(&mut p, &f), "firstabc\nsecond\n");
}

// ---------------------------------------------------------------- AC-12

#[test]
fn ac12_legend_appears_instantly_and_only_in_overlay() {
    let env = Env::new();
    let f = env.write("legend.txt", "foo bar baz\nsecond line\n");
    let mut p = env.open(&f);
    assert!(!has_legend(&p));
    assert!(!status_has_esc(&p));
    let t = Instant::now();
    p.send_keys(&["Esc"]);
    p.wait_for(LEGEND_PART, Duration::from_millis(100));
    let dt = t.elapsed();
    assert!(dt < Duration::from_millis(100), "legend took {dt:?}");
    eprintln!("AC-12: legend visible {dt:?} after Esc");
    for part in [
        "w del word",
        "d del line",
        "k del to end",
        "% bracket",
        "/ search",
        "u undo",
        "r redo",
        "s save",
        "Esc Esc palette",
    ] {
        assert!(
            p.row(29).contains(part),
            "legend misses {part:?}: {}",
            p.row(29)
        );
    }
    assert!(status_has_esc(&p));
    // after an operator
    p.send_keys(&["w"]);
    wait_insert(&p);
    assert!(!status_has_esc(&p));
    // after an unknown key
    open_overlay(&mut p);
    p.send_keys(&["z"]);
    wait_insert(&p);
    assert!(!status_has_esc(&p));
    // after Esc, 500 ms, Esc
    open_overlay(&mut p);
    std::thread::sleep(Duration::from_millis(500));
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    assert!(!status_has_esc(&p));
    assert!(!p.screen().contains("Command palette"));
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_keys(&["y"]);
    p.wait_exit(TIMEOUT);
}

// ---------------------------------------------------------------- AC-13

/// Opens `initial`, sends `keys` (named keys, each a separate write, with the
/// overlay legend awaited after every Esc), saves, quits, returns the file.
fn op_case(initial: &str, keys: &[&str]) -> String {
    let env = Env::new();
    let f = env.write("op.txt", initial);
    let mut p = env.open(&f);
    for k in keys {
        if *k == "Esc" {
            open_overlay(&mut p);
        } else {
            p.send_keys(&[k]);
            if keys_close_overlay(k) {
                wait_insert(&p);
            }
        }
    }
    save_and_wait(&mut p);
    quit_clean(&mut p);
    fs::read_to_string(&f).unwrap()
}

fn keys_close_overlay(k: &str) -> bool {
    matches!(k, "w" | "d" | "k" | "%")
}

#[test]
fn ac13_w_deletes_to_next_word() {
    assert_eq!(
        op_case("foo bar baz", &["Right", "Right", "Right", "Esc", "w"]),
        "foobar baz"
    );
    assert_eq!(op_case("foo bar", &["Esc", "w"]), "bar");
}

#[test]
fn ac13_d_middle_last_and_only_line() {
    assert_eq!(op_case("a\nbb\nc", &["Down", "Right", "Esc", "d"]), "a\nc");
    assert_eq!(op_case("a\nb\nc", &["Down", "Down", "Esc", "d"]), "a\nb");
    assert_eq!(op_case("only", &["Esc", "d"]), "");
}

#[test]
fn ac13_d_cursor_lands_at_same_column_clamped() {
    // a\nb|b\nc -> a\n|c ; then typing X proves where the cursor is
    assert_eq!(
        op_case("a\nbb\nc", &["Down", "Right", "Esc", "d", "X"]),
        "a\nXc"
    );
    assert_eq!(
        op_case("a\nlong\nxyz", &["Down", "Right", "Right", "Esc", "d", "X"]),
        "a\nxyXz"
    );
}

#[test]
fn ac13_k_mid_line_and_at_end_of_line() {
    assert_eq!(
        op_case("abcd\nef", &["Right", "Right", "Esc", "k"]),
        "ab\nef"
    );
    assert_eq!(op_case("ab\nef", &["End", "Esc", "k"]), "abef");
}

#[test]
fn ac13_percent_nested_not_on_bracket_and_unmatched() {
    // on a bracket: jump to its partner; X marks the landing spot
    assert_eq!(op_case("(a[b]c)", &["Esc", "%", "X"]), "(a[b]cX)");
    assert_eq!(
        op_case("(a[b]c)", &["Right", "Right", "Esc", "%", "X"]),
        "(a[bX]c)"
    );
    // not on a bracket: first bracket on the line, then its partner
    assert_eq!(op_case("x(a[b]c)y", &["Esc", "%", "X"]), "x(a[b]cX)y");
}

#[test]
fn ac13_percent_unmatched_shows_message_and_does_not_move() {
    let env = Env::new();
    let f = env.write("op.txt", "(ab");
    let mut p = env.open(&f);
    open_overlay(&mut p);
    p.send_keys(&["%"]);
    p.wait_for("No matching bracket", TIMEOUT);
    wait_status(&p, "Ln 1, Col 1");
    assert!(!status(&p).contains("[+]"));
    p.send_str("X");
    p.wait_gone("No matching bracket", TIMEOUT);
    assert_eq!(save_quit_read(&mut p, &f), "X(ab");
}

// ---------------------------------------------------------------- AC-14

#[test]
fn ac14_double_esc_within_150ms_opens_palette() {
    let env = Env::new();
    let f = env.write("pal.txt", "text\n");
    let mut p = env.open(&f);
    let t = Instant::now();
    open_overlay(&mut p);
    p.send_keys(&["Esc"]);
    let gap = t.elapsed();
    assert!(gap < Duration::from_millis(150), "gap {gap:?}");
    p.wait_for("Command palette", TIMEOUT);
    assert!(!has_legend(&p));
    p.send_keys(&["Esc"]);
    p.wait_gone("Command palette", TIMEOUT);
    wait_insert(&p);
    assert!(!status(&p).contains("[+]"));
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "text\n");
}

#[test]
fn ac14_esc_pause_esc_closes_overlay() {
    let env = Env::new();
    let f = env.write("pal.txt", "text\n");
    let mut p = env.open(&f);
    open_overlay(&mut p);
    std::thread::sleep(Duration::from_millis(500));
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    std::thread::sleep(Duration::from_millis(50));
    assert!(!p.screen().contains("Command palette"));
    assert!(!status(&p).contains("[+]"));
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "text\n");
}

#[test]
fn ac14_esc_ms_800_opens_palette_after_500ms() {
    let env = Env::new();
    let f = env.write("pal.txt", "text\n");
    let mut p = env.open_args(&f, &["--esc-ms", "800"]);
    open_overlay(&mut p);
    std::thread::sleep(Duration::from_millis(500));
    p.send_keys(&["Esc"]);
    p.wait_for("Command palette", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "text\n");
}

#[test]
fn ac14_kyte_esc_ms_env_is_honoured() {
    let env = Env::new();
    let f = env.write("pal.txt", "text\n");
    let mut p = Pty::spawn(
        Spawn::new(bin())
            .env("KYTE_STATE_DIR", env.state.to_string_lossy())
            .env("KYTE_ESC_MS", "800")
            .arg("--no-kitty")
            .arg(f.to_string_lossy()),
    );
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    open_overlay(&mut p);
    std::thread::sleep(Duration::from_millis(500));
    p.send_keys(&["Esc"]);
    p.wait_for("Command palette", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- AC-15

#[test]
fn ac15_unknown_overlay_key_inserts_nothing() {
    let env = Env::new();
    let f = env.write("z.txt", "abc\n");
    let mut p = env.open(&f);
    open_overlay(&mut p);
    p.send_keys(&["z"]);
    wait_insert(&p);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(p.row(0), "  1 │ abc", "{}", p.dump());
    assert!(
        !p.rows()[..28].iter().any(|r| r.contains('z')),
        "{}",
        p.dump()
    );
    assert!(!status(&p).contains("[+]"));
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "abc\n");
}

#[test]
fn ac15_alt_d_bytes_behave_as_esc_then_d() {
    let env = Env::new();
    let f = env.write("alt.txt", "one\ntwo\nthree\n");
    let mut p = env.open(&f);
    p.send_keys(&["Down"]);
    wait_status(&p, "Ln 2");
    p.send(b"\x1bd"); // legacy Alt-d: ESC d in one write
    p.wait_gone("two", TIMEOUT);
    wait_insert(&p);
    assert_eq!(save_quit_read(&mut p, &f), "one\nthree\n");
}

#[test]
fn ac15_ctrl_space_opens_overlay() {
    let env = Env::new();
    let f = env.write("cs.txt", "abc\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-Space"]);
    p.wait_for(LEGEND_PART, TIMEOUT);
    assert!(status_has_esc(&p));
    p.send_keys(&["d"]);
    p.wait_gone("abc", TIMEOUT);
    assert_eq!(save_quit_read(&mut p, &f), "");
}
