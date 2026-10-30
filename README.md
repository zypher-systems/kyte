# Kyte

A terminal text editor written in Rust with Ratatui.

- **Comfort first.** Kyte always starts in insert mode, like Nano: typing types, arrows move, Ctrl-S saves, Ctrl-Q quits.
- **Power one key away.** Esc opens a one-key operator layer and shows a legend. Pressing Esc twice opens a command palette.
- **Crash-safe.** Every edit is written to a swap log before it reaches the screen. After a crash, `kill -9` or a power cut, Kyte offers to recover.

MIT License. See [LICENSE](LICENSE).

![Kyte demo](docs/demo/demo.gif)

## Install and build

You need a recent stable Rust toolchain on Linux (Kyte was built and tested with 1.99.1).

```
cargo build --release          # binary at target/release/kyte
target/release/kyte FILE       # open (or create) FILE
cargo test                     # unit, property, file, swap and pty end-to-end tests
```

To install it for your user: `cargo install --path .`, which puts `kyte` in `~/.cargo/bin`.

The Makefile targets are thin wrappers:

| Target | Runs |
| --- | --- |
| `make build` | `cargo build --release` |
| `make test` | `cargo test` |
| `make lint` | `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` |
| `make e2e` | `cargo test --test e2e` (drives the real binary in a pseudo-terminal) |
| `make demo` | `scripts/demo.sh` (see [Demo](#demo)) |
| `make docker-test` | `cargo test` inside the Docker dev image (see [Docker](#docker)) |

Optional syntax highlighting for Rust, Python and Bash files (by extension), using tree-sitter:

```
cargo build --release --features highlight
cargo test --features highlight
```

Kyte makes no network connections and sends no telemetry. It reads no config file.

## Usage

```
kyte [OPTIONS] [FILE]
```

| Option | Meaning |
| --- | --- |
| `-h`, `--help` | Show usage and exit 0 |
| `-V`, `--version` | Print `kyte 0.1.1` and exit 0 |
| `--esc-ms N` | Double-tap window for Esc Esc in milliseconds (default 300; env `KYTE_ESC_MS`) |
| `--no-kitty` | Never enable the kitty keyboard protocol |
| `--break-lock` | Open FILE even if another Kyte seems to hold it. Only use this if that Kyte is gone. |
| `--print-swap FILE` | Print the text recovered from FILE's swap to stdout and exit 0, or exit 3 if no valid swap exists. Works without a terminal. |

| Exit code | Meaning |
| --- | --- |
| 0 | Normal exit |
| 1 | Runtime error (the message is printed after the terminal is restored) |
| 129, 130, 143 | Stopped by SIGHUP, SIGINT or SIGTERM. The terminal is restored and the swap stays for recovery. |
| 2 | Bad usage, a directory, a FIFO or device, a non-UTF-8 file, stdin or stdout not a terminal, or another live Kyte has the file open |
| 3 | `--print-swap` found no valid swap |

Environment variables: `KYTE_STATE_DIR` (where swap files go; overrides the default), `KYTE_ESC_MS`, and `TERM` as usual.

## Keys

### Insert mode (the default)

| Key | Action |
| --- | --- |
| Printable characters, Enter, Tab | Insert at the cursor (Tab inserts a real tab) |
| Backspace, Delete | Delete the previous or next character (joins lines at line edges) |
| Arrows, Home, End, PageUp, PageDown, Ctrl-Home, Ctrl-End | Move |
| Ctrl-S (also Ctrl-O) | Save. With no file name, Kyte asks for one |
| Ctrl-Q (also Ctrl-X) | Quit. With unsaved changes, Kyte asks first: `y` saves and quits, `n` discards, Esc cancels |
| Ctrl-Z, Ctrl-Y | Undo, redo |
| Ctrl-F | Search |
| Esc, Ctrl-Space | Open the operator layer |
| Alt-*key* | Same as Esc then *key* |
| Ctrl-P | Open the command palette |

Pasted text (bracketed paste) is inserted as one block and undone with a single Ctrl-Z.

### The Esc layer

Esc never waits. The layer opens at once, the status bar shows `ESC`, and the message line shows the legend:

```
w del word  d del line  k del to end  % bracket  / search  u undo  r redo  s save  Esc Esc palette
```

The next key runs one operator and closes the layer:

| Key | Action |
| --- | --- |
| `w` | Delete to the start of the next word |
| `d` | Delete the current line |
| `k` | Delete to the end of the line (at the end of a line, join the next line) |
| `%` | Jump to the matching bracket (`()`, `[]`, `{}`); shows "No matching bracket" otherwise |
| `/` | Search |
| `u`, `r` | Undo, redo |
| `s` | Save |
| Esc, within 300 ms of the first Esc | Open the command palette |
| Esc, later | Close the layer |
| Any other key | Close the layer; the key is discarded, nothing is inserted |

Each operator is one undo step. Alt-d on a terminal that sends `ESC d` behaves exactly like Esc then `d`. If your terminal or multiplexer delays or eats Esc, use Ctrl-Space for the layer and Ctrl-P for the palette. If the 300 ms window is too short or too long, set `--esc-ms N` or `KYTE_ESC_MS`.

Under tmux, set `tmux set -g escape-time 10`. tmux's default of 500 ms delays every lone Esc.

### Command palette

Esc Esc (or Ctrl-P) opens a fuzzy-filtered list. Type to filter, Up and Down to select, Enter to run, Esc to close without running anything.

Commands: Save, Save as, Quit, Quit without saving, Undo, Redo, Find, Go to line, Toggle line numbers, Show key help.

**Save as** asks for a file name, starting from the current one; `~/` means your home directory. It asks before overwriting a file, and refuses a file another Kyte has open. The buffer then belongs to the new file, and the old file stays as it was.

**Go to line** asks for a 1-based line number. Numbers past the end go to the last line, and anything that is not a number shows an error. **Show key help** shows the key table; Esc closes it.

### Search

Ctrl-F (or Esc `/`) opens `Search:` on the message line. Kyte jumps to the first match as you type. Enter jumps to the next match and wraps around. Esc closes the prompt and leaves the cursor on the match, or puts it back where it was if nothing matched.

## Swap files and recovery

- When you open a file, Kyte creates a swap file under `$XDG_STATE_HOME/kyte/swap/` (default `~/.local/state/kyte/swap/`; `KYTE_STATE_DIR` overrides it). It is never placed next to your file. It has mode 0600 and is named after the escaped absolute path, for example `%2Fhome%2Fme%2Fnotes.txt.kyteswp`. A name that would be too long keeps the end of the path and adds a hash.
- Each edit is appended to the swap as a small checksummed record before the screen updates. The swap is fsynced at most once per second. Your real file changes only when you save.
- The first edit also copies the file's content into the swap, so your edits can be recovered even if the file later changes or disappears. Opening a file without editing it copies nothing.
- Saving is atomic. Kyte writes a temporary file in the same directory, fsyncs it, gives it the file's permissions, owner and extended attributes (which include SELinux labels and ACLs), renames it over the original and fsyncs the directory. A failed save (for example in a read-only directory) leaves the original untouched, shows the error and keeps your changes and the swap. Saving through a symlink updates the target and keeps the link.
- A file with hard links, or one whose owner or attributes the temporary file can't take on, is rewritten in place so it stays the same file. Kyte keeps a backup beside it during the write and restores the old content if the write fails.
- If the file changed on disk since you opened or last saved it, Ctrl-S asks before overwriting. The check compares content, so a rewrite of the same bytes doesn't count. A read-only file also needs a `y` before Kyte replaces it. `n` or Esc cancels either question.
- After a save, the swap starts over empty. After a clean quit, the swap is deleted.
- A buffer with no file name has a swap too, named `untitled-<pid>-<start>.kyteswp`. After a crash, running `kyte` with no file offers to recover the newest one.
- If a save fails, your changes stay in the editor and the swap. The error says so, and Save as in the palette writes them elsewhere. Quitting after a failed save still asks first; only `n` discards.
- If Kyte finds a swap with unsaved changes when you open the file, it shows **Recover unsaved changes**:
  - `r` recovers the changes into the editor, marked unsaved. Ctrl-S writes them.
  - `d` discards them and opens the file as it is on disk.
  - `q` or Esc quits and changes nothing.
- If the swap can't be written, or there isn't one, the status bar shows `no crash recovery` until a save starts a working one.
- A damaged tail (for example a record cut off by a crash) is dropped, and everything before it is recovered.
- If the file changed on disk after the swap was written, Kyte says so and still offers to recover your version. Recovering shows your version in the editor; the file on disk changes only when you save.
- A swap Kyte can't read is **stale**: Kyte offers only discard or quit.
- If another running Kyte has the file open, a second `kyte FILE` exits 2 and names that process's PID. The check includes the process's start time, so a reused PID doesn't count. `--break-lock` overrides it.
- `kyte --print-swap FILE` prints what would be recovered, without a terminal.

## Demo

```
make demo        # or ./scripts/demo.sh
```

The demo runs headless, with no display and no tmux. It builds the release binary, then `cargo run --release --example demo` drives Kyte in a pseudo-terminal at 100x30 through the storyboard: open, type, legend, `w`, `d`, undo, palette, go to line, search, save, `kill -9`, recover, quit. It writes to `docs/demo/`:

- `demo.cast`: an asciicast v2 recording (`asciinema play docs/demo/demo.cast`)
- `frames/NN-step.txt`: the screen text at each step
- `steps.tsv`: the step times
- `demo.gif`: rendered with [agg](https://github.com/asciinema/agg) if it is installed
- `demo.mp4` and `NN-step.png` screenshots: made with ffmpeg from the gif

If agg or ffmpeg is missing, the script still writes the cast and the text frames and prints which artifacts it skipped. `DEMO_OUT=dir` writes somewhere else. `cargo test --test demo -- --ignored` runs the same script as a test.

## Docker

Docker is optional; plain cargo is the primary path. The `Dockerfile` has three stages: `dev` (Rust, tmux, ffmpeg, fonts and agg), `build`, and a small `runtime` image.

```
docker build --target dev -t kyte-dev .
docker run --rm -v "$PWD":/src:Z -w /src kyte-dev cargo test
docker run --rm -it -v "$PWD":/src:Z -w /src kyte-dev cargo run -- /tmp/try.txt
make docker-test                 # the same test run
docker compose run --rm dev make demo
```

On SELinux-enforcing systems such as Fedora, a bind mount needs the `:Z` suffix so the container may read it. The Makefile adds it by default (`MOUNT_SUFFIX ?= :Z`), and `docker-compose.yml` uses `./:/src:Z`. Leave SELinux enforcing; relabel the mount instead. On hosts without SELinux the suffix is harmless, or set `MOUNT_SUFFIX=`.

These Docker files have not been run yet, because the build machine had no Docker (see `docs/DECISIONS.md`).

## Known limitations

- **Split escape sequences.** On a legacy terminal over a laggy link, the bytes of an arrow key can arrive split, so the Esc arrives alone. If the rest arrives within 150 ms, Kyte discards it: that key press does nothing, and nothing is typed. If it arrives later than that, its characters are typed as text. Terminals with the kitty keyboard protocol do not have this problem, and Kyte enables the protocol when it is available.
- UTF-8 files only (others are refused and left untouched).
- Kyte keeps each line's ending as it is. New lines use the ending most of the file uses, and the status bar shows `CRLF` for those files. It doesn't convert a file from one to the other. A `\r` that isn't part of a line ending shows as a control picture.
- Linux only (the swap lock checks `/proc`).
- A very long single line costs more per keystroke, depending on what's in it. At the end of a 5 MB line on the build machine, plain ASCII takes about 8 ms, a line full of tabs about 45 ms, and CJK text about 170 ms.
- Not in this version: reader mode, file tree, the Ctrl-K universal palette, light and high-contrast themes, undo history that survives a restart, mouse, selection and clipboard (including OSC 52), multiple buffers or splits, LSP, plugins, a config file, soft wrap, CRLF conversion, non-UTF-8 files, RPM/COPR packaging, and use as the default `$EDITOR`.

## Documentation

- `docs/ARCHITECTURE.md`: the design specification
- `docs/DECISIONS.md`: deviations from the specification and resolved crate versions
- `docs/ACCEPTANCE.md`: the acceptance criteria and how each one was verified
- `docs/KEY_LAB.md`: measured key bytes and event sequences

MIT License. See [LICENSE](LICENSE).
