//! PICKLOCK + the exit lock runtime + re-lock timers (theft.md §8).
//! Pickable types: 2 (door), 7/0xb (secret door/gate). Type-2 state
//! lives in the 0x39c word (disk para2), its pick modifier in para3 and
//! re-lock delay in para4; types 7/0xb keep state in para1, modifier in
//! para2, re-lock in para3. Re-lock units are 300 s each; locked exits
//! block movement. No crime consequence either way.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const CELL: RoomId = RoomId { map: 1, room: 1 };
const VAULT: RoomId = RoomId { map: 1, room: 2 };

fn world() -> Content {
    let mut content = Content::default();
    let mut cell = Room {
        id: CELL,
        name: "Cell".into(),
        ..Default::default()
    };
    // North: a locked type-2 door, friendly pick modifier, 1 re-lock unit.
    cell.exits[Direction::North as usize] = Some(Exit {
        dest: VAULT,
        exit_type: 2,
        param2: 2, // locked (0x39c)
        param3: 60, // pick modifier (0x3b0)
        param4: 1, // re-lock delay units (0x3d8)
        door_closed: true,
        ..Default::default()
    });
    // East: a hard secret gate (type 0xb), heavily negative modifier.
    cell.exits[Direction::East as usize] = Some(Exit {
        dest: VAULT,
        exit_type: 0xb,
        param: 2,    // locked (0x374)
        param2: -200, // pick modifier (0x39c) — buries any skill
        param3: 1,   // re-lock (0x3b0)
        ..Default::default()
    });
    content.add_room(cell);
    let mut vault = Room {
        id: VAULT,
        name: "Vault".into(),
        ..Default::default()
    };
    // The reciprocal locked door back south (unlocks with the pick).
    vault.exits[Direction::South as usize] = Some(Exit {
        dest: CELL,
        exit_type: 2,
        param2: 2,
        param3: 60,
        param4: 1,
        door_closed: true,
        ..Default::default()
    });
    content.add_room(vault);
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
        name: "Thief".into(),
        abilities: vec![(Ability::from_id(0x25).unwrap(), 40)], // Picklocks
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    content.add_class(Class {
        id: ClassId(2),
        name: "Clod".into(),
        abilities: vec![],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

fn thief(name: &str, class: u16) -> Player {
    let stats = StatBlock {
        intellect: 60,
        wisdom: 40,
        strength: 40,
        health: 40,
        agility: 60,
        charm: 40,
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(class),
        level: 5,
        stats,
        base_stats: stats,
        current_hp: 30,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: CELL,
        ..Default::default()
    }
}

fn texts(events: &[Event], who: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session, text } if *session == who => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn syntax_and_no_exit_masking() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(thief("Pick", 1));
    core.drain_events();
    core.input(s, "picklock");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Syntax: PICKLOCK {direction}"), "{out:?}");
    // No exit south: the same fail string masks it.
    core.input(s, "picklock s");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Your skill fails you this time."), "{out:?}");
}

#[test]
fn locked_doors_block_until_picked_then_relock() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(thief("Pick", 1));
    let watcher = core.attach_player(thief("Watch", 2));
    core.drain_events();

    // Locked: movement refuses.
    core.input(s, "n");
    core.drain_events();
    assert_eq!(
        core.player_snapshot(s).location,
        CELL,
        "a locked door blocks movement"
    );

    // Pick it (modifier 60 + skill: practically certain).
    let mut unlocked = false;
    for _ in 0..20 {
        core.input(s, "picklock n");
        let events = core.drain_events();
        let own = texts(&events, s);
        if own.contains("You successfully unlocked the door.") {
            unlocked = true;
            let seen = texts(&events, watcher);
            assert!(
                seen.contains("You see Pick pick the lock on the door to the north."),
                "{seen:?}"
            );
            break;
        }
    }
    assert!(unlocked, "the friendly lock opens");

    // Passable now.
    core.input(s, "n");
    core.drain_events();
    assert_eq!(core.player_snapshot(s).location, VAULT, "picked = open");
    core.input(s, "s");
    core.drain_events();

    // One re-lock unit = 300 s.
    let mut relock_line = String::new();
    for _ in 0..305 {
        core.tick();
        relock_line.push_str(&texts(&core.drain_events(), watcher));
    }
    assert!(
        relock_line.contains("The door to the north just locked!"),
        "{relock_line:?}"
    );
    core.input(s, "n");
    core.drain_events();
    assert_eq!(
        core.player_snapshot(s).location,
        CELL,
        "the re-locked door blocks again"
    );
}

#[test]
fn hard_locks_and_skillless_picks_fail() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(thief("Pick", 1));
    core.drain_events();
    // The east gate's -80 modifier buries the skill: always fails.
    core.input(s, "picklock e");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Your skill fails you this time."), "{out:?}");

    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(thief("Clumsy", 2)); // no Picklocks gate
    core.drain_events();
    core.input(s, "picklock n");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Your skill fails you this time."), "{out:?}");
}
