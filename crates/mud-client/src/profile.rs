//! Per-character TOML profiles — the analog of MegaMud `Chars/*.Ini`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::bot::BotConfig;
use crate::dialect::Target;
use crate::farm::FarmConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub target: Target,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Minimum milliseconds between sends. Defaults by target: 1500 for
    /// the live board (flood control), 0 for the Rust server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace_ms: Option<u64>,
    /// Turn the character's Warn-on-Evil flag OFF at login.
    ///
    /// The board refuses attacks on unprovoked (behaviour 0/4) monsters
    /// while this warning is on, which is most of the shipped bestiary.
    /// Opt-in and deliberately not a `[bot]` toggle: it mutates
    /// PERSISTENT character state, accruing fame and moving the legal
    /// level toward Criminal, so it applies to interactive play just as
    /// much as to a farm run.
    ///
    /// Must precede the `[bot]`/`[farm]` tables in this struct — TOML
    /// puts every bare key before the first table header, so a scalar
    /// declared after them would serialise INTO one.
    #[serde(default)]
    pub disable_evil_warnings: bool,
    /// Bot policy toggles. Absent means every toggle off — a patrol that
    /// walks its circuit and fights nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<BotConfig>,
    /// Patrol circuit for `mmc farm`. Absent means the character has no
    /// farming route configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm: Option<FarmConfig>,
}

/// Keys that were renamed, and what they are called now. Deserialisation
/// accepts both (`#[serde(alias)]` on the fields), so this exists only to
/// SAY SO — a profile that keeps working while its vocabulary has moved
/// on is a profile whose owner never finds out about the new knob next to
/// it. Same habit as the dark-stop warning in `FarmPlan::build`: a
/// warning, never a refusal.
const RENAMED_KEYS: [(&str, &str); 4] = [
    ("heal_at_percent", "rest_at_percent"),
    ("heal_command", "rest_command"),
    ("spell_at_percent", "minor_heal_at_percent"),
    ("heal_spells", "minor_heal_spell and major_heal_spell"),
];

impl Profile {
    /// Read and parse a profile, reporting renamed keys on stderr.
    ///
    /// The three call sites (`play`, `run`, `farm`) had this open-coded
    /// identically; the error string is the caller's to print, because
    /// only the caller knows whether it is fatal.
    pub fn load(path: &std::path::Path) -> Result<Profile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut profile: Profile = toml::from_str(&text).map_err(|e| e.to_string())?;
        for line in text.lines() {
            let key = line.split('=').next().unwrap_or_default().trim();
            if let Some((_, new)) = RENAMED_KEYS.iter().find(|(old, _)| *old == key) {
                eprintln!(
                    "profile {}: `{key}` is now `{new}` (still accepted)",
                    path.display()
                );
            }
        }
        if let Some(bot) = &mut profile.bot {
            bot.normalise();
            bot.validate()?;
        }
        Ok(profile)
    }

    pub fn pace(&self) -> Duration {
        match self.pace_ms {
            Some(ms) => Duration::from_millis(ms),
            None => match self.target {
                Target::MbbsEmu => Duration::from_millis(1500),
                Target::RustServer => Duration::ZERO,
            },
        }
    }
}
