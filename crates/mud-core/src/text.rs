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
