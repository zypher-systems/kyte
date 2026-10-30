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
