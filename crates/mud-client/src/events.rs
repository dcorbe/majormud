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

/// Drop the leading article a message template glues onto a monster name
/// ("The thin giant rat" -> "thin giant rat"). The article belongs to
/// the per-monster template, not the name: "Also here:" and the attack
/// command both use the bare name, and the article's capital defeats the
/// player/monster case rule. Player names never start with one.
pub fn strip_article(name: &str) -> &str {
    ["A ", "An ", "The "]
        .iter()
        .find_map(|art| name.strip_prefix(art))
        .unwrap_or(name)
}

/// Does this line say something about US — "you" or "your" as a whole
/// word? Word-split, not substring, so "at young girl" does not count.
pub fn mentions_you(line: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w == "you" || w == "your")
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
    /// The SGR each `also_here` name was painted in, same length and
    /// order, `None` for a name that carried no colour of its own.
    ///
    /// The board paints occupants by what they ARE, and it is the only
    /// signal that says so: bright magenta `1;35` is an aggressive
    /// monster (or a player — those are capitalised), cyan `0;36` is
    /// passive townsfolk and animals, white `0;37` is guards, healers
    /// and named NPCs. Measured across six live captures, 67 distinct
    /// names, zero counterexamples in either direction. See
    /// [`crate::bot::Bot::aggressive_here`].
    ///
    /// Empty when the block was parsed without colour at all — an
    /// unpainted board, or a fixture — which callers must read as "no
    /// opinion" rather than as "nothing is aggressive".
    pub also_here_sgr: Vec<Option<String>>,
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
