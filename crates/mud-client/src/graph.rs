//! Client-side room graph, loaded from the decoded WG3-NT database
//! (`re/mmud_wgnt.sqlite`). Exit decode rules follow
//! `re/room_graph_wg.py`: direction index 0..9 = N,S,E,W,NE,NW,SE,SW,
//! U,D; dest room = `roomexit_<d+1>` (>0); dest map = `para1_<d+1>`
//! when `roomtype_<d+1>` == 8 (map-change portal), else the room's own
//! map. Placeholder rows (map outside 1..=999, room < 1) are skipped.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

use mud_core::content::{Direction, RoomId};

const DIRECTIONS: [Direction; 10] = [
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
    Direction::NorthEast,
    Direction::NorthWest,
    Direction::SouthEast,
    Direction::SouthWest,
    Direction::Up,
    Direction::Down,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitEdge {
    pub dest: RoomId,
    pub exit_type: i64,
}

#[derive(Debug, Clone, Default)]
pub struct GraphRoom {
    pub name: String,
    pub exits: [Option<ExitEdge>; 10],
    /// The room table's `light` column: 0 is normal, negatives need a
    /// light source (Small Cavern 1/2156 = -200; ~17k shipped rooms are
    /// negative). The exact cutoff is ORACLE-OPEN — the bracketing
    /// evidence is that 0 renders (Arena, Dungeon Entrance) and -175 /
    /// -200 are dark (live captures 2026-07-31) — so `light < 0` is
    /// read as "assume dark": a false positive costs one cheap
    /// pre-light, a false negative costs a blind fight.
    pub light: i64,
}

pub struct RoomGraph {
    rooms: BTreeMap<RoomId, GraphRoom>,
}

impl RoomGraph {
    pub fn load(db: &Path) -> Result<Self, String> {
        let conn = rusqlite::Connection::open_with_flags(
            db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("open {}: {e}", db.display()))?;
        let mut cols = vec!["mapnumber".to_string(), "roomnumber".into(), "name".into()];
        for i in 1..=10 {
            cols.push(format!("roomexit_{i}"));
        }
        for i in 1..=10 {
            cols.push(format!("roomtype_{i}"));
        }
        for i in 1..=10 {
            cols.push(format!("para1_{i}"));
        }
        cols.push("light".into());
        let sql = format!("SELECT {} FROM room", cols.join(","));
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut rooms = BTreeMap::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let map: i64 = row.get(0).map_err(|e| e.to_string())?;
            let room: i64 = row.get(1).map_err(|e| e.to_string())?;
            if !(1..=999).contains(&map) || room < 1 {
                continue;
            }
            let name: Option<String> = row.get(2).map_err(|e| e.to_string())?;
            let light: i64 = row.get(33).unwrap_or(0);
            let mut graph_room = GraphRoom {
                name: name.unwrap_or_default(),
                exits: Default::default(),
                light,
            };
            for d in 0..10 {
                let dest: i64 = row.get(3 + d).map_err(|e| e.to_string())?;
                if dest <= 0 {
                    continue;
                }
                let exit_type: i64 = row.get(13 + d).map_err(|e| e.to_string())?;
                let para1: i64 = row.get(23 + d).map_err(|e| e.to_string())?;
                let dmap = if exit_type == 8 { para1 } else { map };
                let (Ok(dmap), Ok(dest)) = (u16::try_from(dmap), u16::try_from(dest)) else {
                    continue; // malformed edge; never alias via lossy casts
                };
                graph_room.exits[d] = Some(ExitEdge {
                    dest: RoomId {
                        map: dmap,
                        room: dest,
                    },
                    exit_type,
                });
            }
            let (Ok(map), Ok(room)) = (u16::try_from(map), u16::try_from(room)) else {
                continue;
            };
            rooms.insert(RoomId { map, room }, graph_room);
        }
        Ok(RoomGraph { rooms })
    }

    /// Build from in-memory rooms (tests, synthetic worlds).
    pub fn from_rooms(rooms: Vec<(RoomId, GraphRoom)>) -> Self {
        RoomGraph {
            rooms: rooms.into_iter().collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How dangerous each monster template is, keyed by lowercase name.
    ///
    /// Scored on `experience` — the board's own valuation of how hard a
    /// thing is — with `hitpoints` breaking ties, which ranks the shipped
    /// Newhaven dungeon the way a player would: cave bear 100 above acid
    /// slime 16, kobold thief 13, filthbug 12, giant rat 9.
    ///
    /// Deliberately not the client's own damage model. The exp figure is
    /// data rather than a guess, and it is the one number the board
    /// already publishes about difficulty.
    ///
    /// Duplicate names exist (there are eight "giant rat" rows); the
    /// highest-scoring row wins, so a shared name is never ranked below
    /// its most dangerous variant.
    pub fn load_threat(db: &Path) -> Result<crate::bot::ThreatTable, String> {
        let conn = rusqlite::Connection::open_with_flags(
            db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("open {}: {e}", db.display()))?;
        let mut stmt = conn
            .prepare("select lower(name), experience, hitpoints from monster where name != ''")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut table = crate::bot::ThreatTable::new();
        for row in rows {
            let (name, exp, hp) = row.map_err(|e| e.to_string())?;
            let score = exp * 1000 + hp;
            let slot = table.entry(name).or_insert(score);
            if score > *slot {
                *slot = score;
            }
        }
        Ok(table)
    }

    pub fn room(&self, id: RoomId) -> Option<&GraphRoom> {
        self.rooms.get(&id)
    }

    /// Does the graph mark this room as needing a light source? See
    /// [`GraphRoom::light`] for the threshold's evidence and its
    /// ORACLE-OPEN status. Unknown rooms are not assumed dark.
    pub fn dark(&self, id: RoomId) -> bool {
        self.rooms.get(&id).is_some_and(|r| r.light < 0)
    }

    /// Every room carrying this exact name.
    ///
    /// Usually one, but not always — "Newhaven, Narrow Road" is two rooms
    /// (1/2146 and 1/2151), and "Dungeon, Old Mineshaft" is fourteen. A
    /// caller must therefore treat the result as candidates to narrow,
    /// never as an answer; see [`crate::nav::Navigator::localize_view`],
    /// which separates them by their exits.
    ///
    /// Linear over ~26k rooms, which is why it sits behind the cheap
    /// neighbour check rather than in front of it.
    pub fn rooms_named(&self, name: &str) -> Vec<RoomId> {
        self.rooms
            .iter()
            .filter(|(_, r)| r.name == name)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Every room, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (RoomId, &GraphRoom)> {
        self.rooms.iter().map(|(id, room)| (*id, room))
    }

    /// Step counts from `from` to every room it can reach.
    ///
    /// One BFS, so ranking a hundred same-named candidates costs what
    /// routing to one of them does. That is the whole reason this exists
    /// beside [`RoomGraph::route`]: `/go Slum Street` matches 152 rooms,
    /// and ranking those by calling `route` in a loop would be 152 full
    /// traversals over ~26k rooms — seconds of blocking work on the path
    /// that handles a keystroke.
    ///
    /// The BFS skeleton is duplicated rather than shared because `route`
    /// early-exits on its target and tracks parents to rebuild the path;
    /// this does neither, and folding both into one function would cost
    /// more in branches than the dozen lines it saved.
    pub fn distances(&self, from: RoomId) -> BTreeMap<RoomId, usize> {
        let mut seen = BTreeMap::new();
        if !self.rooms.contains_key(&from) {
            return seen;
        }
        seen.insert(from, 0);
        let mut queue = VecDeque::from([from]);
        while let Some(cur) = queue.pop_front() {
            let steps = seen[&cur] + 1;
            for edge in self.rooms[&cur].exits.iter().flatten() {
                if !self.rooms.contains_key(&edge.dest) || seen.contains_key(&edge.dest) {
                    continue;
                }
                seen.insert(edge.dest, steps);
                queue.push_back(edge.dest);
            }
        }
        seen
    }

    /// Shortest route as direction steps (BFS over exits into known
    /// rooms). `None` when unreachable; empty when `from == to`.
    pub fn route(&self, from: RoomId, to: RoomId) -> Option<Vec<Direction>> {
        if from == to {
            return self.rooms.contains_key(&from).then(Vec::new);
        }
        if !self.rooms.contains_key(&from) || !self.rooms.contains_key(&to) {
            return None;
        }
        let mut parent: BTreeMap<RoomId, (RoomId, Direction)> = BTreeMap::new();
        let mut queue = VecDeque::from([from]);
        while let Some(cur) = queue.pop_front() {
            let room = &self.rooms[&cur];
            for (d, edge) in room.exits.iter().enumerate() {
                let Some(edge) = edge else { continue };
                if !self.rooms.contains_key(&edge.dest) {
                    continue;
                }
                if edge.dest == from || parent.contains_key(&edge.dest) {
                    continue;
                }
                parent.insert(edge.dest, (cur, DIRECTIONS[d]));
                if edge.dest == to {
                    let mut steps = Vec::new();
                    let mut at = to;
                    while at != from {
                        let (prev, dir) = parent[&at];
                        steps.push(dir);
                        at = prev;
                    }
                    steps.reverse();
                    return Some(steps);
                }
                queue.push_back(edge.dest);
            }
        }
        None
    }
}
