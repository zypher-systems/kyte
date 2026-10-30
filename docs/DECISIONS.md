# Decisions and deviations

Every deviation from `ARCHITECTURE` (Mnemo page_e336285940d54098ada133cc08df814b) with a reason, plus resolved crate versions.

## Product decisions (Oct 30, 2026)

These replace the matching parts of `ARCHITECTURE`. `docs/ROADMAP.md` holds the plan.

- Kyte shares one idea with Nano: it starts ready to type. It isn't a Nano clone, so its shortcuts don't have to match Nano's.
- Ctrl-K stays the universal palette.
- The unsaved-quit prompt follows Nano: `y` saves and quits, `n` discards, Esc cancels. This replaces section 5.4 and changes AC-18.
- License: MIT. This settles section 16 and the section 4 note.

## Hosting and environment

- Toolchain: rustc/cargo 1.99.1 (stable, 2026-10-13), edition 2021.
- Docker is not installed on the build box. `Dockerfile`, `docker-compose.yml` and `make docker-test` are written per section 12 but were **not run**. Bind mounts use the `:Z` suffix for SELinux; SELinux is never disabled.

## Resolved crate versions (Cargo.lock)

| Crate | Spec | Resolved |
| --- | --- | --- |
| ratatui | 0.30.x | 0.30.2 (re-exports crossterm 0.29.0 as `ratatui::crossterm`; no direct crossterm dependency) |
| ropey | 1.6 | 1.6.1 |
| unicode-segmentation | any | 1.13.3 |
| unicode-width | any | 0.2.3 |
| crc32fast | any | 1.5.3 |
| anyhow | any | 1.0.104 |
| tempfile (dev) | any | 3.27.0 |
| proptest (dev) | any | 1.12.0 |
| portable-pty (dev) | any | 0.9.0 |
| vt100 (dev) | any | 0.18.1 (newest; its API takes `NonZeroU16` sizes) |

## T1 key lab findings

- `ESC ESC` written in one `write()` reaches Kyte as a single Esc (crossterm's parser consumes the pair). Real double-taps are separate reads, so the design stands; test and demo drivers send the two Escs as separate writes. No byte-level parsing was added.
- Pasted text: CR and CRLF line breaks (what terminals send for pasted newlines) are stored as LF. File content is otherwise stored exactly as read.
- The pty harness answers the cursor-position (DSR 6) and primary device-attribute queries like a real terminal would, so a program that asks never stalls. Kyte itself only queries when the kitty check runs (not with `--no-kitty`).

## T2 buffer

- ropey is used with `default-features = false, features = ["simd"]` so only LF (and CRLF) end a line; `\r` alone and Unicode separators stay ordinary characters, matching "Text is stored exactly as read". In a CRLF file the `\r` is shown as a visible control picture at the line end (CRLF conversion is out of the MVP).
- Added `Buffer` helpers beyond the section 6 list (`apply_with_cursor`, `move_lines`, `move_to_line_col`, `line_text`, `line_start`/`line_end`, `prev_boundary`/`next_boundary`, `saved_line_count`, `set_dirty`, `is_dirty`). They exist for operators that place the cursor, PageUp/PageDown, go-to-line, "Saved N lines" and recovery.
- `dirty` is a plain flag: any edit, undo or redo sets it, only a save clears it (undoing back to the saved text still shows `[+]`, as in nano).
- Backspace and Delete are one undo step each (`GroupKind::Other`); only plain typing coalesces.
- Zero-width graphemes and control characters take one display cell (control characters are drawn as Unicode control pictures) so the cursor never sits on a zero-width cell.

## T6 overlay and operators

- `d` cursor placement: section 6 says "same column on the following line, clamped" and its example `a\nb▮b\nc` becomes `a\n▮c`. Kyte clamps to the last character of that line (not past its end), which reproduces the example exactly. When the deleted line was the last line, the cursor goes to the line that is now last.
- `d` on the empty last line after a trailing newline removes that trailing newline (the rule for "last line without a trailing newline" applied literally).
- `w` works on graphemes (a base letter plus combining marks counts as one word character) and treats newlines as whitespace, so `w` at the end of a line deletes the line break and the next line's indentation.
- Added `ops::kill_range` (the `k` range built on `ops::eol`) next to the four section 6 functions.
- A paste while the overlay is open closes the overlay and inserts the paste (it is not an "unknown key"; dropping pasted text silently would lose data). In the palette, pasted text goes into the query with line breaks as spaces; in prompts it is ignored.

## T7 search

- While the search prompt is open the terminal cursor stays on the current match in the text (so the jump is visible); the query is shown on the message line as `Search: <query>`. "No match" is appended there when nothing matches.
- Keys other than typing, Backspace, Enter and Esc are ignored while the search prompt is open.

## T8 palette and prompts

- The popup shows up to 8 result rows (section 9), so with an empty query 8 of the 9 commands are visible at once and the ninth ("Show key help") scrolls into view with Down. AC-17's "lists all nine commands" is verified as: all nine are in the result list (unit test) and all nine are seen on screen while scrolling (e2e).
- The selected row is marked with `▸` as well as the selected colors, so text frames and screen dumps show the selection.
- Added `Mode::Help` for the "Show key help" modal (Esc, Enter or q close it). The section 5.3 `Mode` enum had no slot for a modal other than Recover.
- Go to line: `0` counts as line 1; huge numbers clamp to the last line; anything that is not digits shows "Not a line number" next to the prompt and keeps it open.
- Status bar mode labels besides INSERT and ESC: PALETTE, SEARCH, GOTO, QUIT?, HELP, RECOVER.

## T9 swap log

- The swap is created when a file is opened (not on the first edit), so the PID lock holds for the whole session; it has no records until the first edit. A swap whose replay equals the file (no unsaved edits, e.g. after a kill of an unmodified session) counts as "no swap" and is silently replaced.
- A swap with an unreadable header (bad magic, wrong version, truncated header) is treated as stale: the user may discard it or quit.
- The swap key is the canonical target path: symlinks resolved for existing files; for a new file, its canonicalized directory plus the name. `--print-swap` uses the same key.
- Liveness is `/proc/<pid>` existing and the PID not being Kyte's own; a recycled PID owned by an unrelated process would be reported as a live lock (the safe direction).
- If the swap cannot be created for a reason other than a live lock (for example an unwritable state directory) Kyte still opens the file and shows a warning on the message line that crash recovery is off.
- Replay also stops at a record whose edit falls outside the text (bounds check), in addition to short and CRC-bad records. Unknown record kinds are skipped by length (section 13 hook).
- Very long paths can exceed the 255-byte file-name limit once escaped; this is not handled in the MVP (swap creation fails with a warning and the file still opens).
- Added `SwapWriter::create_in` and `swap::inspect_in` taking an explicit directory so tests do not mutate process-wide environment variables, plus `record_count`, `parse_header`, `encode_record`, `stale_info`, `live_lock_pid`.

## T10 recovery UI

- Recover modal title "Recover unsaved changes", Stale modal title "Stale swap file"; both name the canonical file path and the swap's modification time in UTC (no time-zone database in the MVP). `r` is ignored in the Stale modal.
- After `r` the replayed text becomes the buffer (marked dirty, undo history empty: persistent undo is out of the MVP) and the swap is adopted (torn tail dropped, PID rewritten), so a second crash still recovers everything.
- After `d` the old swap is deleted and a fresh one is created for the file as it is on disk.
- `q`/Esc leave the swap exactly as it was (bytes unchanged) and exit 0.
- `--print-swap` on a stale swap exits 3 ("no valid swap").

## T11 demo

- The storyboard step 05-line first moves down three lines (to line 4) before `Esc d`, so the deleted line is visibly a different one from the edited first line. 11-crash types ` (unsaved: kill -9 me)` at the end of line 10, waits, sends SIGKILL, and relaunches Kyte under `sh -c` so the final frame (13-quit) can show `$ kyte exited with status 0` after the clean exit.
- The demo reuses the pty harness from `tests/common/mod.rs` (`#[path]` module), so the demo and the e2e tests drive Kyte the same way. The two Escs of Esc Esc are separate writes (see T1).
- PNG screenshots are cut from the gif with `ffmpeg -vf fps=20 -ss <t> -frames:v 1`. agg writes a frame only when the screen changes, so the gif is resampled to a constant rate first; without that, a seek past the gif's last frame (step 13) produced no image. Each PNG is taken 0.5 s into the hold after its step's snapshot (`steps.tsv`).
- The mp4 is converted from the gif (`-pix_fmt yuv420p`, even dimensions, `+faststart`), so it is 978 px wide where the gif is 979.
- The text frames and the Recover modal show the demo's temporary directory path (`/tmp/.tmpXXXX/demo.txt`); it contains nothing private.
- `tests/demo.rs` holds the only ignored test (`ac25_...`, about 40 s). It runs `scripts/demo.sh` into a temporary `DEMO_OUT` so the committed artifacts are not touched.
- The optional `scripts/demo-tmux.sh` fallback was not written; the pty pipeline needs neither tmux nor asciinema.

## T12 verification

- Docker is not installed on the build box, so `docker build`, `docker compose` and `make docker-test` were **not run**. The files follow section 12; agg's install step ends in `|| true` as the spec sketch does.
- The README claims nothing about Docker beyond the commands; it says the Docker path is unverified.

## T13 syntax highlighting (stretch, done)

- Every AC passed first, and the grammars resolved against tree-sitter 0.27, so T13 was built. Resolved versions: tree-sitter 0.27.x, tree-sitter-highlight 0.27.1, tree-sitter-rust 0.24.2, tree-sitter-python 0.25.0, tree-sitter-bash 0.25.1 (all grammar ABI 15). All are optional dependencies behind the `highlight` feature, which is off by default. `cargo tree -e normal` for the default build lists no tree-sitter crate.
- `src/highlight.rs`: `Lang::from_path` picks the grammar by extension (`.rs`; `.py`/`.pyi`; `.sh`/`.bash`). `highlight(lang, text, lines) -> Vec<(Range<usize>, HighlightKind)>` returns byte ranges of the whole text that overlap the visible line range. It uses each grammar's bundled `highlights.scm` through `tree-sitter-highlight`; the capture names keyword, string, comment, function, type, constant and number map to `HighlightKind`, and the innermost capture wins. Query configurations are compiled once per language (`OnceLock`).
- The function takes the whole text (not just the visible lines) so multi-line strings and comments are parsed correctly. Each redraw re-parses, so documents over 512 KiB are not highlighted. Incremental parsing is a later improvement.
- Colors are 256-color indexed values on the text background: keyword 176 (bold), string 150, comment 245 (italic), function 111, type 180, constant/number 209.
- The text view uses a styled per-grapheme path only for lines that have spans; other lines take the unchanged `render_line` path. Without the feature, none of this code is compiled.
- Tests: four unit tests in `highlight.rs` (a Rust snippet is tokenized into comment, keyword, function and string; only the requested lines are returned; the Python and Bash grammars load; extension mapping) and `tests/e2e_highlight.rs` (`fn` in a `.rs` file is drawn with foreground index 176 in the vt100 screen; a `.txt` file stays at the default 252).
