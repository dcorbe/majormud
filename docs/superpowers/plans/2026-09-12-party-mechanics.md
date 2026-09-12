# Party Mechanics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two characters run by this client can play as a party: the client knows its role, followers ask the leader for the bank with `@bank`, every follower deposits when dragged into a bank, and `@wait` and `@ok` hold and release the leader.

**Architecture:** A new pure module `party.rs` holds the state machine, the telepath grammar, the hold set and the config. The session owns a `PartyTracker` fed from its reader loop like the purse and contents trackers. Follower behaviour is assist behaviour in `tui.rs` and `window.rs` under `/bot`. Leader behaviour is one new `Interrupt::Held` in the navigator's guard plus two hooks in the farm, at the deposit gate and inside the bank errand.

**Tech Stack:** Rust 2024 edition workspace, tokio, serde and toml, regex. Tests are `cargo test -p mud-client`. Scripted-board integration tests follow `crates/mud-client/tests/bank_scripted.rs`.

**Spec:** `docs/superpowers/specs/2026-09-12-party-mechanics-design.md`

## Global Constraints

- Never run `cargo fmt` or `rustfmt`.
- No test reads `re/`. Fixture content tables only.
- Commit after every task with a tagged message: `feat:`, `fix:`, `refactor:`, `doc:`, `test:`. Never mention plans or specs in a commit message. Never commit `.claude/` or `CLAUDE.md`.
- Every file ends with a blank line.
- Prose the user will read, doc comments included: no em dashes, no parentheses, no semicolons.
- Every board wording in `party.rs` is marked UNVERIFIED on the live board in the module doc, as `bank.rs` does.
- Every automatic send reads its profile key and runs only while `session.bot_on()` is true. Nothing party-shaped sends `invite` or `follow`.
- Names compare case-insensitively everywhere in `party.rs`.
- Nothing new is a job the operator can start. `invite`, `follow` and `par` are typed by hand.

## File map

- Create `crates/mud-client/src/party.rs`: `Role`, `Member`, `PartyState`, `Change`, `Remote`, `Request`, `remote`, `permitted`, `telepath`, `Holds`, `bank_names`, `WaitState`, `PartyConfig`.
- Create `crates/mud-client/tests/party.rs`: unit tests for everything in `party.rs`.
- Create `crates/mud-client/tests/party_session.rs`: the session tracker against a scripted board.
- Create `crates/mud-client/tests/party_follower.rs`: follower behaviours against a scripted board.
- Create `crates/mud-client/tests/party_leader.rs`: leader behaviours against a scripted board.
- Modify `crates/mud-client/src/lib.rs`: `pub mod party;`.
- Modify `crates/mud-client/src/profile.rs`: the `[party]` table.
- Modify `crates/mud-client/src/settings.rs`: three keys in `KEYS`, validate call.
- Modify `crates/mud-client/tests/settings.rs`: the key coverage test.
- Modify `crates/mud-client/src/session.rs`: `PartyTracker`, `feed_party`, accessors.
- Modify `crates/mud-client/src/bank.rs`: `deposit_here` split out of `errand`, `FollowerGate`, `errand` takes `asked`, the bank wait.
- Modify `crates/mud-client/src/nav.rs`: `Interrupt::Held`.
- Modify `crates/mud-client/src/farm.rs`: `FarmGuard` reads the holds, `travel` handles `Held`, `hold_here`, the gate reads party requests.
- Modify `crates/mud-client/src/tui.rs`: `assist_tick` follower behaviours, `start_follower_deposit`, `not_following`, `render_status`.
- Modify `crates/mud-client/src/window.rs`: party notes, `set follow normal`, the arrival deposit, bar text.
- Modify `docs/mud-client.md`: a Parties section, and the Banking section.

---

### Task 1: The party state machine

**Files:**
- Create: `crates/mud-client/src/party.rs`
- Modify: `crates/mud-client/src/lib.rs` (add `pub mod party;` in alphabetical order after `pub mod pack;`)
- Test: `crates/mud-client/tests/party.rs`

**Interfaces:**
- Produces: `party::Role`, `party::Member`, `party::PartyState::{new, observe, is_follower, is_leader, followers}`, `party::Change`.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/mud-client/tests/party.rs
//! The party state machine, the telepath grammar, the hold set and the
//! wait state. Pure functions, no board.

use std::time::{Duration, Instant};

use mud_client::party::{Change, Member, PartyState, Role};

fn state() -> PartyState {
    PartyState::new()
}

#[test]
fn now_following_makes_a_follower() {
    let mut s = state();
    let change = s.observe("You are now following Beef");
    assert_eq!(change, Some(Change::Following("Beef".into())));
    assert_eq!(s.role, Role::Follower);
    assert_eq!(s.leader.as_deref(), Some("Beef"));
    assert!(s.members.is_empty());
}

#[test]
fn started_to_follow_makes_a_leader_and_adds_the_member() {
    let mut s = state();
    assert_eq!(s.observe("Pootwaddle started to follow you."), Some(Change::Joined("Pootwaddle".into())));
    assert_eq!(s.role, Role::Leader);
    assert_eq!(s.members, vec![Member { name: "Pootwaddle".into(), invited: false }]);
}

#[test]
fn an_invite_adds_an_invited_member_and_joining_clears_the_flag() {
    let mut s = state();
    assert_eq!(s.observe("You have invited Pootwaddle to follow you."), Some(Change::Invited("Pootwaddle".into())));
    assert_eq!(s.role, Role::Leader);
    assert!(s.members[0].invited);
    s.observe("Pootwaddle started to follow you.");
    assert_eq!(s.members.len(), 1);
    assert!(!s.members[0].invited);
}

#[test]
fn no_longer_following_and_not_in_a_party_reset() {
    let mut s = state();
    s.observe("You are now following Beef");
    assert_eq!(s.observe("You are no longer following Beef."), Some(Change::Ended));
    assert_eq!(s.role, Role::None);
    assert_eq!(s.leader, None);

    s.observe("Pootwaddle started to follow you.");
    assert_eq!(s.observe("You are not in a party at the present time."), Some(Change::Ended));
    assert_eq!(s.role, Role::None);
    assert!(s.members.is_empty());
}

#[test]
fn removed_from_your_followers_drops_the_member() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("Blueberry started to follow you.");
    assert_eq!(s.observe("Pootwaddle has been removed from your followers."), Some(Change::Left("Pootwaddle".into())));
    assert_eq!(s.members, vec![Member { name: "Blueberry".into(), invited: false }]);
}

#[test]
fn the_roster_replaces_the_member_list_in_either_row_format() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("Stale started to follow you.");
    assert_eq!(s.observe("The following people are in your travel party:"), None);
    assert_eq!(s.observe("  Pootwaddle                     Mystic"), None);
    assert_eq!(s.observe("  Blueberry (Ninja) [H: 90%] - front"), None);
    assert_eq!(s.observe("  Newguy                         Warrior      [Invited]"), None);
    assert_eq!(s.observe(""), Some(Change::Roster));
    assert_eq!(
        s.members,
        vec![
            Member { name: "Pootwaddle".into(), invited: false },
            Member { name: "Blueberry".into(), invited: false },
            Member { name: "Newguy".into(), invited: true },
        ]
    );
    assert_eq!(s.role, Role::Leader);
}

#[test]
fn a_prompt_ends_the_roster_block_too() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Blueberry                      Ninja");
    assert_eq!(s.observe_prompt(), Some(Change::Roster));
    assert_eq!(s.members.len(), 1);
    assert_eq!(s.members[0].name, "Blueberry");
}

#[test]
fn an_empty_roster_while_leading_ends_the_party() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("The following people are in your travel party:");
    assert_eq!(s.observe(""), Some(Change::Ended));
    assert_eq!(s.role, Role::None);
}

#[test]
fn chatter_changes_nothing() {
    let mut s = state();
    assert_eq!(s.observe("Beef says \"You are now following me, ha\""), None);
    assert_eq!(s.role, Role::None);
    assert_eq!(s.observe("A kobold thief started to follow you around the room."), None);
}

#[test]
fn followers_excludes_invited_rows() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("You have invited Newguy to follow you.");
    assert_eq!(s.followers(), vec!["Pootwaddle".to_string()]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test party 2>&1 | tail -5`
Expected: a compile error, `could not find party in mud_client`.

- [ ] **Step 3: Write the module**

```rust
// crates/mud-client/src/party.rs
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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;
use std::time::Instant;

use regex::Regex;
use serde::{Deserialize, Serialize};

use mud_core::content::Content;

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
```

- [ ] **Step 4: Register the module and run the tests**

Add `pub mod party;` to `crates/mud-client/src/lib.rs` after `pub mod pack;`.

Run: `cargo test -p mud-client --test party 2>&1 | tail -5`
Expected: `test result: ok. 10 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/party.rs crates/mud-client/src/lib.rs crates/mud-client/tests/party.rs
git commit -m "feat(client): the party state machine reads the board's follow lines"
```

---

### Task 2: The telepath grammar, permission, the hold set, the wait state, the config

**Files:**
- Modify: `crates/mud-client/src/party.rs`
- Test: `crates/mud-client/tests/party.rs`

**Interfaces:**
- Consumes: `PartyState` from Task 1.
- Produces: `party::Remote`, `party::Request`, `party::remote(line) -> Option<Request>`, `party::permitted(&PartyState, &str) -> bool`, `party::telepath(to, text) -> String`, `party::Holds::{new, hold, release, expire, is_empty, names, retain_members, clear}`, `party::bank_names(&Content) -> BTreeSet<String>`, `party::WaitState::{new, on_rest_sent, on_prompt, on_refused, reset} -> Option<Signal>`, `party::Signal::{Wait, Ok}`, `party::PartyConfig{wait_secs, bank_wait_secs, follow_normal}` with `Default` and `validate`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/party.rs`:

```rust
use mud_client::party::{Holds, PartyConfig, Remote, Request, Signal, WaitState, bank_names, permitted, remote, telepath};
use mud_client::events::Status;

#[test]
fn a_telepath_with_a_known_word_is_a_request() {
    assert_eq!(
        remote("Pootwaddle telepaths: @bank"),
        Some(Request { from: "Pootwaddle".into(), command: Remote::Bank })
    );
    assert_eq!(remote("Beef telepaths: @WAIT").map(|r| r.command), Some(Remote::Wait));
    assert_eq!(remote("Beef telepaths: @Ok now").map(|r| r.command), Some(Remote::Ok));
}

#[test]
fn chat_unknown_words_and_the_send_echo_are_not_requests() {
    assert_eq!(remote("Pootwaddle telepaths: hello there"), None);
    assert_eq!(remote("Pootwaddle telepaths: @reroll"), None);
    assert_eq!(remote("--- Telepath Sent to Pootwaddle ---"), None);
    assert_eq!(remote("Pootwaddle gangpaths: @bank"), None);
}

#[test]
fn only_the_leader_and_joined_members_are_permitted() {
    let mut s = PartyState::new();
    s.observe("You are now following Beef");
    assert!(permitted(&s, "beef"));
    assert!(!permitted(&s, "Stranger"));

    let mut l = PartyState::new();
    l.observe("Pootwaddle started to follow you.");
    l.observe("You have invited Newguy to follow you.");
    assert!(permitted(&l, "POOTWADDLE"));
    assert!(!permitted(&l, "Newguy"));
    assert!(!permitted(&l, "Stranger"));
}

#[test]
fn the_wire_form_is_a_slash_and_the_name() {
    assert_eq!(telepath("Beef", "@ok"), "/Beef @ok");
}

#[test]
fn holds_expire_release_and_follow_the_roster() {
    let t0 = Instant::now();
    let mut h = Holds::new();
    assert!(h.is_empty());
    h.hold("Pootwaddle", t0 + Duration::from_secs(10));
    h.hold("Blueberry", t0 + Duration::from_secs(20));
    assert_eq!(h.names(), vec!["Blueberry".to_string(), "Pootwaddle".to_string()]);
    h.release("pootwaddle");
    assert_eq!(h.names(), vec!["Blueberry".to_string()]);
    assert_eq!(h.expire(t0 + Duration::from_secs(15)), Vec::<String>::new());
    assert_eq!(h.expire(t0 + Duration::from_secs(21)), vec!["Blueberry".to_string()]);
    assert!(h.is_empty());

    h.hold("Gone", t0 + Duration::from_secs(60));
    let mut s = PartyState::new();
    s.observe("Pootwaddle started to follow you.");
    h.retain_members(&s);
    assert!(h.is_empty());
}

#[test]
fn a_hold_is_extended_not_shortened() {
    let t0 = Instant::now();
    let mut h = Holds::new();
    h.hold("Pootwaddle", t0 + Duration::from_secs(60));
    h.hold("Pootwaddle", t0 + Duration::from_secs(10));
    assert!(h.expire(t0 + Duration::from_secs(30)).is_empty());
}

#[test]
fn bank_names_are_the_shop_type_seven_rooms() {
    use mud_core::content::{Room, RoomId, Shop, ShopId, ShopStock};
    let mut c = Content::default();
    c.add_shop(Shop { id: ShopId(8), name: "Bank of Godfrey".into(), shop_type: 7, min_level: 0, max_level: 0, markup: 0, class_limit: 0, stock: [ShopStock::default(); 20] });
    c.add_shop(Shop { id: ShopId(9), name: "Weapons".into(), shop_type: 1, min_level: 0, max_level: 0, markup: 0, class_limit: 0, stock: [ShopStock::default(); 20] });
    c.add_room(Room { id: RoomId { map: 1, room: 297 }, name: "Bank of Godfrey".into(), room_type: 1, shop: Some(ShopId(8)), ..Default::default() });
    c.add_room(Room { id: RoomId { map: 1, room: 298 }, name: "Armoury".into(), room_type: 1, shop: Some(ShopId(9)), ..Default::default() });
    let names = bank_names(&c);
    assert!(names.contains("Bank of Godfrey"));
    assert!(!names.contains("Armoury"));
}

#[test]
fn wait_state_signals_once_each_way() {
    let mut w = WaitState::new();
    assert_eq!(w.on_rest_sent(), Some(Signal::Wait));
    assert_eq!(w.on_rest_sent(), None);
    assert_eq!(w.on_prompt(Some(&Status::Resting)), None);
    assert_eq!(w.on_prompt(Some(&Status::Resting)), None);
    assert_eq!(w.on_prompt(None), Some(Signal::Ok));
    assert_eq!(w.on_prompt(None), None);
}

#[test]
fn a_refused_rest_releases_at_once() {
    let mut w = WaitState::new();
    w.on_rest_sent();
    assert_eq!(w.on_refused(), Some(Signal::Ok));
    assert_eq!(w.on_refused(), None);
    assert_eq!(w.on_prompt(None), None);
}

#[test]
fn party_config_defaults_and_refuses_zero_waits() {
    let cfg = PartyConfig::default();
    assert_eq!(cfg.wait_secs, 90);
    assert_eq!(cfg.bank_wait_secs, 15);
    assert!(cfg.follow_normal);
    assert!(cfg.validate().is_ok());
    assert!(PartyConfig { wait_secs: 0, ..Default::default() }.validate().is_err());
    assert!(PartyConfig { bank_wait_secs: 0, ..Default::default() }.validate().is_err());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test party 2>&1 | grep -c "cannot find"`
Expected: a non-zero count of unresolved names.

- [ ] **Step 3: Add the code**

Append to `crates/mud-client/src/party.rs`:

```rust
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
/// wait.
#[derive(Debug, Clone, Default)]
pub struct Holds {
    by: BTreeMap<String, Instant>,
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
        let slot = self.by.entry(Holds::key(name)).or_insert(until);
        if until > *slot {
            *slot = until;
        }
    }

    pub fn release(&mut self, name: &str) {
        self.by.remove(&Holds::key(name));
    }

    /// Drop and return every name whose deadline has passed.
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        let gone: Vec<String> = self.by.iter().filter(|(_, t)| **t <= now).map(|(n, _)| n.clone()).collect();
        for n in &gone {
            self.by.remove(n);
        }
        gone
    }

    pub fn is_empty(&self) -> bool {
        self.by.is_empty()
    }

    /// Held names as they were given, lowercased, sorted.
    pub fn names(&self) -> Vec<String> {
        self.by.keys().cloned().collect()
    }

    pub fn retain_members(&mut self, state: &PartyState) {
        self.by.retain(|n, _| permitted(state, n));
    }

    pub fn clear(&mut self) {
        self.by.clear();
    }
}

/// The names of the bank rooms, unique across the shipped world,
/// checked 2026-09-12. A room block naming one is a bank arrival with
/// no localisation.
pub fn bank_names(content: &Content) -> BTreeSet<String> {
    content
        .rooms
        .values()
        .filter(|room| room.room_type == 1)
        .filter(|room| {
            room.shop
                .and_then(|s| content.shops.get(&s))
                .is_some_and(|shop| shop.shop_type == crate::bank::BANK_SHOP_TYPE)
        })
        .map(|room| room.name.clone())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Wait,
    Ok,
}

/// A follower's side of the wait handshake. `@wait` goes out with the
/// rest, `@ok` on the first prompt without the Resting status after a
/// prompt that had it, or at once when the board refused the rest.
#[derive(Debug, Clone, Default)]
pub struct WaitState {
    waiting: bool,
    seen_resting: bool,
}

impl WaitState {
    pub fn new() -> WaitState {
        WaitState::default()
    }

    pub fn on_rest_sent(&mut self) -> Option<Signal> {
        if self.waiting {
            return None;
        }
        self.waiting = true;
        self.seen_resting = false;
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
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test party 2>&1 | tail -3`
Expected: `test result: ok. 20 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/party.rs crates/mud-client/tests/party.rs
git commit -m "feat(client): the telepath grammar, party permission, holds and the wait state"
```

---

### Task 3: The `[party]` profile table and its live settings keys

**Files:**
- Modify: `crates/mud-client/src/profile.rs:66-70` (after the `bank` field)
- Modify: `crates/mud-client/src/settings.rs:81-86` (`KEYS`) and `:384` (validate)
- Test: `crates/mud-client/tests/settings.rs:159-200`

**Interfaces:**
- Produces: `Profile::party: PartyConfig`.

- [ ] **Step 1: Extend the key coverage test**

In `crates/mud-client/tests/settings.rs`, the `every_key_the_profile_serialises_is_in_keys` test builds a `full` profile. `PartyConfig` has no `Option` fields, so the default serialises every key. Add a second test below it:

```rust
#[test]
fn party_keys_are_live() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.toml");
    std::fs::write(&path, "target = \"mbbsemu\"\nhost = \"h\"\nport = 1\nusername = \"u\"\npassword = \"p\"\n").unwrap();
    let mut s = Settings::load(&path).unwrap();
    s.set("party.bank_wait_secs", "30").unwrap();
    assert_eq!(s.profile().party.bank_wait_secs, 30);
    assert!(s.set("party.wait_secs", "0").is_err(), "zero is refused by validate");
    assert_eq!(s.profile().party.wait_secs, 90, "a refused set leaves the value alone");
}
```

Match the load and construction calls to how the tests above it build a `Settings` from a file. If they use a helper, use that helper.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test settings 2>&1 | tail -5`
Expected: compile error, no field `party` on `Profile`.

- [ ] **Step 3: Add the table and the keys**

In `profile.rs`, after the `bank` field:

```rust
    /// Party behaviour, as `[party]`. Absent means the defaults. See
    /// [`crate::party::PartyConfig`].
    #[serde(default)]
    pub party: crate::party::PartyConfig,
```

In `settings.rs` `KEYS`, after `"bank.at",`:

```rust
    "party.wait_secs",
    "party.bank_wait_secs",
    "party.follow_normal",
```

In `settings.rs` beside `profile.bank.validate().map_err(ParseFailure::Invalid)?;` add:

```rust
    profile.party.validate().map_err(ParseFailure::Invalid)?;
```

Check `Profile::validate` or wherever `bank.validate()` is also called at profile load in `profile.rs`, and add the party call beside it.

- [ ] **Step 4: Run the whole client test suite**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked" | head`
Expected: every suite `ok`. Struct literals of `Profile` in tests use `..Default::default()`, so nothing else needs the new field.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/profile.rs crates/mud-client/src/settings.rs crates/mud-client/tests/settings.rs
git commit -m "feat(client): the [party] table, wait_secs, bank_wait_secs and follow_normal"
```

---

### Task 4: The session's party tracker

**Files:**
- Modify: `crates/mud-client/src/session.rs` (struct at `:340-386`, constructor `:389-637`, feeds at `:579-582` and `:603-606`, accessors after `contents()` at `:980`)
- Test: `crates/mud-client/tests/party_session.rs`

**Interfaces:**
- Consumes: `party::{PartyState, Holds, Request, remote, permitted}`.
- Produces: `Session::party() -> PartyState`, `Session::party_changes() -> watch::Receiver<PartyState>`, `Session::party_notes() -> broadcast::Receiver<String>`, `Session::take_party_requests() -> Vec<Request>`, `Session::party_hold(name, until)`, `Session::party_holds_clear() -> bool`, `Session::party_expire_holds() -> Vec<String>`, `Session::party_holds() -> Arc<Mutex<Holds>>`.

- [ ] **Step 1: Write the failing test**

```rust
// crates/mud-client/tests/party_session.rs
//! The session learns the party from the board's lines and keeps the
//! holds and requests for whoever drains them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::party::{Remote, Role};
use mud_client::profile::Profile;
use mud_client::session::Session;

/// A board that prints `lines` after the prompt, one every 50ms, and
/// echoes anything sent.
async fn board(lines: Vec<&'static str>) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP=30/MA=0]:").await.unwrap();
        for line in lines {
            tokio::time::sleep(Duration::from_millis(50)).await;
            sock.write_all(format!("\r\n{line}\r\n[HP=30/MA=0]:").as_bytes()).await.unwrap();
        }
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            for l in text.lines() {
                log.lock().unwrap().push(l.trim().to_string());
            }
            sock.write_all(format!("\r\n{}\r\n[HP=30/MA=0]:", text.trim()).as_bytes()).await.unwrap();
        }
    });
    (addr, received)
}

async fn session_for(addr: std::net::SocketAddr) -> Session {
    let profile = Profile {
        target: mud_client::dialect::Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "testuser".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

async fn wait_until(session: &Session, pred: impl Fn(&Session) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !pred(session) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the session should reach the state");
}

#[tokio::test]
async fn the_session_learns_it_is_following_and_queues_the_leaders_requests() {
    let (addr, _) = board(vec![
        "You are now following Beef",
        "Beef telepaths: @wait",
        "Stranger telepaths: @wait",
        "Beef telepaths: @bank",
    ])
    .await;
    let session = session_for(addr).await;
    wait_until(&session, |s| s.party().role == Role::Follower).await;
    assert_eq!(session.party().leader.as_deref(), Some("Beef"));
    wait_until(&session, |s| !s.party_holds_clear()).await;
    assert_eq!(session.party_holds().lock().unwrap().names(), vec!["beef".to_string()]);
    wait_until(&session, |s| s.party().role == Role::Follower && s.party_holds().lock().unwrap().names().len() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let requests = session.take_party_requests();
    assert_eq!(requests.len(), 1, "the stranger's @wait and the stranger are dropped");
    assert_eq!(requests[0].from, "Beef");
    assert_eq!(requests[0].command, Remote::Bank);
    assert!(session.take_party_requests().is_empty(), "drained");
}

#[tokio::test]
async fn ok_releases_a_hold_and_the_party_ending_clears_everything() {
    let (addr, _) = board(vec![
        "Pootwaddle started to follow you.",
        "Pootwaddle telepaths: @wait",
        "Pootwaddle telepaths: @ok",
        "Pootwaddle telepaths: @wait",
        "You are not in a party at the present time.",
    ])
    .await;
    let session = session_for(addr).await;
    let mut changes = session.party_changes();
    wait_until(&session, |s| s.party().role == Role::Leader).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(session.party().role, Role::None);
    assert!(session.party_holds_clear());
    assert!(changes.has_changed().unwrap());
}

#[tokio::test]
async fn notes_name_every_change_and_accepted_request() {
    let (addr, _) = board(vec!["Pootwaddle started to follow you.", "Pootwaddle telepaths: @bank"]).await;
    let session = session_for(addr).await;
    let mut notes = session.party_notes();
    let first = tokio::time::timeout(Duration::from_secs(5), notes.recv()).await.unwrap().unwrap();
    assert_eq!(first, "party: Pootwaddle joined");
    let second = tokio::time::timeout(Duration::from_secs(5), notes.recv()).await.unwrap().unwrap();
    assert_eq!(second, "party: Pootwaddle asks @bank");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test party_session 2>&1 | tail -5`
Expected: compile errors, no method `party` on `Session`.

- [ ] **Step 3: Add the tracker**

In `session.rs`, near `ContentsTracker` at `:281`:

```rust
/// The party the character is in, the followers holding it still, and
/// the requests waiting for whoever drains them. Fed every line, so the
/// state survives a flee's bot rebuild and is readable from every mode.
/// See `docs/superpowers/specs/2026-09-12-party-mechanics-design.md`.
struct PartyTracker {
    state: crate::party::PartyState,
    holds: Arc<Mutex<crate::party::Holds>>,
    requests: Vec<crate::party::Request>,
    changes: watch::Sender<crate::party::PartyState>,
    notes: broadcast::Sender<String>,
    wait_secs: u64,
}
```

Add a field `party: Arc<Mutex<PartyTracker>>` to `Session`, construct it in `connect` beside `contents` with `wait_secs: profile.party.wait_secs`, `changes: watch::Sender::new(PartyState::new())`, `notes: broadcast::channel(64).0`, and add `feed_party(&party, &cor);` right after `feed_equipment(&equipment, &cor);` at both feed sites.

The feed function, beside `feed_equipment`:

```rust
fn feed_party(party: &Mutex<PartyTracker>, cor: &Correlated) {
    let mut t = party.lock().expect("party lock");
    let change = match &cor.event {
        Event::Line(line) => t.state.observe(line),
        Event::Prompt { .. } => t.state.observe_prompt(),
        _ => None,
    };
    if let Some(change) = change {
        use crate::party::Change;
        let note = match &change {
            Change::Following(who) => format!("party: following {who}"),
            Change::Joined(who) => format!("party: {who} joined"),
            Change::Invited(who) => format!("party: invited {who}"),
            Change::Left(who) => format!("party: {who} left"),
            Change::Ended => "party: ended".to_string(),
            Change::Roster => format!("party: roster {}", t.state.followers().join(", ")),
        };
        if change == Change::Ended {
            t.holds.lock().expect("holds lock").clear();
            t.requests.clear();
        } else {
            let state = t.state.clone();
            t.holds.lock().expect("holds lock").retain_members(&state);
        }
        let _ = t.notes.send(note);
        let state = t.state.clone();
        let _ = t.changes.send(state);
    }
    let Event::Line(line) = &cor.event else { return };
    let Some(req) = crate::party::remote(line) else { return };
    if !crate::party::permitted(&t.state, &req.from) {
        return;
    }
    let word = match req.command {
        crate::party::Remote::Bank => "@bank",
        crate::party::Remote::Wait => "@wait",
        crate::party::Remote::Ok => "@ok",
    };
    let _ = t.notes.send(format!("party: {} asks {word}", req.from));
    match req.command {
        crate::party::Remote::Wait => {
            let until = Instant::now() + Duration::from_secs(t.wait_secs);
            t.holds.lock().expect("holds lock").hold(&req.from, until);
        }
        crate::party::Remote::Ok => t.holds.lock().expect("holds lock").release(&req.from),
        crate::party::Remote::Bank => t.requests.push(req),
    }
}
```

`set_profile` must also update `wait_secs` from the new profile, beside where it stores the profile.

Accessors, after `contents()`:

```rust
    pub fn party(&self) -> crate::party::PartyState {
        self.party.lock().expect("party lock").state.clone()
    }

    pub fn party_changes(&self) -> watch::Receiver<crate::party::PartyState> {
        self.party.lock().expect("party lock").changes.subscribe()
    }

    /// One line per change, request accepted, and request sent, in the
    /// shape `rests()` uses, for the window to print.
    pub fn party_notes(&self) -> broadcast::Receiver<String> {
        self.party.lock().expect("party lock").notes.subscribe()
    }

    /// The tracker's own sender, for the assist and the jobs to say what
    /// they telepathed.
    pub fn party_note(&self, text: String) {
        let _ = self.party.lock().expect("party lock").notes.send(text);
    }

    pub fn take_party_requests(&self) -> Vec<crate::party::Request> {
        std::mem::take(&mut self.party.lock().expect("party lock").requests)
    }

    pub fn party_holds(&self) -> Arc<Mutex<crate::party::Holds>> {
        Arc::clone(&self.party.lock().expect("party lock").holds)
    }

    pub fn party_hold(&self, name: &str, until: Instant) {
        self.party_holds().lock().expect("holds lock").hold(name, until);
    }

    pub fn party_holds_clear(&self) -> bool {
        self.party_holds().lock().expect("holds lock").is_empty()
    }

    pub fn party_expire_holds(&self) -> Vec<String> {
        self.party_holds().lock().expect("holds lock").expire(Instant::now())
    }
```

`Instant` and `Duration` are already imported in `session.rs`. Add `use std::sync::Mutex` if the file names it another way.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test party_session 2>&1 | tail -5`
Expected: `test result: ok. 3 passed`.

Then: `cargo test -p mud-client 2>&1 | grep -E "^test result" | grep -v " 0 failed" | head`
Expected: no output, every suite passes.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/session.rs crates/mud-client/tests/party_session.rs
git commit -m "feat(client): the session tracks the party, its holds and its requests"
```

---

### Task 5: Notices, `set follow normal`, the status bar, and jobs that refuse while following

**Files:**
- Modify: `crates/mud-client/src/window.rs` (the event loop near `:855-905` where `session.rests()` is consumed, the assist construction near `:705`, `bar_text` at `:573`)
- Modify: `crates/mud-client/src/tui.rs` (`render_status` at `:1704`, `start_bank` at `:2259` and the other `start_*` functions, `needs_name`)
- Test: `crates/mud-client/tests/party_follower.rs` (new), an existing `start_bank` refusal test to mirror

**Interfaces:**
- Produces: `tui::not_following(&Session) -> Result<(), String>`, `render_status` gains `party: &PartyState`.

- [ ] **Step 1: Write the failing tests**

Find the existing test that pins `start_bank`'s `needs_name` refusal: `grep -rn "start_bank" crates/mud-client/tests/`. Add beside it, in the same file with the same session helper:

```rust
#[tokio::test]
async fn start_bank_refuses_while_following() {
    // Build the session as the test above does, then feed the follow line.
    // The board helper prints lines after the prompt, so put
    // "You are now following Beef" in its script and wait for
    // session.party().role == Role::Follower before calling start_bank.
    let err = start_bank(session.clone(), graph, None, BotConfig::default(), quiet()).err().unwrap();
    assert_eq!(err, "following Beef; a job that moves does not run in a party");
}
```

Adapt the helper names to the file. If no such test exists, create `crates/mud-client/tests/party_follower.rs` with the `board` and `session_for` helpers from Task 4's test file, and put this test there.

And in `crates/mud-client/tests/party.rs`, a pure render test:

```rust
#[test]
fn the_bar_names_the_party() {
    use mud_client::tui::render_status;
    use mud_client::world::GameState;
    let mut s = PartyState::new();
    s.observe("You are now following Beef");
    let text = render_status(&GameState::default(), Instant::now(), None, Default::default(), None, None, None, false, &s, 120);
    assert!(text.contains("following Beef"), "{text}");
    let mut l = PartyState::new();
    l.observe("A started to follow you.");
    l.observe("B started to follow you.");
    let text = render_status(&GameState::default(), Instant::now(), None, Default::default(), None, None, None, false, &l, 120);
    assert!(text.contains("leading 2"), "{text}");
}
```

Check `render_status`'s current parameter list at `tui.rs:1704` and `crate::lost::Fix`'s `Default`. Match the call to the real signature after adding the `party` parameter before `width`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test party 2>&1 | tail -5`
Expected: compile error on `render_status`'s arity.

- [ ] **Step 3: Implement**

In `tui.rs`, beside `needs_name`:

```rust
/// A job that moves does not run while the character is following
/// someone: the board drags a follower wherever the leader walks, and a
/// walk of its own would fight the drag. Public for the test that pins
/// the refusal.
pub fn not_following(session: &Session) -> Result<(), String> {
    let party = session.party();
    match party.leader {
        Some(leader) if party.is_follower() => {
            Err(format!("following {leader}; a job that moves does not run in a party"))
        }
        _ => Ok(()),
    }
}
```

Call `not_following(&session)?;` right after `needs_name(&session)?;` in `start_bank`, and in the same place in every other `start_*` that spawns a walking job: `grep -n "needs_name(&session)?" crates/mud-client/src/tui.rs` lists them. That is `/go`, `/farm`, `/roam`, `/recover` and `/bank`.

In `render_status`, add `party: &crate::party::PartyState` before `width`, and after the `assist | ` prefix block:

```rust
    match party.role {
        crate::party::Role::Follower => {
            s.push_str(&format!("following {} | ", party.leader.as_deref().unwrap_or("?")));
        }
        crate::party::Role::Leader => {
            s.push_str(&format!("leading {} | ", party.followers().len()));
        }
        crate::party::Role::None => {}
    }
```

Update `bar_text` in `window.rs` and every other caller of `render_status` to pass `&session.party()`.

In `window.rs`'s event loop, where `session.rests()` lines are received and printed, add a `party_notes = session.party_notes()` receiver beside it and print each note as `w.note(&format!("-- {note} --"))`. Also, when a note starts with `party: following ` and `session.profile().party.follow_normal` is true, send `session.send("set follow normal")`. Model the select arm on the rests arm exactly.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result" | grep -v " 0 failed"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/src/window.rs crates/mud-client/tests/
git commit -m "feat(client): party notices, set follow normal, the bar's party word, jobs refuse while following"
```

---

### Task 6: `bank::deposit_here`, the errand's at-the-bank half

**Files:**
- Modify: `crates/mud-client/src/bank.rs:335-470` (`errand`)
- Test: `crates/mud-client/tests/bank_scripted.rs` (existing tests must still pass), plus one new test there

**Interfaces:**
- Produces: `pub async fn deposit_here(session: &Session, bank: &BankConfig, name: &str, at: RoomId, stats: &mut FarmStats) -> ErrandEnd`.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/bank_scripted.rs`:

```rust
/// The at-the-bank half on its own: standing in the bank already, read,
/// deposit above the floor, read again.
#[tokio::test]
async fn deposit_here_reads_deposits_and_reads_again() {
    use mud_client::bank::deposit_here;
    use mud_client::farm::FarmStats;
    let (addr, received) = scripted_board(vec![
        ("i", "\r\ni\r\nYou are carrying 15 gold crowns\r\nYou have no keys.\r\nEncumbrance: 5/2400 - None [0%]\r\n[HP=30/MA=0]:".into()),
        ("deposit 1000", "\r\ndeposit 1000\r\nYou deposit 10 gold crowns.\r\n[HP=30/MA=0]:".into()),
        ("i", "\r\ni\r\nYou are carrying 5 gold crowns\r\nYou have no keys.\r\nEncumbrance: 1/2400 - None [0%]\r\n[HP=30/MA=0]:".into()),
    ])
    .await;
    let session = session_for(addr, 5).await;
    let mut stats = FarmStats::default();
    let end = tokio::time::timeout(
        Duration::from_secs(20),
        deposit_here(&session, &session.profile().bank, "Bank of Godfrey", BANK, &mut stats),
    )
    .await
    .unwrap();
    assert_eq!(end, ErrandEnd::Deposited { farthings: 1000, at: BANK, bank: "Bank of Godfrey".into() }, "{:?}", received.lock().unwrap());
    assert_eq!(stats.deposits, 1);
    assert_eq!(session.capabilities().purse.farthings(), 500);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test bank_scripted deposit_here 2>&1 | tail -3`
Expected: compile error, no `deposit_here`.

- [ ] **Step 3: Split the function**

In `bank.rs`, move the block of `errand` that begins `let purse = read_inventory(session).await.coins().purse();` and ends with the `match reply { ... }` producing `end` into:

```rust
/// The errand's at-the-bank half: read the purse, deposit above the
/// keep floor, read again so the purse meter and the pack agree. A
/// follower dragged into a bank runs this on its own.
pub async fn deposit_here(
    session: &Session,
    bank: &BankConfig,
    name: &str,
    at: RoomId,
    stats: &mut FarmStats,
) -> ErrandEnd {
    let purse = read_inventory(session).await.coins().purse();
    let farthings = purse.farthings().saturating_sub(bank.keep().farthings());
    if farthings == 0 {
        return ErrandEnd::Nothing(format!("nothing above the keep floor at {name}"));
    }
    let reply = send_deposit(session, farthings).await;
    let _ = read_inventory(session).await;
    match reply {
        Some(DepositReply::Deposited(_)) => {
            stats.deposits += 1;
            stats.deposited_farthings += farthings;
            ErrandEnd::Deposited { farthings, at, bank: name.to_string() }
        }
        Some(DepositReply::NotABank) => {
            stats.relocalizations += 1;
            ErrandEnd::Nothing(format!(
                "the board says {}/{} is not a bank: the walk did not land where the graph says",
                at.map, at.room
            ))
        }
        Some(DepositReply::Unreasonable) => ErrandEnd::Nothing(format!("the board refused a deposit of {farthings}")),
        None => ErrandEnd::Nothing("no reply to the deposit".into()),
    }
}
```

and in `errand` replace that block with `let end = deposit_here(session, bank, &name, to, stats).await;`. Keep every comment that was on the moved lines with the lines.

- [ ] **Step 4: Run the bank suites**

Run: `cargo test -p mud-client --test bank_scripted --test bank 2>&1 | grep -E "^test result"`
Expected: both `ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/bank.rs crates/mud-client/tests/bank_scripted.rs
git commit -m "refactor(client): the errand's deposit at the bank is its own function"
```

---

### Task 7: The follower deposits when dragged into a bank and says `@ok`

**Files:**
- Modify: `crates/mud-client/src/tui.rs` (a `start_follower_deposit` beside `start_bank` at `:2259`)
- Modify: `crates/mud-client/src/window.rs` (the event loop, in the `job.is_none()` assist block near `:868`)
- Test: `crates/mud-client/tests/party_follower.rs`

**Interfaces:**
- Consumes: `bank::deposit_here`, `party::{bank_names, telepath}`, `Session::{party, bot_on, party_note}`.
- Produces: `tui::start_follower_deposit(session: Arc<Session>, content: Arc<Content>, room: &str, notices: Notices) -> Job`, `tui::follower_bank_arrival(session: &Session, bot_on: bool, cfg: &BankConfig, names: &BTreeSet<String>, cor: &Correlated) -> Option<String>`.

- [ ] **Step 1: Write the failing tests**

`crates/mud-client/tests/party_follower.rs` needs a board that starts the character in the realm, answers `stat`, `health` and `inventory` as `tests/assist_window.rs`'s `low_board` does, then after a delay prints the follow line and a drag into the bank, and answers `i` and `deposit` from a script. Use the `scripted_board` shape from `bank_scripted.rs` for the answers, with an extra `push` list printed unprompted after connect, one line every 100ms. Copy both helpers into this file, they are small, and give the board the pushes:

```rust
const FOLLOW: &str = "You are now following Beef";
const DRAG: &str = " -- Following your Party leader east --\r\n\r\n\x1b[1;36mBank of Godfrey\r\nObvious exits: west\r\n[HP=30/MA=0]:";
```

The pure decision test, in the same file:

```rust
#[test]
fn a_bank_room_block_while_following_starts_the_deposit() {
    use mud_client::correlate::Correlated;
    use mud_client::events::{Event, RoomView};
    use mud_client::party::PartyState;
    use mud_client::tui::follower_bank_arrival_decision;
    let mut party = PartyState::new();
    party.observe(FOLLOW);
    let names: std::collections::BTreeSet<String> = ["Bank of Godfrey".to_string()].into();
    let room = RoomView { name: "Bank of Godfrey".into(), ..Default::default() };
    assert_eq!(follower_bank_arrival_decision(&party, true, true, &names, &Event::RoomSeen(room.clone())), Some("Bank of Godfrey".to_string()));
    assert_eq!(follower_bank_arrival_decision(&party, false, true, &names, &Event::RoomSeen(room.clone())), None, "bot off");
    assert_eq!(follower_bank_arrival_decision(&party, true, false, &names, &Event::RoomSeen(room.clone())), None, "auto_deposit off");
    assert_eq!(follower_bank_arrival_decision(&PartyState::new(), true, true, &names, &Event::RoomSeen(room.clone())), None, "not following");
    let other = RoomView { name: "Home".into(), ..Default::default() };
    assert_eq!(follower_bank_arrival_decision(&party, true, true, &names, &Event::RoomSeen(other)), None);
}
```

The scripted test:

```rust
#[tokio::test]
async fn a_follower_dragged_into_the_bank_deposits_and_says_ok() {
    let (addr, received) = scripted_board(
        vec![FOLLOW.into(), DRAG.into()],
        vec![
            ("i", "\r\ni\r\nYou are carrying 15 gold crowns\r\nYou have no keys.\r\nEncumbrance: 5/2400 - None [0%]\r\n[HP=30/MA=0]:".into()),
            ("deposit 1500", "\r\ndeposit 1500\r\nYou deposit 15 gold crowns.\r\n[HP=30/MA=0]:".into()),
            ("i", "\r\ni\r\nYou are carrying nothing\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:".into()),
        ],
    )
    .await;
    let session = Arc::new(session_for(addr).await);
    session.set_content(Arc::new(content()));
    let job = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if session.party().is_follower() { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // The window would start this off the room block. Drive it directly.
        mud_client::tui::start_follower_deposit(session.clone(), Arc::new(content()), "Bank of Godfrey", quiet())
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(20), job.handle).await.unwrap().unwrap();
    let log = received.lock().unwrap().clone();
    let deposit = log.iter().position(|l| l == "deposit 1500").expect("the deposit");
    let ok = log.iter().position(|l| l == "/beef @ok").expect("the ok");
    assert!(deposit < ok, "{log:?}");
}

#[tokio::test]
async fn a_follower_with_nothing_to_deposit_says_ok_at_once() {
    let (addr, received) = scripted_board(
        vec![FOLLOW.into(), DRAG.into()],
        vec![("i", "\r\ni\r\nYou are carrying nothing\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:".into())],
    )
    .await;
    let session = Arc::new(session_for(addr).await);
    session.set_content(Arc::new(content()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !session.party().is_follower() { tokio::time::sleep(Duration::from_millis(20)).await; }
    }).await.unwrap();
    let job = mud_client::tui::start_follower_deposit(session.clone(), Arc::new(content()), "Bank of Godfrey", quiet());
    tokio::time::timeout(Duration::from_secs(20), job.handle).await.unwrap().unwrap();
    let log = received.lock().unwrap().clone();
    assert!(log.iter().any(|l| l == "/beef @ok"), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("deposit")), "{log:?}");
}
```

`content()` is the fixture from `bank_scripted.rs`. `session_for` here has `pace_ms: Some(0)` and a default `[bank]` with `keep_gold: 0`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test party_follower 2>&1 | tail -5`
Expected: compile errors for the two new `tui` functions.

- [ ] **Step 3: Implement**

In `tui.rs`:

```rust
/// Whether this room block is a bank arrival the follower should act
/// on: following, the switch on, deposits on, and the room named as a
/// bank. Returns the bank's name.
pub fn follower_bank_arrival_decision(
    party: &crate::party::PartyState,
    bot_on: bool,
    auto_deposit: bool,
    banks: &std::collections::BTreeSet<String>,
    ev: &crate::events::Event,
) -> Option<String> {
    let crate::events::Event::RoomSeen(room) = ev else { return None };
    if !(party.is_follower() && bot_on && auto_deposit && banks.contains(&room.name)) {
        return None;
    }
    Some(room.name.clone())
}

/// The follower's deposit on a bank arrival, run as a short automatic
/// task in the job slot so the assist stays quiet for its duration, as
/// it does for a job. Ends with `@ok` to the leader whatever happened.
/// Automatic and under `/bot`, so `not_following` does not apply.
pub fn start_follower_deposit(
    session: Arc<Session>,
    content: Arc<mud_core::content::Content>,
    room: &str,
    notices: crate::farm::Notices,
) -> Job {
    let (tx, rx) = tokio::sync::watch::channel(crate::farm::Phase::default());
    let room = room.to_string();
    let handle = tokio::spawn(async move {
        let bank = session.profile().bank;
        let at = crate::bank::bank_rooms(&content)
            .into_iter()
            .find(|(_, name)| *name == room)
            .map(|(id, _)| id)
            .unwrap_or(mud_core::content::RoomId { map: 0, room: 0 });
        let mut stats = crate::farm::FarmStats::default();
        let end = crate::bank::deposit_here(&session, &bank, &room, at, &mut stats).await;
        let why = match &end {
            crate::bank::ErrandEnd::Deposited { farthings, bank, .. } => format!("deposited {farthings} copper farthings at {bank}"),
            crate::bank::ErrandEnd::Nothing(why) => format!("nothing deposited: {why}"),
            other => format!("{other:?}"),
        };
        notices(&format!("party: {why}"));
        if let Some(leader) = session.party().leader {
            session.send(&crate::party::telepath(&leader, "@ok"));
            session.party_note(format!("party: told {leader} @ok"));
        }
        let _ = tx.send(crate::farm::Phase::Done { why, at: None });
    });
    Job { handle, phase: rx, what: "party deposit" }
}
```

`Job`'s fields are `handle`, `phase`, `what`, as `start_bank` builds them. If `Job.handle` is not public, make the test await the phase receiver reaching `Phase::Done` instead.

In `window.rs`, in the `job.is_none()` block before `assist_tick` runs, add:

```rust
                    if job.is_none()
                        && assist.is_some()
                        && let Some(content) = content.as_ref()
                        && let Some(bank) = follower_bank_arrival_decision(
                            &session.party(),
                            session.bot_on(),
                            session.profile().bank.auto_deposit,
                            &party_banks,
                            &cor.event,
                        )
                    {
                        w.note(&format!("-- party: at {bank}, depositing --"));
                        let started = start_follower_deposit(session.clone(), content.clone(), &bank, notices.clone());
                        phase_rx = Some(started.phase.clone());
                        job = Some(started);
                    }
```

with `let party_banks = content.as_ref().map(|c| crate::party::bank_names(c)).unwrap_or_default();` computed once beside `durations` near `:700`. Import the two functions from `tui` in the `use` at `window.rs:20`. Check that `content` in this scope is an `Option<Arc<Content>>` and that the assist block runs on the same `cor` this reads.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test party_follower 2>&1 | tail -5`
Expected: `3 passed`.

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result" | grep -v " 0 failed"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/src/window.rs crates/mud-client/tests/party_follower.rs
git commit -m "feat(client): a follower dragged into a bank deposits and tells the leader @ok"
```

---

### Task 8: The follower's deposit gate asks the leader with `@bank`

**Files:**
- Modify: `crates/mud-client/src/bank.rs` (a `FollowerGate` after `BankGate`)
- Modify: `crates/mud-client/src/tui.rs` (`AssistCasts` gains the gate, `assist_tick` drives it)
- Test: `crates/mud-client/tests/bank.rs` (pure), `crates/mud-client/tests/party_follower.rs` (scripted)

**Interfaces:**
- Produces: `bank::FollowerGate::{new, seed, on_pickup, wants_reading(now) -> bool, on_reading(&BankConfig, Reading, now) -> bool, deposited}`. `AssistCasts.follower: FollowerGate`.

- [ ] **Step 1: Write the failing tests**

In `crates/mud-client/tests/bank.rs`:

```rust
#[test]
fn the_follower_gate_asks_once_per_five_minutes() {
    use mud_client::bank::{BankConfig, FollowerGate, Reading, WeightClass};
    use mud_client::purse::Coins;
    use std::time::{Duration, Instant};
    let cfg = BankConfig { deposit_at_coins: 10, ..Default::default() };
    let t0 = Instant::now();
    let light = Reading { coins: Coins::default(), class: WeightClass::None };
    let heavy = Reading { coins: Coins { gold: 50, ..Default::default() }, class: WeightClass::None };
    let mut g = FollowerGate::new();
    g.seed(light);
    assert!(!g.wants_reading(t0), "no pickup yet");
    g.on_pickup();
    assert!(g.wants_reading(t0));
    assert!(g.on_reading(&cfg, heavy, t0), "over the mark: ask");
    assert!(!g.wants_reading(t0));
    g.on_pickup();
    assert!(g.wants_reading(t0 + Duration::from_secs(60)));
    assert!(!g.on_reading(&cfg, heavy, t0 + Duration::from_secs(60)), "asked a minute ago");
    g.on_pickup();
    assert!(g.on_reading(&cfg, heavy, t0 + Duration::from_secs(301)), "five minutes on: ask again");
    g.deposited();
    g.on_pickup();
    assert!(g.on_reading(&cfg, heavy, t0 + Duration::from_secs(302)), "a deposit resets the ask");
}
```

Check `Coins`'s field names in `purse.rs` and use its constructor if fields are private.

In `party_follower.rs`, a scripted test: the board pushes `FOLLOW`, then `You picked up 20 gold crowns` followed by a prompt, and answers `i` with 50 gold and `Encumbrance: 5/2400 - None [0%]`. Drive the assist as `tests/assist_window.rs` drives it, through `spawn` with a profile whose `[bot]` has `assist_play = true` and `max_hp = 30` and whose `[bank]` has `deposit_at_coins = 10`. Assert the board received `i` after the pickup line and then `/beef @bank`, once, even after a second pushed pickup line.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test bank the_follower_gate 2>&1 | tail -3`
Expected: compile error, no `FollowerGate`.

- [ ] **Step 3: Implement**

In `bank.rs` after `BankGate`:

```rust
/// A follower's deposit gate. It cannot walk, so on `Deposit` it asks
/// the leader with `@bank`, once, and not again until it has deposited
/// or five minutes have passed.
#[derive(Debug, Clone, Default)]
pub struct FollowerGate {
    gate: BankGate,
    pending: bool,
    asked_at: Option<Instant>,
}

pub const ASK_AGAIN_AFTER: std::time::Duration = std::time::Duration::from_secs(300);

impl FollowerGate {
    pub fn new() -> FollowerGate {
        FollowerGate::default()
    }

    pub fn seed(&mut self, reading: Reading) {
        self.gate.seed(reading);
    }

    /// The board confirmed a coin pickup. The next idle prompt reads.
    pub fn on_pickup(&mut self) {
        self.pending = true;
    }

    pub fn wants_reading(&self, _now: Instant) -> bool {
        self.pending
    }

    /// Judge the reading. True means send `@bank` now.
    pub fn on_reading(&mut self, cfg: &BankConfig, reading: Reading, now: Instant) -> bool {
        self.pending = false;
        if self.gate.judge(cfg, reading) != Judgement::Deposit {
            return false;
        }
        if self.asked_at.is_some_and(|t| now.duration_since(t) < ASK_AGAIN_AFTER) {
            return false;
        }
        self.asked_at = Some(now);
        true
    }

    pub fn deposited(&mut self) {
        self.asked_at = None;
    }
}
```

In `tui.rs`, add `pub follower: crate::bank::FollowerGate` and `pub follower_read: bool` to `AssistCasts` with defaults. In `assist_tick`, after the buff block and before `assist_actions`:

```rust
    let party = session.party();
    if party.is_follower() && cfg_bank.auto_deposit {
        if let crate::events::Event::Line(line) = &cor.event {
            if crate::bot::picked_up(line).is_some() {
                casts.follower.on_pickup();
            }
            if casts.follower_read && line.trim_start().starts_with("Encumbrance:") {
                casts.follower_read = false;
                if let Some(reading) = crate::bank::Reading::of(&session.contents())
                    && casts.follower.on_reading(&cfg_bank, reading, now)
                    && let Some(leader) = &party.leader
                {
                    session.send(&crate::party::telepath(leader, "@bank"));
                    session.party_note(format!("party: asked {leader} @bank"));
                }
            }
        }
        if let crate::events::Event::Prompt { .. } = &cor.event
            && !casts.follower_read
            && bot.is_idle()
            && casts.follower.wants_reading(now)
        {
            casts.follower_read = true;
            session.send("i");
        }
    }
```

`cfg_bank` is `session.profile().bank`, read once at the top of the tick. `Bot::is_idle()` is `self.engaged.is_none() && !self.healing`. Add it to `bot.rs` if there is no equivalent. Seed the gate from the first contents reading with an encumbrance line: when `*book_seen` changes and `Reading::of(&session.contents())` is `Some`, call `casts.follower.seed(reading)`.

The task in Task 7 marks a deposit: in `start_follower_deposit`'s caller in `window.rs`, after the job ends with `Phase::Done` and `what == "party deposit"`, call `casts.follower.deposited()` on the assist casts. Find where the window reacts to a job's `Phase::Done` and add it there.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test bank --test party_follower 2>&1 | grep -E "^test result"`
Expected: both `ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/bank.rs crates/mud-client/src/tui.rs crates/mud-client/src/window.rs crates/mud-client/src/bot.rs crates/mud-client/tests/bank.rs crates/mud-client/tests/party_follower.rs
git commit -m "feat(client): a follower's deposit gate asks the leader with @bank"
```

---

### Task 9: The follower's `@wait` and `@ok` around a rest

**Files:**
- Modify: `crates/mud-client/src/tui.rs` (`AssistCasts.wait: WaitState`, `assist_tick`)
- Test: `crates/mud-client/tests/party_follower.rs`

**Interfaces:**
- Consumes: `party::{WaitState, Signal, telepath}`, `HealWatch::on_event` returning true on a refusal.

- [ ] **Step 1: Write the failing test**

A scripted test in `party_follower.rs`: the board starts at `[HP=10/MA=0]`, answers `health` with `Health: 10/100`, pushes `FOLLOW`, then prompts every 200ms. After the client sends `rest`, the board replies with `[HP=10/MA=0]: (Resting)` prompts twice, then `[HP=90/MA=0]:` once. Drive the assist through `spawn` as in `assist_window.rs` with `[bot] assist_play = true, auto_rest = true, rest_at_percent = 50, rest_until_percent = 80`. Assert the sends, in order: `/beef @wait`, `rest`, `/beef @ok`. Check `assist_window.rs` for how the prompt with a status is written in that harness, the parser reads `(Resting)` after the prompt's closing bracket.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test party_follower wait 2>&1 | tail -5`
Expected: the assertion fails, no `/beef @wait` in the log.

- [ ] **Step 3: Implement**

Add `pub wait: crate::party::WaitState` to `AssistCasts`. In `assist_tick`:

- Where the rest command is sent in the `for cmd in assist_actions(bot, cor)` loop, before `session.send(&cmd)`, when `cfg.is_rest(&cmd)` and `party.is_follower()` and `casts.wait.on_rest_sent() == Some(Signal::Wait)`, send `telepath(leader, "@wait")` first and `party_note` it.
- On `Event::Prompt { status, .. }`, when `party.is_follower()`, `if casts.wait.on_prompt(status.as_ref()) == Some(Signal::Ok)` send `telepath(leader, "@ok")` and note it.
- Where `watch.on_event(&cor.event)` returns true, also `if casts.wait.on_refused() == Some(Signal::Ok)` send `@ok`.
- When the party state is not Follower, call `casts.wait.reset()` so a rest that began before the party ended does not send a stray `@ok` later.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test party_follower 2>&1 | tail -3`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/tests/party_follower.rs
git commit -m "feat(client): a follower tells the leader @wait before a rest and @ok after"
```

---

### Task 10: The leader is held by `@wait`

**Files:**
- Modify: `crates/mud-client/src/nav.rs:107-135` (`Interrupt::Held`)
- Modify: `crates/mud-client/src/farm.rs:1517-1660` (`FarmGuard`), `:2714-2960` (`travel`), a new `hold_here`
- Test: `crates/mud-client/tests/party_leader.rs`

**Interfaces:**
- Produces: `nav::Interrupt::Held`, `FarmGuard::holds(self, Arc<Mutex<Holds>>) -> Self`, `farm::hold_here(...) -> Result<HoldEnd, FarmError>`, `farm::HoldEnd::{Released, Died, TimeUp}`.

- [ ] **Step 1: Write the failing tests**

`party_leader.rs` uses the `farm_scripted.rs` harness: copy its `scripted_board`, `graph`, `session_for` and the circuit `FarmConfig` builder. Read `crates/mud-client/tests/farm_scripted.rs` first and mirror one of its two-room circuit tests. The graph is Home east to Field, Field west to Home. The board's script answers `e` with the Field block and `w` with the Home block. The pushes, printed 300ms after connect: `Pootwaddle started to follow you.` then `Pootwaddle telepaths: @wait`, and 2 seconds later `Pootwaddle telepaths: @ok`.

```rust
#[tokio::test]
async fn a_wait_holds_the_leg_until_ok() {
    // ... build as farm_scripted does, with a one-lap circuit Home -> Field.
    // Run the farm with loops = 1 and a 20s budget.
    let log = received.lock().unwrap().clone();
    let e = log.iter().position(|l| l == "e").expect("the leg");
    let ok_seen = *ok_printed_at.lock().unwrap();
    assert!(e_sent_at >= ok_seen, "the step east waited for @ok: {log:?}");
}
```

Record the instant each line is received in the board's log as `(Instant, String)` and the instant the board printed `@ok`, and assert the `e` came after it. Add a second test where no `@ok` arrives and `[party].wait_secs = 1`: the `e` is sent about a second after the `@wait`, and the run's notices contain `party: hold on pootwaddle expired`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test party_leader 2>&1 | tail -5`
Expected: the first test fails because `e` was sent at once.

- [ ] **Step 3: Implement**

`nav.rs`, in `Interrupt`:

```rust
    /// A follower asked the leader to wait, or the leader is waiting at
    /// the bank for its followers. Raised the moment the hold set is
    /// non-empty, so no further step is sent. Never spends the
    /// interrupt budget.
    Held,
```

`farm.rs`, `FarmGuard` gains `holds: Option<std::sync::Arc<std::sync::Mutex<crate::party::Holds>>>`, `None` in every constructor, and:

```rust
    pub fn holds(mut self, holds: std::sync::Arc<std::sync::Mutex<crate::party::Holds>>) -> Self {
        self.holds = Some(holds);
        self
    }

    fn held(&self) -> bool {
        self.holds.as_ref().is_some_and(|h| {
            let mut h = h.lock().expect("holds lock");
            h.expire(Instant::now());
            !h.is_empty()
        })
    }
```

At the top of `on_event`, before the `match`: `if self.held() { return Some(crate::nav::Interrupt::Held); }`. Death still outranks: put the `is_player_death` and `hp <= 0` arms in a first match, then the `held()` check, then the rest.

In `travel`'s `build` closure, add `.holds(session.party_holds())` to the guard.

A new function in `farm.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldEnd {
    Released,
    Died,
    TimeUp,
}

/// Stand here until every hold is released or expired. Two-second
/// stops so a monster that walks in is fought by the stop pump, the
/// same defence a walk interruption gets. Expired holds are named.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn hold_here(
    session: &crate::session::Session,
    nav: &crate::nav::Navigator,
    graph: &RoomGraph,
    at: RoomId,
    live: &mut Live,
    threat: &std::sync::Arc<crate::bot::ThreatTable>,
    refusals: &crate::bot::Refusals,
    casts: &mut Casts,
    clock: &mut crate::world::RoundClock,
    started: Instant,
    stats: &mut FarmStats,
    phase: PhaseSink<'_>,
    notices: &Notices,
) -> Result<HoldEnd, FarmError> {
    loop {
        for name in session.party_expire_holds() {
            notices(&format!("party: hold on {name} expired"));
        }
        if session.party_holds_clear() {
            return Ok(HoldEnd::Released);
        }
        set_phase(phase, Phase::Holding { at });
        let until = Instant::now() + Duration::from_secs(2);
        match farm_stop(session, nav, graph, at, live, threat, refusals, casts, clock, started, Some(until), true, None, false, None, stats, phase).await? {
            StopEnd::Dwelt { .. } => continue,
            StopEnd::Died => return Ok(HoldEnd::Died),
            StopEnd::TimeUp => return Ok(HoldEnd::TimeUp),
        }
    }
}
```

Add `Phase::Holding { at: RoomId }` to the `Phase` enum with label `"holding for the party"`, wherever `Phase::Banking` is defined and labelled. Match `farm_stop`'s real parameter order from its definition, the call above is the shape used at `farm.rs:2914`.

In `travel`, add an arm before the `Attacked | Hurt` arm:

```rust
            NavErrorKind::Interrupted(Interrupt::Held) => {
                match hold_here(session, nav, graph, err.at, live, threat, refusals, casts, clock, started, stats, phase, notices).await? {
                    HoldEnd::Released => {
                        sneaking = false;
                        continue;
                    }
                    HoldEnd::Died => return Ok(LegEnd::Died),
                    HoldEnd::TimeUp => return Ok(LegEnd::TimeUp),
                }
            }
```

`travel` needs a `notices: &Notices` parameter if it does not have one. Check its signature at `:2714` and thread it from every caller, `grep -n "travel(" crates/mud-client/src/*.rs`.

Also handle `Interrupt::Held` in every other `match` over `Interrupt` that the compiler flags, `go.rs` and `recover.rs` included: those walks are not led walks and treat `Held` as `continue` after a 2 second sleep, or simply build their guards without `.holds(...)` so it never fires. The second is the design: only `farm::travel` attaches the holds.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test party_leader 2>&1 | tail -3`
Expected: both pass.

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result" | grep -v " 0 failed"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/nav.rs crates/mud-client/src/farm.rs crates/mud-client/tests/party_leader.rs
git commit -m "feat(client): a follower's @wait holds the leader's leg until @ok or the timeout"
```

---

### Task 11: The leader's bank detour on `@bank`, and the bank wait

**Files:**
- Modify: `crates/mud-client/src/farm.rs:2134-2180` (the gate site)
- Modify: `crates/mud-client/src/bank.rs:335-470` (`errand` takes `asked`, the bank wait)
- Test: `crates/mud-client/tests/party_leader.rs`

**Interfaces:**
- Consumes: `hold_here`, `Session::{take_party_requests, party, party_hold, party_changes}`.
- Produces: `bank::errand(..., asked: bool, ...)`.

- [ ] **Step 1: Write the failing tests**

In `party_leader.rs`, the harness from Task 10 with the bank fixture from `bank_scripted.rs`: Home, Field east of Home, Bank east of Field. Circuit Home to Field, one lap, `[bank].auto_deposit = true`, `deposit_at_coins = 1000` so the leader's own gate never trips.

Test one: the board pushes `Pootwaddle started to follow you.` then, once the character is at Field, `Pootwaddle telepaths: @bank`. The board answers `par` with the roster header, `  Pootwaddle   Mystic` and a blank line, `i` with 15 gold, `deposit 1500` with the deposit line, and pushes `Pootwaddle telepaths: @ok` 500ms after it sees `par`. Assert the sends contain `e` twice, then `i`, `deposit 1500`, `i`, and that the farm ended within 3 seconds of the second `e`, well under `bank_wait_secs`.

Test two: the same, but no `@ok` ever arrives and `[party].bank_wait_secs = 1`. Assert the run's notices contain `party: hold on pootwaddle expired` and `party: bank wait: no reply from pootwaddle`.

Test three: the leader has nothing above its keep floor. The board answers `i` with `You are carrying nothing`. Assert no `deposit` was sent, the notices contain `nothing above the keep floor`, and the notices do not contain `Deposits are off`.

Collect notices in the tests with a sink `Arc<Mutex<Vec<String>>>` instead of `quiet()`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p mud-client --test party_leader 2>&1 | tail -5`
Expected: compile error on `errand`'s arity, or the first test fails because no detour happened.

- [ ] **Step 3: Implement**

`bank.rs`, `errand` gains `asked: bool` after `bank: &BankConfig`. After `let end = deposit_here(...)`:

```rust
    // Asked by a follower, the leader's own empty purse is not a
    // failure. Deposits stay on.
    let end = match end {
        ErrandEnd::Nothing(why) if asked && why.starts_with("nothing above the keep floor") => {
            notices(&format!("bank: {why}"));
            ErrandEnd::Deposited { farthings: 0, at: to, bank: name.clone() }
        }
        other => other,
    };
    if session.party().is_leader() {
        bank_wait(session, nav, graph, content, live, threat, refusals, casts, clock, started, stats, phase, notices, to).await?;
    }
```

with:

```rust
/// Refresh the roster with `par`, put a hold on every follower, and
/// stand at the bank until each has said `@ok` or the wait runs out.
#[allow(clippy::too_many_arguments)]
async fn bank_wait(
    session: &Session,
    nav: &crate::nav::Navigator,
    graph: &RoomGraph,
    content: &Content,
    live: &mut Live,
    threat: &std::sync::Arc<crate::bot::ThreatTable>,
    refusals: &crate::bot::Refusals,
    casts: &mut Casts,
    clock: &mut crate::world::RoundClock,
    started: Instant,
    stats: &mut FarmStats,
    phase: PhaseSink<'_>,
    notices: &crate::farm::Notices,
    at: RoomId,
) -> Result<(), FarmError> {
    let mut changes = session.party_changes();
    changes.mark_unchanged();
    session.send("par");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed()).await;
    let followers = session.party().followers();
    if followers.is_empty() {
        return Ok(());
    }
    let secs = session.profile().party.bank_wait_secs;
    let until = Instant::now() + std::time::Duration::from_secs(secs);
    for name in &followers {
        session.party_hold(name, until);
    }
    notices(&format!("party: waiting at the bank for {}", followers.join(", ")));
    let before = Instant::now();
    match crate::farm::hold_here(session, nav, graph, at, live, threat, refusals, casts, clock, started, stats, phase, notices).await? {
        crate::farm::HoldEnd::Released => {}
        crate::farm::HoldEnd::Died => return Ok(()),
        crate::farm::HoldEnd::TimeUp => return Ok(()),
    }
    if before.elapsed() >= std::time::Duration::from_secs(secs) {
        let still: Vec<String> = followers.iter().filter(|_| true).cloned().collect();
        notices(&format!("party: bank wait: no reply from {}", still.join(", ")));
    }
    Ok(())
}
```

Make the "no reply" list exact: `hold_here` names each expired hold in a notice, so track them by wrapping `notices` in a closure that records names from `party: hold on X expired` lines, or have `hold_here` return `HoldEnd::Released { expired: Vec<String> }`. Do the second: `Released { expired: Vec<String> }`, and print `no reply from` only when `expired` is non-empty. Update Task 10's match arms accordingly.

`content` is unused in `bank_wait`; drop it from the signature.

`farm.rs`, at the gate site, replace the condition with:

```rust
            let asked = if session.party().is_leader() {
                let by: Vec<String> = session
                    .take_party_requests()
                    .into_iter()
                    .filter(|r| r.command == crate::party::Remote::Bank)
                    .map(|r| r.from)
                    .collect();
                if !by.is_empty() {
                    notices(&format!("bank: {} asked for the bank", by.join(", ")));
                }
                !by.is_empty()
            } else {
                false
            };
            let own = bank_cfg.auto_deposit
                && stats.coin_pickups > judged_pickups
                && content.is_some()
                && {
                    judged_pickups = stats.coin_pickups;
                    let inv = crate::bank::read_inventory(session).await;
                    crate::bank::Reading::of(&inv)
                        .is_some_and(|r| gate.judge(&bank_cfg, r) == crate::bank::Judgement::Deposit)
                };
            if (own || asked) && !bank_off && let Some(content) = &content {
                let out = crate::bank::errand(session, &bank_nav, &graph, content, &bank_cfg, asked, live, ...).await?;
```

Keep the rest of the existing block. A `Deposited { farthings: 0, .. }` from an asked errand is printed as `bank: nothing to deposit, followers banked`, and does not set `bank_off`. Pass `false` for `asked` from `run_bank`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test party_leader --test bank_scripted --test farm_scripted 2>&1 | grep -E "^test result"`
Expected: all `ok`.

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result" | grep -v " 0 failed"`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/farm.rs crates/mud-client/src/bank.rs crates/mud-client/tests/party_leader.rs
git commit -m "feat(client): a follower's @bank detours the leader, who waits at the bank for @ok"
```

---

### Task 12: Documentation

**Files:**
- Modify: `docs/mud-client.md` (a new `## Parties` section before `## Banking` at `:1063`, and a paragraph in Banking)
- Modify: `docs/mud-client.md:33-95` (the profile example gains a `[party]` table)

- [ ] **Step 1: Write the Parties section**

Cover, in the voice the file uses: what a party is to the client and which board lines it reads, the telepath channel and the three commands, who is permitted, a follower is the assist plus party state and what it does under `/bot`, the leader's hold and the bank detour and wait, the `[party]` keys with defaults, `set follow normal`, the jobs that refuse while following, and that every wording is unverified on the live board. Name the follow-ups: the assist's backstab weapon swap, other channels, the queries.

Add to the Banking section one paragraph: a follower deposits when dragged into a bank, and a leader's errand waits for its followers.

Add to the profile example:

```toml
[party]
wait_secs = 90        # a follower's @wait holds the leader this long at most
bank_wait_secs = 15   # the leader waits at the bank this long for @ok replies
follow_normal = true  # send "set follow normal" when following begins
```

- [ ] **Step 2: Check the prose rules**

Run: `grep -n "—\|;" docs/mud-client.md | sed -n '1,5p'` on the new section's line range only. Expected: no em dashes and no semicolons in the lines you added. Parentheses only inside code spans.

- [ ] **Step 3: Commit**

```bash
git add docs/mud-client.md
git commit -m "doc: parties, the telepath channel, @bank, @wait and @ok"
```

---

## Plan self-review

Spec coverage: state machine and wordings, Task 1. Grammar, permission, wire form, holds, bank names, wait state, config, Task 2. Profile and live settings, Task 3. Session tracker with notes, requests and holds, Task 4. Notices, `set follow normal`, status bar, jobs refuse, Task 5. `deposit_here`, Task 6. Deposit on arrival with `@ok`, Task 7. Follower gate and `@bank`, Task 8. Wait handshake, Task 9. `Interrupt::Held`, `hold_here`, held on the way, Task 10. Bank request, `asked`, bank wait, roster at the bank, Task 11. Docs, Task 12. The leader with no job printing requests is Task 4's notes printed by Task 5's window arm. A death clearing holds is the existing death path ending the leg, and the party ending clearing holds is Task 4.

Type consistency: `Holds::names()` returns lowercased keys, and the tests in Task 4 and Task 10 expect lowercase. `HoldEnd::Released { expired }` is the final shape, Task 10's first draft used a unit variant, Task 11 corrects it, and the implementer of Task 10 should use the struct variant from the start. `errand`'s `asked` sits after `bank: &BankConfig` in both Task 11 call sites.
