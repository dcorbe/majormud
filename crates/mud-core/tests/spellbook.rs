//! Tests for the spellbook: the `spells` listing (spellcasting.md §8.5,
//! golden block from oracle_spell_train.raw) and the learnability gate
//! (§2 gates 1-2, suffix behavior proven in §8.3).

use std::collections::BTreeMap;

use mud_core::command::{parse, Command};
use mud_core::content::{
    Class, ClassId, Content, Element, MatchType, Race, RaceId, Room, RoomId, SaveClass, ScalePair,
    Spell, SpellId, StatBlock, TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId, SpellGate};

const MAGE: ClassId = ClassId(1);
const WARRIOR: ClassId = ClassId(2);

// Spell ids deliberately ordered OPPOSITE the display order (BTreeMap
// iterates by id) so the listing tests prove the (level, name) sort.
const ILLUMINATE: SpellId = SpellId(10);
const MAGIC_MISSILE: SpellId = SpellId(20);
const BLUR: SpellId = SpellId(30);
const DEEP_MAGERY: SpellId = SpellId(40);

fn spell(id: SpellId, name: &str, short: &str) -> Spell {
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
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Tower".into(),
        description: vec![],
        room_type: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
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
        id: MAGE,
        name: "Mage".into(),
        abilities: vec![],
        hp_per_level: 2,
        hp_seed: 4,
        // Real Mage values (magictype=1, magiclvl=3). Deliberately unequal
        // so a caster_group/casting_factor field swap in spell_gate fails.
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

    let mut illu = spell(ILLUMINATE, "illuminate", "illu");
    illu.mana_cost = 4;
    illu.required_power = 2;
    let mut mmis = spell(MAGIC_MISSILE, "magic missile", "mmis");
    mmis.mana_cost = 1;
    let mut blur = spell(BLUR, "blur", "blur");
    blur.mana_cost = 4;
    // Same magery group as the mage, but requires a deeper casting factor
    // than the fixture mage's 3 — never castable by either fixture class.
    let mut deep = spell(DEEP_MAGERY, "meteor storm", "mets");
    deep.required_class_level = 5;
    content.add_spell(illu);
    content.add_spell(mmis);
    content.add_spell(blur);
    content.add_spell(deep);
    content
}

fn player(name: &str, class: ClassId, spellbook: BTreeMap<SpellId, bool>) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class,
        level: 1,
        stats: StatBlock::default(),
        base_stats: StatBlock::default(),
        hp_base: 0,
        current_hp: 10,
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
        location: RoomId { map: 1, room: 1 },
        spellbook,
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

#[test]
fn spells_lists_book_sorted_by_level_then_name() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut book = BTreeMap::new();
    book.insert(ILLUMINATE, false);
    book.insert(MAGIC_MISSILE, false);
    book.insert(BLUR, false);
    let s = core.attach_player(player("Vexil", MAGE, book));
    core.drain_events();
    core.input(s, "spells");
    let shown = text_to(&core.drain_events(), s);
    // Golden block copied byte-for-byte (ANSI stripped) from
    // re/oracle/oracle_spell_train.raw lines 188-193; the trailing spaces
    // and final blank line are part of the output. Level 1 rows sort by
    // name (blur < magic missile) despite id order saying otherwise.
    let expected = concat!(
        "You have the following spells:\n",
        "Level Mana Short Spell Name\n",
        "  1   4    blur  blur                          \n",
        "  1   1    mmis  magic missile                 \n",
        "  2   4    illu  illuminate                    \n",
        "\n",
    );
    assert!(shown.contains(expected), "golden block: {shown:?}");
}

#[test]
fn empty_book_prints_you_have_no_spells() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, BTreeMap::new()));
    core.drain_events();
    core.input(s, "spells");
    let shown = text_to(&core.drain_events(), s);
    // VERIFIED oracle_spell_learning.raw line 162: single line, no
    // header, no trailing blank line before the prompt. Prefix match up to
    // the prompt's '[' so an extra blank line can't hide behind contains().
    assert!(
        shown.starts_with("You have no spells.\n["),
        "got: {shown:?}"
    );
    assert!(
        !shown.contains("following spells"),
        "no header on empty book: {shown:?}"
    );
}

#[test]
fn spell_gate_passes_castable_same_group_spell() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, BTreeMap::new()));
    let mage = core.player_snapshot(s);
    assert_eq!(
        core.spell_gate(&mage, &core.content().spells[&MAGIC_MISSILE]),
        SpellGate::Ok
    );
    // Gate 1 only applies to gated spells: an ungated (group 0) spell
    // passes even for the non-caster warrior.
    let w = core.attach_player(player("Grunt", WARRIOR, BTreeMap::new()));
    let warrior = core.player_snapshot(w);
    let mut ungated = spell(SpellId(99), "light", "ligh");
    ungated.class_gate_group = 0;
    assert_eq!(core.spell_gate(&warrior, &ungated), SpellGate::Ok);
}

#[test]
fn spell_gate_rejects_high_required_power_as_too_powerful() {
    // Illuminate requires power 2; the fixture mage is level 1.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, BTreeMap::new()));
    let mage = core.player_snapshot(s);
    assert_eq!(
        core.spell_gate(&mage, &core.content().spells[&ILLUMINATE]),
        SpellGate::TooPowerful
    );
}

#[test]
fn spell_gate_rejects_wrong_magery_group_as_wrong_class() {
    let mut core = Core::new(world(), CoreConfig::default());
    let w = core.attach_player(player("Grunt", WARRIOR, BTreeMap::new()));
    let warrior = core.player_snapshot(w);
    assert_eq!(
        core.spell_gate(&warrior, &core.content().spells[&MAGIC_MISSILE]),
        SpellGate::WrongClass
    );
    // Right group but the class can't ever cast this deep: casting
    // factor 3 < required class level 5 is WrongClass, not TooPowerful.
    let s = core.attach_player(player("Vexil", MAGE, BTreeMap::new()));
    let mage = core.player_snapshot(s);
    assert_eq!(
        core.spell_gate(&mage, &core.content().spells[&DEEP_MAGERY]),
        SpellGate::WrongClass
    );
}

#[test]
fn spells_verb_min_abbreviation() {
    // ORACLE-VERIFY: real minimum unmeasured; 2 chosen to be unambiguous
    // ("s" is the south alias, "st" is status's minimum).
    assert_eq!(parse("sp"), Command::Spells);
    assert_eq!(parse("spells"), Command::Spells);
    assert_eq!(parse("s"), Command::Move(mud_core::content::Direction::South));
}
