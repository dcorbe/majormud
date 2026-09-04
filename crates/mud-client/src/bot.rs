//! Bot policy engine — the MegaMud toggle set (AutoCombat, AutoHeal,
//! AutoGet, AutoFlee) as a pure decision core: events in, commands out.
//! The async runner glues it to a [`crate::session::Session`].

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::events::Event;

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
    /// Pick up coins: piles announced by a kill's drop line, and piles
    /// the room render's "You notice ... here." line lists. Floor
    /// *items* are never taken — the board drops carried loot silently,
    /// and on a shared board a listed item is somebody's gear.
    pub auto_get: bool,
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
    /// never heals or flees — movement and rest belong to the person
    /// holding the keyboard.
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
}

impl Default for BotConfig {
    fn default() -> Self {
        BotConfig {
            auto_combat: false,
            auto_heal: false,
            auto_get: false,
            auto_flee: false,
            minor_heal_at_percent: 70,
            major_heal_at_percent: 40,
            rest_at_percent: 60,
            mana_rest_at_percent: 30,
            rest_until_percent: 95,
            meditate: false,
            flee_at_percent: 20,
            rest_command: "rest".into(),
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
/// [`crate::graph::RoomGraph::load_threat`].
pub type ThreatTable = std::collections::HashMap<String, i64>;

pub struct Bot {
    config: BotConfig,
    /// How dangerous each template is. Empty means "no opinion", and the
    /// bot then keeps the board's own listing order.
    threat: std::sync::Arc<ThreatTable>,
    /// Name currently under attack; cleared once it is gone.
    engaged: Option<String>,
    /// Exits from the most recent room block — the flee routes.
    exits: Vec<String>,
    /// A heal is already in flight; suppresses one per prompt.
    healing: bool,
    /// Already fled this room; suppresses one per prompt.
    fled: bool,
    /// Prompts seen since the last blow involving the engaged target.
    quiet_prompts: u32,
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
    /// A noun we must not re-engage yet, and how many blocks have listed
    /// it since. Set by a TARGETLESS *Combat Off* — the un-latch firing
    /// while we still believed we were engaged, which is the signature of
    /// the target wandering out mid-fight (a kill un-latches on the death
    /// line first). The board keeps listing a leaver for the length of
    /// its leave transition, so the freed bot re-engaged it off the next
    /// block and the board flipped Engaged/Off — ~40 cycles in 400ms live
    /// (run4, 2026-08-01). Cleared by absence from a block, by the leave
    /// or arrival event that settles the question, or by surviving two
    /// listed blocks — a monster still there after two looks is not
    /// leaving, it is standing there.
    cooling: Option<(String, u32)>,
    /// Coin denominations already swept this visit, keyed by the room
    /// name the block carried. A pile the character cannot carry
    /// (encumbrance refusal) stays listed in every block, and a bot
    /// that re-swept per block would `get` at the pacer floor forever.
    /// One try per denomination per visit; a fresh pile mid-stay is the
    /// drop line's job. Name-keyed, so the same-named-twin hazard costs
    /// a missed pile, never a loop.
    swept: (String, HashSet<String>),
    /// What the very next `engage` should open with, primed by
    /// [`Bot::arm_backstab_opener`] from the walk that produced the
    /// CURRENT room (`crate::nav::Arrival::sneaking` /
    /// `crate::nav::Arrival::restore_weapon`). Consumed the first time
    /// `engage` fires — see [`Opener`]'s doc for why this must not
    /// survive past that one use. `None` (never primed) is today's
    /// unconditional `a <target>`.
    opener: Option<Opener>,
    /// How many times the board has told us the wielded weapon could not
    /// backstab (`mud-core`'s `text::CANNOT_BACKSTAB_WEAPON`) — meaning
    /// whatever this bot's caller believed was wielded at swap time was
    /// wrong. Not acted on here (no retry: the opening round is already
    /// spent either way, `2026-08-22-inventory-and-backstab-design.md`
    /// "Failure modes and their cost") — kept as evidence a caller or a
    /// test can consult, the "log it as a correction" the plan calls
    /// for.
    backstab_corrections: u32,
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
            engaged: None,
            exits: Vec::new(),
            healing: false,
            fled: false,
            quiet_prompts: 0,
            room_has_work: false,
            refused,
            cooling: None,
            swept: (String::new(), HashSet::new()),
            opener: None,
            backstab_corrections: 0,
        }
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

    /// Release the one-shot heal/flee latches. Both re-arm on their own
    /// when the situation changes (HP recovers, a new room arrives), but
    /// a heal that never lands leaves HP low forever and would latch the
    /// debounce permanently. The runner calls this when it sees the heal
    /// was refused.
    pub fn rearm(&mut self) {
        self.healing = false;
        self.fled = false;
    }

    /// The name currently under attack. The runner reads this to tell a
    /// quiet room from an unfinished fight.
    pub fn engaged(&self) -> Option<&str> {
        self.engaged.as_deref()
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
        match ev {
            Event::RoomSeen(room) => {
                self.exits = room.exits.clone();
                // Somewhere new: running away is allowed again.
                self.fled = false;
                // Settle the wander-out cooldown before choosing a
                // target: absence means the leave completed; presence in
                // a SECOND block means it never was leaving, and this
                // very block engages it.
                if let Some((noun, listed)) = &mut self.cooling {
                    let present = room
                        .also_here
                        .iter()
                        .any(|name| target_word(name) == noun);
                    *listed += 1;
                    if !present || *listed >= 2 {
                        self.cooling = None;
                    }
                }
                // Compared through `strip_status`, not by exact string.
                // A monster that sits down mid-fight re-renders with a
                // "(Resting) " decoration spliced in (DLL 0xe06f6), and
                // an exact match read the decorated name as a DIFFERENT
                // monster: the latch cleared, the target was re-engaged,
                // and the bot sent a fresh attack on every room block --
                // straight into flood control, on a fight already in
                // progress.
                if self.engaged.as_deref().is_some_and(|target| {
                    !room
                        .also_here
                        .iter()
                        .any(|name| strip_status(name) == strip_status(target))
                }) {
                    self.engaged = None;
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
                if self.config.auto_get && self.engaged.is_none() {
                    for entry in &room.items {
                        if let Some((_, denom)) = coin_pile(entry)
                            && self.swept.1.insert(denom.clone())
                        {
                            actions.push(BotAction::Send(format!("get {denom}")));
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
                if self
                    .cooling
                    .as_ref()
                    .is_some_and(|(noun, _)| target_word(name) == noun)
                {
                    self.cooling = None;
                }
                if self.would_attack(name) {
                    self.room_has_work = true;
                }
                self.engage(name)
            }
            Event::ActorLeft { name, .. } => {
                if self.engaged.as_deref() == Some(name.as_str()) {
                    self.engaged = None;
                }
                // The leave line is the transition COMPLETING — the very
                // thing the cooldown was waiting out.
                if self
                    .cooling
                    .as_ref()
                    .is_some_and(|(noun, _)| target_word(name) == noun)
                {
                    self.cooling = None;
                }
                Vec::new()
            }
            Event::Prompt { hp, .. } => {
                // A fight that has gone silent is over, whatever the
                // board called the ending. Counted here rather than on
                // the death line because the death line is exactly what
                // cannot be relied on.
                if self.engaged.is_some() {
                    self.quiet_prompts += 1;
                    if self.quiet_prompts >= self.config.combat_idle_prompts {
                        self.engaged = None;
                        self.quiet_prompts = 0;
                    }
                }
                self.on_hp(*hp)
            }
            Event::CombatHit {
                attacker, target, ..
            } => {
                if self.involves_target(&actor_name(attacker))
                    || self.involves_target(&actor_name(target))
                {
                    self.quiet_prompts = 0;
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
                    self.quiet_prompts = 0;
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
        self.config.auto_get && room.items.iter().any(|entry| COIN_PILE_RE.is_match(entry))
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
        self.engaged.is_none()
            && self.would_attack(name)
            // Not while its wander-out is in question — see `cooling`.
            // Only here, NOT in `would_attack`: the leaver still counts
            // as the room's work, so the stop waits and nobody rests
            // beside a transition.
            && !self
                .cooling
                .as_ref()
                .is_some_and(|(noun, _)| target_word(name) == noun)
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
        self.engaged = Some(name.to_string());
        self.quiet_prompts = 0;
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
        match self.opener.take() {
            Some(Opener::Backstab { restore: Some(primary) }) => vec![
                BotAction::Send(format!("bs {target}")),
                BotAction::Send(format!("eq {primary}")),
            ],
            Some(Opener::Backstab { restore: None }) => {
                vec![BotAction::Send(format!("bs {target}"))]
            }
            None => vec![BotAction::Send(format!("a {target}"))],
        }
    }

    /// This character's health as a percentage of max, or `None` when
    /// `max_hp` is unknown (0) or the character is downed and HP reads
    /// negative — the two cases [`Bot::on_hp`] refuses to decide on.
    ///
    /// Exposed so the runner's spell-heal dispatch reads exactly the
    /// number the rest and flee marks are compared against, rather than
    /// recomputing it from a `max_hp` it holds separately.
    pub fn hp_percent(&self, hp: i32) -> Option<i32> {
        (self.config.max_hp > 0 && hp > 0).then(|| hp * 100 / self.config.max_hp)
    }

    /// Percent-of-max policies. Flee outranks rest: staying to heal is
    /// what gets a character killed. Both fire once and re-arm on a
    /// change of situation, because prompts arrive in bursts: async
    /// output disturbs the dangling prompt and the board re-prompts, so
    /// several can land in a row — `[HP=31]:[HP=32]:` on one physical
    /// line — while the situation has not changed at all. Deciding per
    /// prompt would send a command per burst and trip flood control,
    /// which is measured at eight sends 1.3s apart (see
    /// `tests/board_cadence.rs`).
    ///
    /// Note the bursts come from output, not from a timer: an idle board
    /// sends nothing whatsoever, for minutes at a stretch.
    ///
    /// The heal marks, `minor_heal_at_percent` and `major_heal_at_percent`,
    /// are deliberately NOT here. Casting a heal has to be confirmed from
    /// the board's own wording, and this core is attribution-blind on
    /// purpose — a monster's "...attempted to cast X at you, but failed."
    /// would read as our own fizzle. It lives in
    /// [`crate::sheet::HealState`], dispatched by the
    /// runner, which is also where the mana floor and the one-cast-per-
    /// round pacing belong. What that leaves here is the ordering these
    /// two marks have always had.
    fn on_hp(&mut self, hp: i32) -> Vec<BotAction> {
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
        if percent >= self.config.rest_at_percent as i32 {
            self.healing = false;
        } else if self.config.auto_heal
            && !self.healing
            // Never rest in a room that holds a fight or work: the board
            // disengages combat to rest, the un-latch frees the bot, the
            // next block re-engages and breaks the rest — the live death
            // spiral (2026-08-01, HP 21/52 vs a cave bear, rest/attack
            // alternating every ~5s round). Fight or flee are the
            // occupied-room choices; flee is checked above and already
            // outranks. No latch is spent on the suppressed path, so the
            // first prompt after the room is proven clear heals.
            && self.engaged.is_none()
            && !self.room_has_work
        {
            self.healing = true;
            return vec![BotAction::Send(self.config.rest_command.clone())];
        }
        Vec::new()
    }

    /// Does this text name the monster we are fighting? Matched on the
    /// trailing noun, the same word the attack command uses, so a rolled
    /// adjective ("fat kobold thief") still counts as our fight.
    fn involves_target(&self, text: &str) -> bool {
        self.engaged
            .as_deref()
            .map(target_word)
            .is_some_and(|noun| text.to_lowercase().contains(&noun.to_lowercase()))
    }

    fn on_line(&mut self, line: &str) -> Vec<BotAction> {
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
            if is_combat_off(line)
                && let Some(target) = &self.engaged
            {
                self.cooling = Some((target_word(target).to_string(), 0));
            }
            self.engaged = None;
            self.quiet_prompts = 0;
        }
        // Our attack echoed back as SPEECH: the target resolved to
        // nobody (it left in the race between the block and the swing),
        // the fight never started, and nothing that ends a fight will
        // ever arrive. Only the echo of the exact attack we have in
        // flight counts — anything else said is just words.
        if let Some(noun) = self.engaged.as_deref().map(target_word)
            && line == format!("You say \"a {noun}\"")
        {
            self.engaged = None;
            self.quiet_prompts = 0;
        }
        // The refusal names no monster, so the target is whichever one we
        // just swung at.
        if ATTACK_REFUSALS.iter().any(|r| line.contains(r))
            && let Some(name) = self.engaged.take()
        {
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
        if !self.config.auto_get {
            return Vec::new();
        }
        // Consult the same per-visit memo the room-render sweep uses
        // (~line 602): a kill's drop line and the room's own "You
        // notice ..." listing name the SAME pile, and without this a
        // kill sent `get gold` here, then the very next block re-listed
        // the pile and the room-render path sent it again. `insert`
        // returns false when the denomination is already claimed, so
        // whichever source sees the pile first wins and the other is a
        // no-op.
        //
        // This does not key on room name the way the render path does —
        // a drop line only ever arrives mid-visit, after the RoomSeen
        // that keyed `swept.0` for the room we are standing in. The one
        // gap is a bot whose very first event is a drop line with no
        // room seen yet: the memo is still keyed to the empty string,
        // and the next RoomSeen resets it, undoing this claim and
        // allowing one repeat `get`. That is the same reset the memo
        // already performs on every room change; it does not survive
        // being asked about a room it has not seen yet.
        coin_drop(line)
            .filter(|(_, denom)| self.swept.1.insert(denom.clone()))
            .map(|(_, denom)| vec![BotAction::Send(format!("get {denom}"))])
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
