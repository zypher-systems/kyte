//! Property tests for the buffer (task T2): any sequence of edits and moves
//! can be fully undone to the original text and fully redone to the final one.

use kyte::buffer::{Buffer, GroupKind};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Type(String),
    Paste(String),
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    MoveTo(usize),
}

fn piece() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            "a", "b", " ", "\n", "é", "e\u{301}", "世", "🙂", "\t", "\r",
        ]),
        1..4,
    )
    .prop_map(|v| v.concat())
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => piece().prop_map(Op::Type),
        1 => piece().prop_map(Op::Paste),
        2 => Just(Op::Backspace),
        1 => Just(Op::Delete),
        1 => Just(Op::Left),
        1 => Just(Op::Right),
        1 => Just(Op::Up),
        1 => Just(Op::Down),
        1 => Just(Op::Home),
        1 => Just(Op::End),
        1 => (0usize..40).prop_map(Op::MoveTo),
    ]
}

fn run(buf: &mut Buffer, op: &Op) {
    match op {
        Op::Type(s) => {
            for c in s.chars() {
                buf.insert_str(&c.to_string(), GroupKind::Typing);
            }
        }
        Op::Paste(s) => buf.insert_str(s, GroupKind::Paste),
        Op::Backspace => buf.backspace(),
        Op::Delete => buf.delete_forward(),
        Op::Left => buf.move_left(),
        Op::Right => buf.move_right(),
        Op::Up => buf.move_up(),
        Op::Down => buf.move_down(),
        Op::Home => buf.move_home(),
        Op::End => buf.move_end(),
        Op::MoveTo(i) => buf.move_to(*i),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn undo_all_restores_original_and_redo_all_restores_final(
        start in "[ab\n世 ]{0,12}",
        ops in prop::collection::vec(op(), 0..40),
    ) {
        let mut buf = Buffer::from_str(&start, None);
        for o in &ops {
            run(&mut buf, o);
            prop_assert!(buf.cursor() <= buf.len_chars());
        }
        let final_text = buf.to_string();
        while buf.undo() {}
        prop_assert_eq!(buf.to_string(), start.clone());
        while buf.redo() {}
        prop_assert_eq!(buf.to_string(), final_text);
    }

    #[test]
    fn pending_edit_log_replays_to_current_text(
        start in "[ab\n ]{0,8}",
        ops in prop::collection::vec(op(), 0..30),
        undos in 0usize..5,
    ) {
        let mut buf = Buffer::from_str(&start, None);
        for o in &ops {
            run(&mut buf, o);
        }
        for _ in 0..undos {
            buf.undo();
        }
        let mut rope = ropey::Rope::from_str(&start);
        for e in buf.take_pending_edits() {
            let n = e.removed.chars().count();
            prop_assert_eq!(rope.slice(e.at..e.at + n).to_string(), e.removed.clone());
            rope.remove(e.at..e.at + n);
            rope.insert(e.at, &e.inserted);
        }
        prop_assert_eq!(rope.to_string(), buf.to_string());
    }
}
