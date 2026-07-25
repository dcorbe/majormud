//! Bot policy engine — the MegaMud toggle set (AutoCombat, AutoHeal,
//! AutoGet, AutoFlee) as a pure decision core: events in, commands out.
//! The async runner glues it to a [`crate::session::Session`].

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::events::Event;

// Coins print "5 copper drop to the ground." — plural verb, full stop.
// An actor collapsing prints "Vexil drops to the ground!", which the
// leading count and the verb keep out of this pattern.
static COIN_DROP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+ (\w+) drop to the ground\.$").unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BotConfig {
    pub auto_combat: bool,
    pub auto_heal: bool,
    pub auto_get: bool,
    pub auto_flee: bool,
    /// Heal when hp% drops below this (needs `max_hp`).
    pub heal_at_percent: u32,
    /// Flee when hp% drops below this (needs `max_hp`).
    pub flee_at_percent: u32,
    /// Command issued to recover (rest, cast a heal, quaff...).
    pub heal_command: String,
    /// Names never attacked.
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

pub struct Bot {
    config: BotConfig,
    /// Name currently under attack; cleared once it is out of the room.
    engaged: Option<String>,
    /// Exits from the most recent room block — the flee routes.
    exits: Vec<String>,
    /// A heal is already in flight; suppresses one per regen tick.
    healing: bool,
}

impl Bot {
    pub fn new(config: BotConfig) -> Self {
        Bot {
            config,
            engaged: None,
            exits: Vec::new(),
            healing: false,
        }
    }

    /// Feed one parsed event; returns the commands to send now.
    pub fn on_event(&mut self, ev: &Event) -> Vec<BotAction> {
        match ev {
            Event::RoomSeen(room) => {
                self.exits = room.exits.clone();
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
            Event::Prompt { hp, .. } => self.on_hp(*hp),
            Event::Line(line) => self.on_line(line),
            _ => Vec::new(),
        }
    }

    /// Attack `name`, unless combat is off, a target is already engaged,
    /// or the name is on the ignore list.
    fn engage(&mut self, name: &str) -> Option<BotAction> {
        if !self.config.auto_combat
            || self.engaged.is_some()
            || self.config.ignore.iter().any(|i| i == name)
        {
            return None;
        }
        self.engaged = Some(name.to_string());
        Some(BotAction::Send(format!("a {name}")))
    }

    /// Percent-of-max policies. Flee outranks heal: staying to heal is
    /// what gets a character killed.
    fn on_hp(&mut self, hp: i32) -> Vec<BotAction> {
        if self.config.max_hp <= 0 {
            return Vec::new();
        }
        let percent = hp * 100 / self.config.max_hp;
        if self.config.auto_flee
            && percent < self.config.flee_at_percent as i32
            && let Some(exit) = self.exits.first()
        {
            return vec![BotAction::Send(exit.clone())];
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
        if !self.config.auto_get {
            return Vec::new();
        }
        COIN_DROP_RE
            .captures(line)
            .map(|c| vec![BotAction::Send(format!("get {}", &c[1]))])
            .unwrap_or_default()
    }
}
