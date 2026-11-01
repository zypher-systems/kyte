//! The settings file: `key = value` lines in `~/.config/kyte/config.toml`.
//! A bad line is reported and skipped, because a typo in a settings file
//! should never stop someone from opening their editor.

use std::fs;
use std::path::PathBuf;

use crate::buffer::Indent;
use crate::cli::Options;
use crate::ui::theme::ThemeName;

const MAX_TAB_WIDTH: usize = 16;

/// What a new settings file holds. Every line is a comment, so it changes nothing.
pub const TEMPLATE: &str = r#"# Kyte settings. Remove the # from a line to use it, then restart Kyte.

# Columns a tab takes up on screen.
# tab_width = 4

# What Tab inserts in a file with no indent to go by: "tabs" or "spaces".
# indent = "tabs"

# How many spaces, when indent is "spaces".
# indent_width = 4

# Show line numbers.
# line_numbers = true

# Wrap long lines at the edge of the screen.
# soft_wrap = false

# Show Markdown, text and LaTeX files for writing: wrapped, with a word count.
# writing = true

# Underline misspelled words in writing.
# spell_check = true

# What builds a LaTeX file, run in its folder. %f is the file.
# build_command = "latexmk -pdf -interaction=nonstopmode -file-line-error %f"

# What shows a PDF.
# pdf_command = "xdg-open %f"

# Colors: "signal", "harbor", "dusk", "ember", "moss" or "daybreak".
# theme = "signal"

# Milliseconds to press Esc a second time for the palette.
# esc_ms = 300

# Keep a history of every edit to each file.
# history = true

# Trim a file's history when it passes this many megabytes.
# history_limit_mb = 100

# Ask GitHub once a day whether a newer Kyte is out. It only tells you.
# update_check = true
"#;

/// Where the settings file lives.
pub fn path() -> PathBuf {
    if let Some(p) = std::env::var_os("KYTE_CONFIG").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("kyte").join("config.toml")
}

fn number(value: &str, max: u64) -> Option<u64> {
    value.parse().ok().filter(|n| *n <= max)
}

fn boolean(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Applies one setting; `None` means the value isn't valid for that key.
fn apply(opts: &mut Options, key: &str, value: &str) -> Option<Result<(), ()>> {
    match key {
        "tab_width" => {
            opts.tab_width = number(value, MAX_TAB_WIDTH as u64).filter(|n| *n > 0)? as usize
        }
        "indent" => {
            opts.default_indent = match value {
                "tabs" => Indent::Tabs,
                "spaces" => match opts.default_indent {
                    Indent::Spaces(n) => Indent::Spaces(n),
                    Indent::Tabs => Indent::Spaces(opts.indent_width),
                },
                _ => return None,
            }
        }
        "indent_width" => {
            let n = number(value, MAX_TAB_WIDTH as u64).filter(|n| *n > 0)? as usize;
            opts.indent_width = n;
            if let Indent::Spaces(_) = opts.default_indent {
                opts.default_indent = Indent::Spaces(n);
            }
        }
        "line_numbers" => opts.line_numbers = boolean(value)?,
        "soft_wrap" => opts.soft_wrap = boolean(value)?,
        "writing" => opts.writing = boolean(value)?,
        "spell_check" => opts.spell_check = boolean(value)?,
        "build_command" => opts.build_command = Some(value.to_string()),
        "pdf_command" => opts.pdf_command = Some(value.to_string()),
        "theme" => opts.theme = ThemeName::parse(value)?,
        "esc_ms" => opts.esc_ms = number(value, 10_000)?,
        "history" => opts.no_history = !boolean(value)?,
        "update_check" => opts.update_check = boolean(value)?,
        "history_limit_mb" => {
            opts.history_limit_mb = number(value, 1_000_000).filter(|n| *n > 0)?
        }
        _ => return Some(Err(())),
    }
    Some(Ok(()))
}

/// Reads settings into `opts` and returns what was wrong with the text.
pub fn parse_into(opts: &mut Options, text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.split(" #").next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            problems.push(format!("line {n}: expected key = value"));
            continue;
        };
        let (key, value) = (key.trim(), value.trim().trim_matches('"'));
        match apply(opts, key, value) {
            Some(Ok(())) => {}
            Some(Err(())) => problems.push(format!("line {n}: unknown setting {key}")),
            None => problems.push(format!("line {n}: bad value for {key}")),
        }
    }
    problems
}

/// The options the settings file gives. A missing file is fine; an unreadable
/// or partly wrong one leaves a warning for the first screen.
pub fn load() -> Options {
    let mut opts = Options::default();
    let path = path();
    match fs::read_to_string(&path) {
        Ok(text) => {
            let problems = parse_into(&mut opts, &text);
            if let Some(first) = problems.first() {
                let more = problems.len() - 1;
                let also = if more > 0 {
                    format!(" (and {more} more)")
                } else {
                    String::new()
                };
                opts.config_warning = Some(format!("Settings: {first}{also}"));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => opts.config_warning = Some(format!("Can't read settings: {e}")),
    }
    opts
}

/// Writes the template if there's no settings file, and returns its path.
pub fn ensure_exists() -> std::io::Result<PathBuf> {
    let path = path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, TEMPLATE)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> (Options, Vec<String>) {
        let mut o = Options::default();
        let p = parse_into(&mut o, text);
        (o, p)
    }

    #[test]
    fn the_template_changes_nothing_and_has_no_problems() {
        let (o, p) = parsed(TEMPLATE);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(o, Options::default());
    }

    #[test]
    fn every_setting_in_the_template_works_when_uncommented() {
        let on: String = TEMPLATE
            .lines()
            .map(|l| {
                l.strip_prefix("# ")
                    .filter(|r| r.contains(" = "))
                    .unwrap_or(l)
            })
            .map(|l| format!("{l}\n"))
            .collect();
        let (_, p) = parsed(&on);
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn settings_are_read() {
        let (o, p) = parsed(concat!(
            "tab_width = 8\n",
            "line_numbers = false\n",
            "soft_wrap = true\n",
            "esc_ms = 500\n",
            "history = false\n",
            "history_limit_mb = 20\n",
            "update_check = false\n",
        ));
        assert!(p.is_empty(), "{p:?}");
        assert!(!o.update_check);
        assert_eq!(o.tab_width, 8);
        assert!(!o.line_numbers);
        assert!(o.soft_wrap);
        assert_eq!(o.esc_ms, 500);
        assert!(o.no_history);
        assert_eq!(o.history_limit_mb, 20);
    }

    #[test]
    fn the_theme_setting_is_read_and_a_bad_one_is_reported() {
        let (o, p) = parsed("theme = \"daybreak\"\n");
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(o.theme, ThemeName::Daybreak);
        let (o, p) = parsed("theme = \"sepia\"\n");
        assert_eq!(p, vec!["line 1: bad value for theme".to_string()]);
        assert_eq!(o.theme, ThemeName::Signal);
    }

    #[test]
    fn indent_settings_work_in_either_order() {
        let (a, _) = parsed("indent = \"spaces\"\nindent_width = 2\n");
        assert_eq!(a.default_indent, Indent::Spaces(2));
        let (b, _) = parsed("indent_width = 2\nindent = \"spaces\"\n");
        assert_eq!(b.default_indent, Indent::Spaces(2));
        let (c, _) = parsed("indent_width = 2\n");
        assert_eq!(
            c.default_indent,
            Indent::Tabs,
            "width alone changes nothing"
        );
    }

    #[test]
    fn trailing_comments_and_quotes_are_fine() {
        let (o, p) = parsed("tab_width = 2 # narrow\n# full comment\n\n");
        assert!(p.is_empty());
        assert_eq!(o.tab_width, 2);
    }

    #[test]
    fn mistakes_are_reported_and_the_rest_still_applies() {
        let (o, p) = parsed("tabwidth = 2\ntab_width = lots\nnonsense\nesc_ms = 400\n");
        assert_eq!(o.esc_ms, 400);
        assert_eq!(o.tab_width, Options::default().tab_width);
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p[0].contains("line 1: unknown setting tabwidth"));
        assert!(p[1].contains("line 2: bad value for tab_width"));
        assert!(p[2].contains("line 3"));
    }

    #[test]
    fn out_of_range_values_are_refused() {
        for bad in [
            "tab_width = 0",
            "tab_width = 99",
            "esc_ms = 99999",
            "history_limit_mb = 0",
        ] {
            assert_eq!(parsed(bad).1.len(), 1, "{bad}");
        }
    }
}
