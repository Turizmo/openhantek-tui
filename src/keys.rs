//! Key bindings of the logger: one table from keys to actions, for the
//! keys that work everywhere and the keys of the log view. The help
//! screen and the README are made from the same table.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::logger::view::Move;

/// What a logger key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKey {
    /// Start or stop recording (everywhere).
    Record,
    /// Switch between the scope and the log view (everywhere).
    ToggleView,
    /// Scroll or zoom the log view.
    Go(Move),
    /// Time cursors on / off.
    Cursors,
    /// Move cursor A (0) or B (1) by dot columns.
    Cursor(usize, i64),
    /// Time labels: clock / seconds since start.
    Axis,
    /// Export the log as CSV (between the cursors when they are on).
    Export,
}

/// Keys that work in every view.
pub const GLOBAL: [(KeyCode, LogKey); 2] = [
    (KeyCode::Char('r'), LogKey::Record),
    (KeyCode::Char('V'), LogKey::ToggleView),
];

/// Keys of the log view. The other keys work as in the scope view
/// (volts/div, positions, channels on/off, quit, help, settings).
pub const LOG_VIEW: [(KeyCode, LogKey); 19] = [
    (KeyCode::Left, LogKey::Go(Move::Divs(-1))),
    (KeyCode::Right, LogKey::Go(Move::Divs(1))),
    (KeyCode::PageUp, LogKey::Go(Move::Pages(-1))),
    (KeyCode::PageDown, LogKey::Go(Move::Pages(1))),
    (KeyCode::Home, LogKey::Go(Move::Home)),
    (KeyCode::End, LogKey::Go(Move::End)),
    (KeyCode::Char('+'), LogKey::Go(Move::ZoomIn)),
    (KeyCode::Char('='), LogKey::Go(Move::ZoomIn)),
    (KeyCode::Char('-'), LogKey::Go(Move::ZoomOut)),
    (KeyCode::Char('F'), LogKey::Go(Move::Fit)),
    (KeyCode::Char('n'), LogKey::Go(Move::Native)),
    (KeyCode::Char('v'), LogKey::Cursors),
    (KeyCode::Char('h'), LogKey::Cursor(0, -1)),
    (KeyCode::Char('l'), LogKey::Cursor(0, 1)),
    (KeyCode::Char('H'), LogKey::Cursor(1, -1)),
    (KeyCode::Char('L'), LogKey::Cursor(1, 1)),
    (KeyCode::Char('t'), LogKey::Axis),
    (KeyCode::Char('E'), LogKey::Export),
    (KeyCode::Char('e'), LogKey::Export),
];

fn find(table: &[(KeyCode, LogKey)], key: &KeyEvent) -> Option<LogKey> {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    table.iter().find(|(k, _)| *k == key.code).map(|(_, a)| *a)
}

/// The logger action of a key: global keys first, then (in the log view)
/// the log view keys.
pub fn log_key(key: &KeyEvent, log_view: bool) -> Option<LogKey> {
    find(&GLOBAL, key).or_else(|| if log_view { find(&LOG_VIEW, key) } else { None })
}

/// The log view keys for the help screen and the README.
pub const LOG_HELP: [(&str, &str); 13] = [
    ("r", "start / stop recording (in every view)"),
    ("V", "log view on / off (in every view)"),
    ("← →", "scroll one division back / on"),
    ("PgUp PgDn", "scroll one screen"),
    ("Home End", "start of the log / newest (follow)"),
    ("+ -", "zoom in / out"),
    ("F n", "whole log / one step per dot column"),
    ("v", "time cursors on / off"),
    ("h l", "move cursor A"),
    ("H L", "move cursor B"),
    ("t", "time labels: clock / seconds"),
    ("E", "export CSV (A–B when the cursors are on)"),
    (
        "↑↓ [ ] 1 2",
        "volts/div, position, channels as in the scope",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_actions() {
        let k = |c| KeyEvent::from(KeyCode::Char(c));
        assert_eq!(log_key(&k('r'), false), Some(LogKey::Record));
        assert_eq!(log_key(&k('V'), true), Some(LogKey::ToggleView));
        assert_eq!(log_key(&k('h'), false), None);
        assert_eq!(log_key(&k('h'), true), Some(LogKey::Cursor(0, -1)));
        let ctrl = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(log_key(&ctrl, true), None);
        // No key twice.
        let all: Vec<KeyCode> = GLOBAL.iter().chain(&LOG_VIEW).map(|x| x.0).collect();
        for (i, a) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(a), "{a:?} twice");
        }
    }
}
