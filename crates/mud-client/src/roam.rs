//! Wandering a region the operator fenced off, rather than walking a
//! circuit they enumerated.
//!
//! A [`Loop`](crate::loops::Loop) says where to go, in order. This says
//! where NOT to go, and the region falls out of that: everything the
//! character can reach from where it stands without crossing a wall or
//! meeting a puzzle exit this character's own capabilities cannot open.
//! The difference matters for the shape of an area — a loop of an
//! irregular region needs a stop per room and an order chosen by hand,
//! while two wall markers can box off a corridor and cost nothing more.
//!
//! **Nothing here persists.** A roam is a once-off: the walls live in the
//! map view, are handed to the runner when the roam starts, and die with
//! it. That is deliberate. A wall that outlived its run would silently
//! shape a later one, and the operator would have no file to look at and
//! no reason to suspect one — the worst kind of stale configuration.
//!
//! **The plane rule is not configurable.** Up, down and anything crossing
//! to another map leave the region, which is the same test the map
//! renderer uses to decide what belongs on a drawn plane
//! (`crate::map::layout`). Keeping the two in agreement is the point: the
//! region a roam covers is exactly the region the operator was looking at
//! when they placed the markers, and a roam that wandered off the screen
//! they were reading would be fencing an area nobody had seen.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use mud_core::content::{Direction, RoomId};

use crate::graph::{Capabilities, ExitEdge, RoomGraph};

/// Rooms the roam must never enter.
///
/// A set, not an ordered list: markers have no sequence, and two placed
/// on the same room are one marker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Walls(BTreeSet<RoomId>);

impl Walls {
    pub fn new(rooms: impl IntoIterator<Item = RoomId>) -> Walls {
        Walls(rooms.into_iter().collect())
    }

    pub fn contains(&self, id: RoomId) -> bool {
        self.0.contains(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = RoomId> + '_ {
        self.0.iter().copied()
    }
}

/// Is this a step that stays on the plane the roam is confined to?
///
/// The same test as `crate::map::layout`'s: up and down do not take a
/// cell in a 2D layout, and neither does an exit into another map, so
/// none of the three is inside the region. Written here as a predicate
/// rather than shared with the renderer because the renderer wants the
/// grid offset and this only wants the yes/no — but if one changes the
/// other must, and they are pinned against each other by test.
fn on_plane(plane: u16, dir: Direction, edge: &ExitEdge) -> bool {
    !matches!(dir, Direction::Up | Direction::Down) && edge.dest.map == plane
}

/// The edge predicate a roam routes and floods with.
///
/// Both uses take it from here so they cannot disagree: a region that
/// contained a room the router refused to reach would send the runner
/// somewhere it could then never leave.
///
/// **Doors are outside a roam, all of them.** Not a preference and not
/// costed — refused, exactly like a wall.
///
/// The client has no key handling of any kind: keys in the inventory are
/// not even parsed, exits' key ids are not read, and there is no `unlock`
/// verb. So the entire repertoire for a shut door is `open`, then bash
/// until a counter runs out. Live on cwgaming, 2026-08-03, at 1/1119
/// Slum Street Dead End: the board answered `open n` with **"The door is
/// locked."** and the walk then sent 88 bashes across four approaches,
/// spending 36 hp of a 75-hp character on a type-7 lock that wanted
/// Picklocks and was never going to yield to force.
///
/// A roam is a wander with no destination that matters — there is always
/// another room — so the cost of skipping a door is nothing and the cost
/// of trying one is measured in health. A patrol with a circuit is a
/// different bargain: its stops were named by the operator and a door in
/// the way has to be opened, so `[farm.nav].bash_doors` still governs
/// there and is untouched.
///
/// **Buttons and levers are inside a roam.** A puzzle exit is costed,
/// not refused: the region floods with the character's own
/// capabilities, so a passage this character can open is in and one
/// that wants an item the pack lacks is out. A lever room outside the
/// fence is the one case the flood cannot see. The walk then fails the
/// leg with a puzzle error, the room leaves the roam for the rest of
/// the run, and the rotation moves on.
///
/// Revisit when there is something better than force to offer a lock.
pub fn passable(plane: u16, walls: &Walls) -> impl Fn(Direction, &ExitEdge) -> bool + '_ {
    move |dir, edge| {
        on_plane(plane, dir, edge)
            && !walls.contains(edge.dest)
            && !crate::nav::is_door(edge.exit_type)
    }
}

/// Every room the character can reach from `from` without crossing a
/// wall, leaving the plane, or meeting an exit it cannot open.
///
/// `caps` is the character's own: a button that wants an item the pack
/// lacks is a wall for this character and the room behind it is not in
/// the region. Always contains `from`, so an empty answer is impossible
/// and callers need no special case for one. A region of exactly one
/// room IS possible, walls on every side, and is a legitimate answer
/// meaning "you fenced yourself in", not a failure.
pub fn region(graph: &RoomGraph, from: RoomId, walls: &Walls, caps: &Capabilities) -> BTreeSet<RoomId> {
    if graph.room(from).is_none() {
        return BTreeSet::new();
    }
    let allow = passable(from.map, walls);
    graph.distances_within_for(from, &allow, caps).into_keys().collect()
}

/// Which room to work next.
///
/// **Least-recently-visited, nearest on ties.** A room never visited
/// sorts before every room that has been, so a fresh roam sweeps outward
/// rather than settling; after that the rotation naturally gives each
/// room the longest recovery its size allows, which is the whole point on
/// a spawner that has to be walked back into to be checked.
///
/// Distance breaks ties because time alone does not: on a first pass
/// every room is equally unvisited, and without the distance term the
/// choice would be whatever the room id ordering happened to put first —
/// a walk across the region and back for no reason.
#[derive(Debug, Clone, Default)]
pub struct Rotation {
    seen: BTreeMap<RoomId, Instant>,
}

impl Rotation {
    pub fn new() -> Rotation {
        Rotation::default()
    }

    /// Note that the character worked this room at `now`.
    pub fn visited(&mut self, id: RoomId, now: Instant) {
        self.seen.insert(id, now);
    }

    /// Where to go next from `from`, within `region`, for a walker with
    /// `caps`. Same capabilities as the flood that built the region, or
    /// a room the flood counted could be one this walker cannot reach.
    ///
    /// `None` means there is nowhere else to be: the region is the one
    /// room the character already stands in. The caller should keep
    /// working that room rather than treat it as an ending, since a
    /// fenced-in single room is a vigil, which is a coherent thing to
    /// ask for.
    pub fn next(
        &self,
        graph: &RoomGraph,
        from: RoomId,
        region: &BTreeSet<RoomId>,
        walls: &Walls,
        caps: &Capabilities,
    ) -> Option<RoomId> {
        let allow = passable(from.map, walls);
        let hops = graph.distances_within_for(from, &allow, caps);
        region
            .iter()
            .copied()
            .filter(|&id| id != from)
            // Unreachable under the fence: it is in the region by the
            // seed's reckoning but not from HERE, which a one-way exit
            // can do. Skip rather than route at it and fail.
            .filter_map(|id| hops.get(&id).map(|&h| (id, h)))
            .min_by_key(|&(id, h)| (self.seen.get(&id).copied(), h, id))
            .map(|(id, _)| id)
    }
}
