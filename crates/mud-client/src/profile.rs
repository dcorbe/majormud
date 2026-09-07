//! Per-character TOML profiles — the analog of MegaMud `Chars/*.Ini`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::bot::BotConfig;
use crate::dialect::Target;
use crate::farm::FarmConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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
    /// Lines a window keeps above its screen for PageUp. A window's
    /// terminal is sized when the window opens, so a change applies to
    /// the next `/new`.
    ///
    /// Before the tables for the same reason as the field above.
    #[serde(default = "default_scrollback")]
    pub scrollback_lines: u32,
    /// Dial the board again and log back in when the line closes on its
    /// own. Off by default, and it needs both `username` and `password`,
    /// because interactive play never types them for you.
    ///
    /// Before the tables for the same reason as the fields above.
    #[serde(default)]
    pub reconnect: bool,
    /// Seconds to wait before each redial. Ten by default, which is
    /// enough for a board that is restarting to finish.
    #[serde(default = "default_reconnect_delay")]
    pub reconnect_delay_seconds: u64,
    /// Bot policy toggles. Absent means every toggle off — a patrol that
    /// walks its circuit and fights nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<BotConfig>,
    /// Patrol circuit for `mmc farm`. Absent means the character has no
    /// farming route configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm: Option<FarmConfig>,
    /// Deposit policy, as `[bank]`. Absent means the defaults, which
    /// deposit at a thousand coins or a weight class crossing, keep
    /// nothing, and use the nearest bank. See [`crate::bank::BankConfig`].
    #[serde(default)]
    pub bank: crate::bank::BankConfig,
}

fn default_scrollback() -> u32 {
    2000
}

fn default_reconnect_delay() -> u64 {
    10
}

impl Default for Profile {
    /// What the lobby starts from: a telnet port and nothing else. The
    /// host is empty on purpose, so nothing dials until `/connect` or
    /// `/set host` names one.
    fn default() -> Self {
        Profile {
            target: Target::default(),
            host: String::new(),
            port: 23,
            username: String::new(),
            password: String::new(),
            pace_ms: None,
            disable_evil_warnings: false,
            scrollback_lines: default_scrollback(),
            reconnect: false,
            reconnect_delay_seconds: default_reconnect_delay(),
            bot: None,
            farm: None,
            bank: crate::bank::BankConfig::default(),
        }
    }
}

impl Profile {
    /// Read and parse a profile, reporting renamed keys on stderr.
    ///
    /// The headless commands call this. The parse and the validation
    /// live in `settings`, which `play` uses directly so it can edit
    /// what it loaded.
    pub fn load(path: &std::path::Path) -> Result<Profile, String> {
        let settings = crate::settings::Settings::load(path)?;
        for warning in settings.warnings() {
            eprintln!("profile {}: {warning}", path.display());
        }
        Ok(settings.profile().clone())
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

    /// A headless command has no lobby to wait in. A profile that names
    /// no host is refused before anything dials.
    pub fn require_host(&self) -> Result<(), String> {
        if self.host.is_empty() {
            return Err("no host set".into());
        }
        Ok(())
    }
}

