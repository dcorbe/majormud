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
