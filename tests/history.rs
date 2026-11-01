//! File history log: writing, reading back, crashes, and moving through time.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use kyte::buffer::Edit;
use kyte::history::{
    compact, log_path_in, sweep_unsaved_in, unsaved_log_path_in, Change, FileHistory, SnapshotKind,
    Timeline,
};
use proptest::prelude::*;
use ropey::Rope;

fn r(s: &str) -> Rope {
    Rope::from_str(s)
}

fn ins(at: usize, s: &str) -> Edit {
    Edit::insert(at, s)
}

fn del(at: usize, s: &str) -> Edit {
    Edit {
        at,
        removed: s.to_string(),
        inserted: String::new(),
    }
}

/// An unsaved buffer's log with `text` typed into it.
fn unsaved(t: &T, text: &str) -> FileHistory {
    let mut h = FileHistory::unsaved_in(&t.dir, Path::new("/state/untitled-7-9.kyteswp"), &r(""));
    h.append(&[ins(0, text)]).unwrap();
    h
}

struct T {
    _d: tempfile::TempDir,
    dir: PathBuf,
    target: PathBuf,
}

fn setup() -> T {
    let d = tempfile::tempdir().unwrap();
    T {
        dir: d.path().join("history"),
        target: d.path().join("file.txt"),
        _d: d,
    }
}

impl T {
    fn writer(&self, base: &str) -> FileHistory {
        FileHistory::new_in(&self.dir, &self.target, &r(base))
    }

    fn log(&self) -> PathBuf {
        log_path_in(&self.dir, &self.target)
    }

    fn timeline(&self) -> Timeline {
        Timeline::load(&self.log()).unwrap()
    }
}

fn text(t: &Timeline, i: usize) -> String {
    t.text_at(i).to_string()
}

#[test]
fn nothing_is_written_until_the_first_edit() {
    let t = setup();
    let mut h = t.writer("base");
    h.close(&r("base")).unwrap();
    assert!(!t.log().exists());
    assert!(!t.dir.exists());
}

#[test]
fn every_edit_is_a_version() {
    let t = setup();
    let mut h = t.writer("abc");
    h.append(&[ins(3, "d")]).unwrap();
    h.append(&[ins(4, "e"), del(0, "a")]).unwrap();
    let tl = t.timeline();
    assert_eq!(tl.len(), 4);
    assert_eq!(text(&tl, 0), "abc");
    assert_eq!(text(&tl, 1), "abcd");
    assert_eq!(text(&tl, 2), "abcde");
    assert_eq!(text(&tl, 3), "bcde");
}

#[test]
fn log_is_private() {
    let t = setup();
    let mut h = t.writer("x");
    h.append(&[ins(0, "y")]).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&t.log()), 0o600);
    assert_eq!(mode(&t.dir), 0o700);
}

#[test]
fn history_carries_across_sessions_without_copying_the_file_again() {
    let t = setup();
    let mut h = t.writer("one");
    h.append(&[ins(3, " two")]).unwrap();
    h.mark_saved(&r("one two")).unwrap();
    h.close(&r("one two")).unwrap();
    let size = fs::metadata(t.log()).unwrap().len();
    let mut h = t.writer("one two");
    h.append(&[ins(7, "!")]).unwrap();
    let tl = t.timeline();
    assert_eq!(tl.len(), 3, "a second session adds only its edit");
    assert_eq!(text(&tl, 2), "one two!");
    assert!(fs::metadata(t.log()).unwrap().len() - size < 64);
}

#[test]
fn a_file_changed_between_sessions_is_stored_as_its_own_version() {
    let t = setup();
    let mut h = t.writer("mine");
    h.append(&[ins(4, "!")]).unwrap();
    h.mark_saved(&r("mine!")).unwrap();
    h.close(&r("mine!")).unwrap();
    let mut h = t.writer("theirs");
    h.append(&[ins(6, "?")]).unwrap();
    let tl = t.timeline();
    assert_eq!(text(&tl, 1), "mine!");
    assert_eq!(text(&tl, 2), "theirs");
    assert_eq!(text(&tl, 3), "theirs?");
}

#[test]
fn a_discarded_session_keeps_its_edits_and_the_next_one_starts_from_disk() {
    let t = setup();
    let mut h = t.writer("disk");
    h.append(&[ins(0, "junk ")]).unwrap();
    h.close(&r("junk disk")).unwrap();
    let mut h = t.writer("disk");
    h.append(&[ins(4, "!")]).unwrap();
    let tl = t.timeline();
    assert_eq!(text(&tl, 1), "junk disk", "every edit stays");
    assert_eq!(text(&tl, tl.len() - 1), "disk!");
}

#[test]
fn torn_tail_is_dropped_and_the_log_keeps_working() {
    let t = setup();
    let mut h = t.writer("abc");
    h.append(&[ins(3, "1")]).unwrap();
    h.append(&[ins(4, "2")]).unwrap();
    drop(h);
    let len = fs::metadata(t.log()).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(t.log())
        .unwrap()
        .set_len(len - 3)
        .unwrap();
    let tl = t.timeline();
    assert_eq!(
        text(&tl, tl.len() - 1),
        "abc1",
        "only the torn record is lost"
    );
    // Reopening after the crash: the last state isn't known, so the base is stored.
    let mut h = t.writer("abc1");
    h.append(&[ins(4, "3")]).unwrap();
    let tl = t.timeline();
    assert_eq!(text(&tl, tl.len() - 1), "abc13");
}

#[test]
fn damaged_snapshot_stops_reading_without_panicking() {
    let t = setup();
    let mut h = t.writer("hello world");
    h.append(&[ins(0, "x")]).unwrap();
    drop(h);
    let mut bytes = fs::read(t.log()).unwrap();
    bytes[60] ^= 0xff;
    fs::write(t.log(), &bytes).unwrap();
    assert!(t.timeline().is_empty());
    let mut h = t.writer("fresh");
    h.append(&[ins(0, "y")]).unwrap();
    let tl = t.timeline();
    assert_eq!(text(&tl, tl.len() - 1), "yfresh", "a broken log recovers");
}

#[test]
fn unrelated_file_at_the_log_path_is_replaced() {
    let t = setup();
    fs::create_dir_all(&t.dir).unwrap();
    fs::write(t.log(), "not a log").unwrap();
    let mut h = t.writer("a");
    h.append(&[ins(1, "b")]).unwrap();
    assert_eq!(text(&t.timeline(), 1), "ab");
}

#[test]
fn large_snapshot_spans_records() {
    let t = setup();
    let big = "0123456789abcde\n".repeat(200_000); // 3.2 MB
    let mut h = t.writer(&big);
    h.append(&[ins(0, "x")]).unwrap();
    let tl = t.timeline();
    assert_eq!(tl.len(), 2);
    assert_eq!(text(&tl, 0), big);
    assert_eq!(text(&tl, 1), format!("x{big}"));
}

#[test]
fn any_version_of_a_long_history_reads_back() {
    let t = setup();
    let mut h = t.writer("");
    for i in 0..2_000 {
        h.append(&[ins(i, "x")]).unwrap();
    }
    let tl = t.timeline();
    assert_eq!(tl.len(), 2_001);
    for i in [0, 1, 255, 256, 257, 1_024, 2_000] {
        assert_eq!(text(&tl, i).len(), i, "version {i}");
    }
}

#[test]
fn outside_version_is_kept_when_a_save_replaces_it() {
    let t = setup();
    let mut h = t.writer("v1");
    h.append(&[ins(2, " mine")]).unwrap();
    h.snapshot(&r("theirs"), SnapshotKind::Outside).unwrap();
    h.mark_saved(&r("v1 mine")).unwrap();
    let tl = t.timeline();
    assert_eq!(text(&tl, 2), "theirs");
    assert_eq!(text(&tl, 3), "v1 mine");
    assert!(tl.version(3).saved);
    assert!(tl.describe(2).contains("outside"));
}

#[test]
fn saving_without_edits_stores_the_text_once() {
    let t = setup();
    let mut h = t.writer("saved");
    h.mark_saved(&r("saved")).unwrap();
    h.mark_saved(&r("saved")).unwrap();
    let tl = t.timeline();
    assert_eq!(tl.len(), 1);
    assert!(tl.version(0).saved);
}

#[test]
fn bursts_end_at_gaps_saves_and_snapshots() {
    let t = setup();
    let mut h = t.writer("");
    h.append_at(&[ins(0, "a")], 10_000).unwrap();
    h.append_at(&[ins(1, "b")], 11_000).unwrap();
    h.append_at(&[ins(2, "c")], 20_000).unwrap();
    h.append_at(&[ins(3, "d")], 21_000).unwrap();
    let tl = t.timeline();
    // 0 snapshot, 1 a, 2 b | 3 c, 4 d
    assert_eq!(tl.prev_burst(4), 2);
    assert_eq!(tl.prev_burst(2), 0);
    assert_eq!(tl.prev_burst(0), 0);
    assert_eq!(tl.next_burst(0), 2);
    assert_eq!(tl.next_burst(2), 4);
    assert_eq!(tl.next_burst(4), 4);
}

#[test]
fn saved_versions_can_be_stepped_through() {
    let t = setup();
    let mut h = t.writer("a");
    h.append(&[ins(1, "b")]).unwrap();
    h.mark_saved(&r("ab")).unwrap();
    h.append(&[ins(2, "c")]).unwrap();
    h.append(&[ins(3, "d")]).unwrap();
    h.mark_saved(&r("abcd")).unwrap();
    let tl = t.timeline();
    assert_eq!(tl.prev_saved(3), Some(1));
    assert_eq!(tl.next_saved(1), Some(3));
    assert_eq!(tl.prev_saved(1), None);
    assert_eq!(tl.next_saved(3), None);
    assert!(tl.describe(3).contains("saved"));
}

#[test]
fn deleting_the_history_starts_it_again() {
    let t = setup();
    let mut h = t.writer("a");
    h.append(&[ins(1, "b")]).unwrap();
    h.delete(&r("ab")).unwrap();
    assert!(!t.log().exists());
    assert!(t.timeline().is_empty());
    h.append(&[ins(2, "c")]).unwrap();
    let tl = t.timeline();
    assert_eq!(tl.len(), 2, "starts from the text at the time");
    assert_eq!(text(&tl, 0), "ab");
    h.delete(&r("abc")).unwrap();
    h.delete(&r("abc")).unwrap();
}

/// A log of sessions: `saves` saved versions, with `typing` edits between.
fn long_log(t: &T, saves: usize, typing: usize) -> Vec<String> {
    let mut text = String::from("start\n");
    let mut h = t.writer(&text);
    let mut saved = Vec::new();
    for s in 0..saves {
        for i in 0..typing {
            let c = char::from(b'a' + (i % 26) as u8).to_string();
            h.append(&[ins(text.chars().count(), &c)]).unwrap();
            text.push_str(&c);
        }
        h.mark_saved(&r(&text)).unwrap();
        saved.push(text.clone());
        let _ = s;
    }
    h.close(&r(&text)).unwrap();
    saved
}

fn saved_texts(tl: &Timeline) -> Vec<String> {
    (0..tl.len())
        .filter(|i| tl.version(*i).saved)
        .map(|i| text(tl, i))
        .collect()
}

#[test]
fn a_log_under_the_limit_is_left_alone() {
    let t = setup();
    long_log(&t, 2, 50);
    let before = fs::read(t.log()).unwrap();
    compact(&t.log(), 1 << 30, 1 << 29).unwrap();
    assert_eq!(fs::read(t.log()).unwrap(), before);
}

#[test]
fn compacting_shrinks_the_log_and_keeps_every_saved_version() {
    let t = setup();
    let saved = long_log(&t, 10, 400);
    let before_len = fs::metadata(t.log()).unwrap().len();
    let before = t.timeline();
    assert!(before.len() > 4_000);
    compact(&t.log(), 1_000, 4_000).unwrap();
    let after = t.timeline();
    assert!(fs::metadata(t.log()).unwrap().len() < before_len / 4);
    assert!(after.len() < before.len() / 2, "{}", after.len());
    assert_eq!(saved_texts(&after), saved, "no saved version is lost");
    assert_eq!(
        text(&after, after.len() - 1),
        text(&before, before.len() - 1),
        "the newest version is untouched"
    );
}

#[test]
fn compacting_keeps_recent_edits_and_drops_old_ones_first() {
    let t = setup();
    long_log(&t, 2, 2_000);
    let before = t.timeline();
    compact(&t.log(), 1_000, 20_000).unwrap();
    let after = t.timeline();
    let (n, m) = (before.len(), after.len());
    assert!(m < n, "something was dropped");
    for back in 1..200 {
        assert_eq!(
            text(&before, n - back),
            text(&after, m - back),
            "{back} back"
        );
    }
}

#[test]
fn compacting_keeps_timestamps() {
    let t = setup();
    let mut h = t.writer("");
    h.append_at(&[ins(0, "a")], 1_000).unwrap();
    h.mark_saved(&r("a")).unwrap();
    for i in 1..300 {
        h.append_at(&[ins(i, "b")], 2_000).unwrap();
    }
    let saved_time = t.timeline().version(1).time_ms;
    drop(h);
    compact(&t.log(), 10, 2_000).unwrap();
    let after = t.timeline();
    assert_eq!(after.version(1).time_ms, saved_time);
    assert!(after.version(1).saved);
}

#[test]
fn outside_versions_survive_compacting() {
    let t = setup();
    let mut h = t.writer("v1");
    for i in 0..500 {
        h.append(&[ins(2 + i, "x")]).unwrap();
    }
    let mine = format!("v1{}", "x".repeat(500));
    h.note_outside(&r("theirs"), &r(&mine)).unwrap();
    for i in 0..500 {
        h.append(&[ins(502 + i, "y")]).unwrap();
    }
    drop(h);
    compact(&t.log(), 10, 2_000).unwrap();
    let tl = t.timeline();
    assert!((0..tl.len()).any(|i| text(&tl, i) == "theirs"));
}

#[test]
fn outside_versions_survive_repeated_compacting() {
    let t = setup();
    let mut h = t.writer("v1");
    for i in 0..500 {
        h.append(&[ins(2 + i, "x")]).unwrap();
    }
    let mine = format!("v1{}", "x".repeat(500));
    h.note_outside(&r("theirs"), &r(&mine)).unwrap();
    for i in 0..500 {
        h.append(&[ins(502 + i, "y")]).unwrap();
    }
    let mine = format!("{mine}{}", "y".repeat(500));
    h.close(&r(&mine)).unwrap();
    let is_theirs = |tl: &Timeline| (0..tl.len()).any(|i| text(tl, i) == "theirs");
    for round in 0..3 {
        compact(&t.log(), 10, 2_000).unwrap();
        let tl = t.timeline();
        assert!(is_theirs(&tl), "lost in round {round}");
        let keeps_label = (0..tl.len()).any(|i| {
            matches!(
                tl.version(i).change,
                Change::Snapshot {
                    kind: SnapshotKind::Outside,
                    ..
                }
            )
        });
        assert!(keeps_label, "not marked as outside in round {round}");
        // Another session piles more edits on top.
        let end = text(&tl, tl.len() - 1);
        let mut h = t.writer(&end);
        for i in 0..1_000 {
            h.append(&[ins(end.chars().count() + i, "z")]).unwrap();
        }
        drop(h);
    }
}

#[test]
fn saved_versions_survive_repeated_compacting() {
    let t = setup();
    let saved = long_log(&t, 6, 300);
    for _ in 0..3 {
        compact(&t.log(), 10, 1_000).unwrap();
        assert_eq!(saved_texts(&t.timeline()), saved);
    }
}

#[test]
fn deleting_the_history_removes_a_leftover_compacting_file() {
    let t = setup();
    let mut h = t.writer("a");
    h.append(&[ins(1, "b")]).unwrap();
    let leftover = t.log().with_extension("compacting");
    fs::write(&leftover, "a full copy of the history").unwrap();
    h.delete(&r("ab")).unwrap();
    assert!(!leftover.exists());
}

#[test]
fn a_new_session_carries_on_after_compacting_without_another_copy() {
    let t = setup();
    let saved = long_log(&t, 3, 300);
    let last = saved.last().unwrap().clone();
    compact(&t.log(), 100, 1_000).unwrap();
    let size = fs::metadata(t.log()).unwrap().len();
    let mut h = t.writer(&last);
    h.append(&[ins(0, "!")]).unwrap();
    assert!(
        fs::metadata(t.log()).unwrap().len() - size < 100,
        "no snapshot"
    );
    let tl = t.timeline();
    assert_eq!(text(&tl, tl.len() - 1), format!("!{last}"));
}

#[test]
fn compacting_leaves_no_stray_file_and_stays_private() {
    let t = setup();
    long_log(&t, 2, 300);
    compact(&t.log(), 100, 500).unwrap();
    let stray = fs::read_dir(&t.dir)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().ends_with(".compacting"));
    assert!(!stray);
    assert_eq!(
        fs::metadata(t.log()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_saved_version_too_large_for_the_limit_is_still_kept() {
    let t = setup();
    let big = "x".repeat(50_000);
    let mut h = t.writer("");
    h.append(&[ins(0, &big)]).unwrap();
    h.mark_saved(&r(&big)).unwrap();
    for i in 0..200 {
        h.append(&[ins(50_000 + i, "y")]).unwrap();
    }
    drop(h);
    compact(&t.log(), 100, 200).unwrap();
    assert!(saved_texts(&t.timeline()).contains(&big));
}

/// A log whose edits are scattered across a file with many saves, the case
/// where collapsing can't help much.
fn scattered_log(t: &T, rounds: usize) -> Vec<String> {
    let base = "line of text\n".repeat(400);
    let mut rope = r(&base);
    let mut h = t.writer(&base);
    let mut saved = Vec::new();
    for round in 0..rounds {
        for i in 0..40 {
            let at = (round * 41 + i) * 7919 % rope.len_chars();
            rope.insert(at, "x");
            h.append(&[ins(at, "x")]).unwrap();
        }
        h.mark_saved(&rope).unwrap();
        saved.push(rope.to_string());
    }
    h.close(&rope).unwrap();
    saved
}

#[test]
fn scattered_edits_never_make_a_log_bigger() {
    let t = setup();
    let saved = scattered_log(&t, 30);
    let before = fs::metadata(t.log()).unwrap().len();
    compact(&t.log(), 100, 200).unwrap();
    assert!(fs::metadata(t.log()).unwrap().len() <= before);
    assert_eq!(saved_texts(&t.timeline()), saved);
}

#[test]
fn a_log_that_cant_shrink_isnt_retried_until_it_grows() {
    let t = setup();
    long_log(&t, 400, 1);
    compact(&t.log(), 100, 200).unwrap();
    let tried = t.log().with_extension("compact-tried");
    assert!(tried.exists(), "still over the limit, so it's noted");
    let size = fs::metadata(t.log()).unwrap().len();
    fs::write(&tried, size.to_string()).unwrap();
    let before = fs::read(t.log()).unwrap();
    compact(&t.log(), 100, 200).unwrap();
    assert_eq!(fs::read(t.log()).unwrap(), before, "not worth another try");
    let mut h = t.writer(saved_last(&t).as_str());
    h.delete(&r("")).unwrap();
    assert!(!tried.exists(), "deleting the history clears the note");
}

fn saved_last(t: &T) -> String {
    let tl = t.timeline();
    text(&tl, tl.len() - 1)
}

#[derive(Debug, Clone)]
enum Step {
    Type(usize),
    Save,
    Outside(u8),
    CloseAndReopen,
    Compact(u64),
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    proptest::collection::vec(
        prop_oneof![
            4 => (1usize..120).prop_map(Step::Type),
            2 => Just(Step::Save),
            1 => any::<u8>().prop_map(Step::Outside),
            1 => Just(Step::CloseAndReopen),
            2 => (0u64..4_000).prop_map(Step::Compact),
        ],
        1..40,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever happens, every saved version and every outside version stays
    /// in the timeline, and the newest version is the text being edited.
    #[test]
    fn retention_never_loses_what_it_promises_to_keep(script in steps()) {
        let t = setup();
        let mut text_now = String::from("start\n");
        let mut h = t.writer(&text_now);
        h.append(&[ins(0, "x")]).unwrap();
        text_now.insert(0, 'x');
        let mut promised: Vec<String> = Vec::new();
        for step in script {
            match step {
                Step::Type(n) => {
                    for i in 0..n {
                        let at = (i * 37) % (text_now.len() + 1);
                        text_now.insert(at, 'y');
                        h.append(&[ins(at, "y")]).unwrap();
                    }
                }
                Step::Save => {
                    h.mark_saved(&r(&text_now)).unwrap();
                    promised.push(text_now.clone());
                }
                Step::Outside(n) => {
                    let outside = format!("outside version {n} {}\n", promised.len());
                    h.note_outside(&r(&outside), &r(&text_now)).unwrap();
                    promised.push(outside);
                }
                Step::CloseAndReopen => {
                    h.close(&r(&text_now)).unwrap();
                    h = t.writer(&text_now);
                    h.append(&[ins(0, "z")]).unwrap();
                    text_now.insert(0, 'z');
                }
                Step::Compact(target) => {
                    h.close(&r(&text_now)).unwrap();
                    compact(&t.log(), 0, target).unwrap();
                    h = t.writer(&text_now);
                    h.append(&[ins(0, "c")]).unwrap();
                    text_now.insert(0, 'c');
                }
            }
        }
        h.close(&r(&text_now)).unwrap();
        compact(&t.log(), 0, 500).unwrap();
        let tl = t.timeline();
        let all: Vec<String> = (0..tl.len()).map(|i| text(&tl, i)).collect();
        for p in &promised {
            prop_assert!(all.contains(p), "lost {p:?}");
        }
        prop_assert_eq!(all.last(), Some(&text_now));
    }
}

#[test]
fn edit_past_the_text_ends_the_timeline_there() {
    let t = setup();
    let mut h = t.writer("ab");
    h.append(&[ins(2, "c")]).unwrap();
    h.append(&[del(100, "zz")]).unwrap();
    h.append(&[ins(0, "never")]).unwrap();
    let tl = t.timeline();
    assert_eq!(tl.len(), 2);
    assert_eq!(text(&tl, 1), "abc");
    assert!(matches!(tl.version(1).change, Change::Edit { .. }));
}

// ---------------------------------------------------------------- unsaved buffers

#[test]
fn an_unsaved_log_goes_by_its_swap() {
    let t = setup();
    let h = unsaved(&t, "a");
    assert!(h.is_unsaved());
    assert_eq!(h.path(), t.dir.join("untitled-7-9.kytehist"));
    assert!(h.path().exists());
}

#[test]
fn an_unsaved_log_moves_to_a_file_with_no_log() {
    let t = setup();
    let mut h = unsaved(&t, "ab");
    let old = h.path().to_path_buf();
    h.move_to_in(&t.dir, &t.target).unwrap();
    assert!(!old.exists());
    assert!(!h.is_unsaved());
    assert_eq!(h.target(), t.target);
    // The open log carries on where it was, with no second snapshot.
    h.append(&[ins(2, "c")]).unwrap();
    h.mark_saved(&r("abc")).unwrap();
    h.close(&r("abc")).unwrap();
    let tl = Timeline::load(&log_path_in(&t.dir, &t.target)).unwrap();
    assert_eq!(tl.len(), 3);
    assert_eq!(tl.text_at(0), r(""));
    assert_eq!(tl.text_at(1), r("ab"));
    assert_eq!(tl.text_at(2), r("abc"));
    assert!(tl.version(2).saved);
}

#[test]
fn an_unsaved_log_with_no_edits_moves_as_nothing() {
    let t = setup();
    let swap = Path::new("/state/untitled-7-9.kyteswp");
    let mut h = FileHistory::unsaved_in(&t.dir, swap, &r(""));
    h.move_to_in(&t.dir, &t.target).unwrap();
    assert!(
        !h.path().exists(),
        "nothing is written until there's something to keep"
    );
    h.append(&[ins(0, "x")]).unwrap();
    h.close(&r("x")).unwrap();
    let tl = Timeline::load(&log_path_in(&t.dir, &t.target)).unwrap();
    assert_eq!(tl.text_at(tl.len() - 1), r("x"));
}

#[test]
fn an_unsaved_log_goes_after_a_log_the_file_already_has() {
    let t = setup();
    let mut old = FileHistory::new_in(&t.dir, &t.target, &r("old"));
    old.append(&[ins(3, "er")]).unwrap();
    old.close(&r("older")).unwrap();

    let mut h = unsaved(&t, "new");
    let from = h.path().to_path_buf();
    h.move_to_in(&t.dir, &t.target).unwrap();
    assert!(!from.exists());
    h.mark_saved(&r("new")).unwrap();
    h.append(&[ins(3, "s")]).unwrap();
    h.close(&r("news")).unwrap();

    let tl = Timeline::load(&log_path_in(&t.dir, &t.target)).unwrap();
    let texts: Vec<String> = (0..tl.len()).map(|i| tl.text_at(i).to_string()).collect();
    assert_eq!(texts, ["old", "older", "", "new", "news"]);
    assert!(tl.version(3).saved);
}

#[test]
fn moving_into_a_log_with_a_torn_tail_drops_the_tail() {
    let t = setup();
    let mut old = FileHistory::new_in(&t.dir, &t.target, &r("old"));
    old.append(&[ins(3, "er")]).unwrap();
    old.close(&r("older")).unwrap();
    let log = log_path_in(&t.dir, &t.target);
    let mut bytes = fs::read(&log).unwrap();
    bytes.extend_from_slice(&[9, 0, 0, 0, 1, 2]);
    fs::write(&log, bytes).unwrap();

    let mut h = unsaved(&t, "new");
    h.move_to_in(&t.dir, &t.target).unwrap();
    h.close(&r("new")).unwrap();
    let tl = Timeline::load(&log).unwrap();
    assert_eq!(tl.text_at(tl.len() - 1), r("new"));
    assert_eq!(tl.text_at(1), r("older"));
}

#[test]
fn moving_a_log_with_a_torn_tail_leaves_the_tail_behind() {
    let t = setup();
    let mut old = FileHistory::new_in(&t.dir, &t.target, &r("old"));
    old.append(&[ins(3, "er")]).unwrap();
    old.close(&r("older")).unwrap();

    let mut h = unsaved(&t, "new");
    let mut torn = OpenOptions::new().append(true).open(h.path()).unwrap();
    std::io::Write::write_all(&mut torn, &[9, 0, 0, 0, 1, 2]).unwrap();
    h.move_to_in(&t.dir, &t.target).unwrap();
    h.append(&[ins(3, "s")]).unwrap();
    h.close(&r("news")).unwrap();
    let tl = Timeline::load(&log_path_in(&t.dir, &t.target)).unwrap();
    assert_eq!(tl.text_at(tl.len() - 1), r("news"));
}

#[test]
fn a_moved_log_stays_private() {
    let t = setup();
    let mut h = unsaved(&t, "a");
    h.move_to_in(&t.dir, &t.target).unwrap();
    let mode = fs::metadata(h.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn the_sweep_takes_only_unsaved_logs_with_no_swap() {
    let t = setup();
    let swaps = t.dir.with_file_name("swap");
    fs::create_dir_all(&t.dir).unwrap();
    fs::create_dir_all(&swaps).unwrap();
    fs::write(swaps.join("untitled-1-2.kyteswp"), "").unwrap();
    let keep = |name: &str| fs::write(t.dir.join(name), "x").unwrap();
    keep("untitled-1-2.kytehist");
    keep("untitled-3-4.kytehist");
    // A long path's log keeps the end of the path, so it can start this way.
    keep("untitled-notes.txt~0000000100000020.kytehist");
    keep("untitled-1.kytehist");
    keep("untitled-1-x.kytehist");
    keep("%2Fhome%2Funtitled-3-4.kytehist");
    keep("untitled-3-4.compact-tried");

    sweep_unsaved_in(&t.dir, &swaps);

    let mut left: Vec<String> = fs::read_dir(&t.dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "%2Fhome%2Funtitled-3-4.kytehist",
            "untitled-1-2.kytehist",
            "untitled-1-x.kytehist",
            "untitled-1.kytehist",
            "untitled-3-4.compact-tried",
            "untitled-notes.txt~0000000100000020.kytehist",
        ]
    );
}

#[test]
fn the_sweep_survives_no_history_directory() {
    let t = setup();
    sweep_unsaved_in(&t.dir, &t.dir);
}

#[test]
fn an_unsaved_log_name_comes_from_the_swap_name() {
    assert_eq!(
        unsaved_log_path_in(Path::new("/h"), Path::new("/s/untitled-12-34.kyteswp")),
        Path::new("/h/untitled-12-34.kytehist")
    );
}
