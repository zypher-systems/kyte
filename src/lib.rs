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

use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event;
use ratatui::Terminal;

pub use cli::Options;

use app::App;
use buffer::Buffer;

/// How long the event loop waits for input before calling `App::tick`.
const POLL: Duration = Duration::from_millis(1000);

/// Runs the editor with parsed options and returns the process exit code.
pub fn run(opts: Options) -> anyhow::Result<ExitCode> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("kyte: stdin and stdout must be a terminal");
        return Ok(ExitCode::from(2));
    }
    let buffer = match &opts.file {
        Some(p) => {
            let abs = fileio::absolute(p)?;
            match fileio::load(&abs) {
                Ok(l) => Buffer::from_str(&l.text, Some(abs)),
                Err(e) => {
                    eprintln!("kyte: {}: {e}", p.display());
                    return Ok(ExitCode::from(2));
                }
            }
        }
        None => Buffer::from_str("", None),
    };
    let mut app = App::new(buffer, &opts);
    event_loop(&mut app, opts.no_kitty)?;
    Ok(ExitCode::SUCCESS)
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
