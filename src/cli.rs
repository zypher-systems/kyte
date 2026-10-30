//! Hand-written command-line parsing (section 7 of the architecture).

use std::fmt;
use std::path::PathBuf;

/// Default double-tap Esc window in milliseconds.
pub const DEFAULT_ESC_MS: u64 = 300;

/// Runtime settings. This is the single source of runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub file: Option<PathBuf>,
    pub esc_ms: u64,
    pub no_kitty: bool,
    pub print_swap: Option<PathBuf>,
    pub break_lock: bool,
    pub no_history: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            file: None,
            esc_ms: DEFAULT_ESC_MS,
            no_kitty: false,
            print_swap: None,
            break_lock: false,
            no_history: false,
        }
    }
}

/// Why parsing stopped. `Help` and `Version` are early exits with code 0;
/// `Usage` is bad usage with code 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    Help,
    Version,
    Usage(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Help => f.write_str(&usage()),
            CliError::Version => f.write_str(&version()),
            CliError::Usage(msg) => {
                write!(f, "kyte: {msg}\nTry 'kyte --help' for more information.")
            }
        }
    }
}

/// `kyte 0.4.0`, taken from the crate version.
pub fn version() -> String {
    format!("kyte {}", env!("CARGO_PKG_VERSION"))
}

/// The `--help` text. Lists every option of section 7.
pub fn usage() -> String {
    format!(
        "{}\nA terminal text editor: always insert, Esc for one-key operators, Esc Esc for the palette.\n\n\
Usage: kyte [OPTIONS] [FILE]\n\n\
Options:\n\
  -h, --help             Print this help and exit\n\
  -V, --version          Print the version and exit\n\
      --esc-ms N         Double-tap Esc window in milliseconds (default {DEFAULT_ESC_MS}; env KYTE_ESC_MS)\n\
      --no-kitty         Never enable the kitty keyboard protocol\n\
      --break-lock       Open FILE even if another kyte seems to hold it\n\
      --no-history       Keep no file history this session\n\
      --print-swap FILE  Print the content recovered from FILE's swap and exit (exit 3 if none)\n\n\
Environment:\n\
  KYTE_STATE_DIR         Swap directory override\n\
  KYTE_ESC_MS            Default for --esc-ms\n\n\
Exit codes: 0 ok, 1 runtime error, 2 bad usage or refused file, 3 no swap for --print-swap\n",
        version()
    )
}

/// Parses `args` (without the program name), reading `KYTE_ESC_MS` from the environment.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Options, CliError> {
    parse_with_env(args, std::env::var("KYTE_ESC_MS").ok())
}

/// Like [`parse`] but with the `KYTE_ESC_MS` value injected (for tests).
pub fn parse_with_env(
    args: impl Iterator<Item = String>,
    env_esc_ms: Option<String>,
) -> Result<Options, CliError> {
    let mut opts = Options::default();
    if let Some(v) = env_esc_ms {
        if !v.is_empty() {
            opts.esc_ms = parse_ms(&v, "KYTE_ESC_MS")?;
        }
    }
    let mut args = args.peekable();
    let mut only_files = false;
    while let Some(arg) = args.next() {
        if only_files || arg == "-" || !arg.starts_with('-') {
            if opts.file.is_some() {
                return Err(CliError::Usage(format!(
                    "unexpected extra argument '{arg}'"
                )));
            }
            opts.file = Some(PathBuf::from(arg));
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        match name.as_str() {
            "--" => only_files = true,
            "-h" | "--help" => return Err(CliError::Help),
            "-V" | "--version" => return Err(CliError::Version),
            "--no-kitty" => opts.no_kitty = true,
            "--break-lock" => opts.break_lock = true,
            "--no-history" => opts.no_history = true,
            "--esc-ms" => {
                let v = value(inline, &mut args, "--esc-ms")?;
                opts.esc_ms = parse_ms(&v, "--esc-ms")?;
            }
            "--print-swap" => {
                let v = value(inline, &mut args, "--print-swap")?;
                opts.print_swap = Some(PathBuf::from(v));
            }
            _ => return Err(CliError::Usage(format!("unknown option '{arg}'"))),
        }
    }
    Ok(opts)
}

fn value(
    inline: Option<String>,
    args: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<String, CliError> {
    match inline.or_else(|| args.next()) {
        Some(v) => Ok(v),
        None => Err(CliError::Usage(format!("option '{name}' needs a value"))),
    }
}

fn parse_ms(v: &str, name: &str) -> Result<u64, CliError> {
    match v.parse::<u64>() {
        Ok(n) if n <= 10_000 => Ok(n),
        _ => Err(CliError::Usage(format!(
            "{name} expects a number of milliseconds (0-10000), got '{v}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Options, CliError> {
        parse_with_env(args.iter().map(|s| s.to_string()), None)
    }

    #[test]
    fn no_args_gives_defaults() {
        assert_eq!(p(&[]).unwrap(), Options::default());
        assert_eq!(Options::default().esc_ms, 300);
    }

    #[test]
    fn file_argument() {
        assert_eq!(p(&["a.txt"]).unwrap().file, Some(PathBuf::from("a.txt")));
        assert_eq!(p(&["--", "-x"]).unwrap().file, Some(PathBuf::from("-x")));
    }

    #[test]
    fn two_files_is_usage_error() {
        assert!(matches!(p(&["a", "b"]), Err(CliError::Usage(_))));
    }

    #[test]
    fn help_and_version() {
        assert_eq!(p(&["-h"]), Err(CliError::Help));
        assert_eq!(p(&["--help"]), Err(CliError::Help));
        assert_eq!(p(&["-V"]), Err(CliError::Version));
        assert_eq!(p(&["--version"]), Err(CliError::Version));
        assert_eq!(version(), "kyte 0.4.0");
    }

    #[test]
    fn help_lists_every_option() {
        let u = usage();
        for o in [
            "--help",
            "--version",
            "--esc-ms",
            "--no-kitty",
            "--print-swap",
            "-h",
            "-V",
        ] {
            assert!(u.contains(o), "usage misses {o}");
        }
    }

    #[test]
    fn esc_ms_forms() {
        assert_eq!(p(&["--esc-ms", "800"]).unwrap().esc_ms, 800);
        assert_eq!(p(&["--esc-ms=150"]).unwrap().esc_ms, 150);
        assert!(matches!(p(&["--esc-ms", "abc"]), Err(CliError::Usage(_))));
        assert!(matches!(p(&["--esc-ms"]), Err(CliError::Usage(_))));
    }

    #[test]
    fn esc_ms_env_and_override() {
        let env = |v: &str, args: &[&str]| {
            parse_with_env(args.iter().map(|s| s.to_string()), Some(v.to_string()))
        };
        assert_eq!(env("450", &[]).unwrap().esc_ms, 450);
        assert_eq!(env("450", &["--esc-ms", "100"]).unwrap().esc_ms, 100);
        assert!(matches!(env("x", &[]), Err(CliError::Usage(_))));
    }

    #[test]
    fn no_kitty_flag() {
        assert!(p(&["--no-kitty"]).unwrap().no_kitty);
        assert!(!p(&[]).unwrap().no_kitty);
    }

    #[test]
    fn print_swap() {
        assert_eq!(
            p(&["--print-swap", "f.txt"]).unwrap().print_swap,
            Some(PathBuf::from("f.txt"))
        );
        assert!(matches!(p(&["--print-swap"]), Err(CliError::Usage(_))));
    }

    #[test]
    fn unknown_option() {
        assert!(matches!(p(&["--bogus"]), Err(CliError::Usage(_))));
        assert!(matches!(p(&["-z"]), Err(CliError::Usage(_))));
    }
}
