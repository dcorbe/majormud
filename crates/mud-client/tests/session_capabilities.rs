//! Whether the session's real capabilities actually reach a walker.
//!
//! Tasks 1-5 built a purse, a state-aware cost, a router that consults
//! it and a toll-direction learner -- and nothing production-side wired
//! any of it in. Every `Navigator` at every real call site took
//! `Capabilities::unrestricted()`, so a fact learned crossing the
//! Silvermere gate on one leg was thrown away with the `Navigator` that
//! learned it, and the next leg paid the toll all over again.
//!
//! This is the test that fails without the wiring: it drives a REAL
//! walk through `go::run_go` (the production call site at `go.rs:228`,
//! not a hand-built `Navigator`), across a toll edge that does not
//! actually charge, then builds a second, independent `Navigator` from
//! the same `Session` and checks it already knows the crossing is free
//! -- cheaply enough to route through it on an empty purse, which only
//! the shared fact can explain.
//!
//! The harness is the `go_scripted.rs` / `nav_toll.rs` shape -- test
//! crates do not share modules, so it is duplicated here, matching this
//! suite's existing pattern.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::farm::FarmConfig;
use mud_client::live::Live;
use mud_client::go::{GoEnd, go_config, run_go};
use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, Navigator};
use mud_client::profile::Profile;
use mud_client::purse::Purse;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};

/// A notices sink that keeps nothing. What a runner says at startup is
/// not what these tests are about.
fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

const GATE: RoomId = RoomId { map: 1, room: 1381 };
const BEYOND: RoomId = RoomId { map: 1, room: 1382 };

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=51/MA=9]:")
}

/// A one-hop toll gate, west from `GATE` to `BEYOND`, 5 gold either
/// direction (the graph does not know which way play actually charges;
/// that is exactly what the walk has to measure).
fn gate_graph() -> Arc<RoomGraph> {
    let mut gate = GraphRoom {
        name: "Silvermere Gate".into(),
        ..Default::default()
    };
    gate.exits[Direction::West as usize] = Some(ExitEdge {
        dest: BEYOND,
        exit_type: 4,
        command: None,
        requirement: ExitRequirement::Toll { gold: 5 },
    });
    let mut beyond = GraphRoom {
        name: "Beyond the Gate".into(),
        ..Default::default()
    };
    beyond.exits[Direction::East as usize] = Some(ExitEdge {
        dest: GATE,
        exit_type: 4,
        command: None,
        requirement: ExitRequirement::Toll { gold: 5 },
    });
    Arc::new(RoomGraph::from_rooms(vec![(GATE, gate), (BEYOND, beyond)]))
}

/// A board that starts the character at the gate, answers every `i` with
/// the same unchanging 10-gold balance (so a crossing never actually
/// deducts), walks `w` straight through, and echoes anything else.
async fn toll_go_board() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
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
                counter.fetch_add(1, Ordering::SeqCst);
                sock.write_all(
                    format!("{echo}\r\nYou are carrying 10 gold crowns\r\n[HP=51/MA=9]:").as_bytes(),
                )
                .await
                .unwrap();
                continue;
            }
            if line == "look" {
                sock.write_all(format!("{echo}{}", room_block("Silvermere Gate", "west")).as_bytes())
                    .await
                    .unwrap();
                continue;
            }
            if line == "w" {
                sock.write_all(format!("{echo}{}", room_block("Beyond the Gate", "east")).as_bytes())
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
        bank: Default::default(),
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

fn bot() -> mud_client::bot::BotConfig {
    mud_client::bot::BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..Default::default()
    }
}

/// No room database behind the two-room gate graph, which `run_go` must
/// treat as "no threat opinion" rather than as fatal.
fn cfg(walking: bool) -> FarmConfig {
    go_config(&FarmConfig {
        content: std::path::PathBuf::from("/nonexistent/rooms.sqlite"),
        fight_while_travelling: walking,
        ..Default::default()
    })
}

/// The property the whole task exists for: a toll fact learned by ONE
/// production walk must still be known to the NEXT navigator built from
/// the same session, even though the `Navigator` that learned it is long
/// gone.
///
/// Priming step first: `run_go` snapshots `session.capabilities()` the
/// moment it builds its `Navigator`, before it ever sends a line -- so
/// the very first crossing has to already find an affordable purse on
/// the session, exactly as a live character who checked `i` earlier in
/// the session would.
#[tokio::test]
async fn a_toll_learned_by_run_go_is_known_to_the_next_navigator() {
    let (addr, asks) = toll_go_board().await;
    let session = session_for(addr).await;

    // Prime the purse from OUTSIDE any Navigator, the way an operator's
    // own `i` (or `tui.rs`'s on-entry check) would before `/go` ever
    // runs.
    session.send("i");
    session
        .expect("You are carrying", Duration::from_secs(5))
        .await
        .expect("purse reply");
    assert_eq!(
        session.capabilities().purse,
        Purse::from_gold(10),
        "the session's own purse must already reflect the primed balance"
    );

    let graph = gate_graph();
    let end = tokio::time::timeout(
        Duration::from_secs(20),
        run_go(&session, Arc::clone(&graph), Some(GATE), &[BEYOND], Live::fixed(bot(), cfg(true)), None, &quiet()),
    )
    .await
    .expect("run_go should not hang")
    .expect("the gate is affordable and open");
    assert_eq!(end, GoEnd::Arrived(BEYOND));

    // The connection under test: `go.rs`'s Navigator learned this fact
    // and was then dropped when `run_go` returned. It must still be on
    // the session.
    let caps = session.capabilities();
    assert!(
        caps.tolls_known_free.is_free(GATE, Direction::West),
        "the session's own toll log must have learned the free crossing"
    );

    // A second, freshly constructed Navigator built from the SAME
    // session -- deliberately given a purse too small to pay the toll on
    // its own. If the fact above were not really shared (a fresh
    // `TollLog`, as `Capabilities::unrestricted()` hands out), routing
    // this edge would require paying, and an empty purse cannot pay.
    let broke = Capabilities {
        purse: Purse::ZERO,
        tolls_known_free: Arc::clone(&caps.tolls_known_free),
        ..Default::default()
    };
    let nav2 = Navigator::new(graph, NavConfig::default()).with_capabilities(broke);
    assert_eq!(
        nav2.route_from(GATE, BEYOND),
        Some(vec![Direction::West]),
        "a freshly built navigator from the same session must already know the gate is free"
    );

    assert_eq!(
        asks.load(Ordering::SeqCst),
        3,
        "one priming ask, plus one before and one after the crossing"
    );
}


/// The character's level comes off the stat sheet, the way picklocks
/// and stealth already do, so a safe route can compare a room against
/// it. `None` until a sheet has been read.
#[tokio::test]
async fn the_stat_sheet_level_reaches_the_walker() {
    const SHEET: &str = "\r\nstat\r\n\
Name: Beef                             Lives/CP:    9/100\r\n\
Race: Dark-Elf    Exp: 0               Perception:     43\r\n\
Class: Ninja      Level: 7             Stealth:        56\r\n\
Hits:    30/30    Armour Class:   0/0  Thievery:        0\r\n\
                                       Traps:          29\r\n\
                                       Picklocks:      31\r\n\
Strength:  40     Agility: 50          Tracking:       26\r\n\
Intellect: 50     Health:  30          Martial Arts:   51\r\n\
Willpower: 30     Charm:   40          MagicRes:       35\r\n\
[HP=30/MA=20]:";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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
            let reply = if line == "stat" {
                SHEET.to_string()
            } else {
                format!("\r\n{line}\r\nYou say \"{line}\"\r\n[HP=30/MA=20]:")
            };
            sock.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    let session = session_for(addr).await;
    assert_eq!(session.capabilities().level, None, "no sheet yet");
    session.send("stat");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while session.stats().level.is_none() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(session.capabilities().level, Some(7));
}
