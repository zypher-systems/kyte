//! End-to-end tests: the real `kyte` binary in a pty at 100x30 with
//! TERM=xterm-256color, KYTE_STATE_DIR in a temp dir and --no-kitty, read back
//! through a vt100 emulator. Tests are named after the acceptance criteria.

mod common;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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

/// Types `text` (one line of plain ASCII) and waits until all of it has
/// landed. `[+]` shows after the first character, so it says nothing about the
/// rest, and a check of the swap or a signal could beat them.
fn type_text(p: &mut Pty, text: &str) {
    let col_now = |s: &str| {
        let line = s.lines().rev().nth(1).unwrap_or("").to_string();
        let digits: String = line
            .split("Col ")
            .nth(1)
            .unwrap_or("")
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse::<usize>().unwrap_or(0)
    };
    let want = col_now(&p.screen()) + text.len();
    p.send_str(text);
    p.wait_until("typing to land", TIMEOUT, |s| col_now(s) == want);
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
    p.wait_for("is a directory", TIMEOUT);
}

#[test]
fn ac03_non_utf8_exits_2_and_file_unchanged() {
    let env = Env::new();
    let f = env.path("bin.dat");
    let bytes = vec![b'o', b'k', 0xff, 0xfe, b'\n', 0xc3];
    fs::write(&f, &bytes).unwrap();
    let mut p = Pty::kyte(&bin(), &env.state, &[f.to_str().unwrap()]);
    assert_eq!(p.wait_exit(TIMEOUT), 2);
    p.wait_for("not valid UTF-8", TIMEOUT);
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
    p.wait_for("Saved 2 lines", TIMEOUT);
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
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

// ---------------------------------------------------------------- AC-18

#[test]
fn long_single_line_stays_responsive() {
    let env = Env::new();
    let f = env.write("long.txt", &"a".repeat(5_000_000));
    let mut p = env.open(&f);
    p.send_keys(&["C-End"]);
    wait_status(&p, "Col 5000001");
    let t = Instant::now();
    p.send_str("x");
    wait_status(&p, "Col 5000002");
    // A loose bound for a debug build on a busy machine; the old path took
    // several times longer.
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::metadata(&f).unwrap().len(), 5_000_001);
}

#[test]
fn ac18_quit_dirty_asks_then_discards() {
    let env = Env::new();
    let f = env.write("q.txt", "keep me\n");
    let mut p = env.open(&f);
    type_text(&mut p, "junk");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(p.try_exit(), None, "must not exit on the first Ctrl-Q");
    // other keys are ignored, including a second Ctrl-Q
    p.send_keys(&["Right", "C-q"]);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(p.try_exit(), None, "a second Ctrl-Q must not discard");
    p.wait_for("Unsaved changes", TIMEOUT);
    // Esc cancels
    p.send_keys(&["Esc"]);
    p.wait_gone("Unsaved changes", TIMEOUT);
    wait_status(&p, "[+]");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), "keep me\n");
}

#[test]
fn ac18_quit_dirty_y_saves_and_quits() {
    let env = Env::new();
    let f = env.write("q.txt", "keep me\n");
    let mut p = env.open(&f);
    type_text(&mut p, "more ");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("y");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), "more keep me\n");
    assert!(swap_files(&env).is_empty());
}

#[test]
fn y_with_a_failed_save_stays_open_and_keeps_the_swap() {
    let env = Env::new();
    if fs::metadata(env.dir.path()).unwrap().uid() == 0 {
        eprintln!("skipped: root can write to anything");
        return;
    }
    let dir = env.path("ro");
    fs::create_dir(&dir).unwrap();
    let f = dir.join("f.txt");
    fs::write(&f, "x\n").unwrap();
    let mut p = env.open(&f);
    type_text(&mut p, "work");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("y");
    p.wait_for("Save failed", TIMEOUT);
    assert_eq!(p.try_exit(), None);
    assert_eq!(swap_files(&env).len(), 1);
    // Save As gets the work out
    let elsewhere = env.path("elsewhere.txt");
    save_as(&mut p, elsewhere.to_str().unwrap());
    p.wait_for("Saved 1 line", TIMEOUT);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "workx\n");
    assert_eq!(fs::read_to_string(&f).unwrap(), "x\n");
    wait_status(&p, "elsewhere.txt");
    quit_clean(&mut p);
}

/// Opens Save As from the palette and replaces the name with `path`.
fn save_as(p: &mut Pty, path: &str) {
    open_palette(p);
    p.send_str("save as");
    p.wait_for("▸ Save as", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Save as:", TIMEOUT);
    for _ in 0..500 {
        p.send_keys(&["Backspace"]);
    }
    p.wait_until("the prompt to empty", TIMEOUT, |s| {
        s.lines().any(|l| l.trim_end() == " Save as:")
    });
    p.send_str(path);
    p.send_keys(&["Enter"]);
}

#[test]
fn save_as_moves_the_buffer_and_its_swap_to_the_new_file() {
    let env = Env::new();
    let f = work_file(&env, "old.txt", "text\n");
    let new = f.with_file_name("new.txt");
    let mut p = env.open(&f);
    type_text(&mut p, "edit ");
    save_as(&mut p, new.to_str().unwrap());
    p.wait_for("Saved 1 line", TIMEOUT);
    wait_status(&p, "new.txt");
    assert_eq!(fs::read_to_string(&f).unwrap(), "text\n", "old file stays");
    assert_eq!(fs::read_to_string(&new).unwrap(), "edit text\n");
    let swaps = swap_files(&env);
    assert_eq!(swaps.len(), 1);
    assert!(swaps[0].to_string_lossy().contains("new.txt"));
    type_text(&mut p, "more ");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&new).unwrap(), "edit more text\n");
}

#[test]
fn save_as_asks_before_overwriting_and_refuses_a_directory() {
    let env = Env::new();
    let f = work_file(&env, "a.txt", "mine\n");
    let other = env.write("other.txt", "precious\n");
    let mut p = env.open(&f);
    save_as(&mut p, other.to_str().unwrap());
    p.wait_for("File exists!", TIMEOUT);
    p.send_str("n");
    p.wait_for("Save cancelled", TIMEOUT);
    assert_eq!(fs::read_to_string(&other).unwrap(), "precious\n");
    save_as(&mut p, env.dir.path().to_str().unwrap());
    p.wait_for("Not a regular file", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("Save cancelled", TIMEOUT);
    save_as(&mut p, other.to_str().unwrap());
    p.wait_for("File exists!", TIMEOUT);
    p.send_str("y");
    p.wait_for("Saved 1 line", TIMEOUT);
    assert_eq!(fs::read_to_string(&other).unwrap(), "mine\n");
    quit_clean(&mut p);
}

#[test]
fn save_as_refuses_a_file_open_in_another_kyte() {
    let env = Env::new();
    let f = work_file(&env, "a.txt", "mine\n");
    let other = work_file(&env, "b.txt", "theirs\n");
    let mut a = env.open(&other);
    let mut p = env.open(&f);
    save_as(&mut p, other.to_str().unwrap());
    p.wait_for("Open in another kyte", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("Save cancelled", TIMEOUT);
    quit_clean(&mut p);
    quit_clean(&mut a);
    assert_eq!(fs::read_to_string(&other).unwrap(), "theirs\n");
}

fn open_untitled(env: &Env) -> Pty {
    let p = Pty::kyte(&bin(), &env.state, &[]);
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    p
}

#[test]
fn untitled_buffer_is_saved_by_asking_for_a_name() {
    let env = Env::new();
    let f = env.path("named.txt");
    let mut p = open_untitled(&env);
    p.send_str("first draft");
    wait_status(&p, "Untitled [+]");
    assert_eq!(swap_files(&env).len(), 1, "typing reaches a swap");
    p.send_keys(&["C-s"]);
    p.wait_for("Save as:", TIMEOUT);
    p.send_str(f.to_str().unwrap());
    p.send_keys(&["Enter"]);
    p.wait_for("Saved 1 line", TIMEOUT);
    wait_status(&p, "named.txt");
    assert_eq!(fs::read_to_string(&f).unwrap(), "first draft");
    let swaps = swap_files(&env);
    assert_eq!(swaps.len(), 1);
    assert!(swaps[0].to_string_lossy().contains("named.txt"));
    quit_clean(&mut p);
}

#[test]
fn quitting_an_untitled_buffer_with_y_saves_then_quits() {
    let env = Env::new();
    let f = env.path("named.txt");
    let mut p = open_untitled(&env);
    type_text(&mut p, "draft");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("y");
    p.wait_for("Save as:", TIMEOUT);
    p.send_str(f.to_str().unwrap());
    p.send_keys(&["Enter"]);
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), "draft");
    assert!(swap_files(&env).is_empty());
}

#[test]
fn untitled_buffer_survives_a_crash() {
    let env = Env::new();
    let mut p = open_untitled(&env);
    type_text(&mut p, "unnamed work");
    std::thread::sleep(Duration::from_millis(100));
    p.kill9();
    let mut p = Pty::kyte(&bin(), &env.state, &[]);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.wait_for("(untitled)", TIMEOUT);
    p.send_str("q");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    let mut p = Pty::kyte(&bin(), &env.state, &[]);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ unnamed work", TIMEOUT);
    wait_status(&p, "Untitled [+]");
    // a second crash still recovers it
    p.send_str("more ");
    p.wait_for("more unnamed work", TIMEOUT);
    std::thread::sleep(Duration::from_millis(100));
    p.kill9();
    let mut p = Pty::kyte(&bin(), &env.state, &[]);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("d");
    p.wait_for("Discarded the swap file", TIMEOUT);
    assert!(!p.screen().contains("unnamed work"));
    assert_eq!(swap_files(&env).len(), 1, "only the new session's swap");
    quit_clean(&mut p);
    assert!(swap_files(&env).is_empty());
}

#[test]
fn crashed_empty_untitled_session_offers_nothing() {
    let env = Env::new();
    let mut p = open_untitled(&env);
    p.kill9();
    let mut p = open_untitled(&env);
    assert!(!p.screen().contains("Recover"));
    assert_eq!(swap_files(&env).len(), 1);
    quit_clean(&mut p);
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
    p.wait_for("shell-before", TIMEOUT);
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
    p.wait_for("2 │ second", TIMEOUT);
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
    // Esc must never wait, so a build that waits is slow every time. A busy
    // machine is slow only sometimes, so one fast attempt is proof enough.
    let mut best = Duration::from_secs(60);
    for _ in 0..5 {
        let t = Instant::now();
        p.send_keys(&["Esc"]);
        p.wait_for(LEGEND_PART, TIMEOUT);
        best = best.min(t.elapsed());
        if best < Duration::from_millis(100) {
            break;
        }
        p.send_keys(&["z"]);
        wait_insert(&p);
    }
    assert!(best < Duration::from_millis(100), "legend took {best:?}");
    eprintln!("AC-12: legend visible {best:?} after Esc");
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
            p.screen().contains(part),
            "legend misses {part:?}: {}",
            p.dump()
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
    p.send_str("n");
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
    // The two Escs must land inside the window. If a busy machine stretched
    // the gap, the second one just closed the layer, so try again.
    for attempt in 0..5 {
        let t = Instant::now();
        open_overlay(&mut p);
        p.send_keys(&["Esc"]);
        if t.elapsed() < Duration::from_millis(150) {
            break;
        }
        assert!(attempt < 4, "never got two Escs within 150 ms");
        std::thread::sleep(Duration::from_millis(400));
        if p.screen().contains("Command palette") {
            p.send_keys(&["Esc"]);
            p.wait_gone("Command palette", TIMEOUT);
        }
        wait_insert(&p);
    }
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
fn a_terminal_that_says_it_takes_24_bit_color_gets_it() {
    let env = Env::new();
    let f = env.write("colors.txt", "text\n");
    for (colorterm, expected) in [
        (Some("truecolor"), vt100::Color::Rgb(0x0b, 0x0c, 0x10)),
        (Some("24bit"), vt100::Color::Rgb(0x0b, 0x0c, 0x10)),
        (None, vt100::Color::Idx(232)),
    ] {
        let mut spawn = Spawn::new(bin())
            .env("KYTE_STATE_DIR", env.state.to_string_lossy())
            .arg("--no-kitty")
            .arg(f.to_string_lossy());
        if let Some(value) = colorterm {
            spawn = spawn.env("COLORTERM", value);
        }
        let mut p = Pty::spawn(spawn);
        p.wait_for("Ln 1, Col 1", TIMEOUT);
        assert_eq!(p.bg(5, 40), Some(expected), "{colorterm:?}\n{}", p.dump());
        quit_clean(&mut p);
    }
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

// ---------------------------------------------------------------- AC-16

#[test]
fn ac16_search_incremental_enter_wrap_esc() {
    let env = Env::new();
    let f = env.write("s.txt", "foo bar baz\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    p.send_str("b");
    p.send_str("a");
    p.wait_for("Search: ba", TIMEOUT);
    wait_status(&p, "Ln 1, Col 5"); // bar
    assert_eq!(p.cursor(), (0, 10));
    p.send_keys(&["Enter"]);
    wait_status(&p, "Ln 1, Col 9"); // baz
    p.send_keys(&["Enter"]);
    wait_status(&p, "Ln 1, Col 5"); // wrapped to bar
    assert!(p.screen().contains("Search: ba"), "prompt stays open");
    p.send_keys(&["Esc"]);
    p.wait_gone("Search:", TIMEOUT);
    wait_status(&p, "Ln 1, Col 5");
    assert!(status(&p).contains(" INSERT "));
    quit_clean(&mut p);
}

#[test]
fn ac16_esc_slash_no_match_restores_origin() {
    let env = Env::new();
    let f = env.write("s.txt", "foo bar baz\n");
    let mut p = env.open(&f);
    p.send_keys(&["Right", "Right"]);
    wait_status(&p, "Ln 1, Col 3");
    open_overlay(&mut p);
    p.send_keys(&["/"]);
    p.wait_for("Search:", TIMEOUT);
    p.send_str("b");
    wait_status(&p, "Ln 1, Col 5");
    p.send_str("q");
    p.wait_for("No match", TIMEOUT);
    wait_status(&p, "Ln 1, Col 3");
    p.send_keys(&["Esc"]);
    p.wait_gone("Search:", TIMEOUT);
    wait_status(&p, "Ln 1, Col 3");
    quit_clean(&mut p);
}

#[test]
fn ac16_empty_query_does_nothing() {
    let env = Env::new();
    let f = env.write("s.txt", "foo bar baz\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    p.send_keys(&["Enter"]);
    std::thread::sleep(Duration::from_millis(50));
    wait_status(&p, "Ln 1, Col 1");
    p.wait_for("Search:", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Search:", TIMEOUT);
    wait_status(&p, "Ln 1, Col 1");
    assert!(!status(&p).contains("[+]"));
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- AC-17

const COMMANDS: [&str; 36] = [
    "Save",
    "Save as",
    "Open file",
    "Quit",
    "Quit without saving",
    "Undo",
    "Redo",
    "Cut",
    "Copy",
    "Paste",
    "Select all",
    "Find",
    "Replace",
    "Go to line",
    "Toggle line numbers",
    "Toggle tabs and spaces",
    "Toggle soft wrap",
    "Choose theme",
    "File tree",
    "Insert citation",
    "Build PDF",
    "Stop build",
    "Show build problems",
    "Open PDF",
    "Fix spelling",
    "Next misspelling",
    "Toggle spell check",
    "Open files",
    "Next file",
    "Previous file",
    "Close file",
    "File history",
    "Delete file history",
    "Reader mode",
    "Open settings",
    "Show key help",
];

fn open_palette(p: &mut Pty) {
    open_overlay(p);
    p.send_keys(&["Esc"]);
    p.wait_for("Command palette", TIMEOUT);
}

/// Row text of the first palette result (below the `> ` input line and its rule).
fn first_result(p: &Pty) -> String {
    let rows = p.rows();
    let i = rows
        .iter()
        .position(|r| r.contains("│> "))
        .expect("palette input row");
    rows[i + 1..]
        .iter()
        .find(|r| !r.contains('├'))
        .expect("palette result row")
        .clone()
}

#[test]
fn ac17_palette_lists_every_command() {
    let env = Env::new();
    let f = env.write("p.txt", "abc\n");
    let mut p = env.open(&f);
    open_palette(&mut p);
    let mut seen: Vec<&str> = COMMANDS
        .iter()
        .copied()
        .filter(|c| p.screen().contains(c))
        .collect();
    assert!(first_result(&p).contains("▸ Save"), "{}", p.dump());
    // 8 rows are visible at once; scrolling down reveals the rest
    for _ in 0..35 {
        p.send_keys(&["Down"]);
        std::thread::sleep(Duration::from_millis(5));
        for c in COMMANDS {
            if p.screen().contains(c) && !seen.contains(&c) {
                seen.push(c);
            }
        }
    }
    p.wait_for("▸ Show key help", TIMEOUT);
    for c in COMMANDS {
        if p.screen().contains(c) && !seen.contains(&c) {
            seen.push(c);
        }
    }
    assert_eq!(seen.len(), COMMANDS.len(), "saw {seen:?}");
    p.send_keys(&["Esc"]);
    p.wait_gone("Command palette", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn ac17_sav_puts_save_first_and_enter_saves() {
    let env = Env::new();
    let f = env.write("p.txt", "abc\n");
    let mut p = env.open(&f);
    type_text(&mut p, "X");
    open_palette(&mut p);
    p.send_str("sav");
    p.wait_for("> sav", TIMEOUT);
    assert!(first_result(&p).contains("▸ Save"), "{}", p.dump());
    p.send_keys(&["Enter"]);
    p.wait_for("Saved 1 line", TIMEOUT);
    assert!(!status(&p).contains("[+]"));
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "Xabc\n");
}

#[test]
fn ac17_go_to_line_prompt() {
    let env = Env::new();
    let text: String = (1..=12).map(|i| format!("line {i}\n")).collect();
    let f = env.write("g.txt", &text);
    let mut p = env.open(&f);
    for query in ["goto", "go to"] {
        open_palette(&mut p);
        p.send_str(query);
        p.wait_for(&format!("> {query}"), TIMEOUT);
        assert!(first_result(&p).contains("▸ Go to line"), "{}", p.dump());
        p.send_keys(&["Enter"]);
        p.wait_for("Go to line:", TIMEOUT);
        p.send_str("9");
        p.send_keys(&["Enter"]);
        p.wait_gone("Go to line:", TIMEOUT);
        wait_status(&p, "Ln 9, Col 1");
        p.send_keys(&["C-Home"]);
        wait_status(&p, "Ln 1, Col 1");
    }
    // out of range clamps to the last line
    open_palette(&mut p);
    p.send_str("goto");
    p.send_keys(&["Enter"]);
    p.wait_for("Go to line:", TIMEOUT);
    p.send_str("999");
    p.send_keys(&["Enter"]);
    p.wait_gone("Go to line:", TIMEOUT);
    wait_status(&p, "Ln 13, Col 1");
    // non-numeric input shows an error and stays open
    open_palette(&mut p);
    p.send_str("goto");
    p.send_keys(&["Enter"]);
    p.wait_for("Go to line:", TIMEOUT);
    p.send_str("abc");
    p.send_keys(&["Enter"]);
    p.wait_for("Not a line number", TIMEOUT);
    p.wait_for("Go to line: abc", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Go to line:", TIMEOUT);
    wait_status(&p, "Ln 13, Col 1");
    assert!(!status(&p).contains("[+]"));
    quit_clean(&mut p);
}

#[test]
fn ac17_esc_closes_without_running() {
    let env = Env::new();
    let f = env.write("p.txt", "abc\n");
    let mut p = env.open(&f);
    type_text(&mut p, "X");
    open_palette(&mut p);
    p.send_str("sav");
    p.wait_for("> sav", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Command palette", TIMEOUT);
    std::thread::sleep(Duration::from_millis(50));
    assert!(status(&p).contains("[+]"), "Save must not have run");
    assert!(!p.screen().contains("Saved"));
    assert_eq!(fs::read_to_string(&f).unwrap(), "abc\n");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    p.wait_exit(TIMEOUT);
}

#[test]
fn ac17_toggle_line_numbers_and_key_help() {
    let env = Env::new();
    let f = env.write("p.txt", "abc\n");
    let mut p = env.open(&f);
    assert_eq!(p.row(0), "  1 │ abc");
    open_palette(&mut p);
    p.send_str("toggle");
    p.send_keys(&["Enter"]);
    p.wait_until("gutter hidden", TIMEOUT, |_| p.row(0) == "abc");
    open_palette(&mut p);
    p.send_str("toggle");
    p.send_keys(&["Enter"]);
    p.wait_until("gutter shown", TIMEOUT, |_| p.row(0) == "  1 │ abc");
    open_palette(&mut p);
    p.send_str("help");
    p.send_keys(&["Enter"]);
    p.wait_for("Kyte keys", TIMEOUT);
    p.wait_for("del word, del line, del to end", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Kyte keys", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn ac17_quit_without_saving_command() {
    let env = Env::new();
    let f = env.write("p.txt", "abc\n");
    let mut p = env.open(&f);
    type_text(&mut p, "junk");
    open_palette(&mut p);
    p.send_str("without");
    p.send_keys(&["Enter"]);
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), "abc\n");
}

// ---------------------------------------------------------------- swap helpers

/// Runs `kyte --print-swap FILE` without a tty; returns (exit code, stdout).
fn print_swap(env: &Env, f: &Path) -> (i32, String) {
    let out = Command::new(bin())
        .arg("--print-swap")
        .arg(f)
        .env("KYTE_STATE_DIR", &env.state)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn swap_files(env: &Env) -> Vec<PathBuf> {
    fs::read_dir(&env.state)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".kyteswp"))
        .collect()
}

/// A work directory separate from the state directory.
fn work_file(env: &Env, name: &str, text: &str) -> PathBuf {
    let dir = env.path("work");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, text).unwrap();
    p
}

fn dir_names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------- AC-19

#[test]
fn ac19_swap_exists_0600_outside_file_dir_and_recreated_after_save() {
    let env = Env::new();
    let f = work_file(&env, "doc.txt", "orig\n");
    let mut p = env.open(&f);
    type_text(&mut p, "abc");
    let swaps = swap_files(&env);
    assert_eq!(swaps.len(), 1, "one swap under KYTE_STATE_DIR");
    let mode = fs::metadata(&swaps[0]).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        dir_names(f.parent().unwrap()),
        vec!["doc.txt".to_string()],
        "nothing next to the file"
    );
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "orig\n",
        "real file untouched before Ctrl-S"
    );
    assert_eq!(print_swap(&env, &f), (0, "abcorig\n".to_string()));
    save_and_wait(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "abcorig\n");
    let swaps = swap_files(&env);
    assert_eq!(swaps.len(), 1, "swap recreated after save");
    assert_eq!(
        kyte::swap::record_count(&swaps[0]).unwrap(),
        0,
        "fresh swap has no edits"
    );
    assert_eq!(print_swap(&env, &f), (0, "abcorig\n".to_string()));
    quit_clean(&mut p);
    assert!(swap_files(&env).is_empty(), "clean quit removes the swap");
    assert_eq!(print_swap(&env, &f).0, 3);
}

#[test]
fn ac19_discard_quit_removes_swap_and_print_swap_exits_3() {
    let env = Env::new();
    let f = work_file(&env, "doc.txt", "orig\n");
    let mut p = env.open(&f);
    type_text(&mut p, "zzz");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert!(swap_files(&env).is_empty());
    assert_eq!(print_swap(&env, &f).0, 3);
    assert_eq!(fs::read_to_string(&f).unwrap(), "orig\n");
}

// ---------------------------------------------------------------- AC-23

#[test]
fn ac23_second_instance_exits_2_naming_pid_and_leaves_swap() {
    let env = Env::new();
    let f = work_file(&env, "lock.txt", "shared\n");
    let mut a = env.open(&f);
    a.send_str("edit ");
    a.wait_for("edit shared", TIMEOUT);
    let swap = swap_files(&env).pop().unwrap();
    let before = fs::read(&swap).unwrap();
    let pid = a.pid();
    let mut b = Pty::kyte(&bin(), &env.state, &[f.to_str().unwrap()]);
    assert_eq!(b.wait_exit(TIMEOUT), 2);
    let msg = b.screen();
    assert!(
        msg.contains(&format!("PID {pid}")),
        "message must name PID {pid}: {msg}"
    );
    assert_eq!(fs::read(&swap).unwrap(), before, "swap untouched");
    // the first instance still works
    a.send_str("more");
    a.wait_for("edit more", TIMEOUT);
    a.send_keys(&["C-q"]);
    a.wait_for("Unsaved changes", TIMEOUT);
    a.send_str("n");
    assert_eq!(a.wait_exit(TIMEOUT), 0);
}

// ---------------------------------------------------------------- crash helpers

/// Opens `f`, types `text`, waits 100 ms and SIGKILLs kyte.
fn edit_and_kill(env: &Env, f: &Path, text: &str) {
    let mut p = env.open(f);
    p.send_str(text);
    p.wait_for(text, TIMEOUT);
    wait_status(&p, "[+]");
    std::thread::sleep(Duration::from_millis(100));
    p.kill9();
}

/// Starts kyte on a file with a swap, without waiting for the insert screen.
fn open_raw(env: &Env, f: &Path) -> Pty {
    Pty::kyte(&bin(), &env.state, &[f.to_str().unwrap()])
}

// ---------------------------------------------------------------- AC-20

#[test]
fn ac20_kill9_then_print_swap_and_recover_r() {
    let env = Env::new();
    let f = work_file(&env, "crash.txt", "line one\nline two\n");
    edit_and_kill(&env, &f, "unsaved text");
    assert_eq!(fs::read_to_string(&f).unwrap(), "line one\nline two\n");
    assert_eq!(
        print_swap(&env, &f),
        (0, "unsaved textline one\nline two\n".to_string())
    );
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.wait_for("crash.txt", TIMEOUT);
    p.wait_for("r  Recover the unsaved changes", TIMEOUT);
    p.wait_for("Swap file written", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ unsaved textline one", TIMEOUT);
    wait_status(&p, "crash.txt [+]");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "unsaved textline one\nline two\n"
    );
    assert_eq!(print_swap(&env, &f).0, 3, "clean exit leaves no swap");
}

#[test]
fn ac20_recover_then_more_edits_survive_a_second_crash() {
    let env = Env::new();
    let f = work_file(&env, "crash.txt", "base\n");
    edit_and_kill(&env, &f, "one ");
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("one base", TIMEOUT);
    p.send_keys(&["End"]);
    p.send_str("two");
    p.wait_for("one basetwo", TIMEOUT);
    std::thread::sleep(Duration::from_millis(100));
    p.kill9();
    assert_eq!(print_swap(&env, &f), (0, "one basetwo\n".to_string()));
}

#[test]
fn ac20_recover_d_discards_and_opens_original() {
    let env = Env::new();
    let f = work_file(&env, "crash.txt", "original\n");
    edit_and_kill(&env, &f, "unsaved text");
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("d");
    p.wait_for("1 │ original", TIMEOUT);
    assert!(!p.screen().contains("unsaved text"));
    assert!(!status(&p).contains("[+]"));
    assert_eq!(print_swap(&env, &f), (0, "original\n".to_string()));
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "original\n");
}

#[test]
fn ac20_recover_q_quits_without_touching_anything() {
    let env = Env::new();
    let f = work_file(&env, "crash.txt", "original\n");
    edit_and_kill(&env, &f, "unsaved text");
    let swap = swap_files(&env).pop().unwrap();
    let before = fs::read(&swap).unwrap();
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("q");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read(&swap).unwrap(), before);
    assert_eq!(fs::read_to_string(&f).unwrap(), "original\n");
}

// ---------------------------------------------------------------- AC-21

fn torn_case(damage: impl Fn(&Path)) {
    let env = Env::new();
    let f = work_file(&env, "torn.txt", "base\n");
    // each typed key is its own record
    edit_and_kill(&env, &f, "abc");
    let swap = swap_files(&env).pop().unwrap();
    damage(&swap);
    assert_eq!(print_swap(&env, &f), (0, "abbase\n".to_string()));
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ abbase", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "abbase\n");
}

#[test]
fn ac21_torn_tail_truncated_3_bytes() {
    torn_case(|swap| {
        let len = fs::metadata(swap).unwrap().len();
        fs::OpenOptions::new()
            .write(true)
            .open(swap)
            .unwrap()
            .set_len(len - 3)
            .unwrap();
    });
}

#[test]
fn ac21_torn_tail_last_byte_flipped() {
    torn_case(|swap| {
        let mut b = fs::read(swap).unwrap();
        let last = b.len() - 1;
        b[last] ^= 0xff;
        fs::write(swap, b).unwrap();
    });
}

// ---------------------------------------------------------------- AC-22

#[test]
fn ac22_edits_survive_a_file_that_changed_after_the_crash() {
    let env = Env::new();
    let f = work_file(&env, "stale.txt", "version 1\n");
    edit_and_kill(&env, &f, "kept edit ");
    fs::write(&f, "version 2 from elsewhere\n").unwrap();
    let (code, text) = print_swap(&env, &f);
    assert_eq!(code, 0);
    assert_eq!(text, "kept edit version 1\n");
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.wait_for("changed on disk", TIMEOUT);
    p.send_str("q");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "version 2 from elsewhere\n"
    );
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ kept edit version 1", TIMEOUT);
    wait_status(&p, "[+]");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn ac22_discarding_recovered_edits_keeps_the_changed_file() {
    let env = Env::new();
    let f = work_file(&env, "stale.txt", "version 1\n");
    edit_and_kill(&env, &f, "lost edit ");
    fs::write(&f, "version 2 from elsewhere\n").unwrap();
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("d");
    p.wait_for("1 │ version 2 from elsewhere", TIMEOUT);
    assert!(!p.screen().contains("lost edit"));
    quit_clean(&mut p);
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "version 2 from elsewhere\n"
    );
}

#[test]
fn ac22_unreadable_swap_offers_only_discard_or_quit() {
    let env = Env::new();
    let f = work_file(&env, "stale.txt", "version 1\n");
    edit_and_kill(&env, &f, "lost edit ");
    let swap = swap_files(&env).pop().unwrap();
    let mut bytes = fs::read(&swap).unwrap();
    bytes[0] ^= 0xff;
    fs::write(&swap, &bytes).unwrap();
    let mut p = open_raw(&env, &f);
    p.wait_for("Stale swap file", TIMEOUT);
    let s = p.screen();
    assert!(s.contains("d  Discard the stale swap"));
    assert!(s.contains("q  Quit"));
    assert!(!s.contains("r  Recover"));
    // r is not offered and does nothing
    p.send_str("r");
    std::thread::sleep(Duration::from_millis(100));
    p.wait_for("Stale swap file", TIMEOUT);
    p.send_str("q");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(
        fs::read(&swap).unwrap(),
        bytes,
        "q leaves the swap as it was"
    );
    assert_eq!(print_swap(&env, &f).0, 3, "an unreadable swap is not valid");
    let mut p = open_raw(&env, &f);
    p.wait_for("Stale swap file", TIMEOUT);
    p.send_str("d");
    p.wait_for("1 │ version 1", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn file_deleted_after_the_crash_is_still_recovered() {
    let env = Env::new();
    let f = work_file(&env, "gone.txt", "text\n");
    edit_and_kill(&env, &f, "more ");
    fs::remove_file(&f).unwrap();
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ more text", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "more text\n");
}

#[test]
fn long_path_opens_with_working_recovery() {
    let env = Env::new();
    let mut dir = env.dir.path().to_path_buf();
    for _ in 0..12 {
        dir.push("a-rather-long-directory-name");
    }
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("deep.txt");
    fs::write(&f, "deep\n").unwrap();
    assert!(f.to_str().unwrap().len() > 255);
    edit_and_kill(&env, &f, "saved by the swap ");
    let (code, text) = print_swap(&env, &f);
    assert_eq!(code, 0);
    assert_eq!(text, "saved by the swap deep\n");
}

#[test]
fn break_lock_opens_a_file_held_by_another_kyte() {
    let env = Env::new();
    let f = work_file(&env, "held.txt", "base\n");
    let mut a = env.open(&f);
    type_text(&mut a, "held ");
    let path = f.to_str().unwrap();
    let mut refused = Pty::kyte(&bin(), &env.state, &[path]);
    assert_eq!(refused.wait_exit(TIMEOUT), 2);
    assert!(refused.screen().contains("--break-lock"));
    let mut b = Pty::kyte(&bin(), &env.state, &["--break-lock", path]);
    b.wait_for("Recover unsaved changes", TIMEOUT);
    a.kill9();
    b.send_str("r");
    b.wait_for("1 │ held base", TIMEOUT);
    b.kill9();
}

// ---------------------------------------------------------------- outside changes

const MARKER: &str = "changed on disk";

fn start_review(p: &mut Pty) {
    p.wait_for(MARKER, TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("review outside changes", TIMEOUT);
    p.send_str("c");
    p.wait_for(" REVIEW ", TIMEOUT);
}

#[test]
fn outside_change_is_noticed_without_interrupting() {
    let env = Env::new();
    let f = work_file(&env, "o.txt", "one\ntwo\n");
    let mut p = env.open(&f);
    fs::write(&f, "one\nTWO\n").unwrap();
    p.wait_for(MARKER, TIMEOUT);
    p.send_str("still typing ");
    p.wait_for("still typing one", TIMEOUT);
    assert!(status(&p).contains("INSERT"));
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn taking_every_change_on_a_clean_buffer_matches_the_file() {
    let env = Env::new();
    let f = work_file(&env, "o.txt", "one\ntwo\nthree\nfour\n");
    let mut p = env.open(&f);
    fs::write(&f, "ONE\ntwo\nthree\nFOUR\n").unwrap();
    start_review(&mut p);
    wait_status(&p, "change 1 of 2");
    p.wait_for("Take this change", TIMEOUT);
    p.send_str("y");
    wait_status(&p, "change 2 of 2");
    p.send_str("y");
    p.wait_for("Took 2 of 2 changes", TIMEOUT);
    p.wait_for("1 │ ONE", TIMEOUT);
    assert!(!status(&p).contains("[+]"), "the buffer matches the file");
    assert!(!status(&p).contains(MARKER));
    quit_clean(&mut p);
}

#[test]
fn your_unsaved_edits_and_their_changes_both_survive() {
    let env = Env::new();
    let f = work_file(&env, "o.txt", "a\nb\nc\nd\ne\nf\n");
    let mut p = env.open(&f);
    type_text(&mut p, "mine ");
    fs::write(&f, "a\nb\nc\nd\ne\nTHEIRS\n").unwrap();
    start_review(&mut p);
    wait_status(&p, "change 1 of 1");
    p.send_str("y");
    p.wait_for("Took 1 of 1 changes", TIMEOUT);
    wait_status(&p, "[+]");
    p.send_keys(&["C-s"]);
    p.wait_for("Saved 6 lines", TIMEOUT);
    quit_clean(&mut p);
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "mine a\nb\nc\nd\ne\nTHEIRS\n"
    );
}

#[test]
fn keeping_yours_saves_without_asking_again_and_history_has_theirs() {
    let env = Env::new();
    let f = work_file(&env, "o.txt", "one\n");
    let mut p = env.open(&f);
    fs::write(&f, "theirs\n").unwrap();
    start_review(&mut p);
    p.send_str("n");
    p.wait_for("Took 0 of 1 changes", TIMEOUT);
    p.wait_for("1 │ one", TIMEOUT);
    type_text(&mut p, "x");
    p.send_keys(&["C-s"]);
    p.wait_for("Saved 1 line", TIMEOUT);
    assert_eq!(fs::read_to_string(&f).unwrap(), "xone\n");
    open_history(&mut p);
    p.send_keys(&["Home"]);
    p.wait_for("1 │ one", TIMEOUT);
    p.send_keys(&["Right"]);
    p.wait_for("changed outside Kyte", TIMEOUT);
    p.wait_for("1 │ theirs", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone(" HISTORY ", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn the_same_lines_changed_on_both_sides_is_flagged() {
    let env = Env::new();
    let f = work_file(&env, "o.txt", "one\n");
    let mut p = env.open(&f);
    type_text(&mut p, "mine ");
    fs::write(&f, "theirs\n").unwrap();
    start_review(&mut p);
    p.wait_for("you edited this too", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("Review stopped", TIMEOUT);
    assert!(status(&p).contains(MARKER), "it stays until it's answered");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn take_all_is_one_undo_step() {
    let env = Env::new();
    let f = work_file(&env, "o.txt", "1\n2\n3\n4\n5\n");
    let mut p = env.open(&f);
    fs::write(&f, "A\n2\n3\n4\nE\n").unwrap();
    start_review(&mut p);
    p.send_str("a");
    p.wait_for("Took 2 of 2 changes", TIMEOUT);
    p.wait_for("5 │ E", TIMEOUT);
    p.send_keys(&["C-z"]);
    p.wait_for("1 │ 1", TIMEOUT);
    p.wait_for("5 │ 5", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn the_save_question_can_open_the_review() {
    let env = Env::new();
    let f = work_file(&env, "o.txt", "one\n");
    let mut p = env.open(&f);
    type_text(&mut p, "x");
    fs::write(&f, "theirs\n").unwrap();
    p.send_keys(&["C-s"]);
    p.wait_for("Review the changes", TIMEOUT);
    p.send_str("r");
    p.wait_for(" REVIEW ", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("Review stopped", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn saving_your_own_changes_raises_no_marker() {
    let env = Env::new();
    let f = work_file(&env, "o.txt", "one\n");
    let mut p = env.open(&f);
    type_text(&mut p, "x");
    save_and_wait(&mut p);
    std::thread::sleep(Duration::from_millis(2300));
    assert!(!status(&p).contains(MARKER), "{}", p.dump());
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- reader mode

fn open_reader(p: &mut Pty) {
    open_overlay(p);
    p.send_str("v");
    p.wait_for(" READER ", TIMEOUT);
}

#[test]
fn reader_dims_what_is_out_of_focus_and_changes_nothing() {
    let env = Env::new();
    let f = env.write("r.txt", "first para\nstill first\n\nsecond para\n");
    let mut p = env.open(&f);
    let plain = p.fg(0, 6);
    open_reader(&mut p);
    p.wait_for("back to editing", TIMEOUT);
    assert_eq!(p.fg(0, 6), plain, "the paragraph in focus is unchanged");
    assert_ne!(p.fg(3, 6), plain, "the next one is dimmed");
    p.send_str("xyz");
    p.send_keys(&["Down", "Down"]);
    p.wait_gone("back to editing", TIMEOUT);
    wait_status(&p, "Ln 3");
    assert_ne!(p.fg(0, 6), plain, "the focus moved on");
    assert_eq!(p.fg(3, 6), plain, "and the page didn't");
    p.send_keys(&["Esc"]);
    p.wait_gone(" READER ", TIMEOUT);
    wait_status(&p, "INSERT");
    assert_eq!(p.fg(0, 6), plain, "nothing stays dimmed");
    assert!(!status(&p).contains("[+]"));
    quit_clean(&mut p);
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "first para\nstill first\n\nsecond para\n"
    );
}

#[test]
fn reader_steps_through_sections_and_shows_keys_on_request() {
    let env = Env::new();
    let f = env.write("r.txt", "one\n\ntwo\n\nthree\n");
    let mut p = env.open(&f);
    open_reader(&mut p);
    p.send_str("n");
    wait_status(&p, "Ln 3");
    p.send_str("n");
    wait_status(&p, "Ln 5");
    p.send_str("n");
    p.wait_for("No more sections", TIMEOUT);
    p.send_str("?");
    p.wait_for("back to editing", TIMEOUT);
    p.send_str("q");
    p.wait_gone(" READER ", TIMEOUT);
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- floating menus

/// The last row, where the message line is.
fn footer(p: &Pty) -> String {
    p.row(29)
}

#[test]
fn choices_float_and_the_footer_stays_clear() {
    let env = Env::new();
    let f = env.write("m.txt", "one two\n");
    let mut p = env.open(&f);
    p.send_keys(&["Esc"]);
    p.wait_for("w del word", TIMEOUT);
    assert!(!footer(&p).contains("del word"), "{}", p.dump());
    p.send_str("z");
    wait_insert(&p);
    type_text(&mut p, "x");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.wait_for("Save and quit", TIMEOUT);
    assert!(!footer(&p).contains("Save and quit"), "{}", p.dump());
    p.send_keys(&["Esc"]);
    p.wait_gone("Unsaved changes", TIMEOUT);
    p.send_keys(&["C-r"]);
    p.wait_for("Replace:", TIMEOUT);
    p.send_str("two");
    p.send_keys(&["Enter"]);
    p.wait_for("With:", TIMEOUT);
    p.send_str("2");
    p.send_keys(&["Enter"]);
    p.wait_for("Replace all the rest", TIMEOUT);
    assert!(!footer(&p).contains("Replace"), "{}", p.dump());
    p.send_keys(&["Esc"]);
    p.wait_for("No matches replaced", TIMEOUT);
    open_overlay(&mut p);
    p.send_str("h");
    p.wait_for("restore this", TIMEOUT);
    assert!(!footer(&p).contains("restore"), "{}", p.dump());
    p.send_keys(&["Esc"]);
    p.wait_gone("restore this", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn tab_shows_completions_in_a_floating_list() {
    let env = Env::new();
    let f = env.write("a.txt", "x\n");
    let one = env.write("note-one.txt", "");
    env.write("note-two.txt", "");
    let stem = one
        .to_str()
        .unwrap()
        .strip_suffix("-one.txt")
        .unwrap()
        .to_string();
    let mut p = env.open(&f);
    p.send_keys(&["C-o"]);
    p.wait_for("Open:", TIMEOUT);
    for _ in 0..500 {
        p.send_keys(&["Backspace"]);
    }
    p.wait_until("the prompt to empty", TIMEOUT, |s| {
        s.lines().any(|l| l.trim_end() == " Open:")
    });
    p.send_str(&stem);
    p.send_keys(&["Tab"]);
    p.wait_for("note-two.txt", TIMEOUT);
    p.wait_for("Matches", TIMEOUT);
    assert!(!footer(&p).contains("note-two"), "{}", p.dump());
    p.send_str("t");
    p.wait_gone("Matches", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- soft wrap

fn open_wrapped(env: &Env, file: &Path) -> Pty {
    open_with_settings(env, file, "soft_wrap = true\n", &[])
}

#[test]
fn a_long_line_wraps_and_stays_one_line_in_the_file() {
    let env = Env::new();
    let text = format!("{}\nnext\n", "word ".repeat(40));
    let f = env.write("w.txt", &text);
    let mut p = open_wrapped(&env, &f);
    p.wait_for("word", TIMEOUT);
    p.wait_for("2 │ next", TIMEOUT);
    assert!(
        p.row(1).contains("word"),
        "the line continues: {}",
        p.dump()
    );
    assert!(p.row(1).trim_start().starts_with("│"), "no number on it");
    p.send_keys(&["Down"]);
    wait_status(&p, "Ln 1");
    p.send_keys(&["End"]);
    wait_status(&p, "Col 201");
    type_text(&mut p, "!");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        format!("{}!\nnext\n", "word ".repeat(40))
    );
}

#[test]
fn nothing_scrolls_sideways_when_wrapped() {
    let env = Env::new();
    let f = env.write("w.txt", &"x".repeat(500));
    let mut p = open_wrapped(&env, &f);
    p.send_keys(&["C-End"]);
    wait_status(&p, "Col 501");
    let (row, col) = p.cursor();
    assert!(row < 28 && col < 100, "cursor on screen at {row},{col}");
    p.send_keys(&["C-Home"]);
    wait_status(&p, "Col 1");
    p.wait_for("1 │ xxx", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn selection_and_search_show_on_wrapped_rows() {
    let env = Env::new();
    let f = env.write("w.txt", &format!("{}needle\n", "a".repeat(120)));
    let mut p = open_wrapped(&env, &f);
    let plain = p.bg(0, 10);
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    p.send_str("needle");
    p.wait_for("Search: needle", TIMEOUT);
    let row: String = p.row(1);
    let col = row.find("needle").expect("the match is on the second row");
    let col = row[..col].chars().count() as u16;
    assert_ne!(p.bg(1, col), plain, "{}", p.dump());
    assert_eq!(p.bg(0, 10), plain, "the first row has no match");
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    p.send_keys(&["C-a"]);
    wait_status(&p, "selected");
    assert_ne!(p.bg(0, 10), plain, "a selection covers every row");
    assert_ne!(p.bg(1, col), plain);
    quit_clean_after_selection(&mut p);
}

fn quit_clean_after_selection(p: &mut Pty) {
    p.send_keys(&["Right"]);
    p.wait_until("the selection to end", TIMEOUT, |s| !s.contains("selected"));
    quit_clean(p);
}

#[test]
fn soft_wrap_can_be_toggled_from_the_palette() {
    let env = Env::new();
    let f = env.write("w.txt", &"y".repeat(150));
    let mut p = env.open(&f);
    assert!(!p.row(1).contains("yyy"), "off by default");
    open_palette(&mut p);
    p.send_str("soft wrap");
    p.wait_for("▸ Toggle soft wrap", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_until("the line to wrap", TIMEOUT, |s| {
        s.lines().nth(1).is_some_and(|l| l.contains("yyy"))
    });
    quit_clean(&mut p);
}

#[test]
fn resizing_rewraps() {
    let env = Env::new();
    let f = env.write("w.txt", &"z".repeat(300));
    let mut p = open_wrapped(&env, &f);
    p.send_keys(&["C-End"]);
    wait_status(&p, "Col 301");
    p.resize(10, 40);
    p.wait_until("status bar on row 8 of 10", TIMEOUT, |_| {
        p.row(8).contains("INSERT")
    });
    let (r, c) = p.cursor();
    assert!(r < 8 && c < 40, "cursor {r},{c}\n{}", p.dump());
    p.resize(30, 100);
    p.wait_until("status bar on row 28 of 30", TIMEOUT, |_| {
        p.row(28).contains("INSERT")
    });
    let (r, c) = p.cursor();
    assert!(r < 28 && c < 100, "cursor {r},{c}\n{}", p.dump());
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- settings

/// Kyte on `file` with a settings file holding `settings`.
fn open_with_settings(env: &Env, file: &Path, settings: &str, extra: &[&str]) -> Pty {
    let config = env.path("config.toml");
    fs::write(&config, format!("{settings}writing = false\n")).unwrap();
    let mut spawn = Spawn::new(bin())
        .env("KYTE_STATE_DIR", env.state.to_string_lossy())
        .env("KYTE_CONFIG", config.to_string_lossy())
        .arg("--no-kitty");
    for a in extra {
        spawn = spawn.arg(a);
    }
    let pty = Pty::spawn(spawn.arg(file.to_str().unwrap()));
    pty.wait_for("Ln 1, Col 1", TIMEOUT);
    pty
}

/// Waits until the text area shows `color`, since a frame can arrive in parts.
fn wait_for_bg(p: &Pty, color: vt100::Color) {
    let deadline = Instant::now() + TIMEOUT;
    while p.bg(5, 2) != Some(color) {
        assert!(Instant::now() < deadline, "{color:?}\n{}", p.dump());
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_theme_setting_and_picker_change_the_colors() {
    let harbor = vt100::Color::Rgb(0x0f, 0x1f, 0x2c);
    let dusk = vt100::Color::Rgb(0x1b, 0x1d, 0x2b);
    let env = Env::new();
    let f = env.write("colors.txt", "text\n");
    let config = env.dir.path().join("config.toml");
    fs::write(&config, "theme = \"harbor\"\n").unwrap();
    let mut p = Pty::spawn(
        Spawn::new(bin())
            .env("KYTE_STATE_DIR", env.state.to_string_lossy())
            .env("KYTE_CONFIG", config.to_string_lossy())
            .env("COLORTERM", "truecolor")
            .arg("--no-kitty")
            .arg(f.to_string_lossy()),
    );
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    assert_eq!(p.bg(5, 2), Some(harbor), "{}", p.dump());
    p.send_keys(&["C-p"]);
    p.wait_for("Command palette", TIMEOUT);
    p.send_str("choose theme");
    p.wait_for("Choose theme", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Daybreak", TIMEOUT);
    p.send_keys(&["Down"]);
    wait_for_bg(&p, dusk);
    p.send_keys(&["Esc"]);
    wait_for_bg(&p, harbor);
    quit_clean(&mut p);
}

#[test]
fn settings_change_how_kyte_behaves() {
    let env = Env::new();
    let f = env.write("s.txt", "a\tb\n");
    let settings = "tab_width = 8\nline_numbers = false\n";
    let mut p = open_with_settings(&env, &f, settings, &[]);
    p.wait_for("a       b", TIMEOUT);
    assert!(!p.screen().contains("1 │"), "{}", p.dump());
    quit_clean(&mut p);
}

#[test]
fn default_indent_applies_only_to_files_with_no_indent_of_their_own() {
    let env = Env::new();
    let settings = "indent = \"spaces\"\nindent_width = 2\n";
    let plain = env.write("plain.txt", "x\n");
    let mut p = open_with_settings(&env, &plain, settings, &[]);
    wait_status(&p, "Spaces 2");
    p.send_keys(&["Tab"]);
    wait_status(&p, "[+]");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&plain).unwrap(), "  x\n");
    let tabbed = env.write("tabbed.txt", "a\n\tb\n");
    let mut p = open_with_settings(&env, &tabbed, settings, &[]);
    assert!(!status(&p).contains("Spaces"), "{}", p.dump());
    quit_clean(&mut p);
}

#[test]
fn a_bad_settings_file_warns_and_still_opens() {
    let env = Env::new();
    let f = env.write("s.txt", "text\n");
    let mut p = open_with_settings(&env, &f, "tabwidth = 2\nesc_ms = nope\n", &[]);
    p.wait_for(
        "Settings: line 1: unknown setting tabwidth (and 1 more)",
        TIMEOUT,
    );
    p.wait_for("1 │ text", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn the_command_line_beats_the_settings_file() {
    let env = Env::new();
    let f = env.write("s.txt", "x\n");
    let mut p = open_with_settings(&env, &f, "esc_ms = 0\n", &["--esc-ms", "800"]);
    p.send_keys(&["Esc"]);
    p.wait_for(LEGEND_PART, TIMEOUT);
    std::thread::sleep(Duration::from_millis(400));
    p.send_keys(&["Esc"]);
    p.wait_for("Command palette", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Command palette", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn history_can_be_turned_off_in_settings() {
    let env = Env::new();
    let f = env.write("s.txt", "x\n");
    let mut p = open_with_settings(&env, &f, "history = false\n", &[]);
    type_text(&mut p, "secret");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert!(!env.state.join("history").exists());
}

#[test]
fn open_settings_creates_a_commented_template() {
    let env = Env::new();
    let f = env.write("s.txt", "x\n");
    let config = env.path("cfg/config.toml");
    let mut p = Pty::spawn(
        Spawn::new(bin())
            .env("KYTE_STATE_DIR", env.state.to_string_lossy())
            .env("KYTE_CONFIG", config.to_string_lossy())
            .arg("--no-kitty")
            .arg(f.to_str().unwrap()),
    );
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    open_palette(&mut p);
    p.send_str("open settings");
    p.wait_for("▸ Open settings", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Settings apply the next time", TIMEOUT);
    p.wait_for("Kyte settings", TIMEOUT);
    quit_clean(&mut p);
    let text = fs::read_to_string(&config).unwrap();
    assert!(text.contains("# tab_width = 4"));
}

// ---------------------------------------------------------------- 80x24

fn open_standard(env: &Env, f: &Path) -> Pty {
    let pty = Pty::spawn(
        Spawn::new(bin())
            .env("KYTE_STATE_DIR", env.state.to_string_lossy())
            .arg("--no-kitty")
            .arg(f.to_str().unwrap())
            .size(24, 80),
    );
    pty.wait_for("Ln 1, Col 1", TIMEOUT);
    pty
}

#[test]
fn standard_terminal_shows_the_legend_and_help_in_full() {
    let env = Env::new();
    let f = env.write("s.txt", "abc\n");
    let mut p = open_standard(&env, &f);
    p.send_keys(&["Esc"]);
    p.wait_for("Esc Esc palette", TIMEOUT);
    p.send_str("z");
    p.wait_gone("Esc Esc palette", TIMEOUT);
    p.send_keys(&["C-p"]);
    p.wait_for("Command palette", TIMEOUT);
    p.send_str("key help");
    p.wait_for("▸ Show key help", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Up, Down scroll", TIMEOUT);
    for _ in 0..35 {
        p.send_keys(&["Down"]);
    }
    p.wait_for("Esc (quickly)", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Kyte keys", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn failed_save_message_fits_a_standard_terminal() {
    let env = Env::new();
    if fs::metadata(env.dir.path()).unwrap().uid() == 0 {
        eprintln!("skipped: root can write to anything");
        return;
    }
    let dir = env.path("ro");
    fs::create_dir(&dir).unwrap();
    let f = dir.join("f.txt");
    fs::write(&f, "x\n").unwrap();
    let mut p = open_standard(&env, &f);
    p.send_str("y");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    p.send_keys(&["C-s"]);
    p.wait_for("Use Save as in the palette", TIMEOUT);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

// ---------------------------------------------------------------- file history

fn open_history(p: &mut Pty) {
    open_overlay(p);
    p.send_str("h");
    p.wait_for(" HISTORY ", TIMEOUT);
}

#[test]
fn history_steps_through_every_edit_and_restores_one() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "base\n");
    let mut p = env.open(&f);
    p.send_str("abc");
    p.wait_for("abcbase", TIMEOUT);
    save_and_wait(&mut p);
    open_history(&mut p);
    wait_status(&p, "4 of 4");
    wait_status(&p, "added 1, saved");
    p.send_keys(&["Left"]);
    wait_status(&p, "3 of 4");
    p.wait_for("1 │ abbase", TIMEOUT);
    p.send_keys(&["Home"]);
    wait_status(&p, "1 of 4");
    wait_status(&p, "as opened");
    p.wait_for("1 │ base", TIMEOUT);
    p.send_keys(&["Right"]);
    wait_status(&p, "2 of 4");
    p.wait_for("1 │ abase", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Restored the version from", TIMEOUT);
    wait_status(&p, "[+]");
    p.wait_for("1 │ abase", TIMEOUT);
    p.send_keys(&["C-z"]);
    p.wait_for("1 │ abcbase", TIMEOUT);
    p.send_keys(&["C-y"]);
    p.wait_for("1 │ abase", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "abase\n");
}

#[test]
fn history_browsing_is_read_only_and_esc_returns_to_your_text() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "base\n");
    let mut p = env.open(&f);
    type_text(&mut p, "x");
    open_history(&mut p);
    p.send_keys(&["Home"]);
    p.wait_for("1 │ base", TIMEOUT);
    p.send_str("typing here");
    std::thread::sleep(Duration::from_millis(50));
    assert!(!p.screen().contains("typing"), "{}", p.dump());
    p.send_keys(&["Esc"]);
    p.wait_gone(" HISTORY ", TIMEOUT);
    p.wait_for("1 │ xbase", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn history_survives_quitting_and_keeps_discarded_edits() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "base\n");
    let mut p = env.open(&f);
    type_text(&mut p, "kept ");
    save_and_wait(&mut p);
    p.send_str("thrown away ");
    p.wait_for("kept thrown away", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&f).unwrap(), "kept base\n");
    let mut p = env.open(&f);
    open_history(&mut p);
    p.wait_for("1 │ kept thrown away base", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Restored the version from", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "kept thrown away base\n");
}

#[test]
fn history_keeps_a_version_a_save_overwrote() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "v1\n");
    let mut p = env.open(&f);
    type_text(&mut p, "mine ");
    fs::write(&f, "theirs\n").unwrap();
    p.send_keys(&["C-s"]);
    p.wait_for("File changed on disk!", TIMEOUT);
    p.send_str("y");
    p.wait_for("Saved 1 line", TIMEOUT);
    assert_eq!(fs::read_to_string(&f).unwrap(), "mine v1\n");
    open_history(&mut p);
    p.send_keys(&["Left"]);
    wait_status(&p, "changed outside Kyte");
    p.wait_for("1 │ theirs", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("1 │ theirs", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "theirs\n");
}

#[test]
fn history_of_a_deleted_file_still_opens() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "precious\n");
    let mut p = env.open(&f);
    type_text(&mut p, "x");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    fs::remove_file(&f).unwrap();
    let mut p = env.open(&f);
    open_history(&mut p);
    p.wait_for("1 │ xprecious", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Restored the version from", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "xprecious\n");
}

#[test]
fn history_survives_a_crash_and_recovery() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "base\n");
    edit_and_kill(&env, &f, "crashed ");
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ crashed base", TIMEOUT);
    open_history(&mut p);
    wait_status(&p, "recovered");
    p.send_keys(&["Home"]);
    p.wait_for("1 │ base", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone(" HISTORY ", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn opening_without_editing_writes_no_history() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "base\n");
    let mut p = env.open(&f);
    open_overlay(&mut p);
    p.send_str("h");
    p.wait_for("No history yet", TIMEOUT);
    quit_clean(&mut p);
    assert!(!env.state.join("history").exists());
}

#[test]
fn no_history_flag_keeps_nothing() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "base\n");
    let mut p = env.open_args(&f, &["--no-history"]);
    type_text(&mut p, "secret");
    save_and_wait(&mut p);
    open_overlay(&mut p);
    p.send_str("h");
    p.wait_for("File history is off", TIMEOUT);
    quit_clean(&mut p);
    assert!(!env.state.join("history").exists());
}

#[test]
fn delete_file_history_asks_then_clears_it() {
    let env = Env::new();
    let f = work_file(&env, "h.txt", "base\n");
    let mut p = env.open(&f);
    type_text(&mut p, "x");
    save_and_wait(&mut p);
    open_palette(&mut p);
    p.send_str("delete file");
    p.wait_for("▸ Delete file history", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Delete all history", TIMEOUT);
    p.send_str("y");
    p.wait_for("Deleted this file's history", TIMEOUT);
    open_overlay(&mut p);
    p.send_str("h");
    p.wait_for("No history yet", TIMEOUT);
    quit_clean(&mut p);
    let left: Vec<_> = fs::read_dir(env.state.join("history")).unwrap().collect();
    assert!(left.is_empty());
}

#[test]
fn save_as_gives_the_new_file_its_own_history() {
    let env = Env::new();
    let f = work_file(&env, "a.txt", "text\n");
    let new = f.with_file_name("b.txt");
    let mut p = env.open(&f);
    save_as(&mut p, new.to_str().unwrap());
    p.wait_for("Saved 1 line", TIMEOUT);
    type_text(&mut p, "x");
    open_history(&mut p);
    wait_status(&p, "2 of 2");
    p.send_keys(&["Esc"]);
    p.wait_gone(" HISTORY ", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- universal palette

/// Kyte on `file`, with the working directory set to `cwd`.
fn open_in(env: &Env, cwd: &Path, file: &Path) -> Pty {
    let pty = Pty::spawn(
        Spawn::new(bin())
            .env("KYTE_STATE_DIR", env.state.to_string_lossy())
            .arg("--no-kitty")
            .arg(file.to_str().unwrap())
            .cwd(cwd),
    );
    pty.wait_for("Ln 1, Col 1", TIMEOUT);
    pty
}

#[test]
fn ctrl_k_opens_a_file_by_name_from_the_working_directory() {
    let env = Env::new();
    let project = env.path("proj");
    fs::create_dir_all(project.join("src")).unwrap();
    let a = project.join("a.txt");
    fs::write(&a, "first\n").unwrap();
    fs::write(project.join("src/widget.rs"), "fn widget() {}\n").unwrap();
    let mut p = open_in(&env, &project, &a);
    p.send_keys(&["C-k"]);
    p.wait_for("Commands, files, :line, /text", TIMEOUT);
    p.send_str("widget");
    p.wait_for("▸ src/widget.rs", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("1 │ fn widget() {}", TIMEOUT);
    wait_status(&p, "widget.rs");
    quit_clean(&mut p);
}

#[test]
fn ctrl_k_goes_to_a_line_and_finds_text() {
    let env = Env::new();
    let f = env.write("k.txt", "one\ntwo\nthree\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-k"]);
    p.wait_for("Commands, files", TIMEOUT);
    p.send_str(":3");
    p.wait_for("▸ Go to line 3", TIMEOUT);
    p.send_keys(&["Enter"]);
    wait_status(&p, "Ln 3, Col 1");
    p.send_keys(&["C-k"]);
    p.wait_for("Commands, files", TIMEOUT);
    p.send_str("/two");
    p.wait_for("▸ Find two", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Search: two", TIMEOUT);
    wait_status(&p, "Ln 2, Col 1");
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    quit_clean(&mut p);
}

#[test]
fn esc_esc_stays_the_command_palette() {
    let env = Env::new();
    let f = env.write("k.txt", "x\n");
    let mut p = env.open(&f);
    open_palette(&mut p);
    p.wait_for("Command palette", TIMEOUT);
    p.send_str(":3");
    p.wait_for("No match", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Command palette", TIMEOUT);
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- indent

#[test]
fn enter_and_tab_follow_the_file_indent() {
    let env = Env::new();
    let f = env.write("i.py", "def a():\n  if b:\n    c\n");
    let mut p = env.open(&f);
    wait_status(&p, "Spaces 2");
    p.send_keys(&["Down", "Down", "End", "Enter"]);
    wait_status(&p, "Ln 4, Col 5");
    p.send_str("d");
    p.send_keys(&["Enter", "Tab"]);
    wait_status(&p, "Ln 5, Col 7");
    p.send_str("e");
    p.send_keys(&["Enter", "Backspace"]);
    wait_status(&p, "Ln 6, Col 5");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "def a():\n  if b:\n    c\n    d\n      e\n    \n"
    );
}

#[test]
fn tab_and_shift_tab_move_selected_lines() {
    let env = Env::new();
    let f = env.write("i.txt", "a\nb\nc\n");
    let mut p = env.open(&f);
    p.send(SHIFT_DOWN);
    p.send(SHIFT_DOWN);
    wait_status(&p, "4 selected");
    p.send_keys(&["Tab"]);
    p.wait_for("2 │ ", TIMEOUT);
    wait_status(&p, "5 selected");
    save_and_wait(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "\ta\n\tb\nc\n");
    p.send(b"\x1b[Z");
    wait_status(&p, "4 selected");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "a\nb\nc\n");
}

// ---------------------------------------------------------------- opening files

/// Ctrl-O, then replaces the prefilled text with `path` and presses Enter.
fn open_from_prompt(p: &mut Pty, path: &str) {
    p.send_keys(&["C-o"]);
    p.wait_for("Open:", TIMEOUT);
    for _ in 0..500 {
        p.send_keys(&["Backspace"]);
    }
    p.wait_until("the prompt to empty", TIMEOUT, |s| {
        s.lines().any(|l| l.trim_end() == " Open:")
    });
    p.send_str(path);
    p.send_keys(&["Enter"]);
}

#[test]
fn ctrl_o_refuses_what_it_cannot_open_and_keeps_your_buffer() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "first\n");
    let binary = env.path("bin.dat");
    fs::write(&binary, [0xff, 0xfe]).unwrap();
    let held = work_file(&env, "held.txt", "held\n");
    let mut other = env.open(&held);
    let mut p = env.open(&a);
    type_text(&mut p, "edit ");
    open_from_prompt(&mut p, binary.to_str().unwrap());
    p.wait_for("not valid UTF-8", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    open_from_prompt(&mut p, env.dir.path().to_str().unwrap());
    p.wait_for("is a directory", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    open_from_prompt(&mut p, held.to_str().unwrap());
    p.wait_for("Open in another kyte", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    quit_clean(&mut other);
}

#[test]
fn ctrl_o_offers_recovery_for_a_crashed_file() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "first\n");
    let b = work_file(&env, "b.txt", "second\n");
    edit_and_kill(&env, &b, "crashed ");
    let mut p = env.open(&a);
    open_from_prompt(&mut p, b.to_str().unwrap());
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ crashed second", TIMEOUT);
    wait_status(&p, "b.txt [+]");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&b).unwrap(), "crashed second\n");
}

#[test]
fn ctrl_o_from_an_untitled_buffer_and_tab_completes() {
    let env = Env::new();
    let b = env.write("completeme.txt", "found\n");
    let mut p = open_untitled(&env);
    p.send_keys(&["C-o"]);
    p.wait_for("Open:", TIMEOUT);
    let stem = b.to_str().unwrap().strip_suffix("completeme.txt").unwrap();
    p.send_str(&format!("{stem}comp"));
    p.send_keys(&["Tab"]);
    p.wait_for("completeme.txt", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("1 │ found", TIMEOUT);
    quit_clean(&mut p);
    assert!(swap_files(&env).is_empty(), "the untitled swap went too");
}

#[test]
fn ctrl_o_opens_a_new_file_that_does_not_exist_yet() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "first\n");
    let new = a.with_file_name("new.txt");
    let mut p = env.open(&a);
    open_from_prompt(&mut p, new.to_str().unwrap());
    p.wait_for("Opened new.txt", TIMEOUT);
    type_text(&mut p, "hello");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&new).unwrap(), "hello");
}

// ---------------------------------------------------------------- search and replace

#[test]
fn search_ignores_case_until_the_query_has_a_capital() {
    let env = Env::new();
    let f = env.write("s.txt", "Hello hello\n");
    let mut p = env.open(&f);
    p.send_keys(&["Right"]);
    wait_status(&p, "Col 2");
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    p.send_str("hello");
    wait_status(&p, "Col 7");
    p.send_keys(&["C-Home"]);
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    p.send_keys(&["Home"]);
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    p.send_str("hello");
    wait_status(&p, "Col 1");
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    quit_clean(&mut p);
}

#[test]
fn search_highlights_matches_while_the_prompt_is_open() {
    let env = Env::new();
    let f = env.write("s.txt", "cat dog cat\n");
    let mut p = env.open(&f);
    let plain = p.bg(0, 6);
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    p.send_str("cat");
    p.wait_for("Search: cat", TIMEOUT);
    let current = p.bg(0, 6);
    let other = p.bg(0, 14);
    assert_ne!(current, plain, "the match under the cursor stands out");
    assert_ne!(other, plain, "other matches do too");
    assert_ne!(current, other, "and the two differ");
    assert_eq!(p.bg(0, 10), plain, "text between them doesn't");
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    assert_eq!(p.bg(0, 14), plain, "highlights end with the prompt");
    quit_clean(&mut p);
}

#[test]
fn replace_through_the_prompts_then_undo() {
    let env = Env::new();
    let f = env.write("r.txt", "one two one\none\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-r"]);
    p.wait_for("Replace:", TIMEOUT);
    p.send_str("one");
    p.send_keys(&["Enter"]);
    p.wait_for("With:", TIMEOUT);
    p.send_str("1");
    p.send_keys(&["Enter"]);
    p.wait_for("Replace this one?", TIMEOUT);
    p.send_str("n");
    wait_status(&p, "Col 9");
    p.send_str("a");
    p.wait_for("Replaced 2 matches", TIMEOUT);
    p.wait_for("1 │ one two 1", TIMEOUT);
    save_and_wait(&mut p);
    p.send_keys(&["C-z"]);
    p.wait_for("1 │ one two one", TIMEOUT);
    p.send_keys(&["C-y"]);
    p.wait_for("1 │ one two 1", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "one two 1\n1\n");
}

#[test]
fn replace_all_survives_a_crash() {
    let env = Env::new();
    let f = work_file(&env, "r.txt", "x x x\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-r"]);
    p.wait_for("Replace:", TIMEOUT);
    p.send_str("x");
    p.send_keys(&["Enter"]);
    p.wait_for("With:", TIMEOUT);
    p.send_str("long");
    p.send_keys(&["Enter"]);
    p.wait_for("Replace this one?", TIMEOUT);
    p.send_str("a");
    p.wait_for("Replaced 3 matches", TIMEOUT);
    std::thread::sleep(Duration::from_millis(100));
    p.kill9();
    assert_eq!(print_swap(&env, &f).1, "long long long\n");
}

// ---------------------------------------------------------------- selection

const SHIFT_RIGHT: &[u8] = b"\x1b[1;2C";
const SHIFT_DOWN: &[u8] = b"\x1b[1;2B";

#[test]
fn shift_selects_shows_on_screen_and_typing_replaces_it() {
    let env = Env::new();
    let f = env.write("s.txt", "hello world\n");
    let mut p = env.open(&f);
    let plain = p.bg(0, 6);
    for _ in 0..5 {
        p.send(SHIFT_RIGHT);
    }
    wait_status(&p, "5 selected");
    assert_ne!(p.bg(0, 6), plain, "selected text is drawn differently");
    assert_eq!(p.bg(0, 11), plain, "the rest of the line isn't");
    p.send_str("bye");
    p.wait_for("1 │ bye world", TIMEOUT);
    assert!(!status(&p).contains("selected"));
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "bye world\n");
}

#[test]
fn esc_on_a_selection_offers_its_actions_and_esc_again_clears_it() {
    let env = Env::new();
    let f = env.write("s.txt", "hello\n");
    let mut p = env.open(&f);
    p.send(SHIFT_RIGHT);
    wait_status(&p, "1 selected");
    p.send_keys(&["Esc"]);
    p.wait_for("Selection", TIMEOUT);
    p.wait_for("x cut", TIMEOUT);
    assert!(!has_legend(&p), "the selection menu replaces the usual one");
    std::thread::sleep(Duration::from_millis(30));
    p.send_keys(&["Esc"]);
    p.wait_until("selection to clear", TIMEOUT, |s| !s.contains("selected"));
    assert!(status(&p).contains("INSERT"));
    quit_clean(&mut p);
}

#[test]
fn selection_action_through_the_menu() {
    let env = Env::new();
    let f = env.write("s.txt", "hello world\n");
    let mut p = env.open(&f);
    for _ in 0..5 {
        p.send(SHIFT_RIGHT);
    }
    wait_status(&p, "5 selected");
    p.send_keys(&["Esc"]);
    p.wait_for("x cut", TIMEOUT);
    p.send_str("U");
    p.wait_for("1 │ HELLO world", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "HELLO world\n");
}

#[test]
fn copy_reaches_the_terminal_clipboard_and_paste_inserts() {
    let env = Env::new();
    let f = env.write("c.txt", "copy me\nnext\n");
    let mut p = env.open(&f);
    for _ in 0..4 {
        p.send(SHIFT_RIGHT);
    }
    wait_status(&p, "4 selected");
    p.send_keys(&["C-c"]);
    p.wait_for("Copied 4 characters", TIMEOUT);
    let raw = p.raw_output();
    let osc = b"\x1b]52;c;Y29weQ==\x07";
    assert!(
        raw.windows(osc.len()).any(|w| w == osc),
        "the copy goes to the terminal as OSC 52"
    );
    p.send_keys(&["Down"]);
    wait_status(&p, "Ln 2");
    p.send_keys(&["C-v"]);
    p.wait_for("2 │ nextcopy", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "copy me\nnextcopy\n");
}

#[test]
fn cut_whole_lines_and_multi_line_selection() {
    let env = Env::new();
    let f = env.write("c.txt", "one\ntwo\nthree\nfour\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-x"]);
    p.wait_for("1 │ two", TIMEOUT);
    p.send(SHIFT_DOWN);
    wait_status(&p, "4 selected");
    p.send_keys(&["C-x"]);
    p.wait_for("1 │ three", TIMEOUT);
    p.send_keys(&["C-End"]);
    wait_status(&p, "Ln 3");
    p.send_keys(&["C-v"]);
    p.wait_for("4 │ ", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "three\nfour\ntwo\n");
}

#[test]
fn select_all_then_terminal_paste_replaces_everything() {
    let env = Env::new();
    let f = env.write("a.txt", "old\ntext\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-a"]);
    wait_status(&p, "9 selected");
    p.paste("new");
    p.wait_for("1 │ new", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "new");
}

// ---------------------------------------------------------------- words

#[test]
fn word_motion_and_delete_through_terminal_bytes() {
    let env = Env::new();
    let f = env.write("w.txt", "foo bar baz\n");
    let mut p = env.open(&f);
    p.send(b"\x1b[1;5C"); // Ctrl-Right
    wait_status(&p, "Col 5");
    p.send(b"\x1b[1;3C"); // Alt-Right
    wait_status(&p, "Col 9");
    p.send(b"\x1b[1;5D"); // Ctrl-Left
    wait_status(&p, "Col 5");
    p.send(b"\x1b[3;5~"); // Ctrl-Delete
    p.wait_for("foo baz", TIMEOUT);
    p.send(b"\x08"); // Ctrl-Backspace on most legacy terminals
    p.wait_for("1 │ baz", TIMEOUT);
    p.send_keys(&["C-z"]);
    p.wait_for("foo baz", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "foo baz\n");
}

// ---------------------------------------------------------------- signals and keys

#[test]
fn sigterm_restores_the_terminal_and_keeps_the_swap() {
    let env = Env::new();
    let f = work_file(&env, "term.txt", "base\n");
    let mut p = env.open(&f);
    type_text(&mut p, "unsaved ");
    assert!(p.alternate_screen());
    let st = Command::new("kill")
        .args(["-TERM", &p.pid().to_string()])
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(p.wait_exit(TIMEOUT), 143);
    assert!(!p.alternate_screen(), "left the alternate screen");
    assert!(!p.cursor_hidden());
    assert_eq!(print_swap(&env, &f).1, "unsaved base\n");
}

#[test]
fn split_key_sequences_never_reach_the_file() {
    let env = Env::new();
    let f = env.write("split.txt", "abc\ndef\n");
    let mut p = env.open(&f);
    for rest in ["[A", "[3~", "OA"] {
        p.send(b"\x1b");
        p.wait_for("ESC", TIMEOUT);
        std::thread::sleep(Duration::from_millis(30));
        p.send(rest.as_bytes());
        wait_insert(&p);
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(!status(&p).contains("[+]"), "{}", p.dump());
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "abc\ndef\n");
}

#[test]
fn paste_into_the_search_prompt_searches() {
    let env = Env::new();
    let f = env.write("s.txt", "needle in haystack\n");
    let mut p = env.open(&f);
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    p.paste("hay");
    p.wait_for("Search: hay", TIMEOUT);
    wait_status(&p, "Col 11");
    p.send_keys(&["Esc"]);
    wait_status(&p, "INSERT");
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- line endings

#[test]
fn crlf_file_keeps_its_line_endings() {
    let env = Env::new();
    let f = env.write("dos.txt", "one\r\ntwo\r\n");
    let mut p = env.open(&f);
    wait_status(&p, "CRLF");
    assert!(!p.screen().contains('\u{240d}'), "no \\r on screen");
    p.send_keys(&["End"]);
    wait_status(&p, "Ln 1, Col 4");
    p.send_str("X");
    p.send_keys(&["Enter"]);
    p.send_str("Y");
    wait_status(&p, "Ln 2, Col 2");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read(&f).unwrap(), b"oneX\r\nY\r\ntwo\r\n");
}

#[test]
fn paste_into_a_crlf_file_uses_its_line_ending() {
    let env = Env::new();
    let f = env.write("dos.txt", "one\r\n");
    let mut p = env.open(&f);
    p.paste("a\rb\r");
    p.wait_for("2 │ b", TIMEOUT);
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read(&f).unwrap(), b"a\r\nb\r\none\r\n");
}

#[test]
fn crlf_operators_and_backspace_keep_the_ending_whole() {
    assert_eq!(
        crlf_case("ab\r\ncd\r\n", &["Esc", "k"]),
        b"\r\ncd\r\n",
        "k stops before the ending"
    );
    assert_eq!(
        crlf_case("ab\r\ncd\r\n", &["Down", "BS"]),
        b"abcd\r\n",
        "Backspace joins"
    );
    assert_eq!(
        crlf_case("ab\r\ncd\r\n", &["End", "Del"]),
        b"abcd\r\n",
        "Delete joins"
    );
    assert_eq!(
        crlf_case("ab\r\ncd", &["Down", "Esc", "d"]),
        b"ab",
        "d on a last line takes the previous ending"
    );
}

fn crlf_case(text: &str, keys: &[&str]) -> Vec<u8> {
    let env = Env::new();
    let f = env.write("dos.txt", text);
    let mut p = env.open(&f);
    for k in keys {
        match *k {
            "k" | "d" => {
                p.send_str(k);
                wait_status(&p, "[+]");
            }
            "Esc" => open_overlay(&mut p),
            "BS" => {
                p.send_keys(&["Backspace"]);
                wait_status(&p, "[+]");
            }
            "Del" => {
                p.send_keys(&["Delete"]);
                wait_status(&p, "[+]");
            }
            "Down" => {
                p.send_keys(&["Down"]);
                wait_status(&p, "Ln 2");
            }
            "End" => {
                p.send_keys(&["End"]);
                wait_status(&p, "Col 3");
            }
            other => panic!("{other}"),
        }
    }
    save_and_wait(&mut p);
    quit_clean(&mut p);
    fs::read(&f).unwrap()
}

// ---------------------------------------------------------------- saving safely

#[test]
fn save_asks_before_overwriting_outside_changes() {
    let env = Env::new();
    let f = work_file(&env, "shared.txt", "v1\n");
    let mut p = env.open(&f);
    type_text(&mut p, "mine ");
    fs::write(&f, "theirs\n").unwrap();
    p.send_keys(&["C-s"]);
    p.wait_for("File changed on disk!", TIMEOUT);
    p.send_str("x");
    std::thread::sleep(Duration::from_millis(50));
    assert!(p.screen().contains("File changed on disk!"), "x is ignored");
    p.send_str("n");
    p.wait_for("Save cancelled", TIMEOUT);
    assert_eq!(fs::read_to_string(&f).unwrap(), "theirs\n");
    wait_status(&p, "[+]");
    p.send_keys(&["C-s"]);
    p.wait_for("File changed on disk!", TIMEOUT);
    p.send_str("y");
    p.wait_for("Saved 1 line", TIMEOUT);
    assert_eq!(fs::read_to_string(&f).unwrap(), "mine v1\n");
    // Having answered once, the next save has nothing to ask about.
    type_text(&mut p, "x");
    save_and_wait(&mut p);
    quit_clean(&mut p);
}

#[test]
fn rewriting_the_same_content_is_not_an_outside_change() {
    let env = Env::new();
    let f = work_file(&env, "same.txt", "same\n");
    let mut p = env.open(&f);
    type_text(&mut p, "x");
    fs::write(&f, "same\n").unwrap();
    p.send_keys(&["C-s"]);
    p.wait_for("Saved 1 line", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn file_deleted_while_open_is_saved_without_asking() {
    let env = Env::new();
    let f = work_file(&env, "gone.txt", "text\n");
    let mut p = env.open(&f);
    type_text(&mut p, "x");
    fs::remove_file(&f).unwrap();
    p.send_keys(&["C-s"]);
    p.wait_for("Saved 1 line", TIMEOUT);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&f).unwrap(), "xtext\n");
}

#[test]
fn read_only_file_is_replaced_only_after_a_yes() {
    let env = Env::new();
    if fs::metadata(env.dir.path()).unwrap().uid() == 0 {
        eprintln!("skipped: root can write to anything");
        return;
    }
    let f = work_file(&env, "ro.txt", "protected\n");
    fs::set_permissions(&f, fs::Permissions::from_mode(0o444)).unwrap();
    let mut p = env.open(&f);
    type_text(&mut p, "x");
    p.send_keys(&["C-s"]);
    p.wait_for("File is read-only!", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("Save cancelled", TIMEOUT);
    assert_eq!(fs::read_to_string(&f).unwrap(), "protected\n");
    p.send_keys(&["C-s"]);
    p.wait_for("File is read-only!", TIMEOUT);
    p.send_str("y");
    p.wait_for("Saved 1 line", TIMEOUT);
    assert_eq!(fs::read_to_string(&f).unwrap(), "xprotected\n");
    assert_eq!(
        fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o444
    );
    quit_clean(&mut p);
}

#[test]
fn recovered_edits_ask_before_replacing_a_changed_file() {
    let env = Env::new();
    let f = work_file(&env, "both.txt", "v1\n");
    edit_and_kill(&env, &f, "mine ");
    fs::write(&f, "theirs\n").unwrap();
    let mut p = open_raw(&env, &f);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ mine v1", TIMEOUT);
    p.send_keys(&["C-s"]);
    p.wait_for("File changed on disk!", TIMEOUT);
    p.send_str("n");
    p.wait_for("Save cancelled", TIMEOUT);
    assert_eq!(fs::read_to_string(&f).unwrap(), "theirs\n");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn fifo_is_refused_at_open() {
    let env = Env::new();
    let fifo = env.path("pipe");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let mut p = Pty::kyte(&bin(), &env.state, &[fifo.to_str().unwrap()]);
    assert_eq!(p.wait_exit(TIMEOUT), 2);
    p.wait_for("not a regular file", TIMEOUT);
}

// ---------------------------------------------------------------- AC-24

#[test]
fn ac24_save_into_read_only_dir_fails_visibly_and_keeps_everything() {
    let env = Env::new();
    if fs::metadata(env.dir.path()).unwrap().uid() == 0 {
        eprintln!("skipped: root can write to anything");
        return;
    }
    let f = work_file(&env, "ro.txt", "untouched\n");
    let dir = f.parent().unwrap().to_path_buf();
    let mut p = env.open(&f);
    type_text(&mut p, "edit ");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    p.send_keys(&["C-s"]);
    p.wait_for("Save failed", TIMEOUT);
    let after = fs::read_to_string(&f).unwrap();
    let names = dir_names(&dir);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(after, "untouched\n");
    assert_eq!(names, vec!["ro.txt".to_string()], "no temp file left");
    assert!(status(&p).contains("[+]"), "buffer stays dirty");
    let swaps = swap_files(&env);
    assert_eq!(swaps.len(), 1, "swap remains");
    assert_eq!(print_swap(&env, &f), (0, "edit untouched\n".to_string()));
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    p.wait_exit(TIMEOUT);
}

#[test]
fn ac24_save_through_symlink_updates_target_keeps_link() {
    let env = Env::new();
    let real = work_file(&env, "real.txt", "real\n");
    let link = real.parent().unwrap().join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut p = env.open(&link);
    type_text(&mut p, "via link ");
    save_and_wait(&mut p);
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&real).unwrap(), "via link real\n");
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), real);
}

// ---------------------------------------------------------------- several open files

/// Starts kyte on `files` and waits for the first frame.
fn open_all(env: &Env, files: &[&Path]) -> Pty {
    let args: Vec<&str> = files.iter().map(|f| f.to_str().unwrap()).collect();
    let pty = Pty::kyte(&bin(), &env.state, &args);
    pty.wait_for("Ln 1, Col 1", TIMEOUT);
    pty
}

/// Esc, then one key of the layer.
fn esc_then(p: &mut Pty, key: &str) {
    open_overlay(p);
    p.send_str(key);
    wait_insert(p);
}

fn run_from_palette(p: &mut Pty, command: &str) {
    open_palette(p);
    p.send_str(command);
    p.wait_for(&format!("▸ {command}"), TIMEOUT);
    p.send_keys(&["Enter"]);
}

#[test]
fn ctrl_o_keeps_both_files_open_each_with_its_swap() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "first\n");
    let b = work_file(&env, "b.txt", "second\n");
    let mut p = env.open(&a);
    assert!(!status(&p).contains("file 1"), "{}", status(&p));
    open_from_prompt(&mut p, b.to_str().unwrap());
    p.wait_for("Opened b.txt", TIMEOUT);
    p.wait_for("1 │ second", TIMEOUT);
    wait_status(&p, "b.txt │ file 2 of 2");
    assert_eq!(swap_files(&env).len(), 2);
    type_text(&mut p, "more ");
    save_and_wait(&mut p);
    esc_then(&mut p, ",");
    p.wait_for("1 │ first", TIMEOUT);
    wait_status(&p, "a.txt │ file 1 of 2");
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&b).unwrap(), "more second\n");
    assert_eq!(fs::read_to_string(&a).unwrap(), "first\n");
    assert!(swap_files(&env).is_empty());
}

#[test]
fn ctrl_k_opening_a_file_leaves_unsaved_work_where_it_is() {
    let env = Env::new();
    let project = env.path("proj");
    fs::create_dir(&project).unwrap();
    let a = project.join("a.txt");
    fs::write(&a, "first\n").unwrap();
    fs::write(project.join("other.txt"), "second\n").unwrap();
    let mut p = open_in(&env, &project, &a);
    type_text(&mut p, "edit ");
    p.send_keys(&["C-k"]);
    p.wait_for("Commands, files", TIMEOUT);
    p.send_str("other");
    p.wait_for("▸ other.txt", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("1 │ second", TIMEOUT);
    wait_status(&p, "file 2 of 2, 1 more unsaved");
    assert_eq!(fs::read_to_string(&a).unwrap(), "first\n");
    esc_then(&mut p, ".");
    p.wait_for("1 │ edit first", TIMEOUT);
    wait_status(&p, "a.txt [+]");
    p.send_keys(&["C-z"]);
    p.wait_for("1 │ first", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes in a.txt", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn a_crash_leaves_every_open_files_edits_recoverable() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "first\n");
    let b = work_file(&env, "b.txt", "second\n");
    let mut p = env.open(&a);
    type_text(&mut p, "aa ");
    open_from_prompt(&mut p, b.to_str().unwrap());
    p.wait_for("1 │ second", TIMEOUT);
    type_text(&mut p, "bb ");
    esc_then(&mut p, ",");
    p.wait_for("1 │ aa first", TIMEOUT);
    type_text(&mut p, "again ");
    p.kill9();
    assert_eq!(print_swap(&env, &a), (0, "aa again first\n".to_string()));
    assert_eq!(print_swap(&env, &b), (0, "bb second\n".to_string()));
}

#[test]
fn opening_a_file_thats_open_moves_to_it_and_never_touches_its_swap() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "first\n");
    let b = work_file(&env, "b.txt", "second\n");
    let link = a.parent().unwrap().join("link.txt");
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let mut p = env.open(&a);
    type_text(&mut p, "keep ");
    open_from_prompt(&mut p, b.to_str().unwrap());
    p.wait_for("1 │ second", TIMEOUT);
    open_from_prompt(&mut p, link.to_str().unwrap());
    p.wait_for("1 │ keep first", TIMEOUT);
    wait_status(&p, "a.txt [+] │ file 1 of 2");
    assert_eq!(swap_files(&env).len(), 2);
    open_from_prompt(&mut p, a.to_str().unwrap());
    p.wait_for("Already open", TIMEOUT);
    p.wait_for("1 │ keep first", TIMEOUT);
    type_text(&mut p, "more ");
    p.kill9();
    assert_eq!(print_swap(&env, &a), (0, "keep more first\n".to_string()));
}

#[test]
fn the_same_file_twice_on_the_command_line_opens_once() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "first\n");
    let mut p = open_all(&env, &[&a, &a]);
    p.wait_for("1 │ first", TIMEOUT);
    assert!(!status(&p).contains("file 1"), "{}", status(&p));
    assert_eq!(swap_files(&env).len(), 1);
    quit_clean(&mut p);
}

#[test]
fn moving_between_files_keeps_their_order() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let c = work_file(&env, "c.txt", "ccc\n");
    let mut p = open_all(&env, &[&a, &b, &c]);
    p.wait_for("1 │ aaa", TIMEOUT);
    wait_status(&p, "a.txt │ file 1 of 3");
    esc_then(&mut p, ".");
    wait_status(&p, "b.txt │ file 2 of 3");
    esc_then(&mut p, ".");
    wait_status(&p, "c.txt │ file 3 of 3");
    esc_then(&mut p, ".");
    wait_status(&p, "a.txt │ file 1 of 3");
    esc_then(&mut p, ",");
    wait_status(&p, "c.txt │ file 3 of 3");
    run_from_palette(&mut p, "Open files");
    p.wait_for("Open files", TIMEOUT);
    p.send_keys(&["Up", "Up"]);
    p.wait_until("a.txt to be selected", TIMEOUT, |s| {
        s.lines().any(|l| l.contains("▸ ") && l.contains("a.txt"))
    });
    p.send_keys(&["Enter"]);
    wait_status(&p, "a.txt │ file 1 of 3");
    esc_then(&mut p, ".");
    wait_status(&p, "b.txt │ file 2 of 3");
    esc_then(&mut p, ",");
    wait_status(&p, "a.txt │ file 1 of 3");
    esc_then(&mut p, ",");
    wait_status(&p, "c.txt │ file 3 of 3");
    quit_clean(&mut p);
}

#[test]
fn the_open_files_box_says_which_are_unsaved() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "first\n");
    let b = work_file(&env, "b.txt", "second\n");
    let mut p = open_all(&env, &[&a, &b]);
    p.wait_for("1 │ first", TIMEOUT);
    type_text(&mut p, "x");
    open_overlay(&mut p);
    p.send_str("b");
    p.wait_for("Open files", TIMEOUT);
    p.wait_for("unsaved", TIMEOUT);
    p.wait_for("b.txt", TIMEOUT);
    p.send_str("b.txt");
    p.wait_until("b.txt to be selected", TIMEOUT, |s| {
        s.lines().any(|l| l.contains("▸ ") && l.contains("b.txt"))
    });
    p.send_keys(&["Enter"]);
    wait_status(&p, "b.txt │ file 2 of 2, 1 more unsaved");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes in a.txt", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn quit_asks_about_each_unsaved_file_in_turn() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let c = work_file(&env, "c.txt", "ccc\n");
    let mut p = open_all(&env, &[&a, &b, &c]);
    p.wait_for("1 │ aaa", TIMEOUT);
    type_text(&mut p, "1");
    esc_then(&mut p, ".");
    p.wait_for("1 │ bbb", TIMEOUT);
    type_text(&mut p, "2");
    esc_then(&mut p, ".");
    p.wait_for("1 │ ccc", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes in a.txt", TIMEOUT);
    p.send_str("y");
    p.wait_for("Unsaved changes in b.txt", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&a).unwrap(), "1aaa\n");
    assert_eq!(fs::read_to_string(&b).unwrap(), "bbb\n");
    assert!(swap_files(&env).is_empty());
}

#[test]
fn esc_while_quitting_keeps_every_file_and_the_next_quit_asks_again() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let mut p = open_all(&env, &[&a, &b]);
    p.wait_for("1 │ aaa", TIMEOUT);
    type_text(&mut p, "1");
    esc_then(&mut p, ".");
    p.wait_for("1 │ bbb", TIMEOUT);
    type_text(&mut p, "2");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes in b.txt", TIMEOUT);
    p.send_str("n");
    p.wait_for("Unsaved changes in a.txt", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Unsaved changes", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes in a.txt", TIMEOUT);
    p.send_str("y");
    p.wait_for("Unsaved changes in b.txt", TIMEOUT);
    p.send_str("y");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&a).unwrap(), "1aaa\n");
    assert_eq!(fs::read_to_string(&b).unwrap(), "2bbb\n");
}

#[test]
fn quit_without_saving_asks_when_another_file_is_unsaved() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let mut p = open_all(&env, &[&a, &b]);
    p.wait_for("1 │ aaa", TIMEOUT);
    type_text(&mut p, "1");
    esc_then(&mut p, ".");
    p.wait_for("1 │ bbb", TIMEOUT);
    run_from_palette(&mut p, "Quit without saving");
    p.wait_for("Discard unsaved changes in 1 other file?", TIMEOUT);
    p.send_str("n");
    p.wait_gone("Discard unsaved", TIMEOUT);
    wait_status(&p, "b.txt │ file 2 of 2, 1 more unsaved");
    run_from_palette(&mut p, "Quit without saving");
    p.wait_for("Discard unsaved changes in 1 other file?", TIMEOUT);
    p.send_str("y");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&a).unwrap(), "aaa\n");
    assert!(swap_files(&env).is_empty());
}

#[test]
fn ctrl_w_closes_a_file_asking_if_its_unsaved_and_the_last_one_quits() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let c = work_file(&env, "c.txt", "ccc\n");
    let mut p = open_all(&env, &[&a, &b, &c]);
    p.wait_for("1 │ aaa", TIMEOUT);
    esc_then(&mut p, ".");
    p.wait_for("1 │ bbb", TIMEOUT);
    p.send_keys(&["C-w"]);
    p.wait_for("1 │ ccc", TIMEOUT);
    wait_status(&p, "c.txt │ file 2 of 2");
    assert_eq!(swap_files(&env).len(), 2);
    type_text(&mut p, "1");
    p.send_keys(&["C-w"]);
    p.wait_for("Unsaved changes in c.txt", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone("Unsaved changes", TIMEOUT);
    p.send_keys(&["C-w"]);
    p.wait_for("Save and close", TIMEOUT);
    p.send_str("y");
    p.wait_for("1 │ aaa", TIMEOUT);
    assert_eq!(fs::read_to_string(&c).unwrap(), "1ccc\n");
    type_text(&mut p, "2");
    p.send_keys(&["C-w"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&a).unwrap(), "aaa\n");
    assert!(swap_files(&env).is_empty());
}

#[test]
fn save_as_onto_a_file_open_here_is_refused() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let mut p = open_all(&env, &[&a, &b]);
    p.wait_for("1 │ aaa", TIMEOUT);
    save_as(&mut p, b.to_str().unwrap());
    p.wait_for("close it first", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    assert_eq!(fs::read_to_string(&b).unwrap(), "bbb\n");
    assert_eq!(swap_files(&env).len(), 2);
    quit_clean(&mut p);
}

#[test]
fn an_empty_untitled_file_gives_way_but_one_with_text_stays() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let mut p = Pty::kyte(&bin(), &env.state, &[]);
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    open_from_prompt(&mut p, a.to_str().unwrap());
    p.wait_for("1 │ aaa", TIMEOUT);
    assert!(!status(&p).contains("file 1"), "{}", status(&p));
    assert_eq!(swap_files(&env).len(), 1);
    quit_clean(&mut p);
    let mut p = Pty::kyte(&bin(), &env.state, &[]);
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    type_text(&mut p, "notes");
    open_from_prompt(&mut p, a.to_str().unwrap());
    p.wait_for("1 │ aaa", TIMEOUT);
    wait_status(&p, "file 2 of 2, 1 more unsaved");
    esc_then(&mut p, ".");
    p.wait_for("1 │ notes", TIMEOUT);
    p.kill9();
}

#[test]
fn a_file_with_crashed_edits_can_be_left_without_quitting() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let mut p = env.open(&b);
    type_text(&mut p, "lost ");
    p.kill9();
    let mut p = env.open(&a);
    type_text(&mut p, "1");
    open_from_prompt(&mut p, b.to_str().unwrap());
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.wait_for("Close this file", TIMEOUT);
    p.send_str("q");
    p.wait_for("1 │ 1aaa", TIMEOUT);
    wait_status(&p, "a.txt [+]");
    assert!(!status(&p).contains("file 1"), "{}", status(&p));
    assert_eq!(print_swap(&env, &b), (0, "lost bbb\n".to_string()));
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(print_swap(&env, &b), (0, "lost bbb\n".to_string()));
}

#[test]
fn a_later_file_with_crashed_edits_stops_to_ask_then_the_rest_open() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let c = work_file(&env, "c.txt", "ccc\n");
    let mut p = env.open(&b);
    type_text(&mut p, "lost ");
    p.kill9();
    let mut p = Pty::kyte(
        &bin(),
        &env.state,
        &[
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            c.to_str().unwrap(),
        ],
    );
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ aaa", TIMEOUT);
    wait_status(&p, "file 1 of 3, 1 more unsaved");
    esc_then(&mut p, ".");
    p.wait_for("1 │ lost bbb", TIMEOUT);
    esc_then(&mut p, ".");
    p.wait_for("1 │ ccc", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes in b.txt", TIMEOUT);
    p.send_str("y");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
    assert_eq!(fs::read_to_string(&b).unwrap(), "lost bbb\n");
}

#[test]
fn a_file_kyte_cant_open_on_the_command_line_doesnt_stop_the_rest() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let bad = env.path("bin.dat");
    fs::write(&bad, [0xff, 0xfe]).unwrap();
    let c = work_file(&env, "c.txt", "ccc\n");
    let mut p = open_all(&env, &[&a, &bad, &c]);
    p.wait_for("not valid UTF-8", TIMEOUT);
    wait_status(&p, "a.txt │ file 1 of 2");
    assert_eq!(swap_files(&env).len(), 2);
    quit_clean(&mut p);
}

#[test]
fn a_change_to_a_file_not_on_screen_shows_up_when_you_look() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let mut p = open_all(&env, &[&a, &b]);
    p.wait_for("1 │ aaa", TIMEOUT);
    fs::write(&b, "outside\n").unwrap();
    std::thread::sleep(Duration::from_millis(2300));
    p.send_keys(&["Right"]);
    esc_then(&mut p, ".");
    wait_status(&p, "changed on disk");
    quit_clean(&mut p);
}

#[test]
fn two_spellings_of_a_new_file_open_once() {
    let env = Env::new();
    let dir = env.path("w");
    fs::create_dir_all(dir.join("sub")).unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink("sub/../new.txt", &link).unwrap();
    let new = dir.join("new.txt");
    let mut p = env.open(&link);
    open_from_prompt(&mut p, new.to_str().unwrap());
    p.wait_for("Already open", TIMEOUT);
    assert_eq!(swap_files(&env).len(), 1);
    quit_clean(&mut p);
}

#[test]
fn a_symlink_moved_onto_an_open_file_cant_save_over_its_swap() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let b = work_file(&env, "b.txt", "bbb\n");
    let link = a.parent().unwrap().join("link.txt");
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let mut p = open_all(&env, &[&link, &b]);
    p.wait_for("1 │ aaa", TIMEOUT);
    esc_then(&mut p, ".");
    p.wait_for("1 │ bbb", TIMEOUT);
    type_text(&mut p, "precious ");
    esc_then(&mut p, ",");
    p.wait_for("1 │ aaa", TIMEOUT);
    type_text(&mut p, "other ");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&b, &link).unwrap();
    p.send_keys(&["C-s"]);
    p.wait_for("changed on disk", TIMEOUT);
    p.send_str("y");
    p.wait_for("That file is open here", TIMEOUT);
    assert_eq!(fs::read_to_string(&b).unwrap(), "bbb\n");
    p.kill9();
    assert_eq!(print_swap(&env, &b), (0, "precious bbb\n".to_string()));
}

#[test]
fn save_as_wont_overwrite_edits_left_by_a_crash() {
    let env = Env::new();
    let notes = env.path("work/notes.txt");
    fs::create_dir_all(notes.parent().unwrap()).unwrap();
    let a = work_file(&env, "a.txt", "aaa\n");
    let mut p = env.open(&notes);
    type_text(&mut p, "precious work");
    p.kill9();
    let mut p = env.open(&a);
    save_as(&mut p, notes.to_str().unwrap());
    p.wait_for("edits from a crash", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    assert!(!notes.exists());
    assert_eq!(print_swap(&env, &notes), (0, "precious work".to_string()));
    quit_clean(&mut p);
    assert_eq!(print_swap(&env, &notes), (0, "precious work".to_string()));
}

#[test]
fn keeping_your_side_of_a_review_counts_as_unsaved() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let mut p = env.open(&a);
    p.wait_for("1 │ aaa", TIMEOUT);
    fs::write(&a, "theirs\n").unwrap();
    wait_status(&p, "changed on disk");
    open_overlay(&mut p);
    p.send_str("c");
    p.wait_for("Keep yours", TIMEOUT);
    p.send_str("n");
    p.wait_gone("Keep yours", TIMEOUT);
    wait_status(&p, "a.txt [+]");
    p.send_keys(&["C-w"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn a_crash_right_after_keeping_your_side_of_a_review_loses_nothing() {
    let env = Env::new();
    let a = work_file(&env, "a.txt", "aaa\n");
    let mut p = env.open(&a);
    p.wait_for("1 │ aaa", TIMEOUT);
    fs::write(&a, "theirs\n").unwrap();
    wait_status(&p, "changed on disk");
    open_overlay(&mut p);
    p.send_str("c");
    p.wait_for("Keep yours", TIMEOUT);
    p.send_str("n");
    wait_status(&p, "a.txt [+]");
    p.kill9();
    assert_eq!(print_swap(&env, &a), (0, "aaa\n".to_string()));
    let mut p = Pty::kyte(&bin(), &env.state, &[a.to_str().unwrap()]);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    p.send_str("r");
    p.wait_for("1 │ aaa", TIMEOUT);
    wait_status(&p, "a.txt [+]");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

// ---------------------------------------------------------------- file tree

/// A small project, and Kyte open on its README from inside it.
fn in_project(env: &Env) -> (PathBuf, Pty) {
    let project = env.path("proj");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(project.join("src/lib.rs"), "fn lib() {}\n").unwrap();
    fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(project.join("README.md"), "# readme\n").unwrap();
    let readme = project.join("README.md");
    let p = open_in(env, &project, &readme);
    p.wait_for("1 │ # readme", TIMEOUT);
    (project, p)
}

fn selected(p: &Pty, name: &str) {
    let name = name.to_string();
    let what = format!("{name} to be selected");
    p.wait_until(&what, TIMEOUT, move |s| {
        s.lines().any(|l| l.contains("▸ ") && l.contains(&name))
    });
}

#[test]
fn the_tree_opens_on_the_current_file_and_a_file_from_it_stays_open_alongside() {
    let env = Env::new();
    let (_project, mut p) = in_project(&env);
    p.send_keys(&["C-t"]);
    p.wait_for("proj", TIMEOUT);
    selected(&p, "README.md");
    wait_status(&p, "FILES");
    p.send_str("s");
    selected(&p, "src/");
    p.send_keys(&["Right"]);
    p.wait_for("lib.rs", TIMEOUT);
    p.send_keys(&["Down"]);
    selected(&p, "lib.rs");
    p.send_keys(&["Enter"]);
    p.wait_for("1 │ fn lib() {}", TIMEOUT);
    wait_status(&p, "lib.rs │ file 2 of 2");
    open_overlay(&mut p);
    p.send_str("t");
    selected(&p, "lib.rs");
    p.wait_for("open", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    wait_status(&p, "lib.rs │ file 2 of 2");
    quit_clean(&mut p);
}

#[test]
fn choosing_an_open_file_in_the_tree_moves_to_it() {
    let env = Env::new();
    let (_project, mut p) = in_project(&env);
    type_text(&mut p, "edit ");
    p.send_keys(&["C-t"]);
    selected(&p, "README.md");
    p.wait_for("unsaved", TIMEOUT);
    p.send_str("s");
    selected(&p, "src/");
    p.send_keys(&["Enter"]);
    p.wait_for("main.rs", TIMEOUT);
    p.send_keys(&["Down", "Down"]);
    selected(&p, "main.rs");
    p.send_keys(&["Enter"]);
    p.wait_for("1 │ fn main() {}", TIMEOUT);
    p.send_keys(&["C-t"]);
    selected(&p, "main.rs");
    p.send_str("r");
    selected(&p, "README.md");
    p.send_keys(&["Enter"]);
    p.wait_for("1 │ edit # readme", TIMEOUT);
    wait_status(&p, "README.md [+] │ file 1 of 2");
    assert_eq!(swap_files(&env).len(), 2);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes in README.md", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn the_tree_shows_hidden_files_on_request_and_folds_with_left() {
    let env = Env::new();
    let (project, mut p) = in_project(&env);
    fs::write(project.join(".env"), "A=1\n").unwrap();
    p.send_keys(&["C-t"]);
    selected(&p, "README.md");
    assert!(!p.screen().contains(".env"), "{}", p.dump());
    p.send_str(".");
    p.wait_for("hidden files shown", TIMEOUT);
    p.wait_for(".env", TIMEOUT);
    p.send_str(".");
    p.wait_gone(".env", TIMEOUT);
    p.send_str("s");
    selected(&p, "src/");
    p.send_keys(&["Right"]);
    p.wait_for("lib.rs", TIMEOUT);
    p.send_keys(&["Left"]);
    p.wait_gone("lib.rs", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    quit_clean(&mut p);
}

#[test]
fn a_file_the_tree_cant_open_is_reported_and_nothing_changes() {
    let env = Env::new();
    let (project, mut p) = in_project(&env);
    fs::write(project.join("bin.dat"), [0xff, 0xfe]).unwrap();
    p.send_keys(&["C-t"]);
    selected(&p, "README.md");
    p.send_str("b");
    selected(&p, "bin.dat");
    p.send_keys(&["Enter"]);
    p.wait_for("not valid UTF-8", TIMEOUT);
    wait_insert(&p);
    assert!(!status(&p).contains("file 1"), "{}", status(&p));
    assert_eq!(swap_files(&env).len(), 1);
    quit_clean(&mut p);
}

// ---------------------------------------------------------------- writing

/// Starts Kyte on `file` with the settings a person would have.
fn open_writing(env: &Env, file: &Path, settings: &str) -> Pty {
    let config = env.path("writing.toml");
    fs::write(&config, settings).unwrap();
    let p = Pty::spawn(
        Spawn::new(bin())
            .env("KYTE_STATE_DIR", env.state.to_string_lossy())
            .env("KYTE_CONFIG", config.to_string_lossy())
            .arg("--no-kitty")
            .arg(file.to_str().unwrap()),
    );
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    p
}

#[test]
fn text_and_markdown_open_for_writing() {
    let env = Env::new();
    let long = "word ".repeat(40);
    let f = env.write("notes.md", &format!("# Title\n{long}\n"));
    let mut p = open_writing(&env, &f, "");
    p.wait_for("# Title", TIMEOUT);
    p.wait_for("1 │ # Title", TIMEOUT);
    wait_status(&p, "41 words");
    assert!(
        p.screen().lines().nth(2).unwrap().contains("word"),
        "wrapped"
    );
    quit_clean(&mut p);
}

#[cfg(feature = "highlight")]
#[test]
fn latex_keeps_its_line_numbers_and_counts_only_what_a_reader_reads() {
    let env = Env::new();
    let text =
        "\\documentclass{article}\n\\begin{document}\nOne two \\cite{a} three.\n\\end{document}\n";
    let f = env.write("paper.tex", text);
    let mut p = open_writing(&env, &f, "");
    p.wait_for("1 │", TIMEOUT);
    wait_status(&p, "3 words");
    quit_clean(&mut p);
}

#[test]
fn code_shows_as_code_and_each_file_keeps_its_own_view() {
    let env = Env::new();
    let md = env.write("a.md", "one two\n");
    let rs = env.write("b.rs", "fn main() {}\n");
    let mut p = open_writing(&env, &md, "");
    p.wait_for("one two", TIMEOUT);
    wait_status(&p, "2 words");
    open_from_prompt(&mut p, rs.to_str().unwrap());
    p.wait_for("1 │ fn main", TIMEOUT);
    assert!(!status(&p).contains("words"), "{}", status(&p));
    esc_then(&mut p, "l");
    p.wait_gone("1 │ fn", TIMEOUT);
    esc_then(&mut p, ",");
    p.wait_for("1 │ one two", TIMEOUT);
    wait_status(&p, "2 words");
    esc_then(&mut p, ".");
    p.wait_for("fn main", TIMEOUT);
    assert!(!p.screen().contains("1 │ fn"), "{}", p.dump());
    quit_clean(&mut p);
}

#[test]
fn line_numbers_show_by_default_and_esc_l_toggles_them() {
    let env = Env::new();
    let f = env.write("notes.md", "one two\n");
    let mut p = open_writing(&env, &f, "");
    p.wait_for("1 │ one two", TIMEOUT);
    open_overlay(&mut p);
    p.wait_for("l line numbers", TIMEOUT);
    p.send_str("l");
    p.wait_gone("1 │ one", TIMEOUT);
    p.wait_for("one two", TIMEOUT);
    esc_then(&mut p, "l");
    p.wait_for("1 │ one two", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn the_line_numbers_setting_still_turns_them_off_everywhere() {
    let env = Env::new();
    let f = env.write("notes.md", "one two\n");
    let mut p = open_writing(&env, &f, "line_numbers = false\n");
    p.wait_for("one two", TIMEOUT);
    assert!(!p.screen().contains("1 │ one"), "{}", p.dump());
    quit_clean(&mut p);
}

#[test]
fn the_writing_setting_turns_it_off() {
    let env = Env::new();
    let f = env.write("notes.md", "one two\n");
    let mut p = open_writing(&env, &f, "writing = false\n");
    p.wait_for("1 │ one two", TIMEOUT);
    assert!(!status(&p).contains("words"), "{}", status(&p));
    quit_clean(&mut p);
}

#[test]
fn saving_under_a_writing_name_changes_the_view() {
    let env = Env::new();
    let a = work_file(&env, "a.rs", "one two\n");
    let md = a.parent().unwrap().join("b.md");
    let mut p = open_writing(&env, &a, "");
    p.wait_for("1 │ one two", TIMEOUT);
    save_as(&mut p, md.to_str().unwrap());
    wait_status(&p, "2 words");
    quit_clean(&mut p);
}

#[test]
fn selecting_text_counts_its_words() {
    let env = Env::new();
    let f = env.write("notes.md", "one two three\n");
    let mut p = open_writing(&env, &f, "");
    p.wait_for("one two", TIMEOUT);
    for _ in 0..7 {
        p.send(b"\x1b[1;2C");
    }
    wait_status(&p, "2 words selected");
    p.send_keys(&["C-q"]);
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

// ---------------------------------------------------------------- spelling

/// Whether `word` shows underlined. Screen columns are found from its text.
#[cfg(feature = "spell")]
fn word_underlined(p: &Pty, word: &str) -> bool {
    let screen = p.screen();
    screen.lines().enumerate().any(|(row, line)| {
        line.find(word).is_some_and(|at| {
            let col = line[..at].chars().count() as u16;
            p.underlined(row as u16, col)
        })
    })
}

#[cfg(feature = "spell")]
fn wait_underlined(p: &Pty, word: &str) {
    let deadline = Instant::now() + TIMEOUT;
    while !word_underlined(p, word) {
        assert!(
            Instant::now() < deadline,
            "{word} isn't underlined\n{}",
            p.dump()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "spell")]
#[test]
fn a_misspelled_word_is_underlined_and_can_be_fixed_then_undone() {
    let env = Env::new();
    let f = env.write("notes.md", "This is teh word.\n");
    let mut p = open_writing(&env, &f, "");
    p.wait_for("This is teh word", TIMEOUT);
    wait_underlined(&p, "teh");
    assert!(!word_underlined(&p, "This"), "{}", p.dump());
    open_overlay(&mut p);
    p.wait_for("next misspelling", TIMEOUT);
    assert!(
        !p.screen().contains("fix spelling"),
        "not on a misspelling yet"
    );
    p.send_str("m");
    wait_status(&p, "Col 9");
    open_overlay(&mut p);
    p.wait_for("fix spelling", TIMEOUT);
    p.send_str("z");
    p.wait_for("Fix teh", TIMEOUT);
    p.wait_for("▸ the", TIMEOUT);
    p.wait_for("Add teh to my words", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("This is the word", TIMEOUT);
    p.send_keys(&["C-z"]);
    p.wait_for("This is teh word", TIMEOUT);
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[cfg(feature = "spell")]
#[test]
fn a_word_you_add_stays_added_in_the_next_session() {
    let env = Env::new();
    let f = env.write("notes.md", "A kytely word.\n");
    let mut p = open_writing(&env, &f, "");
    wait_underlined(&p, "kytely");
    esc_then(&mut p, "m");
    open_overlay(&mut p);
    p.send_str("z");
    p.wait_for("Fix kytely", TIMEOUT);
    p.send_str("add");
    p.wait_for("▸ Add kytely to my words", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("Added kytely to your words", TIMEOUT);
    assert_eq!(
        fs::read_to_string(env.path("words.txt")).unwrap(),
        "kytely\n"
    );
    quit_clean(&mut p);
    let mut p = open_writing(&env, &f, "");
    p.wait_for("A kytely word", TIMEOUT);
    assert!(!word_underlined(&p, "kytely"), "{}", p.dump());
    quit_clean(&mut p);
}

#[cfg(feature = "spell")]
#[test]
fn ignoring_a_word_lasts_only_until_kyte_closes() {
    let env = Env::new();
    let f = env.write("notes.md", "A kytely word.\n");
    let mut p = open_writing(&env, &f, "");
    wait_underlined(&p, "kytely");
    esc_then(&mut p, "m");
    open_overlay(&mut p);
    p.send_str("z");
    p.wait_for("Fix kytely", TIMEOUT);
    p.send_str("ignore");
    p.wait_for("▸ Ignore kytely for now", TIMEOUT);
    p.send_keys(&["Enter"]);
    wait_insert(&p);
    assert!(!env.path("words.txt").exists());
    assert!(!word_underlined(&p, "kytely"), "{}", p.dump());
    quit_clean(&mut p);
    let mut p = open_writing(&env, &f, "");
    wait_underlined(&p, "kytely");
    quit_clean(&mut p);
}

#[cfg(all(feature = "spell", feature = "highlight"))]
#[test]
fn latex_commands_math_and_citations_arent_spell_checked() {
    let env = Env::new();
    let text = concat!(
        "\\documentclass{tehclass}\n",
        "\\begin{document}\n",
        "\\section{Introdction}\n",
        "Prose wrongg here \\tehcmd{} and \\cite{tehkey} and $tehmath$.\n",
        "% tehcomment\n",
        "\\label{sec:tehlabel}\n",
        "\\end{document}\n",
    );
    let f = env.write("paper.tex", text);
    let mut p = open_writing(&env, &f, "");
    wait_underlined(&p, "wrongg");
    wait_underlined(&p, "Introdction");
    for fine in [
        "tehclass",
        "tehcmd",
        "tehkey",
        "tehmath",
        "tehcomment",
        "tehlabel",
    ] {
        assert!(!word_underlined(&p, fine), "{fine}\n{}", p.dump());
    }
    quit_clean(&mut p);
}

#[cfg(all(feature = "spell", feature = "highlight"))]
#[test]
fn code_in_markdown_isnt_spell_checked() {
    let env = Env::new();
    let f = env.write("a.md", "Some wrongg text.\n\n```\nlet tehvar = 1;\n```\n");
    let mut p = open_writing(&env, &f, "");
    wait_underlined(&p, "wrongg");
    assert!(!word_underlined(&p, "tehvar"), "{}", p.dump());
    quit_clean(&mut p);
}

#[cfg(feature = "spell")]
#[test]
fn code_files_have_no_spell_check_until_asked() {
    let env = Env::new();
    let f = env.write("a.xyz", "wrongg\n");
    let mut p = open_writing(&env, &f, "");
    p.wait_for("wrongg", TIMEOUT);
    assert!(!word_underlined(&p, "wrongg"), "{}", p.dump());
    open_overlay(&mut p);
    assert!(!p.screen().contains("next misspelling"), "{}", p.dump());
    p.send_keys(&["Esc"]);
    p.wait_for("Command palette", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    run_from_palette(&mut p, "Toggle spell check");
    wait_underlined(&p, "wrongg");
    quit_clean(&mut p);
}

#[cfg(feature = "spell")]
#[test]
fn the_setting_turns_spell_check_off() {
    let env = Env::new();
    let f = env.write("a.md", "wrongg\n");
    let mut p = open_writing(&env, &f, "spell_check = false\n");
    p.wait_for("wrongg", TIMEOUT);
    wait_status(&p, "1 word");
    assert!(!word_underlined(&p, "wrongg"), "{}", p.dump());
    esc_then(&mut p, "m");
    p.wait_for("Spell check is off", TIMEOUT);
    quit_clean(&mut p);
}

#[cfg(feature = "spell")]
#[test]
fn next_misspelling_wraps_and_says_when_there_are_none() {
    let env = Env::new();
    let f = env.write("a.md", "one wrongg\ntwo\nthree wrongger\n");
    let mut p = open_writing(&env, &f, "");
    wait_underlined(&p, "wrongg");
    esc_then(&mut p, "m");
    wait_status(&p, "Ln 1, Col 5");
    esc_then(&mut p, "m");
    wait_status(&p, "Ln 3, Col 7");
    esc_then(&mut p, "m");
    wait_status(&p, "Ln 1, Col 5");
    quit_clean(&mut p);
    let clean = env.write("b.md", "all fine\n");
    let mut p = open_writing(&env, &clean, "");
    esc_then(&mut p, "m");
    p.wait_for("No misspellings", TIMEOUT);
    quit_clean(&mut p);
}

#[cfg(feature = "highlight")]
#[test]
fn a_latex_word_count_catches_up_when_typing_pauses() {
    let env = Env::new();
    let f = env.write("paper.tex", "One two.\n");
    let mut p = open_writing(&env, &f, "");
    wait_status(&p, "2 words");
    type_text(&mut p, "three ");
    wait_status(&p, "3 words");
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

// ---------------------------------------------------------------- citations

const REFS: &str = "@article{knuth84,\n  author = {Knuth, Donald},\n  title = {Literate Programming},\n  year = 1984,\n}\n@book{lamport94,\n  author = {Lamport, Leslie},\n  title = {LaTeX: A Document Preparation System},\n  year = 1994,\n}\n";

/// A folder with `refs.bib`, and Kyte open on a paper in it.
fn open_paper(env: &Env, tex: &str) -> (PathBuf, Pty) {
    let paper = work_file(env, "paper.tex", tex);
    fs::write(paper.parent().unwrap().join("refs.bib"), REFS).unwrap();
    let p = open_writing(env, &paper, "");
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    (paper, p)
}

/// Quits without saving and asks nothing more.
fn abandon(p: &mut Pty) {
    p.send_keys(&["C-q"]);
    p.wait_for("Unsaved changes", TIMEOUT);
    p.send_str("n");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn typing_a_citation_offers_the_bibliography_and_inserts_the_key() {
    let env = Env::new();
    let (_paper, mut p) = open_paper(&env, "As shown.\n");
    p.send_str("\\cite{");
    p.wait_for(" Cite ", TIMEOUT);
    p.wait_for("knuth84  Knuth, Literate Programming (1984)", TIMEOUT);
    p.wait_for("lamport94  Lamport", TIMEOUT);
    p.send_str("lamport");
    p.wait_for("▸ lamport94", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("\\cite{lamport94}As shown.", TIMEOUT);
    p.wait_gone(" Cite ", TIMEOUT);
    p.send_str(" more");
    p.wait_for("\\cite{lamport94} moreAs shown.", TIMEOUT);
    abandon(&mut p);
}

#[test]
fn a_citation_can_be_found_by_author_or_title_and_holds_several_works() {
    let env = Env::new();
    let (_paper, mut p) = open_paper(&env, "x\n");
    p.send_str("\\citep[p. 3]{");
    p.wait_for(" Cite ", TIMEOUT);
    p.send_str("Literate");
    p.wait_for("▸ knuth84", TIMEOUT);
    p.send_str(",");
    p.wait_for("\\citep[p. 3]{knuth84,x", TIMEOUT);
    p.send_str("Preparation");
    p.wait_for("▸ lamport94", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("\\citep[p. 3]{knuth84,lamport94}x", TIMEOUT);
    abandon(&mut p);
}

#[test]
fn esc_leaves_the_citation_as_typed() {
    let env = Env::new();
    let (_paper, mut p) = open_paper(&env, "x\n");
    p.send_str("\\cite{");
    p.wait_for(" Cite ", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_gone(" Cite ", TIMEOUT);
    p.wait_for("\\cite{x", TIMEOUT);
    abandon(&mut p);
}

#[test]
fn esc_q_starts_a_citation_or_fills_in_the_one_at_the_cursor() {
    let env = Env::new();
    let (_paper, mut p) = open_paper(&env, "\\cite{}\n");
    p.send_keys(&["End"]);
    open_overlay(&mut p);
    p.wait_for("insert citation", TIMEOUT);
    p.send_str("q");
    p.wait_for(" Cite ", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("\\cite{}\\cite{knuth84}", TIMEOUT);
    p.send_keys(&["Left"]);
    p.send_keys(&["Home"]);
    for _ in 0..6 {
        p.send_keys(&["Right"]);
    }
    wait_status(&p, "Col 7");
    open_overlay(&mut p);
    p.send_str("q");
    p.wait_for(" Cite ", TIMEOUT);
    p.send_str("lamport");
    p.wait_for("▸ lamport94", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("\\cite{lamport94}\\cite{knuth84}", TIMEOUT);
    wait_status(&p, "Col 17");
    abandon(&mut p);
}

#[test]
fn with_no_bibliography_typing_is_left_alone_and_asking_says_so() {
    let env = Env::new();
    let paper = work_file(&env, "paper.tex", "x\n");
    let mut p = open_writing(&env, &paper, "");
    p.send_str("\\cite{");
    p.wait_for("\\cite{x", TIMEOUT);
    assert!(!p.screen().contains(" Cite "), "{}", p.dump());
    esc_then(&mut p, "q");
    p.wait_for("No references", TIMEOUT);
    abandon(&mut p);
}

#[test]
fn the_bibliography_the_paper_names_wins_over_one_beside_it() {
    let env = Env::new();
    let paper = work_file(&env, "paper.tex", "\\bibliography{lib/mine}\n");
    let dir = paper.parent().unwrap();
    fs::write(dir.join("other.bib"), "@misc{ignored, title = {No}}\n").unwrap();
    fs::create_dir(dir.join("lib")).unwrap();
    fs::write(dir.join("lib/mine.bib"), "@misc{chosen, title = {Yes}}\n").unwrap();
    let mut p = open_writing(&env, &paper, "");
    esc_then_box(&mut p);
    p.wait_for("chosen  Yes", TIMEOUT);
    assert!(!p.screen().contains("ignored"), "{}", p.dump());
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    abandon(&mut p);
}

fn esc_then_box(p: &mut Pty) {
    open_overlay(p);
    p.send_str("q");
    p.wait_for(" Cite ", TIMEOUT);
}

#[test]
fn a_reference_added_but_not_yet_saved_can_be_cited() {
    let env = Env::new();
    let (paper, mut p) = open_paper(&env, "x\n");
    let refs = paper.parent().unwrap().join("refs.bib");
    open_from_prompt(&mut p, refs.to_str().unwrap());
    p.wait_for("@article{knuth84", TIMEOUT);
    p.send_str("@misc{fresh, title = {New}}");
    p.send_keys(&["Enter"]);
    p.wait_for("@misc{fresh", TIMEOUT);
    esc_then(&mut p, ",");
    wait_status(&p, "paper.tex");
    esc_then_box(&mut p);
    p.wait_for("fresh  New", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    run_from_palette(&mut p, "Quit without saving");
    p.wait_for("Discard unsaved changes", TIMEOUT);
    p.send_str("y");
    assert_eq!(p.wait_exit(TIMEOUT), 0);
}

#[test]
fn other_files_type_cite_as_plain_text() {
    let env = Env::new();
    let f = work_file(&env, "notes.md", "x\n");
    fs::write(f.parent().unwrap().join("refs.bib"), REFS).unwrap();
    let mut p = open_writing(&env, &f, "");
    p.send_str("\\cite{");
    p.wait_for("\\cite{x", TIMEOUT);
    assert!(!p.screen().contains(" Cite "), "{}", p.dump());
    abandon(&mut p);
}

// ---------------------------------------------------------------- building PDFs

/// A paper, with `build_command` set to a script of the test's own.
fn open_build(env: &Env, tex: &str, script: &str) -> (PathBuf, Pty) {
    let paper = work_file(env, "paper.tex", tex);
    let dir = paper.parent().unwrap();
    fs::write(dir.join("build.sh"), script).unwrap();
    let settings = "build_command = \"sh build.sh %f\"\npdf_command = \"sh show.sh %f\"\n";
    let p = open_writing(env, &paper, settings);
    (paper, p)
}

#[test]
fn ctrl_b_saves_then_builds_in_the_papers_folder() {
    let env = Env::new();
    let script = "cat \"$1\" > seen.txt; pwd > where.txt; touch paper.pdf\n";
    let (paper, mut p) = open_build(&env, "text\n", script);
    type_text(&mut p, "new ");
    p.send_keys(&["C-b"]);
    p.wait_for("Built paper.pdf in", TIMEOUT);
    let dir = paper.parent().unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("seen.txt")).unwrap(),
        "new text\n"
    );
    let found = fs::read_to_string(dir.join("where.txt")).unwrap();
    assert_eq!(Path::new(found.trim()), dir.canonicalize().unwrap());
    assert_eq!(fs::read_to_string(&paper).unwrap(), "new text\n");
    quit_clean(&mut p);
}

#[test]
fn a_failed_build_lists_its_errors_and_enter_goes_to_the_line() {
    let env = Env::new();
    let script = "echo './paper.tex:3: Undefined control sequence.'; echo './paper.tex:1: Other.'; exit 12\n";
    let (_paper, mut p) = open_build(&env, "one\ntwo\nthree\n", script);
    p.send_keys(&["C-b"]);
    p.wait_for("2 build problems", TIMEOUT);
    p.wait_for("▸ paper.tex:3  Undefined control sequence.", TIMEOUT);
    p.send_keys(&["Enter"]);
    wait_status(&p, "Ln 3, Col 1");
    assert!(!p.screen().contains("build problems"), "{}", p.dump());
    run_from_palette(&mut p, "Show build problems");
    p.wait_for("2 build problems", TIMEOUT);
    p.send_keys(&["Down", "Enter"]);
    wait_status(&p, "Ln 1, Col 1");
    quit_clean(&mut p);
}

#[test]
fn an_error_in_another_file_opens_that_file() {
    let env = Env::new();
    let script = "echo './ch1.tex:2: Boom.'; exit 1\n";
    let (paper, mut p) = open_build(&env, "\\input{ch1}\n", script);
    fs::write(paper.parent().unwrap().join("ch1.tex"), "a\nb\n").unwrap();
    p.send_keys(&["C-b"]);
    p.wait_for("▸ ch1.tex:2  Boom.", TIMEOUT);
    p.send_keys(&["Enter"]);
    wait_status(&p, "ch1.tex");
    wait_status(&p, "Ln 2, Col 1");
    quit_clean(&mut p);
}

#[test]
fn a_build_with_undefined_references_succeeds_and_says_so() {
    let env = Env::new();
    let script = "echo \"LaTeX Warning: Citation \\`foo' on page 1 undefined on input line 2.\"\n";
    let (_paper, mut p) = open_build(&env, "one\ntwo\n", script);
    p.send_keys(&["C-b"]);
    p.wait_for("Built paper.pdf with 1 warning", TIMEOUT);
    assert!(
        !p.screen().contains("build problem "),
        "no box for a warning\n{}",
        p.dump()
    );
    run_from_palette(&mut p, "Show build problems");
    p.wait_for("▸ paper.tex:2  Citation `foo' on page 1 undefined", TIMEOUT);
    p.send_keys(&["Enter"]);
    wait_status(&p, "Ln 2, Col 1");
    quit_clean(&mut p);
}

#[test]
fn a_failure_with_nothing_to_list_says_what_it_can() {
    let env = Env::new();
    let (_paper, mut p) = open_build(&env, "x\n", "echo 'it went wrong'; exit 3\n");
    p.send_keys(&["C-b"]);
    p.wait_for("Build failed: it went wrong", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn typing_carries_on_during_a_build_and_it_can_be_stopped() {
    let env = Env::new();
    let script = "sleep 2 && touch finished.txt\n";
    let (paper, mut p) = open_build(&env, "x\n", script);
    p.send_keys(&["C-b"]);
    wait_status(&p, "building PDF");
    type_text(&mut p, "typed ");
    p.send_keys(&["C-b"]);
    p.wait_for("Already building", TIMEOUT);
    run_from_palette(&mut p, "Stop build");
    p.wait_for("Build stopped", TIMEOUT);
    assert!(!status(&p).contains("building"), "{}", status(&p));
    std::thread::sleep(Duration::from_millis(2300));
    assert!(!paper.parent().unwrap().join("finished.txt").exists());
    run_from_palette(&mut p, "Stop build");
    p.wait_for("No build is running", TIMEOUT);
    abandon(&mut p);
}

#[test]
fn quitting_stops_a_build_and_doesnt_wait_for_it() {
    let env = Env::new();
    let script = "sleep 3 && touch finished.txt\n";
    let (paper, mut p) = open_build(&env, "x\n", script);
    p.send_keys(&["C-b"]);
    wait_status(&p, "building PDF");
    let start = Instant::now();
    quit_clean(&mut p);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    std::thread::sleep(Duration::from_millis(3300));
    assert!(!paper.parent().unwrap().join("finished.txt").exists());
}

#[test]
fn only_latex_builds_and_a_missing_latexmk_is_explained() {
    let env = Env::new();
    let f = work_file(&env, "notes.md", "x\n");
    let mut p = open_writing(&env, &f, "");
    p.send_keys(&["C-b"]);
    p.wait_for("Build PDF works on LaTeX files", TIMEOUT);
    quit_clean(&mut p);
    let paper = work_file(&env, "paper.tex", "x\n");
    let empty = env.path("no-programs");
    fs::create_dir(&empty).unwrap();
    let mut p = Pty::spawn(
        Spawn::new(bin())
            .env("KYTE_STATE_DIR", env.state.to_string_lossy())
            .env("PATH", empty.to_string_lossy())
            .arg("--no-kitty")
            .arg(paper.to_str().unwrap()),
    );
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    p.send_keys(&["C-b"]);
    p.wait_for("latexmk isn't installed", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn latex_tools_work_with_the_writing_setup_off() {
    let env = Env::new();
    let paper = work_file(&env, "paper.tex", "x\n");
    let dir = paper.parent().unwrap();
    fs::write(dir.join("refs.bib"), REFS).unwrap();
    let settings = "writing = false\nbuild_command = \"true\"\n";
    let mut p = open_writing(&env, &paper, settings);
    p.wait_for("1 │ x", TIMEOUT);
    assert!(!status(&p).contains("word"), "{}", status(&p));
    p.send_str("\\cite{");
    p.wait_for(" Cite ", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("\\cite{knuth84}x", TIMEOUT);
    p.send_keys(&["C-b"]);
    p.wait_for("Built paper.pdf", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn a_chapter_builds_the_root_it_names() {
    let env = Env::new();
    let paper = work_file(&env, "ch1.tex", "% !TEX root = thesis.tex\ntext\n");
    let dir = paper.parent().unwrap();
    fs::write(dir.join("thesis.tex"), "\\documentclass{book}\n").unwrap();
    fs::write(dir.join("build.sh"), "echo \"$1\" > built.txt\n").unwrap();
    let mut p = open_writing(&env, &paper, "build_command = \"sh build.sh %f\"\n");
    p.send_keys(&["C-b"]);
    p.wait_for("Built thesis.pdf", TIMEOUT);
    assert_eq!(
        fs::read_to_string(dir.join("built.txt")).unwrap(),
        "thesis.tex\n"
    );
    quit_clean(&mut p);
}

#[test]
fn esc_o_shows_the_pdf_once_there_is_one() {
    let env = Env::new();
    let (paper, mut p) = open_build(&env, "x\n", "touch paper.pdf\n");
    let dir = paper.parent().unwrap();
    fs::write(dir.join("show.sh"), "echo \"$1\" > shown.txt\n").unwrap();
    open_overlay(&mut p);
    p.wait_for("open PDF", TIMEOUT);
    p.send_str("o");
    p.wait_for("No PDF yet", TIMEOUT);
    p.send_keys(&["C-b"]);
    p.wait_for("Built paper.pdf", TIMEOUT);
    esc_then(&mut p, "o");
    let shown = dir.join("shown.txt");
    let deadline = Instant::now() + TIMEOUT;
    while !shown.exists() {
        assert!(Instant::now() < deadline, "the viewer didn't run");
        std::thread::sleep(Duration::from_millis(10));
    }
    quit_clean(&mut p);
    assert_eq!(fs::read_to_string(&shown).unwrap().trim(), "paper.pdf");
}

#[test]
fn a_build_problem_goes_to_the_paper_that_was_built_whatever_is_on_screen() {
    let env = Env::new();
    let script = "echo './paper.tex:2: Boom.'; exit 1\n";
    let settings = "build_command = \"sh build.sh %f\"\n";
    let root = env.path("papers");
    let (a, b) = (root.join("a"), root.join("b"));
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::write(a.join("paper.tex"), "a1\nright project\n").unwrap();
    fs::write(b.join("paper.tex"), "b1\nwrong project\n").unwrap();
    fs::write(a.join("build.sh"), script).unwrap();
    let notes = root.join("notes.rs");
    fs::write(&notes, "fn notes() {}\n").unwrap();
    let mut p = open_writing(&env, &a.join("paper.tex"), settings);
    p.send_keys(&["C-b"]);
    p.wait_for("▸ paper.tex:2  Boom.", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    open_from_prompt(&mut p, b.join("paper.tex").to_str().unwrap());
    p.wait_for("wrong project", TIMEOUT);
    run_from_palette(&mut p, "Show build problems");
    p.wait_for("▸ paper.tex:2  Boom.", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("right project", TIMEOUT);
    wait_status(&p, "Ln 2, Col 1");
    assert!(!p.screen().contains("Already open"), "{}", p.dump());
    open_from_prompt(&mut p, notes.to_str().unwrap());
    p.wait_for("fn notes", TIMEOUT);
    run_from_palette(&mut p, "Show build problems");
    p.wait_for("▸ paper.tex:2  Boom.", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("right project", TIMEOUT);
    quit_clean(&mut p);
}

#[test]
fn a_build_problem_in_a_file_thats_gone_says_so_and_makes_nothing() {
    let env = Env::new();
    let script = "echo './gone.tex:2: Boom.'; exit 1\n";
    let (paper, mut p) = open_build(&env, "x\n", script);
    p.send_keys(&["C-b"]);
    p.wait_for("▸ gone.tex:2  Boom.", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("gone.tex is gone", TIMEOUT);
    assert!(!status(&p).contains("gone.tex"), "{}", status(&p));
    assert!(!paper.parent().unwrap().join("gone.tex").exists());
    quit_clean(&mut p);
}

#[test]
fn a_warning_inside_a_chapter_opens_that_chapter() {
    let env = Env::new();
    let script = "cat log.txt\n";
    let (paper, mut p) = open_build(&env, "root one\nroot two\n", script);
    let dir = paper.parent().unwrap();
    fs::create_dir(dir.join("ch")).unwrap();
    fs::write(dir.join("ch/one.tex"), "a\nb\nchapter line\n").unwrap();
    let log = "This is pdfTeX\n(./paper.tex\n(./ch/one.tex\n\nLaTeX Warning: Citation `x' on page 1 undefined on input line 3.\n\n))\n";
    fs::write(dir.join("log.txt"), log).unwrap();
    p.send_keys(&["C-b"]);
    p.wait_for("with 1 warning", TIMEOUT);
    run_from_palette(&mut p, "Show build problems");
    p.wait_for("▸ ch/one.tex:3  Citation", TIMEOUT);
    p.send_keys(&["Enter"]);
    p.wait_for("chapter line", TIMEOUT);
    wait_status(&p, "one.tex");
    wait_status(&p, "Ln 3, Col 1");
    quit_clean(&mut p);
}

#[test]
fn a_bibliography_that_isnt_a_plain_file_cant_hang_the_editor() {
    let env = Env::new();
    let paper = work_file(&env, "paper.tex", "x\n");
    let dir = paper.parent().unwrap();
    fs::write(dir.join("good.bib"), REFS).unwrap();
    std::os::unix::fs::symlink("/dev/zero", dir.join("zero.bib")).unwrap();
    let fifo = dir.join("pipe.bib");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let huge = fs::File::create(dir.join("huge.bib")).unwrap();
    huge.set_len(33 * 1024 * 1024).unwrap();
    let mut p = open_writing(&env, &paper, "");
    p.send_str("\\cite{");
    p.wait_for(" Cite ", TIMEOUT);
    p.wait_for("knuth84", TIMEOUT);
    p.wait_for("Left out 3 bibliography files", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    abandon(&mut p);
}

#[test]
fn a_named_bibliography_outside_the_folder_still_works() {
    let env = Env::new();
    let refs = env.path("shared/lib.bib");
    fs::create_dir_all(refs.parent().unwrap()).unwrap();
    fs::write(&refs, "@misc{far, title = {Away}}\n").unwrap();
    let tex = format!("\\bibliography{{{}}}\n", refs.display());
    let paper = work_file(&env, "paper.tex", &tex);
    let mut p = open_writing(&env, &paper, "");
    esc_then_box(&mut p);
    p.wait_for("far  Away", TIMEOUT);
    p.send_keys(&["Esc"]);
    wait_insert(&p);
    abandon(&mut p);
}

#[cfg(all(feature = "spell", feature = "highlight"))]
#[test]
fn next_misspelling_walks_a_long_paper_in_batches_and_wraps() {
    let env = Env::new();
    let body = "A fine line of text.\n".repeat(450);
    let tex = format!("wrongg top\n{body}$tehmath$ and wrongger there\n");
    let f = env.write("paper.tex", &tex);
    let mut p = open_writing(&env, &f, "");
    wait_underlined(&p, "wrongg");
    esc_then(&mut p, "m");
    wait_status(&p, "Ln 452, Col 15");
    esc_then(&mut p, "m");
    wait_status(&p, "Ln 1, Col 1");
    quit_clean(&mut p);
}
