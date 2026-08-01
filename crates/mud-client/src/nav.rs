//! Verified navigation: walk a computed route one step at a time,
//! confirming each arrival by the parsed room name. Never blind —
//! desyncs (lag, blocked exits, combat interruptions) surface as
//! errors or re-localization, not silent drift.
//!
//! Two things the walker knows how to do beyond putting one foot in
//! front of the other:
//!
//! - **Doors.** A route through a door or gate (exit types 2, 7, 0xb)
//!   opens it rather than stopping at it, falling back to bashing when
//!   `open` will not shift it. Only the graph decides an exit is a door;
//!   see [`Navigator::goto`]'s step handling.
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
}

impl Default for NavConfig {
    fn default() -> Self {
        NavConfig {
            step_timeout_ms: 15_000,
            bash_doors: true,
        }
    }
}

/// Verification failures tolerated before a walk aborts.
const MAX_FAILURES: u32 = 3;

/// What one resolved step actually established. `Arrived` carries the
/// name of a room the character MOVED into; `StayedPut` carries the name
/// answering the recovery `look` after a refusal — the walk knows it did
/// not move, and that knowledge must survive: ~51k adjacent room pairs
/// share a name (1/2151 and 1/2146 are both "Newhaven, Narrow Road"),
/// so a refusal's look-answer treated as an arrival drifts `current`
/// into the same-named twin while the character stands still.
enum StepOutcome {
    Arrived(String),
    StayedPut(String),
}

/// Exit types that are a door or gate you can open (`theft.md` §8.1:
/// "pickable types are 2, 7, 0xb"). Deliberately not 6 (hidden), 9/0x18
/// (traps), 0x10 (timed), 0x14 (alignment) or 0x16/0x17 (spell/ability
/// gates) — none of those yield to `open`.
fn is_door(exit_type: i64) -> bool {
    matches!(exit_type, 2 | 7 | 0xb)
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
const DOOR_YIELDED: [&str; 4] = [
    "is now open",
    "was already open",
    "bashed the",
    "unlocked the door",
];

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

/// The cooldown scold: the bash sat on the action timer and never
/// rolled. It paces, it does not fail — counting it against the roll
/// budget deflated twenty nominal rolls to a handful of real ones.
const BASH_PACED: &str = "must wait before you may do that";

/// Real failed rolls per step before the door is declared unbashable —
/// sized to the field evidence (opendoor.lua leaned on this door for up
/// to 60). Scolds are bounded separately and generously; the walk's
/// guard is the health backstop, this bound the diagnosability one.
const BASH_RETRIES: u32 = 60;

/// The direction an "Obvious exits" token points.
///
/// Exits render as display strings, not commands — "closed door north",
/// "open gate west" — so the direction is the trailing word. Trapdoors
/// are the exception worth knowing about: they render as "closed trap
/// door above" / "open trap door below" (DLL 0xccd5d, 0xccda4), so the
/// vertical pair has to accept those words as well as up/down.
fn direction_of(token: &str) -> Option<Direction> {
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
    /// A room block: the name we landed on.
    Arrived(String),
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
    /// The bash never rolled: it sat on the action timer.
    BashPaced,
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
        }
    }

    /// Walk from `from` to `to`, verifying every step by the parsed
    /// destination room name. On mismatch, re-localize among the
    /// previous room's neighbors and re-route; abort after repeated
    /// failures.
    ///
    /// Returns the room the walk ended in — `to` on success, and on
    /// failure [`NavError::at`] carries the last room it verified.
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
    ) -> Result<RoomId, NavError> {
        let mut current = from;
        let mut failures = 0u32;
        let mut armed: Option<Interrupt> = None;
        let mut events = session.events();
        'replan: loop {
            if current == to {
                return Ok(current);
            }
            let route = self
                .graph
                .route(current, to)
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

                let exit_type = self
                    .graph
                    .room(current)
                    .and_then(|r| r.exits[step as usize].as_ref())
                    .map(|e| e.exit_type)
                    .unwrap_or(0);

                // The room the walk believes it is standing in. Needed
                // because a dark answer means "still here" when it
                // follows a look -- see Navigator::blind_position.
                let here_name = self
                    .graph
                    .room(current)
                    .map(|r| r.name.clone())
                    .unwrap_or_default();

                let sent = session.send(dir_word(step));
                let outcome = self
                    .walk_step(
                        step,
                        exit_type,
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
                    StepOutcome::Arrived(name) if name == expected_name => {
                        current = expected_id;
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
                    StepOutcome::StayedPut(name) if name == here_name => {
                        failures += 1;
                        if failures > MAX_FAILURES {
                            return Err(NavError {
                                at: current,
                                kind: NavErrorKind::Desync {
                                    expected: expected_name.clone(),
                                    saw: name,
                                },
                            });
                        }
                        continue 'replan;
                    }
                    StepOutcome::Arrived(name) | StepOutcome::StayedPut(name) => name,
                };
                failures += 1;
                let desync = |at| NavError {
                    at,
                    kind: NavErrorKind::Desync {
                        expected: expected_name.clone(),
                        saw: seen.clone(),
                    },
                };
                if failures > MAX_FAILURES {
                    return Err(desync(current));
                }
                match self.localize(current, &seen) {
                    Some(id) => {
                        current = id;
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
            return Ok(current);
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
    /// The observed exits are treated as a SUBSET of the graph's, never an
    /// equal set: hidden exits (type 6) and text-triggered action exits
    /// (type 10) are in the graph but deliberately absent from the board's
    /// "Obvious exits" line, so demanding equality would reject the right
    /// room. Where several rooms still qualify, an exact match is
    /// preferred before giving up.
    ///
    /// This is what lets a character that was moved further than one step
    /// — a flee chain, a recall — work out where it is and be walked back,
    /// which [`Navigator::localize`] cannot do from a name alone.
    pub fn localize_view(&self, at: RoomId, seen: &crate::events::RoomView) -> Option<RoomId> {
        // A one-hop answer is still the best answer when it exists: it
        // needs no disambiguation and cannot be fooled by a twin.
        if let Some(near) = self.localize(at, &seen.name) {
            return Some(near);
        }
        let observed: Vec<Direction> = seen.exits.iter().filter_map(|e| direction_of(e)).collect();
        let named: Vec<RoomId> = self
            .graph
            .rooms_named(&seen.name)
            .into_iter()
            .filter(|id| {
                self.graph.room(*id).is_some_and(|r| {
                    observed
                        .iter()
                        .all(|d| r.exits[*d as usize].is_some())
                })
            })
            .collect();
        match named.as_slice() {
            [only] => return Some(*only),
            [] => return None,
            _ => {}
        }
        // Several rooms admit the observed exits. Insist on an exact set
        // before answering, and if that is still not unique, say nothing.
        let exact: Vec<RoomId> = named
            .into_iter()
            .filter(|id| {
                self.graph.room(*id).is_some_and(|r| {
                    r.exits.iter().filter(|e| e.is_some()).count() == observed.len()
                })
            })
            .collect();
        match exact.as_slice() {
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
            StepEvent::Arrived(name) => return Ok(StepOutcome::Arrived(name)),
            // A direction was just sent, so dark is the destination
            // reporting itself; the name comes from the graph edge we
            // chose, not from a guess that movement generally works.
            StepEvent::Blind => {
                return Ok(StepOutcome::Arrived(
                    Navigator::blind_position(BlindContext::AfterMove, expected, here).to_string(),
                ));
            }
            // The graph says there is an exit and the board says there is
            // not, so the walk is not where it believes. Ask the room and
            // report what it actually is: goto answers a name mismatch by
            // re-localizing and re-routing, which is precisely the
            // recovery this needs — and asking costs one command instead
            // of a whole step deadline.
            // Bash wordings can only attribute to a bash the walk sent;
            // unreachable here, kept for match completeness.
            StepEvent::BashFailed | StepEvent::BashPaced => {}
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
            StepEvent::Arrived(name) => return Ok(StepOutcome::Arrived(name)),
            // An `open` never moves the character, so a dark line
            // attributed to it says nothing about arrival — treating it
            // as AfterMove would advance `current` a room while the
            // character stands still, on the acceptance circuit's exact
            // terrain (doors into dark).
            StepEvent::Blind => {
                return Ok(StepOutcome::StayedPut(here.to_string()));
            }
            // Unreachable for an open; kept for match completeness.
            StepEvent::BashFailed | StepEvent::BashPaced => {}
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

        if !self.bash_doors {
            return Err(NavErrorKind::Expect(ExpectError::Timeout {
                needle: "room block after movement".into(),
                tail: "door is locked and bash_doors is off".into(),
            }));
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
                StepEvent::Arrived(name) => return Ok(StepOutcome::Arrived(name)),
                StepEvent::Blind => {
                    return Ok(StepOutcome::Arrived(
                        Navigator::blind_position(BlindContext::AfterMove, expected, here)
                            .to_string(),
                    ));
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
                // It only opened it; the step is still owed.
                StepEvent::DoorYielded | StepEvent::DoorBlocked => {
                    let again = session.send(dir);
                    return self
                        .arrival(here, expected, BlindContext::AfterMove, events, guard, armed, again)
                        .await
                        .map(StepOutcome::Arrived);
                }
            }
        }
        Err(NavErrorKind::Expect(ExpectError::Timeout {
            needle: "room block after movement".into(),
            tail: format!("door did not yield to {BASH_RETRIES} bashes"),
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
    ) -> Result<String, NavErrorKind> {
        loop {
            match self
                .wait_room(events, guard, armed, awaiting)
                .await?
            {
                StepEvent::Arrived(name) => return Ok(name),
                StepEvent::Blind => {
                    return Ok(Navigator::blind_position(after, expected, here).to_string());
                }
                StepEvent::NoSuchExit | StepEvent::BashFailed | StepEvent::BashPaced => continue,
                StepEvent::CombatBlocked => {
                    return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                        by: "combat".into(),
                    }));
                }
                StepEvent::DoorYielded | StepEvent::DoorBlocked => continue,
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
                    return Ok(StepEvent::Arrived(room.name));
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
                    if line.contains(BASH_PACED) {
                        return Ok(StepEvent::BashPaced);
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
