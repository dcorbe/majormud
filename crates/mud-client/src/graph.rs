//! Client-side room graph, loaded from the decoded WG3-NT database
//! (`re/mmud_wgnt.sqlite`). Exit decode rules follow
//! `re/room_graph_wg.py`: direction index 0..9 = N,S,E,W,NE,NW,SE,SW,
//! U,D; dest room = `roomexit_<d+1>` (>0); dest map = `para1_<d+1>`
//! when `roomtype_<d+1>` == 8 (map-change portal), else the room's own
//! map. Placeholder rows (map outside 1..=999, room < 1) are skipped.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::path::Path;

use mud_core::content::{Direction, RoomId};

/// Direction index order, shared with the room record's exit arrays.
pub const DIRECTIONS: [Direction; 10] = [
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExitEdge {
    pub dest: RoomId,
    pub exit_type: i64,
    /// The line that traverses a **command exit** ([`COMMAND_EXIT`]),
    /// where walking the direction does nothing at all and the board
    /// expects a phrase instead: `borrow skiff` at the Newhaven ferry,
    /// `go manhole` into the Silvermere sewers, `climb tree` in the
    /// Tasloi village.
    ///
    /// 250 exits in the shipped world are of this kind and every one of
    /// them resolves: `para1` names a MESSAGE whose first line is the
    /// command (the second is an alias, unused — the board matches
    /// loosely enough that one spelling is sufficient, live 2026-08-02).
    /// `None` everywhere else, including the one command exit whose
    /// message is blank.
    pub command: Option<String>,
}

/// Exit type for a command exit — see [`ExitEdge::command`].
pub const COMMAND_EXIT: i64 = 10;

/// What taking this exit costs the walk, in units of one plain step.
///
/// The router used to be a hop-count BFS, which reads every edge as a
/// step and is wrong about 4,000 of them. Live, 2026-08-02: routing out
/// of the Silvermere Small Alleyway (1/405) it chose the type-6 HIDDEN
/// south exit into the secret passage — 24 steps against 27 by the
/// street — and the walk stood in the alley sending `s` at a brick wall
/// until it desynced. Three streets are cheap; three search rolls at a
/// 3% floor are not.
///
/// Costs, not prohibitions. 1,383 shipped exits are hidden and whole
/// areas sit behind them, so refusing a type outright would answer "no
/// route" to rooms that are reachable — a worse failure than the one
/// being fixed. Every type stays finite; the router just has to be paid
/// to use the awkward ones.
///
/// The classification is `re/docs/theft.md` §8.1, which is authoritative
/// for this project over the older display-code list in `vir_schemas.md`.
/// Types it does not name — 3, 4, 5, 0xc, 0xd, 0xe, 0xf, 0x11, 0x13 —
/// are ORACLE-OPEN and priced as plain steps, which is exactly how the
/// hop-count router treated them: no route that works today gets worse.
/// 0x13 alone justifies the default, being the ordinary Silvermere
/// street exit (94 of them, walked live).
pub fn exit_cost(exit_type: i64) -> u32 {
    match exit_type {
        // A door or gate: an `open`, sometimes a bash chain that charges
        // HP. The walk handles it (`nav::is_door`), so it is a detour
        // worth a few streets rather than a wall.
        2 | 7 | 0xb => 5,
        // Hidden: revealed only by SEARCH, and the roll's floor is 3%
        // (`theft.md` §9), so the honest price is tens of commands.
        6 => 40,
        // A trap on the exit. The walk has no DISARM, so this is damage
        // taken on purpose.
        9 | 0x18 => 25,
        // Timed, alignment, spell and ability gates. The walk can neither
        // satisfy nor wait these out, so they are a last resort — still
        // routable, because a route that fails at a gate says something
        // truer than "no route".
        0x10 | 0x14 | 0x16 | 0x17 => 60,
        // Plain, map-change portals, command exits (one reliable phrase),
        // and the unmodelled remainder.
        _ => 1,
    }
}

/// How a room's spawner behaves, from the room's `type` column
/// (`room+0x43c`). Rates are the per-kick roll thresholds in
/// `re/docs/monsters.md` §1: type 0 draws `genrdn(1,100) < 5`, type 2
/// `< 0x5a`, and both are additionally gated on the room holding fewer
/// monsters than players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpawnKind {
    /// Type 0 — a timed spawn, ~4% per 5s kick.
    #[default]
    Timed,
    /// Type 2 — a timed spawn at ~89%, and it bypasses the respawn
    /// cooldown entirely.
    Frequent,
    /// Type 3 — swarm: spawn until the generator refuses.
    Swarm,
    /// Type 1 — filled once at boot; the periodic spawner skips it. This
    /// is what shopkeepers and other fixtures are.
    BootFill,
    /// Anything else: no spawn at all.
    Never,
}

impl SpawnKind {
    fn from_column(ty: i64) -> SpawnKind {
        match ty {
            0 => SpawnKind::Timed,
            2 => SpawnKind::Frequent,
            3 => SpawnKind::Swarm,
            1 => SpawnKind::BootFill,
            _ => SpawnKind::Never,
        }
    }

    /// Roughly how often a kick spawns here, as a percentage, for the
    /// dossier to print. `None` where the question does not apply.
    pub fn rate_percent(self) -> Option<u32> {
        match self {
            SpawnKind::Timed => Some(4),
            SpawnKind::Frequent => Some(89),
            _ => None,
        }
    }
}

/// What a room's spawner will put in the room, before the monster table
/// is consulted. See [`crate::spawn::SpawnTable::candidates`] for the
/// selection this feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spawn {
    pub kind: SpawnKind,
    /// `monstertype` (`room+0x560`): which family of monsters this room
    /// draws from. **0 is a sentinel meaning "none"**, not region zero —
    /// read literally it has Town Gates spawning the group-0 scenery
    /// props (`ancient tapestry`, `mirror portal`).
    pub region: i64,
    /// `minindex`..`maxindex` (`room+0x462`/`+0x464`): the selector window
    /// a candidate must fall inside. `(0, 0)` means the room was never
    /// configured — see [`Spawn::draws_from_region`].
    pub band: (i64, i64),
    /// `bynumber` (`room+0x468`): one specific monster, bypassing the
    /// region draw. Stored in the HIGH WORD — the column is a 32-bit read
    /// of a 16-bit field, and its low word is zero in all 26,720 rooms.
    pub forced: Option<i64>,
    /// `permnpc`: the room's PERMANENT occupant, placed at boot and never
    /// drawn from the region. This is what a shop, a healer or a trainer
    /// actually contains — 475 rooms have one — and missing it is what
    /// made `/room` at the Newhaven healer list 33 quest NPCs instead of
    /// the healer (live, 2026-08-02).
    pub resident: Option<i64>,
}

impl Spawn {
    /// Does the periodic spawner draw from this room's region at all?
    ///
    /// Three ways for the answer to be no, the last two learned by
    /// reading a dossier that was obviously wrong:
    ///
    /// * Region 0 is a sentinel, not a region.
    /// * A boot-fill room is skipped by the spawner outright
    ///   (`re/docs/monsters.md` §1), so its region says nothing about it.
    /// * A zero band is an unconfigured room rather than a level-0
    ///   selector. This reverses an earlier reading, which rested on
    ///   Darkwood Forest's zero band resolving to monsters that looked
    ///   plausible. The evidence against is stronger: every zero-band
    ///   room in Newhaven is named "Blank" — they are dev placeholders —
    ///   and the healer, which is one, holds exactly the single NPC its
    ///   `permnpc` names rather than the 33 its region would draw.
    pub fn draws_from_region(&self) -> bool {
        matches!(
            self.kind,
            SpawnKind::Timed | SpawnKind::Frequent | SpawnKind::Swarm
        ) && self.region > 0
            && self.band.1 > 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct GraphRoom {
    pub name: String,
    pub exits: [Option<ExitEdge>; 10],
    /// Shop number, 0 for a room that sells nothing.
    pub shop: i64,
    pub spawn: Spawn,
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
        // The spawn columns ride along in the one pass. An extra column
        // on a query that already reads every row is free; a second query
        // over 26k rows to answer "what lives here" is not.
        for c in [
            "\"type\"",
            "monstertype",
            "minindex",
            "maxindex",
            "bynumber",
            "shopnum",
            "permnpc",
        ] {
            cols.push(c.into());
        }
        // Command exits point at a MESSAGE for their phrase, so the
        // whole table comes along first: 1-odd thousand short rows
        // against 250 lookups, which is cheaper than 250 queries and far
        // cheaper than discovering at the ferry that we cannot move.
        let commands = Self::load_exit_commands(&conn)?;
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
            let region: i64 = row.get(35).unwrap_or(0);
            let forced: i64 = row.get(38).unwrap_or(0);
            let mut graph_room = GraphRoom {
                name: name.unwrap_or_default(),
                exits: Default::default(),
                shop: row.get(39).unwrap_or(0),
                spawn: Spawn {
                    kind: SpawnKind::from_column(row.get(34).unwrap_or(-1)),
                    region,
                    band: (row.get(36).unwrap_or(0), row.get(37).unwrap_or(0)),
                    forced: (forced > 0).then_some(forced >> 16),
                    resident: {
                        let n: i64 = row.get(40).unwrap_or(0);
                        (n > 0).then_some(n)
                    },
                },
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
                    command: (exit_type == COMMAND_EXIT)
                        .then(|| commands.get(&para1).cloned())
                        .flatten(),
                });
            }
            let (Ok(map), Ok(room)) = (u16::try_from(map), u16::try_from(room)) else {
                continue;
            };
            rooms.insert(RoomId { map, room }, graph_room);
        }
        Ok(RoomGraph { rooms })
    }

    /// Message number -> the command that walks a command exit.
    ///
    /// Blank lines are dropped rather than stored: an empty command is
    /// indistinguishable from "no command" to every caller, and storing
    /// `Some("")` would have the navigator send a bare Enter.
    fn load_exit_commands(
        conn: &rusqlite::Connection,
    ) -> Result<BTreeMap<i64, String>, String> {
        let mut stmt = conn
            .prepare("SELECT number, messageline1 FROM message")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut out = BTreeMap::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let number: i64 = row.get(0).map_err(|e| e.to_string())?;
            let line: Option<String> = row.get(1).map_err(|e| e.to_string())?;
            let line = line.unwrap_or_default().trim().to_string();
            if !line.is_empty() {
                out.insert(number, line);
            }
        }
        Ok(out)
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
    /// One traversal, so ranking a hundred same-named candidates costs
    /// what routing to one of them does. That is the whole reason this
    /// exists beside [`RoomGraph::route`]: `/go Slum Street` matches 152
    /// rooms, and ranking those by calling `route` in a loop would be 152
    /// full traversals over ~26k rooms — seconds of blocking work on the
    /// path that handles a keystroke.
    ///
    /// Hops, not cost: this number is shown to the user as "(N steps)",
    /// and it counts the steps of the route [`RoomGraph::route`] would
    /// actually pick — the two run the same search, so they cannot drift.
    pub fn distances(&self, from: RoomId) -> BTreeMap<RoomId, usize> {
        self.explore(from, None)
            .into_iter()
            .map(|(id, reached)| (id, reached.hops))
            .collect()
    }

    /// Cheapest route as direction steps. `None` when unreachable; empty
    /// when `from == to`.
    ///
    /// Cheapest by [`exit_cost`], not shortest: a walk that saves three
    /// streets by gambling on a hidden exit has not saved anything.
    pub fn route(&self, from: RoomId, to: RoomId) -> Option<Vec<Direction>> {
        if from == to {
            return self.rooms.contains_key(&from).then(Vec::new);
        }
        if !self.rooms.contains_key(&from) || !self.rooms.contains_key(&to) {
            return None;
        }
        let reached = self.explore(from, Some(to));
        let mut steps = Vec::new();
        let mut at = to;
        while at != from {
            let (prev, dir) = reached.get(&at)?.via?;
            steps.push(dir);
            at = prev;
        }
        steps.reverse();
        Some(steps)
    }

    /// Dijkstra over exits into known rooms, ordered by total
    /// [`exit_cost`] and broken by hop count, so the cheapest route is
    /// also the shortest of the equally cheap ones. `target` stops the
    /// search once that room is settled; `None` walks the whole component.
    ///
    /// Shared by [`RoomGraph::route`] and [`RoomGraph::distances`]
    /// precisely because they must agree: the steps one reports are the
    /// steps the other counts.
    fn explore(&self, from: RoomId, target: Option<RoomId>) -> BTreeMap<RoomId, Reached> {
        let mut best: BTreeMap<RoomId, Reached> = BTreeMap::new();
        if !self.rooms.contains_key(&from) {
            return best;
        }
        best.insert(from, Reached::default());
        let mut heap = BinaryHeap::from([Reverse((0u64, 0usize, from))]);
        let mut settled: BTreeSet<RoomId> = BTreeSet::new();
        while let Some(Reverse((cost, hops, cur))) = heap.pop() {
            // A cheaper entry for this room was already popped; this one
            // is the stale copy the push-on-improve strategy leaves
            // behind.
            if !settled.insert(cur) {
                continue;
            }
            if target == Some(cur) {
                break;
            }
            for (d, edge) in self.rooms[&cur].exits.iter().enumerate() {
                let Some(edge) = edge else { continue };
                if !self.rooms.contains_key(&edge.dest) || settled.contains(&edge.dest) {
                    continue;
                }
                let step = (cost + u64::from(exit_cost(edge.exit_type)), hops + 1);
                if best.get(&edge.dest).is_none_or(|r| (r.cost, r.hops) > step) {
                    best.insert(
                        edge.dest,
                        Reached {
                            cost: step.0,
                            hops: step.1,
                            via: Some((cur, DIRECTIONS[d])),
                        },
                    );
                    heap.push(Reverse((step.0, step.1, edge.dest)));
                }
            }
        }
        best
    }
}

/// How the cheapest known route reaches one room: what it cost, how many
/// steps it took, and the edge it arrived by (`None` only at the origin).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Reached {
    cost: u64,
    hops: usize,
    via: Option<(RoomId, Direction)>,
}
