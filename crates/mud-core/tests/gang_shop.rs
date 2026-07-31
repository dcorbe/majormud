//! The gang stock shop — type 0xb (gangs.md §3.1): STOCK / UNSTOCK /
//! MARKUP and the player-priced buy path with the bank-8 deposit.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, Shop, ShopId, ShopStock,
    StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const SHOPROOM: RoomId = RoomId { map: 15, room: 973 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: SHOPROOM,
        name: "Red Gang Shop".into(),
        room_type: 1,
        shop: Some(ShopId(136)),
        gang_house: 1,
        ..Default::default()
    });
    content.add_shop(Shop {
        id: ShopId(136),
        name: "Gang Shop #1".into(),
        shop_type: 11,
        min_level: 0,
        max_level: 0,
        markup: 0,
        class_limit: 0,
        stock: [ShopStock::default(); 20],
    });
    // The house-1 controller key (GShopItem 184 value 1).
    content.add_item(Item {
        id: ItemId(844),
        name: "red key".into(),
        weight: 1,
        item_type: 0,
        uses: -1,
        gettable: 1,
        abilities: vec![(Ability::from_id(184).unwrap(), 1)],
        ..Item::default()
    });
    content.add_item(Item {
        id: ItemId(68),
        name: "dagger".into(),
        weight: 35,
        item_type: 1,
        uses: -1,
        cost: 100,
        cost_denomination: 0,
        gettable: 1,
        ..Item::default()
    });
    content.add_item(Item {
        id: ItemId(70),
        name: "loyal blade".into(),
        weight: 35,
        item_type: 1,
        uses: -1,
        gettable: 1,
        abilities: vec![(Ability::from_id(100).unwrap(), 1)], // LoyalItem
        ..Item::default()
    });
    // Cost 0 → the Free row when stocked bare.
    content.add_item(Item {
        id: ItemId(71),
        name: "wooden bowl".into(),
        weight: 1,
        item_type: 0,
        uses: -1,
        cost: 0,
        gettable: 1,
        ..Item::default()
    });
    // MinLevel 99 → the "(You can't use)" suffix on the shelf row.
    content.add_item(Item {
        id: ItemId(72),
        name: "ancient relic".into(),
        weight: 1,
        item_type: 0,
        uses: -1,
        cost: 10,
        gettable: 1,
        abilities: vec![(Ability::MinLevel, 99)],
        ..Item::default()
    });
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 50, // (110 - 10) = 100% buy factor
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

fn person(name: &str) -> Player {
    let stats = StatBlock {
        intellect: 30,
        wisdom: 50,
        strength: 50,
        health: 50,
        agility: 30,
        charm: 50,
    };
    let mut coins = mud_core::game::Coins::default();
    coins.copper = 5000;
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 10,
        stats,
        base_stats: stats,
        current_hp: 40,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        coins,
        location: SHOPROOM,
        ..Default::default()
    }
}

fn texts(events: &[Event], who: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session, text } if *session == who => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn stocker(core: &mut Core) -> SessionId {
    let s = core.attach_player(person("Salad"));
    core.give_item(s, ItemId(844));
    core.give_item(s, ItemId(68));
    core.drain_events();
    s
}

#[test]
fn stock_needs_the_controller_key() {
    let mut core = Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = core.attach_player(person("NoKey"));
    core.give_item(s, ItemId(68));
    core.drain_events();
    core.input(s, "stock dagger");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You do not have the correct item to stock this shop."),
        "{out:?}"
    );
}

#[test]
fn stock_places_prices_and_records_the_stocker() {
    let mut core = Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    // Second observer sees the room line.
    let w = core.attach_player(person("Watcher"));
    core.drain_events();
    core.input(s, "stock dagger 25 silver");
    let events = core.drain_events();
    let out = texts(&events, s);
    assert!(out.contains("You add the dagger to your shops stock."), "{out:?}");
    assert!(
        texts(&events, w).contains("You see Salad add a dagger to the shops stock."),
        "{:?}", texts(&events, w)
    );
    // The dagger left the stocker's inventory.
    assert!(
        !core.player_snapshot(s).inventory.iter().any(|(i, _)| *i == ItemId(68)),
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistGangShop { shop, state }
            if shop.0 == 136 && state.last_stocker == "Salad"
            && state.slots[0].item == Some(ItemId(68))
            && state.slots[0].count == 1
            && state.slots[0].price == 25
            && state.slots[0].denom == 1)),
        "slot + stocker persisted: {events:?}"
    );
}

#[test]
fn stock_defaults_price_from_the_item_and_refuses_loyal() {
    let mut core = Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.input(s, "stock dagger");
    let events = core.drain_events();
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistGangShop { state, .. }
            if state.slots[0].price == 100 && state.slots[0].denom == 0)),
        "defaults ride the item cost fields"
    );
    core.give_item(s, ItemId(70));
    core.drain_events();
    core.input(s, "stock loyal blade");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("You may not stock that item!"), "{out:?}");
}

#[test]
fn unstock_returns_and_clears_sold_out_slots() {
    let mut core = Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.input(s, "stock dagger");
    core.drain_events();
    core.input(s, "unstock dagger");
    let events = core.drain_events();
    let out = texts(&events, s);
    assert!(out.contains("You remove dagger from the shops stock."), "{out:?}");
    assert!(
        core.player_snapshot(s).inventory.iter().any(|(i, _)| *i == ItemId(68)),
        "item back in inventory"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistGangShop { state, .. }
            if state.slots[0].item.is_none() && state.slots[0].price == 0)),
        "the emptied slot is cleared from the list"
    );
}

#[test]
fn markup_sets_the_gang_shop_percentage() {
    let mut core = Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.input(s, "markup 50");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("New gang shop markup value set to 50 percent."), "{out:?}");
}

#[test]
fn buy_pays_the_stocker_price_and_deposits_to_bank_8() {
    let mut core = Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.input(s, "stock dagger 30 copper");
    core.input(s, "markup 50");
    core.drain_events();
    let b = core.attach_player(person("Buyer"));
    core.drain_events();
    core.input(b, "buy dagger");
    let events = core.drain_events();
    let out = texts(&events, b);
    assert!(out.contains("You just bought"), "{out:?}");
    // price = (110 - 50/5) × (30 × 150 / 100) / 100 = 100 × 45 / 100 = 45.
    assert_eq!(core.player_snapshot(b).coins.copper, 5000 - 45);
    // The full price lands in the ONLINE stocker's bank-8 book.
    let stocker_row = core.player_snapshot(s);
    assert_eq!(
        stocker_row.bankbooks.iter().find(|(shop, _)| *shop == 8).map(|(_, c)| *c),
        Some(45),
        "deposit credited"
    );
    // Sellout clears the slot: a second buy no longer knows the item.
    core.input(b, "buy dagger");
    let out = texts(&core.drain_events(), b);
    assert!(out.contains("is not a known item"), "sold out and delisted: {out:?}");
}

// --- LIST rendering of the runtime shelves (gangs.md §3.1;
// display_shop_items' dedicated type-0xb branch, decompile 34841-34900):
// the same header strings as a regular shop, the 10 runtime slots,
// price = slot price × (markup+100)/100 in the SLOT's own denomination,
// zero price → the Free row, same can't-use suffix pair. NO Charm
// factor and NO >100000 quirk at LIST time — both are buy-side only
// (buy_item 14571+, mirrored in buy_from_gang_shop). ---

#[test]
fn list_renders_the_stocked_shelves() {
    let mut core =
        Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.input(s, "stock dagger 25 silver");
    core.drain_events();
    core.input(s, "list");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("The following items are for sale here:"),
        "shared header (004849fd): {out:?}"
    );
    let row = format!("{:<30}{:<10}{:>4} {}", "dagger", 1, 25, "silver nobles");
    assert!(out.contains(&row), "stocker-priced row in the slot denom: {out:?}");
}

#[test]
fn list_applies_the_gang_markup() {
    let mut core =
        Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.input(s, "stock dagger 30 copper");
    core.input(s, "markup 50");
    core.drain_events();
    core.input(s, "list");
    let out = texts(&core.drain_events(), s);
    let row = format!("{:<30}{:<10}{:>4} {}", "dagger", 1, 45, "copper farthings");
    assert!(out.contains(&row), "30 × 150 / 100 truncated: {out:?}");
}

#[test]
fn list_shows_free_for_zero_priced_stock() {
    let mut core =
        Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.give_item(s, ItemId(71));
    core.drain_events();
    core.input(s, "stock bowl");
    core.drain_events();
    core.input(s, "list");
    let out = texts(&core.drain_events(), s);
    let row = format!("{:<30}{:<10}   Free", "wooden bowl", 1);
    assert!(out.contains(&row), "the shared Free row (00484b35): {out:?}");
}

#[test]
fn sold_out_slots_are_hidden_and_an_empty_shop_lists_nothing() {
    let mut core =
        Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.input(s, "stock dagger 30 copper");
    core.drain_events();
    let b = core.attach_player(person("Buyer"));
    core.drain_events();
    core.input(b, "buy dagger");
    core.drain_events();
    core.input(b, "list");
    let out = texts(&core.drain_events(), b);
    assert!(
        !out.contains("for sale"),
        "sold out delists; a fully empty shop prints nothing: {out:?}"
    );
}

#[test]
fn unusable_stock_carries_the_cant_use_suffix() {
    let mut core =
        Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.give_item(s, ItemId(72));
    core.drain_events();
    core.input(s, "stock relic");
    core.drain_events();
    core.input(s, "list");
    let out = texts(&core.drain_events(), s);
    let row = format!("{:<30}{:<10}{:>4} {} (You can't use)", "ancient relic", 1, 10, "copper farthings");
    assert!(out.contains(&row), "same suffix pair as the regular branch: {out:?}");
}

#[test]
fn offline_stocker_deposit_routes_through_the_event() {
    let mut core = Core::new(world(), CoreConfig { start_location: SHOPROOM, ..CoreConfig::default() });
    let s = stocker(&mut core);
    core.input(s, "stock dagger 30 copper");
    core.drain_events();
    core.detach(s);
    core.drain_events();
    let b = core.attach_player(person("Buyer"));
    core.drain_events();
    core.input(b, "buy dagger");
    let events = core.drain_events();
    assert!(
        events.iter().any(|e| matches!(e, Event::DepositGangGold { stocker, copper }
            if stocker == "Salad" && *copper == 30)),
        "offline credit event: {events:?}"
    );
}
