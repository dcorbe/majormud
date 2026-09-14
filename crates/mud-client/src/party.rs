//! Party state, the telepath command channel, and the hold set.
//!
//! The design is `docs/superpowers/specs/2026-09-12-party-mechanics-design.md`.
//!
//! Every wording here comes from the string table of the stock
//! `WCCMMUD.DLL`. None has been captured on the live board. Each is
//! UNVERIFIED there, the same caveat `bank.rs` carries. A wording that
//! never matches fails safe: the party is never seen and nothing
//! party-shaped runs.
//!
//! Nothing here sends anything.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;
use serde::{Deserialize, Serialize};

use mud_core::content::{Content, Room, RoomId};

/// The stock `You are now following %s` has no trailing period. The
/// roster row is any indented line before the block ends. Its name is
/// the first word and it is invited when `[Invited]` appears anywhere
/// on it, so the stock two-column row and MudPlay's bracketed row both
/// parse.
static NOW_FOLLOWING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^You are now following (\w+)\.?$").unwrap());
static STARTED_TO_FOLLOW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\w+) started to follow you\.$").unwrap());
static YOU_INVITED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^You have invited (\w+) to follow you\.$").unwrap());
static NO_LONGER_FOLLOWING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^You are no longer following (\w+)\.$").unwrap());
static REMOVED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\w+) has been removed from your followers\.$").unwrap());
pub const NOT_IN_A_PARTY: &str = "You are not in a party at the present time.";
pub const ROSTER_HEADER: &str = "The following people are in your travel party:";
static ROSTER_ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+(\w+)\b").unwrap());
/// The live row, captured 2026-09-13:
/// `  Salad   (Ranger)     [M:100%] [H: 86%]   - Midrank`. Each part
/// is read on its own, so the stock two-column row still parses with
/// every part absent.
static ROSTER_CLASS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([^)]+)\)").unwrap());
/// Reads the first pool tag and assumes a row carries at most one.
static ROSTER_POOL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[KM]:\s*(\d+)%\]").unwrap());
static ROSTER_HP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[H:\s*(\d+)%\]").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    #[default]
    None,
    Leader,
    Follower,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub invited: bool,
    /// The roster's word in parentheses. `None` on a row without one.
    pub class: Option<String>,
    /// Health percent, from the roster's `[H: 86%]`.
    pub hp: Option<u8>,
    /// Kai or mana percent, from `[K:100%]` or `[M:100%]`.
    pub pool: Option<u8>,
}

impl Member {
    fn plain(name: &str, invited: bool) -> Member {
        Member { name: name.to_string(), invited, class: None, hp: None, pool: None }
    }

    /// One roster row.
    fn from_row(line: &str, name: &str) -> Member {
        let percent = |re: &Regex| re.captures(line).and_then(|c| c[1].parse::<u8>().ok());
        Member {
            name: name.to_string(),
            invited: line.contains("[Invited]"),
            class: ROSTER_CLASS.captures(line).map(|c| c[1].trim().to_string()),
            hp: percent(&ROSTER_HP),
            pool: percent(&ROSTER_POOL),
        }
    }
}

/// What one line changed, in words the lobby can print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Following(String),
    Joined(String),
    Invited(String),
    Left(String),
    Ended,
    /// The roster named a different set of members.
    Roster,
    /// The roster named the same members with new numbers.
    Vitals,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PartyState {
    pub role: Role,
    pub leader: Option<String>,
    pub members: Vec<Member>,
    /// Rows of a roster block in flight. `None` between blocks.
    roster: Option<Vec<Member>>,
    /// The character's own name. The board lists the character on its own
    /// roster, but the character is never a member of its own party.
    own: Option<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl PartyState {
    pub fn new() -> PartyState {
        PartyState::default()
    }

    /// Set the character's own name. The board lists the character on its
    /// own roster, but the character is never a member of its own party.
    pub fn set_own(&mut self, name: &str) {
        self.own = Some(name.to_string());
    }

    pub fn is_follower(&self) -> bool {
        self.role == Role::Follower
    }

    pub fn is_leader(&self) -> bool {
        self.role == Role::Leader
    }

    /// The members that have actually joined.
    pub fn followers(&self) -> Vec<String> {
        self.members.iter().filter(|m| !m.invited).map(|m| m.name.clone()).collect()
    }

    fn add(&mut self, name: &str, invited: bool) {
        match self.members.iter_mut().find(|m| same(&m.name, name)) {
            Some(m) => m.invited = m.invited && invited,
            None => self.members.push(Member::plain(name, invited)),
        }
    }

    fn reset(&mut self) {
        self.role = Role::None;
        self.leader = None;
        self.members.clear();
        self.roster = None;
    }

    /// Feed one ANSI-stripped line.
    pub fn observe(&mut self, line: &str) -> Option<Change> {
        if let Some(rows) = self.roster.as_mut() {
            if line.trim().is_empty() {
                return self.end_roster();
            }
            if let Some(c) = ROSTER_ROW.captures(line) {
                rows.push(Member::from_row(line, &c[1]));
                return None;
            }
            // Anything else ends the block: the board never prints a
            // row without its indent. The line ends the roster, then
            // is processed on its own. Its own change wins if it has
            // one, otherwise the roster's change is returned.
            let ended = self.end_roster();
            return self.observe(line).or(ended);
        }
        let line = line.trim_end();
        if line == ROSTER_HEADER {
            self.roster = Some(Vec::new());
            return None;
        }
        if line == NOT_IN_A_PARTY {
            self.reset();
            return Some(Change::Ended);
        }
        if let Some(c) = NOW_FOLLOWING.captures(line) {
            self.reset();
            self.role = Role::Follower;
            self.leader = Some(c[1].to_string());
            return Some(Change::Following(c[1].to_string()));
        }
        if let Some(c) = STARTED_TO_FOLLOW.captures(line) {
            self.role = Role::Leader;
            self.leader = None;
            self.add(&c[1], false);
            return Some(Change::Joined(c[1].to_string()));
        }
        if let Some(c) = YOU_INVITED.captures(line) {
            self.role = Role::Leader;
            self.leader = None;
            self.add(&c[1], true);
            return Some(Change::Invited(c[1].to_string()));
        }
        if NO_LONGER_FOLLOWING.is_match(line) {
            self.reset();
            return Some(Change::Ended);
        }
        if let Some(c) = REMOVED.captures(line) {
            let name = c[1].to_string();
            self.remove(&name);
            return Some(Change::Left(name));
        }
        None
    }

    /// Drop a member by name, case insensitively. Absent means nothing
    /// to do.
    pub fn remove(&mut self, name: &str) {
        self.members.retain(|m| !same(&m.name, name));
    }

    /// A prompt ends a roster block the same way a blank line does.
    pub fn observe_prompt(&mut self) -> Option<Change> {
        if self.roster.is_some() { self.end_roster() } else { None }
    }

    fn end_roster(&mut self) -> Option<Change> {
        let mut rows = self.roster.take()?;
        if let Some(own_name) = &self.own {
            rows.retain(|m| !same(&m.name, own_name));
        }
        if rows.is_empty() && self.role == Role::Leader {
            self.reset();
            return Some(Change::Ended);
        }
        let key = |m: &Member| (m.name.to_ascii_lowercase(), m.invited);
        let same_members = rows.len() == self.members.len()
            && rows.iter().map(key).eq(self.members.iter().map(key));
        self.members = rows;
        Some(if same_members { Change::Vitals } else { Change::Roster })
    }
}

/// The three commands this cut understands. Any other word after the
/// `@` is dropped without a word, as MudPlay drops unknown commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    Bank,
    Wait,
    Ok,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub from: String,
    pub command: Remote,
}

/// `Foo telepaths: @bank`. MudPlay's inbound shape, and the DLL's
/// `%s telepaths: %s%s`.
static TELEPATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\w+) telepaths: @(\S+)").unwrap());

pub fn remote(line: &str) -> Option<Request> {
    let c = TELEPATH.captures(line.trim_end())?;
    let command = match c[2].to_ascii_lowercase().as_str() {
        "bank" => Remote::Bank,
        "wait" => Remote::Wait,
        "ok" => Remote::Ok,
        _ => return None,
    };
    Some(Request { from: c[1].to_string(), command })
}

/// The leader, or a member that has joined. An invited character is
/// not in the party yet and cannot hold it. This is the whole
/// permission model until a character database exists.
pub fn permitted(state: &PartyState, sender: &str) -> bool {
    state.leader.as_deref().is_some_and(|l| same(l, sender))
        || state.members.iter().any(|m| !m.invited && same(&m.name, sender))
}

/// The wire form MudPlay sends on the live board.
pub fn telepath(to: &str, text: &str) -> String {
    format!("/{to} {text}")
}

/// Followers the leader must not walk away from, each with a deadline.
/// A follower that said `@wait`, or one the leader put here for a bank
/// wait. Keyed lowercased so the same name never holds twice under a
/// different case. Each entry keeps the name as it was given, for
/// display.
#[derive(Debug, Clone, Default)]
pub struct Holds {
    by: BTreeMap<String, (String, Instant)>,
}

impl Holds {
    pub fn new() -> Holds {
        Holds::default()
    }

    fn key(name: &str) -> String {
        name.to_ascii_lowercase()
    }

    /// Insert, or extend. A later shorter deadline never shortens.
    pub fn hold(&mut self, name: &str, until: Instant) {
        match self.by.entry(Holds::key(name)) {
            Entry::Vacant(e) => {
                e.insert((name.to_string(), until));
            }
            Entry::Occupied(mut e) => {
                if until > e.get().1 {
                    e.get_mut().1 = until;
                }
            }
        }
    }

    pub fn release(&mut self, name: &str) {
        self.by.remove(&Holds::key(name));
    }

    /// Drop and return every name whose deadline has passed.
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        let gone: Vec<String> =
            self.by.iter().filter(|(_, (_, t))| *t <= now).map(|(_, (n, _))| n.clone()).collect();
        for n in &gone {
            self.by.remove(&Holds::key(n));
        }
        gone
    }

    pub fn is_empty(&self) -> bool {
        self.by.is_empty()
    }

    /// Held names as they were given, sorted by their lowercased key.
    pub fn names(&self) -> Vec<String> {
        self.by.values().map(|(n, _)| n.clone()).collect()
    }

    pub fn retain_members(&mut self, state: &PartyState) {
        self.by.retain(|_, (n, _)| permitted(state, n));
    }

    pub fn clear(&mut self) {
        self.by.clear();
    }
}

/// Whether this room is a bank: a shop room whose shop banks.
fn is_bank(content: &Content, room: &Room) -> bool {
    room.room_type == 1
        && room
            .shop
            .and_then(|s| content.shops.get(&s))
            .is_some_and(|shop| shop.shop_type == crate::bank::BANK_SHOP_TYPE)
}

/// The names of the bank rooms, unique across the shipped world,
/// checked 2026-09-12. A room block naming one is a bank arrival with
/// no localisation.
pub fn bank_names(content: &Content) -> BTreeSet<String> {
    content
        .rooms
        .values()
        .filter(|room| is_bank(content, room))
        .map(|room| room.name.clone())
        .collect()
}

/// The bank room the block named, when exactly one bank carries that
/// name. The block prints the ROOM's name, which is not the shop's for
/// four of the five shipped banks, so the shop names `bank::bank_rooms`
/// returns cannot answer this.
pub fn bank_room(content: &Content, name: &str) -> Option<RoomId> {
    let mut found =
        content.rooms.values().filter(|room| room.name == name && is_bank(content, room));
    let first = found.next()?;
    found.next().is_none().then_some(first.id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Wait,
    Ok,
}

/// A follower's side of the wait handshake. `@wait` goes out with the
/// rest, `@ok` on the first prompt without the Resting status after a
/// prompt that had it, or at once when the board refused the rest.
///
/// A rest sent while the hold is already out starts the status watch
/// over. That is the rest after a drag: the leader's step is already on
/// the wire when `@wait` lands, the board takes a second to resolve it,
/// and the whole party is moved one room, which ends the rest. The bot
/// sits down again where it lands, and the release waits for that rest.
/// A drag the bot answers with no new rest is over, and the lost status
/// releases the leader on the next prompt as any rest's end would.
#[derive(Debug, Clone, Default)]
pub struct WaitState {
    waiting: bool,
    seen_resting: bool,
}

impl WaitState {
    pub fn new() -> WaitState {
        WaitState::default()
    }

    /// Whether a `@wait` is out and its `@ok` is still owed. Callers
    /// read the party only when this says something is in flight.
    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    /// A rest under a hold already out is the one after a drag: it
    /// starts the status watch over and owes no second warning.
    pub fn on_rest_sent(&mut self) -> Option<Signal> {
        self.seen_resting = false;
        if self.waiting {
            return None;
        }
        self.waiting = true;
        Some(Signal::Wait)
    }

    pub fn on_prompt(&mut self, status: Option<&crate::events::Status>) -> Option<Signal> {
        if !self.waiting {
            return None;
        }
        let resting = matches!(status, Some(crate::events::Status::Resting | crate::events::Status::Meditating));
        if resting {
            self.seen_resting = true;
            return None;
        }
        if self.seen_resting {
            self.reset();
            return Some(Signal::Ok);
        }
        None
    }

    pub fn on_refused(&mut self) -> Option<Signal> {
        if !self.waiting {
            return None;
        }
        self.reset();
        Some(Signal::Ok)
    }

    pub fn reset(&mut self) {
        self.waiting = false;
        self.seen_resting = false;
    }
}

/// The `[party]` table of a profile. Absent means these defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PartyConfig {
    /// A follower's `@wait` holds the leader this long at most.
    pub wait_secs: u64,
    /// The leader waits at the bank this long for `@ok` replies.
    pub bank_wait_secs: u64,
    /// Send `set follow normal` when the character begins following.
    pub follow_normal: bool,
}

impl Default for PartyConfig {
    fn default() -> Self {
        PartyConfig { wait_secs: 90, bank_wait_secs: 15, follow_normal: true }
    }
}

impl PartyConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.wait_secs == 0 {
            return Err("[party].wait_secs must be above 0".into());
        }
        if self.bank_wait_secs == 0 {
            return Err("[party].bank_wait_secs must be above 0".into());
        }
        Ok(())
    }
}
