//! Farm runner tests. This file covers the configuration layer: turning
//! a `[farm]` TOML table into a plan that is known-good *before* the
//! client connects, so a typo in a room id fails at startup rather than
//! halfway around a patrol circuit.

use std::time::{Duration, Instant};

use mud_client::bot::{Bot, BotConfig};
use mud_client::correlate::{CmdId, Correlated};
use mud_client::events::{Actor, Event, RoomView};
use mud_client::farm::{
    ACK_TIMEOUT, FarmConfig, FarmGuard, FarmPlan, Gate, HealWatch, StopState, Verdict,
    is_player_death, parse_health, parse_room_id,
};
use mud_client::nav::{Interrupt, TravelGuard};
use mud_client::graph::{ExitEdge, GraphRoom, RoomGraph};
use mud_core::content::{Direction, RoomId};

fn rid(map: u16, room: u16) -> RoomId {
    RoomId { map, room }
}

/// gates -N-> square -E-> market, all mutually reachable, plus room 9
/// ("Sealed Vault") with no edges at all — the unroutable case.
fn graph() -> RoomGraph {
    let mk = |n: u16, name: &str, exits: Vec<(Direction, u16)>| {
        let mut room = GraphRoom {
            name: name.into(),
            exits: Default::default(),
            light: 0,
        };
        for (d, dest) in exits {
            room.exits[d as usize] = Some(ExitEdge {
                dest: rid(1, dest),
                exit_type: 0,
            });
        }
        (rid(1, n), room)
    };
    RoomGraph::from_rooms(vec![
        mk(1, "Town Gates", vec![(Direction::North, 2)]),
        mk(
            2,
            "Town Square",
            vec![(Direction::South, 1), (Direction::East, 3)],
        ),
        mk(3, "Market Street", vec![(Direction::West, 2)]),
        mk(9, "Sealed Vault", vec![]),
    ])
}

fn config(start: &str, circuit: &[&str]) -> FarmConfig {
    FarmConfig {
        start: start.into(),
        circuit: circuit.iter().map(|s| (*s).to_string()).collect(),
        ..FarmConfig::default()
    }
}

#[test]
fn parses_map_slash_room() {
    assert_eq!(parse_room_id("1/860"), Some(rid(1, 860)));
    assert_eq!(parse_room_id("12/3"), Some(rid(12, 3)));
}

#[test]
fn rejects_room_ids_that_are_not_map_slash_room() {
    assert_eq!(parse_room_id("1-2"), None);
    assert_eq!(parse_room_id("1"), None);
    assert_eq!(parse_room_id(""), None);
    assert_eq!(parse_room_id("a/b"), None);
    assert_eq!(parse_room_id("1/2/3"), None);
}

#[test]
fn builds_a_plan_from_valid_ids() {
    let plan = FarmPlan::build(&config("1/1", &["1/2", "1/3"]), &graph()).unwrap();
    assert_eq!(plan.start, rid(1, 1));
    assert_eq!(plan.circuit, vec![rid(1, 2), rid(1, 3)]);
}

#[test]
fn rejects_an_empty_circuit() {
    let err = FarmPlan::build(&config("1/1", &[]), &graph()).unwrap_err();
    assert!(err.contains("circuit"), "unhelpful error: {err}");
}

#[test]
fn rejects_a_malformed_room_id() {
    let err = FarmPlan::build(&config("1/1", &["1-2"]), &graph()).unwrap_err();
    assert!(err.contains("1-2"), "error should name the bad id: {err}");
}

#[test]
fn rejects_a_room_the_graph_does_not_know() {
    let err = FarmPlan::build(&config("1/1", &["1/404"]), &graph()).unwrap_err();
    assert!(err.contains("1/404"), "error should name the room: {err}");
}

/// The Sealed Vault is in the graph but has no edges, so no leg can
/// reach it. Catching this at startup is the whole point of the plan.
#[test]
fn rejects_a_circuit_leg_with_no_route() {
    let err = FarmPlan::build(&config("1/1", &["1/2", "1/9"]), &graph()).unwrap_err();
    assert!(err.contains("route"), "unhelpful error: {err}");
}

/// A graph whose Market Street is dark, for the light-aware paths.
fn graph_with_a_dark_market() -> RoomGraph {
    let mk = |n: u16, name: &str, light: i64, exits: Vec<(Direction, u16)>| {
        let mut room = GraphRoom {
            name: name.into(),
            exits: Default::default(),
            light,
        };
        for (d, dest) in exits {
            room.exits[d as usize] = Some(ExitEdge {
                dest: rid(1, dest),
                exit_type: 0,
            });
        }
        (rid(1, n), room)
    };
    RoomGraph::from_rooms(vec![
        mk(1, "Town Gates", 0, vec![(Direction::North, 2)]),
        mk(
            2,
            "Town Square",
            0,
            vec![(Direction::South, 1), (Direction::East, 3)],
        ),
        // Small Cavern's shipped value.
        mk(3, "Market Street", -200, vec![(Direction::West, 2)]),
    ])
}

/// Dark stops warn (the runtime lighting is the real complement) but
/// never refuse: build runs before the connection exists, so it cannot
/// know the character's kit, and refusing would brick mixed circuits
/// that farm their lit stops perfectly well.
#[test]
fn a_dark_stop_warns_but_builds() {
    let g = graph_with_a_dark_market();
    assert!(FarmPlan::build(&config("1/1", &["1/3"]), &g).is_ok());
}

/// The pre-leg question: does this walk cross (or end in) a room the
/// graph marks dark? Decided from the same route goto will compute.
#[test]
fn a_leg_into_a_dark_room_wants_light_first() {
    let g = graph_with_a_dark_market();
    assert!(mud_client::farm::leg_needs_light(&g, rid(1, 1), rid(1, 3)));
}

#[test]
fn a_lit_circuit_wants_none() {
    let g = graph_with_a_dark_market();
    assert!(!mud_client::farm::leg_needs_light(&g, rid(1, 1), rid(1, 2)));
    assert!(!mud_client::farm::leg_needs_light(&g, rid(1, 3), rid(1, 2)));
}

/// The lap wraps: the last stop must be able to reach the first, or the
/// bot completes one loop and strands itself.
#[test]
fn rejects_a_circuit_whose_wrap_around_has_no_route() {
    let g = RoomGraph::from_rooms(vec![
        (
            rid(1, 1),
            GraphRoom {
                name: "Town Gates".into(),
                exits: {
                    let mut e: [Option<ExitEdge>; 10] = Default::default();
                    e[Direction::North as usize] = Some(ExitEdge {
                        dest: rid(1, 2),
                        exit_type: 0,
                    });
                    e
                },
                light: 0,
            },
        ),
        (
            rid(1, 2),
            GraphRoom {
                name: "One Way Ditch".into(),
                exits: Default::default(),
                light: 0,
            },
        ),
    ]);
    // start -> 1/2 routes, but 1/2 -> 1/2 wrap is fine; the failing leg
    // is a two-stop circuit that cannot get back to the first stop.
    let err = FarmPlan::build(&config("1/1", &["1/1", "1/2"]), &g).unwrap_err();
    assert!(err.contains("route"), "unhelpful error: {err}");
}

#[test]
fn rejects_a_start_the_graph_does_not_know() {
    let err = FarmPlan::build(&config("1/404", &["1/2"]), &graph()).unwrap_err();
    assert!(err.contains("1/404"), "error should name the room: {err}");
}

/// A one-stop circuit is legal: sit in one room forever. Its wrap-around
/// leg is zero-length and must not be treated as unroutable.
#[test]
fn accepts_a_single_stop_circuit() {
    let plan = FarmPlan::build(&config("1/1", &["1/2"]), &graph()).unwrap();
    assert_eq!(plan.circuit, vec![rid(1, 2)]);
}

// ---------------------------------------------------------------------
// Gate: the runner's only outbound path.
//
// Session::send is unbounded and unacknowledged, and the live board
// paces at 1500ms, so firing every bot decision straight at it queues
// minutes of stale commands with no way to cancel. The gate holds one
// command in flight until an event ANSWERING that send arrives — its
// echo, or any later attributed reply; never a prompt — and it is what
// turns Event::SlowDown — which means the board DROPPED our input —
// back into a resend.
// ---------------------------------------------------------------------

const BACKOFF: Duration = Duration::from_millis(5000);

fn gate() -> Gate {
    Gate::new(BACKOFF)
}

fn prompt(hp: i32) -> Event {
    Event::Prompt { hp, mana: None }
}

/// An event nobody asked for.
fn unsolicited(ev: Event) -> Correlated {
    Correlated { event: ev, answers: None }
}

/// An event attributed to `id`.
fn answering(ev: Event, id: CmdId) -> Correlated {
    Correlated { event: ev, answers: Some(id) }
}

#[test]
fn an_empty_gate_sends_nothing() {
    let mut g = gate();
    assert_eq!(g.poll(Instant::now()), None);
}

#[test]
fn holds_one_command_in_flight_until_its_echo_acks_it() {
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    g.push("get copper".into());

    assert_eq!(g.poll(t0), Some("a rat".to_string()));
    g.confirm(CmdId(7));
    // The board has not accepted it yet, so nothing else goes out.
    assert_eq!(g.poll(t0 + Duration::from_millis(10)), None);
    assert_eq!(g.in_flight(), Some("a rat"));

    // The echo — any event ANSWERING our send — is the acknowledgement.
    g.on_event(
        &answering(Event::Line("a rat".into()), CmdId(7)),
        t0 + Duration::from_millis(20),
    );
    assert_eq!(g.in_flight(), None);
    assert_eq!(
        g.poll(t0 + Duration::from_millis(30)),
        Some("get copper".to_string())
    );
}

#[test]
fn a_burst_of_unsolicited_prompts_never_acks() {
    // Prompts arrive in bursts and unsolicited — the board re-prompts
    // whenever async output disturbs a dangling one. A prompt is not
    // evidence OUR command was answered; treating it as one is how a
    // stranger's blow used to clear our in-flight command.
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    assert_eq!(g.poll(t0), Some("a rat".to_string()));
    g.confirm(CmdId(7));
    for i in 0..5 {
        g.on_event(&unsolicited(prompt(30)), t0 + Duration::from_millis(i));
    }
    assert_eq!(g.in_flight(), Some("a rat"), "a prompt is not an ack");
    assert_eq!(g.poll(t0 + Duration::from_millis(10)), None);
}

#[test]
fn an_attributed_reply_acks_like_the_echo_does() {
    // "The echo, or any later reply": a RoomSeen attributed to the
    // in-flight send clears it through the same path.
    let t0 = Instant::now();
    let mut g = gate();
    g.push("look".into());
    g.poll(t0);
    g.confirm(CmdId(9));
    g.on_event(
        &answering(Event::RoomSeen(RoomView::default()), CmdId(9)),
        t0 + Duration::from_millis(5),
    );
    assert_eq!(g.in_flight(), None);
}

#[test]
fn an_answer_to_somebody_else_does_not_ack() {
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    g.poll(t0);
    g.confirm(CmdId(7));
    // A stale look's block, attributed to an EARLIER send.
    g.on_event(
        &answering(Event::Line("look".into()), CmdId(3)),
        t0 + Duration::from_millis(5),
    );
    assert_eq!(g.in_flight(), Some("a rat"));
}

#[test]
fn an_unconfirmed_command_still_expires() {
    // poll() handed the command out but the runner never reported the
    // send id (crash between poll and send). The ACK_TIMEOUT floor keeps
    // the queue moving regardless.
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    g.push("get copper".into());
    assert_eq!(g.poll(t0), Some("a rat".to_string()));
    assert_eq!(g.poll(t0 + ACK_TIMEOUT), Some("get copper".to_string()));
}

#[test]
fn slow_down_resends_the_command_the_board_dropped() {
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    assert_eq!(g.poll(t0), Some("a rat".to_string()));

    // "Why don't you slow down for a few seconds?" — the swing never
    // happened, so it has to go out again once the board calms down.
    g.on_event(&unsolicited(Event::SlowDown), t0);
    assert_eq!(
        g.poll(t0 + Duration::from_millis(1)),
        None,
        "must back off first"
    );
    assert_eq!(g.poll(t0 + BACKOFF), Some("a rat".to_string()));
}

#[test]
fn a_burst_of_slow_downs_resends_once() {
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    assert_eq!(g.poll(t0), Some("a rat".to_string()));

    // The board repeats the scolding for every line it drops.
    for i in 0..3 {
        g.on_event(&unsolicited(Event::SlowDown), t0 + Duration::from_millis(i));
    }

    assert_eq!(g.poll(t0 + BACKOFF * 2), Some("a rat".to_string()));
    assert_eq!(g.poll(t0 + BACKOFF * 3), None, "resent more than once");
}

#[test]
fn a_later_slow_down_extends_the_backoff() {
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    g.poll(t0);
    g.on_event(&unsolicited(Event::SlowDown), t0);
    g.on_event(&unsolicited(Event::SlowDown), t0 + Duration::from_millis(2000));

    // Backoff runs from the *last* scolding, not the first.
    assert_eq!(g.poll(t0 + BACKOFF), None);
    assert_eq!(
        g.poll(t0 + Duration::from_millis(2000) + BACKOFF),
        Some("a rat".to_string())
    );
}

/// Flood control with an idle gate: there is nothing to resend, but the
/// board is still angry, so the next command waits too.
#[test]
fn slow_down_with_nothing_in_flight_still_backs_off() {
    let t0 = Instant::now();
    let mut g = gate();
    g.on_event(&unsolicited(Event::SlowDown), t0);
    g.push("a rat".into());

    assert_eq!(g.poll(t0 + Duration::from_millis(1)), None);
    assert_eq!(g.poll(t0 + BACKOFF), Some("a rat".to_string()));
}

/// A prompt is not guaranteed — the board can eat a command silently.
/// Waiting forever would wedge the runner, so in-flight expires.
#[test]
fn an_unacked_command_expires_so_the_queue_keeps_moving() {
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    g.push("get copper".into());
    assert_eq!(g.poll(t0), Some("a rat".to_string()));

    assert_eq!(g.poll(t0 + ACK_TIMEOUT - Duration::from_millis(1)), None);
    assert_eq!(g.poll(t0 + ACK_TIMEOUT), Some("get copper".to_string()));
}

#[test]
fn next_deadline_is_when_the_backoff_ends() {
    let t0 = Instant::now();
    let mut g = gate();
    assert_eq!(g.next_deadline(), None, "idle gate has no deadline");

    g.push("a rat".into());
    g.poll(t0);
    assert_eq!(g.next_deadline(), Some(t0 + ACK_TIMEOUT));

    g.on_event(&unsolicited(Event::SlowDown), t0);
    assert_eq!(g.next_deadline(), Some(t0 + BACKOFF));
}

// ---------------------------------------------------------------------
// HealWatch: the trigger for Bot::rearm.
//
// The bot's heal latch clears only when HP climbs back over the
// threshold. A heal that never lands therefore latches it forever — the
// character sits at 30% and never rests again. bot.rs documents that
// "the runner calls rearm() when it sees the heal was refused", but no
// detector existed. No refusal wording appears anywhere in the 51
// captured transcripts and the Rust server has no rest command at all,
// so there is no line to match; inventing one would be a fixture tidier
// than the board. This watches for *progress* instead, on the board's
// own clock: prompts.
// ---------------------------------------------------------------------

fn heal_watch(refused: &[&str]) -> HealWatch {
    let bot = BotConfig {
        heal_command: "rest".into(),
        ..BotConfig::default()
    };
    let farm = FarmConfig {
        heal_retry_prompts: 3,
        heal_refused: refused.iter().map(|s| (*s).to_string()).collect(),
        ..FarmConfig::default()
    };
    HealWatch::new(&bot, &farm)
}

#[test]
fn a_heal_that_never_moves_hp_gives_up_and_rearms() {
    let mut w = heal_watch(&[]);
    w.on_sent("rest");

    assert!(!w.on_event(&prompt(12)), "first prompt sets the baseline");
    assert!(!w.on_event(&prompt(12)));
    assert!(
        w.on_event(&prompt(12)),
        "three flat prompts means it never landed"
    );
}

/// Once it has given up it must go quiet, or every later prompt rearms
/// the bot and the heal floods right back.
#[test]
fn it_rearms_only_once_per_heal() {
    let mut w = heal_watch(&[]);
    w.on_sent("rest");
    for _ in 0..2 {
        w.on_event(&prompt(12));
    }
    assert!(w.on_event(&prompt(12)));
    assert!(!w.on_event(&prompt(12)), "kept rearming after giving up");
    assert!(!w.on_event(&prompt(12)));
}

#[test]
fn a_heal_that_is_working_never_rearms() {
    let mut w = heal_watch(&[]);
    w.on_sent("rest");
    assert!(!w.on_event(&prompt(12)));
    // Any climb at all is the heal doing its job.
    assert!(!w.on_event(&prompt(13)));
    assert!(!w.on_event(&prompt(14)));
    assert!(!w.on_event(&prompt(14)), "stopped watching once HP moved");
}

/// Losing HP is not progress either — resting through a beating heals
/// nothing, and the bot needs to be free to act again.
#[test]
fn a_heal_that_is_losing_ground_rearms() {
    let mut w = heal_watch(&[]);
    w.on_sent("rest");
    assert!(!w.on_event(&prompt(12)));
    assert!(!w.on_event(&prompt(10)));
    assert!(w.on_event(&prompt(8)));
}

#[test]
fn prompts_do_nothing_when_no_heal_is_outstanding() {
    let mut w = heal_watch(&[]);
    for _ in 0..10 {
        assert!(!w.on_event(&prompt(12)));
    }
}

#[test]
fn only_the_heal_command_arms_it() {
    let mut w = heal_watch(&[]);
    w.on_sent("a rat");
    for _ in 0..5 {
        assert!(!w.on_event(&prompt(12)));
    }
}

/// The escape hatch for when a real refusal line is finally captured off
/// the live board: no waiting three prompts, rearm on the spot.
#[test]
fn a_configured_refusal_line_rearms_immediately() {
    let mut w = heal_watch(&["You can't rest"]);
    w.on_sent("rest");
    assert!(w.on_event(&Event::Line(
        "You can't rest while enemies are near!".into()
    )));
}

#[test]
fn a_refusal_line_is_ignored_when_no_heal_is_outstanding() {
    let mut w = heal_watch(&["You can't rest"]);
    assert!(!w.on_event(&Event::Line("You can't rest here.".into())));
}

/// A fresh heal restarts the watch: new baseline, new patience.
#[test]
fn resending_the_heal_restarts_the_watch() {
    let mut w = heal_watch(&[]);
    w.on_sent("rest");
    w.on_event(&prompt(12));
    w.on_event(&prompt(12));

    w.on_sent("rest");
    assert!(!w.on_event(&prompt(12)), "baseline should have reset");
    assert!(!w.on_event(&prompt(12)));
    assert!(w.on_event(&prompt(12)));
}

// ---------------------------------------------------------------------
// parse_health: where BotConfig.max_hp comes from.
//
// Every percent policy the bot has — heal below 50%, flee below 25% —
// divides by max_hp, and a max_hp of 0 disables both silently. Making
// the operator type their character's max HP into the profile correctly
// is a trap, so the runner asks the board instead. These lines are
// lifted verbatim out of the corpus, spacing included.
// ---------------------------------------------------------------------

#[test]
fn reads_the_boards_health_line() {
    assert_eq!(parse_health("Health:    35/35    [100%]"), Some((35, 35)));
    assert_eq!(parse_health("Health:    27/35    [77%]"), Some((27, 35)));
}

#[test]
fn reads_a_health_line_with_a_mana_pool_after_it() {
    assert_eq!(
        parse_health("Health:    29/29    [100%]  Mana:   8/18  [44%]"),
        Some((29, 29))
    );
}

/// Mystics print Kai where everyone else prints Mana. The health half is
/// identical, and it is the only half that matters here.
#[test]
fn reads_a_mystics_health_line() {
    assert_eq!(
        parse_health("Health:    28/31    [90%]  Kai:   0/1   [0%]"),
        Some((28, 31))
    );
}

#[test]
fn ignores_lines_that_are_not_a_health_report() {
    assert_eq!(parse_health("[HP=35]:"), None);
    assert_eq!(parse_health("You are in good health."), None);
    assert_eq!(parse_health(""), None);
}

/// The line arrives inside a screenful of other output, not alone.
#[test]
fn finds_the_health_line_inside_a_transcript() {
    let transcript = concat!(
        "health\r\n",
        "Name: Nav                     Lvl: 1  Exp: 0\r\n",
        "Health:    27/35    [77%]\r\n",
        "[HP=27]:",
    );
    assert_eq!(parse_health(transcript), Some((27, 35)));
}

// ---------------------------------------------------------------------
// is_player_death: knowing when to stop.
//
// The board prints "<name> is dead." when the character dies, and
// "The <template> is dead." for a monster that has no death message of
// its own. Confusing the two either strands a corpse farming an empty
// room or ends a healthy run on someone else's kill.
// ---------------------------------------------------------------------

#[test]
fn recognises_the_players_own_death() {
    assert!(is_player_death("Nav is dead.", "Nav"));
}

#[test]
fn a_monster_death_is_not_the_players() {
    // The fallback wording for a monster with no death message: the
    // leading "The " is the whole difference.
    assert!(!is_player_death("The giant rat is dead.", "Nav"));
    assert!(!is_player_death(
        "The giant rat falls to the ground with a tortured squeak.",
        "Nav"
    ));
}

/// Another player dying is not our problem.
#[test]
fn someone_elses_death_is_not_ours() {
    assert!(!is_player_death("Vexil is dead.", "Nav"));
}

// ---------------------------------------------------------------------
// FarmGuard: the policy that decides a walk has become the wrong thing
// to be doing. Pure — events in, an interrupt or nothing out. It never
// sends, so travel stays a single-sender phase.
// ---------------------------------------------------------------------

fn guard(max_hp: i32, hurt_at_percent: u32) -> FarmGuard {
    FarmGuard::new(max_hp, hurt_at_percent, "Farmer")
}

#[test]
fn our_own_death_line_stops_the_walk() {
    assert_eq!(
        guard(100, 50).on_event(&Event::Line("Farmer is dead.".into())),
        Some(Interrupt::Died)
    );
}

/// Somebody else dying is news, not an emergency. `<name> is dead.` is
/// the player form and it names whoever it happened to.
#[test]
fn another_players_death_line_is_not_ours() {
    assert_eq!(
        guard(100, 50).on_event(&Event::Line("Vexil is dead.".into())),
        None
    );
}

/// Monsters die with a different line entirely, and a walk that stopped
/// for every kill in earshot would never get anywhere.
#[test]
fn a_monster_death_is_not_a_death() {
    assert_eq!(
        guard(100, 50).on_event(&Event::Line(
            "The giant rat falls to the ground, dead.".into()
        )),
        None
    );
}

/// HP reads negative while downed, and no command lands until a revive.
#[test]
fn a_downed_prompt_stops_the_walk() {
    assert_eq!(guard(100, 50).on_event(&prompt(-3)), Some(Interrupt::Died));
    assert_eq!(guard(100, 50).on_event(&prompt(0)), Some(Interrupt::Died));
}

#[test]
fn hurt_trips_below_the_threshold_and_not_at_it() {
    assert_eq!(
        guard(100, 50).on_event(&prompt(49)),
        Some(Interrupt::Hurt { hp: 49 })
    );
    assert_eq!(guard(100, 50).on_event(&prompt(50)), None);
}

/// Every percent policy in the client divides by max HP, and 0 means the
/// profile never said. Guessing would mis-scale the one decision that
/// keeps a character alive, so the percent trip goes quiet — but dying
/// is not a percentage, and that still stops the walk.
#[test]
fn an_unknown_max_hp_disables_the_percent_trip_but_not_death() {
    assert_eq!(guard(0, 50).on_event(&prompt(1)), None);
    assert_eq!(guard(0, 50).on_event(&prompt(-1)), Some(Interrupt::Died));
}

/// The recovery walk uses this: a character that just fled is already
/// below any sane threshold, so a guard that tripped on hp% would make
/// walking back impossible exactly when it is needed.
#[test]
fn a_zero_threshold_never_trips_on_hp() {
    assert_eq!(FarmGuard::death_only("Farmer").on_event(&prompt(1)), None);
    assert_eq!(
        FarmGuard::death_only("Farmer").on_event(&prompt(-1)),
        Some(Interrupt::Died)
    );
}

#[test]
fn nothing_else_is_an_emergency() {
    let mut g = guard(100, 50);
    assert_eq!(g.on_event(&Event::SlowDown), None);
    // Our own whiff is the defence the walk asked for, not an attack.
    assert_eq!(
        g.on_event(&Event::CombatMiss {
            line: "You swing and miss.".into()
        }),
        None
    );
    // A bystander's fight names neither "you" nor "your".
    assert_eq!(
        g.on_event(&Event::CombatMiss {
            line: "Poop swipes at kobold thief!".into()
        }),
        None
    );
    // An entry only matters to a guard that carries the sighting
    // predicate — without one (recover, the walk home), it is news.
    assert_eq!(
        g.on_event(&Event::ActorEntered {
            name: "giant rat".into(),
            from: Some("east".into()),
        }),
        None
    );
}

/// Live incident (run5, 2026-08-01): "acid slime moves into the room
/// from the north." mid-leg, whiffs only, and the walk kept sending
/// steps — three rooms of "lunges at you" before the slime gave up.
/// An entry the bot's own policy would fight has to stop the leg.
#[test]
fn a_monster_entering_mid_walk_trips_a_sighting_guard() {
    let mut g = guard(100, 50).sighting(Bot::new(BotConfig {
        auto_combat: true,
        ..BotConfig::default()
    }));
    assert_eq!(
        g.on_event(&Event::ActorEntered {
            name: "giant rat".into(),
            from: Some("north".into()),
        }),
        Some(Interrupt::Entered {
            name: "giant rat".into()
        })
    );
    // A spawn ("...from nowhere.") enters with no direction and is just
    // as much in the room.
    assert_eq!(
        g.on_event(&Event::ActorEntered {
            name: "fat kobold thief".into(),
            from: None,
        }),
        Some(Interrupt::Entered {
            name: "fat kobold thief".into()
        })
    );
}

/// `fight_while_travelling = false`, the recovery walk, and the walk
/// home all carry no sighting bot — and must keep walking, exactly as
/// they ignore what an arrival block lists.
#[test]
fn an_entry_without_the_sighting_predicate_changes_nothing() {
    let entered = Event::ActorEntered {
        name: "giant rat".into(),
        from: Some("north".into()),
    };
    assert_eq!(guard(100, 50).on_event(&entered), None);
    assert_eq!(FarmGuard::running(100, 50, "Farmer").on_event(&entered), None);
}

/// The entry name goes through the same would-attack policy as a
/// sighted block: players (capitalised) and refused templates are not
/// fights we start.
#[test]
fn a_player_or_refused_entry_does_not_trip() {
    let mut g = guard(100, 50).sighting(Bot::new(BotConfig {
        auto_combat: true,
        ..BotConfig::default()
    }));
    assert_eq!(
        g.on_event(&Event::ActorEntered {
            name: "Kaimon".into(),
            from: Some("east".into()),
        }),
        None
    );
    let refused = mud_client::bot::Refusals::default();
    refused.lock().unwrap().insert("rat".into());
    let mut g = guard(100, 50).sighting(Bot::with_refusals(
        BotConfig {
            auto_combat: true,
            ..BotConfig::default()
        },
        std::sync::Arc::new(mud_client::bot::ThreatTable::new()),
        refused,
    ));
    assert_eq!(
        g.on_event(&Event::ActorEntered {
            name: "thin giant rat".into(),
            from: Some("east".into()),
        }),
        None
    );
}

/// A whiff aimed at us proves occupancy exactly like a landed blow —
/// the stop pump already lives by that rule, and the walk has to agree:
/// run5's kobold thief lunged across three rooms without connecting
/// once, so a guard waiting for CombatHit never fired. No attacker name
/// can be trusted out of per-monster whiff wording, so none is claimed.
#[test]
fn a_whiff_at_us_stops_a_fighting_walk() {
    let whiff = Event::CombatMiss {
        line: "The fat kobold thief lunges at you with their shortsword!".into(),
    };
    match guard(100, 50).on_event(&whiff) {
        Some(Interrupt::Attacked { .. }) => {}
        other => panic!("expected Attacked, got {other:?}"),
    }
    // The walk home does not fight back, hit or miss alike.
    assert_eq!(FarmGuard::running(100, 50, "Farmer").on_event(&whiff), None);
}

/// No latches. The runner may hand the same guard to a resumed leg, and
/// a still-wounded character must still be able to stop it.
#[test]
fn the_guard_keeps_no_memory_between_trips() {
    let mut g = guard(100, 50);
    assert_eq!(g.on_event(&prompt(20)), Some(Interrupt::Hurt { hp: 20 }));
    assert_eq!(g.on_event(&prompt(20)), Some(Interrupt::Hurt { hp: 20 }));
}

/// The sighting predicate is the bot's own would-attack policy — the
/// case rule, the ignore list, the auto_combat toggle — consulted
/// through an attached Bot. Without one attached (recover, the walk
/// home), a guard sights nothing whatever the block lists.
#[test]
fn a_sighting_guard_trips_only_for_something_the_bot_would_attack() {
    let mut g = guard(100, 50).sighting(Bot::new(BotConfig {
        auto_combat: true,
        ..BotConfig::default()
    }));
    let rat = RoomView {
        name: "Dungeon, Entrance".into(),
        also_here: vec!["thin giant rat".into()],
        ..RoomView::default()
    };
    match g.on_room(&rat) {
        Some(Interrupt::Sighted { room }) => assert_eq!(room, rat),
        other => panic!("expected Sighted, got {other:?}"),
    }
    // Players are capitalised; sighting one is not a fight we start.
    let player = RoomView {
        name: "Dungeon, Entrance".into(),
        also_here: vec!["Kaimon".into()],
        ..RoomView::default()
    };
    assert_eq!(g.on_room(&player), None);
    // No predicate attached: structurally inert.
    assert_eq!(guard(100, 50).on_room(&rat), None);
}

/// A template the board refused stops tripping for the whole run — the
/// Refusals set is shared, so the guard learns it the moment the stop
/// does, and the leg walks past instead of stopping to be refused again.
#[test]
fn a_refused_template_no_longer_trips_the_sighting_guard() {
    let refused = mud_client::bot::Refusals::default();
    refused.lock().unwrap().insert("rat".into());
    let mut g = guard(100, 50).sighting(Bot::with_refusals(
        BotConfig {
            auto_combat: true,
            ..BotConfig::default()
        },
        std::sync::Arc::new(mud_client::bot::ThreatTable::new()),
        refused,
    ));
    let rat = RoomView {
        name: "Dungeon, Entrance".into(),
        also_here: vec!["thin giant rat".into()],
        ..RoomView::default()
    };
    assert_eq!(g.on_room(&rat), None);
}

// ---------------------------------------------------------------------
// The two travel thresholds have to agree, and the plan is where that
// gets settled — before the client connects.
// ---------------------------------------------------------------------

/// Interrupting above the departure gate is a run that goes nowhere: the
/// defend pump ends, wait_for_departure_health releases at
/// depart_at_percent, and the very next prompt trips a higher guard. The
/// whole interrupt budget burns in three prompts without walking a step.
#[test]
fn a_threshold_above_the_departure_gate_is_refused() {
    let cfg = FarmConfig {
        depart_at_percent: 80,
        interrupt_at_percent: 90,
        ..config("1/1", &["1/2"])
    };
    let err = FarmPlan::build(&cfg, &graph()).expect_err("must refuse");
    assert!(err.contains("90"), "error should name the threshold: {err}");
    assert!(err.contains("80"), "error should name the gate: {err}");
}

#[test]
fn a_threshold_at_the_departure_gate_is_allowed() {
    let cfg = FarmConfig {
        depart_at_percent: 80,
        interrupt_at_percent: 80,
        ..config("1/1", &["1/2"])
    };
    assert!(FarmPlan::build(&cfg, &graph()).is_ok());
}

/// With the gate disabled there is nothing to disagree with.
#[test]
fn a_disabled_departure_gate_constrains_nothing() {
    let cfg = FarmConfig {
        depart_at_percent: 0,
        interrupt_at_percent: 101,
        ..config("1/1", &["1/2"])
    };
    assert!(FarmPlan::build(&cfg, &graph()).is_ok());
}

// --- the finish room -------------------------------------------------

/// A run that ends leaves the character standing wherever it stopped --
/// in a lair, linkdead, which is how a character gets killed with nobody
/// driving. An optional finish room is the way out, and it has to be
/// validated up front like every other stop: discovering the route home
/// is unwalkable at the moment you need it is too late.
#[test]
fn a_finish_room_must_be_reachable() {
    let graph = graph();
    let unreachable = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        finish_at: Some("1/99".into()),
        ..FarmConfig::default()
    };
    let err = FarmPlan::build(&unreachable, &graph).expect_err("1/99 is not in the graph");
    assert!(err.contains("1/99"), "{err}");
}

#[test]
fn a_finish_room_is_resolved_into_the_plan() {
    let graph = graph();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        finish_at: Some("1/1".into()),
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    assert_eq!(plan.finish, Some(RoomId { map: 1, room: 1 }));
}

/// Absent means absent: the runner must not invent a destination.
#[test]
fn no_finish_room_configured_means_none_planned() {
    let graph = graph();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    assert_eq!(plan.finish, None);
}

// --- fighting on the way ----------------------------------------------

/// A leg that crosses a hostile room has to fight through it. The guard
/// used to watch only HP, so a character being mauled kept trying to walk
/// while its steps were eaten -- observed live at 1/2150 Newhaven Arena,
/// where the run died with `timed out waiting for "room block after
/// movement"` because three mobs were beating on it.
///
/// Waiting for HP to fall past `interrupt_at_percent` is too late and
/// sometimes never: a healthy character can be swarmed for a long time
/// without dropping below half, and every one of those rounds is a step
/// that does not land.
#[test]
fn being_attacked_interrupts_the_walk() {
    let mut guard = FarmGuard::new(42, 50, "Salad");
    let trip = guard.on_event(&Event::CombatHit {
        attacker: Actor::Other("The fat kobold thief".into()),
        target: Actor::You,
        damage: 3,
    });
    assert!(
        matches!(trip, Some(Interrupt::Attacked { .. })),
        "a blow landing on us must stop the walk: {trip:?}"
    );
}

/// Our own swings are not an attack on us. Without this the guard would
/// trip on the defence it just asked for and never make progress.
#[test]
fn our_own_blows_do_not_interrupt_the_walk() {
    let mut guard = FarmGuard::new(42, 50, "Salad");
    assert!(
        guard
            .on_event(&Event::CombatHit {
                attacker: Actor::You,
                target: Actor::Other("The cave bear".into()),
                damage: 8,
            })
            .is_none()
    );
}

/// The recovery walk stops only for death. It runs right after AutoFlee
/// bolted, so something is by definition still swinging -- tripping on
/// that would make walking back impossible exactly when it is needed.
#[test]
fn the_recovery_walk_still_ignores_being_hit() {
    let mut guard = FarmGuard::death_only("Salad");
    assert!(
        guard
            .on_event(&Event::CombatHit {
                attacker: Actor::Other("The cave bear".into()),
                target: Actor::You,
                damage: 9,
            })
            .is_none()
    );
}

// ---------------------------------------------------------------------
// StopState: what the runner has actually SEEN at this stop.
//
// The stop used to end on a prompt count. A prompt is evidence that the
// board answered SOMETHING; it is not evidence about who is standing in
// the room. Four successive patches tuned that threshold and three live
// failures came out of it -- walking out with three monsters still listed
// under "Also here:", abandoning a live fight, and hanging on an attack
// refusal. The room block states all three outright.
//
// `farm_stop` is an async fn over a live Session and has never had a
// single unit test; that asymmetry with Gate and HealWatch, which are
// pure and covered above, is why those four patches never caught each
// other. These pin the decision itself.
// ---------------------------------------------------------------------

const STOP: &str = "Small Cavern";
const POKE_MS: u64 = 5000;

fn stop_cfg(linger_secs: u64) -> FarmConfig {
    FarmConfig {
        dwell_empty_seconds: linger_secs,
        idle_poke_ms: POKE_MS,
        ..FarmConfig::default()
    }
}

fn stop_state(linger_secs: u64) -> StopState {
    StopState::new(STOP.to_string(), &stop_cfg(linger_secs))
}

fn combat_bot() -> Bot {
    Bot::new(BotConfig {
        auto_combat: true,
        ignore: vec!["town guard".into()],
        ..BotConfig::default()
    })
}

fn view_named(name: &str, also_here: &[&str]) -> RoomView {
    RoomView {
        name: name.into(),
        exits: vec!["north".into()],
        also_here: also_here.iter().map(|s| s.to_string()).collect(),
        items: vec![],
    }
}

fn block_named(name: &str, also_here: &[&str]) -> Event {
    Event::RoomSeen(view_named(name, also_here))
}

fn block(also_here: &[&str]) -> Event {
    block_named(STOP, also_here)
}

/// The id every test look goes out under. `pending_look` clears on each
/// accepted answer, so reusing one id across sequential asks is safe.
const LOOK_ID: CmdId = CmdId(77);

/// The runner's order: the bot folds the event first, then the stop
/// state, so `engaged` and `has_target` already reflect it. Events fed
/// this way are UNSOLICITED (`answers: None`).
fn feed(stop: &mut StopState, bot: &mut Bot, ev: &Event, now: Instant) {
    bot.on_event(ev);
    stop.on_event(&unsolicited(ev.clone()), bot, now);
}

/// Feed an event ATTRIBUTED to the outstanding look.
fn feed_answer(stop: &mut StopState, bot: &mut Bot, ev: &Event, now: Instant) {
    bot.on_event(ev);
    stop.on_event(&answering(ev.clone(), LOOK_ID), bot, now);
}

/// Ask, and answer -- the only request/response pair the runner has.
/// The answer arrives ATTRIBUTED to the ask, as the session guarantees.
fn look_and_see(stop: &mut StopState, bot: &mut Bot, ev: &Event, now: Instant) {
    stop.on_sent("look", LOOK_ID);
    feed_answer(stop, bot, ev, now);
}

/// THE regression test for the whole bug class. Prompts say the board is
/// alive. They say nothing about whether this room still holds anything
/// worth fighting, and no quantity of them may end a stop.
#[test]
fn prompts_do_not_end_a_stop() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    for i in 0..100 {
        feed(&mut stop, &mut bot, &prompt(30), t0);
        assert_eq!(
            stop.verdict(&bot, t0),
            Verdict::Ask,
            "prompt {i} ended a stop the runner had never looked at"
        );
    }
}

/// The bug that shipped: a kill, a prompt, and the runner walked out of
/// a room with three more monsters standing in it.
#[test]
fn a_room_block_listing_a_monster_is_never_empty() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(
        &mut stop,
        &mut bot,
        &block(&["filthbug", "giant rat", "fat giant rat"]),
        t0,
    );
    for i in 0..20 {
        feed(&mut stop, &mut bot, &prompt(30), t0);
        assert_eq!(
            stop.verdict(&bot, t0),
            Verdict::Busy,
            "prompt {i} called a room with three monsters in it empty"
        );
    }
}

#[test]
fn zero_linger_leaves_the_moment_the_room_is_proven_empty() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Ask, "nothing seen yet");
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
}

/// With a respawn budget configured, proving the room empty starts the
/// clock rather than ending the stop.
#[test]
fn an_empty_room_block_is_not_enough_on_its_own() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(15);
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Waiting {
            until: t0 + Duration::from_secs(15)
        }
    );
    // A budget longer than the staleness bound necessarily spans several
    // observations: the runner keeps re-asking, because a respawn would
    // arrive silently, and only leaves on a FRESH block that still says
    // nothing is here. Leaving on a 15-second-old look is exactly the
    // kind of guess this type exists to stop making.
    let t1 = t0 + Duration::from_secs(15);
    assert_eq!(stop.verdict(&bot, t1), Verdict::Ask, "the block went stale");
    look_and_see(&mut stop, &mut bot, &block(&[]), t1);
    assert_eq!(stop.verdict(&bot, t1), Verdict::Empty);
}

/// The race the correlation counter exists for. A `look` goes out, a rat
/// walks in, and the board renders the block for the ORIGINAL look --
/// without the rat. Believing it would clear the fight and walk out.
#[test]
fn a_room_block_that_raced_a_new_arrival_is_not_believed() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);

    stop.on_sent("look", LOOK_ID);
    feed(
        &mut stop,
        &mut bot,
        &Event::ActorEntered {
            name: "giant rat".into(),
            from: Some("north".into()),
        },
        t0,
    );
    // The block ARRIVES ATTRIBUTED to the look — the correlator cannot
    // know the room changed mid-render; only invalidate() forgetting the
    // ask keeps it from being believed. (Mutation-tested: deleting the
    // pending clear from invalidate() must fail here.)
    feed_answer(&mut stop, &mut bot, &block(&[]), t0);
    assert_ne!(
        stop.verdict(&bot, t0),
        Verdict::Empty,
        "believed a room block that predated the arrival"
    );

    // Asked again after things settled, the same answer is trustworthy.
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
}

/// An unfinished fight outranks everything. A block that raced the blow
/// which started the fight is not evidence the fight is over.
#[test]
fn an_unfinished_fight_outranks_an_empty_block() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(&mut stop, &mut bot, &block(&["cave bear"]), t0);
    assert!(bot.engaged().is_some(), "test needs a live fight");
    stop.on_sent("look", LOOK_ID);
    stop.on_event(&answering(block(&[]), LOOK_ID), &bot, t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Busy);
}

#[test]
fn a_kill_invalidates_the_room_block() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(&mut stop, &mut bot, &block(&["giant rat", "filthbug"]), t0);
    feed(
        &mut stop,
        &mut bot,
        &Event::Line("You gain 16 experience.".into()),
        t0,
    );
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Ask,
        "kept trusting a room block taken before the kill"
    );
    look_and_see(&mut stop, &mut bot, &block(&["filthbug"]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Busy);
}

#[test]
fn a_monster_that_walks_in_invalidates_the_room_block() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
    feed(
        &mut stop,
        &mut bot,
        &Event::ActorEntered {
            name: "giant rat".into(),
            from: None,
        },
        t0,
    );
    assert_ne!(stop.verdict(&bot, t0), Verdict::Empty);
}

/// A blow landing on US proves something is here that the block may not
/// have listed. Our own swings prove nothing about occupancy.
#[test]
fn being_hit_invalidates_the_room_block_but_swinging_does_not() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);

    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    stop.on_event(
        &unsolicited(Event::CombatHit {
            attacker: Actor::You,
            target: Actor::Other("giant rat".into()),
            damage: 3,
        }),
        &bot,
        t0,
    );
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty, "our own swing");

    stop.on_event(
        &unsolicited(Event::CombatHit {
            attacker: Actor::Other("giant rat".into()),
            target: Actor::You,
            damage: 7,
        }),
        &bot,
        t0,
    );
    assert_eq!(stop.verdict(&bot, t0), Verdict::Ask, "something hit us");
}

/// The leg's final step already earned an attributed block describing
/// the stop. Seeding it means the first swing goes out without the
/// opening look — the live run spent a full round-trip re-asking for
/// what the arrival render had just said.
#[test]
fn a_seeded_stop_needs_no_opening_look() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    stop.seed(
        view_named(STOP, &["giant rat"]),
        &bot,
        t0,
    );
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Busy,
        "the seeded block lists a target; nothing needs asking"
    );

    // And a seeded EMPTY room needs no look either: with no linger the
    // verdict is Empty on the evidence the traveller brought.
    let mut stop = stop_state(0);
    stop.seed(view_named(STOP, &[]), &bot, t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);

    // The pump's ordering contract: the bot is shown the block BEFORE
    // the state folds it, so `engaged` reflects it — same as on_event.
    let mut stop = stop_state(0);
    bot.on_event(&block(&["giant rat"]));
    stop.seed(view_named(STOP, &["giant rat"]), &bot, t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Busy);
}

/// Same discipline as pending_look: a block naming somewhere else
/// describes somewhere else, however it arrived.
#[test]
fn a_seed_naming_somewhere_else_is_not_believed() {
    let t0 = Instant::now();
    let bot = combat_bot();
    let mut stop = stop_state(0);
    stop.seed(view_named("Somewhere Else", &["giant rat"]), &bot, t0);
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Ask,
        "a foreign block seeded nothing; the stop still has to ask"
    );
}

/// A monster whiffing at us proves occupancy exactly like a blow landing:
/// the live rat that shipped this bug lunged twenty times without ever
/// connecting, and the runner sat on a proven-empty verdict throughout.
/// Whiff wordings are per-monster data, so no name can be trusted out of
/// them — but "something is swinging at us" is enough to re-ask. Our own
/// whiffs prove nothing.
#[test]
fn a_whiff_at_us_invalidates_the_room_block() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
    feed(
        &mut stop,
        &mut bot,
        &Event::CombatMiss {
            line: "The thin giant rat lunges at you!".into(),
        },
        t0,
    );
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Ask,
        "a monster is swinging at us and the runner still called the room empty"
    );

    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    feed(
        &mut stop,
        &mut bot,
        &Event::CombatMiss {
            line: "You swing at giant rat!".into(),
        },
        t0,
    );
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty, "our own whiff");
}

/// "*Combat Off*" ends a fight whatever the death wording said — the
/// live stall this pins had a prose death AND the untrained-XP cap
/// suppressing the award, so no recognised end signal arrived at all
/// and the stop sat Busy on a corpse for 29 seconds. The board's own
/// announcement invalidates the block exactly like a recognised kill.
#[test]
fn combat_off_invalidates_the_room_block() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(&mut stop, &mut bot, &block(&["acid slime"]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Busy);
    feed(
        &mut stop,
        &mut bot,
        &Event::Line("*Combat Off*".into()),
        t0,
    );
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Ask,
        "the fight is over and the pre-fight block cannot be trusted"
    );
}

/// The rest-safety interaction contract: a heal the bot SUPPRESSED (an
/// occupied room) never reaches the gate, so the watch never arms and
/// the "heal never landed → rearm" loop cannot trip on a heal that was
/// never sent. The watch's whole lifecycle keys on on_sent.
#[test]
fn a_heal_the_bot_never_sent_does_not_arm_the_heal_watch() {
    let bot_cfg = BotConfig {
        heal_command: "rest".into(),
        ..BotConfig::default()
    };
    let mut watch = HealWatch::new(&bot_cfg, &FarmConfig::default());
    for _ in 0..50 {
        assert!(
            !watch.on_event(&prompt(10)),
            "armed without a heal ever going out"
        );
    }
}

/// Our attack falling through to SAY means the room changed under the
/// block that prompted the swing: the target is gone, and whatever else
/// the block listed cannot be trusted either. The runner would
/// otherwise sit Busy on the stale listing until the shelf life ran
/// out. (During a run, the only SAY the character produces is a
/// fallthrough — the bot never speaks.)
#[test]
fn a_say_fallthrough_invalidates_the_room_block() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(
        &mut stop,
        &mut bot,
        &block(&["carrion beast", "kobold thief"]),
        t0,
    );
    assert_eq!(stop.verdict(&bot, t0), Verdict::Busy);
    feed(
        &mut stop,
        &mut bot,
        &Event::Line("You say \"a beast\"".into()),
        t0,
    );
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Ask,
        "the swing never started; the block that prompted it is stale"
    );
}

/// NOTHING announces a respawn -- the board simply puts a monster in the
/// room. Silence is not proof the room is unchanged, so an accepted block
/// has a shelf life and must be re-asked.
#[test]
fn an_aged_room_block_has_to_be_asked_again() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(60);
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    let poke = Duration::from_millis(POKE_MS);
    assert!(matches!(
        stop.verdict(&bot, t0 + poke - Duration::from_millis(1)),
        Verdict::Waiting { .. }
    ));
    assert_eq!(stop.verdict(&bot, t0 + poke), Verdict::Ask);

    // Re-answered, the respawn budget keeps running from when the room
    // was FIRST proven empty -- a re-look is not a fresh start.
    look_and_see(&mut stop, &mut bot, &block(&[]), t0 + poke);
    assert_eq!(
        stop.verdict(&bot, t0 + poke),
        Verdict::Waiting {
            until: t0 + Duration::from_secs(60)
        }
    );
}

#[test]
fn the_respawn_budget_restarts_when_something_arrives() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(60);
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    let t1 = t0 + Duration::from_secs(30);
    feed(
        &mut stop,
        &mut bot,
        &Event::ActorLeft {
            name: "Vexil".into(),
            to: None,
        },
        t1,
    );
    look_and_see(&mut stop, &mut bot, &block(&[]), t1);
    assert_eq!(
        stop.verdict(&bot, t1),
        Verdict::Waiting {
            until: t1 + Duration::from_secs(60)
        },
        "the budget carried over from before the room changed"
    );
}

/// The hang that shipped: the board refuses the swing, so no death line
/// and no departure ever arrive to end the stop. Nothing we would attack
/// is left listed, so the stop is done.
#[test]
fn a_refused_monster_does_not_hold_the_stop() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(&mut stop, &mut bot, &block(&["kobold thief"]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Busy);
    feed(
        &mut stop,
        &mut bot,
        &Event::Line(mud_core::crime::WARN_ON_EVIL_REFUSAL.to_string()),
        t0,
    );
    look_and_see(&mut stop, &mut bot, &block(&["kobold thief"]), t0);
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Empty,
        "a monster the board will not let us attack held the stop open"
    );
}

#[test]
fn ignored_names_and_players_do_not_hold_the_stop() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(&mut stop, &mut bot, &block(&["town guard", "Vexil"]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
}

/// A dark room answers `look` with "you can't see anything" and NEVER
/// sends a room block. That is a definite answer, not a missing one --
/// and it must not read as an empty room.
#[test]
fn a_dark_room_is_blind_not_empty() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    stop.on_sent("look", LOOK_ID);
    feed_answer(
        &mut stop,
        &mut bot,
        &Event::Line(format!("  {}!", mud_client::sheet::TOO_DARK)),
        t0,
    );
    assert_eq!(stop.verdict(&bot, t0), Verdict::Blind);
}

#[test]
fn a_room_block_after_lighting_clears_blind() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    stop.on_sent("look", LOOK_ID);
    feed_answer(
        &mut stop,
        &mut bot,
        &Event::Line(mud_client::sheet::TOO_DARK.to_string()),
        t0,
    );
    look_and_see(&mut stop, &mut bot, &block(&["giant rat"]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Busy);
}

/// A lagged broadcast happens exactly when a lot is going on, i.e. in a
/// busy room. Carrying a stale "empty" across one would leave instantly.
#[test]
fn a_lag_forgets_everything() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
    stop.reset();
    assert_eq!(stop.verdict(&bot, t0), Verdict::Ask);
}

/// A block naming somewhere else describes somewhere else. The runner
/// handles that as a flee; it is not an observation of this stop.
#[test]
fn a_room_block_for_somewhere_else_is_not_this_stop() {
    let t0 = Instant::now();
    let bot = combat_bot();
    let mut stop = stop_state(0);
    stop.on_sent("look", LOOK_ID);
    stop.on_event(&answering(block_named("Narrow Road", &[]), LOOK_ID), &bot, t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Ask);
}

#[test]
fn an_idle_gate_owes_the_board_nothing() {
    let t0 = Instant::now();
    let mut g = gate();
    assert!(g.is_idle(), "a fresh gate owes nothing");
    g.push("get copper".into());
    assert!(!g.is_idle(), "a queued command is still owed");
    assert_eq!(g.poll(t0), Some("get copper".to_string()));
    g.confirm(CmdId(4));
    assert!(!g.is_idle(), "an unacknowledged command is still owed");
    g.on_event(&answering(Event::Line("get copper".into()), CmdId(4)), t0);
    assert!(g.is_idle());
}

/// The window between a flee going out and the board saying where it
/// landed. The last block describes a room we may no longer be in, and
/// ending the stop on it records a tidy dwell while the character is
/// standing somewhere else -- so the next leg starts from a lie.
///
/// Found by the live flee-recovery test, which the prompt counter this
/// replaced was simply too slow to reach.
#[test]
fn a_stop_does_not_end_while_a_flee_is_outstanding() {
    let t0 = Instant::now();
    let mut bot = Bot::new(BotConfig {
        auto_flee: true,
        flee_at_percent: 101,
        max_hp: 30,
        ..BotConfig::default()
    });
    let mut stop = stop_state(0);

    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty, "nothing here yet");

    // Any prompt looks like an emergency at 101%, so this flees.
    feed(&mut stop, &mut bot, &prompt(30), t0);
    assert!(bot.fled(), "test needs an outstanding flee");
    assert_eq!(
        stop.verdict(&bot, t0),
        Verdict::Ask,
        "ended the stop while the character was in transit"
    );

    // The next block settles it, whichever room it names.
    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
}

/// Leaving a stop hands the connection to the navigator, which verifies
/// each step by the next room block it sees. A `look` we sent and have
/// not had answered is a room block still owed to US — it arrives
/// mid-step and satisfies it, and the walk believes it is a room further
/// on than it is.
///
/// Measured live as `expected "Dungeon, Entrance", saw "Newhaven, Arena"`
/// — the Arena's own block answering the step out of the Arena.
///
/// `Gate::is_idle` does not cover this: it clears on any prompt, and a
/// room with a fight in it produces plenty that have nothing to do with
/// our look.
#[test]
fn a_stop_does_not_end_while_a_look_is_unanswered() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);

    look_and_see(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty, "proven empty");

    // A fresh look goes out — the runner's idle poke, say — and has not
    // been answered yet.
    stop.on_sent("look", LOOK_ID);
    assert_ne!(
        stop.verdict(&bot, t0),
        Verdict::Empty,
        "left the stop owing a room block to a look already sent"
    );

    // A block that answers NOBODY — somebody else's render — settles
    // nothing: the ask is still owed.
    feed(&mut stop, &mut bot, &block(&[]), t0);
    assert_ne!(stop.verdict(&bot, t0), Verdict::Empty, "unsolicited block settled the look");

    // Its real answer settles it.
    feed_answer(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
}

/// A dark room answers a look with "you can't see anything" and never
/// sends a block, so that IS the answer — the stop must not wait forever
/// for one that is not coming.
#[test]
fn a_dark_answer_settles_an_outstanding_look() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    stop.on_sent("look", LOOK_ID);
    // A STALE dark line answering nobody proves nothing about now.
    feed(
        &mut stop,
        &mut bot,
        &Event::Line(mud_client::sheet::TOO_DARK.to_string()),
        t0,
    );
    assert_ne!(stop.verdict(&bot, t0), Verdict::Blind, "a stale dark line settled the look");
    // The one answering OUR look is the answer.
    feed_answer(
        &mut stop,
        &mut bot,
        &Event::Line(mud_client::sheet::TOO_DARK.to_string()),
        t0,
    );
    assert_eq!(stop.verdict(&bot, t0), Verdict::Blind);
}

/// The light-recovery flow: `light` + `look` go out, and the gate is
/// idle the moment the look's ECHO acks — one event before its answer.
/// Blind-before-pending ended the stop right there, owing the lit
/// room's block, every time lighting worked.
#[test]
fn an_owed_look_outranks_blind() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    stop.on_sent("look", LOOK_ID);
    feed_answer(
        &mut stop,
        &mut bot,
        &Event::Line(mud_client::sheet::TOO_DARK.to_string()),
        t0,
    );
    assert_eq!(stop.verdict(&bot, t0), Verdict::Blind);

    // The runner lit the room and asked again.
    stop.on_sent("look", LOOK_ID);
    assert!(
        matches!(stop.verdict(&bot, t0), Verdict::Waiting { .. }),
        "left (or re-lit) while the lit room's block was still owed: {:?}",
        stop.verdict(&bot, t0)
    );

    // The answer arrives: sighted again.
    feed_answer(&mut stop, &mut bot, &block(&[]), t0);
    assert_eq!(stop.verdict(&bot, t0), Verdict::Empty);
}

/// Our own movement — a flee — is about to change the room, so any
/// in-flight answer predates it: the symmetric hole to the mid-render
/// race. A pre-flee block must not be believed after the flee went out.
#[test]
fn our_own_movement_invalidates_the_stop() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    stop.on_sent("look", LOOK_ID);
    // The flee direction goes out while the look's answer is in flight.
    stop.on_sent("n", CmdId(78));
    // The pre-flee block arrives, genuinely answering the look.
    feed_answer(&mut stop, &mut bot, &block(&[]), t0);
    assert_ne!(
        stop.verdict(&bot, t0),
        Verdict::Empty,
        "believed a room block that predates our own flee"
    );
}

// ---------------------------------------------------------------------
// LightState: light the room, CONFIRM it from the board, only give up
// when nothing can work. The old shape read the plan once, fired it
// blind, and never learned whether it took — live, `cast star` answered
// "You attempt to cast starlight, but fail." and the runner walked into
// the dark anyway.
// ---------------------------------------------------------------------

use mud_client::sheet::{LightAttempt, LightSource, LightState};
use mud_client::world::{ROUND, RoundClock};

const LIGHT_ID: CmdId = CmdId(91);

fn torch() -> LightState {
    LightState::new(vec![LightSource::Item {
        light_cmd: "light torch".into(),
        remove_cmd: "remove torch".into(),
    }])
}

fn spell() -> LightState {
    LightState::new(vec![LightSource::Spell {
        cmd: "cast star".into(),
        mana_cost: 2,
    }])
}

/// attempt() with an unlocked clock — pacing without phase knowledge.
fn ask(l: &mut LightState, now: Instant) -> LightAttempt {
    l.attempt(now, &RoundClock::new())
}

fn mana(l: &mut LightState, mana: i32) {
    l.on_event(&unsolicited(Event::Prompt {
        hp: 50,
        mana: Some(mana),
    }));
}

#[test]
fn a_confirmed_light_is_lit_and_not_relit() {
    let now = Instant::now();
    let mut l = torch();
    assert_eq!(ask(&mut l, now), LightAttempt::Send("light torch".into()));
    l.on_sent("light torch", LIGHT_ID);
    // Outcome owed: no second attempt while one is in flight.
    assert_eq!(ask(&mut l, now), LightAttempt::Nothing);
    l.on_event(&answering(Event::Line("You lit the torch.".into()), LIGHT_ID));
    assert!(l.lit());
    // Lit sources are not re-lit.
    assert_eq!(ask(&mut l, now), LightAttempt::Nothing);
}

#[test]
fn already_lit_counts_as_lit() {
    let now = Instant::now();
    let mut l = torch();
    ask(&mut l, now);
    l.on_sent("light torch", LIGHT_ID);
    l.on_event(&answering(
        Event::Line("You already have something lit!".into()),
        LIGHT_ID,
    ));
    assert!(l.lit());
}

/// The 15-dark-encounters-3-casts bug (run3, 2026-08-01). A fizzle is a
/// random cast roll; Salad had MA 11 with a 2-mana cast and the code
/// gave up after ONE try per visit. Retries are bounded by mana and
/// paced by the round — the board refuses a second cast inside one
/// anyway.
#[test]
fn a_fizzle_is_retried_next_round_while_mana_lasts() {
    let now = Instant::now();
    let mut l = spell();
    mana(&mut l, 12);
    assert_eq!(ask(&mut l, now), LightAttempt::Send("cast star".into()));
    l.on_sent("cast star", LIGHT_ID);
    l.on_event(&answering(
        Event::Line("You attempt to cast starlight, but fail.".into()),
        LIGHT_ID,
    ));
    assert!(!l.lit());
    // Inside the same round: hold until the next one, not give up.
    match ask(&mut l, now + Duration::from_secs(1)) {
        LightAttempt::Hold(until) => assert!(until <= now + ROUND + Duration::from_millis(1)),
        other => panic!("expected Hold, got {other:?}"),
    }
    // Next round: cast again.
    assert_eq!(
        ask(&mut l, now + ROUND + Duration::from_millis(10)),
        LightAttempt::Send("cast star".into())
    );
}

/// Finding from the live captures: the success wording is "You cast
/// starlight!", which the old code never recognised — a spell plan
/// never became lit at all.
#[test]
fn a_cast_success_marks_the_source_lit() {
    let now = Instant::now();
    let mut l = spell();
    mana(&mut l, 12);
    ask(&mut l, now);
    l.on_sent("cast star", LIGHT_ID);
    l.on_event(&answering(Event::Line("You cast starlight!".into()), LIGHT_ID));
    assert!(l.lit());
    assert_eq!(ask(&mut l, now), LightAttempt::Nothing);
}

/// Mana below the cost stops the CASTING, never the plan: the lap (and
/// regen at ~1/round) is the retry.
#[test]
fn mana_below_the_cost_stops_the_casting_not_the_plan() {
    let now = Instant::now();
    let mut l = spell();
    mana(&mut l, 1);
    assert_eq!(ask(&mut l, now), LightAttempt::Nothing);
    mana(&mut l, 4);
    assert_eq!(ask(&mut l, now), LightAttempt::Send("cast star".into()));
}

/// The operator's directive, verbatim: "Your starlight spell fades
/// away." is an indicator that we need to RECAST. The fade kills
/// nothing — recasting is the point of a spell source.
#[test]
fn a_spell_fade_asks_for_a_recast_and_kills_nothing() {
    let now = Instant::now();
    let mut l = spell();
    mana(&mut l, 12);
    ask(&mut l, now);
    l.on_sent("cast star", LIGHT_ID);
    l.on_event(&answering(Event::Line("You cast starlight!".into()), LIGHT_ID));
    assert!(l.lit());
    l.on_event(&unsolicited(Event::Line(
        "Your starlight spell fades away.".into(),
    )));
    assert!(!l.lit());
    assert!(l.wants_recast());
    assert_eq!(
        ask(&mut l, now + ROUND * 2),
        LightAttempt::Send("cast star".into())
    );
}

/// A second torch in the pack must outlive the first one's burn-out —
/// the single-plan shape was why one burn-out went dead-for-the-run.
#[test]
fn an_item_burn_out_advances_to_the_next_source() {
    let now = Instant::now();
    let mut l = LightState::new(vec![
        LightSource::Item {
            light_cmd: "light torch".into(),
            remove_cmd: "remove torch".into(),
        },
        LightSource::Item {
            light_cmd: "light lantern".into(),
            remove_cmd: "remove lantern".into(),
        },
    ]);
    ask(&mut l, now);
    l.on_sent("light torch", LIGHT_ID);
    l.on_event(&answering(Event::Line("You lit the torch.".into()), LIGHT_ID));
    l.on_event(&unsolicited(Event::Line("torch is no longer lit!".into())));
    assert!(!l.lit());
    assert_eq!(
        ask(&mut l, now + ROUND),
        LightAttempt::Send("light lantern".into())
    );
}

/// The burn-out wordings are ITEM wordings; a bystander's torch dying
/// must not kill a spell plan (latent in the old code: any unsolicited
/// burn-out line exhausted whatever the plan was).
#[test]
fn a_bystanders_burn_out_wording_does_not_kill_a_spell_plan() {
    let now = Instant::now();
    let mut l = spell();
    mana(&mut l, 12);
    l.on_event(&unsolicited(Event::Line(
        "Poop's torch is no longer lit!".into(),
    )));
    assert_eq!(ask(&mut l, now), LightAttempt::Send("cast star".into()));
}

/// Blind-while-lit is the backstop for a missed wording, and the kinds
/// diverge: an item that burned out is DEAD; a spell that faded
/// unnoticed is recastable — killing it for the run was the
/// "never recasts again" bug.
#[test]
fn dark_while_lit_kills_an_item_but_only_dims_a_spell() {
    let now = Instant::now();
    let mut l = torch();
    ask(&mut l, now);
    l.on_sent("light torch", LIGHT_ID);
    l.on_event(&answering(Event::Line("You lit the torch.".into()), LIGHT_ID));
    l.source_died();
    assert!(!l.lit());
    l.new_visit();
    assert_eq!(ask(&mut l, now + ROUND), LightAttempt::Nothing);

    let mut l = spell();
    mana(&mut l, 12);
    ask(&mut l, now);
    l.on_sent("cast star", LIGHT_ID);
    l.on_event(&answering(Event::Line("You cast starlight!".into()), LIGHT_ID));
    l.source_died();
    assert!(!l.lit());
    assert!(l.wants_recast());
    assert_eq!(
        ask(&mut l, now + ROUND),
        LightAttempt::Send("cast star".into())
    );
}

#[test]
fn may_not_light_advances_past_the_item() {
    let now = Instant::now();
    let mut l = torch();
    ask(&mut l, now);
    l.on_sent("light torch", LIGHT_ID);
    l.on_event(&answering(
        Event::Line("You may not light that item!".into()),
        LIGHT_ID,
    ));
    assert!(!l.lit());
    // The board refused the item outright: never try it again, and with
    // no other source, there is nothing left.
    assert_eq!(ask(&mut l, now + ROUND), LightAttempt::Nothing);
}

#[test]
fn an_unattributed_outcome_is_ignored() {
    let now = Instant::now();
    let mut l = torch();
    ask(&mut l, now);
    l.on_sent("light torch", LIGHT_ID);
    // Somebody else's lighting, or a stale line: not our outcome.
    l.on_event(&unsolicited(Event::Line("You lit the torch.".into())));
    assert!(!l.lit());
    // Ours settles it.
    l.on_event(&answering(Event::Line("You lit the torch.".into()), LIGHT_ID));
    assert!(l.lit());
}

#[test]
fn light_state_edges_are_pinned() {
    let now = Instant::now();
    // source_died on an UNLIT state is a no-op — it runs on every Blind
    // verdict, including the first at an unlit stop, and must not eat
    // the plan.
    let mut l = torch();
    l.source_died();
    assert_eq!(ask(&mut l, now), LightAttempt::Send("light torch".into()));

    // A non-plan release does not arm the outcome watch.
    let mut l = torch();
    l.on_sent("look", CmdId(5));
    assert_eq!(ask(&mut l, now), LightAttempt::Send("light torch".into()));

    // A lost outcome does not wedge the run: the next visit re-arms.
    let mut l = torch();
    ask(&mut l, now);
    l.on_sent("light torch", LIGHT_ID);
    assert_eq!(ask(&mut l, now), LightAttempt::Nothing, "outcome owed");
    l.new_visit();
    assert_eq!(ask(&mut l, now), LightAttempt::Send("light torch".into()));
}

/// The stale-Blind poisoning: the light took, then an arrival
/// invalidated the ask before its block landed. With `blind` surviving
/// invalidation, the next verdict was Blind-with-lit and source_died ate
/// a burning torch. Invalidation forgets blind along with the rest; the
/// cost is one honest re-look.
#[test]
fn an_arrival_during_light_recovery_does_not_poison_the_plan() {
    let t0 = Instant::now();
    let mut bot = combat_bot();
    let mut stop = stop_state(0);
    stop.on_sent("look", LOOK_ID);
    feed_answer(
        &mut stop,
        &mut bot,
        &Event::Line(mud_client::sheet::TOO_DARK.to_string()),
        t0,
    );
    assert_eq!(stop.verdict(&bot, t0), Verdict::Blind);
    // Light + look go out; the look is owed...
    stop.on_sent("look", LOOK_ID);
    // ...and a monster walks in before its block lands.
    feed(
        &mut stop,
        &mut bot,
        &Event::ActorEntered { name: "giant rat".into(), from: Some("north".into()) },
        t0,
    );
    assert_ne!(
        stop.verdict(&bot, t0),
        Verdict::Blind,
        "a stale Blind verdict would kill the just-lit source"
    );
}

/// Burn-out announces itself unsolicited — "%s is no longer lit!" /
/// "It's uses gone, %s disappears from your inventory!" — and the
/// wording decides lighting, never position, so it is read without
/// attribution. And a confirmed-lit item plan yields the remove that
/// extinguishes it, because one use burns every 3s tick whether anything
/// needs the light or not.
#[test]
fn burn_out_wordings_and_the_extinguish_command() {
    let now = Instant::now();
    let mut l = torch();
    ask(&mut l, now);
    l.on_sent("light torch", LIGHT_ID);
    l.on_event(&answering(Event::Line("You lit the torch.".into()), LIGHT_ID));
    assert!(l.lit());
    assert_eq!(l.extinguish(), Some("remove torch".to_string()));

    l.on_event(&unsolicited(Event::Line("torch is no longer lit!".into())));
    assert!(!l.lit());
    assert_eq!(l.extinguish(), None);
    l.new_visit();
    assert_eq!(
        ask(&mut l, now + ROUND),
        LightAttempt::Nothing,
        "a burned-out source is not retried"
    );

    // A lit spell has nothing to remove.
    let mut l = spell();
    mana(&mut l, 12);
    ask(&mut l, now);
    l.on_sent("cast star", LIGHT_ID);
    l.on_event(&answering(Event::Line("You cast starlight!".into()), LIGHT_ID));
    assert!(l.lit());
    assert_eq!(l.extinguish(), None);
}

/// Cash on the way is work on the way: a leg that walks past a listed
/// pile leaves money on the floor for whoever comes next. A pile trips
/// the sighting guard exactly like a monster, and the same defence pump
/// sweeps it — coins only, and only when the policy actually loots
/// (auto_get, and a sighting bot attached at all).
#[test]
fn a_coin_pile_on_the_way_trips_a_sighting_guard() {
    let mut g = guard(100, 50).sighting(Bot::new(BotConfig {
        auto_combat: true,
        auto_get: true,
        ..BotConfig::default()
    }));
    let pile = RoomView {
        name: "Newhaven, Arena".into(),
        items: vec!["11 silver nobles".into(), "49 copper farthings".into()],
        ..RoomView::default()
    };
    match g.on_room(&pile) {
        Some(Interrupt::Sighted { room }) => assert_eq!(room, pile),
        other => panic!("expected Sighted, got {other:?}"),
    }
    // An item wearing a coin name has no count and is somebody's gear.
    let gear = RoomView {
        name: "Newhaven, Arena".into(),
        items: vec!["silver holy amulet".into()],
        ..RoomView::default()
    };
    assert_eq!(g.on_room(&gear), None);
    // auto_get off: the pile is not this policy's business.
    let mut no_loot = guard(100, 50).sighting(Bot::new(BotConfig {
        auto_combat: true,
        ..BotConfig::default()
    }));
    assert_eq!(no_loot.on_room(&pile), None);
    // No predicate attached (recover, the walk home): inert.
    assert_eq!(guard(100, 50).on_room(&pile), None);
}
