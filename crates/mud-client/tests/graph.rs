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
/// state.
#[test]
fn state_free_requirements_keep_their_old_prices() {
    let caps = Capabilities::unrestricted();
    let edge = RoomId { map: 1, room: 1 };
    for exit_type in [0, 2, 7, 0xb, 9, 0x18, 0x10, 0x14, 0x16, 0x17, 10, 0x13, 3, 5] {
        let req = ExitRequirement::from_exit_type(exit_type, 0);
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
