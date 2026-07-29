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
    // Both confirms are now read straight out of the DLL: the OFF string
    // at 0xd76f1 and the ON string at 0xd772e, adjacent in the binary.
    // Neither matches what was guessed here before.
    core.input(s, "set evil");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You will no longer be stopped from performing evil actions."),
        "{out:?}"
    );
    core.input(s, "attack crier");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("A dark cloud passes over you"), "{out:?}");
    assert_eq!(core.player_fame(s), 10);
    // And back ON.
    core.input(s, "set evil");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You will now be warned and stopped from doing most evil actions."),
        "{out:?}"
    );
}

// --- alignment gates (crime.md §6.1 lattice, §6.3 exits, §2.6 creation) ---

use mud_core::ability::Ability;
use mud_core::content::{Exit, Item, ItemId};

fn aligned_world() -> Content {
    let mut content = world();
    // A type-0x14 alignment exit east: too-good bound 0, too-evil 29.
    let room = content.rooms.get_mut(&SQUARE).unwrap();
    room.exits[2] = Some(Exit {
        dest: SQUARE,
        exit_type: 0x14,
        param: 0,
        param2: 29,
        ..Default::default()
    });
    // A holy circlet: wearable head gear carrying Good (97).
    content.add_item(Item {
        id: ItemId(50),
        name: "holy circlet".into(),
        abilities: vec![(Ability::from_id(97).unwrap(), 1)],
        item_type: 0,
        worn_on: 2,
        uses: -1,
        ..Default::default()
    });
    content
}

#[test]
fn alignment_lattice_refuses_by_legal_level() {
    // Neutral (fame 0) may not wear Good gear; a Good character (fame
    // -100) may. (crime.md §6.1 row 1 vs row 3.)
    let mut core = Core::new(aligned_world(), CoreConfig::default());
    let mut p = citizen("Pilgrim", false);
    p.inventory.push((ItemId(50), -1));
    let s = core.attach_player(p);
    core.drain_events();
    core.input(s, "wear circlet");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("You may not wear that item!"), "{out:?}");

    let mut core = Core::new(aligned_world(), CoreConfig::default());
    let mut p = citizen("Cleric", false);
    p.fame = -100;
    p.inventory.push((ItemId(50), -1));
    let s = core.attach_player(p);
    core.drain_events();
    core.input(s, "wear circlet");
    let out = texts(&core.drain_events(), s);
    assert!(!out.contains("You may not wear"), "good character wears it: {out:?}");
}

#[test]
fn crossing_a_tier_force_removes_illegal_gear() {
    // A Good character wearing Good gear commits evil: the minimum-10
    // bump lands them at fame 10 (Neutral) and the circlet is forced
    // off (update_allowed_worn_items, crime.md §2.4).
    let mut core = Core::new(aligned_world(), CoreConfig::default());
    let mut p = citizen("Fallen", false);
    p.fame = -60; // Good tier
    p.worn.push((ItemId(50), -1));
    let s = core.attach_player(p);
    core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "attack crier");
    let out = texts(&core.drain_events(), s);
    assert_eq!(core.player_fame(s), 10, "minimum-10 bump");
    assert!(
        out.contains("Your holy circlet has been removed."),
        "force-removal fires: {out:?}"
    );
}

#[test]
fn alignment_exits_gate_both_directions() {
    // Bounds (0, 29): fame -60 is too good, fame 30 too evil, 0 passes.
    for (fame, expect) in [
        (-60i16, Some("You are too good to go through this exit!")),
        (30, Some("You are too evil to go through this exit!")),
        (0, None),
    ] {
        let mut core = Core::new(aligned_world(), CoreConfig::default());
        let mut p = citizen("Walker", false);
        p.fame = fame;
        let s = core.attach_player(p);
        core.drain_events();
        core.input(s, "e");
        let out = texts(&core.drain_events(), s);
        match expect {
            Some(msg) => assert!(out.contains(msg), "fame {fame}: {out:?}"),
            None => assert!(
                !out.contains("too good") && !out.contains("too evil"),
                "fame {fame} passes: {out:?}"
            ),
        }
    }
}
