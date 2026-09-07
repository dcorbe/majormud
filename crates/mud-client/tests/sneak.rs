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

/// How the scripted board's own sneak state behaves. The board models
/// the live rules (`theft.md` §11.1, settled 2026-09-04): a move made
/// while armed opens with "Sneaking...", a break on that move adds
/// "You make a sound as you enter the room!" after it, and a move made
/// unarmed says neither.
#[derive(Clone, Copy, Default)]
struct Board {
    /// The character is already sneaking when the client connects.
    starts_armed: bool,
    /// A `sneak` answered with the reply text actually arms. `false`
    /// models the reply shapes that do not (a seen failure, a hard
    /// block, silence) and the bare attempt that failed silently.
    arms: bool,
    /// The `sneak` (0-indexed) that fails silently despite `arms`: a
    /// bare "Attempting to sneak..." with nothing behind it.
    silent_fail_on_arm: Option<usize>,
    /// The move (0-indexed: 0 is the first `n`) whose transit roll
    /// breaks the sneak.
    break_on_move: Option<usize>,
}

/// A two-hop north/north corridor (Guard Post -> Inner Ward -> Keep)
/// whose `sneak` reply is `sneak_reply` -- the raw body text between
/// the echo and the closing prompt, exactly as `mud-core` would print
/// it -- and whose moves answer according to `board`.
async fn sneak_board(
    sneak_reply: &'static str,
    board: Board,
) -> (std::net::SocketAddr, Arc<SneakLog>) {
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
        let mut armed = board.starts_armed;
        let mut arm_index = 0usize;
        let mut move_index = 0usize;
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            let echo = format!("\r\n{line}");
            let reply = match line.as_str() {
                "sneak" => {
                    let idx = arm_index;
                    arm_index += 1;
                    counter.sneaks.fetch_add(1, Ordering::SeqCst);
                    armed = board.arms && board.silent_fail_on_arm != Some(idx);
                    format!("\r\n{sneak_reply}\r\n[HP=30/MA=0]:")
                }
                "n" | "north" => {
                    let idx = move_index;
                    move_index += 1;
                    counter.moves.fetch_add(1, Ordering::SeqCst);
                    let block = if idx == 0 {
                        room_block("Inner Ward", "north south")
                    } else {
                        room_block("Keep", "south")
                    };
                    let mut out = String::new();
                    if armed {
                        out.push_str("\r\nSneaking...");
                        if board.break_on_move == Some(idx) {
                            out.push_str("\r\nYou make a sound as you enter the room!");
                            armed = false;
                        }
                    }
                    out + &block
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });
    (addr, log)
}

const FAR: RoomId = RoomId { map: 1, room: 3 };

fn graph_two_hop() -> Arc<RoomGraph> {
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
        requirement: ExitRequirement::from_exit_type(0, 0, 0, 0),
    });
    let mut mid = GraphRoom {
        name: "Inner Ward".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    mid.exits[Direction::South as usize] = Some(ExitEdge {
        dest: HERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::from_exit_type(0, 0, 0, 0),
    });
    mid.exits[Direction::North as usize] = Some(ExitEdge {
        dest: FAR,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::from_exit_type(0, 0, 0, 0),
    });
    let mut far = GraphRoom {
        name: "Keep".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    far.exits[Direction::South as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::from_exit_type(0, 0, 0, 0),
    });
    Arc::new(RoomGraph::from_rooms(vec![(HERE, here), (THERE, mid), (FAR, far)]))
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
        requirement: ExitRequirement::from_exit_type(0, 0, 0, 0),
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
        requirement: ExitRequirement::from_exit_type(0, 0, 0, 0),
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
        bank: Default::default(),
        ..Default::default()
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
/// line is believed armed for this step -- no second `sneak` -- and the
/// move's own "Sneaking..." is what confirms it.
#[tokio::test]
async fn a_bare_attempt_is_confirmed_by_the_move() {
    let (addr, log) = sneak_board("Attempting to sneak...", Board { arms: true, ..Board::default() }).await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(arrival.sneaking, "the move said Sneaking...");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 1);
    assert_eq!(log.moves.load(Ordering::SeqCst), 1);
}

/// The perception-gated failure line, when it does show up, must be
/// believed over the optimistic default.
#[tokio::test]
async fn a_perceived_failure_is_not_believed_armed() {
    let (addr, _log) = sneak_board(
        "Attempting to sneak...\r\nYou don't think you're sneaking.",
        Board::default(),
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(!arrival.sneaking, "a seen failure must not be believed armed");
}

/// The live board prints the perceived failure on the SAME line as the
/// attempt, with no line break between them (`re/oracle/slice8_abbrevs`:
/// "Attempting to sneak...You don't think you're sneaking."). That line
/// must be read as a seen failure at once. Mutation target: match the
/// two wordings as whole lines only and neither is seen, so the reader
/// sits out its whole step deadline before moving on unarmed.
#[tokio::test]
async fn a_failure_glued_to_the_attempt_is_read_at_once() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...You don't think you're sneaking.",
        Board::default(),
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav_with_timeout(graph_one_hop(), 56, 5_000);
    let started = std::time::Instant::now();
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(!arrival.sneaking, "a seen failure must not be believed armed");
    assert_eq!(arrival.at, THERE);
    assert_eq!(log.moves.load(Ordering::SeqCst), 1);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "the failure line must close the reply, not the step deadline: took {:?}",
        started.elapsed()
    );
}

/// Mutation target (Task 5 Step 5, case 2): a hard block must leave the
/// client unarmed, and must not stop the walk -- it moves anyway,
/// unsneaked.
#[tokio::test]
async fn a_hard_block_is_not_believed_armed_and_does_not_stop_the_walk() {
    let (addr, log) = sneak_board("You may not sneak right now!", Board::default()).await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
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
    let (addr, log) = sneak_board("Attempting to sneak...", Board { arms: true, ..Board::default() }).await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 0);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
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
    let (addr, log) = sneak_board("", Board::default()).await;
    let session = session_for(addr).await;
    let navigator = nav_with_timeout(graph_one_hop(), 56, 400);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(!arrival.sneaking, "silence must not be believed armed");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 1);
}

/// Sneak PERSISTS (theft.md §11.1, corrected 2026-08-22 against
/// live-board play): one `sneak` should cover a walk of several rooms,
/// not one per step. Mutation target: make the client re-arm every
/// step regardless of belief and this fails (`log.sneaks` becomes 2).
#[tokio::test]
async fn one_sneak_covers_several_steps() {
    let (addr, log) = sneak_board("Attempting to sneak...", Board { arms: true, ..Board::default() }).await;
    let session = session_for(addr).await;
    let navigator = nav(graph_two_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, FAR, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(arrival.sneaking, "still believed sneaking at the far end");
    assert_eq!(arrival.at, FAR);
    assert_eq!(
        log.sneaks.load(Ordering::SeqCst),
        1,
        "one arm covers the whole two-step walk"
    );
    assert_eq!(log.moves.load(Ordering::SeqCst), 2, "both steps still walked");
}

/// The board's "You make a sound as you enter the room!" is the break
/// announcement, not decoration: seeing it must clear the belief so the
/// NEXT step re-arms with a fresh `sneak`. Mutation target: make the
/// break line not clear the belief and this fails (`log.sneaks` stays
/// at 1, and `arrival.sneaking` on the broken step stays `true`).
#[tokio::test]
async fn the_break_line_clears_the_belief_and_the_next_step_rearms() {
    // Break announced on move index 0 -- entering Inner Ward, the first
    // step of the two-step walk.
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board { arms: true, break_on_move: Some(0), ..Board::default() },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav(graph_two_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, FAR, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(
        arrival.sneaking,
        "re-armed by the second `sneak` before the final step"
    );
    assert_eq!(
        log.sneaks.load(Ordering::SeqCst),
        2,
        "arm once, break on the first step, re-arm once for the second"
    );
    assert_eq!(log.moves.load(Ordering::SeqCst), 2);
}

/// The belief is the CALLER's to carry: a walk told the character is
/// already sneaking must not spend a round re-arming. Live 2026-09-04
/// (test_timing.log): a roam issues one `goto` per room, and every one
/// of them started unarmed, so the client typed `sneak` before every
/// single step of a walk that was already sneaking. Mutation target:
/// ignore the carried belief and `log.sneaks` reads 1.
#[tokio::test]
async fn a_carried_belief_skips_the_arm() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board { starts_armed: true, arms: true, ..Board::default() },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav(graph_two_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, FAR, &mut NoGuard, true)
        .await
        .unwrap();
    assert!(arrival.sneaking, "still believed sneaking at the far end");
    assert_eq!(arrival.at, FAR);
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 0, "already sneaking: nothing to arm");
    assert_eq!(log.moves.load(Ordering::SeqCst), 2);
}

/// A carried belief is still just a belief: the break line clears it
/// and the next step re-arms, exactly as it does for one the walk
/// formed itself.
#[tokio::test]
async fn a_carried_belief_still_rearms_after_a_break() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board { starts_armed: true, arms: true, break_on_move: Some(0), ..Board::default() },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav(graph_two_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, FAR, &mut NoGuard, true)
        .await
        .unwrap();
    assert!(arrival.sneaking, "re-armed before the final step");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 1, "one re-arm after the break");
    assert_eq!(log.moves.load(Ordering::SeqCst), 2);
}

/// A walk of no steps hands the carried belief straight back: the
/// character did not move, so nothing about its sneak changed.
#[tokio::test]
async fn a_walk_of_no_steps_keeps_the_carried_belief() {
    let (addr, log) = sneak_board("Attempting to sneak...", Board { starts_armed: true, ..Board::default() }).await;
    let session = session_for(addr).await;
    let navigator = nav(graph_two_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, HERE, &mut NoGuard, true)
        .await
        .unwrap();
    assert!(arrival.sneaking);
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 0);
}

/// The same bare reply, but the arm failed silently (theft.md §11.1:
/// a failed sneak roll whose perception roll also misses prints
/// nothing). Three of some thirty did in the capture that settled
/// this (2026-09-04). The move says nothing, and that silence is the
/// answer: not sneaking. Mutation target: believe the bare attempt
/// over the move and `arrival.sneaking` stays `true`.
#[tokio::test]
async fn a_bare_attempt_that_failed_silently_is_read_off_the_move() {
    let (addr, log) = sneak_board("Attempting to sneak...", Board::default()).await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(!arrival.sneaking, "the move said nothing, so the arm did not take");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 1, "one attempt this step");
    assert_eq!(log.moves.load(Ordering::SeqCst), 1);
}

/// A silent arm failure costs exactly one unsneaked step: the move
/// reveals it, and the next step arms again.
#[tokio::test]
async fn a_silent_arm_failure_rearms_on_the_next_step() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board { arms: true, silent_fail_on_arm: Some(0), ..Board::default() },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav(graph_two_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, FAR, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(arrival.sneaking, "the second arm took, and the second move said so");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 2, "arm, silent failure, arm again");
    assert_eq!(log.moves.load(Ordering::SeqCst), 2);
}

/// A carried belief the board does not share -- the caller thought the
/// character was still sneaking, the board did not -- is corrected by
/// the first move's silence, and the next step arms. This is what makes
/// a caller's conservative clearing cheap in the other direction too:
/// whichever way the belief is wrong, one move puts it right.
#[tokio::test]
async fn a_carried_belief_the_board_contradicts_is_corrected_by_the_first_move() {
    let (addr, log) = sneak_board("Attempting to sneak...", Board { arms: true, ..Board::default() }).await;
    let session = session_for(addr).await;
    let navigator = nav(graph_two_hop(), 56);
    let arrival = navigator
        .goto(&session, HERE, FAR, &mut NoGuard, true)
        .await
        .unwrap();
    assert!(arrival.sneaking, "armed before the second step, confirmed by its move");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 1, "no arm on the first step, one on the second");
    assert_eq!(log.moves.load(Ordering::SeqCst), 2);
}

/// `bot.auto_sneak = false`. A stealthy character walks and never arms.
#[tokio::test]
async fn sneak_off_never_sends_sneak() {
    let (addr, log) = sneak_board("Attempting to sneak...", Board { arms: true, ..Board::default() }).await;
    let session = session_for(addr).await;
    let navigator = Navigator::new(
        graph_one_hop(),
        NavConfig { step_timeout_ms: 1500, sneak: false, ..NavConfig::default() },
    )
    .with_capabilities(Capabilities { stealth: 56, ..Capabilities::unrestricted() });
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(!arrival.sneaking);
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 0, "sneak off must not send sneak");
    assert_eq!(log.moves.load(Ordering::SeqCst), 1, "the walk still moves");
}

