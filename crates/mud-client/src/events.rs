//! Parsed game events. The parser turns the cleaned line stream into
//! these; bot, scripts, and TUI consume them without touching wire
//! formats.

/// Who did something in a combat line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    You,
    /// Name as printed, article included ("The kobold thief").
    Other(String),
}

/// One rendered room block (name through exits line).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoomView {
    pub name: String,
    /// Exit tokens as printed after backspace resolution, e.g. "north",
    /// "closed door north", "up". Empty when the board printed NONE!!!.
    pub exits: Vec<String>,
    /// Names from "Also here: ..." (without the trailing period).
    pub also_here: Vec<String>,
    /// Items/coins from "You notice ... here.".
    pub items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    RoomSeen(RoomView),
    /// `[HP=35]:` / `[HP=26/MA=12]:` / `[HP=20/KAI=4]:` — HP may be
    /// negative while downed.
    Prompt { hp: i32, mana: Option<i32> },
    CombatHit {
        attacker: Actor,
        target: Actor,
        damage: i32,
    },
    /// A swing that did no damage (miss, glance, dodge, deflect).
    CombatMiss { line: String },
    ActorEntered {
        name: String,
        /// Direction word as printed ("east", "above", "below",
        /// "nowhere"); None when the line gave no origin.
        from: Option<String>,
    },
    ActorLeft {
        name: String,
        to: Option<String>,
    },
    /// Flood control tripped: "Why don't you slow down for a few
    /// seconds?" — input above this point was DROPPED by the board.
    SlowDown,
    /// Any line the classifier does not recognize (cleaned text).
    /// Never silently dropped.
    Line(String),
}
