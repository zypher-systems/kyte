use std::process::ExitCode;

use kyte::cli::{self, CliError};

fn main() -> ExitCode {
    let opts = match cli::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e @ (CliError::Help | CliError::Version)) => {
            println!("{e}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match kyte::run(opts) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("kyte: {e:#}");
            ExitCode::from(1)
        }
    }
}
