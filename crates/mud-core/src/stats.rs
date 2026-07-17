//! `calculate_secondary_stats` — the derived-stat formula set
//! (`re/docs/leveling.md` §5). Oracle-anchored: the Dwarf Warrior L1 values
//! (HP 35, MR 55, Perception 35) reproduce the MBBSEmu transcript.
//!
//! All arithmetic is C-style i32 (Rust `/` truncates toward zero, matching
//! the original). Callers assemble `StatInputs` from the player, class, and
//! race records plus an [`AbilityBag`] of accumulated `(ability, value)`
//! modifiers (race + class permanents now; gear and spells join in later
//! milestones via the same bag — that is `update_dynamic_stats`' job).

use std::collections::BTreeMap;

use crate::ability::Ability;
use crate::content::StatBlock;

/// Accumulated ability modifiers: the sum of every active `(ability, value)`
/// pair affecting the player.
#[derive(Debug, Clone, Default)]
pub struct AbilityBag {
    values: BTreeMap<Ability, i32>,
}

impl AbilityBag {
    pub fn add(&mut self, ability: Ability, value: i32) {
        *self.values.entry(ability).or_insert(0) += value;
    }

    pub fn value(&self, ability: Ability) -> i32 {
        self.values.get(&ability).copied().unwrap_or(0)
    }

    pub fn has(&self, ability: Ability) -> bool {
        self.values.contains_key(&ability)
    }
}

/// Ability ids used by the formulas (leveling.md §5, abilities.md).
mod abil {
    pub const MR: u16 = 0x24; // 36  M.R.
    pub const PERCEPTION: u16 = 0x4d; // 77  Percep
    pub const STEALTH: u16 = 0x1b; // 27  Stealth (modifier)
    pub const RACE_STEALTH: u16 = 0x66; // 102 gate
    pub const CLASS_STEALTH: u16 = 0x67; // 103 gate
    pub const THIEVERY: u16 = 0x27; // 39  gate + bonus
    pub const FIND_TRAPS: u16 = 0x28; // 40  gate
    pub const DISARM_TRAPS: u16 = 0x29; // 41  bonus
    pub const PICKLOCKS: u16 = 0x25; // 37  gate
    pub const TRACKING: u16 = 0x26; // 38  gate + bonus
    pub const DODGE: u16 = 0x22; // 34
    pub const JUMPKICK: u16 = 0x23; // 35
    pub const MAX_MANA: u16 = 0x45; // 69
    pub const SPELLCASTING: u16 = 0x46; // 70  S.C.
    pub const ALTER_HP: u16 = 0x58; // 88
}

fn a(id: u16) -> Ability {
    Ability::from_id(id).expect("formula ability ids are in the enum")
}

/// Inputs to one derivation pass.
#[derive(Debug, Clone)]
pub struct StatInputs {
    pub level: i32,
    /// Current effective stats (`+0xa2..+0xac`) — buffs included.
    pub stats: StatBlock,
    /// The unmodified Health base copy (`+0x9c`) — the max-HP `/2` term reads
    /// this, so Health buffs do not raise max HP (engine quirk).
    pub health_base: i32,
    /// `player+0x724`, seeded from the class HP field and grown by training.
    pub hp_base: i32,
    /// `class+0x20` — flat HP per level.
    pub class_hp_per_level: i32,
    /// `race+0x2c` — flat HP per level.
    pub race_hp_per_level: i32,
    /// `class+0x40` — 0 non-caster, 1-4 caster stat groups, 5 Kai.
    pub caster_group: i32,
    /// `class+0x42` — casting level factor.
    pub casting_factor: i32,
    pub abilities: AbilityBag,
}

/// One derivation result (`calculate_secondary_stats` output fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub max_hp: i32,
    pub max_mana: i32,
    pub spellcasting: i32,
    pub magic_resist: i32,
    pub perception: i32,
    pub stealth: i32,
    pub thievery: i32,
    pub find_traps: i32,
    pub disarm_traps: i32,
    pub picklocks: i32,
    pub tracking: i32,
    pub dodge_base: i32,
    pub dodge: i32,
    pub carry_capacity: i32,
}

/// The per-level exp ratio table for levels 1-26
/// (`records.md` exp curve; byte-identical in both builds).
const EXP_RATIOS: [(u64, u64); 26] = [
    (1, 1),
    (40, 20),
    (44, 24),
    (44, 24),
    (48, 28),
    (48, 28),
    (52, 32),
    (52, 32),
    (56, 36),
    (56, 36),
    (60, 40),
    (60, 40),
    (65, 45),
    (65, 45),
    (70, 50),
    (70, 50),
    (75, 55),
    (50, 40),
    (50, 40),
    (50, 40),
    (50, 40),
    (50, 40),
    (50, 40),
    (50, 40),
    (50, 40),
    (23, 20),
];

/// `new_calc_exp_needed(level, base)` — the exp required to train TO
/// `level + 1` (callers pass the current level), where
/// `base = class.exp_base + race.exp_chart` (decompile 0x73810).
///
/// High bands (WG3-NT live path): i in [26,53] → 115/100, [54,56] → 109/100,
/// ≥57 → 108/100. Computed exactly in u64 — the original's `/100` rescale
/// dance is an overflow guard, not curve math (`records.md`).
pub fn exp_needed(level: u16, base: u64) -> u64 {
    let mut e = 10 * (base + 100);
    for i in 0..u64::from(level) {
        let (mult, div) = match i {
            0..=25 => EXP_RATIOS[i as usize],
            26..=53 => (115, 100),
            54..=56 => (109, 100),
            _ => (108, 100),
        };
        e = e * mult / div;
    }
    e
}

/// The level growth term `g(L)`: linear to 15, half-rate after.
fn growth(level: i32) -> i32 {
    if level < 16 {
        level
    } else {
        15 + (level - 15) / 2
    }
}

pub fn derive(inputs: &StatInputs) -> Derived {
    let StatBlock {
        intellect,
        wisdom,
        strength,
        health,
        agility,
        charm,
    } = inputs.stats;
    let (int, wis, str_, _hea, agl, chm) = (
        i32::from(intellect),
        i32::from(wisdom),
        i32::from(strength),
        i32::from(health),
        i32::from(agility),
        i32::from(charm),
    );
    let level = inputs.level;
    let g = growth(level);
    let bag = &inputs.abilities;

    let max_hp = inputs.health_base / 2
        + level * (inputs.class_hp_per_level + inputs.race_hp_per_level)
        + (inputs.health_base - 50) * level / 16
        + inputs.hp_base
        + bag.value(a(abil::ALTER_HP));

    let max_mana = match inputs.caster_group {
        1..=4 => bag.value(a(abil::MAX_MANA)) + inputs.casting_factor * level * 2 + 6,
        5 => level - 1,
        _ => 0,
    };

    let sc_stat_term = match inputs.caster_group {
        1 => (int * 3 + wis) / 6,
        2 => (wis * 3 + int) / 6,
        3 => (int + wis) / 3,
        4 => (chm * 3 + wis) / 6,
        5 => 500,
        _ => -150,
    };
    let spellcasting = level * 2
        + sc_stat_term
        + inputs.casting_factor * 5
        + inputs.casting_factor
        + bag.value(a(abil::SPELLCASTING));

    let magic_resist = (int + wis * 3) / 4 + bag.value(a(abil::MR));

    let perception = (int * 5 + wis * 2 + chm) / 8 + bag.value(a(abil::PERCEPTION));

    let stealth = if bag.has(a(abil::RACE_STEALTH)) || bag.has(a(abil::CLASS_STEALTH)) {
        let level_term = if level < 16 { level * 2 } else { 30 + (level - 15) };
        (chm / 6 + agl / 4 + level_term + int / 8 + 0x14 + bag.value(a(abil::STEALTH))).max(0)
    } else {
        0
    };

    let thievery = if bag.has(a(abil::THIEVERY)) {
        ((agl + int + chm + g * 24) / 6 + bag.value(a(abil::THIEVERY))).max(0)
    } else {
        0
    };

    let (find_traps, disarm_traps) = if bag.has(a(abil::FIND_TRAPS)) {
        let base = (int + agl + chm * 2 + g * 28) / 7;
        (
            (base + bag.value(a(abil::FIND_TRAPS)) + bag.value(a(abil::DISARM_TRAPS))).max(0),
            base.max(0),
        )
    } else {
        (0, 0)
    };

    let picklocks = if bag.has(a(abil::PICKLOCKS)) {
        (((agl + int + g * 10) * 2) / 7 + bag.value(a(abil::PICKLOCKS))).max(0)
    } else {
        0
    };

    let tracking = if bag.has(a(abil::TRACKING)) {
        ((int * 2 + wis + chm + g * 40) / 8 + bag.value(a(abil::TRACKING))).max(0)
    } else {
        0
    };

    let dodge_base =
        (level / 10 + (int - 50) / 10 + (agl - 50) / 20 + (chm - 50) / 30).clamp(1, 75);

    let dodge = {
        // VERIFIED (spellcasting.md §8.11): the Dodge (34) ability adds
        // RAW — blur's stored 5 raised the sheet's Martial Arts row (which
        // displays this value) 13→18 exactly; only dodge_base is doubled
        // (the WG3-NT/DOS divergence).
        let d = 2 * dodge_base + chm / 10 + agl / 5 + level / 5 + bag.value(a(abil::DODGE));
        if bag.has(a(abil::JUMPKICK)) {
            (d + level) * 2
        } else {
            d
        }
    };

    let carry_capacity = if str_ > 100 {
        str_ * 48 + str_ * 36 - 3600
    } else {
        str_ * 48
    };

    Derived {
        max_hp,
        max_mana,
        spellcasting,
        magic_resist,
        perception,
        stealth,
        thievery,
        find_traps,
        disarm_traps,
        picklocks,
        tracking,
        dodge_base,
        dodge,
        carry_capacity,
    }
}
