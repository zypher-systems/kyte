# Acceptance results

All 25 criteria from section 10 of `docs/ARCHITECTURE.md`. Each result comes from commands run on the first build machine (Linux, rustc 1.99.1) on 2026-10-30 (America/New_York). End-to-end tests (`tests/e2e.rs`) run the real `kyte` binary in a pseudo-terminal through `portable-pty` and read the screen through a `vt100` emulator. They wait for screen states (`wait_for`); they do not use fixed sleeps, except where a criterion is about time (the AC-14 pauses).

Gate commands, all run from the repo root:

```
cargo build --release                      # 0 warnings (also after `cargo clean --release -p kyte-editor`)
cargo fmt --check                          # clean
cargo clippy --all-targets -- -D warnings  # clean
cargo test                                 # 652 passed, 0 failed, 1 ignored (the AC-25 demo test)
cargo test --test demo -- --ignored        # 1 passed (AC-25)
make demo                                  # writes docs/demo/*
```

Updated on 2026-10-30, on rustc 1.98.1 with SELinux enforcing. `cargo test` breakdown: lib unit tests 355, `tests/buffer_props.rs` 2 (proptest), `tests/e2e.rs` 201, `tests/e2e_highlight.rs` 15, `tests/fileio.rs` 19, `tests/history.rs` 31, `tests/smoke.rs` 4, `tests/swap.rs` 25, `tests/demo.rs` 0 run and 1 ignored. The full suite was also run three times in a row with no failures.

| AC | Result | How it was verified |
| --- | --- | --- |
| AC-01 | pass | `cargo build --release` shows 0 warnings. `target/release/kyte --version` prints `kyte 0.6.0` with exit 0; `--help` exits 0; `--bogus` exits 2. Tests: `smoke.rs` `ac01_version_prints_and_exits_0`, `ac01_help_lists_options_and_exits_0` (checks every section 7 option), `ac01_unknown_option_exits_2`, `ac01_missing_value_exits_2`. |
| AC-02 | pass | `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` are clean. `cargo test`: 180 passed; the only ignored test is the demo (`ac25_demo_script_writes_cast_frames_and_media`). |
| AC-03 | pass | e2e `ac03_not_a_tty_exits_2`, `ac03_stdout_not_a_tty_exits_2`, `ac03_directory_exits_2`, `ac03_non_utf8_exits_2_and_file_unchanged` (byte comparison). By hand: `kyte /tmp </dev/null` exits 2 with "kyte: stdin and stdout must be a terminal". Library level: `tests/fileio.rs`. |
| AC-04 | pass | e2e `ac04_new_file_type_save_quit`: the file is exactly `hello\nworld`, exit 0. |
| AC-05 | pass | e2e `ac05_save_preserves_bytes_newline_mode_no_temp` (mode 0640 kept, trailing newline kept, no temp file); `tests/fileio.rs` save tests. |
| AC-06 | pass | e2e `ac06_cursor_motion_and_status_bar`: every listed key, `Ln X, Col Y` checked against the terminal cursor, and the remembered column across a shorter line. |
| AC-07 | pass | e2e `ac07_unicode_graphemes_and_wide_cursor` on `héllo 世界 🙂` (Backspace removes one grapheme; Left/Right; the cursor column counts double-width cells); buffer-level unit tests. |
| AC-08 | pass | e2e `ac08_bracketed_paste_is_one_undo_step`: a 5-line paste, then one Ctrl-Z restores the original. |
| AC-09 | pass | e2e `ac09_undo_redo_typing_and_operator` (`abc` + Ctrl-Z/Ctrl-Y, `Esc d` undone by one Ctrl-Z, `Esc u`/`Esc r`). |
| AC-10 | pass | e2e `ac10_resize_keeps_bars_at_bottom_and_cursor_visible`: 100x30 to 40x10 and back. |
| AC-11 | pass | e2e `ac11_large_file_ctrl_end_and_typing_within_1s` on 100,000 lines. Measured in one run: opened in 16.5 ms, keystroke visible in 5.4 ms (bound 1 s). |
| AC-12 | pass, changed in 0.3.0 | The legend is now a floating menu, so the footer stays clear. e2e `ac12_legend_appears_instantly_and_only_in_overlay`: the legend showed 5.5 ms after Esc in one run (bound 100 ms). The legend and `ESC` are absent in Insert mode, after an operator, after an unknown key, and after Esc, 500 ms, Esc. |
| AC-13 | pass | e2e `ac13_w_deletes_to_next_word`, `ac13_d_middle_last_and_only_line`, `ac13_d_cursor_lands_at_same_column_clamped`, `ac13_k_mid_line_and_at_end_of_line`, `ac13_percent_nested_not_on_bracket_and_unmatched`, `ac13_percent_unmatched_shows_message_and_does_not_move`. Each one saves and reads the file. There are also unit tests for every row of the operator table (`ops.rs`, `app.rs`). |
| AC-14 | pass | e2e `ac14_double_esc_within_150ms_opens_palette`, `ac14_esc_pause_esc_closes_overlay` (500 ms gap), `ac14_esc_ms_800_opens_palette_after_500ms`, `ac14_kyte_esc_ms_env_is_honoured`; each checks the buffer is unchanged. |
| AC-15 | pass | e2e `ac15_unknown_overlay_key_inserts_nothing` (`z`), `ac15_alt_d_bytes_behave_as_esc_then_d` (raw bytes `ESC d`), `ac15_ctrl_space_opens_overlay`. |
| AC-16 | pass | e2e `ac16_search_incremental_enter_wrap_esc` (`foo bar baz`: `ba` to bar, Enter to baz, wrap to bar, Esc keeps the cursor), `ac16_esc_slash_no_match_restores_origin`, `ac16_empty_query_does_nothing`; unit tests in `search.rs`. |
| AC-17 | pass, changed in 0.1.1 | The palette now has seventeen commands. e2e `ac17_palette_lists_every_command` (all are seen while scrolling the 8-row popup; see DECISIONS T8), `ac17_sav_puts_save_first_and_enter_saves`, `ac17_go_to_line_prompt` (`goto` and `go to`, clamping, non-numeric error), `ac17_esc_closes_without_running`, `ac17_toggle_line_numbers_and_key_help`, `ac17_quit_without_saving_command`; scorer unit tests in `palette.rs`. |
| AC-18 | pass, changed in 0.1.1 | `y` now saves and quits, `n` discards, Esc cancels, and other keys are ignored. e2e `ac18_quit_dirty_asks_then_discards`, `ac18_quit_dirty_y_saves_and_quits`, `ctrl_x_y_saves`, `y_with_a_failed_save_stays_open_and_keeps_the_swap`, `ac18_quit_clean_exits_and_restores_terminal` (alternate screen left, cursor visible). |
| AC-19 | pass | e2e `ac19_swap_exists_0600_outside_file_dir_and_recreated_after_save`, `ac19_discard_quit_removes_swap_and_print_swap_exits_3`; `tests/swap.rs` (permission bits, file name escaping). |
| AC-20 | pass | e2e `ac20_kill9_then_print_swap_and_recover_r` (SIGKILL via `kill -9`, then `--print-swap` output, the Recover modal, `r`, `[+]`, save), `ac20_recover_d_discards_and_opens_original`, `ac20_recover_q_quits_without_touching_anything`, `ac20_recover_then_more_edits_survive_a_second_crash`. The demo also performs this drill (frames 11 and 12). |
| AC-21 | pass | e2e `ac21_torn_tail_truncated_3_bytes`, `ac21_torn_tail_last_byte_flipped`; `tests/swap.rs` torn-tail tests. |
| AC-22 | pass, changed in 0.1.1 | A file that changed after a crash no longer loses the edits: e2e `ac22_edits_survive_a_file_that_changed_after_the_crash`, `ac22_discarding_recovered_edits_keeps_the_changed_file`. Only an unreadable swap is stale: e2e `ac22_unreadable_swap_offers_only_discard_or_quit`; `tests/swap.rs` damaged snapshot. |
| AC-23 | pass | e2e `ac23_second_instance_exits_2_naming_pid_and_leaves_swap` (the swap bytes are unchanged); `tests/swap.rs` live versus dead PID. |
| AC-24 | pass | e2e `ac24_save_into_read_only_dir_fails_visibly_and_keeps_everything`, `ac24_save_through_symlink_updates_target_keeps_link`; `tests/fileio.rs`. |
| AC-25 | pass | `make demo` ran headless (no display, no tmux) and wrote `docs/demo/demo.cast`, `demo.gif` (979x694, 33.75 s), `demo.mp4` (h264 978x694, 32.9 s; both checked with `ffprobe`), 13 PNGs, `steps.tsv` and 13 `frames/NN-*.txt`. `frames/03-legend.txt` contains the legend, `07-palette.txt` the palette, and `11-crash.txt` the Recover modal; PNGs 03, 07 and 11 were also inspected by eye. The skip path was checked with agg and ffmpeg removed from `PATH`: it wrote the cast, steps and frames and listed the gif, mp4 and PNGs as skipped. `cargo test --test demo -- --ignored` passed. |

Syntax highlighting (feature `highlight`, on by default): `cargo build --release --no-default-features` 0 warnings, `cargo clippy --all-targets --no-default-features -- -D warnings` clean, `cargo test --no-default-features` 588 passed, 0 failed, 1 ignored. The default build adds 64 tests that need highlighting.

Not verified: `make docker-test` and the Dockerfile, because Docker is not installed on the first build machine (see `docs/DECISIONS.md`).
