//! Map layout tests.
//!
//! Geometry is derived from the exit graph, not drawn.

use mud_client::graph::RoomGraph;
use mud_client::map::layout;
use mud_core::content::{Direction, RoomId};

// --- the plane records real exits, not grid adjacency -----------------

/// `A --east--> B` and `A --southeast--> D`. That places B at (1,0) and D
/// at (1,1), which makes B and D vertically adjacent on the grid -- with
/// nothing at all joining them. The plane must know the difference.
#[test]
fn the_plane_records_exits_and_not_adjacency() {
    use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom};
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let d = RoomId { map: 1, room: 4 };
    let mut ra = GraphRoom {
        name: "A".into(),
        ..Default::default()
    };
    ra.exits[Direction::East as usize] = Some(ExitEdge {
        dest: b,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    ra.exits[Direction::SouthEast as usize] = Some(ExitEdge {
        dest: d,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let rooms = vec![
        (a, ra),
        (
            b,
            GraphRoom {
                name: "B".into(),
                ..Default::default()
            },
        ),
        (
            d,
            GraphRoom {
                name: "D".into(),
                ..Default::default()
            },
        ),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);

    assert!(plane.has_edge(a, Direction::East), "A really does go east");
    assert!(
        plane.has_edge(a, Direction::SouthEast),
        "A really does go south-east"
    );

    // B is at (1,0) and D at (1,1) -- adjacent, and unconnected.
    assert_eq!(plane.cell_of(b), Some((1, 0)));
    assert_eq!(plane.cell_of(d), Some((1, 1)));
    assert!(
        !plane.has_edge(b, Direction::South),
        "nothing joins B to D; adjacency is not an exit"
    );
    assert!(
        !plane.has_edge(d, Direction::North),
        "and nothing joins D back to B"
    );
}
