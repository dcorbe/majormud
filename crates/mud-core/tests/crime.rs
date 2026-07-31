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

// --- the ability-52 (EvilInCombat) cast charge (crime.md §2.5.1,
// cast_monster_target 43323-43347): a spell CARRYING EvilInCombat
// charges 10 evil + earns a grudge at a passive monster, with no
// spelltype test — 25 of the 29 learnable benign match-4/6/8 spells
// carry it (curse, blind, slow, hold person, the songs) ---

use std::collections::BTreeMap;

use mud_core::content::{
    Element, MatchType, Message, MessageId, SaveClass, ScalePair, Spell, SpellId, TargetMode,
};

/// Benign match-8, carries 52 — the curse/blind/slow shape.
const CURSE: SpellId = SpellId(700);
/// Benign match-8, no 52 — the control.
const SOOTHE: SpellId = SpellId(701);
/// Offensive match-8, instant — pins the untouched 43417 twin.
const ZAP: SpellId = SpellId(702);
/// AffectsLiving(108) ahead of 52 — pins the shipped refusal-first order.
const BANE: SpellId = SpellId(703);
/// Benign match-12 area, carries 52 — the song-of-slumber band shape.
const DOOM: SpellId = SpellId(704);
/// Benign match-12 area, no 52 — the area control.
const BREEZE: SpellId = SpellId(705);

/// A protected room (attribute bit 1) one exit north of the square.
const CHAPEL: RoomId = RoomId { map: 1, room: 2 };

fn charge_spell(id: SpellId, name: &str, short: &str, mode: TargetMode) -> Spell {
    Spell {
        id,
        name: name.into(),
        short_name: short.into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(901)),
        // The shipped curse shape: the 52 marker row plus a payload row
        // (the payload drives the duration-slot entry, as on the real
        // curse/blind/slow templates).
        abilities: vec![(Ability::EvilInCombat, 0), (Ability::AC, -1)],
        level_cap: 0,
        round_cost: 100,
        required_power: 1,
        min_base: 0,
        max_base: 0,
        target_mode: mode,
        save_class: SaveClass::None,
        base_chance: 200, // auto-success: >= 200 skips the roll
        duration_per_level: 0,
        match_type: MatchType::Special8,
        duration: 60,
        element: Element::Magic,
        class_gate_group: 0,
        mana_cost: 4,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

fn caster_world() -> Content {
    let mut content = world();
    content.add_class(Class {
        id: ClassId(2),
        name: "Mage".into(),
        abilities: vec![],
        hp_per_level: 2,
        hp_seed: 4,
        caster_group: 1,
        casting_factor: 3,
        exp_base: 0,
        combat_factor: 2,
        weapon_code: 8,
        armour_code: 9,
    });
    content.add_message(Message {
        id: MessageId(901),
        lines: vec![
            "You cast %s on %s!".into(),
            "%s casts %s upon you!".into(),
            "%s casts %s on %s!".into(),
        ],
    });
    // A passive monster whose SpellImmu(139) outranks every fixture
    // spell (required_power 1 < 5 → "no effect").
    content.add_monster(Monster {
        id: MonsterId(3),
        name: "warded acolyte".into(),
        hitpoints: 50,
        energy: 1000,
        behaviour: 0,
        abilities: vec![(Ability::SpellImmu, 5)],
        ..Default::default()
    });
    // A passive NonLiving body for the AffectsLiving refusal.
    content.add_monster(Monster {
        id: MonsterId(4),
        name: "stone golem".into(),
        hitpoints: 50,
        energy: 1000,
        behaviour: 0,
        abilities: vec![(Ability::NonLiving, 1)],
        ..Default::default()
    });
    content.add_room(Room {
        id: CHAPEL,
        name: "Chapel".into(),
        attributes: 1, // protected
        ..Default::default()
    });
    let mut soothe = charge_spell(SOOTHE, "soothe", "soot", TargetMode::Benign);
    soothe.abilities = vec![(Ability::AC, -1)];
    let mut zap = charge_spell(ZAP, "zap", "zapp", TargetMode::Offensive0);
    zap.duration = 0;
    let mut bane = charge_spell(BANE, "bane", "bane", TargetMode::Benign);
    bane.abilities = vec![(Ability::AffectsLiving, 1), (Ability::EvilInCombat, 0)];
    let mut doom = charge_spell(DOOM, "doom", "doom", TargetMode::Benign);
    doom.match_type = MatchType::AreaC;
    let mut breeze = charge_spell(BREEZE, "breeze", "bree", TargetMode::Benign);
    breeze.match_type = MatchType::AreaC;
    breeze.abilities = vec![(Ability::AC, -1)];
    for s in [
        charge_spell(CURSE, "curse", "curs", TargetMode::Benign),
        soothe,
        zap,
        bane,
        doom,
        breeze,
    ] {
        content.add_spell(s);
    }
    content
}

fn hexer(name: &str, warn: bool) -> Player {
    let book: BTreeMap<SpellId, bool> = [CURSE, SOOTHE, ZAP, BANE, DOOM, BREEZE]
        .into_iter()
        .map(|s| (s, false))
        .collect();
    let mut p = citizen(name, warn);
    p.class = ClassId(2);
    p.current_mana = 100;
    p.spellbook = book;
    p
}

#[test]
fn benign_52_cast_charges_ten_evil_and_earns_a_grudge() {
    // 43330: ability == 0x34 + behaviour ∈ {0,4} + no name link → the
    // same add_evil_points(-1, 10) as melee, then the 43335-43346 grudge
    // (mon[0x50] = 1, name copy, suppression clear). The cast then
    // CONTINUES into costs and effects.
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Warlock", false));
    let m = core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast curse crier");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("A dark cloud passes over you"), "{out:?}");
    assert_eq!(core.player_fame(s), 10);
    assert_eq!(core.monster_target(m), Some(s), "grudge set (43343 name copy)");
    let slots = core.monster_active_spells(m).unwrap();
    assert!(
        slots.iter().any(|a| a.spell == Some(CURSE)),
        "the cast still resolves after the charge: {slots:?}"
    );
}

#[test]
fn benign_52_cast_refused_on_warnings_aborts_uncharged() {
    // 43331-43333: a non-zero add_evil_points return aborts the whole
    // verb before any cost, effect, or grudge.
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Cautious", true));
    let m = core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast curse crier");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("To do this action, you must turn off your evil warnings."),
        "{out:?}"
    );
    assert_eq!(core.player_fame(s), 0);
    assert_eq!(core.monster_target(m), None, "no grudge on refusal");
    let slots = core.monster_active_spells(m).unwrap();
    assert!(slots.iter().all(|a| a.spell.is_none()), "no effect landed: {slots:?}");
}

#[test]
fn benign_spell_without_52_stays_free() {
    // The gate keys on the SPELL'S ability, not on benignity — soothe
    // has no 52 and charges nothing.
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Healer", false));
    let m = core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast soothe crier");
    let _ = core.drain_events();
    assert_eq!(core.player_fame(s), 0);
    assert_eq!(core.monster_target(m), None);
    let slots = core.monster_active_spells(m).unwrap();
    assert!(slots.iter().any(|a| a.spell == Some(SOOTHE)), "{slots:?}");
}

#[test]
fn offensive_cast_at_passive_monster_still_charges() {
    // The 43417 twin (manual attempt loop, spelltype-gated) is untouched
    // by the 52 arm — an offensive instant at a passive body charges.
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Raider", false));
    core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast zap crier");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("A dark cloud passes over you"), "{out:?}");
    assert_eq!(core.player_fame(s), 10);
}

#[test]
fn charge_and_grudge_precede_the_spellimmu_refusal() {
    // DLL order: the ability scan (43297-43376, incl. the 0x34 charge)
    // runs BEFORE the SpellImmu gate (43380-43386) — a warded passive
    // body costs you 10 evil and hates you, and THEN the spell fizzles.
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Warlock", false));
    let m = core.spawn_monster(MonsterId(3), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast curse acolyte");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("A dark cloud passes over you"), "{out:?}");
    assert!(out.contains("Your spell has no effect on"), "{out:?}");
    assert_eq!(core.player_fame(s), 10);
    assert_eq!(core.monster_target(m), Some(s), "grudge survives the fizzle");
    let slots = core.monster_active_spells(m).unwrap();
    assert!(slots.iter().all(|a| a.spell.is_none()), "no effect landed: {slots:?}");
}

#[test]
fn benign_area_52_charges_ten_evil_without_a_grudge() {
    // cast_no_target's per-slot 0x34 arm (crime.md §2.5 last row;
    // decompile 39299-39313 → add_evil_warnings_to_room 38774-38777):
    // ONE 10-point NPC-style charge when an innocent passive monster is
    // a valid target. NO grudge writes — 38759-38812 contains none; the
    // per-victim pair timers are the PvP half (M8).
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Chanter", false));
    let m = core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast doom");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("A dark cloud passes over you"), "{out:?}");
    assert_eq!(core.player_fame(s), 10);
    assert_eq!(core.monster_target(m), None, "the area path takes no grudge");
    let slots = core.monster_active_spells(m).unwrap();
    assert!(
        slots.iter().any(|a| a.spell == Some(DOOM)),
        "the sweep still resolves after the charge: {slots:?}"
    );
}

#[test]
fn benign_area_52_refused_on_warnings_aborts_the_whole_cast() {
    // A non-zero add_evil_warnings_to_room return aborts cast_no_target
    // entirely (39310-39313: `return 0`) — costs unpaid, nothing lands.
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Cautious", true));
    let m = core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast doom");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("To do this action, you must turn off your evil warnings."),
        "{out:?}"
    );
    assert_eq!(core.player_fame(s), 0);
    assert!(out.contains("MA=100"), "mana unpaid on the abort: {out:?}");
    let slots = core.monster_active_spells(m).unwrap();
    assert!(slots.iter().all(|a| a.spell.is_none()), "nothing landed: {slots:?}");
}

#[test]
fn benign_area_without_52_stays_free() {
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Breather", false));
    let m = core.spawn_monster(MonsterId(1), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast breeze");
    let _ = core.drain_events();
    assert_eq!(core.player_fame(s), 0);
    let slots = core.monster_active_spells(m).unwrap();
    assert!(slots.iter().any(|a| a.spell == Some(BREEZE)), "{slots:?}");
}

#[test]
fn benign_area_52_in_a_protected_room_takes_the_guilt_refusal() {
    // The cast_no_target room-protection gate keys on `spelltype < 3 ||
    // spell_has_ability(0x34)` (39164-39171) — a BENIGN 52-carrier is
    // guilt-refused in a protected room, before the charge scan, so the
    // fame stays untouched.
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let mut p = hexer("Pilgrim", false);
    p.location = CHAPEL;
    let s = core.attach_player(p);
    let m = core.spawn_monster(MonsterId(1), CHAPEL).unwrap();
    core.drain_events();
    core.input(s, "cast doom");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You are overcome with a feeling of guilt"),
        "{out:?}"
    );
    assert_eq!(core.player_fame(s), 0, "the gate precedes the charge");
    let slots = core.monster_active_spells(m).unwrap();
    assert!(slots.iter().all(|a| a.spell.is_none()), "nothing landed: {slots:?}");
}

#[test]
fn refusal_ability_before_52_refuses_free() {
    // Shipped order: eligibility refusals run before the 52 charge. The
    // DLL walks ability slots in ORDER, so a spell listing a refusal
    // ability ahead of 52 refuses free there too — this pins the one
    // order we ship (see the divergence note at cast_eligibility_refused
    // for the single learnable exception, 35 poison bolt).
    let mut core = Core::new(caster_world(), CoreConfig::default());
    let s = core.attach_player(hexer("Warlock", false));
    let m = core.spawn_monster(MonsterId(4), SQUARE).unwrap();
    core.drain_events();
    core.input(s, "cast bane golem");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Your spell has no effect on"), "{out:?}");
    assert_eq!(core.player_fame(s), 0, "refused free");
    assert_eq!(core.monster_target(m), None);
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
