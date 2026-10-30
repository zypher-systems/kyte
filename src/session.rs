//! Opening a file for editing: reading it, checking its swap and taking the
//! lock. Startup and Ctrl-O both come through here.

use std::path::Path;

use crate::buffer::Buffer;
use crate::fileio;
use crate::swap::{self, RecoverInfo, SwapStatus, SwapWriter};

/// The result of opening a file.
pub enum Opened {
    Ready {
        buffer: Buffer,
        swap: Option<SwapWriter>,
        /// Why the swap couldn't be used.
        warning: Option<String>,
    },
    /// A crashed session left edits, or a swap Kyte can't read.
    Recover { buffer: Buffer, info: RecoverInfo },
    /// Another running Kyte has the file.
    Locked { pid: u32 },
}

/// Why a file could not be opened.
#[derive(Debug)]
pub enum Refused {
    /// The file itself can't be edited (exit code 2 at startup).
    File(fileio::OpenError),
    /// Something else went wrong (exit code 1 at startup).
    Failed(std::io::Error),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::File(e) => write!(f, "{e}"),
            Refused::Failed(e) => write!(f, "{e}"),
        }
    }
}

fn no_swap_warning(e: &std::io::Error) -> Option<String> {
    Some(format!(
        "Warning: no swap file ({e}); crash recovery is off"
    ))
}

/// Opens `path`, creating its swap unless there's one to deal with first.
pub fn open(path: &Path, break_lock: bool) -> Result<Opened, Refused> {
    let abs = fileio::absolute(path).map_err(Refused::Failed)?;
    let loaded = fileio::load(&abs).map_err(Refused::File)?;
    let target = fileio::resolve_target(&abs).map_err(Refused::Failed)?;
    if break_lock {
        swap::break_lock(&target).map_err(Refused::Failed)?;
    }
    let buffer = Buffer::from_str(&loaded.text, Some(abs));
    let status = match swap::inspect(&target, loaded.text.as_bytes()) {
        Ok(s) => s,
        Err(e) => {
            return Ok(Opened::Ready {
                buffer,
                swap: None,
                warning: no_swap_warning(&e),
            })
        }
    };
    Ok(match status {
        SwapStatus::LiveLock { pid } => Opened::Locked { pid },
        SwapStatus::None => match SwapWriter::create(&target, buffer.rope()) {
            Ok(w) => Opened::Ready {
                buffer,
                swap: Some(w),
                warning: None,
            },
            Err(e) => match swap::live_lock_pid(&e) {
                Some(pid) => Opened::Locked { pid },
                None => Opened::Ready {
                    buffer,
                    swap: None,
                    warning: no_swap_warning(&e),
                },
            },
        },
        SwapStatus::Recoverable(info) => Opened::Recover { buffer, info },
        SwapStatus::Stale => Opened::Recover {
            buffer,
            info: swap::stale_info(&target),
        },
    })
}

/// Whether `path` could be opened, without changing anything.
pub fn check(path: &Path) -> Result<(), String> {
    let abs = fileio::absolute(path).map_err(|e| e.to_string())?;
    let loaded = fileio::load(&abs).map_err(|e| e.to_string())?;
    let target = fileio::resolve_target(&abs).map_err(|e| e.to_string())?;
    match swap::inspect(&target, loaded.text.as_bytes()) {
        Ok(SwapStatus::LiveLock { pid }) => Err(format!("Open in another kyte (PID {pid})")),
        _ => Ok(()),
    }
}
