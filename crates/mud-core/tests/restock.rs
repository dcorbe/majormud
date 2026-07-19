//! Tests for shop restock timers (`check_initiate_restocking` 0x5b58c,
//! `restock_items` 0x5b6f6, economy.md §4).
//!
//! Boot fills every shelf to max (user testimony: on the first run of the
//! world everything is available; the extracted `shopnow` values are
//! played-board runtime state). Timed slots get an event scheduled at
//! random(1, max(2, interval)) minutes; `background_slow` runs the sweep on
//! every 21st slow tick (630 s), and each due event rolls 1-100 < percent
//! to add `amount` (clamped to max) before rescheduling +interval minutes.

use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, Shop, ShopId, ShopStock,
    StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

/// One sweep window: 21 slow ticks of 30 s.
const SWEEP: u64 = 630;

fn world_with_slot(slot: ShopStock) -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Shop".into(),
        description: vec![],
        room_type: 1,
        attributes: 0,
        shop: Some(ShopId(45)),
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    content.add_item(Item {
        id: ItemId(100),
        name: "quarterstaff".into(),
        weight: 100,
        item_type: 1,
        uses: -1,
        min_damage: 2,
        max_damage: 12,
        weapon_type: 1,
        gettable: 1,
        speed: 1200,
        ..Item::default()
    });
    let mut stock = [ShopStock::default(); 20];
    stock[0] = slot;
    content.add_shop(Shop {
        id: ShopId(45),
        name: "Weapons Shop".into(),
        shop_type: 1,
        min_level: 0,
        max_level: 0,
        markup: 50,
        class_limit: 0,
        stock,
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
            charm: 50,
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

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
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

fn quantity_shown(core: &mut Core, s: SessionId) -> i16 {
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    let line = shown
        .lines()
        .find(|l| l.starts_with("quarterstaff"))
        .unwrap_or_else(|| panic!("no stock row in: {shown:?}"))
        .to_string();
    line.split_whitespace()
        .nth(1)
        .and_then(|q| q.parse().ok())
        .unwrap_or_else(|| panic!("no quantity in: {line:?}"))
}

fn ticks(core: &mut Core, n: u64) {
    for _ in 0..n {
        core.tick();
    }
    core.drain_events();
}

#[test]
fn boot_fills_shelves_to_max() {
    // Extracted shopnow is played-board state; a fresh world stocks full.
    let content = world_with_slot(ShopStock {
        item: Some(ItemId(100)),
        max: 5,
        now: 0,
        restock_time: 0,
        restock_amount: 0,
        restock_percent: 0,
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    assert_eq!(quantity_shown(&mut core, s), 5);
}

#[test]
fn timed_slot_restocks_on_the_sweep() {
    // Interval 1 min, +1 per event, percent 100 (fails only on a roll of
    // exactly 100; consecutive sweeps make the test seed-proof).
    let content = world_with_slot(ShopStock {
        item: Some(ItemId(100)),
        max: 5,
        now: 5,
        restock_time: 1,
        restock_amount: 1,
        restock_percent: 100,
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.input(s, "buy quarterstaff");
    core.drain_events();
    assert_eq!(quantity_shown(&mut core, s), 3);

    // The event comes due within max(2, interval) = 2 minutes, but nothing
    // moves until the 21st slow tick fires the sweep.
    ticks(&mut core, SWEEP - 30);
    assert_eq!(quantity_shown(&mut core, s), 3, "before the sweep tick");

    ticks(&mut core, 4 * SWEEP);
    assert_eq!(quantity_shown(&mut core, s), 5, "restocked and clamped");
}

#[test]
fn restock_amount_clamps_at_max() {
    let content = world_with_slot(ShopStock {
        item: Some(ItemId(100)),
        max: 5,
        now: 5,
        restock_time: 1,
        restock_amount: 50,
        restock_percent: 100,
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.drain_events();
    ticks(&mut core, 3 * SWEEP);
    assert_eq!(quantity_shown(&mut core, s), 5);
}

#[test]
fn zero_percent_never_restocks() {
    // genrdn(1,100) < 0 is impossible — the event fires and reschedules
    // but never adds stock.
    let content = world_with_slot(ShopStock {
        item: Some(ItemId(100)),
        max: 5,
        now: 5,
        restock_time: 1,
        restock_amount: 1,
        restock_percent: 0,
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.drain_events();
    ticks(&mut core, 5 * SWEEP);
    assert_eq!(quantity_shown(&mut core, s), 4);
}

#[test]
fn interval_zero_slots_do_not_restock_at_runtime() {
    // Interval-0 top-up happens only in check_initiate_restocking (boot) —
    // restock_items only drains scheduled events.
    let content = world_with_slot(ShopStock {
        item: Some(ItemId(100)),
        max: 5,
        now: 5,
        restock_time: 0,
        restock_amount: 1,
        restock_percent: 100,
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.drain_events();
    ticks(&mut core, 5 * SWEEP);
    assert_eq!(quantity_shown(&mut core, s), 4);
}

#[test]
fn gang_shops_are_skipped() {
    // check_initiate_restocking bails on shop type 11 (gang houses).
    let mut content = world_with_slot(ShopStock {
        item: Some(ItemId(100)),
        max: 5,
        now: 5,
        restock_time: 1,
        restock_amount: 1,
        restock_percent: 100,
    });
    let shop = content.shops.get_mut(&ShopId(45)).unwrap();
    shop.shop_type = 11;
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.drain_events();
    ticks(&mut core, 5 * SWEEP);
    assert_eq!(quantity_shown(&mut core, s), 4);
}
