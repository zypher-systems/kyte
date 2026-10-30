//! File history: an append-only log of every edit to a file, kept across
//! sessions so any past version can be seen and restored. The swap protects
//! the current session; this keeps the rest. The log is one file per edited
//! file, 0600 in a 0700 directory, and nothing is written until the first edit.
//!
//! Each record is framed like a swap record (length, CRC, payload). A payload
//! is a kind byte, the time in milliseconds, then the fields for that kind.
//! A snapshot is a start record followed by data records.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ropey::Rope;

use crate::buffer::Edit;
use crate::diff::diff_lines;
use crate::fileio::{self, DiskState};
use crate::review;
use crate::swap::{frame, name_for, record_at};

const MAGIC: &[u8; 8] = b"KYTEHST1";
const SUFFIX: &str = ".kytehist";

const KIND_SNAP_START: u8 = 1;
const KIND_SNAP_DATA: u8 = 2;
const KIND_EDIT: u8 = 3;
const KIND_MARK: u8 = 4;

const MARK_SAVED: u8 = 1;
const MARK_CLOSED: u8 = 2;

/// Snapshot text is split into records of about this many bytes.
const SNAP_CHUNK: usize = 1 << 20;
/// A rope is kept for every this many versions, so any one is a short replay away.
const CHECKPOINT_EVERY: usize = 256;
/// A log over this size is compacted, down to `COMPACT_TO`.
pub const LOG_LIMIT: u64 = 100_000_000;
/// Compacting stops short of the limit, so it doesn't run again straight away.
const COMPACT_TO: u64 = 75_000_000;
/// How long compacting may look for savings. What it hasn't reached by then
/// stays as it was, so the worst case is a short pause, not a long one.
const COMPACT_BUDGET: Duration = Duration::from_secs(3);
/// Edits further apart than this count as separate bursts of work.
const BURST_GAP_MS: u64 = 3_000;

/// Why the whole text was stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotKind {
    /// What the file held when the log started or caught up with it.
    Opened,
    /// A version from outside Kyte that a save was about to replace.
    Outside,
    /// The buffer after recovering a crashed session.
    Recovered,
    /// What a save wrote, when the log didn't already end there.
    Saved,
    /// The buffer again, after an outside version was stored.
    Current,
}

impl SnapshotKind {
    fn byte(self) -> u8 {
        match self {
            SnapshotKind::Opened => 1,
            SnapshotKind::Outside => 2,
            SnapshotKind::Recovered => 3,
            SnapshotKind::Saved => 4,
            SnapshotKind::Current => 5,
        }
    }

    fn from_byte(b: u8) -> SnapshotKind {
        match b {
            2 => SnapshotKind::Outside,
            3 => SnapshotKind::Recovered,
            4 => SnapshotKind::Saved,
            5 => SnapshotKind::Current,
            _ => SnapshotKind::Opened,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            SnapshotKind::Opened => "as opened",
            SnapshotKind::Outside => "changed outside Kyte",
            SnapshotKind::Recovered => "recovered",
            SnapshotKind::Saved => "saved",
            SnapshotKind::Current => "your version",
        }
    }
}

/// Where `target`'s log lives inside `dir`.
pub fn log_path_in(dir: &Path, target: &Path) -> PathBuf {
    dir.join(name_for(target, SUFFIX))
}

pub fn log_path(target: &Path) -> PathBuf {
    log_path_in(&fileio::history_dir(), target)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn payload_head(kind: u8, time: u64) -> Vec<u8> {
    let mut p = Vec::with_capacity(64);
    p.push(kind);
    p.extend_from_slice(&time.to_le_bytes());
    p
}

fn snapshot_bytes(text: &Rope, kind: SnapshotKind, time: u64) -> Vec<u8> {
    let state = DiskState::of_rope(text);
    let mut p = payload_head(KIND_SNAP_START, time);
    p.push(kind.byte());
    p.extend_from_slice(&state.len().to_le_bytes());
    p.extend_from_slice(&state.crc().to_le_bytes());
    let mut out = frame(&p);
    let mut data = payload_head(KIND_SNAP_DATA, time);
    let head = data.len();
    for chunk in text.chunks() {
        data.extend_from_slice(chunk.as_bytes());
        if data.len() > SNAP_CHUNK {
            out.extend_from_slice(&frame(&data));
            data.truncate(head);
        }
    }
    if data.len() > head {
        out.extend_from_slice(&frame(&data));
    }
    out
}

fn edit_record(at: usize, del: usize, ins: &str, time: u64) -> Vec<u8> {
    let mut p = payload_head(KIND_EDIT, time);
    p.extend_from_slice(&(at as u64).to_le_bytes());
    p.extend_from_slice(&(del as u64).to_le_bytes());
    p.extend_from_slice(ins.as_bytes());
    frame(&p)
}

fn edit_bytes(e: &Edit, time: u64) -> Vec<u8> {
    edit_record(e.at, e.removed.chars().count(), &e.inserted, time)
}

fn mark_bytes(mark: u8, state: DiskState, time: u64) -> Vec<u8> {
    let mut p = payload_head(KIND_MARK, time);
    p.push(mark);
    p.extend_from_slice(&state.len().to_le_bytes());
    p.extend_from_slice(&state.crc().to_le_bytes());
    frame(&p)
}

/// One thing the log says happened.
enum Record {
    Snapshot {
        kind: SnapshotKind,
        time: u64,
        text: String,
    },
    Edit {
        time: u64,
        at: usize,
        del: usize,
        ins: String,
    },
    Saved,
    Closed,
}

/// Calls `f` for each whole record and returns the offset after the last one.
/// A snapshot that was cut short, or anything damaged, ends the walk.
fn walk(bytes: &[u8], mut f: impl FnMut(Record, Option<DiskState>)) -> usize {
    let mut pos = MAGIC.len();
    let mut valid = pos;
    while let Some((payload, next)) = record_at(bytes, pos) {
        let Some(time) = u64_at(payload, 1) else {
            break;
        };
        match payload[0] {
            KIND_SNAP_START => {
                let kind = SnapshotKind::from_byte(payload.get(9).copied().unwrap_or(0));
                let (Some(len), Some(crc)) = (u64_at(payload, 10), u32_at(payload, 18)) else {
                    break;
                };
                let want = DiskState::new(len, crc);
                let mut data = Vec::with_capacity(len as usize);
                let mut at = next;
                while (data.len() as u64) < len {
                    match record_at(bytes, at) {
                        Some((p, n)) if p.first() == Some(&KIND_SNAP_DATA) && p.len() > 9 => {
                            data.extend_from_slice(&p[9..]);
                            at = n;
                        }
                        _ => break,
                    }
                }
                if DiskState::of_bytes(&data) != want {
                    return valid;
                }
                let Ok(text) = String::from_utf8(data) else {
                    return valid;
                };
                f(Record::Snapshot { kind, time, text }, Some(want));
                pos = at;
            }
            KIND_EDIT => {
                let (Some(at), Some(del)) = (u64_at(payload, 9), u64_at(payload, 17)) else {
                    break;
                };
                let Some(Ok(ins)) = payload.get(25..).map(std::str::from_utf8) else {
                    break;
                };
                let (at, del, ins) = (at as usize, del as usize, ins.to_string());
                f(Record::Edit { time, at, del, ins }, None);
                pos = next;
            }
            KIND_MARK => {
                let (Some(len), Some(crc)) = (u64_at(payload, 10), u32_at(payload, 18)) else {
                    break;
                };
                let state = DiskState::new(len, crc);
                let rec = match payload.get(9) {
                    Some(&MARK_SAVED) => Record::Saved,
                    _ => Record::Closed,
                };
                f(rec, Some(state));
                pos = next;
            }
            _ => {
                // A kind from a later version: skip it.
                pos = next;
            }
        }
        valid = pos;
    }
    valid
}

/// The writer for one file's log. It touches the disk only once there's
/// something to keep.
#[derive(Debug)]
pub struct FileHistory {
    path: PathBuf,
    target: PathBuf,
    /// The text before the first edit, in case the log doesn't end there.
    base: Rope,
    file: Option<File>,
    /// The log ends at the buffer's current text.
    synced: bool,
    pending_fsync: bool,
}

impl FileHistory {
    pub fn new(target: &Path, base: &Rope) -> FileHistory {
        Self::new_in(&fileio::history_dir(), target, base)
    }

    pub fn new_in(dir: &Path, target: &Path, base: &Rope) -> FileHistory {
        FileHistory {
            path: log_path_in(dir, target),
            target: target.to_path_buf(),
            base: base.clone(),
            file: None,
            synced: false,
            pending_fsync: false,
        }
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if let Some(f) = self.file.as_mut() {
            f.write_all(bytes)?;
            self.pending_fsync = true;
        }
        Ok(())
    }

    /// Opens the log, drops a torn tail, and stores the base unless the log
    /// already ends there.
    fn ensure_open(&mut self, kind: SnapshotKind) -> io::Result<()> {
        if self.file.is_some() {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let mut last = None;
        let valid = if bytes.starts_with(MAGIC) {
            walk(&bytes, |_, state| last = state)
        } else {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(MAGIC)?;
            MAGIC.len()
        };
        file.set_len(valid as u64)?;
        self.file = Some(file);
        if last == Some(DiskState::of_rope(&self.base)) {
            self.synced = true;
        } else {
            let base = self.base.clone();
            self.write(&snapshot_bytes(&base, kind, now_ms()))?;
            self.synced = true;
        }
        Ok(())
    }

    /// Adds edits to the log.
    pub fn append(&mut self, edits: &[Edit]) -> io::Result<()> {
        self.append_at(edits, now_ms())
    }

    /// [`append`](Self::append) with the time given (tests).
    pub fn append_at(&mut self, edits: &[Edit], time: u64) -> io::Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        self.ensure_open(SnapshotKind::Opened)?;
        let mut buf = Vec::new();
        for e in edits {
            buf.extend_from_slice(&edit_bytes(e, time));
        }
        self.write(&buf)
    }

    /// Stores a whole version that the edits don't lead to.
    pub fn snapshot(&mut self, text: &Rope, kind: SnapshotKind) -> io::Result<()> {
        self.ensure_open(SnapshotKind::Opened)?;
        self.write(&snapshot_bytes(text, kind, now_ms()))?;
        self.synced = false;
        Ok(())
    }

    /// Stores an outside version while editing carries on from `current`.
    pub fn note_outside(&mut self, outside: &Rope, current: &Rope) -> io::Result<()> {
        self.ensure_open(SnapshotKind::Opened)?;
        let time = now_ms();
        self.write(&snapshot_bytes(outside, SnapshotKind::Outside, time))?;
        self.write(&snapshot_bytes(current, SnapshotKind::Current, time))?;
        self.synced = true;
        Ok(())
    }

    /// Notes that `text` was saved.
    pub fn mark_saved(&mut self, text: &Rope) -> io::Result<()> {
        self.ensure_open(SnapshotKind::Saved)?;
        if !self.synced {
            self.write(&snapshot_bytes(text, SnapshotKind::Saved, now_ms()))?;
            self.synced = true;
        }
        self.base = text.clone();
        self.write(&mark_bytes(MARK_SAVED, DiskState::of_rope(text), now_ms()))
    }

    /// Notes where the log ends, so the next session can tell, and trims a
    /// log that's over the limit. That happens here because a pause at the end
    /// of a session goes unnoticed, and in the middle of typing it wouldn't. A
    /// session that edited nothing has nothing to do.
    pub fn close(&mut self, text: &Rope) -> io::Result<()> {
        if self.file.is_none() {
            return Ok(());
        }
        let state = DiskState::of_rope(text);
        self.write(&mark_bytes(MARK_CLOSED, state, now_ms()))?;
        self.fsync_if_pending()?;
        self.file = None;
        compact(&self.path, LOG_LIMIT, COMPACT_TO)
    }

    pub fn fsync_if_pending(&mut self) -> io::Result<()> {
        if self.pending_fsync {
            if let Some(f) = self.file.as_mut() {
                f.sync_data()?;
            }
            self.pending_fsync = false;
        }
        Ok(())
    }

    /// Deletes the log. The history starts again from `text`.
    pub fn delete(&mut self, text: &Rope) -> io::Result<()> {
        self.file = None;
        self.synced = false;
        self.pending_fsync = false;
        self.base = text.clone();
        let _ = fs::remove_file(tried_path(&self.path));
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The edits that turn `old` into `new`, one per changed region and last
/// first, so each position holds when it's applied. They're found by lines
/// because changes can be far apart: one edit spanning them all would hold
/// most of the file.
fn edits_between(old: &str, new: &str) -> Vec<(usize, usize, String)> {
    let (a, b) = (review::lines(old), review::lines(new));
    let mut starts = Vec::with_capacity(a.len() + 1);
    let mut at = 0;
    starts.push(0);
    for line in &a {
        at += line.chars().count();
        starts.push(at);
    }
    diff_lines(&a, &b)
        .iter()
        .rev()
        .map(|h| {
            let del = starts[h.a.end] - starts[h.a.start];
            (starts[h.a.start], del, b[h.b.clone()].concat())
        })
        .collect()
}

/// Where a log's size is noted when compacting couldn't get it under the limit.
fn tried_path(path: &Path) -> PathBuf {
    path.with_extension("compact-tried")
}

/// Whether the log has grown enough since a failed try to be worth another.
fn worth_trying(path: &Path, len: u64) -> bool {
    let tried = fs::read_to_string(tried_path(path))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    tried.is_none_or(|t| len > t + t / 4)
}

/// Shrinks a log that's over `limit` down towards `target`. The oldest edits
/// go first: the edits between two kept versions become one per changed
/// region, unless the originals are smaller. A saved version or an outside
/// version is always kept, so the log may stay over the limit if those alone
/// are that large. The new log replaces the old one in one rename, so a crash
/// leaves one or the other, whole, and a result that isn't smaller is dropped.
pub fn compact(path: &Path, limit: u64, target: u64) -> io::Result<()> {
    let len = match fs::metadata(path) {
        Ok(m) if m.len() > limit && worth_trying(path, m.len()) => m.len(),
        _ => return Ok(()),
    };
    let t = Timeline::load(path)?;
    if t.len() < 2 {
        return Ok(());
    }
    let deadline = Instant::now() + COMPACT_BUDGET;
    // Newest first, find where the budget for keeping everything runs out.
    let size = |v: &Version| -> usize {
        match &v.change {
            Change::Edit { ins, .. } => ins.len() + 40,
            Change::Snapshot { text, .. } => text.len_bytes() + 64,
        }
    };
    let mut budget = target as usize;
    let mut keep_from = t.len();
    for i in (0..t.len()).rev() {
        let s = size(t.version(i));
        if s > budget {
            break;
        }
        budget -= s;
        keep_from = i;
    }
    let kept_old = |v: &Version| {
        v.saved
            || matches!(
                v.change,
                Change::Snapshot {
                    kind: SnapshotKind::Outside,
                    ..
                }
            )
    };
    let version_bytes = |v: &Version| -> Vec<u8> {
        let mut out = match &v.change {
            Change::Edit { at, del, ins } => edit_record(*at, *del, ins, v.time_ms),
            Change::Snapshot { kind, text } => snapshot_bytes(text, *kind, v.time_ms),
        };
        if v.saved {
            // Only the mark at the very end needs its real state.
            out.extend_from_slice(&mark_bytes(MARK_SAVED, DiskState::new(0, 0), v.time_ms));
        }
        out
    };
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&version_bytes(t.version(0)));
    // The oldest part: only the versions to keep.
    let mut run_start = 1;
    for i in 1..keep_from {
        let v = t.version(i);
        if !kept_old(v) && i + 1 != keep_from {
            continue;
        }
        let run = run_start..i + 1;
        let original: Vec<u8> = run.flat_map(|j| version_bytes(t.version(j))).collect();
        let mut short = Vec::new();
        let in_time = Instant::now() < deadline;
        if in_time {
            let old = t.text_at(run_start - 1).to_string();
            let new = t.text_at(i).to_string();
            for (at, del, ins) in edits_between(&old, &new) {
                short.extend_from_slice(&edit_record(at, del, &ins, v.time_ms));
            }
        }
        if in_time && short.len() < original.len() {
            out.extend_from_slice(&short);
            if v.saved {
                out.extend_from_slice(&mark_bytes(MARK_SAVED, DiskState::new(0, 0), v.time_ms));
            }
        } else {
            out.extend_from_slice(&original);
        }
        run_start = i + 1;
    }
    // The newest part: every edit, as it was.
    for i in keep_from.max(1)..t.len() {
        out.extend_from_slice(&version_bytes(t.version(i)));
    }
    // Ends on a mark, so the next session knows what the log ends at.
    let last = t.len() - 1;
    let end = DiskState::of_rope(&t.text_at(last));
    out.extend_from_slice(&mark_bytes(MARK_CLOSED, end, t.version(last).time_ms));
    if (out.len() as u64) < len {
        let tmp = path.with_extension("compacting");
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&out)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
    }
    let now = len.min(out.len() as u64);
    if now > limit {
        fs::write(tried_path(path), now.to_string())?;
    } else {
        let _ = fs::remove_file(tried_path(path));
    }
    Ok(())
}

/// What made one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Edit { at: usize, del: usize, ins: String },
    Snapshot { kind: SnapshotKind, text: Rope },
}

/// One point in a file's past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub time_ms: u64,
    pub change: Change,
    /// This version was saved to the file.
    pub saved: bool,
}

/// Every version of a file, oldest first.
#[derive(Debug, PartialEq, Eq)]
pub struct Timeline {
    versions: Vec<Version>,
    checkpoints: Vec<Rope>,
}

/// The shared timeline a browsing session holds.
pub type SharedTimeline = Arc<Timeline>;

fn apply(rope: &mut Rope, change: &Change) -> bool {
    match change {
        Change::Snapshot { text, .. } => *rope = text.clone(),
        Change::Edit { at, del, ins } => {
            let total = rope.len_chars();
            if *at > total || *del > total - *at {
                return false;
            }
            if *del > 0 {
                rope.remove(*at..*at + *del);
            }
            if !ins.is_empty() {
                rope.insert(*at, ins);
            }
        }
    }
    true
}

impl Timeline {
    /// Reads a log. A missing log is an empty timeline.
    pub fn load(path: &Path) -> io::Result<Timeline> {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        let mut versions: Vec<Version> = Vec::new();
        if bytes.starts_with(MAGIC) {
            walk(&bytes, |rec, _| match rec {
                Record::Snapshot { kind, time, text } => versions.push(Version {
                    time_ms: time,
                    change: Change::Snapshot {
                        kind,
                        text: Rope::from_str(&text),
                    },
                    saved: kind == SnapshotKind::Saved,
                }),
                Record::Edit { time, at, del, ins } => versions.push(Version {
                    time_ms: time,
                    change: Change::Edit { at, del, ins },
                    saved: false,
                }),
                Record::Saved => {
                    if let Some(v) = versions.last_mut() {
                        v.saved = true;
                    }
                }
                Record::Closed => {}
            });
        }
        Ok(Self::build(versions))
    }

    fn build(mut versions: Vec<Version>) -> Timeline {
        let mut rope = Rope::new();
        let mut checkpoints = Vec::new();
        let mut good = 0;
        for (i, v) in versions.iter().enumerate() {
            if !apply(&mut rope, &v.change) {
                break;
            }
            if i % CHECKPOINT_EVERY == 0 {
                checkpoints.push(rope.clone());
            }
            good = i + 1;
        }
        versions.truncate(good);
        Timeline {
            versions,
            checkpoints,
        }
    }

    pub fn len(&self) -> usize {
        self.versions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    pub fn version(&self, i: usize) -> &Version {
        &self.versions[i]
    }

    /// The text of version `i`.
    pub fn text_at(&self, i: usize) -> Rope {
        let cp = i / CHECKPOINT_EVERY;
        let mut rope = self.checkpoints[cp].clone();
        for v in &self.versions[cp * CHECKPOINT_EVERY + 1..=i] {
            apply(&mut rope, &v.change);
        }
        rope
    }

    /// Whether version `i` is the last of a burst of work.
    fn ends_burst(&self, i: usize) -> bool {
        let Some(next) = self.versions.get(i + 1) else {
            return true;
        };
        let v = &self.versions[i];
        v.saved
            || !matches!(next.change, Change::Edit { .. })
            || next.time_ms.saturating_sub(v.time_ms) >= BURST_GAP_MS
    }

    /// The end of the burst before the one holding `i`.
    pub fn prev_burst(&self, i: usize) -> usize {
        (0..i).rev().find(|j| self.ends_burst(*j)).unwrap_or(0)
    }

    /// The end of the burst holding or after `i`, past `i` itself.
    pub fn next_burst(&self, i: usize) -> usize {
        (i + 1..self.len())
            .find(|j| self.ends_burst(*j))
            .unwrap_or(i)
    }

    /// The saved version before `i`.
    pub fn prev_saved(&self, i: usize) -> Option<usize> {
        (0..i).rev().find(|j| self.versions[*j].saved)
    }

    /// The saved version after `i`.
    pub fn next_saved(&self, i: usize) -> Option<usize> {
        (i + 1..self.len()).find(|j| self.versions[*j].saved)
    }

    /// A few words on what version `i` was.
    pub fn describe(&self, i: usize) -> String {
        let v = &self.versions[i];
        let what = match &v.change {
            Change::Snapshot { kind, .. } => kind.describe().to_string(),
            Change::Edit { del, ins, .. } => {
                let added = ins.chars().count();
                match (*del, added) {
                    (0, n) => format!("added {n}"),
                    (n, 0) => format!("removed {n}"),
                    (d, a) => format!("removed {d}, added {a}"),
                }
            }
        };
        if v.saved
            && !matches!(
                v.change,
                Change::Snapshot {
                    kind: SnapshotKind::Saved,
                    ..
                }
            )
        {
            format!("{what}, saved")
        } else {
            what
        }
    }
}

/// How long ago `then` was, in a few words.
pub fn ago(then_ms: u64, now_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => plural(secs / 60, "minute"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        _ => plural(secs / 86_400, "day"),
    }
}

/// [`ago`] measured from now.
pub fn ago_from_now(then_ms: u64) -> String {
    ago(then_ms, now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_reads_naturally() {
        let now = 1_000_000_000;
        assert_eq!(ago(now - 5_000, now), "just now");
        assert_eq!(ago(now - 60_000, now), "1 minute ago");
        assert_eq!(ago(now - 180_000, now), "3 minutes ago");
        assert_eq!(ago(now - 7_200_000, now), "2 hours ago");
        assert_eq!(ago(now - 172_800_000, now), "2 days ago");
        assert_eq!(ago(now + 5, now), "just now");
    }
}
