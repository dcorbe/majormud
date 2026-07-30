//! Raw key encoding for passthrough mode.
//!
//! The board has full-screen data-entry screens — `train stats` is the
//! one that bites, since `edit_character_stats` drives an FSD room — and
//! those are navigated with ANSI cursor keys. MBBSEmu's FSD reads
//! `\x1b[A` and friends directly, so nothing short of the real escape
//! sequence will move between fields.
//!
//! `mmc play`'s line editor owns the arrow keys for its own cursor and
//! history, which is right for typing commands and useless for an FSD
//! screen. Passthrough mode is the way out, and this is its encoder.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mud_client::tui::key_bytes;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn arrows_become_ansi_cursor_sequences() {
    assert_eq!(key_bytes(&key(KeyCode::Up)).unwrap(), b"\x1b[A");
    assert_eq!(key_bytes(&key(KeyCode::Down)).unwrap(), b"\x1b[B");
    assert_eq!(key_bytes(&key(KeyCode::Right)).unwrap(), b"\x1b[C");
    assert_eq!(key_bytes(&key(KeyCode::Left)).unwrap(), b"\x1b[D");
}

/// FSD fields are committed with a bare CR, not CRLF: the screen is not
/// line-oriented and a stray newline would be read as a second key.
#[test]
fn enter_is_a_bare_carriage_return() {
    assert_eq!(key_bytes(&key(KeyCode::Enter)).unwrap(), b"\r");
}

#[test]
fn ordinary_characters_go_through_as_themselves() {
    assert_eq!(key_bytes(&key(KeyCode::Char('7'))).unwrap(), b"7");
    assert_eq!(key_bytes(&key(KeyCode::Char('x'))).unwrap(), b"x");
}

#[test]
fn editing_and_navigation_keys_are_carried_too() {
    assert_eq!(key_bytes(&key(KeyCode::Backspace)).unwrap(), b"\x08");
    assert_eq!(key_bytes(&key(KeyCode::Tab)).unwrap(), b"\t");
    assert_eq!(key_bytes(&key(KeyCode::Esc)).unwrap(), b"\x1b");
    assert_eq!(key_bytes(&key(KeyCode::Home)).unwrap(), b"\x1b[H");
    assert_eq!(key_bytes(&key(KeyCode::End)).unwrap(), b"\x1b[F");
}

/// Ctrl-P is the mode toggle itself and must never reach the board, or
/// leaving passthrough would also send a stray byte into the field.
#[test]
fn the_mode_toggle_is_not_forwarded() {
    let toggle = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert_eq!(key_bytes(&toggle), None);
}
