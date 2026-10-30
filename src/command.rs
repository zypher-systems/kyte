//! Palette commands (section 5.4) and the go-to-line parser.

/// Every command the MVP palette offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Save,
    SaveAs,
    Quit,
    QuitWithoutSaving,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    GotoLine,
    ToggleLineNumbers,
    ShowKeyHelp,
}

/// Registry order is the palette order for an empty query and the tie-breaker.
pub const REGISTRY: &[Command] = &[
    Command::Save,
    Command::SaveAs,
    Command::Quit,
    Command::QuitWithoutSaving,
    Command::Undo,
    Command::Redo,
    Command::Cut,
    Command::Copy,
    Command::Paste,
    Command::SelectAll,
    Command::Find,
    Command::GotoLine,
    Command::ToggleLineNumbers,
    Command::ShowKeyHelp,
];

impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Command::Save => "Save",
            Command::SaveAs => "Save as",
            Command::Quit => "Quit",
            Command::QuitWithoutSaving => "Quit without saving",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Cut => "Cut",
            Command::Copy => "Copy",
            Command::Paste => "Paste",
            Command::SelectAll => "Select all",
            Command::Find => "Find",
            Command::GotoLine => "Go to line",
            Command::ToggleLineNumbers => "Toggle line numbers",
            Command::ShowKeyHelp => "Show key help",
        }
    }

    /// Key hint shown next to the command in the palette.
    pub fn hint(self) -> &'static str {
        match self {
            Command::Save => "Ctrl-S",
            Command::SaveAs => "",
            Command::Quit => "Ctrl-Q",
            Command::QuitWithoutSaving => "",
            Command::Undo => "Ctrl-Z",
            Command::Redo => "Ctrl-Y",
            Command::Cut => "Ctrl-X",
            Command::Copy => "Ctrl-C",
            Command::Paste => "Ctrl-V",
            Command::SelectAll => "Ctrl-A",
            Command::Find => "Ctrl-F",
            Command::GotoLine => "",
            Command::ToggleLineNumbers => "",
            Command::ShowKeyHelp => "",
        }
    }
}

/// Parses go-to-line input: a positive 1-based line number (0 counts as 1).
pub fn parse_line_number(text: &str) -> Result<usize, String> {
    let t = text.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("Not a line number: '{t}'"));
    }
    // Absurdly long numbers saturate (and are clamped to the last line).
    Ok(t.parse::<usize>().unwrap_or(usize::MAX).max(1))
}

/// The key table shown by "Show key help".
pub const KEY_HELP: &[(&str, &str)] = &[
    ("Typing, Enter, Tab", "insert at the cursor"),
    ("Backspace, Delete", "delete previous / next character"),
    ("Arrows, Home, End", "move"),
    ("Ctrl-Left, Ctrl-Right", "move by word"),
    ("Ctrl-Bksp, Ctrl-Del", "delete word before / after"),
    ("PgUp, PgDn", "move by a screen"),
    ("Ctrl-Home, Ctrl-End", "start / end of file"),
    ("Shift + any move, Ctrl-A", "select, select all"),
    ("Ctrl-X, Ctrl-C, Ctrl-V", "cut, copy, paste"),
    ("Ctrl-S (Ctrl-O)", "save"),
    ("Ctrl-Q", "quit (asks if unsaved)"),
    ("Ctrl-Z, Ctrl-Y", "undo, redo"),
    ("Ctrl-F", "search"),
    ("Ctrl-P", "command palette"),
    ("Esc, Ctrl-Space, Alt-key", "operator layer:"),
    ("  w  d  k", "del word, del line, del to end"),
    ("  %  /", "matching bracket, search"),
    ("  u  r  s", "undo, redo, save"),
    ("  Esc (quickly)", "command palette"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_label() {
        assert_eq!(REGISTRY.len(), 14);
        let labels: Vec<&str> = REGISTRY.iter().map(|c| c.label()).collect();
        assert_eq!(
            labels,
            vec![
                "Save",
                "Save as",
                "Quit",
                "Quit without saving",
                "Undo",
                "Redo",
                "Cut",
                "Copy",
                "Paste",
                "Select all",
                "Find",
                "Go to line",
                "Toggle line numbers",
                "Show key help"
            ]
        );
    }

    #[test]
    fn line_numbers() {
        assert_eq!(parse_line_number("9"), Ok(9));
        assert_eq!(parse_line_number(" 12 "), Ok(12));
        assert_eq!(parse_line_number("0"), Ok(1));
        assert_eq!(parse_line_number("99999999999999999999999"), Ok(usize::MAX));
        assert!(parse_line_number("abc").is_err());
        assert!(parse_line_number("").is_err());
        assert!(parse_line_number("-3").is_err());
        assert!(parse_line_number("3x").is_err());
    }
}
