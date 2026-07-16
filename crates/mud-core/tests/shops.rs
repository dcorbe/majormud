//! Tests for shop list/buy/sell (economy.md §2/§3; list format from
//! oracle_m4_items.raw).

use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, Shop, ShopId, ShopStock,
    StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    let mut shop_room = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Weapons Shop".into(),
        description: vec![],
        shop: Some(ShopId(45)),
        placed_items: vec![],
        exits: Default::default(),
    };
    shop_room.exits[mud_core::content::Direction::South as usize] =
        Some(mud_core::content::Exit {
            dest: RoomId { map: 1, room: 2 },
            exit_type: 0,
        });
    content.add_room(shop_room);
    content.add_room(Room {
        id: RoomId { map: 1, room: 2 },
        name: "Outside".into(),
        description: vec![],
        shop: None,
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
    content.add_item(Item {
        id: ItemId(68),
        name: "dagger".into(),
        weight: 35,
        item_type: 1,
        uses: -1,
        cost: 100, // 100 copper base for easy math
        cost_denomination: 0,
        gettable: 1,
        speed: 900,
        ..Item::default()
    });
    let mut stock = [ShopStock::default(); 20];
    stock[0] = ShopStock {
        item: Some(ItemId(100)),
        max: 31,
        now: 31,
        ..ShopStock::default()
    };
    stock[1] = ShopStock {
        item: Some(ItemId(68)),
        max: 5,
        now: 5,
        ..ShopStock::default()
    };
    content.add_shop(Shop {
        id: ShopId(45),
        name: "Weapons Shop".into(),
        shop_type: 1,
        min_level: 0,
        max_level: 0,
        markup: 50, // dagger shelf price 150 copper
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
            charm: 50, // neutral haggle: (110-10)/100 = 1.0
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

#[test]
fn list_matches_the_oracle_format() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("The following items are for sale here:\n\nItem                          Quantity    Price\n------------------------------------------------------\n"),
        "header: {shown:?}"
    );
    assert!(
        shown.contains("quarterstaff                  31           Free\n"),
        "free row: {shown:?}"
    );
    // Shelf price = 100 * 150/100 = 150 copper, no Charm factor.
    assert!(
        shown.contains("dagger                        5            150 copper farthings\n"),
        "ORACLE-VERIFY price rendering: {shown:?}"
    );
}

#[test]
fn buying_free_matches_the_oracle() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You just bought quarterstaff for nothing."),
        "got: {shown:?}"
    );
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You are carrying quarterstaff\n"), "got: {shown:?}");
}

#[test]
fn buying_decrements_stock() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy quarterstaff");
    core.drain_events();
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("quarterstaff                  30           Free\n"),
        "stock 30: {shown:?}"
    );
}

#[test]
fn buy_price_includes_markup_and_charm() {
    // Charm 50 neutral: price = 100 * 1.5 * 1.0 = 150 copper.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_copper(s, 200);
    core.input(s, "buy dagger");
    core.drain_events();
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Wealth: 50 copper farthings"),
        "paid 150: {shown:?}"
    );
}

#[test]
fn cannot_afford_is_reported() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy dagger");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You cannot afford dagger."),
        "ORACLE-VERIFY wording: {shown:?}"
    );
}

#[test]
fn unknown_item_matches_the_oracle() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "buy sword");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("sword is not a known item."),
        "got: {shown:?}"
    );
}

#[test]
fn selling_pays_charm_scaled_and_restocks() {
    // Charm 50: sellback = 100 * (25+25)/100 = 50 copper.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(68));
    core.input(s, "sell dagger");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You sold dagger for"),
        "ORACLE-VERIFY wording: {shown:?}"
    );
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Wealth: 50 copper farthings"),
        "sellback 50: {shown:?}"
    );
}

#[test]
fn selling_something_the_shop_does_not_stock_is_refused() {
    let mut content = world();
    content.add_item(Item {
        id: ItemId(200),
        name: "iron helmet".into(),
        item_type: 0,
        cost: 100,
        gettable: 1,
        ..Item::default()
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(200));
    core.input(s, "sell helmet");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You cannot sell iron helmet here."),
        "got: {shown:?}"
    );
}

#[test]
fn shop_commands_outside_a_shop_fall_through_to_say() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "s");
    core.drain_events();
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You say \"list\""),
        "ORACLE-VERIFY outside-shop behavior: {shown:?}"
    );
}
