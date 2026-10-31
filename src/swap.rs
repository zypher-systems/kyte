//! Crash-safety swap log (section 6): a header naming the target file and its
//! base content, then append-only CRC-checked records. The first edit also
//! writes a snapshot of the base, so sessions that only read a file copy
//! nothing and edits can be recovered even if the file later changes. Every
//! edit is written (not fsynced) as soon as it happens, so it survives
//! `kill -9`; `App::tick` fsyncs at most once per second.

use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ropey::Rope;

use crate::buffer::Edit;
use crate::fileio;

pub const MAGIC: &[u8; 8] = b"KYTESWP1";
pub const VERSION: u32 = 2;
/// Byte offset of the writer PID in the header.
pub const PID_OFFSET: u64 = 12;
/// Byte offset of the writer's process start time in the header.
pub const START_OFFSET: u64 = 16;
/// Record kind byte for an edit (unknown kinds are skipped by replay).
pub const KIND_EDIT: u8 = 0;
/// Record kind byte for a piece of the base snapshot.
pub const KIND_BASE: u8 = 1;
const EDIT_FIXED: usize = 1 + 8 + 8 + 4;
const HEADER_FIXED: usize = 40;
/// The base snapshot is split into records of about this many bytes.
const BASE_CHUNK: usize = 1 << 20;
const SUFFIX: &str = ".kyteswp";
/// Swaps of buffers with no file name start with this. No escaped path can,
/// because every path starts with `%2F`.
const UNTITLED_PREFIX: &str = "untitled-";
/// File-name limit on Linux file systems, in bytes.
const NAME_MAX: usize = 255;

/// Error payload when a live process owns the swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveLockError {
    pub pid: u32,
}

impl fmt::Display for LiveLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the swap file is owned by running process {}", self.pid)
    }
}

impl std::error::Error for LiveLockError {}

/// The PID inside an `io::Error` produced for a live lock.
pub fn live_lock_pid(e: &io::Error) -> Option<u32> {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<LiveLockError>())
        .map(|l| l.pid)
}

/// What the Recover modal needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverInfo {
    /// Canonical target path (also the swap key).
    pub target: PathBuf,
    pub swap_path: PathBuf,
    /// Swap modification time, `YYYY-MM-DD HH:MM:SS UTC`.
    pub modified: String,
    /// Replayed content; `None` when the swap is stale.
    pub recovered: Option<String>,
    /// The file on disk no longer matches what these edits were made against.
    pub file_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapStatus {
    None,
    Recoverable(RecoverInfo),
    Stale,
    LiveLock { pid: u32 },
}

/// Parsed header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub pid: u32,
    /// Start time of the writer process, so a recycled PID is not mistaken for it.
    pub start: u64,
    pub base_len: u64,
    pub base_crc: u32,
    pub path: String,
    /// Length of the header in bytes (records start here).
    pub len: usize,
}

/// Swap file name for `target`.
pub fn escape_name(target: &Path) -> String {
    name_for(target, SUFFIX)
}

/// A state file name for `target`: the absolute path with `%` as `%25` and `/`
/// as `%2F`, plus `suffix`. A name over the file-name limit keeps the end of
/// the path and adds a hash of the whole path.
pub fn name_for(target: &Path, suffix: &str) -> String {
    let s = target.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '/' => out.push_str("%2F"),
            c => out.push(c),
        }
    }
    if out.len() + suffix.len() > NAME_MAX {
        let tag = format!("~{:08x}{:08x}", crc(s.as_bytes()), s.len());
        let mut cut = out.len() - (NAME_MAX - suffix.len() - tag.len());
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        out = format!("{}{tag}", &out[cut..]);
    }
    out.push_str(suffix);
    out
}

/// Swap path for `target` inside `dir`.
pub fn swap_path_in(dir: &Path, target: &Path) -> PathBuf {
    dir.join(escape_name(target))
}

/// Swap path for `target` in the configured state directory.
pub fn swap_path_for(target: &Path) -> PathBuf {
    swap_path_in(&fileio::state_dir(), target)
}

/// Whether `pid` names a running process (Linux: `/proc/<pid>` exists).
pub fn pid_alive(pid: u32) -> bool {
    pid != 0 && Path::new("/proc").join(pid.to_string()).exists()
}

/// Start time of `pid` in clock ticks since boot (field 22 of `/proc/<pid>/stat`).
pub fn process_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name (field 2) may hold spaces and parentheses.
    let after_name = stat.rsplit_once(')')?.1;
    after_name.split_whitespace().nth(19)?.parse().ok()
}

fn own_start_time() -> u64 {
    process_start_time(std::process::id()).unwrap_or(0)
}

/// Whether the process that wrote a swap is still running. A PID now used by
/// a different process has a different start time.
fn owner_alive(pid: u32, start: u64) -> bool {
    pid != std::process::id() && pid_alive(pid) && process_start_time(pid) == Some(start)
}

fn crc(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

fn rope_crc(rope: &Rope) -> u32 {
    let mut h = crc32fast::Hasher::new();
    for chunk in rope.chunks() {
        h.update(chunk.as_bytes());
    }
    h.finalize()
}

fn header_bytes(pid: u32, start: u64, target: &Path, base: &Rope) -> Vec<u8> {
    let path = target.to_string_lossy();
    let mut h = Vec::with_capacity(HEADER_FIXED + path.len());
    h.extend_from_slice(MAGIC);
    h.extend_from_slice(&VERSION.to_le_bytes());
    h.extend_from_slice(&pid.to_le_bytes());
    h.extend_from_slice(&start.to_le_bytes());
    h.extend_from_slice(&(base.len_bytes() as u64).to_le_bytes());
    h.extend_from_slice(&rope_crc(base).to_le_bytes());
    h.extend_from_slice(&(path.len() as u32).to_le_bytes());
    h.extend_from_slice(path.as_bytes());
    h
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Parses the header at the start of `b`.
pub fn parse_header(b: &[u8]) -> io::Result<Header> {
    if b.len() < HEADER_FIXED || &b[..8] != MAGIC {
        return Err(invalid("not a Kyte swap file"));
    }
    let version = u32_at(b, 8).unwrap();
    if version != VERSION {
        return Err(invalid("unsupported swap version"));
    }
    let pid = u32_at(b, PID_OFFSET as usize).unwrap();
    let start = u64_at(b, START_OFFSET as usize).unwrap();
    let base_len = u64_at(b, 24).unwrap();
    let base_crc = u32_at(b, 32).unwrap();
    let path_len = u32_at(b, 36).unwrap() as usize;
    let path = b
        .get(HEADER_FIXED..HEADER_FIXED + path_len)
        .ok_or_else(|| invalid("truncated swap header"))?;
    let path = String::from_utf8(path.to_vec()).map_err(|_| invalid("bad path in swap header"))?;
    Ok(Header {
        pid,
        start,
        base_len,
        base_crc,
        path,
        len: HEADER_FIXED + path_len,
    })
}

/// Encodes one edit record (length, CRC, payload).
pub fn encode_record(e: &Edit) -> Vec<u8> {
    let ins = e.inserted.as_bytes();
    let mut payload = Vec::with_capacity(EDIT_FIXED + ins.len());
    payload.push(KIND_EDIT);
    payload.extend_from_slice(&(e.at as u64).to_le_bytes());
    payload.extend_from_slice(&(e.removed.chars().count() as u64).to_le_bytes());
    payload.extend_from_slice(&(ins.len() as u32).to_le_bytes());
    payload.extend_from_slice(ins);
    frame(&payload)
}

/// Wraps a payload in its length and CRC.
pub(crate) fn frame(payload: &[u8]) -> Vec<u8> {
    let mut rec = Vec::with_capacity(8 + payload.len());
    rec.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    rec.extend_from_slice(&crc(payload).to_le_bytes());
    rec.extend_from_slice(payload);
    rec
}

/// The base snapshot as records, each holding about [`BASE_CHUNK`] bytes.
fn encode_base(base: &Rope) -> Vec<u8> {
    let mut out = Vec::with_capacity(base.len_bytes() + 64);
    let mut payload = vec![KIND_BASE];
    for chunk in base.chunks() {
        payload.extend_from_slice(chunk.as_bytes());
        if payload.len() > BASE_CHUNK {
            out.extend_from_slice(&frame(&payload));
            payload.truncate(1);
        }
    }
    if payload.len() > 1 {
        out.extend_from_slice(&frame(&payload));
    }
    out
}

/// One framed record starting at `pos`: its payload and the offset after it.
pub(crate) fn record_at(bytes: &[u8], pos: usize) -> Option<(&[u8], usize)> {
    let len = u32_at(bytes, pos)? as usize;
    let sum = u32_at(bytes, pos + 4)?;
    let payload = bytes.get(pos + 8..pos + 8 + len)?;
    (crc(payload) == sum).then_some((payload, pos + 8 + len))
}

/// The base snapshot at the start of the record area, and where it ends.
fn read_snapshot(bytes: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut text = Vec::new();
    let mut pos = start;
    while let Some((payload, next)) = record_at(bytes, pos) {
        if payload.first() != Some(&KIND_BASE) {
            break;
        }
        text.extend_from_slice(&payload[1..]);
        pos = next;
    }
    (text, pos)
}

/// Text to replay on, where its edits start, and whether the file has moved on.
struct Base {
    text: String,
    edits_at: usize,
    file_changed: bool,
}

/// Picks the base for replay: the file when it still matches the header,
/// otherwise the snapshot. `None` when neither matches.
fn resolve_base(bytes: &[u8], h: &Header, current: &[u8]) -> Option<Base> {
    let matches = |b: &[u8]| h.base_len == b.len() as u64 && h.base_crc == crc(b);
    let (snapshot, edits_at) = read_snapshot(bytes, h.len);
    if matches(current) {
        let text = String::from_utf8(current.to_vec()).ok()?;
        return Some(Base {
            text,
            edits_at,
            file_changed: false,
        });
    }
    if !matches(&snapshot) {
        return None;
    }
    let text = String::from_utf8(snapshot).ok()?;
    Some(Base {
        text,
        edits_at,
        file_changed: true,
    })
}

/// Result of replaying the record area.
struct Replayed {
    rope: Rope,
    records: usize,
}

/// Applies records in order, stopping at the first short, CRC-bad or
/// out-of-range record (a torn tail is expected after a kill).
fn replay_records(bytes: &[u8], start: usize, base: &str) -> Replayed {
    let mut rope = Rope::from_str(base);
    let mut pos = start;
    let mut records = 0;
    while let Some((payload, next)) = record_at(bytes, pos) {
        if payload.is_empty() {
            break;
        }
        if payload[0] == KIND_EDIT {
            if payload.len() < EDIT_FIXED {
                break;
            }
            let at = u64_at(payload, 1).unwrap() as usize;
            let del = u64_at(payload, 9).unwrap() as usize;
            let ins_len = u32_at(payload, 17).unwrap() as usize;
            if payload.len() != EDIT_FIXED + ins_len {
                break;
            }
            let Ok(ins) = std::str::from_utf8(&payload[EDIT_FIXED..]) else {
                break;
            };
            let total = rope.len_chars();
            if at > total || del > total - at {
                break;
            }
            if del > 0 {
                rope.remove(at..at + del);
            }
            if !ins.is_empty() {
                rope.insert(at, ins);
            }
        }
        // Unknown kinds are length-delimited and skipped (section 13).
        pos = next;
        records += 1;
    }
    Replayed { rope, records }
}

/// Replays the swap at `swap_path` on top of `current` (the real file content)
/// when it matches the header, otherwise on top of the swap's snapshot.
pub fn replay(current: &str, swap_path: &Path) -> io::Result<String> {
    let bytes = fs::read(swap_path)?;
    let h = parse_header(&bytes)?;
    let base = resolve_base(&bytes, &h, current.as_bytes()).ok_or_else(|| {
        invalid("the file changed since the swap was written and the swap has no usable copy")
    })?;
    Ok(replay_records(&bytes, base.edits_at, &base.text)
        .rope
        .to_string())
}

/// (byte offset after the last record with valid framing and CRC, record count).
fn framing_end(bytes: &[u8], start: usize) -> (usize, usize) {
    let mut pos = start;
    let mut n = 0;
    while let Some((_, next)) = record_at(bytes, pos) {
        n += 1;
        pos = next;
    }
    (pos, n)
}

/// Number of records after the base snapshot (tests and diagnostics).
pub fn record_count(swap_path: &Path) -> io::Result<usize> {
    let bytes = fs::read(swap_path)?;
    let h = parse_header(&bytes)?;
    let (_, edits_at) = read_snapshot(&bytes, h.len);
    Ok(framing_end(&bytes, edits_at).1)
}

/// `YYYY-MM-DD HH:MM:SS UTC` (no time-zone database needed).
pub fn format_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn modified_of(p: &Path) -> String {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .map(format_utc)
        .unwrap_or_else(|_| "unknown time".to_string())
}

/// Describes the swap for `target` in `dir` for the Stale modal.
pub fn stale_info_in(dir: &Path, target: &Path) -> RecoverInfo {
    let swap_path = swap_path_in(dir, target);
    RecoverInfo {
        target: target.to_path_buf(),
        modified: modified_of(&swap_path),
        swap_path,
        recovered: None,
        file_changed: false,
    }
}

/// [`stale_info_in`] in the configured state directory.
pub fn stale_info(target: &Path) -> RecoverInfo {
    stale_info_in(&fileio::state_dir(), target)
}

/// Inspects the swap for `target` (canonical path) given the file's current bytes.
pub fn inspect(target: &Path, current_file_bytes: &[u8]) -> io::Result<SwapStatus> {
    inspect_in(&fileio::state_dir(), target, current_file_bytes)
}

/// [`inspect`] with an explicit swap directory.
pub fn inspect_in(dir: &Path, target: &Path, current: &[u8]) -> io::Result<SwapStatus> {
    let swap_path = swap_path_in(dir, target);
    let bytes = match fs::read(&swap_path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(SwapStatus::None),
        Err(e) => return Err(e),
    };
    let Ok(h) = parse_header(&bytes) else {
        return Ok(SwapStatus::Stale);
    };
    if owner_alive(h.pid, h.start) {
        return Ok(SwapStatus::LiveLock { pid: h.pid });
    }
    if bytes.len() == h.len {
        // A session that never edited anything has nothing to offer.
        return Ok(SwapStatus::None);
    }
    let Some(base) = resolve_base(&bytes, &h, current) else {
        return Ok(SwapStatus::Stale);
    };
    let r = replay_records(&bytes, base.edits_at, &base.text);
    let text = r.rope.to_string();
    if r.records == 0 || text.as_bytes() == current {
        // Nothing unsaved to offer.
        return Ok(SwapStatus::None);
    }
    Ok(SwapStatus::Recoverable(RecoverInfo {
        target: target.to_path_buf(),
        modified: modified_of(&swap_path),
        swap_path,
        recovered: Some(text),
        file_changed: base.file_changed,
    }))
}

/// The newest crashed untitled session in `dir` that holds text. Crashed
/// sessions that hold nothing are deleted.
pub fn find_untitled_in(dir: &Path) -> io::Result<Option<RecoverInfo>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut best: Option<(SystemTime, PathBuf, String)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(UNTITLED_PREFIX) || !name.ends_with(SUFFIX) {
            continue;
        }
        let path = entry.path();
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(h) = parse_header(&bytes) else {
            continue;
        };
        if owner_alive(h.pid, h.start) {
            continue;
        }
        let text = replay_records(&bytes, h.len, "").rope.to_string();
        if text.is_empty() {
            let _ = fs::remove_file(&path);
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, ..)| modified > *t) {
            best = Some((modified, path, text));
        }
    }
    Ok(best.map(|(_, swap_path, text)| RecoverInfo {
        target: PathBuf::new(),
        modified: modified_of(&swap_path),
        swap_path,
        recovered: Some(text),
        file_changed: false,
    }))
}

/// [`find_untitled_in`] in the configured state directory.
pub fn find_untitled() -> io::Result<Option<RecoverInfo>> {
    find_untitled_in(&fileio::state_dir())
}

/// Clears the owner of `target`'s swap in `dir` so it no longer counts as a
/// live lock. The file is otherwise untouched; a missing swap is fine.
pub fn break_lock_in(dir: &Path, target: &Path) -> io::Result<()> {
    let path = swap_path_in(dir, target);
    let mut file = match OpenOptions::new().write(true).open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    file.seek(SeekFrom::Start(PID_OFFSET))?;
    file.write_all(&0u32.to_le_bytes())
}

/// [`break_lock_in`] in the configured state directory.
pub fn break_lock(target: &Path) -> io::Result<()> {
    break_lock_in(&fileio::state_dir(), target)
}

/// The owner in the fixed part of a header, if it is a header of this version.
fn parse_fixed_owner(head: &[u8]) -> Option<(u32, u64)> {
    if &head[..8] != MAGIC || u32_at(head, 8) != Some(VERSION) {
        return None;
    }
    Some((
        u32_at(head, PID_OFFSET as usize)?,
        u64_at(head, START_OFFSET as usize)?,
    ))
}

/// Append-only writer for one swap file.
#[derive(Debug)]
pub struct SwapWriter {
    file: File,
    path: PathBuf,
    pending_fsync: bool,
    /// Written ahead of the first edit, then dropped.
    pending_base: Option<Rope>,
}

impl SwapWriter {
    /// Creates (or replaces) the swap for `target` with `base` as the base
    /// content. Fails with a live-lock error if a running process owns it.
    pub fn create(target: &Path, base: &Rope) -> io::Result<SwapWriter> {
        Self::create_in(&fileio::state_dir(), target, base)
    }

    /// [`create`](Self::create) with an explicit swap directory.
    pub fn create_in(dir: &Path, target: &Path, base: &Rope) -> io::Result<SwapWriter> {
        let path = swap_path_in(dir, target);
        Self::create_at(dir, path, target, base)
    }

    /// A swap for a buffer with no file name, unique to this process.
    pub fn create_untitled() -> io::Result<SwapWriter> {
        Self::create_untitled_in(&fileio::state_dir())
    }

    /// [`create_untitled`](Self::create_untitled) with an explicit swap directory.
    pub fn create_untitled_in(dir: &Path) -> io::Result<SwapWriter> {
        let name = format!(
            "{UNTITLED_PREFIX}{}-{}{SUFFIX}",
            std::process::id(),
            own_start_time()
        );
        Self::create_at(dir, dir.join(name), Path::new(""), &Rope::new())
    }

    fn create_at(dir: &Path, path: PathBuf, target: &Path, base: &Rope) -> io::Result<SwapWriter> {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        if let Ok(mut f) = File::open(&path) {
            let mut head = vec![0u8; HEADER_FIXED];
            if f.read_exact(&mut head).is_ok() {
                if let Some((pid, start)) = parse_fixed_owner(&head) {
                    if owner_alive(pid, start) {
                        return Err(io::Error::other(LiveLockError { pid }));
                    }
                }
            }
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&header_bytes(
            std::process::id(),
            own_start_time(),
            target,
            base,
        ))?;
        file.sync_all()?;
        Ok(SwapWriter {
            file,
            path,
            pending_fsync: false,
            pending_base: Some(base.clone()),
        })
    }

    /// Takes over an existing swap after recovery: drops a torn tail, rewrites
    /// the PID, and keeps appending.
    pub fn adopt(swap_path: &Path) -> io::Result<SwapWriter> {
        let mut file = OpenOptions::new().read(true).write(true).open(swap_path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let h = parse_header(&bytes)?;
        // Content was validated by inspect/replay already; drop the torn tail.
        let (pos, _) = framing_end(&bytes, h.len);
        file.set_len(pos as u64)?;
        file.seek(SeekFrom::Start(PID_OFFSET))?;
        file.write_all(&std::process::id().to_le_bytes())?;
        file.write_all(&own_start_time().to_le_bytes())?;
        file.seek(SeekFrom::End(0))?;
        file.sync_all()?;
        Ok(SwapWriter {
            file,
            path: swap_path.to_path_buf(),
            pending_fsync: false,
            pending_base: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A writer whose every write fails (tests).
    #[cfg(test)]
    pub(crate) fn failing() -> SwapWriter {
        SwapWriter {
            file: OpenOptions::new().write(true).open("/dev/full").unwrap(),
            path: PathBuf::from("/dev/full"),
            pending_fsync: false,
            pending_base: None,
        }
    }

    /// Writes the edits immediately (one `write`), without fsync.
    pub fn append(&mut self, edits: &[Edit]) -> io::Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        let mut buf = match self.pending_base.take() {
            Some(base) => encode_base(&base),
            None => Vec::new(),
        };
        for e in edits {
            buf.extend_from_slice(&encode_record(e));
        }
        self.file.write_all(&buf)?;
        self.pending_fsync = true;
        Ok(())
    }

    /// Makes a buffer with no edits recoverable, for when the file moves on
    /// and the buffer stays behind. A swap holds no copy of its base until the
    /// first edit, and with no edits a crash offers nothing, so this writes the
    /// copy and an edit that changes nothing.
    pub fn keep_base(&mut self) -> io::Result<()> {
        if self.pending_base.is_none() {
            return Ok(());
        }
        self.append(&[Edit {
            at: 0,
            removed: String::new(),
            inserted: String::new(),
        }])
    }

    pub fn fsync_if_pending(&mut self) -> io::Result<()> {
        if self.pending_fsync {
            self.file.sync_data()?;
            self.pending_fsync = false;
        }
        Ok(())
    }

    /// Deletes the swap (after an explicit save or a discard).
    pub fn remove(self) -> io::Result<()> {
        let path = self.path.clone();
        drop(self);
        match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formatting() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_793_329_445);
        assert_eq!(format_utc(t), "2026-10-30 03:04:05 UTC");
        assert_eq!(
            format_utc(SystemTime::UNIX_EPOCH),
            "1970-01-01 00:00:00 UTC"
        );
    }

    #[test]
    fn header_roundtrip() {
        let h = header_bytes(42, 7, Path::new("/a/b.txt"), &Rope::from_str("hello"));
        let p = parse_header(&h).unwrap();
        assert_eq!(p.pid, 42);
        assert_eq!(p.start, 7);
        assert_eq!(p.base_len, 5);
        assert_eq!(p.base_crc, crc32fast::hash(b"hello"));
        assert_eq!(p.path, "/a/b.txt");
        assert_eq!(p.len, h.len());
        assert_eq!(u32_at(&h, PID_OFFSET as usize), Some(42));
    }
}
