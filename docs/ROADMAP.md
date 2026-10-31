# Roadmap

Kyte is a terminal text editor that opens ready to type. It starts in edit mode, and Esc reaches its power features. That default is what it shares with Nano; it isn't a Nano clone, and its shortcuts are its own.

Version numbers below are targets. The version changes only when a release is decided.

## Released: 0.1.1, trust

Fixed every known way the build could lose or damage work. Each fix has a regression test.

### Work or data lost

- [x] Unsaved-quit prompt: `y` saves and quits, `n` discards, Esc cancels. Today `y` discards. Remove the double Ctrl-Q discard and update AC-18.
- [x] A buffer with no file name can be saved, and its edits reach a swap. Today Ctrl-S refuses and nothing protects the text.
- [x] A failed save leaves a way out: Save As, and quitting keeps the swap unless the user discards it. Today confirming quit deletes the swap.
- [x] A stale swap can still be recovered. Store the base content in the swap (format v2), so a crash followed by a change on disk doesn't strand the edits.
- [x] Ctrl-S warns when the file changed on disk since it was opened or last saved.
- [x] A read-only file isn't replaced without an explicit choice.

### Files damaged or refused

- [x] A save keeps owner, SELinux label, xattrs and ACLs, and handles hard links. Today the rename resets the label and drops the rest.
- [x] CRLF files keep their line endings: End stops before the `\r`, and Enter inserts the file's own line ending.
- [x] The swap lock checks that the PID really is Kyte, and a stale lock can be overridden.
- [x] Long paths open with crash recovery working: hash the swap file name.

### Smaller fixes

- [x] Split escape sequences no longer type stray text into the file, or the README stops claiming they can't.
- [x] SIGTERM and SIGHUP restore the terminal and keep the swap.
- [x] Paste works in the search and go-to-line prompts.
- [x] Super-chorded keys don't type text.
- [x] FIFOs and device files are refused at open.
- [x] A swap write failure stays visible until it's resolved, not only until the next key.
- [x] Editing a very long single line stays responsive.
- [x] Skip or adapt the read-only-directory tests when running as root, so they can pass in CI containers.

## Released: 0.2.0, daily driver

- [x] Open a file from inside the editor
- [x] Selection, cut, copy and paste, including the system clipboard over OSC 52
- [x] Find and replace; case-insensitive search; visible match highlighting
- [x] Word motion and word delete
- [x] Auto-indent, and tabs or spaces per file
- [x] Ctrl-K universal palette
- [x] Replace the Nano-derived aliases: Ctrl-X, Ctrl-C and Ctrl-V for cut, copy and paste, and Ctrl-O to open a file.

## Released: 0.3.0, standing out

Kyte's pitch is an editor you can't lose work in and don't have to learn.

- [x] Syntax highlighting on by default
- [x] File history: every edit kept across sessions, browsable and restorable
- [x] Floating menus for everything the user picks from
- [x] Smart Esc layer: a menu that changes with context, using tree-sitter
- [x] Reader mode

## Released: 0.4.0, working with other tools

- [x] Review changes made by other tools, built on file history
- [x] CI

## Released: 0.5.0, a working everyday editor

- [x] Highlighting and code actions for twelve more languages
- [x] Scripts recognised by their `#!` line
- [x] Retention for file history, so logs can't grow without limit
- [x] A settings file
- [x] Soft wrap

## Released: 0.6.0, a public editor for code and writing

- [x] Signal theme, centred boxes, and a redrawn status bar and cursor line
- [x] Six themes, a `theme` setting and a theme picker
- [x] Several files open at once
- [x] A file tree
- [x] Highlighting for 33 languages, including Swift, LaTeX and BibTeX
- [x] A Writing setup for Markdown, text and LaTeX: wrap, word count and spell check
- [x] Citation keys from `.bib` files as you type `\cite{`
- [x] Building a paper's PDF from inside Kyte
- [x] Releases with an installer, `.deb`, `.rpm` and signed checksums
- [x] Open source on GitHub
- [x] Highlighting and file history before a file is saved, and a language box

## Next

- [ ] A Fedora COPR and an Arch AUR package, so package managers can update Kyte
- [ ] Multiple cursors and mouse support
- [ ] Code intelligence (LSP)

Keep using Kyte for real work along the way, on actual terminals with the kitty keyboard protocol, and over SSH and tmux.

## Later

- More spell check languages
- A high-contrast theme
- Splits
- Incremental parsing, and languages embedded in others
- Use as `$EDITOR`

## What 1.0 means

Proposed, not yet confirmed.

1.0 is released when every item below is done and Zypher Systems decides to release it. It isn't the automatic next version after 0.x.

- [ ] No known way to lose or damage work through crashes, failed saves, or files changing on disk
- [ ] Saving keeps a file's permissions, owner, labels, xattrs and line endings
- [ ] Selection, clipboard, find and replace, Save As, and opening files
- [ ] Ctrl-K universal palette, and the Esc layer finalized
- [x] Syntax highlighting on by default
- [x] A documented configuration file
- [ ] Tested on real terminals, including over SSH and tmux
- [ ] Installable as a signed package

## Repository

Kyte is open source at [github.com/zypher-systems/kyte](https://github.com/zypher-systems/kyte). `main` holds releases, and `dev` collects finished work.
