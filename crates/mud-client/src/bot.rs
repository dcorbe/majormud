//! Bot policy engine — the MegaMud toggle set (AutoCombat, AutoHeal,
//! AutoGet, AutoFlee) as a pure decision core: events in, commands out.
//! The async runner glues it to a [`crate::session::Session`].

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::events::Event;

// Coins print "5 copper drop to the ground.". The leading count is what
// separates loot from a downed actor's "Vexil drops to the ground!";
// the plural verb and the full stop corroborate it.
static COIN_DROP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+ (\w+) drop to the ground\.$").unwrap());

/// Tail shared by every monster death line ("The kobold thief falls to
/// the ground with a shrill cry."); the wording after it is per-template.
/// A player's death reads "<name> is dead." and is deliberately not
/// matched — it does not end our fight.
const DEATH_MARK: &str = "falls to the ground";

/// The line that follows one of our kills ("You gain %s experience.",
/// DLL 0xbc65f).
///
/// This carries the weight [`DEATH_MARK`] cannot. Death lines are
/// per-template PROSE — of the 1085 monsters shipping a death record,
/// 67 say "falls to the ground" and 1018 say something else entirely
/// ("The filthbug collapses, its legs curling tightly around it.", "The
/// skeleton crumbles into a pile of dust."). Matching them all would mean
/// carrying a thousand strings; the experience line is one, and the board
/// prints it every time a kill of ours pays out.
const EXP_MARK: (&str, &str) = ("you gain ", " experience");

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
    /// Pick up dropped coins. Floor *items* are not covered: the board
    /// drops carried loot silently, so nothing announces them.
    pub auto_get: bool,
    pub auto_flee: bool,
    /// Heal when hp% drops below this (needs `max_hp`).
    pub heal_at_percent: u32,
    /// Flee when hp% drops below this (needs `max_hp`).
    pub flee_at_percent: u32,
    /// Command issued to recover (rest, cast a heal, quaff...).
    pub heal_command: String,
    /// Never attacked. Matched as a substring, so "guardsman" also
    /// covers the rolled "fat guardsman".
    pub ignore: Vec<String>,
    /// Character max HP; 0 = unknown, disables percent policies.
    pub max_hp: i32,
    /// Prompts with no blow struck either way before the fight is
    /// presumed over and the target released.
    ///
    /// The backstop for an ending nothing else recognises: an exp-less
    /// kill, a monster somebody else finished, or one of the 14 templates
    /// that ship with no death record at all. Without it the bot can sit
    /// latched on a corpse forever, and the farm runner reads a latched
    /// bot as a fight in progress — it stops poking the room and stops
    /// counting the stop as idle, so the stop never ends.
    ///
    /// Too low abandons a slow fight, which is worse than the hang this
    /// guards against: `farm_stop` reads a cleared latch as an idle room,
    /// dwells out, and walks off mid-fight.
    ///
    /// It has to be generous because prompts do NOT arrive one per combat
    /// round. They come in bursts — async output disturbs the dangling
    /// prompt and the board re-prompts, so "[HP=31]:[HP=32]:" lands on
    /// one physical line — and a cave bear is 50hp against single-digit
    /// hits, so rounds are seconds apart. Three was measured leaving a
    /// fight after a single burst.
    ///
    /// Being generous costs almost nothing, because this is a BACKSTOP,
    /// not the primary mechanism: the room block from the runner's idle
    /// `look` clears the latch properly by finding the target gone. This
    /// only has to cover the case where no room block is coming.
    pub combat_idle_prompts: u32,
}

impl Default for BotConfig {
    fn default() -> Self {
        BotConfig {
            auto_combat: false,
            auto_heal: false,
            auto_get: false,
            auto_flee: false,
            heal_at_percent: 50,
            flee_at_percent: 25,
            heal_command: "rest".into(),
            ignore: Vec::new(),
            max_hp: 0,
            combat_idle_prompts: 12,
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
fn target_word(name: &str) -> &str {
    let name = strip_status(name);
    name.split_whitespace().last().unwrap_or(name)
}

/// Exits render as a display token, not a command — "closed door north",
/// "closed gate west". The direction is the last word.
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
/// DEFENCE IN DEPTH, not a diagnosis. ` (Resting) ` is a decoration the
/// board splices into a line (DLL 0xe06f6, spaces included) and it was
/// seen attached to a monster's name in an `ActorEntered` — "+  (Resting)
/// fierce filthbug", with the tell-tale doubled space. Monsters do not
/// rest, so that marker was almost certainly the PLAYER's own status
/// bleeding into the actor name upstream, in the parser.
///
/// Fixing it here is still worth doing, because the case rule is the only
/// player/monster discriminator there is and a leading "(" defeats it
/// entirely — but the real bug is wherever the marker got glued on, and
/// this does not address that. Stripping must not smuggle a player
/// through, so the case test still runs, on the name rather than the
/// bracket.
fn strip_status(name: &str) -> &str {
    name.strip_prefix('(')
        .and_then(|rest| rest.split_once(')'))
        .map(|(_, after)| after.trim_start())
        .unwrap_or(name)
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
    /// Targets the board refused to let us attack, keyed by the same
    /// trailing noun the attack command uses — the refusal applies to the
    /// template, so every rolled variant ("fat kobold thief") is covered
    /// by the one entry. Without this the cleared latch would simply
    /// re-engage on the next room block and be refused again forever.
    refused: HashSet<String>,
}

impl Bot {
    pub fn new(config: BotConfig) -> Self {
        Bot::with_threat(config, std::sync::Arc::new(ThreatTable::new()))
    }

    /// As [`Bot::new`], but able to tell a cave bear from a giant rat.
    pub fn with_threat(config: BotConfig, threat: std::sync::Arc<ThreatTable>) -> Self {
        Bot {
            config,
            threat,
            engaged: None,
            exits: Vec::new(),
            healing: false,
            fled: false,
            quiet_prompts: 0,
            refused: HashSet::new(),
        }
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

    /// Feed one parsed event; returns the commands to send now.
    pub fn on_event(&mut self, ev: &Event) -> Vec<BotAction> {
        match ev {
            Event::RoomSeen(room) => {
                self.exits = room.exits.clone();
                // Somewhere new: running away is allowed again.
                self.fled = false;
                if self
                    .engaged
                    .as_ref()
                    .is_some_and(|t| !room.also_here.contains(t))
                {
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
                let target = room
                    .also_here
                    .iter()
                    .enumerate()
                    .filter(|(_, name)| self.attackable(name))
                    .max_by_key(|(i, name)| (self.threat_of(name), std::cmp::Reverse(*i)))
                    .map(|(_, name)| name.clone());
                target
                    .and_then(|name| self.engage(&name))
                    .into_iter()
                    .collect()
            }
            Event::ActorEntered { name, .. } => self.engage(name).into_iter().collect(),
            Event::ActorLeft { name, .. } => {
                if self.engaged.as_deref() == Some(name.as_str()) {
                    self.engaged = None;
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

    /// Is this something we would swing at at all? Split out of
    /// [`Bot::engage`] so candidates can be ranked before one is chosen,
    /// rather than the first acceptable name winning by position.
    fn attackable(&self, name: &str) -> bool {
        self.config.auto_combat
            && self.engaged.is_none()
            && is_attackable(name)
            && !self.refused.contains(target_word(name))
            && !self.config.ignore.iter().any(|i| name.contains(i.as_str()))
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
    fn engage(&mut self, name: &str) -> Option<BotAction> {
        if !self.attackable(name) {
            return None;
        }
        self.engaged = Some(name.to_string());
        self.quiet_prompts = 0;
        Some(BotAction::Send(format!("a {}", target_word(name))))
    }

    /// Percent-of-max policies. Flee outranks heal: staying to heal is
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
    fn on_hp(&mut self, hp: i32) -> Vec<BotAction> {
        // Downed: commands do not land, and HP reads negative.
        if self.config.max_hp <= 0 || hp <= 0 {
            return Vec::new();
        }
        let percent = hp * 100 / self.config.max_hp;
        if self.config.auto_flee
            && percent < self.config.flee_at_percent as i32
            && !self.fled
            && let Some(exit) = self.exits.first()
        {
            self.fled = true;
            return vec![BotAction::Send(exit_command(exit).to_string())];
        }
        if percent >= self.config.heal_at_percent as i32 {
            self.healing = false;
        } else if self.config.auto_heal && !self.healing {
            self.healing = true;
            return vec![BotAction::Send(self.config.heal_command.clone())];
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
        // on a corpse is not.
        let lower = line.to_lowercase();
        if line.contains(DEATH_MARK)
            || (lower.contains(EXP_MARK.0) && lower.contains(EXP_MARK.1))
        {
            self.engaged = None;
            self.quiet_prompts = 0;
        }
        // The refusal names no monster, so the target is whichever one we
        // just swung at.
        if ATTACK_REFUSALS.iter().any(|r| line.contains(r))
            && let Some(name) = self.engaged.take()
        {
            self.refused.insert(target_word(&name).to_string());
        }
        if !self.config.auto_get {
            return Vec::new();
        }
        COIN_DROP_RE
            .captures(line)
            .map(|c| vec![BotAction::Send(format!("get {}", &c[1]))])
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
