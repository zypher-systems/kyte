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

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event;
use ratatui::Terminal;

pub use cli::Options;

use app::App;
use buffer::Buffer;
use swap::{SwapStatus, SwapWriter};

/// How long the event loop waits for input before calling `App::tick`.
const POLL: Duration = Duration::from_millis(1000);

/// Runs the editor with parsed options and returns the process exit code.
pub fn run(opts: Options) -> anyhow::Result<ExitCode> {
    if let Some(p) = &opts.print_swap {
        return Ok(print_swap(p));
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("kyte: stdin and stdout must be a terminal");
        return Ok(ExitCode::from(2));
    }
    let mut app = match &opts.file {
        Some(p) => match open_file(p, &opts)? {
            Ok(app) => app,
            Err(code) => return Ok(code),
        },
        None => App::new(Buffer::from_str("", None), None, &opts),
    };
    let result = event_loop(&mut app, opts.no_kitty);
    if result.is_ok() {
        app.finish();
    }
    result?;
    Ok(ExitCode::SUCCESS)
}

/// Loads `p`, checks its swap and builds the app; `Err(code)` means exit now.
fn open_file(p: &Path, opts: &Options) -> anyhow::Result<Result<App, ExitCode>> {
    let abs = fileio::absolute(p)?;
    let loaded = match fileio::load(&abs) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("kyte: {}: {e}", p.display());
            return Ok(Err(ExitCode::from(2)));
        }
    };
    let target = fileio::resolve_target(&abs)?;
    let buffer = Buffer::from_str(&loaded.text, Some(abs));
    match swap::inspect(&target, loaded.text.as_bytes())? {
        SwapStatus::LiveLock { pid } => Ok(Err(live_lock_exit(p, pid))),
        SwapStatus::None => match SwapWriter::create(&target, loaded.text.as_bytes()) {
            Ok(w) => Ok(Ok(App::new(buffer, Some(w), opts))),
            Err(e) => match swap::live_lock_pid(&e) {
                Some(pid) => Ok(Err(live_lock_exit(p, pid))),
                None => {
                    let mut app = App::new(buffer, None, opts);
                    app.message = Some(format!(
                        "Warning: no swap file ({e}); crash recovery is off"
                    ));
                    app.message_is_error = true;
                    Ok(Ok(app))
                }
            },
        },
        SwapStatus::Recoverable(_) | SwapStatus::Stale => {
            eprintln!(
                "kyte: {} has a swap file; inspect it with `kyte --print-swap {}`",
                p.display(),
                p.display()
            );
            Ok(Err(ExitCode::from(2)))
        }
    }
}

fn live_lock_exit(p: &Path, pid: u32) -> ExitCode {
    eprintln!(
        "kyte: {} is already open in another kyte (PID {pid}); not touching its swap file",
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
    let Ok(base) = String::from_utf8(base) else {
        eprintln!("kyte: {} is not valid UTF-8; no valid swap", p.display());
        return ExitCode::from(3);
    };
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
            }
        }
        app.tick(Instant::now())?;
    }
    Ok(())
}
