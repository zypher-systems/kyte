//! Command palette: a small subsequence scorer and the palette state.

use std::sync::Arc;

use crate::command::{Command, REGISTRY};
use crate::ui::theme::ThemeName;

/// Rows of results the popup shows at once.
pub const VISIBLE_ROWS: usize = 8;
/// Results kept for the universal palette.
const MAX_RESULTS: usize = 200;

/// Case-insensitive subsequence score of `query` against `candidate`.
/// Higher is better; `None` when `query` is not a subsequence. Matches at the
/// start, at word starts and consecutive matches score extra. An empty query
/// scores 0 for everything.
pub fn score(query: &str, candidate: &str) -> Option<i64> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let c: Vec<char> = candidate.to_lowercase().chars().collect();
    let mut total = 0i64;
    let mut ci = 0usize;
    let mut prev: Option<usize> = None;
    for qc in q {
        let pos = (ci..c.len()).find(|&i| c[i] == qc)?;
        total += 1;
        if pos == 0 {
            total += 10;
        } else if !c[pos - 1].is_alphanumeric() {
            total += 3;
        }
        if prev.is_some_and(|p| p + 1 == pos) {
            total += 5;
        }
        prev = Some(pos);
        ci = pos + 1;
    }
    Some(total)
}

/// Registry commands matching `query`, best first; ties keep registry order.
pub fn filter(query: &str) -> Vec<Command> {
    let mut scored: Vec<(i64, usize, Command)> = REGISTRY
        .iter()
        .enumerate()
        .filter_map(|(i, c)| score(query, c.label()).map(|s| (s, i, *c)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, c)| c).collect()
}

/// One row of the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Command(Command),
    /// A file, relative to the working directory.
    File(String),
    /// `:N` in the universal palette.
    Line(usize),
    /// `/text` in the universal palette.
    Find(String),
    /// A row of the theme picker.
    Theme(ThemeName),
}

impl Entry {
    pub fn label(&self) -> String {
        match self {
            Entry::Command(c) => c.label().to_string(),
            Entry::File(p) => p.clone(),
            Entry::Line(n) => format!("Go to line {n}"),
            Entry::Find(q) => format!("Find {q}"),
            Entry::Theme(t) => t.label().to_string(),
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            Entry::Command(c) => c.hint(),
            Entry::File(_) => "file",
            Entry::Line(_) | Entry::Find(_) => "",
            Entry::Theme(t) if t.is_light() => "light",
            Entry::Theme(_) => "dark",
        }
    }
}

/// A file's score: the better of its name and its whole path.
fn file_score(query: &str, path: &str) -> Option<i64> {
    let name = path.rsplit('/').next().unwrap_or(path);
    score(query, name).max(score(query, path))
}

/// Every entry for `query`: `:N` and `/text` are direct, and anything else
/// searches commands and files together. Commands win ties, because they're
/// the shorter list and the more common aim.
fn universal_filter(query: &str, files: &[String]) -> Vec<Entry> {
    if let Some(rest) = query.strip_prefix(':') {
        return crate::command::parse_line_number(rest)
            .map(|n| vec![Entry::Line(n)])
            .unwrap_or_default();
    }
    if let Some(rest) = query.strip_prefix('/') {
        return if rest.is_empty() {
            Vec::new()
        } else {
            vec![Entry::Find(rest.to_string())]
        };
    }
    let mut scored: Vec<(i64, usize, Entry)> = REGISTRY
        .iter()
        .enumerate()
        .filter_map(|(i, c)| score(query, c.label()).map(|s| (s, i, Entry::Command(*c))))
        .collect();
    let base = scored.len() + REGISTRY.len();
    scored.extend(
        files.iter().enumerate().filter_map(|(i, f)| {
            file_score(query, f).map(|s| (s, base + i, Entry::File(f.clone())))
        }),
    );
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(MAX_RESULTS);
    scored.into_iter().map(|(_, _, e)| e).collect()
}

/// State of the open palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
    pub results: Vec<Entry>,
    source: Source,
}

/// What the palette lists.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Commands,
    /// The universal palette, with its file list.
    Universal(Arc<Vec<String>>),
    /// The theme picker, with the theme to go back to on Esc.
    Themes {
        before: ThemeName,
    },
}

impl Default for PaletteState {
    fn default() -> Self {
        PaletteState {
            query: String::new(),
            selected: 0,
            results: filter("").into_iter().map(Entry::Command).collect(),
            source: Source::Commands,
        }
    }
}

impl PaletteState {
    /// The Ctrl-K palette over `files`.
    pub fn universal(files: Vec<String>) -> PaletteState {
        let mut state = PaletteState {
            source: Source::Universal(Arc::new(files)),
            ..PaletteState::default()
        };
        state.set_query(String::new());
        state
    }

    /// The theme picker, starting on `current`.
    pub fn themes(current: ThemeName) -> PaletteState {
        let mut state = PaletteState {
            source: Source::Themes { before: current },
            ..PaletteState::default()
        };
        state.set_query(String::new());
        state.selected = ThemeName::ALL
            .iter()
            .position(|t| *t == current)
            .unwrap_or(0);
        state
    }

    pub fn is_universal(&self) -> bool {
        matches!(self.source, Source::Universal(_))
    }

    /// The theme the picker started from.
    pub fn theme_before(&self) -> Option<ThemeName> {
        match self.source {
            Source::Themes { before } => Some(before),
            _ => None,
        }
    }

    /// The theme on the selected row, to show while choosing.
    pub fn theme_shown(&self) -> Option<ThemeName> {
        match self.selected_entry() {
            Some(Entry::Theme(t)) => Some(*t),
            _ => None,
        }
    }

    pub fn set_query(&mut self, q: String) {
        self.query = q;
        self.results = match &self.source {
            Source::Universal(files) => universal_filter(&self.query, files),
            Source::Commands => filter(&self.query)
                .into_iter()
                .map(Entry::Command)
                .collect(),
            Source::Themes { .. } => ThemeName::ALL
                .into_iter()
                .filter(|t| score(&self.query, t.label()).is_some())
                .map(Entry::Theme)
                .collect(),
        };
        self.selected = 0;
    }

    pub fn push(&mut self, c: char) {
        let mut q = self.query.clone();
        q.push(c);
        self.set_query(q);
    }

    pub fn pop(&mut self) {
        let mut q = self.query.clone();
        q.pop();
        self.set_query(q);
    }

    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self) {
        if self.selected + 1 < self.results.len() {
            self.selected += 1;
        }
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.results.get(self.selected)
    }

    pub fn selected_command(&self) -> Option<Command> {
        match self.selected_entry() {
            Some(Entry::Command(c)) => Some(*c),
            _ => None,
        }
    }

    /// First result row shown, so the selection stays visible.
    pub fn scroll_offset(&self) -> usize {
        (self.selected + 1).saturating_sub(VISIBLE_ROWS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_keeps_registry_order() {
        assert_eq!(filter(""), REGISTRY.to_vec());
        assert_eq!(PaletteState::default().results.len(), 23);
    }

    #[test]
    fn the_theme_picker_starts_on_the_current_theme_and_filters() {
        let mut s = PaletteState::themes(ThemeName::Ember);
        assert_eq!(s.results.len(), ThemeName::ALL.len());
        assert_eq!(s.theme_shown(), Some(ThemeName::Ember));
        assert_eq!(s.theme_before(), Some(ThemeName::Ember));
        typed(&mut s, "mos");
        assert_eq!(s.results, vec![Entry::Theme(ThemeName::Moss)]);
        assert_eq!(s.theme_shown(), Some(ThemeName::Moss));
        typed(&mut s, "zz");
        assert_eq!(s.theme_shown(), None);
        assert_eq!(s.theme_before(), Some(ThemeName::Ember));
    }

    #[test]
    fn only_the_light_theme_says_so() {
        let hint = |t| Entry::Theme(t).hint();
        assert_eq!(hint(ThemeName::Daybreak), "light");
        assert_eq!(hint(ThemeName::Signal), "dark");
    }

    fn universal(files: &[&str]) -> PaletteState {
        PaletteState::universal(files.iter().map(|f| f.to_string()).collect())
    }

    fn typed(s: &mut PaletteState, text: &str) {
        for c in text.chars() {
            s.push(c);
        }
    }

    #[test]
    fn universal_lists_commands_then_files() {
        let s = universal(&["src/app.rs", "README.md"]);
        assert!(s.is_universal());
        assert_eq!(s.results[0], Entry::Command(Command::Save));
        assert_eq!(s.results.len(), REGISTRY.len() + 2);
        assert_eq!(
            s.results.last(),
            Some(&Entry::File("README.md".to_string()))
        );
        assert!(!PaletteState::default().is_universal());
    }

    #[test]
    fn universal_finds_files_by_name_and_path() {
        let mut s = universal(&["docs/guide.md", "src/ui/theme.rs", "src/app.rs"]);
        typed(&mut s, "theme");
        assert_eq!(
            s.selected_entry(),
            Some(&Entry::File("src/ui/theme.rs".to_string()))
        );
        s.set_query("src/ap".to_string());
        assert_eq!(
            s.selected_entry(),
            Some(&Entry::File("src/app.rs".to_string()))
        );
    }

    #[test]
    fn universal_prefers_a_command_on_a_tie() {
        let mut s = universal(&["save"]);
        typed(&mut s, "save");
        assert_eq!(s.selected_command(), Some(Command::Save));
    }

    #[test]
    fn colon_goes_to_a_line_and_slash_finds_text() {
        let mut s = universal(&["a.txt"]);
        typed(&mut s, ":42");
        assert_eq!(s.results, vec![Entry::Line(42)]);
        s.set_query(":x".to_string());
        assert!(s.results.is_empty());
        s.set_query("/needle in".to_string());
        assert_eq!(s.results, vec![Entry::Find("needle in".to_string())]);
        s.set_query("/".to_string());
        assert!(s.results.is_empty());
    }

    #[test]
    fn command_palette_ignores_colon_and_files() {
        let mut s = PaletteState::default();
        typed(&mut s, ":4");
        assert!(s.results.is_empty());
    }

    #[test]
    fn universal_results_are_capped() {
        let files: Vec<String> = (0..1000).map(|i| format!("f{i}")).collect();
        let mut s = PaletteState::universal(files);
        typed(&mut s, "f");
        assert_eq!(s.results.len(), MAX_RESULTS);
    }

    #[test]
    fn prefix_beats_scattered() {
        assert_eq!(filter("sav")[0], Command::Save);
        assert!(score("sav", "Save").unwrap() > score("sav", "Quit without saving").unwrap());
        assert_eq!(
            filter("sav"),
            vec![Command::Save, Command::SaveAs, Command::QuitWithoutSaving]
        );
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(score("SAVE", "Save"), score("save", "save"));
        assert_eq!(filter("FIND")[0], Command::Find);
    }

    #[test]
    fn goto_variants_put_go_to_line_first() {
        assert_eq!(filter("goto")[0], Command::GotoLine);
        assert_eq!(filter("go to")[0], Command::GotoLine);
        assert_eq!(filter("gl")[0], Command::GotoLine);
    }

    #[test]
    fn non_subsequence_is_filtered_out() {
        assert_eq!(score("xyz", "Save"), None);
        assert!(filter("zzz").is_empty());
    }

    #[test]
    fn ties_keep_registry_order() {
        // "u" scores the same in Quit and Undo positions? Undo starts with u.
        let r = filter("q");
        assert_eq!(r, vec![Command::Quit, Command::QuitWithoutSaving]);
    }

    #[test]
    fn state_navigation_and_scrolling() {
        let mut s = PaletteState::default();
        s.up();
        assert_eq!(s.selected, 0);
        for _ in 0..40 {
            s.down();
        }
        assert_eq!(s.selected, 22);
        assert_eq!(s.selected_command(), Some(Command::ShowKeyHelp));
        assert_eq!(s.scroll_offset(), 15);
        s.push('s');
        assert_eq!(s.selected, 0);
        assert_eq!(s.selected_command(), Some(Command::Save));
        s.pop();
        assert_eq!(s.results.len(), 23);
    }
}
