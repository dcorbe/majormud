//! `/recover`: sneak to the room the character died in, search once,
//! pick up everything the search lists, and run back to where the job
//! started. It never attacks, and it turns for home at the first broken
//! sneak.
//!
//! The run home needs no sneak. Aggressive monsters acquire a target
//! inside the combat round and skip a player who moved this round, and
//! pursuit refuses the same player, so a character that keeps moving is
//! neither acquired nor followed. What it needs is no pauses, which is
//! why the walk home is unsneaked: arming costs a round standing still.
//!
//! The pure parts, what the sweep asks for and how an ending reads, sit
//! above the job so they are tested without a board.

use mud_core::content::RoomId;

use crate::farm::Phase;

/// One thing the sweep will ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Take {
    /// The entry as the board listed it, for the report.
    pub label: String,
    /// The `get` that asks for it.
    pub cmd: String,
    pub kind: TakeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeKind {
    Gear,
    Coins,
}

/// The "You notice ... here." entries as `get` commands, gear first and
/// coins last, so a forced exit leaves coins behind rather than
/// equipment.
///
/// A coin pile is taken by denomination, as `get` takes it. Everything
/// else is an item, taken by its printed name with the article dropped:
/// the board matches by word prefix and the printed name is what it
/// printed, so nothing is gained by resolving it first.
pub fn sweep_list(items: &[String]) -> Vec<Take> {
    let mut gear = Vec::new();
    let mut coins = Vec::new();
    for entry in items {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match crate::bot::coin_pile(entry) {
            Some((_, denom)) => coins.push(Take {
                label: entry.to_string(),
                cmd: format!("get {denom}"),
                kind: TakeKind::Coins,
            }),
            None => gear.push(Take {
                label: entry.to_string(),
                cmd: format!("get {}", strip_article(entry)),
                kind: TakeKind::Gear,
            }),
        }
    }
    gear.extend(coins);
    gear
}

fn strip_article(entry: &str) -> &str {
    for article in ["a ", "an ", "the "] {
        if let Some(rest) = entry.strip_prefix(article) {
            return rest.trim();
        }
    }
    entry
}

/// What one line of a `get` reply said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetReply {
    /// The board confirmed the pickup, coins or item.
    Taken,
    /// Somebody else got there first: "You don't see <name> here." or
    /// "You don't see any <plural>". Excludes `rob`'s missing-target
    /// refusal, "You don't see that anywhere!", which shares no `get`.
    Gone,
    Other,
}

pub fn read_get_reply(line: &str) -> GetReply {
    if crate::bot::picked_up(line).is_some() || crate::bot::picked_up_item(line).is_some() {
        return GetReply::Taken;
    }
    let lower = line.to_lowercase();
    if (lower.contains("you don't see") && lower.contains(" here"))
        || lower.contains("you don't see any ")
    {
        return GetReply::Gone;
    }
    GetReply::Other
}

/// What the sweep asked for and what it got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Haul {
    /// The entries taken, as the board listed them, in the order taken.
    pub taken: Vec<String>,
    pub items_wanted: usize,
    pub items_taken: usize,
    pub coins_wanted: usize,
    pub coins_taken: usize,
}

impl Haul {
    pub fn wanted(list: &[Take]) -> Haul {
        Haul {
            items_wanted: list.iter().filter(|t| t.kind == TakeKind::Gear).count(),
            coins_wanted: list.iter().filter(|t| t.kind == TakeKind::Coins).count(),
            ..Haul::default()
        }
    }

    pub fn took(&mut self, take: &Take) {
        match take.kind {
            TakeKind::Gear => self.items_taken += 1,
            TakeKind::Coins => self.coins_taken += 1,
        }
        self.taken.push(take.label.clone());
    }

    /// `3 of 7 items`, with ` and 2 coin piles` when any coins were
    /// listed.
    pub fn summary(&self) -> String {
        let mut s = format!("{} of {} items", self.items_taken, self.items_wanted);
        if self.coins_wanted > 0 {
            s.push_str(&format!(" and {} coin piles", self.coins_taken));
        }
        s
    }
}

/// Why the character came home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeWhy {
    Swept,
    /// The search listed nothing.
    Nothing,
    /// A hop on the way in arrived without `Sneaking...`.
    Broke { at: RoomId, name: String },
    /// The board refused a move on the way in because something had
    /// the character in combat.
    Attacked { at: RoomId, name: String },
    /// Hitpoints fell under the minor heal mark during the sweep.
    Hurt { mark: u32 },
}

/// How the job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverEnd {
    /// Back in the start room.
    Home { at: RoomId, why: HomeWhy, haul: Haul },
    /// The walk home did not complete. Where the character stands.
    Stopped { at: RoomId, name: String, haul: Haul },
    Died { haul: Haul },
}

impl RecoverEnd {
    pub fn haul(&self) -> &Haul {
        match self {
            RecoverEnd::Home { haul, .. }
            | RecoverEnd::Stopped { haul, .. }
            | RecoverEnd::Died { haul } => haul,
        }
    }

    /// The ending as the bar and the lobby read it.
    pub fn phase(&self) -> Phase {
        match self {
            RecoverEnd::Home { at, why, haul } => Phase::Done {
                why: match why {
                    HomeWhy::Swept => format!("recovered {}", haul.summary()),
                    HomeWhy::Nothing => "nothing there".into(),
                    HomeWhy::Broke { at, name } => {
                        format!("sneak broke at {}/{} {name}, nothing taken", at.map, at.room)
                    }
                    HomeWhy::Attacked { at, name } => {
                        format!("attacked at {}/{} {name}, nothing taken", at.map, at.room)
                    }
                    HomeWhy::Hurt { mark } => {
                        format!("hurt under {mark}%, back with {}", haul.summary())
                    }
                },
                at: Some(*at),
            },
            RecoverEnd::Stopped { at, name, haul } => Phase::Done {
                why: format!(
                    "stopped at {}/{} {name} with {}",
                    at.map,
                    at.room,
                    haul.summary()
                ),
                at: Some(*at),
            },
            RecoverEnd::Died { .. } => Phase::Done {
                why: crate::farm::DIED.into(),
                at: None,
            },
        }
    }
}

