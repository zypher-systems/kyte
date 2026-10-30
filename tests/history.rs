//! File history log: writing, reading back, crashes, and moving through time.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use kyte::buffer::Edit;
use kyte::history::{log_path_in, Change, FileHistory, SnapshotKind, Timeline};
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
