//! DISARM TRAP <direction> (theft.md §10). Mechanical (type 9) traps:
//! roll vs DisarmTraps — success disarms (state 0->1) and schedules the
//! re-arm; a near miss (within 10) is safe; a real miss TRIGGERS the
//! trap — the message record fires (user line 1, room line 2) and the
//! damage roll genrdn(rating/2, rating+1) lands. No crime consequence.
//! Spell traps (0x18) await the room-cast plumbing.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Message, MessageId, Race, RaceId, Room, RoomId,
    StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HALL: RoomId = RoomId { map: 1, room: 1 };
const NEXT: RoomId = RoomId { map: 1, room: 2 };

fn world() -> Content {
    let mut content = Content::default();
    let mut hall = Room {
        id: HALL,
        name: "Hall".into(),
        ..Default::default()
    };
    hall.exits[Direction::North as usize] = Some(Exit {
        dest: NEXT,
        exit_type: 9,
        param: 30,  // damage rating
        param2: 0,  // armed
        param4: 900, // trigger message record
        ..Default::default()
    });
    hall.exits[Direction::East as usize] = Some(Exit {
        dest: NEXT,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(hall);
    content.add_room(Room {
        id: NEXT,
        name: "Next".into(),
        ..Default::default()
    });
    content.add_message(Message {
        id: MessageId(900),
        lines: vec![
            "A dart shoots out and stabs you!".into(),
            "A dart shoots out at %s!".into(),
            String::new(),
        ],
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
    // The FindTraps gate ability unlocks the disarm skill derivation.
    content.add_class(Class {
        id: ClassId(1),
        name: "Tinker".into(),
        abilities: vec![(Ability::from_id(0x28).unwrap(), 0)],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    // Ungated: disarm skill 0 — every real miss triggers.
    content.add_class(Class {
        id: ClassId(2),
        name: "Butterfingers".into(),
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

fn person(name: &str, sharp: bool, level: u16) -> Player {
    // DisarmTraps derives purely from stats + level:
    // (Int + Agl + Chm*2 + growth*28)/7.
    let stats = if sharp {
        StatBlock {
            intellect: 90,
            wisdom: 40,
            strength: 40,
            health: 40,
            agility: 90,
            charm: 90,
        }
    } else {
        StatBlock {
            intellect: 5,
            wisdom: 5,
            strength: 40,
            health: 40,
            agility: 5,
            charm: 5,
        }
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(if sharp { 1 } else { 2 }),
        level,
        stats,
        base_stats: stats,
        current_hp: 200,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HALL,
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
fn no_trap_gives_the_fail_line() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Tink", true, 10));
    core.drain_events();
    core.input(s, "disarm trap e");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You failed to disarm any trap to the east."),
        "{out:?}"
    );
}

#[test]
fn success_disarms_until_the_rearm() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Tink", true, 15)); // high skill
    core.drain_events();
    let mut disarmed = false;
    for _ in 0..40 {
        core.input(s, "disarm trap n");
        let own = texts(&core.drain_events(), s);
        if own.contains("You successfully disarmed the trap to the north.") {
            disarmed = true;
            break;
        }
        if core.player_snapshot(s).current_hp < 50 {
            panic!("too many triggers for a sharp tinker");
        }
    }
    assert!(disarmed, "a skilled disarm eventually lands");
    // Disarmed: further attempts find no armed trap.
    core.input(s, "disarm trap n");
    let own = texts(&core.drain_events(), s);
    assert!(
        own.contains("You failed to disarm any trap to the north."),
        "disarmed trap reports no trap: {own:?}"
    );
    // The re-arm kick (1 delay unit = 300 s) arms it again.
    for _ in 0..305 {
        core.tick();
        core.drain_events();
    }
    let mut rearmed = false;
    for _ in 0..40 {
        core.input(s, "disarm trap n");
        let own = texts(&core.drain_events(), s);
        if own.contains("You successfully disarmed the trap to the north.") {
            rearmed = true;
            break;
        }
    }
    assert!(rearmed, "the trap re-armed after the kick");
}

#[test]
fn a_bad_miss_triggers_the_trap() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Butter", false, 1)); // skill ~16
    let watcher = core.attach_player(person("Bystander", true, 5));
    core.drain_events();
    let mut triggered = false;
    for _ in 0..60 {
        let hp_before = core.player_snapshot(s).current_hp;
        core.input(s, "disarm trap n");
        let events = core.drain_events();
        let own = texts(&events, s);
        if own.contains("A dart shoots out and stabs you!") {
            triggered = true;
            let seen = texts(&events, watcher);
            assert!(
                seen.contains("A dart shoots out at Butter!"),
                "room sees line 2 with the name bound: {seen:?}"
            );
            let hp_after = core.player_snapshot(s).current_hp;
            let dmg = hp_before - hp_after;
            assert!(
                (15..=31).contains(&dmg),
                "damage genrdn(rating/2, rating+1) with rating 30: {dmg}"
            );
            break;
        }
        if own.contains("successfully disarmed") {
            // Rearm not scheduled in this branch of the test; just retry
            // against the fail line until the RNG cooperates.
            break;
        }
    }
    assert!(triggered, "a clumsy tinker eventually sets it off");
}
