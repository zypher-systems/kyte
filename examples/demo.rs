//! Scripted headless demo (task T11, section 12).
//!
//! `cargo run --release --example demo -- [OUT_DIR]` spawns the release
//! `kyte` in a pty at 100x30 (TERM=xterm-256color, KYTE_STATE_DIR in a temp
//! dir, --no-kitty), plays the storyboard with deliberate pauses, records
//! every output chunk into OUT_DIR/demo.cast (asciicast v2), writes the
//! emulated screen of each step to OUT_DIR/frames/NN-name.txt and the step
//! times to OUT_DIR/steps.tsv. OUT_DIR defaults to docs/demo. `scripts/demo.sh`
//! then renders the gif, mp4 and PNG screenshots.

#[path = "../tests/common/mod.rs"]
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Pty, Recorder, Spawn, COLS, ROWS, TIMEOUT};

const DEMO_LINES: [&str; 12] = [
    "TODO write the intro",
    "Kyte starts in plain insert mode, like Nano.",
    "Arrows move, typing types, Ctrl-S saves.",
    "Press Esc for the one-key operator layer.",
    "  w deletes a word, d a line, k to end of line.",
    "Press Esc twice for the command palette.",
    "fn bracket_demo(x: [u8; 4]) -> u8 {",
    "    x[0] + (x[1] * x[2])",
    "}",
    "Unsaved work is written to a swap file,",
    "so a crash never loses your edits.",
    "Try the palette: Go to line, Find, Save.",
];

struct Demo {
    out: PathBuf,
    rec: Recorder,
    steps: Vec<(String, f64)>,
}

fn ms(n: u64) {
    std::thread::sleep(Duration::from_millis(n));
}

impl Demo {
    /// Writes the current screen as a text frame and records the step time.
    fn snap(&mut self, p: &Pty, name: &str) {
        ms(150); // let the frame settle in the recording
        let t = self.rec.elapsed();
        let text = p.rows().join("\n") + "\n";
        fs::write(self.out.join("frames").join(format!("{name}.txt")), text).unwrap();
        // The screenshot is taken half a second into the hold that follows.
        self.steps.push((name.to_string(), t + 0.5));
        println!("step {name} at {t:.2}s");
        ms(1800);
    }
}

/// Types text one key at a time, like a person.
fn type_slowly(p: &mut Pty, s: &str) {
    for c in s.chars() {
        p.send_str(&c.to_string());
        ms(45);
    }
}

fn kyte_bin() -> PathBuf {
    if let Ok(b) = std::env::var("KYTE_BIN") {
        return PathBuf::from(b);
    }
    // target/<profile>/examples/demo -> target/<profile>/kyte
    let exe = std::env::current_exe().expect("current exe");
    exe.parent()
        .and_then(Path::parent)
        .map(|d| d.join("kyte"))
        .expect("kyte binary next to the examples directory")
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/demo"));
    let frames = out.join("frames");
    fs::create_dir_all(&frames).unwrap();
    for e in fs::read_dir(&frames).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "txt") {
            fs::remove_file(p).unwrap();
        }
    }
    let kyte = kyte_bin();
    assert!(
        kyte.exists(),
        "build kyte first: {} is missing",
        kyte.display()
    );
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::write(tmp.path().join("demo.txt"), DEMO_LINES.join("\n") + "\n").unwrap();
    let spawn = |rec: &Recorder, wrap_shell: bool| {
        let s = if wrap_shell {
            // The relaunch runs under sh so the clean exit is visible at the end.
            Spawn::new("sh").arg("-c").arg(format!(
                "{} --no-kitty demo.txt; s=$?; printf '\\n$ kyte exited with status %s\\n' \"$s\"; sleep 3",
                kyte.display()
            ))
        } else {
            Spawn::new(&kyte).arg("--no-kitty").arg("demo.txt")
        };
        Pty::spawn(
            s.env("KYTE_STATE_DIR", state.to_string_lossy())
                .env("COLORTERM", "truecolor")
                .cwd(tmp.path())
                .size(ROWS, COLS)
                .recorder(rec.clone()),
        )
    };
    let mut d = Demo {
        out: out.clone(),
        rec: Recorder::new(),
        steps: Vec::new(),
    };

    // 01-open
    let mut p = spawn(&d.rec, false);
    p.wait_for("Ln 1, Col 1", TIMEOUT);
    d.snap(&p, "01-open");

    // 02-type: a sentence at the top
    type_slowly(&mut p, "Kyte: comfort first, power one key away. ");
    p.wait_for("[+]", TIMEOUT);
    d.snap(&p, "02-type");

    // 03-legend: Esc, hold
    p.send_keys(&["Esc"]);
    p.wait_for("Esc Esc palette", TIMEOUT);
    d.snap(&p, "03-legend");

    // 04-word: w deletes "TODO "
    p.send_keys(&["w"]);
    p.wait_gone("TODO", TIMEOUT);
    p.wait_gone("Esc Esc palette", TIMEOUT);
    d.snap(&p, "04-word");

    // 05-line: move to line 4, Esc d
    p.send_keys(&["Down", "Down", "Down"]);
    p.wait_for("Ln 4,", TIMEOUT);
    ms(400);
    p.send_keys(&["Esc"]);
    p.wait_for("Esc Esc palette", TIMEOUT);
    ms(600);
    p.send_keys(&["d"]);
    p.wait_gone("operator layer", TIMEOUT);
    d.snap(&p, "05-line");

    // 06-undo
    p.send_keys(&["C-z"]);
    p.wait_for("operator layer", TIMEOUT);
    d.snap(&p, "06-undo");

    // 07-palette: Esc Esc, type go
    p.send_keys(&["Esc"]);
    p.wait_for("Esc Esc palette", TIMEOUT);
    p.send_keys(&["Esc"]);
    p.wait_for("Command palette", TIMEOUT);
    ms(700);
    type_slowly(&mut p, "go");
    p.wait_for("▸ Go to line", TIMEOUT);
    d.snap(&p, "07-palette");

    // 08-goto: Enter, 9, Enter
    p.send_keys(&["Enter"]);
    p.wait_for("Go to line:", TIMEOUT);
    ms(400);
    type_slowly(&mut p, "9");
    ms(300);
    p.send_keys(&["Enter"]);
    p.wait_for("Ln 9, Col 1", TIMEOUT);
    d.snap(&p, "08-goto");

    // 09-search: Ctrl-F, type a word
    p.send_keys(&["C-f"]);
    p.wait_for("Search:", TIMEOUT);
    type_slowly(&mut p, "swap");
    p.wait_for("Ln 10, Col 30", TIMEOUT);
    d.snap(&p, "09-search");
    p.send_keys(&["Esc"]);
    p.wait_gone("Search:", TIMEOUT);

    // 10-save
    p.send_keys(&["C-s"]);
    p.wait_for("Saved 12 lines", TIMEOUT);
    d.snap(&p, "10-save");

    // 11-crash: type more, kill -9, relaunch
    p.send_keys(&["End"]);
    type_slowly(&mut p, " (unsaved: kill -9 me)");
    p.wait_for("kill -9 me)", TIMEOUT);
    ms(300);
    p.kill9();
    drop(p);
    ms(400);
    let mut p = spawn(&d.rec, true);
    p.wait_for("Recover unsaved changes", TIMEOUT);
    d.snap(&p, "11-crash");

    // 12-recover
    p.send_str("r");
    p.wait_for("kill -9 me)", TIMEOUT);
    p.wait_for("[+]", TIMEOUT);
    d.snap(&p, "12-recover");

    // 13-quit: Ctrl-S, Ctrl-Q
    p.send_keys(&["C-s"]);
    p.wait_for("Saved 12 lines", TIMEOUT);
    ms(900);
    p.send_keys(&["C-q"]);
    p.wait_for("kyte exited with status 0", TIMEOUT);
    d.snap(&p, "13-quit");
    p.wait_exit(TIMEOUT);

    p.record_cast(&out.join("demo.cast")).unwrap();
    let mut tsv = String::from("step\tseconds\n");
    for (n, t) in &d.steps {
        tsv.push_str(&format!("{n}\t{t:.3}\n"));
    }
    fs::write(out.join("steps.tsv"), tsv).unwrap();

    // Self-check: the frames must show the legend, the palette and the Recover modal.
    let frame = |n: &str| fs::read_to_string(frames.join(format!("{n}.txt"))).unwrap();
    assert!(frame("03-legend").contains("w del word"));
    assert!(frame("07-palette").contains("Command palette"));
    assert!(frame("11-crash").contains("Recover unsaved changes"));
    let saved = fs::read_to_string(tmp.path().join("demo.txt")).unwrap();
    assert!(
        saved.contains("(unsaved: kill -9 me)"),
        "recovered text was saved"
    );
    println!(
        "demo: {} steps, cast {:.1}s, frames in {}",
        d.steps.len(),
        d.rec.elapsed(),
        frames.display()
    );
}
