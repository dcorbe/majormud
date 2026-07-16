//! Every player-visible string the engine emits, in one place.
//!
//! Strings marked `VERIFIED` were read out of WCCMMUD.DLL; strings marked
//! `ORACLE-VERIFY` are placeholders to be corrected against MBBSEmu
//! transcripts. Keeping them here makes those corrections one-line diffs.

/// VERIFIED (DLL): broadcast when a player enters the game.
pub fn entered_realm(name: &str) -> String {
    format!("{name} just entered the Realm.")
}

/// VERIFIED (DLL): broadcast when a player leaves the game.
pub fn left_realm(name: &str) -> String {
    format!("{name} just left the Realm.")
}

/// ORACLE-VERIFY: response to input matching no command. The string is not in
/// WCCMMUD.DLL (host-side dispatch), so the wording needs a live transcript.
pub const COMMAND_NOT_UNDERSTOOD: &str = "Your command was not understood.";

/// VERIFIED (DLL): moving where no exit exists.
pub const NO_EXIT: &str = "There is no exit in that direction!";

/// VERIFIED (DLL): the exits-line prefix and empty-exits marker.
pub const OBVIOUS_EXITS: &str = "Obvious exits: ";
pub const NO_EXITS: &str = "NONE!!!";

/// VERIFIED (DLL): the occupant-line prefix.
pub const ALSO_HERE: &str = "Also here: ";

use crate::content::Direction;

/// Display name used in the exits list and arrival broadcasts. Up/down show
/// as "above"/"below" (VERIFIED: DLL exit string table).
pub fn direction_shown(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::East => "east",
        Direction::West => "west",
        Direction::NorthEast => "northeast",
        Direction::NorthWest => "northwest",
        Direction::SouthEast => "southeast",
        Direction::SouthWest => "southwest",
        Direction::Up => "above",
        Direction::Down => "below",
    }
}

/// VERIFIED (DLL): departure broadcast. Compass exits use
/// "just left to the <dir>."; vertical exits have dedicated phrasings.
pub fn left_via(name: &str, direction: Direction) -> String {
    match direction {
        Direction::Up => format!("{name} just left upwards."),
        Direction::Down => format!("{name} just left downwards."),
        d => format!("{name} just left to the {}.", direction_shown(d)),
    }
}

/// VERIFIED (DLL) format string; ORACLE-VERIFY for vertical arrivals
/// ("arrived from the above/below" is presumed).
pub fn arrived_from(name: &str, direction: Direction) -> String {
    format!("{name} just arrived from the {}.", direction_shown(direction))
}
