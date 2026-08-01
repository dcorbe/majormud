//! World-state tests: the board's round clock (and, later, `Here`).
//! Pure and clock-injected like the bot and StopState suites.

use std::time::{Duration, Instant};

use mud_client::correlate::{CmdId, Correlated};
use mud_client::events::{Event, RoomView};
use mud_client::world::{Here, OccupantKind, ROUND, RoundClock};

fn unsolicited(ev: Event) -> Correlated {
    Correlated {
        event: ev,
        answers: None,
    }
}

fn answering(ev: Event, id: CmdId) -> Correlated {
    Correlated {
        event: ev,
        answers: Some(id),
    }
}

fn view(also_here: &[&str]) -> RoomView {
    RoomView {
        name: "Small Cavern".into(),
        exits: vec!["south".into()],
        also_here: also_here.iter().map(|s| s.to_string()).collect(),
        items: vec![],
    }
}

const ASK: CmdId = CmdId(7);

#[test]
fn an_unlocked_clock_paces_by_one_period() {
    let clock = RoundClock::new();
    let t = Instant::now();
    assert_eq!(clock.next_round_after(t), t + ROUND);
}

#[test]
fn a_burst_locks_the_phase() {
    let mut clock = RoundClock::new();
    let t0 = Instant::now();
    clock.observe(t0);
    // Asked mid-round, the next round is the burst's phase plus one
    // period — not "one period from now".
    assert_eq!(
        clock.next_round_after(t0 + Duration::from_secs(1)),
        t0 + ROUND
    );
}

#[test]
fn lines_within_one_burst_do_not_slide_the_phase() {
    let mut clock = RoundClock::new();
    let t0 = Instant::now();
    clock.observe(t0);
    clock.observe(t0 + Duration::from_millis(40));
    clock.observe(t0 + Duration::from_millis(80));
    assert_eq!(
        clock.next_round_after(t0 + Duration::from_secs(1)),
        t0 + ROUND
    );
}

#[test]
fn a_later_burst_relocks_the_phase() {
    let mut clock = RoundClock::new();
    let t0 = Instant::now();
    clock.observe(t0);
    // Lag drifts the observed cadence; a burst clearly past the old
    // phase re-locks to what the board actually did.
    let t1 = t0 + 3 * ROUND + Duration::from_millis(200);
    clock.observe(t1);
    assert_eq!(
        clock.next_round_after(t1 + Duration::from_secs(1)),
        t1 + ROUND
    );
}

// ---------------------------------------------------------------------
// Here: who is standing in the room we are standing in — the attributed
// evidence discipline StopState proved out, with the arrivals and
// departures the pump used to throw away folded in instead.
// ---------------------------------------------------------------------

#[test]
fn an_attributed_block_seeds_here_and_an_unsolicited_one_does_not() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&unsolicited(Event::RoomSeen(view(&["cave bear"]))), now);
    assert!(
        here.view.is_none(),
        "an unsolicited render is somebody else's"
    );
    assert!(here.occupants.is_empty());

    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    assert_eq!(
        here.view.as_ref().map(|v| v.value.name.as_str()),
        Some("Small Cavern")
    );
    assert_eq!(here.occupants.len(), 1);
    assert_eq!(here.occupants[0].name, "cave bear");
    assert_eq!(here.occupants[0].kind, OccupantKind::Monster);
}

#[test]
fn walk_ins_and_walk_outs_mutate_the_occupants() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    // Async truths pass through regardless of attribution.
    here.on_event(
        &unsolicited(Event::ActorEntered {
            name: "giant rat".into(),
            from: None,
        }),
        now,
    );
    assert_eq!(here.occupants.len(), 1);
    here.on_event(
        &unsolicited(Event::ActorLeft {
            name: "giant rat".into(),
            to: Some("north".into()),
        }),
        now,
    );
    assert!(here.occupants.is_empty());
}

#[test]
fn a_kill_line_removes_the_template_it_names() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(
            Event::RoomSeen(view(&["fat kobold thief", "cave bear"])),
            ASK,
        ),
        now,
    );
    // Death lines name the TEMPLATE; the rolled adjective still matches.
    here.on_event(
        &unsolicited(Event::Line(
            "The kobold thief falls to the ground, dead.".into(),
        )),
        now,
    );
    let names: Vec<_> = here.occupants.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, vec!["cave bear"]);
}

#[test]
fn occupant_since_survives_a_reseeding_block() {
    let t0 = Instant::now();
    let t1 = t0 + Duration::from_secs(30);
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), t0);
    here.on_event(
        &answering(Event::RoomSeen(view(&["cave bear", "giant rat"])), ASK),
        t1,
    );
    let bear = here
        .occupants
        .iter()
        .find(|o| o.name == "cave bear")
        .unwrap();
    let rat = here
        .occupants
        .iter()
        .find(|o| o.name == "giant rat")
        .unwrap();
    assert_eq!(bear.since, t0, "the bear was already here");
    assert_eq!(rat.since, t1);
}

#[test]
fn the_case_rule_types_players_and_monsters() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(
            Event::RoomSeen(view(&["Vexil", "cave bear", "thin Templar"])),
            ASK,
        ),
        now,
    );
    let kind = |n: &str| here.occupants.iter().find(|o| o.name == n).unwrap().kind;
    assert_eq!(kind("Vexil"), OccupantKind::Player);
    assert_eq!(kind("cave bear"), OccupantKind::Monster);
    // The seedy-corpus case: a lowercase adjective on a capitalised
    // noun is a named NPC, not a monster.
    assert_eq!(kind("thin Templar"), OccupantKind::Player);
}

#[test]
fn combat_off_stales_the_view_but_keeps_the_occupants() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.on_event(&unsolicited(Event::Line("*Combat Off*".into())), now);
    assert!(here.view.is_none(), "the fight's end stales the render");
    assert_eq!(here.occupants.len(), 1, "the room did not change");
}

#[test]
fn a_dark_answer_marks_here_blind() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(
            Event::Line("The room is very dark - you can't see anything".into()),
            ASK,
        ),
        now,
    );
    assert!(here.blind);
    // The next attributed block clears it.
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    assert!(!here.blind);
}

/// The recast coherence rule: a faded light waits while the room holds
/// a monster the bot would fight — same policy as rest-safety, and for
/// the same reason: the occupied-room choices are fight or flee.
#[test]
fn a_recast_waits_until_the_room_is_cleared() {
    use mud_client::bot::{Bot, BotConfig};
    let now = Instant::now();
    let bot = Bot::new(BotConfig {
        auto_combat: true,
        ..BotConfig::default()
    });
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    assert!(mud_client::farm::recast_waits_for(&bot, &here));

    // A player in the room is company, not work.
    here.on_event(&answering(Event::RoomSeen(view(&["Vexil"])), ASK), now);
    assert!(!mud_client::farm::recast_waits_for(&bot, &here));

    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    assert!(!mud_client::farm::recast_waits_for(&bot, &here));
}
