//! Tests for the status sheet and the [HP=n]: prompt.
//!
//! The nine sheet lines are verbatim from the MBBSEmu oracle transcript
//! (re/oracle/oracle_m1.raw) with ONE whitelisted divergence: Martial Arts
//! shows 11, not the DOS build's 10 — the WG3-NT `calculate_secondary_stats`
//! adds the clamped dodge base twice (decompile line 13638), the DOS build
//! once. The spec (WG3-NT) wins per the design's fidelity rule.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    content.add_race(Race {
        id: RaceId(2),
        name: "Dwarf".into(),
        // Dwarf M.R. +10 (ability 36), Illu 75 (ability 13) as in real data.
        abilities: vec![
            (mud_core::ability::Ability::from_id(36).unwrap(), 10),
            (mud_core::ability::Ability::from_id(13).unwrap(), 75),
        ],
        base_stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 30,
        },
        max_stats: StatBlock {
            intellect: 90,
            wisdom: 120,
            strength: 110,
            health: 120,
            agility: 90,
            charm: 85,
        },
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

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        ..CoreConfig::default()
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

fn create_dwarf_warrior(core: &mut Core, name: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
        saved_evil: 0,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.input(s, "No");
    s
}

const ORACLE_SHEET: &str = "\
Name: Oracle Delver                    Lives/CP:      9/100
Race: Dwarf       Exp: 0               Perception:     35
Class: Warrior    Level: 1             Stealth:         0
Hits:    35/35    Armour Class:   0/0  Thievery:        0
                                       Traps:           0
                                       Picklocks:       0
Strength:  50     Agility: 30          Tracking:        0
Intellect: 30     Health:  50          Martial Arts:   11
Willpower: 50     Charm:   30          MagicRes:       55
";

#[test]
fn stat_command_renders_the_oracle_sheet() {
    let mut core = Core::new(world(), config());
    let s = create_dwarf_warrior(&mut core, "Oracle Delver");
    core.drain_events();

    core.input(s, "stat");
    let shown = text_to(&core.drain_events(), s);
    // Compare line-by-line, tolerating the oracle's %-5d trailing padding.
    let got: Vec<&str> = shown.lines().map(str::trim_end).collect();
    for want in ORACLE_SHEET.lines().map(str::trim_end) {
        assert!(
            got.contains(&want),
            "missing sheet line {want:?}\ngot:\n{shown}"
        );
    }
}

#[test]
fn finished_creation_shows_sheet_and_prompt_not_room() {
    // Oracle: first entry prints the stat sheet + prompt; the room is not
    // shown until the player acts.
    let mut core = Core::new(world(), config());
    let s = create_dwarf_warrior(&mut core, "Oracle Delver");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("Hits:    35/35"), "sheet shown: {shown}");
    assert!(shown.ends_with("[HP=35]:"), "prompt last: {shown:?}");
    assert!(!shown.contains("Town Gates"), "no room display: {shown}");
}

#[test]
fn prompt_follows_every_command_response() {
    let mut core = Core::new(world(), config());
    let s = create_dwarf_warrior(&mut core, "Oracle");
    core.drain_events();

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.ends_with("[HP=35]:"),
        "prompt after look: {shown:?}"
    );

    core.input(s, "xyzzy");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.ends_with("You say \"xyzzy\"\n[HP=35]:"),
        "prompt after say: {shown:?}"
    );
}

#[test]
fn new_character_hp_is_the_derived_maximum() {
    let mut core = Core::new(world(), config());
    let s = create_dwarf_warrior(&mut core, "Oracle");
    let events = core.drain_events();
    let persisted = events
        .iter()
        .find_map(|e| match e {
            Event::Persist(p) => Some(p),
            _ => None,
        })
        .expect("persisted");
    assert_eq!(persisted.current_hp, 35);
    assert_eq!(persisted.current_mana, 0);
    assert_eq!(persisted.hp_base, 4);
    let _ = s;
}

/// A worn armour fixture: `+0x342` (`ac`), `+0x39c` (`dr`), `+0x396`
/// (armour class-strength code, which selects the AC(Blur) divisor).
fn gear(id: u16, name: &str, worn_on: i16, evasion: i16, damage_resist: i16) -> Item {
    Item {
        id: ItemId(id),
        name: name.into(),
        weight: 10,
        item_type: 0,
        uses: -1,
        evasion,
        damage_resist,
        worn_on,
        gettable: 1,
        ..Item::default()
    }
}

/// The 2026-07-26 oracle set, with the shipped columns verbatim
/// (`re/mmud_wgnt.sqlite`): gilded robes ac 70 / dr 0, chain coif 45 / 8,
/// displacer fur cloak 10 / 0, beaded belt 0 / 0, violet orchid 0 / 0.
fn oracle_gear(content: &mut Content) {
    content.add_item(gear(364, "gilded robes", 11, 70, 0));
    content.add_item(gear(23, "chain coif", 2, 45, 8));
    content.add_item(gear(1386, "displacer fur cloak", 7, 10, 0));
    content.add_item(gear(1126, "beaded belt", 10, 0, 0));
    content.add_item(gear(1919, "violet orchid", 16, 0, 0));
}

fn wear_all(core: &mut Core, s: SessionId, names: &[(u16, &str)]) {
    for (id, name) in names {
        core.give_item(s, ItemId(*id));
        core.input(s, &format!("wear {name}"));
    }
    core.drain_events();
}

/// MEASURED (`re/oracle/oracle_dodge_parry_acc-high.raw`): Oracle Delver in
/// exactly this set read `Armour Class:  12/0` on the live board.
///
/// `get_armour_rating` (16956) returns Σ`+0x342` and out-params Σ`+0x39c`;
/// the status line divides BOTH by 10 (31553-31556). Σac = 70+45+10 = 125
/// → 12; Σdr = 8 → 0. This is the pair that proves the two columns are not
/// interchangeable — see `game_combat::worn_dr_soaks_damage_but_worn_ac_does_not`.
#[test]
fn geared_sheet_matches_the_measured_armour_class_pair() {
    let mut content = world();
    oracle_gear(&mut content);
    let mut core = Core::new(content, config());
    let s = create_dwarf_warrior(&mut core, "Oracle Delver");
    wear_all(
        &mut core,
        s,
        &[
            (364, "robes"),
            (23, "coif"),
            (1386, "cloak"),
            (1126, "belt"),
            (1919, "orchid"),
        ],
    );

    core.input(s, "stat");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Armour Class:  12/0"),
        "measured pair from the acc-high capture: {shown:?}"
    );
}

/// The AC(2) ability reaches the DISPLAY, scaled by 10, and the result is
/// clamped at 0 — `ac = Σ+0x342 + ability2*10; if (ac < 0) ac = 0` (17045-
/// 17048). Note the odd calling convention that makes this easy to misread:
/// `get_user_ability_value(7, -1, player, 2, &local_c)` collects ability 7
/// into its RETURN (discarded here) and ability **2** into `local_c`
/// (36832-36864) — so it is AC(2), not DR(7), that moves this number.
///
/// MEASURED: the same character wearing the smoky black talisman (ability
/// 2 = -20) read `Armour Class:   0/0`, not `-8/0`
/// (`oracle_dodge_parry_acc-mid.raw`). 125 + (-20*10) = -75, clamped.
#[test]
fn the_ac_ability_scales_by_ten_and_the_display_clamps_at_zero() {
    let mut content = world();
    oracle_gear(&mut content);
    let mut talisman = gear(608, "smoky black talisman", 8, 0, 0);
    talisman.abilities = vec![(Ability::AC, -20)];
    content.add_item(talisman);
    let mut core = Core::new(content, config());
    let s = create_dwarf_warrior(&mut core, "Oracle Delver");
    wear_all(
        &mut core,
        s,
        &[
            (364, "robes"),
            (23, "coif"),
            (1386, "cloak"),
            (1126, "belt"),
            (1919, "orchid"),
            (608, "talisman"),
        ],
    );

    core.input(s, "stat");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Armour Class:   0/0"),
        "clamped, not -8: {shown:?}"
    );
}
