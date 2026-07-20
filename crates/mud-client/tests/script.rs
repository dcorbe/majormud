//! Lua script-runner tests: the `mud` API drives a real session.

use std::sync::Arc;
use std::time::Duration;

use mud_client::dialect::Target;
use mud_client::profile::Profile;
use mud_client::script::run_script;
use mud_client::session::Session;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn profile_for(addr: std::net::SocketAddr, target: Target) -> Profile {
    Profile {
        target,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "Oracle".into(),
        password: "test123".into(),
        pace_ms: Some(0),
    }
}

/// A tiny fake board: greets, echoes a room-ish response to "look",
/// then a prompt.
async fn fake_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 512];
        sock.write_all(b"Welcome, adventurer!\r\n[HP=42/MA=13]:").await.unwrap();
        loop {
            let n = match sock.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let line = String::from_utf8_lossy(&buf[..n]);
            if line.contains("look") {
                sock.write_all(b"A Fake Room.\r\nObvious exits: none you can use\r\n[HP=42/MA=13]:")
                    .await
                    .unwrap();
            }
        }
    });
    addr
}

fn write_script(name: &str, body: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[tokio::test]
async fn script_drives_session_and_captures_sections() {
    let addr = fake_board().await;
    let profile = profile_for(addr, Target::MbbsEmu);
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());

    let script = write_script(
        "drive.lua",
        r#"
        mud.expect("Welcome, adventurer!")
        mud.expect("]:")
        local m = mud.mark()
        mud.send("look")
        mud.expect("Obvious exits:", 5)
        mud.save_section("look", mud.since(m))
        assert(mud.hp() == 42, "hp is " .. tostring(mud.hp()))
        assert(mud.mana() == 13)
        "#,
    );
    let outcome = run_script(&script, session).await.expect("script runs");
    let look = outcome.sections.get("look").expect("look section");
    assert!(look.contains("A Fake Room."), "section content: {look:?}");
    assert!(look.contains("Obvious exits:"));
}

#[tokio::test]
async fn script_expect_timeout_is_an_error() {
    let addr = fake_board().await;
    let profile = profile_for(addr, Target::MbbsEmu);
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());

    let script = write_script(
        "timeout.lua",
        r#"mud.expect("text that never comes", 0.3)"#,
    );
    let err = run_script(&script, session).await.expect_err("must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("text that never comes"),
        "error names the needle: {msg}"
    );
}

#[tokio::test]
async fn script_sleep_and_room_name() {
    let addr = fake_board().await;
    let profile = profile_for(addr, Target::MbbsEmu);
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());

    // room_name() is nil before any room block was parsed.
    let script = write_script(
        "roomnil.lua",
        r#"
        mud.sleep(0.05)
        assert(mud.room_name() == nil)
        "#,
    );
    run_script(&script, session).await.expect("script runs");
}

/// Acceptance: the oracle_directions scenario ported to Lua, against
/// the in-process Rust server — creation, movement probes, sections.
#[tokio::test]
async fn oracle_directions_lua_against_rust_server() {
    use mud_core::content::{
        Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
    };
    use mud_core::game::CoreConfig;
    use mud_server::{server::Server, state_db::StateDb};

    let mut content = Content::default();
    let mut gates = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
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
    let config = CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        exit_meditation_seconds: 1,
        ansi: true,
        ..CoreConfig::default()
    };
    let server = Server::start(content, config, StateDb::open_in_memory().unwrap(), "127.0.0.1:0")
        .await
        .unwrap();

    let profile = profile_for(server.local_addr(), Target::RustServer);
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());

    let script = std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/oracle_directions.lua"
    ));
    let outcome = run_script(&script, session).await.expect("script runs");

    let look = outcome.sections.get("look").expect("look section");
    assert!(look.contains("Town Gates"), "look: {look}");
    let move_n = outcome.sections.get("move_n").expect("move_n section");
    assert!(move_n.contains("Town Square"), "move_n: {move_n}");
    let bad = outcome.sections.get("bad_direction").expect("bad_direction");
    assert!(
        bad.contains("There is no exit in that direction!"),
        "bad_direction: {bad}"
    );
}

// Keep the timeout import used even if tests change shape.
#[allow(dead_code)]
fn _t() -> Duration {
    Duration::from_secs(1)
}
