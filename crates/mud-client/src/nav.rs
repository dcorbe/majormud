//! Verified navigation: walk a computed route one step at a time,
//! confirming each arrival by the parsed room name. Never blind —
//! desyncs (lag, blocked exits, combat interruptions) surface as
//! errors or re-localization, not silent drift.

use std::sync::Arc;

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
        }
    }
}

impl std::error::Error for NavError {}

pub struct Navigator {
    graph: Arc<RoomGraph>,
    /// Consecutive verification failures tolerated before aborting.
    max_failures: u32,
    /// Per-step arrival deadline.
    step_timeout: std::time::Duration,
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
    pub fn new(graph: Arc<RoomGraph>) -> Self {
        Navigator {
            graph,
            max_failures: 3,
            step_timeout: std::time::Duration::from_secs(15),
        }
    }

    /// Walk from `from` to `to`, verifying every step by the parsed
    /// destination room name. On mismatch, re-localize among the
    /// previous room's neighbors and re-route; abort after repeated
    /// failures.
    ///
    /// Returns the room the walk ended in — `to` on success, and on
    /// failure [`NavError::at`] carries the last room it verified.
    pub async fn goto(
        &self,
        session: &Session,
        from: RoomId,
        to: RoomId,
    ) -> Result<RoomId, NavError> {
        let mut current = from;
        let mut failures = 0u32;
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
                // echo) must not satisfy this step's verification.
                crate::session::drain(&mut events);
                session.send(dir_word(step));
                let seen = self
                    .wait_room(&mut events)
                    .await
                    .map_err(|kind| NavError { at: current, kind })?;
                if seen == expected_name {
                    current = expected_id;
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
                if failures > self.max_failures {
                    return Err(desync(current));
                }
                match self.localize(current, &seen) {
                    Some(id) => {
                        current = id;
                        continue 'replan;
                    }
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

    /// Next RoomSeen name within the step timeout.
    async fn wait_room(
        &self,
        events: &mut tokio::sync::broadcast::Receiver<crate::events::Event>,
    ) -> Result<String, NavErrorKind> {
        let deadline = tokio::time::Instant::now() + self.step_timeout;
        loop {
            let ev = tokio::time::timeout_at(deadline, events.recv()).await;
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
                Ok(Ok(crate::events::Event::RoomSeen(room))) => return Ok(room.name),
                Ok(Ok(_)) => continue,
            }
        }
    }
}
