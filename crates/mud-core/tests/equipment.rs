//! Tests for arm/wield/equip, wear/remove, and combat integration
//! (oracle_m4_round2.raw).

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, StatBlock,
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
        accuracy: 4,
        ..Item::default()
    }
}

fn helmet() -> Item {
    Item {
        id: ItemId(200),
        name: "iron helmet".into(),
        weight: 60,
        item_type: 0,
        uses: -1,
        ac: 20,
        worn_on: 2,
        gettable: 1,
        // +5 accuracy while worn, to prove abilities flow to the bag.
        abilities: vec![(Ability::from_id(22).unwrap(), 5)],
        ..Item::default()
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Weapons Shop".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_item(quarterstaff());
    content.add_item(helmet());
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
fn arm_holds_the_weapon() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "arm quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are now holding quarterstaff."),
        "got: {shown:?}"
    );

    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are carrying quarterstaff (Two handed)\n"),
        "got: {shown:?}"
    );
}

#[test]
fn wield_and_eq_are_synonyms_with_partial_names() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "wield quar");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You are now holding quarterstaff."), "got: {shown:?}");

    let s2 = create(&mut core, "Bofur");
    core.give_item(s2, ItemId(100));
    core.input(s2, "eq quar");
    let shown = text_to(&core.drain_events(), s2);
    assert!(shown.contains("You are now holding quarterstaff."), "got: {shown:?}");
}

#[test]
fn arming_an_absent_item_matches_the_oracle() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "arm dagger");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You do not have dagger left unequipped."),
        "got: {shown:?}"
    );
}

#[test]
fn armed_weapon_feeds_the_fighter() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "arm quarterstaff");
    core.drain_events();

    let (fighter, eu) = core.combat_debug(s);
    // Damage from the staff (2-12), Str 50 adds nothing.
    assert_eq!(fighter.min_damage, 2);
    assert_eq!(fighter.max_damage, 12);
    // skill = staff accuracy 4 (floor bypassed) + 15 unencumbered = 19;
    // accuracy = 0 + 2*((6-1)*1 + 12 + 0 + 19/2 - 2) + (30-50)/6
    //          = 2*(5+12+0+9-2) - 3 = 48 - 3 = 45.
    assert_eq!(fighter.accuracy, 45);
    // EU with staff speed 1200 = same as fists at L1: 1200000/1530 = 784.
    assert_eq!(eu, 784);
}

#[test]
fn wear_and_remove_armor() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(200));
    core.input(s, "wear helmet");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are now wearing iron helmet."),
        "got: {shown:?}"
    );

    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    // oracle_m4_verify.raw: worn items carry their location name.
    assert!(
        shown.contains("You are carrying iron helmet (Head)\n"),
        "got: {shown:?}"
    );

    core.input(s, "remove helmet");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You have removed iron helmet."),
        "got: {shown:?}"
    );
}

#[test]
fn removing_unworn_matches_the_oracle() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "remove quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are not wearing quarterstaff."),
        "got: {shown:?}"
    );
}

#[test]
fn worn_item_abilities_feed_derived_stats() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(200));
    core.input(s, "wear helmet");
    core.drain_events();

    // Helmet grants Accuracy +5 (ability 22) — visible via the fighter's
    // dynamic accuracy accumulator (+0x70a analogue).
    let (fighter, _) = core.combat_debug(s);
    let naked = {
        let s2 = create(&mut core, "Bofur");
        core.combat_debug(s2).0
    };
    assert_eq!(fighter.accuracy, naked.accuracy + 5);
    // And the worn AC contributes to the defender's armor.
    let defender = core.defender_debug(s);
    assert_eq!(defender.armor, 20);
}

#[test]
fn dropping_the_armed_weapon_unarms_it() {
    // Oracle: "drop quarterstaff" while holding it just dropped it.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "arm quarterstaff");
    core.drain_events();
    core.input(s, "drop quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You dropped quarterstaff."), "got: {shown:?}");

    let (fighter, _) = core.combat_debug(s);
    assert_eq!(fighter.max_damage, 4, "back to fists");
}

#[test]
fn carrying_line_suffixes_and_grouping_match_the_oracle() {
    // oracle_m4_verify.raw: "chain coif (Head), dagger (Weapon Hand),
    // quarterstaff, 3 sickle" — worn first (location name), armed weapon
    // next (hand suffix), then loose items grouped with a count prefix.
    let mut content = world();
    content.add_item(Item {
        id: ItemId(74),
        name: "sickle".into(),
        weight: 30,
        item_type: 1,
        uses: -1,
        weapon_type: 0, // one-handed
        gettable: 1,
        ..Item::default()
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(200));
    core.give_item(s, ItemId(100));
    core.give_item(s, ItemId(74));
    core.give_item(s, ItemId(74));
    core.give_item(s, ItemId(74));
    core.input(s, "wear helmet");
    core.input(s, "arm sickle");
    core.drain_events();

    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains(
            "You are carrying iron helmet (Head), sickle (Weapon Hand), quarterstaff, 2 sickle\n"
        ),
        "worn -> armed -> grouped loose: {shown:?}"
    );

    // Two-handed weapons get the other suffix.
    core.input(s, "arm quarterstaff");
    core.drain_events();
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("quarterstaff (Two handed)"),
        "2H suffix: {shown:?}"
    );
    assert!(
        shown.contains("3 sickle"),
        "unarmed sickles regroup: {shown:?}"
    );
}
