//! Verified navigation: walk a computed route one step at a time,
//! confirming each arrival by the parsed room name. Never blind —
//! desyncs (lag, blocked exits, combat interruptions) surface as
//! errors or re-localization, not silent drift.
//!
//! Three things the walker knows how to do beyond putting one foot in
//! front of the other:
//!
//! - **Doors.** A route through a door or gate (exit types 2, 7, 0xb)
//!   opens it rather than stopping at it, falling back to bashing when
//!   `open` will not shift it. Only the graph decides an exit is a door;
//!   see [`Navigator::goto`]'s step handling.
//! - **Hidden exits.** A route through a type-6 exit searches it out
//!   rather than reading the board's "no exit in that direction" as a
//!   desync — the two are the same sentence, and only the graph knows
//!   which one it is. See [`Navigator::find_hidden`].
//! - **Working out where it is.** [`Navigator::localize`] answers from a
//!   room name alone and is therefore limited to one hop, because names
//!   repeat across the ~26k-room world. [`Navigator::localize_view`]
//!   answers from a whole room block, using the exits to tell same-named
//!   rooms apart, and so can recover from arbitrary displacement — a
//!   flee chain, a recall — rather than giving up.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::graph::RoomGraph;
use crate::session::{ExpectError, Session};
use mud_core::content::{Direction, RoomId};

/// A walk that did not finish, and — always — where it left the
/// character standing.
///
/// `at` is the last room the walk actually *verified* by name, which is
/// the only position a caller can act on: recovery has to start from
/// somewhere true, and neither the room the walk set off from nor the
/// one it was aiming at is that.
#[derive(Debug)]
pub struct NavError {
    pub at: RoomId,
    pub kind: NavErrorKind,
}

#[derive(Debug)]
pub enum NavErrorKind {
    NoRoute,
    /// The room we arrived in does not match the graph's expectation
    /// and could not be re-localized among neighbors.
    Desync { expected: String, saw: String },
    Expect(ExpectError),
    /// The door in the way is locked and nothing in this walk's
    /// repertoire opened it.
    ///
    /// Its own variant rather than a timeout, because it is neither: the
    /// board answered immediately and said exactly what was wrong. It
    /// shipped as `Expect(Timeout { needle: "room block after movement" })`,
    /// which told the operator the client had hung waiting for movement
    /// when in fact it had been told "The door is locked." a second
    /// earlier (live, beef.raw 2026-08-22). A walk that stops must say
    /// something the operator can act on.
    DoorLocked {
        /// The direction the door sits in, as the board words it.
        dir: String,
        /// What the walk actually tried, so "locked" and "locked and I
        /// was not allowed to try anything" are distinguishable.
        tried: String,
    },
    /// A [`TravelGuard`] decided that walking had become the wrong thing
    /// to be doing.
    Interrupted(Interrupt),
}

/// Why a guard took the walk back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interrupt {
    Died,
    Hurt { hp: i32 },
    /// Something is hitting us mid-walk.
    ///
    /// Distinct from [`Interrupt::Hurt`], which is a threshold: a healthy
    /// character can be swarmed for a long time without dropping past
    /// `interrupt_at_percent`, and every one of those rounds is a step
    /// that does not land. Waiting for the HP gate is too late, and
    /// sometimes never.
    Attacked { by: String },
    /// The attributed arrival block listed something the caller's policy
    /// wants to fight. Live incident (2026-07-31 arena run): a leg
    /// walked through "Also here: angry kobold thief, thin giant rat,
    /// large kobold thief." and kept sending steps while they attacked.
    /// Carries the block itself so the defence can start from this
    /// evidence instead of re-asking the board.
    Sighted { room: crate::events::RoomView },
    /// Something the caller's policy would fight walked in mid-step.
    /// Live incident (run5, 2026-08-01): "acid slime moves into the
    /// room from the north." during a leg, then whiffs only — no blow
    /// landed, so nothing stopped the walk, and the slime chased the
    /// character across three rooms. Unlike [`Interrupt::Sighted`]
    /// there is no attributed block to carry: entry wording is
    /// per-monster data, so the name is display-only and the defence
    /// starts by asking the board where it stands.
    Entered { name: String },
}

/// Watches the events a walk goes past and says when to stop walking.
///
/// The guard only ever observes. It sends nothing, which is what lets
/// travel stay a single-sender phase: the navigator keeps the connection
/// for the whole walk and merely learns when to hand it back.
pub trait TravelGuard {
    fn on_event(&mut self, ev: &crate::events::Event) -> Option<Interrupt>;

    /// Consulted ONLY with a room block attributed to the step in
    /// flight — never with the unfiltered stream `on_event` sees. That
    /// is what keeps a stale look answer or a foreign render from being
    /// read as an arrival: every desync this machinery ever had came
    /// from believing somebody else's block.
    fn on_room(&mut self, _room: &crate::events::RoomView) -> Option<Interrupt> {
        None
    }
}

/// Walk unprotected.
pub struct NoGuard;

impl TravelGuard for NoGuard {
    fn on_event(&mut self, _ev: &crate::events::Event) -> Option<Interrupt> {
        None
    }
}

impl std::fmt::Display for NavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let RoomId { map, room } = self.at;
        write!(f, "at {map}/{room}: ")?;
        match &self.kind {
            NavErrorKind::NoRoute => write!(f, "no route to target"),
            NavErrorKind::Desync { expected, saw } => {
                write!(f, "desync: expected {expected:?}, saw {saw:?}")
            }
            NavErrorKind::Expect(e) => write!(f, "{e}"),
            NavErrorKind::DoorLocked { dir, tried } => {
                write!(f, "the door {dir} is locked ({tried})")
            }
            NavErrorKind::Interrupted(i) => write!(f, "travel interrupted: {i:?}"),
        }
    }
}

impl std::error::Error for NavError {}

/// Navigation limits.
///
/// Only the step deadline is configurable, and only because it is the
/// one limit whose right value differs by board: the in-process server
/// answers instantly, the live board under load does not. `max_failures`
/// stays a constant until something can exercise it — a knob no test can
/// move is a knob that quietly rots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NavConfig {
    /// Per-step arrival deadline.
    pub step_timeout_ms: u64,
    /// Fall back to bashing a door that `open` would not shift.
    ///
    /// On by default, because a locked door is otherwise a hard stop that
    /// strands the walk. It is a switch rather than unconditional
    /// behaviour because bashing is not free: the board charges HP for it
    /// ("You take %d damage for bashing the door!") and refuses outright
    /// without a weapon.
    pub bash_doors: bool,
    /// Pick a locked door rather than breaking it.
    ///
    /// On by default, and tried BEFORE bashing: a roll against the
    /// character's Picklocks costs one command and no health, where a
    /// bash charges HP and needs a weapon. The 88-bash incident
    /// (cwgaming 2026-08-03, 1/1119) burned 36 hp of a 75-hp character
    /// on a type-7 lock that wanted Picklocks and was never going to
    /// yield to force.
    ///
    /// A character with no Picklocks simply fails the roll, which costs
    /// commands rather than health — so the switch exists for flood
    /// control and for operators who would rather stop at a locked door,
    /// not because picking can hurt.
    pub pick_locks: bool,
    /// Reveal a hidden exit (type 6) with SEARCH instead of treating the
    /// board's refusal as a desync.
    ///
    /// On by default: a hidden exit the router chose is otherwise a hard
    /// stop, and the router only chooses one when it is the way through
    /// (see [`crate::graph::exit_cost`]). A switch, for the same reason
    /// `bash_doors` is one — SEARCH costs a command per roll and breaks
    /// hide and sneak (`theft.md` §9).
    pub search_hidden: bool,
}

impl Default for NavConfig {
    fn default() -> Self {
        NavConfig {
            step_timeout_ms: 15_000,
            bash_doors: true,
            pick_locks: true,
            search_hidden: true,
        }
    }
}

/// Verification failures tolerated before a walk aborts.
const MAX_FAILURES: u32 = 3;

/// What the walk learned about the room it is standing in.
///
/// A name is the least it can be, because a dark room renders no block
/// at all and the name then comes from the graph edge that was walked.
/// When there IS a block it is kept whole: it is the board's own account
/// of the room, attributed to our own command, and the caller that
/// asked for the walk is about to want exactly that — see
/// [`Arrival::seen`].
#[derive(Debug)]
enum Sighting {
    /// The board rendered the room and the block answered our step.
    Block(crate::events::RoomView),
    /// Too dark to render: only a name, and not the board's.
    Dark(String),
}

impl Sighting {
    fn name(&self) -> &str {
        match self {
            Sighting::Block(room) => &room.name,
            Sighting::Dark(name) => name,
        }
    }

    /// The block, if there was one. Consumes: a block handed on is not
    /// one this module keeps reasoning about.
    fn block(self) -> Option<crate::events::RoomView> {
        match self {
            Sighting::Block(room) => Some(room),
            Sighting::Dark(_) => None,
        }
    }
}

/// What one resolved step actually established. `Arrived` describes a
/// room the character MOVED into; `StayedPut` describes what answered
/// the recovery `look` after a refusal — the walk knows it did not move,
/// and that knowledge must survive: ~51k adjacent room pairs share a
/// name (1/2151 and 1/2146 are both "Newhaven, Narrow Road"), so a
/// refusal's look-answer treated as an arrival drifts `current` into the
/// same-named twin while the character stands still.
enum StepOutcome {
    Arrived(Sighting),
    StayedPut(Sighting),
}

/// Where a walk ended, and what the board said about it on the way in.
///
/// The block is the whole reason this is not a bare [`RoomId`]. The step
/// that lands on the destination is answered by the destination's own
/// render, attributed to that step — and then the caller used to throw
/// it away and send a `look` for the same text. A farm stop paid that
/// round-trip once per stop; a roam, which works one room per pass, paid
/// it once per room walked.
#[derive(Debug, Clone)]
pub struct Arrival {
    /// The room the walk ended in.
    pub at: RoomId,
    /// The block that described `at`, when the walk has one to give.
    ///
    /// `None` is the honest answer more often than it looks: a walk of
    /// no steps (already there) never asked anything, a dark room
    /// rendered nothing, and a walk that ended somewhere other than the
    /// room its last block named has no block ABOUT `at`. Callers must
    /// treat it as "ask if you need to know", never as "empty room".
    pub seen: Option<crate::events::RoomView>,
}

impl Arrival {
    /// Pair a finishing position with the walk's last accepted block,
    /// keeping the block only if it is the one that described `at`.
    ///
    /// The guard is the safety property, and it is why the block is
    /// carried keyed to a room rather than on its own: a walk that took
    /// another step, re-planned or localized elsewhere after its last
    /// block would otherwise hand the caller a render of somewhere the
    /// character no longer is — a stop would then work a room it had
    /// left, which is the exact failure verified navigation exists to
    /// prevent.
    fn new(at: RoomId, landed: Option<(RoomId, crate::events::RoomView)>) -> Arrival {
        let seen = match landed {
            Some((id, room)) if id == at => Some(room),
            _ => None,
        };
        Arrival { at, seen }
    }
}

/// Exit types that are a door or gate you can open (`theft.md` §8.1:
/// "pickable types are 2, 7, 0xb"). Deliberately not 6 (hidden), 9/0x18
/// (traps), 0x10 (timed), 0x14 (alignment) or 0x16/0x17 (spell/ability
/// gates) — none of those yield to `open`.
/// Does this exit have a door on it?
///
/// Public because the MegaMud room-id codec needs the same answer: its
/// exit signature doubles a direction's value when the exit has a door,
/// and a second opinion about what a door is would put every hash out.
pub fn is_door(exit_type: i64) -> bool {
    matches!(exit_type, 2 | 7 | 0xb)
}

/// Is this exit hidden until SEARCH reveals it (`theft.md` §9)?
///
/// 1,383 shipped exits are, and the board neither lists them on the
/// "Obvious exits" line nor admits they exist when you walk at them — it
/// answers "There is no exit in that direction!", the same wording as a
/// genuine desync. Only the graph can tell the two apart.
pub fn is_hidden(exit_type: i64) -> bool {
    exit_type == 6
}

/// Lines that mean a shut door turned the step back. Lowercased before
/// matching, because the DLL ships both "Closed!" and "closed!". These
/// are _move_user's and _cmd_open's wordings only — "There is a closed
/// door in that direction!" is _move_user's (ReMUD decompile), NOT a
/// look refusal; _cmd_look's "The door is closed in that direction!"
/// never reaches this classifier because it can only be attributed to a
/// `look <dir>` the walk never sends. The bangs keep the two apart.
/// Both terminators, for the same reason `correlate::completes` carries
/// both (b26afe5): stock's `_move_user` prints the bang, and foreign
/// reimplementations soften it to a full stop ("The door is closed.",
/// cwrun2.raw 2026-08-01 — 7 occurrences, never a bang). The correlator
/// learned that and the navigator did not, so the step was retired
/// without ever being read as a blocked door: no `open`, no bash, just
/// `n` into a shut door again until the leg timed out.
///
/// The period cannot collide with `_cmd_look`'s refusal — that wording
/// continues "...in that direction!" and never carries a stop here.
const DOOR_BLOCKED: [&str; 7] = [
    "the door is closed!",
    "the door is closed.",
    "the gate is closed!",
    "the gate is closed.",
    "closed door in that direction",
    "the door is locked",
    "the gate is locked",
];

/// Lines that mean the door gave way but we have NOT moved yet, so the
/// step still has to be walked ("You bashed the door open.", 0xd54b0).
const DOOR_YIELDED: [&str; 3] = ["is now open", "was already open", "bashed the"];

/// The lock gave, but the LATCH did not: "You unlocked the door." leaves
/// the door standing shut, so the walk still owes it an `open` before
/// the step.
///
/// This lived in [`DOOR_YIELDED`] and that was wrong. Yielded means the
/// way is clear and the direction can be sent; unlocked means one more
/// command first. Conflating them sent the direction straight into a
/// closed door and the leg died there (live, 2026-08-22: "the
/// picklocking worked, but it forgot to try opening the door and then
/// gave up").
const DOOR_UNLOCKED: &str = "unlocked the door";

/// The board's answer when the exit is not there at all. The graph and
/// the board disagree, so the walk is somewhere other than it believes —
/// re-localizing is the only honest response, and waiting out a deadline
/// first just makes it slow.
const NO_SUCH_EXIT: &str = "no exit in that direction";

/// The board refuses movement outright while something is fighting you
/// (DLL 0xbc6a8). Nothing about the step is wrong — the character simply
/// cannot leave until the fight is dealt with — so a walk that waits out
/// its deadline here is waiting for an answer that will never come.
const COMBAT_BLOCKED: &str = "may not enter that room while in combat";

/// The other bash outcome (0xd538e) carries the character through the
/// doorway itself, so a room block is already on its way and sending the
/// direction again would overshoot by a room.
const BASH_CARRIED_THROUGH: &str = "walk through";

/// Bashing is a ROLL: "Your attempts to bash through fail!" (captured
/// live, stopstate-run1). One try per step was why the restart-locked
/// door at 1/2150 needed a hand-run script that leaned on it for up to
/// sixty rolls.
const BASH_FAILED: &str = "bash through fail";

/// The picklock roll's failure (`re/docs/theft.md` §8.2/§8.3): the skill
/// check missed, or the exit was never pickable. Both mean the door is
/// still shut, which is all the walk needs to decide whether to roll
/// again. Success is already covered by [`DOOR_YIELDED`]'s "unlocked the
/// door".
const PICK_FAILED: &str = "skill fails you";

/// Picking gets the same budget as bashing: it is the same kind of
/// thing — a roll repeated until the lock gives or the count says why —
/// and a thief who can open a door at all usually needs several tries.
const PICK_RETRIES: u32 = BASH_RETRIES;

/// The cooldown scold: the bash sat on the action timer and never
/// rolled. It paces, it does not fail — counting it against the roll
/// budget deflated twenty nominal rolls to a handful of real ones.
const BASH_PACED: &str = "must wait before you may do that";

/// Real failed rolls per step before the door is declared unbashable —
/// sized to the field evidence (opendoor.lua leaned on this door for up
/// to 60). Scolds are bounded separately and generously; the walk's
/// guard is the health backstop, this bound the diagnosability one.
const BASH_RETRIES: u32 = 60;

/// SEARCH revealed the hidden exit: "You found an exit to the %s!", and
/// the two vertical wordings "You found an exit upwards!" /
/// "...downwards!" (`theft.md` §9). The common prefix covers all three.
///
/// The find is not permanent — the board arms a ticker that re-hides the
/// exit in about five minutes — which is why the step is walked
/// immediately rather than remembered.
const HIDDEN_FOUND: &str = "you found an exit";

/// The roll came up short: "You notice nothing different to the %s."
///
/// It is ALSO what an already-found exit says: the found state falls
/// through to the same else. So this line is not evidence that the exit
/// is still hidden, only that this roll changed nothing — which is why
/// the search runs on the board's refusal rather than ahead of the step.
const HIDDEN_MISSED: &str = "you notice nothing different";

/// Search rolls per step before a hidden exit is declared unfindable.
///
/// Sized from the roll itself (`theft.md` §9: success iff
/// `genrdn(0,100) < max(Perception - 15, 3)`). The floor is 3%, where a
/// hundred rolls is ~95% to reveal; any real Perception clears it in a
/// handful. Bounded for the same reason [`BASH_RETRIES`] is: a walk that
/// cannot get through should say so, not rummage forever.
const SEARCH_ROLLS: u32 = 100;

/// The direction an "Obvious exits" token points.
///
/// Exits render as display strings, not commands — "closed door north",
/// "open gate west" — so the direction is the trailing word. Trapdoors
/// are the exception worth knowing about: they render as "closed trap
/// door above" / "open trap door below" (DLL 0xccd5d, 0xccda4), so the
/// vertical pair has to accept those words as well as up/down.
pub fn direction_of(token: &str) -> Option<Direction> {
    let word = token.split_whitespace().next_back()?.to_lowercase();
    Some(match word.as_str() {
        "north" | "n" => Direction::North,
        "south" | "s" => Direction::South,
        "east" | "e" => Direction::East,
        "west" | "w" => Direction::West,
        "northeast" | "ne" => Direction::NorthEast,
        "northwest" | "nw" => Direction::NorthWest,
        "southeast" | "se" => Direction::SouthEast,
        "southwest" | "sw" => Direction::SouthWest,
        "up" | "u" | "above" => Direction::Up,
        "down" | "d" | "below" => Direction::Down,
        _ => return None,
    })
}

/// What one command produced while walking a step.
enum StepEvent {
    /// A room block: the board's own account of where we landed.
    Arrived(crate::events::RoomView),
    /// A shut door turned us back.
    DoorBlocked,
    /// The door is open now, but we are still on this side of it.
    DoorYielded,
    /// Movement refused: we are in combat.
    CombatBlocked,
    /// There is no such exit; the graph and the board disagree.
    NoSuchExit,
    /// The bash roll came up short; the door still stands.
    BashFailed,
    /// The picklock roll missed; the door is still shut.
    PickFailed,
    /// The lock gave but the door is still shut — it needs opening.
    DoorUnlocked,
    /// The bash never rolled: it sat on the action timer.
    BashPaced,
    /// SEARCH revealed a hidden exit; the step is still owed.
    HiddenFound,
    /// SEARCH changed nothing — a failed roll, or an exit that was
    /// already revealed.
    HiddenMissed,
    /// The room is too dark to see: the board sent no room block at all,
    /// only "you can't see anything".
    ///
    /// Deliberately NOT named "arrived" any more. The same line answers a
    /// `look` from a standing start, so on its own it says nothing about
    /// whether a step landed — see [`Navigator::blind_position`].
    Blind,
}

/// What the walk had just asked when the board answered it could not see.
///
/// The dark line is identical either way, so the question is the only
/// thing that distinguishes them. See [`Navigator::blind_position`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlindContext {
    /// A direction was sent, so dark is the destination reporting itself.
    AfterMove,
    /// A `look` was sent, which moves nothing.
    AfterLook,
}

pub struct Navigator {
    graph: Arc<RoomGraph>,
    step_timeout: std::time::Duration,
    bash_doors: bool,
    pick_locks: bool,
    search_hidden: bool,
    /// The plane a fenced walk is confined to, alongside `fence`.
    plane: Option<u16>,
    /// Rooms every route must avoid ([`crate::roam::Walls`]).
    ///
    /// Without this a roam's fence would be advisory for PATHING while
    /// binding on destinations: the rotation only ever picks rooms
    /// inside the region, but the walk there is a plain `route` and
    /// would happily cut through a wall when that was cheaper. A fence
    /// you can walk through is not a fence.
    fence: Option<crate::roam::Walls>,
    /// What this walker can bring to bear on a costed exit, and what it
    /// has personally measured about toll edges. Defaults to
    /// [`crate::graph::Capabilities::unrestricted`], which is exactly
    /// the old behaviour: nothing that does not call
    /// [`Navigator::with_capabilities`] changes.
    capabilities: crate::graph::Capabilities,
}

/// The direction word the board understands for each step.
pub fn dir_word(d: Direction) -> &'static str {
    match d {
        Direction::North => "n",
        Direction::South => "s",
        Direction::East => "e",
        Direction::West => "w",
        Direction::NorthEast => "ne",
        Direction::NorthWest => "nw",
        Direction::SouthEast => "se",
        Direction::SouthWest => "sw",
        Direction::Up => "u",
        Direction::Down => "d",
    }
}

impl Navigator {
    pub fn new(graph: Arc<RoomGraph>, cfg: NavConfig) -> Self {
        Navigator {
            graph,
            step_timeout: std::time::Duration::from_millis(cfg.step_timeout_ms),
            bash_doors: cfg.bash_doors,
            pick_locks: cfg.pick_locks,
            search_hidden: cfg.search_hidden,
            fence: None,
            plane: None,
            capabilities: crate::graph::Capabilities::unrestricted(),
        }
    }

    /// Route and learn as this walker, not as "everything is
    /// satisfiable".
    ///
    /// Without this call every route this navigator computes and every
    /// toll it crosses uses [`crate::graph::Capabilities::unrestricted`]
    /// — unlimited purse, and a toll log nobody else can see — which is
    /// exactly the behaviour every existing caller already had. A
    /// caller that wants routing to respect a real purse, and wants a
    /// measured toll fact to survive between walks, supplies its own
    /// `Capabilities` (typically sharing one `Arc<TollLog>` across every
    /// navigator that walks the same character).
    pub fn with_capabilities(mut self, caps: crate::graph::Capabilities) -> Self {
        self.capabilities = caps;
        self
    }

    /// Refuse to route through these rooms, or off the plane they sit on.
    ///
    /// Consumed by a roam and by nothing else: `/go`, `/farm` and the
    /// recovery walks keep the whole world, because a marker means one
    /// thing and it is "not on this roam".
    pub fn fenced(mut self, walls: crate::roam::Walls, plane: u16) -> Self {
        self.fence = Some(walls);
        self.plane = Some(plane);
        // Second layer. `roam::passable` already refuses every door, so
        // no fenced route should ever arrive at one -- but "should never
        // happen" is how the walk got to 88 bashes in the first place,
        // and the failure mode is measured in the character's health.
        // If a door is somehow reached, stop honestly instead.
        self.bash_doors = false;
        // Picking is free where bashing is not, but the reasoning here
        // is the region's rather than the character's: `roam::passable`
        // holds that what is behind a door is not part of the area at
        // all, so opening one by ANY means contradicts the fence.
        self.pick_locks = false;
        self
    }

    /// The route this walk is allowed to take.
    ///
    /// Unfenced, this is exactly `RoomGraph::route`. Fenced, it is the
    /// same search over the edges a roam may use, so the walk and the
    /// region it is walking cannot disagree about where the fence is.
    ///
    /// Public so a test can ask the question directly. It is otherwise
    /// hard to observe: the roam's rotation already picks its targets
    /// with the fenced distances, so end to end the runner rarely ASKS
    /// for a route that would want to cross a wall — which is exactly
    /// why a fence that bound only destinations would look fine for a
    /// long time and then quietly walk through one.
    pub fn route_from(&self, from: RoomId, to: RoomId) -> Option<Vec<Direction>> {
        match (&self.fence, self.plane) {
            (Some(walls), Some(plane)) => self.graph.route_within_for(
                from,
                to,
                &crate::roam::passable(plane, walls),
                &self.capabilities,
            ),
            _ => self.graph.route_for(from, to, &self.capabilities),
        }
    }

    /// Walk from `from` to `to`, verifying every step by the parsed
    /// destination room name. On mismatch, re-localize among the
    /// previous room's neighbors and re-route; abort after repeated
    /// failures.
    ///
    /// Returns an [`Arrival`]: the room the walk ended in — `to` on
    /// success — together with the block the last step was answered
    /// with, so the caller need not `look` for what the board has just
    /// finished sending. On failure [`NavError::at`] carries the last
    /// room the walk verified.
    ///
    /// `guard` sees every event the walk goes past, including the ones
    /// the between-step drain throws away, and can end the walk early.
    /// A trip is honoured differently by kind, because the direction
    /// word for the current step is already on the wire by the time the
    /// board's answer arrives:
    ///
    /// - [`Interrupt::Hurt`] arms the walk and lets the step finish, so
    ///   the room it reports is one it actually verified rather than one
    ///   the character is in the act of leaving.
    /// - [`Interrupt::Died`] hands back at once. A downed character is
    ///   not going to complete the step, and waiting for a room block
    ///   that will never come would cost the whole deadline on every
    ///   death. `at` is then the last room verified before the fatal
    ///   step, which is as true as anything can be — and the run is
    ///   over regardless.
    pub async fn goto(
        &self,
        session: &Session,
        from: RoomId,
        to: RoomId,
        guard: &mut impl TravelGuard,
    ) -> Result<Arrival, NavError> {
        let mut current = from;
        let mut failures = 0u32;
        let mut armed: Option<Interrupt> = None;
        let mut events = session.events();
        // The last block the walk accepted, and the room it had just
        // established when it did. The pairing is the whole safety
        // property: a block is handed on ONLY if the walk finished in
        // the room that block described, so a re-plan, a further step or
        // a localization that moved us on can never pass off a stale
        // render as the destination's.
        let mut landed: Option<(RoomId, crate::events::RoomView)> = None;
        'replan: loop {
            if current == to {
                return Ok(Arrival::new(current, landed));
            }
            let route = self
                .route_from(current, to)
                .ok_or(NavErrorKind::NoRoute)
                .map_err(|kind| NavError { at: current, kind })?;
            for step in route {
                let expected_id = self
                    .graph
                    .room(current)
                    .and_then(|r| r.exits[step as usize].as_ref())
                    .map(|e| e.dest)
                    .ok_or(NavError {
                        at: current,
                        kind: NavErrorKind::NoRoute,
                    })?;
                let expected_name = self
                    .graph
                    .room(expected_id)
                    .map(|r| r.name.clone())
                    .ok_or(NavError {
                        at: current,
                        kind: NavErrorKind::NoRoute,
                    })?;

                // Attribution is what keeps stale blocks from
                // satisfying the step now; this drain remains for the
                // guard (a death in the discarded window still counts)
                // and to keep the receiver from lagging. A trip here is
                // honoured before the step goes out at all.
                crate::session::drain(&mut events, |ev| {
                    armed = armed.take().or_else(|| guard.on_event(&ev.event));
                });
                if let Some(interrupt) = armed.take() {
                    return Err(NavError {
                        at: current,
                        kind: NavErrorKind::Interrupted(interrupt),
                    });
                }

                let edge = self
                    .graph
                    .room(current)
                    .and_then(|r| r.exits[step as usize].as_ref());
                let exit_type = edge.map(|e| e.exit_type).unwrap_or(0);
                // A type-6 exit concealed by a puzzle bit-word answers
                // SEARCH exactly as a searchable one does and can never
                // be revealed by it, so the graph has to say which this
                // is. Searching a puzzle exit is unbounded: the roll can
                // never succeed.
                let searchable_hidden = edge
                    .map(|e| {
                        matches!(
                            e.requirement,
                            crate::graph::ExitRequirement::Hidden { searchable: true }
                        )
                    })
                    .unwrap_or(false);
                // A command exit is not walked, it is spoken: the
                // direction word does nothing at all at the Newhaven
                // ferry, and the leg simply stalls there until the
                // deadline. See `ExitEdge::command`.
                let command = edge.and_then(|e| e.command.clone());

                // The room the walk believes it is standing in. Needed
                // because a dark answer means "still here" when it
                // follows a look -- see Navigator::blind_position.
                let here_name = self
                    .graph
                    .room(current)
                    .map(|r| r.name.clone())
                    .unwrap_or_default();

                // The edge this step is about to cross, and where it
                // starts from -- captured before `current` moves, so a
                // toll measured after arrival can still be filed against
                // the room it was actually charged in.
                let depart_room = current;
                let toll_edge = edge
                    .map(|e| matches!(e.requirement, crate::graph::ExitRequirement::Toll { .. }))
                    .unwrap_or(false);
                // The data says both directions of the Silvermere gate
                // charge and play says only one does -- theft.md 8.1
                // does not name type 4 and move_user is unread, so
                // instead of guessing, ask the board what the character
                // is carrying before an actual crossing and compare
                // after. Only ever asked for a toll edge: every other
                // step pays no extra round trip for a fact it does not
                // need.
                let toll_before = if toll_edge {
                    Some(
                        self.read_purse(session, &mut events, guard, &mut armed)
                            .await
                            .map_err(|kind| NavError { at: current, kind })?,
                    )
                } else {
                    None
                };

                // Correlated as a move whatever it says, because that is
                // what it is: `kind_of` reads direction words, and
                // "borrow skiff" would otherwise be Opaque, so the
                // arrival block would answer nothing and the step would
                // wait out its whole deadline having already arrived.
                let sent = match command.as_deref() {
                    Some(cmd) => session.send_move(cmd),
                    None => session.send(dir_word(step)),
                };
                let outcome = self
                    .walk_step(
                        step,
                        exit_type,
                        searchable_hidden,
                        sent,
                        &expected_name,
                        &here_name,
                        session,
                        &mut events,
                        guard,
                        &mut armed,
                    )
                    .await;
                let seen = match outcome {
                    Ok(seen) => seen,
                    // A step that never lands while the guard is armed
                    // is the interrupt's story, not the deadline's.
                    Err(kind) => {
                        let kind = match armed.take() {
                            Some(interrupt) => NavErrorKind::Interrupted(interrupt),
                            None => kind,
                        };
                        return Err(NavError { at: current, kind });
                    }
                };
                let seen = match seen {
                    StepOutcome::Arrived(sighting) if sighting.name() == expected_name => {
                        current = expected_id;
                        // The step actually landed, so a pending toll
                        // measurement gets its second reading now: if
                        // the purse did not move, this direction is free
                        // and every future route may prefer it. If it
                        // did move, the pessimistic default already had
                        // it right and nothing changes -- but the
                        // measurement is recorded either way, so a
                        // gate's answer is never generations stale.
                        if let Some(before) = toll_before {
                            let after = self
                                .read_purse(session, &mut events, guard, &mut armed)
                                .await
                                .map_err(|kind| NavError { at: current, kind })?;
                            self.capabilities.tolls_known_free.record(
                                depart_room,
                                step,
                                after != before,
                            );
                        }
                        // The block that answered this step describes
                        // the room the step just landed in. Kept HERE,
                        // keyed to that room, so the check at the top of
                        // the loop can hand it to the caller when this
                        // was the last step of the walk. A dark arrival
                        // keeps nothing, which is the correct answer:
                        // there was no block.
                        landed = sighting.block().map(|room| (current, room));
                        if let Some(interrupt) = armed.take() {
                            return Err(NavError {
                                at: current,
                                kind: NavErrorKind::Interrupted(interrupt),
                            });
                        }
                        continue;
                    }
                    // The walk KNOWS it did not move (a refused step's
                    // recovery look answered with this room's own name),
                    // and that knowledge must outrank name matching:
                    // with a same-named twin next door, localize would
                    // confidently relocate a character that never went
                    // anywhere. Re-plan from where we still stand.
                    StepOutcome::StayedPut(sighting) if sighting.name() == here_name => {
                        failures += 1;
                        if failures > MAX_FAILURES {
                            return Err(NavError {
                                at: current,
                                kind: NavErrorKind::Desync {
                                    expected: expected_name.clone(),
                                    saw: sighting.name().to_string(),
                                },
                            });
                        }
                        continue 'replan;
                    }
                    StepOutcome::Arrived(sighting) | StepOutcome::StayedPut(sighting) => sighting,
                };
                failures += 1;
                // Taken before the sighting is consumed below, and it is
                // the only thing the error wants from it.
                let saw = seen.name().to_string();
                let desync = |at| NavError {
                    at,
                    kind: NavErrorKind::Desync {
                        expected: expected_name.clone(),
                        saw: saw.clone(),
                    },
                };
                if failures > MAX_FAILURES {
                    return Err(desync(current));
                }
                match self.localize(current, &saw) {
                    Some(id) => {
                        current = id;
                        // Localizing is what settles which room the
                        // block was describing, so this is the same
                        // pairing as a clean step — and this arm CAN
                        // land on the destination, which is precisely
                        // when the caller wants the block.
                        landed = seen.block().map(|room| (id, room));
                        // Same rule as a clean step: the walk is now
                        // localized, so hand back from somewhere true.
                        if let Some(interrupt) = armed.take() {
                            return Err(NavError {
                                at: current,
                                kind: NavErrorKind::Interrupted(interrupt),
                            });
                        }
                        continue 'replan;
                    }
                    // A desync outranks being hurt: the caller cannot
                    // act on a position nobody can work out.
                    None => return Err(desync(current)),
                }
            }
            // Every step of the route was walked, so this is the normal
            // end of a walk: `current` is `to` and the last step's block
            // is the destination's own.
            return Ok(Arrival::new(current, landed));
        }
    }

    /// Work out which room `seen` names, given that we were just in
    /// `at`. Only `at` itself and its immediate neighbors are considered:
    /// one unexpected step is recoverable, an arbitrary jump is not, and
    /// room names repeat across the ~26k-room world so a global search
    /// would confidently return the wrong room.
    ///
    /// `Some(at)` means the move never took — a legitimate answer, not a
    /// desync. `None` means stop and say so rather than guess.
    ///
    /// [`Navigator::goto`] uses this to recover a mis-stepped route; the
    /// farm runner uses it to find out where an AutoFlee left the
    /// character, since fleeing moves it with no navigator involved.
    /// Which room this block describes, given that we were last at `at`.
    ///
    /// The whole graph is in scope here, unlike [`Navigator::localize`],
    /// because a room block carries more than a name. Names repeat —
    /// Newhaven alone has two "Newhaven, Narrow Road" rooms (1/2146 and
    /// 1/2151) — but their exits do not: 1/2146 shows north/east/west/down
    /// where 1/2151 shows only north. So a name match is narrowed by the
    /// exits before it is believed, and an answer is only returned when
    /// exactly one room survives.
    ///
    /// The matching rule lives in [`crate::lost::candidates`], which is
    /// also what the walking recovery starts from — one rule, so a room
    /// this cannot place and one that walking can place are talking about
    /// the same set.
    ///
    /// This is what lets a character that was moved further than one step
    /// — a flee chain, a recall — work out where it is and be walked back,
    /// which [`Navigator::localize`] cannot do from a name alone.
    ///
    /// Still only answers when the room is unique on sight, which is
    /// 18.7% of the world. When it returns `None` the honest next move is
    /// [`crate::lost::relocalize`], which walks until the answer is
    /// forced.
    pub fn localize_view(&self, at: RoomId, seen: &crate::events::RoomView) -> Option<RoomId> {
        // A one-hop answer is still the best answer when it exists: it
        // needs no disambiguation and cannot be fooled by a twin.
        if let Some(near) = self.localize(at, &seen.name) {
            return Some(near);
        }
        match crate::lost::candidates(&self.graph, seen).as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }

    /// Where a blind answer leaves the walk.
    ///
    /// "The room is very dark - you can't see anything" is printed BOTH on
    /// entering an unlit room AND in reply to a `look` while standing in
    /// one. It is evidence about the ROOM, never about whether a step
    /// landed, and this code used to read it as arrival wherever it turned
    /// up. Verified in `re/oracle` and reproduced live:
    ///
    /// ```text
    /// There is no exit in that direction!
    /// [HP=46/MA=11]:look
    /// The room is very dark - you can't see anything
    /// ```
    ///
    /// The board has just said the move did not happen, the navigator asks
    /// where it is, cannot see — and concluded it had ARRIVED at the room
    /// it was told it could not reach. `current` then advanced a room past
    /// reality and every later step compounded it, which is the desync
    /// that ended live runs with a stray `s` into a wall.
    ///
    /// So the answer depends entirely on what was asked:
    /// [`BlindContext::AfterMove`] follows a direction we sent, where dark
    /// really is the destination reporting itself; [`BlindContext::
    /// AfterLook`] follows a `look`, which moves nothing, so the walk is
    /// still exactly where it was.
    pub fn blind_position<'a>(after: BlindContext, expected: &'a str, here: &'a str) -> &'a str {
        match after {
            BlindContext::AfterMove => expected,
            BlindContext::AfterLook => here,
        }
    }

    /// The world this navigator routes over. Recovery needs it too, and
    /// a caller holding a navigator should not have to hold the graph
    /// beside it just to ask.
    pub fn graph(&self) -> &RoomGraph {
        &self.graph
    }

    pub fn localize(&self, at: RoomId, seen: &str) -> Option<RoomId> {
        self.graph
            .room(at)
            .into_iter()
            .flat_map(|r| r.exits.iter().flatten().map(|e| e.dest))
            .chain([at])
            .find(|&id| self.graph.room(id).is_some_and(|r| r.name == seen))
    }

    /// Resolve one step that has already been sent, opening a door in the
    /// way if there is one.
    ///
    /// `here` is the room the walk believes it is standing in, and it is
    /// load-bearing rather than decorative — see [`blind_position`].
    ///
    /// `open` is tried before `bash` because it is free: the board answers
    /// "The door was already open." when there was nothing to do, whereas
    /// a bash charges HP and needs a weapon. Every branch waits on a
    /// wording rather than a deadline, so a shut door costs a command
    /// instead of a step timeout.
    ///
    /// Only the graph decides whether an exit is a door. Reacting to the
    /// board's "the door is closed" alone would mean sending `open` at
    /// exits that have no door — a wasted command against flood control
    /// on every step of every walk.
    #[allow(clippy::too_many_arguments)]
    async fn walk_step(
        &self,
        step: Direction,
        exit_type: i64,
        searchable_hidden: bool,
        sent: crate::correlate::CmdId,
        expected: &str,
        here: &str,
        session: &Session,
        events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<StepOutcome, NavErrorKind> {
        let dir = dir_word(step);
        match self.wait_room(events, guard, armed, sent).await? {
            StepEvent::Arrived(room) => return Ok(StepOutcome::Arrived(Sighting::Block(room))),
            // A direction was just sent, so dark is the destination
            // reporting itself; the name comes from the graph edge we
            // chose, not from a guess that movement generally works.
            StepEvent::Blind => {
                return Ok(StepOutcome::Arrived(Sighting::Dark(
                    Navigator::blind_position(BlindContext::AfterMove, expected, here).to_string(),
                )));
            }
            // The graph says there is an exit and the board says there is
            // not, so the walk is not where it believes. Ask the room and
            // report what it actually is: goto answers a name mismatch by
            // re-localizing and re-routing, which is precisely the
            // recovery this needs — and asking costs one command instead
            // of a whole step deadline.
            // Bash and search wordings can only attribute to a bash or a
            // search the walk sent; unreachable here, kept for match
            // completeness.
            StepEvent::BashFailed
            | StepEvent::BashPaced
            | StepEvent::PickFailed
            | StepEvent::HiddenFound
            | StepEvent::HiddenMissed => {}
            // Unlocked but still shut: the `open` below is precisely
            // what it now needs.
            StepEvent::DoorUnlocked => {}
            // "There is no exit in that direction!" is what a HIDDEN exit
            // says too, and it is the only thing it says. The graph is
            // the only witness that this wall is a door, so it decides:
            // search here, re-localize everywhere else.
            StepEvent::NoSuchExit if self.search_hidden && searchable_hidden => {
                return self
                    .find_hidden(step, expected, here, session, events, guard, armed)
                    .await;
            }
            StepEvent::NoSuchExit => {
                let ask = session.send("look");
                return self
                    .arrival(here, expected, BlindContext::AfterLook, events, guard, armed, ask)
                    .await
                    .map(StepOutcome::StayedPut);
            }
            // Hand straight back so the caller can fight: no deadline is
            // going to produce a room block while this is true.
            StepEvent::CombatBlocked => {
                return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                    by: "combat".into(),
                }));
            }
            StepEvent::DoorYielded => {
                // Someone else's door, or one that swung on its own.
                let again = session.send(dir);
                return self
                    .arrival(here, expected, BlindContext::AfterMove, events, guard, armed, again)
                    .await
                    .map(StepOutcome::Arrived);
            }
            StepEvent::DoorBlocked if !is_door(exit_type) => {
                // The graph says there is no door here, so we have no
                // business opening one. Let the deadline path report it.
                return Err(NavErrorKind::Expect(ExpectError::Timeout {
                    needle: "room block after movement".into(),
                    tail: "blocked by a door the graph does not know about".into(),
                }));
            }
            StepEvent::DoorBlocked => {}
        }

        let opened = session.send(&format!("open {dir}"));
        match self.wait_room(events, guard, armed, opened).await? {
            // Unreachable under the reply grammar (a block never
            // attributes to an open); kept for match completeness.
            StepEvent::Arrived(room) => return Ok(StepOutcome::Arrived(Sighting::Block(room))),
            // An `open` never moves the character, so a dark line
            // attributed to it says nothing about arrival — treating it
            // as AfterMove would advance `current` a room while the
            // character stands still, on the acceptance circuit's exact
            // terrain (doors into dark).
            StepEvent::Blind => {
                return Ok(StepOutcome::StayedPut(Sighting::Dark(here.to_string())));
            }
            // Unreachable for an open; kept for match completeness.
            StepEvent::BashFailed
            | StepEvent::BashPaced
            | StepEvent::PickFailed
            | StepEvent::HiddenFound
            | StepEvent::HiddenMissed => {}
            // Unlocked but still shut: the `open` below is precisely
            // what it now needs.
            StepEvent::DoorUnlocked => {}
            StepEvent::NoSuchExit => {
                let ask = session.send("look");
                return self
                    .arrival(here, expected, BlindContext::AfterLook, events, guard, armed, ask)
                    .await
                    .map(StepOutcome::StayedPut);
            }
            StepEvent::CombatBlocked => {
                return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                    by: "combat".into(),
                }));
            }
            StepEvent::DoorYielded => {
                let again = session.send(dir);
                return self
                    .arrival(here, expected, BlindContext::AfterMove, events, guard, armed, again)
                    .await
                    .map(StepOutcome::Arrived);
            }
            StepEvent::DoorBlocked => {}
        }

        // Picking first: it is a roll like bashing, but it costs a
        // command and no health, so where the character has the skill it
        // is strictly the cheaper way through. A character without the
        // skill just fails the roll.
        if self.pick_locks {
            let mut rolls = 0u32;
            while rolls < PICK_RETRIES {
                // Same reasoning as the bash loop: the guard IS the
                // health backstop and must be heard between rolls.
                if let Some(interrupt) = armed.take() {
                    return Err(NavErrorKind::Interrupted(interrupt));
                }
                let picked = session.send(&format!("picklock {dir}"));
                match self.wait_room(events, guard, armed, picked).await? {
                    // The lock gave. The door is still SHUT, so open it
                    // and only then walk -- sending the direction here
                    // walks into a closed door.
                    StepEvent::DoorUnlocked => {
                        let opened = session.send(&format!("open {dir}"));
                        return match self.wait_room(events, guard, armed, opened).await? {
                            StepEvent::DoorYielded => {
                                let again = session.send(dir);
                                self.arrival(
                                    here, expected, BlindContext::AfterMove, events, guard, armed,
                                    again,
                                )
                                .await
                                .map(StepOutcome::Arrived)
                            }
                            // Unlocked and still refusing to open is not
                            // something more picking will help with.
                            _ => Err(NavErrorKind::DoorLocked {
                                dir: dir.to_string(),
                                tried: "picked the lock, but the door would not open".into(),
                            }),
                        };
                    }
                    // Already open (somebody else's doing, or it swung):
                    // the way is clear, so walk it.
                    StepEvent::DoorYielded => {
                        let again = session.send(dir);
                        return self
                            .arrival(here, expected, BlindContext::AfterMove, events, guard, armed, again)
                            .await
                            .map(StepOutcome::Arrived);
                    }
                    StepEvent::PickFailed => {
                        rolls += 1;
                    }
                    // The cooldown scold paces the roll, it does not
                    // spend it -- counting it would deflate the budget
                    // exactly as it did for bashing (df861241).
                    StepEvent::BashPaced => {}
                    StepEvent::CombatBlocked => {
                        return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                            by: "combat".into(),
                        }));
                    }
                    // Anything else means the door is not the story any
                    // more; fall through to force rather than keep
                    // rolling at something that is not answering.
                    _ => break,
                }
            }
        }

        if !self.bash_doors {
            return Err(NavErrorKind::DoorLocked {
                dir: dir.to_string(),
                tried: if self.pick_locks {
                    format!("{PICK_RETRIES} picks, bashing off")
                } else {
                    "picking and bashing both off".into()
                },
            });
        }

        let mut rolls = 0u32;
        let mut scolds = 0u32;
        while rolls < BASH_RETRIES {
            // The guard IS the health backstop, so it must be heard
            // between rolls: a monster spawning mid-door-work otherwise
            // swings freely at a character locked in the loop.
            if let Some(interrupt) = armed.take() {
                return Err(NavErrorKind::Interrupted(interrupt));
            }
            let bashed = session.send(&format!("bash {dir}"));
            match self.wait_room(events, guard, armed, bashed).await? {
                // Attributable only to a pick, which this is not.
                StepEvent::PickFailed => {}
                // The lock gave to the swing but the door still stands;
                // spend the roll rather than looping on it for free.
                StepEvent::DoorUnlocked => {
                    rolls += 1;
                    continue;
                }
                // The roll came up short; the door still stands.
                StepEvent::BashFailed => {
                    rolls += 1;
                    continue;
                }
                // Never rolled: the action timer. Pacing spaces the
                // resend; bounded only against a board that never stops
                // scolding.
                StepEvent::BashPaced => {
                    scolds += 1;
                    if scolds > BASH_RETRIES * 4 {
                        break;
                    }
                    continue;
                }
                // The bash carried us through the doorway.
                StepEvent::Arrived(room) => return Ok(StepOutcome::Arrived(Sighting::Block(room))),
                StepEvent::Blind => {
                    return Ok(StepOutcome::Arrived(Sighting::Dark(
                        Navigator::blind_position(BlindContext::AfterMove, expected, here)
                            .to_string(),
                    )));
                }
                StepEvent::NoSuchExit => {
                    let ask = session.send("look");
                    return self
                        .arrival(here, expected, BlindContext::AfterLook, events, guard, armed, ask)
                        .await
                        .map(StepOutcome::StayedPut);
                }
                StepEvent::CombatBlocked => {
                    return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                        by: "combat".into(),
                    }));
                }
                // It only opened it; the step is still owed. The search
                // wordings are unreachable for a bash and join this arm
                // rather than a bare `continue`: a bounded loop that some
                // future wording could spin forever in is not bounded.
                StepEvent::DoorYielded
                | StepEvent::DoorBlocked
                | StepEvent::HiddenFound
                | StepEvent::HiddenMissed => {
                    let again = session.send(dir);
                    return self
                        .arrival(here, expected, BlindContext::AfterMove, events, guard, armed, again)
                        .await
                        .map(StepOutcome::Arrived);
                }
            }
        }
        Err(NavErrorKind::DoorLocked {
            dir: dir.to_string(),
            tried: format!("did not yield to {BASH_RETRIES} bashes"),
        })
    }

    /// Reveal a hidden exit (type 6) the board has just denied, then walk
    /// it.
    ///
    /// Entered only from a [`StepEvent::NoSuchExit`] on an exit the GRAPH
    /// calls hidden, which is what makes searching safe: the same wording
    /// on a plain exit means the walk is somewhere else and re-localizing
    /// is the right answer, and a hundred searches would hide that.
    ///
    /// Reactive rather than pre-emptive because SEARCH cannot tell "still
    /// hidden" from "already found" — both answer [`HIDDEN_MISSED`] — so a
    /// search-first walk would spend its whole budget on an exit that
    /// needed nothing. Sending the direction first asks the only question
    /// with two different answers.
    ///
    /// The guard is heard between rolls for the same reason it is between
    /// bashes: this loop can run for a hundred commands, and a character
    /// rummaging at a wall is a character standing still while something
    /// swings at it.
    #[allow(clippy::too_many_arguments)]
    async fn find_hidden(
        &self,
        step: Direction,
        expected: &str,
        here: &str,
        session: &Session,
        events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<StepOutcome, NavErrorKind> {
        let dir = dir_word(step);
        let mut rolls = 0u32;
        while rolls < SEARCH_ROLLS {
            if let Some(interrupt) = armed.take() {
                return Err(NavErrorKind::Interrupted(interrupt));
            }
            let searched = session.send(&format!("search {dir}"));
            match self.wait_room(events, guard, armed, searched).await? {
                // Found — and the board re-hides it in about five
                // minutes, so the step goes out now.
                StepEvent::HiddenFound => {
                    let again = session.send(dir);
                    return self
                        .arrival(here, expected, BlindContext::AfterMove, events, guard, armed, again)
                        .await
                        .map(StepOutcome::Arrived);
                }
                StepEvent::CombatBlocked => {
                    return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                        by: "combat".into(),
                    }));
                }
                // A search moves nobody, so a block attributed to one
                // means the walk is not where it believed. Report it as
                // the standing position and let `goto` re-localize.
                StepEvent::Arrived(room) => {
                    return Ok(StepOutcome::StayedPut(Sighting::Block(room)));
                }
                StepEvent::Blind => {
                    return Ok(StepOutcome::StayedPut(Sighting::Dark(here.to_string())));
                }
                // The failed roll, and every wording a search has no
                // business producing: spend a roll rather than loop on it.
                StepEvent::HiddenMissed
                | StepEvent::NoSuchExit
                | StepEvent::DoorBlocked
                | StepEvent::DoorYielded
                | StepEvent::BashFailed
                | StepEvent::BashPaced
                | StepEvent::PickFailed
                | StepEvent::DoorUnlocked => {
                    rolls += 1;
                }
            }
        }
        Err(NavErrorKind::Expect(ExpectError::Timeout {
            needle: "hidden exit revealed by search".into(),
            tail: format!("{SEARCH_ROLLS} searches did not reveal the exit {dir}"),
        }))
    }

    /// Wait specifically for a room block, treating door chatter as noise.
    ///
    /// `after` says what was last sent, which is the only thing that can
    /// tell a dark answer's meaning apart — see
    /// [`Navigator::blind_position`].
    async fn arrival(
        &self,
        here: &str,
        expected: &str,
        after: BlindContext,
        events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
        awaiting: crate::correlate::CmdId,
    ) -> Result<Sighting, NavErrorKind> {
        loop {
            match self
                .wait_room(events, guard, armed, awaiting)
                .await?
            {
                StepEvent::Arrived(room) => return Ok(Sighting::Block(room)),
                StepEvent::Blind => {
                    return Ok(Sighting::Dark(
                        Navigator::blind_position(after, expected, here).to_string(),
                    ));
                }
                StepEvent::NoSuchExit
                | StepEvent::BashFailed
                | StepEvent::BashPaced
                | StepEvent::PickFailed
                | StepEvent::DoorUnlocked
                | StepEvent::HiddenFound
                | StepEvent::HiddenMissed => continue,
                StepEvent::CombatBlocked => {
                    return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                        by: "combat".into(),
                    }));
                }
                StepEvent::DoorYielded | StepEvent::DoorBlocked => continue,
            }
        }
    }

    /// Ask the board what the character is carrying and wait for the
    /// answer.
    ///
    /// `i`'s reply body is unattributed -- its `Kind` is `Opaque`, whose
    /// `completes` never fires (see [`crate::purse::PurseMeter`]'s own
    /// doc) -- so the correlator can only mark the ECHO of our send.
    /// This arms a fresh meter on that echo, then keeps offering lines to
    /// it until one is actually shaped like an inventory reply (see
    /// `PurseMeter::observe`'s own doc): interleaved traffic in between
    /// -- a shout, another player's action -- is skipped rather than
    /// mistaken for the answer, which matters here specifically because
    /// this is the reading the toll learner compares before and after a
    /// crossing. Bounded by the same step deadline as a walk step: an
    /// `i` that never answers is exactly as informative as a step that
    /// never lands.
    async fn read_purse(
        &self,
        session: &Session,
        events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<crate::purse::Purse, NavErrorKind> {
        let id = session.send("i");
        let mut meter = crate::purse::PurseMeter::default();
        let deadline = tokio::time::Instant::now() + self.step_timeout;
        loop {
            let ev = tokio::time::timeout_at(deadline, events.recv()).await;
            if let Ok(Ok(ev)) = &ev {
                match guard.on_event(&ev.event) {
                    // Nothing more is going to land, same as `wait_room`.
                    Some(Interrupt::Died) => {
                        return Err(NavErrorKind::Interrupted(Interrupt::Died));
                    }
                    Some(hurt) => *armed = armed.take().or(Some(hurt)),
                    None => {}
                }
            }
            let cor = match ev {
                Err(_) => {
                    return Err(NavErrorKind::Expect(ExpectError::Timeout {
                        needle: "inventory reply".into(),
                        tail: String::new(),
                    }));
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => {
                    return Err(NavErrorKind::Expect(ExpectError::Closed { tail: String::new() }));
                }
                Ok(Ok(cor)) => cor,
            };
            if let crate::events::Event::Line(line) = &cor.event {
                if cor.answers == Some(id) {
                    // The echo: arms the meter, but is never itself fed
                    // to `observe` -- it is the send bouncing back, not
                    // the reply. Which line after this counts as the
                    // reply is `observe`'s call, not this loop's: it
                    // only clears on a line that looks like one.
                    meter.expect_reply();
                    continue;
                }
                if meter.observe(line) {
                    return Ok(meter.current());
                }
            }
        }
    }

    /// The next answer to `awaiting` within the step timeout, showing
    /// everything that goes past to the guard on the way.
    ///
    /// ONLY events attributed to `awaiting` classify. An unattributed
    /// room block — a stale look's answer, a login render, somebody
    /// else's re-render — used to satisfy the step and silently drift
    /// `current` a room ahead of the character, which is the desync that
    /// ended live runs. It now goes past like any other noise; if the
    /// step's real answer never arrives, the deadline surfaces as an
    /// error and the leg ends at the last VERIFIED position — honest
    /// about not knowing rather than confidently wrong. (Re-localize
    /// runs only on a mismatched arrival, not on a timeout.)
    async fn wait_room(
        &self,
        events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
        awaiting: crate::correlate::CmdId,
    ) -> Result<StepEvent, NavErrorKind> {
        let deadline = tokio::time::Instant::now() + self.step_timeout;
        loop {
            let ev = tokio::time::timeout_at(deadline, events.recv()).await;
            if let Ok(Ok(ev)) = &ev {
                match guard.on_event(&ev.event) {
                    // Nothing more is going to land. Say so now rather
                    // than sit out the deadline.
                    Some(Interrupt::Died) => {
                        return Err(NavErrorKind::Interrupted(Interrupt::Died));
                    }
                    Some(hurt) => *armed = armed.take().or(Some(hurt)),
                    None => {}
                }
            }
            let cor = match ev {
                Err(_) => {
                    return Err(NavErrorKind::Expect(ExpectError::Timeout {
                        needle: "room block after movement".into(),
                        tail: String::new(),
                    }));
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => {
                    return Err(NavErrorKind::Expect(ExpectError::Closed {
                        tail: String::new(),
                    }));
                }
                Ok(Ok(cor)) => cor,
            };
            if cor.answers != Some(awaiting) {
                continue;
            }
            match cor.event {
                crate::events::Event::RoomSeen(room) => {
                    // The one place a block is both attributed and about
                    // to become `current` — the only stream `on_room`
                    // ever sees. ARM rather than return: `goto` advances
                    // `current` before every `armed.take()`, so erring
                    // here would report the room being left, not the
                    // room this block described.
                    if let Some(sighted) = guard.on_room(&room) {
                        *armed = armed.take().or(Some(sighted));
                    }
                    return Ok(StepEvent::Arrived(room));
                }
                crate::events::Event::Line(line) => {
                    let line = line.to_lowercase();
                    // Checked before DOOR_YIELDED: this wording contains
                    // "open" too, but it means we are already through and
                    // the room block is behind it.
                    if line.contains(BASH_CARRIED_THROUGH) {
                        continue;
                    }
                    if line.contains(COMBAT_BLOCKED) {
                        return Ok(StepEvent::CombatBlocked);
                    }
                    if line.contains(NO_SUCH_EXIT) {
                        return Ok(StepEvent::NoSuchExit);
                    }
                    if line.contains(crate::sheet::TOO_DARK) {
                        return Ok(StepEvent::Blind);
                    }
                    if line.contains(BASH_FAILED) {
                        return Ok(StepEvent::BashFailed);
                    }
                    if line.contains(PICK_FAILED) {
                        return Ok(StepEvent::PickFailed);
                    }
                    if line.contains(BASH_PACED) {
                        return Ok(StepEvent::BashPaced);
                    }
                    if line.contains(HIDDEN_FOUND) {
                        return Ok(StepEvent::HiddenFound);
                    }
                    if line.contains(HIDDEN_MISSED) {
                        return Ok(StepEvent::HiddenMissed);
                    }
                    if line.contains(DOOR_UNLOCKED) {
                        return Ok(StepEvent::DoorUnlocked);
                    }
                    if DOOR_BLOCKED.iter().any(|m| line.contains(m)) {
                        return Ok(StepEvent::DoorBlocked);
                    }
                    if DOOR_YIELDED.iter().any(|m| line.contains(m)) {
                        return Ok(StepEvent::DoorYielded);
                    }
                    // The echo itself, or an unmodelled wording: the
                    // board accepted us; the outcome is still coming.
                    continue;
                }
                _ => continue,
            }
        }
    }
}
