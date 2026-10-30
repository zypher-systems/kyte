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
