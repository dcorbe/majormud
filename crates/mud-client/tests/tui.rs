//! Pure-logic tests for the interactive client: line editor and status
//! bar rendering. The terminal loop itself is thin glue over these.

use mud_client::correlate::Correlated;
use mud_client::events::{Actor, Event, RoomView, Status};
use mud_client::lost::Fix;
use mud_client::session::GameState;
use mud_client::sheet::{Casting, HealChoice, HealState, Spellbook};
use mud_client::tui::{InputEditor, assist_heal, help_text, render_status};
use mud_client::world::{TickClock, ROUND};
use mud_core::content::RoomId;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let s = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 80);
    assert!(s.contains("HP 35"));
    assert!(s.contains("MA 12"));
    assert!(s.contains("Newhaven, Village Entrance"));
    assert!(s.contains("mbbs"));

    // Width is respected (padded or truncated to exactly `width`).
    let narrow = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 20);
    assert_eq!(narrow.chars().count(), 20);
    let wide = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 120);
    assert_eq!(wide.chars().count(), 120);
}

#[test]
fn status_line_without_room_or_mana() {
    let state = GameState {
        hp: 10,
        mana: None,
        room: None,
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let s = render_status(&state, Instant::now(), "rust", None, Fix::Unknown, None, None, false, 80);
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
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let s = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 100);
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
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let phase = mud_client::farm::Phase::Fighting {
        at: RoomId {
            map: 1,
            room: 2156,
        },
        target: "cave bear".into(),
    };
    let s = render_status(&state, Instant::now(), "mbbs", Some(&phase), Fix::Confirmed(RoomId { map: 1, room: 2156 }), None, None, false, 120);
    assert!(s.contains("attacking cave bear"), "{s}");
    assert!(s.contains("HP 23"), "{s}");
    assert!(s.contains("Small Cavern"), "{s}");
    assert!(s.contains("1/2156"), "{s}");
}

/// A stale fix still shows the room id -- better than nothing -- but the
/// trailing `?` is the whole reason the type reaches the bar at all: the
/// operator must be able to see the client has lost the thread before
/// trusting it as a walk's origin.
#[test]
fn a_stale_fix_shows_the_room_with_a_question_mark() {
    let state = GameState {
        hp: 23,
        mana: Some(8),
        room: Some(a_room("Small Cavern")),
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let stale = render_status(&state, Instant::now(), "mbbs", None, Fix::Stale(RoomId { map: 1, room: 2156 }), None, None, false, 120);
    assert!(stale.contains("1/2156?"), "{stale}");

    let confirmed = render_status(&state, Instant::now(), "mbbs", None, Fix::Confirmed(RoomId { map: 1, room: 2156 }), None, None, false, 120);
    assert!(confirmed.contains("1/2156]"), "{confirmed}");
    assert!(!confirmed.contains("1/2156?"), "{confirmed}");
}

/// Still exactly `width` characters, whichever form it takes -- the bar
/// is painted into a fixed row and a long room name must not wrap it.
#[test]
fn the_bar_is_always_exactly_the_width_asked_for() {
    let state = GameState {
        hp: 5,
        mana: None,
        room: Some(a_room("A Room With A Very Long Name Indeed That Runs On")),
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    for w in [20usize, 80, 120] {
        assert_eq!(render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, w).chars().count(), w);
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
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let with = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, Some(255), None, false, 120);
    assert!(with.contains("255 xp/hr"), "{with}");

    let without = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 120);
    assert!(!without.contains("xp/hr"), "{without}");
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
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let phase = mud_client::farm::Phase::Failed {
        why: "at 1/2152: timed out waiting for \"room block after movement\"; tail:\n\n  look\n"
            .into(),
    };
    let s = render_status(&state, Instant::now(), "mbbs", Some(&phase), Fix::Unknown, None, None, false, 120);
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
        bank: Default::default(),
        ..Default::default()
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

#[test]
fn a_typed_line_comes_back_as_send_even_during_a_farm_run() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    for c in "gossip hello".chars() {
        assert_eq!(
            handle_key(&key(KeyCode::Char(c)), &mut editor, &mut passthrough, true),
            KeyOutcome::Continue
        );
    }
    assert_eq!(editor.line(), "gossip hello", "keys must reach the editor while farming");
    assert_eq!(
        handle_key(&key(KeyCode::Enter), &mut editor, &mut passthrough, true),
        KeyOutcome::Send("gossip hello".into())
    );
    assert_eq!(editor.line(), "");
}

#[test]
fn ctrl_f_takes_the_keyboard_back_from_any_job() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    assert_eq!(handle_key(&ctrl('f'), &mut editor, &mut passthrough, true), KeyOutcome::TakeOver);
    assert_eq!(handle_key(&ctrl('q'), &mut editor, &mut passthrough, true), KeyOutcome::QuitNow);
}

#[test]
fn slash_commands_are_claimed_and_everything_else_is_sent() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    let mut submit = |line: &str| {
        for c in line.chars() {
            handle_key(&key(KeyCode::Char(c)), &mut editor, &mut passthrough, false);
        }
        handle_key(&key(KeyCode::Enter), &mut editor, &mut passthrough, false)
    };
    for line in ["/bot", "/go Grungy Shop", "/go", "/farm", "/set", "/save"] {
        assert!(!matches!(submit(line), KeyOutcome::Send(_)), "{line} must not go to the board");
    }
    assert_eq!(submit("gossip hi"), KeyOutcome::Send("gossip hi".into()));
}

#[test]
fn passthrough_keys_come_back_as_raw_bytes() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    assert_eq!(handle_key(&ctrl('p'), &mut editor, &mut passthrough, false), KeyOutcome::Continue);
    assert!(passthrough);
    assert_eq!(
        handle_key(&key(KeyCode::Up), &mut editor, &mut passthrough, false),
        KeyOutcome::Raw(b"\x1b[A".to_vec())
    );
    assert_eq!(handle_key(&ctrl('q'), &mut editor, &mut passthrough, false), KeyOutcome::QuitNow);
}

#[test]
fn tab_completes_and_lists() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    // "bot.ign" alone is ambiguous: both `bot.ignore` and
    // `bot.ignore_coins` match it. One more character is unique, same
    // as `a_unique_key_completes_after_set` in tests/settings.rs.
    for c in "/set bot.ignore_c".chars() {
        handle_key(&key(KeyCode::Char(c)), &mut editor, &mut passthrough, false);
    }
    assert_eq!(handle_key(&key(KeyCode::Tab), &mut editor, &mut passthrough, false), KeyOutcome::Continue);
    assert_eq!(editor.line(), "/set bot.ignore_coins ");
    let mut editor = InputEditor::new();
    for c in "/set bot.rest_".chars() {
        handle_key(&key(KeyCode::Char(c)), &mut editor, &mut passthrough, false);
    }
    match handle_key(&key(KeyCode::Tab), &mut editor, &mut passthrough, false) {
        KeyOutcome::Note(text) => {
            assert!(text.contains("bot.rest_at_percent"));
            assert!(text.contains("bot.rest_until_percent"));
        }
        other => panic!("expected the candidates, got {other:?}"),
    }
    assert_eq!(editor.line(), "/set bot.rest_");
}

#[test]
fn the_settings_verbs_parse() {
    assert_eq!(slash("/set"), Some(KeyOutcome::SetList { pattern: String::new() }));
    assert_eq!(slash("/set bot.rest*"), Some(KeyOutcome::SetList { pattern: "bot.rest*".into() }));
    assert_eq!(
        slash("/set bot.rest_command sit down"),
        Some(KeyOutcome::Set { key: "bot.rest_command".into(), value: "sit down".into() })
    );
    assert!(matches!(slash("/set bot.* 5"), Some(KeyOutcome::Refuse(_))));
    assert_eq!(slash("/unset bank.at"), Some(KeyOutcome::Unset { key: "bank.at".into() }));
    assert!(matches!(slash("/unset"), Some(KeyOutcome::Refuse(_))));
    assert_eq!(slash("/save"), Some(KeyOutcome::Save { file: None }));
    assert_eq!(slash("/save chars/dan.toml"), Some(KeyOutcome::Save { file: Some("chars/dan.toml".into()) }));
    assert_eq!(slash("/load chars/dan.toml"), Some(KeyOutcome::Load { file: "chars/dan.toml".into() }));
    assert!(matches!(slash("/load"), Some(KeyOutcome::Refuse(_))));
    assert_eq!(slash("/connect"), Some(KeyOutcome::Connect { target: None }));
    assert_eq!(slash("/connect bbs.example.com:2327"), Some(KeyOutcome::Connect { target: Some("bbs.example.com:2327".into()) }));
    assert_eq!(slash("/disconnect"), Some(KeyOutcome::Disconnect));
}

#[test]
fn every_verb_in_the_completion_list_is_claimed_and_in_help() {
    for verb in mud_client::tui::VERBS {
        assert!(slash(verb).is_some() || slash(&format!("{verb} x")).is_some(), "{verb} is not claimed");
        assert!(help_text().contains(verb), "{verb} is not in /help");
    }
}

// ---------------------------------------------------------------------
// The assist's reply to one correlated event. The farm's pump re-looks
// after every fight; playing by hand, the assist must poke its own —
// a fight's end says nothing about who else is standing in the room,
// and without the poke the assist killed one monster and stopped
// (live, cwgaming board, 2026-08-01).
// ---------------------------------------------------------------------

use mud_client::bot::{Bot, BotConfig};
use mud_client::correlate::CmdId;
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
/// arithmetic on the xp/hr figure beside it.
#[test]
fn the_bar_shows_the_level_and_the_time_to_the_next_one() {
    use mud_client::progress::LevelProgress;
    let state = GameState {
        hp: 43,
        mana: Some(10),
        room: None,
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let p = LevelProgress { exp: 57209, level: 3, needed: 7200 };
    let s = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, Some(6_000), Some(p), false, 120);
    assert!(s.contains("L3->4 1h12m"), "got {s:?}");
}

/// Nothing earned yet means no honest estimate, and the bar says so
/// rather than inventing one.
#[test]
fn the_bar_admits_when_it_cannot_estimate() {
    use mud_client::progress::LevelProgress;
    let state = GameState {
        hp: 43,
        mana: Some(10),
        room: None,
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let p = LevelProgress { exp: 1, level: 1, needed: 500 };
    let s = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, Some(p), false, 120);
    assert!(s.contains("L1->2 ?"), "got {s:?}");
}

/// A farm that ends on its own hands the character back to the assist,
/// which keeps fighting — and with nothing in the phase slot that is
/// indistinguishable from a farm still running. It cost an operator a
/// hunt for a Ctrl-F regression that did not exist (2026-08-01).
#[test]
fn the_bar_names_the_assist_when_it_is_driving() {
    let state = GameState {
        hp: 43,
        mana: Some(10),
        room: None,
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let s = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, true, 120);
    assert!(s.starts_with("assist | "), "got {s:?}");
}

/// A farm owns the connection outright while it runs, so its phase wins
/// the slot even with the assist configured on.
#[test]
fn a_running_farm_outranks_the_assist_in_the_bar() {
    let state = GameState {
        hp: 43,
        mana: Some(10),
        room: None,
        status: None,
        ticks: mud_client::world::TickClock::new(),
    };
    let phase = mud_client::farm::Phase::Done { why: "loops walked".into(), at: None };
    let s = render_status(&state, Instant::now(), "mbbs", Some(&phase), Fix::Unknown, None, None, true, 120);
    assert!(!s.contains("assist"), "got {s:?}");
}

// --- slash commands --------------------------------------------------
//
// The client claims a handful of verbs off the input line. Two things
// have to be true and neither was asserted before `/go` existed: what
// the client claims must NOT reach the board, and what it does not
// claim must.

use mud_client::tui::slash;
use mud_client::settings::Settings;
use mud_client::tui::{apply_settings, assist_config_for, connect_target, needs_username};
use mud_client::profile::Profile;

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

/// `/help` and `/?` are the same command under two names.
#[test]
fn help_has_two_spellings() {
    assert_eq!(slash("/help"), Some(KeyOutcome::Help));
    assert_eq!(slash("/?"), Some(KeyOutcome::Help));
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
    let bar = render_status(&state, Instant::now(), "mbbs", Some(&phase), Fix::Unknown, None, None, false, 100);
    assert!(bar.contains("1/2324"), "{bar}");
}

use mud_client::purse::Purse;
use mud_client::tui::on_realm_entry;

/// A board that answers `i` with a fixed 10-gold balance and echoes
/// anything else -- just enough to prove `on_realm_entry` armed and fed
/// the session's own purse tracker, without needing the rest of `play`.
async fn realm_entry_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        // `exp` and `i` go out back to back with no pacing in this test
        // profile, and can arrive in ONE read -- buffer and split on
        // newlines, the same as the `scripted_board` helpers in the
        // other test files do, rather than treating a whole read as one
        // line.
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim().to_lowercase();
                let echo = format!("\r\n{line}");
                if line == "i" {
                    sock.write_all(
                        format!("{echo}\r\nYou are carrying 10 gold crowns\r\n[HP=51/MA=9]:")
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
                    continue;
                }
                let reply = format!("\r\nYou say \"{line}\"\r\n[HP=51/MA=9]:");
                sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
            }
        }
    });
    addr
}

/// `play` cannot be driven end to end (real terminal raw mode, a
/// background OS thread reading `crossterm::event::read()`), so this
/// exercises the realm-entry priming directly through the seam
/// `on_realm_entry` -- the same function `play`'s `select!` arm calls
/// the moment `realm_presence` first reports `true`.
///
/// This is the property the whole fix restores: an operator who has
/// just walked into the game, and typed nothing yet, must already have
/// a session whose purse reflects the board's real answer -- not
/// `Purse::ZERO` waiting on an `i` nobody is going to send by hand.
#[tokio::test]
async fn entering_the_realm_arms_and_fills_the_sessions_purse() {
    let addr = realm_entry_board().await;
    // Arc, not a bare Session: on_realm_entry now also spawns the
    // inventory/spellbook probe in the background (see its doc comment),
    // which needs an owned handle that outlives the call.
    let session = std::sync::Arc::new(session_to(addr).await);

    on_realm_entry(&session, None);
    session
        .expect("You are carrying", std::time::Duration::from_secs(5))
        .await
        .expect("purse reply");

    assert_eq!(
        session.capabilities().purse,
        Purse::from_gold(10),
        "entering the realm must arm and fill the session's purse before anything else asks for it"
    );
}

/// A board that answers `inventory` plainly, redirects `spells` with the
/// mystic KAI wording (`mud_core::text::KAI_NO_SPELLS`), and answers
/// `powers` with a one-line listing -- everything else echoes generically,
/// same shape as `realm_entry_board`.
async fn mystic_realm_entry_board() -> (std::net::SocketAddr, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&received);
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
                let line = line.trim().to_lowercase();
                log.lock().unwrap().push(line.clone());
                let echo = format!("\r\n{line}");
                let reply = match line.as_str() {
                    "inventory" => "\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=51/MA=9]:".to_string(),
                    "spells" => "\r\nYou may not list your spells. You are KAI! You must list your powers.\r\n[HP=51/MA=9]:".to_string(),
                    "powers" => "\r\n  1  3    heal        Heal Self\r\n[HP=51/MA=9]:".to_string(),
                    other => format!("\r\nYou say \"{other}\"\r\n[HP=51/MA=9]:"),
                };
                sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
            }
        }
    });
    (addr, received)
}

/// Mystics are found out rather than configured: `spells` answers with
/// the KAI redirect and the client is expected to try `powers` instead,
/// exactly once, driven entirely by `on_realm_entry`'s background probe
/// -- this test never sends either command itself.
///
/// `probe_sheet` has no terminal wording to watch for on the spell
/// listing, so each of its asks waits out its own window before
/// `Session::set_sheet` is called. A text-based `session.expect` on the
/// transcript would return the instant the board's reply arrives, well
/// before that. A fixed sleep here went stale the day `stat` joined the
/// probe and added a window of its own, so this polls the sheet until
/// the casting flips, bounded by a deadline, instead of guessing the sum.
#[tokio::test]
async fn the_mystic_redirect_happens_once_at_login() {
    let (addr, received) = mystic_realm_entry_board().await;
    let session = std::sync::Arc::new(session_to(addr).await);

    on_realm_entry(&session, None);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let (_, _, casting) = session.raw_sheet();
        if casting == mud_client::sheet::Casting::Powers {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a KAI character must be found out from the redirect, not left on Spells"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let log = received.lock().unwrap();
    assert_eq!(
        log.iter().filter(|l| **l == "spells").count(),
        1,
        "spells must be asked exactly once: {log:?}"
    );
    assert_eq!(
        log.iter().filter(|l| **l == "powers").count(),
        1,
        "the redirect must fire exactly once, not loop: {log:?}"
    );
}

// ---------------------------------------------------------------------
// A job handing the character back. The walk's last step answered with
// the destination's block, and the walk consumed it: the assist is
// rebuilt fresh at the handover and has never seen the room. Live,
// 2026-09-04: a `/go` ended in a Dark Cave listing a giant rat and a
// cave worm, the worm lunged, and the assist stood there until the
// operator typed the backstab. The handover pokes a look, and the
// assist engages off its attributed answer exactly as it does after a
// fight.
// ---------------------------------------------------------------------

use mud_client::farm::Phase;
use mud_client::tui::handover_actions;

#[test]
fn a_job_ending_somewhere_known_pokes_a_look_for_the_assist() {
    let at = RoomId { map: 1, room: 2226 };
    let done = Phase::Done { why: "arrived at 1/2226".into(), at: Some(at) };
    assert_eq!(handover_actions(&done, true), vec!["look".to_string()]);
    let placed = Phase::Placed { at, steps: 0 };
    assert_eq!(handover_actions(&placed, true), vec!["look".to_string()]);
}

/// Nothing is sent for a job that does not know where it left the
/// character: after a death the board is not even at a room prompt.
#[test]
fn a_job_ending_nowhere_known_pokes_nothing() {
    let died = Phase::Done { why: "died".into(), at: None };
    assert_eq!(handover_actions(&died, true), Vec::<String>::new());
    let failed = Phase::Failed { why: "no route".into() };
    assert_eq!(handover_actions(&failed, true), Vec::<String>::new());
}

/// With the assist off the operator gets the keyboard back and nothing
/// else; a look nobody acts on is a wasted round trip.
#[test]
fn a_job_ending_with_the_assist_off_pokes_nothing() {
    let at = RoomId { map: 1, room: 2226 };
    let done = Phase::Done { why: "arrived at 1/2226".into(), at: Some(at) };
    assert_eq!(handover_actions(&done, false), Vec::<String>::new());
}

#[test]
fn status_line_shows_the_prompt_status_after_the_vitals() {
    let state = GameState {
        hp: 42,
        mana: Some(12),
        room: None,
        status: Some(Status::Resting),
        ticks: mud_client::world::TickClock::new(),
    };
    let s = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 80);
    assert!(s.contains("HP 42 MA 12 (Resting)"), "{s}");

    let bare = GameState { status: None, ..state };
    let s = render_status(&bare, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 80);
    assert!(!s.contains("(Resting)"), "{s}");
}

fn ticks_after(events: &[(Event, Duration)], t0: Instant) -> TickClock {
    let mut clock = TickClock::new();
    for (ev, at) in events {
        let cor = Correlated { event: ev.clone(), answers: None, elsewhere: false };
        clock.on_event(&cor, t0 + *at);
    }
    clock
}

#[test]
fn status_line_shows_dashes_until_a_clock_is_locked() {
    let state = GameState {
        hp: 42,
        mana: None,
        room: None,
        status: None,
        ticks: TickClock::new(),
    };
    let s = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 120);
    assert!(s.contains("| Tick - | HP - |"), "{s}");
    // No pool, no mana countdown.
    assert!(!s.contains("MA "), "{s}");
}

#[test]
fn status_line_counts_down_the_round_and_the_regen_cycles() {
    let t0 = Instant::now();
    let hit = Event::CombatHit {
        attacker: Actor::Other("The giant rat".into()),
        target: Actor::You,
        damage: 2,
    };
    let ticks = ticks_after(
        &[
            (Event::Prompt { hp: 30, mana: Some(10), status: None }, Duration::ZERO),
            (Event::Prompt { hp: 32, mana: Some(12), status: None }, Duration::from_secs(1)),
            (hit, Duration::from_secs(2)),
        ],
        t0,
    );
    let state = GameState { hp: 32, mana: Some(12), room: None, status: None, ticks };
    let now = t0 + Duration::from_secs(3);
    let s = render_status(&state, now, "mbbs", None, Fix::Unknown, None, None, false, 120);
    let round = (ROUND - Duration::from_secs(1)).as_secs_f64();
    assert!(s.contains(&format!("Tick {round:.1} | HP 28.0 | MA 28.0")), "{s}");
}

#[test]
fn status_line_shows_the_rest_cycle_beside_the_natural_one_while_resting() {
    let t0 = Instant::now();
    let ticks = ticks_after(
        &[
            (Event::Prompt { hp: 30, mana: None, status: None }, Duration::ZERO),
            (Event::Prompt { hp: 32, mana: None, status: None }, Duration::from_secs(1)),
            (Event::Prompt { hp: 32, mana: None, status: Some(Status::Resting) }, Duration::from_secs(2)),
        ],
        t0,
    );
    let state = GameState { hp: 32, mana: None, room: None, status: Some(Status::Resting), ticks };
    let now = t0 + Duration::from_secs(4);
    let s = render_status(&state, now, "mbbs", None, Fix::Unknown, None, None, false, 120);
    assert!(s.contains("HP 27.0/18.0"), "{s}");
}

// ---------------------------------------------------------------------
// The assist's own heal, by the profile's marks. `assist_heal` lends the
// bot's percent arithmetic to the heal state without letting the bot
// decide anything itself, so the assist and the farm's stop loop read
// the same number.
// ---------------------------------------------------------------------

fn healer() -> HealState {
    let book = Spellbook::parse("  1    2  mihe   minor healing\n  4    6  mahe   major healing\n");
    HealState::new(book.heal_spells(HealChoice { minor: "", major: "", regen: "" }, &BTreeMap::new(), Casting::Spells).0)
}

#[test]
fn the_assist_casts_by_the_marks_and_once_per_round() {
    let cfg = BotConfig { auto_heal: true, max_hp: 100, max_mana: 20, ..BotConfig::default() };
    let bot = Bot::new(cfg.clone());
    let mut heal = healer();
    let clock = mud_client::world::RoundClock::new();
    let now = Instant::now();
    heal.on_event(
        &Correlated { event: Event::Prompt { hp: 30, mana: Some(9), status: None }, answers: None, elsewhere: false },
        now,
    );
    assert_eq!(assist_heal(&cfg, &bot, &mut heal, &clock, 30, now), Some("cast mahe".into()));
    // The same round asks again: held.
    assert_eq!(assist_heal(&cfg, &bot, &mut heal, &clock, 30, now + Duration::from_secs(1)), None);
    // Healthy: nothing.
    assert_eq!(assist_heal(&cfg, &bot, &mut heal, &clock, 90, now + ROUND * 2), None);
    // Auto heal off: nothing.
    let off = BotConfig { auto_heal: false, ..cfg.clone() };
    assert_eq!(assist_heal(&off, &bot, &mut heal, &clock, 30, now + ROUND * 3), None);
}

#[test]
fn bank_is_its_own_outcome() {
    assert_eq!(slash("/bank"), Some(KeyOutcome::Bank));
    assert_eq!(slash("  /bank  "), Some(KeyOutcome::Bank));
}

#[test]
fn editor_replace_swaps_a_word_and_parks_the_cursor_after_it() {
    let mut e = InputEditor::new();
    for c in "/set bot.ign 5".chars() {
        e.insert(c);
    }
    e.replace(5, 12, "bot.ignore_coins ");
    assert_eq!(e.line(), "/set bot.ignore_coins  5");
    assert_eq!(e.cursor(), 22);
}

#[test]
fn set_changes_the_profile_and_says_so_unsaved() {
    let mut s = Settings::default();
    let applied = apply_settings(
        &KeyOutcome::Set { key: "bot.rest_at_percent".into(), value: "45".into() },
        &mut s,
    )
    .unwrap();
    assert!(applied.profile_changed);
    assert!(applied.bot_changed);
    assert!(applied.note.contains("bot.rest_at_percent = 45"), "{}", applied.note);
    assert!(applied.note.contains("unsaved"), "{}", applied.note);
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 45);
    // Nothing to save is nothing to say about saving.
    let mut clean = Settings::parse("[bot]\nrest_at_percent = 60\n").unwrap();
    let applied = apply_settings(
        &KeyOutcome::Set { key: "bot.rest_at_percent".into(), value: "60".into() },
        &mut clean,
    )
    .unwrap();
    assert!(!clean.dirty(), "60 is what the document already said");
    assert!(applied.note.contains("bot.rest_at_percent = 60"), "{}", applied.note);
    assert!(!applied.note.contains("unsaved"), "{}", applied.note);
}

#[test]
fn a_refused_set_changes_nothing_and_reports() {
    let mut s = Settings::default();
    let applied = apply_settings(
        &KeyOutcome::Set { key: "bot.rest_at_percent".into(), value: "soon".into() },
        &mut s,
    )
    .unwrap();
    assert!(!applied.profile_changed);
    assert!(applied.note.contains("set:"), "{}", applied.note);
    assert!(s.profile().bot.is_none());
}

#[test]
fn a_bank_key_does_not_touch_the_assist() {
    let mut s = Settings::default();
    let applied = apply_settings(&KeyOutcome::Set { key: "bank.keep_gold".into(), value: "5".into() }, &mut s).unwrap();
    assert!(applied.profile_changed);
    assert!(!applied.bot_changed);
}

#[test]
fn a_listing_is_one_row_per_key() {
    let mut s = Settings::default();
    let applied = apply_settings(&KeyOutcome::SetList { pattern: "bot.rest".into() }, &mut s).unwrap();
    assert_eq!(applied.note.lines().count(), 3, "{}", applied.note);
    assert!(applied.note.lines().all(|l| l.contains(" = ")));
    assert!(!applied.profile_changed);
    let none = apply_settings(&KeyOutcome::SetList { pattern: "zebra".into() }, &mut s).unwrap();
    assert!(none.note.contains("no setting matches"));
}

#[test]
fn load_replaces_everything_and_rebuilds_the_assist() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("tui");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loaded.toml");
    std::fs::write(&path, "username = \"ann\"\n[bot]\nauto_heal = true\n").unwrap();
    let mut s = Settings::default();
    let applied = apply_settings(&KeyOutcome::Load { file: path.display().to_string() }, &mut s).unwrap();
    assert!(applied.profile_changed);
    assert!(applied.bot_changed);
    assert_eq!(s.profile().username, "ann");
    assert_eq!(s.path(), Some(path.as_path()));
    let missing = apply_settings(&KeyOutcome::Load { file: dir.join("none.toml").display().to_string() }, &mut s).unwrap();
    assert!(missing.note.contains("load:"));
    assert_eq!(s.profile().username, "ann", "a failed load keeps what was there");
}

#[test]
fn save_reports_the_path_or_asks_for_one() {
    let mut s = Settings::default();
    let asked = apply_settings(&KeyOutcome::Save { file: None }, &mut s).unwrap();
    assert!(asked.note.contains("/save <file>"), "{}", asked.note);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("tui");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.toml");
    let saved = apply_settings(&KeyOutcome::Save { file: Some(path.display().to_string()) }, &mut s).unwrap();
    assert!(saved.note.contains("saved"), "{}", saved.note);
    assert!(path.exists());
}

#[test]
fn other_outcomes_are_not_settings_commands() {
    let mut s = Settings::default();
    assert!(apply_settings(&KeyOutcome::Where, &mut s).is_none());
    assert!(apply_settings(&KeyOutcome::Send("look".into()), &mut s).is_none());
}

#[test]
fn a_profile_without_a_bot_table_gets_the_attack_and_loot_assist() {
    let cfg = assist_config_for(&Profile::default());
    assert!(cfg.auto_combat && cfg.auto_get && !cfg.auto_heal);
    let with = Profile {
        bot: Some(BotConfig { auto_heal: true, ..Default::default() }),
        ..Default::default()
    };
    let cfg = assist_config_for(&with);
    assert!(!cfg.auto_combat && cfg.auto_heal);
}

#[test]
fn jobs_need_a_username() {
    let err = needs_username(&Profile::default()).unwrap_err();
    assert!(err.contains("/set username"), "{err}");
    let named = Profile {
        username: "dan".into(),
        ..Default::default()
    };
    assert!(needs_username(&named).is_ok());
}

#[test]
fn connect_target_defaults_the_port_to_telnet() {
    assert_eq!(connect_target("bbs.example.com").unwrap(), ("bbs.example.com".into(), 23));
    assert_eq!(connect_target("127.0.0.1:2327").unwrap(), ("127.0.0.1".into(), 2327));
    assert!(connect_target("host:port").is_err());
    assert!(connect_target("").is_err());
}

use mud_client::session::Session;
use mud_client::tui::{ContentCache, LobbyStep, lobby_step};

async fn banner_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\nUsername: ").await.unwrap();
        let mut hold = [0u8; 64];
        use tokio::io::AsyncReadExt;
        let _ = sock.read(&mut hold).await;
    });
    addr
}

/// The lobby's whole job: settings, then `/connect`, then a session that
/// sees the board. The terminal loop around it cannot be driven from a
/// test, so this drives the seam it is built on.
#[tokio::test]
async fn the_lobby_sets_host_and_port_then_connects_and_sees_the_banner() {
    let addr = banner_board().await;
    let mut settings = Settings::default();
    let mut armed = false;
    let (step, note) = lobby_step(KeyOutcome::Send("look".into()), &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    assert!(note.unwrap().contains("not connected"));
    let (step, _) = lobby_step(slash(&format!("/connect {}:{}", addr.ip(), addr.port())).unwrap(), &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Connect));
    assert_eq!(settings.profile().host, addr.ip().to_string());
    assert_eq!(settings.profile().port, addr.port());
    assert!(settings.dirty(), "the host and port are settings, so /save keeps them");
    let session = Session::connect(settings.profile(), None).await.unwrap();
    session.expect("Welcome to the Test Board", std::time::Duration::from_secs(5)).await.unwrap();
}

#[test]
fn the_lobby_refuses_to_connect_nowhere_and_refuses_jobs() {
    let mut settings = Settings::default();
    let mut armed = false;
    let (step, note) = lobby_step(KeyOutcome::Connect { target: None }, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    assert!(note.unwrap().contains("no host"));
    let (step, note) = lobby_step(KeyOutcome::StartFarm { loop_name: None }, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    assert!(note.unwrap().contains("not connected"));
}

#[test]
fn quit_in_the_lobby_asks_once_while_unsaved() {
    let mut settings = Settings::default();
    let mut armed = false;
    settings.set("host", "\"h\"").unwrap();
    let (step, note) = lobby_step(KeyOutcome::Quit, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    assert!(note.unwrap().contains("unsaved"));
    let (step, _) = lobby_step(KeyOutcome::Quit, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Quit));
    let mut armed = false;
    let (step, _) = lobby_step(KeyOutcome::QuitNow, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Quit), "Ctrl-Q never asks");
    let mut clean = Settings::default();
    let (step, _) = lobby_step(KeyOutcome::Quit, &mut clean, &mut armed);
    assert!(matches!(step, LobbyStep::Quit), "nothing unsaved, nothing to ask");
}

/// The host is written into the document as TOML. Rust's `Debug` spells
/// an escape its own way, and a control character escaped that way is
/// not a string the TOML parser will take back.
#[test]
fn the_lobby_writes_the_host_as_toml() {
    let mut settings = Settings::default();
    let mut armed = false;
    let (step, note) = lobby_step(
        KeyOutcome::Connect {
            target: Some("a\u{7}b:2327".into()),
        },
        &mut settings,
        &mut armed,
    );
    assert!(matches!(step, LobbyStep::Connect), "{note:?}");
    assert_eq!(settings.profile().host, "a\u{7}b");
    assert_eq!(settings.profile().port, 2327);
}

/// A refusal is dashed on both sides of a connection, so the same
/// mistake reads the same in the lobby as in play.
#[test]
fn the_lobby_dashes_a_refusal_the_way_play_does() {
    let mut settings = Settings::default();
    let mut armed = false;
    let (step, note) = lobby_step(slash("/unset").unwrap(), &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    let note = note.unwrap();
    assert!(note.starts_with("-- ") && note.ends_with(" --"), "{note}");
    assert!(note.contains("/unset"), "{note}");
}

/// The refusal itself, which the lobby and play both go through. Play's
/// key arm cannot be driven from a test, so this is where its rule is
/// pinned.
#[test]
fn the_quit_refusal_asks_once_while_unsaved() {
    use mud_client::tui::quit_refusal;

    let mut settings = Settings::default();
    let mut armed = false;
    assert!(
        quit_refusal(&settings, &mut armed).is_none(),
        "nothing unsaved, nothing to ask"
    );
    assert!(!armed, "a quit that proceeds arms nothing");
    settings.set("host", "\"h\"").unwrap();
    let why = quit_refusal(&settings, &mut armed).expect("unsaved settings ask once");
    assert!(why.contains("unsaved settings"), "{why}");
    assert!(armed, "the refusal arms the next quit");
    assert!(
        quit_refusal(&settings, &mut armed).is_none(),
        "armed and still dirty, so the second quit goes through"
    );
}

#[test]
fn a_command_between_two_quits_disarms_the_second() {
    let mut settings = Settings::default();
    settings.set("host", "\"h\"").unwrap();
    let mut armed = false;
    let (step, _) = lobby_step(KeyOutcome::Quit, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    let _ = lobby_step(KeyOutcome::SetList { pattern: String::new() }, &mut settings, &mut armed);
    let (step, _) = lobby_step(KeyOutcome::Quit, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay), "the refusal is for two /quit in a row");
}

#[test]
fn a_missing_world_database_is_none_and_asked_again_next_time() {
    let mut cache = ContentCache::default();
    let nowhere = std::path::Path::new("nowhere/at/all.sqlite");
    assert!(cache.world(nowhere).is_none());
    assert!(cache.world(nowhere).is_none());
}


/// Every job start refuses a character with no name, and the refusal
/// says the same thing wherever it comes from. The gate is inside the
/// start functions rather than at their call sites, which is what makes
/// the map's roam and go refuse too.
#[tokio::test]
async fn every_job_start_refuses_without_a_username() {
    use mud_client::graph::{GraphRoom, RoomGraph};
    use mud_client::tui::{start_bank, start_farm, start_go, start_roam, start_where};
    use std::sync::Arc;

    let addr = banner_board().await;
    let profile = Profile {
        host: addr.ip().to_string(),
        port: addr.port(),
        ..Default::default()
    };
    assert!(profile.username.is_empty(), "the character has no name");
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());
    let here = mud_core::content::RoomId { map: 1, room: 1 };
    let graph = Arc::new(RoomGraph::from_rooms(vec![(
        here,
        GraphRoom {
            name: "Home".into(),
            ..Default::default()
        },
    )]));
    let bot = mud_client::bot::BotConfig::default();
    let refusals = vec![
        start_farm(session.clone(), None).err(),
        start_roam(
            session.clone(),
            mud_client::roam::Walls::new([]),
            mud_client::lost::Fix::Confirmed(here),
        )
        .err(),
        start_go(
            session.clone(),
            graph.clone(),
            Some(here),
            here,
            bot.clone(),
            false,
        )
        .err(),
        start_bank(session.clone(), graph.clone(), Some(here), bot, false).err(),
        start_where(session.clone(), graph.clone(), Some(here)).err(),
    ];
    for refusal in refusals {
        let why = refusal.expect("a job started without a username");
        assert!(why.contains("username is empty"), "{why}");
    }
}

#[test]
fn the_window_verbs_parse() {
    assert_eq!(slash("/new"), Some(KeyOutcome::NewWindow { file: None }));
    assert_eq!(slash("/new chars/ann.toml"), Some(KeyOutcome::NewWindow { file: Some("chars/ann.toml".into()) }));
    assert_eq!(slash("/close"), Some(KeyOutcome::CloseWindow));
    assert_eq!(slash("/windows"), Some(KeyOutcome::Windows));
    assert_eq!(slash("/1"), Some(KeyOutcome::Switch(1)));
    assert_eq!(slash("/9"), Some(KeyOutcome::Switch(9)));
    assert_eq!(slash("/0"), None, "there is no window 0, so the board gets it");
    assert_eq!(slash("/10"), None, "only one digit switches");
    for verb in ["/new", "/close", "/windows"] {
        assert!(help_text().contains(verb), "{verb} is not in /help");
        assert!(mud_client::tui::VERBS.contains(&verb), "{verb} is not offered by Tab");
    }
    assert!(help_text().contains("/1"), "the switch is in /help");
    assert!(help_text().contains("PageUp"), "PageUp is in /help");
}

#[test]
fn the_lobby_ignores_the_window_verbs() {
    let mut settings = Settings::default();
    let mut armed = false;
    assert_eq!(
        lobby_step(KeyOutcome::Windows, &mut settings, &mut armed),
        (LobbyStep::Stay, None)
    );
    assert_eq!(
        lobby_step(KeyOutcome::Switch(2), &mut settings, &mut armed),
        (LobbyStep::Stay, None)
    );
}

