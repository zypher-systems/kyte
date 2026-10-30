//! Crash-safety swap log (section 6): a header naming the target file and its
//! base content, then append-only CRC-checked edit records. Every edit is
//! written (not fsynced) as soon as it happens, so it survives `kill -9`;
//! `App::tick` fsyncs at most once per second.

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
pub const VERSION: u32 = 1;
/// Byte offset of the writer PID in the header.
pub const PID_OFFSET: u64 = 12;
/// Record kind byte for an edit (other kinds are reserved and skipped by replay).
pub const KIND_EDIT: u8 = 0;
const EDIT_FIXED: usize = 1 + 8 + 8 + 4;

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
    pub base_len: u64,
    pub base_crc: u32,
    pub path: String,
    /// Length of the header in bytes (records start here).
    pub len: usize,
}

/// Swap file name for `target`: the absolute path with `%` as `%25` and `/` as `%2F`, plus `.kyteswp`.
pub fn escape_name(target: &Path) -> String {
    let s = target.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '/' => out.push_str("%2F"),
            c => out.push(c),
        }
    }
    out.push_str(".kyteswp");
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

fn crc(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

fn header_bytes(pid: u32, target: &Path, base: &[u8]) -> Vec<u8> {
    let path = target.to_string_lossy();
    let mut h = Vec::with_capacity(32 + path.len());
    h.extend_from_slice(MAGIC);
    h.extend_from_slice(&VERSION.to_le_bytes());
    h.extend_from_slice(&pid.to_le_bytes());
    h.extend_from_slice(&(base.len() as u64).to_le_bytes());
    h.extend_from_slice(&crc(base).to_le_bytes());
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
    if b.len() < 32 || &b[..8] != MAGIC {
        return Err(invalid("not a Kyte swap file"));
    }
    let version = u32_at(b, 8).unwrap();
    if version != VERSION {
        return Err(invalid("unsupported swap version"));
    }
    let pid = u32_at(b, 12).unwrap();
    let base_len = u64_at(b, 16).unwrap();
    let base_crc = u32_at(b, 24).unwrap();
    let path_len = u32_at(b, 28).unwrap() as usize;
    let path = b
        .get(32..32 + path_len)
        .ok_or_else(|| invalid("truncated swap header"))?;
    let path = String::from_utf8(path.to_vec()).map_err(|_| invalid("bad path in swap header"))?;
    Ok(Header {
        pid,
        base_len,
        base_crc,
        path,
        len: 32 + path_len,
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
    let mut rec = Vec::with_capacity(8 + payload.len());
    rec.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    rec.extend_from_slice(&crc(&payload).to_le_bytes());
    rec.extend_from_slice(&payload);
    rec
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
    while let (Some(len), Some(sum)) = (u32_at(bytes, pos), u32_at(bytes, pos + 4)) {
        let len = len as usize;
        let Some(payload) = bytes.get(pos + 8..pos + 8 + len) else {
            break;
        };
        if crc(payload) != sum || payload.is_empty() {
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
        pos += 8 + len;
        records += 1;
    }
    Replayed { rope, records }
}

/// Replays the swap at `swap_path` on top of `base` (the real file content,
/// whose CRC and length must match the header).
pub fn replay(base: &str, swap_path: &Path) -> io::Result<String> {
    let bytes = fs::read(swap_path)?;
    let h = parse_header(&bytes)?;
    if h.base_len != base.len() as u64 || h.base_crc != crc(base.as_bytes()) {
        return Err(invalid(
            "the file changed since the swap was written (stale swap)",
        ));
    }
    Ok(replay_records(&bytes, h.len, base).rope.to_string())
}

/// (byte offset after the last record with valid framing and CRC, record count).
fn framing_end(bytes: &[u8], start: usize) -> (usize, usize) {
    let mut pos = start;
    let mut n = 0;
    while let (Some(len), Some(sum)) = (u32_at(bytes, pos), u32_at(bytes, pos + 4)) {
        let len = len as usize;
        match bytes.get(pos + 8..pos + 8 + len) {
            Some(p) if crc(p) == sum => {
                n += 1;
                pos += 8 + len;
            }
            _ => break,
        }
    }
    (pos, n)
}

/// Number of records with valid framing in the swap (tests and diagnostics).
pub fn record_count(swap_path: &Path) -> io::Result<usize> {
    let bytes = fs::read(swap_path)?;
    let h = parse_header(&bytes)?;
    Ok(framing_end(&bytes, h.len).1)
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
    if h.pid != std::process::id() && pid_alive(h.pid) {
        return Ok(SwapStatus::LiveLock { pid: h.pid });
    }
    if h.base_len != current.len() as u64 || h.base_crc != crc(current) {
        return Ok(SwapStatus::Stale);
    }
    let Ok(base) = std::str::from_utf8(current) else {
        return Ok(SwapStatus::Stale);
    };
    let r = replay_records(&bytes, h.len, base);
    let text = r.rope.to_string();
    if r.records == 0 || text == base {
        // Nothing unsaved to offer.
        return Ok(SwapStatus::None);
    }
    Ok(SwapStatus::Recoverable(RecoverInfo {
        target: target.to_path_buf(),
        modified: modified_of(&swap_path),
        swap_path,
        recovered: Some(text),
    }))
}

/// Append-only writer for one swap file.
#[derive(Debug)]
pub struct SwapWriter {
    file: File,
    path: PathBuf,
    pending_fsync: bool,
}

impl SwapWriter {
    /// Creates (or replaces) the swap for `target` with `base` as the base
    /// content. Fails with a live-lock error if a running process owns it.
    pub fn create(target: &Path, base: &[u8]) -> io::Result<SwapWriter> {
        Self::create_in(&fileio::state_dir(), target, base)
    }

    /// [`create`](Self::create) with an explicit swap directory.
    pub fn create_in(dir: &Path, target: &Path, base: &[u8]) -> io::Result<SwapWriter> {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let path = swap_path_in(dir, target);
        if let Ok(mut f) = File::open(&path) {
            let mut head = vec![0u8; 16];
            if f.read_exact(&mut head).is_ok() && &head[..8] == MAGIC {
                let pid = u32_at(&head, 12).unwrap();
                if pid != std::process::id() && pid_alive(pid) {
                    return Err(io::Error::other(LiveLockError { pid }));
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
        file.write_all(&header_bytes(std::process::id(), target, base))?;
        file.sync_all()?;
        Ok(SwapWriter {
            file,
            path,
            pending_fsync: false,
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
        file.seek(SeekFrom::End(0))?;
        file.sync_all()?;
        Ok(SwapWriter {
            file,
            path: swap_path.to_path_buf(),
            pending_fsync: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes the edits immediately (one `write`), without fsync.
    pub fn append(&mut self, edits: &[Edit]) -> io::Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::new();
        for e in edits {
            buf.extend_from_slice(&encode_record(e));
        }
        self.file.write_all(&buf)?;
        self.pending_fsync = true;
        Ok(())
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
        let h = header_bytes(42, Path::new("/a/b.txt"), b"hello");
        let p = parse_header(&h).unwrap();
        assert_eq!(p.pid, 42);
        assert_eq!(p.base_len, 5);
        assert_eq!(p.base_crc, crc32fast::hash(b"hello"));
        assert_eq!(p.path, "/a/b.txt");
        assert_eq!(p.len, h.len());
        assert_eq!(u32_at(&h, PID_OFFSET as usize), Some(42));
    }
}
