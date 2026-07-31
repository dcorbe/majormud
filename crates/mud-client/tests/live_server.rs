//! Session-engine integration tests: a real socket against the
//! in-process mud-server reimplementation. No external network
//! dependency. The MBBSEmu login dialogue is covered by
//! `tests/dialect.rs`, which replays a real board capture.

use std::time::Duration;

use mud_client::dialect::{self, LoginOutcome, Target};
use mud_client::events::Event;
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::CoreConfig;
use mud_server::server::Server;
use mud_server::state_db::StateDb;

fn world() -> Content {
    let mut content = Content::default();
    let mut gates = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec!["You are before the massive town gates.".into()],
        ..Default::default()
    };
    gates.exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        ..Default::default()
    });
    let mut square = Room {
        id: RoomId { map: 1, room: 2 },
        name: "Town Square".into(),
        ..Default::default()
    };
    square.exits[Direction::South as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 1 },
        ..Default::default()
    });
    content.add_room(gates);
    content.add_room(square);
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 40,
            wisdom: 40,
            strength: 40,
            health: 40,
            agility: 40,
            charm: 40,
        },
        max_stats: StatBlock {
            intellect: 100,
            wisdom: 100,
            strength: 100,
            health: 100,
            agility: 100,
            charm: 100,
        },
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
        abilities: vec![],
        hp_per_level: 6,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 6,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        exit_meditation_seconds: 1,
        // The client's room classifier keys on the 1;36 name color; the
        // client always runs with ANSI on (as MegaMud did).
        ansi: true,
        ..CoreConfig::default()
    }
}

async fn start_server() -> Server {
    let state = StateDb::open_in_memory().expect("state db");
    Server::start(world(), config(), state, "127.0.0.1:0")
        .await
        .expect("start server")
}

fn rust_profile(addr: std::net::SocketAddr) -> Profile {
    Profile {
        target: Target::RustServer,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "Alice".into(),
        password: "hunter2".into(),
        pace_ms: None,
        disable_evil_warnings: false,
        bot: None,
        farm: None,
    }
}

/// Create an account, walk character creation, and verify the parsed
/// room arrives as an event and in the shared game state.
#[tokio::test]
async fn rust_server_login_create_look() {
    let server = start_server().await;
    let session = Session::connect(&rust_profile(server.local_addr()), None)
        .await
        .expect("connect");
    let mut events = session.events();

    let outcome = dialect::login(&session, &rust_profile(server.local_addr()))
        .await
        .expect("login");
    assert_eq!(outcome, LoginOutcome::CharacterCreation);

    let t = Duration::from_secs(5);
    session.send("1"); // race
    session
        .expect("Please choose a class from the following list:", t)
        .await
        .expect("class list");
    session.send("1"); // class
    session.expect("Do you want to be Lawful?", t).await.expect("lawful");
    session.send("No");
    session.expect("[HP=", t).await.expect("prompt");

    let look = session.send("look");
    session.expect("Obvious exits:", t).await.expect("room");

    // The server echoes the accepted command like the real board does
    // (that echo is what correlation stands on), so the RoomSeen that
    // came through the broadcast must be ATTRIBUTED to our look.
    let mut seen = None;
    while let Ok(ev) = events.try_recv() {
        if let Event::RoomSeen(r) = ev.event {
            assert_eq!(ev.answers, Some(look), "the fixture server must echo");
            seen = Some(r);
        }
    }
    let room = seen.expect("RoomSeen event");
    assert_eq!(room.name, "Town Gates");
    assert_eq!(room.exits, vec!["north"]);

    // ...and into the game state watch.
    let state = session.state().borrow().clone();
    assert_eq!(state.room.as_ref().map(|r| r.name.as_str()), Some("Town Gates"));
    assert!(state.hp > 0, "prompt HP tracked, got {}", state.hp);
}

/// The board hides a junk character and a backspace inside every
/// direction word it prints. The client's pipeline resolves those before
/// anything classifies the text, so a scrambled exits line has to parse
/// exactly like a clean one — that resolution step is the reason period
/// clients could read the board at all.
#[tokio::test]
async fn a_scrambled_exits_line_parses_like_a_clean_one() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(
        world(),
        CoreConfig {
            wire_noise: true,
            ..config()
        },
        state,
        "127.0.0.1:0",
    )
    .await
    .expect("start server");

    let profile = rust_profile(server.local_addr());
    let session = Session::connect(&profile, None).await.expect("connect");
    let mut events = session.events();
    dialect::login(&session, &profile).await.expect("login");
    dialect::finish_creation(&session).await.expect("creation");

    let look = session.send("look");
    session
        .expect("Obvious exits:", Duration::from_secs(5))
        .await
        .expect("room");

    let mut seen = None;
    while let Ok(ev) = events.try_recv() {
        if let Event::RoomSeen(r) = ev.event {
            assert_eq!(ev.answers, Some(look), "still attributed to our look");
            seen = Some(r);
        }
    }
    let room = seen.expect("RoomSeen event");
    assert_eq!(room.name, "Town Gates");
    assert_eq!(
        room.exits,
        vec!["north"],
        "the direction survives the anti-bot junk"
    );
}

/// `finish_creation` drives the race/class/alignment dialogue that
/// `login` stops in front of, and lands on the game prompt. The runner
/// (and the nav tests) rely on it instead of re-inlining the sequence.
#[tokio::test]
async fn finish_creation_reaches_the_game_prompt() {
    let server = start_server().await;
    let session = Session::connect(&rust_profile(server.local_addr()), None)
        .await
        .expect("connect");

    let outcome = dialect::login(&session, &rust_profile(server.local_addr()))
        .await
        .expect("login");
    assert_eq!(outcome, LoginOutcome::CharacterCreation);

    dialect::finish_creation(&session).await.expect("finish creation");

    // In game: the prompt has been seen, so the state watch carries HP.
    assert!(
        session.state().borrow().hp > 0,
        "expected the game prompt to have been parsed, state was {:?}",
        session.state().borrow().clone()
    );
}

#[tokio::test]
async fn expect_times_out_with_tail() {
    let server = start_server().await;
    let session = Session::connect(&rust_profile(server.local_addr()), None)
        .await
        .expect("connect");
    let err = session
        .expect("this text never appears", Duration::from_millis(300))
        .await
        .expect_err("must time out");
    let msg = format!("{err}");
    assert!(msg.contains("this text never appears"), "error names the needle: {msg}");
}

#[tokio::test]
async fn expect_cursor_advances_past_matches() {
    let server = start_server().await;
    let session = Session::connect(&rust_profile(server.local_addr()), None)
        .await
        .expect("connect");
    let t = Duration::from_secs(5);
    // The server greets with "Account: ". A second expect for the same
    // needle must NOT match the already-consumed occurrence.
    session.expect("Account: ", t).await.expect("first greet");
    let err = session
        .expect("Account: ", Duration::from_millis(300))
        .await
        .expect_err("cursor advanced; no second occurrence yet");
    drop(err);
}

/// Capture files: .raw gets the raw socket bytes; the timing log gets
/// `EPOCH.mmm TAG payload` lines with TX for sends.
#[tokio::test]
async fn capture_writes_raw_and_timing() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let raw_path = dir.join("cap_test.raw");
    let log_path = dir.join("cap_test_timing.log");
    let _ = std::fs::remove_file(&raw_path);
    let _ = std::fs::remove_file(&log_path);

    let server = start_server().await;
    let session = Session::connect(
        &rust_profile(server.local_addr()),
        Some(mud_client::session::Capture {
            raw: raw_path.clone(),
            timing: Some(log_path.clone()),
        }),
    )
    .await
    .expect("connect");
    let t = Duration::from_secs(5);
    session.expect("Account: ", t).await.expect("greet");
    session.send("Alice");
    session.expect("Create new account?", t).await.expect("create?");
    drop(session);

    // Flush is on write; give the actor a beat to drain.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let raw = std::fs::read(&raw_path).expect("raw capture exists");
    let raw_text = String::from_utf8_lossy(&raw);
    assert!(raw_text.contains("Account: "), "raw has server bytes");
    let log = std::fs::read_to_string(&log_path).expect("timing log exists");
    assert!(log.lines().any(|l| l.contains(" TX Alice")), "timing has TX: {log}");
    let first = log.lines().next().unwrap();
    // EPOCH.mmm TAG ...
    let stamp = first.split_whitespace().next().unwrap();
    assert!(stamp.contains('.'), "millisecond stamp: {first}");
    assert!(stamp.split('.').next().unwrap().parse::<u64>().is_ok());
}

/// Ask the board for the character's real max HP rather than trusting a
/// number typed into a profile. A wrong `max_hp` silently mis-scales
/// every percent policy the bot has, and `0` disables them outright.
#[tokio::test]
async fn discover_max_hp_asks_the_board() {
    let server = start_server().await;
    let session = Session::connect(&rust_profile(server.local_addr()), None)
        .await
        .expect("connect");
    dialect::login(&session, &rust_profile(server.local_addr()))
        .await
        .expect("login");
    dialect::finish_creation(&session).await.expect("finish creation");

    let max = mud_client::farm::discover_max_hp(&session)
        .await
        .expect("health report");

    // A level-1 Warrior of this fixture race: whatever the server rolled,
    // it must agree with the prompt the client already parsed.
    assert!(max > 0, "max hp should be positive, got {max}");
    let hp = session.state().borrow().hp;
    assert!(
        hp <= max,
        "prompt HP {hp} exceeds the reported max {max}"
    );
}
