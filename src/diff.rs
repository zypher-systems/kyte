//! Line diff: Myers' algorithm, after trimming what the two sides share at
//! each end. Past a limit it gives up and reports one big change, so a file
//! that was rewritten can't stall the editor.

use std::ops::Range;

/// Lines `a` in the old text became lines `b` in the new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub a: Range<usize>,
    pub b: Range<usize>,
}

/// How many differing lines to search through before giving up.
const MAX_EDITS: usize = 4096;

/// Which lines of `a` were removed and which of `b` were added.
fn myers(a: &[&str], b: &[&str]) -> Option<(Vec<bool>, Vec<bool>)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m).min(MAX_EDITS as isize);
    let offset = max + 1;
    let mut v = vec![0isize; 2 * offset as usize + 1];
    // Each entry is v as it was before step d, for k in -d..=d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let at = |k: isize| (k + offset) as usize;
    for d in 0..=max {
        trace.push(v[at(-d - 1)..=at(d + 1)].to_vec());
        for k in (-d..=d).step_by(2) {
            let down = k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]);
            let mut x = if down { v[at(k + 1)] } else { v[at(k - 1)] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                return Some(walk_back(&trace, a.len(), b.len()));
            }
        }
    }
    None
}

fn walk_back(trace: &[Vec<isize>], n: usize, m: usize) -> (Vec<bool>, Vec<bool>) {
    let mut removed = vec![false; n];
    let mut added = vec![false; m];
    let (mut x, mut y) = (n as isize, m as isize);
    for d in (1..trace.len() as isize).rev() {
        let v = &trace[d as usize];
        let get = |k: isize| v[(k + d + 1) as usize];
        let k = x - y;
        let down = k == -d || (k != d && get(k - 1) < get(k + 1));
        let prev_k = if down { k + 1 } else { k - 1 };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        if down {
            added[prev_y as usize] = true;
        } else {
            removed[prev_x as usize] = true;
        }
        x = prev_x;
        y = prev_y;
    }
    (removed, added)
}

/// The changes from `a` to `b`, in order.
pub fn diff_lines(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let limit = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(limit)
        .take_while(|(x, y)| x == y)
        .count();
    let (mid_a, mid_b) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    if mid_a.is_empty() && mid_b.is_empty() {
        return Vec::new();
    }
    let whole = || {
        vec![Hunk {
            a: prefix..a.len() - suffix,
            b: prefix..b.len() - suffix,
        }]
    };
    if mid_a.is_empty() || mid_b.is_empty() {
        return whole();
    }
    let Some((removed, added)) = myers(mid_a, mid_b) else {
        return whole();
    };
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < removed.len() || j < added.len() {
        let is_removed = |i: usize| removed.get(i).copied().unwrap_or(false);
        let is_added = |j: usize| added.get(j).copied().unwrap_or(false);
        if !is_removed(i) && !is_added(j) {
            i += 1;
            j += 1;
            continue;
        }
        let (si, sj) = (i, j);
        while is_removed(i) {
            i += 1;
        }
        while is_added(j) {
            j += 1;
        }
        hunks.push(Hunk {
            a: prefix + si..prefix + i,
            b: prefix + sj..prefix + j,
        });
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn apply(a: &[&str], b: &[&str], hunks: &[Hunk]) -> Vec<String> {
        let mut out = Vec::new();
        let mut pos = 0;
        for h in hunks {
            out.extend(a[pos..h.a.start].iter().map(|s| s.to_string()));
            out.extend(b[h.b.clone()].iter().map(|s| s.to_string()));
            pos = h.a.end;
        }
        out.extend(a[pos..].iter().map(|s| s.to_string()));
        out
    }

    #[test]
    fn same_text_has_no_hunks() {
        assert!(diff_lines(&["a", "b"], &["a", "b"]).is_empty());
        assert!(diff_lines(&[], &[]).is_empty());
    }

    #[test]
    fn one_changed_line_is_one_hunk() {
        let h = diff_lines(&["a", "b", "c"], &["a", "X", "c"]);
        assert_eq!(h, vec![Hunk { a: 1..2, b: 1..2 }]);
    }

    #[test]
    fn additions_and_removals() {
        assert_eq!(
            diff_lines(&["a", "c"], &["a", "b", "c"]),
            vec![Hunk { a: 1..1, b: 1..2 }]
        );
        assert_eq!(
            diff_lines(&["a", "b", "c"], &["a", "c"]),
            vec![Hunk { a: 1..2, b: 1..1 }]
        );
        assert_eq!(diff_lines(&[], &["a"]), vec![Hunk { a: 0..0, b: 0..1 }]);
    }

    #[test]
    fn separate_changes_stay_separate() {
        let a = ["1", "2", "3", "4", "5"];
        let b = ["1", "X", "3", "Y", "5"];
        assert_eq!(diff_lines(&a, &b).len(), 2);
    }

    #[test]
    fn a_full_rewrite_past_the_limit_is_one_hunk() {
        let a: Vec<String> = (0..6000).map(|i| format!("a{i}")).collect();
        let b: Vec<String> = (0..6000).map(|i| format!("b{i}")).collect();
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        let b: Vec<&str> = b.iter().map(String::as_str).collect();
        assert_eq!(diff_lines(&a, &b).len(), 1);
    }

    proptest! {
        #[test]
        fn applying_the_hunks_gives_the_new_text(
            a in proptest::collection::vec(proptest::sample::select(vec!["a", "b", "c", "d"]), 0..30),
            b in proptest::collection::vec(proptest::sample::select(vec!["a", "b", "c", "d"]), 0..30),
        ) {
            let hunks = diff_lines(&a, &b);
            prop_assert_eq!(apply(&a, &b, &hunks), b.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        }
    }
}
