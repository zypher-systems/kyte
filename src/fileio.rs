//! Opening files (UTF-8 only), atomic save, and the state directory (section 6).

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
    Io(io::Error),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NotUtf8 => {
                f.write_str("file is not valid UTF-8 (Kyte refuses to convert it)")
            }
            OpenError::IsDirectory => f.write_str("is a directory"),
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

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Temp file in the same directory, write, fsync, copy permissions, rename over
/// the target (resolving symlinks first), fsync the directory. On any error the
/// original is untouched and the temp file is removed.
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
    let mode = fs::metadata(&target)
        .ok()
        .map(|m| m.permissions().mode() & 0o7777);
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.kyte-tmp.{}.{n}", std::process::id()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode.unwrap_or(0o666))
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        if let Some(m) = mode {
            f.set_permissions(fs::Permissions::from_mode(m))?;
        }
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &target)
    })();
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
