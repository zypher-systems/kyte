//! Command palette state (fuzzy filtering arrives with task T8).

/// State of the open palette.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
}
