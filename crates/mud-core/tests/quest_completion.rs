//! M7 slice 6: the quest completion detector (`FUN_00414d23`,
//! `re/wg_nt_ghidra/exports/FUN_00414d23.asm`; quests.md §4.3) and
//! load_player's class-skill strip (0x15084, decompile 10164-10201;
//! §4.4) — both run at attach, before stat derivation.

use mud_core::ability::Ability;
use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };

fn ab(id: u16) -> Ability {
    Ability::from_id(id).unwrap()
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: A,
        name: "Sanctum".into(),
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
    // Classes spanning the §4.3 alignment-package groups: 1 Warrior
    // (MaxDamage), 5 Priest (S.C./mana), 7 Thief (backstab/stealth),
    // and 0 (no package).
    for id in [0u16, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15] {
        content.add_class(Class {
            id: ClassId(id),
            name: format!("Class{id}"),
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
    }
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: A,
        ..CoreConfig::default()
    }
}

fn player(name: &str, class: u16, level: u16) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(class),
        level,
        current_hp: 40,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: A,
        ..Default::default()
    }
}

fn attach(p: Player) -> (Core, SessionId) {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(p);
    (core, s)
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

// --- the §4.3 threshold table, every row ---

#[test]
fn icesorc_at_two_grants_ac() {
    // asm 00414e4d-00414e5c: EDX == 2 -> AC(0x2)+1.
    let mut p = player("Sorc", 1, 10);
    p.innate[0] = (Some(ab(125)), 2);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(2)), 1);

    let mut p = player("Sorc", 1, 10);
    p.innate[0] = (Some(ab(125)), 1);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(2)), 0, "== 2 only");
}

#[test]
fn alignment_package_is_per_class() {
    // asm 00414e5f-00414f5d: Good >= 8 || Neutral >= 8 || Evil >= 4
    // -> the class-keyed package; the package re-derives from the
    // CURRENT class (§5).
    // Warrior (group {1,2,3,0xf}): MaxDamage +1.
    let mut p = player("War", 1, 10);
    p.innate[0] = (Some(ab(126)), 8); // GoodQuest
    let (core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.innate_value(ab(4)), 1);
    assert_eq!(snap.innate_value(ab(0x45)), 0);

    // Priest-group class 5 ({5,0xc,0xd}): S.C. +1, MaxMana +10 — via
    // the EVIL path threshold (>= 4).
    let mut p = player("Pri", 5, 10);
    p.innate[0] = (Some(ab(128)), 4); // EvilQuest
    let (core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.innate_value(ab(0x46)), 1);
    assert_eq!(snap.innate_value(ab(0x45)), 10);

    // Thief-group class 7 ({7,8,0xe}): Bs +10/+10, Stealth +2 — via
    // Neutral >= 8.
    let mut p = player("Thf", 7, 10);
    p.innate[0] = (Some(ab(127)), 8); // NeutralQuest
    let (core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.innate_value(ab(0x75)), 10);
    assert_eq!(snap.innate_value(ab(0x76)), 10);
    assert_eq!(snap.innate_value(ab(0x1b)), 2);

    // Class 0: no package.
    let mut p = player("Non", 0, 10);
    p.innate[0] = (Some(ab(126)), 8);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(4)), 0);

    // Below every threshold: nothing.
    let mut p = player("Low", 1, 10);
    p.innate[0] = (Some(ab(126)), 7);
    p.innate[1] = (Some(ab(128)), 3);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(4)), 0);
}

#[test]
fn darkdruid_bloodchamp_wererat_rows() {
    // asm 00414f60/00414f73/0041506a: == 2 exactly.
    let mut p = player("Dru", 1, 10);
    p.innate[0] = (Some(ab(129)), 2); // DarkDruid -> S.C. +1
    p.innate[1] = (Some(ab(130)), 2); // BloodChamp -> Accuracy +3
    p.innate[2] = (Some(ab(132)), 2); // Wererat -> Dodge +1
    let (core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.innate_value(ab(0x46)), 1);
    assert_eq!(snap.innate_value(ab(0x16)), 3);
    assert_eq!(snap.innate_value(ab(0x22)), 1);
}

#[test]
fn shedragon_at_three_grants_crits_and_sc() {
    let mut p = player("Sla", 1, 10);
    p.innate[0] = (Some(ab(131)), 3);
    let (core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.innate_value(ab(0x3a)), 1);
    assert_eq!(snap.innate_value(ab(0x46)), 2);
    assert_eq!(snap.innate_value(ab(131)), 3, "counter survives at 3");
}

#[test]
fn shedragon_at_two_strips_exp_delevels_and_clears() {
    // asm 00414fa5-00415068: exp > 35,000,000 -> subtract, de-level
    // (FUN_00414c39: level drops while exp is below the curve), print
    // the two stripped lines; the 0x83 slots clear REGARDLESS.
    let mut p = player("Che", 1, 10);
    p.experience = 35_000_001;
    p.innate[0] = (Some(ab(131)), 2);
    let (core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.experience, 1);
    assert_eq!(snap.level, 1, "de-leveled to the floor");
    assert_eq!(snap.innate_value(ab(131)), 0, "counter cleared");
    let mut core = core;
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You have been stripped of 35,000,000 exp"),
        "got: {shown:?}"
    );
    assert!(
        shown.contains("exit and re-enter the realm"),
        "got: {shown:?}"
    );
}

#[test]
fn shedragon_penalty_skips_the_strip_below_the_bar() {
    // asm 00414fbc JBE + 00414ffe JC: at or below 35,000,000 nothing is
    // taken and nothing prints — but the counter still clears.
    let mut p = player("Poor", 1, 5);
    p.experience = 35_000_000;
    p.innate[0] = (Some(ab(131)), 2);
    let (mut core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.experience, 35_000_000, "untouched");
    assert_eq!(snap.level, 5);
    assert_eq!(snap.innate_value(ab(131)), 0, "counter still cleared");
    let shown = text_to(&core.drain_events(), s);
    assert!(!shown.contains("stripped"), "silent: got {shown:?}");
}

#[test]
fn reward_slots_rezero_and_regrant_idempotently() {
    // The LAB_00414dd0 pre-pass zeroes every reward-id slot before the
    // grants — attach twice, values identical (quests.md §4.3).
    let mut p = player("Idem", 1, 10);
    p.innate[0] = (Some(ab(129)), 2); // DarkDruid -> S.C. +1
    p.innate[1] = (Some(ab(0x46)), 7); // a stale S.C. grant to clear
    let (core, s) = attach(p);
    let first = core.player_snapshot(s);
    assert_eq!(first.innate_value(ab(0x46)), 1, "stale grant cleared");

    let (core2, s2) = attach(first);
    assert_eq!(core2.player_snapshot(s2).innate_value(ab(0x46)), 1);
}

#[test]
fn magebane_phoenix_daolord_counters_pass_through() {
    // Not in the threshold switch (asm scan captures only 0x7d-0x84):
    // the counters persist with no engine effect (quests.md §6).
    let mut p = player("Mage", 1, 10);
    p.innate[0] = (Some(ab(50)), 5);
    p.innate[1] = (Some(ab(133)), 9);
    p.innate[2] = (Some(ab(134)), 9);
    let (core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.innate_value(ab(50)), 5);
    assert_eq!(snap.innate_value(ab(133)), 9);
    assert_eq!(snap.innate_value(ab(134)), 9);
}

// --- the §4.4 class-skill strip (load_player 10164-10201) ---

#[test]
fn smash_strips_below_the_class_gate() {
    // Smash (0x20): class 1 keeps at level >= 22.
    let mut p = player("War", 1, 22);
    p.innate[0] = (Some(ab(0x20)), 1);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(0x20)), 1, "kept");

    let mut p = player("War", 1, 21);
    p.innate[0] = (Some(ab(0x20)), 1);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(0x20)), 0, "stripped");

    // A class outside the Smash set loses it at any level.
    let mut p = player("Thf", 7, 30);
    p.innate[0] = (Some(ab(0x20)), 1);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(0x20)), 0);
}

#[test]
fn perfect_stealth_and_meditate_gates() {
    // PerfectStealth (0xba): class 7 >= 20; Meditate (0xbb): class 5
    // >= 20 — both from the load_player tables (10175-10199).
    let mut p = player("Thf", 7, 20);
    p.innate[0] = (Some(ab(0xba)), 1);
    p.innate[1] = (Some(ab(0xbb)), 1); // class 7 is NOT in the Meditate set
    let (core, s) = attach(p);
    let snap = core.player_snapshot(s);
    assert_eq!(snap.innate_value(ab(0xba)), 1, "stealth kept");
    assert_eq!(snap.innate_value(ab(0xbb)), 0, "meditate stripped");

    let mut p = player("Pri", 5, 20);
    p.innate[0] = (Some(ab(0xbb)), 1);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(0xbb)), 1);

    let mut p = player("Pri", 5, 19);
    p.innate[0] = (Some(ab(0xbb)), 1);
    let (core, s) = attach(p);
    assert_eq!(core.player_snapshot(s).innate_value(ab(0xbb)), 0);
}
