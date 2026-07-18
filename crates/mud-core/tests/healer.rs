//! Tests for healer services (shop type 5) and the room shop-active gate
//! (oracle_healer2.raw / oracle_healer_gates.raw).
//!
//! `buy healing` costs (max−cur)×2 copper and heals to full; `buy curing`
//! when not poisoned costs 15 SILVER (the constant sits in the silver arg
//! of check_currency — live: 150 copper). Shops only operate in rooms with
//! type 1 (`room+0x43c`): the Silvermere Temple Healer (room type 3,
//! shopnum 4) refuses LIST and lets `buy healing` fall through to say.

use mud_core::content::{
    Class, ClassId, Content, Race, RaceId, Room, RoomId, Shop, ShopId, StatBlock,
};
use mud_core::game::{AccountProfile, Coins, Core, CoreConfig, Event, Gender, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 2190 },
        name: "Newhaven, Healer".into(),
        description: vec![],
        room_type: 1,
        attributes: 0,
        shop: Some(ShopId(4)),
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_room(Room {
        id: RoomId { map: 1, room: 527 },
        name: "Temple Healer".into(),
        description: vec![],
        room_type: 3, // healer NPC room, NOT shop-active
        attributes: 0,
        shop: Some(ShopId(4)),
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_shop(Shop {
        id: ShopId(4),
        name: "Temple".into(),
        shop_type: 5, // healer
        min_level: 0,
        max_level: 0,
        markup: 0,
        class_limit: 0,
        stock: Default::default(),
    });
    content.add_race(Race {
        id: RaceId(2),
        name: "Dwarf".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 30,
        },
        max_stats: StatBlock::default(),
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

/// A benign fixture spell (for the poisoned-cure slot sweep).
fn test_spell(id: mud_core::content::SpellId, name: &str) -> mud_core::content::Spell {
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, Spell, TargetMode};
    Spell {
        id,
        name: name.into(),
        short_name: name.chars().take(4).collect(),
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
        msg_style: 0,
    }
}

fn config(room: u16) -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room },
        ..CoreConfig::default()
    }
}

fn create(core: &mut Core, name: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.input(s, "No");
    core.drain_events();
    s
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
fn buy_healing_charges_twice_the_missing_hp_and_heals() {
    let mut core = Core::new(world(), config(2190));
    let s = create(&mut core, "Dain");
    core.give_copper(s, 100);
    core.set_current_hp(s, 31); // Dwarf Warrior max 35 -> 4 missing
    core.input(s, "buy healing");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You hand over 8 copper farthings and all your wounds are healed."),
        "got: {shown:?}"
    );
    let p = core.player_snapshot(s);
    assert_eq!(p.current_hp, 35);
    assert_eq!(p.coins.copper, 92);
}

#[test]
fn buy_healing_at_full_hp_hands_over_nothing() {
    let mut core = Core::new(world(), config(2190));
    let s = create(&mut core, "Dain");
    core.input(s, "buy healing");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You hand over nothing and all your wounds are healed."),
        "got: {shown:?}"
    );
}

#[test]
fn buy_curing_unpoisoned_costs_fifteen_silver() {
    let mut core = Core::new(world(), config(2190));
    let s = create(&mut core, "Dain");
    core.set_coins(
        s,
        Coins { runic: 0, platinum: 0, gold: 1, silver: 4, copper: 10 },
    );
    core.input(s, "buy curing");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains(
            "You hand over 1 gold crown, 4 silver nobles, 10 copper farthings and find that you were not poisoned!"
        ),
        "got: {shown:?}"
    );
    assert_eq!(core.player_snapshot(s).coins, Coins::default());

    // "buy cure poison" is the same service.
    core.give_copper(s, 150);
    core.input(s, "buy cure poison");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("were not poisoned!"), "got: {shown:?}");
}

#[test]
fn buy_curing_poisoned_costs_twenty_five_silver_and_cures() {
    // Poisoned path (decompile buy_item 14295-14329): check_currency with
    // 0x19 = 25 in the SAME silver arg as the not-poisoned 0xf = 15
    // (economy.md addendum) -> 250 copper; "and your poisoning is cured."
    // (DLL 0xbd28d — note the trailing PERIOD, unlike the not-poisoned
    // bang), counter cleared.
    let mut core = Core::new(world(), config(2190));
    let s = create(&mut core, "Dain");
    core.set_poison(s, 4);
    core.set_coins(
        s,
        Coins { runic: 0, platinum: 0, gold: 2, silver: 5, copper: 0 },
    );
    core.input(s, "buy curing");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains(
            "You hand over 2 gold crowns, 5 silver nobles and your poisoning is cured."
        ),
        "got: {shown:?}"
    );
    assert_eq!(core.poison(s), 0);
    assert_eq!(core.player_snapshot(s).coins, Coins::default());

    // No longer poisoned: the next purchase takes the 15-silver path.
    core.give_copper(s, 150);
    core.input(s, "buy cure poison");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("were not poisoned!"), "got: {shown:?}");
}

#[test]
fn poisoned_cure_terminates_poison_carrying_spell_slots() {
    // After clearing the counter the DLL sweeps the active slots and
    // terminates every spell carrying Poison(19) with its stored value,
    // chain honored (14306-14327); other slots survive.
    use mud_core::ability::Ability;
    use mud_core::content::SpellId;
    use mud_core::game::ActiveSpell;

    let mut world = world();
    let mut venom = test_spell(SpellId(700), "venom touch");
    venom.duration = 50;
    venom.abilities = vec![(Ability::Poison, 5)];
    let mut ward = test_spell(SpellId(710), "stone ward");
    ward.duration = 50;
    ward.abilities = vec![(Ability::AC, 2)];
    world.add_spell(venom);
    world.add_spell(ward);

    let mut core = Core::new(world, config(2190));
    let s = create(&mut core, "Dain");
    core.set_poison(s, 9);
    core.give_copper(s, 250);
    core.set_active_spell(s, 0, ActiveSpell { spell: Some(SpellId(700)), value: 5, remaining: 40 });
    core.set_active_spell(s, 1, ActiveSpell { spell: Some(SpellId(710)), value: 2, remaining: 40 });
    core.input(s, "buy curing");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("your poisoning is cured."), "got: {shown:?}");
    let p = core.player_snapshot(s);
    assert!(p.active_spells[0].spell.is_none(), "poison spell terminated");
    assert_eq!(p.active_spells[1].spell, Some(SpellId(710)), "other slots survive");
    assert_eq!(p.poison, 0, "counter cleared; the termination subtract floors at 0");
}

#[test]
fn healer_list_is_silent() {
    // The list header prints lazily per stocked row; a healer has none.
    let mut core = Core::new(world(), config(2190));
    let s = create(&mut core, "Dain");
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        !shown.contains("for sale") && !shown.contains("cannot LIST"),
        "silent: {shown:?}"
    );
}

#[test]
fn non_shop_rooms_refuse_list_and_buy_says() {
    // Temple Healer: room type 3 — the shopnum is inert
    // (oracle_healer_gates.raw).
    let mut core = Core::new(world(), config(527));
    let s = create(&mut core, "Dain");
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You cannot LIST if you are not in a shop!"),
        "got: {shown:?}"
    );
    core.input(s, "buy healing");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You say \"buy healing\""),
        "buy falls to say: {shown:?}"
    );
}
