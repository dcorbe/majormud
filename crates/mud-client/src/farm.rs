//! Autonomous farming: the `[farm]` configuration and the validated
//! patrol plan built from it.
//!
//! A patrol circuit is a list of rooms the character walks between,
//! farming each with the [`crate::bot`] policy core. Every room id and
//! every leg between them is checked against the room graph up front —
//! a typo in `[farm].circuit` must fail before the client connects, not
//! halfway around the lap with a live character standing in a spawn.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use mud_core::content::RoomId;

use crate::events::Event;
use crate::graph::RoomGraph;

/// How long to wait for the prompt that acknowledges a sent command
/// before assuming the board swallowed it and moving on. Waiting forever
/// would wedge the runner on any line the board answers silently.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// `"1/860"` -> map 1, room 860. The `mmc path` argument syntax.
pub fn parse_room_id(s: &str) -> Option<RoomId> {
    let (map, room) = s.split_once('/')?;
    Some(RoomId {
        map: map.parse().ok()?,
        room: room.parse().ok()?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FarmConfig {
    /// Room graph database (the decoded WG3-NT dump).
    pub content: PathBuf,
    /// Where the character is standing at login, as `map/room`. Verified
    /// by room name before the first step; the runner refuses to walk
    /// from a position it cannot confirm.
    pub start: String,
    /// Rooms to farm, in patrol order. The lap wraps from the last back
    /// to the first.
    pub circuit: Vec<String>,
    /// Laps to walk; 0 = until stopped.
    pub loops: u32,
    /// Wall-clock cap; 0 = unlimited.
    pub max_seconds: u64,
    /// Leave a stop after this many consecutive prompts with nothing to
    /// fight and nothing to do.
    pub dwell_idle_prompts: u32,
    /// Never start a leg below this hp% (needs a known max HP); 0
    /// disables the gate. Travel is unprotected — the bot does not fight
    /// or flee while the navigator is walking — so this is what keeps a
    /// wounded character from setting off.
    pub depart_at_percent: u32,
    /// How long to hold off sending after the board says it dropped our
    /// input ("Why don't you slow down for a few seconds?").
    pub slowdown_backoff_ms: u64,
    /// Prompts to wait for a heal to show progress before concluding it
    /// never landed and re-arming the policy.
    pub heal_retry_prompts: u32,
    /// Lines that mean the heal was refused outright, matched as
    /// substrings. Empty by default: no refusal wording has been
    /// captured off the board yet, and inventing one would be a fixture
    /// that is tidier than reality.
    pub heal_refused: Vec<String>,
}

impl Default for FarmConfig {
    fn default() -> Self {
        FarmConfig {
            content: PathBuf::from("re/mmud_wgnt.sqlite"),
            start: String::new(),
            circuit: Vec::new(),
            loops: 0,
            max_seconds: 0,
            dwell_idle_prompts: 3,
            depart_at_percent: 80,
            slowdown_backoff_ms: 5000,
            heal_retry_prompts: 3,
            heal_refused: Vec::new(),
        }
    }
}

/// A circuit whose rooms all exist and whose every leg is walkable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FarmPlan {
    pub start: RoomId,
    pub circuit: Vec<RoomId>,
}

impl FarmPlan {
    pub fn build(cfg: &FarmConfig, graph: &RoomGraph) -> Result<FarmPlan, String> {
        if cfg.circuit.is_empty() {
            return Err("circuit is empty; [farm].circuit needs at least one room".into());
        }
        let resolve = |s: &String| -> Result<RoomId, String> {
            let id = parse_room_id(s)
                .ok_or_else(|| format!("bad room id {s:?}; want map/room, e.g. 1/1"))?;
            if graph.room(id).is_none() {
                return Err(format!("room {}/{} is not in the graph", id.map, id.room));
            }
            Ok(id)
        };

        let start = resolve(&cfg.start)?;
        let circuit = cfg
            .circuit
            .iter()
            .map(resolve)
            .collect::<Result<Vec<_>, _>>()?;

        // Every leg the runner will ever walk: onto the circuit, around
        // it, and the wrap that starts the next lap.
        let mut legs = vec![(start, circuit[0])];
        for pair in circuit.windows(2) {
            legs.push((pair[0], pair[1]));
        }
        legs.push((circuit[circuit.len() - 1], circuit[0]));
        for (from, to) in legs {
            if graph.route(from, to).is_none() {
                return Err(format!(
                    "no route from {}/{} to {}/{}",
                    from.map, from.room, to.map, to.room
                ));
            }
        }

        Ok(FarmPlan { start, circuit })
    }
}

/// The runner's only outbound path.
///
/// [`crate::session::Session::send`] is an unbounded, unacknowledged
/// queue, and the live board paces at 1500ms, so firing every bot
/// decision straight at it buries the character under minutes of stale
/// commands with no way to cancel. The gate keeps exactly one command in
/// flight until the board's prompt acknowledges it.
///
/// It is also the only thing that acts on [`Event::SlowDown`], which
/// means the board *dropped* our input. The dropped command goes back to
/// the front of the queue and sending pauses until the board calms down.
/// The bot's own latches stay truthful through all of this: it decided to
/// swing once, and the swing does eventually happen, so nothing has to
/// re-arm.
pub struct Gate {
    queue: VecDeque<String>,
    /// The command awaiting its prompt, and when it went out.
    in_flight: Option<(String, Instant)>,
    /// Nothing may be sent before this instant (flood control).
    blocked_until: Option<Instant>,
    backoff: Duration,
}

impl Gate {
    pub fn new(backoff: Duration) -> Self {
        Gate {
            queue: VecDeque::new(),
            in_flight: None,
            blocked_until: None,
            backoff,
        }
    }

    /// Queue a command. Order is preserved; a resend jumps ahead of it.
    pub fn push(&mut self, line: String) {
        self.queue.push_back(line);
    }

    /// The command awaiting acknowledgement, if any.
    pub fn in_flight(&self) -> Option<&str> {
        self.in_flight.as_ref().map(|(line, _)| line.as_str())
    }

    pub fn on_event(&mut self, ev: &Event, now: Instant) {
        match ev {
            // The board answered, so whatever we sent landed.
            Event::Prompt { .. } => self.in_flight = None,
            Event::SlowDown => {
                // Flood control ate the in-flight command. Taking it here
                // is what keeps a burst of scoldings from queueing a
                // resend apiece: the second SlowDown finds nothing left.
                if let Some((line, _)) = self.in_flight.take() {
                    self.queue.push_front(line);
                }
                self.blocked_until = Some(now + self.backoff);
            }
            _ => {}
        }
    }

    /// The next command clear to send, or `None` while the gate is
    /// waiting on an acknowledgement or a backoff.
    pub fn poll(&mut self, now: Instant) -> Option<String> {
        if let Some(until) = self.blocked_until {
            if now < until {
                return None;
            }
            self.blocked_until = None;
        }
        if let Some((_, sent_at)) = &self.in_flight {
            if now.duration_since(*sent_at) < ACK_TIMEOUT {
                return None;
            }
            self.in_flight = None;
        }
        let line = self.queue.pop_front()?;
        self.in_flight = Some((line.clone(), now));
        Some(line)
    }

    /// When the gate could next release a command without any new event
    /// arriving — a backoff expiry or an unacknowledged send timing out.
    /// The runner sleeps until this rather than polling.
    pub fn next_deadline(&self) -> Option<Instant> {
        let backoff = self.blocked_until;
        let ack = self.in_flight.as_ref().map(|(_, at)| *at + ACK_TIMEOUT);
        match (backoff, ack) {
            // Both must pass before anything can go out.
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Decides when a heal has plainly failed, so the runner can call
/// [`crate::bot::Bot::rearm`].
///
/// The bot's heal latch clears only when HP climbs back over the
/// threshold, so a heal that never lands latches it forever: the
/// character sits wounded and never rests again. `bot.rs` documents that
/// the runner rearms it "when it sees the heal was refused" — but there
/// is nothing to see. No refusal wording survives in any of the 51
/// captured transcripts, and the Rust server has no rest command at all,
/// so a string predicate would be a fixture tidier than the board.
///
/// This watches for *progress* instead, clocked by the board's own
/// prompts: a heal that has not moved HP after `heal_retry_prompts`
/// prompts never landed, whatever the reason. `[farm].heal_refused`
/// remains as the escape hatch for the day a real refusal line is
/// finally captured off the live board.
///
/// Note that [`crate::bot::Bot::rearm`] releases the flee latch along
/// with the heal, so a rearm may also let a wounded character run again.
/// That is the right outcome: if resting is not working, leaving is the
/// other option.
pub struct HealWatch {
    heal_command: String,
    retry_prompts: u32,
    refused: Vec<String>,
    /// HP at the first prompt after the heal went out; `None` until then.
    baseline: Option<i32>,
    /// Prompts seen since the heal went out. `None` = not watching.
    prompts: Option<u32>,
}

impl HealWatch {
    pub fn new(bot: &crate::bot::BotConfig, farm: &FarmConfig) -> Self {
        HealWatch {
            heal_command: bot.heal_command.clone(),
            retry_prompts: farm.heal_retry_prompts,
            refused: farm.heal_refused.clone(),
            baseline: None,
            prompts: None,
        }
    }

    /// Called for every command the gate actually releases. A fresh heal
    /// restarts the watch: new baseline, new patience.
    pub fn on_sent(&mut self, line: &str) {
        if line == self.heal_command {
            self.baseline = None;
            self.prompts = Some(0);
        }
    }

    /// Returns true exactly once per heal, when it is time to rearm.
    pub fn on_event(&mut self, ev: &Event) -> bool {
        if self.prompts.is_none() {
            return false;
        }
        match ev {
            Event::Line(line) => {
                if self.refused.iter().any(|r| line.contains(r.as_str())) {
                    self.stop();
                    return true;
                }
                false
            }
            Event::Prompt { hp, .. } => {
                let seen = self.prompts.unwrap_or(0) + 1;
                self.prompts = Some(seen);
                match self.baseline {
                    // First prompt after the heal: this is the mark to beat.
                    None => {
                        self.baseline = Some(*hp);
                        false
                    }
                    // Any climb at all is the heal doing its job.
                    Some(base) if *hp > base => {
                        self.stop();
                        false
                    }
                    Some(_) => {
                        if seen >= self.retry_prompts {
                            self.stop();
                            return true;
                        }
                        false
                    }
                }
            }
            _ => false,
        }
    }

    fn stop(&mut self) {
        self.baseline = None;
        self.prompts = None;
    }
}
