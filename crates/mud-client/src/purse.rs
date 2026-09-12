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

/// The words `get` and the pickup line use, low to high, in the order
/// [`Coins::counts`] is indexed. `DENOMINATIONS` above carries the full
/// names the carried list prints. Same order, on purpose: one index
/// reads both tables.
pub const DENOMINATION_WORDS: [&str; 5] = ["copper", "silver", "gold", "platinum", "runic"];

/// Coin counts per denomination, indexed as [`DENOMINATION_WORDS`].
///
/// The purse is one number and stays one number for routing. The bank
/// gate needs the counts underneath it: the weight of coins is a third
/// of a unit each, divided per denomination, and the coin gate counts
/// coins rather than value. Reading the counts once and deriving the
/// purse from them keeps one parser for both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coins {
    pub counts: [u32; 5],
}

impl Coins {
    /// Every coin counts one.
    pub fn count(&self) -> u32 {
        self.counts.iter().sum()
    }

    /// Coin weight as `mud-core`'s `carried_weight` computes it: each
    /// denomination's count divided by 3, integer division, summed.
    /// Per denomination, not on the total. 11 silver and 49 copper
    /// weigh 3 + 16 = 19, not 60 / 3 = 20.
    pub fn weight(&self) -> i64 {
        self.counts.iter().map(|&n| i64::from(n) / 3).sum()
    }

    /// The same coins as money.
    pub fn purse(&self) -> Purse {
        let total = self
            .counts
            .iter()
            .zip(DENOMINATIONS.iter())
            .map(|(&n, (_, _, value))| u64::from(n) * value)
            .sum();
        Purse(total)
    }

    /// Read the leading coin entries of a carried list, as
    /// [`crate::sheet::Inventory::items`] holds it: coins first, one
    /// entry per denomination, then the gear. Stops at the first entry
    /// that is not a coin entry, for the reason `leading_coins` gives.
    pub fn from_entries(entries: &[String]) -> Coins {
        let mut coins = Coins::default();
        for entry in entries {
            let Some((index, count)) = parse_coin_segment(entry) else {
                break;
            };
            coins.counts[index] = coins.counts[index].saturating_add(count);
        }
        coins
    }
}

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

    /// `count` coins of one denomination, named as `get` and the pickup
    /// line name it ("silver"). `None` for a word that is not a
    /// denomination, so a caller cannot value coins it misread.
    pub fn from_coins(count: u32, word: &str) -> Option<Purse> {
        let index = DENOMINATION_WORDS.iter().position(|w| *w == word)?;
        Some(Purse(u64::from(count) * DENOMINATIONS[index].2))
    }

    /// The same money in gold crowns, the unit a player thinks in, with
    /// the smaller coins as a fraction.
    pub fn gold(&self) -> f64 {
        self.0 as f64 / GOLD as f64
    }
}

/// Match one comma-separated segment ("2 gold crowns") against the
/// denomination table, giving the denomination's index and the count.
/// `None` means this segment is not a coin entry at all. The boundary
/// [`leading_coins`] uses to know where a mixed "You are carrying ..."
/// list stops being money and starts being gear.
///
/// The count is a `u32`, and a segment whose count does not fit fails
/// the parse rather than saturating, the same fail-closed policy the
/// purse arithmetic always had.
fn parse_coin_segment(segment: &str) -> Option<(usize, u32)> {
    let segment = segment.trim();
    let (count, rest) = segment.split_once(' ')?;
    let count: u32 = count.parse().ok()?;
    let rest = rest.trim();
    let index = DENOMINATIONS
        .iter()
        .position(|(one, many, _)| rest == *one || rest == *many)?;
    Some((index, count))
}

/// Read a coin listing into a purse, or `None` if the line is not one.
///
/// `None` and `Some(Purse::ZERO)` are different answers on purpose: a
/// line that says nothing about money must not be read as "carrying
/// nothing", or every unrelated line would zero the purse.
///
/// All-or-nothing: every comma-separated segment must be a coin entry, or
/// the whole line is rejected. This is the segment-level primitive with
/// its own tests and its own job. It deliberately does NOT know about
/// `show_inventory`'s "You are carrying " wrapper or about coins sharing
/// a line with items. That is [`leading_coins`]'s job, for
/// [`PurseMeter`] alone.
pub fn parse_coin_line(line: &str) -> Option<Purse> {
    let mut coins = Coins::default();
    let mut matched = false;
    for part in line.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (index, count) = parse_coin_segment(part)?;
        coins.counts[index] = coins.counts[index].checked_add(count)?;
        matched = true;
    }
    matched.then(|| coins.purse())
}

/// The literal wrapper `mud-core`'s `show_inventory` puts around the
/// carried list: `out.push_str(&format!("You are carrying {}\n",
/// names.join(", ")))` (`crates/mud-core/src/game.rs:12892`), with coins
/// pushed as the FIRST joined element when there are any
/// (`crates/mud-core/src/game.rs:12842-12846`) and `"You are carrying
/// Nothing!"` (`text::CARRYING_NOTHING`) when the character carries
/// nothing at all.
///
/// UNVERIFIED against the board this client actually plays: that board
/// is a different reimplementation ("MMud Reborn"), its wording has
/// already diverged from stock elsewhere, and nobody has captured its
/// inventory reply. `mud-core` is the best offline authority there is,
/// not a live oracle for this exact string — if the real board only
/// differs in this WRAPPER (a different lead-in phrase around the same
/// coins-first, comma-joined list), this is the one line to change.
/// A board that also differs in coin ordering or vocabulary is a bigger
/// edit: `DENOMINATIONS` and [`leading_coins`]'s coins-first assumption
/// would need to change too, not just this constant.
const CARRYING_PREFIX: &str = "You are carrying ";

/// Sum the coin-shaped segments at the START of a "You are carrying ..."
/// body, stopping at the first segment that is not one — never skipping
/// past it to a later segment that IS coin-shaped: a stock board always
/// joins coins first (`show_inventory`), so a non-coin segment means
/// coins are over for this line, and any coin-shaped text found further
/// on belongs to an item's own name or count, not the purse. `None` if
/// there were no LEADING coin segments at all (an empty purse, or an
/// items-only carry) — the same "no line = no fact" distinction
/// [`parse_coin_line`] makes, kept separate because a `PurseMeter` reply
/// is allowed to have money AND gear on the same line and a bare
/// [`parse_coin_line`] would refuse the whole thing the moment it hit the
/// first item.
///
/// Items follow coins in `show_inventory`'s join and must never be
/// mistaken for money — an item can itself wear a denomination word
/// ("silver holy amulet", live in oracle_charm_lifecycle per
/// `bot.rs::COIN_PILE_RE`'s own caution) but [`parse_coin_segment`]'s
/// leading-count requirement is what keeps it out: "silver" is not a
/// number.
///
/// Sums with `checked_add` and fails the WHOLE parse (returns `None`) on
/// overflow, matching [`parse_coin_segment`]'s and [`parse_coin_line`]'s
/// policy rather than silently saturating: one segment primitive shared
/// by two callers should not disagree about what "too much money" means,
/// and failing closed is the smaller surprise given the primitive
/// already does.
fn leading_coins(body: &str) -> Option<Coins> {
    let mut coins = Coins::default();
    let mut matched = false;
    for part in body.split(',') {
        let part = part.trim();
        let Some((index, count)) = parse_coin_segment(part) else {
            break;
        };
        coins.counts[index] = coins.counts[index].checked_add(count)?;
        matched = true;
    }
    matched.then_some(coins)
}

/// The carried balance, fed from the board's own inventory reply.
///
/// A coin listing on the wire is not necessarily the purse: a pile on the
/// floor prints the same shape (`bot.rs`'s `COIN_PILE_RE` sweeps those),
/// and reading either one as the balance would let the router refuse a
/// toll the character can actually afford. So the meter only ever accepts
/// the single line immediately following [`PurseMeter::expect_reply`] —
/// the reply to OUR `i` — and nothing else, ever.
///
/// That line is authoritative whether or not it looks like coins: the
/// board never sends a bare coin line for an inventory reply — coins
/// arrive wrapped in `CARRYING_PREFIX` and often followed by gear on the
/// same line — so the balance comes from stripping that prefix and
/// reading [`leading_coins`], not from [`parse_coin_line`] directly.
/// `"You are carrying Nothing!"` and an items-only carry both have no
/// leading coin segment, and an ATTRIBUTED line saying so is a real
/// answer, not a shrug: it resets the balance to [`Purse::ZERO`] just as
/// surely as a coin-bearing line sets it to something else, because the
/// dangerous direction here is overcounting — a stale non-zero balance
/// surviving past a reply that said "nothing" would let the router think
/// a toll is affordable when it no longer is.
///
/// `current` is always an assignment from the board, never an
/// accumulation — deliberately, so that a `Purse` built from
/// `Capabilities::unrestricted()`'s `u64::MAX` sentinel can never reach
/// this meter's arithmetic and overflow it.
#[derive(Debug, Clone, Copy, Default)]
pub struct PurseMeter {
    current: Purse,
    /// Set by `expect_reply`, cleared only when a line that actually
    /// LOOKS like an inventory reply arrives (see
    /// [`looks_like_inventory_reply`]) -- never by the mere fact that
    /// some line followed. One inventory ask gets exactly one answer;
    /// anything else in between -- coin-shaped or not -- belongs to the
    /// room or another player, not the character, and must not end the
    /// wait.
    expecting: bool,
}

/// Does `line` have the shape of an inventory reply's body at all,
/// rather than merely happen to be whatever arrived next?
///
/// Attribution here is positional, not textual: `i`'s reply body is
/// `Kind::Opaque` (see [`PurseMeter::expect_reply`]'s own doc), so the
/// correlator can only mark the ECHO, never the reply itself. Before
/// this check existed, `observe` treated whichever line followed the
/// echo as the whole answer unconditionally -- and on a live board any
/// interleaved traffic (a shout, another player's action) can land in
/// that gap. Usually that just yields a spurious zero, which undercounts
/// and is the safe direction on its own. But the toll learner
/// (`Navigator::read_purse`) takes a reading before AND after a
/// crossing: if both readings land on unrelated interlopers and both
/// come out zero, `after != before` is false and the crossing gets
/// recorded FREE even though it charged -- exactly the failure this
/// feature exists to prevent.
///
/// So a reply must look like one: `mud-core`'s `show_inventory` always
/// wraps the carried list in [`CARRYING_PREFIX`], and `"You are carrying
/// Nothing!"` is not a second case to check -- it already starts with
/// that same prefix. A line that does not match is left unconsumed
/// (`observe` returns `false` and `expecting` stays set), which chooses
/// the failure mode deliberately: a board worded differently from this
/// one never satisfies the expectation and the purse simply never
/// updates, so the router treats every toll as unaffordable and detours.
/// That is the safe direction -- worse than a live balance, better than
/// consuming an arbitrary line and risking a mismeasured toll.
fn looks_like_inventory_reply(line: &str) -> bool {
    line.starts_with(CARRYING_PREFIX)
}

impl PurseMeter {
    /// Call this once the board has accepted our `i` (the correlator
    /// attributes its echo) — never at raw send time, and never on the
    /// echo line itself: the reply body is unattributed (`i`'s `Kind` is
    /// `Opaque`, whose `completes` is always `false`, so the correlator
    /// cannot mark any line of the actual reply), and the echo is not
    /// that reply.
    pub fn expect_reply(&mut self) {
        self.expecting = true;
    }

    /// Note a line. Returns `true` iff this line was the one owed to a
    /// pending `expect_reply` -- an attributed answer that also LOOKS
    /// like one (see [`looks_like_inventory_reply`]) -- and `false`
    /// otherwise: either nothing was pending at all (an unattributed
    /// line, such as a coin pile on the floor, changes NOTHING and is
    /// always refused), or something is pending but this line does not
    /// have the shape of a reply, in which case it is skipped and the
    /// expectation stays open for whatever comes next.
    ///
    /// Strips `CARRYING_PREFIX` (verified present by the shape check
    /// above) and reads [`leading_coins`] off what remains, so gear
    /// listed after the coins on the same line is ignored rather than
    /// poisoning the whole parse. When there are no leading coins at all
    /// the balance is still overwritten -- to [`Purse::ZERO`], not left
    /// as whatever it used to hold: this line is the character's WHOLE
    /// answer, and a stale non-zero balance surviving an attributed
    /// "nothing" reply would overcount, the direction that lets the
    /// router misjudge a toll as affordable.
    pub fn observe(&mut self, line: &str) -> bool {
        if !self.expecting {
            return false;
        }
        if !looks_like_inventory_reply(line) {
            return false;
        }
        self.expecting = false;
        let body = line.strip_prefix(CARRYING_PREFIX).unwrap_or(line);
        self.current = leading_coins(body).map_or(Purse::ZERO, |c| c.purse());
        true
    }

    pub fn current(&self) -> Purse {
        self.current
    }

    /// True once a line that both answers our `i` AND looks like its
    /// reply has arrived (or there was never a pending request at all).
    /// False for the whole window between sending `i` and that answer --
    /// which an interloper in between no longer shortens, now that
    /// `observe` verifies shape before it clears this.
    pub fn settled(&self) -> bool {
        !self.expecting
    }
}
