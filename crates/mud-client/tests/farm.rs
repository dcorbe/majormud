//! Farm runner tests. This file covers the configuration layer: turning
//! a `[farm]` TOML table into a plan that is known-good *before* the
//! client connects, so a typo in a room id fails at startup rather than
//! halfway around a patrol circuit.

use std::time::{Duration, Instant};

use mud_client::bot::BotConfig;
use mud_client::events::Event;
use mud_client::farm::{
    ACK_TIMEOUT, FarmConfig, FarmGuard, FarmPlan, Gate, HealWatch, is_player_death, parse_health,
    parse_room_id,
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
            },
        ),
        (
            rid(1, 2),
            GraphRoom {
                name: "One Way Ditch".into(),
                exits: Default::default(),
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
// command in flight until the board's prompt acknowledges it, and it is
// what turns Event::SlowDown — which means the board DROPPED our input —
// back into a resend.
// ---------------------------------------------------------------------

const BACKOFF: Duration = Duration::from_millis(5000);

fn gate() -> Gate {
    Gate::new(BACKOFF)
}

fn prompt(hp: i32) -> Event {
    Event::Prompt { hp, mana: None }
}

#[test]
fn an_empty_gate_sends_nothing() {
    let mut g = gate();
    assert_eq!(g.poll(Instant::now()), None);
}

#[test]
fn holds_one_command_in_flight_until_the_prompt_acks_it() {
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    g.push("get copper".into());

    assert_eq!(g.poll(t0), Some("a rat".to_string()));
    // The board has not answered yet, so nothing else goes out.
    assert_eq!(g.poll(t0 + Duration::from_millis(10)), None);
    assert_eq!(g.in_flight(), Some("a rat"));

    g.on_event(&prompt(30), t0 + Duration::from_millis(20));
    assert_eq!(g.in_flight(), None);
    assert_eq!(
        g.poll(t0 + Duration::from_millis(30)),
        Some("get copper".to_string())
    );
}

#[test]
fn slow_down_resends_the_command_the_board_dropped() {
    let t0 = Instant::now();
    let mut g = gate();
    g.push("a rat".into());
    assert_eq!(g.poll(t0), Some("a rat".to_string()));

    // "Why don't you slow down for a few seconds?" — the swing never
    // happened, so it has to go out again once the board calms down.
    g.on_event(&Event::SlowDown, t0);
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
        g.on_event(&Event::SlowDown, t0 + Duration::from_millis(i));
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
    g.on_event(&Event::SlowDown, t0);
    g.on_event(&Event::SlowDown, t0 + Duration::from_millis(2000));

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
    g.on_event(&Event::SlowDown, t0);
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

    g.on_event(&Event::SlowDown, t0);
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
    assert_eq!(
        g.on_event(&Event::CombatMiss {
            line: "You swing and miss.".into()
        }),
        None
    );
    assert_eq!(
        g.on_event(&Event::ActorEntered {
            name: "a giant rat".into(),
            from: Some("east".into()),
        }),
        None
    );
}

/// No latches. The runner may hand the same guard to a resumed leg, and
/// a still-wounded character must still be able to stop it.
#[test]
fn the_guard_keeps_no_memory_between_trips() {
    let mut g = guard(100, 50);
    assert_eq!(g.on_event(&prompt(20)), Some(Interrupt::Hurt { hp: 20 }));
    assert_eq!(g.on_event(&prompt(20)), Some(Interrupt::Hurt { hp: 20 }));
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
