//! Banking: the deposit gate, the bank list, and the errand.
//!
//! A farm picks up every pile it kills over and never puts any of it
//! down. Coins weigh a third of a unit each, so a long run drifts the
//! character up a weight class, and a death loses the lot. This module
//! decides when a run should go and deposit, finds the bank, and does
//! the errand. The design is `docs/superpowers/specs/2026-09-06-banking-design.md`.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use mud_core::content::{Content, RoomId};

use crate::farm::{Casts, FarmConfig, FarmError, FarmStats, LegEnd, Phase, PhaseSink};
use crate::graph::{Capabilities, RoomGraph};
use crate::purse::{Coins, Purse};
use crate::session::Session;
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
/// prints it. That function is the authority. This is a restatement so
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

/// How the errand ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrandEnd {
    Deposited {
        farthings: u64,
        at: RoomId,
        bank: String,
    },
    /// Nothing was deposited, and why. The run carries on.
    Nothing(String),
    Died,
    TimeUp,
    TooHurt,
}

/// Send `i` and parse the reply. The session's own contents tracker
/// reads the same reply, so `session.contents()` agrees afterwards.
/// Bounded by `farm::ask`'s deadline: a lost reply parses as an empty
/// inventory with no encumbrance line, which `Reading::of` refuses.
pub(crate) async fn read_inventory(session: &Session) -> Inventory {
    Inventory::parse(&crate::farm::ask(session, "i", "Encumbrance:").await)
}

/// Send the deposit and wait for one of its three replies. `None` when
/// the deadline passed with no reply the grammar knows.
async fn send_deposit(session: &Session, farthings: u64) -> Option<DepositReply> {
    use crate::correlate::Correlated;
    use crate::events::Event;
    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});
    session.send(&format!("deposit {farthings}"));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(Correlated { event: Event::Line(line), .. })) => {
                if let Some(reply) = deposit_reply(&line) {
                    return Some(reply);
                }
            }
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) | Err(_) => return None,
        }
    }
}

/// Walk to the bank, deposit the purse above the keep floor, and leave
/// `current` at the bank. The next leg walks on from there.
///
/// The walk is `farm::travel`, so fights on the way, interrupts, the
/// time budget and desync recovery are the leg's. The deposit is
/// computed from a fresh reading at the bank, never from the one the
/// gate judged: a toll on the way changed the purse and nothing
/// observes tolls. After the deposit the purse is read again so the
/// purse meter and the pack reflect it before any route is planned.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn errand(
    session: &Session,
    nav: &crate::nav::Navigator,
    graph: &RoomGraph,
    content: &Content,
    bank: &BankConfig,
    cfg: &FarmConfig,
    bot_config: &crate::bot::BotConfig,
    threat: &std::sync::Arc<crate::bot::ThreatTable>,
    refusals: &crate::bot::Refusals,
    casts: &mut Casts,
    clock: &mut crate::world::RoundClock,
    started: Instant,
    stats: &mut FarmStats,
    phase: PhaseSink<'_>,
    current: &mut RoomId,
) -> Result<ErrandEnd, FarmError> {
    let to = match choose_bank(bank, graph, content, *current, &session.capabilities()) {
        Ok(Some(to)) => to,
        Ok(None) => {
            return Ok(ErrandEnd::Nothing(format!(
                "no bank reachable from {}/{}",
                current.map, current.room
            )));
        }
        Err(why) => return Ok(ErrandEnd::Nothing(why)),
    };
    let name = bank_rooms(content)
        .into_iter()
        .find(|(room, _)| *room == to)
        .map(|(_, name)| name)
        .unwrap_or_default();
    crate::farm::set_phase(phase, Phase::Banking { at: to });
    if *current != to {
        let leg = crate::farm::travel(
            session, nav, graph, current, to, cfg, bot_config, threat, refusals, casts, clock,
            started, stats, phase, false,
        )
        .await;
        // A bank the walk cannot reach ends the errand, not the run: a
        // roam's fence can forbid the room `choose_bank` picked, and a
        // configured `at` can sit behind a door no route opens. Every
        // other error is the run's, so a disconnect still ends it.
        // `travel` has already written `current` on the error path, so
        // the next leg starts from wherever the walk stopped.
        let leg = match leg {
            Ok(leg) => leg,
            Err(FarmError::Nav(e)) => {
                return Ok(ErrandEnd::Nothing(format!(
                    "no way to the bank at {}/{}: {e}",
                    to.map, to.room
                )));
            }
            Err(e) => return Err(e),
        };
        match leg {
            LegEnd::Arrived { .. } => {}
            LegEnd::Died => return Ok(ErrandEnd::Died),
            LegEnd::TimeUp => return Ok(ErrandEnd::TimeUp),
            LegEnd::TooHurt => return Ok(ErrandEnd::TooHurt),
        }
    }
    let purse = read_inventory(session).await.coins().purse();
    let farthings = purse.farthings().saturating_sub(bank.keep().farthings());
    if farthings == 0 {
        return Ok(ErrandEnd::Nothing(format!(
            "nothing above the keep floor at {name}"
        )));
    }
    let reply = send_deposit(session, farthings).await;
    // Read again whatever the reply was, so the purse the router sees
    // is the board's, not a guess.
    let _ = read_inventory(session).await;
    match reply {
        Some(DepositReply::Deposited(_)) => {
            stats.deposits += 1;
            stats.deposited_farthings += farthings;
            Ok(ErrandEnd::Deposited {
                farthings,
                at: to,
                bank: name,
            })
        }
        Some(DepositReply::NotABank) => {
            stats.relocalizations += 1;
            Ok(ErrandEnd::Nothing(format!(
                "the board says {}/{} is not a bank: the walk did not land where the graph says",
                to.map, to.room
            )))
        }
        Some(DepositReply::Unreasonable) => Ok(ErrandEnd::Nothing(format!(
            "the board refused a deposit of {farthings}"
        ))),
        None => Ok(ErrandEnd::Nothing("no reply to the deposit".into())),
    }
}
