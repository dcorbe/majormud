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
            let mut graph_room = GraphRoom {
                name: name.unwrap_or_default(),
                exits: Default::default(),
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

    pub fn room(&self, id: RoomId) -> Option<&GraphRoom> {
        self.rooms.get(&id)
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
