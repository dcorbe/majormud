//! World-state tests: the board's round clock (and, later, `Here`).
//! Pure and clock-injected like the bot and StopState suites.

use std::time::{Duration, Instant};

use mud_client::correlate::{CmdId, Correlated};
use mud_client::events::{Event, RoomView};
use mud_client::world::{DivergenceKind, Here, OccupantKind, ROUND, RoundClock};

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

/// Another player's kill: no experience award (it was not ours) and a
/// wording `is_kill_line`'s single phrase does not carry. Before the
/// death lexicon the corpse stayed in the model forever — 10 overclaims
/// across 27 blocks of a shared Arena (tests/world_corpus.rs), which is
/// what failed the stage-1 gate.
#[test]
fn a_death_only_the_lexicon_knows_still_empties_the_room() {
    mud_client::deaths::init_with(mud_client::deaths::DeathLexicon::from_pairs([(
        "acid slime".to_string(),
        "The acid slime dissolves into a puddle of bluish goo.".to_string(),
    )]));
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
