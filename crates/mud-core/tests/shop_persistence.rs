//! Tests for shop-shelf persistence and the daily cleanup restock.
//!
//! The original saved shop stock to Btrieve (dirty byte +0x1dc) and re-ran
//! `check_initiate_restocking` on every module load; Worldgroup's nightly
//! cleanup restarted the module, so interval-0 top-ups happened daily. A
//! standalone server that never restarts emulates that with a 24 h job.

use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, Shop, ShopId, ShopStock,
    StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

const DAY: u64 = 86_400;

fn world_with_slot(slot: ShopStock) -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Shop".into(),
        description: vec![],
        room_type: 1,
        shop: Some(ShopId(45)),
        placed_items: vec![],
        exits: Default::default(),
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

fn stock_events(events: &[Event]) -> Vec<(ShopId, [i16; 20])> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::PersistShopStock { shop, counts } => Some((*shop, *counts)),
            _ => None,
        })
        .collect()
}

fn ticks(core: &mut Core, n: u64) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..n {
        core.tick();
        all.append(&mut core.drain_events());
    }
    all
}

fn plain_slot() -> ShopStock {
    ShopStock {
        item: Some(ItemId(100)),
        max: 5,
        now: 5,
        restock_time: 0,
        restock_amount: 0,
        restock_percent: 0,
    }
}

#[test]
fn buying_emits_a_stock_persist_event() {
    let mut core = Core::new(world_with_slot(plain_slot()), config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    let persisted = stock_events(&core.drain_events());
    assert_eq!(persisted.len(), 1, "one persist event per change");
    assert_eq!(persisted[0].0, ShopId(45));
    assert_eq!(persisted[0].1[0], 4);
}

#[test]
fn selling_back_emits_a_stock_persist_event() {
    let mut core = Core::new(world_with_slot(plain_slot()), config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.drain_events();
    core.input(s, "sell quarterstaff");
    let persisted = stock_events(&core.drain_events());
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].1[0], 5, "restocked by the sale");
}

#[test]
fn restock_sweep_emits_persist_only_on_change() {
    let mut core = Core::new(
        world_with_slot(ShopStock {
            item: Some(ItemId(100)),
            max: 5,
            now: 5,
            restock_time: 1,
            restock_amount: 1,
            restock_percent: 100,
        }),
        config(),
    );
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.drain_events();
    // Sweeps until the shelf is full again emit persist events...
    let events = ticks(&mut core, 4 * 630);
    let persisted = stock_events(&events);
    assert!(!persisted.is_empty(), "sweep restock persists");
    assert_eq!(persisted.last().unwrap().1[0], 5);
    // ...and full-shelf sweeps stay silent.
    let events = ticks(&mut core, 630);
    assert!(stock_events(&events).is_empty(), "no change, no persist");
}

#[test]
fn restore_applies_saved_counts() {
    let mut core = Core::new(world_with_slot(plain_slot()), config());
    core.restore_shop_stock(&[(ShopId(45), 0, 2)]);
    let s = create(&mut core, "Dain");
    assert_eq!(quantity_shown(&mut core, s), 2);
}

#[test]
fn restore_ignores_unknown_shops_and_slots() {
    let mut core = Core::new(world_with_slot(plain_slot()), config());
    core.restore_shop_stock(&[(ShopId(999), 0, 2), (ShopId(45), 19, 3)]);
    let s = create(&mut core, "Dain");
    assert_eq!(quantity_shown(&mut core, s), 5, "slot 0 untouched");
}

#[test]
fn restore_clamps_overstock_down() {
    // check_initiate_restocking clamps current > max down (content patch
    // lowered a max between runs) and marks the shop dirty.
    let mut core = Core::new(world_with_slot(plain_slot()), config());
    core.restore_shop_stock(&[(ShopId(45), 0, 9)]);
    let persisted = stock_events(&core.drain_events());
    assert_eq!(persisted.len(), 1, "clamp-down persists");
    assert_eq!(persisted[0].1[0], 5);
    let s = create(&mut core, "Dain");
    assert_eq!(quantity_shown(&mut core, s), 5);
}

#[test]
fn restore_tops_up_interval_zero_slots() {
    // The boot reconciliation gives dented interval-0 slots one
    // probability-gated top-up (single roll, single amount).
    let mut core = Core::new(
        world_with_slot(ShopStock {
            item: Some(ItemId(100)),
            max: 5,
            now: 5,
            restock_time: 0,
            restock_amount: 2,
            restock_percent: 100,
        }),
        config(),
    );
    core.restore_shop_stock(&[(ShopId(45), 0, 1)]);
    let s = create(&mut core, "Dain");
    assert_eq!(quantity_shown(&mut core, s), 3, "1 + one amount of 2");
}

#[test]
fn daily_cleanup_tops_up_interval_zero_slots() {
    // Worldgroup nightly cleanup restarted the module and re-ran the
    // interval-0 top-up; the standalone server emulates it every 24 h.
    let mut core = Core::new(
        world_with_slot(ShopStock {
            item: Some(ItemId(100)),
            max: 5,
            now: 5,
            restock_time: 0,
            restock_amount: 1,
            restock_percent: 100,
        }),
        config(),
    );
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.drain_events();

    ticks(&mut core, DAY - 10);
    assert_eq!(quantity_shown(&mut core, s), 4, "not before the day mark");

    ticks(&mut core, 20);
    assert_eq!(quantity_shown(&mut core, s), 5, "cleanup topped up");
}
