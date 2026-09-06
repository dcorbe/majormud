//! Banking: the deposit gate, the bank list, and the errand.
//!
//! A farm picks up every pile it kills over and never puts any of it
//! down. Coins weigh a third of a unit each, so a long run drifts the
//! character up a weight class, and a death loses the lot. This module
//! decides when a run should go and deposit, finds the bank, and does
//! the errand. The design is `docs/superpowers/specs/2026-09-06-banking-design.md`.

use serde::{Deserialize, Serialize};

use mud_core::content::{Content, RoomId};

use crate::graph::{Capabilities, RoomGraph};
use crate::purse::{Coins, Purse};
use crate::sheet::Inventory;

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

/// The shop type of a bank, `shop.shop_type` in the shipped table.
/// `mud-core`'s `bank_here` reads the same number.
pub const BANK_SHOP_TYPE: i16 = 7;

/// The rooms a `deposit` works in: shop-active rooms whose shop is a
/// bank, with the bank's name. The shipped database has five. A room
/// that carries a bank's shop number without being shop-active, such
/// as a vault, refuses the command and is not listed.
pub fn bank_rooms(content: &Content) -> Vec<(RoomId, String)> {
    content
        .rooms
        .values()
        .filter(|room| room.room_type == 1)
        .filter_map(|room| {
            let shop = content.shops.get(&room.shop?)?;
            (shop.shop_type == BANK_SHOP_TYPE).then(|| (room.id, shop.name.clone()))
        })
        .collect()
}

/// The bank the fewest hops away along a route this walker can take.
/// Hops of the cheapest route, the same number `RoomGraph::distances`
/// shows an operator, so a toll the purse cannot pay walls the bank
/// off rather than pricing it high.
pub fn nearest_bank(
    graph: &RoomGraph,
    content: &Content,
    from: RoomId,
    caps: &Capabilities,
) -> Option<RoomId> {
    let hops = graph.distances_within_for(from, &|_, _| true, caps);
    bank_rooms(content)
        .into_iter()
        .filter_map(|(room, _)| hops.get(&room).map(|&h| (h, room)))
        .min()
        .map(|(_, room)| room)
}

/// The bank an errand should walk to: the configured one, or the
/// nearest. `Err` names a configured room that is not a bank and lists
/// the banks there are. `Ok(None)` means no bank is reachable.
pub fn choose_bank(
    cfg: &BankConfig,
    graph: &RoomGraph,
    content: &Content,
    from: RoomId,
    caps: &Capabilities,
) -> Result<Option<RoomId>, String> {
    let Some(at) = cfg.at_room() else {
        return Ok(nearest_bank(graph, content, from, caps));
    };
    let banks = bank_rooms(content);
    if banks.iter().any(|(room, _)| *room == at) {
        return Ok(Some(at));
    }
    let listed: Vec<String> = banks
        .iter()
        .map(|(room, name)| format!("{}/{} {name}", room.map, room.room))
        .collect();
    Err(format!(
        "[bank].at {}/{} is not a bank room; the banks are {}",
        at.map,
        at.room,
        listed.join(", ")
    ))
}

/// The encumbrance descriptor as `mud-core`'s `text::encumbrance_descriptor`
/// prints it. That function is the authority; this is a restatement so
/// the client can compare two readings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WeightClass {
    None,
    Light,
    Medium,
    Heavy,
}

/// The class for a carried weight against a capacity. The percent is
/// `carried * 100 / capacity` in integer arithmetic, as `show_inventory`
/// computes it, and no capacity at all reads as full, as
/// `encumbrance_percent` does.
pub fn weight_class(carried: i64, capacity: i64) -> WeightClass {
    if capacity <= 0 {
        return WeightClass::Heavy;
    }
    match carried * 100 / capacity {
        p if p < 33 => WeightClass::None,
        p if p < 66 => WeightClass::Light,
        p if p < 100 => WeightClass::Medium,
        _ => WeightClass::Heavy,
    }
}

/// What one inventory reply says that the gate cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub coins: Coins,
    pub class: WeightClass,
}

impl Reading {
    /// `None` when the reply carried no `Encumbrance:` line: a reply
    /// that never finished, or a board worded differently. No reading
    /// is better than a class guessed from a missing line.
    pub fn of(inv: &Inventory) -> Option<Reading> {
        let (carried, capacity) = inv.encumbrance?;
        Some(Reading {
            coins: inv.coins(),
            class: weight_class(carried, capacity),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judgement {
    Deposit,
    Hold,
}

/// Decides whether a run should go and deposit. Holds the last reading
/// it judged so a class crossing can be seen.
#[derive(Debug, Clone, Default)]
pub struct BankGate {
    last: Option<Reading>,
}

impl BankGate {
    pub fn new() -> BankGate {
        BankGate::default()
    }

    /// Set the reading a crossing is measured from without judging it:
    /// the realm-entry reading at a run's start, and the reading taken
    /// after a deposit.
    pub fn seed(&mut self, reading: Reading) {
        self.last = Some(reading);
    }

    /// Judge one reading and remember it.
    ///
    /// The count gate needs the purse above the keep floor as well, or
    /// a floor above the mark would send the character to the bank
    /// after every stop for a deposit of nothing. The class gate fires
    /// on a rise between this reading and the last one, never on a
    /// class that was already raised.
    pub fn judge(&mut self, cfg: &BankConfig, reading: Reading) -> Judgement {
        let over_the_mark = cfg.deposit_at_coins > 0
            && reading.coins.count() > cfg.deposit_at_coins
            && reading.coins.purse() > cfg.keep();
        let crossed = cfg.deposit_on_weight_class
            && self.last.is_some_and(|last| reading.class > last.class);
        self.last = Some(reading);
        if over_the_mark || crossed {
            Judgement::Deposit
        } else {
            Judgement::Hold
        }
    }
}

/// How the board answered a `deposit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepositReply {
    /// "You deposit 10 silver nobles." with the coins as printed.
    Deposited(String),
    /// "You cannot DEPOSIT if you are not in a bank!"
    NotABank,
    /// "Please specify a more reasonable amount."
    Unreasonable,
}

/// The stock wordings, VERIFIED in `oracle_bank3.raw`. UNVERIFIED
/// against the board this client plays, the same caveat `purse.rs`
/// carries for the inventory wrapper. The correlator matches the same
/// three lines in its own lowercase grammar, duplicated rather than
/// shared so that file reads as one table.
pub fn deposit_reply(line: &str) -> Option<DepositReply> {
    let line = line.trim();
    if let Some(coins) = line.strip_prefix("You deposit ") {
        return Some(DepositReply::Deposited(
            coins.trim_end_matches('.').to_string(),
        ));
    }
    if line == mud_core::text::NOT_IN_BANK_DEPOSIT {
        return Some(DepositReply::NotABank);
    }
    if line == mud_core::text::UNREASONABLE_AMOUNT {
        return Some(DepositReply::Unreasonable);
    }
    None
}
