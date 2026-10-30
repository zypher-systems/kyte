//! Opening files (UTF-8 only), atomic save, and the state directory (section 6).

use std::collections::VecDeque;
use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{fchown, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ropey::Rope;
use rustix::fs::{access, fsetxattr, getxattr, listxattr, Access, XattrFlags};
use rustix::io::Errno;

/// A file as read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub text: String,
    pub existed: bool,
    /// Permission bits (`st_mode & 0o7777`) when the file existed.
    pub mode: Option<u32>,
}

/// Why a file could not be opened.
#[derive(Debug)]
pub enum OpenError {
    NotUtf8,
    IsDirectory,
    NotRegular,
    Io(io::Error),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NotUtf8 => {
                f.write_str("file is not valid UTF-8 (Kyte refuses to convert it)")
            }
            OpenError::IsDirectory => f.write_str("is a directory"),
            OpenError::NotRegular => f.write_str("is not a regular file"),
            OpenError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Reads `path`. A nonexistent path is an empty, not-yet-existing file.
/// Non-UTF-8 content is refused, never lossy-converted.
pub fn load(path: &Path) -> Result<Loaded, OpenError> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Loaded {
                text: String::new(),
                existed: false,
                mode: None,
            })
        }
        Err(e) => return Err(OpenError::Io(e)),
    };
    if meta.is_dir() {
        return Err(OpenError::IsDirectory);
    }
    if !meta.is_file() {
        return Err(OpenError::NotRegular);
    }
    let bytes = fs::read(path).map_err(OpenError::Io)?;
    let text = String::from_utf8(bytes).map_err(|_| OpenError::NotUtf8)?;
    Ok(Loaded {
        text,
        existed: true,
        mode: Some(meta.permissions().mode() & 0o7777),
    })
}

/// The real file a save must replace: symlinks are resolved so the link stays intact.
pub fn resolve_target(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Nonexistent (or dangling) target: follow a dangling symlink once,
            // otherwise keep the path and canonicalize its directory.
            if let Ok(link) = fs::read_link(path) {
                let base = path.parent().unwrap_or(Path::new("."));
                return Ok(base.join(link));
            }
            absolute(path)
        }
        Err(e) => Err(e),
    }
}

/// A path typed into a prompt, with a leading `~/` meaning the home directory.
pub fn expand_tilde(text: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (text.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => PathBuf::from(text),
    }
}

/// Directories too noisy to be worth listing.
const SKIPPED_DIRS: &[&str] = &["node_modules", "target"];

/// Files under `root`, as sorted relative paths, for the universal palette.
/// Hidden files and build directories are left out. It stops at `max` files
/// or after `budget`, nearest files first, so a huge tree can't stall the editor.
pub fn list_files(root: &Path, max: usize, budget: Duration) -> Vec<String> {
    let start = Instant::now();
    let mut files = Vec::new();
    let mut dirs = VecDeque::from([PathBuf::new()]);
    'walk: while let Some(rel) = dirs.pop_front() {
        let Ok(entries) = fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= max || start.elapsed() > budget {
                break 'walk;
            }
            let name = entry.file_name();
            let text = name.to_string_lossy();
            if text.starts_with('.') {
                continue;
            }
            // Symlinks are skipped: one could point back up the tree.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = rel.join(&name);
            if kind.is_dir() && !SKIPPED_DIRS.contains(&text.as_ref()) {
                dirs.push_back(path);
            } else if kind.is_file() {
                files.push(path.to_string_lossy().into_owned());
            }
        }
    }
    files.sort();
    files
}

/// What Tab did to a path being typed.
#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    /// The text with as much added as every choice shares.
    pub text: String,
    /// Names that match, sorted.
    pub choices: Vec<String>,
}

/// Completes the last part of a typed path from what's on disk. Hidden files
/// count only once the name starts with a dot. `None` when nothing matches.
pub fn complete_path(text: &str) -> Option<Completion> {
    let (dir_part, stem) = match text.rfind('/') {
        Some(i) => text.split_at(i + 1),
        None => ("", text),
    };
    let dir = match dir_part {
        "" => PathBuf::from("."),
        d => expand_tilde(d),
    };
    let mut names: Vec<(String, bool)> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let wanted =
                name.starts_with(stem) && (stem.starts_with('.') || !name.starts_with('.'));
            let is_dir = fs::metadata(e.path()).is_ok_and(|m| m.is_dir());
            wanted.then_some((name, is_dir))
        })
        .collect();
    names.sort();
    let first = &names.first()?.0;
    let shared = names.iter().fold(first.len(), |n, (name, _)| {
        first
            .char_indices()
            .zip(name.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, c), _)| i + c.len_utf8())
            .min(n)
    });
    let slash = if names.len() == 1 && names[0].1 {
        "/"
    } else {
        ""
    };
    Some(Completion {
        text: format!("{dir_part}{}{slash}", &first[..shared]),
        choices: names.into_iter().map(|(n, _)| n).collect(),
    })
}

/// Absolute form of `path` with its parent directory canonicalized when possible.
pub fn absolute(path: &Path) -> io::Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if let (Some(parent), Some(name)) = (abs.parent(), abs.file_name()) {
        if let Ok(p) = fs::canonicalize(parent) {
            return Ok(p.join(name));
        }
    }
    Ok(abs)
}

/// Length and checksum of a file's content, for noticing outside changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskState {
    len: u64,
    crc: u32,
}

impl DiskState {
    pub fn new(len: u64, crc: u32) -> DiskState {
        DiskState { len, crc }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn crc(&self) -> u32 {
        self.crc
    }

    pub fn of_bytes(bytes: &[u8]) -> DiskState {
        DiskState {
            len: bytes.len() as u64,
            crc: crc32fast::hash(bytes),
        }
    }

    pub fn of_rope(rope: &Rope) -> DiskState {
        let mut h = crc32fast::Hasher::new();
        for chunk in rope.chunks() {
            h.update(chunk.as_bytes());
        }
        DiskState {
            len: rope.len_bytes() as u64,
            crc: h.finalize(),
        }
    }
}

/// The state of `path` on disk, `None` when it doesn't exist.
pub fn disk_state(path: &Path) -> io::Result<Option<DiskState>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(DiskState::of_bytes(&b))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether `path` exists and this process may not write to it.
pub fn is_read_only(path: &Path) -> bool {
    path.exists() && access(path, Access::WRITE_OK).is_err()
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Gives the new file the old file's owner, group and extended attributes.
/// Extended attributes hold SELinux labels and ACLs too.
fn carry_metadata(new: &File, old_path: &Path, old: &Metadata) -> io::Result<()> {
    let meta = new.metadata()?;
    if meta.uid() != old.uid() || meta.gid() != old.gid() {
        fchown(new, Some(old.uid()), Some(old.gid()))?;
    }
    let mut names = vec![0u8; 4096];
    let n = loop {
        match listxattr(old_path, &mut names[..]) {
            Ok(n) => break n,
            Err(Errno::RANGE) => names.resize(names.len() * 2, 0),
            Err(Errno::NOTSUP) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    };
    let mut value = vec![0u8; 4096];
    for name in names[..n].split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let len = loop {
            match getxattr(old_path, name, &mut value[..]) {
                Ok(l) => break l,
                Err(Errno::RANGE) => value.resize(value.len() * 2, 0),
                Err(e) => return Err(e.into()),
            }
        };
        fsetxattr(new, name, &value[..len], XattrFlags::empty())?;
    }
    Ok(())
}

fn overwrite(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).open(path)?;
    f.write_all(bytes)?;
    f.set_len(bytes.len() as u64)?;
    f.sync_all()
}

/// Rewrites `target` in its own inode, for files a rename would damage. The
/// old content goes to a backup first and is put back if the write fails.
fn save_in_place(target: &Path, backup: &Path, text: &str) -> io::Result<()> {
    let old = fs::read(target)?;
    let mut b = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(backup)?;
    if let Err(e) = b.write_all(&old).and_then(|_| b.sync_all()) {
        let _ = fs::remove_file(backup);
        return Err(e);
    }
    drop(b);
    if let Err(e) = overwrite(target, text.as_bytes()) {
        if overwrite(target, &old).is_ok() {
            let _ = fs::remove_file(backup);
            return Err(e);
        }
        return Err(io::Error::new(
            e.kind(),
            format!("{e}; the old content is in {}", backup.display()),
        ));
    }
    fs::remove_file(backup)
}

/// Temp file in the same directory, write, fsync, copy permissions, owner and
/// extended attributes, rename over the target (resolving symlinks first),
/// fsync the directory. On any error the original is untouched and the temp
/// file is removed. A hard-linked file, or one whose owner or attributes the
/// temp file can't take on, is rewritten in place instead, because a rename
/// would change what the file is.
pub fn save_atomic(path: &Path, text: &str) -> io::Result<()> {
    let target = resolve_target(path)?;
    let dir = target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
        .to_string_lossy()
        .into_owned();
    let old = fs::metadata(&target).ok();
    let mode = old.as_ref().map(|m| m.permissions().mode() & 0o7777);
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let tmp = dir.join(format!(".{name}.kyte-tmp.{pid}.{n}"));
    let mut in_place = false;
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode.unwrap_or(0o666))
            .open(&tmp)?;
        if let Some(m) = &old {
            if m.nlink() > 1 || carry_metadata(&f, &target, m).is_err() {
                in_place = true;
                return Ok(());
            }
        }
        f.write_all(text.as_bytes())?;
        // After the chown, which clears the setuid and setgid bits.
        if let Some(m) = mode {
            f.set_permissions(fs::Permissions::from_mode(m))?;
        }
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &target)
    })();
    if in_place {
        let _ = fs::remove_file(&tmp);
        let backup = dir.join(format!(".{name}.kyte-bak.{pid}.{n}"));
        return save_in_place(&target, &backup, text);
    }
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // The rename is done; a failed directory fsync cannot be undone, so ignore it.
    if let Ok(d) = File::open(&dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Swap directory: `$KYTE_STATE_DIR`, else `$XDG_STATE_HOME/kyte/swap`, else
/// `~/.local/state/kyte/swap`.
pub fn state_dir() -> PathBuf {
    state_dir_from(
        std::env::var_os("KYTE_STATE_DIR").map(PathBuf::from),
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// History directory: inside `$KYTE_STATE_DIR` if set, else next to `swap`.
pub fn history_dir() -> PathBuf {
    let swap = state_dir();
    if std::env::var_os("KYTE_STATE_DIR").is_some_and(|d| !d.is_empty()) {
        swap.join("history")
    } else {
        swap.with_file_name("history")
    }
}

/// Pure form of [`state_dir`] for tests. Empty values count as unset.
pub fn state_dir_from(
    kyte: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
) -> PathBuf {
    let set = |p: Option<PathBuf>| p.filter(|p| !p.as_os_str().is_empty());
    if let Some(k) = set(kyte) {
        return k;
    }
    if let Some(x) = set(xdg) {
        return x.join("kyte").join("swap");
    }
    set(home)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".local/state/kyte/swap")
}
