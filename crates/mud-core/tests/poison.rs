//! Tests for the poison counter (`+0xbe`, `re/docs/regeneration.md` §4):
//! the slow-tick damage ("You feel ill.", decompile 19518-19533), the
//! Poison(19) set-if-greater hard write at cast application (cast_no_target
//! case 0x13, 40520-40546, ImmuPoison-gated), the CurePoison(20) instant
//! subtract (40669-40673) and recurring upkeep subtract (44778-44785), the
//! termination reversal (`poison -= stored`, floor 0; 44853-44857), and the
//! death-path clear (check_kill_user 13066).

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Race, RaceId, Room, RoomId, Spell, SpellId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const MAGE: ClassId = ClassId(1);
const WARRIOR: ClassId = ClassId(2);
const HUMAN: RaceId = RaceId(1);
/// Carries (ImmuPoison, 1): ability 21 blocks the Poison(19) apply wholesale
/// (decompile 40520: `user_has_ability(0x15)` gates the entire case).
const IRONGUT: RaceId = RaceId(3);

/// Benign instant [(Poison, 6)] — the fixed-value hard-write probe.
const STING: SpellId = SpellId(500);
/// Benign instant [(Poison, 4)] — the set-if-greater contrast.
const PRICK: SpellId = SpellId(510);
/// Benign duration 5 flat, fixed [(Poison, 5)] — hard write at slot entry,
/// reversal (`poison -= 5`, floor 0) at termination.
const VENOMOUS: SpellId = SpellId(520);
/// Benign instant [(CurePoison, 3)].
const ANTIDOTE: SpellId = SpellId(530);
/// Benign duration 5 flat, recurring [(CurePoison, 2)] — the upkeep
/// subtract; CurePoison has NO termination arm (spec §5).
const SALVE: SpellId = SpellId(540);

const TOWER: RoomId = RoomId { map: 1, room: 1 };

fn spell(id: SpellId, name: &str, short: &str) -> Spell {
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, TargetMode};
    Spell {
        id,
        name: name.into(),
        short_name: short.into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![],
        level_cap: 0,
        round_cost: 0,
        required_power: 1,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200,
        duration_per_level: 0,
        match_type: MatchType::Single0,
        duration: 0,
        element: Element::Cold,
        class_gate_group: 1,
        mana_cost: 0,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: TOWER,
        name: "Tower".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_race(Race {
        id: HUMAN,
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_race(Race {
        id: IRONGUT,
        name: "Irongut".into(),
        abilities: vec![(Ability::ImmuPoison, 1)],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: MAGE,
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
    content.add_class(Class {
        id: WARRIOR,
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

    let mut sting = spell(STING, "sting", "stin");
    sting.abilities = vec![(Ability::Poison, 6)];
    let mut prick = spell(PRICK, "prick", "pric");
    prick.abilities = vec![(Ability::Poison, 4)];
    let mut venomous = spell(VENOMOUS, "venomous grip", "veno");
    venomous.duration = 5; // flat: no per-level roll
    venomous.abilities = vec![(Ability::Poison, 5)];
    let mut antidote = spell(ANTIDOTE, "antidote", "anti");
    antidote.abilities = vec![(Ability::CurePoison, 3)];
    let mut salve = spell(SALVE, "soothing salve", "salv");
    salve.duration = 5;
    salve.abilities = vec![(Ability::CurePoison, 2)];
    for s in [sting, prick, venomous, antidote, salve] {
        content.add_spell(s);
    }
    content
}

fn book() -> BTreeMap<SpellId, bool> {
    let mut book = BTreeMap::new();
    for id in [STING, PRICK, VENOMOUS, ANTIDOTE, SALVE] {
        book.insert(id, false);
    }
    book
}

fn player(name: &str, race: RaceId, class: ClassId) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race,
        class,
        level: 1,
        stats: StatBlock { health: 50, ..StatBlock::default() },
        base_stats: StatBlock { health: 50, ..StatBlock::default() },
        hp_base: 0,
        current_hp: 20,
        current_mana: 6,
        hunger: 1000,
        thirst: 1000,
        coins: Default::default(),
        lawful: false,
        inventory: vec![],
        weapon: None,
        bankbooks: vec![],
        worn: vec![],
        cp_unspent: 0,
        cp_lifetime: 0,
        lives: 9,
        experience: 0,
        location: TOWER,
        spellbook: book(),
        poison: 0,
        active_spells: Default::default(),
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

fn ticks(core: &mut Core, n: u64) {
    for _ in 0..n {
        core.tick();
    }
}

fn cast(core: &mut Core, s: SessionId, line: &str) -> String {
    core.input(s, line);
    text_to(&core.drain_events(), s)
}

/// A fresh combat round so the one-cast-per-round flag clears.
fn energy_round(core: &mut Core) {
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
}

// --- the slow-tick poison damage (regeneration.md §4; 19518-19533) ---

#[test]
fn poison_damages_on_the_slow_tick_with_the_feel_ill_line() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Dain", HUMAN, WARRIOR));
    core.drain_events();
    core.set_poison(s, 3);
    // L1 health-50 warrior: max 25+4=29... whatever the derived max is,
    // set below it so regen interplay is observable.
    let max = core.max_hp(s);
    core.set_current_hp(s, max);
    ticks(&mut core, 29);
    let shown = text_to(&core.drain_events(), s);
    assert!(!shown.contains("You feel ill."), "no tick before 30: {shown:?}");
    assert_eq!(core.current_hp(s), max);
    ticks(&mut core, 1);
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You feel ill.\n"), "got: {shown:?}");
    // Poison hits first, then HP regen fires in the SAME tick (the DLL's
    // else-if chain reads the post-poison HP; regeneration.md §4): the
    // L1 health-50 fixture regens 1.
    assert_eq!(core.current_hp(s), max - 3 + 1);
    assert_eq!(core.poison(s), 3, "the counter does not decay on its own");
}

#[test]
fn poison_crossing_below_zero_drops_and_then_bleeds() {
    // Drop announce on old > 0 && new < 0 (19526-19528: FUN_0043c91d),
    // then the near-death branch bleeds the downed player one MORE in the
    // same tick (the DLL's separate if-chain).
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Dain", HUMAN, WARRIOR));
    let watcher = core.attach_player(player("Grunt", HUMAN, WARRIOR));
    core.drain_events();
    core.set_poison(s, 6);
    core.set_current_hp(s, 5);
    ticks(&mut core, 30);
    let events = core.drain_events();
    let shown = text_to(&events, s);
    assert!(shown.contains("You feel ill.\n"), "got: {shown:?}");
    assert!(shown.contains("Dain drops to the ground!\n"), "got: {shown:?}");
    let seen = text_to(&events, watcher);
    assert!(seen.contains("Dain drops to the ground!\n"), "room sees: {seen:?}");
    assert_eq!(core.current_hp(s), -2, "poison -6, then the bleed -1");
}

#[test]
fn poison_kills_through_the_death_path_and_clears() {
    // check_kill_user after the poison hit (19529-19532); the death path
    // zeroes the counter after terminating the slots (13066).
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Dain", HUMAN, WARRIOR));
    core.drain_events();
    core.set_poison(s, 250);
    core.set_current_hp(s, 10);
    ticks(&mut core, 30);
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You have been killed!"), "died: {shown:?}");
    let p = core.player_snapshot(s);
    assert_eq!(p.lives, 8, "miracle respawn");
    assert_eq!(p.poison, 0, "death clears the counter");
}

// --- Poison (19) at cast application: set-if-greater (40520-40546) ---

#[test]
fn instant_poison_apply_is_set_if_greater() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", HUMAN, MAGE));
    core.drain_events();
    cast(&mut core, s, "c sting");
    assert_eq!(core.poison(s), 6, "hard write");
    energy_round(&mut core);
    cast(&mut core, s, "c prick");
    assert_eq!(core.poison(s), 6, "a smaller poison does not lower the counter");
    core.set_poison(s, 1);
    energy_round(&mut core);
    cast(&mut core, s, "c prick");
    assert_eq!(core.poison(s), 4, "a larger poison raises it");
}

#[test]
fn immu_poison_blocks_the_apply() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Gutsy", IRONGUT, MAGE));
    core.drain_events();
    cast(&mut core, s, "c sting");
    assert_eq!(core.poison(s), 0, "ImmuPoison (21) gates the whole case");
}

#[test]
fn duration_poison_hard_writes_at_entry_and_reverses_at_termination() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", HUMAN, MAGE));
    core.drain_events();
    cast(&mut core, s, "c veno");
    assert_eq!(core.poison(s), 5, "hard write rides the slot entry");
    let p = core.player_snapshot(s);
    assert!(p.find_active(VENOMOUS).is_some(), "slot entered");
    // Doubly poisoned meanwhile (a bigger later hit): termination
    // subtracts only ITS stored 5.
    core.set_poison(s, 9);
    // Flat duration 5 -> expiry on the 5th upkeep tick (15 s).
    ticks(&mut core, 15);
    let p = core.player_snapshot(s);
    assert!(p.active_spells.iter().all(|a| a.spell.is_none()), "expired");
    assert_eq!(core.poison(s), 4, "9 - stored 5");
}

#[test]
fn duration_poison_termination_floors_at_zero() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", HUMAN, MAGE));
    core.drain_events();
    cast(&mut core, s, "c veno");
    assert_eq!(core.poison(s), 5);
    core.set_poison(s, 2); // partially cured meanwhile
    ticks(&mut core, 15);
    assert_eq!(core.poison(s), 0, "2 - 5 floors at 0");
}

// --- CurePoison (20): instant + recurring (40669-40673, 44778-44785) ---

#[test]
fn instant_cure_poison_subtracts_and_floors_at_zero() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", HUMAN, MAGE));
    core.drain_events();
    core.set_poison(s, 5);
    cast(&mut core, s, "c anti");
    assert_eq!(core.poison(s), 2, "poison -= 3");
    energy_round(&mut core);
    cast(&mut core, s, "c anti");
    assert_eq!(core.poison(s), 0, "2 - 3 floors at 0");
}

#[test]
fn recurring_cure_poison_subtracts_each_upkeep_tick() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", HUMAN, MAGE));
    core.drain_events();
    core.set_poison(s, 5);
    cast(&mut core, s, "c salv");
    assert_eq!(core.poison(s), 5, "no hard write at entry for a cure");
    ticks(&mut core, 3);
    assert_eq!(core.poison(s), 3, "first upkeep tick: -= 2");
    ticks(&mut core, 3);
    assert_eq!(core.poison(s), 1);
    ticks(&mut core, 3);
    assert_eq!(core.poison(s), 0, "floors at 0");
    // Expiry (tick 5) has no CurePoison termination arm: still 0.
    ticks(&mut core, 6);
    let p = core.player_snapshot(s);
    assert!(p.active_spells.iter().all(|a| a.spell.is_none()), "expired");
    assert_eq!(core.poison(s), 0);
}

// --- persistence ---

#[test]
fn poison_survives_a_persist_snapshot() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Dain", HUMAN, WARRIOR));
    core.drain_events();
    core.set_poison(s, 7);
    core.detach(s);
    let persisted = core
        .drain_events()
        .into_iter()
        .find_map(|e| match e {
            Event::Persist(p) => Some(*p),
            _ => None,
        })
        .expect("quit persists");
    assert_eq!(persisted.poison, 7);
}
