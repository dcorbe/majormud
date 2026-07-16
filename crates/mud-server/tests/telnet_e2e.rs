//! End-to-end test: TCP client connects, creates an account and character,
//! looks around, walks, quits.

use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::CoreConfig;
use mud_server::server::Server;
use mud_server::state_db::StateDb;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn world() -> Content {
    let mut content = Content::default();
    let mut gates = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec!["You are before the massive town gates.".into()],
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    };
    gates.exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        exit_type: 0,
        trigger_msg: None,
    });
    let mut square = Room {
        id: RoomId { map: 1, room: 2 },
        name: "Town Square".into(),
        description: vec![],
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    };
    square.exits[Direction::South as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 1 },
        exit_type: 0,
        trigger_msg: None,
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
    });
    content
}


fn test_config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        exit_meditation_seconds: 1,
        ..CoreConfig::default()
    }
}

/// Reads from the socket until `needle` appears in the accumulated transcript
/// (with a timeout so failures are readable, not hangs).
async fn read_until(stream: &mut TcpStream, transcript: &mut String, needle: &str) {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    let mut buf = [0u8; 4096];
    while !transcript.contains(needle) {
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf))
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got:\n{transcript}"))
            .expect("read");
        assert!(n > 0, "connection closed waiting for {needle:?}; got:\n{transcript}");
        // Strip telnet IAC sequences crudely for the transcript (test-side).
        transcript.push_str(&String::from_utf8_lossy(&buf[..n]).replace('\u{fffd}', ""));
    }
}

async fn send(stream: &mut TcpStream, line: &str) {
    stream
        .write_all(format!("{line}\r\n").as_bytes())
        .await
        .expect("write");
}

#[tokio::test]
async fn full_session_create_walk_quit() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");
    let addr = server.local_addr();

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut t = String::new();

    read_until(&mut stream, &mut t, "Account: ").await;
    send(&mut stream, "Alice").await;

    read_until(&mut stream, &mut t, "Create new account? (y/n)").await;
    send(&mut stream, "y").await;

    read_until(&mut stream, &mut t, "Password: ").await;
    send(&mut stream, "hunter2").await;

    read_until(&mut stream, &mut t, "Gender (M/F): ").await;
    send(&mut stream, "F").await;

    read_until(&mut stream, &mut t, "Please choose a race from the following list:").await;
    send(&mut stream, "1").await;

    read_until(&mut stream, &mut t, "Please choose a class from the following list:").await;
    send(&mut stream, "1").await;

    read_until(&mut stream, &mut t, "Do you want to be Lawful?").await;
    send(&mut stream, "No").await;

    // Oracle flow: creation ends on the stat sheet + prompt, not the room.
    read_until(&mut stream, &mut t, "Hits:").await;
    read_until(&mut stream, &mut t, "[HP=").await;
    send(&mut stream, "look").await;
    read_until(&mut stream, &mut t, "Town Gates").await;
    read_until(&mut stream, &mut t, "Obvious exits: north").await;

    send(&mut stream, "n").await;
    read_until(&mut stream, &mut t, "Town Square").await;

    send(&mut stream, "x").await;
    // Server closes the connection after quit.
    let mut buf = [0u8; 256];
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("timely close")
            .expect("read")
        {
            0 => break,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn returning_player_resumes_saved_character() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");
    let addr = server.local_addr();

    // First visit: create and walk north, then quit (position persists).
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut t = String::new();
    read_until(&mut stream, &mut t, "Account: ").await;
    send(&mut stream, "Bob").await;
    read_until(&mut stream, &mut t, "Create new account? (y/n)").await;
    send(&mut stream, "y").await;
    read_until(&mut stream, &mut t, "Password: ").await;
    send(&mut stream, "pw").await;
    read_until(&mut stream, &mut t, "Gender (M/F): ").await;
    send(&mut stream, "M").await;
    read_until(&mut stream, &mut t, "race").await;
    send(&mut stream, "1").await;
    read_until(&mut stream, &mut t, "class").await;
    send(&mut stream, "1").await;
    read_until(&mut stream, &mut t, "Lawful").await;
    send(&mut stream, "No").await;
    read_until(&mut stream, &mut t, "[HP=").await;
    send(&mut stream, "look").await;
    read_until(&mut stream, &mut t, "Town Gates").await;
    send(&mut stream, "n").await;
    read_until(&mut stream, &mut t, "Town Square").await;
    send(&mut stream, "x").await;
    // Wait for the meditation to finish and the server to close (persist
    // completes before we reconnect — avoids racing the save).
    let mut buf = [0u8; 256];
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("timely close")
            .expect("read")
        {
            0 => break,
            _ => continue,
        }
    }

    // Second visit: no creation, resumes in Town Square.
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut t = String::new();
    read_until(&mut stream, &mut t, "Account: ").await;
    send(&mut stream, "Bob").await;
    read_until(&mut stream, &mut t, "Password: ").await;
    send(&mut stream, "pw").await;
    read_until(&mut stream, &mut t, "Town Square").await;
    assert!(
        !t.contains("choose a race"),
        "no creation on return: {t}"
    );
}

#[tokio::test]
async fn wrong_password_is_rejected() {
    let state = StateDb::open_in_memory().expect("state db");
    state
        .create_account("Carol", "right", mud_core::game::Gender::Female)
        .expect("account");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr()).await.expect("connect");
    let mut t = String::new();
    read_until(&mut stream, &mut t, "Account: ").await;
    send(&mut stream, "Carol").await;
    read_until(&mut stream, &mut t, "Password: ").await;
    send(&mut stream, "wrong").await;
    read_until(&mut stream, &mut t, "Invalid credentials.").await;
}
