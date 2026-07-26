//! Tests for user_can_use (0x1fced): class/race allowlists, level gates,
//! the class weapon/armour permission matrix, AntiMagic, and the shop-list
//! annotation (oracle_healer_gates.raw).

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, Shop, ShopId, ShopStock,
    StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn base_content() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Shop".into(),
        description: vec![],
        room_type: 1,
        attributes: 0,
        shop: Some(ShopId(33)),
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
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
    // Class 1: Warrior (weapon 8 = everything, armour 9).
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
    // Class 5: Priest (weapon 7 = blunt only, armour 1).
    content.add_class(Class {
        id: ClassId(5),
        name: "Priest".into(),
        abilities: vec![],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 1,
        casting_factor: 10,
        exp_base: 0,
        combat_factor: 3,
        weapon_code: 7,
        armour_code: 1,
    });
    // Class 12: Mage (weapon 9 = quarterstaff/dagger only, armour 1).
    content.add_class(Class {
        id: ClassId(12),
        name: "Mage".into(),
        abilities: vec![],
        hp_per_level: 4,
        hp_seed: 4,
        caster_group: 2,
        casting_factor: 10,
        exp_base: 0,
        combat_factor: 2,
        weapon_code: 9,
        armour_code: 1,
    });
    content
}

fn shop_with(content: &mut Content, items: &[ItemId]) {
    let mut stock = [ShopStock::default(); 20];
    for (i, id) in items.iter().enumerate() {
        stock[i] = ShopStock {
            item: Some(*id),
            max: 5,
            now: 5,
            ..ShopStock::default()
        };
    }
    content.add_shop(Shop {
        id: ShopId(33),
        name: "Jewellry Shop".into(),
        shop_type: 10,
        min_level: 0,
        max_level: 0,
        markup: 100,
        class_limit: 0,
        stock,
    });
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        ..CoreConfig::default()
    }
}

fn create_class(core: &mut Core, name: &str, class_choice: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
        saved_evil: 0,
    });
    core.input(s, "2"); // Dwarf
    core.input(s, class_choice);
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
fn class_restricted_item_annotates_and_refuses_wear() {
    // Live capture: the Paladin-only silver holy amulet for a Warrior.
    let mut content = base_content();
    content.add_item(Item {
        id: ItemId(166),
        name: "silver holy amulet".into(),
        weight: 10,
        item_type: 0,
        uses: -1,
        cost: 15,
        cost_denomination: 2,
        classes: vec![ClassId(3)], // Paladin
        worn_on: 8,                // Neck
        gettable: 1,
        ..Item::default()
    });
    shop_with(&mut content, &[ItemId(166)]);
    let mut core = Core::new(content, config());
    let s = create_class(&mut core, "Dain", "1");

    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("silver holy amulet            5           30 gold crowns (You can't use)"),
        "annotated row: {shown:?}"
    );

    // Buying is NOT gated (live: the Warrior bought it fine)...
    core.give_copper(s, 5000);
    core.input(s, "buy amulet");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You just bought silver holy amulet"), "got: {shown:?}");

    // ...wearing is.
    core.input(s, "wear amulet");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not wear that item!"),
        "got: {shown:?}"
    );
}

#[test]
fn min_level_gates_arm() {
    let mut content = base_content();
    content.add_item(Item {
        id: ItemId(969),
        name: "serrated scimitar".into(),
        weight: 40,
        item_type: 1,
        uses: -1,
        weapon_type: 2,
        gettable: 1,
        abilities: vec![(Ability::from_id(135).unwrap(), 10)], // MinLevel 10
        ..Item::default()
    });
    shop_with(&mut content, &[]);
    let mut core = Core::new(content, config());
    let s = create_class(&mut core, "Dain", "1");
    core.give_item(s, ItemId(969));
    core.input(s, "arm scimitar");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not use that weapon."),
        "L1 vs MinLevel 10: {shown:?}"
    );
}

#[test]
fn priest_is_blunt_only() {
    let mut content = base_content();
    content.add_item(Item {
        id: ItemId(68),
        name: "dagger".into(),
        weight: 35,
        item_type: 1,
        uses: -1,
        weapon_type: 2, // one-handed sharp
        gettable: 1,
        ..Item::default()
    });
    content.add_item(Item {
        id: ItemId(92),
        name: "spiked club".into(),
        weight: 100,
        item_type: 1,
        uses: -1,
        weapon_type: 0, // one-handed blunt
        gettable: 1,
        ..Item::default()
    });
    shop_with(&mut content, &[]);
    let mut core = Core::new(content, config());
    let s = create_class(&mut core, "Prio", "5");
    core.give_item(s, ItemId(68));
    core.give_item(s, ItemId(92));
    core.input(s, "arm dagger");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not use that weapon."),
        "sharp refused: {shown:?}"
    );
    core.input(s, "arm club");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are now holding spiked club."),
        "blunt fine: {shown:?}"
    );
}

#[test]
fn mage_uses_only_the_config_weapons() {
    // Weapon code 9: only items 100 (quarterstaff) and 68 (dagger).
    let mut content = base_content();
    content.add_item(Item {
        id: ItemId(100),
        name: "quarterstaff".into(),
        weight: 100,
        item_type: 1,
        uses: -1,
        weapon_type: 1,
        gettable: 1,
        ..Item::default()
    });
    content.add_item(Item {
        id: ItemId(92),
        name: "spiked club".into(),
        weight: 100,
        item_type: 1,
        uses: -1,
        weapon_type: 0,
        gettable: 1,
        ..Item::default()
    });
    shop_with(&mut content, &[]);
    let mut core = Core::new(content, config());
    let s = create_class(&mut core, "Magus", "12");
    core.give_item(s, ItemId(100));
    core.give_item(s, ItemId(92));
    core.input(s, "arm quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are now holding quarterstaff."),
        "config staff allowed: {shown:?}"
    );
    core.input(s, "arm club");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not use that weapon."),
        "club refused: {shown:?}"
    );
}

#[test]
fn heavy_armour_refused_below_class_armour_code() {
    let mut content = base_content();
    content.add_item(Item {
        id: ItemId(34),
        name: "chain coif".into(),
        weight: 40,
        item_type: 0,
        uses: -1,
        armour_req: 5, // heavier than the Mage's armour code 1
        worn_on: 2,
        gettable: 1,
        ..Item::default()
    });
    shop_with(&mut content, &[]);
    let mut core = Core::new(content, config());
    let mage = create_class(&mut core, "Magus", "12");
    core.give_item(mage, ItemId(34));
    core.input(mage, "wear coif");
    let shown = text_to(&core.drain_events(), mage);
    assert!(
        shown.contains("You may not wear that item!"),
        "got: {shown:?}"
    );
    // A class allowlist match bypasses the matrix entirely.
    let mut content = base_content();
    content.add_item(Item {
        id: ItemId(34),
        name: "chain coif".into(),
        weight: 40,
        item_type: 0,
        uses: -1,
        armour_req: 5,
        classes: vec![ClassId(12)], // explicitly Mage-allowed
        worn_on: 2,
        gettable: 1,
        ..Item::default()
    });
    shop_with(&mut content, &[]);
    let mut core = Core::new(content, config());
    let mage = create_class(&mut core, "Magus", "12");
    core.give_item(mage, ItemId(34));
    core.input(mage, "wear coif");
    let shown = text_to(&core.drain_events(), mage);
    assert!(
        shown.contains("You are now wearing chain coif."),
        "allowlist bypass: {shown:?}"
    );
}

#[test]
fn antimagic_player_cannot_use_magical_items() {
    let mut content = base_content();
    // Give the race AntiMagic (51).
    content.races.get_mut(&RaceId(2)).unwrap().abilities =
        vec![(Ability::from_id(51).unwrap(), 1)];
    content.add_item(Item {
        id: ItemId(500),
        name: "glowing ring".into(),
        weight: 1,
        item_type: 0,
        uses: -1,
        worn_on: 4, // Finger
        gettable: 1,
        abilities: vec![(Ability::from_id(28).unwrap(), 1)], // Magical
        ..Item::default()
    });
    shop_with(&mut content, &[]);
    let mut core = Core::new(content, config());
    let s = create_class(&mut core, "Dain", "1");
    core.give_item(s, ItemId(500));
    core.input(s, "wear ring");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not wear that item!"),
        "AntiMagic vs Magical: {shown:?}"
    );
}

#[test]
fn zero_stock_rows_are_hidden_from_the_list() {
    // Live: brass knuckles and the serrated scimitar never appeared in
    // their shops' lists (board stock 0).
    let mut content = base_content();
    content.add_item(Item {
        id: ItemId(64),
        name: "longsword".into(),
        weight: 80,
        item_type: 1,
        uses: -1,
        cost: 12,
        cost_denomination: 2,
        weapon_type: 2,
        gettable: 1,
        ..Item::default()
    });
    shop_with(&mut content, &[ItemId(64)]);
    content.shops.get_mut(&ShopId(33)).unwrap().stock[0].max = 0;
    let mut core = Core::new(content, config());
    let s = create_class(&mut core, "Dain", "1");
    core.input(s, "list");
    let shown = text_to(&core.drain_events(), s);
    assert!(!shown.contains("longsword"), "hidden row: {shown:?}");
}
