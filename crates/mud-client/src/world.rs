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
