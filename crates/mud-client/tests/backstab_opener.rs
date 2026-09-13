//! Task 6, "wiring the opener": `Navigator`'s decide -> swap -> sneak ->
//! move ordering, and `Bot::engage` consuming what a walk believed on
//! arrival.
//!
//! The nav half drives a scripted board over a real TCP socket (the
//! `tests/nav_doors.rs` / `tests/sneak.rs` pattern). The bot half is the
//! pure decision-core pattern `tests/bot.rs` uses -- no sockets, no timing.

use std::sync::Arc;
use std::sync::Mutex;

use mud_client::bot::{Bot, BotAction, BotConfig};
use mud_client::events::{Event, RoomView};
use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, Navigator, NoGuard};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    let arrival = navigator.goto(&session, HERE, THERE, &mut NoGuard, false).await.unwrap();

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

// ---------------------------------------------------------------------
// bot.rs: the second round after a backstab opener
// ---------------------------------------------------------------------

fn line(text: &str) -> Event {
    Event::Line(text.into())
}

fn our_hit(target: &str) -> Event {
    Event::CombatHit {
        attacker: mud_client::events::Actor::You,
        target: mud_client::events::Actor::Other(target.into()),
        damage: 65,
    }
}

/// The board drops a backstab to a plain attack the moment its first
/// blow lands, so the fight's own verb goes out on that blow: a mystic
/// who opened with `bs` and did nothing else spent every later round on
/// a single punch (cw-blueberry-4, 2026-09-13: surprise punch 72, then
/// punch 12, where `ju` swings two or three times a round).
#[test]
fn the_fights_verb_is_sent_again_when_the_surprise_blow_lands() {
    let mut bot = Bot::new(BotConfig {
        auto_combat: true,
        attack_command: "ju".into(),
        ..BotConfig::default()
    });
    bot.arm_backstab_opener(true, None);
    assert_eq!(bot.on_event(&room(&["shade"])), vec![BotAction::Send("bs shade".into())]);
    // The hit regex reads "surprise punch shade" as the target.
    assert_eq!(bot.on_event(&our_hit("punch shade")), vec![BotAction::Send("ju shade".into())]);
    assert_eq!(bot.engaged(), Some("shade"));
}

/// A verb re-sent mid-fight makes the board print `*Combat Off*` and
/// `*Combat Engaged*` back to back (live, every re-sent `a` and `ju` in
/// the 2026-09-13 captures). That Off is a mode switch, not the target
/// leaving: the fight stays engaged and no wander-out cooldown starts,
/// or the next block would refuse the monster still standing there.
#[test]
fn the_mode_switch_combat_off_ends_nothing() {
    let mut bot = combat_bot();
    bot.arm_backstab_opener(true, None);
    let _ = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(bot.on_event(&our_hit("punch kobold thief")), vec![BotAction::Send("a thief".into())]);
    assert!(bot.on_event(&line("*Combat Off*")).is_empty());
    assert!(bot.on_event(&line("*Combat Engaged*")).is_empty());
    assert_eq!(bot.engaged(), Some("kobold thief"), "the fight is still on");
    assert!(bot.on_event(&room(&["kobold thief"])).is_empty(), "already engaged");
    // The real ending, then a fresh thief: no cooldown stands in the way.
    let _ = bot.on_event(&line("You gain 40 experience."));
    let _ = bot.on_event(&line("*Combat Off*"));
    assert_eq!(bot.on_event(&room(&["kobold thief"])), vec![BotAction::Send("a thief".into())]);
}

/// Most surprise blows kill outright, and the death arrives in the same
/// burst as the blow. The verb is already out by then; the board answers
/// it with a quiet "Your command had no effect." and the kill's own
/// Combat Off must still be read as the ending it is.
#[test]
fn a_kill_in_the_same_burst_still_ends_the_fight() {
    let mut bot = combat_bot();
    bot.arm_backstab_opener(true, None);
    let _ = bot.on_event(&room(&["big skeleton"]));
    assert_eq!(bot.on_event(&our_hit("punch big skeleton")), vec![BotAction::Send("a skeleton".into())]);
    let _ = bot.on_event(&line("You gain 55 experience."));
    assert!(bot.on_event(&line("*Combat Off*")).is_empty());
    assert_eq!(bot.engaged(), None);
    assert!(bot.on_event(&line("Your command had no effect.")).is_empty());
    // The next fight opens plainly and its Combat Off is an ordinary one.
    assert_eq!(bot.on_event(&room(&["giant rat"])), vec![BotAction::Send("a rat".into())]);
    let _ = bot.on_event(&line("*Combat Off*"));
    assert_eq!(bot.engaged(), None, "a targetless Combat Off still un-latches");
}

/// A fight opened with the verb itself is already in the right mode:
/// its blows send nothing, and neither does a second blow after the
/// re-send.
#[test]
fn only_a_backstab_opener_re_sends_and_only_once() {
    let mut bot = combat_bot();
    let _ = bot.on_event(&room(&["kobold thief"]));
    assert!(bot.on_event(&our_hit("kobold thief")).is_empty());

    let mut bot = combat_bot();
    bot.arm_backstab_opener(true, None);
    let _ = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(bot.on_event(&our_hit("punch kobold thief")), vec![BotAction::Send("a thief".into())]);
    assert!(bot.on_event(&our_hit("kobold thief")).is_empty());
}

/// A party member's blow on the target is not the opening round
/// resolving.
#[test]
fn someone_elses_blow_does_not_end_the_opening_round() {
    let mut bot = combat_bot();
    bot.arm_backstab_opener(true, None);
    let _ = bot.on_event(&room(&["kobold thief"]));
    let theirs = Event::CombatHit {
        attacker: mud_client::events::Actor::Other("Potato".into()),
        target: mud_client::events::Actor::Other("kobold thief".into()),
        damage: 12,
    };
    assert!(bot.on_event(&theirs).is_empty());
    assert_eq!(bot.on_event(&our_hit("punch kobold thief")), vec![BotAction::Send("a thief".into())]);
}
