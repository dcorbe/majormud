//! The region a roam covers grows through the puzzles this character
//! can solve and stops at the ones it cannot.
//!
//! Before this the flood ran as an unrestricted walker, so a puzzle
//! whose lever wanted a titanium fork was inside every roam and the
//! rotation could pick a room the walk then could not reach.

use std::collections::BTreeSet;
use std::sync::Arc;

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::pack::PackHandle;
use mud_client::puzzle::{Puzzle, PuzzleAction};
use mud_client::roam::{Rotation, Walls, region};
use mud_client::sheet::Inventory;
use mud_core::content::{Content, Direction, Item, ItemId, RoomId};

const POST: RoomId = RoomId { map: 1, room: 1 };
const VAULT: RoomId = RoomId { map: 1, room: 2 };
const FORK: ItemId = ItemId(500);

fn room(name: &str, exits: &[(Direction, RoomId)]) -> GraphRoom {
    let mut r = GraphRoom {
        name: name.into(),
        ..Default::default()
    };
    for (d, dest) in exits {
        r.exits[*d as usize] = Some(ExitEdge {
            dest: *dest,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
    }
    r
}

/// A guard post whose north wall is a puzzle opened by a button in the
/// post itself, needing `item` if any.
fn world(item: Option<ItemId>) -> RoomGraph {
    let mut post = room("Guard Post", &[]);
    post.exits[Direction::North as usize] = Some(ExitEdge {
        dest: VAULT,
        exit_type: 6,
        command: None,
        requirement: ExitRequirement::Puzzle(Puzzle {
            word: 16,
            actions: vec![PuzzleAction {
                room: POST,
                number: 1,
                phrases: vec!["push button".into()],
                item,
                reply: None,
                hops: Some(0),
            }],
        }),
    });
    RoomGraph::from_rooms(vec![
        (POST, post),
        (VAULT, room("Vault", &[(Direction::South, POST)])),
    ])
}

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

fn empty_handed() -> Capabilities {
    Capabilities::unrestricted()
}

#[test]
fn a_region_grows_through_a_solvable_puzzle() {
    let g = world(None);
    let got = region(&g, POST, &Walls::default(), &empty_handed());
    assert_eq!(got, BTreeSet::from([POST, VAULT]));
}

#[test]
fn a_region_stops_at_a_puzzle_the_pack_cannot_solve() {
    let g = world(Some(FORK));
    assert_eq!(
        region(&g, POST, &Walls::default(), &empty_handed()),
        BTreeSet::from([POST]),
        "no fork, no vault"
    );
    assert_eq!(
        region(&g, POST, &Walls::default(), &with_fork()),
        BTreeSet::from([POST, VAULT]),
        "the fork opens the vault"
    );
}

/// The rotation asks the same question the flood did. A region seeded
/// by a walker with the fork, handed to a rotation without it, never
/// sends the character at the vault.
#[test]
fn the_rotation_never_picks_a_room_it_cannot_reach() {
    let g = world(Some(FORK));
    let both = region(&g, POST, &Walls::default(), &with_fork());
    let rotation = Rotation::new();
    assert_eq!(
        rotation.next(&g, POST, &both, &Walls::default(), &with_fork()),
        Some(VAULT)
    );
    assert_eq!(
        rotation.next(&g, POST, &both, &Walls::default(), &empty_handed()),
        None
    );
}

/// A guard post whose north exit wants the fork carried.
fn gated_world() -> RoomGraph {
    let mut post = room("Guard Post", &[]);
    post.exits[Direction::North as usize] = Some(ExitEdge {
        dest: VAULT,
        exit_type: 3,
        command: None,
        requirement: ExitRequirement::ItemGate { item: FORK },
    });
    RoomGraph::from_rooms(vec![
        (POST, post),
        (VAULT, room("Vault", &[(Direction::South, POST)])),
    ])
}

/// An item gate is inside a roam for the character carrying the item
/// and a wall for one who is not. Doors stay outside either way.
#[test]
fn a_region_grows_through_an_item_gate_only_with_the_item() {
    let graph = gated_world();
    let walls = Walls::default();
    assert_eq!(
        region(&graph, POST, &walls, &with_fork()),
        BTreeSet::from([POST, VAULT])
    );
    assert_eq!(
        region(&graph, POST, &walls, &empty_handed()),
        BTreeSet::from([POST])
    );
}
