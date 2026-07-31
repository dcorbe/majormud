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

    /// The first carried thing that would light a dark room, named as the
    /// board would want it referred to.
    pub fn light_source(&self) -> Option<String> {
        self.items.iter().find_map(|item| {
            let noun = item.split_whitespace().next_back()?.to_lowercase();
            LIGHT_ITEMS
                .iter()
                .find(|l| noun == **l)
                .map(|l| (*l).to_string())
        })
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

    /// The short name of a spell that would light a dark room.
    pub fn light_spell(&self) -> Option<String> {
        self.spells
            .iter()
            .find(|s| LIGHT_SPELLS.contains(&s.name.to_lowercase().as_str()))
            .map(|s| s.short.clone())
    }
}

/// The command that would light the current room, or `None` when the
/// character has no way to.
///
/// A carried light wins over a spell: it costs no mana — which is wanted
/// for whatever the dark room is hiding — and once lit it keeps burning,
/// where a spell has a duration. Both verbs are the board's own:
/// `light <item>` (DLL 0xdb07a, "You lit the %s." at 0xdb52d) and
/// `cast <short>` ("Syntax: CAST {spell} [{target}]", 0xd984f).
///
/// `None` is a real answer and must not be papered over with a guess: an
/// unrecognised command is SAID OUT LOUD by the board, so inventing one
/// would broadcast it to the room and leave the character still blind.
pub fn light_plan(inventory: &Inventory, spellbook: &Spellbook) -> Option<String> {
    if let Some(item) = inventory.light_source() {
        return Some(format!("light {item}"));
    }
    spellbook.light_spell().map(|short| format!("cast {short}"))
}

/// The board's reply on entering a room too dark to see in — "The room is
/// %s - you can't see anything" (DLL 0xdf37e). The descriptor varies, so
/// the tail is what is matched.
pub const TOO_DARK: &str = "you can't see anything";

/// Light the room, CONFIRM it from the board, and only give up when
/// nothing can work. Walking a dark room blind is the LAST resort.
///
/// The board's own wordings are the state transitions, each attributed
/// to OUR light command by the session's correlation — a stale or
/// somebody-else's outcome line proves nothing:
///
/// - "You lit the %s." (DLL 0xdb52d) and "You already have something
///   lit!" mean a source is burning.
/// - "You may not light that item!" means the plan is wrong outright and
///   is never retried.
/// - A failed cast ("...but fail.", resist, no mana) spends this VISIT's
///   attempt: mana does not come back inside a stop visit (measured live
///   — extra attempts bought nothing and cost four commands each), and
///   the stop is revisited every lap, which is the retry.
/// - Burn-out has NO wording. The darkness is the message: the stop
///   going Blind again while a source was believed lit means it died,
///   and a dead source is not retried ([`LightState::source_died`]).
pub struct LightState {
    /// The command that lights, from [`light_plan`]; None means nothing
    /// on the character can light a room.
    plan: Option<String>,
    /// The board confirmed a burning source and nothing has gone dark
    /// since.
    lit: bool,
    /// A light command is out; its outcome will carry this id.
    pending: Option<crate::correlate::CmdId>,
    /// Attempts spent at the current stop visit.
    spent: u32,
    /// Plans the board refused or that burned out: dead for the run.
    exhausted: Vec<String>,
}

impl LightState {
    pub fn new(plan: Option<String>) -> Self {
        LightState {
            plan,
            lit: false,
            pending: None,
            spent: 0,
            exhausted: Vec::new(),
        }
    }

    pub fn lit(&self) -> bool {
        self.lit
    }

    /// The current plan, for flows that fire one attempt themselves
    /// (the finish walk).
    pub fn plan(&self) -> Option<&String> {
        self.plan.as_ref()
    }

    /// A fresh stop visit: the per-visit attempt budget resets, and so
    /// does the outcome watch — an outcome that never arrived (a lost
    /// wording, a lag resubscribe) must not wedge lighting for the rest
    /// of the run. The worst case self-corrects with one "You already
    /// have something lit!" round trip.
    pub fn new_visit(&mut self) {
        self.spent = 0;
        self.pending = None;
    }

    /// The command worth sending now, if any attempt can work: none
    /// while an outcome is owed, a source is already burning, the visit
    /// budget is spent, or the plan is exhausted.
    pub fn attempt(&mut self) -> Option<String> {
        if self.lit || self.pending.is_some() || self.spent >= 1 {
            return None;
        }
        let plan = self.plan.as_ref()?;
        if self.exhausted.contains(plan) {
            return None;
        }
        Some(plan.clone())
    }

    /// Called for every command the gate releases, like the other
    /// watchers: only our own plan's send arms the outcome watch.
    pub fn on_sent(&mut self, line: &str, id: crate::correlate::CmdId) {
        if Some(line) == self.plan.as_deref() {
            self.pending = Some(id);
            self.spent += 1;
        }
    }

    /// Fold one attributed event; only the outcome answering OUR light
    /// command moves the state.
    pub fn on_event(&mut self, cor: &crate::correlate::Correlated) {
        let Some(pending) = self.pending else { return };
        if cor.answers != Some(pending) {
            return;
        }
        let crate::events::Event::Line(line) = &cor.event else {
            return;
        };
        let line = line.to_lowercase();
        if line.contains("you lit the") || line.contains("already have something lit") {
            self.pending = None;
            self.lit = true;
        } else if line.contains("may not light that item") {
            self.pending = None;
            if let Some(plan) = &self.plan {
                self.exhausted.push(plan.clone());
            }
        } else if line.contains("but fail")
            || line.contains("spell is resisted")
            || line.contains("resists your spell")
            || line.contains("enough mana to cast")
            || line.contains("already cast a spell")
        {
            // Recoverable: the visit's attempt is spent, the next visit
            // may try again.
            self.pending = None;
        }
    }

    /// The stop went Blind while a source was believed burning: it
    /// burned out. There is no wording for this — the darkness is the
    /// message — and a dead source is not retried.
    pub fn source_died(&mut self) {
        if self.lit {
            self.lit = false;
            if let Some(plan) = self.plan.take() {
                self.exhausted.push(plan);
            }
        }
    }
}
