//! The plan over a puzzle exit's concealment word: which levers, in
//! which order, and what routing pays for the trip.

use std::sync::Arc;

use mud_client::graph::{Capabilities, Cost};
use mud_client::pack::PackHandle;
use mud_client::puzzle::{Puzzle, PuzzleAction};
use mud_client::sheet::Inventory;
use mud_core::content::{Content, Item, ItemId, RoomId};

const EXIT_ROOM: RoomId = RoomId { map: 1, room: 1056 };
const LEVER_A: RoomId = RoomId { map: 1, room: 1044 };
const LEVER_B: RoomId = RoomId { map: 1, room: 1038 };
const FORK: ItemId = ItemId(500);

fn action(room: RoomId, number: u8, item: Option<ItemId>, hops: Option<u32>) -> PuzzleAction {
    PuzzleAction {
        room,
        number,
        phrases: vec!["pull lever".into()],
        item,
        reply: None,
        hops,
    }
}

/// A walker whose pack holds the titanium fork, the item 79 shipped
/// slots ask for.
fn with_fork() -> Capabilities {
    let mut content = Content::default();
    content.add_item(Item {
        id: FORK,
        name: "titanium fork".into(),
        ..Default::default()
    });
    let pack = PackHandle::new(Arc::new(content));
    pack.refresh(&Inventory {
        items: vec!["titanium fork".into()],
        keys: Vec::new(),
        encumbrance: None,
    });
    Capabilities {
        pack: Some(pack),
        ..Capabilities::unrestricted()
    }
}

/// A walker with no pack at all, which is what routing assumes until a
/// session hands one over.
fn empty_handed() -> Capabilities {
    Capabilities::unrestricted()
}

fn rooms(plan: &[&PuzzleAction]) -> Vec<RoomId> {
    plan.iter().map(|a| a.room).collect()
}

fn numbers(plan: &[&PuzzleAction]) -> Vec<u8> {
    plan.iter().map(|a| a.number).collect()
}

/// The shipped state words are runs of bits from 0x10 up, so the
/// actions a word needs are 1 through its bit count.
#[test]
fn the_word_names_the_actions_it_needs() {
    let needed = |word| {
        Puzzle {
            word,
            actions: Vec::new(),
        }
        .needed()
    };
    assert_eq!(needed(16), vec![1]);
    assert_eq!(needed(48), vec![1, 2]);
    assert_eq!(needed(240), vec![1, 2, 3, 4]);
    assert_eq!(needed(2032), vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(needed(0x2000), vec![10]);
    assert_eq!(
        needed(4),
        Vec::<u8>::new(),
        "bit 2 is the searchable flag, not a puzzle bit"
    );
}

/// Each action clears one bit. Action 0 clears every puzzle bit.
#[test]
fn an_action_knows_the_bit_it_clears() {
    assert_eq!(action(LEVER_A, 1, None, None).bit(), 0x10);
    assert_eq!(action(LEVER_A, 2, None, None).bit(), 0x20);
    assert_eq!(action(LEVER_A, 10, None, None).bit(), 0x2000);
    assert_eq!(action(LEVER_A, 0, None, None).bit(), 0x3ff0);
}

/// The Crypt pair: 1/1044 is action 1 and 1/1038 is action 2 on a word
/// of 48. The plan pulls the higher number first, which the ordered
/// rule requires and the unordered rule allows.
#[test]
fn a_crypt_lever_pair_pulls_the_higher_number_first() {
    let puzzle = Puzzle {
        word: 48,
        actions: vec![
            action(LEVER_A, 1, None, Some(1)),
            action(LEVER_B, 2, None, Some(1)),
        ],
    };
    let plan = puzzle.plan(&empty_handed()).expect("both levers are free");
    assert_eq!(rooms(&plan), vec![LEVER_B, LEVER_A]);
}

/// An action 0 clears the whole word in one phrase, so it beats pulling
/// four levers.
#[test]
fn an_action_zero_does_the_whole_job_in_one_phrase() {
    let puzzle = Puzzle {
        word: 240,
        actions: vec![
            action(LEVER_A, 1, None, Some(1)),
            action(LEVER_A, 2, None, Some(1)),
            action(LEVER_A, 3, None, Some(1)),
            action(LEVER_A, 4, None, Some(1)),
            action(EXIT_ROOM, 0, None, Some(0)),
        ],
    };
    let plan = puzzle.plan(&empty_handed()).expect("the button is free");
    assert_eq!(numbers(&plan), vec![0]);
    assert_eq!(rooms(&plan), vec![EXIT_ROOM]);
}

/// A word that needs a bit no action clears is a wall for everyone.
#[test]
fn a_missing_bit_has_no_plan() {
    let puzzle = Puzzle {
        word: 48,
        actions: vec![action(LEVER_A, 1, None, Some(1))],
    };
    assert_eq!(puzzle.plan(&with_fork()), None);
}

/// 86 shipped slots want an item, 79 of them a titanium fork. Without
/// it the action is not available and the plan fails. With it the plan
/// is the same as for a free lever.
#[test]
fn an_action_needing_an_item_counts_only_when_the_pack_holds_it() {
    let puzzle = Puzzle {
        word: 16,
        actions: vec![action(EXIT_ROOM, 1, Some(FORK), Some(0))],
    };
    assert_eq!(puzzle.plan(&empty_handed()), None);
    let plan = puzzle.plan(&with_fork()).expect("the fork is carried");
    assert_eq!(numbers(&plan), vec![1]);
}

/// An action 0 the walker cannot use is skipped in favour of the bits,
/// and used as soon as the pack allows.
#[test]
fn an_unavailable_action_zero_falls_back_to_the_bits() {
    let puzzle = Puzzle {
        word: 48,
        actions: vec![
            action(EXIT_ROOM, 0, Some(FORK), Some(0)),
            action(LEVER_A, 1, None, Some(1)),
            action(LEVER_B, 2, None, Some(1)),
        ],
    };
    let bare = puzzle.plan(&empty_handed()).expect("the levers are free");
    assert_eq!(numbers(&bare), vec![2, 1]);
    let forked = puzzle.plan(&with_fork()).expect("the button is now usable");
    assert_eq!(numbers(&forked), vec![0]);
}

/// A gate's word carries no puzzle bits: the lever toggles its lock, so
/// any one action does. The graph lists actions nearest first, and the
/// plan takes the first it can use.
#[test]
fn a_word_with_no_puzzle_bits_opens_on_the_nearest_usable_action() {
    let puzzle = Puzzle {
        word: 0,
        actions: vec![
            action(LEVER_A, 0, Some(FORK), Some(1)),
            action(LEVER_B, 0, None, Some(3)),
        ],
    };
    let bare = puzzle.plan(&empty_handed()).expect("the far lever is free");
    assert_eq!(rooms(&bare), vec![LEVER_B]);
    let forked = puzzle.plan(&with_fork()).expect("the near lever is usable");
    assert_eq!(rooms(&forked), vec![LEVER_A]);
    let nothing = Puzzle {
        word: 0,
        actions: Vec::new(),
    };
    assert_eq!(nothing.plan(&with_fork()), None);
}

/// The price is the exit's own base, then one step for each phrase and
/// two per hop for the walk to the lever and back.
#[test]
fn the_cost_is_the_base_plus_a_phrase_and_a_round_trip_per_action() {
    let button = Puzzle {
        word: 16,
        actions: vec![action(EXIT_ROOM, 1, None, Some(0))],
    };
    assert_eq!(button.cost(1, &empty_handed()), Cost::Steps(2));
    let pair = Puzzle {
        word: 48,
        actions: vec![
            action(LEVER_A, 1, None, Some(1)),
            action(LEVER_B, 2, None, Some(1)),
        ],
    };
    assert_eq!(pair.cost(1, &empty_handed()), Cost::Steps(7));
    assert_eq!(pair.cost(5, &empty_handed()), Cost::Steps(11), "a gate keeps its door price");
}

/// Two ways to be a wall: no plan, or a plan through a room no walk
/// reaches.
#[test]
fn no_plan_or_an_unreachable_lever_room_is_impassable() {
    let needs_fork = Puzzle {
        word: 16,
        actions: vec![action(EXIT_ROOM, 1, Some(FORK), Some(0))],
    };
    assert_eq!(needs_fork.cost(1, &empty_handed()), Cost::Impassable);
    let unreachable = Puzzle {
        word: 16,
        actions: vec![action(LEVER_A, 1, None, None)],
    };
    assert_eq!(unreachable.cost(1, &with_fork()), Cost::Impassable);
}
