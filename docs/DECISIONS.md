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
