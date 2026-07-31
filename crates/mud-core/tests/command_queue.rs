//! Commands queue behind the round timer, and the board echoes a queued
//! command a SECOND time when its turn finally comes.
//!
//! MEASURED (`re/oracle/oracle_blur_duration_timing.log`): `w` is sent,
//! then `n` five milliseconds later. The board says nothing at all for
//! 1.15s — no prompt, no acknowledgement, nothing about `n` — then `w`'s
//! room arrives, then a fresh prompt with `n` echoed against it, and only
//! 1.25s after that does `n`'s own room follow. The echo immediately
//! precedes the reply it belongs to, which is what makes it usable as
//! attribution (`mud_client::correlate`).
//!
//! `command_round_seconds` defaults to 0, which means "run everything
//! the moment it arrives" — the shape every other test in this crate
//! and every client fixture was written against.

use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HALL: RoomId = RoomId { map: 1, room: 1 };
const CELL: RoomId = RoomId { map: 1, room: 2 };

fn world() -> Content {
    let mut content = Content::default();
    let mut hall = Room {
        id: HALL,
        name: "Hall".into(),
        ..Default::default()
    };
    hall.exits[Direction::North as usize] = Some(Exit {
        dest: CELL,
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    let mut cell = Room {
        id: CELL,
        name: "Cell".into(),
        ..Default::default()
    };
    cell.exits[Direction::South as usize] = Some(Exit {
        dest: HALL,
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    content.add_room(hall);
    content.add_room(cell);
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
        abilities: vec![],
        hp_per_level: 6,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 6,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

fn player() -> Player {
    Player {
        name: "Alice".into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        current_hp: 10,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HALL,
        ..Default::default()
    }
}

fn shown(events: &[Event], session: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == session => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn rounds(seconds: u8) -> CoreConfig {
    CoreConfig {
        command_round_seconds: seconds,
        ..CoreConfig::default()
    }
}

/// The first command of a quiet moment runs at once and is never echoed
/// by the core — the socket already echoed it on receipt.
#[test]
fn an_unqueued_command_runs_immediately_and_is_not_re_echoed() {
    let mut core = Core::new(world(), rounds(3));
    let alice = core.attach_player(player());
    core.drain_events();

    core.input(alice, "look");
    let out = shown(&core.drain_events(), alice);
    assert!(out.contains("Hall"), "it answered right away: {out:?}");
    assert!(
        !out.contains("look\n"),
        "the core does not repeat the receipt echo: {out:?}"
    );
}

/// The measured shape: silence while queued, then echo immediately
/// before the reply.
#[test]
fn a_queued_command_stays_silent_then_echoes_before_its_reply() {
    let mut core = Core::new(world(), rounds(3));
    let alice = core.attach_player(player());
    core.drain_events();

    core.input(alice, "look"); // runs now, arms the round
    core.drain_events();
    core.input(alice, "n"); // queues behind it

    let while_queued = shown(&core.drain_events(), alice);
    assert!(
        while_queued.is_empty(),
        "a queued command hears nothing about itself, not even a prompt: {while_queued:?}"
    );

    // Two ticks are still inside the round.
    core.tick();
    core.tick();
    assert!(
        shown(&core.drain_events(), alice).is_empty(),
        "still inside the round"
    );

    core.tick();
    let out = shown(&core.drain_events(), alice);
    let echo = out.find("n\n").expect("the execution echo");
    let reply = out.find("Cell").expect("the room it moved to");
    assert!(echo < reply, "the echo precedes its reply: {out:?}");
    assert!(
        out.trim_end_matches(' ').ends_with("[HP=10]:"),
        "and a prompt closes the burst: {out:?}"
    );
}

/// The echo glues to the dangling prompt instead of erasing it — the
/// capture shows `[HP=26/MA=12]:n` on one physical line.
#[test]
fn the_execution_echo_does_not_erase_the_prompt() {
    let mut core = Core::new(
        world(),
        CoreConfig {
            ansi: true,
            ..rounds(1)
        },
    );
    let mut ansi = player();
    ansi.ansi = true;
    let alice = core.attach_player(ansi);
    core.drain_events();

    core.input(alice, "look");
    core.drain_events();
    core.input(alice, "n");
    core.tick();

    let out = shown(&core.drain_events(), alice);
    assert!(
        out.starts_with("n\n"),
        "the echo lands straight on the prompt line: {out:?}"
    );
}

#[test]
fn queued_commands_drain_in_order_one_per_round() {
    let mut core = Core::new(world(), rounds(2));
    let alice = core.attach_player(player());
    core.drain_events();

    core.input(alice, "look"); // runs, arms the round
    core.drain_events();
    core.input(alice, "n"); // queued first
    core.input(alice, "s"); // queued behind it

    core.tick();
    core.tick();
    let first = shown(&core.drain_events(), alice);
    assert!(first.contains("Cell"), "north ran first: {first:?}");
    assert!(!first.contains("Hall"), "and only north: {first:?}");

    core.tick();
    core.tick();
    let second = shown(&core.drain_events(), alice);
    assert!(second.contains("Hall"), "south ran next: {second:?}");
}

/// The default is the behaviour every other test was written against:
/// no queue, no gate, no second echo.
#[test]
fn without_a_round_configured_nothing_queues() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player());
    core.drain_events();

    core.input(alice, "look");
    core.input(alice, "n");
    let out = shown(&core.drain_events(), alice);
    assert!(out.contains("Hall") && out.contains("Cell"), "{out:?}");
    assert!(
        !out.contains("n\n"),
        "and no execution echo is ever emitted: {out:?}"
    );
}

/// A round timer must not turn an idle session into a ticking prompt
/// machine — `board_cadence` pins that an idle board sends nothing.
#[test]
fn an_idle_session_stays_silent_across_rounds() {
    let mut core = Core::new(world(), rounds(3));
    let alice = core.attach_player(player());
    core.drain_events();

    for _ in 0..10 {
        core.tick();
    }
    assert!(
        shown(&core.drain_events(), alice).is_empty(),
        "quiet rounds print nothing"
    );
}

/// The exit meditation refuses input on the spot; queuing the refusal
/// until after the player has already left would answer nobody.
#[test]
fn the_exit_meditation_refusal_is_never_queued() {
    let mut core = Core::new(world(), rounds(3));
    let alice = core.attach_player(player());
    core.drain_events();

    core.input(alice, "x"); // begins the meditation
    core.drain_events();
    core.input(alice, "look");
    let out = shown(&core.drain_events(), alice);
    assert!(
        out.contains(mud_core::text::MEDITATION_BLOCKED),
        "refused immediately: {out:?}"
    );
}
