//! Key-input lab (task T1).
//!
//! `cargo run --example keylab` puts the terminal in raw mode with bracketed
//! paste and prints every crossterm event with a timestamp, its raw debug
//! string and Kyte's mapping. Ctrl-C quits. `--kitty` also tries to push the
//! kitty DISAMBIGUATE_ESCAPE_CODES flag. `--log FILE` appends each line to FILE.
//!
//! `cargo run --example keylab -- --drive [--tmux]` runs the lab itself inside
//! the headless pty harness (optionally through tmux with escape-time 10),
//! sends each key sequence of T1 and prints a Markdown table of what arrived.

#[path = "../tests/common/mod.rs"]
mod common;

use std::io::Write;
use std::time::{Duration, Instant};

use kyte::input::{map_event, normalize, Input};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--drive") {
        drive(args.iter().any(|a| a == "--tmux"));
        return Ok(());
    }
    let kitty = args.iter().any(|a| a == "--kitty");
    let log = args
        .iter()
        .position(|a| a == "--log")
        .and_then(|i| args.get(i + 1).cloned());
    lab(kitty, log)
}

fn lab(kitty: bool, log: Option<String>) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    terminal::enable_raw_mode()?;
    execute!(out, EnableBracketedPaste)?;
    let mut pushed = false;
    if kitty && terminal::supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        pushed = true;
    }
    let mut logf = log.map(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .expect("open log")
    });
    let start = Instant::now();
    let banner = format!("keylab ready (kitty pushed: {pushed}); Ctrl-C quits");
    write!(out, "{banner}\r\n")?;
    if let Some(f) = logf.as_mut() {
        writeln!(f, "{banner}")?;
    }
    out.flush()?;
    loop {
        let ev = event::read()?;
        let quit = matches!(&ev, Event::Key(k) if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL));
        let mapped = map_event(ev.clone());
        let norm = match &mapped {
            Some(Input::Key(k)) => format!("{:?}", normalize(k.clone(), true)),
            _ => "-".to_string(),
        };
        let line = format!(
            "EV {:9.3}ms | {:?} | {:?} | {}",
            start.elapsed().as_secs_f64() * 1000.0,
            ev,
            mapped,
            norm
        );
        write!(out, "{line}\r\n")?;
        out.flush()?;
        if let Some(f) = logf.as_mut() {
            writeln!(f, "{line}")?;
        }
        if quit {
            break;
        }
    }
    if pushed {
        execute!(out, PopKeyboardEnhancementFlags)?;
    }
    execute!(out, DisableBracketedPaste)?;
    terminal::disable_raw_mode()?;
    Ok(())
}

/// One row of the lab: a name and a list of (bytes, pause-after) writes.
struct Case {
    name: &'static str,
    sends: Vec<(Vec<u8>, u64)>,
}

fn case(name: &'static str, sends: &[(&[u8], u64)]) -> Case {
    Case {
        name,
        sends: sends.iter().map(|(b, p)| (b.to_vec(), *p)).collect(),
    }
}

fn drive(tmux: bool) {
    let exe = std::env::current_exe().expect("current exe");
    let dir = tempfile::tempdir().expect("tempdir");
    let log = dir.path().join("keylab.log");
    let spawn = if tmux {
        let conf = dir.path().join("tmux.conf");
        std::fs::write(&conf, "set -g escape-time 10\nset -g status off\n").unwrap();
        let sock = dir.path().join("sock");
        common::Spawn::new("tmux")
            .arg("-S")
            .arg(sock.to_string_lossy())
            .arg("-f")
            .arg(conf.to_string_lossy())
            .arg("new-session")
            .arg(format!("{} --log {}", exe.display(), log.display()))
    } else {
        common::Spawn::new(&exe)
            .arg("--log")
            .arg(log.to_string_lossy())
    };
    let mut pty = common::Pty::spawn(spawn);
    let t0 = Instant::now();
    while !std::fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("keylab ready")
    {
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "keylab did not start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let cases = vec![
        case("Esc alone", &[(b"\x1b", 400)]),
        case(
            "Esc, Esc 100 ms apart (two writes)",
            &[(b"\x1b", 100), (b"\x1b", 400)],
        ),
        case("ESC ESC in one write", &[(b"\x1b\x1b", 400)]),
        case("ESC d in one write (Alt-d)", &[(b"\x1bd", 400)]),
        case("ESC, then d 50 ms later", &[(b"\x1b", 50), (b"d", 400)]),
        case("Up arrow", &[(b"\x1b[A", 300)]),
        case("Down arrow", &[(b"\x1b[B", 300)]),
        case("Left arrow", &[(b"\x1b[D", 300)]),
        case("Right arrow", &[(b"\x1b[C", 300)]),
        case("Home (CSI H)", &[(b"\x1b[H", 300)]),
        case("End (CSI F)", &[(b"\x1b[F", 300)]),
        case("Home (CSI 1~)", &[(b"\x1b[1~", 300)]),
        case("End (CSI 4~)", &[(b"\x1b[4~", 300)]),
        case("PageUp", &[(b"\x1b[5~", 300)]),
        case("PageDown", &[(b"\x1b[6~", 300)]),
        case("Ctrl-Home", &[(b"\x1b[1;5H", 300)]),
        case("Ctrl-End", &[(b"\x1b[1;5F", 300)]),
        case("Ctrl-S (0x13)", &[(b"\x13", 300)]),
        case("Ctrl-Q (0x11)", &[(b"\x11", 300)]),
        case("Ctrl-Z (0x1a)", &[(b"\x1a", 300)]),
        case("Ctrl-Y (0x19)", &[(b"\x19", 300)]),
        case("Ctrl-Space (0x00)", &[(b"\x00", 300)]),
        case("Ctrl-P (0x10)", &[(b"\x10", 300)]),
        case("Backspace (0x7f)", &[(b"\x7f", 300)]),
        case("Delete", &[(b"\x1b[3~", 300)]),
        case("Bracketed paste a\\rb", &[(b"\x1b[200~a\rb\x1b[201~", 400)]),
        case(
            "Up arrow split: ESC, 30 ms, [A",
            &[(b"\x1b", 30), (b"[A", 400)],
        ),
    ];
    println!("| Sent | Bytes | Events observed (crossterm, then Kyte mapping) |");
    println!("| --- | --- | --- |");
    let mut seen = 1; // the banner line
    for c in &cases {
        for (bytes, pause) in &c.sends {
            pty.send(bytes);
            std::thread::sleep(Duration::from_millis(*pause));
        }
        let lines: Vec<String> = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect();
        let new: Vec<String> = lines[seen..]
            .iter()
            .map(|l| {
                let parts: Vec<&str> = l.splitn(4, " | ").collect();
                let ev = parts.get(1).copied().unwrap_or("");
                let mapped = parts.get(2).copied().unwrap_or("");
                format!("`{}` -> `{}`", short(ev), mapped)
            })
            .collect();
        seen = lines.len();
        let bytes: Vec<String> = c
            .sends
            .iter()
            .map(|(b, _)| {
                b.iter()
                    .map(|x| format!("{x:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        println!(
            "| {} | `{}` | {} |",
            c.name,
            bytes.join("` + `"),
            if new.is_empty() {
                "(nothing)".to_string()
            } else {
                new.join("<br>")
            }
        );
    }
    pty.send(b"\x03");
    let _ = pty.wait_exit(Duration::from_secs(5));
}

/// Shortens crossterm's verbose KeyEvent debug output for the table.
fn short(ev: &str) -> String {
    ev.replace("KeyEvent { ", "")
        .replace(", kind: Press, state: KeyEventState(0x0)", "")
        .replace(" }", "")
        .replace("KeyModifiers(0x0)", "NONE")
}
