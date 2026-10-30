//! Kyte: a terminal text editor.
//!
//! The binary in `main.rs` parses options and calls [`run`].

pub mod app;
pub mod buffer;
pub mod cli;
pub mod command;
pub mod fileio;
pub mod input;
pub mod ops;
pub mod palette;
pub mod search;
pub mod swap;
pub mod term;
pub mod ui;

use std::process::ExitCode;

pub use cli::Options;

/// Runs the editor with parsed options and returns the process exit code.
pub fn run(_opts: Options) -> anyhow::Result<ExitCode> {
    anyhow::bail!("the editor is not built yet")
}
