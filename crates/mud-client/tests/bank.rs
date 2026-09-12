//! The bank list, the nearest-bank search and the deposit gate, pure:
//! hand-built content and graphs, no board.

use std::sync::Arc;

use mud_client::bank::{
    BankConfig, BankGate, DepositReply, Judgement, Reading, WeightClass, bank_rooms,
    choose_bank, deposit_reply, nearest_bank, weight_class,
};
use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::purse::{Coins, Purse};
use mud_client::sheet::Inventory;
use mud_core::content::{Content, Direction, Room, RoomId, Shop, ShopId, ShopStock};

const HOME: RoomId = RoomId { map: 1, room: 1 };
const TOLL_BANK: RoomId = RoomId { map: 1, room: 2 };
const STREET_A: RoomId = RoomId { map: 1, room: 3 };
const STREET_B: RoomId = RoomId { map: 1, room: 4 };
const FAR_BANK: RoomId = RoomId { map: 1, room: 5 };
const VAULT: RoomId = RoomId { map: 1, room: 6 };
const TRAINER: RoomId = RoomId { map: 1, room: 7 };

fn shop(id: u16, name: &str, shop_type: i16) -> Shop {
    Shop {
        id: ShopId(id),
        name: name.into(),
        shop_type,
        min_level: 0,
        max_level: 0,
        markup: 0,
        class_limit: 0,
        stock: [ShopStock::default(); 20],
    }
}

fn room(id: RoomId, name: &str, room_type: i16, shop: Option<u16>) -> Room {
    Room {
        id,
        name: name.into(),
        room_type,
        shop: shop.map(ShopId),
        ..Default::default()
    }
}

/// Two banks, a vault behind one of them, and a trainer. The vault has
/// the bank's shop number but is not shop-active. The trainer is shop
/// type 8.
fn content() -> Content {
    let mut c = Content::default();
    c.add_shop(shop(8, "Bank of Godfrey", 7));
    c.add_shop(shop(83, "Rhudaur Bank", 7));
    c.add_shop(shop(9, "Warriors Guild", 8));
    c.add_room(room(HOME, "Home", 0, None));
    c.add_room(room(TOLL_BANK, "Bank of Rhudaur", 1, Some(83)));
    c.add_room(room(STREET_A, "Street", 0, None));
    c.add_room(room(STREET_B, "Street", 0, None));
    c.add_room(room(FAR_BANK, "Bank of Godfrey", 1, Some(8)));
    c.add_room(room(VAULT, "Bank Vault", 0, Some(8)));
    c.add_room(room(TRAINER, "Warriors Guild", 1, Some(9)));
    c
}

fn edge(dest: RoomId) -> ExitEdge {
    ExitEdge {
        dest,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    }
}

/// HOME east through a 5 gold toll into TOLL_BANK, one hop. HOME north
/// along two streets to FAR_BANK, three hops. Every edge has its way
/// back.
fn graph() -> Arc<RoomGraph> {
    let mut home = GraphRoom {
        name: "Home".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    home.exits[Direction::East as usize] = Some(ExitEdge {
        dest: TOLL_BANK,
        exit_type: 4,
        command: None,
        requirement: ExitRequirement::Toll { gold: 5 },
    });
    home.exits[Direction::North as usize] = Some(edge(STREET_A));
    let mut toll_bank = GraphRoom {
        name: "Bank of Rhudaur".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    toll_bank.exits[Direction::West as usize] = Some(edge(HOME));
    let mut a = GraphRoom {
        name: "Street".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    a.exits[Direction::South as usize] = Some(edge(HOME));
    a.exits[Direction::North as usize] = Some(edge(STREET_B));
    let mut b = GraphRoom {
        name: "Street".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    b.exits[Direction::South as usize] = Some(edge(STREET_A));
    b.exits[Direction::North as usize] = Some(edge(FAR_BANK));
    let mut far = GraphRoom {
        name: "Bank of Godfrey".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    far.exits[Direction::South as usize] = Some(edge(STREET_B));
    Arc::new(RoomGraph::from_rooms(vec![
        (HOME, home),
        (TOLL_BANK, toll_bank),
        (STREET_A, a),
        (STREET_B, b),
        (FAR_BANK, far),
    ]))
}

fn caps_with(purse: Purse) -> Capabilities {
    Capabilities {
        purse,
        ..Capabilities::unrestricted()
    }
}

/// A bank is a shop-active room whose shop is type 7. The vault carries
/// the bank's number and is not one. The trainer is shop-active and is
/// not one.
#[test]
fn bank_rooms_are_the_shop_active_type_7_rooms() {
    let banks = bank_rooms(&content());
    assert_eq!(
        banks,
        vec![
            (TOLL_BANK, "Rhudaur Bank".to_string()),
            (FAR_BANK, "Bank of Godfrey".to_string()),
        ]
    );
}

/// Rhudaur is one hop behind a 5 gold toll. With the toll in the purse
/// it is the nearest. Without it the toll is a wall and the three-hop
/// bank wins.
#[test]
fn the_nearest_bank_is_priced_against_the_purse() {
    let graph = graph();
    let content = content();
    assert_eq!(
        nearest_bank(&graph, &content, HOME, &caps_with(Purse::from_gold(5))),
        Some(TOLL_BANK)
    );
    assert_eq!(
        nearest_bank(&graph, &content, HOME, &caps_with(Purse::ZERO)),
        Some(FAR_BANK)
    );
    assert_eq!(
        nearest_bank(&graph, &content, VAULT, &caps_with(Purse::ZERO)),
        None,
        "a room the graph does not hold reaches nothing"
    );
}

/// A configured bank wins over a nearer one. A configured room that is
/// not a bank is refused, naming the banks there are.
#[test]
fn a_configured_bank_wins_and_a_non_bank_is_refused() {
    let graph = graph();
    let content = content();
    let caps = caps_with(Purse::from_gold(5));
    let fixed = BankConfig {
        at: Some("1/5".into()),
        ..BankConfig::default()
    };
    assert_eq!(
        choose_bank(&fixed, &graph, &content, HOME, &caps),
        Ok(Some(FAR_BANK))
    );
    let nearest = BankConfig::default();
    assert_eq!(
        choose_bank(&nearest, &graph, &content, HOME, &caps),
        Ok(Some(TOLL_BANK))
    );
    let wrong = BankConfig {
        at: Some("1/7".into()),
        ..BankConfig::default()
    };
    let err = choose_bank(&wrong, &graph, &content, HOME, &caps).expect_err("a trainer");
    assert!(err.contains("1/7"), "{err}");
    assert!(err.contains("1/2 Rhudaur Bank"), "{err}");
    assert!(err.contains("1/5 Bank of Godfrey"), "{err}");
}

/// The boundaries are mud-core's `encumbrance_descriptor`: None below
/// 33 percent, Light below 66, Medium below 100, Heavy from there.
#[test]
fn the_weight_classes_sit_on_the_core_boundaries() {
    assert_eq!(weight_class(32, 100), WeightClass::None);
    assert_eq!(weight_class(33, 100), WeightClass::Light);
    assert_eq!(weight_class(65, 100), WeightClass::Light);
    assert_eq!(weight_class(66, 100), WeightClass::Medium);
    assert_eq!(weight_class(99, 100), WeightClass::Medium);
    assert_eq!(weight_class(100, 100), WeightClass::Heavy);
    assert_eq!(weight_class(960, 2880), WeightClass::Light);
    assert_eq!(weight_class(1, 0), WeightClass::Heavy, "no capacity is full");
    assert!(WeightClass::None < WeightClass::Light);
    assert!(WeightClass::Medium < WeightClass::Heavy);
}

fn reading(counts: [u32; 5], carried: i64) -> Reading {
    Reading {
        coins: Coins { counts },
        class: weight_class(carried, 2400),
    }
}

/// A reading is the coins and the class off one inventory reply. No
/// encumbrance line, no reading.
#[test]
fn a_reading_comes_off_the_inventory_reply() {
    let inv = Inventory::parse(
        "You are carrying 11 silver nobles, 49 copper farthings, quarterstaff\n\
         You have no keys.\n\
         Encumbrance: 800/2400 - Light [33%]\n",
    );
    let r = Reading::of(&inv).expect("a reply with an encumbrance line");
    assert_eq!(r.coins.counts, [49, 11, 0, 0, 0]);
    assert_eq!(r.class, WeightClass::Light);
    assert!(Reading::of(&Inventory::default()).is_none());
}

/// The count gate: over the mark and with something to deposit above
/// the keep floor.
#[test]
fn the_count_gate_fires_over_the_mark_when_a_deposit_can_lower_it() {
    let cfg = BankConfig::default();
    let mut gate = BankGate::new();
    assert_eq!(
        gate.judge(&cfg, reading([1000, 0, 0, 0, 0], 333)),
        Judgement::Hold,
        "exactly the mark is not over it"
    );
    assert_eq!(
        gate.judge(&cfg, reading([1001, 0, 0, 0, 0], 333)),
        Judgement::Deposit
    );
    let high_floor = BankConfig {
        keep_gold: 20,
        ..BankConfig::default()
    };
    let mut gate = BankGate::new();
    assert_eq!(
        gate.judge(&high_floor, reading([1001, 0, 0, 0, 0], 333)),
        Judgement::Hold,
        "1001 copper is 10 gold, all of it under a 20 gold floor"
    );
    let off = BankConfig {
        deposit_at_coins: 0,
        deposit_on_weight_class: false,
        ..BankConfig::default()
    };
    let mut gate = BankGate::new();
    assert_eq!(
        gate.judge(&off, reading([5000, 0, 0, 0, 0], 1666)),
        Judgement::Hold
    );
}

/// The class gate fires on a crossing between two readings, never on a
/// class that was already raised, and a two-step jump counts once.
#[test]
fn the_class_gate_fires_on_a_crossing_only() {
    let cfg = BankConfig {
        deposit_at_coins: 0,
        ..BankConfig::default()
    };
    let mut gate = BankGate::new();
    assert_eq!(
        gate.judge(&cfg, reading([300, 0, 0, 0, 0], 700)),
        Judgement::Hold,
        "the first reading has nothing to cross from"
    );
    assert_eq!(
        gate.judge(&cfg, reading([600, 0, 0, 0, 0], 800)),
        Judgement::Deposit,
        "None to Light"
    );
    assert_eq!(
        gate.judge(&cfg, reading([900, 0, 0, 0, 0], 900)),
        Judgement::Hold,
        "still Light, nothing crossed"
    );
    assert_eq!(
        gate.judge(&cfg, reading([3000, 0, 0, 0, 0], 2400)),
        Judgement::Deposit,
        "Light to Heavy is a crossing"
    );
    let mut seeded = BankGate::new();
    seeded.seed(reading([0, 0, 0, 0, 0], 700));
    assert_eq!(
        seeded.judge(&cfg, reading([300, 0, 0, 0, 0], 800)),
        Judgement::Deposit,
        "a seeded gate compares against the seed"
    );
    let off = BankConfig {
        deposit_at_coins: 0,
        deposit_on_weight_class: false,
        ..BankConfig::default()
    };
    let mut gate = BankGate::new();
    gate.seed(reading([0, 0, 0, 0, 0], 700));
    assert_eq!(
        gate.judge(&off, reading([300, 0, 0, 0, 0], 800)),
        Judgement::Hold
    );
}

/// The three replies a deposit gets, from `oracle_bank3.raw`.
#[test]
fn the_deposit_replies_are_told_apart() {
    assert_eq!(
        deposit_reply("You deposit 10 silver nobles."),
        Some(DepositReply::Deposited("10 silver nobles".into()))
    );
    assert_eq!(
        deposit_reply("You cannot DEPOSIT if you are not in a bank!"),
        Some(DepositReply::NotABank)
    );
    assert_eq!(
        deposit_reply("Please specify a more reasonable amount."),
        Some(DepositReply::Unreasonable)
    );
    assert_eq!(deposit_reply("You picked up 11 silver nobles"), None);
}

/// A follower cannot walk to the bank, so its gate asks the leader. It
/// asks once, and not again until it has deposited or five minutes
/// have passed. Every ask needs a fresh pickup behind it: the reading
/// is taken because coins arrived, not on a timer.
#[test]
fn the_follower_gate_asks_once_per_five_minutes() {
    use mud_client::bank::FollowerGate;
    use std::time::{Duration, Instant};
    let cfg = BankConfig {
        deposit_at_coins: 10,
        ..BankConfig::default()
    };
    let t0 = Instant::now();
    let light = reading([0, 0, 0, 0, 0], 5);
    let heavy = reading([0, 0, 50, 0, 0], 16);
    let mut g = FollowerGate::new();
    g.seed(light);
    assert!(!g.wants_reading(t0), "no pickup yet");
    g.on_pickup();
    assert!(g.wants_reading(t0));
    assert!(g.on_reading(&cfg, heavy, t0), "over the mark: ask");
    assert!(!g.wants_reading(t0), "the reading answered the pickup");
    g.on_pickup();
    assert!(g.wants_reading(t0 + Duration::from_secs(60)));
    assert!(
        !g.on_reading(&cfg, heavy, t0 + Duration::from_secs(60)),
        "asked a minute ago"
    );
    g.on_pickup();
    assert!(
        g.on_reading(&cfg, heavy, t0 + Duration::from_secs(301)),
        "five minutes on: ask again"
    );
    g.deposited();
    g.on_pickup();
    assert!(
        g.on_reading(&cfg, heavy, t0 + Duration::from_secs(302)),
        "a deposit resets the ask"
    );
    g.on_pickup();
    assert!(
        !g.on_reading(&cfg, light, t0 + Duration::from_secs(303)),
        "nothing worth banking, no ask"
    );
}
