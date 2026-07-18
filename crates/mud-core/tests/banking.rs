//! Tests for banking, verified coin ratios, coin display, and
//! text-triggered exits (oracle_bank*.raw).

use mud_core::content::{
    Class, ClassId, Content, Exit, Message, MessageId, Race, RaceId, Room, RoomId, Shop, ShopId,
    StatBlock,
};
use mud_core::game::{AccountProfile, Coins, Core, CoreConfig, Event, Gender, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    let mut dock = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Docks".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    // Type-10 action exit south, phrases in message 8564.
    dock.exits[mud_core::content::Direction::South as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        exit_type: 10,
        trigger_msg: Some(MessageId(8564)),
        ..Default::default()
    });
    content.add_room(dock);
    content.add_room(Room {
        id: RoomId { map: 1, room: 2 },
        name: "Small Pier".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    let mut bank_room = Room {
        id: RoomId { map: 1, room: 297 },
        name: "Bank of Godfrey".into(),
        description: vec![],
        room_type: 1,
        attributes: 0,
        shop: Some(ShopId(8)),
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    bank_room.exits[mud_core::content::Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 1 },
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    content.add_room(bank_room);
    content.add_message(Message {
        id: MessageId(8564),
        lines: vec!["borrow skiff|go skiff|row skiff".into()],
    });
    content.add_shop(Shop {
        id: ShopId(8),
        name: "Bank of Godfrey".into(),
        shop_type: 7, // bank
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

fn config(start_room: u16) -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: start_room },
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
fn official_conversion_rates() {
    // Bank lobby sign (oracle): 10c=1s, 10s=1g, 100g=1p, 100p=1r.
    let ratios = CoreConfig::default().coin_ratios;
    assert_eq!(ratios, [10, 10, 100, 100]);
    let purse = Coins {
        runic: 0,
        platinum: 0,
        gold: 1,
        silver: 0,
        copper: 9,
    };
    assert_eq!(purse.total_copper(ratios), 109);
}

#[test]
fn coins_appear_in_the_carrying_line_before_items() {
    let mut core = Core::new(world(), config(1));
    let s = create(&mut core, "Dain");
    core.set_coins(s, Coins { runic: 0, platinum: 0, gold: 0, silver: 11, copper: 49 });
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are carrying 11 silver nobles, 49 copper farthings\n"),
        "got: {shown:?}"
    );
    assert!(shown.contains("Wealth: 159 copper farthings"), "got: {shown:?}");
}

#[test]
fn coin_pickup_says_picked_up() {
    let mut core = Core::new(world(), config(1));
    let s = create(&mut core, "Dain");
    core.add_room_coins(RoomId { map: 1, room: 1 }, [0, 11, 0, 0, 0]);
    core.input(s, "get silver");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You picked up 11 silver nobles"),
        "oracle wording: {shown:?}"
    );
}

#[test]
fn action_exit_moves_on_trigger_phrase() {
    let mut core = Core::new(world(), config(1));
    let s = create(&mut core, "Dain");
    core.input(s, "row skiff");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("Small Pier"), "moved: {shown:?}");

    // The action exit is hidden from the obvious-exits line.
    let s2 = create(&mut core, "Bofur");
    core.input(s2, "look");
    let shown = text_to(&core.drain_events(), s2);
    assert!(
        shown.contains("Obvious exits: NONE!!!"),
        "type-10 hidden: {shown:?}"
    );
    // And plain movement south does not work.
    core.input(s2, "south");
    let shown = text_to(&core.drain_events(), s2);
    assert!(shown.contains("There is no exit in that direction!"), "got: {shown:?}");
}

#[test]
fn balance_deposit_withdraw_cycle() {
    let mut core = Core::new(world(), config(297));
    let s = create(&mut core, "Dain");
    core.set_coins(s, Coins { runic: 0, platinum: 0, gold: 0, silver: 11, copper: 49 });

    core.input(s, "balance");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Your balance at Bank of Godfrey (#8) is:\nOn deposit: 0 copper farthings [0. 0 gold crowns]"),
        "got: {shown:?}"
    );

    core.input(s, "deposit 100");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You deposit 10 silver nobles."),
        "largest coins first: {shown:?}"
    );

    core.input(s, "balance");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("On deposit: 100 copper farthings [1. 0 gold crowns]"),
        "got: {shown:?}"
    );

    core.input(s, "withdraw 50");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You withdrew 50 copper farthings."),
        "got: {shown:?}"
    );

    // Final purse minted upward: 159-100+50 = 109 = 1 gold, 9 copper.
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are carrying 1 gold crown, 9 copper farthings\n"),
        "minted purse: {shown:?}"
    );
}

#[test]
fn over_withdrawal_is_silent() {
    let mut core = Core::new(world(), config(297));
    let s = create(&mut core, "Dain");
    core.input(s, "withdraw 99999");
    let shown = text_to(&core.drain_events(), s);
    // Oracle: no message at all (just the prompt).
    assert!(
        !shown.contains("withdrew") && !shown.contains("cannot"),
        "silent refusal: {shown:?}"
    );
}

#[test]
fn junk_amounts_are_rejected() {
    let mut core = Core::new(world(), config(297));
    let s = create(&mut core, "Dain");
    core.give_copper(s, 10);
    core.input(s, "deposit all");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Please specify a more reasonable amount."),
        "got: {shown:?}"
    );
}

#[test]
fn banking_outside_a_bank_is_refused() {
    let mut core = Core::new(world(), config(1));
    let s = create(&mut core, "Dain");
    core.input(s, "deposit 10");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You cannot DEPOSIT if you are not in a bank!"),
        "got: {shown:?}"
    );
    core.input(s, "withdraw 10");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You cannot WITHDRAW if you are not in a bank!"),
        "got: {shown:?}"
    );
}

#[test]
fn balance_persists_on_the_player() {
    let mut core = Core::new(world(), config(297));
    let s = create(&mut core, "Dain");
    core.give_copper(s, 100);
    core.input(s, "deposit 100");
    core.drain_events();
    core.detach(s);
    let persisted = core
        .drain_events()
        .into_iter()
        .find_map(|e| match e {
            Event::Persist(p) => Some(p),
            _ => None,
        })
        .expect("persisted");
    assert_eq!(persisted.bankbooks, vec![(8u16, 100u64)]);
}

#[test]
fn deduct_spends_exactly_like_the_original() {
    // Three consecutive purchases traced live (oracle_m4_verify.raw):
    // the message lists the true coins handed over, change-making
    // included (deduct_currency 0x1edca: greedy pass, break ONE smallest
    // higher coin, repeat full pass).
    let ratios = CoreConfig::default().coin_ratios;
    let mut purse = Coins { runic: 0, platinum: 25, gold: 2, silver: 1, copper: 4 };

    // Dagger, 208 copper: "2 gold crowns, 8 copper farthings."
    let spent = purse.deduct_copper(208, ratios);
    assert_eq!((spent.gold, spent.silver, spent.copper), (2, 0, 8));
    assert_eq!(
        (purse.platinum, purse.gold, purse.silver, purse.copper),
        (25, 0, 0, 6)
    );

    // Sickle, 104 copper: "9 silver nobles, 14 copper farthings."
    let spent = purse.deduct_copper(104, ratios);
    assert_eq!((spent.gold, spent.silver, spent.copper), (0, 9, 14));
    assert_eq!(
        (purse.platinum, purse.gold, purse.silver, purse.copper),
        (24, 99, 0, 2)
    );

    // Sickle again, 104 copper: "1 gold crown, 4 copper farthings."
    let spent = purse.deduct_copper(104, ratios);
    assert_eq!((spent.gold, spent.silver, spent.copper), (1, 0, 4));
    assert_eq!(
        (purse.platinum, purse.gold, purse.silver, purse.copper),
        (24, 97, 9, 8)
    );
}
