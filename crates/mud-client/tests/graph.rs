//! Exit-cost tests for the room graph: what a step through a gated exit
//! costs the walker.

use mud_core::content::{Direction, RoomId};
use mud_client::graph::{Capabilities, Cost, ExitRequirement, exit_cost_for};
use mud_client::purse::Purse;

/// A toll you can pay is a step. A toll you cannot pay is a wall.
#[test]
fn a_toll_costs_a_step_when_affordable_and_is_impassable_otherwise() {
    let toll = ExitRequirement::Toll { gold: 5 };
    let edge = RoomId { map: 1, room: 1381 };
    let rich = Capabilities {
        purse: Purse::from_farthings(500),
        ..Default::default()
    };
    let broke = Capabilities {
        purse: Purse::from_farthings(499),
        ..Default::default()
    };
    assert_eq!(
        exit_cost_for(&toll, 4, edge, Direction::East, &rich),
        Cost::Steps(1)
    );
    assert_eq!(
        exit_cost_for(&toll, 4, edge, Direction::East, &broke),
        Cost::Impassable
    );
}

/// A puzzle exit is priced by its plan: the exit itself, then a phrase
/// and a round trip per lever. A searchable hidden exit keeps the
/// search price. A puzzle this walker cannot solve is a wall, not a
/// dear edge: pricing it high would still route through it whenever the
/// detour was longer, and then stall at the wall.
#[test]
fn a_puzzle_exit_is_priced_by_its_plan() {
    use mud_client::puzzle::{Puzzle, PuzzleAction};
    use mud_core::content::ItemId;

    let caps = Capabilities::unrestricted();
    let edge = RoomId { map: 1, room: 506 };
    let button = |item| Puzzle {
        word: 16,
        actions: vec![PuzzleAction {
            room: edge,
            number: 1,
            phrases: vec!["push button".into()],
            item,
            reply: None,
            hops: Some(0),
        }],
    };
    assert_eq!(
        exit_cost_for(&ExitRequirement::Puzzle(button(None)), 6, edge, Direction::South, &caps),
        Cost::Steps(2),
        "a revealed passage is a step, plus the phrase"
    );
    assert_eq!(
        exit_cost_for(&ExitRequirement::Puzzle(button(None)), 0xb, edge, Direction::South, &caps),
        Cost::Steps(6),
        "a lever on a gate keeps the door price underneath"
    );
    assert_eq!(
        exit_cost_for(
            &ExitRequirement::Puzzle(button(Some(ItemId(500)))),
            6,
            edge,
            Direction::South,
            &caps
        ),
        Cost::Impassable,
        "an action needing an item the pack lacks is a wall"
    );
    assert_eq!(
        exit_cost_for(&ExitRequirement::Hidden, 6, edge, Direction::North, &caps),
        Cost::Steps(40),
        "a searchable hidden exit keeps its old price"
    );
}

/// Everything exit_cost priced before must still cost the same, or this
/// change silently reroutes the whole world. The old function is the
/// oracle for the new one on every requirement that does not consult
/// state. Type 2 is gone from the list: a key door is priced by its
/// lock now, see `a_key_door_is_free_with_the_key_and_a_lock_without`.
#[test]
fn state_free_requirements_keep_their_old_prices() {
    let caps = Capabilities::unrestricted();
    let edge = RoomId { map: 1, room: 1 };
    for exit_type in [0, 7, 0xb, 9, 0x18, 0x10, 0x14, 0x16, 0x17, 10, 0x13, 3, 5] {
        let req = ExitRequirement::from_exit_type(exit_type, 0, 0, 0);
        let want = mud_client::graph::exit_cost(exit_type);
        assert_eq!(
            exit_cost_for(&req, exit_type, edge, Direction::North, &caps),
            Cost::Steps(want),
            "type {exit_type:#x} changed price"
        );
    }
}

/// A walker with no pack holds nothing, and so does the unrestricted
/// walker: an unlimited pack would route every character through item
/// gates it cannot pass, which is the wrong direction to be wrong in.
#[test]
fn a_walker_holds_nothing_until_handed_a_pack() {
    use mud_client::pack::PackHandle;
    use mud_client::sheet::Inventory;
    use mud_core::content::{Content, Item, ItemId};
    use std::sync::Arc;

    assert!(!Capabilities::default().has_item(ItemId(172)));
    assert!(!Capabilities::unrestricted().has_item(ItemId(172)));

    let mut content = Content::default();
    content.add_item(Item {
        id: ItemId(172),
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    let pack = PackHandle::new(Arc::new(content));
    pack.refresh(&Inventory {
        items: Vec::new(),
        keys: vec!["black star key".into()],
        encumbrance: None,
    });
    let caps = Capabilities {
        pack: Some(pack),
        ..Capabilities::unrestricted()
    };
    assert!(caps.has_item(ItemId(172)));
    assert!(!caps.has_item(ItemId(173)));
}

/// The pick formula, `theft.md` sections 8.2 and 8.3: the roll is
/// `genrdn(0,100) < modifier + skill` and needs a skill of at least 1.
/// The shipped key door modifiers are the cases: below the line the
/// pick fails every time and no retry budget changes that.
#[test]
fn pickable_is_a_line_the_skill_must_cross() {
    use mud_client::graph::pickable;

    assert!(!pickable(0, 0), "no skill, no pick, whatever the lock");
    assert!(pickable(0, 1));
    assert!(pickable(30, 1), "an easy lock still wants a skill of 1");
    assert!(!pickable(30, 0));
    for modifier in [-30, -60, -99, -100, -160, -290, -999] {
        let line = u32::try_from(-modifier).unwrap();
        assert!(!pickable(modifier, line), "{modifier} + {line} is not above zero");
        assert!(pickable(modifier, line + 1), "{modifier} + {} is", line + 1);
    }
    assert!(pickable(-999, u32::MAX), "the unrestricted walker picks anything");
}

/// The expected number of `picklock` commands: the inverse of the
/// chance, rounded up, never below one.
#[test]
fn pick_rolls_is_the_inverse_of_the_chance() {
    use mud_client::graph::pick_rolls;

    assert_eq!(pick_rolls(0, 100), 1, "a certain pick is one command");
    assert_eq!(pick_rolls(30, u32::MAX), 1);
    assert_eq!(pick_rolls(-30, 80), 2, "50 in 100");
    assert_eq!(pick_rolls(-60, 80), 5, "20 in 100");
    assert_eq!(pick_rolls(-99, 100), 100, "1 in 100");
    assert_eq!(pick_rolls(-90, 100), 10);
    assert_eq!(pick_rolls(-70, 100), 4, "30 in 100 rounds up");
}

/// The Black House door: free with the key on the ring, a lock without
/// it. The lock is priced by the expected rolls and capped at a
/// searchable hidden exit, and a lock this character cannot pick is a
/// wall whether or not bashing is on.
#[test]
fn a_key_door_is_free_with_the_key_and_a_lock_without() {
    use mud_client::pack::PackHandle;
    use mud_client::sheet::Inventory;
    use mud_core::content::{Content, Item, ItemId};
    use std::sync::Arc;

    let edge = RoomId { map: 1, room: 1224 };
    let door = ExitRequirement::KeyDoor { key: ItemId(172), pick: -99 };
    let mut content = Content::default();
    content.add_item(Item {
        id: ItemId(172),
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    let pack = PackHandle::new(Arc::new(content));
    pack.refresh(&Inventory {
        items: Vec::new(),
        keys: vec!["black star key".into()],
        encumbrance: None,
    });
    let with_key = Capabilities {
        picklocks: 0,
        pack: Some(pack),
        ..Capabilities::unrestricted()
    };
    assert_eq!(
        exit_cost_for(&door, 2, edge, Direction::East, &with_key),
        Cost::Steps(5),
        "the key makes it an ordinary door"
    );
    let thief = Capabilities {
        picklocks: 100,
        ..Capabilities::unrestricted()
    };
    assert_eq!(
        exit_cost_for(&door, 2, edge, Direction::East, &thief),
        Cost::Steps(40),
        "1 in 100 is 100 rolls, capped at the hidden exit price"
    );
    let strong = Capabilities {
        picklocks: 149,
        ..Capabilities::unrestricted()
    };
    assert_eq!(
        exit_cost_for(&door, 2, edge, Direction::East, &strong),
        Cost::Steps(7),
        "50 in 100 is two rolls on top of the door"
    );
    for bash_doors in [true, false] {
        let weak = Capabilities {
            picklocks: 99,
            bash_doors,
            ..Capabilities::unrestricted()
        };
        assert_eq!(
            exit_cost_for(&door, 2, edge, Direction::East, &weak),
            Cost::Impassable,
            "-99 + 99 is not above zero, bashing {bash_doors}"
        );
    }
}

/// A locked door is priced by its lock. Pickable, the door plus the
/// rolls. Not pickable, the door when bashing is on, since force is a
/// separate roll, and a wall when it is off. An unlocked door is a
/// door.
#[test]
fn a_locked_door_is_priced_by_the_lock() {
    let edge = RoomId { map: 1, room: 1119 };
    let locked = ExitRequirement::Door { locked: true, pick: -30 };
    let unlocked = ExitRequirement::Door { locked: false, pick: -30 };
    let thief = Capabilities {
        picklocks: 80,
        ..Capabilities::unrestricted()
    };
    assert_eq!(exit_cost_for(&locked, 7, edge, Direction::North, &thief), Cost::Steps(7));
    let basher = Capabilities {
        picklocks: 0,
        bash_doors: true,
        ..Capabilities::unrestricted()
    };
    assert_eq!(exit_cost_for(&locked, 7, edge, Direction::North, &basher), Cost::Steps(5));
    let neither = Capabilities {
        picklocks: 0,
        bash_doors: false,
        ..Capabilities::unrestricted()
    };
    assert_eq!(
        exit_cost_for(&locked, 7, edge, Direction::North, &neither),
        Cost::Impassable
    );
    assert_eq!(
        exit_cost_for(&unlocked, 7, edge, Direction::North, &neither),
        Cost::Steps(5),
        "an unlocked door only wants an open"
    );
    assert_eq!(
        exit_cost_for(&locked, 0xb, edge, Direction::North, &basher),
        Cost::Steps(5),
        "a gate is priced like a door"
    );
}

/// An item gate is a step with the item and a wall without it.
#[test]
fn an_item_gate_is_a_step_with_the_item_and_a_wall_without() {
    use mud_client::pack::PackHandle;
    use mud_client::sheet::Inventory;
    use mud_core::content::{Content, Item, ItemId};
    use std::sync::Arc;

    let edge = RoomId { map: 1, room: 1 };
    let gate = ExitRequirement::ItemGate { item: ItemId(1054) };
    assert_eq!(
        exit_cost_for(&gate, 3, edge, Direction::North, &Capabilities::unrestricted()),
        Cost::Impassable,
        "the unrestricted walker holds nothing"
    );
    let mut content = Content::default();
    content.add_item(Item {
        id: ItemId(1054),
        name: "silver pass".into(),
        ..Default::default()
    });
    let pack = PackHandle::new(Arc::new(content));
    pack.refresh(&Inventory {
        items: vec!["silver pass".into()],
        keys: Vec::new(),
        encumbrance: None,
    });
    let holder = Capabilities {
        pack: Some(pack),
        ..Capabilities::unrestricted()
    };
    assert_eq!(exit_cost_for(&gate, 3, edge, Direction::North, &holder), Cost::Steps(1));
}
