//! Farm runner tests. This file covers the configuration layer: turning
//! a `[farm]` TOML table into a plan that is known-good *before* the
//! client connects, so a typo in a room id fails at startup rather than
//! halfway around a patrol circuit.

use mud_client::farm::{FarmConfig, FarmPlan, parse_room_id};
use mud_client::graph::{ExitEdge, GraphRoom, RoomGraph};
use mud_core::content::{Direction, RoomId};

fn rid(map: u16, room: u16) -> RoomId {
    RoomId { map, room }
}

/// gates -N-> square -E-> market, all mutually reachable, plus room 9
/// ("Sealed Vault") with no edges at all — the unroutable case.
fn graph() -> RoomGraph {
    let mk = |n: u16, name: &str, exits: Vec<(Direction, u16)>| {
        let mut room = GraphRoom {
            name: name.into(),
            exits: Default::default(),
        };
        for (d, dest) in exits {
            room.exits[d as usize] = Some(ExitEdge {
                dest: rid(1, dest),
                exit_type: 0,
            });
        }
        (rid(1, n), room)
    };
    RoomGraph::from_rooms(vec![
        mk(1, "Town Gates", vec![(Direction::North, 2)]),
        mk(
            2,
            "Town Square",
            vec![(Direction::South, 1), (Direction::East, 3)],
        ),
        mk(3, "Market Street", vec![(Direction::West, 2)]),
        mk(9, "Sealed Vault", vec![]),
    ])
}

fn config(start: &str, circuit: &[&str]) -> FarmConfig {
    FarmConfig {
        start: start.into(),
        circuit: circuit.iter().map(|s| (*s).to_string()).collect(),
        ..FarmConfig::default()
    }
}

#[test]
fn parses_map_slash_room() {
    assert_eq!(parse_room_id("1/860"), Some(rid(1, 860)));
    assert_eq!(parse_room_id("12/3"), Some(rid(12, 3)));
}

#[test]
fn rejects_room_ids_that_are_not_map_slash_room() {
    assert_eq!(parse_room_id("1-2"), None);
    assert_eq!(parse_room_id("1"), None);
    assert_eq!(parse_room_id(""), None);
    assert_eq!(parse_room_id("a/b"), None);
    assert_eq!(parse_room_id("1/2/3"), None);
}

#[test]
fn builds_a_plan_from_valid_ids() {
    let plan = FarmPlan::build(&config("1/1", &["1/2", "1/3"]), &graph()).unwrap();
    assert_eq!(plan.start, rid(1, 1));
    assert_eq!(plan.circuit, vec![rid(1, 2), rid(1, 3)]);
}

#[test]
fn rejects_an_empty_circuit() {
    let err = FarmPlan::build(&config("1/1", &[]), &graph()).unwrap_err();
    assert!(err.contains("circuit"), "unhelpful error: {err}");
}

#[test]
fn rejects_a_malformed_room_id() {
    let err = FarmPlan::build(&config("1/1", &["1-2"]), &graph()).unwrap_err();
    assert!(err.contains("1-2"), "error should name the bad id: {err}");
}

#[test]
fn rejects_a_room_the_graph_does_not_know() {
    let err = FarmPlan::build(&config("1/1", &["1/404"]), &graph()).unwrap_err();
    assert!(err.contains("1/404"), "error should name the room: {err}");
}

/// The Sealed Vault is in the graph but has no edges, so no leg can
/// reach it. Catching this at startup is the whole point of the plan.
#[test]
fn rejects_a_circuit_leg_with_no_route() {
    let err = FarmPlan::build(&config("1/1", &["1/2", "1/9"]), &graph()).unwrap_err();
    assert!(err.contains("route"), "unhelpful error: {err}");
}

/// The lap wraps: the last stop must be able to reach the first, or the
/// bot completes one loop and strands itself.
#[test]
fn rejects_a_circuit_whose_wrap_around_has_no_route() {
    let g = RoomGraph::from_rooms(vec![
        (
            rid(1, 1),
            GraphRoom {
                name: "Town Gates".into(),
                exits: {
                    let mut e: [Option<ExitEdge>; 10] = Default::default();
                    e[Direction::North as usize] = Some(ExitEdge {
                        dest: rid(1, 2),
                        exit_type: 0,
                    });
                    e
                },
            },
        ),
        (
            rid(1, 2),
            GraphRoom {
                name: "One Way Ditch".into(),
                exits: Default::default(),
            },
        ),
    ]);
    // start -> 1/2 routes, but 1/2 -> 1/2 wrap is fine; the failing leg
    // is a two-stop circuit that cannot get back to the first stop.
    let err = FarmPlan::build(&config("1/1", &["1/1", "1/2"]), &g).unwrap_err();
    assert!(err.contains("route"), "unhelpful error: {err}");
}

#[test]
fn rejects_a_start_the_graph_does_not_know() {
    let err = FarmPlan::build(&config("1/404", &["1/2"]), &graph()).unwrap_err();
    assert!(err.contains("1/404"), "error should name the room: {err}");
}

/// A one-stop circuit is legal: sit in one room forever. Its wrap-around
/// leg is zero-length and must not be treated as unroutable.
#[test]
fn accepts_a_single_stop_circuit() {
    let plan = FarmPlan::build(&config("1/1", &["1/2"]), &graph()).unwrap();
    assert_eq!(plan.circuit, vec![rid(1, 2)]);
}
