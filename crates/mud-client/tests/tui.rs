//! Pure-logic tests for the interactive client: line editor and status
//! bar rendering. The terminal loop itself is thin glue over these.

use mud_client::events::RoomView;
use mud_client::lost::Fix;
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
    let s = render_status(&state, "mbbs", None, Fix::Unknown, None, None, false, 80);
    assert!(s.contains("HP 35"));
    assert!(s.contains("MA 12"));
    assert!(s.contains("Newhaven, Village Entrance"));
    assert!(s.contains("mbbs"));

    // Width is respected (padded or truncated to exactly `width`).
    let narrow = render_status(&state, "mbbs", None, Fix::Unknown, None, None, false, 20);
    assert_eq!(narrow.chars().count(), 20);
    let wide = render_status(&state, "mbbs", None, Fix::Unknown, None, None, false, 120);
    assert_eq!(wide.chars().count(), 120);
}

#[test]
fn status_line_without_room_or_mana() {
    let state = GameState {
        hp: 10,
        mana: None,
        room: None,
    };
    let s = render_status(&state, "rust", None, Fix::Unknown, None, None, false, 80);
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
        also_here_sgr: Vec::new(),
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
    let s = render_status(&state, "mbbs", None, Fix::Unknown, None, None, false, 100);
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
    let s = render_status(&state, "mbbs", Some(&phase), Fix::Confirmed(RoomId { map: 1, room: 2156 }), None, None, false, 120);
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
        assert_eq!(render_status(&state, "mbbs", None, Fix::Unknown, None, None, false, w).chars().count(), w);
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
    let with = render_status(&state, "mbbs", None, Fix::Unknown, Some(255), None, false, 120);
    assert!(with.contains("255 xp/min"), "{with}");

    let without = render_status(&state, "mbbs", None, Fix::Unknown, None, None, false, 120);
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
    let s = render_status(&state, "mbbs", Some(&phase), Fix::Unknown, None, None, false, 120);
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
async fn ctrl_f_takes_the_keyboard_back_from_any_job() {
    let (addr, _) = capture_board().await;
    let session = session_to(addr).await;
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    assert!(matches!(
        handle_key(&ctrl('f'), &mut editor, &session, &mut passthrough, true),
        KeyOutcome::TakeOver
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
        elsewhere: false,
    }
}

fn peeked(event: Event) -> Correlated {
    Correlated {
        event,
        answers: Some(CmdId(1)),
        elsewhere: true,
    }
}

fn unsolicited(event: Event) -> Correlated {
    Correlated {
        event,
        answers: None,
        elsewhere: false,
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

/// A `look <direction>` block names the NEIGHBOUR's occupants. Feeding
/// it to the assist would attack a monster standing in the room next
/// door — the same class of bug Task 1 fixed for position tracking,
/// reachable because manual typing (including a directional look) is
/// never blocked while the assist runs.
#[test]
fn a_directional_look_starts_nothing() {
    let mut bot = assist_bot();
    let room = RoomView {
        name: "Slum Entrance".into(),
        also_here: vec!["guardsman".into()],
        ..RoomView::default()
    };
    assert_eq!(
        assist_actions(&mut bot, &peeked(Event::RoomSeen(room))),
        Vec::<String>::new()
    );
}

/// "done" alone hid WHY a run ended — a healthy character standing in
/// the Arena with a "done" bar and no explanation (live, cwgaming
/// 2026-08-01: the run had ended TooHurt on whiff-spent budget, and
/// nothing said so). The end now rides in the phase like Failed's why.
#[test]
fn the_done_phase_carries_the_reason() {
    let phase = mud_client::farm::Phase::Done {
        why: "too hurt: travel interrupt budget spent".into(),
        at: None,
    };
    assert_eq!(phase.label(), "done: too hurt: travel interrupt budget spent");
}

/// The status bar carries the level and how long until the next one, so
/// an operator can see whether a stop is worth staying at without doing
/// arithmetic on the xp/min figure beside it.
#[test]
fn the_bar_shows_the_level_and_the_time_to_the_next_one() {
    use mud_client::progress::LevelProgress;
    let state = GameState { hp: 43, mana: Some(10), room: None };
    let p = LevelProgress { exp: 57209, level: 3, needed: 7200 };
    let s = render_status(&state, "mbbs", None, Fix::Unknown, Some(100), Some(p), false, 120);
    assert!(s.contains("L3->4 1h12m"), "got {s:?}");
}

/// Nothing earned yet means no honest estimate, and the bar says so
/// rather than inventing one.
#[test]
fn the_bar_admits_when_it_cannot_estimate() {
    use mud_client::progress::LevelProgress;
    let state = GameState { hp: 43, mana: Some(10), room: None };
    let p = LevelProgress { exp: 1, level: 1, needed: 500 };
    let s = render_status(&state, "mbbs", None, Fix::Unknown, None, Some(p), false, 120);
    assert!(s.contains("L1->2 ?"), "got {s:?}");
}

/// A farm that ends on its own hands the character back to the assist,
/// which keeps fighting — and with nothing in the phase slot that is
/// indistinguishable from a farm still running. It cost an operator a
/// hunt for a Ctrl-F regression that did not exist (2026-08-01).
#[test]
fn the_bar_names_the_assist_when_it_is_driving() {
    let state = GameState { hp: 43, mana: Some(10), room: None };
    let s = render_status(&state, "mbbs", None, Fix::Unknown, None, None, true, 120);
    assert!(s.starts_with("assist | "), "got {s:?}");
}

/// A farm owns the connection outright while it runs, so its phase wins
/// the slot even with the assist configured on.
#[test]
fn a_running_farm_outranks_the_assist_in_the_bar() {
    let state = GameState { hp: 43, mana: Some(10), room: None };
    let phase = mud_client::farm::Phase::Done { why: "loops walked".into(), at: None };
    let s = render_status(&state, "mbbs", Some(&phase), Fix::Unknown, None, None, true, 120);
    assert!(!s.contains("assist"), "got {s:?}");
}

// --- slash commands --------------------------------------------------
//
// The client claims a handful of verbs off the input line. Two things
// have to be true and neither was asserted before `/go` existed: what
// the client claims must NOT reach the board, and what it does not
// claim must.

use mud_client::tui::slash;

#[test]
fn go_carries_its_target_verbatim() {
    assert_eq!(
        slash("/go Grungy Shop"),
        Some(KeyOutcome::Go {
            target: "Grungy Shop".into()
        })
    );
    assert_eq!(
        slash("  /go   1/2324  "),
        Some(KeyOutcome::Go {
            target: "1/2324".into()
        }),
        "surrounding whitespace is the terminal's, not the operator's"
    );
}

#[test]
fn go_without_a_target_is_refused_locally() {
    assert!(matches!(slash("/go"), Some(KeyOutcome::Refuse(_))));
    assert!(matches!(slash("/go   "), Some(KeyOutcome::Refuse(_))));
}

/// `/where` takes no argument — the whole question is about the room the
/// character is standing in, and there is nothing else to ask it about.
#[test]
fn where_takes_no_argument() {
    assert_eq!(slash("/where"), Some(KeyOutcome::Where));
    assert_eq!(slash("  /where   "), Some(KeyOutcome::Where));
}

#[test]
fn the_known_verbs_are_claimed() {
    assert_eq!(slash("/quit"), Some(KeyOutcome::Quit));
    assert_eq!(
        slash("/farm"),
        Some(KeyOutcome::StartFarm { loop_name: None })
    );
    assert_eq!(slash("/bot"), Some(KeyOutcome::ToggleAssist));
}

/// Unlike `/go`, a bare `/room` is not a mistake: the room you are
/// standing in is the one you ask about most.
#[test]
fn room_defaults_to_where_you_stand() {
    assert_eq!(slash("/room"), Some(KeyOutcome::Room { target: None }));
    assert_eq!(slash("/room   "), Some(KeyOutcome::Room { target: None }));
    assert_eq!(
        slash("/room 1/2156"),
        Some(KeyOutcome::Room {
            target: Some("1/2156".into())
        })
    );
    assert_eq!(
        slash("  /room   Small Cavern  "),
        Some(KeyOutcome::Room {
            target: Some("Small Cavern".into())
        })
    );
}

/// A named loop from the library replaces the profile's circuit; a bare
/// `/farm` still walks the profile's own.
#[test]
fn farm_takes_an_optional_loop_name() {
    assert_eq!(
        slash("/farm cavebear"),
        Some(KeyOutcome::StartFarm {
            loop_name: Some("cavebear".into())
        })
    );
    assert_eq!(slash("/loop"), Some(KeyOutcome::Loops { name: None }));
    assert_eq!(
        slash("/loop slum-sweep"),
        Some(KeyOutcome::Loops {
            name: Some("slum-sweep".into())
        })
    );
}

/// `/map` takes the same optional argument with the same meaning, and
/// resolves it through the same `go::resolve`.
#[test]
fn map_takes_the_same_argument_as_room() {
    assert_eq!(slash("/map"), Some(KeyOutcome::Map { target: None }));
    assert_eq!(
        slash("/map 1/1076"),
        Some(KeyOutcome::Map {
            target: Some("1/1076".into())
        })
    );
}

/// Deliberate: the board says unknown commands out loud rather than
/// erroring, so swallowing every slash-prefixed line would silently eat
/// board syntax nobody has audited.
#[test]
fn an_unknown_slash_command_still_reaches_the_board() {
    assert_eq!(slash("/who"), None);
    assert_eq!(slash("/gossip hi"), None);
    assert_eq!(slash("north"), None);
    assert_eq!(slash(""), None);
}

/// The absence assertion that matters, with a positive control: a bare
/// "the board heard nothing" would pass just as well against a broken
/// socket.
#[tokio::test]
async fn slash_commands_are_intercepted_not_sent_to_the_board() {
    let (addr, received) = capture_board().await;
    let session = session_to(addr).await;
    let mut editor = InputEditor::new();
    let mut passthrough = false;

    for line in ["/bot", "/go Grungy Shop", "/go", "/farm"] {
        for c in line.chars() {
            handle_key(&key(KeyCode::Char(c)), &mut editor, &session, &mut passthrough, false);
        }
        handle_key(&key(KeyCode::Enter), &mut editor, &session, &mut passthrough, false);
    }
    // The positive control: one line the client does NOT claim.
    for c in "gossip hi".chars() {
        handle_key(&key(KeyCode::Char(c)), &mut editor, &session, &mut passthrough, false);
    }
    handle_key(&key(KeyCode::Enter), &mut editor, &session, &mut passthrough, false);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if received.lock().unwrap().iter().any(|l| l == "gossip hi") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "board never heard the control line");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let log = received.lock().unwrap().clone();
    assert_eq!(log, vec!["gossip hi"], "only the unclaimed line goes out");
}

/// `/go` reuses the farm's phase channel and therefore its status bar;
/// this pins that there is not a second bar to keep in step.
#[test]
fn the_bar_shows_where_a_go_is_walking() {
    let state = GameState::default();
    let phase = mud_client::farm::Phase::Travelling {
        to: RoomId {
            map: 1,
            room: 2324,
        },
    };
    let bar = render_status(&state, "mbbs", Some(&phase), Fix::Unknown, None, None, false, 100);
    assert!(bar.contains("1/2324"), "{bar}");
}
