//! Headless pty harness: spawn a program in a pseudo-terminal, feed its output
//! into a vt100 emulator, send bytes or named keys, wait for screen text, and
//! record an asciicast v2 file. Shared by `tests/e2e.rs`, `examples/demo.rs`
//! and `examples/keylab.rs` (via `#[path]`).
#![allow(dead_code)]

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Default e2e terminal size (section 10).
pub const ROWS: u16 = 30;
pub const COLS: u16 = 100;
/// Default timeout for `wait_for`.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Collects output chunks with timestamps, possibly across several processes
/// (the demo kills kyte and relaunches it into the same recording).
#[derive(Clone)]
pub struct Recorder {
    start: Instant,
    events: Arc<Mutex<Vec<(f64, String)>>>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    pub fn new() -> Recorder {
        Recorder {
            start: Instant::now(),
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Seconds since the recorder started.
    pub fn elapsed(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn push(&self, data: String) {
        let t = self.elapsed();
        self.events.lock().unwrap().push((t, data));
    }

    /// Writes asciicast v2: a JSON header line, then `[seconds, "o", data]` lines.
    pub fn write_cast(&self, path: &Path, cols: u16, rows: u16) -> std::io::Result<()> {
        let mut out = String::new();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push_str(&format!(
            "{{\"version\": 2, \"width\": {cols}, \"height\": {rows}, \"timestamp\": {ts}, \"env\": {{\"TERM\": \"xterm-256color\", \"SHELL\": \"/bin/sh\"}}}}\n"
        ));
        for (t, data) in self.events.lock().unwrap().iter() {
            out.push_str(&format!("[{t:.6}, \"o\", {}]\n", json_string(data)));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, out)
    }
}

/// JSON string literal with escapes.
pub fn json_string(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                o.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// What to spawn.
pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
    pub recorder: Option<Recorder>,
}

impl Spawn {
    pub fn new(program: impl Into<PathBuf>) -> Spawn {
        Spawn {
            program: program.into(),
            args: Vec::new(),
            // Tests must never read the settings of whoever runs them.
            env: vec![(
                "KYTE_CONFIG".to_string(),
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/settings.toml").to_string(),
            )],
            cwd: None,
            rows: ROWS,
            cols: COLS,
            recorder: None,
        }
    }
    pub fn arg(mut self, a: impl AsRef<str>) -> Spawn {
        self.args.push(a.as_ref().to_string());
        self
    }
    pub fn env(mut self, k: &str, v: impl AsRef<str>) -> Spawn {
        self.env.push((k.to_string(), v.as_ref().to_string()));
        self
    }
    pub fn size(mut self, rows: u16, cols: u16) -> Spawn {
        self.rows = rows;
        self.cols = cols;
        self
    }
    pub fn recorder(mut self, r: Recorder) -> Spawn {
        self.recorder = Some(r);
        self
    }
    pub fn cwd(mut self, d: impl Into<PathBuf>) -> Spawn {
        self.cwd = Some(d.into());
        self
    }
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap()
}

/// A running program in a pty.
pub struct Pty {
    parser: Arc<Mutex<vt100::Parser>>,
    raw: Arc<Mutex<Vec<u8>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    recorder: Recorder,
    reader: Option<thread::JoinHandle<()>>,
    exit: Option<u32>,
}

impl Pty {
    pub fn spawn(s: Spawn) -> Pty {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: s.rows,
                cols: s.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(&s.program);
        cmd.args(&s.args);
        cmd.env_clear();
        if let Ok(p) = std::env::var("PATH") {
            cmd.env("PATH", p);
        }
        if let Ok(h) = std::env::var("HOME") {
            cmd.env("HOME", h);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("LANG", "C.UTF-8");
        for (k, v) in &s.env {
            cmd.env(k, v);
        }
        let cwd = s
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap());
        cmd.cwd(cwd);
        let child = pair.slave.spawn_command(cmd).expect("spawn in pty");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("pty reader");
        let writer = Arc::new(Mutex::new(pair.master.take_writer().expect("pty writer")));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(nz(s.rows), nz(s.cols), 0)));
        let raw = Arc::new(Mutex::new(Vec::new()));
        let recorder = s.recorder.clone().unwrap_or_default();
        let (p2, r2, w2, rec2) = (
            parser.clone(),
            raw.clone(),
            writer.clone(),
            recorder.clone(),
        );
        let handle = thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut carry: Vec<u8> = Vec::new();
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let chunk = &buf[..n];
                {
                    let mut p = p2.lock().unwrap();
                    p.process(chunk);
                    // Answer terminal queries a real terminal would answer, so
                    // programs that ask never stall: cursor position (DSR 6) and
                    // primary device attributes (no kitty protocol reply).
                    let mut replies = Vec::new();
                    if contains(chunk, b"\x1b[6n") {
                        let (r, c) = p.screen().cursor_position();
                        replies.extend_from_slice(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
                    }
                    if contains(chunk, b"\x1b[c") {
                        replies.extend_from_slice(b"\x1b[?62;22c");
                    }
                    if !replies.is_empty() {
                        let _ = w2.lock().unwrap().write_all(&replies);
                    }
                }
                r2.lock().unwrap().extend_from_slice(chunk);
                carry.extend_from_slice(chunk);
                let valid = match std::str::from_utf8(&carry) {
                    Ok(_) => carry.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    Err(_) => carry.len(),
                };
                let text = String::from_utf8_lossy(&carry[..valid]).into_owned();
                carry.drain(..valid);
                if !text.is_empty() {
                    rec2.push(text);
                }
            }
        });
        Pty {
            parser,
            raw,
            writer,
            master: pair.master,
            child,
            recorder,
            reader: Some(handle),
            exit: None,
        }
    }

    /// Spawns the compiled `kyte` binary with `--no-kitty` and `KYTE_STATE_DIR`.
    pub fn kyte(bin: &Path, state_dir: &Path, args: &[&str]) -> Pty {
        let mut s = Spawn::new(bin)
            .env("KYTE_STATE_DIR", state_dir.to_string_lossy())
            .arg("--no-kitty");
        for a in args {
            s = s.arg(a);
        }
        Pty::spawn(s)
    }

    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).expect("write to pty");
        w.flush().unwrap();
    }

    pub fn send_str(&mut self, s: &str) {
        self.send(s.as_bytes());
    }

    /// Sends named keys, for example `["Esc", "d", "C-s", "Up"]`. Each key is a
    /// separate write. Unknown names are sent as literal text.
    pub fn send_keys(&mut self, keys: &[&str]) {
        for k in keys {
            let b = key_bytes(k);
            self.send(&b);
        }
    }

    /// Bracketed paste of `text`.
    pub fn paste(&mut self, text: &str) {
        let mut b = b"\x1b[200~".to_vec();
        b.extend_from_slice(text.as_bytes());
        b.extend_from_slice(b"\x1b[201~");
        self.send(&b);
    }

    pub fn screen(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    /// Screen rows as strings (no trailing spaces).
    pub fn rows(&self) -> Vec<String> {
        let p = self.parser.lock().unwrap();
        let (_, cols) = p.screen().size();
        p.screen()
            .rows(0, cols.get())
            .map(|r| r.trim_end().to_string())
            .collect()
    }

    pub fn row(&self, n: usize) -> String {
        self.rows().get(n).cloned().unwrap_or_default()
    }

    /// (row, col), zero-based.
    pub fn cursor(&self) -> (u16, u16) {
        self.parser.lock().unwrap().screen().cursor_position()
    }

    pub fn cursor_hidden(&self) -> bool {
        self.parser.lock().unwrap().screen().hide_cursor()
    }

    pub fn alternate_screen(&self) -> bool {
        self.parser.lock().unwrap().screen().alternate_screen()
    }

    /// Background color of the cell at (row, col).
    pub fn bg(&self, row: u16, col: u16) -> Option<vt100::Color> {
        self.parser
            .lock()
            .unwrap()
            .screen()
            .cell(row, col)
            .map(|c| c.bgcolor())
    }

    /// Whether the cell at (row, col) is underlined.
    pub fn underlined(&self, row: u16, col: u16) -> bool {
        self.parser
            .lock()
            .unwrap()
            .screen()
            .cell(row, col)
            .is_some_and(|c| c.underline())
    }

    /// Foreground color of the cell at (row, col).
    pub fn fg(&self, row: u16, col: u16) -> Option<vt100::Color> {
        self.parser
            .lock()
            .unwrap()
            .screen()
            .cell(row, col)
            .map(|c| c.fgcolor())
    }

    pub fn raw_output(&self) -> Vec<u8> {
        self.raw.lock().unwrap().clone()
    }

    /// Waits until the screen contains `text`; panics with a screen dump on timeout.
    pub fn wait_for(&self, text: &str, timeout: Duration) {
        let what = format!("screen to contain {text:?}");
        self.wait_until(&what, timeout, |s| s.contains(text));
    }

    /// Waits until the screen no longer contains `text`.
    pub fn wait_gone(&self, text: &str, timeout: Duration) {
        let what = format!("screen to stop containing {text:?}");
        self.wait_until(&what, timeout, |s| !s.contains(text));
    }

    /// Waits until `pred(screen)` holds; panics with a screen dump on timeout.
    /// Waits for the output to go quiet. A frame can arrive in pieces, so
    /// what a wait is looking for may show before the rest of the frame does,
    /// and whatever the test checks next would see a half-painted screen.
    fn settle(&self) {
        let quiet_for = Duration::from_millis(25);
        let give_up = Instant::now() + Duration::from_secs(1);
        let mut seen = self.raw.lock().unwrap().len();
        let mut quiet_since = Instant::now();
        while quiet_since.elapsed() < quiet_for && Instant::now() < give_up {
            thread::sleep(Duration::from_millis(2));
            let now = self.raw.lock().unwrap().len();
            if now != seen {
                seen = now;
                quiet_since = Instant::now();
            }
        }
    }

    pub fn wait_until(&self, what: &str, timeout: Duration, pred: impl Fn(&str) -> bool) {
        let start = Instant::now();
        loop {
            if pred(&self.screen()) {
                self.settle();
                if pred(&self.screen()) {
                    return;
                }
            }
            if start.elapsed() > timeout {
                panic!(
                    "timed out after {:?} waiting for {what}\n----- screen -----\n{}\n------------------",
                    timeout,
                    self.dump()
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Screen text with row numbers, for failure messages.
    pub fn dump(&self) -> String {
        self.rows()
            .iter()
            .enumerate()
            .map(|(i, r)| format!("{i:02}|{r}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(nz(rows), nz(cols));
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("resize pty");
    }

    pub fn pid(&self) -> u32 {
        self.child.process_id().expect("child pid")
    }

    /// SIGKILL the child (simulates a crash).
    pub fn kill9(&mut self) {
        let pid = self.pid();
        let st = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .expect("run kill");
        assert!(st.success(), "kill -9 {pid} failed");
        let _ = self.wait_exit(TIMEOUT);
    }

    /// Waits for the child to exit and returns its exit code.
    pub fn wait_exit(&mut self, timeout: Duration) -> u32 {
        if let Some(c) = self.exit {
            return c;
        }
        let start = Instant::now();
        loop {
            if let Some(st) = self.child.try_wait().expect("try_wait") {
                // Let the reader drain the final output.
                if let Some(h) = self.reader.take() {
                    let _ = h.join();
                }
                self.exit = Some(st.exit_code());
                return st.exit_code();
            }
            if start.elapsed() > timeout {
                panic!("process did not exit within {timeout:?}\n{}", self.dump());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn try_exit(&mut self) -> Option<u32> {
        if self.exit.is_some() {
            return self.exit;
        }
        self.child.try_wait().ok().flatten().map(|s| s.exit_code())
    }

    /// Writes everything recorded so far as an asciicast v2 file.
    pub fn record_cast(&self, path: &Path) -> std::io::Result<()> {
        let p = self.parser.lock().unwrap();
        let (rows, cols) = p.screen().size();
        self.recorder.write_cast(path, cols.get(), rows.get())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if self.exit.is_none() {
            if let Ok(None) = self.child.try_wait() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Legacy (xterm) byte encoding of a named key.
pub fn key_bytes(name: &str) -> Vec<u8> {
    let b: &[u8] = match name {
        "Esc" => b"\x1b",
        "Enter" => b"\r",
        "Tab" => b"\t",
        "Backspace" | "BS" => b"\x7f",
        "Delete" | "Del" => b"\x1b[3~",
        "Up" => b"\x1b[A",
        "Down" => b"\x1b[B",
        "Right" => b"\x1b[C",
        "Left" => b"\x1b[D",
        "Home" => b"\x1b[H",
        "End" => b"\x1b[F",
        "PageUp" => b"\x1b[5~",
        "PageDown" => b"\x1b[6~",
        "C-Home" => b"\x1b[1;5H",
        "C-End" => b"\x1b[1;5F",
        "C-Space" => b"\x00",
        _ => {
            if let Some(c) = name.strip_prefix("C-") {
                let ch = c.as_bytes();
                if ch.len() == 1 && ch[0].is_ascii_alphabetic() {
                    return vec![ch[0].to_ascii_lowercase() - b'a' + 1];
                }
            }
            if let Some(c) = name.strip_prefix("M-") {
                let mut v = b"\x1b".to_vec();
                v.extend_from_slice(c.as_bytes());
                return v;
            }
            return name.as_bytes().to_vec();
        }
    };
    b.to_vec()
}
