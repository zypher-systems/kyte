//! The file tree: the directory Kyte started in, as a list of rows that fold.
//! A folder is read when it's opened, so a huge project costs nothing up front.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Rows kept at most, so unfolding everything can't stall the editor.
const MAX_ROWS: usize = 5000;
/// How far PageUp and PageDown move.
const PAGE: usize = 10;

/// One line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub open: bool,
}

/// State of the open tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTree {
    pub root: PathBuf,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub show_hidden: bool,
    open: BTreeSet<PathBuf>,
}

/// What's in `dir`: `(path, name, is_dir)`, folders first, then by name.
fn entries(dir: &Path, hidden: bool) -> std::io::Result<Vec<(PathBuf, String, bool)>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // Follows symlinks. A loop can only go as deep as someone unfolds it.
        let is_dir = fs::metadata(&path).is_ok_and(|m| m.is_dir());
        found.push((path, name, is_dir));
    }
    found.sort_by_key(|(_, name, is_dir)| (!is_dir, name.to_lowercase()));
    Ok(found)
}

impl FileTree {
    /// The tree of `root`, unfolded down to `reveal` if it's inside.
    pub fn new(root: PathBuf, reveal: Option<&Path>) -> FileTree {
        let mut open = BTreeSet::new();
        let inside = reveal.filter(|p| p.starts_with(&root));
        if let Some(path) = inside {
            let dirs = path.ancestors().skip(1);
            let dirs = dirs.take_while(|d| d.starts_with(&root) && *d != root);
            open.extend(dirs.map(Path::to_path_buf));
        }
        let mut tree = FileTree {
            root,
            rows: Vec::new(),
            selected: 0,
            show_hidden: false,
            open,
        };
        tree.rebuild();
        if let Some(i) = inside.and_then(|p| tree.rows.iter().position(|r| r.path == p)) {
            tree.selected = i;
        }
        tree
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        self.add_rows(&self.root, 0, &mut rows);
        self.rows = rows;
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn add_rows(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        for (path, name, is_dir) in entries(dir, self.show_hidden).unwrap_or_default() {
            if rows.len() >= MAX_ROWS {
                return;
            }
            let open = is_dir && self.open.contains(&path);
            rows.push(Row {
                path: path.clone(),
                name,
                depth,
                is_dir,
                open,
            });
            if open {
                self.add_rows(&path, depth + 1, rows);
            }
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn up(&mut self, by: usize) {
        self.selected = self.selected.saturating_sub(by);
    }

    pub fn down(&mut self, by: usize) {
        let last = self.rows.len().saturating_sub(1);
        self.selected = self.selected.saturating_add(by).min(last);
    }

    pub fn page_up(&mut self) {
        self.up(PAGE);
    }

    pub fn page_down(&mut self) {
        self.down(PAGE);
    }

    /// Opens the selected folder. Right on an open folder moves into it.
    pub fn unfold(&mut self) -> Result<(), String> {
        let Some(row) = self.selected_row().filter(|r| r.is_dir).cloned() else {
            return Ok(());
        };
        if row.open {
            self.down(1);
            return Ok(());
        }
        fs::read_dir(&row.path).map_err(|e| format!("Can't read {}: {e}", row.name))?;
        self.open.insert(row.path);
        self.rebuild();
        Ok(())
    }

    /// Closes the selected folder, or moves up to the folder holding this row.
    pub fn fold(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.open {
            self.open.remove(&row.path);
            self.rebuild();
        } else if let Some(i) = self.rows[..self.selected]
            .iter()
            .rposition(|r| r.depth < row.depth)
        {
            self.selected = i;
        }
    }

    pub fn toggle_hidden(&mut self) {
        let at = self.selected_row().map(|r| r.path.clone());
        self.show_hidden = !self.show_hidden;
        self.rebuild();
        if let Some(i) = at.and_then(|p| self.rows.iter().position(|r| r.path == p)) {
            self.selected = i;
        }
    }

    /// Moves to the next row starting with `c`, wrapping around.
    pub fn jump(&mut self, c: char) {
        let c = c.to_lowercase().to_string();
        let n = self.rows.len();
        let next = (1..=n)
            .map(|step| (self.selected + step) % n)
            .find(|&i| self.rows[i].name.to_lowercase().starts_with(&c));
        if let Some(i) = next {
            self.selected = i;
        }
    }

    /// First row to show in `rows` lines, keeping the selection in the middle.
    pub fn scroll_offset(&self, rows: usize) -> usize {
        let max = self.rows.len().saturating_sub(rows);
        self.selected.saturating_sub(rows / 2).min(max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for d in ["src/ui", "docs", ".git"] {
            fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        for f in ["src/ui/theme.rs", "src/app.rs", "README.md", ".env"] {
            fs::write(dir.path().join(f), "x").unwrap();
        }
        dir
    }

    fn names(t: &FileTree) -> Vec<String> {
        let row = |r: &Row| format!("{}{}", "  ".repeat(r.depth), r.name);
        t.rows.iter().map(row).collect()
    }

    fn tree(dir: &tempfile::TempDir) -> FileTree {
        FileTree::new(dir.path().to_path_buf(), None)
    }

    fn select(t: &mut FileTree, name: &str) {
        t.selected = t.rows.iter().position(|r| r.name == name).unwrap();
    }

    #[test]
    fn folders_come_first_and_hidden_files_are_left_out() {
        let dir = project();
        assert_eq!(names(&tree(&dir)), ["docs", "src", "README.md"]);
    }

    #[test]
    fn folders_fold_and_unfold() {
        let dir = project();
        let mut t = tree(&dir);
        select(&mut t, "src");
        t.unfold().unwrap();
        assert_eq!(names(&t), ["docs", "src", "  ui", "  app.rs", "README.md"]);
        t.unfold().unwrap();
        assert_eq!(t.selected_row().unwrap().name, "ui", "moves in");
        t.fold();
        assert_eq!(t.selected_row().unwrap().name, "src", "moves up");
        t.fold();
        assert_eq!(names(&t), ["docs", "src", "README.md"]);
    }

    #[test]
    fn it_opens_on_the_current_file() {
        let dir = project();
        let file = dir.path().join("src/ui/theme.rs");
        let t = FileTree::new(dir.path().to_path_buf(), Some(&file));
        assert_eq!(t.selected_row().unwrap().path, file);
        assert_eq!(t.selected_row().unwrap().depth, 2);
    }

    #[test]
    fn a_file_outside_the_tree_is_ignored() {
        let dir = project();
        let t = FileTree::new(dir.path().to_path_buf(), Some(Path::new("/etc/hosts")));
        assert_eq!(t.selected, 0);
        assert_eq!(names(&t), ["docs", "src", "README.md"]);
    }

    #[test]
    fn hidden_files_can_be_shown_and_the_selection_stays_put() {
        let dir = project();
        let mut t = tree(&dir);
        select(&mut t, "README.md");
        t.toggle_hidden();
        assert_eq!(names(&t), [".git", "docs", "src", ".env", "README.md"]);
        assert_eq!(t.selected_row().unwrap().name, "README.md");
        t.selected = 0;
        t.toggle_hidden();
        assert_eq!(t.selected_row().unwrap().name, "docs");
    }

    #[test]
    fn a_letter_jumps_to_the_next_name_and_wraps() {
        let dir = project();
        let mut t = tree(&dir);
        t.jump('R');
        assert_eq!(t.selected_row().unwrap().name, "README.md");
        t.jump('r');
        assert_eq!(t.selected_row().unwrap().name, "README.md");
        t.jump('z');
        assert_eq!(t.selected_row().unwrap().name, "README.md");
    }

    #[test]
    fn an_empty_tree_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = tree(&dir);
        t.down(1);
        t.up(1);
        t.fold();
        t.unfold().unwrap();
        t.jump('a');
        assert_eq!(t.scroll_offset(10), 0);
        assert!(t.selected_row().is_none());
    }

    #[test]
    fn the_selection_stays_on_screen() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..40 {
            fs::write(dir.path().join(format!("f{i:02}")), "").unwrap();
        }
        let mut t = tree(&dir);
        assert_eq!(t.scroll_offset(10), 0);
        t.down(20);
        assert_eq!(t.scroll_offset(10), 15);
        t.down(100);
        assert_eq!(t.selected, 39);
        assert_eq!(t.scroll_offset(10), 30);
        t.page_up();
        assert_eq!(t.selected, 29);
    }

    #[test]
    fn a_vanished_folder_is_reported_and_stays_folded() {
        let dir = project();
        let mut t = tree(&dir);
        select(&mut t, "docs");
        fs::remove_dir(dir.path().join("docs")).unwrap();
        assert!(t.unfold().unwrap_err().contains("docs"));
        assert!(!t.rows[0].open);
    }

    #[test]
    fn a_symlink_loop_only_goes_as_deep_as_its_unfolded() {
        let dir = project();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("docs/loop")).unwrap();
        let mut t = tree(&dir);
        select(&mut t, "docs");
        t.unfold().unwrap();
        select(&mut t, "loop");
        t.unfold().unwrap();
        assert!(t.rows.len() < 20, "{}", t.rows.len());
    }
}
