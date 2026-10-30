# Decisions and deviations

Every deviation from `ARCHITECTURE` (Mnemo page_e336285940d54098ada133cc08df814b) with a reason, plus resolved crate versions.

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
