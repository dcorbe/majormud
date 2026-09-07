//! A window writes the death log. One test in this binary, because it
//! sets `XDG_CONFIG_HOME` for the whole process and nothing else may
//! run beside it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::settings::Settings;
use mud_client::spawn::SpawnTable;
use mud_client::tui::ContentCache;
use mud_client::window::{EventKind, FrontMsg, spawn};
use mud_core::content::{Content, Direction, RoomId};

const DEAD: RoomId = RoomId { map: 1, room: 2810 };
const RECALL: RoomId = RoomId { map: 1, room: 1 };

/// A board that greets, shows the room the character is standing in,
/// waits for the window to settle, then kills it and recalls it, and
/// holds the line open.
///
/// The death and the room the character wakes up in go out in ONE
/// write, which is what the live board does: the recall block follows
/// the killing prompt with nothing in between. Both are ready before
/// the window task is polled, and the window reads them through two
/// different arms of one select, so this is the arrival order that lets
/// a recall room be logged as the room the character died in.
async fn killing_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(
            b"Welcome to the Test Board\r\n\x1b[1;36mDark Cave\r\nObvious exits: west\r\n[HP=22/MA=0]:",
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut killed = String::from("\r\n");
        // Chatter ahead of the death, because the window reads events
        // one per turn of its select while the room it holds is
        // refreshed in one step. Without these the death is the first
        // event of the write and the race is nearly always won by the
        // arm that reads it, which hides the bug this test is for.
        for _ in 0..40 {
            killed.push_str("Water drips from the ceiling.\r\n");
        }
        killed.push_str("You have been killed.\r\n[HP=0/MA=0]:");
        killed.push_str("\r\n\x1b[1;36mMain Street\r\nObvious exits: north\r\n[HP=1/MA=0]:");
        sock.write_all(killed.as_bytes()).await.unwrap();
        let mut hold = [0u8; 256];
        while sock.read(&mut hold).await.unwrap_or(0) > 0 {}
    });
    addr
}

fn edge(dest: RoomId) -> ExitEdge {
    ExitEdge {
        dest,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    }
}

/// Two rooms, one hop apart, named as the board prints them: the cave
/// the character dies in and the street it wakes up on.
fn world() -> (Arc<RoomGraph>, Arc<SpawnTable>, Arc<Content>) {
    let mut cave = GraphRoom {
        name: "Dark Cave".into(),
        ..Default::default()
    };
    cave.exits[Direction::West as usize] = Some(edge(RECALL));
    let mut street = GraphRoom {
        name: "Main Street".into(),
        ..Default::default()
    };
    street.exits[Direction::North as usize] = Some(edge(DEAD));
    let graph = RoomGraph::from_rooms(vec![(DEAD, cave), (RECALL, street)]);
    (
        Arc::new(graph),
        Arc::new(SpawnTable::default()),
        Arc::new(Content::default()),
    )
}

#[tokio::test]
async fn a_death_in_hand_play_is_logged_in_the_room_it_happened_in() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("deathlog_window");
    let _ = std::fs::remove_dir_all(&base);
    // Before any other task exists in this process. The only test in
    // this binary, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let addr = killing_board().await;
    // Never opened. The window asks the cache for this path and the
    // cache already has an answer.
    let db = base.join("world.sqlite");
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beef\"").unwrap();
    s.set("farm.content", &format!("{:?}", db.to_string_lossy())).unwrap();
    let mut cache = ContentCache::default();
    cache.insert(db, world());
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn(7, s, 24, 80, Arc::new(Mutex::new(cache)), tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut connected = false;
    loop {
        let msg = tokio::time::timeout_at(deadline, front.recv())
            .await
            .expect("timed out waiting for the death to be logged")
            .expect("the window task ended");
        if matches!(msg, FrontMsg::Event { kind: EventKind::Connected, .. }) {
            connected = true;
        }
        if handle.screen.lock().unwrap().text().contains("death logged") {
            break;
        }
    }
    assert!(connected);
    // The recall block came in the same write, so waiting for it is
    // waiting for everything the window could still act on.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !handle.screen.lock().unwrap().text().contains("Main Street") {
        tokio::time::timeout_at(deadline, front.recv())
            .await
            .expect("timed out waiting for the recall room")
            .expect("the window task ended");
    }
    // Long enough for a second line to have been written if anything
    // were going to write one.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let text = std::fs::read_to_string(base.join("mmc").join("deaths.log")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one death, one line: {text}");
    // `beef` is the profile's username, and this board printed no stat
    // sheet, so it is the only name the client ever had. The room is
    // the cave, never the street the character was recalled to.
    assert!(lines[0].ends_with(" beef 1/2810 Dark Cave confirmed"), "{text}");
}

