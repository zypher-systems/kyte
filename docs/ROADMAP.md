# Roadmap

Kyte is a terminal text editor that opens ready to type. It starts in edit mode, and Esc reaches its power features. That default is what it shares with Nano; it isn't a Nano clone, and its shortcuts are its own.

Version numbers below are targets. The version changes only when a release is decided.

## Now: 0.1.1, trust

Fix every way the current build can lose or damage work. Each fix gets a regression test.

### Work or data lost

- [x] Unsaved-quit prompt: `y` saves and quits, `n` discards, Esc cancels. Today `y` discards. Remove the double Ctrl-Q discard and update AC-18.
- [x] A buffer with no file name can be saved, and its edits reach a swap. Today Ctrl-S refuses and nothing protects the text.
- [x] A failed save leaves a way out: Save As, and quitting keeps the swap unless the user discards it. Today confirming quit deletes the swap.
- [x] A stale swap can still be recovered. Store the base content in the swap (format v2), so a crash followed by a change on disk doesn't strand the edits.
- [x] Ctrl-S warns when the file changed on disk since it was opened or last saved.
- [x] A read-only file isn't replaced without an explicit choice.

### Files damaged or refused

- [x] A save keeps owner, SELinux label, xattrs and ACLs, and handles hard links. Today the rename resets the label and drops the rest.
- [ ] CRLF files keep their line endings: End stops before the `\r`, and Enter inserts the file's own line ending.
- [x] The swap lock checks that the PID really is Kyte, and a stale lock can be overridden.
- [x] Long paths open with crash recovery working: hash the swap file name.

### Smaller fixes

- [ ] Split escape sequences no longer type stray text into the file, or the README stops claiming they can't.
- [ ] SIGTERM and SIGHUP restore the terminal and keep the swap.
- [ ] Paste works in the search and go-to-line prompts.
- [ ] Super-chorded keys don't type text.
- [x] FIFOs and device files are refused at open.
- [ ] A swap write failure stays visible until it's resolved, not only until the next key.
- [ ] Editing a very long single line stays responsive (about 350 ms per key at 5 MB today).
- [ ] Skip or adapt the read-only-directory tests when running as root, so they can pass in CI containers.

## Next: 0.2.0, daily driver

- [ ] Open a file from inside the editor
- [ ] Selection, cut, copy and paste, including the system clipboard over OSC 52
- [ ] Find and replace; case-insensitive search; visible match highlighting
- [ ] Word motion and word delete
- [ ] Auto-indent, and tabs or spaces per file
- [ ] Ctrl-K universal palette
- [ ] Review the Nano-derived aliases Ctrl-O and Ctrl-X. Proposed: Ctrl-X, Ctrl-C and Ctrl-V for cut, copy and paste, and Ctrl-O to open a file.

Before moving on, use Kyte for real work on actual terminals with the kitty keyboard protocol, and over SSH and tmux. So far that path has only been emulated.

## Later

- Reader mode
- Undo history that survives a restart
- Light and high-contrast themes
- Config file
- File tree
- Multiple buffers and splits
- Syntax highlighting on by default, with incremental parsing
- Packaging, and use as `$EDITOR`

## What 1.0 means

Proposed, not yet confirmed.

1.0 is released when every item below is done and Zypher Systems decides to release it. It isn't the automatic next version after 0.x.

- [ ] No known way to lose or damage work through crashes, failed saves, or files changing on disk
- [ ] Saving keeps a file's permissions, owner, labels, xattrs and line endings
- [ ] Selection, clipboard, find and replace, Save As, and opening files
- [ ] Ctrl-K universal palette, and the Esc layer finalized
- [ ] Syntax highlighting on by default
- [ ] A documented configuration file
- [ ] Tested on real terminals, including over SSH and tmux
- [ ] Installable as a signed package

## Repository

