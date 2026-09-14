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

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{MatchType, Spell, SpellId};

/// The `target` column of the spell table, decoded as
/// [`MatchType`]. Value 1 is a cast on oneself: starlight, camouflage
/// and every other self buff carry it, bless carries 2 for another
/// player, glitterdust 0 for a monster. The variant name is the
/// decoder's placeholder from the first pass over the table, and the
/// field called `target_mode` is a different column. This is the one
/// discovery reads.
const SELF_CAST: MatchType = MatchType::Single1;

/// Does casting this on oneself light the room? Room illumination, on a
/// self cast. The old three-name list found one of the same spells and
/// named two that the shipped table does not carry.
pub fn is_light_spell(spell: &Spell) -> bool {
    spell.match_type == SELF_CAST
        && spell.abilities.iter().any(|(a, _)| *a == Ability::RoomIllu)
}

/// Does casting this on oneself raise stealth? The stealth ability with
/// a value that is not negative, on a self cast. The value reads as zero
/// on every shipped self buff because the amount is level scaled, so
/// presence decides. A negative value is a penalty, which keeps cross of
/// vengeance and the stealth trap out.
pub fn is_stealth_spell(spell: &Spell) -> bool {
    spell.match_type == SELF_CAST
        && spell
            .abilities
            .iter()
            .any(|(a, v)| *a == Ability::Stealth && *v >= 0)
}

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
///
/// `way of the swan` (36) is the mystic's heal: ability 18, no
/// duration, invoked rather than cast. Without it a mystic's book
/// discovers no heal at all.
pub const HEAL_SPELLS: [&str; 10] = [
    "minor healing",
    "way of the swan",
    "mend",
    "healing rain",
    "greater healing",
    "major healing",
    "major healing rain",
    "greater healing rain",
    "godheal",
    "divine healing",
];

/// The heals that target the room. `target` 13 in the spell table:
/// every player in the room is healed, the caster included.
pub const RAIN_SPELLS: [&str; 3] = ["healing rain", "major healing rain", "greater healing rain"];
pub const CURE_POISON: &str = "cure poison";

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

/// A class's caster group (`class.magictype`, `content::Class::caster_group`)
/// by name, matched case-insensitively against the content database's own
/// class table.
///
/// Returns `None` on ANY miss -- an empty or unread class name, a
/// spelling the database does not carry, a customised board -- and a
/// miss MUST be read as "I do not know", never as "no magic": see
/// `2026-08-22-one-path-to-content-design.md`'s "Race and class". The
/// one caller of this ([`crate::farm::probe_sheet`]) falls through to
/// its ordinary spells-then-maybe-redirect probe on `None`; the board's
/// own [`Casting::redirected`] stays authoritative regardless of what
/// this says.
pub fn class_caster_group(content: &mud_core::content::Content, class_name: &str) -> Option<i16> {
    let name = class_name.trim();
    if name.is_empty() {
        return None;
    }
    content
        .classes
        .values()
        .find(|c| c.name.trim().eq_ignore_ascii_case(name))
        .map(|c| c.caster_group)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    /// Carried items as the board lists them, wrapping rejoined.
    pub items: Vec<String>,
    /// The key ring as the board lists it: "You have the following
    /// keys:  black star key." Empty on "You have no keys." Keys live
    /// on a ring of their own, not in the carried list, so the walker
    /// has to read this line to know it holds one.
    pub keys: Vec<String>,
    /// `Encumbrance: 119/2880` — carried and capacity.
    pub encumbrance: Option<(i64, i64)>,
}

/// The ring line's lead-in, live 2026-09-05. The board pads it with two
/// spaces after the colon, which `trim` absorbs.
const KEY_RING_PREFIX: &str = "You have the following keys:";

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
            if let Some(rest) = line.trim_start().strip_prefix(KEY_RING_PREFIX) {
                inv.keys = rest
                    .trim()
                    .trim_end_matches('.')
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
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

    /// The coins on the carried list, by denomination. The bank gate
    /// reads its count and weight off this and the encumbrance pair.
    pub fn coins(&self) -> crate::purse::Coins {
        crate::purse::Coins::from_entries(&self.items)
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

    /// The heals this character casts, and the names it could not use.
    ///
    /// Discovery reads the book: the cheapest heal is the minor and the
    /// dearest the major, when the two differ. A named minor or major
    /// wins over discovery. The regen is only ever named, and needs a
    /// duration from the spell table so it is not recast while running.
    /// A name the book does not know, or a regen with no duration, is
    /// refused with the reason, never skipped quietly.
    pub fn heal_spells(
        &self,
        choice: HealChoice,
        durations: &std::collections::BTreeMap<String, u32>,
        casting: Casting,
    ) -> (Vec<HealSource>, Vec<String>) {
        let mut out = Vec::new();
        let mut refused = Vec::new();
        // The full name or the short one, the same way a buff is named.
        let known = |name: &str| {
            let lower = name.to_lowercase();
            self.spells
                .iter()
                .find(|s| s.name.to_lowercase() == lower || s.short.to_lowercase() == lower)
        };
        let source = |s: &KnownSpell, kind: HealKind| HealSource {
            name: s.name.clone(),
            cmd: casting.command(&s.short),
            mana_cost: s.mana as i32,
            kind,
        };
        let mut heals: Vec<&KnownSpell> = self
            .spells
            .iter()
            .filter(|s| HEAL_SPELLS.contains(&s.name.to_lowercase().as_str()))
            .collect();
        heals.sort_by_key(|s| s.mana);
        let pick = |name: &str, fallback: Option<&KnownSpell>, what: &str| -> Result<Option<KnownSpell>, String> {
            if name.is_empty() {
                return Ok(fallback.cloned());
            }
            match known(name) {
                Some(s) => Ok(Some(s.clone())),
                None => Err(format!("{what} `{name}` is not in the spellbook")),
            }
        };
        let minor = match pick(choice.minor, heals.first().copied(), "minor_heal_spell") {
            Ok(m) => m,
            Err(why) => {
                refused.push(why);
                None
            }
        };
        let dearest = heals.last().copied().filter(|d| Some(d.name.as_str()) != minor.as_ref().map(|m| m.name.as_str()));
        let major = match pick(choice.major, dearest, "major_heal_spell") {
            Ok(m) => m,
            Err(why) => {
                refused.push(why);
                None
            }
        };
        if let Some(m) = &minor {
            out.push(source(m, HealKind::Minor));
        }
        if let Some(m) = &major {
            out.push(source(m, HealKind::Major));
        }
        if !choice.regen.is_empty() {
            match known(choice.regen) {
                Some(s) => match durations.get(&s.name.to_lowercase()) {
                    Some(rounds) if *rounds > 0 => out.push(source(s, HealKind::Regen { rounds: *rounds })),
                    _ => refused.push(format!(
                        "hp_regen_spell `{}` has no duration in the spell table",
                        s.name
                    )),
                },
                None => refused.push(format!("hp_regen_spell `{}` is not in the spellbook", choice.regen)),
            }
        }
        (out, refused)
    }

    /// The spells this character knows that `keep` accepts, matched
    /// against the content's spell map by name, in the book's order.
    /// A known spell the map does not carry is skipped: the board's
    /// listing is the truth about what is castable, the map is the
    /// truth about what a cast does, and discovery needs both.
    pub fn known_matching<'a>(
        &'a self,
        spells: &'a BTreeMap<SpellId, Spell>,
        keep: fn(&Spell) -> bool,
    ) -> Vec<(&'a KnownSpell, &'a Spell)> {
        self.spells
            .iter()
            .filter_map(|known| {
                let lower = known.name.to_lowercase();
                spells
                    .values()
                    .find(|s| s.name.to_lowercase() == lower)
                    .filter(|s| keep(s))
                    .map(|s| (known, s))
            })
            .collect()
    }

    /// The casts this character can make on the party: the self heals
    /// it already has, minus a regen, then the first rain and cure
    /// poison if the book has them.
    pub fn party_heals(&self, heals: &[HealSource], casting: Casting) -> Vec<PartySource> {
        let mut out: Vec<PartySource> = heals
            .iter()
            .filter_map(|h| match h.kind {
                HealKind::Minor | HealKind::Major => Some(PartySource {
                    name: h.name.clone(),
                    cmd: h.cmd.clone(),
                    mana_cost: h.mana_cost,
                    kind: PartyKind::Single(h.kind),
                }),
                HealKind::Regen { .. } => None,
            })
            .collect();
        let named = |name: &str| self.spells.iter().find(|s| s.name.eq_ignore_ascii_case(name));
        if let Some(s) = RAIN_SPELLS.iter().find_map(|n| named(n)) {
            out.push(PartySource { name: s.name.clone(), cmd: casting.command(&s.short), mana_cost: s.mana as i32, kind: PartyKind::Area });
        }
        if let Some(s) = named(CURE_POISON) {
            out.push(PartySource { name: s.name.clone(), cmd: casting.command(&s.short), mana_cost: s.mana as i32, kind: PartyKind::Cure });
        }
        out
    }
}

/// What a heal source is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealKind {
    Minor,
    Major,
    /// A heal over time. `rounds` is its duration from the spell table,
    /// in combat rounds, and it is not recast while running.
    Regen { rounds: u32 },
}

/// The player's choice of heals by book name. Empty means discover, or
/// for the regen, none.
#[derive(Debug, Clone, Copy)]
pub struct HealChoice<'a> {
    pub minor: &'a str,
    pub major: &'a str,
    pub regen: &'a str,
}

/// Which heal a round wants, decided by the runner from the HP percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealNeed {
    Minor,
    /// The regen spell if named and not running, else the minor.
    Regen,
    /// The major heal if the pool affords it, else the minor.
    Major,
}

/// One healing spell this character knows, with what the board says it
/// costs and what it is for. There is no item or potion variant: unlike
/// lighting, every way of healing modelled here fails the same way, so
/// one shape suffices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealSource {
    /// The book's full name, for the operator's benefit and for matching
    /// a `[bot].minor_heal_spell` / `major_heal_spell` / `hp_regen_spell`.
    pub name: String,
    /// `cast maj` / `invoke lay`, built by [`Casting::command`].
    pub cmd: String,
    /// From the book, not from configuration.
    pub mana_cost: i32,
    pub kind: HealKind,
}

/// What a party cast is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyKind {
    /// One member by name, the same minor or major the self heal uses.
    Single(HealKind),
    /// The room.
    Area,
    /// Cure poison, on one member or on the caster.
    Cure,
}

/// One spell this character can cast on the party.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartySource {
    pub name: String,
    /// `cast mahe`, without a target. The target is appended per cast.
    pub cmd: String,
    pub mana_cost: i32,
    pub kind: PartyKind,
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

/// What the board said about a cast of ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The spell is not in the book. Terminal for that source.
    Unknown,
    /// It landed.
    Cast,
    /// A roll, a pool or a round. Comes round again.
    Failed,
}

/// The one outcome grammar, shared by every cast machine. The caller
/// gates by attribution: a monster's "%s attempted to cast %s at you,
/// but failed." is routine din, and this reads only lines the
/// correlator says answer our cast.
pub fn cast_outcome(line: &str) -> Option<Outcome> {
    let line = line.to_lowercase();
    if line.contains("do not know how to cast") || line.contains("do not know how to invoke") {
        return Some(Outcome::Unknown);
    }
    if line.starts_with("you cast ") || line.starts_with("you invoke ") {
        return Some(Outcome::Cast);
    }
    if (line.starts_with("you attempt to") && line.contains("but fail"))
        || line.contains("spell is resisted")
        || line.contains("resists your spell")
        || line.contains("enough mana to cast")
        || line.contains("enough kai to invoke")
        || line.contains("already cast a spell")
        || line.contains("already invoked a power")
        // The target has left the room. UNVERIFIED for a cast target:
        // the wording is `mud_core::text::dont_see_here`, seen for
        // items only. It comes round again, since the member is either
        // back next round or off the roster.
        || line.starts_with("you don't see ")
    {
        return Some(Outcome::Failed);
    }
    None
}

/// Cast a healing spell, and confirm it from the board.
///
/// This is [`LightState`] with a different trigger, and the differences
/// are the interesting part:
///
/// - **It fires in combat.** Resting is suppressed while the room holds a
///   fight, because the board disengages combat to rest and the
///   re-engage breaks it, the 2026-08-01 death spiral. A cast ends the
///   fight too, with a `*Combat Off*` printed ahead of the cast line,
///   but a cast is a moment and a rest is a state: the next room block
///   re-engages the target and nothing is broken by it. So this is the
///   only recovery a character has while something is still hitting it,
///   and that is the whole reason this exists. The runner reads the
///   Combat Off a cast draws through [`HealState::in_flight`], so the
///   bot does not take it for the target leaving.
/// - **Sources are not a preference order but a choice by kind.** Lighting
///   walks its sources in order and kills them as they fail; healing
///   holds at most one source per [`HealKind`] and the caller's
///   [`HealNeed`] picks among them, the pool affording it decides the
///   rest.
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
    /// At most one per [`HealKind`] ([`Spellbook::heal_spells`]).
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
    /// When the regen was last confirmed cast; `None` means never, or its
    /// rounds have already run out.
    regen_cast_at: Option<std::time::Instant>,
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
            regen_cast_at: None,
        }
    }

    /// This character cannot heal itself by casting. Worth saying out
    /// loud at startup rather than discovering at 20% health.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// The heals in play: at most one per [`HealKind`].
    pub fn sources(&self) -> &[HealSource] {
        &self.sources
    }

    /// A cast is out and its outcome has not arrived.
    ///
    /// The runner asks this about a `*Combat Off*`: the board prints
    /// one ahead of any cast made mid-fight, a self-targeted heal
    /// included, and the correlator attributes it to nothing, so the
    /// cast being out is the only sign that it is ours and not the
    /// target leaving.
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// The live source of `kind` the pool affords.
    fn affordable(&self, kind: impl Fn(&HealKind) -> bool) -> Option<usize> {
        let mana = self.mana?;
        self.sources
            .iter()
            .zip(&self.dead)
            .position(|(s, dead)| !dead && kind(&s.kind) && s.mana_cost <= mana)
    }

    /// Is the regen still paying out.
    fn regen_running(&self, now: std::time::Instant, clock: &crate::world::RoundClock) -> bool {
        let Some(at) = self.regen_cast_at else {
            return false;
        };
        let rounds = self
            .sources
            .iter()
            .find_map(|s| match s.kind {
                HealKind::Regen { rounds } => Some(rounds),
                _ => None,
            })
            .unwrap_or(0);
        now.duration_since(at) < clock.period() * rounds
    }

    /// What healing is worth doing at `now` for `need`.
    ///
    /// The caller owns the HP test. This knows about mana, rounds, and
    /// which source answers which need: the major heal falls back to
    /// the minor when the pool cannot afford it, and the regen falls
    /// back to the minor when it is running or not named.
    pub fn attempt(
        &mut self,
        now: std::time::Instant,
        clock: &crate::world::RoundClock,
        need: HealNeed,
    ) -> CastAttempt {
        if self.pending.is_some() {
            return CastAttempt::Nothing;
        }
        let minor = || self.affordable(|k| *k == HealKind::Minor);
        let picked = match need {
            HealNeed::Minor => minor(),
            HealNeed::Major => self.affordable(|k| *k == HealKind::Major).or_else(minor),
            HealNeed::Regen => {
                if self.regen_running(now, clock) {
                    minor()
                } else {
                    self.affordable(|k| matches!(k, HealKind::Regen { .. })).or_else(minor)
                }
            }
        };
        let Some(i) = picked else {
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
    pub fn on_event(&mut self, cor: &crate::correlate::Correlated, now: std::time::Instant) {
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
        match cast_outcome(line) {
            Some(Outcome::Unknown) => {
                self.dead[i] = true;
                self.pending = None;
            }
            Some(Outcome::Cast) => {
                if matches!(self.sources[i].kind, HealKind::Regen { .. }) {
                    self.regen_cast_at = Some(now);
                }
                self.pending = None;
            }
            Some(Outcome::Failed) => self.pending = None,
            None => {}
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

    /// Whether a live source could answer `need` with the pool as last
    /// seen. The ask for a party heal reads this. A character that can
    /// cast for itself does not ask.
    pub fn affords(&self, need: HealNeed) -> bool {
        let minor = self.affordable(|k| *k == HealKind::Minor).is_some();
        match need {
            HealNeed::Minor => minor,
            HealNeed::Major => self.affordable(|k| *k == HealKind::Major).is_some() || minor,
            HealNeed::Regen => self.affordable(|k| matches!(k, HealKind::Regen { .. })).is_some() || minor,
        }
    }

    /// Another cast of ours went out at `now`. The board allows one
    /// cast a round, so this one waits its turn.
    pub fn hold_round(&mut self, now: std::time::Instant) {
        self.last_attempt = Some(now);
    }
}

/// Who a party cast was for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Me,
    One(String),
    /// The rows that were counted when the area cast went out.
    Room(Vec<String>),
}

/// A party cast [`PartyHeal::attempt`] chose, before the gate has
/// released it.
struct Planned {
    source: usize,
    target: Target,
    cmd: String,
    /// When it was chosen, which is the round it goes out in.
    at: std::time::Instant,
}

/// A party cast on the board with no outcome yet.
struct Pending {
    source: usize,
    target: Target,
    id: crate::correlate::CmdId,
    /// When it went out. A cast still unanswered two rounds later is
    /// given up on, so one wording nobody has captured cannot wedge
    /// every party cast for the rest of the run.
    at: std::time::Instant,
}

/// Cast heals on the party, and confirm them from the board.
///
/// The judgments live here: which row is due, which spell answers it,
/// and who was healed by a cast. The facts, the rows themselves, live
/// in [`crate::party::Health`] and are handed in on every attempt.
///
/// A row is due when its number was seen after this character last
/// healed it. A member still hurt says `@heal` again each round, and
/// every member is polled, so a healed row comes due again on its own
/// when it is still low.
pub struct PartyHeal {
    sources: Vec<PartySource>,
    dead: Vec<bool>,
    mana: Option<i32>,
    /// A cast chosen by `attempt` and not yet released by `on_sent`.
    planned: Option<Planned>,
    /// A cast out and unanswered.
    pending: Option<Pending>,
    last_attempt: Option<std::time::Instant>,
    healed: std::collections::BTreeMap<String, std::time::Instant>,
    cured: std::collections::BTreeMap<String, std::time::Instant>,
    /// When this character last saw its own poison tick.
    self_poisoned: Option<std::time::Instant>,
}

impl PartyHeal {
    pub fn new(sources: Vec<PartySource>) -> Self {
        let dead = vec![false; sources.len()];
        PartyHeal {
            sources,
            dead,
            mana: None,
            planned: None,
            pending: None,
            last_attempt: None,
            healed: Default::default(),
            cured: Default::default(),
            self_poisoned: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn has_cure(&self) -> bool {
        self.sources.iter().zip(&self.dead).any(|(s, d)| !d && s.kind == PartyKind::Cure)
    }

    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    pub fn hold_round(&mut self, now: std::time::Instant) {
        self.last_attempt = Some(now);
    }

    /// `You feel ill.` was printed: this character is poisoned.
    pub fn poison_self(&mut self, now: std::time::Instant) {
        self.self_poisoned = Some(now);
    }

    fn affordable(&self, kind: impl Fn(PartyKind) -> bool) -> Option<usize> {
        let mana = self.mana?;
        self.sources.iter().zip(&self.dead).position(|(s, d)| !d && kind(s.kind) && s.mana_cost <= mana)
    }

    fn key(name: &str) -> String {
        name.to_ascii_lowercase()
    }

    fn plan(&mut self, i: usize, target: Target, now: std::time::Instant) -> CastAttempt {
        let cmd = match &target {
            Target::One(name) => format!("{} {}", self.sources[i].cmd, Self::key(name)),
            Target::Me | Target::Room(_) => self.sources[i].cmd.clone(),
        };
        self.last_attempt = Some(now);
        self.planned = Some(Planned { source: i, target, cmd: cmd.clone(), at: now });
        CastAttempt::Send(cmd)
    }

    /// Nothing may be cast at `now`: a cast of this state is still
    /// out, there is nothing to cast at all, or this round is already
    /// spent.
    ///
    /// A cast unanswered two rounds later is given up on first. The
    /// board's answers are read only through the correlator, so a
    /// reply the correlator does not recognise leaves `pending` set
    /// and every future party cast would return nothing. Giving up
    /// costs one wasted round trip; not giving up costs the healer.
    fn blocked(&mut self, now: std::time::Instant, clock: &crate::world::RoundClock) -> Option<CastAttempt> {
        if let Some(p) = &self.pending
            && now.duration_since(p.at) >= clock.period() * 2
        {
            self.pending = None;
        }
        if self.pending.is_some() || self.sources.is_empty() {
            return Some(CastAttempt::Nothing);
        }
        if let Some(at) = self.last_attempt {
            let next = clock.next_round_after(at);
            if now < next {
                return Some(CastAttempt::Hold(next));
            }
        }
        None
    }

    /// The one cast worth making at `now`, if any. Cure before heal,
    /// self before others, rain when two or more are under the mark,
    /// else the lowest by the marks.
    pub fn attempt(
        &mut self,
        now: std::time::Instant,
        clock: &crate::world::RoundClock,
        health: &crate::party::Health,
        own_percent: Option<i32>,
        cfg: &crate::bot::BotConfig,
    ) -> CastAttempt {
        if let Some(held) = self.blocked(now, clock) {
            return held;
        }
        if self.self_poisoned.is_some()
            && let Some(i) = self.affordable(|k| k == PartyKind::Cure)
        {
            return self.plan(i, Target::Me, now);
        }
        // The first poisoned row by name. One poisoned member at a
        // time is the common case, and the next round takes the next
        // one, so there is nothing to rank them by.
        if let Some(i) = self.affordable(|k| k == PartyKind::Cure)
            && let Some(row) = health.rows().find(|v| {
                !v.resists_magic()
                    && v.poisoned.is_some_and(|p| self.cured.get(&Self::key(&v.name)).is_none_or(|c| *c < p))
            })
        {
            return self.plan(i, Target::One(row.name.clone()), now);
        }
        let mark = cfg.minor_heal_at_percent as i32;
        if mark == 0 {
            return CastAttempt::Nothing;
        }
        let mut due: Vec<&crate::party::Vitals> = health
            .rows()
            .filter(|v| {
                !v.resists_magic()
                    && v.hp.is_some_and(|h| (h as i32) < mark)
                    && self.healed.get(&Self::key(&v.name)).is_none_or(|h| *h < v.seen)
            })
            .collect();
        due.sort_by_key(|v| v.hp);
        let count = due.len() + usize::from(own_percent.is_some_and(|p| p < mark));
        if count >= 2
            && let Some(i) = self.affordable(|k| k == PartyKind::Area)
        {
            let names = due.iter().map(|v| v.name.clone()).collect();
            return self.plan(i, Target::Room(names), now);
        }
        let Some(lowest) = due.first() else {
            return CastAttempt::Nothing;
        };
        let hp = lowest.hp.unwrap_or(0) as i32;
        let minor = self.affordable(|k| k == PartyKind::Single(HealKind::Minor));
        let picked = match crate::bot::heal_need(cfg, hp) {
            Some(HealNeed::Major) => self.affordable(|k| k == PartyKind::Single(HealKind::Major)).or(minor),
            Some(HealNeed::Minor | HealNeed::Regen) => minor,
            None => None,
        };
        let Some(i) = picked else {
            return CastAttempt::Nothing;
        };
        let name = lowest.name.clone();
        self.plan(i, Target::One(name), now)
    }

    /// Called for every command the gate releases.
    pub fn on_sent(&mut self, line: &str, id: crate::correlate::CmdId) {
        if let Some(p) = self.planned.take()
            && p.cmd == line
        {
            self.pending = Some(Pending { source: p.source, target: p.target, id, at: p.at });
        }
    }

    pub fn on_event(&mut self, cor: &crate::correlate::Correlated, now: std::time::Instant) {
        if let crate::events::Event::Prompt { mana: Some(mana), .. } = &cor.event {
            self.mana = Some(*mana);
        }
        let Some(pending) = &self.pending else {
            return;
        };
        if cor.answers != Some(pending.id) {
            return;
        }
        let crate::events::Event::Line(line) = &cor.event else {
            return;
        };
        let i = pending.source;
        match cast_outcome(line) {
            Some(Outcome::Unknown) => {
                self.dead[i] = true;
                self.pending = None;
            }
            Some(Outcome::Cast) => {
                let kind = self.sources[i].kind;
                let done = self.pending.take().expect("pending checked above");
                let marks = if kind == PartyKind::Cure { &mut self.cured } else { &mut self.healed };
                match done.target {
                    Target::Me => self.self_poisoned = None,
                    Target::One(name) => {
                        marks.insert(Self::key(&name), now);
                    }
                    Target::Room(names) => {
                        for name in names {
                            marks.insert(Self::key(&name), now);
                        }
                    }
                }
            }
            Some(Outcome::Failed) => self.pending = None,
            None => {}
        }
    }

    /// A fresh visit clears an outcome that never arrived, the same
    /// bargain [`HealState::new_visit`] makes.
    pub fn new_visit(&mut self) {
        self.pending = None;
        self.planned = None;
    }
}

/// One spell the character keeps up on itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buff {
    pub name: String,
    pub cmd: String,
    pub mana_cost: i32,
    /// From the shipped `spell` table's `duration`
    /// ([`crate::views::spell_durations`]), in combat
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

    /// A cast has gone out and the board has not said how it went.
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// Mana from somewhere other than a prompt this state saw: the
    /// session's last prompt, for a state built after it passed.
    ///
    /// Unconditional, because the session's reading is never the older
    /// one: it is fed by the same prompt stream this state folds, and a
    /// caller seeds from it at the moment it is about to decide. A seed
    /// that only filled a blank left a state carrying whatever mana the
    /// first prompt it happened to see said, for as long as no prompt
    /// reached it directly.
    pub fn seed_mana(&mut self, mana: i32) {
        self.mana = Some(mana);
    }

    /// The board answered a cast with a wording this does not know, or
    /// the caller stopped waiting for it. Counted as a failure, the way
    /// a fizzle is: the budget stays lapsed, so the spell is tried again
    /// at the next opportunity, and nothing is in flight any more.
    pub fn give_up(&mut self) {
        self.pending = None;
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

/// Build a buff for a known spell from its shipped duration, if it has
/// one worth keeping. The one lookup both `buffs` and `stealth_spells`
/// need: find the duration by name, and refuse a zero, because a budget
/// of 0 rounds is always expired. `None` covers both a missing entry and
/// a zero one, so the caller words its own refusal either way.
fn buff_for(known: &KnownSpell, durations: &BTreeMap<String, u32>, casting: Casting) -> Option<Buff> {
    let &rounds = durations.get(&known.name.to_lowercase())?;
    (rounds > 0).then(|| Buff {
        name: known.name.clone(),
        cmd: casting.command(&known.short),
        mana_cost: known.mana as i32,
        rounds,
    })
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
        // The full name or the short one: `shld` is what the operator
        // types at the board, and what the book's own listing shows.
        let Some(known) = spellbook
            .spells
            .iter()
            .find(|s| s.name.to_lowercase() == lower || s.short.to_lowercase() == lower)
        else {
            refused.push(format!("`{name}` is not in this character's book"));
            continue;
        };
        match buff_for(known, durations, casting) {
            Some(buff) => out.push(buff),
            None => refused.push(format!(
                "`{name}` has no duration, so it is not something to keep up"
            )),
        }
    }
    (out, refused)
}

/// The stealth spells this character knows, as buffs to keep up on a
/// sneak. Discovery, not configuration: the book says what is castable
/// and the spell map says what raises stealth. A spell the map carries
/// and the book does not is simply absent. One the book carries with no
/// duration in the table is refused out loud, the same way a buff is,
/// because a budget of zero rounds is always lapsed.
pub fn stealth_spells(
    spellbook: &Spellbook,
    spells: &BTreeMap<SpellId, Spell>,
    durations: &BTreeMap<String, u32>,
    casting: Casting,
) -> (Vec<Buff>, Vec<String>) {
    let mut out = Vec::new();
    let mut refused = Vec::new();
    for (known, _) in spellbook.known_matching(spells, is_stealth_spell) {
        match buff_for(known, durations, casting) {
            Some(buff) => out.push(buff),
            None => refused.push(format!(
                "`{}` raises stealth but has no duration in the spell table",
                known.name
            )),
        }
    }
    (out, refused)
}

/// What a job says about stealth at startup, beside its heal and light
/// lines. One line per spell found, one per refusal, and "none known"
/// for a book with no stealth spell in it. A character with no book at
/// all gets nothing, because nothing was looked for.
pub fn stealth_lines(spellbook: &Spellbook, found: &[Buff], refused: &[String]) -> Vec<String> {
    if spellbook.spells.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = refused.iter().map(|r| format!("stealth: {r}")).collect();
    for b in found {
        lines.push(format!(
            "stealth: {} ({} mana, {} rounds)",
            b.name, b.mana_cost, b.rounds
        ));
    }
    if lines.is_empty() {
        lines.push("stealth: none known".to_string());
    }
    lines
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
///
/// A book may hold more than one spell that lights a room, and the
/// cheapest of them is the one taken: cross of vengeance lights a room
/// too, at 15 mana against starlight's 4, and it is a combat spell that
/// happens to glow. Book order would have picked whichever the board
/// happened to list first. Ties keep book order.
pub fn light_sources(
    inventory: &Inventory,
    spellbook: &Spellbook,
    spells: &BTreeMap<SpellId, Spell>,
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
    if let Some((known, _)) = spellbook
        .known_matching(spells, is_light_spell)
        .into_iter()
        .min_by_key(|(known, _)| known.mana)
    {
        sources.push(LightSource::Spell {
            cmd: casting.command(&known.short),
            mana_cost: known.mana as i32,
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

