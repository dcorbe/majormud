//! Pure-logic tests for the interactive client: line editor and status
//! bar rendering. The terminal loop itself is thin glue over these.

use mud_client::events::RoomView;
use mud_client::session::GameState;
use mud_client::tui::{InputEditor, render_status};

#[test]
fn editor_inserts_and_takes_line() {
    let mut e = InputEditor::new();
    for c in "north".chars() {
        e.insert(c);
    }
    assert_eq!(e.line(), "north");
    assert_eq!(e.cursor(), 5);
    assert_eq!(e.take_line(), "north");
    assert_eq!(e.line(), "");
    assert_eq!(e.cursor(), 0);
}

#[test]
fn editor_backspace_and_cursor_movement() {
    let mut e = InputEditor::new();
    for c in "lok".chars() {
        e.insert(c);
    }
    e.left();
    e.backspace(); // remove the 'o' before cursor: "lk", cursor 1
    assert_eq!(e.line(), "lk");
    assert_eq!(e.cursor(), 1);
    e.insert('o');
    e.insert('o');
    assert_eq!(e.line(), "look");
    e.home();
    assert_eq!(e.cursor(), 0);
    e.backspace(); // no-op at start
    assert_eq!(e.line(), "look");
    e.end();
    assert_eq!(e.cursor(), 4);
    e.right(); // no-op at end
    assert_eq!(e.cursor(), 4);
}

#[test]
fn editor_history_recall() {
    let mut e = InputEditor::new();
    for c in "north".chars() {
        e.insert(c);
    }
    assert_eq!(e.take_line(), "north");
    for c in "south".chars() {
        e.insert(c);
    }
    assert_eq!(e.take_line(), "south");

    e.history_prev();
    assert_eq!(e.line(), "south");
    e.history_prev();
    assert_eq!(e.line(), "north");
    e.history_prev(); // clamped at oldest
    assert_eq!(e.line(), "north");
    e.history_next();
    assert_eq!(e.line(), "south");
    e.history_next(); // back past newest -> empty draft
    assert_eq!(e.line(), "");
}

#[test]
fn editor_history_preserves_draft() {
    let mut e = InputEditor::new();
    for c in "attack".chars() {
        e.insert(c);
    }
    assert_eq!(e.take_line(), "attack");
    for c in "dra".chars() {
        e.insert(c);
    }
    e.history_prev();
    assert_eq!(e.line(), "attack");
    e.history_next();
    assert_eq!(e.line(), "dra", "draft restored");
}

#[test]
fn status_line_shows_hp_room_and_fits_width() {
    let state = GameState {
        hp: 35,
        mana: Some(12),
        room: Some(RoomView {
            name: "Newhaven, Village Entrance".into(),
            ..Default::default()
        }),
    };
    let s = render_status(&state, "mbbs", 80);
    assert!(s.contains("HP 35"));
    assert!(s.contains("MA 12"));
    assert!(s.contains("Newhaven, Village Entrance"));
    assert!(s.contains("mbbs"));

    // Width is respected (padded or truncated to exactly `width`).
    let narrow = render_status(&state, "mbbs", 20);
    assert_eq!(narrow.chars().count(), 20);
    let wide = render_status(&state, "mbbs", 120);
    assert_eq!(wide.chars().count(), 120);
}

#[test]
fn status_line_without_room_or_mana() {
    let state = GameState {
        hp: 10,
        mana: None,
        room: None,
    };
    let s = render_status(&state, "rust", 80);
    assert!(s.contains("HP 10"));
    assert!(!s.contains("MA "));
    assert_eq!(s.chars().count(), 80);
}
