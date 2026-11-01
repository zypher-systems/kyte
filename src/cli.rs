//! Hand-written command-line parsing (section 7 of the architecture).

use std::fmt;
use std::path::PathBuf;

use crate::buffer::Indent;
use crate::config;
use crate::ui::theme::ThemeName;
use crate::update::{Action, Request};

/// Default double-tap Esc window in milliseconds.
pub const DEFAULT_ESC_MS: u64 = 300;

/// Runtime settings. This is the single source of runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub file: Option<PathBuf>,
    /// The files after the first.
    pub more_files: Vec<PathBuf>,
    pub esc_ms: u64,
    pub no_kitty: bool,
    pub print_swap: Option<PathBuf>,
    pub break_lock: bool,
    pub no_history: bool,
    /// Run the updater instead of the editor.
    pub update: Option<Request>,
    /// Ask once a day, at launch, whether a newer release is out.
    pub update_check: bool,
    /// Columns a tab takes up on screen.
    pub tab_width: usize,
    /// What Tab inserts in a file with no indent to go by.
    pub default_indent: Indent,
    /// How many spaces, once `default_indent` is spaces.
    pub indent_width: usize,
    pub line_numbers: bool,
    pub soft_wrap: bool,
    /// Markdown, text and LaTeX get the writing setup.
    pub writing: bool,
    /// What builds a LaTeX file, instead of `latexmk`.
    pub build_command: Option<String>,
    /// What shows a PDF, instead of `xdg-open`.
    pub pdf_command: Option<String>,
    /// Writing gets its spelling checked.
    pub spell_check: bool,
    pub theme: ThemeName,
    pub history_limit_mb: u64,
    /// What was wrong with the settings file, for the first screen.
    pub config_warning: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            file: None,
            more_files: Vec::new(),
            esc_ms: DEFAULT_ESC_MS,
            no_kitty: false,
            print_swap: None,
            break_lock: false,
            no_history: false,
            update: None,
            update_check: true,
            tab_width: 4,
            default_indent: Indent::Tabs,
            indent_width: 4,
            line_numbers: true,
            soft_wrap: false,
            writing: true,
            build_command: None,
            pdf_command: None,
            spell_check: true,
            theme: ThemeName::Signal,
            history_limit_mb: 100,
            config_warning: None,
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

/// `kyte 0.7.0`, taken from the crate version.
pub fn version() -> String {
    format!("kyte {}", env!("CARGO_PKG_VERSION"))
}

/// The `--help` text. Lists every option of section 7.
pub fn usage() -> String {
    format!(
        "{}\nA terminal text editor: always insert, Esc for one-key operators, Esc Esc for the palette.\n\n\
Usage: kyte [OPTIONS] [FILE]...\n\n\
Options:\n\
  -h, --help             Print this help and exit\n\
  -V, --version          Print the version and exit\n\
      --esc-ms N         Double-tap Esc window in milliseconds (default {DEFAULT_ESC_MS}; env KYTE_ESC_MS)\n\
      --no-kitty         Never enable the kitty keyboard protocol\n\
      --break-lock       Open FILE even if another kyte seems to hold it\n\
      --no-history       Keep no file history this session\n\
      --print-swap FILE  Print the content recovered from FILE's swap and exit (exit 3 if none)\n\
      --update           Install the newest release, the way this copy was installed\n\
      --update-check     Only say whether a newer release is out\n\
      --update-to V      Install version V instead of the newest\n\
      --update-rollback  Go back to the version the last update replaced\n\
  -y, --yes              Don't ask before updating\n\n\
Environment:\n\
  KYTE_STATE_DIR         Swap directory override\n\
  KYTE_CONFIG            Settings file (default ~/.config/kyte/config.toml)\n\
  KYTE_ESC_MS            Default for --esc-ms\n\
  KYTE_RELEASES_API      Where --update asks for the latest release\n\
  KYTE_DOWNLOAD_URL      Where --update downloads from (a mirror; signatures still apply)\n\n\
Exit codes: 0 ok, 1 runtime error, 2 bad usage or refused file, 3 no swap for --print-swap\n",
        version()
    )
}

/// Parses `args` (without the program name), reading `KYTE_ESC_MS` from the environment.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Options, CliError> {
    parse_onto(config::load(), args, std::env::var("KYTE_ESC_MS").ok())
}

/// Like [`parse`] but with the `KYTE_ESC_MS` value injected (for tests).
pub fn parse_with_env(
    args: impl Iterator<Item = String>,
    env_esc_ms: Option<String>,
) -> Result<Options, CliError> {
    parse_onto(Options::default(), args, env_esc_ms)
}

/// Layers the environment and then `args` over `base`, the settings file's
/// values, so each later source wins.
pub fn parse_onto(
    base: Options,
    args: impl Iterator<Item = String>,
    env_esc_ms: Option<String>,
) -> Result<Options, CliError> {
    let mut opts = base;
    if let Some(v) = env_esc_ms {
        if !v.is_empty() {
            opts.esc_ms = parse_ms(&v, "KYTE_ESC_MS")?;
        }
    }
    let mut args = args.peekable();
    let mut only_files = false;
    let mut action = None;
    let mut yes = false;
    let mut choose = |wanted: Action, name: &str| match action.replace(wanted) {
        None => Ok(()),
        Some(_) => Err(CliError::Usage(format!(
            "'{name}' can't go with another update option"
        ))),
    };
    while let Some(arg) = args.next() {
        if only_files || arg == "-" || !arg.starts_with('-') {
            if opts.file.is_some() {
                opts.more_files.push(PathBuf::from(arg));
            } else {
                opts.file = Some(PathBuf::from(arg));
            }
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
            "--update" => choose(Action::Latest, "--update")?,
            "--update-check" => choose(Action::Check, "--update-check")?,
            "--update-rollback" => choose(Action::Rollback, "--update-rollback")?,
            "--update-to" => {
                let v = value(inline, &mut args, "--update-to")?;
                choose(Action::To(v), "--update-to")?;
            }
            "-y" | "--yes" => yes = true,
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
    match action {
        Some(_) if opts.file.is_some() => Err(CliError::Usage(
            "the update options don't open files".to_string(),
        )),
        Some(action) => {
            opts.update = Some(Request { action, yes });
            Ok(opts)
        }
        None if yes => Err(CliError::Usage(
            "'--yes' only goes with an update option".to_string(),
        )),
        None => Ok(opts),
    }
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
    fn every_file_named_is_kept_in_order() {
        let o = p(&["a.txt", "b.txt", "--", "-c"]).unwrap();
        assert_eq!(o.file, Some(PathBuf::from("a.txt")));
        assert_eq!(
            o.more_files,
            vec![PathBuf::from("b.txt"), PathBuf::from("-c")]
        );
    }

    #[test]
    fn help_and_version() {
        assert_eq!(p(&["-h"]), Err(CliError::Help));
        assert_eq!(p(&["--help"]), Err(CliError::Help));
        assert_eq!(p(&["-V"]), Err(CliError::Version));
        assert_eq!(p(&["--version"]), Err(CliError::Version));
        assert_eq!(version(), "kyte 0.7.0");
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
            "--update",
            "--update-check",
            "--update-to",
            "--update-rollback",
            "--yes",
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
    fn update_options() {
        let wants = |args: &[&str]| p(args).unwrap().update;
        let request = |action, yes| Some(Request { action, yes });
        assert_eq!(wants(&[]), None);
        assert_eq!(wants(&["--update"]), request(Action::Latest, false));
        assert_eq!(wants(&["--update", "-y"]), request(Action::Latest, true));
        assert_eq!(wants(&["--yes", "--update"]), request(Action::Latest, true));
        assert_eq!(wants(&["--update-check"]), request(Action::Check, false));
        assert_eq!(
            wants(&["--update-rollback"]),
            request(Action::Rollback, false)
        );
        assert_eq!(
            wants(&["--update-to", "0.6.0"]),
            request(Action::To("0.6.0".into()), false)
        );
        assert_eq!(
            wants(&["--update-to=v0.6.0"]),
            request(Action::To("v0.6.0".into()), false)
        );
    }

    #[test]
    fn update_options_dont_mix() {
        for bad in [
            &["--update", "--update-check"][..],
            &["--update-to", "0.6.0", "--update-rollback"],
            &["--update", "notes.txt"],
            &["--yes"],
            &["--yes", "notes.txt"],
            &["--update-to"],
        ] {
            assert!(matches!(p(bad), Err(CliError::Usage(_))), "{bad:?}");
        }
    }

    /// `kyte NAME` keeps opening NAME, even when that's the word.
    #[test]
    fn a_file_called_update_is_a_file() {
        let o = p(&["update"]).unwrap();
        assert_eq!(o.file, Some(PathBuf::from("update")));
        assert_eq!(o.update, None);
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
