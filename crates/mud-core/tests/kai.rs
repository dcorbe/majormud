//! Kai/mystic tests (spellcasting.md §8.12, oracle_kai_mystic*.raw):
//! trainer-granted powers, the cast/spells hard refusals, the `powers`
//! listing, the `invoke` verb riding the cast pipeline with kai wording,
//! the level-1 pool (max kai = level - 1) surfaces (prompt/health/st),
//! and the flat +1 slow-tick regen.

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::command::{parse, Command};
use mud_core::content::{
    Class, ClassId, Content, Message, MessageId, Race, RaceId, Room, RoomId, Shop, ShopId,
    Spell, SpellId, StatBlock,
};
use mud_core::game::{Coins, Core, CoreConfig, Event, Gender, Player, SessionId};

const MYSTIC: ClassId = ClassId(5);
const MAGE: ClassId = ClassId(1);

/// The way of the swan model (spell 36): instant self-heal, kai 1, L2.
const SWAN: SpellId = SpellId(36);
/// The way of the owl model (spell 37): duration 60, MR +10, kai 2, L3.
const OWL: SpellId = SpellId(37);
/// The way of the cat model (spell 39): L4 — must NOT be granted at L2/L3.
const CAT: SpellId = SpellId(39);
/// A magery-group-1 (mage) spell at required_power 2: never kai-granted.
const FIZZ: SpellId = SpellId(10);

const START: RoomId = RoomId { map: 1, room: 1 };
const GUILD: RoomId = RoomId { map: 1, room: 2 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: START,
        name: "Village Entrance".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    let mut guild = Room {
        id: GUILD,
        name: "Adventurer's Guild".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    guild.shop = Some(ShopId(38));
    content.add_room(guild);
    // markup 0 reproduces the measured Newhaven Training Room costs:
    // (0+100)*level*5/100 = 5 SILVER at L1->2, 10 at L2->3, i.e. 50 and
    // 100 copper — paid from this fixture's copper-only purse as
    // "50 copper farthings" / "100 copper farthings" (§8.12 receipt).
    content.add_shop(Shop {
        id: ShopId(38),
        name: "Training Room".into(),
        shop_type: 8,
        min_level: 1,
        max_level: 10,
        markup: 0,
        class_limit: 0,
        stock: Default::default(),
    });
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 40,
            wisdom: 40,
            strength: 40,
            health: 40,
            agility: 40,
            charm: 40,
        },
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: MYSTIC,
        name: "Mystic".into(),
        abilities: vec![],
        hp_per_level: 4,
        hp_seed: 0, // deterministic hp_base across trains
        caster_group: 5,
        casting_factor: 3,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    content.add_class(Class {
        id: MAGE,
        name: "Mage".into(),
        abilities: vec![],
        hp_per_level: 2,
        hp_seed: 0,
        caster_group: 1,
        casting_factor: 3,
        exp_base: 0,
        combat_factor: 2,
        weapon_code: 8,
        armour_code: 9,
    });
    // The swan castmsgb model (real msg 8276): the invoke wording lives in
    // the MESSAGE DATA — caster line1 "You invoke the %s.", room line3.
    content.add_message(Message {
        id: MessageId(500),
        lines: vec![
            "You invoke the %s.".into(),
            String::new(),
            "%s invokes the %s!".into(),
        ],
    });
    // The owl castmsgb model (real msg 8277).
    content.add_message(Message {
        id: MessageId(501),
        lines: vec![
            "You invoke the %s.".into(),
            "%s invokes the %s on you!".into(),
            "%s invokes the %s on %s!".into(),
        ],
    });
    // The owl DescMsg model (real msg 8546): wear-off line1 (measured
    // exclamation mark), active line3.
    content.add_message(Message {
        id: MessageId(502),
        lines: vec![
            "The effects of way of the owl wear off!".into(),
            String::new(),
            "You feel strong-willed!".into(),
        ],
    });

    let swan = Spell {
        id: SWAN,
        name: "way of the swan".into(),
        short_name: "swan".into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(500)),
        abilities: vec![(Ability::Heal, 0)],
        level_cap: 0,
        round_cost: 0,
        required_power: 2,
        min_base: 3,
        max_base: 3,
        target_mode: mud_core::content::TargetMode::Benign,
        save_class: mud_core::content::SaveClass::None,
        base_chance: 200,
        duration_per_level: 0,
        match_type: mud_core::content::MatchType::Single0,
        duration: 0,
        element: mud_core::content::Element::Magic,
        class_gate_group: 5,
        mana_cost: 1,
        max_increase: mud_core::content::ScalePair::NONE,
        required_class_level: 0,
        min_increase: mud_core::content::ScalePair::NONE,
        duration_increase: mud_core::content::ScalePair::NONE,
        msg_style: 32, // even
    };
    let mut owl = swan.clone();
    owl.id = OWL;
    owl.name = "way of the owl".into();
    owl.short_name = "owl".into();
    owl.required_power = 3;
    owl.mana_cost = 2;
    owl.duration = 60;
    owl.min_base = 10;
    owl.max_base = 10;
    owl.cast_msg_b = Some(MessageId(501));
    owl.abilities = vec![(Ability::MR, 10), (Ability::DescMsg, 502)];
    let mut cat = swan.clone();
    cat.id = CAT;
    cat.name = "way of the cat".into();
    cat.short_name = "cat".into();
    cat.required_power = 4;
    cat.mana_cost = 3;
    let mut fizz = swan.clone();
    fizz.id = FIZZ;
    fizz.name = "fizz".into();
    fizz.short_name = "fizz".into();
    fizz.class_gate_group = 1;
    content.add_spell(swan);
    content.add_spell(owl);
    content.add_spell(cat);
    content.add_spell(fizz);
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: START,
        ..CoreConfig::default()
    }
}

fn player(name: &str, class: ClassId, level: u16, book: &[SpellId]) -> Player {
    let stats = StatBlock {
        intellect: 40,
        wisdom: 40,
        strength: 40,
        health: 40,
        agility: 40,
        charm: 40,
    };
    let mut spellbook = BTreeMap::new();
    for id in book {
        spellbook.insert(*id, false);
    }
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class,
        level,
        stats,
        base_stats: stats,
        hp_base: 0,
        current_hp: 10,
        current_mana: 0,
        hunger: 1000,
        thirst: 1000,
        coins: Coins {
            copper: 1000,
            ..Default::default()
        },
        lawful: false,
        inventory: vec![],
        weapon: None,
        bankbooks: vec![],
        worn: vec![],
        cp_unspent: 100,
        cp_lifetime: 100,
        lives: 9,
        experience: 0,
        location: START,
        spellbook,
        poison: 0,
        active_spells: Default::default(),
        ..Default::default()
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

// --- parsing ---

#[test]
fn powers_and_invoke_parse() {
    assert_eq!(parse("powers"), Command::Powers);
    // ORACLE-VERIFY: the minimum abbreviation is unmeasured; 2 assumed.
    assert_eq!(parse("po"), Command::Powers);
    assert_eq!(parse("invoke"), Command::Invoke(String::new()));
    assert_eq!(parse("invoke swan zzz"), Command::Invoke("swan zzz".into()));
    assert_eq!(parse("INVOKE swan"), Command::Invoke("swan".into()));
}

#[test]
fn invoke_has_no_abbreviation_and_inventory_keeps_i() {
    // MEASURED (§8.12): `in` and `inv` fall through to say.
    assert_eq!(parse("in"), Command::Unknown("in".into()));
    assert_eq!(parse("inv swan"), Command::Unknown("inv swan".into()));
    // ORACLE-VERIFY: invo/invok unmeasured; the no-abbreviation model
    // sends them to say as well.
    assert_eq!(parse("invo"), Command::Unknown("invo".into()));
    assert_eq!(parse("invok"), Command::Unknown("invok".into()));
    // `i` is still inventory (oracle).
    assert_eq!(parse("i"), Command::Inventory);
}

// --- command gating ---

#[test]
fn mystic_spells_is_redirected_to_powers() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 2, &[SWAN]));
    core.drain_events();
    core.input(s, "spells");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not list your spells. You are KAI! You must list your powers.\n"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("following spells"), "no listing: {shown:?}");
}

#[test]
fn mystic_cast_is_hard_refused_before_parsing() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 2, &[SWAN]));
    core.drain_events();
    // MEASURED (§8.12): bare, garbage, known power and full name all hit
    // the same refusal (double space after KAI!).
    for input in ["cast", "c zzz", "c swan", "cast way of the swan"] {
        core.input(s, input);
        let shown = text_to(&core.drain_events(), s);
        assert!(
            shown.contains("You may not cast... You are KAI!  You must invoke your powers.\n"),
            "{input}: {shown:?}"
        );
    }
}

#[test]
fn non_kai_powers_and_invoke_are_refused() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Vexil", MAGE, 2, &[FIZZ]));
    core.drain_events();
    // ORACLE-VERIFY: both refusals are unmeasured parallels of the kai
    // redirects.
    core.input(s, "powers");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not list your powers. You are not KAI! You must list your spells.\n"),
        "got: {shown:?}"
    );
    core.input(s, "invoke fizz");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not invoke... You are not KAI!  You must cast your spells.\n"),
        "got: {shown:?}"
    );
}

#[test]
fn bare_invoke_prints_the_syntax_line() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 1, &[]));
    core.drain_events();
    core.input(s, "invoke");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Syntax: INVOKE {power} [{target}]\n"),
        "got: {shown:?}"
    );
}

#[test]
fn invoke_unknown_power_keeps_the_cast_wording() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 2, &[SWAN]));
    core.drain_events();
    // MEASURED (§8.12): the unknown line still says "cast".
    for (input, echo) in [("invoke zzz", "zzz"), ("invoke owl", "owl")] {
        core.input(s, input);
        let shown = text_to(&core.drain_events(), s);
        assert!(
            shown.contains(&format!("You do not know how to cast {echo}.\n")),
            "{input}: {shown:?}"
        );
    }
}

#[test]
fn invoke_without_kai_is_refused() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 2, &[SWAN]));
    core.drain_events();
    for input in ["invoke swan", "invoke way of the swan"] {
        core.input(s, input);
        let shown = text_to(&core.drain_events(), s);
        assert!(
            shown.contains("You do not have enough kai to invoke that power.\n"),
            "{input}: {shown:?}"
        );
    }
    assert_eq!(core.current_mana(s), 0);
}

#[test]
fn invoke_success_line_heals_and_deducts_kai() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 2, &[SWAN]));
    core.set_current_mana(s, 1);
    core.drain_events();
    core.input(s, "invoke swan");
    let shown = text_to(&core.drain_events(), s);
    // MEASURED (§8.12): the castmsgb caster line IS the invoke wording
    // (message data), the heal lands silently, kai deducted at the prompt.
    assert!(
        shown.contains("You invoke the way of the swan.\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 0, "kai charged");
    // Magnitude bounds 3..3 roll 3..=4 (the min..max+1 genrdn quirk).
    let hp = core.current_hp(s);
    assert!((13..=14).contains(&hp), "silent heal, got hp {hp}");
    assert!(!shown.contains("healed"), "no heal line: {shown:?}");
}

#[test]
fn one_invoke_per_round_is_checked_before_the_kai_deduction() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 3, &[SWAN, OWL]));
    core.set_current_mana(s, 2);
    core.drain_events();
    core.input(s, "invoke swan");
    core.drain_events();
    assert_eq!(core.current_mana(s), 1);
    core.input(s, "invoke swan");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You have already invoked a power this round!\n"),
        "got: {shown:?}"
    );
    // MEASURED (§8.12): the round flag fires with kai still in the pool
    // and charges nothing.
    assert_eq!(core.current_mana(s), 1, "second invoke charged nothing");
}

#[test]
fn invoke_unmatched_target_charges_nothing() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 3, &[SWAN, OWL]));
    core.set_current_mana(s, 2);
    core.drain_events();
    for input in ["invoke swan zzz", "invoke way of the owl zzz"] {
        core.input(s, input);
        let shown = text_to(&core.drain_events(), s);
        assert!(
            shown.contains("You do not see zzz here!\n"),
            "{input}: {shown:?}"
        );
    }
    assert_eq!(core.current_mana(s), 2, "kai unchanged");
}

#[test]
fn invoke_duration_power_enters_a_slot_with_the_active_line() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 3, &[SWAN, OWL]));
    core.set_current_mana(s, 2);
    core.drain_events();
    core.input(s, "invoke owl");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You invoke the way of the owl.\n"),
        "got: {shown:?}"
    );
    assert!(shown.contains("You feel strong-willed!\n"), "got: {shown:?}");
    assert_eq!(core.current_mana(s), 0);
    let p = core.player_snapshot(s);
    assert_eq!(p.active_spells[0].spell, Some(OWL));
}

// --- the powers listing ---

#[test]
fn powers_with_an_empty_book() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 1, &[]));
    core.drain_events();
    core.input(s, "powers");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You have no powers.\n"), "got: {shown:?}");
}

#[test]
fn powers_lists_the_measured_table() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 3, &[SWAN, OWL]));
    core.drain_events();
    core.input(s, "powers");
    let shown = text_to(&core.drain_events(), s);
    // MEASURED (§8.12, byte-exact): header swaps Mana->Kai, the 4-char
    // short column is RIGHT-aligned, table ends with a blank line.
    let want = "You have the following powers:\n\
                Level Kai  Short Spell Name\n\
                \u{20} 2   1    swan  way of the swan               \n\
                \u{20} 3   2     owl  way of the owl                \n\
                \n";
    assert!(shown.contains(want), "got: {shown:?}");
}

// --- trainer grants ---

#[test]
fn mystic_train_grants_the_new_levels_kai_powers() {
    let mut core = Core::new(world(), config());
    let mut p = player("Kaimon", MYSTIC, 1, &[]);
    p.location = GUILD;
    p.experience = 2600; // covers L1->2 (1300) and L2->3 (2600)
    let s = core.attach_player(p);
    core.drain_events();

    core.input(s, "train");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    // MEASURED (§8.12): the full receipt, in order.
    let lines = [
        "You hand over 50 copper farthings and you receive training to attain level 2.\n",
        "You receive the following:\n",
        "10 additional character points\n",
        "You learn the following Kai abilities:\n",
        "way of the swan\n",
    ];
    let mut at = 0;
    for line in lines {
        let found = shown[at..].find(line);
        assert!(found.is_some(), "missing {line:?} in order, got: {shown:?}");
        at += found.unwrap();
    }
    let p = core.player_snapshot(s);
    assert_eq!(p.spellbook.get(&SWAN), Some(&false), "permanent book entry");
    assert!(!p.spellbook.contains_key(&OWL));
    assert!(!p.spellbook.contains_key(&CAT));
    assert!(!p.spellbook.contains_key(&FIZZ), "mage school not granted");
    // MEASURED (§8.12): training does NOT refill the pool.
    assert_eq!(p.current_mana, 0);
    // The grant persists.
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Persist(p) if p.spellbook.contains_key(&SWAN)
        )),
        "train persists the granted book"
    );

    core.input(s, "train");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You hand over 100 copper farthings and you receive training to attain level 3.\n"),
        "got: {shown:?}"
    );
    assert!(shown.contains("way of the owl\n"), "got: {shown:?}");
    assert!(!shown.contains("way of the cat"), "L4 power not granted at L3");
    let p = core.player_snapshot(s);
    assert_eq!(p.spellbook.get(&OWL), Some(&false));
}

#[test]
fn non_kai_train_grants_nothing() {
    // MEASURED (§8.1): a mage's train inserts no spells — the grant is
    // gated on caster_group 5. ORACLE-VERIFY: other classes unmeasured.
    let mut core = Core::new(world(), config());
    let mut p = player("Vexil", MAGE, 1, &[]);
    p.location = GUILD;
    p.experience = 1300;
    let s = core.attach_player(p);
    core.drain_events();
    core.input(s, "train");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("you receive training to attain level 2."),
        "got: {shown:?}"
    );
    assert!(
        !shown.contains("You learn the following Kai abilities:"),
        "got: {shown:?}"
    );
    assert!(core.player_snapshot(s).spellbook.is_empty());
}

// --- the pool surfaces ---

#[test]
fn prompt_hides_kai_at_level_1_and_shows_it_from_level_2() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 1, &[]));
    core.drain_events();
    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    // MEASURED (§8.12): the L1 prompt has NO kai segment (max kai 0).
    assert!(shown.ends_with("[HP=10]:"), "got: {shown:?}");

    let s2 = core.attach_player(player("Kaidan", MYSTIC, 2, &[]));
    core.drain_events();
    core.input(s2, "look");
    let shown = text_to(&core.drain_events(), s2);
    assert!(shown.ends_with("[HP=10/KAI=0]:"), "got: {shown:?}");
}

#[test]
fn health_gains_the_kai_clause_from_level_2() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 1, &[]));
    core.drain_events();
    core.input(s, "health");
    let shown = text_to(&core.drain_events(), s);
    assert!(!shown.contains("Kai:"), "L1 omits the clause: {shown:?}");

    let s2 = core.attach_player(player("Kaidan", MYSTIC, 2, &[]));
    core.drain_events();
    core.input(s2, "health");
    let shown = text_to(&core.drain_events(), s2);
    // MEASURED (§8.12): `Kai:   0/1   [0%]` — same numeric widths as the
    // mage's Mana clause (§8.2).
    assert!(shown.contains("  Kai:   0/1   [0%]"), "got: {shown:?}");
}

#[test]
fn mage_health_carries_the_mana_clause() {
    let mut core = Core::new(world(), config());
    let mut p = player("Vexil", MAGE, 1, &[]);
    p.current_mana = 12; // max = 6 + cf 3 * L1 * 2 = 12
    let s = core.attach_player(p);
    core.drain_events();
    core.input(s, "health");
    let shown = text_to(&core.drain_events(), s);
    // MEASURED (§8.2): `Mana:  12/12  [100%]`.
    assert!(shown.contains("  Mana:  12/12  [100%]"), "got: {shown:?}");
}

#[test]
fn stat_sheet_shows_the_kai_row() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 2, &[]));
    core.drain_events();
    core.input(s, "st");
    let shown = text_to(&core.drain_events(), s);
    // MEASURED (§8.12): the mana row fills the Traps row's left column;
    // the L1 sheet shows Kai: 0/0 (oracle_kai_mystic.raw).
    assert!(
        shown.contains("Kai:      0/1                          Traps:"),
        "got: {shown:?}"
    );
    let s2 = core.attach_player(player("Kaidan", MYSTIC, 1, &[]));
    core.drain_events();
    core.input(s2, "st");
    let shown = text_to(&core.drain_events(), s2);
    assert!(shown.contains("Kai:      0/0"), "got: {shown:?}");
}

// --- regen ---

#[test]
fn kai_regen_is_flat_one_per_slow_tick_capped_at_max() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 3, &[]));
    core.set_current_mana(s, 0);
    core.drain_events();
    for _ in 0..30 {
        core.tick();
    }
    assert_eq!(core.current_mana(s), 1, "flat +1 per slow tick");
    for _ in 0..30 {
        core.tick();
    }
    assert_eq!(core.current_mana(s), 2);
    for _ in 0..30 {
        core.tick();
    }
    assert_eq!(core.current_mana(s), 2, "capped at level - 1");
}

#[test]
fn level_1_mystic_never_regenerates_kai() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Kaimon", MYSTIC, 1, &[]));
    core.set_current_mana(s, 0);
    core.drain_events();
    for _ in 0..30 {
        core.tick();
    }
    assert_eq!(core.current_mana(s), 0, "max kai 0 at L1");
}
