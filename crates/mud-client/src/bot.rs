//! Bot policy engine — the MegaMud toggle set (AutoCombat, AutoHeal,
//! AutoGet, AutoFlee) as a pure decision core: events in, commands out.
//! The async runner glues it to a [`crate::session::Session`].

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
/// are candidates.
fn is_attackable(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_lowercase)
}

pub struct Bot {
    config: BotConfig,
    /// Name currently under attack; cleared once it is gone.
    engaged: Option<String>,
    /// Exits from the most recent room block — the flee routes.
    exits: Vec<String>,
    /// A heal is already in flight; suppresses one per regen tick.
    healing: bool,
    /// Already fled this room; suppresses one per regen tick.
    fled: bool,
}

impl Bot {
    pub fn new(config: BotConfig) -> Self {
        Bot {
            config,
            engaged: None,
            exits: Vec::new(),
            healing: false,
            fled: false,
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
                room.also_here
                    .iter()
                    .find_map(|name| self.engage(name))
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
            Event::Prompt { hp, .. } => self.on_hp(*hp),
            Event::Line(line) => self.on_line(line),
            _ => Vec::new(),
        }
    }

    /// Attack `name`, unless combat is off, a target is already engaged,
    /// the name is not a monster, or it is on the ignore list.
    fn engage(&mut self, name: &str) -> Option<BotAction> {
        if !self.config.auto_combat
            || self.engaged.is_some()
            || !is_attackable(name)
            || self.config.ignore.iter().any(|i| name.contains(i.as_str()))
        {
            return None;
        }
        self.engaged = Some(name.to_string());
        Some(BotAction::Send(format!("a {}", target_word(name))))
    }

    /// Percent-of-max policies. Flee outranks heal: staying to heal is
    /// what gets a character killed. Both fire once and re-arm on a
    /// change of situation — prompts reprint on every regen tick (and
    /// can double up on one line), so an undebounced policy floods.
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

    fn on_line(&mut self, line: &str) -> Vec<BotAction> {
        // The fight ended: re-arm so the next arrival is engaged. Death
        // lines name the template, not the rolled instance, so any death
        // clears — a redundant re-attack is harmless, a permanent latch
        // on a corpse is not.
        if line.contains(DEATH_MARK) {
            self.engaged = None;
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
