# Decisions and deviations

Every deviation from `ARCHITECTURE` (Mnemo page_e336285940d54098ada133cc08df814b) with a reason, plus resolved crate versions.

## Product decisions (Oct 30, 2026)

These replace the matching parts of `ARCHITECTURE`. `docs/ROADMAP.md` holds the plan.

- Kyte shares one idea with Nano: it starts ready to type. It isn't a Nano clone, so its shortcuts don't have to match Nano's.
- Ctrl-K stays the universal palette.
- The unsaved-quit prompt follows Nano: `y` saves and quits, `n` discards, Esc cancels. This replaces section 5.4 and changes AC-18.
- License: MIT. This settles section 16 and the section 4 note.
- Kyte isn't tied to any one OS. It's meant to be open source for everyone.

## CI

- That image has no Node, which `actions/checkout` needs, so the job checks out with git.
- The first runs found test races that never showed on the build machine, because the runner is slower: a popup's title could arrive before its rows. Tests now wait for what they check instead of reading the screen once.
- The container runs as root, so the tests that need a read-only directory or file skip there. They run only on a normal user's machine, so run `cargo test` locally too.

## Reviewing outside changes

- Kyte never interrupts for an outside change. A popup could take the next keystroke as an answer while someone is typing, so a status marker reports it, and Esc `c` starts the review. Ctrl-S's overwrite question also offers it. A clean buffer doesn't reload by itself either, for the same reason, and because the user should see what changed.
- The tick checks the file about once a second. An unchanged file costs one `stat`; a changed stamp reads the file and compares its content, so a `touch` raises nothing.
- The review is three-way: `disk_text` (the last version read or written), the buffer, and the file. A two-way diff would present the user's own unsaved edits as changes to revert. Hunks are placed in the buffer by the user's own diff against the base. Those touching the user's edits are grouped with them and marked as conflicts, and taking one replaces that whole region. A change the buffer already has is dropped.
- The diff is a small Myers implementation in `diff.rs` rather than a crate. Past 4,096 differing lines it reports one change, so a rewritten file can't stall the editor.
- Each `y` is one undo step, and `a` is one for all the rest. An answered review records the file as the known version, so Ctrl-S doesn't ask again. Stopping with Esc leaves it unknown, so the marker and the question stay.
- The file's version goes into the history when the review opens, followed by a snapshot of the buffer, because the edits that follow apply to the buffer and not to the file.
- A review shows the incoming lines under the lines they'd replace, in a temporary view, using the same view mechanism as history.

## Reader mode

- Esc `v` opens it, which replaces the design's `esc+Home`. It's read-only, so a stray key can't change the file, and Esc leaves the cursor where reading stopped, ready to edit there.
- A section is the function at the cursor, widened upward over its doc comments and attributes, or a paragraph elsewhere. Outside a function in code, it's the paragraph.
- Up and Down move the cursor and the focus follows it, so reading through a file walks the spotlight along. `n` and `p` jump. The view scrolls only when a section isn't fully on screen, with two lines above it, because moving the page mid-read loses the reader's place.
- The breadcrumb goes in the status bar, not a popup, because it's status and there's nothing to pick.
- The keys are a floating menu by the floating-menus rule, but a menu that stayed up would cover the text being read. So it shows on entry, hides at the first key, and `?` brings it back.
- Dimming overrides the foreground, syntax colors included, and leaves the background alone, so the focus reads as the only colored part.
- Code over 512 KiB falls back to paragraphs, because reader mode parses on every key.
- Idle auto-activation from the design isn't built. It should be off by default, which needs a config file first.

## Smart Esc layer

- The Esc menu is built from context each time it's drawn: `App::layer_rows` lists what works here, so a key is never offered and then ignored. The core keys stay, in the same place, so muscle memory holds. Context adds to the list, except that a selection replaces it.
- Esc with a selection opens the selection's menu, and a second Esc clears the selection at any speed. That gives up Esc Esc for the palette while something is selected; Ctrl-P and Ctrl-K still open it.
- Keys in the selection menu use `U`, `L`, `S` and `R` where the lowercase letter already means something else in the layer, so `u` and `r` never quietly change meaning. A key that isn't an action closes the menu and leaves the selection alone.
- Code actions cover Rust, Python and Bash, the grammars Kyte ships. `syntax.rs` parses only when an action runs, because a parse on every Esc would break the rule that Esc never waits. Listing the keys needs only the file extension.
- `#` works in any file with a known line comment, by extension, with or without tree-sitter. Comments go on at the lines' shared indent, and come off only if every non-blank line has one.
- Without the `highlight` feature the code actions aren't offered, and `syntax.rs` provides stubs.
- Sorting compares bytes, and uses the file's line ending.

## Floating menus

- Anything the user picks from goes in a floating box, by the user's direction: the Esc layer, the unsaved, overwrite and delete questions, replace's choices, the history keys and Tab completions. The footer keeps status and passing messages.
- Typed input for search, replace, go to line, Open and Save As stays on the message line, because there's nothing to pick from, and search needs the text in view.
- A question takes the middle of the screen. The Esc layer, replace and history sit in a corner clear of the cursor, because what's under the cursor matters there.
- Rows that don't fit the height flow into more columns, so every key stays visible on a short terminal. This replaces the short legend wording.

## File history

- Every edit is kept, by decision: not only saves and crashes. So a file's log grows with every keystroke, and nothing prunes it yet. Measured with a million single-character edits on a 30 KB file: the log is 34 MB (34 bytes per edit), the timeline opens in 105 ms, any version shows in about 8 µs, and a session's first edit takes 31 ms, because it reads the log. Later edits take under a microsecond.
- The log is separate from the swap. The swap is short-lived and protects the session; the log is permanent. Keeping them apart leaves tested crash recovery alone, at the cost of writing each edit twice.
- A log is written only from the first edit on, so opening a file costs nothing and leaves nothing behind. The first edit reads the log, drops a torn tail, and stores the current text only if the log doesn't already end there. So a file changed between sessions becomes a version of its own, and a normal reopen adds just the edit.
- The log's end state comes from its last snapshot or mark. After a crash the log ends in edits, so the state isn't known and the next session stores a snapshot. That's one file copy, only after a crash.
- Discarded edits stay in the log. A version a save overwrites from outside Kyte goes in as well, so answering `y` to the overwrite question loses nothing.
- Version text comes from replaying forward. Timeline keeps a `Rope` every 256 versions, and ropes share memory, so any step replays at most 255 edits. The log therefore stores no removed text.
- Time shows as "5 minutes ago", which needs no time-zone database.
- Restore uses `Buffer::replace_all`, which trims the common start and end, so the undo step and the log record hold only what differs.
- The browser swaps in a read-only `Buffer` through `App::view_buffer`, so text view, highlighting and scrolling work unchanged, and what a step added is shown as a selection.
- History is keyed by path. Following renames would need file identity that survives across tools, which is out of scope for now.
- A failed history write stops history for the session and shows `no history`, and never stops editing.
- The logs are off in unit tests unless `enable_history` is called, because `App::new` alone must not touch the real state directory.

## Highlighting on by default

- `highlight` is now a default feature; `--no-default-features` leaves it out. The binary grows from about 1.5 MB to about 7 MB, and building needs a C compiler. This replaces T13's "off by default".
- `render` used to skip selection and search painting on any line with syntax colors. Nothing caught it because those tests used `.txt` files, and the highlight build had no test that mixed them. `tests/e2e_highlight.rs` now covers both on `.rs` files.
- Measured on a release build, per keystroke at the end of a file: 13 ms at 100 KB, 27 ms at 250 KB, 54 ms at 500 KB. That's good enough to leave incremental parsing for later and keep the 512 KiB cap.
- Only Rust, Python and Bash are covered, so other files still show plain.

## 0.2.0 small terminals

- Everything had been tested at 100x30. At 80x24 the key help lost its last rows, the legend lost `Esc Esc palette`, and the save error lost its advice. The e2e tests now run those at 80x24 too.
- The legend had short labels for terminals under 100 columns until it became a floating menu.
- The key help scrolls with Up and Down when it doesn't fit.

## 0.2.0 universal palette

- Ctrl-K searches commands and files together, with `:N` for a line and `/text` for search. The concept page wasn't available when this was built, so this is the working definition; change it here if the concept says otherwise.
- Esc Esc and Ctrl-P stay commands-only. That keeps a short, predictable list, and it keeps what the Esc layer's legend promises.
- A command wins a tie against a file, because the command list is small and it's the more common aim.
- The scan walks the working directory breadth-first, so the nearest files survive the limits of 20,000 files and 100 ms, and it blocks the key press for no longer than that. It runs on every Ctrl-K, so new files show up. Symlinks aren't followed, since one could point back up the tree. `.gitignore` isn't read; hidden files, `target` and `node_modules` are left out by name.
- The file list sits behind an `Arc`, because `App` clones the mode on each key.
- The fuzzy scorer now counts any non-alphanumeric character as a word start, not just a space, so `/`, `-` and `.` help file names.

## 0.2.0 indent

- Tab replaces the "Tab inserts a real tab" rule of the MVP where the file uses spaces. The indent is read from the file, using the first 10,000 lines: the majority of indented lines picks tabs or spaces, and the most common step between neighbouring lines picks the width. Steps of one space are skipped, because they're usually comment alignment. With no signal, spaces default to 4. A file with no indent at all uses tabs, as before.
- Tab stops on screen stay at 4 columns for real tabs. Config is on the later list.
- Enter copies the indent before the cursor. On a line that's only indent, the indent moves down with the cursor, so Enter leaves no trailing whitespace.
- Backspace inside a space indent goes back to the previous stop. Past the indent it deletes one grapheme.
- Tab with a selection across lines indents those lines as one undo step and skips blank ones; within one line it replaces the selection, like any typing. A selection ending at the very start of a line leaves that line alone.

## 0.2.0 opening files

- Ctrl-O opens a file, so it no longer saves. Kyte holds one buffer at a time; opening a file replaces it, and multiple buffers stay on the later list. The clipboard survives the switch, which makes it the way to move text between files.
- Startup and Ctrl-O share `session.rs`, so a file opened either way gets the same swap checks, lock and recovery.
- `session::check` runs when Enter is pressed and changes nothing, so a file that can't be opened leaves the buffer alone and the prompt open. Only then does Kyte ask about unsaved changes.
- The unsaved prompt now says what it's for. `After` carries what a save was for through Save As and the save questions, so `y` while opening a file still reaches the file even if the buffer has no name yet.
- Reopening the current file after pressing `n` removes its swap first. Otherwise its own discarded edits would be offered back for recovery.
- If the opened file has a crashed session, `q` in the recovery modal closes Kyte. The old buffer has already been left, and the modal does say quit.
- Tab completes from what's on disk, stops at what the choices share, and lists them on the message line. Hidden files need a leading dot.

## 0.2.0 search and replace

- Search uses smart case instead of a toggle: no capitals matches any case, and any capital is exact. This needs no extra key. It replaces the "case-sensitive, smart-case is out" rule in T7.
- Case folding is one char to one char, using the first char of `to_lowercase`, so a match keeps its length and char offsets stay right. Full Unicode case folding, where `ß` matches `ss`, is out of scope. ASCII text takes a faster path.
- Highlighting runs only while a search or replace prompt is open, and only looks at cells on screen. A match that spans lines isn't highlighted, though it's still found.
- Replace is literal text, with no regular expressions. It runs from the cursor to the end, then wraps to where it started, so it never reads its own output twice. `a` is one undo step, and each `y` is one.
- `a` uses a new `Buffer::apply_group`, which applies the edits last to first so earlier positions stay valid, and the swap gets them in the same order.

## 0.2.0 selection and clipboard

- Kyte's shortcuts are its own, so Ctrl-X, Ctrl-C and Ctrl-V are cut, copy and paste, and Ctrl-X no longer quits. This fits the Ctrl-S, Ctrl-Z, Ctrl-Y and Ctrl-F keys Kyte already had.
- Every cursor move and edit clears the selection inside `Buffer`, and only Shift-motions put the anchor back. Code that doesn't know about selection therefore can't leave a stale one behind.
- Esc with a selection cleared it and did nothing else, until the smart layer gave selections their own menu.
- With nothing selected, cut and copy take the whole line and a whole line pastes above the current one, wherever the cursor is in that line. This gives the old Esc `d` a paste partner.
- `d`, `k` and `w` still only delete; they don't touch the clipboard.
- OSC 52 only writes. Terminals usually refuse reads, and a read can hang waiting on a terminal that never answers. So Ctrl-V uses Kyte's own clipboard, and the terminal's paste key arrives as a bracketed paste. Text over 1 MiB stays inside Kyte, because terminals drop big sequences silently.
- Base64 is 20 lines in `clipboard.rs` rather than a crate.

## 0.1.1 swap format (version 2)

These replace the swap format and flows in `ARCHITECTURE` sections 6 and 8.

- The header gained the writer's process start time (offset 16), and everything after it moved by 8 bytes. A version 1 swap counts as unreadable: Kyte offers discard or quit. Kyte had no users of the old format.
- New record kind 1 holds the base snapshot, split at about 1 MiB. It's written once, just ahead of the first edit, so a session that only reads copies nothing to disk, and `SwapWriter` holds a `Rope` clone of the base until then. The swap therefore holds the whole file while edits are unsaved; it stays 0600 in a 0700 directory and is deleted the same way as before.
- Replay uses the file when it still matches the header and the snapshot otherwise. So a file that changed or was deleted after a crash no longer strands the edits, and "stale" now means only that the swap can't be read. AC-22 changed to match.
- A live lock needs both the PID and its start time from `/proc/<pid>/stat` to match. `--break-lock` zeroes the swap's PID and changes nothing else, so recovery still works.
- A swap name over 255 bytes keeps the end of the escaped path and adds the path's CRC and length. If the swap can't be read at all, the file still opens, with the crash-recovery warning. Before this, both cases stopped Kyte from opening the file, despite what the README said.

## 0.1.1 saving

- Added `rustix` as a direct dependency for xattrs and `access`. crossterm already uses it, so the build has no new crate.
- The temp file takes the old file's owner and every extended attribute, which covers SELinux labels, ACLs and file capabilities. Listing returning "not supported" counts as nothing to copy.
- A rename would change a hard-linked file into a separate file, and a failed chown or xattr copy would change its owner or label. Those cases write in place: back up, overwrite without truncating, set the length, fsync, and restore on failure. The temp file is still created first, so a read-only directory fails as AC-24 requires.
- Outside changes are found by comparing length and CRC at save time, not by mtime. A timestamp can miss a same-tick write, and reading the file costs little next to writing it. A file deleted while open is saved without asking, since there's nothing to lose.
- After recovering edits over a changed file the last known disk state is unknown, so the first save asks.
- In the save questions any key other than `y`, `n` or Esc is ignored. They guard a destructive action, so a stray key shouldn't answer one.
- FIFOs and devices are refused at open (exit 2): reading one could hang forever, and a rename would replace the device node.

## 0.1.1 quitting, Save As and untitled buffers

- The unsaved-quit prompt ignores every key other than `y`, `n` and Esc, including a second Ctrl-Q. It guards the only action that discards work and its swap, so a stray key shouldn't answer it. AC-18 changed to match. "Quit without saving" in the palette still discards at once, since it's named for that.
- `y` when the save fails, or is cancelled, stays in the editor. Nothing is lost, and the error points at Save as.
- Save As starts from the current path, so a small rename needs little typing. It adds a tenth palette command. It refuses a target that another Kyte holds, because the swap can't be replaced underneath that session. It writes straight away when the target doesn't exist, and asks first when it does.
- A buffer with no file name gets its own swap, named by PID and start time, so several can run at once. Its base is empty, so it never needs a snapshot. A bare `kyte` offers the newest crashed one; any others stay until that one is resolved. Crashed untitled swaps that hold no text are deleted.
- After recovery the cursor starts at the top of the buffer, as with any freshly opened file.

## 0.1.1 line endings

- A CRLF is one line ending: End stops before the `\r`, and Left, Right, Backspace, Delete, `k`, `d` and `w` all treat the pair as one step. This replaces the T2 note that showed the `\r`, and the rule that `\r` is always an ordinary character. A lone `\r` still is.
- Enter and paste use the ending most of the file uses, with LF on a tie, decided when the buffer opens. Each existing line keeps its own ending, so a mixed file stays mixed. Converting a file is still out of scope.
- No edit may leave the cursor between `\r` and `\n`; `apply`, undo and redo move it back before the `\r`.
- The status bar shows `CRLF`, since the `\r` is no longer visible.

## 0.1.1 smaller fixes

- Split key sequences: this replaces the "do not heal" rule in `ARCHITECTURE` section 5.2. A `[` or `O` within 150 ms of the Esc starts discarding: `[` until its final character, `O` for one character. Only those two keys start it, and neither is an operator, so a typed operator is never affected. A sequence split by more than 150 ms is still typed as text. Kyte has to guess here, and a longer window would start eating real typing.
- Added `signal-hook` as a direct dependency for SIGTERM, SIGHUP and SIGINT. crossterm already uses it, so the build has no new crate. A thread waits for the signal, restores the terminal and exits with 128 + signal. Edits have already reached the swap, so no fsync is needed for the process to die safely; only a power cut can lose the last second.
- After a swap write fails Kyte stops appending, because records after a torn one could never be replayed. The status bar shows `no crash recovery` until a save creates a working swap. The same marker shows when a file opened with no swap.
- Super, Hyper and Meta chords are ignored whole, including on non-text keys.
- Pasting into the search prompt searches. Go to line and Save as take the first pasted line.
- Tests that need a read-only directory skip when run as root, because root can write to it anyway.

## 0.1.1 long lines

- `graphemes_with_char_offsets` built a vector for the whole line, at 24 bytes per grapheme, several times per key. It's now a lazy iterator, so callers stop as soon as they have their answer.
- ASCII takes a fast path: an ASCII byte followed by another ASCII byte is its own grapheme unless it's the CR of a CRLF, because everything that joins graphemes is non-ASCII. Runs of printable ASCII are skipped in bulk when finding a column. Property tests check all of this against full segmentation.
- Measured on a release build, per keystroke at the end of a 5 MB single line: ASCII went from about 400 ms to 6 ms. Text that's mostly non-ASCII still goes through segmentation, and costs more; `README.md` gives the current figures.

## 0.1.1 release number

The versioning rule makes any release with a new feature a MINOR bump, and 0.1.1 adds Save As, untitled recovery and `--break-lock`. It was released as a PATCH by decision on Oct 30, 2026, because those features exist to fix lost-work bugs. It's an exception, not a change to the rule.

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
- A paste while the overlay is open closes the overlay and inserts the paste (it is not an "unknown key"; dropping pasted text silently would lose data). In the palette, pasted text goes into the query with line breaks as spaces; in prompts it is ignored (changed in 0.1.1).

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
- Very long paths can exceed the 255-byte file-name limit once escaped; this is not handled in the MVP (swap creation fails with a warning and the file still opens). Fixed in 0.1.1; see above.
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
