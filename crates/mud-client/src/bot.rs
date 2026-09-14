//! Bot policy engine — the MegaMud toggle set (AutoCombat, AutoHeal,
//! AutoGet, AutoFlee) as a pure decision core: events in, commands out.
//! The async runner glues it to a [`crate::session::Session`].

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::events::{Event, Status};

// Coins print "5 copper drop to the ground." — and "1 silver drops to
// the ground." when there is only one, because the verb agrees with the
// coins. The LEADING COUNT is what separates loot from a downed actor's
// "Vexil drops to the ground!"; the verb never was, and pinning it to
// the plural silently dropped every single-coin pile (7 in one Arena
// session, cwrun2.raw — the whole `pile-missing` count).
static COIN_DROP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+) (\w+) drops? to the ground\.$").unwrap());

/// Did a kill just drop coins, and how many of what?
///
/// Deliberately NOT whitelisted to the five minted denominations, unlike
/// [`coin_pile`]: the leading count already carries the exclusion the
/// wording needs, and narrowing it here would change what the bot sweeps.
pub fn coin_drop(line: &str) -> Option<(u32, String)> {
    let c = COIN_DROP_RE.captures(line)?;
    Some((c[1].parse().ok()?, c[2].to_string()))
}

/// Is this "You notice ..." entry a coin pile, and how many of what?
pub fn coin_pile(entry: &str) -> Option<(u32, String)> {
    let c = COIN_PILE_RE.captures(entry)?;
    Some((c[1].parse().ok()?, c[2].to_string()))
}

/// The board's acknowledgement of a `get`: "You picked up 11 silver
/// nobles" (VERIFIED, oracle_bank.raw — no trailing period, unlike the
/// drop line).
static PICKED_UP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^You picked up (\d+) (copper|silver|gold|platinum|runic) (?:farthing|noble|crown|piece|coin)s?$",
    )
    .unwrap()
});

/// Did the board just confirm a pile left the floor, and which one?
///
/// `get` is otherwise fire-and-forget: nothing has ever told the client
/// whether a sweep worked, so an encumbrance refusal and a successful
/// pickup looked identical. Returns the count taken and the denomination
/// as `get` takes it ("silver").
///
/// The denomination whitelist is load-bearing, not decoration: the board
/// announces taking an ITEM the same way ("You picked up a silver holy
/// amulet"), and reading that as a coin pile would retire a pile still
/// sitting on the floor.
///
/// Public and predicate-shaped for the same reason as [`is_kill_line`]:
/// [`crate::world::Here`] asks the same question of the same
/// `Event::Line`, and the alternative — a new `Event` variant — would
/// silently break `correlate::completes(Kind::Get, ..)`, which matches
/// "you picked up" on `Event::Line` to retire the `get` that earned it.
pub fn picked_up(line: &str) -> Option<(u32, String)> {
    let c = PICKED_UP_RE.captures(line)?;
    Some((c[1].parse().ok()?, c[2].to_string()))
}

/// The stock wording is "You took <item>.", the DLL's `%s` string
/// (`took_item` in `mud-core::text`, ported from WCCMMUD.DLL, verified
/// against `crates/mud-core/tests/inventory.rs`). "You picked up
/// <item>" (no trailing period), captured live in
/// oracle_charm_lifecycle, is kept for the reimplemented board this
/// client actually plays, whose item wording has never been captured.
/// Coins are [`picked_up`]'s and are refused here, and the leading
/// article is dropped.
static PICKED_UP_ITEM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^You picked up (?:an? |the )?(.+?)\.?$").unwrap());

/// The DLL's stock wording: "You took <item>.".
static TOOK_ITEM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^You took (.+)\.$").unwrap());

/// Did the board just confirm an item, not coins, left the floor and
/// entered the pack? Returns the item's name as printed.
///
/// The DLL also prints "You took %d damage." and "You took %d damage!"
/// for a landed blow, sharing the "You took " prefix with a pickup: a
/// name ending in "damage" is refused so a hit is never read as one.
pub fn picked_up_item(line: &str) -> Option<String> {
    if picked_up(line).is_some() {
        return None;
    }
    let caps = TOOK_ITEM_RE
        .captures(line)
        .or_else(|| PICKED_UP_ITEM_RE.captures(line))?;
    let name = caps[1].to_string();
    if name.ends_with("damage") {
        return None;
    }
    Some(name)
}

/// "You don't see any silver nobles": the pile was gone when our `get`
/// landed. Somebody else was faster, on a shared floor. Returns the
/// denomination as `get` takes it.
static PILE_GONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^You don't see any (copper|silver|gold|platinum|runic) (?:farthing|noble|crown|piece|coin)s?$",
    )
    .unwrap()
});

/// Did the board just answer a coin `get` with "not here", and for
/// which denomination?
pub fn pile_gone(line: &str) -> Option<String> {
    PILE_GONE_RE.captures(line).map(|c| c[1].to_string())
}

/// Somebody ELSE swept the floor: "Mystic picked up some coins."
/// (VERIFIED, cwrun2.raw — 35 of them in one shared Arena session).
///
/// The board tells us neither how much nor of what, which is the whole
/// character of the fact: it says the floor changed and refuses to say
/// how. Named separately from [`picked_up`] because the two return
/// different things — one a pile, the other only the news.
static SWEPT_BY_OTHER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\S+ picked up some coins\.$").unwrap());

/// Did another player just take coins off this floor?
pub fn swept_by_other(line: &str) -> bool {
    SWEPT_BY_OTHER_RE.is_match(line)
}

/// A coin pile as the room's "You notice ... here." line names one:
/// "11 silver nobles", "2968 copper farthings". The leading count is the
/// discriminator — an ITEM can wear a denomination word ("silver holy
/// amulet", live in oracle_charm_lifecycle) but never a count. Items are
/// deliberately not swept: on somebody else's board that is somebody
/// else's dropped gear.
static COIN_PILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d+) (copper|silver|gold|platinum|runic) (?:farthing|noble|crown|piece|coin)s?$")
        .unwrap()
});

/// Tail shared by every monster death line ("The kobold thief falls to
/// the ground with a shrill cry."); the wording after it is per-template.
/// A player's death reads "<name> is dead." and is deliberately not
/// matched — it does not end our fight.
const DEATH_MARK: &str = "falls to the ground";

/// Did a monster just die?
///
/// Either the classic phrase, or the experience award that follows any
/// kill of ours — see [`crate::progress::exp_award`] for why the award is
/// the load-bearing half and the phrase only covers 67 of 1085 templates.
///
/// Public because the farm runner asks the same question, and used to
/// answer it with its own inlined copy of both strings.
pub fn is_kill_line(line: &str) -> bool {
    line.contains(DEATH_MARK) || crate::progress::is_exp_award(line)
}

/// The board's own combat-mode announcement, printed whenever OUR fight
/// ends — whatever ended it and however the death was worded. This is
/// the general un-latch the engaged-forever family kept asking for: a
/// kill can hide BOTH recognised end signals at once (a prose death
/// line while the untrained-XP cap suppresses the award), and the
/// latched bot then ignored fresh spawns for ~29s live (2026-08-01
/// arena run) until the quiet-prompt backstop expired.
pub fn is_combat_off(line: &str) -> bool {
    line.contains("*Combat Off*")
}

/// The board's word that a fight has BEGUN. A lone one opens a fresh
/// fight; one landing immediately after a `*Combat Off*` is the second
/// half of a target switch — the same attack the board answered by
/// ending the old fight and starting the new one in a single breath, so
/// nothing actually ended. See [`Bot`]'s `switch_watch`.
pub fn is_combat_engaged(line: &str) -> bool {
    line.contains("*Combat Engaged*")
}

/// Which heal the HP percent asks for, by the marks. None above the
/// minor mark. Below the major mark the instant heal, never the slow
/// regen. Between them the regen when one is named, else the minor.
pub fn heal_need(cfg: &BotConfig, percent: i32) -> Option<crate::sheet::HealNeed> {
    use crate::sheet::HealNeed;
    if cfg.major_heal_at_percent > 0 && percent < cfg.major_heal_at_percent as i32 {
        return Some(HealNeed::Major);
    }
    if cfg.minor_heal_at_percent > 0 && percent < cfg.minor_heal_at_percent as i32 {
        return Some(if cfg.hp_regen_spell.is_empty() { HealNeed::Minor } else { HealNeed::Regen });
    }
    None
}

/// The board's three ways of refusing an attack outright (crime.md §3,
/// all present verbatim in the shipped DLL). A refusal aborts the swing,
/// so unlike a real fight it is never followed by a death line, an
/// `ActorLeft`, or a room block without the target — nothing that would
/// otherwise clear the engaged latch.
const ATTACK_REFUSALS: [&str; 3] = [
    mud_core::crime::WARN_ON_EVIL_REFUSAL,
    mud_core::crime::TOO_EVIL_REFUSAL,
    mud_core::crime::LAWFUL_REFUSAL,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BotConfig {
    pub auto_combat: bool,
    pub auto_heal: bool,
    /// Rest or meditate when the marks call for it. Off leaves the
    /// character standing at low hp or mana, whatever `auto_heal` does
    /// with the spells. `rest_at_percent`, `mana_rest_at_percent` and
    /// `rest_until_percent` are unchanged by this switch, only whether
    /// they ever fire.
    pub auto_rest: bool,
    /// Pick up coins: piles announced by a kill's drop line, and piles
    /// the room render's "You notice ... here." line lists. Floor
    /// *items* are never taken — the board drops carried loot silently,
    /// and on a shared board a listed item is somebody's gear.
    pub auto_get: bool,
    /// Pick up keys listed on the floor that the ring does not hold.
    /// On by default and independent of `auto_get`: a key is not loot,
    /// it is the difference between a door and a wall, and the walker
    /// cannot plan a route through a key door it has not found the key
    /// for. Needs the pack, so it does nothing until the session has
    /// the item table.
    pub take_keys: bool,
    /// Arm a sneak before walking, when the character has any stealth.
    /// Off means no walk sneaks and no stealth spell is cast for one.
    /// The assist reads it too: standing idle it keeps a sneak armed,
    /// so the next move, its own or a party leader's, goes unseen, and
    /// a room it sneaked into is opened with a backstab.
    pub auto_sneak: bool,
    /// Hide when idle instead of sneaking, when the character has any
    /// stealth. Off by default. On, the assist sends `hide` whenever
    /// it finds itself standing idle, never sends `sneak`, and never
    /// opens with a backstab: hidden in place is what was asked for.
    #[serde(default)]
    pub auto_hide: bool,
    /// Denominations the sweep leaves on the floor, named as `get`
    /// takes them: copper, silver, gold, platinum, runic. At higher
    /// levels a copper pile is not worth the send. Applies to every
    /// sweep site: the kill's drop line, the room render, and the
    /// stop's own floor model.
    #[serde(default)]
    pub ignore_coins: Vec<String>,
    pub auto_flee: bool,
    /// Cast the minor heal when hp% drops below this. 0 = never. Was
    /// `spell_at_percent`, which still parses.
    ///
    /// The heal marks are the only recovery that works in a fight.
    /// Casting costs mana and does not disengage combat, so it is the
    /// answer to being hurt while something is still hitting you. The
    /// spells come from the character's own spellbook and the casting
    /// lives in [`crate::sheet::HealState`], because confirming a cast
    /// needs the correlation this pure core does without.
    #[serde(alias = "spell_at_percent")]
    pub minor_heal_at_percent: u32,
    /// Cast the major heal when hp% drops below this. 0 = never. Below
    /// this mark the instant heal is wanted, never the slow regen.
    pub major_heal_at_percent: u32,
    /// Stop and rest when hp% drops below this. 0 = never.
    ///
    /// Free, restores mana as well as health, and only coherent in an
    /// empty room. The board disengages combat to rest, so resting
    /// beside a monster is the death spiral `on_vitals` guards against.
    ///
    /// The alias is what this knob was called when resting was the only
    /// recovery there was. Profiles written then still mean rest.
    #[serde(alias = "heal_at_percent")]
    pub rest_at_percent: u32,
    /// Rest, or meditate, when mana% drops below this and HP is fine.
    /// 0 = never. Ignored without a pool.
    pub mana_rest_at_percent: u32,
    /// A rest is over once HP and, with a pool, mana are at or above
    /// this. A meditation is over once mana is. 0 never ends one and
    /// disables the departure gates.
    pub rest_until_percent: u32,
    /// Send `meditate` instead of `rest` when only mana needs
    /// recovering. Meditate is a quest ability the client cannot
    /// detect, so the player says.
    pub meditate: bool,
    /// Flee when hp% drops below this. The lowest mark, and it outranks
    /// every other.
    pub flee_at_percent: u32,
    /// Command issued to rest. Aliased for the same reason as
    /// `rest_at_percent`.
    #[serde(alias = "heal_command")]
    pub rest_command: String,
    /// The verb a fight opens and continues with, sent as it is with
    /// the target's noun after it: `a` for a swing, `pu` or `ju` for a
    /// mystic, `cast lbol` for a caster who fights with magic. A
    /// backstab opener is `bs` regardless, since it is the sneak that
    /// asked for it.
    #[serde(default = "default_attack_command")]
    pub attack_command: String,
    /// The minor heal, by the book's name. Empty means the cheapest
    /// heal in the book.
    pub minor_heal_spell: String,
    /// The major heal, by the book's name. Empty means the dearest heal
    /// in the book.
    pub major_heal_spell: String,
    /// A regen over time spell, by the book's name. Empty means none.
    /// Cast between the minor and major marks when not already running.
    pub hp_regen_spell: String,
    /// The old list form. Folded into the two names by
    /// [`BotConfig::normalise`] at load.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub heal_spells: Vec<String>,
    /// Buffs to keep up, by spell name — `bless` and the like. These are
    /// NOT healing: they are cast on a duration budget, not at an HP
    /// mark, and maintained by [`crate::sheet::BuffState`].
    pub buffs: Vec<String>,
    /// Never attacked. Matched as a substring, so "guardsman" also
    /// covers the rolled "fat guardsman".
    pub ignore: Vec<String>,
    /// Character max HP; 0 = unknown, disables percent policies.
    pub max_hp: i32,
    /// Character max mana or kai. 0 means no pool or unknown. Filled
    /// alongside `max_hp` whenever that probe runs, and never what
    /// gates the probe.
    pub max_mana: i32,
    /// Prompts with no blow struck either way before the fight is
    /// presumed over and the target released.
    ///
    /// The backstop for an ending nothing else recognises: an exp-less
    /// kill, a monster somebody else finished, or one of the 14 templates
    /// that ship with no death record at all. Without it the bot can sit
    /// latched on a corpse forever, and the farm runner reads a latched
    /// bot as a fight in progress, so the stop never ends.
    ///
    /// It has to be generous because prompts do NOT arrive one per combat
    /// round. They come in bursts — async output disturbs the dangling
    /// prompt and the board re-prompts, so "[HP=31]:[HP=32]:" lands on
    /// one physical line — and a cave bear is 50hp against single-digit
    /// hits, so rounds are seconds apart. Three was measured leaving a
    /// fight after a single burst.
    ///
    /// Being generous costs almost nothing, and costs less than it used
    /// to. This is a BACKSTOP, not the primary mechanism: the room block
    /// clears the latch properly by finding the target gone, and
    /// [`crate::farm::StopState`] now overrides this outright. If the
    /// count fires early in a genuinely slow fight, the last accepted room
    /// block still lists the monster, so the stop stays `Busy` and the
    /// runner does not walk off mid-fight — which it previously did, by
    /// reading the cleared latch as an idle room. The evidence beats the
    /// timer.
    ///
    /// What is left for this to cover is the case where no room block is
    /// coming at all — a dark room being the obvious one.
    pub combat_idle_prompts: u32,
    /// Start `mmc play` with the assist bot already on. The assist is
    /// the TUI's, not this struct's: it fights and loots beside the
    /// operator while no farm runs (`/bot` toggles it live), and it
    /// recovers by the same marks a farm does, and hides after a rest
    /// when the sheet shows Stealth.
    pub assist_play: bool,
}

impl BotConfig {
    /// Check the marks describe two ladders, before the socket opens.
    /// A typo should cost an error message, not a dead character.
    ///
    /// Out of order they cancel: with the major mark above the minor,
    /// the minor never fires. With the rest mark above the until mark,
    /// the bot rests and stops in the same breath. With the flee mark
    /// above anything, it runs before it recovers. A mark of 0 is OFF
    /// and is skipped rather than compared.
    pub fn validate(&self) -> Result<(), String> {
        for word in &self.ignore_coins {
            if !crate::purse::DENOMINATION_WORDS.contains(&word.as_str()) {
                return Err(format!(
                    "[bot].ignore_coins names {word:?}, which is not a denomination: \
                     the words are {}",
                    crate::purse::DENOMINATION_WORDS.join(", ")
                ));
            }
        }
        let ladders: [&[(&str, u32)]; 3] = [
            &[
                ("minor_heal_at_percent", self.minor_heal_at_percent),
                ("major_heal_at_percent", self.major_heal_at_percent),
                ("flee_at_percent", self.flee_at_percent),
            ],
            &[
                ("rest_until_percent", self.rest_until_percent),
                ("rest_at_percent", self.rest_at_percent),
                ("flee_at_percent", self.flee_at_percent),
            ],
            &[
                ("rest_until_percent", self.rest_until_percent),
                ("mana_rest_at_percent", self.mana_rest_at_percent),
            ],
        ];
        for ladder in ladders {
            let set: Vec<_> = ladder.iter().filter(|(_, v)| *v != 0).collect();
            for pair in set.windows(2) {
                let ((upper, u), (lower, l)) = (pair[0], pair[1]);
                if u < l {
                    return Err(format!(
                        "[bot].{upper} ({u}) is below {lower} ({l}): the marks are a ladder, \
                         and out of order the lower one fires first and the higher one never \
                         gets a chance"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Fold the old `heal_spells` list into the two named spells. The
    /// first entry is the minor heal and the last the major, unless a
    /// name is already set.
    pub fn normalise(&mut self) {
        if self.minor_heal_spell.is_empty()
            && let Some(first) = self.heal_spells.first()
        {
            self.minor_heal_spell = first.clone();
        }
        if self.major_heal_spell.is_empty()
            && self.heal_spells.len() > 1
            && let Some(last) = self.heal_spells.last()
        {
            self.major_heal_spell = last.clone();
        }
    }

    /// Mana as a percentage of the pool, or None without a pool or a
    /// reading.
    pub fn mana_percent(&self, mana: Option<i32>) -> Option<i32> {
        let mana = mana?;
        (self.max_mana > 0).then(|| mana * 100 / self.max_mana)
    }

    /// Both pools as the lobby's rest log prints them: `hp 12/52 23%`,
    /// then `mana 5/20 25%` when there is a pool and a reading, which
    /// are the two conditions under which [`Self::mana_percent`]
    /// answers. The percents are the ones the marks are compared
    /// against. No percent without a max: 0 is what the profile says
    /// when it never said.
    pub fn pools(&self, hp: i32, mana: Option<i32>) -> String {
        let mut out = format!("hp {hp}/{}", self.max_hp);
        if self.max_hp > 0 {
            out.push_str(&format!(" {}%", hp * 100 / self.max_hp));
        }
        if let (Some(m), Some(p)) = (mana, self.mana_percent(mana)) {
            out.push_str(&format!(", mana {m}/{} {p}%", self.max_mana));
        }
        out
    }

    /// Is this command one of the two recoveries the bot sends? The
    /// rest command is the profile's own word for it; `meditate` is the
    /// board's, and the bot sends it verbatim.
    pub fn is_rest(&self, cmd: &str) -> bool {
        cmd == self.rest_command || cmd == "meditate"
    }

    /// Why a rest the bot decided on went out, for the lobby's log: the
    /// command, the pools, and the two marks `Bot::on_vitals` read.
    pub fn rest_reason(&self, cmd: &str, hp: i32, mana: Option<i32>) -> String {
        format!(
            "{cmd}, {}, rest_at {}%, mana_rest_at {}%",
            self.pools(hp, mana),
            self.rest_at_percent,
            self.mana_rest_at_percent
        )
    }
}

fn default_attack_command() -> String {
    "a".into()
}

impl BotConfig {
    /// The table with every automatic policy off. What a job runs under
    /// while `/bot` is off: no fighting, healing, resting, looting, key
    /// taking, sneaking, hiding, fleeing or buffing, whatever the
    /// profile says.
    /// The marks, the spell names, the ignore lists and the pools are
    /// choices rather than policies and stay, so the next press
    /// restores the table as it was.
    pub fn switched_off(&self) -> BotConfig {
        BotConfig {
            auto_combat: false,
            auto_heal: false,
            auto_rest: false,
            auto_get: false,
            take_keys: false,
            auto_sneak: false,
            auto_hide: false,
            auto_flee: false,
            buffs: Vec::new(),
            ..self.clone()
        }
    }
}

impl Default for BotConfig {
    fn default() -> Self {
        BotConfig {
            auto_combat: true,
            auto_heal: true,
            auto_rest: true,
            auto_get: true,
            take_keys: true,
            auto_sneak: true,
            auto_hide: false,
            ignore_coins: Vec::new(),
            auto_flee: true,
            minor_heal_at_percent: 70,
            major_heal_at_percent: 60,
            rest_at_percent: 40,
            mana_rest_at_percent: 30,
            rest_until_percent: 95,
            meditate: false,
            flee_at_percent: 20,
            rest_command: "rest".into(),
            attack_command: default_attack_command(),
            minor_heal_spell: String::new(),
            major_heal_spell: String::new(),
            hp_regen_spell: String::new(),
            heal_spells: Vec::new(),
            buffs: Vec::new(),
            ignore: Vec::new(),
            max_hp: 0,
            max_mana: 0,
            combat_idle_prompts: 12,
            assist_play: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotAction {
    Send(String),
    /// Ask for a room block. The bot has an arrival it can neither
    /// attack nor ignore, and only a painted "Also here:" line can say
    /// which; whoever owns the looks decides how to answer.
    Look,
}

/// The word the board's targeting actually accepts. Spawned instances
/// carry a rolled adjective ("fat kobold thief") but resolution matches
/// the *template* name a word at a time, so the full display name is
/// too long to match and would be said aloud instead of swung. The
/// trailing noun always matches, and is what operators type.
///
/// `pub(crate)` because [`crate::world::Here`] picks a death line's
/// victim by the same word, and two copies of "which word names this
/// monster" would drift.
pub(crate) fn target_word(name: &str) -> &str {
    let name = strip_status(name);
    name.split_whitespace().last().unwrap_or(name)
}

/// Exits render as a display token, not a command — "closed door north",
/// "closed gate west". The direction is the last word.
/// The SGR the board paints an aggressive monster in.
///
/// The board colours occupants by what they ARE, and it is the only
/// signal that says so. Measured across six live captures, 67 distinct
/// names, zero counterexamples either way:
///
/// | SGR | behaviour mode | who |
/// |---|---|---|
/// | `1;35` bright magenta | 1, 2 | aggressive monsters — and players |
/// | `0;36` cyan | 0, 3 | townsfolk, animals, brawlers |
/// | `0;37` white | 4, 5 | guards, healers, named NPCs |
///
/// Players are bright magenta too, so colour alone is not enough: they
/// are told apart by capitalisation, which [`is_attackable`] already
/// tests. Aggressive monster = bright magenta AND lowercase.
pub const AGGRESSIVE_SGR: &str = "1;35";

/// Does the room block's own colouring permit swinging at occupant
/// `idx`?
///
/// **Self-calibrating.** A block that painted nobody has no opinion, and
/// the case rule alone decides — that is every unpainted board and every
/// fixture, so nothing that worked before stops working. A block that
/// painted anybody is trusted for everybody.
///
/// This exists because ranking by `exp * 1000 + hp` picks the fattest
/// thing in the room, and the fattest thing in a town is a passive
/// `drunken brawler`: 250 exp and 110 HP, more than twice a cave bear,
/// which never would have touched the character. A hand-maintained
/// ignore list was always going to miss one (live, 2026-08-02).
pub fn aggressive_here(room: &crate::events::RoomView, idx: usize) -> bool {
    if room.also_here_sgr.iter().all(Option::is_none) {
        return true; // unpainted: no opinion
    }
    room.also_here_sgr.get(idx).and_then(|c| c.as_deref()) == Some(AGGRESSIVE_SGR)
}

fn exit_command(exit: &str) -> &str {
    exit.split_whitespace().last().unwrap_or(exit)
}

/// "Also here:" lists players first, then monsters. Players and named
/// NPCs print capitalised; wandering monsters are lowercase generic
/// nouns. Attacking a player is a PK attempt, so only lowercase names
/// are candidates — and the TRAILING noun must be lowercase too:
/// adjective-prefixed named NPCs render like "thin Templar" (slice-8
/// seedy corpus), where the first word is a lowercase adjective but the
/// targeting noun is a capitalised name.
fn is_attackable(name: &str) -> bool {
    let name = strip_status(name);
    let first_lower = name.chars().next().is_some_and(char::is_lowercase);
    let noun_lower = name
        .split_whitespace()
        .last()
        .and_then(|w| w.chars().next())
        .is_some_and(char::is_lowercase);
    first_lower && noun_lower
}

/// Drop a leading "(Resting) " style status marker.
///
/// DEFENCE IN DEPTH. ` (Resting) ` is the status the board paints into
/// its prompt, and the pool template paints it AFTER the frame, so it
/// once rode into an `ActorEntered` name as "+  (Resting) fierce
/// filthbug". The parser now reads it off the prompt, and this strip
/// stays for the chunk boundary case that can still leave it on a line.
/// Stripping must not smuggle a player through, so the case test still
/// runs, on the name rather than the bracket.
fn strip_status(name: &str) -> &str {
    crate::correlate::strip_decoration(name)
}

/// Monster name (as the shipped data spells it, lowercase) -> how
/// dangerous it is. Built from the content database; see
/// [`crate::views::threat_table`].
pub type ThreatTable = std::collections::HashMap<String, i64>;

pub struct Bot {
    config: BotConfig,
    /// How dangerous each template is. Empty means "no opinion", and the
    /// bot then keeps the board's own listing order.
    threat: std::sync::Arc<ThreatTable>,
    /// The target under attack, the wander-out cooldown, and the
    /// target-switch window — see [`crate::combat::CombatState`].
    combat: crate::combat::CombatState,
    /// Exits from the most recent room block — the flee routes.
    exits: Vec<String>,
    /// A heal is already in flight; suppresses one per prompt.
    healing: bool,
    /// Already fled this room; suppresses one per prompt.
    fled: bool,
    /// The last room view this bot was shown listed something it would
    /// attack (the pump only shows it ATTRIBUTED blocks), or something
    /// attackable walked in since. Consulted before healing: in an
    /// occupied room the coherent choices are fight or flee — a rest is
    /// disengaged by the board and re-broken by the next engage, which
    /// was the live death spiral (2026-08-01, HP 21/52 vs a cave bear,
    /// rest/attack alternating every round). Deliberately NOT cleared
    /// by kill lines, ActorLeft or Combat Off: one death does not prove
    /// a room empty, and the stop machinery re-looks after every such
    /// event, so the next attributed block recomputes it honestly.
    room_has_work: bool,
    /// Targets the board refused to let us attack, keyed by the same
    /// trailing noun the attack command uses — the refusal applies to the
    /// template, so every rolled variant ("fat kobold thief") is covered
    /// by the one entry. Without this the cleared latch would simply
    /// re-engage on the next room block and be refused again forever.
    ///
    /// Shared rather than owned, because the farm runner builds a fresh
    /// bot for every stop and again on every lag and recovery. A refusal
    /// forgotten is a refused swing repeated, once per monster per stop
    /// per lap — and on the live board a refused swing is a crime-system
    /// interaction, not a free no-op.
    refused: Refusals,
    /// Coin denominations claimed this visit, keyed by the room name
    /// the block carried. A pile the character cannot carry
    /// (encumbrance refusal) stays listed in every block, and a bot
    /// that re-swept per block would `get` at the pacer floor forever.
    /// A claim lasts until the board answers it: a pickup or a "You
    /// don't see any" releases it, and a drop line is a new pile that
    /// claims afresh. A `get` the board never answers keeps its claim
    /// for the visit. Without the release, one contested `get` in a
    /// room the character never left silenced every later pile there
    /// (cw-carrot, 2026-09-12). Name-keyed, so the same-named-twin
    /// hazard costs a missed pile, never a loop.
    swept: (String, HashSet<String>),
    /// How the last painted block coloured each attackable noun:
    /// true for aggressive, false for anything else. Arrival lines
    /// paint every monster alike, guardsman and rogue both in bright
    /// yellow (cw-beef, 2026-09-12), so an arrival is judged by the
    /// noun's last listing instead.
    paint: HashMap<String, bool>,
    /// Some block painted somebody: this board colours its occupants,
    /// so an arrival it has not coloured yet is an open question, not
    /// a target. Kept apart from `paint`, which holds only monsters: a
    /// block listing nothing but players still proves the board paints.
    painted: bool,
    /// Keys already asked for this visit, keyed by room the same way
    /// `swept` is, and for the same reason: the board relists the
    /// floor on every block.
    taken: (String, HashSet<String>),
    /// The character's pack, when the owner had an item table to give.
    /// Without it no floor listing can be told to be a key.
    pack: Option<crate::pack::PackHandle>,
    /// What the very next `engage` should open with, primed by
    /// [`Bot::arm_backstab_opener`] from the walk that produced the
    /// CURRENT room (`crate::nav::Arrival::sneaking` /
    /// `crate::nav::Arrival::restore_weapon`). Consumed the first time
    /// `engage` fires — see [`Opener`]'s doc for why this must not
    /// survive past that one use. `None` (never primed) is today's
    /// unconditional `a <target>`.
    opener: Option<Opener>,
    /// The fight in progress opened with `bs` and its first blow has not
    /// landed yet. The board drops a backstab to a plain attack the
    /// moment that blow lands (`mud-core`'s `game.rs`, the autocombat
    /// revert), which for a mystic is a single punch a round where
    /// `attack_command` swings two or three times. So the verb goes out
    /// again on the landing blow, and this is what says the fight owes
    /// one. Assigned on every engage, so it can never outlive its fight.
    backstab_open: bool,
    /// How many times the board has told us the wielded weapon could not
    /// backstab (`mud-core`'s `text::CANNOT_BACKSTAB_WEAPON`) — meaning
    /// whatever this bot's caller believed was wielded at swap time was
    /// wrong. Not acted on here (no retry: the opening round is already
    /// spent either way, `2026-08-22-inventory-and-backstab-design.md`
    /// "Failure modes and their cost") — kept as evidence a caller or a
    /// test can consult, the "log it as a correction" the plan calls
    /// for.
    backstab_corrections: u32,
    /// The character has Stealth on the sheet, so idle time is spent
    /// hidden or sneaking (`auto_hide` says which). Set by the assist,
    /// never by a farm: a farm's stops are not idle.
    stealth: bool,
    /// The board answers a successful hide with silence, so this is a
    /// belief: set by `Attempting to hide...`, cleared by the noticed
    /// failure wording, by any room block (moving un-hides, and a look
    /// only ever follows a send) and by any other send.
    hidden: bool,
    /// The same belief for sneak. Set by `Attempting to sneak...`,
    /// which is also what a silently failed attempt prints, so the
    /// move's own word outranks it: a block behind "Sneaking..." keeps
    /// the belief and any other block drops it.
    sneaking: bool,
    /// "Sneaking..." has printed since the last room block: the move
    /// that block describes was made sneaking. Consumed by the block.
    sneak_held: bool,
    /// Prompts left to wait on a hide or sneak that is out. The echo
    /// clears it early. Counted down rather than held, because a
    /// stealth command sent too fast behind another is swallowed by the
    /// board without a word, and a flag would then wait forever.
    stealth_pending: u32,
    /// Stealth commands sent since the situation last changed, capped
    /// at three. A room block, a recovery and any other send reset it.
    stealth_tries: u32,
}

/// One arrival's belief about how the very next `engage` should open —
/// see [`Bot::arm_backstab_opener`]. Taken, not cloned, the moment
/// `engage` consumes it, so stale arrival evidence can never be reused
/// to open a SECOND, later fight in the same room: sneak covers exactly
/// the one move that carried the walk in
/// (`2026-08-22-inventory-and-backstab-design.md` "The state model is
/// trivial"), never whatever spawns minutes afterward.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Opener {
    /// Open with `bs <target>` — the walk believed itself armed and the
    /// wielded weapon (however it got there) is expected to backstab.
    /// `restore`, when set, is the primary weapon to `eq` back
    /// immediately after: the walk swapped to a backstab-only weapon
    /// before moving in, so the opening round is the only round it
    /// should spend wielding it (`2026-08-22-inventory-and-backstab-
    /// design.md` "The swap-back is safe: by then the character is in
    /// combat and the sneak is already spent").
    Backstab { restore: Option<String> },
}

/// Stealth commands sent per situation before the bot stops trying, and
/// the prompts waited between one and the next. See
/// [`Bot::idle_stealth`].
const STEALTH_TRIES: u32 = 3;
const STEALTH_WAIT: u32 = 3;

/// The set of targets the board has refused, shared across every bot a
/// run builds. See [`Bot::refused`].
pub type Refusals = std::sync::Arc<std::sync::Mutex<HashSet<String>>>;

impl Bot {
    pub fn new(config: BotConfig) -> Self {
        Bot::with_threat(config, std::sync::Arc::new(ThreatTable::new()))
    }

    /// As [`Bot::new`], but able to tell a cave bear from a giant rat.
    pub fn with_threat(config: BotConfig, threat: std::sync::Arc<ThreatTable>) -> Self {
        Bot::with_refusals(config, threat, Refusals::default())
    }

    /// As [`Bot::with_threat`], but inheriting refusals already learned.
    /// A run keeps one set and hands it to every bot it builds.
    pub fn with_refusals(
        config: BotConfig,
        threat: std::sync::Arc<ThreatTable>,
        refused: Refusals,
    ) -> Self {
        Bot {
            config,
            threat,
            combat: crate::combat::CombatState::new(),
            exits: Vec::new(),
            healing: false,
            fled: false,
            room_has_work: false,
            refused,
            swept: (String::new(), HashSet::new()),
            paint: HashMap::new(),
            painted: false,
            taken: (String::new(), HashSet::new()),
            pack: None,
            opener: None,
            backstab_open: false,
            backstab_corrections: 0,
            stealth: false,
            hidden: false,
            sneaking: false,
            sneak_held: false,
            stealth_pending: 0,
            stealth_tries: 0,
        }
    }

    /// Swap the policy and keep the memory. A settings change mid stop
    /// must not forget the fight in progress: a fresh bot reads an
    /// empty latch as an idle room, and the stop walks out on a monster
    /// still standing in it.
    pub fn reconfigure(&mut self, config: BotConfig) {
        self.config = config;
    }

    /// Tell this bot what the walk that produced the CURRENT room
    /// believed about its own backstab opener — see
    /// `crate::nav::Arrival::sneaking` / `crate::nav::Arrival::
    /// restore_weapon`. Call this once per arrival, before whatever
    /// `on_event` call might engage a target from it; the belief is
    /// consumed (taken) the first time `engage` fires, so it never
    /// leaks into a later, unrelated fight in the same room.
    ///
    /// `sneaking = false` disarms it outright (today's `a <target>`,
    /// whatever `restore` says) — a caller that never calls this at all
    /// gets the identical default.
    pub fn arm_backstab_opener(&mut self, sneaking: bool, restore: Option<String>) {
        self.opener = sneaking.then_some(Opener::Backstab { restore });
    }

    /// How many times the board has refused a `bs` for carrying the
    /// wrong weapon. See the field's own doc.
    pub fn backstab_corrections(&self) -> u32 {
        self.backstab_corrections
    }

    /// Keep stealth up when idle. The assist turns this on when the
    /// sheet shows Stealth. A farm never does, its stops are not idle.
    pub fn with_stealth(mut self, stealth: bool) -> Self {
        self.set_stealth(stealth);
        self
    }

    /// The same switch as [`Bot::with_stealth`], settable after the
    /// build. The assist needs it: it is built before the realm entry
    /// probe has read the stat sheet, so Stealth reads 0 at the build
    /// and only becomes known a few prompts later.
    pub fn set_stealth(&mut self, stealth: bool) {
        self.stealth = stealth;
    }

    /// Hand the bot the character's pack, so it can tell a key on the
    /// floor from somebody's dropped dagger. `None` leaves key pickup
    /// off, whatever `take_keys` says.
    pub fn with_pack(mut self, pack: Option<crate::pack::PackHandle>) -> Self {
        self.set_pack(pack);
        self
    }

    /// The same switch as [`Bot::with_pack`], settable after the build.
    /// The assist needs it for the same reason it needs
    /// [`Bot::set_hide`]: it is built before realm entry has called
    /// `Session::set_content`, so the build always sees `None`.
    pub fn set_pack(&mut self, pack: Option<crate::pack::PackHandle>) {
        self.pack = pack;
    }

    /// Does the bot believe the character is hidden.
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// Does the bot believe the character is sneaking.
    pub fn sneaking(&self) -> bool {
        self.sneaking
    }

    /// Release the one-shot heal/flee latches. Both re-arm on their own
    /// when the situation changes (HP recovers, a new room arrives), but
    /// a heal that never lands leaves HP low forever and would latch the
    /// debounce permanently. The runner calls this when it sees the heal
    /// was refused.
    pub fn rearm(&mut self) {
        self.healing = false;
        self.fled = false;
    }

    /// The board printed `*Combat Off*` in answer to a cast this client
    /// sent. Any cast made mid-fight ends the fight this way, a
    /// self-targeted heal included, and the target has not moved. This
    /// is the one Combat Off that must NOT start the wander-out
    /// cooldown: the runner pokes a look, the block lists the target,
    /// and the cooldown would refuse it once and then wait for a second
    /// block nobody sends. Live 2026-09-12: a mend mid-fight, then
    /// standing beside the monster until it killed the character.
    pub fn disengaged_by_own_cast(&mut self) {
        self.combat.clear();
    }

    /// The name currently under attack. The runner reads this to tell a
    /// quiet room from an unfinished fight.
    pub fn engaged(&self) -> Option<&str> {
        self.combat.engaged()
    }

    /// Nothing in hand: no fight running and no heal in flight. What a
    /// caller asks before spending the character's turn on an errand of
    /// its own, such as the follower gate's inventory read.
    pub fn is_idle(&self) -> bool {
        self.combat.engaged().is_none() && !self.healing
    }

    /// Did the last room block list something this bot would swing at,
    /// fight started or not. What makes a room not quiet.
    pub fn has_work(&self) -> bool {
        self.room_has_work
    }

    /// Has the board already refused to let us attack this?
    fn is_refused(&self, noun: &str) -> bool {
        self.refused.lock().expect("refusals").contains(noun)
    }

    /// The shared refusal set, for handing to the next bot a run builds.
    pub fn refusals(&self) -> Refusals {
        self.refused.clone()
    }

    /// A flee has been decided and no room block has arrived since, so
    /// where the character is standing is currently unknown.
    ///
    /// Cleared by the next block, whichever room it names — including the
    /// same one, when the flee did not take. The farm runner reads this to
    /// avoid ending a stop in the window between the flee going out and
    /// the board saying where it landed: the last block describes a room
    /// we may no longer be in.
    pub fn fled(&self) -> bool {
        self.fled
    }

    /// Feed one parsed event; returns the commands to send now.
    pub fn on_event(&mut self, ev: &Event) -> Vec<BotAction> {
        let actions = self.decide(ev);
        // Nearly every command breaks hide and sneak alike. Any send
        // that is not one of the two forgets both beliefs, and is a
        // new situation for the retry cap.
        // A look is not a command the board sees as movement or
        // action; it leaves both beliefs alone.
        if actions
            .iter()
            .any(|a| matches!(a, BotAction::Send(cmd) if cmd != "hide" && cmd != "sneak"))
        {
            self.forget_stealth();
        }
        actions
    }

    fn decide(&mut self, ev: &Event) -> Vec<BotAction> {
        // Resolve a Combat Off's one-event window (see
        // `crate::combat::CombatState`'s switch window doc). A
        // `*Combat Engaged*` right after the Off is the second half of
        // a target switch: the fight never ended, so restore the target
        // and cancel the wander-out cooldown the Off armed against it.
        // Any other event closes the window and the un-latch stands.
        if matches!(ev, Event::Line(l) if is_combat_engaged(l)) {
            self.combat.on_combat_engaged();
        } else {
            self.combat.close_switch_window();
        }
        match ev {
            Event::RoomSeen(room) => {
                self.exits = room.exits.clone();
                // Somewhere new: running away is allowed again.
                self.fled = false;
                // A block is a move or a look. Moving un-hides, and a
                // look only ever follows a send that forgot the hide.
                // Sneak survives a move exactly when the move said so.
                // The retry cap starts over in a NEW room only: a
                // re-render of this one (an empty line typed at the
                // board prints the room again) is not a new situation,
                // and reading it as one sent three sneaks per re-render
                // into a room that refused every one (live, 2026-09-08,
                // the Bank of Godfrey).
                self.hidden = false;
                self.sneaking = std::mem::take(&mut self.sneak_held);
                self.stealth_pending = 0;
                if self.swept.0 != room.name {
                    self.stealth_tries = 0;
                }
                // A sneaked move is the arrival a backstab opens from,
                // the same evidence a walk hands `arm_backstab_opener`.
                // Not under `auto_hide`: hidden in place was asked for,
                // not a backstab on entry.
                if self.sneaking && !self.config.auto_hide {
                    self.opener = Some(Opener::Backstab { restore: None });
                }
                // Settle the wander-out cooldown before choosing a
                // target: absence means the leave completed; presence in
                // a SECOND block means it never was leaving, and this
                // very block engages it.
                if let Some(noun) = self.combat.cooling_noun() {
                    let present = room
                        .also_here
                        .iter()
                        .any(|name| target_word(name) == noun);
                    self.combat.settle_cooling(present);
                }
                // Compared through `strip_status`, not by exact string.
                // A monster that sits down mid-fight re-renders with a
                // "(Resting) " decoration spliced in (DLL 0xe06f6), and
                // an exact match read the decorated name as a DIFFERENT
                // monster: the latch cleared, the target was re-engaged,
                // and the bot sent a fresh attack on every room block --
                // straight into flood control, on a fight already in
                // progress.
                if self.combat.engaged().is_some_and(|target| {
                    !room
                        .also_here
                        .iter()
                        .any(|name| strip_status(name) == strip_status(target))
                }) {
                    self.combat.clear();
                }
                // Biggest threat first. "Also here:" is in the board's
                // own order, which is not danger order -- taking the
                // first attackable name meant punching a giant rat while
                // a cave bear hit for 17.
                //
                // Ties keep the board's order, which `max_by_key` alone
                // would invert: it yields the LAST maximum, so equal
                // scores would pick the last name listed.
                // Judged with would_attack, not attackable: a fight in
                // progress is still work, and work is what makes
                // resting incoherent.
                self.room_has_work = room
                    .also_here
                    .iter()
                    .enumerate()
                    .any(|(i, name)| self.would_attack(name) && aggressive_here(room, i));
                if room.also_here_sgr.iter().any(Option::is_some) {
                    self.painted = true;
                    for (i, name) in room.also_here.iter().enumerate() {
                        if is_attackable(name) {
                            self.paint
                                .insert(target_word(name).to_string(), aggressive_here(room, i));
                        }
                    }
                }
                if self.swept.0 != room.name {
                    self.swept = (room.name.clone(), HashSet::new());
                }
                let mut actions: Vec<BotAction> = Vec::new();
                // Money on the floor is swept BEFORE the next fight is
                // picked — the same priority `StopState::verdict` uses,
                // so the assist and a `/farm` run behave identically.
                //
                // This used to be `!self.room_has_work`, i.e. never
                // sweep while anything was worth fighting, and the
                // attack was emitted first. On a shared board that
                // loses every contested pile: measured live 2026-08-02
                // (cwrun6.raw), a block announcing 15 copper was
                // answered `a rat`, then `look`, and only then `get
                // copper` — by which time another player had taken it.
                //
                // An ONGOING fight still outranks the floor: `engaged`
                // is set only while a swing has been traded, and it is
                // cleared above when the target stops being listed.
                // Standing over a pile mid-fight is how loot gets a
                // character killed; standing over one BEFORE the fight
                // costs at most a round of grace.
                if self.combat.engaged().is_none() {
                    for entry in &room.items {
                        if let Some((_, denom)) = coin_pile(entry)
                            && self.wants_coin(&denom)
                            && self.swept.1.insert(denom.clone())
                        {
                            actions.push(BotAction::Send(format!("get {denom}")));
                        }
                    }
                }
                // Keys are fetched whether or not coins are: a key is
                // the difference between a door and a wall on every
                // later route, and it is not somebody's loot the way a
                // dropped weapon is. Same per-visit memo as the sweep,
                // same reason.
                if self.config.take_keys && self.combat.engaged().is_none() {
                    if let Some(pack) = &self.pack {
                        if self.taken.0 != room.name {
                            self.taken = (room.name.clone(), HashSet::new());
                        }
                        for entry in &room.items {
                            let Some(item) = crate::items::resolve(pack.content(), entry) else {
                                continue;
                            };
                            if crate::pack::is_key(item)
                                && !pack.has(item.id)
                                && self.taken.1.insert(item.name.clone())
                            {
                                actions.push(BotAction::Send(format!("get {}", item.name)));
                            }
                        }
                    }
                }
                let target = room
                    .also_here
                    .iter()
                    .enumerate()
                    .filter(|(i, name)| self.attackable(name) && aggressive_here(room, *i))
                    .max_by_key(|(i, name)| (self.threat_of(name), std::cmp::Reverse(*i)))
                    .map(|(_, name)| name.clone());
                if let Some(name) = target {
                    actions.extend(self.engage(&name));
                }
                actions
            }
            Event::ActorEntered { name, .. } => {
                // An arrival is affirmative evidence: a NEW instance
                // walked in, whatever noun it shares with the leaver.
                if self.combat.cooling_noun() == Some(target_word(name)) {
                    self.combat.settle_cooling(false);
                }
                // On a board that paints, the noun's last listing
                // decides: passive is left alone, aggressive is engaged
                // at once, and a noun never listed is asked about.
                if self.painted {
                    match self.paint.get(target_word(name)) {
                        Some(true) => {}
                        Some(false) => return Vec::new(),
                        None => {
                            return if self.attackable(name) {
                                vec![BotAction::Look]
                            } else {
                                Vec::new()
                            };
                        }
                    }
                }
                if self.would_attack(name) {
                    self.room_has_work = true;
                }
                self.engage(name)
            }
            Event::ActorLeft { name, .. } => {
                if self.combat.engaged() == Some(name.as_str()) {
                    self.combat.clear();
                }
                // The leave line is the transition COMPLETING — the very
                // thing the cooldown was waiting out.
                if self.combat.cooling_noun() == Some(target_word(name)) {
                    self.combat.settle_cooling(false);
                }
                Vec::new()
            }
            Event::Prompt { hp, mana, status } => {
                // A fight that has gone silent is over, whatever the
                // board called the ending. Counted here rather than on
                // the death line because the death line is exactly what
                // cannot be relied on.
                self.combat.note_quiet_prompt(self.config.combat_idle_prompts);
                self.on_vitals(*hp, *mana, status.as_ref())
            }
            Event::CombatHit {
                attacker, target, ..
            } => {
                if self.involves_target(&actor_name(attacker))
                    || self.involves_target(&actor_name(target))
                {
                    self.combat.note_blow();
                }
                // The backstab's one blow landed: the board is now in
                // plain-attack mode, so the fight's own verb goes out
                // for the second round. Sent on the blow, not on a
                // later prompt, because the death that usually follows
                // arrives in the same burst and there is no earlier
                // moment that knows either way; the verb then hits
                // nothing and the board says so quietly ("Your command
                // had no effect."). See `backstab_open`.
                if self.backstab_open
                    && *attacker == crate::events::Actor::You
                    && self.involves_target(&actor_name(target))
                    && let Some(noun) = self.combat.engaged().map(target_word)
                {
                    self.backstab_open = false;
                    return vec![BotAction::Send(format!("{} {noun}", self.config.attack_command))];
                }
                // Deliberately NO counter-attack here. The attacker slot
                // of a hit line cannot be split out reliably: the attack
                // verb is per-monster data and can be multi-word — "The
                // fierce orc trainee all-out slashes you for 37 damage!"
                // (live, oracle_charm_lifecycle5.raw) parses its attacker
                // as "...trainee all-out", and a counter would have sent
                // "a all-out". Being hit by something unlisted is instead
                // answered by the farm's re-look (StopState invalidates
                // on a blow landing on us), where the room block names
                // the attacker properly and THIS bot engages from it.
                Vec::new()
            }
            Event::CombatMiss { line } => {
                if self.involves_target(line) {
                    self.combat.note_blow();
                }
                Vec::new()
            }
            Event::Line(line) => self.on_line(line),
            _ => Vec::new(),
        }
    }

    /// Is there anything in this room block we would swing at — now, or
    /// once the current fight ends?
    ///
    /// The farm runner asks this to decide whether a stop is finished.
    /// "No death line was seen" is a guess; "the board listed nobody we
    /// would fight" is a fact, and it is printed in every room block.
    ///
    /// Deliberately blind to [`Bot::engaged`]: a monster we are mid-fight
    /// with is STILL listed under "Also here:" — verbatim in
    /// `re/oracle/oracle_attack_syntax.raw`, where the block renders twice
    /// with the kobold thief stabbing throughout. Answering this with
    /// [`Bot::attackable`] instead would report an empty room in the
    /// middle of a fight, and the runner would walk out of it.
    pub fn has_target(&self, room: &crate::events::RoomView) -> bool {
        room.also_here
            .iter()
            .enumerate()
            .any(|(i, name)| self.would_attack(name) && aggressive_here(room, i))
    }

    /// The same question asked of any names at all, not just the ones a
    /// block happened to carry.
    ///
    /// [`crate::world::Here`] maintains an occupant list across kills,
    /// arrivals and departures, and it is more current than the last
    /// block by construction. Both forms route here so the two ways of
    /// asking cannot drift apart.
    pub fn has_target_among<'a>(&self, names: impl Iterator<Item = &'a str>) -> bool {
        names.into_iter().any(|name| self.would_attack(name))
    }

    /// Does this room list cash the policy would sweep? Coins only — an
    /// item wearing a denomination word has no leading count. Predicate
    /// only, like [`Bot::has_target`]: the travel guard asks it about
    /// arrival blocks, and the acting bot's once-per-visit memo is not
    /// consulted — whether a pile is worth STOPPING for and whether this
    /// instance already tried it are different questions.
    pub fn has_loot(&self, room: &crate::events::RoomView) -> bool {
        room.items
            .iter()
            .filter_map(|entry| coin_pile(entry))
            .any(|(_, denom)| self.wants_coin(&denom))
    }

    /// Would the policy pick up this denomination? `auto_get` and not
    /// on the ignore list. The render sweep, the drop-line sweep, and
    /// `has_loot` all ask this one question.
    pub fn wants_coin(&self, denom: &str) -> bool {
        self.config.auto_get && !self.ignores_coin(denom)
    }

    /// Is this denomination on the ignore list? The list alone, blind
    /// to `auto_get`. The stop's floor model consults it through
    /// `StopState::verdict`, whose bot has `auto_get` forced off so it
    /// is not also a loot owner. This is the one place that reads
    /// `ignore_coins`, and `wants_coin` goes through it.
    pub fn ignores_coin(&self, denom: &str) -> bool {
        self.config.ignore_coins.iter().any(|d| d == denom)
    }

    /// Would we swing at this name at all? The toggle, the case rule that
    /// tells a monster from a player, the ignore list, and the set of
    /// targets the board has already refused.
    ///
    /// Says nothing about whether we are *already busy* — that is
    /// [`Bot::attackable`]. The split exists because "is this room worth
    /// staying in" and "should I attack this now" are different questions
    /// and only the second one cares about the current fight. Public
    /// because the world-state consumers (the recast coherence gate) ask
    /// the same question about maintained occupants.
    pub fn would_attack(&self, name: &str) -> bool {
        self.config.auto_combat
            && is_attackable(name)
            && !self.is_refused(target_word(name))
            && !self.config.ignore.iter().any(|i| name.contains(i.as_str()))
    }

    /// Is this something we would swing at right now? Split out of
    /// [`Bot::engage`] so candidates can be ranked before one is chosen,
    /// rather than the first acceptable name winning by position.
    fn attackable(&self, name: &str) -> bool {
        self.combat.engaged().is_none()
            && self.would_attack(name)
            // Not while its wander-out is in question — see
            // `crate::combat::CombatState`. Only here, NOT in
            // `would_attack`: the leaver still counts as the room's
            // work, so the stop waits and nobody rests beside a
            // transition.
            && self.combat.cooling_noun() != Some(target_word(name))
    }

    /// How dangerous `name` is.
    ///
    /// Instances carry a rolled adjective ("fierce filthbug") while the
    /// table is keyed by template ("filthbug"), so the longest table entry
    /// that the display name ENDS WITH wins — longest because "cave bear"
    /// must beat a hypothetical "bear". Unknown names score 0 and so keep
    /// the board's order among themselves.
    fn threat_of(&self, name: &str) -> i64 {
        let lower = name.to_lowercase();
        self.threat
            .iter()
            .filter(|(template, _)| lower.ends_with(template.as_str()))
            .max_by_key(|(template, _)| template.len())
            .map(|(_, score)| *score)
            .unwrap_or(0)
    }

    /// Attack `name`, unless combat is off, a target is already engaged,
    /// the name is not a monster, or it is on the ignore list.
    ///
    /// Opens with a backstab instead of an ordinary swing when
    /// [`Bot::arm_backstab_opener`] believes this arrival is armed for
    /// one — see [`Bot::opening_attack`]. Returns every command the
    /// opener needs, in order: `bs`, then (only when the walk swapped
    /// for it) the restore `eq` right behind it, so the primary weapon
    /// is back before the SECOND round rather than lingering wielded for
    /// the rest of the fight.
    fn engage(&mut self, name: &str) -> Vec<BotAction> {
        if !self.attackable(name) {
            return Vec::new();
        }
        self.combat.engage(name);
        self.opening_attack(name)
    }

    /// The command(s) that open a fight with `name`: an ordinary swing
    /// by default, or `bs` (plus a restore `eq`) when
    /// [`Bot::arm_backstab_opener`] armed this arrival. Takes the primed
    /// belief — see [`Opener`]'s doc for why it must not survive past
    /// this one use.
    ///
    /// A `bs` sent while the character was not actually stealthy is a
    /// silent plain attack — cheap, which is the whole reason the ALWAYS
    /// SNEAK policy can afford to try whenever the walk believed itself
    /// armed rather than proving it first
    /// (`2026-08-22-inventory-and-backstab-design.md` "Failure modes and
    /// their cost").
    fn opening_attack(&mut self, name: &str) -> Vec<BotAction> {
        let target = target_word(name);
        let opener = self.opener.take();
        self.backstab_open = opener.is_some();
        match opener {
            Some(Opener::Backstab { restore: Some(primary) }) => vec![
                BotAction::Send(format!("bs {target}")),
                BotAction::Send(format!("eq {primary}")),
            ],
            Some(Opener::Backstab { restore: None }) => {
                vec![BotAction::Send(format!("bs {target}"))]
            }
            None => vec![BotAction::Send(format!("{} {target}", self.config.attack_command))],
        }
    }

    /// This character's health as a percentage of max, or `None` when
    /// `max_hp` is unknown (0) or the character is downed and HP reads
    /// negative — the two cases [`Bot::on_vitals`] refuses to decide on.
    ///
    /// Exposed so the runner's spell-heal dispatch reads exactly the
    /// number the rest and flee marks are compared against, rather than
    /// recomputing it from a `max_hp` it holds separately.
    pub fn hp_percent(&self, hp: i32) -> Option<i32> {
        (self.config.max_hp > 0 && hp > 0).then(|| hp * 100 / self.config.max_hp)
    }

    /// Percent of max policies, decided on every prompt.
    ///
    /// Flee outranks everything: staying to heal is what gets a
    /// character killed. Then the board's own word on a recovery in
    /// progress: while the prompt says resting or meditating nothing is
    /// sent, and the recovery is over once the pools clear
    /// `rest_until_percent`. The board has no command to end one, so
    /// being over means the bot is free to act again, and
    /// [`Bot::on_recovered`] says what it does with that. Below the
    /// marks, standing, in a clear room, a rest or a meditation goes out
    /// once, and the latch holds until the board shows it landed.
    ///
    /// Prompts arrive in bursts, so every decision here fires once and
    /// re-arms on a change of situation. Deciding per prompt would send
    /// a command per burst and trip flood control.
    ///
    /// The heal marks are deliberately not here. Casting has to be
    /// confirmed from the board's own wording and this core is
    /// attribution blind on purpose. [`heal_need`] picks the kind of
    /// heal from the percent, and [`crate::sheet::HealState`] casts it.
    fn on_vitals(&mut self, hp: i32, mana: Option<i32>, status: Option<&Status>) -> Vec<BotAction> {
        // Downed: commands do not land, and HP reads negative.
        let Some(percent) = self.hp_percent(hp) else {
            return Vec::new();
        };
        if self.config.auto_flee
            && percent < self.config.flee_at_percent as i32
            && !self.fled
            && let Some(exit) = self.exits.first()
        {
            self.fled = true;
            return vec![BotAction::Send(exit_command(exit).to_string())];
        }
        self.stealth_pending = self.stealth_pending.saturating_sub(1);
        let mana_percent = self.config.mana_percent(mana);
        let until = self.config.rest_until_percent as i32;
        let resting = matches!(status, Some(Status::Resting));
        let meditating = matches!(status, Some(Status::Meditating));
        if resting || meditating {
            // The rest landed. The latch has done its job.
            self.healing = false;
            let hp_ok = percent >= until;
            let mana_ok = mana_percent.is_none_or(|m| m >= until);
            let over = until > 0 && if resting { hp_ok && mana_ok } else { mana_ok };
            return if over { self.on_recovered() } else { Vec::new() };
        }
        let hp_low = percent < self.config.rest_at_percent as i32;
        let mana_low = self.config.mana_rest_at_percent > 0
            && mana_percent.is_some_and(|m| m < self.config.mana_rest_at_percent as i32);
        if !hp_low && !mana_low {
            self.healing = false;
            return if self.combat.engaged().is_none() && !self.room_has_work {
                self.idle_stealth()
            } else {
                Vec::new()
            };
        }
        // Never rest in a room that holds a fight or work: the board
        // disengages combat to rest, the un-latch frees the bot, the
        // next block re-engages and breaks the rest. That was the live
        // death spiral of 2026-08-01. Fight or flee are the occupied
        // room choices, and flee is checked above.
        if !self.config.auto_rest || self.healing || self.combat.engaged().is_some() || self.room_has_work {
            return Vec::new();
        }
        self.healing = true;
        // A recovery is a new situation for the stealth that follows it.
        self.stealth_tries = 0;
        let cmd = if hp_low || !self.config.meditate {
            self.config.rest_command.clone()
        } else {
            "meditate".to_string()
        };
        vec![BotAction::Send(cmd)]
    }

    /// The recovery is over and the bot is free to act. With Stealth
    /// that means hide or sneak, which also ends the rest. Once, guarded
    /// by the pending count, because the echo's own prompt still says
    /// resting.
    fn on_recovered(&mut self) -> Vec<BotAction> {
        self.idle_stealth()
    }

    /// The stealth command idle time is spent under: `hide` when
    /// `auto_hide` says so, else `sneak` when `auto_sneak` does, and
    /// nothing without Stealth on the sheet or with both off.
    fn stealth_command(&self) -> Option<&'static str> {
        if !self.stealth {
            None
        } else if self.config.auto_hide {
            Some("hide")
        } else if self.config.auto_sneak {
            Some("sneak")
        } else {
            None
        }
    }

    /// Standing idle: keep stealth up. Once per situation while the
    /// belief says it is down, waited on through `stealth_pending`, and
    /// capped at three sends so a room the character cannot hide in
    /// does not draw a hide per prompt.
    fn idle_stealth(&mut self) -> Vec<BotAction> {
        let Some(cmd) = self.stealth_command() else {
            return Vec::new();
        };
        let up = if cmd == "hide" { self.hidden } else { self.sneaking };
        if up || self.stealth_pending > 0 || self.stealth_tries >= STEALTH_TRIES {
            return Vec::new();
        }
        self.stealth_pending = STEALTH_WAIT;
        self.stealth_tries += 1;
        vec![BotAction::Send(cmd.into())]
    }

    /// The situation changed: nothing is believed up and the retry cap
    /// starts over.
    fn forget_stealth(&mut self) {
        self.hidden = false;
        self.sneaking = false;
        self.stealth_pending = 0;
        self.stealth_tries = 0;
    }

    /// Does this text name the monster we are fighting? Matched on the
    /// trailing noun, the same word the attack command uses, so a rolled
    /// adjective ("fat kobold thief") still counts as our fight.
    fn involves_target(&self, text: &str) -> bool {
        self.combat
            .engaged()
            .map(target_word)
            .is_some_and(|noun| text.to_lowercase().contains(&noun.to_lowercase()))
    }

    fn on_line(&mut self, line: &str) -> Vec<BotAction> {
        // The stealth echoes and their failures. Matched anywhere on
        // the line: the live board glues a seen failure onto the
        // attempt ("Attempting to sneak...You don't think you're
        // sneaking."). A failed roll is retried after a fresh wait,
        // under the same cap as a silence: retrying at the very next
        // prompt sent three in a row inside a second, since every
        // failure prints one. A hard block is not a roll and is not
        // retried until the situation changes.
        if line.contains("Attempting to hide") {
            self.hidden = true;
            self.stealth_pending = 0;
        }
        if line.contains("Attempting to sneak") {
            self.sneaking = true;
            self.stealth_pending = 0;
        }
        if line.contains("don't think you are hidden") {
            self.hidden = false;
            self.stealth_pending = STEALTH_WAIT;
        }
        if line.contains("don't think you're sneaking") {
            self.sneaking = false;
            self.stealth_pending = STEALTH_WAIT;
        }
        if line.contains("You may not sneak right now") {
            self.sneaking = false;
            self.stealth_pending = 0;
            self.stealth_tries = STEALTH_TRIES;
        }
        // The move's own word on sneak, the only trustworthy one: it
        // opens a sneaked move, right before the room block, and a
        // failed roll is announced on entry instead.
        let lower = line.trim_start().to_lowercase();
        if lower.starts_with("sneaking...") {
            self.sneak_held = true;
        }
        if lower.contains("you make a sound as you enter the room") {
            self.sneaking = false;
        }
        // The fight ended: re-arm so the next arrival is engaged. Death
        // lines name the template, not the rolled instance, so any death
        // clears — a redundant re-attack is harmless, a permanent latch
        // on a corpse is not. "*Combat Off*" is the board's own
        // announcement and covers the endings the other signals miss: a
        // prose death under the untrained-XP cap produces neither a
        // death mark nor an award, and the latch then held for 29s live.
        if is_kill_line(line) || is_combat_off(line) {
            // A Combat Off that still finds the latch held is TARGETLESS:
            // no death line or award preceded it, so the fight ended some
            // way we did not see — the wander-out. Start the same-noun
            // cooldown. A kill's Combat Off finds the latch already
            // cleared and starts nothing.
            //
            // The un-latch is applied at once, as a real ending must be:
            // most Combat Offs are real, and the tests and the runner
            // read `engaged` the instant this returns. A target switch
            // is the exception the board words as `*Combat Off*` then
            // `*Combat Engaged*` in one breath; the switch window holds
            // what was cleared so the very next event can undo it if the
            // Engaged proves the fight never ended.
            if is_combat_off(line) {
                self.combat.on_combat_off();
            } else {
                self.combat.on_kill();
            }
        }
        // Our attack echoed back as SPEECH: the target resolved to
        // nobody (it left in the race between the block and the swing),
        // the fight never started, and nothing that ends a fight will
        // ever arrive. Only the echo of the exact attack we have in
        // flight counts — anything else said is just words.
        if let Some(noun) = self.combat.engaged().map(target_word)
            && line == format!("You say \"{} {noun}\"", self.config.attack_command)
        {
            self.combat.clear();
        }
        // The refusal names no monster, so the target is whichever one we
        // just swung at.
        if ATTACK_REFUSALS.iter().any(|r| line.contains(r))
            && let Some(name) = self.combat.engaged().map(str::to_string)
        {
            self.combat.clear();
            self.refused
                .lock()
                .expect("refusals")
                .insert(target_word(&name).to_string());
        }
        // The wielded weapon could not backstab -- whatever the caller
        // believed was wielded when it swapped for this opener was
        // wrong. `attack_with_mode` still ran the swing as a normal
        // attack (`mud-core`'s `backstab_command`), so the fight is not
        // refused and `self.engaged` stands; there is nothing to retry,
        // the opening round is already spent either way. Recorded as
        // evidence only -- see `backstab_corrections`'s doc.
        if line == mud_core::text::CANNOT_BACKSTAB_WEAPON {
            self.backstab_corrections += 1;
        }
        // An item reached the pack. Re-read it from the board rather
        // than adding the item by inference: the reply is the truth
        // and the refresh is free.
        if self.config.take_keys && self.pack.is_some() && picked_up_item(line).is_some() {
            return vec![BotAction::Send("i".into())];
        }
        // The board answered a `get`: the claim on that denomination
        // is settled either way, and the next listing is a new pile.
        if let Some((_, denom)) = picked_up(line) {
            self.swept.1.remove(&denom);
        }
        if let Some(denom) = pile_gone(line) {
            self.swept.1.remove(&denom);
        }
        // A drop line is a NEW pile, whatever the memo says: the claim
        // is taken afresh, so the block that follows -- which lists
        // the very pile the kill just reported -- finds it claimed and
        // sends nothing. Before this the drop line deferred to the
        // memo, and a pile dropped after a contested `get` was never
        // taken.
        //
        // This does not key on room name the way the render path does:
        // a drop line only ever arrives mid-visit, after the RoomSeen
        // that keyed `swept.0` for the room we are standing in. The one
        // gap is a bot whose very first event is a drop line with no
        // room seen yet: the memo is still keyed to the empty string,
        // and the next RoomSeen resets it, undoing this claim and
        // allowing one repeat `get`.
        coin_drop(line)
            .filter(|(_, denom)| self.wants_coin(denom))
            .map(|(_, denom)| {
                self.swept.1.insert(denom.clone());
                vec![BotAction::Send(format!("get {denom}"))]
            })
            .unwrap_or_default()
    }
}

/// The name an [`Actor`] prints as; "you" for the player, so it never
/// matches a monster's trailing noun by accident.
fn actor_name(a: &crate::events::Actor) -> String {
    match a {
        crate::events::Actor::You => "you".to_string(),
        crate::events::Actor::Other(name) => name.clone(),
    }
}

