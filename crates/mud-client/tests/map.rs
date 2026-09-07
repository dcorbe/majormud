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

// --- a plane crosses map boundaries, but only sideways ------------------

use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom};

fn room(name: &str, exits: &[(Direction, RoomId)]) -> GraphRoom {
    let mut r = GraphRoom {
        name: name.into(),
        ..Default::default()
    };
    for (dir, dest) in exits {
        r.exits[*dir as usize] = Some(ExitEdge {
            dest: *dest,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
    }
    r
}

fn id(map: u16, room: u16) -> RoomId {
    RoomId { map, room }
}

/// The Western Road: `1/1 --west--> 2/1 --west--> 2/2`, with the road
/// back east. The next map is drawn where the road leads, joined by a
/// connector, and nothing is left as a link to hop.
#[test]
fn a_compass_exit_into_the_next_map_is_drawn_on_the_plane() {
    let (a, b, c) = (id(1, 1), id(2, 1), id(2, 2));
    let graph = RoomGraph::from_rooms(vec![
        (a, room("Silvermere Entrance", &[(Direction::West, b)])),
        (b, room("Western Road", &[(Direction::East, a), (Direction::West, c)])),
        (c, room("Western Road", &[(Direction::East, b)])),
    ]);
    let plane = layout(&graph, a);

    assert_eq!(plane.cell_of(b), Some((-1, 0)));
    assert_eq!(plane.cell_of(c), Some((-2, 0)));
    assert!(plane.has_edge(a, Direction::West), "the road crosses the seam");
    assert!(plane.has_edge(b, Direction::East), "and comes back over it");
    assert!(plane.has_edge(b, Direction::West), "and carries on inside map 2");
    assert!(plane.links().is_empty(), "a map boundary is not a hop");
    assert!(plane.conflicts().is_empty());
    assert_eq!(plane.maps(), 2);
}

/// Every map the compass exits reach joins, not just the neighbours:
/// `1/1 --west--> 2/1 --west--> 3/1` draws all three in a row.
#[test]
fn every_map_the_compass_exits_reach_joins_the_plane() {
    let (a, b, c) = (id(1, 1), id(2, 1), id(3, 1));
    let graph = RoomGraph::from_rooms(vec![
        (a, room("A", &[(Direction::West, b)])),
        (b, room("B", &[(Direction::West, c)])),
        (c, room("C", &[])),
    ]);
    let plane = layout(&graph, a);

    assert_eq!(plane.cell_of(c), Some((-2, 0)));
    assert!(plane.links().is_empty());
    assert_eq!(plane.maps(), 3);
}

/// The map 16 caves. From the anchor a stub in map 1 is one step north,
/// while the cave that wants that same cell is two steps away through
/// the west. A shortest-walk layout would seat the stub and cut the rest
/// of the caves off behind the conflict. The map the anchor is on is
/// laid out whole first, so the cave keeps its cell and the stub is the
/// room that goes unplaced.
#[test]
fn the_home_map_keeps_its_cell_when_a_neighbour_wants_it() {
    let (anchor, west, cave, stub) = (id(16, 1), id(16, 2), id(16, 3), id(1, 1));
    let graph = RoomGraph::from_rooms(vec![
        (
            anchor,
            room(
                "Dark Cave",
                &[(Direction::North, stub), (Direction::West, west)],
            ),
        ),
        (west, room("Dark Cave", &[(Direction::NorthEast, cave)])),
        (cave, room("Dark Cave", &[])),
        (stub, room("Dark Cave", &[])),
    ]);
    let plane = layout(&graph, anchor);

    assert_eq!(plane.cell_of(cave), Some((0, -1)), "the home map's room wins");
    assert_eq!(plane.cell_of(stub), None, "the neighbour's room is the one unplaced");
    assert_eq!(plane.conflicts().len(), 1);
    assert_eq!(plane.conflicts()[0].dest, stub);
    assert_eq!(plane.conflicts()[0].cell, (0, -1));
    assert!(
        !plane.has_edge(anchor, Direction::North),
        "no connector toward a room that is not there"
    );
}

/// Up and down never take a cell, whichever map they lead to.
#[test]
fn up_and_down_still_leave_the_plane() {
    let (a, above, below) = (id(1, 1), id(1, 2), id(2, 1));
    let graph = RoomGraph::from_rooms(vec![
        (
            a,
            room(
                "Stairwell",
                &[(Direction::Up, above), (Direction::Down, below)],
            ),
        ),
        (above, room("Landing", &[])),
        (below, room("Cellar", &[])),
    ]);
    let plane = layout(&graph, a);

    assert_eq!(plane.len(), 1);
    let links: Vec<(Direction, RoomId)> = plane.links().iter().map(|l| (l.dir, l.dest)).collect();
    assert_eq!(
        links,
        vec![(Direction::Up, above), (Direction::Down, below)]
    );
}

// --- a stairwell says which way it goes ---------------------------------

use mud_client::map::{Paint, PaintCtx, styles};
use mud_client::spawn::SpawnTable;

fn glyph_of(exits: &[(Direction, RoomId)]) -> char {
    let a = id(1, 1);
    let mut rooms = vec![(a, room("Stairwell", exits))];
    for (_, dest) in exits {
        rooms.push((*dest, room("Elsewhere", &[])));
    }
    let graph = RoomGraph::from_rooms(rooms);
    let plane = layout(&graph, a);
    let styles = styles(
        &plane,
        &graph,
        &SpawnTable::default(),
        Paint::Terrain,
        &PaintCtx::default(),
    );
    styles[&a].glyph
}

#[test]
fn a_stairwell_glyph_points_the_way_the_stair_goes() {
    let (above, below, beside) = (id(1, 2), id(1, 3), id(1, 4));
    assert_eq!(glyph_of(&[(Direction::Up, above)]), '\u{2191}', "up");
    assert_eq!(glyph_of(&[(Direction::Down, below)]), '\u{2193}', "down");
    assert_eq!(
        glyph_of(&[(Direction::Up, above), (Direction::Down, below)]),
        '\u{2195}',
        "both"
    );
    assert_eq!(
        glyph_of(&[(Direction::East, beside)]),
        mud_client::map::ROOM,
        "a compass exit is not a stair"
    );
}
