//! Swap log tests (task T9): format round trip, torn tail, base CRC mismatch,
//! live versus dead PID, permission bits, file-name escaping, adopt.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use kyte::buffer::Edit;
use kyte::swap::{
    self, encode_record, escape_name, inspect_in, pid_alive, record_count, replay, swap_path_in,
    SwapStatus, SwapWriter, PID_OFFSET,
};

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
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"hello\n").unwrap();
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
    let mut w = SwapWriter::create_in(&t.state, &t.target, "héllo 世界".as_bytes()).unwrap();
    w.append(&[ins(8, "🙂"), del(1, "é")]).unwrap();
    let p = swap_path_in(&t.state, &t.target);
    assert_eq!(replay("héllo 世界", &p).unwrap(), "hllo 世界🙂");
}

#[test]
fn torn_tail_truncated_by_3_bytes() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
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
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
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
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
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
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
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
fn base_crc_mismatch_is_stale() {
    let t = setup("original");
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"original").unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    write_pid(&p, dead_pid());
    assert!(replay("changed!", &p).is_err());
    assert_eq!(
        inspect_in(&t.state, &t.target, b"changed!").unwrap(),
        SwapStatus::Stale
    );
}

#[test]
fn no_swap_and_empty_swap_are_none() {
    let t = setup("abc");
    assert_eq!(
        inspect_in(&t.state, &t.target, b"abc").unwrap(),
        SwapStatus::None
    );
    let w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
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
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
    w.append(&[ins(0, "x")]).unwrap();
    drop(w);
    let p = swap_path_in(&t.state, &t.target);
    let mut sleeper = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = sleeper.id();
    write_pid(&p, pid);
    assert!(pid_alive(pid));
    assert_eq!(
        inspect_in(&t.state, &t.target, b"abc").unwrap(),
        SwapStatus::LiveLock { pid }
    );
    let before = fs::read(&p).unwrap();
    let err = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap_err();
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
fn permission_bits() {
    let t = setup("abc");
    let w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
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
fn adopt_drops_torn_tail_rewrites_pid_and_appends() {
    let t = setup("abc");
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
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
    let mut w = SwapWriter::create_in(&t.state, &t.target, b"abc").unwrap();
    w.append(&[ins(3, "d")]).unwrap();
    w.remove().unwrap();
    let w2 = SwapWriter::create_in(&t.state, &t.target, b"abcd").unwrap();
    let p = w2.path().to_path_buf();
    assert_eq!(record_count(&p).unwrap(), 0);
    assert_eq!(replay("abcd", &p).unwrap(), "abcd");
}
