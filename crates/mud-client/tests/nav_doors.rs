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

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
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
    picks: AtomicUsize,
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
    door_board_worded(locked, "The door is closed!").await
}

/// The same board, but saying `refusal` when a shut door turns the step
/// back. Stock prints the bang; foreign reimplementations soften it, and
/// the navigator has to recognise a blocked door either way or it never
/// reaches for `open`.
async fn door_board_worded(
    locked: bool,
    refusal: &'static str,
) -> (std::net::SocketAddr, Arc<DoorLog>) {
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
                        format!("\r\n{refusal}\r\n[HP=30/MA=0]:")
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
        light: 0,
        ..Default::default()
    };
    here.exits[Direction::North as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type,
        command: None,
        requirement: ExitRequirement::from_exit_type(exit_type, 0, 0, 0),
    });
    let mut there = GraphRoom {
        name: "Inner Ward".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    there.exits[Direction::South as usize] = Some(ExitEdge {
        dest: HERE,
        exit_type,
        command: None,
        requirement: ExitRequirement::from_exit_type(exit_type, 0, 0, 0),
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

/// A board where the lock and the latch are SEPARATE, which is how the
/// real one behaves: `picklock` answers "You unlocked the door." and the
/// door still stands shut, so it must then be opened before it can be
/// walked through.
async fn pick_then_open_board() -> (std::net::SocketAddr, Arc<DoorLog>) {
    pick_then_open_board_worded("door").await
}

/// The same board with `leaf` as the word the board uses for what is in
/// the way: "door" for a type-7 exit, "gate" for a type-0xb one. The
/// success line is theft.md §8.5's "You successfully unlocked the %s.",
/// which is what the live board prints (test.raw 2026-09-05: "You
/// successfully unlocked the gate." at the graveyard gates, after which
/// the walk sent nothing at all).
async fn pick_then_open_board_worded(leaf: &'static str) -> (std::net::SocketAddr, Arc<DoorLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(DoorLog::default());
    let counter = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut locked = true;
        let mut open = false;
        sock.write_all(room_block("Guard Post", &format!("closed {leaf} north")).as_bytes())
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
                "n" | "north" => {
                    counter.moves.fetch_add(1, Ordering::SeqCst);
                    if open {
                        room_block("Inner Ward", &format!("open {leaf} south"))
                    } else {
                        format!("\r\nThe {leaf} is closed.\r\n[HP=30/MA=0]:")
                    }
                }
                "open n" | "open north" => {
                    counter.opens.fetch_add(1, Ordering::SeqCst);
                    if locked {
                        format!("\r\nThe {leaf} is locked.\r\n[HP=30/MA=0]:")
                    } else {
                        open = true;
                        format!("\r\nThe {leaf} is now open.\r\n[HP=30/MA=0]:")
                    }
                }
                "picklock n" | "picklock north" => {
                    counter.picks.fetch_add(1, Ordering::SeqCst);
                    // Unlocked, NOT open. This is the whole point.
                    locked = false;
                    format!("\r\nYou successfully unlocked the {leaf}.\r\n[HP=30/MA=0]:")
                }
                "bash n" | "bash north" => {
                    counter.bashes.fetch_add(1, Ordering::SeqCst);
                    open = true;
                    format!("\r\nYou bashed the {leaf} open.\r\n[HP=30/MA=0]:")
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });
    (addr, log)
}

/// A navigator walking a character with NO Picklocks, so the force path
/// is what gets tested.
///
/// Picking is tried BEFORE bashing whenever the character has the skill
/// (`Navigator::new`'s default `Capabilities` is
/// [`Capabilities::unrestricted`], which reads as "can pick anything"),
/// so a test that means to exercise a bash has to hand the walker a
/// character who cannot — otherwise the lock gives to a pick and the
/// bash it asserts never happens. Whether picking is attempted at all is
/// no longer a config switch: it is read off the character, the same
/// way an empty purse — not a setting — is what makes a toll
/// unaffordable.
fn forcing_nav(graph: Arc<RoomGraph>) -> Navigator {
    Navigator::new(
        graph,
        NavConfig {
            step_timeout_ms: 1500,
            ..NavConfig::default()
        },
    )
    .with_capabilities(Capabilities {
        picklocks: 0,
        ..Capabilities::unrestricted()
    })
}

/// A closed but unlocked door must be opened and walked through. This is
/// the Newhaven case: the Arena's north exit into the dungeon is a type-7
/// door, and until now it was a hard stop that had to be bashed by hand.
#[tokio::test]
async fn a_closed_door_is_opened_and_traversed() {
    let (addr, log) = door_board(false).await;
    let session = session_for(addr).await;
    let n = forcing_nav(graph_with_exit(7));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("should have got through the door");

    assert_eq!(at.at, THERE);
    assert_eq!(log.opens.load(Ordering::SeqCst), 1, "should have opened it once");
    assert_eq!(log.bashes.load(Ordering::SeqCst), 0, "bash costs HP; not needed here");
}

/// A LOCKED door does not yield to `open`. Giving up here is what left
/// the character stranded, so the navigator falls back to bashing.
#[tokio::test]
async fn a_locked_door_is_bashed() {
    let (addr, log) = door_board(true).await;
    let session = session_for(addr).await;
    let n = forcing_nav(graph_with_exit(7));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("should have bashed through");

    assert_eq!(at.at, THERE);
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
            ..NavConfig::default()
        },
    )
    .with_capabilities(Capabilities {
        picklocks: 0,
        ..Capabilities::unrestricted()
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
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
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
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
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
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
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("a dark room is still somewhere");

    assert_eq!(at.at, THERE, "position comes from the graph edge taken");
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
                    let attempt = counter.bashes.fetch_add(1, Ordering::SeqCst);
                    // Every second attempt sits on the action timer: the
                    // scold paces the roll, it must not spend the budget.
                    if attempt % 2 == 1 {
                        "\r\nYou must wait before you may do that!\r\n[HP=28/MA=0]:".to_string()
                    } else if failed < fails {
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
    let n = forcing_nav(graph_with_exit(7));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("the third roll opens it");
    assert_eq!(at.at, THERE);
    // Two real fails, the yield, and the interleaved scolds — five
    // sends, but only two spent the roll budget.
    assert_eq!(log.bashes.load(Ordering::SeqCst), 5, "scold, fail, scold, fail, yield");
}

/// A door that never gives ends in a bounded, diagnosable error — not a
/// hang, and not an unbounded HP drain.
#[tokio::test]
async fn a_door_that_never_yields_fails_cleanly_within_the_retry_budget() {
    let (addr, log) = rolling_door_board(usize::MAX).await;
    let session = session_for(addr).await;
    let n = forcing_nav(graph_with_exit(7));

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang");
    assert!(result.is_err(), "{result:?}");
    let bashes = log.bashes.load(Ordering::SeqCst);
    // 60 real rolls plus the interleaved scolds.
    assert!(
        (1..=121).contains(&bashes),
        "unbounded bashing: {bashes} attempts in {:?}",
        started.elapsed()
    );
}

/// The guard IS the health backstop for the HP each roll costs, so it
/// must be heard BETWEEN rolls: a monster spawning mid-door-work
/// otherwise swings freely at a character locked in the loop for up to
/// sixty rolls (measured live: a kobold thief spawned into the Arena
/// during exactly this door work, stopstate-run1).
#[tokio::test]
async fn a_guard_interrupt_breaks_the_bash_loop() {
    struct ArmOnHit;
    impl mud_client::nav::TravelGuard for ArmOnHit {
        fn on_event(&mut self, ev: &mud_client::events::Event) -> Option<mud_client::nav::Interrupt> {
            match ev {
                mud_client::events::Event::CombatHit { .. } => {
                    Some(mud_client::nav::Interrupt::Attacked { by: "kobold".into() })
                }
                _ => None,
            }
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(DoorLog::default());
    let counter = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
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
                "n" => "\r\nThe door is closed!\r\n[HP=30/MA=0]:".to_string(),
                "open n" => "\r\nThe door is locked.\r\n[HP=30/MA=0]:".to_string(),
                "bash n" => {
                    counter.bashes.fetch_add(1, Ordering::SeqCst);
                    // A monster is swinging while the roll fails.
                    "\r\nThe kobold thief slashes you for 5 damage!\r\nYour attempts to bash through fail!\r\n[HP=25/MA=0]:"
                        .to_string()
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });
    let session = session_for(addr).await;
    let n = forcing_nav(graph_with_exit(7));

    let err = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut ArmOnHit, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the guard must take the walk back");
    assert!(
        matches!(
            err.kind,
            mud_client::nav::NavErrorKind::Interrupted(mud_client::nav::Interrupt::Attacked { .. })
        ),
        "{err:?}"
    );
    assert!(
        log.bashes.load(Ordering::SeqCst) <= 2,
        "kept rolling with a monster swinging: {} bashes",
        log.bashes.load(Ordering::SeqCst)
    );
}

/// Door work under fire (live shape, cwgaming 2026-08-01: three mobs
/// whiffing at the character through four bash rolls). A whiff at us is
/// an Attacked interrupt everywhere else on a fighting walk; the bash
/// loop hears the guard between rolls and must hand back instead of
/// standing there rolling while something swings.
#[tokio::test]
async fn a_whiff_during_door_work_stops_the_walk() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bashes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&bashes);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
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
                "n" => "\r\nThe door is closed!\r\n[HP=30/MA=0]:".to_string(),
                "open n" | "open north" => "\r\nThe door is locked.\r\n[HP=30/MA=0]:".to_string(),
                "bash n" | "bash north" => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    // The failed roll, with the room's occupant swinging
                    // at us in the same burst.
                    "\r\nYour attempts to bash through fail!\r\nThe nasty giant rat lunges at you!\r\n[HP=30/MA=0]:"
                        .to_string()
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });

    let session = session_for(addr).await;
    let n = forcing_nav(graph_with_exit(7));
    let mut guard = mud_client::farm::FarmGuard::new(30, 25, "Farmer");

    let err = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut guard, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the whiff must stop the door work");
    assert!(
        matches!(
            err.kind,
            mud_client::nav::NavErrorKind::Interrupted(mud_client::nav::Interrupt::Entered {
                ..
            })
        ),
        "expected Entered, got {:?}",
        err.kind
    );
    assert!(
        bashes.load(Ordering::SeqCst) <= 2,
        "kept bashing under fire: {} rolls",
        bashes.load(Ordering::SeqCst)
    );
}

/// A foreign board softens the refusal to a full stop: "The door is
/// closed." — 7 occurrences in cwrun2.raw (cwgaming, 2026-08-01) and
/// never once a bang.
///
/// `correlate.rs` learned both terminators in b26afe5, so the move was
/// correctly RETIRED; the navigator's own table was not, so the same
/// line was never classified as a blocked door. The walk therefore never
/// reached for `open` or `bash` — it just sent `n` into a shut door
/// again on the next attempt, live, until the leg timed out.
///
/// Adding the full stop cannot collide with `_cmd_look`'s refusal: that
/// wording continues "...in that direction!" and never carries a period
/// at this position.
#[tokio::test]
async fn a_softened_door_refusal_is_still_a_blocked_door() {
    let (addr, log) = door_board_worded(false, "The door is closed.").await;
    let session = session_for(addr).await;
    let n = forcing_nav(graph_with_exit(7));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("a softened refusal must still open the door");

    assert_eq!(at.at, THERE);
    assert_eq!(
        log.opens.load(Ordering::SeqCst),
        1,
        "the refusal was never read as a blocked door, so `open` never went out"
    );
}

// --- picking, for a character who has the skill ----------------------

/// A locked door that wants Picklocks does not want force. The 88-bash
/// incident (cwgaming 2026-08-03, 1/1119) spent 36 hp of a 75-hp
/// character on a type-7 lock that was never going to yield to force,
/// and the character later died with nothing to show for it. A thief
/// picks it instead, for the cost of a command and no health at all.
///
/// `Navigator::new`'s default `Capabilities` is already
/// [`Capabilities::unrestricted`] — a character who can pick anything —
/// so nothing further needs supplying here; the point being tested is
/// only that `bash_doors: false` still finds a way through.
#[tokio::test]
async fn a_locked_door_is_picked_by_a_character_with_the_skill() {
    let (addr, log) = pick_then_open_board().await;
    let session = session_for(addr).await;
    let n = Navigator::new(
        graph_with_exit(7),
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
            ..NavConfig::default()
        },
    );

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("should have picked through");

    assert_eq!(at.at, THERE);
    assert!(log.picks.load(Ordering::SeqCst) >= 1, "a locked door needs a pick");
    assert_eq!(
        log.bashes.load(Ordering::SeqCst),
        0,
        "picking succeeded, so nothing should have been bashed"
    );
}

/// The graveyard gates (live, test.raw 2026-09-05): `picklock n` was
/// answered "You successfully unlocked the gate." and the walk then sent
/// nothing for the rest of the session. The unlocked wording is
/// theft.md §8.5's "You successfully unlocked the %s." with `gate` for a
/// type-0xb exit, and both the walker and the correlator had pinned the
/// `door` spelling.
#[tokio::test]
async fn a_locked_gate_is_picked_opened_and_walked_through() {
    let (addr, log) = pick_then_open_board_worded("gate").await;
    let session = session_for(addr).await;
    let n = Navigator::new(
        graph_with_exit(0xb),
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
            ..NavConfig::default()
        },
    );

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("should have picked the gate and walked through");

    assert_eq!(at.at, THERE);
    assert!(log.picks.load(Ordering::SeqCst) >= 1, "a locked gate needs a pick");
    assert!(
        log.opens.load(Ordering::SeqCst) >= 2,
        "the gate is unlocked, not open: it still owes an open after the pick"
    );
    assert_eq!(log.bashes.load(Ordering::SeqCst), 0);
}

/// A character with no Picklocks does not pick — the roll costs a
/// command for nothing, so there is no reason to try it. This used to be
/// an operator-managed switch; now it is read straight off the
/// character, the same way `forcing_nav` reads it.
#[tokio::test]
async fn a_character_with_no_picklocks_does_not_pick() {
    let (addr, log) = pick_then_open_board().await;
    let session = session_for(addr).await;
    let n = Navigator::new(
        graph_with_exit(7),
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
            ..NavConfig::default()
        },
    )
    .with_capabilities(Capabilities {
        picklocks: 0,
        ..Capabilities::unrestricted()
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang");

    assert!(result.is_err(), "a locked door with no skill and no bashing is a dead end");
    assert_eq!(log.picks.load(Ordering::SeqCst), 0, "must not pick with no Picklocks");
    assert_eq!(log.bashes.load(Ordering::SeqCst), 0, "must not bash when switched off");
}

/// A locked door is a KNOWN, INSTANT condition. Reporting it as a
/// timeout -- which is what shipped -- leaves the operator unable to
/// tell a shut door from a lagging board, and reads as though the client
/// hung (live, beef.raw 2026-08-22: "timed out waiting for room block
/// after movement" with an empty tail, at a door that had answered
/// immediately).
#[tokio::test]
async fn a_locked_door_says_it_is_locked_rather_than_timing_out() {
    let (addr, _log) = door_board(true).await;
    let session = session_for(addr).await;
    let n = Navigator::new(
        graph_with_exit(7),
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
            ..NavConfig::default()
        },
    )
    .with_capabilities(Capabilities {
        picklocks: 0,
        ..Capabilities::unrestricted()
    });

    let err = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("a locked door with no way through is an error");

    let said = err.to_string().to_lowercase();
    assert!(said.contains("locked"), "the reason must name the lock, got: {said}");
    assert!(
        !said.contains("timed out"),
        "an instant, known refusal must not masquerade as a timeout: {said}"
    );
}

/// A picked lock is UNLOCKED, not OPEN. The board answers "You unlocked
/// the door." and the door still stands shut, so the walk owes it an
/// `open` before the step. Sending the direction straight after the pick
/// walks into a closed door and the leg dies there — live, Daniel's
/// board 2026-08-22: "the picklocking worked, but it forgot to try
/// opening the door and then gave up".
#[tokio::test]
async fn a_picked_lock_is_opened_before_it_is_walked() {
    let (addr, log) = pick_then_open_board().await;
    let session = session_for(addr).await;
    let n = Navigator::new(
        graph_with_exit(7),
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
            ..NavConfig::default()
        },
    );

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("a picked and opened door is walkable");

    assert_eq!(at.at, THERE);
    assert!(log.picks.load(Ordering::SeqCst) >= 1, "the lock had to be picked");
    assert!(
        log.opens.load(Ordering::SeqCst) >= 2,
        "one open before the pick, and one after it: the pick unlocks, it does not open"
    );
}
