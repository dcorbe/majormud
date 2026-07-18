//! Tests for room display (look) and movement.

use mud_core::content::{ClassId, Content, Direction, Exit, RaceId, Room, RoomId};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    let mut gates = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    gates.exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    gates.exits[Direction::Up as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 3 },
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    let mut square = Room {
        id: RoomId { map: 1, room: 2 },
        name: "Town Square".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    square.exits[Direction::South as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 1 },
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    let tower = Room {
        id: RoomId { map: 1, room: 3 },
        name: "Guard Tower".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    content.add_room(gates);
    content.add_room(square);
    content.add_room(tower);
    content
}

fn player_at(name: &str, room: u16) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        stats: Default::default(),
        base_stats: Default::default(),
        hp_base: 0,
        current_hp: 10,
        current_mana: 0,
        hunger: 1000,
        thirst: 1000,
        coins: Default::default(),
        lawful: false,
        inventory: vec![],
        weapon: None,
        bankbooks: vec![],
        worn: vec![],
        cp_unspent: 0,
        cp_lifetime: 0,
        lives: 9,
        experience: 0,
        location: RoomId { map: 1, room },
        spellbook: std::collections::BTreeMap::new(),
        poison: 0,
        active_spells: Default::default(),
    }
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
fn attach_shows_the_current_room() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains("Town Gates"), "room name shown: {shown:?}");
    assert!(
        shown.contains("Obvious exits: north, up"),
        "exits listed with up (USER TESTIMONY, not above): {shown:?}"
    );
}

#[test]
fn look_renders_name_description_exits() {
    let mut content = world();
    content.rooms.get_mut(&RoomId { map: 1, room: 1 }).unwrap().name = "Town Gates".into();
    let mut core = Core::new(content, CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    core.drain_events();

    core.input(alice, "look");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains("Town Gates"));
    assert!(shown.contains("Obvious exits: north, up"));
}

#[test]
fn room_with_no_exits_shows_none() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 3));
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.contains("Obvious exits: NONE!!!"),
        "got: {shown:?}"
    );
}

#[test]
fn other_players_in_room_are_listed() {
    let mut core = Core::new(world(), CoreConfig::default());
    let _alice = core.attach_player(player_at("Alice", 1));
    let bob = core.attach_player(player_at("Bob", 1));
    let shown = text_to(&core.drain_events(), bob);
    assert!(
        shown.contains("Also here: Alice."),
        "Bob sees Alice, got: {shown:?}"
    );
}

#[test]
fn moving_shows_new_room_and_broadcasts_both_sides() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    let bob = core.attach_player(player_at("Bob", 1));
    let carol = core.attach_player(player_at("Carol", 2));
    core.drain_events();

    core.input(bob, "n");
    let events = core.drain_events();

    let to_bob = text_to(&events, bob);
    assert!(to_bob.contains("Town Square"), "mover sees new room");
    let to_alice = text_to(&events, alice);
    assert!(
        to_alice.contains("Bob just left to the north."),
        "old room sees departure: {to_alice:?}"
    );
    let to_carol = text_to(&events, carol);
    assert!(
        to_carol.contains("Bob walks into the room from the south."),
        "new room sees arrival from opposite side (oracle 2026-07-18: \
         'Kaimon walks into the room from the east.'; 'just arrived from' \
         never appears in any capture — it is the spawn-arrival string): \
         {to_carol:?}"
    );
}

#[test]
fn vertical_movement_uses_upwards_phrasing() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    let bob = core.attach_player(player_at("Bob", 1));
    let carol = core.attach_player(player_at("Carol", 3));
    core.drain_events();

    core.input(bob, "u");
    let events = core.drain_events();
    let to_alice = text_to(&events, alice);
    assert!(
        to_alice.contains("Bob just left upwards."),
        "got: {to_alice:?}"
    );
    let to_carol = text_to(&events, carol);
    assert!(
        to_carol.contains("Bob walks into the room from below."),
        "vertical arrival says from below, no article (oracle: 'Oracle \
         walks into the room from above.'): {to_carol:?}"
    );
}

#[test]
fn blocked_direction_reports_no_exit() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    core.drain_events();

    core.input(alice, "west");
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.contains("There is no exit in that direction!"),
        "got: {shown:?}"
    );
}

#[test]
fn description_first_line_is_indented_four_spaces() {
    let mut content = world();
    content
        .rooms
        .get_mut(&RoomId { map: 1, room: 1 })
        .unwrap()
        .description = vec!["Welcome to the gates.".into(), "Second line.".into()];
    let mut core = Core::new(content, CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    let shown = text_to(&core.drain_events(), alice);
    assert!(
        shown.contains("Town Gates\n    Welcome to the gates.\nSecond line.\n"),
        "oracle format: 4-space indent on first line only, got: {shown:?}"
    );
}

#[test]
fn blank_input_shows_brief_room_without_description() {
    let mut content = world();
    content
        .rooms
        .get_mut(&RoomId { map: 1, room: 1 })
        .unwrap()
        .description = vec!["Welcome to the gates.".into()];
    let mut core = Core::new(content, CoreConfig::default());
    let alice = core.attach_player(player_at("Alice", 1));
    core.drain_events();

    core.input(alice, "");
    let shown = text_to(&core.drain_events(), alice);
    assert!(shown.contains("Town Gates"), "brief shows name: {shown:?}");
    assert!(
        shown.contains("Obvious exits: north, up"),
        "brief shows exits: {shown:?}"
    );
    assert!(
        !shown.contains("Welcome to the gates."),
        "brief omits description: {shown:?}"
    );
}

#[test]
fn movement_only_broadcasts_to_the_two_rooms_involved() {
    let mut core = Core::new(world(), CoreConfig::default());
    let bob = core.attach_player(player_at("Bob", 1));
    let carol = core.attach_player(player_at("Carol", 3));
    core.drain_events();

    core.input(bob, "n");
    let to_carol = text_to(&core.drain_events(), carol);
    assert!(
        to_carol.is_empty(),
        "unrelated room hears nothing: {to_carol:?}"
    );
}
