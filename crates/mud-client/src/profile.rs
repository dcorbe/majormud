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

impl Profile {
    /// The same profile with send pacing off, for interactive use.
    ///
    /// Pacing is flood control, and flood control is for automation: the
    /// measured limit is eight sends 1.3s apart, which a bot walking a
    /// circuit will hit and a person typing will not. Applying a
    /// farm-tuned `pace_ms` to `mmc play` delays every command after the
    /// first in a burst by the full interval — 2.5s per step when
    /// walking — for a risk the operator can see and react to anyway.
    pub fn interactive(&self) -> Profile {
        Profile {
            pace_ms: Some(0),
            ..self.clone()
        }
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
