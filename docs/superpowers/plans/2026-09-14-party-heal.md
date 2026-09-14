# Party Heal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A party member says when it is hurt or poisoned, every member polls the roster's numbers, members introduce their race and class, and a member holding heals casts them on whoever needs them.

**Architecture:** Facts from the board live in the session's party tracker: the roster with numbers, the said words, the introductions. Judgments live in the assist: an ask state that says `@heal`, a met set that says `@iam`, and a party cast state in the sheet module that picks cure, rain or a single heal off the table and confirms it from the board with the same outcome grammar the self heal uses.

**Tech Stack:** Rust 2024, tokio, regex, the existing `mud-client` test boards (a TCP listener per test binary).

**Spec:** `docs/superpowers/specs/2026-09-14-party-heal-design.md`

## Global Constraints

- Never run `rustfmt` or `cargo fmt`.
- Run tests with one build job and, for the whole suite, one thread: `cargo test -p mud-client -j 1 --test <name>`; the box has 7 GB.
- No test reads `re/`.
- Every file ends with a blank line.
- Commit after every task with a tag: `feat:`, `fix:`, `refactor:`, `test:`, `doc:`. No planning documents in commit messages. No attribution lines.
- Prose in comments and docs: plain words, one idea per sentence, no em dashes, no parentheses, no semicolons.
- Wordings that have not been seen on the live board are marked UNVERIFIED in the code comment that carries them.

---

## File map

- `crates/mud-client/src/party.rs`: roster rows with numbers, `Change::Vitals`, the three new words in two shapes, `say`, the `Health` table, `AskState`.
- `crates/mud-client/src/session.rs`: the tracker holds `Health`, `feed_party` writes it, `Session::party_health`, `Session::party_config`.
- `crates/mud-client/src/sheet.rs`: `cast_outcome` shared by `HealState` and the new `PartyHeal`, `Spellbook::party_heals`, `PartySource`, `PartyHeal`, `HealState::affords` and `hold_round`.
- `crates/mud-client/src/farm.rs`: `Sheet` gains `party`, `sheet_from` fills it.
- `crates/mud-client/src/tui.rs`: `AssistCasts` gains `party`, `ask`, `met`; `assist_tick` introduces, asks, answers.
- `crates/mud-client/src/window.rs`: the `party` poll.
- `crates/mud-client/src/settings.rs`: two keys.
- `docs/mud-client.md`: the `[party]` block and a paragraph.
- Tests: `tests/party.rs`, `tests/party_session.rs`, `tests/sheet.rs`, `tests/settings.rs`, new binaries `tests/party_window_poll.rs`, `tests/party_window_ask.rs`, `tests/party_window_iam.rs`, `tests/party_window_heal.rs`.

---

### Task 1: Roster rows with numbers

**Files:**
- Modify: `crates/mud-client/src/party.rs` (`Member`, `ROSTER_ROW`, `Change`, `observe`, `end_roster`)
- Modify: `crates/mud-client/src/session.rs:1437-1470` (`feed_party` note match)
- Test: `crates/mud-client/tests/party.rs`

**Interfaces:**
- Produces: `Member { name: String, invited: bool, class: Option<String>, hp: Option<u8>, pool: Option<u8> }`, `Change::Vitals`.

- [ ] **Step 1: Write the failing tests**

In `crates/mud-client/tests/party.rs`, replace the test `the_roster_replaces_the_member_list_in_either_row_format` with this one and add the two after it:

```rust
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
            Member { name: "Pootwaddle".into(), invited: false, class: None, hp: None, pool: None },
            Member { name: "Blueberry".into(), invited: false, class: Some("Ninja".into()), hp: Some(90), pool: None },
            Member { name: "Newguy".into(), invited: true, class: None, hp: None, pool: None },
        ]
    );
    assert_eq!(s.role, Role::Leader);
}

/// The live board's row, captured 2026-09-13: class in parentheses,
/// the pool tagged K or M when there is one, then the health.
#[test]
fn a_live_roster_row_carries_class_pool_and_health() {
    let mut s = state();
    s.observe("Beef started to follow you.");
    s.observe("Carrot started to follow you.");
    s.observe("Salad started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Blueberry                      (Mystic)     [K:100%] [H:100%]   - Frontrank");
    s.observe("  Beef                           (Ninja)               [H:100%]   - Midrank");
    s.observe("  Salad                          (Ranger)     [M:100%] [H: 86%]   - Midrank");
    s.observe("");
    let by = |name: &str| s.members.iter().find(|m| m.name == name).cloned().unwrap();
    assert_eq!(by("Blueberry").class.as_deref(), Some("Mystic"));
    assert_eq!(by("Blueberry").pool, Some(100));
    assert_eq!(by("Beef").pool, None);
    assert_eq!(by("Beef").hp, Some(100));
    assert_eq!(by("Salad").hp, Some(86));
    assert_eq!(by("Salad").pool, Some(100));
}

/// A poll that changed only the numbers is not a change to the party.
/// The window prints nothing for it, so a roster every twenty seconds
/// is silent.
#[test]
fn a_roster_that_changed_only_numbers_is_vitals_not_roster() {
    let mut s = state();
    s.observe("Beef started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Beef                           (Ninja)               [H:100%]   - Midrank");
    assert_eq!(s.observe(""), Some(Change::Vitals));
    s.observe("The following people are in your travel party:");
    s.observe("  Beef                           (Ninja)               [H: 40%]   - Midrank");
    assert_eq!(s.observe(""), Some(Change::Vitals));
    assert_eq!(s.members[0].hp, Some(40));
    s.observe("The following people are in your travel party:");
    s.observe("  Beef                           (Ninja)               [H: 40%]   - Midrank");
    s.observe("  Carrot                         (Paladin)    [M: 90%] [H: 70%]   - Midrank");
    assert_eq!(s.observe(""), Some(Change::Roster), "a new name is a change to the party");
}
```

Then fix the other `Member { .. }` literals in that file (in `removed_from_your_followers_drops_the_member` and `a_non_indented_line_ends_the_roster_and_is_then_read_itself`) by adding `class: None, hp: None, pool: None` to each.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p mud-client -j 1 --test party 2>&1 | grep -E '^error|test result' | head`
Expected: compile errors, `no field class` and `no variant Vitals`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/party.rs`, replace the `ROSTER_ROW` static and `Member`:

```rust
static ROSTER_ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+(\w+)\b").unwrap());
/// The live row, captured 2026-09-13:
/// `  Salad   (Ranger)     [M:100%] [H: 86%]   - Midrank`. Each part
/// is read on its own, so the stock two-column row still parses with
/// every part absent.
static ROSTER_CLASS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([^)]+)\)").unwrap());
static ROSTER_POOL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[KM]:\s*(\d+)%\]").unwrap());
static ROSTER_HP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[H:\s*(\d+)%\]").unwrap());

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
```

Add `Vitals` to `Change`:

```rust
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
```

In `PartyState::add`, replace `self.members.push(Member { name: name.to_string(), invited })` with `self.members.push(Member::plain(name, invited))`.

In `observe`, replace the row push:

```rust
            if let Some(c) = ROSTER_ROW.captures(line) {
                rows.push(Member::from_row(line, &c[1]));
                return None;
            }
```

Replace `end_roster`:

```rust
    fn end_roster(&mut self) -> Option<Change> {
        let rows = self.roster.take()?;
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
```

In `crates/mud-client/src/session.rs` `feed_party`, the note becomes optional. Replace the `let note = match &change { ... };` block and the `if let Some(tx) = &t.notes { let _ = tx.send(note); }` after it with:

```rust
        let note = match &change {
            Change::Following(who) => Some(format!("party: following {who}")),
            Change::Joined(who) => Some(format!("party: {who} joined")),
            Change::Invited(who) => Some(format!("party: invited {who}")),
            Change::Left(who) => Some(format!("party: {who} left")),
            Change::Ended => Some("party: ended".to_string()),
            Change::Roster => Some(format!("party: roster {}", t.state.followers().join(", "))),
            // A poll that changed only numbers. Said out loud every
            // twenty seconds it would be the only thing in the window.
            Change::Vitals => None,
        };
```

and

```rust
        if let (Some(tx), Some(note)) = (&t.notes, note) {
            let _ = tx.send(note);
        }
```

Keep the `if change == Change::Ended { ... } else { ... retain_members ... }` block between them as it is.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client -j 1 --test party --test party_session --test party_leader 2>&1 | grep -E '^error|test result'`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/party.rs crates/mud-client/src/session.rs crates/mud-client/tests/party.rs
git commit -m "feat(client): the roster keeps class, pool and health, and a poll is silent"
```

---

### Task 2: Three new words, in two shapes, and `say`

**Files:**
- Modify: `crates/mud-client/src/party.rs` (`Remote`, `TELEPATH`, `remote`, add `SAID`, `say`)
- Modify: `crates/mud-client/src/session.rs:1479-1494` (`feed_party` match on `req.command`)
- Test: `crates/mud-client/tests/party.rs`

**Interfaces:**
- Produces: `Remote::{Bank, Wait, Ok, Heal(u8), Cure, Iam { race: String, class: String }}` (no longer `Copy`), `party::say(text: &str) -> String`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/mud-client/tests/party.rs`, after the existing `remote` tests:

```rust
/// The room hears a said word. Captured 2026-09-13 as
/// `Blueberry says "toot"`; the `@` passing through unchanged is
/// UNVERIFIED.
#[test]
fn the_new_words_read_in_both_shapes() {
    assert_eq!(remote("Celery telepaths: @heal 35").map(|r| r.command), Some(Remote::Heal(35)));
    assert_eq!(remote("Celery says \"@heal 35\"").map(|r| r.command), Some(Remote::Heal(35)));
    assert_eq!(remote("Celery says \"@heal\"").map(|r| r.command), Some(Remote::Heal(0)));
    assert_eq!(remote("Celery says \"@heal lots\"").map(|r| r.command), Some(Remote::Heal(0)));
    assert_eq!(remote("Celery says \"@cure\"").map(|r| r.command), Some(Remote::Cure));
    assert_eq!(
        remote("Celery says \"@iam Human Witchunter\"").map(|r| r.command),
        Some(Remote::Iam { race: "Human".into(), class: "Witchunter".into() })
    );
    assert_eq!(
        remote("Celery says \"@iam Half-Elf Cleric\"").map(|r| r.command),
        Some(Remote::Iam { race: "Half-Elf".into(), class: "Cleric".into() })
    );
    assert_eq!(remote("Celery says \"@iam Human\""), None, "a class is required");
    assert_eq!(remote("Beef says \"@wait\"").map(|r| r.command), Some(Remote::Wait));
    assert_eq!(remote("You say \"@heal 35\""), None, "the own echo is not a request");
    assert_eq!(remote("Celery says \"heal me\""), None);
}

#[test]
fn say_is_the_wire_form_for_a_said_word() {
    assert_eq!(say("@heal 35"), "say @heal 35");
}
```

Add `say` to the `use mud_client::party::{...}` list at the top of the file.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p mud-client -j 1 --test party 2>&1 | grep -E '^error|test result' | head`
Expected: compile errors, `no variant Heal`, `cannot find function say`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/party.rs`, replace `Remote`, `TELEPATH` and `remote`:

```rust
/// The commands this client understands. Any other word after the
/// `@` is dropped without a word, as MudPlay drops unknown commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    Bank,
    Wait,
    Ok,
    /// `@heal <percent>`: the sender is at that health and cannot heal
    /// itself. A missing or unreadable percent reads as 0.
    Heal(u8),
    /// `@cure`: the sender is poisoned and cannot cure itself.
    Cure,
    /// `@iam <race> <class>`: the sender introducing itself.
    Iam { race: String, class: String },
}

/// `Foo telepaths: @bank`. MudPlay's inbound shape, and the DLL's
/// `%s telepaths: %s%s`.
static TELEPATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(\w+) telepaths: @(\S+)(?: (.*))?$"#).unwrap());
/// `Foo says "@heal 35"`. A word said in the room reaches every
/// member at once, which is what a request for a healer needs. The
/// plain say is captured; the `@` passing through is UNVERIFIED.
static SAID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(\w+) says "@(\S+)(?: ([^"]*))?"$"#).unwrap());

pub fn remote(line: &str) -> Option<Request> {
    let line = line.trim_end();
    let c = TELEPATH.captures(line).or_else(|| SAID.captures(line))?;
    let arg = c.get(3).map(|m| m.as_str().trim()).unwrap_or("");
    let command = match c[2].to_ascii_lowercase().as_str() {
        "bank" => Remote::Bank,
        "wait" => Remote::Wait,
        "ok" => Remote::Ok,
        "heal" => Remote::Heal(arg.parse().unwrap_or(0)),
        "cure" => Remote::Cure,
        "iam" => {
            let (race, class) = arg.split_once(' ')?;
            Remote::Iam { race: race.to_string(), class: class.trim().to_string() }
        }
        _ => return None,
    };
    Some(Request { from: c[1].to_string(), command })
}
```

Below `telepath`, add:

```rust
/// The wire form of a word said to the room.
pub fn say(text: &str) -> String {
    format!("say {text}")
}
```

In `crates/mud-client/src/session.rs` `feed_party`, `Remote` is no longer `Copy`. Replace the `let word = match req.command { ... };` and the `match req.command { ... }` at the end with:

```rust
    let word = match &req.command {
        crate::party::Remote::Bank => "@bank".to_string(),
        crate::party::Remote::Wait => "@wait".to_string(),
        crate::party::Remote::Ok => "@ok".to_string(),
        crate::party::Remote::Heal(p) => format!("@heal {p}"),
        crate::party::Remote::Cure => "@cure".to_string(),
        crate::party::Remote::Iam { race, class } => format!("@iam {race} {class}"),
    };
    if let Some(tx) = &t.notes {
        let _ = tx.send(format!("party: {} asks {word}", req.from));
    }
    match &req.command {
        crate::party::Remote::Wait => {
            let until = Instant::now() + Duration::from_secs(t.wait_secs);
            t.holds.lock().expect("holds lock").hold(&req.from, until);
        }
        crate::party::Remote::Ok => t.holds.lock().expect("holds lock").release(&req.from),
        crate::party::Remote::Bank => t.requests.push(req),
        // Written into the health table by Task 3.
        crate::party::Remote::Heal(_) | crate::party::Remote::Cure | crate::party::Remote::Iam { .. } => {}
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client -j 1 --test party --test party_session --test party_leader --test party_follower 2>&1 | grep -E '^error|test result'`
Expected: all `ok`. If `farm.rs:2184` complains about `PartialEq`, it does not: `Remote` still derives it.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/party.rs crates/mud-client/src/session.rs crates/mud-client/tests/party.rs
git commit -m "feat(client): @heal, @cure and @iam, heard as telepaths and as words said in the room"
```

---

### Task 3: The health table

**Files:**
- Modify: `crates/mud-client/src/party.rs` (add `Vitals`, `Health`)
- Modify: `crates/mud-client/src/session.rs` (`PartyTracker`, `feed_party`, `Session::party_health`)
- Test: `crates/mud-client/tests/party.rs`, `crates/mud-client/tests/party_session.rs`

**Interfaces:**
- Produces:
  - `party::Vitals { name, class: Option<String>, race: Option<String>, hp: Option<u8>, pool: Option<u8>, seen: Instant, poisoned: Option<Instant> }` with `fn resists_magic(&self) -> bool`.
  - `party::Health` with `new()`, `on_roster(&mut self, members: &[Member], now: Instant)`, `on_heal(&mut self, name: &str, hp: u8, now: Instant)`, `on_cure(&mut self, name: &str, now: Instant)`, `on_iam(&mut self, name: &str, race: &str, class: &str)`, `clear(&mut self)`, `rows(&self) -> impl Iterator<Item = &Vitals>`, `get(&self, name: &str) -> Option<&Vitals>`, `is_empty(&self) -> bool`.
  - `Session::party_health(&self) -> Health`.

- [ ] **Step 1: Write the failing unit tests**

Add to `crates/mud-client/tests/party.rs`:

```rust
fn member(name: &str, class: &str, hp: u8) -> Member {
    Member { name: name.into(), invited: false, class: Some(class.into()), hp: Some(hp), pool: None }
}

#[test]
fn a_roster_fills_the_table_and_drops_the_departed() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_roster(&[member("Beef", "Ninja", 100), member("Salad", "Ranger", 86)], t0);
    assert_eq!(h.get("salad").map(|v| v.hp), Some(Some(86)));
    assert_eq!(h.get("Beef").map(|v| v.seen), Some(t0));
    let t1 = t0 + Duration::from_secs(20);
    h.on_roster(&[member("Salad", "Ranger", 90)], t1);
    assert!(h.get("Beef").is_none(), "the departed are dropped");
    assert_eq!(h.get("Salad").map(|v| (v.hp, v.seen)), Some((Some(90), t1)));
}

#[test]
fn a_request_is_a_fresher_row() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_roster(&[member("Celery", "Mage", 80)], t0);
    let t1 = t0 + Duration::from_secs(3);
    h.on_heal("celery", 35, t1);
    let v = h.get("Celery").unwrap();
    assert_eq!((v.hp, v.seen), (Some(35), t1));
    assert_eq!(v.class.as_deref(), Some("Mage"), "a request keeps what the roster said");
    h.on_heal("Newcomer", 20, t1);
    assert_eq!(h.get("newcomer").map(|v| v.hp), Some(Some(20)), "a request from a name the roster has not shown yet still counts");
}

#[test]
fn a_cure_request_holds_until_the_next_roster() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_roster(&[member("Celery", "Mage", 80)], t0);
    let t1 = t0 + Duration::from_secs(3);
    h.on_cure("Celery", t1);
    assert_eq!(h.get("Celery").unwrap().poisoned, Some(t1));
    h.on_roster(&[member("Celery", "Mage", 80)], t1 + Duration::from_secs(20));
    assert_eq!(h.get("Celery").unwrap().poisoned, None, "the member says it again on the next tick if still poisoned");
}

#[test]
fn an_introduction_sets_race_and_class_and_a_witchunter_resists() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_roster(&[member("Beef", "Ninja", 100)], t0);
    h.on_iam("Beef", "Human", "Witchunter");
    let v = h.get("Beef").unwrap();
    assert_eq!(v.race.as_deref(), Some("Human"));
    assert!(v.resists_magic());
    h.on_iam("Carrot", "Dwarf", "Paladin");
    assert!(!h.get("Carrot").unwrap().resists_magic());
    let mut from_roster = Health::new();
    from_roster.on_roster(&[member("Beef", "Witchunter", 100)], t0);
    assert!(from_roster.get("Beef").unwrap().resists_magic(), "the roster's class word marks it too");
    h.clear();
    assert!(h.is_empty());
}
```

Add `Health` to the `use mud_client::party::{...}` list.

- [ ] **Step 2: Write the failing session test**

Add to `crates/mud-client/tests/party_session.rs`:

```rust
/// The roster's numbers and a said request both reach the table a
/// healer reads, and a stranger's word does not.
#[tokio::test]
async fn the_session_keeps_the_party_health() {
    let (addr, _received) = board(vec![
        "Beef started to follow you.",
        "The following people are in your travel party:\r\n  Beef                           (Ninja)               [H:100%]   - Midrank\r\n",
        "Beef says \"@heal 35\"",
        "Stranger says \"@heal 5\"",
        "Beef says \"@iam Human Witchunter\"",
    ])
    .await;
    let session = session_for(addr).await;
    wait_until(&session, |s| s.party_health().get("Beef").is_some_and(|v| v.race.is_some())).await;
    let health = session.party_health();
    let beef = health.get("Beef").unwrap();
    assert_eq!(beef.hp, Some(35), "the request overwrote the roster's number");
    assert_eq!(beef.class.as_deref(), Some("Witchunter"));
    assert!(beef.resists_magic());
    assert!(health.get("Stranger").is_none());
    session.close().await;
}
```

Check how the other tests in that file end a session and copy that call if it is not `close().await`.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p mud-client -j 1 --test party --test party_session 2>&1 | grep -E '^error|test result' | head`
Expected: compile errors, `cannot find type Health`, `no method party_health`.

- [ ] **Step 4: Implement the table**

In `crates/mud-client/src/party.rs`, after `Holds`:

```rust
/// One member as the board has described it. Facts only: the roster's
/// numbers, the member's own words, and when they were said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vitals {
    pub name: String,
    /// From the roster's parentheses or from `@iam`.
    pub class: Option<String>,
    /// From `@iam` only.
    pub race: Option<String>,
    pub hp: Option<u8>,
    pub pool: Option<u8>,
    /// When `hp` was last written, by a roster or by `@heal`.
    pub seen: Instant,
    /// When the member last said `@cure`. Cleared by the next roster,
    /// since the member says it again on the next poison tick if it
    /// still needs curing.
    pub poisoned: Option<Instant>,
}

impl Vitals {
    fn new(name: &str, now: Instant) -> Vitals {
        Vitals { name: name.to_string(), class: None, race: None, hp: None, pool: None, seen: now, poisoned: None }
    }

    /// Witchunters resist every spell. A heal cast on one is mana
    /// spent for nothing. The class word is the roster's, UNVERIFIED
    /// for a witchunter, or the member's own.
    pub fn resists_magic(&self) -> bool {
        self.class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case("witchunter"))
    }
}

/// The party's health as the board has told it, keyed lowercased.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    by: BTreeMap<String, Vitals>,
}

impl Health {
    pub fn new() -> Health {
        Health::default()
    }

    fn row(&mut self, name: &str, now: Instant) -> &mut Vitals {
        self.by.entry(Holds::key(name)).or_insert_with(|| Vitals::new(name, now))
    }

    /// A roster block: every row's numbers are fresh, a cure said
    /// before it is spent, and names it does not list are gone.
    pub fn on_roster(&mut self, members: &[Member], now: Instant) {
        let keep: BTreeSet<String> = members.iter().map(|m| Holds::key(&m.name)).collect();
        self.by.retain(|k, _| keep.contains(k));
        for m in members {
            let v = self.row(&m.name, now);
            if m.class.is_some() {
                v.class = m.class.clone();
            }
            v.hp = m.hp;
            v.pool = m.pool;
            v.seen = now;
            v.poisoned = None;
        }
    }

    pub fn on_heal(&mut self, name: &str, hp: u8, now: Instant) {
        let v = self.row(name, now);
        v.hp = Some(hp);
        v.seen = now;
    }

    pub fn on_cure(&mut self, name: &str, now: Instant) {
        self.row(name, now).poisoned = Some(now);
    }

    pub fn on_iam(&mut self, name: &str, race: &str, class: &str) {
        let v = self.row(name, Instant::now());
        v.race = Some(race.to_string());
        v.class = Some(class.to_string());
    }

    pub fn get(&self, name: &str) -> Option<&Vitals> {
        self.by.get(&Holds::key(name))
    }

    pub fn rows(&self) -> impl Iterator<Item = &Vitals> {
        self.by.values()
    }

    pub fn is_empty(&self) -> bool {
        self.by.is_empty()
    }

    pub fn clear(&mut self) {
        self.by.clear();
    }
}
```

`Holds::key` is private today. Make it `pub(crate) fn key`.

- [ ] **Step 5: Wire the session**

In `crates/mud-client/src/session.rs`:

Add to `PartyTracker`, after `requests`:

```rust
    /// The party's health as the board has told it. Read by a healer.
    health: crate::party::Health,
```

and to its construction at line 525: `health: crate::party::Health::new(),`.

In `feed_party`, inside `if let Some(change) = change { ... }`, replace the `if change == Change::Ended { ... } else { ... }` block with:

```rust
        if change == Change::Ended {
            t.holds.lock().expect("holds lock").clear();
            t.requests.clear();
            t.health.clear();
        } else {
            let state = t.state.clone();
            t.holds.lock().expect("holds lock").retain_members(&state);
            if matches!(change, Change::Roster | Change::Vitals) {
                t.health.on_roster(&state.members, Instant::now());
            }
        }
```

Replace the last `match &req.command { ... }` arms for the three new words:

```rust
        crate::party::Remote::Heal(p) => t.health.on_heal(&req.from, *p, Instant::now()),
        crate::party::Remote::Cure => t.health.on_cure(&req.from, Instant::now()),
        crate::party::Remote::Iam { race, class } => t.health.on_iam(&req.from, race, class),
```

Add to `impl Session`, after `party()`:

```rust
    /// The party's health as the board has told it. A copy, read only
    /// when the assist is about to decide, the same rule as `party`.
    pub fn party_health(&self) -> crate::party::Health {
        self.party.lock().expect("party lock").health.clone()
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p mud-client -j 1 --test party --test party_session 2>&1 | grep -E '^error|test result'`
Expected: all `ok`.

- [ ] **Step 7: Commit**

```bash
git add crates/mud-client/src/party.rs crates/mud-client/src/session.rs crates/mud-client/tests/party.rs crates/mud-client/tests/party_session.rs
git commit -m "feat(client): the session keeps the party's health from the roster and the said words"
```

---

### Task 4: Settings and docs

**Files:**
- Modify: `crates/mud-client/src/party.rs` (`PartyConfig`)
- Modify: `crates/mud-client/src/settings.rs:88-90`
- Modify: `crates/mud-client/src/session.rs` (`Session::party_config`)
- Modify: `docs/mud-client.md:94-98`
- Test: `crates/mud-client/tests/party.rs`, `crates/mud-client/tests/settings.rs`

**Interfaces:**
- Produces: `PartyConfig { wait_secs, bank_wait_secs, follow_normal, poll_secs: u64, heal: bool }`, `Session::party_config(&self) -> PartyConfig`.

- [ ] **Step 1: Write the failing tests**

In `crates/mud-client/tests/party.rs`, replace `party_config_defaults_and_refuses_zero_waits`:

```rust
#[test]
fn party_config_defaults_and_refuses_zero_waits() {
    let cfg = PartyConfig::default();
    assert_eq!(cfg.wait_secs, 90);
    assert_eq!(cfg.bank_wait_secs, 15);
    assert!(cfg.follow_normal);
    assert_eq!(cfg.poll_secs, 20);
    assert!(cfg.heal);
    assert!(cfg.validate().is_ok());
    assert!(PartyConfig { wait_secs: 0, ..Default::default() }.validate().is_err());
    assert!(PartyConfig { bank_wait_secs: 0, ..Default::default() }.validate().is_err());
    assert!(PartyConfig { poll_secs: 0, ..Default::default() }.validate().is_ok(), "zero turns the poll off");
}
```

In `crates/mud-client/tests/settings.rs` `party_keys_are_live`, add before the closing brace:

```rust
    s.set("party.poll_secs", "0").unwrap();
    assert_eq!(s.profile().party.poll_secs, 0);
    s.set("party.heal", "false").unwrap();
    assert!(!s.profile().party.heal);
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p mud-client -j 1 --test party --test settings 2>&1 | grep -E '^error|test result' | head`
Expected: compile error `no field poll_secs`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/party.rs`, `PartyConfig`:

```rust
pub struct PartyConfig {
    /// A follower's `@wait` holds the leader this long at most.
    pub wait_secs: u64,
    /// The leader waits at the bank this long for `@ok` replies.
    pub bank_wait_secs: u64,
    /// Send `set follow normal` when the character begins following.
    pub follow_normal: bool,
    /// Send `party` this often while in a party, for the roster's
    /// numbers. 0 turns the poll off.
    pub poll_secs: u64,
    /// Cast heals on the party. Off, the character still asks for its
    /// own and still introduces itself.
    pub heal: bool,
}

impl Default for PartyConfig {
    fn default() -> Self {
        PartyConfig { wait_secs: 90, bank_wait_secs: 15, follow_normal: true, poll_secs: 20, heal: true }
    }
}
```

In `crates/mud-client/src/settings.rs`, after `"party.follow_normal",` add `"party.poll_secs",` and `"party.heal",`.

In `crates/mud-client/src/session.rs`, after `profile()`:

```rust
    /// The `[party]` table alone. Small, so the assist can read it on
    /// every prompt without cloning the profile.
    pub fn party_config(&self) -> crate::party::PartyConfig {
        self.profile.borrow().party.clone()
    }
```

In `docs/mud-client.md`, the `[party]` block becomes:

```toml
[party]
wait_secs = 90        # a follower's @wait holds the leader this long at most
bank_wait_secs = 15   # the leader waits at the bank this long for @ok replies
follow_normal = true  # send "set follow normal" when following begins
poll_secs = 20        # send "party" this often while in a party; 0 turns it off
heal = true           # cast heals on party members that need them
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client -j 1 --test party --test settings --test profile 2>&1 | grep -E '^error|test result'`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/party.rs crates/mud-client/src/settings.rs crates/mud-client/src/session.rs docs/mud-client.md crates/mud-client/tests/party.rs crates/mud-client/tests/settings.rs
git commit -m "feat(client): party.poll_secs and party.heal"
```

---

### Task 5: The poll

**Files:**
- Modify: `crates/mud-client/src/window.rs:770-781` and `1117-1126`
- Create: `crates/mud-client/tests/party_window_poll.rs`

**Interfaces:**
- Consumes: `Session::party_config().poll_secs`, `Session::party().role`.

- [ ] **Step 1: Write the failing test**

Create `crates/mud-client/tests/party_window_poll.rs`. Copy `crates/mud-client/tests/party_window_wait.rs` whole, then make these changes:

Module doc:

```rust
//! Every member polls `party` on a timer while in a party, and a poll
//! that changes only numbers prints nothing. One test in this binary,
//! because it sets `XDG_CONFIG_HOME` for the whole process.
```

Rename `rest_board` to `poll_board`. In its tick branch, replace the whole `let say = if step == 0 { ... } else { ... };` with:

```rust
                    let say = if step == 0 {
                        format!("\r\nBeef started to follow you.\r\n[HP={hp}/MA=0]:")
                    } else {
                        format!("\r\n[HP={hp}/MA=0]:")
                    };
```

In its read branch, add a `"party"` arm before the `_ =>` arm:

```rust
                            "party" => format!(
                                "\r\nparty\r\nThe following people are in your travel party:\r\n  Beef                           (Ninja)               [H: 40%]   - Midrank\r\n\r\n[HP={hp}/MA=0]:"
                            ),
```

Rename the test to `a_member_polls_the_party_on_its_interval_without_a_note`, the scratch dir to `party_window_poll`, and the settings: keep host, port, username, `farm.content`, `bot.assist_play` and `bot.max_hp`, drop the rest marks, and add `s.set("party.poll_secs", "1").unwrap();`.

Replace the wait loop and the assertions with:

```rust
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if sent.lock().unwrap().iter().filter(|l| l.as_str() == "party").count() >= 2 {
            break;
        }
        if tokio::time::timeout_at(deadline, front.recv()).await.is_err() {
            break;
        }
    }
    let log = sent.lock().unwrap().clone();
    let screen = handle.screen.lock().unwrap().text();
    let polls = log.iter().filter(|l| l.as_str() == "party").count();
    assert!(polls >= 2, "two polls within the deadline: {log:?}\nscreen:\n{screen}");
    assert!(!screen.contains("party: roster"), "a poll that changed only numbers is silent:\n{screen}");
    assert!(screen.contains("party: Beef joined"), "the join itself is still said:\n{screen}");
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p mud-client -j 1 --test party_window_poll 2>&1 | grep -E '^error|test result|panicked' | head`
Expected: FAIL, `two polls within the deadline`, no `party` in the log.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/window.rs`, after `let mut level_tick = tokio::time::interval(LEVEL_POLL);` add:

```rust
    // The roster's numbers, for the healers. `[party].poll_secs` of 0
    // is off: the interval still exists so the select arm compiles,
    // and the arm checks the setting before sending.
    let poll_secs = session.party_config().poll_secs.max(1);
    let mut party_tick = tokio::time::interval(std::time::Duration::from_secs(poll_secs));
    party_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
```

After the `_ = level_tick.tick() => { ... }` arm add:

```rust
            _ = party_tick.tick() => {
                // Only in the realm and in a party. Opaque to the
                // correlator like `exp`: the roster is recognised by
                // its header whoever asked.
                if in_realm
                    && session.party_config().poll_secs > 0
                    && session.party().role != crate::party::Role::None
                {
                    session.send("party");
                }
            }
```

A live change of `poll_secs` takes effect on the next window start. Say so in the settings docs line if it is not already implied.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client -j 1 --test party_window_poll --test party_window_wait --test party_window_drag 2>&1 | grep -E '^error|test result'`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/window.rs crates/mud-client/tests/party_window_poll.rs
git commit -m "feat(client): every member polls party on a timer"
```

---

### Task 6: One outcome grammar for casts

**Files:**
- Modify: `crates/mud-client/src/sheet.rs:655-695` (`HealState::on_event`)
- Test: `crates/mud-client/tests/sheet.rs`

**Interfaces:**
- Produces: `sheet::Outcome::{Unknown, Cast, Failed}`, `sheet::cast_outcome(line: &str) -> Option<Outcome>`.

- [ ] **Step 1: Write the failing test**

Add to `crates/mud-client/tests/sheet.rs`:

```rust
#[test]
fn the_outcome_grammar_reads_the_three_families() {
    use mud_client::sheet::{Outcome, cast_outcome};
    assert_eq!(cast_outcome("You do not know how to cast mahe."), Some(Outcome::Unknown));
    assert_eq!(cast_outcome("You cast major healing on Celery!"), Some(Outcome::Cast));
    assert_eq!(cast_outcome("You cast healing rain on the room!"), Some(Outcome::Cast));
    assert_eq!(cast_outcome("You invoke way of the swan!"), Some(Outcome::Cast));
    assert_eq!(cast_outcome("You attempt to cast major healing at Celery, but fail."), Some(Outcome::Failed));
    assert_eq!(cast_outcome("You do not have enough mana to cast that spell!"), Some(Outcome::Failed));
    assert_eq!(cast_outcome("You have already cast a spell this round!"), Some(Outcome::Failed));
    assert_eq!(cast_outcome("Celery is looking around the room."), None);
    assert_eq!(cast_outcome("A rat attempted to cast fire at you, but failed."), None, "the caller gates by attribution, this only names the family");
}
```

Note the last case: `cast_outcome` reads the whole line lowercased and `"but fail"` alone would match it. The function must require the line to begin with `you ` for `Failed` as it does for `Cast`, except the three pool and round wordings which begin with `you ` on the board anyway.

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p mud-client -j 1 --test sheet 2>&1 | grep -E '^error|test result' | head`
Expected: compile error, `cannot find cast_outcome`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/sheet.rs`, before `pub struct HealState`:

```rust
/// What the board said about a cast of ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The spell is not in the book. Terminal for that source.
    Unknown,
    /// It landed.
    Cast,
    /// A roll, a pool or a round. Comes round again.
    Failed,
}

/// The one outcome grammar, shared by every cast machine. The caller
/// gates by attribution: a monster's "%s attempted to cast %s at you,
/// but failed." is routine din, and this reads only lines the
/// correlator says answer our cast.
pub fn cast_outcome(line: &str) -> Option<Outcome> {
    let line = line.to_lowercase();
    if line.contains("do not know how to cast") || line.contains("do not know how to invoke") {
        return Some(Outcome::Unknown);
    }
    if line.starts_with("you cast ") || line.starts_with("you invoke ") {
        return Some(Outcome::Cast);
    }
    if (line.starts_with("you attempt to") && line.contains("but fail"))
        || line.contains("spell is resisted")
        || line.contains("resists your spell")
        || line.contains("enough mana to cast")
        || line.contains("enough kai to invoke")
        || line.contains("already cast a spell")
        || line.contains("already invoked a power")
    {
        return Some(Outcome::Failed);
    }
    None
}
```

Replace the body of `HealState::on_event` from `let line = line.to_lowercase();` to the end of the function with:

```rust
        match cast_outcome(line) {
            Some(Outcome::Unknown) => {
                self.dead[i] = true;
                self.pending = None;
            }
            Some(Outcome::Cast) => {
                if matches!(self.sources[i].kind, HealKind::Regen { .. }) {
                    self.regen_cast_at = Some(now);
                }
                self.pending = None;
            }
            Some(Outcome::Failed) => self.pending = None,
            None => {}
        }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client -j 1 --test sheet --test tui --test farm 2>&1 | grep -E '^error|test result'`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/sheet.rs crates/mud-client/tests/sheet.rs
git commit -m "refactor(client): one outcome grammar for casts"
```

---

### Task 7: The party cast state

**Files:**
- Modify: `crates/mud-client/src/sheet.rs` (add `PartyKind`, `PartySource`, `Spellbook::party_heals`, `Target`, `PartyHeal`, `HealState::affords`, `HealState::hold_round`)
- Test: `crates/mud-client/tests/sheet.rs`

**Interfaces:**
- Consumes: `party::Health`, `party::Vitals`, `bot::heal_need`, `bot::BotConfig`, `Outcome`, `cast_outcome`.
- Produces:
  - `PartyKind::{Single(HealKind), Area, Cure}`, `PartySource { name, cmd, mana_cost, kind }`.
  - `Spellbook::party_heals(&self, heals: &[HealSource], casting: Casting) -> Vec<PartySource>`.
  - `Target::{Me, One(String), Room(Vec<String>)}`.
  - `PartyHeal::new(sources)`, `is_empty()`, `has_cure()`, `in_flight()`, `hold_round(now)`, `poison_self(now)`, `attempt(now, clock, health: &Health, own_percent: Option<i32>, cfg: &BotConfig) -> CastAttempt`, `on_sent(line, id)`, `on_event(cor, now)`, `new_visit()`.
  - `HealState::affords(&self, need: HealNeed) -> bool`, `HealState::hold_round(&mut self, now)`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/mud-client/tests/sheet.rs`. Add these imports at the top: `use std::time::{Duration, Instant};`, `use mud_client::party::{Health, Member};`, `use mud_client::bot::BotConfig;`, and `PartyHeal, PartyKind, Target` to the `mud_client::sheet` import list (add `HealSource, HealState, HealNeed, CastAttempt, CmdId, Correlated, Event, RoundClock` wherever the file does not already import them; check the existing `use` lines first).

```rust
// --- PartyHeal ---------------------------------------------------------

fn party_book() -> Spellbook {
    Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   2    mihe  minor healing                 \n\
         \x20 8   6    mahe  major healing                 \n\
         \x2010   5    rain  healing rain                  \n\
         \x20 7   8    cure  cure poison                   \n",
    )
}

fn party_state() -> PartyHeal {
    let book = party_book();
    let heals = book.heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0;
    PartyHeal::new(book.party_heals(&heals, Casting::Spells))
}

fn marks() -> BotConfig {
    BotConfig { minor_heal_at_percent: 70, major_heal_at_percent: 40, ..BotConfig::default() }
}

fn row(name: &str, class: &str, hp: u8) -> Member {
    Member { name: name.into(), invited: false, class: Some(class.into()), hp: Some(hp), pool: None }
}

fn health(rows: &[Member], at: Instant) -> Health {
    let mut h = Health::new();
    h.on_roster(rows, at);
    h
}

#[test]
fn the_book_yields_singles_area_and_cure() {
    let book = party_book();
    let heals = book.heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0;
    let sources = book.party_heals(&heals, Casting::Spells);
    let kinds: Vec<PartyKind> = sources.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![PartyKind::Single(HealKind::Minor), PartyKind::Single(HealKind::Major), PartyKind::Area, PartyKind::Cure]
    );
    assert_eq!(sources[1].cmd, "cast mahe");
    assert_eq!(sources[2].cmd, "cast rain");
    assert_eq!(sources[3].cmd, "cast cure");
    let plain = healer_book();
    let heals = plain.heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0;
    assert!(plain.party_heals(&heals, Casting::Spells).iter().all(|s| matches!(s.kind, PartyKind::Single(_))));
}

#[test]
fn one_hurt_member_gets_the_single_by_the_marks() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Ninja", 100)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let h = health(&[row("Celery", "Mage", 60)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mihe celery".into()));
}

#[test]
fn the_major_falls_back_to_the_minor_on_the_pool() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 4), t0);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mihe celery".into()));
    let mut broke = party_state();
    broke.on_event(&prompt(100, 1), t0);
    assert_eq!(broke.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Nothing);
    let mut unseen = party_state();
    assert_eq!(unseen.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Nothing, "a pool never seen affords nothing");
}

#[test]
fn two_under_the_mark_is_rain_and_the_healer_counts_itself() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Ninja", 50)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast rain".into()));
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(50), &marks()), CastAttempt::Send("cast rain".into()));
    let mut p = party_state();
    p.on_event(&prompt(100, 4), t0);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Ninja", 50)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mihe celery".into()), "rain unaffordable, the lowest gets what the pool buys");
}

#[test]
fn a_witchunter_is_neither_counted_nor_healed() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Witchunter", 20)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    let h = health(&[row("Beef", "Witchunter", 20)], t0);
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Nothing);
}

#[test]
fn cure_comes_before_heal_and_self_before_others() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let mut h = health(&[row("Celery", "Mage", 30)], t0);
    h.on_cure("Celery", t0);
    p.poison_self(t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast cure".into()));
    p.on_sent("cast cure", CmdId(1));
    p.on_event(&answering("You cast cure poison on Yourself!", CmdId(1)), t0);
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast cure celery".into()));
    p.on_sent("cast cure celery", CmdId(2));
    p.on_event(&answering("You cast cure poison on Celery!", CmdId(2)), t1);
    let t2 = t1 + Duration::from_secs(10);
    assert_eq!(p.attempt(t2, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()), "cured, the heal is next");
}

#[test]
fn a_cast_marks_its_targets_until_a_fresher_row() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let mut h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(1));
    assert!(p.in_flight());
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Nothing, "one cast out at a time");
    p.on_event(&answering("You cast major healing on Celery!", CmdId(1)), t1);
    assert!(!p.in_flight());
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Nothing, "healed since the row was seen");
    let t2 = t1 + Duration::from_secs(10);
    h.on_heal("Celery", 35, t2);
    assert_eq!(p.attempt(t2, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()), "a fresher row is due again");
}

#[test]
fn rain_marks_every_counted_row() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Ninja", 50)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast rain".into()));
    p.on_sent("cast rain", CmdId(1));
    p.on_event(&answering("You cast healing rain on the room!", CmdId(1)), t0);
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Nothing);
}

#[test]
fn a_failure_frees_the_next_round_and_an_unknown_spell_retires_the_source() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(1));
    p.on_event(&answering("You attempt to cast major healing at Celery, but fail.", CmdId(1)), t0);
    assert!(matches!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Hold(_)), "the round is spent");
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(2));
    p.on_event(&answering("You do not know how to cast mahe.", CmdId(2)), t1);
    let t2 = t1 + Duration::from_secs(10);
    assert_eq!(p.attempt(t2, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mihe celery".into()), "the major is dead, the minor answers");
}

#[test]
fn a_self_cast_holds_the_party_cast_a_round() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    p.hold_round(t0);
    assert!(matches!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Hold(_)));
    let mut s = heal_state();
    s.on_event(&prompt(20, 9), t0);
    s.hold_round(t0);
    assert!(matches!(s.attempt(t0, &clock, HealNeed::Minor), CastAttempt::Hold(_)));
    assert!(s.affords(HealNeed::Minor));
    let mut broke = heal_state();
    broke.on_event(&prompt(20, 1), t0);
    assert!(!broke.affords(HealNeed::Major));
}
```

If `prompt` and `answering` in this file take different arguments than shown, use the file's own helpers.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p mud-client -j 1 --test sheet 2>&1 | grep -E '^error|test result' | head`
Expected: compile errors, `cannot find type PartyHeal`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/sheet.rs`, add to `impl HealState`:

```rust
    /// Whether a live source could answer `need` with the pool as last
    /// seen. The ask for a party heal reads this: a character that
    /// can cast for itself does not ask.
    pub fn affords(&self, need: HealNeed) -> bool {
        let minor = self.affordable(|k| *k == HealKind::Minor).is_some();
        match need {
            HealNeed::Minor => minor,
            HealNeed::Major => self.affordable(|k| *k == HealKind::Major).is_some() || minor,
            HealNeed::Regen => self.affordable(|k| matches!(k, HealKind::Regen { .. })).is_some() || minor,
        }
    }

    /// Another cast of ours went out at `now`. The board allows one
    /// cast a round, so this one waits its turn.
    pub fn hold_round(&mut self, now: std::time::Instant) {
        self.last_attempt = Some(now);
    }
```

After `HEAL_SPELLS`, add:

```rust
/// The heals that target the room. `target` 13 in the spell table:
/// every player in the room is healed, the caster included.
pub const RAIN_SPELLS: [&str; 3] = ["healing rain", "major healing rain", "greater healing rain"];
pub const CURE_POISON: &str = "cure poison";
```

After `HealSource`, add:

```rust
/// What a party cast is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyKind {
    /// One member by name, the same minor or major the self heal uses.
    Single(HealKind),
    /// The room.
    Area,
    /// Cure poison, on one member or on the caster.
    Cure,
}

/// One spell this character can cast on the party.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartySource {
    pub name: String,
    /// `cast mahe`, without a target. The target is appended per cast.
    pub cmd: String,
    pub mana_cost: i32,
    pub kind: PartyKind,
}
```

Add to `impl Spellbook`:

```rust
    /// The casts this character can make on the party: the self heals
    /// it already has, minus a regen, then the first rain and cure
    /// poison if the book has them.
    pub fn party_heals(&self, heals: &[HealSource], casting: Casting) -> Vec<PartySource> {
        let mut out: Vec<PartySource> = heals
            .iter()
            .filter_map(|h| match h.kind {
                HealKind::Minor | HealKind::Major => Some(PartySource {
                    name: h.name.clone(),
                    cmd: h.cmd.clone(),
                    mana_cost: h.mana_cost,
                    kind: PartyKind::Single(h.kind),
                }),
                HealKind::Regen { .. } => None,
            })
            .collect();
        let named = |name: &str| self.spells.iter().find(|s| s.name.eq_ignore_ascii_case(name));
        if let Some(s) = RAIN_SPELLS.iter().find_map(|n| named(n)) {
            out.push(PartySource { name: s.name.clone(), cmd: casting.command(&s.short), mana_cost: s.mana as i32, kind: PartyKind::Area });
        }
        if let Some(s) = named(CURE_POISON) {
            out.push(PartySource { name: s.name.clone(), cmd: casting.command(&s.short), mana_cost: s.mana as i32, kind: PartyKind::Cure });
        }
        out
    }
```

After `HealState`'s impl, add:

```rust
/// Who a party cast was for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Me,
    One(String),
    /// The rows that were counted when the area cast went out.
    Room(Vec<String>),
}

/// Cast heals on the party, and confirm them from the board.
///
/// The judgments live here: which row is due, which spell answers it,
/// and who was healed by a cast. The facts, the rows themselves, live
/// in [`crate::party::Health`] and are handed in on every attempt.
///
/// A row is due when its number was seen after this character last
/// healed it. A member still hurt says `@heal` again each round, and
/// every member is polled, so a healed row comes due again on its own
/// when it is still low.
pub struct PartyHeal {
    sources: Vec<PartySource>,
    dead: Vec<bool>,
    mana: Option<i32>,
    /// A cast chosen by `attempt` and not yet released by `on_sent`.
    planned: Option<(usize, Target, String)>,
    /// A cast out and unanswered.
    pending: Option<(usize, Target, crate::correlate::CmdId)>,
    last_attempt: Option<std::time::Instant>,
    healed: std::collections::BTreeMap<String, std::time::Instant>,
    cured: std::collections::BTreeMap<String, std::time::Instant>,
    /// When this character last saw its own poison tick.
    self_poisoned: Option<std::time::Instant>,
}

impl PartyHeal {
    pub fn new(sources: Vec<PartySource>) -> Self {
        let dead = vec![false; sources.len()];
        PartyHeal {
            sources,
            dead,
            mana: None,
            planned: None,
            pending: None,
            last_attempt: None,
            healed: Default::default(),
            cured: Default::default(),
            self_poisoned: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn has_cure(&self) -> bool {
        self.sources.iter().zip(&self.dead).any(|(s, d)| !d && s.kind == PartyKind::Cure)
    }

    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    pub fn hold_round(&mut self, now: std::time::Instant) {
        self.last_attempt = Some(now);
    }

    /// `You feel ill.` was printed: this character is poisoned.
    pub fn poison_self(&mut self, now: std::time::Instant) {
        self.self_poisoned = Some(now);
    }

    fn affordable(&self, kind: impl Fn(PartyKind) -> bool) -> Option<usize> {
        let mana = self.mana?;
        self.sources.iter().zip(&self.dead).position(|(s, d)| !d && kind(s.kind) && s.mana_cost <= mana)
    }

    fn key(name: &str) -> String {
        name.to_ascii_lowercase()
    }

    fn plan(&mut self, i: usize, target: Target) -> CastAttempt {
        let cmd = match &target {
            Target::One(name) => format!("{} {}", self.sources[i].cmd, Self::key(name)),
            Target::Me | Target::Room(_) => self.sources[i].cmd.clone(),
        };
        self.planned = Some((i, target, cmd.clone()));
        CastAttempt::Send(cmd)
    }

    /// The one cast worth making at `now`, if any. Cure before heal,
    /// self before others, rain when two or more are under the mark,
    /// else the lowest by the marks.
    pub fn attempt(
        &mut self,
        now: std::time::Instant,
        clock: &crate::world::RoundClock,
        health: &crate::party::Health,
        own_percent: Option<i32>,
        cfg: &crate::bot::BotConfig,
    ) -> CastAttempt {
        if self.pending.is_some() || self.sources.is_empty() {
            return CastAttempt::Nothing;
        }
        if let Some(at) = self.last_attempt {
            let next = clock.next_round_after(at);
            if now < next {
                return CastAttempt::Hold(next);
            }
        }
        let cure = self.affordable(|k| k == PartyKind::Cure);
        if self.self_poisoned.is_some()
            && let Some(i) = cure
        {
            self.last_attempt = Some(now);
            return self.plan(i, Target::Me);
        }
        if let Some(i) = cure
            && let Some(row) = health.rows().find(|v| {
                !v.resists_magic()
                    && v.poisoned.is_some_and(|p| self.cured.get(&Self::key(&v.name)).is_none_or(|c| *c < p))
            })
        {
            self.last_attempt = Some(now);
            return self.plan(i, Target::One(row.name.clone()));
        }
        let mark = cfg.minor_heal_at_percent as i32;
        if mark == 0 {
            return CastAttempt::Nothing;
        }
        let mut due: Vec<&crate::party::Vitals> = health
            .rows()
            .filter(|v| {
                !v.resists_magic()
                    && v.hp.is_some_and(|h| (h as i32) < mark)
                    && self.healed.get(&Self::key(&v.name)).is_none_or(|h| *h < v.seen)
            })
            .collect();
        due.sort_by_key(|v| v.hp);
        let count = due.len() + usize::from(own_percent.is_some_and(|p| p < mark));
        if count >= 2
            && let Some(i) = self.affordable(|k| k == PartyKind::Area)
        {
            let names = due.iter().map(|v| v.name.clone()).collect();
            self.last_attempt = Some(now);
            return self.plan(i, Target::Room(names));
        }
        let Some(lowest) = due.first() else {
            return CastAttempt::Nothing;
        };
        let hp = lowest.hp.unwrap_or(0) as i32;
        let minor = self.affordable(|k| k == PartyKind::Single(HealKind::Minor));
        let picked = match crate::bot::heal_need(cfg, hp) {
            Some(HealNeed::Major) => self.affordable(|k| k == PartyKind::Single(HealKind::Major)).or(minor),
            Some(HealNeed::Minor | HealNeed::Regen) => minor,
            None => None,
        };
        let Some(i) = picked else {
            return CastAttempt::Nothing;
        };
        let name = lowest.name.clone();
        self.last_attempt = Some(now);
        self.plan(i, Target::One(name))
    }

    /// Called for every command the gate releases.
    pub fn on_sent(&mut self, line: &str, id: crate::correlate::CmdId) {
        if let Some((i, target, cmd)) = self.planned.take()
            && cmd == line
        {
            self.pending = Some((i, target, id));
        }
    }

    pub fn on_event(&mut self, cor: &crate::correlate::Correlated, now: std::time::Instant) {
        if let crate::events::Event::Prompt { mana: Some(mana), .. } = &cor.event {
            self.mana = Some(*mana);
        }
        let Some((i, _, pending)) = &self.pending else {
            return;
        };
        if cor.answers != Some(*pending) {
            return;
        }
        let crate::events::Event::Line(line) = &cor.event else {
            return;
        };
        let i = *i;
        match cast_outcome(line) {
            Some(Outcome::Unknown) => {
                self.dead[i] = true;
                self.pending = None;
            }
            Some(Outcome::Cast) => {
                let (_, target, _) = self.pending.take().expect("pending checked above");
                let marks = if self.sources[i].kind == PartyKind::Cure { &mut self.cured } else { &mut self.healed };
                match target {
                    Target::Me => self.self_poisoned = None,
                    Target::One(name) => {
                        marks.insert(Self::key(&name), now);
                    }
                    Target::Room(names) => {
                        for name in names {
                            marks.insert(Self::key(&name), now);
                        }
                    }
                }
            }
            Some(Outcome::Failed) => self.pending = None,
            None => {}
        }
    }

    /// A fresh visit clears an outcome that never arrived, the same
    /// bargain [`HealState::new_visit`] makes.
    pub fn new_visit(&mut self) {
        self.pending = None;
        self.planned = None;
    }
}
```

`sheet.rs` has no `use` of `crate::party` today; the paths above are fully qualified, so none is needed. If the borrow checker refuses the `marks` binding inside the `Cast` arm because `self.sources[i]` is read after `self.pending.take()`, read the kind into a local before the `take`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client -j 1 --test sheet 2>&1 | grep -E '^error|test result|panicked'`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/sheet.rs crates/mud-client/tests/sheet.rs
git commit -m "feat(client): the party cast state picks cure, rain or a single heal off the table"
```

---

### Task 8: Introductions, asking and the poison tick in the assist

**Files:**
- Modify: `crates/mud-client/src/party.rs` (add `AskState`)
- Modify: `crates/mud-client/src/farm.rs:2443-2450` and `2623-2649` (`Sheet.party`, `sheet_from`)
- Modify: `crates/mud-client/src/tui.rs` (`AssistCasts`, `carry_party`, `assist_tick`)
- Create: `crates/mud-client/tests/party_window_ask.rs`, `crates/mud-client/tests/party_window_iam.rs`
- Test: `crates/mud-client/tests/party.rs`

**Interfaces:**
- Consumes: `PartyHeal`, `Spellbook::party_heals`, `HealState::affords`, `party::say`, `Session::stats().race/class`, `Session::party_config()`.
- Produces: `party::AskState::new()`, `due(&self, now, clock) -> bool`, `on_sent(&mut self, now)`; `AssistCasts { .., party: PartyHeal, ask: AskState, met: BTreeSet<String> }`; `Sheet.party: Vec<PartySource>`.

- [ ] **Step 1: Write the failing unit test**

Add to `crates/mud-client/tests/party.rs` (import `AskState` and `mud_client::world::RoundClock`):

```rust
#[test]
fn the_ask_goes_out_once_per_round() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut a = AskState::new();
    assert!(a.due(t0, &clock));
    a.on_sent(t0);
    assert!(!a.due(t0 + Duration::from_millis(500), &clock));
    assert!(a.due(t0 + Duration::from_secs(10), &clock));
}
```

- [ ] **Step 2: Write the failing window tests**

Create `crates/mud-client/tests/party_window_ask.rs` by copying `crates/mud-client/tests/party_window_wait.rs` and changing:

Module doc:

```rust
//! A member under its heal mark with nothing to cast says `@heal` to
//! the room, once per round, and stops once it is over the mark. One
//! test in this binary, because it sets `XDG_CONFIG_HOME`.
```

Rename `rest_board` to `ask_board`. Its `stat` reply stays. Replace the tick branch's `let say = ...;` with:

```rust
                    let say = if step == 0 {
                        let say = format!("\r\nYou are now following Beef\r\n[HP={hp}/MA=0]:");
                        hp = 30;
                        say
                    } else if step < 8 {
                        format!("\r\n[HP={hp}/MA=0]:")
                    } else {
                        hp = 90;
                        format!("\r\n[HP={hp}/MA=0]:")
                    };
```

Drop the `rested` and `resting_left` variables and the `"rest"` arm. Change the tick interval to `Duration::from_millis(600)` so eight prompts span about five seconds, which is one round at the default clock.

Rename the test to `a_hurt_member_with_nothing_to_cast_asks_once_per_round`, the scratch dir to `party_window_ask`, and the settings: keep the connection keys, `farm.content`, `bot.assist_play`, `bot.max_hp`, and add:

```rust
    s.set("bot.auto_heal", "true").unwrap();
    s.set("bot.auto_rest", "false").unwrap();
    s.set("bot.minor_heal_at_percent", "70").unwrap();
    s.set("bot.major_heal_at_percent", "40").unwrap();
```

Replace the wait loop and assertions with:

```rust
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        if sent.lock().unwrap().iter().any(|l| l == "say @heal 30")
            && tokio::time::Instant::now() > deadline - Duration::from_secs(4)
        {
            break;
        }
        if tokio::time::timeout_at(deadline, front.recv()).await.is_err() {
            break;
        }
    }
    let log = sent.lock().unwrap().clone();
    let screen = handle.screen.lock().unwrap().text();
    let asks = log.iter().filter(|l| l.as_str() == "say @heal 30").count();
    assert!(asks >= 1, "the ask went out: {log:?}\nscreen:\n{screen}");
    assert!(asks <= 2, "once per round, not per prompt: {log:?}\nscreen:\n{screen}");
    assert!(!log.iter().any(|l| l == "say @heal 90"), "over the mark there is nothing to ask: {log:?}");
```

Create `crates/mud-client/tests/party_window_iam.rs` the same way:

```rust
//! A member introduces itself with `@iam <race> <class>` when it joins
//! a party and again to a newcomer. One test in this binary, because
//! it sets `XDG_CONFIG_HOME`.
```

Rename the board to `iam_board`. Its `stat` arm must carry a race:

```rust
                            "stat" => format!("\r\nstat\r\nName: Beefy   Lives/CP: 9/2\r\nRace: Human      Exp: 0\r\nClass: Warrior   Level: 15\r\nMagicRes: 0\r\n[HP={hp}/MA=0]:"),
```

Add a `"party"` arm:

```rust
                            "party" => format!(
                                "\r\nparty\r\nThe following people are in your travel party:\r\n  Beef                           (Ninja)               [H:100%]   - Frontrank\r\n  Carrot                         (Paladin)    [M:100%] [H:100%]   - Midrank\r\n\r\n[HP={hp}/MA=0]:"
                            ),
```

Tick branch:

```rust
                    let say = if step == 0 {
                        format!("\r\nYou are now following Beef\r\n[HP={hp}/MA=0]:")
                    } else {
                        format!("\r\n[HP={hp}/MA=0]:")
                    };
```

Test name `a_member_introduces_itself_on_joining_and_to_a_newcomer`, scratch dir `party_window_iam`, settings: connection keys, `farm.content`, `bot.assist_play`, `bot.max_hp`, `party.poll_secs = "1"`. Wait loop: break when the log holds two lines equal to `say @iam Human Warrior` or at the ten second deadline. Assertions:

```rust
    let iams = log.iter().filter(|l| l.as_str() == "say @iam Human Warrior").count();
    assert_eq!(iams, 2, "once on joining, once for Carrot: {log:?}\nscreen:\n{screen}");
```

Note the first `@iam` needs the stat sheet, which the realm entry probe reads before the assist ticks, and the party, which the follow line gives. The second needs the roster from the poll to name Carrot.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p mud-client -j 1 --test party --test party_window_ask --test party_window_iam 2>&1 | grep -E '^error|test result|panicked' | head`
Expected: `party` fails to compile on `AskState`; the two window binaries build and fail their assertions.

- [ ] **Step 4: Implement `AskState` and the sheet**

In `crates/mud-client/src/party.rs`, after `WaitState`:

```rust
/// A member's requests to the room, one per round at most. A member
/// still hurt asks again next round, which keeps its row fresh for
/// every healer.
#[derive(Debug, Clone, Default)]
pub struct AskState {
    last: Option<Instant>,
}

impl AskState {
    pub fn new() -> AskState {
        AskState::default()
    }

    pub fn due(&self, now: Instant, clock: &crate::world::RoundClock) -> bool {
        self.last.is_none_or(|at| now >= clock.next_round_after(at))
    }

    pub fn on_sent(&mut self, now: Instant) {
        self.last = Some(now);
    }
}
```

In `crates/mud-client/src/farm.rs`, `Sheet` gains:

```rust
    /// The casts this character can make on the party.
    pub party: Vec<crate::sheet::PartySource>,
```

and `sheet_from` builds it after `heals`:

```rust
    let heals = book.heal_spells(
        crate::sheet::HealChoice {
            minor: &bot.minor_heal_spell,
            major: &bot.major_heal_spell,
            regen: &bot.hp_regen_spell,
        },
        durations,
        casting,
    );
    let party = book.party_heals(&heals.0, casting);
    Sheet {
        light: crate::sheet::light_sources(&inventory, &book, spells, casting),
        heals,
        party,
        buffs: crate::sheet::buffs(&book, &bot.buffs, durations, casting),
    }
```

Any test that builds a `Sheet` literal gains `party: Vec::new()`.

- [ ] **Step 5: Implement the assist side**

In `crates/mud-client/src/tui.rs`, `AssistCasts` gains three fields and `Default` fills them:

```rust
    /// The casts this character makes on the party. Empty until the
    /// sheet is read, and empty for a character with no heals.
    pub party: crate::sheet::PartyHeal,
    /// The member's own `@heal` and `@cure`, one per round.
    pub ask: crate::party::AskState,
    /// Members this character has introduced itself to.
    pub met: std::collections::BTreeSet<String>,
```

```rust
            party: crate::sheet::PartyHeal::new(Vec::new()),
            ask: crate::party::AskState::new(),
            met: Default::default(),
```

`carry_party` carries all three: `self.party = old.party; self.ask = old.ask; self.met = old.met;`. Extend its doc: the party cast state carries a cast in flight and the healed marks, and `met` carries who has been greeted, which a rebuild must not forget or the character greets the party again.

In `assist_tick`:

In the `if spells != *book_seen { ... }` block, after `casts.buff = ...`, add `casts.party = crate::sheet::PartyHeal::new(sheet.party);` and before `refusals = sheet.heals.1;` nothing else changes. Move the `refusals = sheet.heals.1;` line so `sheet.party` is moved out before `sheet.heals` is; both are moves out of a local, which Rust allows field by field.

After `casts.buff.on_event(cor, now);` add `casts.party.on_event(cor, now);`.

Replace the self heal block:

```rust
    if let crate::events::Event::Prompt { hp, .. } = &cor.event
        && let Some(cmd) = assist_heal(cfg, bot, &mut casts.heal, clock, *hp, now)
    {
        let id = session.send(&cmd);
        casts.heal.on_sent(&cmd, id);
        watch.on_sent(&cmd);
    }
```

with:

```rust
    let mut cast_this_prompt = false;
    if let crate::events::Event::Prompt { hp, .. } = &cor.event {
        if let Some(cmd) = assist_heal(cfg, bot, &mut casts.heal, clock, *hp, now) {
            let id = session.send(&cmd);
            casts.heal.on_sent(&cmd, id);
            casts.party.hold_round(now);
            watch.on_sent(&cmd);
            cast_this_prompt = true;
        } else if cfg.auto_heal
            && !bot.fled()
            && let Some(percent) = bot.hp_percent(*hp)
            && let Some(need) = crate::bot::heal_need(cfg, percent)
            && !casts.heal.affords(need)
            && !casts.heal.in_flight()
            && session.party().role != crate::party::Role::None
            && casts.ask.due(now, clock)
        {
            // Hurt, and nothing of its own to cast: the room is told.
            session.send(&crate::party::say(&format!("@heal {percent}")));
            session.party_note(format!("party: asked @heal {percent}"));
            casts.ask.on_sent(now);
        }
        introduce(session, casts);
    }
    if let crate::events::Event::Line(line) = &cor.event
        && line.trim() == "You feel ill."
    {
        // The poison tick, spellcasting.md §8.14. Self cure if the
        // book has it, else the room is asked. The line comes again
        // on the next tick, so an unanswered ask is made again.
        casts.party.poison_self(now);
        if !casts.party.has_cure() && session.party().role != crate::party::Role::None {
            session.send(&crate::party::say("@cure"));
            session.party_note("party: asked @cure".to_string());
        }
    }
```

`cast_this_prompt` is read by Task 9. Until then, prefix it with an underscore or read it in a `let _ = cast_this_prompt;` so the build stays warning free.

Add the function after `follower_leader`:

```rust
/// Say `@iam <race> <class>` to every member not yet greeted, once.
/// The whole party is unmet on joining and a newcomer is unmet when
/// the roster or the join line first names it. The race and class are
/// the stat sheet's; with either missing this waits for the next
/// prompt. A party that ended forgets everyone, so a party formed
/// again is greeted again.
fn introduce(session: &Session, casts: &mut AssistCasts) {
    let party = session.party();
    if party.role == crate::party::Role::None {
        casts.met.clear();
        return;
    }
    let names: Vec<String> = party.leader.iter().cloned().chain(party.followers()).collect();
    if names.iter().all(|n| casts.met.contains(&n.to_ascii_lowercase())) {
        return;
    }
    let stats = session.stats();
    let (Some(race), Some(class)) = (stats.race, stats.class) else {
        return;
    };
    session.send(&crate::party::say(&format!("@iam {race} {class}")));
    session.party_note(format!("party: said @iam {race} {class}"));
    casts.met.extend(names.iter().map(|n| n.to_ascii_lowercase()));
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p mud-client -j 1 --test party --test party_window_ask --test party_window_iam --test party_window_wait --test party_window_drag --test tui --test farm 2>&1 | grep -E '^error|^warning|test result|panicked'`
Expected: all `ok`. If the ask test sees zero asks, check that the board's prompt after `You are now following Beef` carries `HP=30` and that `bot.max_hp` is 100 so the percent is 30.

- [ ] **Step 7: Commit**

```bash
git add crates/mud-client/src/party.rs crates/mud-client/src/farm.rs crates/mud-client/src/tui.rs crates/mud-client/tests/party.rs crates/mud-client/tests/party_window_ask.rs crates/mud-client/tests/party_window_iam.rs
git commit -m "feat(client): a member introduces itself and asks the room for a heal or a cure"
```

---

### Task 9: Answering

**Files:**
- Modify: `crates/mud-client/src/tui.rs` (`assist_tick`)
- Modify: `docs/mud-client.md` (a paragraph in the party section)
- Create: `crates/mud-client/tests/party_window_heal.rs`

**Interfaces:**
- Consumes: `PartyHeal::attempt/on_sent`, `Session::party_health`, `Session::party_config().heal`, `cast_this_prompt` from Task 8.

- [ ] **Step 1: Write the failing window test**

Create `crates/mud-client/tests/party_window_heal.rs` from `crates/mud-client/tests/party_window_wait.rs`:

```rust
//! A member holding heals casts on a member that says it is hurt, and
//! again only after a fresh request. One test in this binary, because
//! it sets `XDG_CONFIG_HOME`.
```

The board is `heal_board`. Its prompts carry mana: every `[HP={hp}/MA=0]` in the file becomes `[HP={hp}/MA=20]`, `HOME_BLOCK` included. The `stat` reply stays. Add a `"spells"` arm:

```rust
                            "spells" => format!(
                                "\r\nspells\r\nYou have the following spells:\r\nLevel Mana Short Spell Name\r\n  1   2    mihe  minor healing                 \r\n  8   6    mahe  major healing                 \r\n[HP={hp}/MA=20]:"
                            ),
```

and a `"cast mahe beef"` arm:

```rust
                            "cast mahe beef" => format!("\r\ncast mahe beef\r\nYou cast major healing on Beef!\r\n[HP={hp}/MA=20]:"),
```

Tick branch:

```rust
                    let say = if step == 0 {
                        format!("\r\nYou are now following Beef\r\n[HP={hp}/MA=20]:")
                    } else if step == 3 || step == 12 {
                        format!("\r\nBeef says \"@heal 30\"\r\n[HP={hp}/MA=20]:")
                    } else {
                        format!("\r\n[HP={hp}/MA=20]:")
                    };
```

with the tick at 600 ms, so the two requests are about five seconds apart.

The world's class must cast: in `world()`, set `caster_group: 1` and `casting_factor: 1` so the realm entry probe reads `spells`. Check `crates/mud-client/src/farm.rs` `probe_sheet` for the exact condition it uses to decide whether to send `spells`, and set the class fields that condition reads.

Test name `a_healer_answers_a_said_request_by_name`, scratch dir `party_window_heal`, settings: connection keys, `farm.content`, `bot.assist_play`, `bot.max_hp`, `bot.auto_heal = "true"`, `bot.auto_rest = "false"`, `bot.minor_heal_at_percent = "70"`, `bot.major_heal_at_percent = "40"`, `party.poll_secs = "0"`.

Wait loop: break when the log holds two `cast mahe beef` lines or at a twelve second deadline. Assertions:

```rust
    let casts = log.iter().filter(|l| l.as_str() == "cast mahe beef").count();
    assert_eq!(casts, 2, "one cast per request, none in between: {log:?}\nscreen:\n{screen}");
    assert!(!log.iter().any(|l| l.starts_with("cast mahe beefy")), "never on itself by name: {log:?}");
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p mud-client -j 1 --test party_window_heal 2>&1 | grep -E '^error|test result|panicked' | head`
Expected: FAIL, zero casts.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/tui.rs` `assist_tick`, after the buff block and before `follower_gate(...)`:

```rust
    // The party's turn, after the character's own. One cast a round
    // between the two states, and nothing while either is out.
    if let crate::events::Event::Prompt { hp, .. } = &cor.event
        && !cast_this_prompt
        && !casts.heal.in_flight()
        && !casts.party.is_empty()
        && session.party_config().heal
        && session.party().role != crate::party::Role::None
    {
        let health = session.party_health();
        let own = bot.hp_percent(*hp);
        if let crate::sheet::CastAttempt::Send(cmd) = casts.party.attempt(now, clock, &health, own, cfg) {
            let id = session.send(&cmd);
            casts.party.on_sent(&cmd, id);
            casts.heal.hold_round(now);
            watch.on_sent(&cmd);
        }
    }
```

Remove the placeholder read of `cast_this_prompt` from Task 8.

In `docs/mud-client.md`, in the party section that describes `@wait` and `@ok`, add:

```markdown
A member under its heal mark with nothing of its own to cast says
`@heal <percent>` to the room, once per round while that holds, and
`@cure` on every poison tick it cannot cure itself. Every member polls
`party` every `poll_secs` seconds, so every window holds the roster's
class, pool and health for the whole party. A member whose book holds
a heal, healing rain or cure poison reads that table on every prompt
and casts: cure first, rain when two or more members are under its
minor mark, else the major or minor by its own marks on the lowest
member, by name. Every healer that can afford the spell casts, and a
double heal is accepted. Members say `@iam <race> <class>` on joining
and to each newcomer, and a witchunter, by the roster's class or its
own word, is never healed: it resists every spell.
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client -j 1 --test party_window_heal --test party_window_ask --test party_window_iam --test party_window_wait --test party_window_drag --test party_window_poll --test tui --test sheet --test party 2>&1 | grep -E '^error|^warning|test result|panicked'`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/tui.rs docs/mud-client.md crates/mud-client/tests/party_window_heal.rs
git commit -m "feat(client): a member with heals casts them on the party"
```

---

### Task 10: The whole suite

**Files:** none new.

- [ ] **Step 1: Build everything**

Run: `cargo build -p mud-client -j 1 --all-targets 2>&1 | grep -E '^(error|warning)' | head`
Expected: nothing.

- [ ] **Step 2: Run the whole client suite, one thread**

Run: `cargo test -p mud-client -j 1 -- --test-threads=1 2>&1 | grep -E 'test result|FAILED|panicked' | grep -v 'ok\. .* 0 failed' | head`
Expected: nothing, every binary `ok`. If a binary fails, fix it in the task that owns the file and re-run that binary before re-running the whole suite.

- [ ] **Step 3: Read the diff of the plan against the spec**

Run: `git log --oneline main..HEAD` if on a branch, else `git log --oneline -10`, and check each spec section has a commit: roster numbers, the words, the table, settings, the poll, the outcome grammar, the cast state, asking and introductions, answering, docs.
