//! Maps crossterm events to Kyte keys (section 5.3). Kyte adds no byte-level
//! parsing of its own: crossterm's parser decides what a lone Esc is.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// A key as Kyte understands it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Esc,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    CtrlHome,
    CtrlEnd,
    /// Lowercase letter; Ctrl-Space is `Ctrl(' ')`.
    Ctrl(char),
    Alt(char),
    F(u8),
}

/// One unit of input for the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Key(Key),
    Paste(String),
    /// (columns, rows)
    Resize(u16, u16),
}

/// `None` for events Kyte ignores (key release, mouse, focus, unmapped keys).
pub fn map_event(ev: Event) -> Option<Input> {
    match ev {
        Event::Key(k) => map_key(k).map(Input::Key),
        Event::Paste(s) => Some(Input::Paste(normalize_paste(&s))),
        Event::Resize(cols, rows) => Some(Input::Resize(cols, rows)),
        _ => None,
    }
}

fn map_key(k: KeyEvent) -> Option<Key> {
    if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    // Kitty-protocol terminals can report these; Kyte binds none of them.
    let unbound = KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META;
    if k.modifiers.intersects(unbound) {
        return None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let key = match k.code {
        KeyCode::Char(_) if ctrl && alt => return None,
        KeyCode::Char(c) if ctrl => Key::Ctrl(c.to_ascii_lowercase()),
        KeyCode::Char(c) if alt => Key::Alt(c),
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Esc => Key::Esc,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home if ctrl => Key::CtrlHome,
        KeyCode::End if ctrl => Key::CtrlEnd,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    Some(key)
}

/// Terminals send pasted line breaks as CR; store them as LF.
fn normalize_paste(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// `Alt(c)` in Insert mode becomes `[Esc, Char(c)]`; every other key passes through.
pub fn normalize(key: Key, mode_is_insert: bool) -> Vec<Key> {
    match key {
        Key::Alt(c) if mode_is_insert => vec![Key::Esc, Key::Char(c)],
        other => vec![other],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn super_chords_do_nothing() {
        for m in [KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META] {
            for code in [KeyCode::Char('s'), KeyCode::Enter, KeyCode::Left] {
                let ev = Event::Key(KeyEvent::new(code, m));
                assert_eq!(map_event(ev), None);
            }
        }
    }
    use ratatui::crossterm::event::{KeyEventState, MouseEvent, MouseEventKind};

    fn ev(code: KeyCode, m: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, m))
    }
    fn key(code: KeyCode, m: KeyModifiers) -> Option<Input> {
        map_event(ev(code, m))
    }
    const N: KeyModifiers = KeyModifiers::NONE;
    const C: KeyModifiers = KeyModifiers::CONTROL;
    const A: KeyModifiers = KeyModifiers::ALT;

    #[test]
    fn plain_keys() {
        let k = |c| key(c, N).unwrap();
        assert_eq!(k(KeyCode::Char('a')), Input::Key(Key::Char('a')));
        assert_eq!(k(KeyCode::Enter), Input::Key(Key::Enter));
        assert_eq!(k(KeyCode::Tab), Input::Key(Key::Tab));
        assert_eq!(k(KeyCode::Backspace), Input::Key(Key::Backspace));
        assert_eq!(k(KeyCode::Delete), Input::Key(Key::Delete));
        assert_eq!(k(KeyCode::Esc), Input::Key(Key::Esc));
        assert_eq!(k(KeyCode::Left), Input::Key(Key::Left));
        assert_eq!(k(KeyCode::Right), Input::Key(Key::Right));
        assert_eq!(k(KeyCode::Up), Input::Key(Key::Up));
        assert_eq!(k(KeyCode::Down), Input::Key(Key::Down));
        assert_eq!(k(KeyCode::Home), Input::Key(Key::Home));
        assert_eq!(k(KeyCode::End), Input::Key(Key::End));
        assert_eq!(k(KeyCode::PageUp), Input::Key(Key::PageUp));
        assert_eq!(k(KeyCode::PageDown), Input::Key(Key::PageDown));
        assert_eq!(k(KeyCode::F(5)), Input::Key(Key::F(5)));
    }

    #[test]
    fn shifted_char_stays_char() {
        assert_eq!(
            key(KeyCode::Char('A'), KeyModifiers::SHIFT),
            Some(Input::Key(Key::Char('A')))
        );
    }

    #[test]
    fn ctrl_keys() {
        assert_eq!(key(KeyCode::Char('s'), C), Some(Input::Key(Key::Ctrl('s'))));
        assert_eq!(key(KeyCode::Char('Q'), C), Some(Input::Key(Key::Ctrl('q'))));
        assert_eq!(key(KeyCode::Char(' '), C), Some(Input::Key(Key::Ctrl(' '))));
        assert_eq!(key(KeyCode::Home, C), Some(Input::Key(Key::CtrlHome)));
        assert_eq!(key(KeyCode::End, C), Some(Input::Key(Key::CtrlEnd)));
    }

    #[test]
    fn alt_keys() {
        assert_eq!(key(KeyCode::Char('d'), A), Some(Input::Key(Key::Alt('d'))));
        assert_eq!(key(KeyCode::Char('d'), C | A), None);
    }

    #[test]
    fn release_ignored_repeat_kept() {
        let mut k = KeyEvent::new(KeyCode::Char('a'), N);
        k.kind = KeyEventKind::Release;
        assert_eq!(map_event(Event::Key(k)), None);
        k.kind = KeyEventKind::Repeat;
        k.state = KeyEventState::NONE;
        assert_eq!(map_event(Event::Key(k)), Some(Input::Key(Key::Char('a'))));
    }

    #[test]
    fn paste_resize_and_ignored_events() {
        assert_eq!(
            map_event(Event::Paste("a\r\nb\rc\nd".into())),
            Some(Input::Paste("a\nb\nc\nd".into()))
        );
        assert_eq!(
            map_event(Event::Resize(40, 10)),
            Some(Input::Resize(40, 10))
        );
        assert_eq!(map_event(Event::FocusGained), None);
        let m = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: N,
        };
        assert_eq!(map_event(Event::Mouse(m)), None);
        assert_eq!(key(KeyCode::Insert, N), None);
    }

    #[test]
    fn normalize_alt_in_insert_only() {
        assert_eq!(
            normalize(Key::Alt('d'), true),
            vec![Key::Esc, Key::Char('d')]
        );
        assert_eq!(normalize(Key::Alt('d'), false), vec![Key::Alt('d')]);
        assert_eq!(normalize(Key::Char('x'), true), vec![Key::Char('x')]);
        assert_eq!(normalize(Key::Esc, true), vec![Key::Esc]);
    }
}
