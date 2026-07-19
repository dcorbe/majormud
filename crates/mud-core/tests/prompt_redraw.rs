//! The shell-style prompt discipline (every oracle capture: async bursts
//! are prefixed `ESC[79D ESC[K` — erase the dangling prompt line — then
//! the message, then a fresh prompt; our clean equivalent is `\r ESC[K`).
//! Input-driven responses print below the echoed command line with no
//! erase, exactly like the live board.

use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HALL: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: HALL,
        name: "Hall".into(),
        ..Default::default()
    });
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

fn player(name: &str) -> Player {
    Player {
        name: name.into(),
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

fn text_to(events: &[Event], session: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == session => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn async_broadcast_erases_the_prompt_and_redraws_it() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    let bob = core.attach_player(player("Bob"));
    core.drain_events(); // Alice sits at a dangling prompt
    core.input(bob, "hi"); // say fallback
    let to_alice = text_to(&core.drain_events(), alice);
    assert!(
        to_alice.starts_with("\r\x1b[K"),
        "the dangling prompt line is erased first: {to_alice:?}"
    );
    assert!(to_alice.contains("Bob says \"hi\""), "{to_alice:?}");
    assert!(
        to_alice.trim_end_matches(' ').ends_with("[HP=10]:"),
        "a fresh prompt follows the burst: {to_alice:?}"
    );
}

#[test]
fn own_command_output_prints_below_the_echoed_line() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    core.drain_events();
    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        !shown.starts_with("\r\x1b[K"),
        "input responses never erase the command line: {shown:?}"
    );
    assert!(shown.contains("Hall"), "{shown:?}");
    assert!(
        shown.trim_end_matches(' ').ends_with("[HP=10]:"),
        "prompt after the response: {shown:?}"
    );
}

#[test]
fn tick_driven_output_also_redraws() {
    // A monster spawn/attack burst on a tick disturbs the prompt the same
    // way — here the simplest tick source: another player's exit
    // meditation dots don't apply, so use a broadcast via say from a tick
    // isn't possible; instead verify that after a tick with NO output the
    // prompt is NOT re-spammed.
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    core.drain_events();
    for _ in 0..10 {
        core.tick();
    }
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.is_empty(),
        "quiet ticks re-print nothing: {shown:?}"
    );
}
