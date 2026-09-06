//! Banking: the deposit gate, the bank list, and the errand.
//!
//! A farm picks up every pile it kills over and never puts any of it
//! down. Coins weigh a third of a unit each, so a long run drifts the
//! character up a weight class, and a death loses the lot. This module
//! decides when a run should go and deposit, finds the bank, and does
//! the errand. The design is `docs/superpowers/specs/2026-09-06-banking-design.md`.

use serde::{Deserialize, Serialize};

use mud_core::content::RoomId;

use crate::purse::Purse;

/// The `[bank]` table of a profile. Absent means these defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BankConfig {
    /// Judge the gate during a farm and detour when it trips. `/bank`
    /// works either way.
    pub auto_deposit: bool,
    /// Raw coin count, every denomination counting one. 0 disables
    /// the count gate.
    pub deposit_at_coins: u32,
    /// Fire when the coins picked up since the last reading lifted the
    /// weight class one step.
    pub deposit_on_weight_class: bool,
    /// What the deposit leaves in the purse, in gold crowns.
    pub keep_gold: u32,
    /// A fixed bank as `map/room`. Unset means the nearest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl Default for BankConfig {
    fn default() -> Self {
        BankConfig {
            auto_deposit: true,
            deposit_at_coins: 1000,
            deposit_on_weight_class: true,
            keep_gold: 0,
            at: None,
        }
    }
}

impl BankConfig {
    /// `at` must be a room id. Whether that room is a bank needs the
    /// world database, which the profile loader does not have, so that
    /// check waits for the first use, see `choose_bank`.
    pub fn validate(&self) -> Result<(), String> {
        match &self.at {
            Some(at) if crate::farm::parse_room_id(at).is_none() => Err(format!(
                "[bank].at ({at:?}) is not a room id: write it as map/room, such as 1/297"
            )),
            _ => Ok(()),
        }
    }

    /// The keep floor as money.
    pub fn keep(&self) -> Purse {
        Purse::from_gold(self.keep_gold)
    }

    /// The configured bank room, if any. `validate` has already refused
    /// an unparsable one, so a `None` here means unset.
    pub fn at_room(&self) -> Option<RoomId> {
        self.at.as_deref().and_then(crate::farm::parse_room_id)
    }
}
