//! The Realm Deed Shop — buy_item's type-0xc branch (gangs.md §3.1,
//! decompile 14394-14466) — and the gang-shop sell surfaces.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, Shop, ShopId, ShopStock,
    StatBlock,
};
use mud_core::gang::{Gang, GANG_SATURATED, GF_PAPERWORK};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const STORE: RoomId = RoomId { map: 15, room: 732 };
const DEED_PRICE_COPPER: u32 = 150; // 100 base × markup 50 (neutral charm)

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: STORE,
        name: "Realm Deed Shop".into(),
        room_type: 1,
        shop: Some(ShopId(124)),
        ..Default::default()
    });
    content.add_item(Item {
        id: ItemId(1008),
        name: "red parchment deed".into(),
        weight: 1,
        item_type: 0,
        uses: -1,
        cost: 100,
        cost_denomination: 0,
        gettable: 1,
        abilities: vec![(Ability::from_id(181).unwrap(), 1)], // GHouseDeed tier 1
        ..Item::default()
    });
    let mut stock = [ShopStock::default(); 20];
    stock[0] = ShopStock {
        item: Some(ItemId(1008)),
        max: 3,
        now: 3,
        ..ShopStock::default()
    };
    content.add_shop(Shop {
        id: ShopId(124),
        name: "Realm Deed Shop".into(),
        shop_type: 12,
        min_level: 0,
        max_level: 0,
        markup: 50,
        class_limit: 0,
        stock,
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

fn person(name: &str, gang: &str) -> Player {
    let stats = StatBlock {
        intellect: 30,
        wisdom: 50,
        strength: 50,
        health: 50,
        agility: 30,
        charm: 50,
    };
    let mut coins = mud_core::game::Coins::default();
    coins.copper = 1000;
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
        location: STORE,
        gang: gang.into(),
        ..Default::default()
    }
}

fn core_with_gang(pool: u32, saturated: bool) -> Core {
    let mut gang = Gang::new("Iron Fist", "Salad", 0);
    gang.exp_pool = pool;
    if saturated {
        gang.flags |= GANG_SATURATED;
    }
    let config = CoreConfig {
        restored_gangs: vec![gang],
        restored_gang_members: vec![("Salad".into(), "Iron Fist".into(), 0)],
        start_location: STORE,
        ..CoreConfig::default()
    };
    Core::new(world(), config)
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

#[test]
fn deed_needs_a_gang_leader() {
    let mut core = core_with_gang(20_000_000, false);
    // A member who is not the leader, and a gangless stranger.
    let mut member = person("Torgo", "Iron Fist");
    member.gang_flags = 0;
    let s = core.attach_player(member);
    core.drain_events();
    core.input(s, "buy deed");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You must be a gang leader to purchase a gang house deed."),
        "{out:?}"
    );
    let s2 = core.attach_player(person("Loner", ""));
    core.drain_events();
    core.input(s2, "buy deed");
    let out = texts(&core.drain_events(), s2);
    assert!(
        out.contains("You must be a gang leader to purchase a gang house deed."),
        "{out:?}"
    );
}

#[test]
fn deed_pool_gate_and_success_without_debit() {
    // Pool short (default GANGEXP 1000 ×10000 = 10,000,000).
    let mut core = core_with_gang(9_999_999, false);
    let s = core.attach_player(person("Salad", "Iron Fist"));
    core.drain_events();
    core.input(s, "buy deed");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("Your gang does not have enough experience for you to purchase a gang house now."),
        "{out:?}"
    );

    // Pool sufficient: a normal purchase — and the pool is NOT debited
    // (the threshold is a gate, not a price; decompile 14416-14437 only
    // reads it).
    let mut core = core_with_gang(10_000_000, false);
    let s = core.attach_player(person("Salad", "Iron Fist"));
    core.drain_events();
    core.input(s, "buy deed");
    let events = core.drain_events();
    let out = texts(&events, s);
    assert!(out.contains("You just bought"), "{out:?}");
    assert_eq!(core.gang("Iron Fist").unwrap().exp_pool, 10_000_000, "no debit");
    let p = core.player_snapshot(s);
    assert_eq!(p.coins.copper, 1000 - DEED_PRICE_COPPER, "paid the shelf price");
    assert!(
        p.inventory.iter().any(|(i, _)| *i == ItemId(1008)),
        "carrying the deed"
    );
}

#[test]
fn saturated_pool_auto_passes() {
    let mut core = core_with_gang(0, true);
    let s = core.attach_player(person("Salad", "Iron Fist"));
    core.drain_events();
    core.input(s, "buy deed");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("You just bought"), "{out:?}");
}

#[test]
fn owner_hits_the_dead_code_4_clobber() {
    // The already-owner scan sets code 4, but the pool check clobbers
    // it to 5 unconditionally (decompile 14435: `|| local_21 == 4`) —
    // so an owner sees the EXP refusal and the owner string is dead
    // code in WG3-NT. ORACLE-VERIFY: a live capture could overturn.
    let mut core = core_with_gang(20_000_000, false);
    let s = core.attach_player(person("Salad", "Iron Fist"));
    core.drain_events();
    core.input(s, "buy deed");
    core.drain_events();
    core.input(s, "buy deed");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("Your gang does not have enough experience for you to purchase a gang house now."),
        "{out:?}"
    );
    assert!(!out.contains("already the owner"), "{out:?}");
}

#[test]
fn paperwork_overrides_and_is_set_by_selling_back() {
    let mut core = core_with_gang(20_000_000, false);
    let mut leader = person("Salad", "Iron Fist");
    leader.gang_flags = GF_PAPERWORK;
    let s = core.attach_player(leader);
    core.drain_events();
    core.input(s, "buy deed");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("Due to outstanding paper-work we are unable to provide you with another"),
        "{out:?}"
    );
    assert!(out.contains("property today. Please call back tomorrow!"), "{out:?}");

    // Selling a stocked item back to the deed shop files the paperwork
    // (the 0x4000 setter, decompile 14740).
    let mut core = core_with_gang(20_000_000, false);
    let s = core.attach_player(person("Salad", "Iron Fist"));
    core.drain_events();
    core.input(s, "buy deed");
    core.drain_events();
    core.input(s, "sell deed");
    let events = core.drain_events();
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p)
            if p.gang_flags & GF_PAPERWORK != 0)),
        "paperwork persisted"
    );
    core.input(s, "buy deed");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Due to outstanding paper-work"), "{out:?}");
}
