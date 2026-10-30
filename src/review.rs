//! Reviewing changes another program made to an open file. It's a three-way
//! comparison: what the file was (the base), the buffer (ours), and what the
//! file is now (theirs). Comparing only ours and theirs would show the user's
//! own unsaved edits as changes to undo.

use std::ops::Range;

use crate::diff::{diff_lines, Hunk};

/// One change to review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The lines in the buffer that it replaces.
    pub ours: Range<usize>,
    /// The lines from the file that would replace them.
    pub theirs: Range<usize>,
    /// The user changed these lines too.
    pub conflict: bool,
}

/// Lines with their endings, so joining them gives the text back.
pub fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn touches(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start <= b.end && b.start <= a.end
}

fn growth(h: &Hunk) -> isize {
    h.b.len() as isize - h.a.len() as isize
}

/// The changes from `base` to `theirs`, placed in `ours`, in order. Changes
/// that touch the user's own edits are grouped with them and marked as
/// conflicts. A change the buffer already has is left out.
pub fn changes(base: &str, ours: &str, theirs: &str) -> Vec<Change> {
    let (b, o, t) = (lines(base), lines(ours), lines(theirs));
    let mine = diff_lines(&b, &o);
    let outside = diff_lines(&b, &t);
    let mut out = Vec::new();
    let mut i = 0;
    while i < outside.len() {
        let mut region = outside[i].a.clone();
        i += 1;
        // Grow the region until nothing else touches it.
        loop {
            let mut grew = false;
            for m in &mine {
                if touches(&m.a, &region) {
                    let wider = region.start.min(m.a.start)..region.end.max(m.a.end);
                    grew |= wider != region;
                    region = wider;
                }
            }
            while i < outside.len() && touches(&outside[i].a, &region) {
                region = region.start.min(outside[i].a.start)..region.end.max(outside[i].a.end);
                i += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        let shift_to = |hunks: &[Hunk]| {
            let before: isize = hunks
                .iter()
                .filter(|h| h.a.end <= region.start && !touches(&h.a, &region))
                .map(growth)
                .sum();
            let inside: isize = hunks
                .iter()
                .filter(|h| touches(&h.a, &region))
                .map(growth)
                .sum();
            let start = region.start as isize + before;
            let end = region.end as isize + before + inside;
            start as usize..end as usize
        };
        let conflict = mine.iter().any(|m| touches(&m.a, &region));
        let ours_range = shift_to(&mine);
        let theirs_range = shift_to(&outside[..i]);
        if o[ours_range.clone()] != t[theirs_range.clone()] {
            out.push(Change {
                ours: ours_range,
                theirs: theirs_range,
                conflict,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// `ours` with the changes for which `take` says yes applied.
    fn apply(ours: &str, theirs: &str, list: &[Change], take: impl Fn(usize) -> bool) -> String {
        let (o, t) = (lines(ours), lines(theirs));
        let mut out = String::new();
        let mut pos = 0;
        for (i, c) in list.iter().enumerate() {
            out.push_str(&o[pos..c.ours.start].concat());
            if take(i) {
                out.push_str(&t[c.theirs.clone()].concat());
            } else {
                out.push_str(&o[c.ours.clone()].concat());
            }
            pos = c.ours.end;
        }
        out.push_str(&o[pos..].concat());
        out
    }

    #[test]
    fn with_no_edits_of_our_own_it_is_a_plain_diff() {
        let list = changes("a\nb\nc\n", "a\nb\nc\n", "a\nX\nc\n");
        assert_eq!(
            list,
            vec![Change {
                ours: 1..2,
                theirs: 1..2,
                conflict: false
            }]
        );
    }

    #[test]
    fn our_edits_are_not_shown_as_changes() {
        let base = "a\nb\nc\nd\ne\n";
        let ours = "MINE\na\nb\nc\nd\ne\n";
        assert!(changes(base, ours, base).is_empty());
        let theirs = "a\nb\nc\nd\nTHEIRS\n";
        let list = changes(base, ours, theirs);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ours, 5..6, "moved down by our added line");
        assert_eq!(list[0].theirs, 4..5);
        assert!(!list[0].conflict);
        assert_eq!(
            apply(ours, theirs, &list, |_| true),
            "MINE\na\nb\nc\nd\nTHEIRS\n",
            "both survive"
        );
    }

    #[test]
    fn the_same_lines_changed_on_both_sides_is_a_conflict() {
        let base = "a\nb\nc\n";
        let list = changes(base, "a\nMINE\nc\n", "a\nTHEIRS\nc\n");
        assert_eq!(list.len(), 1);
        assert!(list[0].conflict);
        assert_eq!(
            apply("a\nMINE\nc\n", "a\nTHEIRS\nc\n", &list, |_| true),
            "a\nTHEIRS\nc\n"
        );
    }

    #[test]
    fn a_change_we_already_have_is_left_out() {
        assert!(changes("a\nb\n", "a\nX\n", "a\nX\n").is_empty());
    }

    #[test]
    fn text_with_no_final_newline_works() {
        let list = changes("a\nb", "a\nb", "a\nc");
        assert_eq!(apply("a\nb", "a\nc", &list, |_| true), "a\nc");
    }

    #[test]
    fn keeping_ours_changes_nothing() {
        let (base, ours, theirs) = ("1\n2\n3\n4\n5\n", "1\n2\n3\n4\nM\n", "T\n2\n3\n4\n5\n");
        let list = changes(base, ours, theirs);
        assert_eq!(apply(ours, theirs, &list, |_| false), ours);
    }

    proptest! {
        #[test]
        fn accepting_everything_on_a_clean_buffer_gives_the_file(
            base in proptest::collection::vec(proptest::sample::select(vec!["a\n", "b\n", "c\n"]), 0..25),
            theirs in proptest::collection::vec(proptest::sample::select(vec!["a\n", "b\n", "c\n"]), 0..25),
        ) {
            let (base, theirs) = (base.concat(), theirs.concat());
            let list = changes(&base, &base, &theirs);
            prop_assert_eq!(apply(&base, &theirs, &list, |_| true), theirs);
        }

        #[test]
        fn three_way_ranges_stay_in_bounds_and_in_order(
            base in proptest::collection::vec(proptest::sample::select(vec!["a\n", "b\n", "c\n"]), 0..25),
            ours in proptest::collection::vec(proptest::sample::select(vec!["a\n", "b\n", "c\n"]), 0..25),
            theirs in proptest::collection::vec(proptest::sample::select(vec!["a\n", "b\n", "c\n"]), 0..25),
        ) {
            let (base, ours, theirs) = (base.concat(), ours.concat(), theirs.concat());
            let list = changes(&base, &ours, &theirs);
            let (o, t) = (lines(&ours).len(), lines(&theirs).len());
            let mut last = 0;
            for c in &list {
                prop_assert!(c.ours.start >= last && c.ours.start <= c.ours.end && c.ours.end <= o);
                prop_assert!(c.theirs.start <= c.theirs.end && c.theirs.end <= t);
                last = c.ours.end;
            }
            prop_assert_eq!(apply(&ours, &theirs, &list, |_| false), ours);
        }
    }
}
