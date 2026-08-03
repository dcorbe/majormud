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

/// Spells that restore health, by exact name from the shipped `spell`
/// table. Exact, not substring: `blessed vision` and `rapid healing` both
/// contain a word this list uses and neither belongs here.
///
/// Order is irrelevant — the caster's own mana costs decide which one is
/// picked (see [`Spellbook::heal_spells`]).
///
/// Two near-misses are excluded on purpose:
/// - `rapid healing` (138/831) is a **regen buff**: duration 60, and its
///   `min`/`max` of 200 is an ability value, not a number of hit points.
///   It belongs in `[bot].buffs`.
/// - `dead heal` (1252, level 999) and `divine healing` (1059, level 50)
///   are not reachable by a playable character; `divine healing` is
///   listed anyway because nothing breaks if it ever is.
///
/// The `... rain` family targets the room rather than one character
/// (`target` 13), which still heals the caster — a group heal cast solo
/// is just an expensive self-heal, and the mana cost says so.
pub const HEAL_SPELLS: [&str; 9] = [
    "minor healing",
    "mend",
    "healing rain",
    "greater healing",
    "major healing",
    "major healing rain",
    "greater healing rain",
    "godheal",
    "divine healing",
];

/// Whether this character casts spells or invokes powers.
///
/// Mystics (caster group 5) are a wholesale vocabulary swap, not a
/// dialect quirk: `powers` for `spells`, `invoke` for `cast`, kai for
/// mana. The board refuses the wrong one outright
/// (`mud_core::text::KAI_NO_CAST`), so this is worked out from its own
/// redirect at startup rather than configured or guessed from the class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Casting {
    #[default]
    Spells,
    Powers,
}

impl Casting {
    /// The command that lists what this character knows.
    pub fn list_command(self) -> &'static str {
        match self {
            Casting::Spells => "spells",
            Casting::Powers => "powers",
        }
    }

    /// The command that uses `short`, which is the abbreviation the
    /// listing prints and the only form the board reliably takes.
    pub fn command(self, short: &str) -> String {
        match self {
            Casting::Spells => format!("cast {short}"),
            Casting::Powers => format!("invoke {short}"),
        }
    }

    /// Did the board just say we asked the wrong way round? Both
    /// redirects are VERIFIED (`mud_core::text` §8.12), and either one
    /// names the vocabulary that should have been used.
    pub fn redirected(reply: &str) -> Option<Casting> {
        let l = reply.to_lowercase();
        if l.contains("you must list your powers") || l.contains("you must invoke your powers") {
            return Some(Casting::Powers);
        }
        if l.contains("you must list your spells") || l.contains("you must cast your spells") {
            return Some(Casting::Spells);
        }
        None
    }
}

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
            // "You have the following powers:" and its "Level Kai  Short
            // Spell Name" header are already covered by the two prefixes
            // above them; only the empty-book wording differs.
            if line.is_empty()
                || line.starts_with("You have the following")
                || line.starts_with("Level")
                || line.starts_with("You have no spells")
                || line.starts_with("You have no powers")
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

    /// Every way this character can heal itself, **cheapest first**.
    ///
    /// `only` overrides the discovery: a non-empty list names the spells
    /// to use and nothing else is considered, so an operator can stop the
    /// bot reaching for the expensive one. Names are matched
    /// case-insensitively against the book's full spell name, and an
    /// entry the character does not know is silently absent rather than
    /// an error — the book is the authority on what it knows.
    ///
    /// **Cheapest, not biggest.** Sizing the spell to the wound was the
    /// obvious alternative and it cannot be done honestly: the shipped
    /// `min`/`max` are the level-1 figures and the real heal scales with
    /// caster level, so any table here would understate by more the
    /// longer the character had been played. Mana is the scarce resource,
    /// an under-heal is retried next round for free, and the book's own
    /// costs are real data about this character. So: cheapest affordable.
    pub fn heal_spells(&self, only: &[String], casting: Casting) -> Vec<HealSource> {
        let wanted = |name: &str| {
            let lower = name.to_lowercase();
            if only.is_empty() {
                HEAL_SPELLS.contains(&lower.as_str())
            } else {
                only.iter().any(|o| o.to_lowercase() == lower)
            }
        };
        let mut heals: Vec<HealSource> = self
            .spells
            .iter()
            .filter(|s| wanted(&s.name))
            .map(|s| HealSource {
                name: s.name.clone(),
                cmd: casting.command(&s.short),
                mana_cost: s.mana as i32,
            })
            .collect();
        heals.sort_by_key(|h| h.mana_cost);
        heals
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

/// One healing spell this character knows, with what the board says it
/// costs. There is no item or potion variant: unlike lighting, every way
/// of healing modelled here fails the same way, so one shape suffices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealSource {
    /// The book's full name, for the operator's benefit and for matching
    /// a `[bot].heal_spells` entry.
    pub name: String,
    /// `cast maj` / `invoke lay`, built by [`Casting::command`].
    pub cmd: String,
    /// From the book, not from configuration.
    pub mana_cost: i32,
}

/// What one of the spell machines wants to do right now.
///
/// Shared by all three — lighting, healing, buffs — because the answer
/// has the same shape whatever the spell is, and `Hold` in particular has
/// the same single cause: the board refuses a second cast inside one
/// round (*"You have already cast a spell this round!"*), and it does not
/// care which spell the first one was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastAttempt {
    Send(String),
    /// A cast already went out this round; ask again at this instant.
    Hold(std::time::Instant),
    /// Nothing can work — the reasons differ per machine, and none of
    /// them is an error.
    Nothing,
}

/// Cast a healing spell, and confirm it from the board.
///
/// This is [`LightState`] with a different trigger, and the differences
/// are the interesting part:
///
/// - **It fires in combat.** Resting is suppressed while the room holds a
///   fight, because the board disengages combat to rest and the
///   re-engage breaks it — the 2026-08-01 death spiral. Casting
///   disengages nothing, so it is the only recovery a character has while
///   something is still hitting it. That is the whole reason this exists.
/// - **Sources are not a preference order but a price list.** Lighting
///   walks its sources in order and kills them as they fail; healing
///   picks the cheapest one the current mana affords, every time.
/// - **Failure is nearly always temporary.** A fizzle, an empty pool, a
///   second cast in one round: all retried. The single terminal outcome
///   is *"You do not know how to cast %s."*, which means the spell is not
///   in the book and never will be — that one source is retired.
///
/// Below the mana floor this returns [`CastAttempt::Nothing`] rather than
/// anything louder, and the rest mark takes over: resting restores mana
/// as well as health, so the two marks compose without either knowing
/// about the other.
pub struct HealState {
    /// Cheapest first ([`Spellbook::heal_spells`]).
    sources: Vec<HealSource>,
    /// Sources the board says are not in the book: dead for the run.
    dead: Vec<bool>,
    /// Last mana (or kai) seen on a prompt; `None` until one arrives, and
    /// `None` means "unknown", which is treated as affording nothing —
    /// a character whose pool has never been seen is not a caster.
    mana: Option<i32>,
    /// A cast is out and its outcome has not arrived: which source, and
    /// the id its answer will carry.
    pending: Option<(usize, crate::correlate::CmdId)>,
    /// When the last cast was released, for round pacing.
    last_attempt: Option<std::time::Instant>,
}

impl HealState {
    pub fn new(sources: Vec<HealSource>) -> Self {
        let dead = vec![false; sources.len()];
        HealState {
            sources,
            dead,
            mana: None,
            pending: None,
            last_attempt: None,
        }
    }

    /// This character cannot heal itself by casting. Worth saying out
    /// loud at startup rather than discovering at 20% health.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// The spells found, cheapest first — for the startup announcement.
    pub fn sources(&self) -> &[HealSource] {
        &self.sources
    }

    /// A cast is out and its outcome has not arrived.
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// The cheapest live source the pool currently affords.
    fn affordable(&self) -> Option<usize> {
        let mana = self.mana?;
        self.sources
            .iter()
            .zip(&self.dead)
            .position(|(s, dead)| !dead && s.mana_cost <= mana)
    }

    /// What healing is worth doing at `now`.
    ///
    /// The caller owns the HP test — this knows about mana and rounds,
    /// not about marks.
    pub fn attempt(
        &mut self,
        now: std::time::Instant,
        clock: &crate::world::RoundClock,
    ) -> CastAttempt {
        if self.pending.is_some() {
            return CastAttempt::Nothing;
        }
        let Some(i) = self.affordable() else {
            return CastAttempt::Nothing;
        };
        if let Some(at) = self.last_attempt {
            let next = clock.next_round_after(at);
            if now < next {
                return CastAttempt::Hold(next);
            }
        }
        self.last_attempt = Some(now);
        CastAttempt::Send(self.sources[i].cmd.clone())
    }

    /// Called for every command the gate releases. Any of our own casts
    /// arms the outcome watch — unlike lighting, which only ever has one
    /// candidate in play, the source chosen here varies with the pool.
    pub fn on_sent(&mut self, line: &str, id: crate::correlate::CmdId) {
        if let Some(i) = self.sources.iter().position(|s| s.cmd == line) {
            self.pending = Some((i, id));
        }
    }

    /// Fold one event. Prompts carry the pool; everything else is only
    /// read when the correlator says it answers OUR cast — a monster's
    /// "%s attempted to cast %s at you, but failed." is routine din and
    /// would otherwise read as our own fizzle.
    pub fn on_event(&mut self, cor: &crate::correlate::Correlated) {
        if let crate::events::Event::Prompt {
            mana: Some(mana), ..
        } = &cor.event
        {
            self.mana = Some(*mana);
        }
        let Some((i, pending)) = self.pending else {
            return;
        };
        if cor.answers != Some(pending) {
            return;
        }
        let crate::events::Event::Line(line) = &cor.event else {
            return;
        };
        let line = line.to_lowercase();
        // The one terminal outcome: the spell is not in the book. Every
        // other failure is a roll, a pool or a round, and comes round
        // again.
        if line.contains("do not know how to cast") || line.contains("do not know how to invoke") {
            self.dead[i] = true;
            self.pending = None;
        } else if line.starts_with("you cast ")
            || line.starts_with("you invoke ")
            || line.contains("but fail")
            || line.contains("spell is resisted")
            || line.contains("resists your spell")
            || line.contains("enough mana to cast")
            || line.contains("enough kai to invoke")
            || line.contains("already cast a spell")
            || line.contains("already invoked a power")
        {
            self.pending = None;
        }
    }

    /// A fresh visit clears an outcome that never arrived, so a lost
    /// wording cannot wedge healing for the rest of the run — the same
    /// bargain [`LightState::new_visit`] makes, and the same worst case:
    /// one wasted round trip.
    pub fn new_visit(&mut self) {
        self.pending = None;
        self.last_attempt = None;
    }
}

/// One spell the character keeps up on itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buff {
    pub name: String,
    pub cmd: String,
    pub mana_cost: i32,
    /// From the shipped `spell` table's `duration`
    /// ([`crate::graph::RoomGraph::load_spell_durations`]), in combat
    /// rounds. A floor: the real duration scales with caster level, so
    /// recasting on this is early and never late.
    pub rounds: u32,
}

/// Keep buffs up.
///
/// A buff is not a heal and this is not [`HealState`] with a different
/// list. `bless` restores no health at all — it is +3 for 40 rounds — so
/// there is no HP mark to fire it at. What decides is time: cast it, note
/// when, recast when the budget runs out.
///
/// **Why a timer and not the wear-off line.** The wording family is real
/// and pinned (`The effects of %s wear off.`, live as *"The effects of
/// blur wear off."* / *"The effects of shockshield wear off!"*), but the
/// `%s` is the spell's own free-text `DescMsg` — the same trap as the
/// per-monster movement messages, where matching a name against
/// author-written prose gets it wrong in both directions. So the wording
/// is used as an EARLY TRIGGER only: any wear-off expires every budget,
/// and the timer is the thing that is actually correct. Being early
/// costs one extra cast; being late costs the fight.
///
/// Buffs are cast in a quiet room, never mid-fight: a buff bought during
/// the fight it was meant to help is mana spent too late to matter.
pub struct BuffState {
    buffs: Vec<Buff>,
    /// When each was last confirmed cast; `None` = never, or lapsed.
    cast_at: Vec<Option<std::time::Instant>>,
    mana: Option<i32>,
    pending: Option<(usize, crate::correlate::CmdId)>,
    last_attempt: Option<std::time::Instant>,
}

impl BuffState {
    pub fn new(buffs: Vec<Buff>) -> Self {
        let cast_at = vec![None; buffs.len()];
        BuffState {
            buffs,
            cast_at,
            mana: None,
            pending: None,
            last_attempt: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.buffs.is_empty()
    }

    pub fn buffs(&self) -> &[Buff] {
        &self.buffs
    }

    /// The first buff that is lapsed (or never cast) and affordable.
    fn wanted(&self, now: std::time::Instant, clock: &crate::world::RoundClock) -> Option<usize> {
        let mana = self.mana?;
        self.buffs.iter().enumerate().position(|(i, b)| {
            if b.mana_cost > mana {
                return false;
            }
            match self.cast_at[i] {
                None => true,
                Some(at) => now.duration_since(at) >= clock.period() * b.rounds,
            }
        })
    }

    /// What upkeep is worth doing at `now`. The caller owns the "is the
    /// room quiet" question; this owns mana, the round, and the budget.
    pub fn attempt(
        &mut self,
        now: std::time::Instant,
        clock: &crate::world::RoundClock,
    ) -> CastAttempt {
        if self.pending.is_some() {
            return CastAttempt::Nothing;
        }
        let Some(i) = self.wanted(now, clock) else {
            return CastAttempt::Nothing;
        };
        if let Some(at) = self.last_attempt {
            let next = clock.next_round_after(at);
            if now < next {
                return CastAttempt::Hold(next);
            }
        }
        self.last_attempt = Some(now);
        CastAttempt::Send(self.buffs[i].cmd.clone())
    }

    pub fn on_sent(&mut self, line: &str, id: crate::correlate::CmdId) {
        if let Some(i) = self.buffs.iter().position(|b| b.cmd == line) {
            self.pending = Some((i, id));
        }
    }

    /// Fold one event. The budget is only started by a cast the board
    /// CONFIRMED — a fizzle leaves the buff lapsed, which is the truth,
    /// and it is retried next round.
    pub fn on_event(&mut self, cor: &crate::correlate::Correlated, now: std::time::Instant) {
        if let crate::events::Event::Prompt {
            mana: Some(mana), ..
        } = &cor.event
        {
            self.mana = Some(*mana);
        }
        if let crate::events::Event::Line(line) = &cor.event {
            let l = line.to_lowercase();
            // Unsolicited and unattributable: something of ours ran out.
            // Which one it was cannot be read off the wording, so every
            // budget expires and the next quiet moment re-establishes
            // whatever is actually missing. One redundant cast is the
            // worst case; a buff silently down is not.
            if l.contains("the effects of") && l.contains("wear off") {
                self.cast_at.iter_mut().for_each(|at| *at = None);
            }
        }
        let Some((i, pending)) = self.pending else {
            return;
        };
        if cor.answers != Some(pending) {
            return;
        }
        let crate::events::Event::Line(line) = &cor.event else {
            return;
        };
        let line = line.to_lowercase();
        if line.starts_with("you cast ") || line.starts_with("you invoke ") {
            self.cast_at[i] = Some(now);
            self.pending = None;
        } else if line.contains("do not know how to cast")
            || line.contains("do not know how to invoke")
        {
            // Not in the book: never ask again. The budget stays "never
            // cast", so `wanted` would keep picking it — remove it.
            self.buffs.remove(i);
            self.cast_at.remove(i);
            self.pending = None;
        } else if line.contains("but fail")
            || line.contains("enough mana to cast")
            || line.contains("enough kai to invoke")
            || line.contains("already cast a spell")
            || line.contains("already invoked a power")
        {
            self.pending = None;
        }
    }

    pub fn new_visit(&mut self) {
        self.pending = None;
        self.last_attempt = None;
    }
}

/// Pick the buffs `wanted` names out of the spellbook, with the board's
/// own mana cost and the shipped duration.
///
/// A name the character does not know is absent rather than an error, and
/// a spell with no duration is refused outright with a word about it: a
/// heal named here would be recast forever, since a budget of 0 rounds is
/// always expired.
pub fn buffs(
    spellbook: &Spellbook,
    wanted: &[String],
    durations: &std::collections::BTreeMap<String, u32>,
    casting: Casting,
) -> (Vec<Buff>, Vec<String>) {
    let mut out = Vec::new();
    let mut refused = Vec::new();
    for name in wanted {
        let lower = name.to_lowercase();
        let Some(known) = spellbook
            .spells
            .iter()
            .find(|s| s.name.to_lowercase() == lower)
        else {
            refused.push(format!("`{name}` is not in this character's book"));
            continue;
        };
        match durations.get(&lower) {
            Some(&rounds) if rounds > 0 => out.push(Buff {
                name: known.name.clone(),
                cmd: casting.command(&known.short),
                mana_cost: known.mana as i32,
                rounds,
            }),
            _ => refused.push(format!(
                "`{name}` has no duration, so it is not something to keep up"
            )),
        }
    }
    (out, refused)
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
pub fn light_sources(
    inventory: &Inventory,
    spellbook: &Spellbook,
    casting: Casting,
) -> Vec<LightSource> {
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
            cmd: casting.command(&short),
            mana_cost: mana as i32,
        });
    }
    sources
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
    ) -> CastAttempt {
        if self.lit || self.pending.is_some() {
            return CastAttempt::Nothing;
        }
        let Some(source) = self.current() else {
            return CastAttempt::Nothing;
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
            return CastAttempt::Nothing;
        }
        if let Some(at) = self.last_attempt {
            let next = clock.next_round_after(at);
            if now < next {
                return CastAttempt::Hold(next);
            }
        }
        self.last_attempt = Some(now);
        CastAttempt::Send(cmd)
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
