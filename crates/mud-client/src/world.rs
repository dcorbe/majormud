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

use std::collections::VecDeque;
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

/// One way the maintained model and the board's own render disagreed.
///
/// The two occupant directions are NOT equally meaningful, and reading
/// them as one number is how a model gets trusted for the wrong reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergenceKind {
    /// The model claims an occupant the block does not list. **The
    /// defect signal.** No respawn can produce it: it means a death or a
    /// departure went unparsed, which is what leaves a bot swinging at a
    /// ghost or a stop stuck `Busy`. (One benign source exists — the
    /// mid-render race, where a block's content predates an arrival
    /// emitted before it — so a small nonzero count is not automatically
    /// a bug, but a growing one is.)
    OccupantExtra,
    /// The block lists an occupant the model does not. Ambiguous by
    /// construction: a silent respawn produces exactly this signal, and
    /// so does a movemsg wording the parser cannot read. Only the trend
    /// means anything.
    OccupantMissing,
    /// The model claims coins the block does not list — another player
    /// swept them, or a pickup we did not send was credited to us.
    PileExtra,
    /// The block lists coins the model never heard drop: an unparsed
    /// drop wording.
    PileMissing,
}

impl DivergenceKind {
    /// Every kind, in reading order: the defect signal first, then the
    /// ambiguous one, then the same pair for the floor.
    pub const ALL: [DivergenceKind; 4] = [
        DivergenceKind::OccupantExtra,
        DivergenceKind::OccupantMissing,
        DivergenceKind::PileExtra,
        DivergenceKind::PileMissing,
    ];

    fn slot(self) -> usize {
        match self {
            DivergenceKind::OccupantExtra => 0,
            DivergenceKind::OccupantMissing => 1,
            DivergenceKind::PileExtra => 2,
            DivergenceKind::PileMissing => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DivergenceKind::OccupantExtra => "occupant-extra",
            DivergenceKind::OccupantMissing => "occupant-missing",
            DivergenceKind::PileExtra => "pile-extra",
            DivergenceKind::PileMissing => "pile-missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub kind: DivergenceKind,
    /// The occupant or denomination the two disagreed about.
    pub name: String,
    pub at: Instant,
}

/// How often the model and the board disagreed, and the last few times.
///
/// Counts are the measurement and are never cleared by [`Here::reset`] —
/// a flee drops beliefs, not evidence. The ring is bounded because an
/// hour-long run must not accumulate a record per block.
#[derive(Debug, Default)]
pub struct Reconcile {
    counts: [u32; 4],
    recent: VecDeque<Divergence>,
}

/// Enough to read the last few by hand without holding a run's worth.
const RECENT_CAP: usize = 64;

impl Reconcile {
    pub fn count(&self, kind: DivergenceKind) -> u32 {
        self.counts[kind.slot()]
    }

    pub fn recent(&self) -> &VecDeque<Divergence> {
        &self.recent
    }

    /// Every disagreement so far, all kinds together. Unbounded, unlike
    /// the ring: it is the measurement.
    pub fn total(&self) -> u32 {
        self.counts.iter().sum()
    }

    /// Every kind that fired at least once, with its count — for the
    /// run summary, so a clean run prints nothing at all.
    pub fn tally(&self) -> Vec<(DivergenceKind, u32)> {
        DivergenceKind::ALL
            .into_iter()
            .map(|k| (k, self.count(k)))
            .filter(|(_, n)| *n > 0)
            .collect()
    }

    fn record(&mut self, kind: DivergenceKind, name: &str, at: Instant) {
        self.counts[kind.slot()] += 1;
        if self.recent.len() == RECENT_CAP {
            self.recent.pop_front();
        }
        self.recent.push_back(Divergence {
            kind,
            name: name.to_string(),
            at,
        });
    }
}

/// A pile of coins lying in the room we are standing in.
///
/// Items are deliberately absent: on a shared board a listed item is
/// somebody's dropped gear, and the sweep policy has always been coins
/// only (`bot::COIN_PILE_RE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pile {
    /// Denomination as `get` takes it ("silver").
    pub denom: String,
    /// Count as last rendered or dropped. Reconciliation compares it;
    /// nothing decides on it.
    pub count: u32,
    /// When this denomination was first seen on this floor, preserved
    /// across reseeding blocks like [`Occupant::since`].
    pub since: Instant,
    /// `get` attempts spent on it. Bounded by the caller: a pile the
    /// character cannot carry (encumbrance refusal) stays listed in
    /// every block forever, so an unbounded work item would never drain.
    pub tries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occupant {
    pub name: String,
    pub kind: OccupantKind,
    /// When this name was first seen here — preserved across reseeding
    /// blocks, so "how long has that been standing there" is answerable.
    pub since: Instant,
}

/// Split a board line into the words a monster name could be matched
/// against: lowercased, stripped of the sentence's punctuation, hyphens
/// kept (`wererat plague-crafter`).
fn words(line: &str) -> Vec<&str> {
    line.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Which occupant did this death line take?
///
/// **Exactly one, or none.** A pack renders as separately rolled
/// instances of one template — "angry kobold thief", "thin kobold
/// thief" — and every instance shares the template's trailing noun.
/// Removing every occupant that matched therefore buried the whole pack
/// on its first casualty: `occupant-missing` went 6 -> 35 across the
/// shared Arena stretch the moment the death lexicon made this fire on
/// anyone's kill, against 10 -> 5 on the overclaims it was fixing.
///
/// Two namings, in order of how much they guess:
///
/// 1. The lexicon knows the wording and answers with the TEMPLATE
///    ("kobold thief"). Nothing is inferred from the prose at all.
/// 2. [`crate::bot::is_kill_line`]'s phrase half ("falls to the
///    ground") carries the name inside the sentence, so a word of the
///    line that IS an occupant's targeting noun names the victim. Its
///    award half ("You gain 412 experience.") names nobody, and this
///    finds nobody — a model that picked an occupant off a line with no
///    name in it would be inventing the answer.
///
/// Matching is by whole word, never by substring: "The wererat squeals
/// in agony, and dies!" carries "rat" and used to take the giant rat
/// with it.
fn victim(occupants: &[Occupant], line: &str) -> Option<usize> {
    let noun = match crate::deaths::killed(line) {
        Some(template) => crate::bot::target_word(template).to_lowercase(),
        None => {
            let said = words(line);
            occupants
                .iter()
                .map(|o| crate::bot::target_word(&o.name).to_lowercase())
                .find(|noun| said.iter().any(|w| w.eq_ignore_ascii_case(noun)))?
        }
    };
    // Longest-standing first: instances are interchangeable to every
    // consumer, so the tie needs a rule only to keep the fold
    // deterministic.
    occupants
        .iter()
        .enumerate()
        .filter(|(_, o)| crate::bot::target_word(&o.name).eq_ignore_ascii_case(&noun))
        .min_by_key(|(_, o)| o.since)
        .map(|(i, _)| i)
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
    /// The coins on this floor: seeded by each attributed block, raised
    /// by drop lines, cleared by the board's own pickup acknowledgement.
    pub piles: Vec<Pile>,
    /// How often the model disagreed with the board that seeded it.
    pub reconcile: Reconcile,
    /// The room name the model is currently seeded from, or `None` when
    /// it holds no beliefs worth comparing against.
    ///
    /// Two jobs, and it must be a field of its own for both. It marks
    /// the first block at a room as a SEED — `Here` is built fresh per
    /// stop (`farm.rs`), so reconciling that one would report every
    /// occupant of every room the run visits. And it is what tells a
    /// step apart from a re-render.
    ///
    /// Deliberately NOT derived from `view`: the kill and combat-off
    /// arms null that on purpose, and a fight is exactly what precedes
    /// walking out of a room — which made the model blame the room
    /// behind the character for the contents of the one ahead (live,
    /// cwgaming 2026-08-01).
    seeded: Option<String>,
}

impl Here {
    /// Diff what we believe against what the board just rendered.
    ///
    /// Records only; it never corrects the model — the reseed that
    /// follows does that. Keeping the two apart is the point: a
    /// reconciler that silently repaired would report a clean run while
    /// hiding the very gaps it exists to find.
    fn compare(
        rec: &mut Reconcile,
        occupants: &[Occupant],
        piles: &[Pile],
        room: &crate::events::RoomView,
        now: Instant,
    ) {
        let listed: Vec<&str> = room
            .also_here
            .iter()
            .map(|n| crate::correlate::strip_decoration(n))
            .collect();
        for o in occupants {
            if !listed.contains(&o.name.as_str()) {
                rec.record(DivergenceKind::OccupantExtra, &o.name, now);
            }
        }
        for name in &listed {
            if !occupants.iter().any(|o| o.name == *name) {
                rec.record(DivergenceKind::OccupantMissing, name, now);
            }
        }
        let dropped: Vec<String> = room
            .items
            .iter()
            .filter_map(|e| crate::bot::coin_pile(e))
            .map(|(_, denom)| denom)
            .collect();
        for p in piles {
            if !dropped.contains(&p.denom) {
                rec.record(DivergenceKind::PileExtra, &p.denom, now);
            }
        }
        for denom in &dropped {
            if !piles.iter().any(|p| &p.denom == denom) {
                rec.record(DivergenceKind::PileMissing, denom, now);
            }
        }
    }

    /// Fold one correlated event.
    pub fn on_event(&mut self, cor: &crate::correlate::Correlated, now: Instant) {
        use crate::events::Event;
        match &cor.event {
            Event::RoomSeen(room) => {
                if cor.answers.is_none() {
                    return;
                }
                // A block naming somewhere else describes somewhere
                // else. The farm rebuilds `Here` per stop, but the
                // assist follows an operator who walks where they like,
                // and without this every step would report the room
                // behind them as an overclaim. Beliefs about the old
                // room are dropped rather than compared.
                let moved = self.seeded.as_deref().is_some_and(|n| n != room.name);
                if moved {
                    self.occupants.clear();
                    self.piles.clear();
                    self.seeded = None;
                }
                // Reconcile BEFORE the reseed below overwrites the
                // model with the block: this is the only instant the
                // two beliefs coexist. Disjoint field borrows, so the
                // comparison reads the model while the tally is written.
                if self.seeded.is_some() {
                    Self::compare(&mut self.reconcile, &self.occupants, &self.piles, room, now);
                }
                self.seeded = Some(room.name.clone());
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
                // The floor reseeds on the same discipline as the
                // occupants: the block is the truth, but `since` and the
                // attempts already spent are OURS and survive it. A pile
                // the character cannot carry is listed by every block,
                // and a reseed that reset `tries` would let the try cap
                // recede forever.
                let old_piles = std::mem::take(&mut self.piles);
                self.piles = room
                    .items
                    .iter()
                    .filter_map(|entry| crate::bot::coin_pile(entry))
                    .map(|(count, denom)| {
                        let prior = old_piles.iter().find(|p| p.denom == denom);
                        Pile {
                            count,
                            since: prior.map_or(now, |p| p.since),
                            tries: prior.map_or(0, |p| p.tries),
                            denom,
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
                // Loot lines are settled first and alone. A kill drops
                // coins BEFORE any block re-renders, so waiting for the
                // next `look` to learn about them is exactly the
                // round-trip this model exists to remove — and the kill
                // arm below nulls `view`, which must never be read as
                // the floor being swept.
                if let Some((count, denom)) = crate::bot::coin_drop(line) {
                    match self.piles.iter_mut().find(|p| p.denom == denom) {
                        // A second kill onto the same floor: the board
                        // renders one merged pile, so the counts add.
                        Some(pile) => pile.count += count,
                        None => self.piles.push(Pile {
                            denom,
                            count,
                            since: now,
                            tries: 0,
                        }),
                    }
                    return;
                }
                // The board's own acknowledgement — the only positive
                // proof a pile left the floor. Without it a successful
                // sweep and an encumbrance refusal look identical, and
                // the floor could only ever be re-derived from the next
                // block.
                if let Some((_, denom)) = crate::bot::picked_up(line) {
                    self.piles.retain(|p| p.denom != denom);
                    return;
                }
                // Somebody else's sweep names no denomination, so the
                // whole floor goes. Conservative in the only direction
                // that is cheap: what survived is relisted by the next
                // block, at worst one `recheck` later, whereas KEEPING a
                // pile nobody can take spends the `get` budget on
                // coins already in another player's pack.
                if crate::bot::swept_by_other(line) {
                    self.piles.clear();
                    return;
                }
                // Two different questions, deliberately kept apart.
                // `is_kill_line` asks "did OUR fight end" and leans on
                // the experience award, which only fires for a kill we
                // landed; the lexicon asks "did ANY monster die here"
                // and reads the board's own per-monster wording. The
                // model needs the second — a corpse another player made
                // is still a corpse — while `Bot` must keep using the
                // first, or somebody else's kill would unlatch it from
                // a fight that is still going.
                if crate::bot::is_kill_line(line) || crate::deaths::killed(line).is_some() {
                    // ONE death, one corpse. The view goes stale either
                    // way — something died out of it — but which
                    // occupant left is a question with a single answer,
                    // and `victim` is where it is asked.
                    if let Some(i) = victim(&self.occupants, line) {
                        self.occupants.remove(i);
                    }
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

    /// Who the model believes is standing here, for
    /// [`crate::bot::Bot::has_target_among`]. More current than any
    /// block by construction: the kills, arrivals and departures since
    /// the last render are already folded in.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.occupants.iter().map(|o| o.name.as_str())
    }

    /// Does the model hold beliefs worth deciding on?
    ///
    /// False before the first block at a stop and after a
    /// [`Here::reset`] — and an empty occupant list means those two
    /// cases and "the room is empty" would otherwise be the same
    /// answer. They are opposite answers: one is no evidence, the other
    /// is evidence of nothing.
    pub fn seeded(&self) -> bool {
        self.seeded.is_some()
    }

    /// Record that a `get` went out for this denomination. Called by
    /// whoever sends it, so the try cap counts ATTEMPTS rather than
    /// blocks — an encumbrance refusal produces neither an
    /// acknowledgement nor a change in the render, and counting either
    /// of those would never terminate.
    pub fn note_get_attempt(&mut self, denom: &str) {
        if let Some(pile) = self.piles.iter_mut().find(|p| p.denom == denom) {
            pile.tries += 1;
        }
    }

    /// The owner resolved which room the character is standing in.
    ///
    /// This outranks the printed name, and it has to: identity by name
    /// cannot tell twins apart (Newhaven 1/2146 and 1/2151 share one,
    /// and the sewers run hundreds of blocks under "Sewer Tunnel"), so a
    /// step between them reads as a re-render and every occupant of the
    /// room behind us reports as an overclaim. Callers that cannot
    /// resolve an id simply never call this and keep the name guard.
    ///
    /// Noting the SAME room again is a no-op — owners call it per block.
    pub fn note_room(&mut self, id: mud_core::content::RoomId) {
        if self.room != Some(id) {
            self.reset();
            self.room = Some(id);
        }
    }

    /// Everything observed is dropped — a lagged broadcast, a flee.
    pub fn reset(&mut self) {
        self.room = None;
        self.view = None;
        self.occupants.clear();
        self.piles.clear();
        self.blind = false;
        // Beliefs go; the measurement stays. Clearing the tally here
        // would discard exactly the evidence a bad run produces, and the
        // next block seeds rather than indicts an emptied model.
        self.seeded = None;
    }
}
