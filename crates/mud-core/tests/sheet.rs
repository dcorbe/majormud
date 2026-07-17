//! Tests for the status sheet and the [HP=n]: prompt.
//!
//! The nine sheet lines are verbatim from the MBBSEmu oracle transcript
//! (re/oracle/oracle_m1.raw) with ONE whitelisted divergence: Martial Arts
//! shows 11, not the DOS build's 10 — the WG3-NT `calculate_secondary_stats`
//! adds the clamped dodge base twice (decompile line 13638), the DOS build
//! once. The spec (WG3-NT) wins per the design's fidelity rule.

use mud_core::content::{
    Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec![],
        room_type: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
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
