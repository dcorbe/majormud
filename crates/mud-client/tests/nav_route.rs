//! The route mode a walk uses is the navigator's own `[farm.nav].route`,
//! stamped onto whatever capabilities it is handed, the way `bash_doors`
//! already is. The session builds capabilities without knowing which
//! profile table the walk was configured from.

use std::sync::Arc;

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph, RouteMode};
use mud_client::nav::{NavConfig, Navigator};
use mud_core::content::{Direction, RoomId};

fn id(n: u16) -> RoomId {
    RoomId { map: 1, room: n }
}

fn plain(dest: RoomId) -> Option<ExitEdge> {
    Some(ExitEdge {
        dest,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    })
}

/// A to D in two steps through a room that spawns level 10 hostiles,
/// or in four steps round it.
fn detour_graph() -> Arc<RoomGraph> {
    let mut a = GraphRoom::default();
    a.exits[Direction::North as usize] = plain(id(2));
    a.exits[Direction::East as usize] = plain(id(3));
    let mut b = GraphRoom {
        hostile_level: Some(10),
        ..Default::default()
    };
    b.exits[Direction::North as usize] = plain(id(6));
    let mut c1 = GraphRoom::default();
    c1.exits[Direction::East as usize] = plain(id(4));
    let mut c2 = GraphRoom::default();
    c2.exits[Direction::North as usize] = plain(id(5));
    let mut c3 = GraphRoom::default();
    c3.exits[Direction::West as usize] = plain(id(6));
    Arc::new(RoomGraph::from_rooms(vec![
        (id(1), a),
        (id(2), b),
        (id(3), c1),
        (id(4), c2),
        (id(5), c3),
        (id(6), GraphRoom::default()),
    ]))
}

#[test]
fn a_navigator_routes_by_its_own_route_mode() {
    let level_five = Capabilities {
        level: Some(5),
        route: RouteMode::Short,
        ..Capabilities::unrestricted()
    };
    let safe = Navigator::new(
        detour_graph(),
        NavConfig {
            route: RouteMode::Safe,
            ..NavConfig::default()
        },
    )
    .with_capabilities(level_five.clone());
    assert_eq!(
        safe.route_from(id(1), id(6)),
        Some(vec![Direction::East, Direction::East, Direction::North, Direction::West]),
        "a safe navigator detours whatever the capabilities said"
    );
    let short = Navigator::new(detour_graph(), NavConfig::default()).with_capabilities(Capabilities {
        route: RouteMode::Safe,
        ..level_five
    });
    assert_eq!(
        short.route_from(id(1), id(6)),
        Some(vec![Direction::North, Direction::North]),
        "the default navigator takes the short way whatever the capabilities said"
    );
}
