//! Client-side room graph, a view over `mud_core::content::Content` --
//! itself decoded from the WG3-NT database (`re/mmud_wgnt.sqlite`) by
//! `mud_core::content_db`, the one decoder both `mud-server` and this
//! client use (`2026-08-22-one-path-to-content`). Exit decode rules
//! follow `re/room_graph_wg.py`: direction index 0..9 = N,S,E,W,NE,NW,
//! SE,SW,U,D; dest room = `roomexit_<d+1>` (>0); dest map = `para1_<d+1>`
//! when `roomtype_<d+1>` == 8 (map-change portal), else the room's own
//! map -- all folded into `content::Exit::dest` at decode time.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use mud_core::content::{Content, Direction, ItemId, MessageId, RoomId};

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

/// What an exit requires of whoever walks it.
///
/// Computed once at load from `roomtype`, the `para*` slots and the
/// room's `cmdtext` script, so that routing and walking share one
/// answer rather than each re-deriving it from a bare type number.
///
/// The taxonomy is ours, from `re/docs/theft.md` §8.1 plus the quest
/// VM's `remoteaction` verb. It deliberately says what an exit NEEDS and
/// not what it costs: cost depends on who is walking, and lives in
/// `exit_cost`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitRequirement {
    /// Nothing. Walk it.
    None,
    /// A door or gate, types 7 and 0xb. `locked` is the shipped lock
    /// state, `para1 == 2`, the word mud-core's `exit_lock_state`
    /// reads. `pick` is the pick modifier, `para2`, negative on hard
    /// locks. The state is the boot state: a lock with a positive
    /// modifier never re-locks once picked, so a door the data calls
    /// locked can stand unlocked all day. The walk tries `open` first
    /// either way, this only prices the route.
    Door { locked: bool, pick: i32 },
    /// A key door, type 2. Always locked. `key` is the item that opens
    /// it, `para1`, and `pick` the modifier, `para3`. The walk says
    /// `use <key> <direction>` with the key on the ring and picks
    /// without it.
    KeyDoor { key: ItemId, pick: i32 },
    /// Passable only while carrying `item`, `para1`. Type 3, 173 of
    /// them. Walked as a plain step, routing does the checking.
    ItemGate { item: ItemId },
    /// Concealed, and a search can reveal it. Type 6, unless a button
    /// or lever targets it, in which case it is
    /// [`ExitRequirement::Puzzle`] and no search roll ever clears it.
    Hidden,
    /// A trap on the exit. The walk has no DISARM. Types 9, 0x18.
    Trap,
    /// Not walked but spoken: the phrase is on [`ExitEdge::command`].
    /// Type 10, 250 of them.
    Command,
    /// Costs money to pass. `gold` is in GOLD CROWNS, the unit `para1`
    /// carries. Type 4.
    ///
    /// INFERRED unit, on two supports: the observed 5-gold Silvermere
    /// toll matches `para1 = 5`, and map 17's `para1 = 10000` is exactly
    /// 100 platinum = 1 runic coin. Proof would come from `move_user` in
    /// the WCCMMUD decompile, which nobody has read. The conversion to
    /// the client's base unit lives in exactly one place so that a
    /// correction is a one-line change.
    Toll { gold: u32 },
    /// Gated on alignment, a known spell, or an ability. Types 0x14,
    /// 0x16, 0x17. Not yet distinguished from one another: the walk can
    /// satisfy none of them today, so one variant is as actionable as
    /// three.
    Gate,
    /// Opens and shuts on a timer of its own. Type 0x10, 2 of them.
    Timed,
    /// Opened by a button or a lever: a type 12 slot or a cmdtext
    /// `remoteaction` line targets this exit. The slot itself is never
    /// an edge. The exit is a hidden passage, type 6, or a gate, type 7
    /// or 0xb, and [`ExitEdge::exit_type`] still says which, so the walk
    /// knows a gate a lever unlocked still wants an `open`. The plan and
    /// the price live in [`crate::puzzle::Puzzle`].
    Puzzle(crate::puzzle::Puzzle),
}

impl Default for ExitRequirement {
    fn default() -> Self {
        ExitRequirement::None
    }
}

impl ExitRequirement {
    /// The requirement an exit type implies on its own, before the
    /// cmdtext pass gets a say.
    ///
    /// `para1`, `para2` and `para3` are the raw slot fields. Which one
    /// means what depends on the type, see the lock variants' docs,
    /// and a fixture that cares about none of them passes zeros.
    ///
    /// Shared with the navigator's test fixtures deliberately. Those
    /// fixtures build exits by type and care about the WALKER's
    /// behaviour, so they must classify exactly as `load` does or they
    /// test a world that cannot exist. The tests that pin the
    /// classification itself do NOT call this — they assert
    /// hand-written expectations against fixture rooms, so they can
    /// still fail when this is wrong.
    pub fn from_exit_type(exit_type: i64, para1: i64, para2: i64, para3: i64) -> ExitRequirement {
        let item = |id: i64| u16::try_from(id).ok().filter(|id| *id != 0).map(ItemId);
        let modifier = |m: i64| i32::try_from(m).unwrap_or(0);
        match exit_type {
            2 => match item(para1) {
                Some(key) => ExitRequirement::KeyDoor { key, pick: modifier(para3) },
                // No key exists for it, so the lock is all there is.
                None => ExitRequirement::Door { locked: true, pick: modifier(para3) },
            },
            // 7 shipped gates want item 0, which is no item at all.
            3 => match item(para1) {
                Some(item) => ExitRequirement::ItemGate { item },
                None => ExitRequirement::None,
            },
            7 | 0xb => ExitRequirement::Door {
                locked: para1 == 2,
                pick: modifier(para2),
            },
            // Searchable until a slot or a script targets it.
            6 => ExitRequirement::Hidden,
            9 | 0x18 => ExitRequirement::Trap,
            COMMAND_EXIT => ExitRequirement::Command,
            4 => ExitRequirement::Toll {
                gold: u32::try_from(para1).unwrap_or(0),
            },
            0x14 | 0x16 | 0x17 => ExitRequirement::Gate,
            0x10 => ExitRequirement::Timed,
            _ => ExitRequirement::None,
        }
    }
}

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
    /// What this exit requires of whoever walks it.
    pub requirement: ExitRequirement,
}

/// Exit type for a command exit — see [`ExitEdge::command`].
pub const COMMAND_EXIT: i64 = 10;

/// Exit type for a remote action slot, a button or a lever. Never an
/// edge: the graph decodes it into a [`crate::puzzle::PuzzleAction`] on
/// the exit it opens and drops the slot.
pub const REMOTE_ACTION_EXIT: i64 = 12;

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

/// Can this character ever pick a lock with this modifier?
///
/// `theft.md` sections 8.2 and 8.3, mirrored in mud-core's
/// `picklock_command`: the skill must be at least 1 and the roll is
/// `genrdn(0,100) < modifier + skill`. Below zero the roll never
/// passes, so the pick fails every time and no budget of retries
/// changes that. Shipped key doors carry -60, -99, -100, -160, -290
/// and -999, and the plain doors mostly 0, -20, -70 and -999. The one
/// place this line is drawn: routing asks it to price a lock and the
/// walk asks it before spending a roll.
pub fn pickable(modifier: i32, picklocks: u32) -> bool {
    picklocks >= 1 && i64::from(modifier) + i64::from(picklocks) > 0
}

/// How many `picklock` commands a lock is expected to take from a
/// character [`pickable`] says can open it. The roll passes with
/// chance `modifier + skill` in 100, so the expectation is the inverse,
/// rounded up. Never below one: a certain pick is still a command.
pub fn pick_rolls(modifier: i32, picklocks: u32) -> u32 {
    let chance = (i64::from(modifier) + i64::from(picklocks)).clamp(1, 100);
    u32::try_from((100 + chance - 1) / chance).unwrap_or(u32::MAX)
}

/// The price of a lock a pickable character will pick: the door, then
/// the expected rolls, never above a searchable hidden exit. The cap
/// keeps a 1 in 100 lock routable at all, and the walk's own retry
/// budget is what decides whether it gives.
fn picked_cost(modifier: i32, picklocks: u32) -> Cost {
    let door = exit_cost(7);
    Cost::Steps((door + pick_rolls(modifier, picklocks)).min(exit_cost(6)))
}

/// What the walker can currently bring to bear on an exit.
///
/// A snapshot, passed to routing rather than read from a global: two
/// routes computed for different characters in the same process must be
/// able to disagree.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub purse: crate::purse::Purse,
    /// Which `(room, direction)` toll crossings have been MEASURED to
    /// not charge — see [`TollLog`]. `Arc` because the same log has to
    /// be consulted by every routing call this walker makes AND updated
    /// by whichever walk is currently crossing the edge; a plain field
    /// would give each `Capabilities` clone its own amnesia.
    pub tolls_known_free: Arc<TollLog>,
    /// The character's own `stat`-sheet `Picklocks` skill. Routing
    /// prices a lock by it through [`pickable`] and [`pick_rolls`],
    /// and [`crate::nav::Navigator`] asks the same formula before
    /// spending a roll. Zero by default: an operator who never asked
    /// [`crate::session::Session::stats`] gets a character who cannot
    /// pick, the same safe direction an empty purse already takes for
    /// tolls.
    pub picklocks: u32,
    /// The character's own `stat`-sheet `Stealth` skill. Same pattern as
    /// `picklocks`: read off the sheet, not an operator setting. Zero
    /// means the character cannot usefully arm sneak at all, so
    /// [`crate::nav::Navigator`] never sends it — a `sneak` a Stealth-0
    /// character sent would still fail the roll, spending a command and
    /// a delay tick for nothing.
    ///
    /// Unlike `picklocks`, [`Capabilities::unrestricted`] does NOT set
    /// this to `u32::MAX`: picking is a gated COST, consulted only at
    /// the moment a route actually meets a lock, so assuming "yes" there
    /// changes nothing for a walk that never meets one. Sneaking is a
    /// proactive SEND on every single step — assuming "yes" here would
    /// have every existing caller of `unrestricted()` (every navigator
    /// built before this field existed, and every test fixture that
    /// never scripted a `sneak` reply) start sending a command its board
    /// does not answer, paying a full step timeout on every step for
    /// nothing. "Unrestricted" means every already-existing cost is
    /// affordable, not that a brand new optional action gets taken.
    pub stealth: u32,
    /// What the character carries, shared with every other navigator
    /// and bot for the same character. `None` until a caller with the
    /// item table hands one over, see [`crate::session::Session::set_content`],
    /// and a walker with no pack holds nothing.
    pub pack: Option<crate::pack::PackHandle>,
    /// Whether the walk may bash a door it can neither open nor pick.
    /// A navigator sets this from its own `bash_doors` config, the one
    /// switch there is, so routing and the walk agree on which locked
    /// doors are a wall. The session leaves it off: it has no config,
    /// and a roam, the one thing that routes with the session's own
    /// capabilities, never crosses a door at all.
    pub bash_doors: bool,
}

impl Capabilities {
    /// Everything satisfiable — what routing assumed before it could ask.
    ///
    /// Used by callers that genuinely want the shape of the world rather
    /// than one character's view of it, and as the migration default.
    /// A caller that wants a character's real answer must pass that
    /// character's capabilities; this one will happily route through a
    /// 10,000-gold toll and pick anything.
    pub fn unrestricted() -> Capabilities {
        Capabilities {
            purse: crate::purse::Purse::from_farthings(u64::MAX),
            tolls_known_free: Arc::new(TollLog::default()),
            picklocks: u32::MAX,
            // See the field's own doc: a proactive send, not a gated
            // cost, so "unrestricted" leaves it off rather than arming
            // it for every caller that has never heard of it.
            stealth: 0,
            // Not "everything": an unlimited pack would route every
            // walker through item gates and key doors it cannot pass,
            // and the cost of that mistake is a character stranded at
            // one. A missing item is the safe answer.
            pack: None,
            // Every already existing cost is affordable, and bashing is a cost.
            bash_doors: true,
        }
    }

    /// Has this edge, in this direction, been MEASURED not to charge?
    ///
    /// Delegates to [`TollLog::is_free`] so that `exit_cost_for`'s
    /// consult and a test's direct assertion on the log are the same
    /// question asked two ways, never two answers that could drift
    /// apart.
    pub fn is_known_free(&self, room: RoomId, dir: Direction) -> bool {
        self.tolls_known_free.is_free(room, dir)
    }

    /// Does this walker hold the item, on the ring or in the pack?
    pub fn has_item(&self, id: mud_core::content::ItemId) -> bool {
        self.pack.as_ref().is_some_and(|p| p.has(id))
    }
}

/// What a toll crossing was actually measured to do, per `(room,
/// direction)` edge.
///
/// Exists because the data cannot be trusted on its own: both directions
/// of the Silvermere gate carry `type=4, para1=5` and only the outbound
/// crossing charges in play. `re/docs/theft.md` §8.1 does not name type
/// 4 and the real rule is in `move_user` in the WCCMMUD decompile,
/// unread — so instead of guessing, the walk measures the purse before
/// and after an actual crossing and remembers what it saw.
///
/// Absence of an entry means "never crossed this way, assume it
/// charges" — the pessimistic default the whole feature exists to
/// protect. A wrong "charges" wastes a detour; a wrong "free" could
/// strand a character on the wrong side of a gate it cannot pay to
/// re-cross, so that direction is never the one guessed.
#[derive(Debug, Default)]
pub struct TollLog {
    charged: Mutex<BTreeMap<(RoomId, Direction), bool>>,
}

impl TollLog {
    /// Record what one actual crossing measured: `charged` true if the
    /// purse moved, false if it did not. Overwrites any earlier
    /// measurement of the same edge — the most recent crossing is the
    /// best evidence there is.
    pub fn record(&self, room: RoomId, dir: Direction, charged: bool) {
        self.charged
            .lock()
            .expect("toll log lock")
            .insert((room, dir), charged);
    }

    /// Was this edge measured free? `false` covers both "measured
    /// charged" and "never measured" on purpose — a caller deciding
    /// whether it is safe to assume the gate is open must not be able
    /// to tell "no" and "don't know" apart.
    pub fn is_free(&self, room: RoomId, dir: Direction) -> bool {
        matches!(
            self.charged.lock().expect("toll log lock").get(&(room, dir)),
            Some(false)
        )
    }
}

/// What one edge costs this walker, or that it cannot be walked at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cost {
    Steps(u32),
    Impassable,
}

/// [`exit_cost`], but able to consult the walker.
///
/// Still a COST and not a prohibition wherever a cost can express the
/// truth — the argument in `exit_cost`'s own comment stands: refusing a
/// type answers "no route" to rooms that are genuinely reachable.
/// `Impassable` is reserved for edges no amount of walking opens: a toll
/// beyond the purse, a lever exit this walker has no plan for, a lock
/// it cannot pick and may not bash, a key door without the key or the
/// skill, and an item gate without the item.
///
/// `room` and `dir` name the exact edge being costed — needed only to
/// consult [`Capabilities::is_known_free`], since a per-edge learned
/// fact cannot be looked up from a bare requirement and type number.
pub fn exit_cost_for(
    req: &ExitRequirement,
    exit_type: i64,
    room: RoomId,
    dir: Direction,
    caps: &Capabilities,
) -> Cost {
    match req {
        ExitRequirement::Toll { gold } => {
            if caps.is_known_free(room, dir) {
                Cost::Steps(1)
            } else if caps.purse.farthings() >= crate::purse::Purse::from_gold(*gold).farthings() {
                Cost::Steps(1)
            } else {
                Cost::Impassable
            }
        }
        // A lever exit costs what its plan costs. No plan is a wall:
        // pricing it high would still route through it whenever the
        // detour was longer, and then stall at the wall.
        ExitRequirement::Puzzle(puzzle) => {
            // The exit itself once open: a gate still wants an open, a
            // revealed passage is a step. The search price never
            // applies, no roll is spent.
            let base = if crate::nav::is_door(exit_type) {
                exit_cost(exit_type)
            } else {
                1
            };
            puzzle.cost(base, caps)
        }
        // An unlocked door only wants an open.
        ExitRequirement::Door { locked: false, .. } => Cost::Steps(exit_cost(exit_type)),
        // A lock: picked when the formula allows, else bashed when
        // bashing is on, since force is a separate roll, else a wall.
        ExitRequirement::Door { locked: true, pick } => {
            if pickable(*pick, caps.picklocks) {
                picked_cost(*pick, caps.picklocks)
            } else if caps.bash_doors {
                Cost::Steps(exit_cost(exit_type))
            } else {
                Cost::Impassable
            }
        }
        // The key makes it an ordinary door. Without it the lock is
        // all there is, and a key door nobody can pick is a wall
        // whether or not bashing is on.
        ExitRequirement::KeyDoor { key, pick } => {
            if caps.has_item(*key) {
                Cost::Steps(exit_cost(exit_type))
            } else if pickable(*pick, caps.picklocks) {
                picked_cost(*pick, caps.picklocks)
            } else {
                Cost::Impassable
            }
        }
        ExitRequirement::ItemGate { item } => {
            if caps.has_item(*item) {
                Cost::Steps(1)
            } else {
                Cost::Impassable
            }
        }
        // Everything else is priced exactly as before, by type.
        _ => Cost::Steps(exit_cost(exit_type)),
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

#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Loads and decodes `db` via `mud_core::content_db` (the one decoder,
    /// `2026-08-22-one-path-to-content`) and builds the graph as a view
    /// over the result. No SQL of its own: the five hand-written queries
    /// this module used to run are gone, split between
    /// `mud_core::content_db`'s loaders and [`Self::from_content`]'s and
    /// [`crate::views`]'s filtering.
    pub fn load(db: &Path) -> Result<Self, String> {
        let content = mud_core::content_db::load(db).map_err(|e| e.to_string())?;
        Ok(Self::from_content(&content))
    }

    /// Build from an already-decoded [`Content`] -- the view that
    /// replaces this module's own SQL (`2026-08-22-one-path-to-content`
    /// Task 4). `content::Exit` already carries the raw `exit_type` and
    /// `param` (`para1`), so [`ExitRequirement::from_exit_type`] and the
    /// destination/trigger-message folding port unchanged; only the
    /// per-view filtering below (command-message trimming, the
    /// `remoteaction` scan) is new work.
    pub fn from_content(content: &Content) -> Self {
        let mut remote_actions = Self::remote_actions_from_content(content);
        let mut rooms = BTreeMap::new();
        for (id, room) in &content.rooms {
            let mut graph_room = GraphRoom {
                name: room.name.clone(),
                exits: Default::default(),
                shop: room.shop.map(|s| i64::from(s.0)).unwrap_or(0),
                spawn: Spawn {
                    kind: SpawnKind::from_column(i64::from(room.room_type)),
                    region: i64::from(room.spawn_zone),
                    band: (i64::from(room.min_level), i64::from(room.max_level)),
                    forced: room.forced_monster.map(|m| i64::from(m.0)),
                    resident: room.boss_monster.map(|m| i64::from(m.0)),
                },
                light: i64::from(room.light),
            };
            for (d, exit) in room.exits.iter().enumerate() {
                let Some(exit) = exit else { continue };
                let exit_type = i64::from(exit.exit_type);
                // A button or lever. Not an edge: it is recorded against
                // the exit it opens and the slot itself vanishes.
                if exit_type == REMOTE_ACTION_EXIT {
                    if let Some((key, action)) = Self::slot(content, *id, exit) {
                        remote_actions.entry(key).or_default().push(action);
                    }
                    continue;
                }
                let param = i64::from(exit.param);
                graph_room.exits[d] = Some(ExitEdge {
                    dest: exit.dest,
                    exit_type,
                    command: (exit_type == COMMAND_EXIT)
                        .then(|| Self::exit_command(content, exit.trigger_msg))
                        .flatten(),
                    requirement: ExitRequirement::from_exit_type(
                        exit_type,
                        param,
                        i64::from(exit.param2),
                        i64::from(exit.param3),
                    ),
                });
            }
            rooms.insert(*id, graph_room);
        }
        // Second pass, because the target exit may be read before or
        // after the room that opens it, and because the hop count needs
        // every room in place.
        for ((target, exit), mut actions) in remote_actions {
            let word = content
                .rooms
                .get(&target)
                .and_then(|r| r.exits[exit].as_ref())
                .map(|e| e.param)
                .unwrap_or(0);
            let wanted: BTreeSet<RoomId> = actions.iter().map(|a| a.room).collect();
            let hops = Self::hops_from(&rooms, target, &wanted);
            for action in &mut actions {
                action.hops = hops.get(&action.room).copied();
            }
            // Nearest first, so a plan that may pick any one action
            // picks the closest. Unreachable rooms sort last.
            actions.sort_by_key(|a| (a.hops.unwrap_or(u32::MAX), a.room, a.number));
            let Some(edge) = rooms
                .get_mut(&target)
                .and_then(|r| r.exits[exit].as_mut())
            else {
                continue;
            };
            // Only a hidden exit or a gate answers a lever. mud-core's
            // remote action dispatch does nothing for any other type,
            // so a slot aimed at a plain exit changes nothing about it.
            if !matches!(edge.exit_type, 6 | 7 | 0xb) {
                continue;
            }
            edge.requirement = ExitRequirement::Puzzle(crate::puzzle::Puzzle {
                word: u32::try_from(word).unwrap_or(0),
                actions,
            });
        }
        RoomGraph { rooms }
    }

    /// A command exit's phrase: the trigger message's FIRST line
    /// (`messageline1`), trimmed, entry only when non-empty -- the old
    /// hand-written `load_exit_commands`'s filter, mutation-tested in
    /// `tests/graph_content.rs`.
    fn exit_command(content: &Content, trigger_msg: Option<MessageId>) -> Option<String> {
        let line = content
            .messages
            .get(&trigger_msg?)?
            .lines
            .first()?
            .trim()
            .to_string();
        (!line.is_empty()).then_some(line)
    }

    /// Decode one type 12 slot: which exit it opens and what doing so
    /// takes. Nightmare's map source decodes the same fields, frmMap.frm
    /// near line 21705 in the bbs backup. `None` for a slot with no
    /// phrase or an unusable action number.
    fn slot(
        content: &Content,
        actor: RoomId,
        exit: &mud_core::content::Exit,
    ) -> Option<((RoomId, usize), crate::puzzle::PuzzleAction)> {
        // para2 below 10 is a bare exit index for action 0. Otherwise
        // the tens are the action and the ones the exit index.
        let para2 = u32::try_from(exit.param2).ok()?;
        let (number, index) = if para2 < 10 {
            (0, para2)
        } else {
            (para2 / 10, para2 % 10)
        };
        let number = u8::try_from(number).ok().filter(|n| *n <= 10)?;
        let index = usize::try_from(index).ok()?;
        let phrases: Vec<String> = Self::message_lines(content, exit.param).take(2).collect();
        if phrases.is_empty() {
            return None;
        }
        let reply = Self::message_lines(content, exit.param3).next();
        let item = u16::try_from(exit.param4)
            .ok()
            .filter(|id| *id != 0)
            .map(ItemId);
        Some((
            (exit.dest, index),
            crate::puzzle::PuzzleAction {
                room: actor,
                number,
                phrases,
                item,
                reply,
                hops: None,
            },
        ))
    }

    /// The non-blank lines of a message, trimmed, in order. Empty when
    /// the id is zero or unknown.
    fn message_lines(content: &Content, id: i32) -> impl Iterator<Item = String> + '_ {
        u16::try_from(id)
            .ok()
            .filter(|m| *m != 0)
            .and_then(|m| content.messages.get(&MessageId(m)))
            .into_iter()
            .flat_map(|m| m.lines.iter())
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
    }

    /// Steps from `from` to each of `wanted` by the fewest edges, over
    /// the rooms built so far. Stops as soon as every wanted room is
    /// found. A room it never reaches is absent from the answer.
    fn hops_from(
        rooms: &BTreeMap<RoomId, GraphRoom>,
        from: RoomId,
        wanted: &BTreeSet<RoomId>,
    ) -> BTreeMap<RoomId, u32> {
        let mut found = BTreeMap::new();
        let mut seen = BTreeSet::from([from]);
        let mut frontier = VecDeque::from([(from, 0u32)]);
        while let Some((at, hops)) = frontier.pop_front() {
            if wanted.contains(&at) {
                found.insert(at, hops);
                if found.len() == wanted.len() {
                    break;
                }
            }
            let Some(room) = rooms.get(&at) else { continue };
            for edge in room.exits.iter().flatten() {
                if rooms.contains_key(&edge.dest) && seen.insert(edge.dest) {
                    frontier.push_back((edge.dest, hops + 1));
                }
            }
        }
        found
    }

    /// The old hand-written `load_remote_actions`, over a decoded
    /// [`Content`] instead of a live connection: same grammar, same
    /// per-line parse, sourced from `room.command_block` +
    /// `content.textblocks` instead of the `room JOIN textblock` query.
    /// Feeds the same map as the type 12 slots decoded in
    /// [`Self::slot`], which are the main mechanism: this `cmdtext`
    /// form is the rarer, hand-scripted one.
    fn remote_actions_from_content(
        content: &Content,
    ) -> BTreeMap<(RoomId, usize), Vec<crate::puzzle::PuzzleAction>> {
        let mut out: BTreeMap<(RoomId, usize), Vec<crate::puzzle::PuzzleAction>> = BTreeMap::new();
        for (&actor, room) in &content.rooms {
            let Some(block) = room.command_block else {
                continue;
            };
            let Some(block) = content.textblocks.get(&block) else {
                continue;
            };
            if !block.body.contains("remoteaction") {
                continue;
            }
            for line in block
                .body
                .split(['\r', '\n'])
                .filter(|l| !l.trim().is_empty())
            {
                let mut parts = line.split(':');
                let Some(phrase) = parts.next().map(str::trim) else {
                    continue;
                };
                if phrase.is_empty() {
                    continue;
                }
                for verb in parts {
                    let mut w = verb.split_whitespace();
                    if w.next() != Some("remoteaction") {
                        continue;
                    }
                    let nums: Vec<i64> = w.filter_map(|n| n.parse().ok()).collect();
                    // remoteaction <room> <msg> <action> <exit>
                    let [target, msg, action, exit] = nums[..] else {
                        continue;
                    };
                    let (Ok(target), Ok(exit), Ok(number)) =
                        (u16::try_from(target), usize::try_from(exit), u8::try_from(action))
                    else {
                        continue;
                    };
                    if exit > 9 || number > 10 {
                        continue;
                    }
                    let key = (
                        RoomId {
                            map: actor.map,
                            room: target,
                        },
                        exit,
                    );
                    let entry = out.entry(key).or_default();
                    match entry
                        .iter_mut()
                        .find(|a| a.room == actor && a.number == number)
                    {
                        Some(a) => a.phrases.push(phrase.to_string()),
                        // The directive's message pair prints the room
                        // line first and the actor line second, which
                        // is the other way round from a slot's para3.
                        None => entry.push(crate::puzzle::PuzzleAction {
                            room: actor,
                            number,
                            phrases: vec![phrase.to_string()],
                            item: None,
                            reply: Self::message_lines(content, i32::try_from(msg).unwrap_or(0)).nth(1),
                            hops: None,
                        }),
                    }
                }
            }
        }
        out
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
        let content = mud_core::content_db::load(db).map_err(|e| e.to_string())?;
        Ok(crate::views::threat_table(&content))
    }

    /// How long each spell lasts, in combat rounds, by lowercased name.
    ///
    /// Only rows with a non-zero `duration` are returned, which is
    /// exactly the set of spells it makes sense to *keep up* — a heal has
    /// duration 0 because it happens and is over.
    ///
    /// **The figure is a floor, not the truth.** Real duration scales
    /// with caster level (`mud_core::content` `Scaling`), so a buff
    /// recast on the table value is always recast early and never late.
    /// That is the property that lets buff upkeep work off a timer at
    /// all, without needing to recognise a wear-off wording it cannot
    /// reliably identify.
    ///
    /// Duplicate names exist (`rapid healing` is both 138 and 831); the
    /// SHORTEST wins, for the same reason — early is safe.
    pub fn load_spell_durations(db: &Path) -> Result<BTreeMap<String, u32>, String> {
        let content = mud_core::content_db::load(db).map_err(|e| e.to_string())?;
        Ok(crate::views::spell_durations(&content))
    }

    /// The raw decoded content database, for a caller that needs item
    /// identity directly (`crate::items::resolve`, the backstab
    /// opener's weapon check -- `Navigator::with_backstab`) rather than
    /// one of `RoomGraph`'s own derived views. Same "reload the path
    /// again" shape as [`Self::load_threat`] / [`Self::load_spell_durations`]:
    /// one more view over the same file, not a new concept. `RoomGraph`
    /// itself does not keep the `Content` it was built from
    /// ([`Self::from_content`] consumes a borrow and discards it), so
    /// there is no cheaper way to hand one back.
    pub fn load_content(db: &Path) -> Result<Content, String> {
        mud_core::content_db::load(db).map_err(|e| e.to_string())
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
        self.distances_within_for(from, &|_, _| true, &Capabilities::unrestricted())
    }

    /// As [`RoomGraph::distances`], over the edges `allow` accepts, for
    /// a walker with these capabilities.
    ///
    /// An edge the walker cannot open is not counted, so
    /// a room behind it is absent unless another way reaches it. A roam
    /// floods its region with this, since a region containing a room
    /// the walk then cannot reach would send the character at a wall.
    pub fn distances_within_for(
        &self,
        from: RoomId,
        allow: &dyn Fn(Direction, &ExitEdge) -> bool,
        caps: &Capabilities,
    ) -> BTreeMap<RoomId, usize> {
        self.explore(from, None, allow, caps)
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
        self.route_within_for(from, to, &|_, _| true, &Capabilities::unrestricted())
    }

    /// As [`RoomGraph::route`], over the edges `allow` accepts.
    ///
    /// Note this is a PROHIBITION, which [`exit_cost`] deliberately is
    /// not: exit *types* are costed rather than refused, because refusing
    /// a type answers "no route" to rooms that are genuinely reachable.
    /// That argument does not transfer. `allow` carries an operator's
    /// decision that a room is off-limits — a roam's wall markers — and
    /// there "no route" is the correct answer rather than a defect. The
    /// caller asked for a fence and gets one.
    pub fn route_within(
        &self,
        from: RoomId,
        to: RoomId,
        allow: &dyn Fn(Direction, &ExitEdge) -> bool,
    ) -> Option<Vec<Direction>> {
        self.route_within_for(from, to, allow, &Capabilities::unrestricted())
    }

    /// As [`RoomGraph::route`], for a walker with these capabilities.
    ///
    /// The difference is not academic: leaving Silvermere through the
    /// gate is one hop and costs 5 gold, and the cheapest way round is
    /// 34 hops for a cost of 77. A
    /// router that cannot be told which walker is asking gets that
    /// wrong every time, in the same direction.
    pub fn route_for(
        &self,
        from: RoomId,
        to: RoomId,
        caps: &Capabilities,
    ) -> Option<Vec<Direction>> {
        self.route_within_for(from, to, &|_, _| true, caps)
    }

    /// As [`RoomGraph::route_within`], for a walker with these
    /// capabilities.
    pub fn route_within_for(
        &self,
        from: RoomId,
        to: RoomId,
        allow: &dyn Fn(Direction, &ExitEdge) -> bool,
        caps: &Capabilities,
    ) -> Option<Vec<Direction>> {
        if from == to {
            return self.rooms.contains_key(&from).then(Vec::new);
        }
        if !self.rooms.contains_key(&from) || !self.rooms.contains_key(&to) {
            return None;
        }
        let reached = self.explore(from, Some(to), allow, caps);
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
    /// [`exit_cost_for`] and broken by hop count, so the cheapest route is
    /// also the shortest of the equally cheap ones. `target` stops the
    /// search once that room is settled; `None` walks the whole component.
    ///
    /// Shared by [`RoomGraph::route`] and [`RoomGraph::distances`]
    /// precisely because they must agree: the steps one reports are the
    /// steps the other counts. `allow` is shared for the same reason —
    /// a constrained search and the distances taken over it must see the
    /// same world.
    fn explore(
        &self,
        from: RoomId,
        target: Option<RoomId>,
        allow: &dyn Fn(Direction, &ExitEdge) -> bool,
        caps: &Capabilities,
    ) -> BTreeMap<RoomId, Reached> {
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
                if !self.rooms.contains_key(&edge.dest)
                    || settled.contains(&edge.dest)
                    || !allow(DIRECTIONS[d], edge)
                {
                    continue;
                }
                let step = match exit_cost_for(
                    &edge.requirement,
                    edge.exit_type,
                    cur,
                    DIRECTIONS[d],
                    caps,
                ) {
                    // An edge this walker cannot open is not a dear edge.
                    // Skipping it is what lets the detour win.
                    Cost::Impassable => continue,
                    Cost::Steps(c) => (cost + u64::from(c), hops + 1),
                };
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
