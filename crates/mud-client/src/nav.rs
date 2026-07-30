//! Verified navigation: walk a computed route one step at a time,
//! confirming each arrival by the parsed room name. Never blind —
//! desyncs (lag, blocked exits, combat interruptions) surface as
//! errors or re-localization, not silent drift.

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
}

/// Watches the events a walk goes past and says when to stop walking.
///
/// The guard only ever observes. It sends nothing, which is what lets
/// travel stay a single-sender phase: the navigator keeps the connection
/// for the whole walk and merely learns when to hand it back.
pub trait TravelGuard {
    fn on_event(&mut self, ev: &crate::events::Event) -> Option<Interrupt>;
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

/// Exit types that are a door or gate you can open (`theft.md` §8.1:
/// "pickable types are 2, 7, 0xb"). Deliberately not 6 (hidden), 9/0x18
/// (traps), 0x10 (timed), 0x14 (alignment) or 0x16/0x17 (spell/ability
/// gates) — none of those yield to `open`.
fn is_door(exit_type: i64) -> bool {
    matches!(exit_type, 2 | 7 | 0xb)
}

/// Lines that mean a shut door turned the step back. Lowercased before
/// matching, because the DLL ships both "Closed!" and "closed!".
const DOOR_BLOCKED: [&str; 5] = [
    "the door is closed",
    "the gate is closed",
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

/// The other bash outcome (0xd538e) carries the character through the
/// doorway itself, so a room block is already on its way and sending the
/// direction again would overshoot by a room.
const BASH_CARRIED_THROUGH: &str = "walk through";

/// What one command produced while walking a step.
enum StepEvent {
    /// A room block: the name we landed on.
    Arrived(String),
    /// A shut door turned us back.
    DoorBlocked,
    /// The door is open now, but we are still on this side of it.
    DoorYielded,
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

                // Stale room blocks (a prior look, an earlier step's
                // echo) must not satisfy this step's verification. The
                // guard still sees them: nothing is in flight yet, so a
                // trip here is honoured before the step goes out at all.
                crate::session::drain(&mut events, |ev| {
                    armed = armed.take().or_else(|| guard.on_event(ev));
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

                session.send(dir_word(step));
                let outcome = self
                    .walk_step(step, exit_type, session, &mut events, guard, &mut armed)
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
                if seen == expected_name {
                    current = expected_id;
                    if let Some(interrupt) = armed.take() {
                        return Err(NavError {
                            at: current,
                            kind: NavErrorKind::Interrupted(interrupt),
                        });
                    }
                    continue;
                }
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
    async fn walk_step(
        &self,
        step: Direction,
        exit_type: i64,
        session: &Session,
        events: &mut tokio::sync::broadcast::Receiver<crate::events::Event>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<String, NavErrorKind> {
        let dir = dir_word(step);
        match self.wait_room(events, guard, armed).await? {
            StepEvent::Arrived(name) => return Ok(name),
            StepEvent::DoorYielded => {
                // Someone else's door, or one that swung on its own.
                session.send(dir);
                return self.arrival(events, guard, armed).await;
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

        session.send(&format!("open {dir}"));
        match self.wait_room(events, guard, armed).await? {
            // Some boards walk you through on the open itself.
            StepEvent::Arrived(name) => return Ok(name),
            StepEvent::DoorYielded => {
                session.send(dir);
                return self.arrival(events, guard, armed).await;
            }
            StepEvent::DoorBlocked => {}
        }

        if !self.bash_doors {
            return Err(NavErrorKind::Expect(ExpectError::Timeout {
                needle: "room block after movement".into(),
                tail: "door is locked and bash_doors is off".into(),
            }));
        }

        session.send(&format!("bash {dir}"));
        match self.wait_room(events, guard, armed).await? {
            // The bash carried us through the doorway.
            StepEvent::Arrived(name) => Ok(name),
            // It only opened it; the step is still owed.
            StepEvent::DoorYielded | StepEvent::DoorBlocked => {
                session.send(dir);
                self.arrival(events, guard, armed).await
            }
        }
    }

    /// Wait specifically for a room block, treating door chatter as noise.
    async fn arrival(
        &self,
        events: &mut tokio::sync::broadcast::Receiver<crate::events::Event>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<String, NavErrorKind> {
        loop {
            match self.wait_room(events, guard, armed).await? {
                StepEvent::Arrived(name) => return Ok(name),
                StepEvent::DoorYielded | StepEvent::DoorBlocked => continue,
            }
        }
    }

    /// Next RoomSeen name within the step timeout, showing everything
    /// that goes past to the guard on the way.
    async fn wait_room(
        &self,
        events: &mut tokio::sync::broadcast::Receiver<crate::events::Event>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<StepEvent, NavErrorKind> {
        let deadline = tokio::time::Instant::now() + self.step_timeout;
        loop {
            let ev = tokio::time::timeout_at(deadline, events.recv()).await;
            if let Ok(Ok(ev)) = &ev {
                match guard.on_event(ev) {
                    // Nothing more is going to land. Say so now rather
                    // than sit out the deadline.
                    Some(Interrupt::Died) => {
                        return Err(NavErrorKind::Interrupted(Interrupt::Died));
                    }
                    Some(hurt) => *armed = armed.take().or(Some(hurt)),
                    None => {}
                }
            }
            match ev {
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
                Ok(Ok(crate::events::Event::RoomSeen(room))) => {
                    return Ok(StepEvent::Arrived(room.name));
                }
                Ok(Ok(crate::events::Event::Line(line))) => {
                    let line = line.to_lowercase();
                    // Checked before DOOR_YIELDED: this wording contains
                    // "open" too, but it means we are already through and
                    // the room block is behind it.
                    if line.contains(BASH_CARRIED_THROUGH) {
                        continue;
                    }
                    if DOOR_BLOCKED.iter().any(|m| line.contains(m)) {
                        return Ok(StepEvent::DoorBlocked);
                    }
                    if DOOR_YIELDED.iter().any(|m| line.contains(m)) {
                        return Ok(StepEvent::DoorYielded);
                    }
                    continue;
                }
                Ok(Ok(_)) => continue,
            }
        }
    }
}
