//! Tests for the game core session mechanics: attach, input dispatch,
//! multi-session broadcast, quit.

use mud_core::content::{ClassId, Content, Direction, Exit, RaceId, Room, RoomId};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

fn two_room_content() -> Content {
    let mut content = Content::default();
    let mut gates = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec![],
        shop: None,
        exits: Default::default(),
    };
    gates.exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        exit_type: 0,
    });
    let mut square = Room {
        id: RoomId { map: 1, room: 2 },
        name: "Town Square".into(),
        description: vec![],
        shop: None,
        exits: Default::default(),
    };
    square.exits[Direction::South as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 1 },
        exit_type: 0,
    });
    content.add_room(gates);
    content.add_room(square);
    content
}

fn player(name: &str) -> Player {
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
        cp_unspent: 0,
        cp_lifetime: 0,
        lives: 9,
        experience: 0,
        location: RoomId { map: 1, room: 1 },
    }
}

fn outputs_for(events: &[Event], session: SessionId) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == session => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn attaching_a_player_announces_entry_to_others() {
    let mut core = Core::new(two_room_content(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    core.drain_events();

    let bob = core.attach_player(player("Bob"));
    let events = core.drain_events();

    let to_alice = outputs_for(&events, alice).join("");
    assert!(
        to_alice.contains("Bob just entered the Realm."),
        "Alice sees Bob enter, got: {to_alice:?}"
    );
    let to_bob = outputs_for(&events, bob).join("");
    assert!(
        !to_bob.contains("just entered the Realm"),
        "Bob does not see his own entry, got: {to_bob:?}"
    );
}

#[test]
fn quit_disconnects_persists_and_announces() {
    let mut core = Core::new(two_room_content(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    let bob = core.attach_player(player("Bob"));
    core.drain_events();

    core.input(bob, "x");
    // The exit completes after the meditation delay (oracle).
    for _ in 0..10 {
        core.tick();
    }
    let events = core.drain_events();

    assert!(
        events.iter().any(|e| matches!(e, Event::Disconnect(s) if *s == bob)),
        "quit emits Disconnect"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Persist(p) if p.name == "Bob")),
        "quit persists the player"
    );
    let to_alice = outputs_for(&events, alice).join("");
    assert!(
        to_alice.contains("Bob just left the Realm."),
        "Alice sees Bob leave, got: {to_alice:?}"
    );
}

#[test]
fn unmatched_input_is_spoken_aloud() {
    // Oracle: unknown input is SAY, not an error. Speaker sees
    // `You say "..."`; others in the room see `Name says "..."` (DLL).
    let mut core = Core::new(two_room_content(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    let bob = core.attach_player(player("Bob"));
    core.drain_events();

    core.input(alice, "xyzzy");
    let events = core.drain_events();
    let to_alice = outputs_for(&events, alice).join("");
    assert!(
        to_alice.contains("You say \"xyzzy\""),
        "got: {to_alice:?}"
    );
    let to_bob = outputs_for(&events, bob).join("");
    assert!(
        to_bob.contains("Alice says \"xyzzy\""),
        "got: {to_bob:?}"
    );
}

#[test]
fn dropped_connection_detach_persists_and_announces() {
    let mut core = Core::new(two_room_content(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    let bob = core.attach_player(player("Bob"));
    core.drain_events();

    core.detach(bob); // carrier dropped, no quit command
    let events = core.drain_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Persist(p) if p.name == "Bob")),
        "detach persists the player"
    );
    let to_alice: String = events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == alice => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(to_alice.contains("Bob just left the Realm."));
}

#[test]
fn input_from_detached_session_is_ignored() {
    let mut core = Core::new(two_room_content(), CoreConfig::default());
    let alice = core.attach_player(player("Alice"));
    core.input(alice, "x");
    core.drain_events();

    core.input(alice, "look"); // already disconnected
    assert_eq!(core.drain_events(), vec![]);
}
