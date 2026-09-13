//! World-state tests: the board's round clock (and, later, `Here`).
//! Pure and clock-injected like the bot and StopState suites.

use std::time::{Duration, Instant};

use mud_client::correlate::{CmdId, Correlated};
use mud_client::events::{Event, RoomView, Status};
use mud_client::world::{
    CAST_WINDOW, CLAIM_GRACE, DivergenceKind, Here, OccupantKind, REGEN_MEDITATE, REGEN_NATURAL,
    REGEN_REST, ROUND, RegenCycle, RoundClock, TickClock,
};

fn unsolicited(ev: Event) -> Correlated {
    Correlated {
        event: ev,
        answers: None,
        elsewhere: false,
    }
}

fn answering(ev: Event, id: CmdId) -> Correlated {
    Correlated {
        event: ev,
        answers: Some(id),
        elsewhere: false,
    }
}

fn peeked(ev: Event, id: CmdId) -> Correlated {
    Correlated {
        event: ev,
        answers: Some(id),
        elsewhere: true,
    }
}

fn view(also_here: &[&str]) -> RoomView {
    RoomView {
        name: "Small Cavern".into(),
        exits: vec!["south".into()],
        also_here: also_here.iter().map(|s| s.to_string()).collect(),
        items: vec![],
        also_here_sgr: Vec::new(),
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

/// A `look <direction>` block names the NEIGHBOUR's occupants, not
/// ours. Folding it in would report a monster a room away as standing
/// here — the same class of bug attributed blocks fixed for position,
/// now fixed for the occupant model too.
#[test]
fn a_directional_look_block_does_not_seed_here() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&peeked(Event::RoomSeen(view(&["guardsman"])), ASK), now);
    assert!(
        here.view.is_none(),
        "a peek at the neighbour is not evidence about this room"
    );
    assert!(here.occupants.is_empty());
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

/// Darkness is not a fact about room contents, so it disturbs nothing
/// in the model. Per `_CAN_SEE` the board still lets the character
/// attack and `get` in the dark — only the re-observation is refused —
/// so the occupants and the floor stand exactly as they were. Blindness
/// belongs to whoever owns the outstanding `look` (`farm::StopState`),
/// which is the only thing that can tell OUR unanswered look apart from
/// a dark line answering anything else.
#[test]
fn a_dark_answer_disturbs_nothing_the_model_believes() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.on_event(
        &answering(
            Event::Line("The room is very dark - you can't see anything".into()),
            ASK,
        ),
        now,
    );
    assert!(here.seeded(), "the model still holds beliefs");
    assert_eq!(
        here.names().collect::<Vec<_>>(),
        ["cave bear"],
        "the bear did not leave because the torch went out"
    );
    assert!(here.view.is_some(), "the render is not staled by darkness");
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

// ---------------------------------------------------------------------
// Here: what is lying on the floor.
//
// `get` was fire-and-forget — nothing ever told the client whether a
// sweep worked — so an encumbrance refusal and a successful pickup were
// indistinguishable, and the only record of a pile was whatever the last
// room block happened to list. The farm's departure gate is send-queue
// emptiness, not work-completeness, so a stop could end over its own
// loot.
// ---------------------------------------------------------------------

fn view_with_loot(items: &[&str]) -> RoomView {
    RoomView {
        items: items.iter().map(|s| s.to_string()).collect(),
        ..view(&[])
    }
}

#[test]
fn a_block_seeds_the_floor_from_the_notice_line() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(
            Event::RoomSeen(view_with_loot(&["11 silver nobles", "a rusty dagger"])),
            ASK,
        ),
        now,
    );
    // Coins only: the dagger is somebody's dropped gear, not our work.
    assert_eq!(here.piles.len(), 1);
    assert_eq!(here.piles[0].denom, "silver");
    assert_eq!(here.piles[0].count, 11);
}

/// A kill drops coins before any block re-renders. Waiting for the next
/// `look` to learn about them is the round-trip this whole model exists
/// to remove.
#[test]
fn a_drop_line_puts_a_fresh_pile_on_the_floor() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    here.on_event(
        &unsolicited(Event::Line("12 silver drop to the ground.".into())),
        now,
    );
    assert_eq!(here.piles.len(), 1);
    assert_eq!(here.piles[0].denom, "silver");
    assert_eq!(here.piles[0].count, 12);
}

#[test]
fn the_pickup_acknowledgement_takes_the_pile_off_the_floor() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view_with_loot(&["11 silver nobles"])), ASK),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line("You picked up 11 silver nobles".into())),
        now,
    );
    assert!(here.piles.is_empty(), "the pile was taken, not still owed");
}

/// The kill arm nulls `view` — something died out of the render. It must
/// not null the FLOOR: a kill is what creates piles, and clearing them
/// here would forget the loot at the instant it appeared.
#[test]
fn a_kill_does_not_sweep_the_floor() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.on_event(
        &unsolicited(Event::Line("12 silver drop to the ground.".into())),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line(
            "The cave bear falls to the ground with a shrill cry.".into(),
        )),
        now,
    );
    assert!(here.view.is_none(), "the render is stale — something died");
    assert!(here.occupants.is_empty(), "the bear left the occupant list");
    assert_eq!(here.piles.len(), 1, "the coins it dropped are still there");
}

/// A pile the character cannot carry stays listed in every block. The
/// attempts already spent must survive the reseed or the try cap can
/// never be reached and the stop would `get` forever.
#[test]
fn a_reseed_preserves_the_attempts_already_spent() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view_with_loot(&["11 silver nobles"])), ASK),
        now,
    );
    here.note_get_attempt("silver");
    here.on_event(
        &answering(Event::RoomSeen(view_with_loot(&["11 silver nobles"])), ASK),
        now,
    );
    assert_eq!(here.piles.len(), 1);
    assert_eq!(here.piles[0].tries, 1);
}

/// A flee or a lagged broadcast drops everything observed. The floor is
/// an observation like any other: coins believed after the character has
/// been moved out from under them belong to a room it is no longer
/// standing in.
#[test]
fn a_reset_clears_the_floor_too() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view_with_loot(&["11 silver nobles"])), ASK),
        now,
    );
    assert_eq!(here.piles.len(), 1);
    here.reset();
    assert!(here.piles.is_empty());
}

// ---------------------------------------------------------------------
// Reconciliation: does the maintained model still agree with the board?
//
// A poll is self-correcting and a model is not. Today a missed wording
// costs one redundant `look`; once decisions run off `Here` the same
// miss becomes a lie the bot acts on. These counters are what earns the
// model that trust — and on a foreign board they are a live
// dialect-divergence detector, since an occupant only the block lists,
// in a room where nothing respawned, is a movemsg wording we cannot
// parse.
// ---------------------------------------------------------------------

/// `Here` is built fresh per stop, so its first attributed block has
/// nothing to disagree with. Counting that as divergence would report
/// every occupant of every room the run visits.
#[test]
fn the_seeding_block_reconciles_nothing() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view(&["cave bear", "giant rat"])), ASK),
        now,
    );
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantMissing), 0);
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantExtra), 0);
}

#[test]
fn a_block_that_agrees_with_the_model_records_nothing() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    assert!(here.reconcile.recent().is_empty());
}

/// The defect direction: the model believes somebody the board does not
/// list. Nothing about a respawn can produce it — it means a death or a
/// departure went unparsed, and it is what makes a bot swing at a ghost
/// or hold a stop `Busy` forever.
#[test]
fn an_occupant_the_block_omits_is_recorded_as_extra() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantExtra), 1);
    assert_eq!(here.reconcile.recent()[0].name, "cave bear");
}

/// The ambiguous direction: a silent respawn produces exactly this, and
/// so does a movemsg wording the parser cannot read. Counted separately
/// and never read as a defect on its own.
#[test]
fn an_occupant_only_the_block_lists_is_recorded_as_missing() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantMissing), 1);
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantExtra), 0);
}

/// Somebody else's render says nothing about our beliefs, and it never
/// seeds the model — so it must never be allowed to indict it either.
#[test]
fn an_unsolicited_block_reconciles_nothing() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.on_event(&unsolicited(Event::RoomSeen(view(&[]))), now);
    assert!(here.reconcile.recent().is_empty());
}

/// Another player swept the pile out from under us.
#[test]
fn a_pile_the_block_omits_is_recorded_as_extra() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view_with_loot(&["11 silver nobles"])), ASK),
        now,
    );
    here.on_event(&answering(Event::RoomSeen(view_with_loot(&[])), ASK), now);
    assert_eq!(here.reconcile.count(DivergenceKind::PileExtra), 1);
}

/// A drop wording we cannot parse: coins reached the floor and the model
/// never heard about it.
#[test]
fn a_pile_only_the_block_lists_is_recorded_as_missing() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view_with_loot(&[])), ASK), now);
    here.on_event(
        &answering(Event::RoomSeen(view_with_loot(&["11 silver nobles"])), ASK),
        now,
    );
    assert_eq!(here.reconcile.count(DivergenceKind::PileMissing), 1);
}

/// The ring is for reading the last few by hand; the counts are the
/// metric. An hour-long run must not accumulate a divergence per block.
#[test]
fn the_recent_ring_is_bounded() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    for _ in 0..200 {
        here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
        here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    }
    assert!(here.reconcile.recent().len() <= 64);
    // The counts are not bounded — they are the measurement.
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantExtra), 200);
}

/// A flee drops beliefs, not measurements. Clearing the counters there
/// would quietly discard exactly the evidence a bad run produces.
#[test]
fn a_reset_drops_beliefs_but_keeps_the_measurement() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantExtra), 1);
    here.reset();
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantExtra), 1);
    // ...but the next block seeds again rather than indicting the model.
    here.on_event(&answering(Event::RoomSeen(view(&["giant rat"])), ASK), now);
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantMissing), 0);
}

/// The run summary prints only what actually fired, so a clean run says
/// nothing at all rather than four zeroes.
#[test]
fn the_tally_lists_only_the_kinds_that_fired() {
    let now = Instant::now();
    let mut here = Here::default();
    assert_eq!(here.reconcile.total(), 0);
    assert!(here.reconcile.tally().is_empty());

    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    assert_eq!(here.reconcile.total(), 1);
    assert_eq!(
        here.reconcile.tally(),
        vec![(DivergenceKind::OccupantExtra, 1)]
    );
}

/// `Here` is per-stop inside the farm, but the assist follows an
/// operator who walks wherever they like. A block naming a DIFFERENT
/// room describes a different floor and different occupants: it must
/// reseed, not indict the model for having believed the last room.
/// Without this every step the operator takes reports the room behind
/// them as an overclaim.
#[test]
fn a_block_from_another_room_reseeds_rather_than_indicts() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    let elsewhere = RoomView {
        name: "Newhaven, Arena".into(),
        also_here: vec!["Mystic".into()],
        ..RoomView::default()
    };
    here.on_event(&answering(Event::RoomSeen(elsewhere), ASK), now);
    assert_eq!(
        here.reconcile.total(),
        0,
        "walking into a new room is not a divergence"
    );
    assert_eq!(here.occupants.len(), 1);
    assert_eq!(here.occupants[0].name, "Mystic");
}

/// The room-identity guard cannot hang off `view`: the kill and
/// combat-off arms NULL it on purpose ("something died out of that
/// render"), and a fight is exactly what precedes walking out of a room.
///
/// Live (cwgaming, 2026-08-01): a kill in Newhaven Arena, a step to
/// Narrow Road, and the model then reported the Arena's occupant and the
/// Arena's coins as divergences of Narrow Road — telling the operator
/// about the contents of a room they had already left.
#[test]
fn a_kill_before_the_step_does_not_blame_the_room_behind_us() {
    let now = Instant::now();
    let mut here = Here::default();
    let arena = RoomView {
        name: "Newhaven, Arena".into(),
        also_here: vec!["Mystic".into()],
        items: vec!["7 copper farthings".into()],
        ..RoomView::default()
    };
    here.on_event(&answering(Event::RoomSeen(arena), ASK), now);
    // A kill stales the render — this is what defeated the old guard.
    here.on_event(
        &unsolicited(Event::Line(
            "The filthbug falls to the ground with a yelp.".into(),
        )),
        now,
    );
    assert!(here.view.is_none(), "the kill staled the render");

    let narrow_road = RoomView {
        name: "Newhaven, Narrow Road".into(),
        ..RoomView::default()
    };
    here.on_event(&answering(Event::RoomSeen(narrow_road), ASK), now);
    assert_eq!(
        here.reconcile.total(),
        0,
        "the room we walked out of is not a divergence of the one we walked into"
    );
    assert!(here.occupants.is_empty());
    assert!(here.piles.is_empty());
}

/// The lexicon is a process-wide `OnceLock` and the first caller wins,
/// so every test in this binary that needs one installs the SAME
/// superset. Installing per-test wordings would make the suite depend on
/// which test the harness happened to run first.
fn lexicon() {
    mud_client::deaths::init_with(mud_client::deaths::DeathLexicon::from_pairs([
        (
            "acid slime".to_string(),
            "The acid slime dissolves into a puddle of bluish goo.".to_string(),
        ),
        (
            "kobold thief".to_string(),
            "The kobold thief falls to the ground with a shrill cry.".to_string(),
        ),
        (
            "wererat".to_string(),
            "The wererat squeals in agony, and dies!".to_string(),
        ),
        (
            "skeleton".to_string(),
            "The skeleton crumbles into a pile of dust.".to_string(),
        ),
    ]));
}

/// Two instances of one template share its death line, and the rolled
/// adjective is the only thing telling them apart: "small skeleton" and
/// "skeleton" in the Crypt (live, test.raw 2026-09-05). Our own swing
/// names the instance it hit -- "You critically slash skeleton for 51
/// damage!" -- so when the template's death line follows, that is the
/// corpse. Taking the longest-standing noun match instead removed
/// "small skeleton" while "skeleton" was the one that fell, and the next
/// look reported one occupant extra and one missing.
#[test]
fn a_kill_takes_the_instance_our_swing_named() {
    lexicon();
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view(&["small skeleton", "skeleton"])), ASK),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line("You critically slash skeleton for 51 damage!".into())),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line("The skeleton crumbles into a pile of dust.".into())),
        now,
    );
    let names: Vec<_> = here.occupants.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, vec!["small skeleton"]);

    // And the other way round: hitting the small one takes the small one.
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view(&["small skeleton", "skeleton"])), ASK),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line("You slice small skeleton for 9 damage!".into())),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line("The skeleton crumbles into a pile of dust.".into())),
        now,
    );
    let names: Vec<_> = here.occupants.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, vec!["skeleton"]);
    // The look that follows agrees with the model, so nothing diverges.
    here.on_event(&answering(Event::RoomSeen(view(&["skeleton"])), ASK), now);
    assert_eq!(here.reconcile.total(), 0, "the look after the kill agrees");
}

/// Another player's kill: no experience award (it was not ours) and a
/// wording `is_kill_line`'s single phrase does not carry. Before the
/// death lexicon the corpse stayed in the model forever — 10 overclaims
/// across 27 blocks of a shared Arena (tests/world_corpus.rs), which is
/// what failed the stage-1 gate.
#[test]
fn a_death_only_the_lexicon_knows_still_empties_the_room() {
    lexicon();
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view(&["large acid slime"])), ASK),
        now,
    );
    assert_eq!(here.occupants.len(), 1);
    here.on_event(
        &unsolicited(Event::Line(
            "The acid slime dissolves into a puddle of bluish goo.".into(),
        )),
        now,
    );
    assert!(
        here.occupants.is_empty(),
        "the rolled instance died with its template"
    );
    // ...and the next block agrees, so nothing is reported as a defect.
    here.on_event(&answering(Event::RoomSeen(view(&[])), ASK), now);
    assert_eq!(here.reconcile.count(DivergenceKind::OccupantExtra), 0);
}

/// A death line announces ONE death. The board renders a pack as
/// separately rolled instances of one template — "angry kobold thief",
/// "thin kobold thief" — and every one of them shares the template's
/// trailing noun, so removing by noun removed the whole pack on the
/// first casualty. Measured: `occupant-missing` 6 -> 35 on the shared
/// Arena stretch the moment the lexicon made this arm fire on anyone's
/// kill (tests/world_corpus.rs, 2026-08-01).
#[test]
fn one_death_line_kills_one_monster() {
    lexicon();
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(
            Event::RoomSeen(view(&["angry kobold thief", "thin kobold thief", "kobold thief"])),
            ASK,
        ),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line(
            "The kobold thief falls to the ground with a shrill cry.".into(),
        )),
        now,
    );
    assert_eq!(
        here.occupants.len(),
        2,
        "one thief died; the other two are still standing there"
    );
}

/// The other half of the same bug: the noun test was `str::contains`, so
/// "The wererat squeals in agony, and dies!" carried the substring "rat"
/// and took the giant rat with it. A template noun matches a word, never
/// a fragment of one.
#[test]
fn a_wererat_death_leaves_the_giant_rat_standing() {
    lexicon();
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view(&["wererat", "giant rat"])), ASK),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line("The wererat squeals in agony, and dies!".into())),
        now,
    );
    let names: Vec<_> = here.occupants.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, vec!["giant rat"]);
}

/// The experience award is the half of `is_kill_line` that fires for
/// every kill of ours whatever the wording — and it names nobody. It
/// stales the render, because something did die out of it, but a model
/// that guessed WHICH occupant off a line with no name in it would be
/// inventing the answer.
#[test]
fn an_experience_award_alone_removes_nobody() {
    lexicon();
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(Event::RoomSeen(view(&["cave bear", "giant rat"])), ASK),
        now,
    );
    here.on_event(
        &unsolicited(Event::Line("You gain 412 experience.".into())),
        now,
    );
    assert_eq!(here.occupants.len(), 2, "the award named nobody");
    assert!(here.view.is_none(), "but something died out of the render");
}

/// Somebody else swept the floor. The board announces it without a
/// denomination — "Mystic picked up some coins." — so the honest fold is
/// to drop the whole floor: what is left of it is a question only the
/// next block can answer.
///
/// Erring this way costs at most one `recheck` before a block relists
/// what is still there. Erring the other way is what the reconciler
/// measured: 35 of these went unmodelled in one Arena session, and every
/// one left a pile in the model that nothing would ever remove — 27
/// `pile-extra`, each of which would spend the `get` budget on coins
/// that were already in somebody else's pack.
#[test]
fn another_players_sweep_clears_the_floor() {
    let now = Instant::now();
    let mut here = Here::default();
    let floor = RoomView {
        name: "Small Cavern".into(),
        items: vec!["11 silver nobles".into(), "7 copper farthings".into()],
        ..RoomView::default()
    };
    here.on_event(&answering(Event::RoomSeen(floor), ASK), now);
    assert_eq!(here.piles.len(), 2);
    here.on_event(
        &unsolicited(Event::Line("Mystic picked up some coins.".into())),
        now,
    );
    assert!(here.piles.is_empty(), "the floor is somebody else's now");
}

/// Work remaining is DERIVED, never queued: a pile is unswept if it is
/// on the floor and the attempts spent on it are under the cap.
#[test]
fn an_unswept_pile_is_work_until_the_cap() {
    let now = Instant::now();
    let mut here = Here::default();
    assert!(here.unswept(2).is_none(), "a bare floor is no work");
    here.on_event(
        &unsolicited(Event::Line("11 silver drop to the ground.".into())),
        now,
    );
    assert_eq!(here.unswept(2).map(|p| p.denom.as_str()), Some("silver"));

    // A `get` the board neither acknowledged nor refused visibly: the
    // pile is listed by every block and the count never terminates, so
    // the ATTEMPTS do. Each attempt waits for the block that lists the
    // pile again -- a get already out is not work until something new
    // says the pile is still there.
    here.note_get_attempt("silver");
    assert!(here.unswept(2).is_none(), "a get is out and nothing has answered it");
    let relisted = answering(Event::RoomSeen(view_with_loot(&["11 silver nobles"])), CmdId(1));
    here.on_event(&relisted, now);
    assert!(here.unswept(2).is_some(), "one try of two, and the block lists it again");
    here.note_get_attempt("silver");
    here.on_event(&relisted, now);
    assert!(
        here.unswept(2).is_none(),
        "a pile the character cannot carry must stop being work"
    );
}

/// A second kill onto the same floor merges into the listed pile and
/// is new evidence there are coins to take, whatever became of the
/// get already out.
#[test]
fn a_fresh_drop_puts_a_pile_with_a_get_out_back_to_work() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &unsolicited(Event::Line("11 silver drop to the ground.".into())),
        now,
    );
    here.note_get_attempt("silver");
    assert!(here.unswept(2).is_none());
    here.on_event(
        &unsolicited(Event::Line("3 silver drop to the ground.".into())),
        now,
    );
    assert_eq!(here.unswept(2).map(|p| p.count), Some(14));
}

/// The acknowledgement takes it off the floor outright, whatever the
/// attempt count says.
#[test]
fn a_swept_pile_is_no_longer_work() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &unsolicited(Event::Line("11 silver drop to the ground.".into())),
        now,
    );
    here.note_get_attempt("silver");
    here.on_event(
        &unsolicited(Event::Line("You picked up 11 silver nobles".into())),
        now,
    );
    assert!(here.unswept(2).is_none());
}

/// Room identity by NAME cannot tell two rooms apart when the board
/// prints the same name for both — Newhaven has twins at 1/2146 and
/// 1/2151, and the sewers run 528 blocks under one name
/// ("Sewer Tunnel", oracle_engage_lock_emptysweep). A step between them
/// then reads as a re-render and every occupant of the room behind us
/// reports as an overclaim.
///
/// An owner that can resolve the id says so, and that outranks the name.
#[test]
fn a_resolved_room_id_outranks_a_shared_room_name() {
    use mud_core::content::RoomId;
    let now = Instant::now();
    let mut here = Here::default();
    here.note_room(RoomId { map: 1, room: 2146 });
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    assert_eq!(here.occupants.len(), 1);

    // Same printed name, different room.
    here.note_room(RoomId { map: 1, room: 2151 });
    here.on_event(&answering(Event::RoomSeen(view(&["giant rat"])), ASK), now);
    assert_eq!(
        here.reconcile.total(),
        0,
        "a step between same-named rooms is not a divergence"
    );
    assert_eq!(here.occupants.len(), 1);
    assert_eq!(here.occupants[0].name, "giant rat");
}

/// Re-noting the SAME room must not wipe what we know — the owner calls
/// this on every block it resolves.
#[test]
fn re_noting_the_same_room_keeps_the_beliefs() {
    use mud_core::content::RoomId;
    let now = Instant::now();
    let mut here = Here::default();
    here.note_room(RoomId { map: 1, room: 2146 });
    here.on_event(&answering(Event::RoomSeen(view(&["cave bear"])), ASK), now);
    here.note_room(RoomId { map: 1, room: 2146 });
    assert_eq!(here.occupants.len(), 1, "same room, same beliefs");
}

// ---------------------------------------------------------------------
// RegenCycle: one phase anchor with a period. Pure and clock-injected.
// ---------------------------------------------------------------------

#[test]
fn an_unstarted_cycle_has_no_next_tick() {
    let c = RegenCycle::new(REGEN_NATURAL);
    assert!(!c.active());
    assert_eq!(c.time_to_next(Instant::now()), None);
    assert!(!c.is_due(Instant::now()));
}

#[test]
fn start_anchors_once_and_stop_forgets() {
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.start(t0);
    // A second start does not move a running anchor.
    c.start(t0 + Duration::from_secs(5));
    assert_eq!(c.time_to_next(t0 + Duration::from_secs(5)), Some(Duration::from_secs(25)));
    c.stop();
    assert!(!c.active());
    assert_eq!(c.time_to_next(t0), None);
}

#[test]
fn time_to_next_counts_in_period_steps_from_the_anchor() {
    // A silent tick at full HP moves no anchor. The projection still
    // lands on the board's cadence.
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.observe(t0);
    assert_eq!(c.time_to_next(t0 + Duration::from_secs(70)), Some(Duration::from_secs(20)));
    assert_eq!(c.time_to_next(t0), Some(REGEN_NATURAL));
}

#[test]
fn a_cycle_is_due_a_period_after_its_anchor_less_the_grace() {
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.observe(t0);
    assert!(!c.is_due(t0 + Duration::from_secs(10)));
    assert!(c.is_due(t0 + REGEN_NATURAL - CLAIM_GRACE));
    assert!(c.is_due(t0 + REGEN_NATURAL + CLAIM_GRACE));
}

#[test]
fn observe_reanchors_a_running_cycle() {
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.observe(t0);
    let t1 = t0 + Duration::from_secs(31);
    c.observe(t1);
    assert_eq!(c.time_to_next(t1), Some(REGEN_NATURAL));
}

#[test]
fn a_stale_cycle_is_due_only_near_a_projected_boundary() {
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.observe(t0);
    // Three periods on, mid period: not this cycle's tick.
    assert!(!c.is_due(t0 + REGEN_NATURAL * 3 + Duration::from_secs(12)));
    // Just before and just after the fourth boundary: due.
    assert!(c.is_due(t0 + REGEN_NATURAL * 4 - Duration::from_millis(500)));
    assert!(c.is_due(t0 + REGEN_NATURAL * 4 + Duration::from_millis(500)));
    // Well past the grace after a boundary: not due.
    assert!(!c.is_due(t0 + REGEN_NATURAL * 4 + Duration::from_secs(3)));
}

#[test]
fn the_round_clock_says_whether_it_is_locked() {
    let mut clock = RoundClock::new();
    assert!(!clock.locked());
    clock.observe(Instant::now());
    assert!(clock.locked());
}

// ---------------------------------------------------------------------
// TickClock: the board never announces a tick. A pool rising between
// two prompts is one, a volley is a round, and our own cast is neither.
// ---------------------------------------------------------------------

fn prompt(hp: i32, mana: Option<i32>, status: Option<Status>) -> Correlated {
    unsolicited(Event::Prompt { hp, mana, status })
}

fn hit() -> Event {
    Event::CombatHit {
        attacker: mud_client::events::Actor::Other("The giant rat".into()),
        target: mud_client::events::Actor::You,
        damage: 3,
    }
}

#[test]
fn the_first_prompt_only_sets_the_baseline() {
    let mut c = TickClock::new();
    c.on_event(&prompt(30, None, None), Instant::now());
    assert!(!c.hp_natural.active());
}

#[test]
fn hp_rising_between_prompts_anchors_the_natural_cycle() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(32, None, None), t1);
    assert_eq!(c.hp_natural.time_to_next(t1), Some(REGEN_NATURAL));
    // Natural HP and natural mana ride one server pulse.
    assert_eq!(c.mana_natural.time_to_next(t1), Some(REGEN_NATURAL));
}

#[test]
fn hp_falling_anchors_nothing() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    c.on_event(&prompt(25, None, None), t0 + Duration::from_secs(5));
    assert!(!c.hp_natural.active());
}

#[test]
fn a_gain_a_period_later_is_the_cycles_own_tick() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(32, None, None), t1);
    let t2 = t1 + REGEN_NATURAL - Duration::from_millis(500);
    c.on_event(&prompt(34, None, None), t2);
    assert_eq!(c.hp_natural.time_to_next(t2), Some(REGEN_NATURAL));
}

#[test]
fn a_gain_well_before_the_period_reanchors_the_natural_cycle() {
    // MudPlay's rule: a gain no running cycle can claim anchors the
    // natural cycle on itself. The board's cadence corrects it on the
    // next real tick.
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(32, None, None), t1);
    let t2 = t1 + Duration::from_secs(10);
    c.on_event(&prompt(33, None, None), t2);
    assert_eq!(c.hp_natural.time_to_next(t2), Some(REGEN_NATURAL));
}

#[test]
fn resting_starts_the_rest_cycle_and_a_bare_prompt_stops_it() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, Some(Status::Resting)), t0);
    assert!(c.hp_rest.active());
    assert!(!c.mana_meditate.active());
    c.on_event(&prompt(30, None, None), t0 + Duration::from_secs(1));
    assert!(!c.hp_rest.active());
}

#[test]
fn an_unknown_status_word_stops_the_bonus_cycles() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, Some(10), Some(Status::Resting)), t0);
    assert!(c.hp_rest.active());
    c.on_event(&prompt(30, Some(10), Some(Status::Other("Stunned".into()))), t0 + Duration::from_secs(1));
    assert!(!c.hp_rest.active());
    assert!(!c.mana_meditate.active());
}

#[test]
fn a_due_gain_while_resting_credits_the_rest_cycle_not_the_natural_one() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, Some(Status::Resting)), t0);
    let t1 = t0 + REGEN_REST;
    c.on_event(&prompt(33, None, Some(Status::Resting)), t1);
    assert_eq!(c.hp_rest.time_to_next(t1), Some(REGEN_REST));
    assert!(!c.hp_natural.active());
}

#[test]
fn a_stale_natural_cycle_does_not_steal_a_rest_tick() {
    // Rested to full, silent for minutes, then hurt. The first rest
    // tick after that belongs to the rest cycle alone.
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(32, None, None), t1);
    let rest_at = t1 + Duration::from_secs(200);
    c.on_event(&prompt(20, None, Some(Status::Resting)), rest_at);
    let tick = rest_at + REGEN_REST;
    c.on_event(&prompt(23, None, Some(Status::Resting)), tick);
    assert_eq!(c.hp_rest.time_to_next(tick), Some(REGEN_REST));
    // 220 s after its anchor the natural cycle sits mid period and
    // must not have moved onto the rest tick.
    assert_eq!(c.hp_natural.time_to_next(tick), Some(REGEN_NATURAL * 8 - Duration::from_secs(220)));
}

#[test]
fn a_gain_just_after_our_cast_is_the_spell_not_a_tick() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, Some(10), None), t0);
    let cast = t0 + Duration::from_secs(1);
    c.on_event(&answering(Event::Line("cast heal".into()), ASK), cast);
    c.on_event(&prompt(40, Some(6), None), cast + Duration::from_secs(1));
    assert!(!c.hp_natural.active());
    // Past the window a gain counts again.
    let later = cast + CAST_WINDOW + Duration::from_secs(1);
    c.on_event(&prompt(42, Some(6), None), later);
    assert_eq!(c.hp_natural.time_to_next(later), Some(REGEN_NATURAL));
}

#[test]
fn a_cast_line_nobody_answered_opens_no_window() {
    // A monster's "attempted to cast" din is unattributed and is not
    // our spell.
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    c.on_event(&unsolicited(Event::Line("cast heal".into())), t0 + Duration::from_secs(1));
    let t1 = t0 + Duration::from_secs(2);
    c.on_event(&prompt(32, None, None), t1);
    assert_eq!(c.hp_natural.time_to_next(t1), Some(REGEN_NATURAL));
}

#[test]
fn a_volley_locks_the_round() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    assert_eq!(c.time_to_round(t0), None);
    c.on_event(&unsolicited(hit()), t0);
    assert_eq!(c.time_to_round(t0 + Duration::from_secs(1)), Some(ROUND - Duration::from_secs(1)));
}

#[test]
fn meditating_starts_the_meditate_cycle_and_a_due_gain_credits_it() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, Some(10), Some(Status::Meditating)), t0);
    assert!(c.mana_meditate.active());
    let t1 = t0 + REGEN_MEDITATE;
    c.on_event(&prompt(30, Some(12), Some(Status::Meditating)), t1);
    assert_eq!(c.mana_meditate.time_to_next(t1), Some(REGEN_MEDITATE));
    assert!(!c.mana_natural.active());
}

#[test]
fn mana_rising_anchors_the_natural_mana_cycle_and_the_hp_pulse() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, Some(10), None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(30, Some(12), None), t1);
    assert_eq!(c.mana_natural.time_to_next(t1), Some(REGEN_NATURAL));
    assert_eq!(c.hp_natural.time_to_next(t1), Some(REGEN_NATURAL));
}

#[test]
fn a_natural_tick_on_one_pool_reanchors_the_other() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, Some(10), None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(32, Some(10), None), t1);
    // HP sits at max from here. Mana keeps ticking and carries the
    // shared pulse onto the HP cycle.
    let t2 = t1 + REGEN_NATURAL + Duration::from_millis(400);
    c.on_event(&prompt(32, Some(12), None), t2);
    assert_eq!(c.hp_natural.time_to_next(t2), Some(REGEN_NATURAL));
}

#[test]
fn a_room_block_moves_no_clock() {
    let mut c = TickClock::new();
    let before = c.clone();
    c.on_event(&unsolicited(Event::RoomSeen(view(&[]))), Instant::now());
    assert_eq!(c, before);
}

/// `is_clear` is the one question the farm's verdict and the assist will
/// both ask of a room: no target the bot would fight, no pile it wants.
/// It composes `aggressive_names`/`has_target_among` and
/// `unswept_wanted` exactly as `farm::StopState::verdict` already does —
/// it must not reimplement either judgement.
#[test]
fn is_clear_is_false_with_a_target_and_true_when_empty() {
    use mud_client::bot::{Bot, BotConfig};
    let now = Instant::now();
    let bot = Bot::new(BotConfig {
        auto_combat: true,
        ..BotConfig::default()
    });
    let mut here = Here::default();
    let room = RoomView {
        also_here: vec!["cave bear".into()],
        items: vec!["11 silver nobles".into()],
        ..view(&[])
    };
    here.on_event(&answering(Event::RoomSeen(room), ASK), now);
    assert!(
        !here.is_clear(&bot),
        "an aggressive monster and a wanted pile are both work"
    );

    // Kill the bear...
    here.on_event(
        &unsolicited(Event::Line(
            "The cave bear falls to the ground with a shrill cry.".into(),
        )),
        now,
    );
    assert!(!here.is_clear(&bot), "the pile is still on the floor");

    // ...and sweep the pile.
    here.on_event(
        &unsolicited(Event::Line("You picked up 11 silver nobles".into())),
        now,
    );
    assert!(here.is_clear(&bot), "nothing left to fight or sweep");
}

/// The stop's floor model is the third sweep site. Work it reports is
/// filtered by what the policy wants, so an ignored pile never holds a
/// stop open and a wanted pile behind it is still found.
#[test]
fn unswept_work_skips_denominations_nobody_wants() {
    let now = Instant::now();
    let mut here = Here::default();
    here.on_event(
        &answering(
            Event::RoomSeen(view_with_loot(&["49 copper farthings", "11 silver nobles"])),
            CmdId(1),
        ),
        now,
    );
    let not_copper = |denom: &str| denom != "copper";
    assert_eq!(
        here.unswept_wanted(2, &not_copper).map(|p| p.denom.as_str()),
        Some("silver")
    );
    let nothing = |_: &str| false;
    assert!(here.unswept_wanted(2, &nothing).is_none());
    assert_eq!(
        here.unswept(2).map(|p| p.denom.as_str()),
        Some("copper"),
        "the unfiltered question still answers first-listed"
    );
}
