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
