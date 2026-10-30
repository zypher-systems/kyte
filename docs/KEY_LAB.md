# Key-input lab (task T1)

Results from real pty runs on the build box (Linux, rustc 1.99.1, crossterm 0.29.0 via ratatui 0.30.2), Oct 30, 2026.

How these were produced (reproducible):

```
cargo run --example keylab -- --drive          # raw bytes straight into a pty (legacy xterm encoding)
cargo run --example keylab -- --drive --tmux   # the same bytes through tmux 3.5a with `set -g escape-time 10`
cargo run --example keylab                     # interactive: press keys yourself, Ctrl-C quits (--kitty tries the kitty flags)
```

The driver spawns `keylab` in the headless pty harness (`tests/common/mod.rs`), writes each byte sequence (separate `write`s where the table shows `+`, with the stated pauses), and reads back the events keylab logged. The kitty keyboard protocol was not available in either run (the harness terminal and tmux do not answer the kitty query), so everything below is legacy encoding.

## Direct pty (legacy encoding)

| Sent | Bytes | Events observed (crossterm, then Kyte mapping) |
| --- | --- | --- |
| Esc alone | `1b` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))` |
| Esc, Esc 100 ms apart (two writes) | `1b` + `1b` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))`<br>`Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))` |
| ESC ESC in one write | `1b 1b` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))` |
| ESC d in one write (Alt-d) | `1b 64` | `Key(code: Char('d'), modifiers: KeyModifiers(ALT))` -> `Some(Key(Alt('d')))` |
| ESC, then d 50 ms later | `1b` + `64` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))`<br>`Key(code: Char('d'), modifiers: NONE)` -> `Some(Key(Char('d')))` |
| Up arrow | `1b 5b 41` | `Key(code: Up, modifiers: NONE)` -> `Some(Key(Up))` |
| Down arrow | `1b 5b 42` | `Key(code: Down, modifiers: NONE)` -> `Some(Key(Down))` |
| Left arrow | `1b 5b 44` | `Key(code: Left, modifiers: NONE)` -> `Some(Key(Left))` |
| Right arrow | `1b 5b 43` | `Key(code: Right, modifiers: NONE)` -> `Some(Key(Right))` |
| Home (CSI H) | `1b 5b 48` | `Key(code: Home, modifiers: NONE)` -> `Some(Key(Home))` |
| End (CSI F) | `1b 5b 46` | `Key(code: End, modifiers: NONE)` -> `Some(Key(End))` |
| Home (CSI 1~) | `1b 5b 31 7e` | `Key(code: Home, modifiers: NONE)` -> `Some(Key(Home))` |
| End (CSI 4~) | `1b 5b 34 7e` | `Key(code: End, modifiers: NONE)` -> `Some(Key(End))` |
| PageUp | `1b 5b 35 7e` | `Key(code: PageUp, modifiers: NONE)` -> `Some(Key(PageUp))` |
| PageDown | `1b 5b 36 7e` | `Key(code: PageDown, modifiers: NONE)` -> `Some(Key(PageDown))` |
| Ctrl-Home | `1b 5b 31 3b 35 48` | `Key(code: Home, modifiers: KeyModifiers(CONTROL))` -> `Some(Key(CtrlHome))` |
| Ctrl-End | `1b 5b 31 3b 35 46` | `Key(code: End, modifiers: KeyModifiers(CONTROL))` -> `Some(Key(CtrlEnd))` |
| Ctrl-S (0x13) | `13` | `Key(code: Char('s'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('s')))` |
| Ctrl-Q (0x11) | `11` | `Key(code: Char('q'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('q')))` |
| Ctrl-Z (0x1a) | `1a` | `Key(code: Char('z'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('z')))` |
| Ctrl-Y (0x19) | `19` | `Key(code: Char('y'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('y')))` |
| Ctrl-Space (0x00) | `00` | `Key(code: Char(' '), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl(' ')))` |
| Ctrl-P (0x10) | `10` | `Key(code: Char('p'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('p')))` |
| Backspace (0x7f) | `7f` | `Key(code: Backspace, modifiers: NONE)` -> `Some(Key(Backspace))` |
| Delete | `1b 5b 33 7e` | `Key(code: Delete, modifiers: NONE)` -> `Some(Key(Delete))` |
| Bracketed paste a\rb | `1b 5b 32 30 30 7e 61 0d 62 1b 5b 32 30 31 7e` | `Paste("a\rb")` -> `Some(Paste("a\nb"))` |
| Up arrow split: ESC, 30 ms, [A | `1b` + `5b 41` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))`<br>`Key(code: Char('['), modifiers: NONE)` -> `Some(Key(Char('[')))`<br>`Key(code: Char('A'), modifiers: KeyModifiers(SHIFT))` -> `Some(Key(Char('A')))` |

## Through tmux 3.5a (`escape-time 10`)

| Sent | Bytes | Events observed (crossterm, then Kyte mapping) |
| --- | --- | --- |
| Esc alone | `1b` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))` |
| Esc, Esc 100 ms apart (two writes) | `1b` + `1b` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))`<br>`Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))` |
| ESC ESC in one write | `1b 1b` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))` |
| ESC d in one write (Alt-d) | `1b 64` | `Key(code: Char('d'), modifiers: KeyModifiers(ALT))` -> `Some(Key(Alt('d')))` |
| ESC, then d 50 ms later | `1b` + `64` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))`<br>`Key(code: Char('d'), modifiers: NONE)` -> `Some(Key(Char('d')))` |
| Up arrow | `1b 5b 41` | `Key(code: Up, modifiers: NONE)` -> `Some(Key(Up))` |
| Down arrow | `1b 5b 42` | `Key(code: Down, modifiers: NONE)` -> `Some(Key(Down))` |
| Left arrow | `1b 5b 44` | `Key(code: Left, modifiers: NONE)` -> `Some(Key(Left))` |
| Right arrow | `1b 5b 43` | `Key(code: Right, modifiers: NONE)` -> `Some(Key(Right))` |
| Home (CSI H) | `1b 5b 48` | `Key(code: Home, modifiers: NONE)` -> `Some(Key(Home))` |
| End (CSI F) | `1b 5b 46` | `Key(code: End, modifiers: NONE)` -> `Some(Key(End))` |
| Home (CSI 1~) | `1b 5b 31 7e` | `Key(code: Home, modifiers: NONE)` -> `Some(Key(Home))` |
| End (CSI 4~) | `1b 5b 34 7e` | `Key(code: End, modifiers: NONE)` -> `Some(Key(End))` |
| PageUp | `1b 5b 35 7e` | `Key(code: PageUp, modifiers: NONE)` -> `Some(Key(PageUp))` |
| PageDown | `1b 5b 36 7e` | `Key(code: PageDown, modifiers: NONE)` -> `Some(Key(PageDown))` |
| Ctrl-Home | `1b 5b 31 3b 35 48` | `Key(code: Home, modifiers: KeyModifiers(CONTROL))` -> `Some(Key(CtrlHome))` |
| Ctrl-End | `1b 5b 31 3b 35 46` | `Key(code: End, modifiers: KeyModifiers(CONTROL))` -> `Some(Key(CtrlEnd))` |
| Ctrl-S (0x13) | `13` | `Key(code: Char('s'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('s')))` |
| Ctrl-Q (0x11) | `11` | `Key(code: Char('q'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('q')))` |
| Ctrl-Z (0x1a) | `1a` | `Key(code: Char('z'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('z')))` |
| Ctrl-Y (0x19) | `19` | `Key(code: Char('y'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('y')))` |
| Ctrl-Space (0x00) | `00` | `Key(code: Char(' '), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl(' ')))` |
| Ctrl-P (0x10) | `10` | `Key(code: Char('p'), modifiers: KeyModifiers(CONTROL))` -> `Some(Key(Ctrl('p')))` |
| Backspace (0x7f) | `7f` | `Key(code: Backspace, modifiers: NONE)` -> `Some(Key(Backspace))` |
| Delete | `1b 5b 33 7e` | `Key(code: Delete, modifiers: NONE)` -> `Some(Key(Delete))` |
| Bracketed paste a\rb | `1b 5b 32 30 30 7e 61 0d 62 1b 5b 32 30 31 7e` | `Paste("a\rb")` -> `Some(Paste("a\nb"))` |
| Up arrow split: ESC, 30 ms, [A | `1b` + `5b 41` | `Key(code: Esc, modifiers: NONE)` -> `Some(Key(Esc))`<br>`Key(code: Char('['), modifiers: NONE)` -> `Some(Key(Char('[')))`<br>`Key(code: Char('A'), modifiers: KeyModifiers(SHIFT))` -> `Some(Key(Char('A')))` |

## Findings

- **Ctrl-S, Ctrl-Q, Ctrl-Z (and Ctrl-Y, Ctrl-P) arrive as keys** in raw mode: no XON/XOFF flow control and no SIGTSTP. Confirmed in both runs.
- **Lone Esc** is reported as `Esc` as soon as the read returns with nothing after it (crossterm's own parser). No extra timeout is involved, so the overlay can open instantly (section 5.2 item 3).
- **Two Escs in separate writes** (a human double-tap, 100 ms apart) arrive as two `Esc` events. This is the double-tap path.
- **`ESC ESC` in a single write** arrives as **one** `Esc` event: crossterm's parser emits Esc for the pair and drops the second byte. A human double-tap never arrives in one read in practice, but scripts that drive Kyte must send the two Escs as separate writes (the e2e tests and the demo do). Recorded in `docs/DECISIONS.md`; no byte-level parsing was added (section 5.2 item 1).
- **`ESC d` in one write** arrives as `Alt('d')`, which `input::normalize` turns into `[Esc, Char('d')]` in Insert mode (section 5.2 item 5).
- **`ESC`, then `d` 50 ms later** arrives as `Esc` then `d`, the same effect as Alt-d.
- **Arrows, Home/End (both CSI H/F and CSI 1~/4~), PageUp/PageDown, Ctrl-Home/Ctrl-End, Delete, Backspace** all map to the expected keys.
- **Ctrl-Space** arrives as NUL and is reported as `Char(' ')` with CONTROL, mapped to `Ctrl(' ')` (the alternate overlay key).
- **Bracketed paste** arrives as one `Paste` event; the terminal-style CR line break is kept by crossterm, and Kyte maps CR and CRLF in pasted text to LF.
- **Known limitation (section 5.2 item 7) observed:** an Up arrow whose bytes are split (`ESC`, 30 ms pause, `[A`) arrives as `Esc`, `Char('[')`, `Char('A')`. In Kyte the Esc opens the overlay and `[` is an unknown overlay key, which closes it and is discarded; the trailing `A` is then typed as text. Kyte did not heal this in the MVP. Since 0.1.1 it discards the rest of the sequence when it arrives within 150 ms of the Esc, so the `A` is no longer typed.
- tmux with `escape-time 10` behaved identically to the direct pty for every row.

## Chosen default

`esc_ms` = **300 ms** (unchanged from section 5.2). Real double-taps arrive as separate events well inside it; `--esc-ms N` / `KYTE_ESC_MS` change it.
