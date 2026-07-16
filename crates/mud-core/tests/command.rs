//! Tests for the in-game command parser.
//!
//! M1 command set: movement, look, quit. Matching semantics: case-insensitive;
//! single-letter direction aliases are exact; everything else prefix-matches
//! against the verb table in precedence order (first match wins).

use mud_core::command::{parse, Command};
use mud_core::content::Direction;

#[test]
fn single_letter_direction_aliases() {
    assert_eq!(parse("n"), Command::Move(Direction::North));
    assert_eq!(parse("s"), Command::Move(Direction::South));
    assert_eq!(parse("e"), Command::Move(Direction::East));
    assert_eq!(parse("w"), Command::Move(Direction::West));
    assert_eq!(parse("ne"), Command::Move(Direction::NorthEast));
    assert_eq!(parse("nw"), Command::Move(Direction::NorthWest));
    assert_eq!(parse("se"), Command::Move(Direction::SouthEast));
    assert_eq!(parse("sw"), Command::Move(Direction::SouthWest));
    assert_eq!(parse("u"), Command::Move(Direction::Up));
    assert_eq!(parse("d"), Command::Move(Direction::Down));
}

#[test]
fn full_direction_words() {
    assert_eq!(parse("north"), Command::Move(Direction::North));
    assert_eq!(parse("southwest"), Command::Move(Direction::SouthWest));
    assert_eq!(parse("up"), Command::Move(Direction::Up));
    assert_eq!(parse("down"), Command::Move(Direction::Down));
}

#[test]
fn matching_is_case_insensitive() {
    assert_eq!(parse("NORTH"), Command::Move(Direction::North));
    assert_eq!(parse("Look"), Command::Look);
}

#[test]
fn prefix_matches_resolve_in_precedence_order() {
    // "no" is a prefix of "north" only.
    assert_eq!(parse("no"), Command::Move(Direction::North));
    // "sou" prefixes "south" before "southeast"/"southwest" (table order).
    assert_eq!(parse("sou"), Command::Move(Direction::South));
    assert_eq!(parse("lo"), Command::Look);
}

#[test]
fn look_and_quit() {
    assert_eq!(parse("look"), Command::Look);
    assert_eq!(parse("l"), Command::Look);
    assert_eq!(parse("quit"), Command::Quit);
    assert_eq!(parse("x"), Command::Quit); // the original's exit alias
}

#[test]
fn whitespace_is_tolerated() {
    assert_eq!(parse("  north  "), Command::Move(Direction::North));
    assert_eq!(parse(""), Command::Blank);
    assert_eq!(parse("   "), Command::Blank);
}

#[test]
fn unknown_input_is_reported_verbatim() {
    assert_eq!(parse("xyzzy"), Command::Unknown("xyzzy".into()));
}
