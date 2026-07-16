//! Tests for `calculate_secondary_stats` (`re/docs/leveling.md` §5).
//!
//! The Dwarf Warrior L1 values are oracle-verified (MBBSEmu transcript,
//! `re/docs/character_creation.md` §6.5): Hits 35/35, MagicRes 55,
//! Perception 35, Stealth/Thievery/Traps/Picklocks/Tracking 0.

use mud_core::ability::Ability;
use mud_core::content::StatBlock;
use mud_core::stats::{derive, AbilityBag, StatInputs};

fn dwarf_warrior_l1() -> StatInputs {
    // Dwarf: Int 30, Wis 50, Str 50, Hea 50, Agl 30, Chm 30; MR ability +10.
    // Warrior: hp_per_level 6, hp_seed 4, caster group 0.
    let stats = StatBlock {
        intellect: 30,
        wisdom: 50,
        strength: 50,
        health: 50,
        agility: 30,
        charm: 30,
    };
    let mut abilities = AbilityBag::default();
    abilities.add(Ability::from_id(36).unwrap(), 10); // Dwarf M.R. +10
    StatInputs {
        level: 1,
        stats,
        health_base: 50,
        hp_base: 4,
        class_hp_per_level: 6,
        race_hp_per_level: 0,
        caster_group: 0,
        casting_factor: 0,
        abilities,
    }
}

#[test]
fn oracle_dwarf_warrior_level_one() {
    let d = derive(&dwarf_warrior_l1());
    assert_eq!(d.max_hp, 35);
    assert_eq!(d.max_mana, 0); // non-caster group
    assert_eq!(d.magic_resist, 55);
    assert_eq!(d.perception, 35);
    assert_eq!(d.stealth, 0);
    assert_eq!(d.thievery, 0);
    assert_eq!(d.find_traps, 0);
    assert_eq!(d.picklocks, 0);
    assert_eq!(d.tracking, 0);
}

#[test]
fn max_hp_health_term_uses_the_base_copy_not_the_buffed_stat() {
    // Engine quirk (leveling.md §0): the /2 term reads the +0x9c base copy,
    // so a Health buff on the effective stat must NOT raise max HP.
    let mut inputs = dwarf_warrior_l1();
    inputs.stats.health = 90; // buffed effective Health
    let d = derive(&inputs);
    assert_eq!(d.max_hp, 35, "buffed Health must not change max HP");
}

#[test]
fn max_hp_grows_with_level_and_health() {
    // L5, Health base 66: 66/2 + 5*6 + (66-50)*5/16 + 4 = 33+30+5+4 = 72.
    let mut inputs = dwarf_warrior_l1();
    inputs.level = 5;
    inputs.health_base = 66;
    inputs.stats.health = 66;
    let d = derive(&inputs);
    assert_eq!(d.max_hp, 72);
}

#[test]
fn caster_mana_and_spellcasting() {
    // Human Mage L1 (caster group 1, casting factor 3):
    // mana = 0 + 3*1*2 + 0 + 6 = 12
    // SC   = 1*2 + (40*3+40)/6 + 3*5 + 3 = 2 + 26 + 15 + 3 = 46
    let inputs = StatInputs {
        level: 1,
        stats: StatBlock {
            intellect: 40,
            wisdom: 40,
            strength: 40,
            health: 40,
            agility: 40,
            charm: 40,
        },
        health_base: 40,
        hp_base: 3,
        class_hp_per_level: 3,
        race_hp_per_level: 0,
        caster_group: 1,
        casting_factor: 3,
        abilities: AbilityBag::default(),
    };
    let d = derive(&inputs);
    assert_eq!(d.max_mana, 12);
    assert_eq!(d.spellcasting, 46);
}

#[test]
fn kai_mana_is_level_minus_one() {
    let mut inputs = dwarf_warrior_l1();
    inputs.caster_group = 5;
    inputs.level = 7;
    let d = derive(&inputs);
    assert_eq!(d.max_mana, 6);
}

#[test]
fn thief_skills_unlock_via_gate_abilities() {
    // Thievery (gate ability 0x27=39): (Agl + Int + Chm + g*24)/6 + value.
    // Dwarf stats, L1, gate value 0: (30 + 30 + 30 + 24)/6 = 19.
    let mut inputs = dwarf_warrior_l1();
    inputs.abilities.add(Ability::from_id(39).unwrap(), 0);
    let d = derive(&inputs);
    assert_eq!(d.thievery, 19);
    // A nonzero gate value is a flat bonus.
    inputs.abilities.add(Ability::from_id(39).unwrap(), 5);
    assert_eq!(derive(&inputs).thievery, 24);
}

#[test]
fn growth_term_halves_past_level_15() {
    // g(20) = 15 + (20-15)/2 = 17; thievery = (30+30+30+17*24)/6 = 498/6 = 83.
    let mut inputs = dwarf_warrior_l1();
    inputs.abilities.add(Ability::from_id(39).unwrap(), 0);
    inputs.level = 20;
    let d = derive(&inputs);
    assert_eq!(d.thievery, 83);
}

#[test]
fn dodge_base_clamps_to_at_least_one() {
    // Dwarf L1: 1/10 + (30-50)/10 + (30-50)/20 + (30-50)/30
    //         = 0 + -2 + -1 + 0 = -3 -> clamped to 1.
    let d = derive(&dwarf_warrior_l1());
    assert_eq!(d.dodge_base, 1);
}

#[test]
fn magic_resist_maxes_from_int_and_triple_wisdom() {
    // (Int + Wis*3)/4 + ability: (30 + 150)/4 + 10 = 45 + 10 = 55 (oracle).
    // Without the racial ability: 45.
    let mut inputs = dwarf_warrior_l1();
    inputs.abilities = AbilityBag::default();
    let d = derive(&inputs);
    assert_eq!(d.magic_resist, 45);
}

#[test]
fn carry_capacity_is_strength_scaled() {
    let d = derive(&dwarf_warrior_l1());
    assert_eq!(d.carry_capacity, 50 * 48);
    // Above 100 Strength the growth steepens: Str*48 + Str*36 - 3600.
    let mut inputs = dwarf_warrior_l1();
    inputs.stats.strength = 110;
    let d = derive(&inputs);
    assert_eq!(d.carry_capacity, 110 * 48 + 110 * 36 - 3600);
}
