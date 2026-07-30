//! M7 slice 6: the 30-slot player innate/quest ability table
//! (quests.md §1.1 — ids at `+0x73a[30]`, values at `+0x776[30]`).
//!
//! Writer semantics from the decompile:
//! - `give` = `FUN_0046c507` (65893-65928): refuses id 0xa0; adds the
//!   value into EVERY slot already holding the id; otherwise claims the
//!   first empty slot; full table with no match → failure.
//! - `raise` = the `addability` arm (69590-69637): raises every matching
//!   slot below the value (skipping the raise for 0xa0), otherwise claims
//!   the first empty slot; full table with no match → failure.
//! - `remove` = the `removeability` arm (69523-69558): zeroes every
//!   matching slot; id absent → failure.
//! - Reads sum matching slots (`get_user_ability_value` 36850-36869).

use mud_core::ability::Ability;
use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Gender, Player};

const ACCURACY: u16 = 22; // 0x16
const DARK_DRUID: u16 = 129; // 0x81 DarkDruidQuest
const TEMP_SPELL: u16 = 160; // 0xa0 GiveTempSpell — the refused id

fn ab(id: u16) -> Ability {
    Ability::from_id(id).unwrap()
}

#[test]
fn innate_value_sums_matching_slots() {
    // get_user_ability_value 36850-36869: every slot holding the id
    // contributes; sum semantics for ordinary abilities.
    let mut p = Player::default();
    p.innate[0] = (Some(ab(DARK_DRUID)), 2);
    p.innate[7] = (Some(ab(DARK_DRUID)), 3);
    p.innate[3] = (Some(ab(ACCURACY)), 9);
    assert_eq!(p.innate_value(ab(DARK_DRUID)), 5);
    assert_eq!(p.innate_value(ab(ACCURACY)), 9);
    assert_eq!(p.innate_value(ab(2)), 0);
}

#[test]
fn give_accumulates_into_every_matching_slot() {
    // FUN_0046c507 65908-65916: `+=` into each slot holding the id.
    let mut p = Player::default();
    p.innate[2] = (Some(ab(DARK_DRUID)), 1);
    p.innate[5] = (Some(ab(DARK_DRUID)), 4);
    assert!(p.give_innate_ability(ab(DARK_DRUID), 2));
    assert_eq!(p.innate[2], (Some(ab(DARK_DRUID)), 3));
    assert_eq!(p.innate[5], (Some(ab(DARK_DRUID)), 6));
}

#[test]
fn give_claims_first_empty_slot_when_absent() {
    // FUN_0046c507 65917-65925.
    let mut p = Player::default();
    p.innate[0] = (Some(ab(ACCURACY)), 1);
    assert!(p.give_innate_ability(ab(DARK_DRUID), 2));
    assert_eq!(p.innate[1], (Some(ab(DARK_DRUID)), 2));
}

#[test]
fn give_refuses_the_temp_spell_id() {
    // FUN_0046c507 65904-65906: id 0xa0 returns 0 without touching the
    // table (the spell grant belongs to `addability`, not `giveability`).
    let mut p = Player::default();
    assert!(!p.give_innate_ability(ab(TEMP_SPELL), 5));
    assert_eq!(p.innate, [(None, 0); 30]);
}

#[test]
fn give_on_a_full_table_is_a_silent_drop() {
    // FUN_0046c507: no matching slot and no empty slot → returns 0.
    let mut p = Player::default();
    for i in 0..30 {
        p.innate[i] = (Some(ab(ACCURACY)), 1);
    }
    assert!(!p.give_innate_ability(ab(DARK_DRUID), 2));
    assert_eq!(p.innate_value(ab(DARK_DRUID)), 0);
}

#[test]
fn raise_lifts_only_slots_below_the_value() {
    // addability arm 69604-69613: every matching slot below the value is
    // raised to it; slots at or above keep their value (at-least, not add).
    let mut p = Player::default();
    p.innate[1] = (Some(ab(DARK_DRUID)), 1);
    p.innate[4] = (Some(ab(DARK_DRUID)), 7);
    assert!(p.raise_innate_ability(ab(DARK_DRUID), 3));
    assert_eq!(p.innate[1], (Some(ab(DARK_DRUID)), 3));
    assert_eq!(p.innate[4], (Some(ab(DARK_DRUID)), 7));
}

#[test]
fn raise_claims_first_empty_slot_when_absent() {
    // addability arm 69614-69627.
    let mut p = Player::default();
    p.innate[0] = (Some(ab(ACCURACY)), 1);
    assert!(p.raise_innate_ability(ab(DARK_DRUID), 2));
    assert_eq!(p.innate[1], (Some(ab(DARK_DRUID)), 2));
}

#[test]
fn raise_on_a_full_table_fails() {
    // addability arm 69628-69634: no slot found → the verb fail-stops;
    // the helper reports failure.
    let mut p = Player::default();
    for i in 0..30 {
        p.innate[i] = (Some(ab(ACCURACY)), 1);
    }
    assert!(!p.raise_innate_ability(ab(DARK_DRUID), 2));
}

#[test]
fn remove_zeroes_every_matching_slot() {
    // removeability arm 69531-69548; absent id reports failure (69549-69556).
    let mut p = Player::default();
    p.innate[0] = (Some(ab(DARK_DRUID)), 2);
    p.innate[9] = (Some(ab(DARK_DRUID)), 5);
    p.innate[3] = (Some(ab(ACCURACY)), 9);
    assert!(p.remove_innate_ability(ab(DARK_DRUID)));
    assert_eq!(p.innate[0], (None, 0));
    assert_eq!(p.innate[9], (None, 0));
    assert_eq!(p.innate[3], (Some(ab(ACCURACY)), 9));
    assert!(!p.remove_innate_ability(ab(DARK_DRUID)));
}

// --- the innate table feeds the ability bag (get_user_ability_value
// folds `+0x73a` alongside spells/race/class/items, 36850-36869) ---

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Test Room".into(),
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
        current_hp: 400,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: RoomId { map: 1, room: 1 },
        ..Default::default()
    }
}

#[test]
fn innate_accuracy_feeds_the_derived_fighter() {
    let config = CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config.clone());
    let naked = core.attach_player(player("Naked"));
    let base = core.combat_debug(naked).0.accuracy;

    let mut gifted = player("Gifted");
    assert!(gifted.raise_innate_ability(ab(ACCURACY), 5));
    let mut core = Core::new(world(), config);
    let s = core.attach_player(gifted);
    assert_eq!(
        core.combat_debug(s).0.accuracy,
        base + 5,
        "an innate Accuracy(22) slot must reach the dynamic accumulator"
    );
}
