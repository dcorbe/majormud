//! The in-game command parser.
//!
//! Design philosophy (player testimony, oracle-reconciled): **best-effort
//! matching everywhere, say as the universal fallback.** Verbs prefix-match
//! against the table in precedence order down to a single letter ("a" =
//! attack, "at" = attack, "ai" = aid); argument words are resolved by the
//! game layer with the same word-prefix leniency ("a kob th", "get sil");
//! and whenever the engine cannot intuit what the player meant — unknown
//! verb OR unresolvable argument — the whole line is spoken aloud instead
//! of erroring. Handlers signal argument-resolution failure by returning
//! [`Resolution::FallThrough`]; `Core::game_command` owns the say fallback.

use crate::content::Direction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Move(Direction),
    Look,
    Status,
    Experience,
    Health,
    Train,
    /// `attack [target]` — empty target means auto-pick.
    Attack(String),
    /// `aid <player>` — stabilize a downed player.
    Aid(String),
    Quit,
    Blank,
    Unknown(String),
}

/// What an argument-taking handler did with its input. `FallThrough` makes
/// the dispatcher say the raw line (the parser's universal fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Handled,
    FallThrough,
}

/// Exact-match aliases, checked before the verb table (single/double letter
/// shortcuts that must not be shadowed by prefix matching).
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

/// Verb constructors for the precedence table.
#[derive(Clone, Copy)]
enum Verb {
    Plain(fn() -> Command),
    /// Takes the rest of the line as an argument (may be empty).
    WithArgs(fn(String) -> Command),
}

/// Prefix-matched verbs in precedence order: any prefix of a name matches,
/// first entry wins ("a" → attack because attack precedes aid).
const VERBS: [(&str, Verb); 21] = [
    ("north", Verb::Plain(|| Command::Move(Direction::North))),
    ("south", Verb::Plain(|| Command::Move(Direction::South))),
    ("east", Verb::Plain(|| Command::Move(Direction::East))),
    ("west", Verb::Plain(|| Command::Move(Direction::West))),
    ("northeast", Verb::Plain(|| Command::Move(Direction::NorthEast))),
    ("northwest", Verb::Plain(|| Command::Move(Direction::NorthWest))),
    ("southeast", Verb::Plain(|| Command::Move(Direction::SouthEast))),
    ("southwest", Verb::Plain(|| Command::Move(Direction::SouthWest))),
    ("up", Verb::Plain(|| Command::Move(Direction::Up))),
    ("down", Verb::Plain(|| Command::Move(Direction::Down))),
    ("attack", Verb::WithArgs(Command::Attack)),
    ("aid", Verb::WithArgs(Command::Aid)),
    ("look", Verb::Plain(|| Command::Look)),
    ("status", Verb::Plain(|| Command::Status)),
    ("stat", Verb::Plain(|| Command::Status)),
    ("experience", Verb::Plain(|| Command::Experience)),
    ("exp", Verb::Plain(|| Command::Experience)),
    ("health", Verb::Plain(|| Command::Health)),
    ("train", Verb::Plain(|| Command::Train)),
    ("quit", Verb::Plain(|| Command::Quit)),
    ("exit", Verb::Plain(|| Command::Quit)),
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
    for (name, kind) in &VERBS {
        if name.starts_with(&verb) {
            return match kind {
                Verb::Plain(make) => make(),
                Verb::WithArgs(make) => make(trimmed[verb.len()..].trim().to_string()),
            };
        }
    }
    Command::Unknown(trimmed.to_string())
}
