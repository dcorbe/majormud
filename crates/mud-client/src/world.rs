//! What the client believes about the world it is standing in.
//!
//! Until this module existed, every fact lived as a transient trigger:
//! room contents were one `StopState` block with a shelf life, arrivals
//! and departures were invalidation pulses and then discarded, and the
//! board's round cadence was nowhere at all. Every farm bug of
//! 2026-07-31/08-01 — the invisible spawn, the corpse latch, the rest
//! spiral, the one-cast dark room — traced back to deciding off
//! wording-triggers because there was no maintained state to consult.
//!
//! Deliberately absent: population tracking for rooms the character is
//! NOT standing in. Nothing consumes it, respawns are player-driven so
//! it decays instantly, and speculative state rots.

use std::time::{Duration, Instant};

/// The board's combat/regen round, measured live: whiff bursts arrive
/// 5.12–5.13s apart (run2/run3 captures, 2026-07-31). Everything the
/// board does to us is quantised to it — swings, casts, regen — so
/// retry pacing that ignores it either bursts into flood control or
/// waits arbitrary made-up delays.
pub const ROUND: Duration = Duration::from_millis(5130);

/// Phase-locked round tracker. Combat lines arrive in bursts on round
/// boundaries; observing them locks the phase, and consumers ask "when
/// does the next round start" instead of sleeping guesses.
///
/// First (and only) consumer: lighting retry pacing — one cast per
/// round, because the board refuses a second cast inside one anyway
/// ("You have already cast a spell this round!").
pub struct RoundClock {
    period: Duration,
    /// The start of the most recently observed burst.
    last_burst: Option<Instant>,
}

impl RoundClock {
    pub fn new() -> Self {
        RoundClock::with_period(ROUND)
    }

    pub fn with_period(period: Duration) -> Self {
        RoundClock {
            period,
            last_burst: None,
        }
    }

    /// Feed combat evidence (a hit or whiff line's arrival time).
    /// Lines inside one burst land milliseconds apart and must not
    /// slide the phase; anything later than half a period is a new
    /// burst and re-locks it — lag drift is corrected by the board's
    /// own next volley rather than modelled.
    pub fn observe(&mut self, now: Instant) {
        match self.last_burst {
            Some(at) if now.duration_since(at) < self.period / 2 => {}
            _ => self.last_burst = Some(now),
        }
    }

    pub fn period(&self) -> Duration {
        self.period
    }

    /// The earliest instant strictly after `t` that begins a new round.
    /// Unlocked (no combat observed yet), the honest answer is one full
    /// period from `t`: pacing without phase knowledge.
    pub fn next_round_after(&self, t: Instant) -> Instant {
        let Some(phase) = self.last_burst else {
            return t + self.period;
        };
        let mut next = phase;
        while next <= t {
            next += self.period;
        }
        next
    }
}

impl Default for RoundClock {
    fn default() -> Self {
        RoundClock::new()
    }
}

/// A value with when it was learned, for staleness bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped<T> {
    pub value: T,
    pub at: Instant,
}

/// The "Also here:" case rule — the only player/monster discriminator
/// there is: players and named NPCs capitalise, wandering monsters are
/// lowercase generic nouns, and the TRAILING noun decides (a lowercase
/// adjective on a capitalised name — "thin Templar", slice-8 seedy
/// corpus — is a named NPC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccupantKind {
    Player,
    Monster,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occupant {
    pub name: String,
    pub kind: OccupantKind,
    /// When this name was first seen here — preserved across reseeding
    /// blocks, so "how long has that been standing there" is answerable.
    pub since: Instant,
}

fn kind_of(name: &str) -> OccupantKind {
    let name = crate::correlate::strip_decoration(name);
    let first_lower = name.chars().next().is_some_and(char::is_lowercase);
    let noun_lower = name
        .split_whitespace()
        .last()
        .and_then(|w| w.chars().next())
        .is_some_and(char::is_lowercase);
    if first_lower && noun_lower {
        OccupantKind::Monster
    } else {
        OccupantKind::Player
    }
}

/// Who is standing in the room we are standing in — maintained
/// continuously, not re-derived per stop. The fold discipline is the
/// one [`crate::farm::StopState`] proved out over four live failures:
/// attributed blocks are believed; unsolicited renders (somebody
/// else's look, a stale answer) are ignored; async truths (arrivals,
/// departures, deaths) pass through regardless of attribution.
///
/// Deliberately NOT a replacement for StopState's `seen` — that is "a
/// block answering OUR look naming THIS stop, within shelf life", the
/// verdict's evidence with invariants four live failures paid for.
/// `Here` believes any attributed block, whoever asked, and earns
/// trust as a consumer before any collapse is considered.
#[derive(Debug, Default)]
pub struct Here {
    /// Localized identity, written by the owner (the pump knows the
    /// stop id; a flee clears it). Here never guesses ids from names.
    pub room: Option<mud_core::content::RoomId>,
    /// The last attributed block, with when it arrived.
    pub view: Option<Stamped<crate::events::RoomView>>,
    /// The living occupant list: seeded by each attributed block, then
    /// mutated by the events the pump used to throw away.
    pub occupants: Vec<Occupant>,
    /// The board said "too dark" here more recently than any block.
    pub blind: bool,
}

impl Here {
    /// Fold one correlated event.
    pub fn on_event(&mut self, cor: &crate::correlate::Correlated, now: Instant) {
        use crate::events::Event;
        match &cor.event {
            Event::RoomSeen(room) => {
                if cor.answers.is_none() {
                    return;
                }
                self.blind = false;
                let old = std::mem::take(&mut self.occupants);
                self.occupants = room
                    .also_here
                    .iter()
                    .map(|name| {
                        let clean = crate::correlate::strip_decoration(name);
                        let since = old
                            .iter()
                            .find(|o| o.name == clean)
                            .map(|o| o.since)
                            .unwrap_or(now);
                        Occupant {
                            name: clean.to_string(),
                            kind: kind_of(name),
                            since,
                        }
                    })
                    .collect();
                self.view = Some(Stamped {
                    value: room.clone(),
                    at: now,
                });
            }
            Event::ActorEntered { name, .. } => {
                let clean = crate::correlate::strip_decoration(name).to_string();
                if !self.occupants.iter().any(|o| o.name == clean) {
                    self.occupants.push(Occupant {
                        kind: kind_of(&clean),
                        name: clean,
                        since: now,
                    });
                }
            }
            Event::ActorLeft { name, .. } => {
                let clean = crate::correlate::strip_decoration(name);
                self.occupants.retain(|o| o.name != clean);
            }
            Event::Line(line) => {
                if crate::bot::is_kill_line(line) {
                    // Death lines name the TEMPLATE; a rolled adjective
                    // still matches on the trailing noun, the same word
                    // the attack command uses. The award-only form
                    // names nobody and removes nobody; the view goes
                    // stale either way — something died out of it.
                    self.occupants.retain(|o| {
                        !o.name
                            .split_whitespace()
                            .last()
                            .is_some_and(|noun| line.contains(noun))
                    });
                    self.view = None;
                } else if crate::bot::is_combat_off(line) || line.starts_with("You say \"") {
                    // The fight's end (or a swing that fell through to
                    // SAY) stales the render — our belief about the
                    // FIGHT changed, not the room, so the occupants
                    // stand.
                    self.view = None;
                } else if line.contains(crate::sheet::TOO_DARK) && cor.answers.is_some() {
                    self.blind = true;
                }
            }
            _ => {}
        }
    }

    /// Everything observed is dropped — a lagged broadcast, a flee.
    pub fn reset(&mut self) {
        self.room = None;
        self.view = None;
        self.occupants.clear();
        self.blind = false;
    }
}
