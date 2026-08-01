//! Pure-logic tests for the interactive client: line editor and status
//! bar rendering. The terminal loop itself is thin glue over these.

use mud_client::events::RoomView;
use mud_client::session::GameState;
use mud_client::tui::{InputEditor, render_status};
use mud_core::content::RoomId;

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
    let s = render_status(&state, "mbbs", None, None, None, 80);
    assert!(s.contains("HP 35"));
    assert!(s.contains("MA 12"));
    assert!(s.contains("Newhaven, Village Entrance"));
    assert!(s.contains("mbbs"));

    // Width is respected (padded or truncated to exactly `width`).
    let narrow = render_status(&state, "mbbs", None, None, None, 20);
    assert_eq!(narrow.chars().count(), 20);
    let wide = render_status(&state, "mbbs", None, None, None, 120);
    assert_eq!(wide.chars().count(), 120);
}

#[test]
fn status_line_without_room_or_mana() {
    let state = GameState {
        hp: 10,
        mana: None,
        room: None,
    };
    let s = render_status(&state, "rust", None, None, None, 80);
    assert!(s.contains("HP 10"));
    assert!(!s.contains("MA "));
    assert_eq!(s.chars().count(), 80);
}

// --- one status line for both commands --------------------------------
//
// `play` and `farm` showed different bars built by different code, so the
// same session looked different depending on which command you had
// started it from. One renderer, with the farm-only parts optional.

fn a_room(name: &str) -> RoomView {
    RoomView {
        name: name.into(),
        exits: vec![],
        also_here: vec![],
        items: vec![],
    }
}

/// Interactive play has no runner, so no activity and no room id: the
/// bar degrades to what it always showed.
#[test]
fn without_a_runner_the_bar_is_hp_room_and_target() {
    let state = GameState {
        hp: 33,
        mana: Some(8),
        room: Some(a_room("Newhaven, Narrow Road")),
    };
    let s = render_status(&state, "mbbs", None, None, None, 100);
    assert!(s.contains("HP 33"), "{s}");
    assert!(s.contains("MA 8"), "{s}");
    assert!(s.contains("Newhaven, Narrow Road"), "{s}");
    assert!(s.contains("mbbs"), "{s}");
}

/// With a runner attached the same bar gains what it is doing and the
/// room NUMBER, which the board never prints.
#[test]
fn with_a_runner_the_bar_gains_activity_and_room_number() {
    let state = GameState {
        hp: 23,
        mana: Some(8),
        room: Some(a_room("Small Cavern")),
    };
    let phase = mud_client::farm::Phase::Fighting {
        at: RoomId {
            map: 1,
            room: 2156,
        },
        target: "cave bear".into(),
    };
    let s = render_status(&state, "mbbs", Some(&phase), Some(RoomId { map: 1, room: 2156 }), None, 120);
    assert!(s.contains("attacking cave bear"), "{s}");
    assert!(s.contains("HP 23"), "{s}");
    assert!(s.contains("Small Cavern"), "{s}");
    assert!(s.contains("1/2156"), "{s}");
}

/// Still exactly `width` characters, whichever form it takes -- the bar
/// is painted into a fixed row and a long room name must not wrap it.
#[test]
fn the_bar_is_always_exactly_the_width_asked_for() {
    let state = GameState {
        hp: 5,
        mana: None,
        room: Some(a_room("A Room With A Very Long Name Indeed That Runs On")),
    };
    for w in [20usize, 80, 120] {
        assert_eq!(render_status(&state, "mbbs", None, None, None, w).chars().count(), w);
    }
}

/// The number that says whether a circuit is worth running. Absent until
/// there is enough elapsed time for it to mean anything.
#[test]
fn the_bar_shows_the_experience_rate_when_there_is_one() {
    let state = GameState {
        hp: 27,
        mana: Some(8),
        room: Some(a_room("Small Cavern")),
    };
    let with = render_status(&state, "mbbs", None, None, Some(255), 120);
    assert!(with.contains("255 xp/min"), "{with}");

    let without = render_status(&state, "mbbs", None, None, None, 120);
    assert!(!without.contains("xp/min"), "{without}");
}

/// The bar is ONE row. An error carried into it brings a NavError's
/// multi-line `tail:` with it, and a newline written into a fixed row
/// scrolls the terminal — which reads as the whole screen flashing.
/// Observed live with `Phase::Failed`.
#[test]
fn the_bar_never_contains_a_control_character() {
    let state = GameState {
        hp: 1,
        mana: None,
        room: Some(a_room("Somewhere")),
    };
    let phase = mud_client::farm::Phase::Failed {
        why: "at 1/2152: timed out waiting for \"room block after movement\"; tail:\n\n  look\n"
            .into(),
    };
    let s = render_status(&state, "mbbs", Some(&phase), None, None, 120);
    assert!(
        !s.chars().any(|c| c.is_control()),
        "a control character in a fixed-row bar wrecks the display: {s:?}"
    );
    assert_eq!(s.chars().count(), 120);
}

// ---------------------------------------------------------------------
// The keyboard during a farm run. The runner's beliefs are protected by
// attribution — a typed command's answer is attributed to the typed
// command, never to a farm step or look — so the operator may speak
// while the runner drives. What the farming branch must still own is
// Ctrl-F, the take-over.
// ---------------------------------------------------------------------

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mud_client::tui::{KeyOutcome, handle_key};

async fn capture_board() -> (
    std::net::SocketAddr,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&received);
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                log.lock().unwrap().push(line.trim().to_string());
            }
        }
    });
    (addr, received)
}

async fn session_to(addr: std::net::SocketAddr) -> mud_client::session::Session {
    let profile = mud_client::profile::Profile {
        target: mud_client::dialect::Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "testuser".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        disable_evil_warnings: false,
        bot: None,
        farm: None,
    };
    mud_client::session::Session::connect(&profile, None)
        .await
        .unwrap()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[tokio::test]
async fn typing_reaches_the_board_during_a_farm_run() {
    let (addr, received) = capture_board().await;
    let session = session_to(addr).await;
    let mut editor = InputEditor::new();
    let mut passthrough = false;

    for c in "gossip hello".chars() {
        handle_key(&key(KeyCode::Char(c)), &mut editor, &session, &mut passthrough, true);
    }
    assert_eq!(editor.line(), "gossip hello", "keys must reach the editor while farming");
    handle_key(&key(KeyCode::Enter), &mut editor, &session, &mut passthrough, true);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if received.lock().unwrap().iter().any(|l| l == "gossip hello") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "typed line never reached the board: {:?}",
            received.lock().unwrap()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn ctrl_f_still_takes_the_keyboard_back() {
    let (addr, _) = capture_board().await;
    let session = session_to(addr).await;
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    assert!(matches!(
        handle_key(&ctrl('f'), &mut editor, &session, &mut passthrough, true),
        KeyOutcome::StopFarm
    ));
    assert!(matches!(
        handle_key(&ctrl('q'), &mut editor, &session, &mut passthrough, true),
        KeyOutcome::Quit
    ));
}

// ---------------------------------------------------------------------
// The assist's reply to one correlated event. The farm's pump re-looks
// after every fight; playing by hand, the assist must poke its own —
// a fight's end says nothing about who else is standing in the room,
// and without the poke the assist killed one monster and stopped
// (live, cwgaming board, 2026-08-01).
// ---------------------------------------------------------------------

use mud_client::bot::{Bot, BotConfig};
use mud_client::correlate::{CmdId, Correlated};
use mud_client::events::Event;
use mud_client::tui::assist_actions;

fn assist_bot() -> Bot {
    Bot::new(BotConfig {
        auto_combat: true,
        ..BotConfig::default()
    })
}

fn attributed(event: Event) -> Correlated {
    Correlated {
        event,
        answers: Some(CmdId(1)),
    }
}

fn unsolicited(event: Event) -> Correlated {
    Correlated {
        event,
        answers: None,
    }
}

#[test]
fn a_fight_ending_pokes_a_look_so_the_next_monster_is_seen() {
    let mut bot = assist_bot();
    let room = RoomView {
        name: "Dungeon, Entrance".into(),
        also_here: vec!["giant rat".into(), "kobold thief".into()],
        ..RoomView::default()
    };
    assert_eq!(
        assist_actions(&mut bot, &attributed(Event::RoomSeen(room.clone()))),
        vec!["a rat".to_string()]
    );
    assert_eq!(
        assist_actions(
            &mut bot,
            &unsolicited(Event::Line(
                "The giant rat falls to the ground with a tortured squeak.".into()
            ))
        ),
        Vec::<String>::new()
    );
    // The board's own fight-over announcement is the poke's trigger.
    assert_eq!(
        assist_actions(&mut bot, &unsolicited(Event::Line("*Combat Off*".into()))),
        vec!["look".to_string()]
    );
    // The poke's answer names the survivor and the assist engages it.
    let survivor = RoomView {
        name: "Dungeon, Entrance".into(),
        also_here: vec!["kobold thief".into()],
        ..RoomView::default()
    };
    assert_eq!(
        assist_actions(&mut bot, &attributed(Event::RoomSeen(survivor))),
        vec!["a thief".to_string()]
    );
}

/// The runner's own believing rule: an unattributed block — somebody
/// else's render, a stale answer — must not start a swing.
#[test]
fn an_unattributed_block_starts_nothing() {
    let mut bot = assist_bot();
    let room = RoomView {
        name: "Dungeon, Entrance".into(),
        also_here: vec!["giant rat".into()],
        ..RoomView::default()
    };
    assert_eq!(
        assist_actions(&mut bot, &unsolicited(Event::RoomSeen(room))),
        Vec::<String>::new()
    );
}
