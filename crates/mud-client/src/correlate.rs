//! Which command does this event answer?
//!
//! The board echoes every command it ACCEPTS, and replies strictly in
//! send order. That is the whole basis of position tracking: the room
//! block that answers `n` is the one that follows `n`'s echo, and a block
//! with no echo is somebody else's render and must never satisfy a
//! pending step (docs/board-correlation.md, measured 94.1% of 6,344
//! corpus blocks; the shortfall is genuinely unsolicited output).
//!
//! Three measured facts shape the rules (stopstate-run5/run7 live
//! captures, transcribed into tests/correlate.rs):
//!
//! - **A busy board echoes twice.** A receipt echo ~2ms after send, and a
//!   second execution echo right before the reply when commands queue
//!   behind the round timer. Post-parse both are `Line(cmd)`, so a
//!   duplicate must re-confirm the entry it echoes, never advance.
//! - **Replies are FIFO.** With four `n` pipelined, echo text identifies
//!   nothing; only order does. Replies attribute to the OLDEST accepted
//!   (echoed) entry, and its reply retires it.
//! - **A wrong association is worse than none.** Everything ambiguous —
//!   a never-arriving echo, a worse-than-observed split — falls to the
//!   deadline, and the consumer re-asks or re-localizes.
//!
//! What retires the head: a room block (`RoomSeen`), or the first
//! unrecognized line after acceptance — single-line replies (door
//! refusals, the dark line, cast failures) are the board's norm, and
//! mid-block description text never reaches the event stream (the parser
//! folds it into the block). Classified async events — combat, actors
//! entering and leaving, prompts — answer nothing and retire nothing.
//! The known cost: a stray broadcast line lands as a false single-line
//! reply and the real answer then reads unsolicited. That is a missed
//! association, never a wrong one, and the deadline path absorbs it.
//!
//! `SlowDown` flushes everything pending: flood control DROPPED input,
//! and whether a dropped command still echoes is unverified.
//!
//! This was nearly built two other ways, both wrong: counting unanswered
//! sends at the session layer (reverted in `7ddfd17` — login lines are
//! answered by menus, the count never drains, "a proxy for correlation is
//! not correlation"), and per-consumer echo watching (dies at phase
//! handoffs, double-counts the execution echo).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::events::Event;

/// Identity of one sent line, allocated by the session in send order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CmdId(pub u64);

/// An event plus the command it answers, if any. `None` means
/// unsolicited: it must never satisfy anyone's pending step, but it still
/// flows — flee detection and re-localization live on unsolicited blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correlated {
    pub event: Event,
    pub answers: Option<CmdId>,
}

/// Strip the board's single leading parenthesized status decoration:
/// `(Resting) look` -> `look` (DLL 0xe06f6; other markers uncaptured, so
/// any single `(word)` token strips).
pub fn strip_decoration(s: &str) -> &str {
    s.strip_prefix('(')
        .and_then(|rest| rest.split_once(')'))
        .map(|(_, after)| after.trim_start())
        .unwrap_or(s)
}

/// THE definition of "this line is the echo of that command".
///
/// After trimming and stripping a status decoration: the exact text, or a
/// proper suffix of it — the split echo, where async output lands
/// mid-echo and `look` reaches the parser as `l` + `ook` (~0.4% live).
/// Floors keep the fragment rule honest: the command must be >= 3 chars
/// and the fragment >= 2, so a stray `n` never reads as the tail of
/// `open n` and one-letter directions only ever match exactly.
pub fn is_echo(line: &str, cmd: &str) -> bool {
    let line = strip_decoration(line.trim());
    if line == cmd {
        return true;
    }
    cmd.len() >= 3 && line.len() >= 2 && line.len() < cmd.len() && cmd.ends_with(line)
}

struct Entry {
    id: CmdId,
    text: String,
    /// The board echoed it: accepted, reply owed. Set by the receipt or
    /// execution echo, whichever arrives first.
    echoed: bool,
    /// Past this instant the entry is forgotten and its late answers read
    /// as unsolicited. Refreshed by each echo occurrence, so a command
    /// the round timer sat on still gets its reply attributed.
    deadline: Instant,
}

/// The one correlator, owned by the session: registry fed in wire order,
/// events attributed in arrival order.
pub struct Correlator {
    queue: VecDeque<Entry>,
    ttl: Duration,
}

impl Correlator {
    pub fn new(ttl: Duration) -> Self {
        Correlator { queue: VecDeque::new(), ttl }
    }

    /// Record a line the writer put on the wire. Must be called in wire
    /// order — echoes arrive in send order and attribution is FIFO.
    pub fn sent(&mut self, id: CmdId, line: &str, now: Instant) {
        self.expire(now);
        self.queue.push_back(Entry {
            id,
            text: line.to_string(),
            echoed: false,
            deadline: now + self.ttl,
        });
    }

    /// Forget everything pending (reconnect, phase reset, flood flush).
    pub fn flush(&mut self) {
        self.queue.clear();
    }

    /// Attribute one parsed event.
    pub fn on_event(&mut self, event: Event, now: Instant) -> Correlated {
        self.expire(now);
        let answers = match &event {
            Event::SlowDown => {
                // Input above this point was DROPPED; whether dropped
                // commands still echo is unverified. Forget everything.
                self.flush();
                None
            }
            Event::Line(l) => self.on_line(l, now),
            Event::RoomSeen(_) => self.retire_head(),
            // Classified async traffic: combat, actors, prompts. The
            // board emits these freely; they answer nothing.
            Event::Prompt { .. }
            | Event::CombatHit { .. }
            | Event::CombatMiss { .. }
            | Event::ActorEntered { .. }
            | Event::ActorLeft { .. } => None,
        };
        Correlated { event, answers }
    }

    /// A line is one of three things, checked in order: the echo of the
    /// oldest not-yet-accepted entry (acceptance — and every earlier
    /// never-echoed entry was eaten, echoes arrive in send order); a
    /// duplicate echo of an already-accepted entry (the execution echo —
    /// confirm, refresh, never advance); or a single-line reply to the
    /// oldest accepted entry, which it retires.
    fn on_line(&mut self, line: &str, now: Instant) -> Option<CmdId> {
        if let Some(pos) = self
            .queue
            .iter()
            .position(|e| !e.echoed && is_echo(line, &e.text))
        {
            let id = {
                let e = &mut self.queue[pos];
                e.echoed = true;
                e.deadline = now + self.ttl;
                e.id
            };
            // Every EARLIER entry that never echoed was eaten — echoes
            // arrive in send order. Earlier echoed entries still owe
            // their replies and stay.
            let mut idx = 0;
            self.queue.retain(|e| {
                let keep = e.echoed || idx >= pos;
                idx += 1;
                keep
            });
            return Some(id);
        }
        if let Some(dup) = self
            .queue
            .iter_mut()
            .find(|e| e.echoed && is_echo(line, &e.text))
        {
            dup.deadline = now + self.ttl;
            return Some(dup.id);
        }
        self.retire_head()
    }

    /// The reply belongs to the oldest accepted entry, and completes it.
    fn retire_head(&mut self) -> Option<CmdId> {
        let pos = self.queue.iter().position(|e| e.echoed)?;
        Some(self.queue.remove(pos).unwrap().id)
    }

    fn expire(&mut self, now: Instant) {
        self.queue.retain(|e| now <= e.deadline);
    }
}
