//! What the character is carrying, as one number.
//!
//! MajorMUD has five denominations and the board prints them mixed
//! ("2 gold crowns, 8 copper farthings"). Carrying them as five counters
//! means every comparison, every subtraction and every toll check has to
//! normalise first, and each of those is a place to get the ladder
//! wrong. One integer in the smallest unit has none of those places.

/// Farthings per unit, low to high. The ratios are `mud-core`'s
/// `CoreConfig::coin_ratios` default (`crates/mud-core/src/game.rs:592`),
/// ORACLE-VERIFIED off the Bank of Godfrey lobby sign: 10 copper = 1
/// silver, 10 silver = 1 gold, 100 gold = 1 platinum, 100 platinum = 1
/// runic. That line is the authority; this is a restatement in the
/// client's own unit, not a second source of truth.
const COPPER: u64 = 1;
const SILVER: u64 = 10;
const GOLD: u64 = 100;
const PLATINUM: u64 = 10_000;
const RUNIC: u64 = 1_000_000;

/// Denomination names as the board writes them, singular and plural,
/// paired with what one is worth. Longest names first is not required —
/// each is matched whole.
const DENOMINATIONS: [(&str, &str, u64); 5] = [
    ("copper farthing", "copper farthings", COPPER),
    ("silver noble", "silver nobles", SILVER),
    ("gold crown", "gold crowns", GOLD),
    ("platinum piece", "platinum pieces", PLATINUM),
    ("runic coin", "runic coins", RUNIC),
];

/// Money on hand, in copper farthings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Purse(u64);

impl Purse {
    pub const ZERO: Purse = Purse(0);

    pub fn from_farthings(n: u64) -> Purse {
        Purse(n)
    }

    pub fn farthings(&self) -> u64 {
        self.0
    }

    /// Gold crowns to farthings.
    ///
    /// THE conversion for `ExitRequirement::Toll`, whose `gold` field is
    /// the raw `para1`. That `para1` is denominated in gold is INFERRED
    /// (the observed 5-gold Silvermere toll matches `para1 = 5`, and map
    /// 17's `10000` is exactly one runic coin) and not proven — the
    /// proof is in `move_user` in the WCCMMUD decompile, unread. If it
    /// turns out to be farthings, this function is the only edit.
    pub fn from_gold(gold: u32) -> Purse {
        Purse(u64::from(gold) * GOLD)
    }
}

/// Read a coin listing into a purse, or `None` if the line is not one.
///
/// `None` and `Some(Purse::ZERO)` are different answers on purpose: a
/// line that says nothing about money must not be read as "carrying
/// nothing", or every unrelated line would zero the purse.
pub fn parse_coin_line(line: &str) -> Option<Purse> {
    let mut total: u64 = 0;
    let mut matched = false;
    for part in line.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (count, rest) = part.split_once(' ')?;
        let count: u64 = count.parse().ok()?;
        let rest = rest.trim();
        let (_, _, value) = DENOMINATIONS
            .iter()
            .find(|(one, many, _)| rest == *one || rest == *many)?;
        total = total.checked_add(count.checked_mul(*value)?)?;
        matched = true;
    }
    matched.then_some(Purse(total))
}
