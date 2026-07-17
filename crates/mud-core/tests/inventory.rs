//! Tests for inventory, get/drop (items and coins), and encumbrance
//! (oracle transcripts oracle_m4_items.raw / oracle_m4_round2.raw).

use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, PlacedItem, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn quarterstaff() -> Item {
    Item {
        id: ItemId(100),
        name: "quarterstaff".into(),
        weight: 100,
        item_type: 1,
        uses: -1,
        min_damage: 2,
        max_damage: 12,
        weapon_type: 1, // two-handed
        gettable: 1,
        speed: 1200,
        ..Item::default()
    }
}

fn manual() -> Item {
    Item {
        id: ItemId(1098),
        name: "newbie manual".into(),
        weight: 10,
        gettable: 0, // fixture
        ..Item::default()
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Village Entrance".into(),
        description: vec![],
        room_type: 0,
        shop: None,
        placed_items: vec![PlacedItem {
            item: ItemId(1098),
            quantity: 1,
        }],
        exits: Default::default(),
    });
    content.add_item(quarterstaff());
    content.add_item(manual());
    content.add_race(Race {
        id: RaceId(2),
        name: "Dwarf".into(),
        // Encum +20% (ability 96) as in the real data -> capacity 2880.
        abilities: vec![(mud_core::ability::Ability::from_id(96).unwrap(), 20)],
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
fn empty_inventory_matches_the_oracle() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains(
            "You are carrying Nothing!\nYou have no keys.\nWealth: 0 copper farthings\nEncumbrance: 0/2880 - None [0%]"
        ),
        "got: {shown:?}"
    );
}

#[test]
fn floor_items_show_and_get_take_works() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100)); // hand a staff over, then drop it
    core.input(s, "drop quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You dropped quarterstaff."), "got: {shown:?}");

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You notice newbie manual, quarterstaff here."),
        "floor line: {shown:?}"
    );

    core.input(s, "get quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You took quarterstaff."), "got: {shown:?}");

    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are carrying quarterstaff\n"),
        "got: {shown:?}"
    );
    assert!(
        shown.contains("Encumbrance: 100/2880 - None [3%]"),
        "weight of the staff: {shown:?}"
    );
}

#[test]
fn partial_item_names_resolve() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "drop quar");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You dropped quarterstaff."), "got: {shown:?}");
    core.input(s, "get qu");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You took quarterstaff."), "got: {shown:?}");
}

#[test]
fn fixtures_cannot_be_taken() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "get manual");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You don't see manual here."),
        "got: {shown:?}"
    );
}

#[test]
fn get_and_drop_errors_match_the_oracle() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "drop quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You don't have quarterstaff to drop!"),
        "got: {shown:?}"
    );
    core.input(s, "get silver");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You don't see any silver nobles"),
        "got: {shown:?}"
    );
}

#[test]
fn coins_can_be_picked_up_from_piles() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.add_room_coins(RoomId { map: 1, room: 1 }, [43, 7, 0, 0, 0]); // low->high
    core.input(s, "get silver");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You picked up 7 silver nobles"),
        "oracle wording: {shown:?}"
    );
    core.input(s, "get copper");
    core.drain_events();

    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Wealth: 113 copper farthings"),
        "7 silver = 70 copper + 43: {shown:?}"
    );
    // 50 coins * 1/3 weight = 16.
    assert!(
        shown.contains("Encumbrance: 16/2880"),
        "coin weight: {shown:?}"
    );
}

#[test]
fn inventory_wealth_shows_total_copper_value() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_copper(s, 5);
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Wealth: 5 copper farthings"),
        "got: {shown:?}"
    );
}
