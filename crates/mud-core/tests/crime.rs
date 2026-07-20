//! Crime / fame / legal levels (`crime.md`). Slice-3 scope: the pure
//! tier function, the NPC-path evil charge (`add_evil_points` with
//! victim = -1 — refusal gates in order, the dark-cloud line, the
//! minimum-10 bump, the 30000 cap), and the tier name table. Pair
//! timers and the player-victim path land with rob in slice 4.

use mud_core::crime::{self, LegalLevel};

#[test]
fn legal_level_thresholds_match_the_decompile() {
    // get_legal_level 0x44e390 (crime.md §1): < -200 → 7, < -0x32 → 6,
    // < 0x1e → 0, < 0x28 → 1, < 0x50 → 2, < 0x78 → 3, < 0xd2 → 4, else 5.
    let cases: &[(i16, u8)] = &[
        (-32768, 7),
        (-201, 7),
        (-200, 6),
        (-51, 6),
        (-50, 0),
        (0, 0),
        (29, 0),
        (30, 1),
        (39, 1),
        (40, 2),
        (79, 2),
        (80, 3),
        (119, 3),
        (120, 4),
        (209, 4),
        (210, 5),
        (32767, 5),
    ];
    for &(fame, level) in cases {
        assert_eq!(
            crime::legal_level(fame) as u8,
            level,
            "fame {fame} -> level {level}"
        );
    }
}

#[test]
fn tier_names_match_the_dll_table() {
    // Name table at 0x4881a4, indexed by level (crime.md §1). Level 0
    // prints no word in the WHO list but the name is "Neutral".
    let names: &[(LegalLevel, &str)] = &[
        (LegalLevel::Neutral, "Neutral"),
        (LegalLevel::Seedy, "Seedy"),
        (LegalLevel::Outlaw, "Outlaw"),
        (LegalLevel::Criminal, "Criminal"),
        (LegalLevel::Villain, "Villain"),
        (LegalLevel::Fiend, "FIEND"),
        (LegalLevel::Good, "Good"),
        (LegalLevel::Saint, "Saint"),
    ];
    for (level, name) in names {
        assert_eq!(level.name(), *name);
    }
}

#[test]
fn npc_charge_gate_order_and_messages() {
    // crime.md §2.1, checked in order. Each refusal returns the message
    // and leaves fame untouched.
    // 1. Warn on Evil.
    let mut fame = 0i16;
    let r = crime::charge_npc_evil(&mut fame, true, false, 10);
    assert_eq!(
        r,
        Err("To do this action, you must turn off your evil warnings."),
        "warn-on-evil refuses first"
    );
    assert_eq!(fame, 0);
    // 2. The 300 action ceiling (checked before Lawful).
    let mut fame = 301i16;
    let r = crime::charge_npc_evil(&mut fame, false, true, 10);
    assert_eq!(
        r,
        Err("You have progressed too far to the evil side to do this action."),
    );
    assert_eq!(fame, 301);
    // 3. Committed Lawful.
    let mut fame = 0i16;
    let r = crime::charge_npc_evil(&mut fame, false, true, 10);
    assert_eq!(
        r,
        Err("You have chosen a way of life which does not allow this action."),
    );
    // Otherwise: the dark-cloud line and the add.
    let mut fame = 50i16;
    let r = crime::charge_npc_evil(&mut fame, false, false, 10);
    assert_eq!(r, Ok("A dark cloud passes over you"));
    assert_eq!(fame, 60);
}

#[test]
fn good_side_attackers_jump_straight_to_ten() {
    // crime.md §2.4: fame < 0 and fame + points < 10 → fame becomes 10.
    let mut fame = -201i16; // a Saint
    crime::charge_npc_evil(&mut fame, false, false, 10).unwrap();
    assert_eq!(fame, 10, "one evil act erases sainthood");
    // A good-side attacker whose add already clears 10 keeps the sum.
    let mut fame = -2i16;
    crime::charge_npc_evil(&mut fame, false, false, 15).unwrap();
    assert_eq!(fame, 13);
}

#[test]
fn the_action_ceiling_precedes_everything_reachable() {
    // fame > 300 refuses BEFORE any add — so the §2.4 "fame < 30000"
    // add-guard is unreachable through this path (quest addevil is the
    // writer that can push past the ceiling). Under the ceiling the add
    // lands even when it crosses it.
    let mut fame = 30000i16;
    let r = crime::charge_npc_evil(&mut fame, false, false, 10);
    assert!(r.is_err(), "the 300 ceiling refuses first");
    assert_eq!(fame, 30000);
    let mut fame = 299i16;
    crime::charge_npc_evil(&mut fame, false, false, 10).unwrap();
    assert_eq!(fame, 309, "under the ceiling the add still lands past it");
}

// --- the shipped evil writers: attacking passive monsters (crime.md
// §2.5 attack_user_monster 26116, gate 26113) + the SET EVIL toggle ---

use mud_core::content::{Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId,
    StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const SQUARE: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: SQUARE,
        name: "Square".into(),
        ..Default::default()
    });
    // behaviour 0 = passive townsfolk; behaviour 1 = aggressive.
    content.add_monster(Monster {
        id: MonsterId(1),
        name: "town crier".into(),
        hitpoints: 50,
        energy: 1000,
        behaviour: 0,
        ..Default::default()
    });
    content.add_monster(Monster {
        id: MonsterId(2),
        name: "bandit".into(),
        hitpoints: 50,
        energy: 1000,
        behaviour: 1,
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

fn citizen(name: &str, warn: bool) -> Player {
    let stats = StatBlock {
        intellect: 40,
        wisdom: 30,
        strength: 60,
        health: 30,
        agility: 60,
        charm: 50,
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 5,
        stats,
        base_stats: stats,
        current_hp: 40,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: SQUARE,
        warn_on_evil: warn,
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
fn warn_on_evil_refuses_the_passive_attack() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(citizen("Cautious", true));
    core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "attack crier");
    let events = core.drain_events();
    let out = texts(&events, s);
    assert!(
        out.contains("To do this action, you must turn off your evil warnings."),
        "{out:?}"
    );
    assert!(!out.contains("*Combat Engaged*"), "refusal aborts: {out:?}");
    assert_eq!(core.player_fame(s), 0);
}

#[test]
fn attacking_a_passive_monster_charges_ten_evil() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(citizen("Thug", false));
    core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "attack crier");
    let events = core.drain_events();
    let out = texts(&events, s);
    assert!(out.contains("A dark cloud passes over you"), "{out:?}");
    assert!(out.contains("*Combat Engaged*"), "{out:?}");
    assert_eq!(core.player_fame(s), 10);
    // Re-attacking while the crier is now fighting back is free.
    core.input(s, "attack crier");
    let _ = core.drain_events();
    assert_eq!(core.player_fame(s), 10, "no charge once it targets you");
}

#[test]
fn attacking_an_aggressive_monster_is_free() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(citizen("Guard", true)); // warn ON — still free
    core.spawn_monster(MonsterId(2), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "attack bandit");
    let events = core.drain_events();
    let out = texts(&events, s);
    assert!(out.contains("*Combat Engaged*"), "{out:?}");
    assert_eq!(core.player_fame(s), 0);
}

#[test]
fn set_evil_toggles_the_warning() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(citizen("Learner", true));
    core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    // Toggle warnings OFF (the DLL confirm for ON is measured; the OFF
    // wording is ORACLE-VERIFY).
    core.input(s, "set evil");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You will no longer be warned before performing evil actions."),
        "{out:?}"
    );
    core.input(s, "attack crier");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("A dark cloud passes over you"), "{out:?}");
    assert_eq!(core.player_fame(s), 10);
    // And back ON (the measured DLL string).
    core.input(s, "set evil");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You will now be warned and stopped from performing evil actions"),
        "{out:?}"
    );
}
