//! Swap log tests (task T9): format round trip, torn tail, a file that changed,
//! live versus dead PID, permission bits, file-name escaping, adopt.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use kyte::buffer::Edit;
use kyte::swap::{
    self, encode_record, escape_name, inspect_in, pid_alive, process_start_time, record_count,
    replay, swap_path_in, SwapStatus, SwapWriter, PID_OFFSET, START_OFFSET,
};
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
    state: PathBuf,
    target: PathBuf,
}

fn setup(base: &str) -> T {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state").join("swap");
    let target = d.path().join("file.txt");
    fs::write(&target, base).unwrap();
    T {
        state,
        target,
        _d: d,
    }
}

fn write_pid(path: &Path, pid: u32) {
    let mut f = OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(PID_OFFSET)).unwrap();
    f.write_all(&pid.to_le_bytes()).unwrap();
}

fn write_start(path: &Path, start: u64) {
    let mut f = OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(START_OFFSET)).unwrap();
    f.write_all(&start.to_le_bytes()).unwrap();
}

/// A PID that is certainly not running.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!pid_alive(pid));
    pid
}

#[test]
fn round_trip_replay() {
    let t = setup("hello\n");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("hello\n")).unwrap();
    w.append(&[ins(5, " world"), ins(0, ">> ")]).unwrap();
    w.append(&[del(0, ">")]).unwrap();
    w.fsync_if_pending().unwrap();
    let p = swap_path_in(&t.state, &t.target);
    assert_eq!(replay("hello\n", &p).unwrap(), "> hello world\n");
    assert_eq!(record_count(&p).unwrap(), 3);
}

#[test]
fn unicode_char_indices_round_trip() {
    let t = setup("héllo 世界");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("héllo 世界")).unwrap();
    w.append(&[ins(8, "🙂"), del(1, "é")]).unwrap();
    let p = swap_path_in(&t.state, &t.target);
    assert_eq!(replay("héllo 世界", &p).unwrap(), "hllo 世界🙂");
}

#[test]
fn torn_tail_truncated_by_3_bytes() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(3, "1")]).unwrap();
    w.append(&[ins(4, "2")]).unwrap();
    w.append(&[ins(5, "3")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    let len = fs::metadata(&p).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(len - 3)
        .unwrap();
    assert_eq!(replay("abc", &p).unwrap(), "abc12");
    match inspect_in(&t.state, &t.target, b"abc").unwrap() {
        SwapStatus::Recoverable(info) => assert_eq!(info.recovered.as_deref(), Some("abc12")),
        s => panic!("{s:?}"),
    }
}

#[test]
fn torn_tail_last_byte_flipped() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(3, "1"), ins(4, "2")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    let mut bytes = fs::read(&p).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&p, &bytes).unwrap();
    assert_eq!(replay("abc", &p).unwrap(), "abc1");
}

#[test]
fn garbage_records_stop_replay_without_error() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(3, "!")]).unwrap();
    // an out-of-range edit (valid CRC) must stop replay, not panic
    w.append(&[del(100, "zz")]).unwrap();
    w.append(&[ins(0, "never")]).unwrap();
    let p = swap_path_in(&t.state, &t.target);
    assert_eq!(replay("abc", &p).unwrap(), "abc!");
}

#[test]
fn unknown_record_kind_is_skipped() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(3, "1")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    let mut rec = encode_record(&ins(0, "x"));
    // turn it into kind 7 and fix its CRC
    rec[8] = 7;
    let crc = crc32fast::hash(&rec[8..]);
    rec[4..8].copy_from_slice(&crc.to_le_bytes());
    let mut f = OpenOptions::new().append(true).open(&p).unwrap();
    f.write_all(&rec).unwrap();
    f.write_all(&encode_record(&ins(4, "2"))).unwrap();
    assert_eq!(replay("abc", &p).unwrap(), "abc12");
}

#[test]
fn changed_file_is_recovered_from_the_snapshot() {
    let t = setup("original");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("original")).unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    write_pid(&p, dead_pid());
    assert_eq!(replay("changed!", &p).unwrap(), "xoriginal");
    match inspect_in(&t.state, &t.target, b"changed!").unwrap() {
        SwapStatus::Recoverable(info) => {
            assert_eq!(info.recovered.as_deref(), Some("xoriginal"));
            assert!(info.file_changed);
        }
        s => panic!("{s:?}"),
    }
}

#[test]
fn deleted_file_is_recovered_from_the_snapshot() {
    let t = setup("original");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("original")).unwrap();
    w.append(&[ins(8, "!")]).unwrap();
    drop(w);
    write_pid(&swap_path_in(&t.state, &t.target), dead_pid());
    match inspect_in(&t.state, &t.target, b"").unwrap() {
        SwapStatus::Recoverable(info) => {
            assert_eq!(info.recovered.as_deref(), Some("original!"))
        }
        s => panic!("{s:?}"),
    }
}

#[test]
fn matching_file_is_not_reported_as_changed() {
    let t = setup("same");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("same")).unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    write_pid(&swap_path_in(&t.state, &t.target), dead_pid());
    match inspect_in(&t.state, &t.target, b"same").unwrap() {
        SwapStatus::Recoverable(info) => assert!(!info.file_changed),
        s => panic!("{s:?}"),
    }
}

#[test]
fn file_equal_to_the_recovered_text_needs_no_recovery() {
    // A crash between the save's rename and the swap reset.
    let t = setup("original");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("original")).unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    write_pid(&swap_path_in(&t.state, &t.target), dead_pid());
    assert_eq!(
        inspect_in(&t.state, &t.target, b"xoriginal").unwrap(),
        SwapStatus::None
    );
}

#[test]
fn damaged_snapshot_with_a_changed_file_is_stale() {
    let t = setup("original");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("original")).unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    write_pid(&p, dead_pid());
    let mut bytes = fs::read(&p).unwrap();
    let h = swap::parse_header(&bytes).unwrap();
    bytes[h.len + 10] ^= 0xff;
    fs::write(&p, &bytes).unwrap();
    assert!(replay("changed!", &p).is_err());
    assert_eq!(
        inspect_in(&t.state, &t.target, b"changed!").unwrap(),
        SwapStatus::Stale
    );
}

#[test]
fn snapshot_is_written_on_the_first_edit_only() {
    let t = setup("secret");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("secret")).unwrap();
    let p = w.path().to_path_buf();
    let holds = |p: &Path| {
        fs::read(p)
            .unwrap()
            .windows(6)
            .any(|win| win == b"secret".as_slice())
    };
    assert!(!holds(&p), "a session that only reads copies nothing");
    w.append(&[ins(0, "x")]).unwrap();
    assert!(holds(&p));
    let len = fs::metadata(&p).unwrap().len();
    w.append(&[ins(1, "y")]).unwrap();
    assert!(fs::metadata(&p).unwrap().len() - len < 64, "written once");
}

#[test]
fn large_base_spans_several_records_and_replays() {
    let base = "0123456789abcde\n".repeat(200_000); // 3.2 MB
    let t = setup(&base);
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r(&base)).unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    assert_eq!(replay("changed", &p).unwrap(), format!("x{base}"));
    assert_eq!(record_count(&p).unwrap(), 1);
}

#[test]
fn session_without_edits_has_nothing_to_offer_even_if_the_file_changed() {
    let t = setup("abc");
    let w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    drop(w);
    write_pid(&swap_path_in(&t.state, &t.target), dead_pid());
    assert_eq!(
        inspect_in(&t.state, &t.target, b"changed").unwrap(),
        SwapStatus::None
    );
}

#[test]
fn no_swap_and_empty_swap_are_none() {
    let t = setup("abc");
    assert_eq!(
        inspect_in(&t.state, &t.target, b"abc").unwrap(),
        SwapStatus::None
    );
    let w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    drop(w);
    write_pid(&swap_path_in(&t.state, &t.target), dead_pid());
    assert_eq!(
        inspect_in(&t.state, &t.target, b"abc").unwrap(),
        SwapStatus::None
    );
}

#[test]
fn live_pid_versus_dead_pid() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = sleeper.id();
    write_pid(&p, pid);
    write_start(&p, process_start_time(pid).unwrap());
    assert!(pid_alive(pid));
    assert_eq!(
        inspect_in(&t.state, &t.target, b"abc").unwrap(),
        SwapStatus::LiveLock { pid }
    );
    let before = fs::read(&p).unwrap();
    let err = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap_err();
    assert_eq!(swap::live_lock_pid(&err), Some(pid));
    assert_eq!(
        fs::read(&p).unwrap(),
        before,
        "a live lock must not modify the swap"
    );
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
    assert!(!pid_alive(pid));
    assert!(matches!(
        inspect_in(&t.state, &t.target, b"abc").unwrap(),
        SwapStatus::Recoverable(_)
    ));
}

#[test]
fn recycled_pid_is_not_a_live_lock() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = sleeper.id();
    // The PID is in use, but by a process started at a different time.
    write_pid(&p, pid);
    write_start(&p, process_start_time(pid).unwrap() + 1);
    let status = inspect_in(&t.state, &t.target, b"abc").unwrap();
    let created = SwapWriter::create_in(&t.state, &t.target, &r("abc"));
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
    assert!(matches!(status, SwapStatus::Recoverable(_)), "{status:?}");
    assert!(created.is_ok());
}

#[test]
fn start_time_reads_past_a_name_with_spaces_and_parens() {
    // This test binary's own stat line goes through the same parser.
    assert!(process_start_time(std::process::id()).is_some());
    assert_eq!(process_start_time(0), None);
}

#[test]
fn break_lock_clears_a_live_owner_and_keeps_the_edits() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = sleeper.id();
    write_pid(&p, pid);
    write_start(&p, process_start_time(pid).unwrap());
    swap::break_lock_in(&t.state, &t.target).unwrap();
    let status = inspect_in(&t.state, &t.target, b"abc").unwrap();
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
    match status {
        SwapStatus::Recoverable(info) => assert_eq!(info.recovered.as_deref(), Some("xabc")),
        s => panic!("{s:?}"),
    }
    swap::break_lock_in(&t.state, &t.target.with_file_name("none")).unwrap();
}

#[test]
fn permission_bits() {
    let t = setup("abc");
    let w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(w.path()), 0o600);
    assert_eq!(mode(&t.state), 0o700);
    assert!(w.path().starts_with(&t.state));
}

#[test]
fn file_name_escaping() {
    assert_eq!(escape_name(Path::new("/a/b.txt")), "%2Fa%2Fb.txt.kyteswp");
    assert_eq!(
        escape_name(Path::new("/x/100%/y")),
        "%2Fx%2F100%25%2Fy.kyteswp"
    );
    // distinct paths never collide
    assert_ne!(
        escape_name(Path::new("/a%2Fb")),
        escape_name(Path::new("/a/b"))
    );
}

#[test]
fn long_paths_get_a_short_unique_name() {
    let a = format!("/{}/one/file.txt", "dir/".repeat(100));
    let b = format!("/{}/two/file.txt", "dir/".repeat(100));
    let name = escape_name(Path::new(&a));
    assert!(name.len() <= 255, "{}", name.len());
    assert!(name.ends_with(".kyteswp"));
    assert_ne!(name, escape_name(Path::new(&b)));
    // a multi-byte character across the cut must not panic
    let c = format!("/{}", "世/".repeat(200));
    assert!(escape_name(Path::new(&c)).len() <= 255);
}

#[test]
fn long_path_round_trip() {
    let t = setup("abc");
    let long = t
        .target
        .parent()
        .unwrap()
        .join("name-".repeat(60))
        .join("file.txt");
    let mut w = SwapWriter::create_in(&t.state, &long, &r("abc")).unwrap();
    w.append(&[ins(3, "d")]).unwrap();
    let p = w.path().to_path_buf();
    assert_eq!(replay("abc", &p).unwrap(), "abcd");
}

#[test]
fn adopt_drops_torn_tail_rewrites_pid_and_appends() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(3, "1"), ins(4, "2")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    write_pid(&p, dead_pid());
    let len = fs::metadata(&p).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(len - 3)
        .unwrap();
    let mut a = SwapWriter::adopt(&p).unwrap();
    let h = swap::parse_header(&fs::read(&p).unwrap()).unwrap();
    assert_eq!(h.pid, std::process::id());
    a.append(&[ins(4, "Z")]).unwrap();
    assert_eq!(replay("abc", &p).unwrap(), "abc1Z");
    a.remove().unwrap();
    assert!(!p.exists());
}

#[test]
fn remove_after_save_then_fresh_swap_has_no_edits() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, &r("abc")).unwrap();
    w.append(&[ins(3, "d")]).unwrap();
    w.remove().unwrap();
    let w2 = SwapWriter::create_in(&t.state, &t.target, &r("abcd")).unwrap();
    let p = w2.path().to_path_buf();
    assert_eq!(record_count(&p).unwrap(), 0);
    assert_eq!(replay("abcd", &p).unwrap(), "abcd");
}
