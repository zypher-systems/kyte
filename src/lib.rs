//! Kyte: a terminal text editor.
//!
//! The binary in `main.rs` parses options and calls [`run`].

pub mod app;
pub mod buffer;
pub mod cli;
pub mod clipboard;
pub mod command;
pub mod config;
pub mod diff;
pub mod fileio;
#[cfg(feature = "highlight")]
pub mod highlight;
pub mod history;
pub mod input;
pub mod ops;
pub mod palette;
pub mod reader;
pub mod review;
pub mod search;
pub mod session;
pub mod swap;
pub mod syntax;
pub mod term;
pub mod ui;
pub mod wrap;

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event;
use ratatui::Terminal;

pub use cli::Options;

use app::{App, Mode};
use buffer::Buffer;
use session::{Opened, Refused};
use swap::SwapWriter;

/// How long the event loop waits for input before calling `App::tick`.
const POLL: Duration = Duration::from_millis(1000);

/// Runs the editor with parsed options and returns the process exit code.
pub fn run(opts: Options) -> anyhow::Result<ExitCode> {
    if let Some(p) = &opts.print_swap {
        return Ok(print_swap(p));
    }
    buffer::set_tab_width(opts.tab_width);
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("kyte: stdin and stdout must be a terminal");
        return Ok(ExitCode::from(2));
    }
    let mut app = match &opts.file {
        Some(p) => match open_file(p, &opts)? {
            Ok(app) => app,
            Err(code) => return Ok(code),
        },
        None => open_untitled(&opts),
    };
    if !opts.no_history {
        app.enable_history();
    }
    if app.message.is_none() {
        app.message = opts.config_warning.clone();
        app.message_is_error = app.message.is_some();
    }
    let result = event_loop(&mut app, opts.no_kitty);
    if result.is_ok() {
        app.finish();
    }
    result?;
    Ok(ExitCode::SUCCESS)
}

/// An empty buffer with no file name, or a crashed one to recover.
fn open_untitled(opts: &Options) -> App {
    let buffer = Buffer::from_str("", None);
    if let Ok(Some(info)) = swap::find_untitled() {
        let mut app = App::new(buffer, None, opts);
        app.mode = Mode::Recover(info);
        return app;
    }
    match SwapWriter::create_untitled() {
        Ok(w) => App::new(buffer, Some(w), opts),
        Err(e) => without_swap(buffer, opts, e),
    }
}

/// Opens `p` and builds the app; `Err(code)` means exit now.
fn open_file(p: &Path, opts: &Options) -> anyhow::Result<Result<App, ExitCode>> {
    match session::open(p, opts.break_lock) {
        Ok(Opened::Ready {
            buffer,
            swap,
            warning,
        }) => {
            let mut app = App::new(buffer, swap, opts);
            if warning.is_some() {
                app.message = warning;
                app.message_is_error = true;
            }
            Ok(Ok(app))
        }
        Ok(Opened::Recover { buffer, info }) => {
            let mut app = App::new(buffer, None, opts);
            app.mode = Mode::Recover(info);
            Ok(Ok(app))
        }
        Ok(Opened::Locked { pid }) => Ok(Err(live_lock_exit(p, pid))),
        Err(Refused::File(e)) => {
            eprintln!("kyte: {}: {e}", p.display());
            Ok(Err(ExitCode::from(2)))
        }
        Err(Refused::Failed(e)) => Err(e.into()),
    }
}

/// An untitled buffer still opens when its swap can't be made, with a warning.
fn without_swap(buffer: Buffer, opts: &Options, e: std::io::Error) -> App {
    let mut app = App::new(buffer, None, opts);
    app.message = Some(format!(
        "Warning: no swap file ({e}); crash recovery is off"
    ));
    app.message_is_error = true;
    app
}

fn live_lock_exit(p: &Path, pid: u32) -> ExitCode {
    eprintln!(
        "kyte: {} is already open in another kyte (PID {pid}); not touching its swap file\n\
         If that kyte is gone, open the file with --break-lock.",
        p.display()
    );
    ExitCode::from(2)
}

/// `--print-swap FILE`: prints the recovered content; exit 3 when there is no valid swap.
fn print_swap(p: &Path) -> ExitCode {
    let target = match fileio::absolute(p).and_then(|a| fileio::resolve_target(&a)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("kyte: {}: {e}", p.display());
            return ExitCode::from(3);
        }
    };
    let base = match std::fs::read(&target) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            eprintln!("kyte: {}: {e}", p.display());
            return ExitCode::from(3);
        }
    };
    let swap_path = swap::swap_path_for(&target);
    if !swap_path.exists() {
        eprintln!("kyte: no swap file for {}", p.display());
        return ExitCode::from(3);
    }
    // A file that is no longer UTF-8 can't be the base, but the snapshot can.
    let base = String::from_utf8(base).unwrap_or_default();
    match swap::replay(&base, &swap_path) {
        Ok(text) => {
            let mut out = std::io::stdout();
            if out
                .write_all(text.as_bytes())
                .and_then(|_| out.flush())
                .is_err()
            {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("kyte: no valid swap for {}: {e}", p.display());
            ExitCode::from(3)
        }
    }
}

fn event_loop(app: &mut App, no_kitty: bool) -> anyhow::Result<()> {
    term::install_panic_hook();
    term::install_signal_handler()?;
    let _guard = term::TerminalGuard::enter(no_kitty)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let mut redraw = true;
    loop {
        if redraw {
            let size = terminal.size()?;
            app.layout(size.width, size.height);
            terminal.draw(|f| ui::render(app, f))?;
            redraw = false;
        }
        if app.quit {
            break;
        }
        if event::poll(POLL)? {
            let ev = event::read()?;
            if let Some(input) = input::map_event(ev) {
                app.handle_input(input, Instant::now())?;
                redraw = true;
                if let Some(seq) = app.take_terminal_output() {
                    // A terminal that can't take it just ignores the copy.
                    let backend = terminal.backend_mut();
                    let _ = backend
                        .write_all(seq.as_bytes())
                        .and_then(|_| backend.flush());
                }
            }
        }
        app.tick(Instant::now())?;
        redraw |= app.take_redraw();
    }
    Ok(())
}
