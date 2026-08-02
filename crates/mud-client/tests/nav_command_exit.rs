//! Command exits: the ones you cannot walk.
//!
//! Live incident (2026-08-02, cwrun6.raw): a `/go` from Newhaven to
//! Silvermere stalled at `1/2149` Newhaven Docks. The graph has a south
//! exit to the Small Pier and the navigator dutifully sent `s`, but the
//! board only moves you there for `borrow skiff` — the direction word
//! does nothing whatsoever, so the step waited out its whole deadline
//! and the walk reported a timeout at the docks.
//!
//! 250 exits in the shipped world are of this kind (`roomtype == 10`):
//! `go manhole` into the Silvermere sewers, `climb tree` in the Tasloi
//! village, `pull lever`, `push button`, `go portal`.
//!
//! Harness is the `nav_echo.rs` shape — test crates do not share
//! modules, so it is duplicated, which is this suite's existing pattern.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mud_client::graph::{COMMAND_EXIT, ExitEdge, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, Navigator, NoGuard};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DOCKS: RoomId = RoomId { map: 1, room: 2149 };
const PIER: RoomId = RoomId { map: 1, room: 2335 };

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:")
}

/// The docks and the pier, joined by the ferry exit.
fn ferry(command: Option<&str>) -> Arc<RoomGraph> {
    let mut docks = GraphRoom {
        name: "Newhaven, Docks".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    docks.exits[Direction::South as usize] = Some(ExitEdge {
        dest: PIER,
        exit_type: COMMAND_EXIT,
        command: command.map(str::to_string),
    });
    let pier = GraphRoom {
        name: "Small Pier".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    Arc::new(RoomGraph::from_rooms(vec![(DOCKS, docks), (PIER, pier)]))
}

async fn session_for(addr: std::net::SocketAddr) -> Session {
    let profile = Profile {
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
    Session::connect(&profile, None).await.unwrap()
}

fn nav(g: Arc<RoomGraph>) -> Navigator {
    Navigator::new(
        g,
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
        },
    )
}

/// A board that models the real ferryman: the direction word is
/// answered with nothing at all (as the live board does — `s` at the
/// docks produces no reply), and only the phrase moves you.
///
/// Returns every line the board received, so the test can assert what
/// was actually spoken rather than only where we ended up.
async fn ferry_board() -> (std::net::SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&log);
    let crossings = AtomicUsize::new(0);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Newhaven, Docks", "north south").as_bytes())
            .await
            .unwrap();
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
                seen.lock().unwrap().push(line.clone());
                let reply = if line.contains("skiff") {
                    crossings.fetch_add(1, Ordering::Relaxed);
                    format!(
                        "\r\n{line}\r\nYou climb into one of the skiffs, and row to Silvermere.{}",
                        room_block("Small Pier", "north")
                    )
                } else if line == "s" {
                    // The live board's answer to walking a command exit:
                    // silence. Only the echo comes back.
                    format!("\r\n{line}\r\n[HP=30/MA=0]:")
                } else {
                    format!("\r\n{line}\r\nYou say \"{line}\"\r\n[HP=30/MA=0]:")
                };
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        }
    });
    (addr, log)
}

/// The fix: the phrase is sent, the crossing happens, and the direction
/// word is never spoken at all.
#[tokio::test]
async fn a_command_exit_is_spoken_not_walked() {
    let (addr, log) = ferry_board().await;
    let session = session_for(addr).await;
    let nav = nav(ferry(Some("borrow skiff")));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        nav.goto(&session, DOCKS, PIER, &mut NoGuard),
    )
    .await
    .expect("must not hang")
    .unwrap_or_else(|e| panic!("the ferry must be crossable: {e}\nboard heard: {:?}", log.lock().unwrap()));

    assert_eq!(at, PIER);
    let heard = log.lock().unwrap().clone();
    assert!(
        heard.iter().any(|l| l == "borrow skiff"),
        "the phrase must be sent: {heard:?}"
    );
    assert!(
        !heard.iter().any(|l| l == "s"),
        "the direction word does nothing here and must not be sent: {heard:?}"
    );
}

/// The regression itself. Without the command the navigator falls back
/// to the direction word, which the board ignores — so the step burns
/// its deadline and the walk reports failure standing at the docks.
/// This is exactly what the live `/go` did.
#[tokio::test]
async fn without_the_command_the_walk_stalls_at_the_dock() {
    let (addr, log) = ferry_board().await;
    let session = session_for(addr).await;
    let nav = nav(ferry(None));

    let err = tokio::time::timeout(
        Duration::from_secs(10),
        nav.goto(&session, DOCKS, PIER, &mut NoGuard),
    )
    .await
    .expect("must not hang")
    .expect_err("a direction word cannot cross the ferry");

    assert_eq!(err.at, DOCKS, "and it knows where it is stuck");
    let heard = log.lock().unwrap().clone();
    assert!(heard.iter().any(|l| l == "s"), "{heard:?}");
    assert!(
        !heard.iter().any(|l| l.contains("skiff")),
        "nothing taught it the phrase: {heard:?}"
    );
}

/// The graph is where the phrase comes from, so it has to survive the
/// load. Against the shipped world, not a fixture.
#[test]
fn the_shipped_world_carries_the_ferry_command() {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    if !db.exists() {
        eprintln!("skipping: {} not present", db.display());
        return;
    }
    let g = RoomGraph::load(&db).expect("load room graph");
    let docks = g.room(DOCKS).expect("Newhaven, Docks");
    let south = docks.exits[Direction::South as usize]
        .as_ref()
        .expect("the ferry exit");
    assert_eq!(south.dest, PIER);
    assert_eq!(south.exit_type, COMMAND_EXIT);
    assert_eq!(south.command.as_deref(), Some("borrow skiff"));

    // The manhole into the Silvermere sewers, same mechanism.
    let brass = g.room(RoomId { map: 1, room: 17 }).expect("Brass & River");
    let down = brass.exits[Direction::Down as usize].as_ref().expect("manhole");
    assert_eq!(down.command.as_deref(), Some("go manhole"));

    // And an ordinary exit carries nothing, so the navigator keeps
    // walking every other room in the world with a direction word.
    let gates = g.room(RoomId { map: 1, room: 1 }).expect("Town Gates");
    assert!(
        gates.exits[Direction::North as usize]
            .as_ref()
            .is_some_and(|e| e.command.is_none())
    );
}

/// Every command exit in the shipped world resolves to a phrase, so the
/// walk never silently falls back to a direction word that does nothing.
#[test]
fn every_command_exit_in_the_world_has_a_phrase() {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    if !db.exists() {
        eprintln!("skipping: {} not present", db.display());
        return;
    }
    let g = RoomGraph::load(&db).expect("load room graph");
    let mut total = 0;
    let mut mute = Vec::new();
    for (id, room) in g.iter() {
        for edge in room.exits.iter().flatten() {
            if edge.exit_type == COMMAND_EXIT {
                total += 1;
                if edge.command.is_none() {
                    mute.push((id, room.name.clone()));
                }
            }
        }
    }
    assert_eq!(total, 250, "the shipped count of command exits");
    // One exit's message is blank in the data itself (a Portal Room
    // link); it is a data gap, not a loader bug, and pinning it keeps a
    // regression in the join from hiding behind it.
    assert_eq!(mute.len(), 1, "unexpected mute command exits: {mute:?}");
}
