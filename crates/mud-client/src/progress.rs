//! The live progress feed for a long-running command.
//!
//! `mmc farm` used to print nothing between "started" and "finished", so
//! a run that was working and a run that was wedged looked exactly the
//! same for as long as they ran — on a live board that can be an hour.
//!
//! This renders the parsed event stream into lines an operator can watch.
//! It reads the same [`crate::events::Event`] broadcast the bot does, so
//! it needs no cooperation from the runner and cannot perturb it: a
//! progress view that had to be threaded through [`crate::farm::run_farm`]
//! would be another thing to keep in step with every phase change.
//!
//! Two levels. The default is what an operator cares about — where the
//! character is, what it is fighting, what it killed, what its HP is
//! doing — and `watch` is the firehose, every line the board sent.

use crate::events::{Actor, Event};

/// Lines that are worth showing even in the quiet feed, because each one
/// is either the point of the run or a reason it might be going wrong.
const NOTABLE: [&str; 8] = [
    // A kill: the thing the run exists to produce.
    "falls to the ground",
    "you gain",
    // Something went wrong, or the character is in trouble.
    "is dead",
    "you are dead",
    // The board refused an action; the runner may be about to give up on
    // a target.
    "evil warnings",
    "you must",
    // Doors, so a walk that stalls at one is visible.
    "the door",
    "bashed the",
];

pub struct ProgressView {
    watch: bool,
    /// Suppresses the repeats: a prompt lands after nearly every line and
    /// almost always carries the same HP.
    last_hp: Option<i32>,
}

impl ProgressView {
    pub fn new(watch: bool) -> Self {
        ProgressView {
            watch,
            last_hp: None,
        }
    }

    /// One line to show the operator, or `None` when this event is noise.
    pub fn on_event(&mut self, ev: &Event) -> Option<String> {
        match ev {
            Event::RoomSeen(room) => {
                let mut s = format!("-> {}", room.name);
                if !room.also_here.is_empty() {
                    s.push_str(&format!("  [{}]", room.also_here.join(", ")));
                }
                Some(s)
            }
            Event::Prompt { hp, mana } => {
                if self.last_hp == Some(*hp) {
                    return None;
                }
                self.last_hp = Some(*hp);
                Some(match mana {
                    Some(m) => format!("   HP={hp} MA={m}"),
                    None => format!("   HP={hp}"),
                })
            }
            Event::CombatHit {
                attacker,
                target,
                damage,
            } => Some(format!(
                "   {} -> {} ({damage})",
                actor(attacker),
                actor(target)
            )),
            Event::CombatMiss { line } => self.watch.then(|| format!("   {line}")),
            Event::ActorEntered { name, from } => Some(match from {
                Some(d) => format!("   + {name} (from {d})"),
                None => format!("   + {name}"),
            }),
            Event::ActorLeft { name, to } => Some(match to {
                Some(d) => format!("   - {name} ({d})"),
                None => format!("   - {name}"),
            }),
            // Input above this point was DROPPED by the board. Never noise.
            Event::SlowDown => Some("!! flood control: input dropped".to_string()),
            Event::Line(line) => {
                let lower = line.to_lowercase();
                if self.watch || NOTABLE.iter().any(|m| lower.contains(m)) {
                    Some(format!("   {line}"))
                } else {
                    None
                }
            }
        }
    }
}

fn actor(a: &Actor) -> String {
    match a {
        Actor::You => "you".to_string(),
        Actor::Other(name) => name.clone(),
    }
}

/// Experience earned since the run began, and the rate it is coming in.
///
/// Fed from "You gain %s experience." (DLL 0xbc65f) — the same line the
/// bot reads to notice a kill whose death wording it does not know. This
/// is the only honest measure of whether a circuit, or a change to its
/// tuning, is worth anything.
#[derive(Debug, Default, Clone)]
pub struct ExpMeter {
    total: i64,
}

impl ExpMeter {
    /// Note a line; awards are counted, everything else ignored.
    pub fn observe(&mut self, line: &str) {
        let lower = line.to_lowercase();
        let Some(rest) = lower.strip_prefix("you gain ") else {
            return;
        };
        let Some(number) = rest.split_whitespace().next() else {
            return;
        };
        if !rest.contains("experience") {
            return;
        }
        // The board groups thousands once the awards get large.
        if let Ok(n) = number.replace(',', "").parse::<i64>() {
            self.total += n;
        }
    }

    pub fn total(&self) -> i64 {
        self.total
    }

    /// Experience per minute over `elapsed`, or `None` when too little
    /// time has passed for the figure to mean anything — which is a
    /// better answer than a number produced by dividing by nearly zero.
    pub fn per_minute(&self, elapsed: std::time::Duration) -> Option<i64> {
        let secs = elapsed.as_secs_f64();
        if secs < 1.0 {
            return None;
        }
        Some((self.total as f64 * 60.0 / secs).round() as i64)
    }
}
