//! Martial-arts unarmed damage (`move_player_to_fighter` 24520-24571 +
//! `cmd_attack` 49700-49725; ORACLE oracle_m6_arena_fight.raw).
//!
//! A bare `attack` with no weapon auto-selects mode-1 "fists of fury"
//! when the attacker has the Punch ability (0x1d — the Mystic class
//! carries Punch/Kick/JumpKick at value 1): min = L*V/8 + 2, max =
//! (L+3)*V/4 + 6 (L = level capped at 20, V = folded Punch value), plus
//! the PunchDmg (92) flat add and the usual Strength adjustments.
//! Classes without Punch keep the plain 1-4 fists. Live pin: Nekojin
//! Mystic L1 V1 Str40 punched raw 2..6 -> shown 1..5 through the giant
//! rat's DR 1 (observed exactly {1,1,1,4,5}).

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player};

const DOJO: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: DOJO,
        name: "Dojo".into(),
        ..Default::default()
    });
    content.add_monster(Monster {
        id: MonsterId(1),
        name: "practice dummy".into(),
        hitpoints: 100_000,
        energy: 0, // never swings back
        magic_resist: 0,
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
    // A mystic-shaped class: Punch/Kick/JumpKick at 1 (class 15's shape).
    content.add_class(Class {
        id: ClassId(1),
        name: "Mystic".into(),
        abilities: vec![
            (Ability::from_id(0x1d).unwrap(), 1),
            (Ability::from_id(0x1e).unwrap(), 1),
            (Ability::from_id(0x23).unwrap(), 1),
        ],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 5,
        casting_factor: 3,
        exp_base: 0,
        combat_factor: 5,
        weapon_code: 8,
        armour_code: 9,
    });
    // A plain warrior: no martial arts.
    content.add_class(Class {
        id: ClassId(2),
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

fn fighter(name: &str, class: u16, level: u16) -> Player {
    let stats = StatBlock {
        intellect: 40,
        wisdom: 30,
        strength: 50, // Str 50: no damage adjustment — the raw formula shows
        health: 30,
        agility: 60,
        charm: 50,
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(class),
        level,
        stats,
        base_stats: stats,
        current_hp: 400,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: DOJO,
        ..Default::default()
    }
}

/// Collect the shown damage numbers from N combat rounds of punching.
fn damage_census(class: u16, level: u16, rounds: u32) -> Vec<i32> {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(fighter("Student", class, level));
    core.spawn_monster(MonsterId(1), DOJO).unwrap();
    core.drain_events();
    core.input(s, "attack dummy");
    let mut out = Vec::new();
    for _ in 0..rounds * 5 {
        core.tick();
        for e in core.drain_events() {
            if let Event::Output { session, text } = e
                && session == s
                && !text.contains("critically") // crits scale past the band
                && let Some(rest) = text.split(" for ").nth(1)
                && let Some(n) = rest.split(' ').next().and_then(|n| n.parse::<i32>().ok())
            {
                out.push(n);
            }
        }
    }
    out
}

#[test]
fn mystic_bare_attack_uses_the_punch_formula() {
    // L1 V1 Str50: min = 1*1/8 + 2 = 2, max = (1+3)*1/4 + 6 = 7. The
    // dummy has DR 0, so shown damage = raw.
    let hits = damage_census(1, 1, 30);
    assert!(!hits.is_empty(), "some punches landed");
    assert!(
        hits.iter().all(|d| (2..=7).contains(d)),
        "mode-1 band 2..=7: {hits:?}"
    );
    assert!(
        hits.iter().any(|d| *d > 4),
        "punches exceed the plain-fists cap: {hits:?}"
    );
}

#[test]
fn punch_scales_with_level() {
    // L10 V1: min = 10/8 + 2 = 3, max = 13/4 + 6 = 9.
    let hits = damage_census(1, 10, 30);
    assert!(!hits.is_empty());
    assert!(
        hits.iter().all(|d| (3..=9).contains(d)),
        "L10 band 3..=9: {hits:?}"
    );
}

#[test]
fn plain_class_keeps_the_1_to_4_fists() {
    let hits = damage_census(2, 1, 30);
    assert!(!hits.is_empty());
    assert!(
        hits.iter().all(|d| (1..=4).contains(d)),
        "plain fists 1..=4: {hits:?}"
    );
}
