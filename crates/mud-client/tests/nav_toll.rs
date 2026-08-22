//! Which way the Silvermere gate actually charges.
//!
//! Both directions of the gate carry `type=4, para1=5` in the room data
//! and only the OUTBOUND crossing charges in play. `re/docs/theft.md`
//! §8.1 does not name type 4 at all, and the real rule is in `move_user`
//! in the WCCMMUD decompile, which nobody has read. So instead of
//! guessing, the walk measures: it assumes every type-4 crossing
//! charges, and when one actually happens it compares the carried
//! balance before and after. If the balance did not move, that
//! `(room, direction)` is remembered free.
//!
//! The pessimism is deliberate and the direction matters: a wrong
//! "charges" wastes a detour, a wrong "free" could strand a character on
//! the wrong side of a gate it cannot pay to re-cross.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph, TollLog};
use mud_client::nav::{NavConfig, Navigator, NoGuard};
use mud_client::profile::Profile;
use mud_client::purse::Purse;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const GATE: RoomId = RoomId { map: 1, room: 1381 };
const BEYOND: RoomId = RoomId { map: 1, room: 1382 };

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=51/MA=9]:")
}

/// A one-hop toll gate: `GATE` to `BEYOND` by `dir`, `type=4, para1=5`
/// both the graph and the fixture agree is 5 gold either way.
fn gate_graph(dir: Direction) -> Arc<RoomGraph> {
    let mut gate = GraphRoom {
        name: "Silvermere Gate".into(),
        ..Default::default()
    };
    gate.exits[dir as usize] = Some(ExitEdge {
        dest: BEYOND,
        exit_type: 4,
        command: None,
        requirement: ExitRequirement::Toll { gold: 5 },
    });
    let mut beyond = GraphRoom {
        name: "Beyond the Gate".into(),
        ..Default::default()
    };
    // The return edge exists in the data too (both directions carry the
    // toll), but no test here ever walks it.
    let back = match dir {
        Direction::West => Direction::East,
        Direction::East => Direction::West,
        _ => unreachable!("tests only use West/East"),
    };
    beyond.exits[back as usize] = Some(ExitEdge {
        dest: GATE,
        exit_type: 4,
        command: None,
        requirement: ExitRequirement::Toll { gold: 5 },
    });
    Arc::new(RoomGraph::from_rooms(vec![(GATE, gate), (BEYOND, beyond)]))
}

/// A board that answers `i` with a fixed purse the first time and either
/// the same or a reduced one the second time, and answers `dir_word` with
/// a room block for `BEYOND`.
async fn toll_board(dir_word: &'static str, charges: bool) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let inventory_asks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&inventory_asks);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Silvermere Gate", "west").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            let echo = format!("\r\n{line}");
            if line == "i" {
                let asked = counter.fetch_add(1, Ordering::SeqCst) + 1;
                let purse = if asked == 1 || !charges {
                    "You are carrying 10 gold crowns"
                } else {
                    "You are carrying 5 gold crowns"
                };
                sock.write_all(format!("{echo}\r\n{purse}\r\n[HP=51/MA=9]:").as_bytes())
                    .await
                    .unwrap();
                continue;
            }
            if line == dir_word {
                sock.write_all(
                    format!("{echo}{}", room_block("Beyond the Gate", "east")).as_bytes(),
                )
                .await
                .unwrap();
                continue;
            }
            let reply = format!("\r\nYou say \"{line}\"\r\n[HP=51/MA=9]:");
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });
    (addr, inventory_asks)
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

fn nav(graph: Arc<RoomGraph>, caps: Capabilities) -> Navigator {
    Navigator::new(
        graph,
        NavConfig {
            step_timeout_ms: 1500,
            ..NavConfig::default()
        },
    )
    .with_capabilities(caps)
}

/// Crossing a toll edge that does NOT deduct is remembered as free, and
/// the fact is visible on the shared log the walk was given -- the next
/// route computed with that same `Capabilities` prefers it.
///
/// The data says both directions of the Silvermere gate are tolled and
/// play says only outbound charges. The rule is in `move_user` in the
/// WCCMMUD decompile, unread -- so the client measures instead of
/// guessing, and corrects itself on first contact.
#[tokio::test]
async fn a_crossing_that_does_not_deduct_is_remembered_as_free() {
    let (addr, asks) = toll_board("w", false).await;
    let session = session_for(addr).await;
    let toll_log = Arc::new(TollLog::default());
    let caps = Capabilities {
        purse: Purse::from_gold(10),
        tolls_known_free: Arc::clone(&toll_log),
    };
    let n = nav(gate_graph(Direction::West), caps);

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, GATE, BEYOND, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("the gate is affordable and open");

    assert_eq!(at.at, BEYOND);
    assert!(
        toll_log.is_free(GATE, Direction::West),
        "an unchanged purse must be remembered as a free crossing"
    );
    assert_eq!(
        asks.load(Ordering::SeqCst),
        2,
        "purse must be read once before and once after the crossing"
    );
}

/// And one that DOES deduct stays tolled.
#[tokio::test]
async fn a_crossing_that_deducts_stays_tolled() {
    let (addr, asks) = toll_board("e", true).await;
    let session = session_for(addr).await;
    let toll_log = Arc::new(TollLog::default());
    let caps = Capabilities {
        purse: Purse::from_gold(10),
        tolls_known_free: Arc::clone(&toll_log),
    };
    let n = nav(gate_graph(Direction::East), caps);

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, GATE, BEYOND, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("the gate is affordable and open");

    assert_eq!(at.at, BEYOND);
    assert!(
        !toll_log.is_free(GATE, Direction::East),
        "a crossing that actually paid must not be remembered as free"
    );
    assert_eq!(asks.load(Ordering::SeqCst), 2);
}
