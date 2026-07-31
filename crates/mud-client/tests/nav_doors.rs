//! Door traversal.
//!
//! `tests/nav.rs` drives the in-process mud-server, which models closed
//! doors but not the open/bash command family (`game.rs` says so in as
//! many words). So doors are exercised here against a scripted board
//! whose wording comes from the shipped DLL:
//!
//! ```text
//! 0xbcab6  The door is closed!
//! 0xd6486  The door is now open.
//! 0xd6424  The door was already open.
//! 0xd649d  The door is locked.
//! 0xd54b0  You bashed the %s open.
//! ```
//!
//! Door/gate exit types are 2, 7 and 0xb (`theft.md` §8.1: "pickable
//! types are 2, 7, 0xb"); everything else is not a door and must not
//! provoke an open.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mud_client::graph::{ExitEdge, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, Navigator, NoGuard};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HERE: RoomId = RoomId { map: 1, room: 1 };
const THERE: RoomId = RoomId { map: 1, room: 2 };

/// How the scripted board was driven.
#[derive(Default)]
struct DoorLog {
    opens: AtomicUsize,
    bashes: AtomicUsize,
    moves: AtomicUsize,
}

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:")
}

/// A board with one door between HERE and THERE.
///
/// `locked` means `open` will not shift it and only a bash will, which
/// is the case that must not silently give up.
async fn door_board(locked: bool) -> (std::net::SocketAddr, Arc<DoorLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(DoorLog::default());
    let counter = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut open = false;
        // Greet with the room we start in, so the client has a prompt.
        sock.write_all(room_block("Guard Post", "closed door north").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            // The real board echoes every accepted line; the reply
            // follows the echo. The client's attribution stands on this.
            let echo = format!("\r\n{line}");
            let reply = match line.as_str() {
                "n" => {
                    counter.moves.fetch_add(1, Ordering::SeqCst);
                    if open {
                        room_block("Inner Ward", "open door south")
                    } else {
                        "\r\nThe door is closed!\r\n[HP=30/MA=0]:".to_string()
                    }
                }
                "open n" | "open north" => {
                    counter.opens.fetch_add(1, Ordering::SeqCst);
                    if locked {
                        "\r\nThe door is locked.\r\n[HP=30/MA=0]:".to_string()
                    } else {
                        open = true;
                        "\r\nThe door is now open.\r\n[HP=30/MA=0]:".to_string()
                    }
                }
                "bash n" | "bash north" => {
                    counter.bashes.fetch_add(1, Ordering::SeqCst);
                    open = true;
                    // The 0xd54b0 variant: opens it but does NOT walk
                    // you through, so the mover still owes a step.
                    "\r\nYou bashed the door open.\r\n[HP=30/MA=0]:".to_string()
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });
    (addr, log)
}

fn graph_with_exit(exit_type: i64) -> Arc<RoomGraph> {
    let mut here = GraphRoom {
        name: "Guard Post".into(),
        exits: Default::default(),
    };
    here.exits[Direction::North as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type,
    });
    let mut there = GraphRoom {
        name: "Inner Ward".into(),
        exits: Default::default(),
    };
    there.exits[Direction::South as usize] = Some(ExitEdge {
        dest: HERE,
        exit_type,
    });
    Arc::new(RoomGraph::from_rooms(vec![(HERE, here), (THERE, there)]))
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

fn nav(graph: Arc<RoomGraph>) -> Navigator {
    Navigator::new(
        graph,
        NavConfig {
            step_timeout_ms: 1500,
            ..NavConfig::default()
        },
    )
}

/// A closed but unlocked door must be opened and walked through. This is
/// the Newhaven case: the Arena's north exit into the dungeon is a type-7
/// door, and until now it was a hard stop that had to be bashed by hand.
#[tokio::test]
async fn a_closed_door_is_opened_and_traversed() {
    let (addr, log) = door_board(false).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(7));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("should have got through the door");

    assert_eq!(at, THERE);
    assert_eq!(log.opens.load(Ordering::SeqCst), 1, "should have opened it once");
    assert_eq!(log.bashes.load(Ordering::SeqCst), 0, "bash costs HP; not needed here");
}

/// A LOCKED door does not yield to `open`. Giving up here is what left
/// the character stranded, so the navigator falls back to bashing.
#[tokio::test]
async fn a_locked_door_is_bashed() {
    let (addr, log) = door_board(true).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(7));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("should have bashed through");

    assert_eq!(at, THERE);
    assert!(log.bashes.load(Ordering::SeqCst) >= 1, "a locked door needs a bash");
}

/// Bashing costs HP and needs a weapon, so it must stay behind the
/// config switch rather than being something the runner just does.
#[tokio::test]
async fn bashing_can_be_switched_off() {
    let (addr, log) = door_board(true).await;
    let session = session_for(addr).await;
    let n = Navigator::new(
        graph_with_exit(7),
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
        },
    );

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");

    assert!(result.is_err(), "a locked door with bashing off is a dead end");
    assert_eq!(log.bashes.load(Ordering::SeqCst), 0, "must not bash when switched off");
}

/// An ordinary exit is not a door. Sending `open` at every step would be
/// a wasted command against flood control on every move in the game.
#[tokio::test]
async fn a_plain_exit_is_never_opened() {
    let (addr, log) = door_board(false).await;
    let session = session_for(addr).await;
    // exit_type 0: a plain doorway. The board still refuses the first
    // move, so this also pins that we do not treat "the door is closed"
    // as an open-able door when the graph says there is none.
    let n = nav(graph_with_exit(0));

    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");

    assert_eq!(log.opens.load(Ordering::SeqCst), 0, "not a door: must not send open");
    assert_eq!(log.bashes.load(Ordering::SeqCst), 0, "not a door: must not bash");
}


/// The board refuses movement outright while something is fighting you
/// (DLL 0xbc6a8, "You may not enter that room while in combat."). No
/// deadline will produce a room block while that is true, so the walk has
/// to hand back at once and let the caller fight -- waiting it out cost a
/// full step timeout per attempt, which is how a patrol crossing the
/// Newhaven Arena died.
#[tokio::test]
async fn a_step_refused_for_combat_hands_back_at_once() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Guard Post", "north").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_string();
            sock.write_all(
                format!("\r\n{line}\r\nYou may not enter that room while in combat.\r\n[HP=30/MA=0]:")
                    .as_bytes(),
            )
            .await
            .unwrap();
        }
    });
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(0));

    let started = std::time::Instant::now();
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the step cannot land while in combat");

    assert!(
        matches!(
            err.kind,
            mud_client::nav::NavErrorKind::Interrupted(mud_client::nav::Interrupt::Attacked { .. })
        ),
        "should report being attacked, not a timeout: {:?}",
        err.kind
    );
    // The point of recognising the wording: no deadline was waited out.
    assert!(
        started.elapsed() < Duration::from_millis(1200),
        "handed back after {:?}; the step timeout is 1500ms",
        started.elapsed()
    );
}

/// A dark room sends NO room block — only "The room is very dark - you
/// can't see anything" (DLL 0xdf37e). Verified navigation confirms every
/// step by the destination's name, so that used to be a dead end: the
/// walk waited out its deadline for a name that was never coming.
///
/// Dead reckoning is sound HERE specifically, and only here: the board
/// says this on ENTERING such a room, so it is positive evidence the step
/// landed. The name then comes from the graph edge we chose — not from
/// assuming movement generally works, which is what verified navigation
/// exists to prevent.
#[tokio::test]
async fn a_dark_room_is_navigated_by_dead_reckoning() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Guard Post", "north").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_string();
            sock.write_all(
                format!("\r\n{line}\r\nThe room is very dark - you can't see anything\r\n[HP=30/MA=0]:")
                    .as_bytes(),
            )
            .await
            .unwrap();
        }
    });
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(0));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("a dark room is still somewhere");

    assert_eq!(at, THERE, "position comes from the graph edge taken");
}

/// A board whose door needs `fails` bash rolls before it gives — the
/// live 1/2150 door after a restart is exactly this (opendoor.lua leaned
/// on it up to 60 tries). Bash costs HP and the board says so; the
/// damage line is chatter, not an outcome.
async fn rolling_door_board(fails: usize) -> (std::net::SocketAddr, Arc<DoorLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(DoorLog::default());
    let counter = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut open = false;
        let mut failed = 0usize;
        sock.write_all(room_block("Guard Post", "closed door north").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            let echo = format!("\r\n{line}");
            let reply = match line.as_str() {
                "n" => {
                    counter.moves.fetch_add(1, Ordering::SeqCst);
                    if open {
                        room_block("Inner Ward", "open door south")
                    } else {
                        "\r\nThe door is closed!\r\n[HP=30/MA=0]:".to_string()
                    }
                }
                "open n" | "open north" => {
                    counter.opens.fetch_add(1, Ordering::SeqCst);
                    "\r\nThe door is locked.\r\n[HP=30/MA=0]:".to_string()
                }
                "bash n" | "bash north" => {
                    counter.bashes.fetch_add(1, Ordering::SeqCst);
                    if failed < fails {
                        failed += 1;
                        "\r\nYou take 2 damage for bashing the door!\r\nYour attempts to bash through fail!\r\n[HP=28/MA=0]:"
                            .to_string()
                    } else {
                        open = true;
                        "\r\nYou bashed the door open.\r\n[HP=28/MA=0]:".to_string()
                    }
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });
    (addr, log)
}

/// Bashing is a roll ("Your attempts to bash through fail!", captured
/// live in stopstate-run1) — one try was why the restart-locked door at
/// 1/2150 needed a hand-run script. The walk keeps rolling.
#[tokio::test]
async fn a_bash_that_fails_is_rolled_again_until_the_door_gives() {
    let (addr, log) = rolling_door_board(2).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(7));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("the third roll opens it");
    assert_eq!(at, THERE);
    assert_eq!(log.bashes.load(Ordering::SeqCst), 3, "two fails then the yield");
}

/// A door that never gives ends in a bounded, diagnosable error — not a
/// hang, and not an unbounded HP drain.
#[tokio::test]
async fn a_door_that_never_yields_fails_cleanly_within_the_retry_budget() {
    let (addr, log) = rolling_door_board(usize::MAX).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(7));

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");
    assert!(result.is_err(), "{result:?}");
    let bashes = log.bashes.load(Ordering::SeqCst);
    assert!(
        (1..=25).contains(&bashes),
        "unbounded bashing: {bashes} attempts in {:?}",
        started.elapsed()
    );
}
