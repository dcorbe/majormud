# Death Log and Recover Job Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The client logs every death with the room it happened in, and `/recover` sneaks to that room, searches once, picks up everything the search lists, and runs back to where it started without ever attacking.

**Architecture:** A new `deathlog.rs` owns the append-only `deaths.log` under the config directory and a `DeathWatch` that fires once per death from any of the three wordings. A new `recover.rs` owns the job: a pure `sweep_list` that turns the room's item line into `get` commands gear first, a `Haul` that counts what was taken, a `RecoverEnd` that renders the endings, and `run_recover`, which walks the route one hop at a time through the existing navigator so every arrival's sneak state is inspected. The correlator learns the bare `search` as a room-rendering command. The window and the map start the job the way they start a go.

**Tech Stack:** Rust 2024 edition, tokio, the existing `Navigator`, `FarmGuard`, `LightState` and `BuffState`, the scripted-board test harness from `tests/go_scripted.rs`, the keystroke-driven `MapView` tests in `tests/mapview.rs`, and the window rig in `tests/window.rs`.

**Spec:** `docs/plans/2026-09-07-recover-job-design.md`

## Global Constraints

- Never run `rustfmt` or `cargo fmt`.
- Every file ends with a blank line.
- No em dashes, parentheses or semicolons in prose you write: doc comments, commit messages, the doc. Code punctuation is code.
- Commit messages carry a tag with the crate in brackets as the repo does: `feat(client): ...`, `refactor(client): ...`, `test(client): ...`, `doc(client): ...`. No attribution lines. Do not mention the plan or the spec in commit messages.
- No test may read `re/`. Fixtures are hand-built. A test that needs a file puts it under `env!("CARGO_TARGET_TMPDIR")`, which cargo sets for integration tests. Never `/tmp`.
- Each task's test must fail before the implementation and pass after. Each task also runs the named mutation check: change the rule, watch the test fail, revert.
- Run the crate's tests with `cargo test -p mud-client` from the repo root. Build with `cargo build -p mud-client`. Pass `--test <file>` while iterating and run the whole crate once before each commit. `cargo clippy -p mud-client --all-targets` must add no warnings.
- Working directory for every command below is the repo root `/home/daniel/majormud/majormud`.
- This plan runs after the settings plan and the stealth plan. It consumes these names verbatim and never re-implements them: `BotConfig::auto_sneak: bool`, `NavConfig::sneak: bool`, `Session::profile_changes(&self) -> tokio::sync::watch::Receiver<Profile>`, `profile::config_dir() -> PathBuf`, `Navigator::with_stealth(buffs: Vec<Buff>, clock: RoundClock)`, `sheet::stealth_spells(spellbook, spells, durations, casting) -> (Vec<Buff>, Vec<String>)`.
- The job never sends an attack. Both switches are set: `session.travel_fights()` is false for the job's life, and the job's `BotConfig` has `auto_combat`, `auto_flee` and `auto_get` false.

---

## File Structure

- `crates/mud-client/src/lib.rs` (modify): `pub mod deathlog;` and `pub mod recover;`.
- `crates/mud-client/src/deathlog.rs` (create): `FixWord`, `Death` with `line` and `parse`, `stamp`, `civil_from_days`, `path`, `death_of`, `record_in`, `record`, `last_in`, `last`, `DeathWatch`, `watch_headless`.
- `crates/mud-client/src/correlate.rs` (modify): `Kind::SearchRoom` for the bare `search` and `sea`, completed by a room block, `Your search revealed nothing.` or `You may not search while attacking!`. `Kind::Get` also completes on `You don't see`.
- `crates/mud-client/src/recover.rs` (create): `Take`, `TakeKind`, `sweep_list`, `strip_article`, `GetReply`, `read_get_reply`, `Haul`, `HomeWhy`, `RecoverEnd`, `refusal`, `Configs`, `run_recover`, `cast_buffs`, and the private `recover`, `sweep`, `go_home`, `wait_a_prompt`, `room_name`.
- `crates/mud-client/src/farm.rs` (modify): `Phase::Preparing`, `Phase::SneakingIn`, `Phase::Sweeping`, `Phase::GoingHome` with labels. `ensure_lit` becomes `pub(crate)`.
- `crates/mud-client/src/sheet.rs` (modify): `BuffState::in_flight`.
- `crates/mud-client/src/tui.rs` (modify): `VERBS` gains `/recover`, `KeyOutcome::Recover { target: Option<String> }`, the `slash` arm, the `help_text` line, the lobby arm, `start_recover`.
- `crates/mud-client/src/window.rs` (modify): the `DeathWatch` in `play`, the death record after the map closes on a death, the `KeyOutcome::Recover` arm, the `ViewAction::Recover` arm.
- `crates/mud-client/src/mapview.rs` (modify): `ViewAction::Recover(RoomId)`, the `R` key, the legend.
- `crates/mud-client/src/bin/mmc.rs` (modify): `farm_command` spawns `deathlog::watch_headless`.
- `docs/mud-client.md` (modify): a `/recover` row in the command table, a "Recovering gear" section, the death log.
- Tests: `tests/deathlog.rs` (create), `tests/deathlog_window.rs` (create), `tests/correlate.rs` (modify), `tests/recover.rs` (create), `tests/recover_scripted.rs` (create), `tests/tui.rs` (modify), `tests/mapview.rs` (modify).

---

### Task 1: The death log file

**Files:**
- Create: `crates/mud-client/src/deathlog.rs`
- Modify: `crates/mud-client/src/lib.rs`
- Test: `crates/mud-client/tests/deathlog.rs`

**Interfaces:**
- Consumes: `profile::config_dir() -> PathBuf` from the settings plan. `crate::farm::parse_room_id(&str) -> Option<RoomId>` at `src/farm.rs:48`. `crate::lost::Fix` at `src/lost.rs:349`.
- Produces: `deathlog::FixWord`, `deathlog::Death { stamp: String, character: String, room: Option<RoomId>, name: String, fix: FixWord }`, `Death::line(&self) -> String`, `Death::parse(&str) -> Option<Death>`, `deathlog::stamp(SystemTime) -> String`, `deathlog::path() -> PathBuf`, `deathlog::death_of(character: &str, fix: Fix, name_of: impl FnOnce(RoomId) -> String, now: SystemTime) -> Death`, `deathlog::record_in(path: &Path, death: &Death) -> io::Result<()>`, `deathlog::record(death: &Death) -> io::Result<()>`, `deathlog::last_in(path: &Path, character: &str) -> Option<Death>`, `deathlog::last(character: &str) -> Option<Death>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/mud-client/tests/deathlog.rs`:

```rust
//! One line per death, kept across sessions, and read back for
//! `/recover`.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mud_client::deathlog::{Death, FixWord, death_of, last_in, record_in, stamp};
use mud_client::lost::Fix;
use mud_core::content::RoomId;

fn log_in(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("deathlog").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("deaths.log")
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn the_stamp_is_utc_to_the_second() {
    assert_eq!(stamp(at(1_788_792_127)), "2026-09-07T14:42:07Z");
    assert_eq!(stamp(at(951_782_400)), "2000-02-29T00:00:00Z", "a leap day");
    assert_eq!(stamp(at(0)), "1970-01-01T00:00:00Z");
}

#[test]
fn a_line_round_trips_with_a_room_name_that_has_spaces() {
    let death = Death {
        stamp: "2026-09-07T14:42:07Z".into(),
        character: "beef".into(),
        room: Some(RoomId { map: 1, room: 2810 }),
        name: "Darkwood Forest".into(),
        fix: FixWord::Confirmed,
    };
    assert_eq!(death.line(), "2026-09-07T14:42:07Z beef 1/2810 Darkwood Forest confirmed");
    assert_eq!(Death::parse(&death.line()), Some(death));
}

#[test]
fn a_death_with_no_room_still_writes_a_line() {
    let death = death_of("beef", Fix::Unknown, |_| unreachable!("no room to name"), at(0));
    assert_eq!(death.line(), "1970-01-01T00:00:00Z beef - - unknown");
    let back = Death::parse(&death.line()).unwrap();
    assert_eq!(back.room, None);
    assert_eq!(back.fix, FixWord::Unknown);
}

#[test]
fn a_stale_fix_is_written_as_stale() {
    let death = death_of(
        "beef",
        Fix::Stale(RoomId { map: 1, room: 2811 }),
        |id| format!("Room {}", id.room),
        at(0),
    );
    assert_eq!(death.line(), "1970-01-01T00:00:00Z beef 1/2811 Room 2811 stale");
}

#[test]
fn garbage_does_not_parse() {
    assert_eq!(Death::parse(""), None);
    assert_eq!(Death::parse("2026-09-07T14:42:07Z beef"), None);
    assert_eq!(Death::parse("2026-09-07T14:42:07Z beef 1/2810 Darkwood Forest maybe"), None);
}

#[test]
fn last_is_the_newest_line_with_a_room_for_that_character() {
    let path = log_in("last");
    let deaths = [
        death_of("beef", Fix::Confirmed(RoomId { map: 1, room: 1 }), |_| "One".into(), at(10)),
        death_of("salad", Fix::Confirmed(RoomId { map: 1, room: 2 }), |_| "Two".into(), at(20)),
        death_of("beef", Fix::Stale(RoomId { map: 1, room: 3 }), |_| "Three".into(), at(30)),
        death_of("beef", Fix::Unknown, |_| String::new(), at(40)),
    ];
    for d in &deaths {
        record_in(&path, d).unwrap();
    }
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 4, "append only: {text}");
    let last = last_in(&path, "beef").unwrap();
    assert_eq!(last.room, Some(RoomId { map: 1, room: 3 }), "the unknown one is skipped");
    assert_eq!(last.name, "Three");
    assert_eq!(last_in(&path, "salad").unwrap().room, Some(RoomId { map: 1, room: 2 }));
    assert_eq!(last_in(&path, "nobody"), None);
}

#[test]
fn a_missing_log_has_no_last_death() {
    let path = log_in("missing");
    assert_eq!(last_in(&path, "beef"), None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test deathlog`
Expected: a compile error, `unresolved import mud_client::deathlog`.

- [ ] **Step 3: Write the module**

Create `crates/mud-client/src/deathlog.rs`:

```rust
//! Where the character died, kept across sessions.
//!
//! A full death drops everything the character carries into the room it
//! happened in, and the client had no record of which room that was: a
//! job that ended in a death carried no room, and hand play recorded
//! nothing at all. One line per death in `deaths.log` under the config
//! directory is the record. `/recover` with no argument reads the newest
//! one back.
//!
//! Append only, never rewritten, so the file is the history.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mud_core::content::RoomId;

use crate::lost::Fix;

/// How much the logged room was worth when the death was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixWord {
    Confirmed,
    Stale,
    Unknown,
}

impl FixWord {
    fn word(self) -> &'static str {
        match self {
            FixWord::Confirmed => "confirmed",
            FixWord::Stale => "stale",
            FixWord::Unknown => "unknown",
        }
    }

    fn parse(word: &str) -> Option<FixWord> {
        Some(match word {
            "confirmed" => FixWord::Confirmed,
            "stale" => FixWord::Stale,
            "unknown" => FixWord::Unknown,
            _ => return None,
        })
    }
}

/// One logged death.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Death {
    /// UTC, to the second, as [`stamp`] writes it.
    pub stamp: String,
    pub character: String,
    /// `None` when nothing was known about where.
    pub room: Option<RoomId>,
    /// The room's name as the graph has it. Empty when `room` is `None`.
    pub name: String,
    pub fix: FixWord,
}

impl Death {
    /// The line as written: stamp, character, id or `-`, name or `-`,
    /// fix word. Single spaces between fields. The name may contain
    /// spaces, which is why it sits between two fields that cannot.
    pub fn line(&self) -> String {
        let (id, name) = match self.room {
            Some(r) => (format!("{}/{}", r.map, r.room), self.name.as_str()),
            None => ("-".to_string(), "-"),
        };
        format!("{} {} {} {} {}", self.stamp, self.character, id, name, self.fix.word())
    }

    pub fn parse(line: &str) -> Option<Death> {
        let mut words = line.split_whitespace();
        let stamp = words.next()?.to_string();
        let character = words.next()?.to_string();
        let id = words.next()?;
        let rest: Vec<&str> = words.collect();
        let (fix, name) = rest.split_last()?;
        let fix = FixWord::parse(fix)?;
        let room = if id == "-" {
            None
        } else {
            Some(crate::farm::parse_room_id(id)?)
        };
        let name = match room {
            Some(_) => name.join(" "),
            None => String::new(),
        };
        Some(Death {
            stamp,
            character,
            room,
            name,
            fix,
        })
    }
}

/// `2026-09-07T14:42:07Z`. UTC, because the client has no timezone table
/// and an honest UTC beats a wrong local time. Hand-rolled because the
/// crate carries no date dependency and this is the only date it writes.
pub fn stamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let t = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

/// Days since 1970-01-01 to a civil date. Howard Hinnant's algorithm,
/// which is exact for every day the proleptic Gregorian calendar has.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `~/.config/mmc/deaths.log`, beside the profiles and the loop library.
pub fn path() -> PathBuf {
    crate::profile::config_dir().join("deaths.log")
}

/// A death as it will be logged: the character's last fix, current or
/// not, because a stale room beats no room. `name_of` is asked only when
/// there is a room to name, so a caller with no graph can pass a closure
/// that never runs.
pub fn death_of(
    character: &str,
    fix: Fix,
    name_of: impl FnOnce(RoomId) -> String,
    now: SystemTime,
) -> Death {
    let (room, word) = match fix {
        Fix::Confirmed(at) => (Some(at), FixWord::Confirmed),
        Fix::Stale(at) => (Some(at), FixWord::Stale),
        Fix::Unknown => (None, FixWord::Unknown),
    };
    let name = room.map(name_of).unwrap_or_default();
    Death {
        stamp: stamp(now),
        character: character.to_string(),
        room,
        name,
        fix: word,
    }
}

/// Append one line. The directory is made if it is missing.
pub fn record_in(path: &Path, death: &Death) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", death.line())
}

pub fn record(death: &Death) -> std::io::Result<()> {
    record_in(&path(), death)
}

/// The newest logged death for `character` that carries a room. A line
/// that does not parse is skipped rather than fatal: the file is
/// hand-editable and one bad line must not hide the good ones.
pub fn last_in(path: &Path, character: &str) -> Option<Death> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .filter_map(Death::parse)
        .find(|d| d.character == character && d.room.is_some())
}

pub fn last(character: &str) -> Option<Death> {
    last_in(&path(), character)
}
```

Add to `crates/mud-client/src/lib.rs`, in alphabetical order after `pub mod correlate;`:

```rust
pub mod deathlog;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test deathlog`
Expected: `test result: ok. 7 passed`

- [ ] **Step 5: Mutation check**

In `last_in`, change `.find(|d| d.character == character && d.room.is_some())` to `.find(|d| d.character == character)`. Run the test file. `last_is_the_newest_line_with_a_room_for_that_character` fails because the unknown line wins. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/deathlog.rs crates/mud-client/src/lib.rs crates/mud-client/tests/deathlog.rs
git commit -m "feat(client): a death log under the config directory, one line per death"
```

---

### Task 2: Seeing a death once

**Files:**
- Modify: `crates/mud-client/src/deathlog.rs`
- Test: `crates/mud-client/tests/deathlog.rs`

**Interfaces:**
- Consumes: `crate::events::Event` at `src/events.rs:90`. `crate::farm::is_player_death(line: &str, username: &str) -> bool` at `src/farm.rs:1478`.
- Produces: `deathlog::DeathWatch` with `DeathWatch::default()`, `on_event(&mut self, ev: &Event, character: &str) -> bool`, `mark_dead(&mut self)`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/deathlog.rs`:

```rust
use mud_client::deathlog::DeathWatch;
use mud_client::events::Event;

fn prompt(hp: i32) -> Event {
    Event::Prompt {
        hp,
        mana: None,
        status: None,
    }
}

fn line(text: &str) -> Event {
    Event::Line(text.to_string())
}

/// The board says a death three ways, and all three usually arrive for
/// one death. The first fires, the rest are held until the character
/// is alive again.
#[test]
fn the_first_of_the_three_wordings_fires_and_the_rest_are_held() {
    let mut w = DeathWatch::default();
    assert!(w.on_event(&line("You have been killed."), "Beef"));
    assert!(!w.on_event(&line("Beef is dead."), "Beef"));
    assert!(!w.on_event(&prompt(0), "Beef"));
    assert!(!w.on_event(&prompt(-3), "Beef"));
}

#[test]
fn each_wording_fires_on_its_own() {
    for ev in [line("You have been killed."), line("Beef is dead."), prompt(0)] {
        let mut w = DeathWatch::default();
        assert!(w.on_event(&ev, "Beef"), "{ev:?}");
    }
}

#[test]
fn a_prompt_above_zero_re_arms_the_watch() {
    let mut w = DeathWatch::default();
    assert!(w.on_event(&prompt(0), "Beef"));
    assert!(!w.on_event(&prompt(0), "Beef"));
    assert!(!w.on_event(&prompt(22), "Beef"), "alive again is not a death");
    assert!(w.on_event(&prompt(0), "Beef"), "the next death counts");
}

#[test]
fn somebody_elses_death_is_not_ours() {
    let mut w = DeathWatch::default();
    assert!(!w.on_event(&line("Salad is dead."), "Beef"));
    assert!(!w.on_event(&line("The giant rat is dead."), "Beef"));
    assert!(!w.on_event(&prompt(22), "Beef"));
}

#[test]
fn a_death_marked_from_outside_is_not_fired_again() {
    let mut w = DeathWatch::default();
    w.mark_dead();
    assert!(!w.on_event(&prompt(0), "Beef"));
    assert!(!w.on_event(&prompt(22), "Beef"));
    assert!(w.on_event(&prompt(0), "Beef"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test deathlog`
Expected: a compile error, `unresolved import mud_client::deathlog::DeathWatch`.

- [ ] **Step 3: Write the watch**

Append to `crates/mud-client/src/deathlog.rs`:

```rust
/// Sees a death exactly once.
///
/// The board says it three ways: the prompt drops to zero hitpoints,
/// `You have been killed.` is printed to the dying player, and
/// `<name> is dead.` is printed to the room. All three usually arrive
/// for one death, so the first fires and the rest are held until a
/// prompt shows the character alive again.
#[derive(Debug, Default)]
pub struct DeathWatch {
    dead: bool,
}

impl DeathWatch {
    /// True exactly once per death.
    pub fn on_event(&mut self, ev: &crate::events::Event, character: &str) -> bool {
        use crate::events::Event;
        match ev {
            Event::Prompt { hp, .. } if *hp > 0 => {
                self.dead = false;
                false
            }
            Event::Prompt { .. } => self.fire(),
            Event::Line(line)
                if line.trim() == "You have been killed."
                    || crate::farm::is_player_death(line, character) =>
            {
                self.fire()
            }
            _ => false,
        }
    }

    /// A death seen by somebody else, the map closing on the death
    /// line, so the wordings still to arrive do not fire a second time.
    pub fn mark_dead(&mut self) {
        self.dead = true;
    }

    fn fire(&mut self) -> bool {
        if self.dead {
            return false;
        }
        self.dead = true;
        true
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test deathlog`
Expected: `test result: ok. 12 passed`

- [ ] **Step 5: Mutation check**

In `on_event`, delete the arm `Event::Prompt { hp, .. } if *hp > 0 => { self.dead = false; false }`. `a_prompt_above_zero_re_arms_the_watch` fails on the fourth assertion. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/deathlog.rs crates/mud-client/tests/deathlog.rs
git commit -m "feat(client): a death is seen once, from any of the three wordings"
```

---

### Task 3: The window and the headless farm write the log

**Files:**
- Modify: `crates/mud-client/src/window.rs` (the `play` locals around line 594, the `ev = events.recv()` arm around line 688, the map arm around line 1130)
- Modify: `crates/mud-client/src/deathlog.rs`
- Modify: `crates/mud-client/src/bin/mmc.rs` (`farm_command`, after the session connects around line 398)
- Test: `crates/mud-client/tests/deathlog_window.rs`

**Interfaces:**
- Consumes: `DeathWatch`, `death_of`, `record` from Tasks 1 and 2. `Session::character_name` at `src/session.rs:899`. `Session::state()` at `src/session.rs:850` and `GameState { hp, room, .. }` at `src/session.rs:85`. `RoomGraph::rooms_named` at `src/graph.rs:1001`. `window::spawn` and the rig shape in `tests/window.rs:142`.
- Produces: `deathlog::watch_headless(session: Arc<Session>, graph: Arc<RoomGraph>) -> tokio::task::JoinHandle<()>`. The window prints `-- death logged: <line> --` when it writes.

- [ ] **Step 1: Write the failing test**

Create `crates/mud-client/tests/deathlog_window.rs`. One test in its own binary, because it points the config directory at a scratch directory through the environment, and the environment is process-wide.

```rust
//! A window writes the death log. One test in this binary, because it
//! sets `XDG_CONFIG_HOME` for the whole process and nothing else may
//! run beside it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::settings::Settings;
use mud_client::tui::ContentCache;
use mud_client::window::{EventKind, FrontMsg, spawn};

/// A board that greets, waits for the window to settle, then kills the
/// character with the wording the dying player sees, and holds the
/// line open.
async fn killing_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\n[HP=22/MA=0]:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sock.write_all(b"\r\nYou have been killed.\r\n[HP=0/MA=0]:").await.unwrap();
        let mut hold = [0u8; 256];
        while sock.read(&mut hold).await.unwrap_or(0) > 0 {}
    });
    addr
}

#[tokio::test]
async fn a_death_in_hand_play_is_logged_with_no_room_when_none_is_known() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("deathlog_window");
    let _ = std::fs::remove_dir_all(&base);
    // Before any other task exists in this process. The only test in
    // this binary, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let addr = killing_board().await;
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beef\"").unwrap();
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let cache = Arc::new(Mutex::new(ContentCache::default()));
    let handle = spawn(7, s, 24, 80, cache, tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut connected = false;
    loop {
        let msg = tokio::time::timeout_at(deadline, front.recv())
            .await
            .expect("timed out waiting for the death to be logged")
            .expect("the window task ended");
        if matches!(msg, FrontMsg::Event { kind: EventKind::Connected, .. }) {
            connected = true;
        }
        if handle.screen.lock().unwrap().text().contains("death logged") {
            break;
        }
    }
    assert!(connected);
    let text = std::fs::read_to_string(base.join("mmc").join("deaths.log")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one death, one line: {text}");
    assert!(lines[0].ends_with(" beef - - unknown"), "{text}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mud-client --test deathlog_window`
Expected: FAIL, `timed out waiting for the death to be logged`.

- [ ] **Step 3: Wire the window**

In `crates/mud-client/src/window.rs`, in `play`, after `let mut here = crate::lost::Fix::Unknown;` around line 594, add:

```rust
    // One death, one line in the log. Fed every event the session
    // produces, in hand play and under a job alike, so the room is the
    // one `here` held when the killing prompt arrived rather than the
    // recall room the board renders a moment later.
    let mut deaths = crate::deathlog::DeathWatch::default();
```

In the `ev = events.recv()` arm, right after `if let Ok(cor) = &ev {` and before the `realm_presence` check, add:

```rust
                    let name = session.character_name().unwrap_or_default();
                    if deaths.on_event(&cor.event, &name) {
                        let death = crate::deathlog::death_of(
                            &name,
                            here,
                            |id| {
                                graph
                                    .as_ref()
                                    .and_then(|g| g.room(id))
                                    .map(|r| r.name.clone())
                                    .unwrap_or_default()
                            },
                            std::time::SystemTime::now(),
                        );
                        w.note(&match crate::deathlog::record(&death) {
                            Ok(()) => format!("-- death logged: {} --", death.line()),
                            Err(e) => format!("-- death not logged: {e} --"),
                        });
                    }
```

In the map arm, where `Ok(exit) =>` handles the map's return around line 1130, after `here = exit.here;` and the `interrupted` note, add:

```rust
                                                    // The map consumed the events while
                                                    // it owned the screen, so a death it
                                                    // closed on is logged here from the
                                                    // fix it handed back, and the watch is
                                                    // told so the prompt that follows does
                                                    // not log it twice.
                                                    if exit.interrupted.as_deref() == Some("you died; the map is closed") {
                                                        deaths.mark_dead();
                                                        let name = session.character_name().unwrap_or_default();
                                                        let death = crate::deathlog::death_of(
                                                            &name,
                                                            here,
                                                            |id| g.room(id).map(|r| r.name.clone()).unwrap_or_default(),
                                                            std::time::SystemTime::now(),
                                                        );
                                                        w.note(&match crate::deathlog::record(&death) {
                                                            Ok(()) => format!("-- death logged: {} --", death.line()),
                                                            Err(e) => format!("-- death not logged: {e} --"),
                                                        });
                                                    }
```

`g` is the graph the map arm already matched on. Check the exact binding name in that arm before using it.

- [ ] **Step 4: The headless watcher**

Append to `crates/mud-client/src/deathlog.rs`:

```rust
/// Log deaths on a session no window is watching, which is `mmc farm`.
///
/// The room is the last block the board rendered, resolved by its name
/// when the graph has exactly one room by that name, and unknown
/// otherwise. That is the same rule the headless status bar uses, and
/// it is coarser than the window's locator on purpose: a headless run
/// has no locator, and a death is not the moment to build one.
pub fn watch_headless(
    session: std::sync::Arc<crate::session::Session>,
    graph: std::sync::Arc<crate::graph::RoomGraph>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut events = session.events();
        let mut watch = DeathWatch::default();
        let mut fix = Fix::Unknown;
        loop {
            let cor = match events.recv().await {
                Ok(cor) => cor,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            };
            if let crate::events::Event::RoomSeen(room) = &cor.event
                && !cor.elsewhere
            {
                let named = graph.rooms_named(&room.name);
                fix = match named.as_slice() {
                    [one] => Fix::Confirmed(*one),
                    _ => fix.demote(),
                };
            }
            let name = session.character_name().unwrap_or_default();
            if watch.on_event(&cor.event, &name) {
                let death = death_of(
                    &name,
                    fix,
                    |id| graph.room(id).map(|r| r.name.clone()).unwrap_or_default(),
                    SystemTime::now(),
                );
                match record(&death) {
                    Ok(()) => eprintln!("death logged: {}", death.line()),
                    Err(e) => eprintln!("death not logged: {e}"),
                }
            }
        }
    })
}
```

In `crates/mud-client/src/bin/mmc.rs`, in `farm_command`, directly after the `let session = match Session::connect(...)` block that binds `session` as an `Arc`, add:

```rust
        // Nobody is watching this session, so the death log is written
        // from here. The handle is dropped with the runtime.
        let _deaths = mud_client::deathlog::watch_headless(session.clone(), graph.clone());
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test deathlog_window --test deathlog --test window`
Expected: all pass. The window binary's existing tests still pass, since the watch fires nothing on a banner.

- [ ] **Step 6: Mutation check**

In the window's event arm, change `if deaths.on_event(&cor.event, &name) {` to `if false && deaths.on_event(&cor.event, &name) {`. `a_death_in_hand_play_is_logged_with_no_room_when_none_is_known` times out. Revert.

- [ ] **Step 7: Commit**

```bash
git add crates/mud-client/src/window.rs crates/mud-client/src/deathlog.rs crates/mud-client/src/bin/mmc.rs crates/mud-client/tests/deathlog_window.rs
git commit -m "feat(client): the window and the headless farm write the death log"
```

---

### Task 4: The correlator learns the bare search and the missing item

**Files:**
- Modify: `crates/mud-client/src/correlate.rs` (`Kind` at line 144, `kind_of` at line 252, `completes` at line 296)
- Test: `crates/mud-client/tests/correlate.rs`

**Interfaces:**
- Consumes: the `Correlator`, `CmdId`, and the `ans`, `line`, `room` helpers already in `tests/correlate.rs:36-52`.
- Produces: a bare `search` or `sea` is `Kind::SearchRoom`, retired by a room block, `Your search revealed nothing.` or `You may not search while attacking!`. A `get` is also retired by `You don't see`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/correlate.rs`:

```rust
// ------------------------------------------------------ the bare search

/// A bare `search` re-lists the room, hidden items included, so the
/// reply IS a room block. The directed form keeps its own kind, which
/// a block never answers.
#[test]
fn a_bare_search_is_answered_by_the_room_it_re_lists() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "search", t);
    assert_eq!(ans(&mut c, line("search"), t), Some(CmdId(1)), "the echo accepts");
    assert_eq!(ans(&mut c, room("Darkwood Forest"), t), Some(CmdId(1)));
}

#[test]
fn a_bare_search_that_found_nothing_answers_with_the_nothing_line() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "sea", t);
    assert_eq!(ans(&mut c, line("sea"), t), Some(CmdId(1)));
    assert_eq!(ans(&mut c, line("Your search revealed nothing."), t), Some(CmdId(1)));
}

#[test]
fn a_bare_search_in_combat_answers_with_the_refusal() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "search", t);
    assert_eq!(ans(&mut c, line("search"), t), Some(CmdId(1)));
    assert_eq!(
        ans(&mut c, line("You may not search while attacking!"), t),
        Some(CmdId(1))
    );
}

/// Somebody else got there first. Without this the `get` lingered for
/// its whole deadline and the next reply landed on it.
#[test]
fn a_get_is_answered_by_dont_see() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "get rusty dagger", t);
    assert_eq!(ans(&mut c, line("get rusty dagger"), t), Some(CmdId(1)));
    assert_eq!(
        ans(&mut c, line("You don't see a rusty dagger here."), t),
        Some(CmdId(1))
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test correlate`
Expected: the three bare search tests fail with `left: None, right: Some(CmdId(1))` on the second assertion, and `a_get_is_answered_by_dont_see` fails the same way.

- [ ] **Step 3: Teach the correlator**

`completes` is the only exhaustive match on `Kind`, so the new variant needs one arm there and nowhere else.

In `crates/mud-client/src/correlate.rs`, in `enum Kind` after `Search,` add:

```rust
    /// A bare `search`, or its shortest live form `sea`. It re-lists the
    /// room with the hidden items shown, so the reply is a room block,
    /// or the nothing line when the floor is bare, or the refusal when
    /// something is fighting the character. Its own kind rather than
    /// `Search` because a block never answers the directed form.
    SearchRoom,
```

In `kind_of`, before the `if cmd.starts_with("search ") {` arm, add:

```rust
    if cmd == "search" || cmd == "sea" {
        return Kind::SearchRoom;
    }
```

Rewrite the comment above the directed arm so it no longer says the bare form is unmodelled:

```rust
    // The DIRECTED form. The bare form is `SearchRoom` above.
```

In `completes`, in the `Event::RoomSeen(_) =>` arm, add `Kind::SearchRoom` to the `matches!`:

```rust
            return matches!(
                kind,
                Kind::Move | Kind::Look | Kind::LookDir | Kind::Bash | Kind::SearchRoom
            );
```

In the `match kind` below it, after the `Kind::Search =>` arm, add:

```rust
        Kind::SearchRoom => {
            has("search revealed nothing") || has("may not search while attacking")
        }
```

Change the `Kind::Get` arm to:

```rust
        Kind::Get => {
            has("you picked up") || (has("you took ") && !has("damage")) || has("you don't see")
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test correlate`
Expected: all pass, including the three existing directed-search tests, which still refuse a room block.

- [ ] **Step 5: Mutation check**

Remove `| Kind::SearchRoom` from the `RoomSeen` arm. `a_bare_search_is_answered_by_the_room_it_re_lists` fails. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/correlate.rs crates/mud-client/tests/correlate.rs
git commit -m "feat(client): the correlator retires a bare search and a get nobody can see"
```

---

### Task 5: The sweep's list, replies, haul and endings, without a board

**Files:**
- Create: `crates/mud-client/src/recover.rs`
- Modify: `crates/mud-client/src/lib.rs`
- Test: `crates/mud-client/tests/recover.rs`

**Interfaces:**
- Consumes: `crate::bot::coin_pile(&str) -> Option<(u32, String)>` at `src/bot.rs:33`, `crate::bot::picked_up` at `src/bot.rs:65`, `crate::bot::picked_up_item` at `src/bot.rs:90`, `crate::farm::Phase` and `crate::farm::DIED` at `src/farm.rs:387` and `1310`.
- Produces: `recover::Take { label: String, cmd: String, kind: TakeKind }`, `recover::TakeKind::{Gear, Coins}`, `recover::sweep_list(items: &[String]) -> Vec<Take>`, `recover::GetReply::{Taken, Gone, Other}`, `recover::read_get_reply(line: &str) -> GetReply`, `recover::Haul { taken: Vec<String>, items_wanted, items_taken, coins_wanted, coins_taken: usize }` with `Haul::wanted(&[Take]) -> Haul`, `Haul::took(&mut self, &Take)`, `Haul::summary(&self) -> String`, `recover::HomeWhy::{Swept, Nothing, Broke { at, name }, Attacked { at, name }, Hurt { mark }}`, `recover::RecoverEnd::{Home { at, why, haul }, Stopped { at, name, haul }, Died { haul }}` with `RecoverEnd::phase(&self) -> Phase` and `RecoverEnd::haul(&self) -> &Haul`.

- [ ] **Step 1: Write the failing tests**

Create `crates/mud-client/tests/recover.rs`:

```rust
//! The recover job's pure parts: what the sweep asks for, what a reply
//! meant, and how an ending reads.

use mud_client::farm::{DIED, Phase};
use mud_client::recover::{
    GetReply, Haul, HomeWhy, RecoverEnd, Take, TakeKind, read_get_reply, sweep_list,
};
use mud_core::content::RoomId;

fn items(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn cmds(takes: &[Take]) -> Vec<&str> {
    takes.iter().map(|t| t.cmd.as_str()).collect()
}

/// Gear first, coins last, so a forced exit leaves coins behind rather
/// than equipment.
#[test]
fn the_sweep_takes_gear_before_coins_in_listed_order() {
    let list = sweep_list(&items(&[
        "5 copper farthings",
        "a rusty dagger",
        "an iron helm",
        "11 silver nobles",
        "the Book of Eldritch Lore",
    ]));
    assert_eq!(
        cmds(&list),
        vec![
            "get rusty dagger",
            "get iron helm",
            "get Book of Eldritch Lore",
            "get copper",
            "get silver"
        ]
    );
    assert_eq!(list[0].kind, TakeKind::Gear);
    assert_eq!(list[0].label, "a rusty dagger");
    assert_eq!(list[3].kind, TakeKind::Coins);
    assert_eq!(list[3].label, "5 copper farthings");
}

/// An item that wears a denomination word is still an item: only a
/// leading count makes a coin pile.
#[test]
fn a_silver_amulet_is_gear_not_silver() {
    let list = sweep_list(&items(&["a silver holy amulet"]));
    assert_eq!(cmds(&list), vec!["get silver holy amulet"]);
    assert_eq!(list[0].kind, TakeKind::Gear);
}

#[test]
fn an_empty_entry_is_skipped() {
    assert!(sweep_list(&items(&["", "  "])).is_empty());
}

#[test]
fn a_get_reply_is_read_by_its_wording() {
    assert_eq!(read_get_reply("You took a rusty dagger."), GetReply::Taken);
    assert_eq!(read_get_reply("You picked up 5 copper farthings"), GetReply::Taken);
    assert_eq!(read_get_reply("You picked up a rusty dagger"), GetReply::Taken);
    assert_eq!(read_get_reply("You don't see a rusty dagger here."), GetReply::Gone);
    assert_eq!(read_get_reply("You don't see any copper farthings"), GetReply::Gone);
    assert_eq!(read_get_reply("You took 12 damage."), GetReply::Other, "a blow is not a pickup");
    assert_eq!(read_get_reply("The giant rat swings at you but misses!"), GetReply::Other);
}

fn haul(taken_items: usize, items: usize, taken_coins: usize, coins: usize) -> Haul {
    Haul {
        taken: Vec::new(),
        items_wanted: items,
        items_taken: taken_items,
        coins_wanted: coins,
        coins_taken: taken_coins,
    }
}

#[test]
fn the_haul_counts_what_it_wanted_and_what_it_took() {
    let list = sweep_list(&items(&["a rusty dagger", "an iron helm", "5 copper farthings"]));
    let mut h = Haul::wanted(&list);
    assert_eq!(h.items_wanted, 2);
    assert_eq!(h.coins_wanted, 1);
    h.took(&list[0]);
    h.took(&list[2]);
    assert_eq!(h.items_taken, 1);
    assert_eq!(h.coins_taken, 1);
    assert_eq!(h.taken, vec!["a rusty dagger", "5 copper farthings"]);
    assert_eq!(h.summary(), "1 of 2 items and 1 coin piles");
}

#[test]
fn the_summary_leaves_coins_out_when_none_were_listed() {
    assert_eq!(haul(3, 7, 0, 0).summary(), "3 of 7 items");
    assert_eq!(haul(5, 7, 2, 2).summary(), "5 of 7 items and 2 coin piles");
}

const DEATH_ROOM: RoomId = RoomId { map: 1, room: 2810 };
const HOME: RoomId = RoomId { map: 1, room: 2400 };

fn done(end: &RecoverEnd) -> (String, Option<RoomId>) {
    match end.phase() {
        Phase::Done { why, at } => (why, at),
        other => panic!("not a done phase: {other:?}"),
    }
}

#[test]
fn every_ending_reads_as_the_table_says() {
    let swept = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Swept,
        haul: haul(5, 7, 2, 2),
    };
    assert_eq!(
        done(&swept),
        ("recovered 5 of 7 items and 2 coin piles".to_string(), Some(HOME))
    );

    let broke = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Broke {
            at: DEATH_ROOM,
            name: "Darkwood Forest".into(),
        },
        haul: Haul::default(),
    };
    assert_eq!(
        done(&broke).0,
        "sneak broke at 1/2810 Darkwood Forest, nothing taken"
    );

    let attacked = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Attacked {
            at: DEATH_ROOM,
            name: "Darkwood Forest".into(),
        },
        haul: Haul::default(),
    };
    assert_eq!(done(&attacked).0, "attacked at 1/2810 Darkwood Forest, nothing taken");

    let hurt = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Hurt { mark: 70 },
        haul: haul(3, 7, 0, 0),
    };
    assert_eq!(done(&hurt).0, "hurt under 70%, back with 3 of 7 items");

    let nothing = RecoverEnd::Home {
        at: HOME,
        why: HomeWhy::Nothing,
        haul: Haul::default(),
    };
    assert_eq!(done(&nothing).0, "nothing there");

    let stopped = RecoverEnd::Stopped {
        at: RoomId { map: 1, room: 2812 },
        name: "Darkwood Forest".into(),
        haul: haul(3, 7, 0, 0),
    };
    assert_eq!(
        done(&stopped),
        (
            "stopped at 1/2812 Darkwood Forest with 3 of 7 items".to_string(),
            Some(RoomId { map: 1, room: 2812 })
        )
    );

    let died = RecoverEnd::Died { haul: haul(1, 7, 0, 0) };
    assert_eq!(done(&died), (DIED.to_string(), None));
    assert_eq!(died.haul().items_taken, 1);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test recover`
Expected: a compile error, `unresolved import mud_client::recover`.

- [ ] **Step 3: Write the pure half of the module**

Create `crates/mud-client/src/recover.rs`:

```rust
//! `/recover`: sneak to the room the character died in, search once,
//! pick up everything the search lists, and run back to where the job
//! started. It never attacks, and it turns for home at the first broken
//! sneak.
//!
//! The run home needs no sneak. Aggressive monsters acquire a target
//! inside the combat round and skip a player who moved this round, and
//! pursuit refuses the same player, so a character that keeps moving is
//! neither acquired nor followed. What it needs is no pauses, which is
//! why the walk home is unsneaked: arming costs a round standing still.
//!
//! The pure parts, what the sweep asks for and how an ending reads, sit
//! above the job so they are tested without a board.

use mud_core::content::RoomId;

use crate::farm::Phase;

/// One thing the sweep will ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Take {
    /// The entry as the board listed it, for the report.
    pub label: String,
    /// The `get` that asks for it.
    pub cmd: String,
    pub kind: TakeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeKind {
    Gear,
    Coins,
}

/// The "You notice ... here." entries as `get` commands, gear first and
/// coins last, so a forced exit leaves coins behind rather than
/// equipment.
///
/// A coin pile is taken by denomination, as `get` takes it. Everything
/// else is an item, taken by its printed name with the article dropped:
/// the board matches by word prefix and the printed name is what it
/// printed, so nothing is gained by resolving it first.
pub fn sweep_list(items: &[String]) -> Vec<Take> {
    let mut gear = Vec::new();
    let mut coins = Vec::new();
    for entry in items {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match crate::bot::coin_pile(entry) {
            Some((_, denom)) => coins.push(Take {
                label: entry.to_string(),
                cmd: format!("get {denom}"),
                kind: TakeKind::Coins,
            }),
            None => gear.push(Take {
                label: entry.to_string(),
                cmd: format!("get {}", strip_article(entry)),
                kind: TakeKind::Gear,
            }),
        }
    }
    gear.extend(coins);
    gear
}

fn strip_article(entry: &str) -> &str {
    for article in ["a ", "an ", "the "] {
        if let Some(rest) = entry.strip_prefix(article) {
            return rest.trim();
        }
    }
    entry
}

/// What one line of a `get` reply said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetReply {
    /// The board confirmed the pickup, coins or item.
    Taken,
    /// `You don't see ...`: somebody else got there first.
    Gone,
    Other,
}

pub fn read_get_reply(line: &str) -> GetReply {
    if crate::bot::picked_up(line).is_some() || crate::bot::picked_up_item(line).is_some() {
        return GetReply::Taken;
    }
    if line.trim_start().starts_with("You don't see ") {
        return GetReply::Gone;
    }
    GetReply::Other
}

/// What the sweep asked for and what it got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Haul {
    /// The entries taken, as the board listed them, in the order taken.
    pub taken: Vec<String>,
    pub items_wanted: usize,
    pub items_taken: usize,
    pub coins_wanted: usize,
    pub coins_taken: usize,
}

impl Haul {
    pub fn wanted(list: &[Take]) -> Haul {
        Haul {
            items_wanted: list.iter().filter(|t| t.kind == TakeKind::Gear).count(),
            coins_wanted: list.iter().filter(|t| t.kind == TakeKind::Coins).count(),
            ..Haul::default()
        }
    }

    pub fn took(&mut self, take: &Take) {
        match take.kind {
            TakeKind::Gear => self.items_taken += 1,
            TakeKind::Coins => self.coins_taken += 1,
        }
        self.taken.push(take.label.clone());
    }

    /// `3 of 7 items`, with ` and 2 coin piles` when any coins were
    /// listed.
    pub fn summary(&self) -> String {
        let mut s = format!("{} of {} items", self.items_taken, self.items_wanted);
        if self.coins_wanted > 0 {
            s.push_str(&format!(" and {} coin piles", self.coins_taken));
        }
        s
    }
}

/// Why the character came home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeWhy {
    Swept,
    /// The search listed nothing.
    Nothing,
    /// A hop on the way in arrived without `Sneaking...`.
    Broke { at: RoomId, name: String },
    /// The board refused a move on the way in because something had
    /// the character in combat.
    Attacked { at: RoomId, name: String },
    /// Hitpoints fell under the minor heal mark during the sweep.
    Hurt { mark: u32 },
}

/// How the job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverEnd {
    /// Back in the start room.
    Home { at: RoomId, why: HomeWhy, haul: Haul },
    /// The walk home did not complete. Where the character stands.
    Stopped { at: RoomId, name: String, haul: Haul },
    Died { haul: Haul },
}

impl RecoverEnd {
    pub fn haul(&self) -> &Haul {
        match self {
            RecoverEnd::Home { haul, .. }
            | RecoverEnd::Stopped { haul, .. }
            | RecoverEnd::Died { haul } => haul,
        }
    }

    /// The ending as the bar and the lobby read it.
    pub fn phase(&self) -> Phase {
        match self {
            RecoverEnd::Home { at, why, haul } => Phase::Done {
                why: match why {
                    HomeWhy::Swept => format!("recovered {}", haul.summary()),
                    HomeWhy::Nothing => "nothing there".into(),
                    HomeWhy::Broke { at, name } => {
                        format!("sneak broke at {}/{} {name}, nothing taken", at.map, at.room)
                    }
                    HomeWhy::Attacked { at, name } => {
                        format!("attacked at {}/{} {name}, nothing taken", at.map, at.room)
                    }
                    HomeWhy::Hurt { mark } => {
                        format!("hurt under {mark}%, back with {}", haul.summary())
                    }
                },
                at: Some(*at),
            },
            RecoverEnd::Stopped { at, name, haul } => Phase::Done {
                why: format!(
                    "stopped at {}/{} {name} with {}",
                    at.map,
                    at.room,
                    haul.summary()
                ),
                at: Some(*at),
            },
            RecoverEnd::Died { .. } => Phase::Done {
                why: crate::farm::DIED.into(),
                at: None,
            },
        }
    }
}
```

Add to `crates/mud-client/src/lib.rs`, in alphabetical order after `pub mod purse;` and before `pub mod roam;`:

```rust
pub mod recover;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test recover`
Expected: `test result: ok. 8 passed`

- [ ] **Step 5: Mutation check**

In `sweep_list`, replace `gear.extend(coins); gear` with `coins.extend(gear); coins`. `the_sweep_takes_gear_before_coins_in_listed_order` fails. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/recover.rs crates/mud-client/src/lib.rs crates/mud-client/tests/recover.rs
git commit -m "feat(client): the recover sweep's list, replies, haul and endings"
```

---

### Task 6: Phases for the job, and two small doors opened for it

**Files:**
- Modify: `crates/mud-client/src/farm.rs` (`Phase` at line 387, `label` at line 446, `room` at line 476, `ensure_lit` at line 2930)
- Modify: `crates/mud-client/src/sheet.rs` (`BuffState` at line 675)
- Test: `crates/mud-client/tests/recover.rs`

**Interfaces:**
- Produces: `Phase::Preparing`, `Phase::SneakingIn { to: RoomId }`, `Phase::Sweeping { at: RoomId }`, `Phase::GoingHome { to: RoomId }`. `farm::ensure_lit` is `pub(crate)`. `BuffState::in_flight(&self) -> bool`.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/recover.rs`:

```rust
#[test]
fn the_jobs_phases_have_labels_the_bar_can_show() {
    assert_eq!(Phase::Preparing.label(), "preparing");
    assert_eq!(
        Phase::SneakingIn { to: DEATH_ROOM }.label(),
        "sneaking in to 1/2810"
    );
    assert_eq!(Phase::Sweeping { at: DEATH_ROOM }.label(), "sweeping 1/2810");
    assert_eq!(Phase::GoingHome { to: HOME }.label(), "going home to 1/2400");
    assert_eq!(
        Phase::Sweeping { at: DEATH_ROOM }.room(),
        Some(DEATH_ROOM),
        "standing still in a known room is a position"
    );
    assert_eq!(Phase::SneakingIn { to: DEATH_ROOM }.room(), None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mud-client --test recover`
Expected: a compile error, `no variant named Preparing`.

- [ ] **Step 3: Add the phases and open the doors**

In `crates/mud-client/src/farm.rs`, in `pub enum Phase` after the `Banking { at: RoomId },` variant, add:

```rust
    /// `/recover`: lighting up and buffing in the start room before the
    /// sneak.
    Preparing,
    /// `/recover`: walking to the death room one hop at a time, armed.
    SneakingIn {
        to: RoomId,
    },
    /// `/recover`: searching and picking up in the death room.
    Sweeping {
        at: RoomId,
    },
    /// `/recover`: the unsneaked run back to the start room.
    GoingHome {
        to: RoomId,
    },
```

In `Phase::label`, after the `Phase::Banking { at } =>` arm, add:

```rust
            Phase::Preparing => "preparing".into(),
            Phase::SneakingIn { to } => format!("sneaking in to {}/{}", to.map, to.room),
            Phase::Sweeping { at } => format!("sweeping {}/{}", at.map, at.room),
            Phase::GoingHome { to } => format!("going home to {}/{}", to.map, to.room),
```

In `Phase::room`, add `| Phase::Sweeping { at }` to the first arm so it reads:

```rust
            Phase::Waiting { at }
            | Phase::Fighting { at, .. }
            | Phase::Resting { at }
            | Phase::Sweeping { at }
            | Phase::Placed { at, .. } => Some(*at),
```

`label` and `room` are the only exhaustive matches on `Phase` in the crate. `bank.rs` constructs `Phase::Banking` and matches nothing, and the bar renders through `label`, so nothing else needs an arm.

Change `async fn ensure_lit(` at `src/farm.rs:2930` to `pub(crate) async fn ensure_lit(`.

In `crates/mud-client/src/sheet.rs`, in `impl BuffState`, after `pub fn buffs(&self) -> &[Buff]`, add:

```rust
    /// A cast has gone out and the board has not yet said what became
    /// of it. `attempt` answers `Nothing` while this is true, which is
    /// not the same `Nothing` as "nothing wanted", and a caller casting
    /// before a sneak has to tell the two apart.
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test recover && cargo build -p mud-client --all-targets`
Expected: `test result: ok. 9 passed`, and the build is clean.

- [ ] **Step 5: Mutation check**

Remove `| Phase::Sweeping { at }` from `room`. `the_jobs_phases_have_labels_the_bar_can_show` fails on the `Sweeping` room assertion. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/farm.rs crates/mud-client/src/sheet.rs crates/mud-client/tests/recover.rs
git commit -m "feat(client): phases for a recovery, and the light and buff states open to it"
```

---

### Task 7: The job walks in armed, searches, and runs home

**Files:**
- Modify: `crates/mud-client/src/recover.rs`
- Test: `crates/mud-client/tests/recover_scripted.rs`

**Interfaces:**
- Consumes: `Navigator::new`, `with_capabilities`, `with_stealth`, `goto` at `src/nav.rs:698`, `726`, `883`. `Arrival { at, sneaking, .. }` at `src/nav.rs:285`. `NavError { at, kind, .. }`, `NavErrorKind::Interrupted`, `Interrupt::{Died, Attacked}` at `src/nav.rs:40`, `62`, `107`. `FarmGuard::death_only(username: &str)` at `src/farm.rs:1568`. `farm::look_around`, `farm::next_room_view`, `farm::discover_vitals`, `farm::sheet_from`, `farm::content_for`, `farm::ensure_lit`, `farm::leg_needs_light`, `farm::set_phase`, `farm::is_player_death`, `farm::LOOT_TRIES`. `go::go_config` at `src/go.rs:172`. `lost::place` at `src/lost.rs:325`. `session::drain` at `src/session.rs:1169`. `tui::content_path` at `src/tui.rs:2174`. `Session::profile_changes`, `BotConfig::auto_sneak`, `NavConfig::sneak`, `sheet::stealth_spells` from the earlier plans. `sheet::CastAttempt`, `LightState`, `BuffState`.
- Produces: `recover::refusal(session: &Session, graph: &RoomGraph, here: Option<RoomId>, target: RoomId) -> Result<RoomId, String>`, `recover::run_recover(session: &Session, graph: Arc<RoomGraph>, from: RoomId, target: RoomId, live: watch::Receiver<Profile>, phase: PhaseSink<'_>, notices: &Notices) -> Result<RecoverEnd, FarmError>`. The private `Configs`, `reload`, `cast_buffs`, `sweep` returning `SweepEnd`, `go_home`, `wait_a_prompt`, `room_name`, `vitals_end`.

- [ ] **Step 1: Write the failing tests**

Create `crates/mud-client/tests/recover_scripted.rs`:

```rust
//! `/recover` against a scripted echoing board.
//!
//! The harness is the `go_scripted.rs` shape. Test crates do not share
//! modules, so it is duplicated here, which is this suite's pattern.
//!
//! A corridor of three rooms: Guard Post, the start room and the safe
//! room, Inner Ward, and Keep, where the character died. The board
//! echoes every line, logs it lowercased, and answers from a script
//! consumed strictly in order, each entry once. A re-ask replays the
//! most recently consumed matching entry. Anything unmatched is said
//! aloud.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::bot::BotConfig;
use mud_client::farm::{FarmError, probe_sheet};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::recover::{HomeWhy, RecoverEnd, run_recover};
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};

fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

const START: RoomId = RoomId { map: 1, room: 1 };
const MIDWAY: RoomId = RoomId { map: 1, room: 2 };
const DEATH: RoomId = RoomId { map: 1, room: 3 };

const PROMPT: &str = "[HP=30/MA=0]:";

fn block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n{PROMPT}")
}

/// A block whose floor lists `items`, as the board renders after a bare
/// `search`, with the prompt `prompt` so a test can show hitpoints
/// falling. Used from Task 8 on. The allow keeps this task's build
/// clean and comes off in Task 8.
#[allow(dead_code)]
fn block_with(name: &str, items: &str, exits: &str, prompt: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nYou notice {items} here.\r\nObvious exits: {exits}\r\n{prompt}")
}

fn reply(cmd: &str, body: &str) -> String {
    format!("\r\n{cmd}\r\n{body}\r\n{PROMPT}")
}

/// The stat sheet `probe_sheet` reads. Stealth is what lets the job
/// start at all.
const NINJA_SHEET: &str = "\r\nstat\r\n\
Name: Beef                             Lives/CP:    9/100\r\n\
Race: Dark-Elf    Exp: 0               Perception:     43\r\n\
Class: Ninja      Level: 1             Stealth:        56\r\n\
Hits:    30/30    Armour Class:   0/0  Thievery:        0\r\n\
                                       Traps:          29\r\n\
                                       Picklocks:      31\r\n\
Strength:  40     Agility: 50          Tracking:       26\r\n\
Intellect: 50     Health:  30          Martial Arts:   51\r\n\
Willpower: 30     Charm:   40          MagicRes:       35\r\n\
[HP=30/MA=0]:";

const EMPTY_HANDED: &str =
    "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:";

fn edge(dest: RoomId) -> Option<ExitEdge> {
    Some(ExitEdge {
        dest,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    })
}

/// Guard Post -> Inner Ward -> Keep, north each step, with the way back.
/// `keep_light` is the death room's light, so a test can make it dark.
fn corridor(keep_light: i64) -> Arc<RoomGraph> {
    let mut start = GraphRoom {
        name: "Guard Post".into(),
        ..Default::default()
    };
    start.exits[Direction::North as usize] = edge(MIDWAY);
    let mut midway = GraphRoom {
        name: "Inner Ward".into(),
        ..Default::default()
    };
    midway.exits[Direction::North as usize] = edge(DEATH);
    midway.exits[Direction::South as usize] = edge(START);
    let mut death = GraphRoom {
        name: "Keep".into(),
        light: keep_light,
        ..Default::default()
    };
    death.exits[Direction::South as usize] = edge(MIDWAY);
    Arc::new(RoomGraph::from_rooms(vec![
        (START, start),
        (MIDWAY, midway),
        (DEATH, death),
    ]))
}

async fn scripted_board(
    script: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(block("Guard Post", "north").as_bytes())
            .await
            .unwrap();
        let mut used: Vec<Option<u64>> = vec![None; script.len()];
        let mut clock: u64 = 0;
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim().to_lowercase();
                log.lock().unwrap().push(line.clone());
                let next = used.iter().position(Option::is_none);
                let fresh = next.filter(|&i| script[i].0 == line);
                let reply = match fresh {
                    Some(i) => {
                        clock += 1;
                        used[i] = Some(clock);
                        script[i].1.clone()
                    }
                    None => match script
                        .iter()
                        .enumerate()
                        .filter(|(i, (m, _))| used[*i].is_some() && *m == line)
                        .max_by_key(|(i, _)| used[*i])
                    {
                        Some((_, (_, r))) => r.clone(),
                        None => format!("\r\n{line}\r\nYou say \"{line}\"\r\n{PROMPT}"),
                    },
                };
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        }
    });
    (addr, received)
}

async fn session_for(addr: std::net::SocketAddr) -> Session {
    let profile = Profile {
        target: mud_client::dialect::Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "Beef".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        // Known maxima, so the job never asks `health` and every
        // percent mark is against 30.
        bot: Some(BotConfig {
            max_hp: 30,
            minor_heal_at_percent: 70,
            ..BotConfig::default()
        }),
        farm: None,
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

/// The sheet, the inventory, the opening look and the sneak: how every
/// run starts.
fn opening() -> Vec<(&'static str, String)> {
    vec![
        ("stat", NINJA_SHEET.into()),
        ("inventory", EMPTY_HANDED.into()),
        ("look", format!("\r\nlook{}", block("Guard Post", "north"))),
        ("sneak", reply("sneak", "Attempting to sneak...")),
    ]
}

fn sneaky_step(name: &str, exits: &str) -> (&'static str, String) {
    ("n", format!("\r\nn\r\nSneaking...{}", block(name, exits)))
}

fn home_steps() -> Vec<(&'static str, String)> {
    vec![
        ("s", format!("\r\ns{}", block("Inner Ward", "north south"))),
        ("s", format!("\r\ns{}", block("Guard Post", "north"))),
    ]
}

async fn recover_over(
    graph: Arc<RoomGraph>,
    script: Vec<(&'static str, String)>,
) -> (Result<RecoverEnd, FarmError>, Vec<String>) {
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    probe_sheet(&session, None).await;
    let live = session.profile_changes();
    let out = tokio::time::timeout(
        Duration::from_secs(40),
        run_recover(&session, graph, START, DEATH, live, None, &quiet()),
    )
    .await
    .expect("run_recover should finish, not hang");
    let log = received.lock().unwrap().clone();
    (out, log)
}

fn count(log: &[String], line: &str) -> usize {
    log.iter().filter(|l| *l == line).count()
}

// ------------------------------------------------------------ refusals

/// Nothing is sent before the refusal: the character cannot sneak, so
/// there is no job to run.
#[tokio::test]
async fn stealth_zero_is_refused_before_anything_is_sent() {
    let (addr, received) = scripted_board(vec![]).await;
    let session = session_for(addr).await;
    let live = session.profile_changes();
    let out = run_recover(&session, corridor(0), START, DEATH, live, None, &quiet()).await;
    let why = match out {
        Err(FarmError::Config(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert!(why.contains("Stealth is 0"), "{why}");
    assert!(received.lock().unwrap().is_empty(), "nothing may be sent");
}

// ---------------------------------------------------------- the walk in

/// The first hop lands without `Sneaking...`. The sneak broke, and the
/// job turns for home from Inner Ward without ever searching.
#[tokio::test]
async fn a_broken_sneak_turns_for_home_without_searching() {
    let mut script = opening();
    script.push(("n", format!("\r\nn{}", block("Inner Ward", "north south"))));
    script.push(("s", format!("\r\ns{}", block("Guard Post", "north"))));
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must survive a break");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Broke {
                at: MIDWAY,
                name: "Inner Ward".into()
            },
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "search"), 0, "no search after a break: {log:?}");
    assert_eq!(count(&log, "sneak"), 1, "the walk home does not arm: {log:?}");
}

/// The floor is bare. One search, then home.
#[tokio::test]
async fn an_empty_search_goes_home() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push(("search", reply("search", "Your search revealed nothing.")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Nothing,
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "search"), 1, "exactly one search: {log:?}");
    let sneak_at = log.iter().position(|l| l == "sneak").unwrap();
    let first_move = log.iter().position(|l| l == "n").unwrap();
    assert!(sneak_at < first_move, "the sneak is armed before the first move: {log:?}");
}

/// The board refuses the search because something already has the
/// character. Home at once.
#[tokio::test]
async fn a_search_refused_for_combat_goes_home() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push(("search", reply("search", "You may not search while attacking!")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Attacked {
                at: DEATH,
                name: "Keep".into()
            },
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert!(!log.iter().any(|l| l.starts_with("a ")), "never an attack: {log:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test recover_scripted`
Expected: a compile error, `unresolved import mud_client::recover::run_recover`.

- [ ] **Step 3: Write the job**

Append to `crates/mud-client/src/recover.rs`. The `use` lines go at the top of the file with the existing ones.

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::bot::BotConfig;
use crate::events::Event;
use crate::farm::{FarmConfig, FarmError, Notices, PhaseSink, set_phase};
use crate::graph::RoomGraph;
use crate::nav::{Interrupt, NavErrorKind, Navigator};
use crate::profile::Profile;
use crate::session::Session;
```

Then the job:

```rust
/// How long the search may take to answer. A room block behind a
/// paced board, and nothing longer.
const SEARCH_WAIT: Duration = Duration::from_secs(15);
/// How long one `get` may take to answer before the attempt is spent.
/// An encumbrance refusal is silent, so silence is the only sign.
const GET_WAIT: Duration = Duration::from_secs(3);
/// Moves the walk home retries when the board refuses one for combat.
/// Each retry waits out a prompt, and the round passes with it.
const COMBAT_RETRIES: u32 = 12;

/// Why the job will not start. Checked before anything is sent, by the
/// window before it spawns the job and by the job itself, so neither
/// can forget.
pub fn refusal(
    session: &Session,
    graph: &RoomGraph,
    here: Option<RoomId>,
    target: RoomId,
) -> Result<RoomId, String> {
    let from = here.ok_or(
        "nobody knows with any confidence where you are standing; /where first, then recover",
    )?;
    if session.capabilities().stealth == 0 {
        return Err("cannot sneak: the sheet says Stealth is 0".into());
    }
    let sneaks = session.profile().bot.as_ref().is_none_or(|b| b.sneak);
    if !sneaks {
        return Err("bot.auto_sneak is off, and a recovery is a sneak: /set bot.auto_sneak true".into());
    }
    if graph.room(target).is_none() {
        return Err(format!("no such room {}/{}", target.map, target.room));
    }
    if graph.route(from, target).is_none() {
        return Err(format!(
            "no route from {}/{} to {}/{}",
            from.map, from.room, target.map, target.room
        ));
    }
    Ok(from)
}

/// Everything the job derives from the profile. Rebuilt whole on a live
/// change, with the job's own forced fields reapplied to whatever
/// profile arrives, so no setting can talk the job into a fight.
struct Configs {
    bot: BotConfig,
    cfg: FarmConfig,
    /// The walk in: arms a sneak and casts the stealth buff for it.
    nav: Navigator,
    /// The walk home: never arms, because arming costs a round standing
    /// still and the run home lives on not stopping.
    runner: Navigator,
}

impl Configs {
    fn derive(
        profile: &Profile,
        graph: &Arc<RoomGraph>,
        session: &Session,
        content: Option<&Arc<mud_core::content::Content>>,
        durations: &std::collections::BTreeMap<String, u32>,
        clock: &crate::world::RoundClock,
        vitals: Option<crate::farm::Vitals>,
    ) -> Configs {
        let mut bot = profile.bot.clone().unwrap_or_default();
        bot.auto_combat = false;
        bot.auto_flee = false;
        bot.auto_get = false;
        if let Some(v) = vitals
            && bot.max_hp == 0
        {
            bot.max_hp = v.max_hp;
            bot.max_mana = v.max_mana;
        }
        let base = profile.farm.clone().unwrap_or_else(|| FarmConfig {
            content: crate::tui::content_path(profile),
            ..Default::default()
        });
        let mut cfg = crate::go::go_config(&base, false);
        cfg.interrupt_at_percent = 0;
        cfg.nav.sneak = bot.auto_sneak;
        let caps = session.capabilities();
        let mut nav = Navigator::new(graph.clone(), cfg.nav.clone()).with_capabilities(caps.clone());
        if let Some(content) = content {
            let (_, book, casting) = session.raw_sheet();
            let (stealth, _) = crate::sheet::stealth_spells(&book, &content.spells, durations, casting);
            nav = nav.with_stealth(stealth, clock.clone());
        }
        let runner = Navigator::new(
            graph.clone(),
            crate::nav::NavConfig {
                sneak: false,
                ..cfg.nav.clone()
            },
        )
        .with_capabilities(caps);
        Configs {
            bot,
            cfg,
            nav,
            runner,
        }
    }
}

/// Everything `derive` needs that does not change for the job's life,
/// so a live reload can rebuild the configs from the new profile alone.
struct Fixed {
    graph: Arc<RoomGraph>,
    content: Option<Arc<mud_core::content::Content>>,
    durations: std::collections::BTreeMap<String, u32>,
    clock: crate::world::RoundClock,
    vitals: Option<crate::farm::Vitals>,
}

/// Pick up a settings change, if one arrived. One line on the window
/// per change, not per key.
fn reload(
    configs: &mut Configs,
    live: &mut watch::Receiver<Profile>,
    session: &Session,
    fixed: &Fixed,
    notices: &Notices,
) {
    if !live.has_changed().unwrap_or(false) {
        return;
    }
    let profile = live.borrow_and_update().clone();
    *configs = Configs::derive(
        &profile,
        &fixed.graph,
        session,
        fixed.content.as_ref(),
        &fixed.durations,
        &fixed.clock,
        fixed.vitals,
    );
    notices("-- recover: settings reloaded --");
}

fn room_name(graph: &RoomGraph, id: RoomId) -> String {
    graph.room(id).map(|r| r.name.clone()).unwrap_or_default()
}

/// Sneak to `target` from `from`, search once, take what is listed, and
/// run back to `from`. `from` is where the character stands when the
/// job starts, and it is also the safe room.
///
/// `live` is the profile as the window keeps it. A change is picked up
/// at the next hop, the next pickup, or the next wait in the sweep.
#[allow(clippy::too_many_arguments)]
pub async fn run_recover(
    session: &Session,
    graph: Arc<RoomGraph>,
    from: RoomId,
    target: RoomId,
    mut live: watch::Receiver<Profile>,
    phase: PhaseSink<'_>,
    notices: &Notices,
) -> Result<RecoverEnd, FarmError> {
    refusal(session, &graph, Some(from), target).map_err(FarmError::Config)?;
    // Both fight switches off for the job's life. The guard reads this
    // one live, and the job's own bot config has combat off, so no code
    // path in the runner can swing.
    let fought = session.travel_fights().get();
    session.travel_fights().set(false);
    let out = recover(session, graph, from, target, &mut live, phase, notices).await;
    session.travel_fights().set(fought);
    out
}

async fn recover(
    session: &Session,
    graph: Arc<RoomGraph>,
    from: RoomId,
    target: RoomId,
    live: &mut watch::Receiver<Profile>,
    phase: PhaseSink<'_>,
    notices: &Notices,
) -> Result<RecoverEnd, FarmError> {
    set_phase(phase, Phase::Preparing);
    let name = session.character_name().unwrap_or_default();
    let profile = live.borrow_and_update().clone();
    let content_path = crate::tui::content_path(&profile);
    let content = crate::farm::content_for(
        session,
        &FarmConfig {
            content: content_path.clone(),
            ..Default::default()
        },
        notices,
    );
    let durations = RoomGraph::load_spell_durations(&content_path).unwrap_or_default();
    let mut fixed = Fixed {
        graph: graph.clone(),
        content,
        durations,
        clock: crate::world::RoundClock::new(),
        vitals: None,
    };
    let mut configs = Configs::derive(
        &profile,
        &graph,
        session,
        fixed.content.as_ref(),
        &fixed.durations,
        &fixed.clock,
        None,
    );

    // Where the character really stands. The caller's room is a hint the
    // locator checks against the board's own block.
    let seen = crate::farm::look_around(session, "the recovery's opening look").await?;
    let home = crate::lost::place(session, &graph, &configs.nav, from, &seen)
        .await
        .map_err(FarmError::Lost)?
        .at;

    // Every percent mark divides by the maximum, and 0 means nobody said.
    if configs.bot.max_hp == 0
        && let Some(v) = crate::farm::discover_vitals(session).await
    {
        fixed.vitals = Some(v);
        configs.bot.max_hp = v.max_hp;
        configs.bot.max_mana = v.max_mana;
    }

    // Light and buffs, standing still, before the sneak. Either breaks
    // one, which is why both come first.
    let sheet = crate::farm::sheet_from(session, &configs.bot, &fixed.durations);
    let mut light = crate::sheet::LightState::new(sheet.light);
    let mut buff = crate::sheet::BuffState::new(sheet.buffs.0);
    for why in sheet.buffs.1 {
        notices(&why);
    }
    if graph.dark(target) || crate::farm::leg_needs_light(&graph, home, target) {
        crate::farm::ensure_lit(session, &mut light, &fixed.clock).await;
    }
    cast_buffs(session, &mut buff, &fixed.clock).await;

    // The route, as rooms, so every hop is its own walk and every
    // arrival's sneak state is read. One walk to the target would hide
    // every break before the last.
    let route = graph
        .route(home, target)
        .ok_or_else(|| FarmError::Config("no route to the target".into()))?;
    let mut hops = Vec::with_capacity(route.len());
    let mut at = home;
    for dir in route {
        let next = graph
            .room(at)
            .and_then(|r| r.exits[dir as usize].as_ref())
            .map(|e| e.dest)
            .ok_or_else(|| FarmError::Config("the route left the graph".into()))?;
        hops.push(next);
        at = next;
    }

    set_phase(phase, Phase::SneakingIn { to: target });
    let haul = Haul::default();
    let mut here = home;
    let mut sneaking = false;
    let mut guard = crate::farm::FarmGuard::death_only(&name);
    for next in hops {
        reload(&mut configs, live, session, &fixed, notices);
        match configs.nav.goto(session, here, next, &mut guard, sneaking).await {
            Ok(arrived) => {
                here = arrived.at;
                sneaking = arrived.sneaking;
                if here != next || !sneaking {
                    let why = HomeWhy::Broke {
                        at: here,
                        name: room_name(&graph, here),
                    };
                    return go_home(session, &graph, &mut configs, live, &fixed, phase, notices, here, home, why, haul, &name).await;
                }
            }
            Err(e) => {
                here = e.at;
                return match e.kind {
                    NavErrorKind::Interrupted(Interrupt::Died) => Ok(RecoverEnd::Died { haul }),
                    NavErrorKind::Interrupted(Interrupt::Attacked { .. }) => {
                        let why = HomeWhy::Attacked {
                            at: here,
                            name: room_name(&graph, here),
                        };
                        go_home(session, &graph, &mut configs, live, &fixed, phase, notices, here, home, why, haul, &name).await
                    }
                    _ => Err(FarmError::Nav(e)),
                };
            }
        }
    }

    set_phase(phase, Phase::Sweeping { at: here });
    let mut haul = haul;
    let why = match sweep(session, &mut configs, live, &fixed, &mut haul, &name, notices).await? {
        SweepEnd::Died => return Ok(RecoverEnd::Died { haul }),
        SweepEnd::Swept => HomeWhy::Swept,
        SweepEnd::Nothing => HomeWhy::Nothing,
        SweepEnd::Hurt { mark } => HomeWhy::Hurt { mark },
        SweepEnd::Blocked => HomeWhy::Attacked {
            at: here,
            name: room_name(&graph, here),
        },
    };
    go_home(session, &graph, &mut configs, live, &fixed, phase, notices, here, home, why, haul, &name).await
}

/// Cast every lapsed buff, standing still, before the sneak. Bounded
/// by a deadline like `ensure_lit`. A fizzle is not retried: a buff is
/// a bonus, and the sneak goes without it.
async fn cast_buffs(
    session: &Session,
    buff: &mut crate::sheet::BuffState,
    clock: &crate::world::RoundClock,
) {
    if buff.is_empty() {
        return;
    }
    let mut events = session.events();
    crate::session::drain(&mut events, |cor| buff.on_event(cor, Instant::now()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        match buff.attempt(Instant::now(), clock) {
            crate::sheet::CastAttempt::Send(cmd) => {
                let id = session.send(&cmd);
                buff.on_sent(&cmd, id);
            }
            crate::sheet::CastAttempt::Hold(_) => {}
            crate::sheet::CastAttempt::Nothing if !buff.in_flight() => return,
            crate::sheet::CastAttempt::Nothing => {}
        }
        match tokio::time::timeout(Duration::from_millis(300), events.recv()).await {
            Ok(Ok(cor)) => buff.on_event(&cor, Instant::now()),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(_)) => return,
            Err(_) => {}
        }
    }
}

/// How the sweep ended.
enum SweepEnd {
    Swept,
    Nothing,
    Hurt { mark: u32 },
    /// The board refused the search for combat.
    Blocked,
    Died,
}

/// A death or the hitpoint mark, read off any event during the sweep.
fn vitals_end(ev: &Event, bot: &BotConfig, name: &str) -> Option<SweepEnd> {
    match ev {
        Event::Line(line) if crate::farm::is_player_death(line, name) => Some(SweepEnd::Died),
        Event::Prompt { hp, .. } if *hp <= 0 => Some(SweepEnd::Died),
        Event::Prompt { hp, .. }
            if bot.max_hp > 0 && *hp * 100 / bot.max_hp < bot.minor_heal_at_percent as i32 =>
        {
            Some(SweepEnd::Hurt {
                mark: bot.minor_heal_at_percent,
            })
        }
        _ => None,
    }
}

/// One bare `search`, then a `get` per listed entry. This task lands the
/// search. Task 8 lands the pickups where the marker comment sits.
async fn sweep(
    session: &Session,
    configs: &mut Configs,
    live: &mut watch::Receiver<Profile>,
    fixed: &Fixed,
    haul: &mut Haul,
    name: &str,
    notices: &Notices,
) -> Result<SweepEnd, FarmError> {
    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});
    let ask = session.send("search");
    let deadline = tokio::time::Instant::now() + SEARCH_WAIT;
    let items: Vec<String> = loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(cor)) => {
                if let Some(end) = vitals_end(&cor.event, &configs.bot, name) {
                    return Ok(end);
                }
                if cor.answers != Some(ask) {
                    continue;
                }
                match &cor.event {
                    Event::RoomSeen(room) => break room.items.clone(),
                    Event::Line(l) if l.to_lowercase().contains("search revealed nothing") => {
                        return Ok(SweepEnd::Nothing);
                    }
                    Event::Line(l) if l.to_lowercase().contains("may not search while attacking") => {
                        return Ok(SweepEnd::Blocked);
                    }
                    _ => continue,
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return Err(FarmError::Disconnected),
            Err(_) => {
                return Err(FarmError::NoRoomBlock {
                    whose: "the recovery's search".into(),
                });
            }
        }
    };
    let list = sweep_list(&items);
    if list.is_empty() {
        return Ok(SweepEnd::Nothing);
    }
    *haul = Haul::wanted(&list);
    let _ = (configs, live, fixed, notices);
    // Task 8: the pickups.
    Ok(SweepEnd::Swept)
}

/// Wait for the next prompt, so a move refused for combat is retried
/// after the round rather than at once. Bounded, because a board that
/// prints no prompt must not wedge the walk home.
async fn wait_a_prompt(session: &Session) {
    let mut events = session.events();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(cor)) if matches!(cor.event, Event::Prompt { .. }) => return,
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) | Err(_) => return,
        }
    }
}

/// The unsneaked run back. Only a death stops it: blows do not, and a
/// move the board refuses for combat is sent again once the round has
/// passed.
#[allow(clippy::too_many_arguments)]
async fn go_home(
    session: &Session,
    graph: &RoomGraph,
    configs: &mut Configs,
    live: &mut watch::Receiver<Profile>,
    fixed: &Fixed,
    phase: PhaseSink<'_>,
    notices: &Notices,
    here: RoomId,
    home: RoomId,
    why: HomeWhy,
    haul: Haul,
    name: &str,
) -> Result<RecoverEnd, FarmError> {
    set_phase(phase, Phase::GoingHome { to: home });
    let mut here = here;
    let mut guard = crate::farm::FarmGuard::death_only(name);
    let mut refused = 0u32;
    loop {
        reload(configs, live, session, fixed, notices);
        match configs.runner.goto(session, here, home, &mut guard, false).await {
            Ok(arrived) => {
                return Ok(RecoverEnd::Home {
                    at: arrived.at,
                    why,
                    haul,
                });
            }
            Err(e) => {
                here = e.at;
                match e.kind {
                    NavErrorKind::Interrupted(Interrupt::Died) => {
                        return Ok(RecoverEnd::Died { haul });
                    }
                    NavErrorKind::Interrupted(Interrupt::Attacked { .. })
                        if refused < COMBAT_RETRIES =>
                    {
                        refused += 1;
                        wait_a_prompt(session).await;
                    }
                    _ => {
                        return Ok(RecoverEnd::Stopped {
                            at: here,
                            name: room_name(graph, here),
                            haul,
                        });
                    }
                }
            }
        }
    }
}
```

`Capabilities` derives `Clone` at `src/graph.rs:262`, which is what lets one reading serve both navigators.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test recover_scripted`
Expected: `test result: ok. 4 passed`. If `a_broken_sneak_turns_for_home_without_searching` sees a second `sneak` in the log, the runner navigator is arming: check that `NavConfig { sneak: false, .. }` reached it and that `arm_sneak` returns early on `sneak: false`, which the settings plan added.

- [ ] **Step 5: Mutation check**

In `recover`, change `if here != next || !sneaking {` to `if here != next {`. `a_broken_sneak_turns_for_home_without_searching` fails because the job searches. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/recover.rs crates/mud-client/tests/recover_scripted.rs
git commit -m "feat(client): the recover job sneaks in hop by hop, searches once, and runs home"
```

---

### Task 8: The pickups

**Files:**
- Modify: `crates/mud-client/src/recover.rs` (`sweep`, at the Task 8 marker)
- Test: `crates/mud-client/tests/recover_scripted.rs`

**Interfaces:**
- Consumes: `sweep_list`, `read_get_reply`, `Haul` from Task 5. `crate::farm::LOOT_TRIES` at `src/farm.rs:638`.
- Produces: the sweep sends one `get` per entry, retires it on the board's word, drops it on `You don't see`, spends three silent attempts before dropping it, reads the prompt after every reply, and picks up a live settings change while it waits.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/recover_scripted.rs`:

```rust
// ------------------------------------------------------------ the sweep

fn up_to_the_search(items: &str, prompt: &str) -> Vec<(&'static str, String)> {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push(("search", format!("\r\nsearch{}", block_with("Keep", items, "south", prompt))));
    script
}

/// Gear, then coins, then home, and the ending names the counts.
#[tokio::test]
async fn the_happy_path_takes_gear_then_coins_and_reports_the_haul() {
    let mut script = up_to_the_search("a rusty dagger, 5 copper farthings", PROMPT);
    script.push(("get rusty dagger", reply("get rusty dagger", "You took a rusty dagger.")));
    script.push(("get copper", reply("get copper", "You picked up 5 copper farthings")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    let RecoverEnd::Home { at, why, haul } = end else {
        panic!("expected home, got {end:?}, log {log:?}");
    };
    assert_eq!(at, START);
    assert_eq!(why, HomeWhy::Swept);
    assert_eq!(haul.taken, vec!["a rusty dagger", "5 copper farthings"]);
    assert_eq!(haul.summary(), "1 of 1 items and 1 coin piles");
    let dagger = log.iter().position(|l| l == "get rusty dagger").unwrap();
    let copper = log.iter().position(|l| l == "get copper").unwrap();
    assert!(dagger < copper, "gear before coins: {log:?}");
    assert_eq!(count(&log, "search"), 1);
}

/// Somebody else got the dagger. Dropped without a retry, and the cap
/// is still taken.
#[tokio::test]
async fn a_dont_see_drops_the_entry_without_a_retry() {
    let mut script = up_to_the_search("a rusty dagger, a leather cap", PROMPT);
    script.push((
        "get rusty dagger",
        reply("get rusty dagger", "You don't see a rusty dagger here."),
    ));
    script.push(("get leather cap", reply("get leather cap", "You took a leather cap.")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(end.haul().summary(), "1 of 2 items", "log: {log:?}");
    assert_eq!(count(&log, "get rusty dagger"), 1, "no retry: {log:?}");
}

/// The board says nothing to the `get`, three times. That is what an
/// item too heavy to lift looks like, and the entry is dropped after
/// the third silence.
#[tokio::test]
async fn three_silent_attempts_drop_the_entry() {
    let mut script = up_to_the_search("an anvil", PROMPT);
    script.push(("get anvil", format!("\r\nget anvil\r\n{PROMPT}")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(end.haul().summary(), "0 of 1 items", "log: {log:?}");
    assert_eq!(count(&log, "get anvil"), 3, "three attempts: {log:?}");
    assert_eq!(count(&log, "s"), 2, "and then home: {log:?}");
}

/// The prompt after the first pickup shows 15 of 30, under the 70 mark.
/// The sweep ends there with what it has, and the cap stays on the
/// floor.
#[tokio::test]
async fn hitpoints_under_the_mark_end_the_sweep_with_a_partial_haul() {
    let mut script = up_to_the_search("a rusty dagger, a leather cap", PROMPT);
    script.push((
        "get rusty dagger",
        "\r\nget rusty dagger\r\nYou took a rusty dagger.\r\n[HP=15/MA=0]:".to_string(),
    ));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    let RecoverEnd::Home { why, haul, .. } = end else {
        panic!("expected home, got {end:?}, log {log:?}");
    };
    assert_eq!(why, HomeWhy::Hurt { mark: 70 });
    assert_eq!(haul.summary(), "1 of 2 items");
    assert_eq!(count(&log, "get leather cap"), 0, "the sweep stopped: {log:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test recover_scripted`
Expected: the four new tests fail. `the_happy_path_takes_gear_then_coins_and_reports_the_haul` fails on `haul.taken`, which is empty.

- [ ] **Step 3: Write the pickups**

Remove the `#[allow(dead_code)]` and its two comment lines from `block_with` in `tests/recover_scripted.rs`, since this task uses it.

In `crates/mud-client/src/recover.rs`, in `sweep`, replace these three lines:

```rust
    let _ = (configs, live, fixed, notices);
    // Task 8: the pickups.
    Ok(SweepEnd::Swept)
```

with:

```rust
    for take in &list {
        let mut tries = 0u32;
        'entry: while tries < crate::farm::LOOT_TRIES {
            tries += 1;
            reload(configs, live, session, fixed, notices);
            let sent = session.send(&take.cmd);
            let deadline = tokio::time::Instant::now() + GET_WAIT;
            loop {
                tokio::select! {
                    changed = live.changed() => {
                        if changed.is_ok() {
                            reload(configs, live, session, fixed, notices);
                        }
                    }
                    ev = tokio::time::timeout_at(deadline, events.recv()) => match ev {
                        Ok(Ok(cor)) => {
                            if let Some(end) = vitals_end(&cor.event, &configs.bot, name) {
                                return Ok(end);
                            }
                            if cor.answers != Some(sent) {
                                continue;
                            }
                            if let Event::Line(line) = &cor.event {
                                match read_get_reply(line) {
                                    GetReply::Taken => {
                                        haul.took(take);
                                        break 'entry;
                                    }
                                    GetReply::Gone => break 'entry,
                                    GetReply::Other => {}
                                }
                            }
                        }
                        Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                        Ok(Err(_)) => return Err(FarmError::Disconnected),
                        // Silence: the attempt is spent.
                        Err(_) => break,
                    },
                }
            }
        }
        // The prompt that followed the reply may already say the
        // character is too hurt to stand here for the next one.
        let mut end = None;
        crate::session::drain(&mut events, |cor| {
            if end.is_none() {
                end = vitals_end(&cor.event, &configs.bot, name);
            }
        });
        if let Some(end) = end {
            return Ok(end);
        }
    }
    Ok(SweepEnd::Swept)
```

The drain closure borrows `configs.bot` while `end` is written: if the borrow checker objects, copy `let bot = configs.bot.clone();` before the drain and read `&bot` inside it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test recover_scripted`
Expected: `test result: ok. 8 passed`. `three_silent_attempts_drop_the_entry` takes about nine seconds.

- [ ] **Step 5: Mutation check**

Change `GetReply::Gone => break 'entry,` to `GetReply::Gone => {}`. `a_dont_see_drops_the_entry_without_a_retry` fails with three `get rusty dagger` lines. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/recover.rs crates/mud-client/tests/recover_scripted.rs
git commit -m "feat(client): the recovery sweep takes gear first, drops what is gone, and stops when hurt"
```

---

### Task 9: A swing mid sweep, and a dark death room

**Files:**
- Test: `crates/mud-client/tests/recover_scripted.rs`

**Interfaces:**
- Consumes: everything Tasks 7 and 8 produced. `sheet::LIGHT_ITEMS` recognises `torch` at `src/sheet.rs:22`, and `Inventory::parse` reads `You are carrying a torch.` at `src/sheet.rs:149`.

No new code is expected. These pin two promises the earlier tasks make: the job never attacks whatever swings at it, and it lights up in the start room before the sneak.

- [ ] **Step 1: Write the tests**

Append to `crates/mud-client/tests/recover_scripted.rs`:

```rust
// ------------------------------------------------- swings and darkness

/// A rat swings during the sweep. Above the mark the sweep carries on,
/// and nothing the job sends is an attack.
#[tokio::test]
async fn a_swing_mid_sweep_is_ignored_and_never_answered() {
    let mut script = up_to_the_search("a rusty dagger, a leather cap", PROMPT);
    script.push((
        "get rusty dagger",
        reply(
            "get rusty dagger",
            "The giant rat swings at you but misses!\r\nYou took a rusty dagger.",
        ),
    ));
    script.push(("get leather cap", reply("get leather cap", "You took a leather cap.")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(end.haul().summary(), "2 of 2 items", "log: {log:?}");
    assert!(!log.iter().any(|l| l.starts_with("a ")), "never an attack: {log:?}");
    assert_eq!(count(&log, "look"), 1, "no defence look either: {log:?}");
}

/// The death room is dark and the character carries a torch. The torch
/// is lit in the start room, before the sneak, because lighting breaks
/// one.
#[tokio::test]
async fn a_dark_death_room_is_lit_for_before_the_sneak() {
    let script = vec![
        ("stat", NINJA_SHEET.into()),
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying a torch.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .to_string(),
        ),
        ("look", format!("\r\nlook{}", block("Guard Post", "north"))),
        ("light torch", reply("light torch", "You lit the torch.")),
        ("sneak", reply("sneak", "Attempting to sneak...")),
        sneaky_step("Inner Ward", "north south"),
        sneaky_step("Keep", "south"),
        ("search", reply("search", "Your search revealed nothing.")),
        ("s", format!("\r\ns{}", block("Inner Ward", "north south"))),
        ("s", format!("\r\ns{}", block("Guard Post", "north"))),
    ];
    let (out, log) = recover_over(corridor(-150), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(end.haul().summary(), "0 of 0 items", "log: {log:?}");
    let lit = log.iter().position(|l| l == "light torch").expect("the torch is lit");
    let armed = log.iter().position(|l| l == "sneak").expect("the sneak is armed");
    assert!(lit < armed, "light before sneak: {log:?}");
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p mud-client --test recover_scripted`
Expected: `test result: ok. 10 passed`. If `a_dark_death_room_is_lit_for_before_the_sneak` never sends `light torch`, check that `sheet_from` read the torch: `Inventory::parse` splits the carried list on commas after dropping the final period, and `light_items` matches the last word of each entry against `LIGHT_ITEMS`.

- [ ] **Step 3: Mutation check**

In `recover`, change `if graph.dark(target) || crate::farm::leg_needs_light(&graph, home, target) {` to `if false {`. `a_dark_death_room_is_lit_for_before_the_sneak` fails at `expect("the torch is lit")`. Revert.

- [ ] **Step 4: Commit**

```bash
git add crates/mud-client/tests/recover_scripted.rs
git commit -m "test(client): a recovery ignores a swing and lights up before it sneaks"
```

---

### Task 10: `/recover [room]`

**Files:**
- Modify: `crates/mud-client/src/tui.rs` (`VERBS` at line 904, `KeyOutcome` at line 913, the lobby arm at line 225, `slash` at line 1010, `help_text` at line 1097, after `start_go` at line 2050)
- Modify: `crates/mud-client/src/window.rs` (after the `KeyOutcome::Go { target }` arm at line 925)
- Test: `crates/mud-client/tests/tui.rs`

**Interfaces:**
- Consumes: `recover::refusal`, `recover::run_recover`, `RecoverEnd::phase`, `RecoverEnd::haul` from Tasks 5 and 7. `deathlog::last` from Task 1. `go::resolve(graph, from, typed) -> Result<RoomId, GoRefusal>` and `GoRefusal::lines` at `src/go.rs:110` and `63`. `needs_name` at `src/tui.rs:1227`. `Job` at `src/tui.rs:1795`. `Session::profile_changes`.
- Produces: `KeyOutcome::Recover { target: Option<String> }`, `tui::start_recover(session: Arc<Session>, graph: Arc<RoomGraph>, here: Option<RoomId>, target: RoomId, notices: Notices) -> Result<Job, String>`, `/recover` in `VERBS` and `help_text`.

- [ ] **Step 1: Write the failing tests**

In `crates/mud-client/tests/tui.rs`, append beside the other `slash` tests:

```rust
#[test]
fn recover_parses_with_and_without_a_room() {
    assert_eq!(slash("/recover"), Some(KeyOutcome::Recover { target: None }));
    assert_eq!(
        slash("/recover 1/2810"),
        Some(KeyOutcome::Recover {
            target: Some("1/2810".into())
        })
    );
    assert_eq!(
        slash("/recover Darkwood Forest"),
        Some(KeyOutcome::Recover {
            target: Some("Darkwood Forest".into())
        })
    );
}
```

In `every_job_start_refuses_without_a_name` at `tests/tui.rs:1377`, add `start_recover` to the `use` line and this entry to the `refusals` vector, after the `start_bank` entry:

```rust
        start_recover(session.clone(), graph.clone(), Some(here), here, quiet()).err(),
```

Then add a second refusal test after it:

```rust
/// With a name but no stealth, the job is refused before it spawns, so
/// the operator reads the reason on the spot rather than as a failed
/// phase.
#[tokio::test]
async fn recover_refuses_a_character_that_cannot_sneak() {
    use mud_client::graph::{GraphRoom, RoomGraph};
    use mud_client::tui::start_recover;
    use std::sync::Arc;

    let addr = banner_board().await;
    let profile = Profile {
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "dan".into(),
        ..Default::default()
    };
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());
    let here = mud_core::content::RoomId { map: 1, room: 1 };
    let graph = Arc::new(RoomGraph::from_rooms(vec![(
        here,
        GraphRoom {
            name: "Home".into(),
            ..Default::default()
        },
    )]));
    let why = start_recover(session, graph, Some(here), here, quiet()).unwrap_err();
    assert!(why.contains("Stealth is 0"), "{why}");
}
```

`every_verb_in_the_completion_list_is_claimed_and_in_help` at `tests/tui.rs:414` needs no change. It fails on its own the moment `/recover` joins `VERBS` without a `slash` arm or a help line.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test tui recover`
Expected: a compile error, `no variant named Recover`.

- [ ] **Step 3: The verb, the outcome, the help line, and the starter**

In `crates/mud-client/src/tui.rs`:

Add `"/recover"` to `VERBS` after `"/bank"`:

```rust
pub const VERBS: &[&str] = &[
    "/quit", "/farm", "/loop", "/bot", "/go", "/bank", "/recover", "/where", "/room", "/map", "/help",
    "/set", "/unset", "/save", "/load", "/connect", "/disconnect", "/new", "/close", "/windows",
];
```

In `enum KeyOutcome`, after the `Bank,` variant, add:

```rust
    /// Sneak to a room, search it once, take everything listed, and run
    /// back here. `None` means the room of the character's last logged
    /// death. Unparsed for the same reason `Go` is.
    Recover {
        target: Option<String>,
    },
```

In `lobby_step`, add `| KeyOutcome::Recover { .. }` to the group that answers `-- not connected: /connect host[:port] --`, beside `KeyOutcome::Bank`.

In `slash`, after the `"/bank"` arm, add:

```rust
        "/recover" => Some(KeyOutcome::Recover {
            target: (!rest.is_empty()).then(|| rest.to_string()),
        }),
```

In `help_text`, after the `/bank` line, add:

```text
/recover [room]      sneak to where you died, search once, take everything, run back here
```

After `start_go` and before `start_bank`, add:

```rust
/// `/recover`. The same job shape as `start_go`, ending in a
/// `Phase::Done` that says what was taken and where the character is.
///
/// Refused outright when the character has no name, for the reason
/// `start_go` gives, and when the job's own `refusal` finds a reason:
/// no confirmed position, no stealth, `bot.auto_sneak` off, or no route. The
/// refusal is read here, synchronously, so the operator sees it on the
/// spot rather than as a failed phase a moment later.
pub fn start_recover(
    session: Arc<Session>,
    graph: Arc<crate::graph::RoomGraph>,
    here: Option<mud_core::content::RoomId>,
    target: mud_core::content::RoomId,
    notices: crate::farm::Notices,
) -> Result<Job, String> {
    needs_name(&session)?;
    let from = crate::recover::refusal(&session, &graph, here, target)?;
    // Automation goes back under flood control, exactly as a go does.
    session.set_pace(session.profile().pace());
    let live = session.profile_changes();
    let (tx, rx) = tokio::sync::watch::channel(crate::farm::Phase::default());
    let handle = tokio::spawn(async move {
        let end = match crate::recover::run_recover(
            &session,
            graph,
            from,
            target,
            live,
            Some(&tx),
            &notices,
        )
        .await
        {
            Ok(end) => {
                // What came back, one per line, so it can be checked
                // against what was lost.
                for item in &end.haul().taken {
                    notices(&format!("recovered {item}"));
                }
                end.phase()
            }
            Err(e) => crate::farm::Phase::Failed { why: e.to_string() },
        };
        let _ = tx.send(end);
    });
    Ok(Job {
        handle,
        phase: rx,
        what: "recover",
    })
}
```

- [ ] **Step 4: The window arm**

In `crates/mud-client/src/window.rs`, after the `KeyOutcome::Go { target } => { ... }` arm and before `KeyOutcome::Bank =>`, add:

```rust
                            KeyOutcome::Recover { target } => {
                                if let Some(j) = job.as_ref() {
                                    w.note(&format!("-- {} already running (Ctrl-F to take over) --", j.what));
                                } else {
                                    match graph.as_ref() {
                                        None => w.note(&format!(
                                            "-- recover: no room database at {} --",
                                            world_path.display()
                                        )),
                                        Some(g) => {
                                            // A named room, or the last logged death.
                                            let to = match target {
                                                Some(typed) => crate::go::resolve(g, here.confirmed(), &typed)
                                                    .map_err(|r| r.lines().join("\n")),
                                                None => {
                                                    let name = session.character_name().unwrap_or_default();
                                                    crate::deathlog::last(&name)
                                                        .and_then(|d| d.room)
                                                        .ok_or_else(|| "recover: no logged death with a room. /recover <room>".to_string())
                                                }
                                            };
                                            match to {
                                                Err(why) => w.note(&format!("-- {why} --")),
                                                Ok(to) => {
                                                    let name = g.room(to).map(|r| r.name.clone()).unwrap_or_default();
                                                    match start_recover(
                                                        session.clone(),
                                                        g.clone(),
                                                        here.confirmed(),
                                                        to,
                                                        notices.clone(),
                                                    ) {
                                                        Err(e) => w.note(&format!("-- recover: {e} --")),
                                                        Ok(started) => {
                                                            exp.reset();
                                                            exp_since = std::time::Instant::now();
                                                            w.note(&format!(
                                                                "-- recovering from {name} [{}/{}] (Ctrl-F to take over) --",
                                                                to.map, to.room
                                                            ));
                                                            phase_rx = Some(started.phase.clone());
                                                            job = Some(started);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
```

`start_recover` is reached through whatever `use` brings `start_go` into `window.rs`. Add it beside `start_go` there.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test tui`
Expected: all pass, including `every_verb_in_the_completion_list_is_claimed_and_in_help`, `every_job_start_refuses_without_a_name` and `recover_refuses_a_character_that_cannot_sneak`.

- [ ] **Step 6: Mutation check**

In `start_recover`, delete the line `let from = crate::recover::refusal(&session, &graph, here, target)?;` and replace it with `let from = here.unwrap_or(target);`. `recover_refuses_a_character_that_cannot_sneak` fails because a job starts. Revert.

- [ ] **Step 7: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/src/window.rs crates/mud-client/tests/tui.rs
git commit -m "feat(client): /recover starts the recovery from the window, defaulting to the last logged death"
```

---

### Task 11: `R` on the map

**Files:**
- Modify: `crates/mud-client/src/mapview.rs` (`ViewAction` at line 40, the `g` arm at line 383, the legend at line 635)
- Modify: `crates/mud-client/src/window.rs` (after the `ViewAction::Go(to)` arm at line 1180)
- Test: `crates/mud-client/tests/mapview.rs`

**Interfaces:**
- Consumes: `MapView::on_key`, `MapView::cursor_room`, the `view` and `press` helpers in `tests/mapview.rs`. `start_recover` from Task 10.
- Produces: `ViewAction::Recover(RoomId)` on `R` with a room under the cursor. The legend reads `... g go  R recover  q leave`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/mapview.rs`:

```rust
use mud_client::mapview::ViewAction;

#[test]
fn shift_r_on_a_room_asks_to_recover_from_it() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('h'));
    assert_eq!(v.cursor_room(), Some(ROAD));
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
    assert_eq!(action, ViewAction::Recover(ROAD));
}

#[test]
fn shift_r_on_nothing_says_so_and_stays() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('k'));
    assert_eq!(v.cursor_room(), None, "the cell north of the door is empty");
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
    assert_eq!(action, ViewAction::Continue);
    assert_eq!(v.message(), Some("no room under the cursor"));
}

#[test]
fn the_legend_names_the_recover_key() {
    let v = view(DOOR, Fix::Confirmed(DOOR));
    let status = v.lines().pop().unwrap();
    assert!(status.contains("g go  R recover  q leave"), "{status}");
}
```

`ViewAction` must derive `PartialEq` and `Debug` for the assertions. It already carries `Debug, Clone, PartialEq, Eq` at `src/mapview.rs:39`. `MapView::message` is public at `src/mapview.rs:178`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test mapview`
Expected: a compile error, `no variant named Recover`.

- [ ] **Step 3: The key, the action, the legend, and the window arm**

In `crates/mud-client/src/mapview.rs`, in `enum ViewAction` after `Go(RoomId),` add:

```rust
    /// Leave the map and recover from this room: sneak there, search
    /// once, take everything, run back to where the character stands.
    Recover(RoomId),
```

In `on_key`, after the `KeyCode::Char('g') => { ... }` arm, add:

```rust
            KeyCode::Char('R') => {
                return match self.cursor_room() {
                    Some(id) => ViewAction::Recover(id),
                    None => {
                        self.message = Some("no room under the cursor".into());
                        ViewAction::Continue
                    }
                };
            }
```

This arm sits below the shifted panning arms, which claim only `H`, `J`, `K` and `L`, so a shifted `R` reaches it.

In `status`, change the legend text `r roams  g go  q leave` to `r roams  g go  R recover  q leave`.

In `crates/mud-client/src/window.rs`, after the `crate::mapview::ViewAction::Go(to) => { ... }` arm, add:

```rust
                                                        crate::mapview::ViewAction::Recover(to) if job.is_some() => {
                                                            w.note("-- something is already driving (Ctrl-F to take over) --");
                                                            let _ = to;
                                                        }
                                                        crate::mapview::ViewAction::Recover(to) => {
                                                            let name = g.room(to).map(|r| r.name.clone()).unwrap_or_default();
                                                            match start_recover(
                                                                session.clone(),
                                                                g.clone(),
                                                                here.confirmed(),
                                                                to,
                                                                notices.clone(),
                                                            ) {
                                                                Err(e) => w.note(&format!("-- recover: {e} --")),
                                                                Ok(started) => {
                                                                    w.note(&format!(
                                                                        "-- recovering from {name} [{}/{}] (Ctrl-F to take over) --",
                                                                        to.map, to.room
                                                                    ));
                                                                    phase_rx = Some(started.phase.clone());
                                                                    job = Some(started);
                                                                }
                                                            }
                                                        }
```

`mmc map` offline, in `bin/mmc.rs`, matches on the action `run_offline` returns. Find that match with `grep -n "ViewAction::Go" crates/mud-client/src/bin/mmc.rs` and give `Recover` the same treatment `Go` gets there, which is a note that there is no connection to walk with.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test mapview && cargo build -p mud-client --all-targets`
Expected: all pass and the build is clean.

- [ ] **Step 5: Mutation check**

Change the new arm's `Some(id) => ViewAction::Recover(id),` to `Some(id) => ViewAction::Go(id),`. `shift_r_on_a_room_asks_to_recover_from_it` fails. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/mapview.rs crates/mud-client/src/window.rs crates/mud-client/src/bin/mmc.rs crates/mud-client/tests/mapview.rs
git commit -m "feat(client): R on the map recovers from the room under the cursor"
```

---

### Task 12: The docs

**Files:**
- Modify: `docs/mud-client.md` (the command table near the top, the `/go` and `/bank` sections around line 430, the Live settings section)

**Interfaces:**
- Consumes: nothing new. This task writes prose only.

- [ ] **Step 1: The command row**

In the slash command table of `docs/mud-client.md`, after the `/bank` row, add:

```markdown
| `/recover [room]` | Sneak to a room, search it once, take everything listed, and run back here. Without a room, the room of the last logged death. |
```

- [ ] **Step 2: The section**

After the `/bank` section, add:

```markdown
## Recovering gear

A full death drops everything the character carries into the room it died
in. `/recover` goes and gets it back. The character stands in a safe room,
the job sneaks to the death room one hop at a time, searches once, picks up
everything the search lists, and runs back to the safe room. It never
attacks. On the map, `R` with the cursor on a room does the same.

The job refuses to start, and says why, when another job is running, when
the character's position is not confirmed, when there is no route, when the
stat sheet says Stealth is 0, or when `bot.auto_sneak` is off.

Before the sneak it lights up if the route or the death room is dark, and
casts the buffs in `bot.buffs`. Both break a sneak, so both come first. The
stealth spell the sheet found is cast by the navigator as part of arming.

On the way in, a hop that arrives without the board's `Sneaking...` line is
a break, and the job turns for home from that room without searching. A
move the board refuses for combat is treated the same way.

In the death room the job sends one bare `search`. The floor is listed as
`You notice ... here.`, and every entry is taken with one `get` each, gear
first and coins last, including denominations in `bot.ignore_coins`. An
entry the board says it does not see is dropped. An entry the board says
nothing about is tried three times and dropped, since there is no wording
for an item too heavy to lift. After every reply the job reads the prompt,
and hitpoints under `bot.minor_heal_at_percent` end the sweep at once.

The run home is unsneaked and does not stop for blows. Aggressive monsters
acquire a target inside the combat round and skip a player who moved this
round, so a character that keeps moving is neither acquired nor followed.

The job ends the way a go does. The bar shows the reason, the window adopts
the room, and the items taken are printed one per line:

| ending | phase text |
| --- | --- |
| swept and home | `recovered 5 of 7 items and 2 coin piles` |
| break on the way in | `sneak broke at 1/2810 Darkwood Forest, nothing taken` |
| refused a move for combat | `attacked at 1/2810 Darkwood Forest, nothing taken` |
| hurt mid sweep | `hurt under 70%, back with 3 of 7 items` |
| empty search | `nothing there` |
| walk home did not complete | `stopped at 1/2812 Darkwood Forest with 3 of 7 items` |
| died | `died` |

The bare search's item line is unverified on the live board. The first live
run settles it.

### The death log

Every death writes one line to `~/.config/mmc/deaths.log`, beside the
profiles, from hand play, under a job, and under the map alike. `mmc farm`
writes it too. Append only. A line reads:

    2026-09-07T14:42:07Z beef 1/2810 Darkwood Forest confirmed

The last word is `confirmed`, `stale` or `unknown`, and says how sure the
client was of the room. A death with no known room writes `- -` for the id
and the name, so the death itself is not lost. `/recover` with no argument
takes this character's newest line that carries a room.
```

- [ ] **Step 3: The live settings table**

In the Live settings section's "When a change takes effect" lists, which the settings plan rewrote, add `/recover` wherever `/go` is named as a job whose target is fixed at start. The recover target and start room are fixed at start. Everything else the job reads is live.

- [ ] **Step 4: Check the prose**

Run: `grep -n "—\|(" docs/mud-client.md | sed -n '/Recovering gear/,$p' | head`
Expected: no em dashes and no parentheses in the added text. Fix any that appear.

- [ ] **Step 5: Run everything once**

Run: `cargo test -p mud-client && cargo clippy -p mud-client --all-targets`
Expected: every test passes and clippy prints no warning for the touched files.

- [ ] **Step 6: Commit**

```bash
git add docs/mud-client.md
git commit -m "doc(client): /recover and the death log"
```

