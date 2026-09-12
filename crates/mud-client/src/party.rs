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

use std::sync::LazyLock;

use regex::Regex;

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
}

/// What one line changed, in words the lobby can print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Following(String),
    Joined(String),
    Invited(String),
    Left(String),
    Ended,
    Roster,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PartyState {
    pub role: Role,
    pub leader: Option<String>,
    pub members: Vec<Member>,
    /// Rows of a roster block in flight. `None` between blocks.
    roster: Option<Vec<Member>>,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl PartyState {
    pub fn new() -> PartyState {
        PartyState::default()
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
            None => self.members.push(Member { name: name.to_string(), invited }),
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
                rows.push(Member { name: c[1].to_string(), invited: line.contains("[Invited]") });
                return None;
            }
            // Anything else ends the block: the board never prints a
            // row without its indent.
            let ended = self.end_roster();
            return ended.or_else(|| self.observe(line));
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
            self.members.retain(|m| !same(&m.name, &name));
            return Some(Change::Left(name));
        }
        None
    }

    /// A prompt ends a roster block the same way a blank line does.
    pub fn observe_prompt(&mut self) -> Option<Change> {
        if self.roster.is_some() { self.end_roster() } else { None }
    }

    fn end_roster(&mut self) -> Option<Change> {
        let rows = self.roster.take()?;
        if rows.is_empty() && self.role == Role::Leader {
            self.reset();
            return Some(Change::Ended);
        }
        self.members = rows;
        Some(Change::Roster)
    }
}
