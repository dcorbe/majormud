//! Roam regions and rotation, against the real WG3-NT database.
//!
//! The rooms are chosen for shape rather than convenience: the Small
//! Cavern is a dead end with a vertical link out, and the slum streets
//! are a connected grid with more than one way between any two rooms,
//! which is what makes "routed around" distinguishable from "refused".

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use mud_client::graph::RoomGraph;
use mud_client::roam::{Rotation, Walls, region};
use mud_core::content::RoomId;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn graph() -> &'static RoomGraph {
    use std::sync::OnceLock;
    static G: OnceLock<RoomGraph> = OnceLock::new();
    G.get_or_init(|| RoomGraph::load(&db_path()).expect("load room graph"))
}

const SLUM_ENTRANCE: RoomId = RoomId { map: 1, room: 1072 };
const SLUM_INTERSECTION: RoomId = RoomId { map: 1, room: 1074 };
const CROSSROADS: RoomId = RoomId { map: 1, room: 1076 };

/// The whole point: the region is not drawn, it falls out of where the
/// walls are. Fencing rooms shrinks it, and every room left is still
/// reachable from the seed.
#[test]
fn walls_shrink_the_region() {
    let g = graph();
    let open = region(g, SLUM_INTERSECTION, &Walls::default());
    let fenced = region(
        g,
        SLUM_INTERSECTION,
        &Walls::new([SLUM_ENTRANCE, CROSSROADS]),
    );

    assert!(
        fenced.len() < open.len(),
        "two walls should cut the region down: {} vs {}",
        fenced.len(),
        open.len()
    );
    assert!(!fenced.contains(&SLUM_ENTRANCE), "a wall is not in its own region");
    assert!(!fenced.contains(&CROSSROADS));
    assert!(fenced.contains(&SLUM_INTERSECTION), "the seed is always in");
    assert!(
        fenced.is_subset(&open),
        "fencing can only ever remove rooms"
    );
}

/// No walls is a legitimate way to say "roam this whole plane", not a
/// missing configuration.
#[test]
fn no_walls_is_the_whole_plane() {
    let g = graph();
    let all = region(g, SLUM_INTERSECTION, &Walls::default());
    assert!(
        all.len() > 100,
        "the Silvermere plane is large; got {}",
        all.len()
    );
    assert!(all.contains(&SLUM_ENTRANCE));
    assert!(all.contains(&CROSSROADS));
}

/// The plane rule is static and not negotiable: up, down and cross-map
/// exits are outside the region however few walls are placed. This is
/// what keeps the roamed area the same area the operator was looking at
/// when they placed the markers.
#[test]
fn the_region_never_leaves_the_plane() {
    let g = graph();
    let all = region(g, SLUM_INTERSECTION, &Walls::default());
    assert!(
        all.iter().all(|id| id.map == SLUM_INTERSECTION.map),
        "a region escaped to another map"
    );

    // And a vertical link really is refused. Newhaven's Narrow Road
    // (1/2146) reaches the Arena (1/2150) by a DOWN exit and nothing
    // else — the Arena's only way back is the matching up — so a roam
    // started on the road must not find itself in the pit.
    let road = RoomId { map: 1, room: 2146 };
    let arena = RoomId { map: 1, room: 2150 };
    assert!(
        g.route(road, arena).is_some(),
        "the fixture assumes the Arena is reachable at all"
    );
    let street = region(g, road, &Walls::default());
    assert!(
        !street.contains(&arena),
        "the roam took a vertical exit off its plane"
    );
    // The dungeon behind the Arena is horizontal, so it is off-plane for
    // the same reason and not by its own doing: unreachable without the
    // stair.
    assert!(!street.contains(&RoomId { map: 1, room: 2156 }));
}

/// Fencing yourself in is an answer, not a failure. A region of one room
/// is a vigil, which is a coherent thing to ask for.
#[test]
fn a_room_walled_on_every_side_is_a_region_of_one() {
    let g = graph();
    let here = SLUM_INTERSECTION;
    let neighbours: Vec<RoomId> = g
        .room(here)
        .expect("the intersection is in the graph")
        .exits
        .iter()
        .flatten()
        .map(|e| e.dest)
        .collect();
    assert!(!neighbours.is_empty(), "the fixture needs exits to wall off");

    let boxed = region(g, here, &Walls::new(neighbours));
    assert_eq!(boxed, BTreeSet::from([here]));
}

// --- rotation ---------------------------------------------------------

/// Never-visited beats long-ago-visited, which is what makes a fresh
/// roam sweep the region instead of settling into the first room it
/// happens to like.
#[test]
fn a_room_never_visited_is_taken_first() {
    let g = graph();
    let walls = Walls::default();
    let here = SLUM_INTERSECTION;
    let region = region(g, here, &Walls::new([CROSSROADS]));

    let now = Instant::now();
    let mut rot = Rotation::new();
    // Visit everything except one room, all of it long ago.
    let long_ago = now - Duration::from_secs(600);
    let mut unvisited = None;
    for &id in region.iter().filter(|&&id| id != here) {
        if unvisited.is_none() {
            unvisited = Some(id);
            continue;
        }
        rot.visited(id, long_ago);
    }
    let unvisited = unvisited.expect("the region has more than one room");

    assert_eq!(
        rot.next(g, here, &region, &walls),
        Some(unvisited),
        "an unseen room outranks every room seen ten minutes ago"
    );
}

/// Among rooms seen equally recently — which is every room on the first
/// pass — the nearest wins. Without the distance tie-break the choice
/// would fall to room-id order and walk the region end to end for
/// nothing.
#[test]
fn ties_are_broken_by_distance() {
    let g = graph();
    let walls = Walls::default();
    let here = SLUM_INTERSECTION;
    let region = region(g, here, &walls);
    let rot = Rotation::new();

    let picked = rot.next(g, here, &region, &walls).expect("somewhere to go");
    let hops = g.distances(here);
    assert_eq!(
        hops.get(&picked).copied(),
        Some(1),
        "with nothing visited, the next room should be adjacent"
    );
}

/// Visiting a room pushes it to the back of the rotation.
#[test]
fn visiting_a_room_defers_it() {
    let g = graph();
    let walls = Walls::default();
    let here = SLUM_INTERSECTION;
    let region = region(g, here, &walls);

    let mut rot = Rotation::new();
    let first = rot.next(g, here, &region, &walls).expect("somewhere to go");
    rot.visited(first, Instant::now());
    let second = rot.next(g, here, &region, &walls).expect("somewhere else");
    assert_ne!(first, second, "the rotation stalled on one room");
}

/// The one-room region has nowhere to move to, and says so rather than
/// pretending.
#[test]
fn a_region_of_one_has_no_next_room() {
    let g = graph();
    let here = SLUM_INTERSECTION;
    let neighbours: Vec<RoomId> = g
        .room(here)
        .unwrap()
        .exits
        .iter()
        .flatten()
        .map(|e| e.dest)
        .collect();
    let walls = Walls::new(neighbours);
    let region = region(g, here, &walls);
    assert_eq!(Rotation::new().next(g, here, &region, &walls), None);
}

// --- doors are outside a roam ------------------------------------------

/// The live incident this rule exists for, in the shipped data.
///
/// 1/1119 "Slum Street, Dead End" has a type-7 lock north into 1/2436.
/// On cwgaming, 2026-08-03, the board answered `open n` with "The door is
/// locked." and the walk sent 88 bashes across four approaches, spending
/// 36 hp of a 75-hp character on a lock that wanted Picklocks. The client
/// has no key handling at all, so force is its whole repertoire.
///
/// A roam always has somewhere else to be, so the door is simply not in
/// the region.
#[test]
fn a_roam_region_stops_at_a_door() {
    let g = graph();
    const DEAD_END: RoomId = RoomId { map: 1, room: 1119 };
    const BEHIND_THE_DOOR: RoomId = RoomId { map: 1, room: 2436 };

    // The fixture is only meaningful while the data still says so.
    let north = g
        .room(DEAD_END)
        .expect("the dead end is in the graph")
        .exits[mud_core::content::Direction::North as usize]
        .as_ref()
        .expect("it has a north exit");
    assert_eq!(north.dest, BEHIND_THE_DOOR);
    assert!(
        mud_client::nav::is_door(north.exit_type),
        "1/1119 north should still be a door; got exit type {}",
        north.exit_type
    );

    let region = region(g, DEAD_END, &Walls::default());
    assert!(region.contains(&DEAD_END), "the seed is always in");
    assert!(
        !region.contains(&BEHIND_THE_DOOR),
        "a roam must not include what is behind a locked door"
    );
}

/// And the walk agrees with the region: a fenced navigator will not route
/// through a door even when it is the only way, which is the honest
/// answer rather than 88 bashes.
#[test]
fn a_roaming_walk_will_not_route_through_a_door() {
    let g = graph();
    const DEAD_END: RoomId = RoomId { map: 1, room: 1119 };
    const BEHIND_THE_DOOR: RoomId = RoomId { map: 1, room: 2436 };

    let nav = mud_client::nav::Navigator::new(
        std::sync::Arc::new(RoomGraph::load(&db_path()).expect("graph")),
        mud_client::nav::NavConfig::default(),
    )
    .fenced(Walls::default(), DEAD_END.map);
    assert_eq!(nav.route_from(DEAD_END, BEHIND_THE_DOOR), None);

    // An ordinary walk still opens doors: this is a roam rule, not a
    // client-wide one. `/go` and a named circuit keep their bash budget.
    let open = mud_client::nav::Navigator::new(
        std::sync::Arc::new(RoomGraph::load(&db_path()).expect("graph")),
        mud_client::nav::NavConfig::default(),
    );
    assert!(
        open.route_from(DEAD_END, BEHIND_THE_DOOR).is_some(),
        "a patrol whose stop is behind a door still has to try it"
    );
    let _ = g;
}
