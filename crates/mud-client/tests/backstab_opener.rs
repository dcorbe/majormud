//! Task 6, "wiring the opener": `Navigator`'s decide -> swap -> sneak ->
//! move ordering, and `Bot::engage` consuming what a walk believed on
//! arrival.
//!
//! The nav half drives a scripted board over a real TCP socket (the
//! `tests/nav_doors.rs` / `tests/sneak.rs` pattern) against the real
//! WG3-NT item database, the same rows `tests/backstab.rs` and
//! `tests/items.rs` already established: quarterstaff (100) lacks
//! BSAccu, dagger (68) carries it. The bot half is the pure
//! decision-core pattern `tests/bot.rs` uses -- no sockets, no timing.

use std::sync::Arc;
use std::sync::Mutex;

use mud_client::bot::{Bot, BotAction, BotConfig};
use mud_client::events::{Event, RoomView};
use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, Navigator, NoGuard};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Content, Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn content() -> Arc<Content> {
    use std::sync::OnceLock;
    static C: OnceLock<Arc<Content>> = OnceLock::new();
    C.get_or_init(|| Arc::new(mud_core::content_db::load(&db_path()).expect("load content")))
        .clone()
}

// ---------------------------------------------------------------------
// nav.rs: decide -> swap -> sneak -> move
// ---------------------------------------------------------------------

const HERE: RoomId = RoomId { map: 1, room: 1 };
const THERE: RoomId = RoomId { map: 1, room: 2 };

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:")
}

/// Records every accepted command in receipt order, so the ordering
/// itself -- not just which commands were sent -- can be asserted.
async fn ordering_board() -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let counter = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Guard Post", "north").as_bytes())
            .await
            .unwrap();
        // Two quick unpaced sends (the swap, then `sneak`) can land in ONE
        // `read()` call -- pace_ms is 0 here specifically so the ordering
        // this test cares about is not hidden behind artificial spacing.
        // Splitting on newlines, not per-read, is what keeps that from
        // reading as a single garbled command instead of two, in order.
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        'read: while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(pos) = pending.find('\n') {
                let raw: String = pending.drain(..=pos).collect();
                let line = raw.trim().to_lowercase();
                if line.is_empty() {
                    continue;
                }
                counter.lock().unwrap().push(line.clone());
                let echo = format!("\r\n{line}");
                let reply = match line.as_str() {
                    "sneak" => "\r\nAttempting to sneak...\r\n[HP=30/MA=0]:".to_string(),
                    "n" | "north" => room_block("Inner Ward", "south"),
                    other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
                };
                if sock.write_all(format!("{echo}{reply}").as_bytes()).await.is_err() {
                    break 'read;
                }
            }
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

/// A character wielding a non-capable weapon with a capable one in the
/// pack must swap BEFORE sneaking, and sneak BEFORE moving -- equipping
/// breaks sneak, so a swap sent after would be too late for the sneak
/// that covers this very step
/// (`2026-08-22-inventory-and-backstab-design.md` "The backstab
/// decision").
///
/// This is Task 6 Step 5's mutation target: reordering the swap-send
/// and the sneak-send in `Navigator::goto` must fail this assertion.
#[tokio::test]
async fn the_swap_lands_before_sneak_which_lands_before_the_move() {
    let (addr, log) = ordering_board().await;
    let session = session_for(addr).await;
    let navigator = Navigator::new(graph_one_hop(), NavConfig {
        step_timeout_ms: 1500,
        ..NavConfig::default()
    })
    .with_capabilities(Capabilities { stealth: 56, ..Capabilities::unrestricted() })
    .with_backstab(content(), Some("quarterstaff".to_string()), vec!["dagger".to_string()]);

    let arrival = navigator.goto(&session, HERE, THERE, &mut NoGuard).await.unwrap();

    assert_eq!(
        log.lock().unwrap().as_slice(),
        ["eq dagger", "sneak", "n"],
        "decide -> swap -> sneak -> move, in that exact order"
    );
    assert_eq!(arrival.restore_weapon, Some("quarterstaff".to_string()));
    assert!(arrival.sneaking);
}

/// A dual-purpose wielded weapon needs no swap at all -- `eq` must never
/// appear on the wire.
#[tokio::test]
async fn a_dual_purpose_wielded_weapon_sends_no_swap() {
    let (addr, log) = ordering_board().await;
    let session = session_for(addr).await;
    let navigator = Navigator::new(graph_one_hop(), NavConfig {
        step_timeout_ms: 1500,
        ..NavConfig::default()
    })
    .with_capabilities(Capabilities { stealth: 56, ..Capabilities::unrestricted() })
    .with_backstab(content(), Some("dagger".to_string()), vec![]);

    let arrival = navigator.goto(&session, HERE, THERE, &mut NoGuard).await.unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), ["sneak", "n"]);
    assert_eq!(arrival.restore_weapon, None);
}

/// A navigator never told `with_backstab` sends no `eq` and reports no
/// restore weapon -- exactly today's behaviour for every existing
/// caller.
#[tokio::test]
async fn a_navigator_with_no_backstab_prep_sends_no_swap() {
    let (addr, log) = ordering_board().await;
    let session = session_for(addr).await;
    let navigator = Navigator::new(graph_one_hop(), NavConfig {
        step_timeout_ms: 1500,
        ..NavConfig::default()
    })
    .with_capabilities(Capabilities { stealth: 56, ..Capabilities::unrestricted() });

    let arrival = navigator.goto(&session, HERE, THERE, &mut NoGuard).await.unwrap();

    assert_eq!(log.lock().unwrap().as_slice(), ["sneak", "n"]);
    assert_eq!(arrival.restore_weapon, None);
}

// ---------------------------------------------------------------------
// bot.rs: `Bot::arm_backstab_opener` / `Bot::engage`
// ---------------------------------------------------------------------

fn view(also_here: &[&str]) -> RoomView {
    RoomView {
        name: "Arena, Blood Pit".into(),
        exits: vec!["closed door north".into(), "up".into()],
        also_here: also_here.iter().map(|s| s.to_string()).collect(),
        items: vec![],
        also_here_sgr: Vec::new(),
    }
}

fn room(also_here: &[&str]) -> Event {
    Event::RoomSeen(view(also_here))
}

fn combat_bot() -> Bot {
    Bot::new(BotConfig { auto_combat: true, ..BotConfig::default() })
}

/// A caller that never primes the opener gets today's unconditional
/// ordinary swing.
#[test]
fn no_opener_primed_is_an_ordinary_attack() {
    let mut bot = combat_bot();
    let actions = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(actions, vec![BotAction::Send("a thief".into())]);
}

/// A dual-purpose opener (no swap needed) sends only `bs`.
#[test]
fn an_armed_opener_with_no_restore_sends_only_backstab() {
    let mut bot = combat_bot();
    bot.arm_backstab_opener(true, None);
    let actions = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(actions, vec![BotAction::Send("bs thief".into())]);
}

/// A swapped opener restores the primary weapon right behind the
/// backstab, so it is back before the second round.
#[test]
fn an_armed_opener_with_a_restore_swaps_back_after_the_backstab() {
    let mut bot = combat_bot();
    bot.arm_backstab_opener(true, Some("quarterstaff".to_string()));
    let actions = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(
        actions,
        vec![
            BotAction::Send("bs thief".into()),
            BotAction::Send("eq quarterstaff".into()),
        ]
    );
}

/// `arm_backstab_opener(false, ..)` disarms it outright, whatever
/// `restore` says.
#[test]
fn disarming_the_opener_falls_back_to_an_ordinary_attack() {
    let mut bot = combat_bot();
    bot.arm_backstab_opener(false, Some("quarterstaff".to_string()));
    let actions = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(actions, vec![BotAction::Send("a thief".into())]);
}

/// The primed belief describes exactly ONE arrival. A second, later
/// engagement in the same room (the first target left, a new one
/// walked in) must not reuse it.
#[test]
fn the_opener_is_consumed_once_and_does_not_leak_into_a_later_fight() {
    let mut bot = combat_bot();
    bot.arm_backstab_opener(true, None);
    let first = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(first, vec![BotAction::Send("bs thief".into())]);

    // The thief leaves, ending the fight; a new monster walks in with
    // no fresh `arm_backstab_opener` call -- exactly what a caller that
    // only primes once per walk-arrival produces.
    let _ = bot.on_event(&Event::ActorLeft { name: "kobold thief".into(), to: None });
    let second = bot.on_event(&Event::ActorEntered { name: "giant rat".into(), from: None });
    assert_eq!(
        second,
        vec![BotAction::Send("a rat".into())],
        "a stale opener belief must not open a later, unrelated fight"
    );
}

/// The refusal is evidence the equipment model was wrong, recorded but
/// not retried (there is no retry seam: the opening round is already
/// spent).
#[test]
fn a_wrong_weapon_refusal_is_recorded_as_a_correction() {
    let mut bot = combat_bot();
    bot.arm_backstab_opener(true, None);
    let _ = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(bot.backstab_corrections(), 0);
    let actions = bot.on_event(&Event::Line("You cannot backstab with this weapon!".into()));
    assert!(actions.is_empty());
    assert_eq!(bot.backstab_corrections(), 1);
    assert_eq!(bot.engaged(), Some("kobold thief"), "the fight is not refused, only the mode");
}
