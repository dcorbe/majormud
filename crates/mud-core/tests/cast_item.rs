//! Item-target casts (match type 6/7 → `cast_item_target`, decompile
//! 0x49232): the DetectMagic(26) banding on the item's Magical(28) value
//! (case 0x1a, 44620-44667), the "%s casts %s on %s." room line, and the
//! shared cost/roll skeleton. DATA (slice-5 Task 6 check,
//! re/mmud_wgnt.sqlite): 53 shipped match-6/7 spells, exactly TWO learnable
//! — detect magic (24, mage L5; scroll 121 sold at the Newhaven Mage Spell
//! Shop 9) and song of lore (41, bard — out of scope) — both carrying only
//! DetectMagic(26). Every string here is decompile-only: ORACLE-VERIFY.

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Element, Item, ItemId, MatchType, Race, RaceId, Room, RoomId,
    SaveClass, ScalePair, Spell, SpellId, StatBlock, TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const MAGE: ClassId = ClassId(1);
const WARRIOR: ClassId = ClassId(2);
const HUMAN: RaceId = RaceId(1);
const TOWER: RoomId = RoomId { map: 1, room: 1 };

/// The detect magic (24) model: match 7, instant, [(DetectMagic, 1)].
const DETECT: SpellId = SpellId(600);

const AMULET: ItemId = ItemId(700); // Magical 1  -> faintly/small
const TRINKET: ItemId = ItemId(710); // Magical 3 -> softly/good
const CROWN: ItemId = ItemId(720); // Magical 5  -> brightly/large
const RELIC: ItemId = ItemId(730); // Magical 7  -> blinding/immense
const ROCK: ItemId = ItemId(740); // no Magical  -> no magic

fn item(id: ItemId, name: &str, magical: Option<i16>) -> Item {
    Item {
        id,
        name: name.into(),
        abilities: magical.map(|v| (Ability::Magical, v)).into_iter().collect(),
        weight: 1,
        uses: -1,
        gettable: 1,
        ..Item::default()
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
        ..Default::default()
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
    let detect = Spell {
        id: DETECT,
        name: "detect magic".into(),
        short_name: "dete".into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![(Ability::DetectMagic, 1)],
        level_cap: 0,
        round_cost: 100,
        required_power: 1,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200,
        duration_per_level: 0,
        match_type: MatchType::Item7,
        duration: 0,
        element: Element::Cold,
        class_gate_group: 1,
        mana_cost: 8,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    };
    content.add_spell(detect);
    for it in [
        item(AMULET, "gold amulet", Some(1)),
        item(TRINKET, "silver trinket", Some(3)),
        item(CROWN, "jeweled crown", Some(5)),
        item(RELIC, "ancient relic", Some(7)),
        item(ROCK, "plain rock", None),
    ] {
        content.add_item(it);
    }
    content
}

fn player(name: &str, class: ClassId) -> Player {
    let mut book = BTreeMap::new();
    book.insert(DETECT, false);
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: HUMAN,
        class,
        level: 5,
        stats: StatBlock::default(),
        base_stats: StatBlock::default(),
        hp_base: 0,
        current_hp: 10,
        current_mana: 20,
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
        spellbook: book,
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

fn cast(core: &mut Core, s: SessionId, line: &str) -> String {
    core.input(s, line);
    text_to(&core.drain_events(), s)
}

fn energy_round(core: &mut Core) {
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
}

fn mage_with_items() -> (Core, SessionId) {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE));
    for it in [AMULET, TRINKET, CROWN, RELIC, ROCK] {
        core.give_item(s, it);
    }
    core.drain_events();
    (core, s)
}

#[test]
fn detect_magic_bands_on_the_item_magical_value() {
    // case 0x1a: Magical 1 / 2-3 / 4-5 / 6+ / absent (44622-44658).
    let (mut core, s) = mage_with_items();
    let shown = cast(&mut core, s, "c dete amulet");
    assert!(
        shown.contains(
            "gold amulet glows faintly, indicating a small amount of magic within.\n"
        ),
        "got: {shown:?}"
    );
    energy_round(&mut core);
    core.set_current_mana(s, 20);
    let shown = cast(&mut core, s, "c dete trinket");
    assert!(
        shown.contains(
            "silver trinket glows softly, indicating a good amount of magic within.\n"
        ),
        "got: {shown:?}"
    );
    energy_round(&mut core);
    core.set_current_mana(s, 20);
    let shown = cast(&mut core, s, "c dete crown");
    assert!(
        shown.contains(
            "jeweled crown glows brightly, indicating a large amount of magic within.\n"
        ),
        "got: {shown:?}"
    );
    energy_round(&mut core);
    core.set_current_mana(s, 20);
    let shown = cast(&mut core, s, "c dete relic");
    assert!(
        shown.contains(
            "You are almost blinded by the aura from ancient relic, indicating immense magical properties!\n"
        ),
        "got: {shown:?}"
    );
    energy_round(&mut core);
    core.set_current_mana(s, 20);
    let shown = cast(&mut core, s, "c dete rock");
    assert!(
        shown.contains("You detect no magic in that item!\n"),
        "got: {shown:?}"
    );
}

#[test]
fn detect_magic_announces_to_the_room_not_the_caster() {
    // 44660-44663: "%s casts %s on %s." (caster, spell, item) through
    // tell_room with the caster excluded.
    let (mut core, s) = mage_with_items();
    let watcher = core.attach_player(player("Grunt", WARRIOR));
    core.drain_events();
    core.input(s, "c dete amulet");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("Vexil casts detect magic"),
        "caster does not see the room line: {shown:?}"
    );
    let seen = text_to(&events, watcher);
    assert!(
        seen.contains("Vexil casts detect magic on gold amulet.\n"),
        "got: {seen:?}"
    );
}

#[test]
fn item_cast_pays_costs_and_spends_the_round() {
    let (mut core, s) = mage_with_items();
    let energy = core.round_energy(s);
    cast(&mut core, s, "c dete amulet");
    assert_eq!(core.current_mana(s), 12, "mana 8 charged on success");
    assert_eq!(core.round_energy(s), energy - 100, "round cost charged");
    let second = cast(&mut core, s, "c dete trinket");
    assert!(
        second.contains("You have already cast a spell this round!"),
        "one cast per round: {second:?}"
    );
}

#[test]
fn item_cast_failed_roll_charges_half_mana() {
    // The non-caster trick (SC term is deeply negative for caster_group
    // 0): a rollable item cast can never succeed. Fail path 44670-44700:
    // full round cost, half mana, the same fail lines as cast_no_target.
    let mut world = world();
    let mut risky = world.spells[&DETECT].clone();
    risky.id = SpellId(610);
    risky.name = "risky lore".into();
    risky.short_name = "risk".into();
    risky.base_chance = 15;
    world.add_spell(risky);
    let mut core = Core::new(world, CoreConfig::default());
    let mut grunt = player("Grunt", WARRIOR);
    grunt.spellbook.insert(SpellId(610), false);
    let s = core.attach_player(grunt);
    core.give_item(s, AMULET);
    core.drain_events();
    let shown = cast(&mut core, s, "c risk amulet");
    assert!(
        shown.contains("You attempt to cast risky lore, but fail.\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 16, "half of mana 8 charged");
    assert!(!shown.contains("glows"), "no banding on a failed roll: {shown:?}");
}

#[test]
fn item_cast_energy_shortage_prints_the_already_cast_line() {
    // cast_item_target's own triple gate (44397-44409): round pool <
    // round cost prints "You have already cast a spell this round!" —
    // UNLIKE the benign self-cast path's measured silent no-op. No costs
    // move.
    let mut world = world();
    let mut bulky = world.spells[&DETECT].clone();
    bulky.id = SpellId(620);
    bulky.name = "bulky lore".into();
    bulky.short_name = "bulk".into();
    bulky.round_cost = 2000; // above the full 1000 pool
    world.add_spell(bulky);
    let mut core = Core::new(world, CoreConfig::default());
    let mut vexil = player("Vexil", MAGE);
    vexil.spellbook.insert(SpellId(620), false);
    let s = core.attach_player(vexil);
    core.give_item(s, AMULET);
    core.drain_events();
    let shown = cast(&mut core, s, "c bulk amulet");
    assert!(
        shown.contains("You have already cast a spell this round!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 20, "no mana charged");
    assert!(!shown.contains("glows"), "no banding: {shown:?}");
}

#[test]
fn bare_item_cast_with_empty_target_takes_the_benign_self_path() {
    // ORACLE-VERIFY (pinning CURRENT behavior, not a measured line):
    // `c dete` with no target never reaches the item arm (it matches on
    // a non-empty remainder only) and falls through to the benign
    // self-cast path — costs are paid, the round is consumed, and
    // nothing prints (detect magic has no castmsgb and DetectMagic(26)
    // has no self-target handler). The DLL dispatcher's kind-8
    // find_action_target arm with an empty name is unmeasured — it may
    // refuse instead.
    let (mut core, s) = mage_with_items();
    let energy = core.round_energy(s);
    let shown = cast(&mut core, s, "c dete");
    assert!(!shown.contains("You do not see"), "no refusal: {shown:?}");
    assert!(!shown.contains("glows"), "no banding: {shown:?}");
    assert_eq!(core.current_mana(s), 12, "full mana charged");
    assert_eq!(core.round_energy(s), energy - 100, "round cost charged");
    let second = cast(&mut core, s, "c dete amulet");
    assert!(
        second.contains("You have already cast a spell this round!"),
        "round consumed: {second:?}"
    );
}

#[test]
fn item_cast_needs_the_item_in_inventory() {
    // Unmatched item: the same do-not-see refusal as any benign targeted
    // lookup, before any cost (ORACLE-VERIFY: the DLL's find_action_target
    // may print "You are not carrying %s!" for ground items — unmeasured).
    let (mut core, s) = mage_with_items();
    let shown = cast(&mut core, s, "c dete zzz");
    assert!(shown.contains("You do not see zzz here!"), "got: {shown:?}");
    assert_eq!(core.current_mana(s), 20, "no cost");
}
