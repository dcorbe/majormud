//! `Navigator::arm_sneak` -- Task 5, "the client learns to sneak".
//!
//! Drives a scripted board over a real TCP socket, the same pattern
//! `tests/nav_doors.rs` uses for wire-shaped behaviour `tests/nav.rs`'s
//! in-process server does not model. Wordings come straight off
//! `mud-core`'s `sneak_command` (`crates/mud-core/src/game.rs`) and
//! `text::MAY_NOT_SNEAK`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, Navigator, NoGuard};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HERE: RoomId = RoomId { map: 1, room: 1 };
const THERE: RoomId = RoomId { map: 1, room: 2 };

#[derive(Default)]
struct SneakLog {
    sneaks: AtomicUsize,
    moves: AtomicUsize,
}

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:")
}

/// A board with one plain (doorless) exit north from HERE to THERE,
/// whose `sneak` reply is whatever `sneak_reply` says -- the raw body
/// text between the echo and the closing prompt, exactly as `mud-core`
/// would print it (a bare "Attempting to sneak...", that plus the
/// perception-gated failure line, or the hard-block refusal alone).
async fn sneak_board(sneak_reply: &'static str) -> (std::net::SocketAddr, Arc<SneakLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(SneakLog::default());
    let counter = Arc::clone(&log);
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
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            let echo = format!("\r\n{line}");
            let reply = match line.as_str() {
                "sneak" => {
                    counter.sneaks.fetch_add(1, Ordering::SeqCst);
                    format!("\r\n{sneak_reply}\r\n[HP=30/MA=0]:")
                }
                "n" | "north" => {
                    counter.moves.fetch_add(1, Ordering::SeqCst);
                    room_block("Inner Ward", "south")
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });
    (addr, log)
}

fn graph_one_hop() -> Arc<RoomGraph> {
    let mut here = GraphRoom {
        name: "Guard Post".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    here.exits[Direction::North as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::from_exit_type(0, 0),
    });
    let mut there = GraphRoom {
        name: "Inner Ward".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    there.exits[Direction::South as usize] = Some(ExitEdge {
        dest: HERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::from_exit_type(0, 0),
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

fn nav(graph: Arc<RoomGraph>, stealth: u32) -> Navigator {
    nav_with_timeout(graph, stealth, 1500)
}

fn nav_with_timeout(graph: Arc<RoomGraph>, stealth: u32, step_timeout_ms: u64) -> Navigator {
    Navigator::new(graph, NavConfig { step_timeout_ms, ..NavConfig::default() })
        .with_capabilities(Capabilities { stealth, ..Capabilities::unrestricted() })
}

/// The ordinary case: a bare "Attempting to sneak..." with no failure
/// line is the best evidence this reply shape ever gives, and is
/// treated as armed.
#[tokio::test]
async fn a_bare_attempt_with_no_failure_line_is_believed_armed() {
    let (addr, log) = sneak_board("Attempting to sneak...").await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard)
        .await
        .unwrap();
    assert!(arrival.sneaking, "a bare attempt with nothing else must read as armed");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 1);
    assert_eq!(log.moves.load(Ordering::SeqCst), 1);
}

/// The perception-gated failure line, when it does show up, must be
/// believed over the optimistic default.
#[tokio::test]
async fn a_perceived_failure_is_not_believed_armed() {
    let (addr, _log) = sneak_board("Attempting to sneak...\r\nYou don't think you're sneaking.").await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard)
        .await
        .unwrap();
    assert!(!arrival.sneaking, "a seen failure must not be believed armed");
}

/// Mutation target (Task 5 Step 5, case 2): a hard block must leave the
/// client unarmed, and must not stop the walk -- it moves anyway,
/// unsneaked.
#[tokio::test]
async fn a_hard_block_is_not_believed_armed_and_does_not_stop_the_walk() {
    let (addr, log) = sneak_board("You may not sneak right now!").await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard)
        .await
        .unwrap();
    assert!(!arrival.sneaking, "\"You may not sneak right now!\" must not be believed armed");
    assert_eq!(arrival.at, THERE, "a hard block must not stop the walk");
    assert_eq!(log.moves.load(Ordering::SeqCst), 1, "no retry this step");
}

/// Mutation target (Task 5 Step 5, case 1): a Stealth-0 character must
/// never send `sneak` at all.
#[tokio::test]
async fn zero_stealth_never_sends_sneak() {
    let (addr, log) = sneak_board("Attempting to sneak...").await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 0);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard)
        .await
        .unwrap();
    assert!(!arrival.sneaking);
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 0, "stealth:0 must not send sneak");
    assert_eq!(log.moves.load(Ordering::SeqCst), 1, "the walk still moves");
}

/// Total silence -- no "Attempting to sneak..." at all, as `mud-core`
/// prints when `delay_blocked` -- must not be believed armed either:
/// unlike the bare-attempt case, there is no attempt to be optimistic
/// about.
#[tokio::test]
async fn silence_is_not_believed_armed() {
    let (addr, log) = sneak_board("").await;
    let session = session_for(addr).await;
    let navigator = nav_with_timeout(graph_one_hop(), 56, 400);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard)
        .await
        .unwrap();
    assert!(!arrival.sneaking, "silence must not be believed armed");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 1);
}
