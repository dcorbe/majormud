//! SNEAK and HIDE (theft.md §11). Success is SILENT (the DLL prints a
//! bare `\r`); failure self-doubt lines are perception-gated on the
//! actor; sneak movement replaces the leave/arrive broadcasts with
//! perception-FILTERED "You notice %s sneaking..." lines. The delay
//! gates (`add_delay`) and the HIDE item/coin stash arrive with the
//! room-hidden-storage work later in the slice.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};
use mud_core::stats::stealth_chance;

const HERE: RoomId = RoomId { map: 1, room: 1 };
const THERE: RoomId = RoomId { map: 1, room: 2 };

fn world() -> Content {
    let mut content = Content::default();
    let mut here = Room {
        id: HERE,
        name: "Shadows".into(),
        ..Default::default()
    };
    here.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(here);
    let mut there = Room {
        id: THERE,
        name: "Lamplight".into(),
        ..Default::default()
    };
    there.exits[Direction::South as usize] = Some(Exit {
        dest: HERE,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(there);
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
    // A thief-shaped class: Stealth granted, plus PerStealth for the
    // sneak auto-success path.
    content.add_class(Class {
        id: ClassId(1),
        name: "Thief".into(),
        abilities: vec![
            (Ability::from_id(0x67).unwrap(), 1),  // ClassStealth (the gate)
            (Ability::from_id(0x1b).unwrap(), 20), // Stealth bonus
            (Ability::from_id(0xba).unwrap(), 1),  // PerStealth
        ],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    // A plain observer class.
    content.add_class(Class {
        id: ClassId(2),
        name: "Watcher".into(),
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

fn person(name: &str, class: u16, perception_stats: bool) -> Player {
    // High Int/Agl push the Perception derivation up; low ones bury it.
    let stats = if perception_stats {
        StatBlock {
            intellect: 90,
            wisdom: 70,
            strength: 40,
            health: 40,
            agility: 90,
            charm: 40,
        }
    } else {
        StatBlock {
            intellect: 5,
            wisdom: 5,
            strength: 40,
            health: 40,
            agility: 5,
            charm: 40,
        }
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(class),
        level: 10,
        stats,
        base_stats: stats,
        current_hp: 30,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HERE,
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
fn stealth_chance_matches_the_helper_formula() {
    // theft.md §11.3: base Stealth, encumbrance bands −10/−5, −1 per
    // other player and live monster, cap then floor.
    assert_eq!(stealth_chance(60, 70, 2, 3, 95), 45);
    assert_eq!(stealth_chance(60, 40, 0, 0, 95), 55);
    assert_eq!(stealth_chance(60, 0, 1, 0, 95), 59);
    assert_eq!(stealth_chance(120, 0, 0, 0, 95), 95, "cap");
    assert_eq!(stealth_chance(3, 90, 4, 4, 95), 0, "floored at zero");
}

#[test]
fn sneak_with_perstealth_arms_silently_and_filters_the_move() {
    let mut core = Core::new(world(), CoreConfig::default());
    let sneak = core.attach_player(person("Shade", 1, false));
    let sharp = core.attach_player(person("Hawk", 2, true));
    let dull = core.attach_player(person("Mole", 2, false));
    core.drain_events();

    core.input(sneak, "sneak");
    let events = core.drain_events();
    let own = texts(&events, sneak);
    assert!(own.contains("Attempting to sneak..."), "{own:?}");
    assert!(
        !own.contains("don't think"),
        "PerStealth auto-success is silent: {own:?}"
    );

    core.input(sneak, "n");
    let events = core.drain_events();
    let to_sharp = texts(&events, sharp);
    let to_dull = texts(&events, dull);
    assert!(
        to_sharp.contains("You notice Shade sneaking out to the north"),
        "high perception sees the sneak: {to_sharp:?}"
    );
    assert!(
        !to_dull.contains("Shade"),
        "low perception sees nothing at all: {to_dull:?}"
    );
    assert!(
        !to_sharp.contains("just left") && !to_dull.contains("just left"),
        "the normal departure broadcast is suppressed"
    );
}

#[test]
fn hide_sets_the_hidden_flag_and_leaves_also_here() {
    let mut core = Core::new(world(), CoreConfig::default());
    let hider = core.attach_player(person("Wisp", 1, false));
    let watcher = core.attach_player(person("Guard", 2, true));
    core.drain_events();

    // Stealth ~high for a level-10 thief; the fixed test seed decides
    // the roll — assert on the resulting state, not the roll.
    core.input(hider, "hide");
    let own = texts(&core.drain_events(), hider);
    assert!(own.contains("Attempting to hide..."), "{own:?}");
    if core.player_hidden(hider) {
        core.input(watcher, "look");
        let seen = texts(&core.drain_events(), watcher);
        assert!(
            !seen.contains("Wisp"),
            "hidden players leave the Also-here line: {seen:?}"
        );
    } else {
        // Failure is either the perception-gated self-doubt line or
        // plain silence (the DLL's bare \r) — never a success claim.
        assert!(!own.contains("hidden!"), "no success claim: {own:?}");
    }
}

#[test]
fn moving_normally_clears_hidden() {
    let mut core = Core::new(world(), CoreConfig::default());
    let hider = core.attach_player(person("Wisp", 1, false));
    core.drain_events();
    for _ in 0..20 {
        core.input(hider, "hide");
        core.drain_events();
        if core.player_hidden(hider) {
            break;
        }
    }
    assert!(core.player_hidden(hider), "eventually hides");
    core.input(hider, "n"); // un-sneaked movement
    core.drain_events();
    assert!(
        !core.player_hidden(hider),
        "non-sneak movement clears the hidden byte"
    );
}
