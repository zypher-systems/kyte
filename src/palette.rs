//! Command palette: a small subsequence scorer and the palette state.

use crate::command::{Command, REGISTRY};

/// Rows of results the popup shows at once.
pub const VISIBLE_ROWS: usize = 8;

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
        } else if c[pos - 1] == ' ' {
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

/// State of the open palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
    pub results: Vec<Command>,
}

impl Default for PaletteState {
    fn default() -> Self {
        PaletteState {
            query: String::new(),
            selected: 0,
            results: filter(""),
        }
    }
}

impl PaletteState {
    pub fn set_query(&mut self, q: String) {
        self.query = q;
        self.results = filter(&self.query);
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

    pub fn selected_command(&self) -> Option<Command> {
        self.results.get(self.selected).copied()
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
        assert_eq!(PaletteState::default().results.len(), 9);
    }

    #[test]
    fn prefix_beats_scattered() {
        assert_eq!(filter("sav")[0], Command::Save);
        assert!(score("sav", "Save").unwrap() > score("sav", "Quit without saving").unwrap());
        assert_eq!(
            filter("sav"),
            vec![Command::Save, Command::QuitWithoutSaving]
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
        for _ in 0..20 {
            s.down();
        }
        assert_eq!(s.selected, 8);
        assert_eq!(s.selected_command(), Some(Command::ShowKeyHelp));
        assert_eq!(s.scroll_offset(), 1);
        s.push('s');
        assert_eq!(s.selected, 0);
        assert_eq!(s.selected_command(), Some(Command::Save));
        s.pop();
        assert_eq!(s.results.len(), 9);
    }
}
