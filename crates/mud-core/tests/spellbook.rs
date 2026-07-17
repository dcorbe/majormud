//! Tests for the spellbook: the `spells` listing (spellcasting.md §8.5,
//! golden block from oracle_spell_train.raw), the learnability gate
//! (§2 gates 1-2, suffix behavior proven in §8.3), and the `use`/`read`
//! LearnSp scroll handler (§8.4, oracle_use_verbs.raw / oracle_use_verbs2.raw).

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::command::{parse, Command};
use mud_core::content::{
    Class, ClassId, Content, Element, Item, ItemId, MatchType, PlacedItem, Race, RaceId, Room,
    RoomId, SaveClass, ScalePair, Shop, ShopId, ShopStock, Spell, SpellId, StatBlock, TargetMode,
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
// A priest-group spell (§8.3's "scroll of minor healing" shape) — wrong
// magery group for both fixture classes.
const MINOR_HEALING: SpellId = SpellId(50);

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

const MMIS_SCROLL: ItemId = ItemId(200);
const ILLU_SCROLL: ItemId = ItemId(201);
const CLUB: ItemId = ItemId(202);
const HEAL_SCROLL: ItemId = ItemId(203);
/// Teaches a spell id that resolves to nothing in `content.spells`.
const GHOST_SCROLL: ItemId = ItemId(204);
const SHOP_ROOM: RoomId = RoomId { map: 1, room: 2 };

/// A LearnSp(42) scroll teaching `teaches`, with the real parchment
/// description lines (item 119) so the read-unowned golden matches the
/// oracle bytes.
fn scroll(id: ItemId, name: &str, teaches: SpellId) -> Item {
    Item {
        id,
        name: name.into(),
        abilities: vec![(Ability::from_id(42).unwrap(), teaches.0 as i16)],
        uses: 1,
        gettable: 1,
        description: vec![
            "This parchment is inscribed with runes of magic, but exactly".into(),
            "what is written can only be learned by reading it.".into(),
        ],
        ..Item::default()
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Tower".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        // A club on the floor: the read-unowned floor path.
        placed_items: vec![PlacedItem { item: CLUB, quantity: 1 }],
        exits: Default::default(),
    });
    content.add_room(Room {
        id: SHOP_ROOM,
        name: "Spell Shop".into(),
        description: vec![],
        room_type: 1,
        attributes: 0,
        shop: Some(ShopId(48)),
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_item(scroll(MMIS_SCROLL, "scroll of magic missile", MAGIC_MISSILE));
    content.add_item(scroll(ILLU_SCROLL, "scroll of illuminate", ILLUMINATE));
    content.add_item(scroll(HEAL_SCROLL, "scroll of minor healing", MINOR_HEALING));
    content.add_item(scroll(GHOST_SCROLL, "scroll of oblivion", SpellId(999)));
    content.add_item(Item {
        id: CLUB,
        name: "club".into(),
        item_type: 1,
        uses: -1,
        gettable: 1,
        description: vec!["A stout wooden club.".into()],
        ..Item::default()
    });
    let mut stock = [ShopStock::default(); 20];
    for (slot, item) in [MMIS_SCROLL, ILLU_SCROLL, HEAL_SCROLL, GHOST_SCROLL]
        .into_iter()
        .enumerate()
    {
        stock[slot] = ShopStock {
            item: Some(item),
            max: 5,
            now: 5,
            ..ShopStock::default()
        };
    }
    content.add_shop(Shop {
        id: ShopId(48),
        name: "Spell Shop".into(),
        shop_type: 2,
        min_level: 0,
        max_level: 0,
        markup: 0,
        class_limit: 0,
        stock,
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
    let mut heal = spell(MINOR_HEALING, "minor healing", "mihe");
    heal.class_gate_group = 2;
    content.add_spell(illu);
    content.add_spell(mmis);
    content.add_spell(blur);
    content.add_spell(deep);
    content.add_spell(heal);
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

// --- use/read LearnSp handler (spellcasting.md §8.4) ---

/// The oracle render of the fixture scroll's description: stored lines
/// re-flowed as a word stream (oracle_spell_cast.raw `read scroll of
/// smite`, raw bytes) — interior lines break flush, the final line keeps
/// one trailing space.
const SCROLL_DESCRIPTION: &str = "This parchment is inscribed with runes of magic, but exactly \
                                  what is written\ncan only be learned by reading it. \n";

#[test]
fn use_scroll_learns_and_consumes() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.inventory.push((MMIS_SCROLL, 1));
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "use scroll of magic missile");
    let shown = text_to(&core.drain_events(), s);
    // VERIFIED (§8.4): the learn line plus a trailing blank line, then
    // the prompt.
    assert!(
        shown.contains(
            "You read scroll of magic missile and learn the spell magic missile.\n\n["
        ),
        "got: {shown:?}"
    );
    let after = core.player_snapshot(s);
    assert!(after.inventory.is_empty(), "scroll consumed");
    assert_eq!(after.spellbook.get(&MAGIC_MISSILE), Some(&false));
}

#[test]
fn read_scroll_adds_disintegrate_line() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.inventory.push((MMIS_SCROLL, 1));
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "read scroll of magic missile");
    let shown = text_to(&core.drain_events(), s);
    // VERIFIED (§8.4): same learn line, then the destruction line — and
    // NO trailing blank line before the prompt (unlike `use`).
    assert!(
        shown.contains(
            "You read scroll of magic missile and learn the spell magic missile.\n\
             Its magic used, the scroll disintegrates.\n["
        ),
        "got: {shown:?}"
    );
    let after = core.player_snapshot(s);
    assert!(after.inventory.is_empty(), "scroll consumed");
    assert_eq!(after.spellbook.get(&MAGIC_MISSILE), Some(&false));
}

#[test]
fn too_high_scroll_refused_not_consumed() {
    // Illuminate requires power 2; the fixture mage is level 1.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.inventory.push((ILLU_SCROLL, 1));
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "use scroll of illuminate");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not use that item!"),
        "got: {shown:?}"
    );
    let after = core.player_snapshot(s);
    assert_eq!(after.inventory, vec![(ILLU_SCROLL, 1)], "scroll kept");
    assert!(after.spellbook.is_empty(), "book unchanged");
}

#[test]
fn wrong_class_scroll_refused_not_consumed() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut grunt = player("Grunt", WARRIOR, BTreeMap::new());
    grunt.inventory.push((MMIS_SCROLL, 1));
    let s = core.attach_player(grunt);
    core.drain_events();
    core.input(s, "use scroll of magic missile");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not use that item!"),
        "got: {shown:?}"
    );
    let after = core.player_snapshot(s);
    assert_eq!(after.inventory, vec![(MMIS_SCROLL, 1)], "scroll kept");
    assert!(after.spellbook.is_empty(), "book unchanged");
}

#[test]
fn read_unowned_scroll_prints_description() {
    // VERIFIED (§8.4, oracle_spell_cast.raw): reading a shelf scroll you
    // don't carry prints the item description paragraph and learns nothing.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.location = SHOP_ROOM;
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "read scroll of magic missile");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains(SCROLL_DESCRIPTION), "got: {shown:?}");
    let after = core.player_snapshot(s);
    assert!(after.spellbook.is_empty(), "learns nothing");
    assert!(after.inventory.is_empty(), "gains nothing");
}

#[test]
fn read_unowned_floor_item_prints_description() {
    // The floor variant of the description path (club placed in Tower).
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, BTreeMap::new()));
    core.drain_events();
    core.input(s, "read club");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("A stout wooden club. \n"),
        "got: {shown:?}"
    );
}

#[test]
fn use_unowned_never_reads_the_shelf() {
    // VERIFIED (oracle_use_verbs.raw): `use` on a shelf-only item prints
    // the don't-have line; only `read` falls back to the description.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.location = SHOP_ROOM;
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "use scroll of magic missile");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You don't have scroll of magic missile.\n"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("parchment"), "no description: {shown:?}");
}

#[test]
fn read_invisible_item_prints_do_not_see() {
    // VERIFIED (oracle_use_verbs.raw): nothing owned, nothing visible.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, BTreeMap::new()));
    core.drain_events();
    core.input(s, "read zzz");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You do not see zzz here!\n"),
        "got: {shown:?}"
    );
}

#[test]
fn non_learnsp_item_refused_by_both_verbs() {
    // VERIFIED (oracle_use_verbs2.raw): an owned item with no use action
    // gets the refusal from both verbs and is kept.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.inventory.push((CLUB, -1));
    let s = core.attach_player(vexil);
    core.drain_events();
    for verb in ["use club", "read club"] {
        core.input(s, verb);
        let shown = text_to(&core.drain_events(), s);
        assert!(
            shown.contains("You may not use that item!"),
            "{verb}: {shown:?}"
        );
    }
    assert_eq!(core.player_snapshot(s).inventory, vec![(CLUB, -1)]);
}

#[test]
fn already_known_scroll_behaves_like_oracle() {
    // VERIFIED (oracle_use_verbs.raw): both verbs, not consumed, book
    // unchanged, no learn line.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut book = BTreeMap::new();
    book.insert(MAGIC_MISSILE, false);
    let mut vexil = player("Vexil", MAGE, book.clone());
    vexil.inventory.push((MMIS_SCROLL, 1));
    let s = core.attach_player(vexil);
    core.drain_events();
    for verb in ["use scroll of magic missile", "read scroll of magic missile"] {
        core.input(s, verb);
        let shown = text_to(&core.drain_events(), s);
        assert!(
            shown.contains("You realize that you already know this scroll!\n"),
            "{verb}: {shown:?}"
        );
        assert!(!shown.contains("learn the spell"), "{verb}: {shown:?}");
    }
    let after = core.player_snapshot(s);
    assert_eq!(after.inventory, vec![(MMIS_SCROLL, 1)], "scroll kept");
    assert_eq!(after.spellbook, book, "book unchanged");
}

#[test]
fn learned_spell_persists() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.inventory.push((MMIS_SCROLL, 1));
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "use scroll of magic missile");
    let events = core.drain_events();
    let persisted = events.iter().find_map(|e| match e {
        Event::Persist(p) if p.name == "Vexil" => Some(p),
        _ => None,
    });
    let persisted = persisted.expect("Event::Persist after learning");
    assert_eq!(persisted.spellbook.get(&MAGIC_MISSILE), Some(&false));
    assert!(persisted.inventory.is_empty());
}

// --- shop listing suffixes (spellcasting.md §8.3) ---

/// The shop row for `name` in the listing, panicking if absent.
fn shop_row(shown: &str, name: &str) -> String {
    shown
        .lines()
        .find(|l| l.contains(name))
        .unwrap_or_else(|| panic!("{name} row missing: {shown:?}"))
        .to_string()
}

#[test]
fn shop_rows_annotate_scrolls_by_spell_gate() {
    // VERIFIED (§8.3): a mage viewing the spell shop — castable scroll
    // bare, same-group-too-high scroll "(Too powerful)", other-group
    // scroll "(You can't use)".
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.location = SHOP_ROOM;
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    let mmis = shop_row(&shown, "scroll of magic missile");
    assert!(
        mmis.trim_end().ends_with("Free"),
        "castable scroll unsuffixed: {mmis:?}"
    );
    let illu = shop_row(&shown, "scroll of illuminate");
    assert!(
        illu.ends_with(" (Too powerful)"),
        "level-gated scroll: {illu:?}"
    );
    let heal = shop_row(&shown, "scroll of minor healing");
    assert!(
        heal.ends_with(" (You can't use)"),
        "wrong-group scroll: {heal:?}"
    );
}

#[test]
fn warrior_sees_cant_use_on_every_scroll() {
    // VERIFIED (§8.3): the non-caster fails gate 1 on all gated spells —
    // never "(Too powerful)", even on the level-gated illuminate.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut grunt = player("Grunt", WARRIOR, BTreeMap::new());
    grunt.location = SHOP_ROOM;
    let s = core.attach_player(grunt);
    core.drain_events();
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    for name in [
        "scroll of magic missile",
        "scroll of illuminate",
        "scroll of minor healing",
    ] {
        let row = shop_row(&shown, name);
        assert!(row.ends_with(" (You can't use)"), "{name}: {row:?}");
    }
    assert!(!shown.contains("Too powerful"), "got: {shown:?}");
}

#[test]
fn scroll_teaching_unknown_spell_lists_as_cant_use() {
    // A LearnSp value that resolves to no spell can never be learned;
    // annotate like WrongClass rather than pretending it's usable.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, BTreeMap::new());
    vexil.location = SHOP_ROOM;
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    let row = shop_row(&shown, "scroll of oblivion");
    assert!(row.ends_with(" (You can't use)"), "got: {row:?}");
}
