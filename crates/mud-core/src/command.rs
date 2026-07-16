//! The in-game command parser.
//!
//! Matching semantics: case-insensitive; single-letter direction aliases are
//! exact matches; other verbs prefix-match against the table in precedence
//! order, first match wins. The M1 verb set covers movement, look, and quit;
//! the table grows with each milestone. Parser behavior is oracle-checked
//! (matching precedence is player-visible).

use crate::content::Direction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Move(Direction),
    Look,
    Quit,
    Blank,
    Unknown(String),
}

/// Exact-match aliases, checked before the verb table.
const ALIASES: [(&str, Command); 12] = [
    ("n", Command::Move(Direction::North)),
    ("s", Command::Move(Direction::South)),
    ("e", Command::Move(Direction::East)),
    ("w", Command::Move(Direction::West)),
    ("ne", Command::Move(Direction::NorthEast)),
    ("nw", Command::Move(Direction::NorthWest)),
    ("se", Command::Move(Direction::SouthEast)),
    ("sw", Command::Move(Direction::SouthWest)),
    ("u", Command::Move(Direction::Up)),
    ("d", Command::Move(Direction::Down)),
    ("l", Command::Look),
    ("x", Command::Quit),
];

/// Prefix-matched verbs, in precedence order.
const VERBS: [(&str, Command); 13] = [
    ("north", Command::Move(Direction::North)),
    ("south", Command::Move(Direction::South)),
    ("east", Command::Move(Direction::East)),
    ("west", Command::Move(Direction::West)),
    ("northeast", Command::Move(Direction::NorthEast)),
    ("northwest", Command::Move(Direction::NorthWest)),
    ("southeast", Command::Move(Direction::SouthEast)),
    ("southwest", Command::Move(Direction::SouthWest)),
    ("up", Command::Move(Direction::Up)),
    ("down", Command::Move(Direction::Down)),
    ("look", Command::Look),
    ("quit", Command::Quit),
    ("exit", Command::Quit),
];

pub fn parse(input: &str) -> Command {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Command::Blank;
    }
    let verb = trimmed
        .split_whitespace()
        .next()
        .expect("non-empty after trim")
        .to_ascii_lowercase();

    for (alias, command) in &ALIASES {
        if verb == *alias {
            return command.clone();
        }
    }
    for (name, command) in &VERBS {
        if name.starts_with(&verb) {
            return command.clone();
        }
    }
    Command::Unknown(trimmed.to_string())
}
