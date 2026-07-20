//! Verified navigation: walk a computed route one step at a time,
//! confirming each arrival by the parsed room name. Never blind —
//! desyncs (lag, blocked exits, combat interruptions) surface as
//! errors or re-localization, not silent drift.

use std::sync::Arc;

use crate::graph::RoomGraph;
use crate::session::{ExpectError, Session};
use mud_core::content::{Direction, RoomId};

#[derive(Debug)]
pub enum NavError {
    NoRoute,
    /// The room we arrived in does not match the graph's expectation
    /// and could not be re-localized among neighbors.
    Desync {
        expected: String,
        saw: Option<String>,
    },
    Expect(ExpectError),
}

impl std::fmt::Display for NavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NavError::NoRoute => write!(f, "no route to target"),
            NavError::Desync { expected, saw } => {
                write!(f, "desync: expected {expected:?}, saw {saw:?}")
            }
            NavError::Expect(e) => write!(f, "{e}"),
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
    pub async fn goto(&self, session: &Session, from: RoomId, to: RoomId) -> Result<(), NavError> {
        let mut current = from;
        let mut failures = 0u32;
        let mut events = session.events();
        'replan: loop {
            if current == to {
                return Ok(());
            }
            let route = self.graph.route(current, to).ok_or(NavError::NoRoute)?;
            for step in route {
                let expected_id = self
                    .graph
                    .room(current)
                    .and_then(|r| r.exits[step as usize].as_ref())
                    .map(|e| e.dest)
                    .ok_or(NavError::NoRoute)?;
                let expected_name = self
                    .graph
                    .room(expected_id)
                    .map(|r| r.name.clone())
                    .ok_or(NavError::NoRoute)?;

                // Stale room blocks (a prior look, an earlier step's
                // echo) must not satisfy this step's verification.
                while events.try_recv().is_ok() {}
                session.send(dir_word(step));
                let seen = self.wait_room(&mut events).await?;
                if seen == expected_name {
                    current = expected_id;
                    continue;
                }
                failures += 1;
                if failures > self.max_failures {
                    return Err(NavError::Desync {
                        expected: expected_name,
                        saw: Some(seen),
                    });
                }
                // Re-localize: did we land in a known neighbor (or not
                // move at all)?
                let neighbors = self
                    .graph
                    .room(current)
                    .into_iter()
                    .flat_map(|r| r.exits.iter().flatten().map(|e| e.dest))
                    .chain([current]);
                let mut found = None;
                for id in neighbors {
                    if self.graph.room(id).is_some_and(|r| r.name == seen) {
                        found = Some(id);
                        break;
                    }
                }
                match found {
                    Some(id) => {
                        current = id;
                        continue 'replan;
                    }
                    None => {
                        return Err(NavError::Desync {
                            expected: expected_name,
                            saw: Some(seen),
                        });
                    }
                }
            }
            return Ok(());
        }
    }

    /// Next RoomSeen name within the step timeout.
    async fn wait_room(
        &self,
        events: &mut tokio::sync::broadcast::Receiver<crate::events::Event>,
    ) -> Result<String, NavError> {
        let deadline = tokio::time::Instant::now() + self.step_timeout;
        loop {
            let ev = tokio::time::timeout_at(deadline, events.recv()).await;
            match ev {
                Err(_) => {
                    return Err(NavError::Expect(ExpectError::Timeout {
                        needle: "room block after movement".into(),
                        tail: String::new(),
                    }));
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => {
                    return Err(NavError::Expect(ExpectError::Closed {
                        tail: String::new(),
                    }));
                }
                Ok(Ok(crate::events::Event::RoomSeen(room))) => return Ok(room.name),
                Ok(Ok(_)) => continue,
            }
        }
    }
}
