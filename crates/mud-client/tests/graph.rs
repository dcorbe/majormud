//! Room-graph tests against the real WG3-NT database. Ground truths
//! computed with re/room_graph_wg.py (the decode reference).

use mud_client::graph::RoomGraph;
use mud_core::content::{Direction, RoomId};

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn graph() -> &'static RoomGraph {
    use std::sync::OnceLock;
    static G: OnceLock<RoomGraph> = OnceLock::new();
    G.get_or_init(|| RoomGraph::load(&db_path()).expect("load room graph"))
}

#[test]
fn loads_all_rooms() {
    assert_eq!(graph().len(), 26720);
}

#[test]
fn town_gates_decodes_exactly() {
    let g = graph();
    let gates = RoomId { map: 1, room: 1 };
    let room = g.room(gates).expect("room (1,1)");
    assert_eq!(room.name, "Town Gates");
    let n = room.exits[Direction::North as usize].as_ref().unwrap();
    assert_eq!(n.dest, RoomId { map: 1, room: 3 });
    assert_eq!(n.exit_type, 19);
    let s = room.exits[Direction::South as usize].as_ref().unwrap();
    assert_eq!(s.dest, RoomId { map: 1, room: 100 });
    assert_eq!(s.exit_type, 19);
    let e = room.exits[Direction::East as usize].as_ref().unwrap();
    assert_eq!(e.dest, RoomId { map: 1, room: 1381 });
    assert_eq!(e.exit_type, 11);
    let w = room.exits[Direction::West as usize].as_ref().unwrap();
    assert_eq!(w.dest, RoomId { map: 1, room: 101 });
    assert_eq!(w.exit_type, 0);
    assert_eq!(
        room.exits.iter().filter(|e| e.is_some()).count(),
        4,
        "exactly four exits"
    );
}

#[test]
fn route_to_adjacent_room() {
    let g = graph();
    let route = g
        .route(RoomId { map: 1, room: 1 }, RoomId { map: 1, room: 100 })
        .expect("route exists");
    assert_eq!(route, vec![Direction::South]);
}

#[test]
fn route_crosses_map_change_portal() {
    // (1,1) -> (15,982) is 3 steps; the last hop is a type-8 map-change
    // exit N from (1,1382) (ground truth: room_graph_wg.py BFS).
    let g = graph();
    let route = g
        .route(RoomId { map: 1, room: 1 }, RoomId { map: 15, room: 982 })
        .expect("route exists");
    assert_eq!(route.len(), 3);
    assert_eq!(*route.last().unwrap(), Direction::North);
}

#[test]
fn route_to_self_is_empty() {
    let g = graph();
    let route = g
        .route(RoomId { map: 1, room: 1 }, RoomId { map: 1, room: 1 })
        .expect("trivial route");
    assert!(route.is_empty());
}

#[test]
fn route_to_unknown_room_is_none() {
    let g = graph();
    assert!(
        g.route(RoomId { map: 1, room: 1 }, RoomId { map: 999, room: 9999 })
            .is_none()
    );
}

/// Threat ranking comes from the shipped data, not from a guess, and it
/// has to rank the Newhaven dungeon the way a player would: the cave bear
/// above the wanderers that drift into its lair.
#[test]
fn threat_ranks_the_dungeon_the_way_a_player_would() {
    let db = std::path::Path::new("../../re/mmud_wgnt.sqlite");
    if !db.exists() {
        eprintln!("skipping: {} not present", db.display());
        return;
    }
    let threat = RoomGraph::load_threat(db).expect("threat table");
    let get = |n: &str| *threat.get(n).unwrap_or_else(|| panic!("missing {n}"));

    assert!(get("cave bear") > get("acid slime"), "the bear is the boss here");
    assert!(get("acid slime") > get("kobold thief"));
    assert!(get("kobold thief") > get("filthbug"));
    assert!(get("filthbug") > get("giant rat"));
}

/// `distances` duplicates `route`'s BFS skeleton on purpose — one
/// early-exits and rebuilds a path, the other does neither — so the
/// invariant that keeps them honest has to be asserted rather than
/// assumed.
#[test]
fn distances_agree_with_route_lengths() {
    let g = graph();
    let from = RoomId { map: 1, room: 1 };
    let d = g.distances(from);
    assert_eq!(d.get(&from), Some(&0), "standing still costs nothing");
    for to in [
        RoomId { map: 1, room: 3 },
        RoomId { map: 1, room: 1072 },
        RoomId { map: 1, room: 2324 },
        RoomId { map: 1, room: 2146 },
    ] {
        let steps = g.route(from, to).expect("reachable").len();
        assert_eq!(d.get(&to), Some(&steps), "{to:?}");
    }
}

/// The Silvermere Small Alleyway (1/405) has a type-6 HIDDEN exit south
/// into the secret passage that runs under the Adventurer's Guild. A
/// hop-count router takes it — 24 steps against 27 by the street — and
/// the walk then stands in the alley sending `s` at a brick wall until it
/// desyncs (live, 2026-08-02, walking a ranger to its trainer).
///
/// Three steps is a cheap price for not gambling on three search rolls,
/// so the route has to leave by the only exit the board will show.
#[test]
fn a_route_prefers_three_more_streets_to_a_hidden_exit() {
    let g = graph();
    let alley = RoomId { map: 1, room: 405 };
    let trainer = RoomId { map: 1, room: 502 };
    let route = g.route(alley, trainer).expect("the guild is reachable");
    assert_eq!(
        route.first(),
        Some(&Direction::North),
        "left by the hidden exit instead of the street: {route:?}"
    );
    assert_eq!(route.len(), 27, "the street route, not the secret passage");
}

/// Costed, not forbidden. 1,383 shipped exits are hidden and whole areas
/// sit behind them, so a router that refused type 6 outright would answer
/// "no route" to rooms that are perfectly reachable — worse than the bug
/// it fixes.
#[test]
fn a_hidden_exit_is_still_taken_when_it_is_the_only_way() {
    let g = graph();
    let alley = RoomId { map: 1, room: 405 };
    let passage = RoomId { map: 1, room: 452 };
    assert_eq!(
        g.route(alley, passage),
        Some(vec![Direction::South]),
        "the secret passage is only reachable through its hidden exit"
    );
}

#[test]
fn distances_omit_unreachable_rooms() {
    let g = graph();
    let d = g.distances(RoomId { map: 1, room: 1 });
    assert!(!d.contains_key(&RoomId { map: 999, room: 9999 }));
    assert!(d.len() <= g.len());
}

/// A room that is not in the graph reaches nothing, rather than
/// reporting itself at distance zero.
#[test]
fn distances_from_an_unknown_room_are_empty() {
    assert!(
        graph()
            .distances(RoomId {
                map: 999,
                room: 9999
            })
            .is_empty()
    );
}
