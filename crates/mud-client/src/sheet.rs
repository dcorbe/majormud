//! What the character is carrying and what it knows how to cast.
//!
//! Both are read from the board's own listings rather than tracked
//! incrementally, because incremental tracking drifts: items are dropped
//! by the board on death, consumed, stolen, and burned out, and none of
//! those announce themselves reliably. Asking is cheap and always right.
//!
//! The immediate use is darkness. `1/2156` Small Cavern has `light =
//! -200`, and a character with no light gets "The room is very dark - you
//! can't see anything" and NO room block at all — so verified navigation
//! cannot confirm an arrival and the bot cannot see what is in the room.
//! Knowing whether a torch or a light spell is available is what turns
//! that from a dead end into a decision.

/// Light-source items are `item.type == 6` in the shipped data. These are
/// the ones that exist: torch (175, 800 uses), lantern (176, 2400), brass
/// lamp (286, 1800), moon-lamp (1153, 4000), scaled lantern (1233, 6000).
///
/// Matched on the trailing noun of the display name, so a rolled or
/// adjectived instance ("a battered torch") still counts — the same rule
/// the bot's targeting uses, and for the same reason.
const LIGHT_ITEMS: [&str; 4] = ["torch", "lantern", "lamp", "moon-lamp"];

/// Spells that light a room. `starlight` (spell 26) is the one the
/// shipped book actually offers; the others are named for when a caster
/// turns up with them.
const LIGHT_SPELLS: [&str; 3] = ["starlight", "light", "continual light"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    /// Carried items as the board lists them, wrapping rejoined.
    pub items: Vec<String>,
    /// `Encumbrance: 119/2880` — carried and capacity.
    pub encumbrance: Option<(i64, i64)>,
}

impl Inventory {
    /// Parse the reply to `inv`.
    pub fn parse(text: &str) -> Inventory {
        let mut inv = Inventory::default();
        let mut carrying: Option<String> = None;
        for line in text.lines() {
            let line = line.trim_end();
            if let Some(rest) = line.trim_start().strip_prefix("You are carrying ") {
                carrying = Some(rest.to_string());
                continue;
            }
            if let Some(rest) = line.trim_start().strip_prefix("Encumbrance: ") {
                inv.encumbrance = rest
                    .split_once(&['/', ' '][..])
                    .and_then(|(a, b)| {
                        let cap = b.split(|c: char| !c.is_ascii_digit()).next()?;
                        Some((a.trim().parse().ok()?, cap.parse().ok()?))
                    });
                continue;
            }
            // The carried list wraps at the terminal width, mid-item, so
            // anything before the next known heading belongs to it.
            if let Some(acc) = carrying.as_mut() {
                let ends = line.trim_start();
                if ends.starts_with("You have")
                    || ends.starts_with("Wealth:")
                    || ends.starts_with("Encumbrance:")
                    || ends.is_empty()
                {
                    carrying = Some(std::mem::take(acc));
                    // Fall through so the heading is still examined.
                    if ends.starts_with("Encumbrance:") {
                        continue;
                    }
                    continue;
                }
                acc.push(' ');
                acc.push_str(ends);
            }
        }
        if let Some(list) = carrying {
            let list = list.trim().trim_end_matches('.');
            if !list.is_empty() && list != "nothing" {
                inv.items = list
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
        inv
    }

    /// EVERY carried thing that would light a dark room, in carry
    /// order. The single-Option shape was exactly why one burn-out went
    /// dead-for-the-run while a second torch sat in the pack.
    fn light_items(&self) -> Vec<String> {
        self.items
            .iter()
            .filter_map(|item| {
                let noun = item.split_whitespace().next_back()?.to_lowercase();
                LIGHT_ITEMS
                    .iter()
                    .find(|l| noun == **l)
                    .map(|l| (*l).to_string())
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownSpell {
    pub level: i16,
    pub mana: i16,
    /// The short name, which is what `cast` takes.
    pub short: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spellbook {
    pub spells: Vec<KnownSpell>,
}

impl Spellbook {
    /// Parse the reply to `spells`.
    ///
    /// Rows are fixed-width (`mud_core::text::spell_row`: level in 3,
    /// mana in 4, four spaces, short in 6, name in 30) but parsed by
    /// whitespace rather than by column, so a differently padded build
    /// still reads. The name may contain spaces, so it is whatever
    /// remains after the first three fields.
    pub fn parse(text: &str) -> Spellbook {
        let mut book = Spellbook::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with("You have the following")
                || line.starts_with("Level")
                || line.starts_with("You have no spells")
            {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(level), Some(mana), Some(short)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let (Ok(level), Ok(mana)) = (level.parse::<i16>(), mana.parse::<i16>()) else {
                continue;
            };
            let name = parts.collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                continue;
            }
            book.spells.push(KnownSpell {
                level,
                mana,
                short: short.to_string(),
                name,
            });
        }
        book
    }

    /// The spell that would light a dark room, with the book's mana
    /// cost — the caster's mana floor comes from here, not from
    /// configuration.
    fn light_spell_with_cost(&self) -> Option<(String, i16)> {
        self.spells
            .iter()
            .find(|s| LIGHT_SPELLS.contains(&s.name.to_lowercase().as_str()))
            .map(|s| (s.short.clone(), s.mana))
    }
}

/// One way the character can light a dark room. The KINDS matter
/// because their failure modes differ completely: a spell fizzles
/// (random cast roll — retry), fades ("Your starlight spell fades
/// away.", live 2026-08-01 — recast) and costs mana; an item is
/// deterministic but burns one use per 3s tick while lit and dies for
/// good at zero ("is no longer lit" — advance to the NEXT source).
/// Collapsing both into one string plan was why a fizzle got one try
/// per visit and a fade poisoned the spell for the whole run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LightSource {
    Spell { cmd: String, mana_cost: i32 },
    Item {
        light_cmd: String,
        remove_cmd: String,
    },
}

impl LightSource {
    /// The command that lights via this source.
    pub fn command(&self) -> &str {
        match self {
            LightSource::Spell { cmd, .. } => cmd,
            LightSource::Item { light_cmd, .. } => light_cmd,
        }
    }
}

/// Every way the character can light a room, in preference order:
/// items first (no mana, keep burning), the spell last (survives any
/// number of burn-outs). Empty is a real answer — an unrecognised
/// command is SAID OUT LOUD by the board, so nothing is invented.
pub fn light_sources(inventory: &Inventory, spellbook: &Spellbook) -> Vec<LightSource> {
    let mut sources: Vec<LightSource> = inventory
        .light_items()
        .into_iter()
        .map(|item| LightSource::Item {
            light_cmd: format!("light {item}"),
            remove_cmd: format!("remove {item}"),
        })
        .collect();
    if let Some((short, mana)) = spellbook.light_spell_with_cost() {
        sources.push(LightSource::Spell {
            cmd: format!("cast {short}"),
            mana_cost: mana as i32,
        });
    }
    sources
}

/// The board's reply on entering a room too dark to see in — "The room is
/// %s - you can't see anything" (DLL 0xdf37e). The descriptor varies, so
/// the tail is what is matched.
pub const TOO_DARK: &str = "you can't see anything";

/// What lighting is worth doing right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LightAttempt {
    /// Send this command.
    Send(String),
    /// An attempt was made this round; the board refuses a second cast
    /// inside one ("You have already cast a spell this round!"), so
    /// wait until this instant and ask again.
    Hold(std::time::Instant),
    /// Nothing can work: lit already, an outcome owed, mana below the
    /// cost, or every source dead.
    Nothing,
}

/// Light the room, CONFIRM it from the board, and only give up when
/// nothing can work. Walking a dark room blind is the LAST resort.
///
/// The board's own wordings are the state transitions, each attributed
/// to OUR light command by the session's correlation — a stale or
/// somebody-else's outcome line proves nothing:
///
/// - "You lit the %s." (DLL 0xdb52d), "You already have something
///   lit!" and the cast success "You cast starlight!" (live 2026-08-01
///   run2 — the old code knew no spell success wording at all, so a
///   spell plan never became lit) mean a source is burning.
/// - "You may not light that item!" kills THAT source and the next one
///   is tried.
/// - A fizzle ("...but fail.", resist, no mana, already-cast) is a
///   random cast roll: retried on the NEXT ROUND while mana covers the
///   book's cost. The old one-attempt-per-visit budget produced the
///   live 15-dark-encounters-3-casts run with MA 11 in hand.
/// - "Your starlight spell fades away." (unsolicited, live run2/run3)
///   is an INDICATOR TO RECAST: lit drops, the source stays healthy.
/// - Burn-out wordings — "%s is no longer lit!", "It's uses gone, %s
///   disappears from your inventory!" (_MEDIUM_UPDATE_CHARACTER burns
///   one use per 3s medium tick while lit; 800 uses = 40 minutes) —
///   arrive UNSOLICITED and kill the current source only when it IS an
///   item: they are item wordings, and a bystander's torch dying must
///   not kill a spell plan. The darkness returning is the backstop for
///   a missed wording ([`LightState::source_died`]), with the same
///   kind-split: a burned-out item is dead, a faded spell is recast.
pub struct LightState {
    /// Every way this character can light a room, preference-ordered
    /// ([`light_sources`]). Empty means it cannot.
    sources: Vec<LightSource>,
    /// Sources the board refused or that burned out: dead for the run.
    dead: Vec<bool>,
    /// The board confirmed a burning source and nothing has gone dark
    /// since.
    lit: bool,
    /// A light command is out; its outcome will carry this id.
    pending: Option<crate::correlate::CmdId>,
    /// Last mana seen on a prompt; None until one arrives. The caster's
    /// floor is the book's cost — no configuration involved.
    mana: Option<i32>,
    /// When the last attempt was released, for round pacing.
    last_attempt: Option<std::time::Instant>,
    /// A fade was seen (or inferred by darkness) while the spell was
    /// believed burning: the operator's directive is that this means
    /// RECAST, proactively, not on the next blind look.
    faded: bool,
}

impl LightState {
    pub fn new(sources: Vec<LightSource>) -> Self {
        let dead = vec![false; sources.len()];
        LightState {
            sources,
            dead,
            lit: false,
            pending: None,
            mana: None,
            last_attempt: None,
            faded: false,
        }
    }

    pub fn lit(&self) -> bool {
        self.lit
    }

    /// The first live source's command, for flows that fire one attempt
    /// themselves (the finish walk, the startup announcement).
    pub fn first_command(&self) -> Option<&str> {
        self.current().map(|s| s.command())
    }

    /// A fade arrived while the light was relied on; a recast is wanted
    /// at the next sensible opportunity, not at the next blind look.
    pub fn wants_recast(&self) -> bool {
        self.faded && !self.lit
    }

    /// A light command is out and its outcome has not arrived.
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    fn current(&self) -> Option<&LightSource> {
        self.sources
            .iter()
            .zip(&self.dead)
            .find(|(_, dead)| !**dead)
            .map(|(s, _)| s)
    }

    fn kill_current(&mut self) {
        if let Some(i) = self.dead.iter().position(|d| !*d) {
            self.dead[i] = true;
        }
    }

    /// A fresh stop visit resets the outcome watch — an outcome that
    /// never arrived (a lost wording, a lag resubscribe) must not wedge
    /// lighting for the rest of the run. The worst case self-corrects
    /// with one "You already have something lit!" round trip. There is
    /// no per-visit attempt budget any more: retries are bounded by
    /// mana and paced by the round.
    pub fn new_visit(&mut self) {
        self.pending = None;
        // Round pacing is a within-visit discipline; getting here took a
        // leg, which is longer than any round.
        self.last_attempt = None;
    }

    /// What lighting is worth doing at `now`.
    pub fn attempt(
        &mut self,
        now: std::time::Instant,
        clock: &crate::world::RoundClock,
    ) -> LightAttempt {
        if self.lit || self.pending.is_some() {
            return LightAttempt::Nothing;
        }
        let Some(source) = self.current() else {
            return LightAttempt::Nothing;
        };
        let cmd = source.command().to_string();
        let mana_floor = match source {
            LightSource::Spell { mana_cost, .. } => Some(*mana_cost),
            LightSource::Item { .. } => None,
        };
        if let Some(cost) = mana_floor
            && self.mana.is_some_and(|m| m < cost)
        {
            // Below the floor the CASTING stops, never the plan: mana
            // regens ~1/round and the lap revisits, which is the retry.
            return LightAttempt::Nothing;
        }
        if let Some(at) = self.last_attempt {
            let next = clock.next_round_after(at);
            if now < next {
                return LightAttempt::Hold(next);
            }
        }
        self.last_attempt = Some(now);
        LightAttempt::Send(cmd)
    }

    /// Called for every command the gate releases, like the other
    /// watchers: only our own source's send arms the outcome watch.
    pub fn on_sent(&mut self, line: &str, id: crate::correlate::CmdId) {
        if Some(line) == self.current().map(|s| s.command()) {
            self.pending = Some(id);
        }
    }

    /// Fold one event. Outcomes answering OUR light command move the
    /// source state; fades and burn-outs arrive unsolicited and are
    /// read by wording; prompts carry the mana the cast floor needs.
    pub fn on_event(&mut self, cor: &crate::correlate::Correlated) {
        if let crate::events::Event::Prompt {
            mana: Some(mana), ..
        } = &cor.event
        {
            self.mana = Some(*mana);
        }
        if let crate::events::Event::Line(line) = &cor.event {
            let l = line.to_lowercase();
            if l.contains("is no longer lit") || l.contains("uses gone") {
                // Item wordings: they kill the current source only when
                // it IS an item — a bystander's burn-out must not kill
                // a spell plan.
                if matches!(self.current(), Some(LightSource::Item { .. })) {
                    self.lit = false;
                    self.kill_current();
                }
                return;
            }
            if l.contains("spell fades away") {
                if self.lit {
                    self.lit = false;
                    self.faded = true;
                }
                return;
            }
        }
        let Some(pending) = self.pending else { return };
        if cor.answers != Some(pending) {
            return;
        }
        let crate::events::Event::Line(line) = &cor.event else {
            return;
        };
        let line = line.to_lowercase();
        if line.contains("you lit the")
            || line.contains("already have something lit")
            || line.starts_with("you cast ")
        {
            self.pending = None;
            self.lit = true;
            self.faded = false;
        } else if line.contains("may not light that item") {
            self.pending = None;
            self.kill_current();
        } else if line.contains("but fail")
            || line.contains("spell is resisted")
            || line.contains("resists your spell")
            || line.contains("enough mana to cast")
            || line.contains("already cast a spell")
        {
            // A fizzle: retry next round, mana permitting.
            self.pending = None;
        }
    }

    /// The `remove` that extinguishes the burning item, when the
    /// current source is an item and the board confirmed it lit. One
    /// use burns every 3s medium tick while lit — dark room or not —
    /// so a run that walks away burning spends the whole burn budget
    /// on idle time.
    pub fn extinguish(&self) -> Option<String> {
        if !self.lit {
            return None;
        }
        match self.current()? {
            LightSource::Item { remove_cmd, .. } => Some(remove_cmd.clone()),
            LightSource::Spell { .. } => None,
        }
    }

    /// The stop went Blind while a source was believed burning — the
    /// backstop for a missed wording; the darkness is also the message.
    /// The kinds diverge: a burned-out item is dead and the next source
    /// is tried; a spell that faded unnoticed is RECAST — killing it
    /// for the run was the live "never recasts again" bug.
    pub fn source_died(&mut self) {
        if !self.lit {
            return;
        }
        self.lit = false;
        match self.current() {
            Some(LightSource::Item { .. }) => self.kill_current(),
            Some(LightSource::Spell { .. }) => self.faded = true,
            None => {}
        }
    }
}
