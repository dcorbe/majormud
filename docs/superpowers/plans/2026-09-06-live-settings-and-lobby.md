# Live Settings and Lobby Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `/set`, `/unset`, `/save` and `/load` edit the profile live with tab completion, and `mmc play` starts in a lobby where `/connect host[:port]` opens the connection without a profile file.

**Architecture:** A new `settings.rs` owns the profile as a `toml_edit` document, re-parsed into a `Profile` after every edit so validation stays in one place and a `/save` keeps the file's comments. The key handler becomes a pure function returning what to send, so the lobby and play share it. `tui::run` loops between a lobby and the existing play loop, which now returns whether the user quit or the line closed.

**Tech Stack:** Rust 2024 edition, tokio, serde, toml 0.8 and toml_edit 0.22, crossterm, the existing fake-board test harness in `tests/tui.rs`.

**Spec:** `docs/superpowers/specs/2026-09-06-live-settings-and-lobby-design.md`

## Global Constraints

- Never run `rustfmt` or `cargo fmt`.
- Every file ends with a blank line.
- No em dashes, parentheses or semicolons in prose you write: doc comments, commit messages, the doc. Code punctuation is code.
- Commit messages carry a tag with the crate in brackets as the repo does: `feat(client): ...`, `refactor(client): ...`, `test(client): ...`, `doc(client): ...`. No attribution lines. Do not mention the plan or the spec in commit messages.
- No test may read `re/`. Fixtures are hand-built. A test that needs a file puts it under `env!("CARGO_TARGET_TMPDIR")`, which cargo sets for integration tests. Never `/tmp`.
- Each task's test must fail before the implementation and pass after. Each task also runs the named mutation check: change the rule, watch the test fail, revert.
- Run the crate's tests with `cargo test -p mud-client` from the repo root. Build with `cargo build -p mud-client`. Pass `--test <file>` while iterating and run the whole crate once before each commit. `cargo clippy -p mud-client --all-targets` must add no warnings.
- Working directory for every command below is the repo root `/home/daniel/majormud/majormud`.
- The value syntax is TOML. A bare word is a string. That rule lives in `Settings::set` and nowhere else.

---

## File Structure

- `crates/mud-client/Cargo.toml` (modify): add `toml_edit = "0.22"`.
- `crates/mud-client/src/lib.rs` (modify): `pub mod settings;`.
- `crates/mud-client/src/profile.rs` (modify): `Default` for `Profile`, `#[serde(default)]`, `Profile::require_host`, `Profile::load` calls through `Settings`. `RENAMED_KEYS` moves to `settings.rs`.
- `crates/mud-client/src/dialect.rs` (modify): `#[derive(Default)]` on `Target` with `MbbsEmu` as the default.
- `crates/mud-client/src/settings.rs` (create): `Settings`, `KEYS`, `glob_match`, `Completion`, `complete`, `common_prefix`.
- `crates/mud-client/src/session.rs` (modify): the profile behind a lock, `profile()` returns a clone, `set_profile`, `close`, `Cmd::Close`.
- `crates/mud-client/src/tui.rs` (modify): `InputEditor::replace`, `VERBS`, `KeyOutcome` gains `Send`, `Raw`, `Note`, `QuitNow`, `SetList`, `Set`, `Unset`, `Save`, `Load`, `Connect`, `Disconnect`. `handle_key` loses its session parameter. `apply_settings`, `Applied`, `assist_config_for`, `needs_username`, `connect_target`, `lobby_step`, `LobbyStep`, `ContentCache`, `World`, `PlayEnd`, `run`, `lobby`. `play` takes settings, the cache, the key channel and stdout and returns `PlayEnd`.
- `crates/mud-client/src/mapview.rs`, `src/farm.rs`, `src/dialect.rs`, `src/script.rs`, `src/go.rs`, `src/bank.rs` (modify): call sites of `Session::profile`.
- `crates/mud-client/src/cli.rs` (modify): `Play.profile` is optional.
- `crates/mud-client/src/bin/mmc.rs` (modify): `play_command` builds `Settings` and calls `tui::run`. `run_command` and `farm_command` call `require_host`.
- `docs/mud-client.md` (modify): a Live settings section, the lobby, the `mmc play` synopsis.
- Tests: `tests/profile.rs`, `tests/settings.rs` (create), `tests/tui.rs`, `tests/session_capabilities.rs` or a new `tests/session_close.rs`, `tests/cli.rs`.

---

### Task 1: A profile with nothing in it

**Files:**
- Modify: `crates/mud-client/src/profile.rs`
- Modify: `crates/mud-client/src/dialect.rs:19-25`
- Modify: `crates/mud-client/src/bin/mmc.rs` (`run_command`, `farm_command`)
- Test: `crates/mud-client/tests/profile.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `impl Default for Profile`: target `MbbsEmu`, host empty, port 23, username and password empty, `pace_ms` `None`, `disable_evil_warnings` false, `bot` and `farm` `None`, `bank` default.
  - `#[serde(default)]` on `Profile`, so an empty document parses.
  - `impl Profile { pub fn require_host(&self) -> Result<(), String> }`.
  - `Target: Default`, `MbbsEmu`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/profile.rs`:

```rust
/// The lobby starts from nothing and fills the profile in with `/set`,
/// so an empty document has to be a profile.
#[test]
fn an_empty_document_is_the_default_profile() {
    let p: Profile = toml::from_str("").unwrap();
    assert_eq!(p, Profile::default());
    assert_eq!(p.target, Target::MbbsEmu);
    assert_eq!(p.port, 23);
    assert!(p.host.is_empty());
    assert!(p.username.is_empty());
    assert!(p.bot.is_none());
    assert!(p.farm.is_none());
}

/// A headless command has no lobby to wait in, so it refuses a profile
/// that names no host instead of dialling nowhere.
#[test]
fn a_profile_without_a_host_is_refused_by_require_host() {
    let empty = Profile::default();
    assert!(empty.require_host().is_err());
    let mut named = Profile::default();
    named.host = "127.0.0.1".into();
    assert!(named.require_host().is_ok());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test profile empty_document require_host`
Expected: compile error, `Profile` has no `Default` and no `require_host`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/dialect.rs`, change the `Target` derive:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Target {
    #[default]
    #[serde(rename = "mbbs")]
    MbbsEmu,
    #[serde(rename = "rust")]
    RustServer,
}
```

In `crates/mud-client/src/profile.rs`, add `#[serde(default)]` under the derive on `Profile`, and after the struct:

```rust
impl Default for Profile {
    /// What the lobby starts from: a telnet port and nothing else. The
    /// host is empty on purpose, so nothing dials until `/connect` or
    /// `/set host` names one.
    fn default() -> Self {
        Profile {
            target: Target::default(),
            host: String::new(),
            port: 23,
            username: String::new(),
            password: String::new(),
            pace_ms: None,
            disable_evil_warnings: false,
            bot: None,
            farm: None,
            bank: crate::bank::BankConfig::default(),
        }
    }
}
```

Add to `impl Profile`:

```rust
    /// A headless command has no lobby to wait in. A profile that names
    /// no host is refused before anything dials.
    pub fn require_host(&self) -> Result<(), String> {
        if self.host.is_empty() {
            return Err("no host set".into());
        }
        Ok(())
    }
```

In `crates/mud-client/src/bin/mmc.rs`, in both `run_command` and `farm_command`, directly after the `Profile::load` match:

```rust
    if let Err(e) = profile.require_host() {
        eprintln!("profile {}: {e}", profile_path.display());
        return ExitCode::FAILURE;
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test profile`
Expected: all pass, including the two new ones.

Mutation check: change the default port to 0, the first test fails on `port`. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/profile.rs crates/mud-client/src/dialect.rs crates/mud-client/src/bin/mmc.rs crates/mud-client/tests/profile.rs
git commit -m "feat(client): an empty document is a profile, and headless runs need a host"
```

---

### Task 2: Settings, the profile as a document

**Files:**
- Modify: `crates/mud-client/Cargo.toml`
- Modify: `crates/mud-client/src/lib.rs`
- Create: `crates/mud-client/src/settings.rs`
- Test: `crates/mud-client/tests/settings.rs` (create)

**Interfaces:**
- Consumes: `Profile`, `BotConfig::normalise`, `BotConfig::validate`, `BankConfig::validate`.
- Produces:
  - `pub const KEYS: &[&str]`, every settable key in file order.
  - `pub struct Settings`, `Default`.
  - `Settings::parse(text: &str) -> Result<Settings, String>`.
  - `Settings::set(&mut self, key: &str, text: &str) -> Result<(), String>`.
  - `Settings::unset(&mut self, key: &str) -> Result<(), String>`.
  - `Settings::profile(&self) -> &Profile`, `dirty(&self) -> bool`, `text(&self) -> String`.

- [ ] **Step 1: Add the dependency**

In `crates/mud-client/Cargo.toml` under `[dependencies]`, after `toml = "0.8"`:

```toml
toml_edit = "0.22"
```

In `crates/mud-client/src/lib.rs`, after `pub mod session;`:

```rust
pub mod settings;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/mud-client/tests/settings.rs`:

```rust
//! Live settings: the profile as an editable document.

use mud_client::settings::{KEYS, Settings};

const COMMENTED: &str = r#"# my character
target = "mbbs"
host = "127.0.0.1"   # the local emulator
port = 2327
username = "dan"
password = "pw"

[bot]
auto_combat = true   # fight back
rest_at_percent = 60
"#;

#[test]
fn a_scalar_lands_in_the_profile_and_the_text() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    assert!(!s.dirty());
    s.set("bot.rest_at_percent", "45").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 45);
    assert!(s.text().contains("rest_at_percent = 45"));
    assert!(s.dirty());
}

#[test]
fn a_list_is_a_toml_array() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bot.ignore_coins", r#"["copper", "silver"]"#).unwrap();
    assert_eq!(
        s.profile().bot.as_ref().unwrap().ignore_coins,
        vec!["copper".to_string(), "silver".to_string()]
    );
}

#[test]
fn a_bare_word_is_a_string_and_so_is_a_phrase() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bot.rest_command", "rest").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_command, "rest");
    s.set("bot.rest_command", "sit down").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_command, "sit down");
    assert!(s.text().contains(r#"rest_command = "sit down""#));
}

#[test]
fn a_key_in_an_absent_table_creates_the_table() {
    let mut s = Settings::default();
    s.set("bank.keep_gold", "5").unwrap();
    assert_eq!(s.profile().bank.keep_gold, 5);
    s.set("bot.auto_get", "true").unwrap();
    assert!(s.profile().bot.as_ref().unwrap().auto_get);
    assert!(s.text().contains("[bot]"));
}

#[test]
fn a_nested_table_key_is_reachable() {
    let mut s = Settings::default();
    s.set("farm.nav.step_timeout_ms", "9000").unwrap();
    assert_eq!(s.profile().farm.as_ref().unwrap().nav.step_timeout_ms, 9000);
    assert!(s.text().contains("[farm.nav]"));
}

#[test]
fn a_wrong_type_is_refused_and_nothing_changes() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    let before = s.text();
    let err = s.set("bot.rest_at_percent", "soon").unwrap_err();
    assert!(err.contains("rest_at_percent"), "{err}");
    assert_eq!(s.text(), before);
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 60);
    assert!(!s.dirty());
}

#[test]
fn an_unknown_key_is_refused_before_the_text_changes() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    let before = s.text();
    let err = s.set("bot.ignore_currency", "[]").unwrap_err();
    assert!(err.contains("unknown key bot.ignore_currency"), "{err}");
    assert_eq!(s.text(), before);
}

#[test]
fn a_value_the_validator_rejects_is_refused() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    let err = s.set("bot.ignore_coins", r#"["pennies"]"#).unwrap_err();
    assert!(err.contains("pennies"), "{err}");
    assert!(s.profile().bot.as_ref().unwrap().ignore_coins.is_empty());
}

#[test]
fn unset_returns_a_key_to_its_default() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bank.at", r#""1/297""#).unwrap();
    assert_eq!(s.profile().bank.at.as_deref(), Some("1/297"));
    s.unset("bank.at").unwrap();
    assert_eq!(s.profile().bank.at, None);
    assert!(!s.text().contains("at ="));
    s.unset("bot.rest_at_percent").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 60);
}

#[test]
fn setting_the_new_name_drops_the_old_alias() {
    let mut s = Settings::parse(
        r#"
[bot]
heal_at_percent = 50
"#,
    )
    .unwrap();
    s.set("bot.rest_at_percent", "40").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 40);
    assert!(!s.text().contains("heal_at_percent"));
}

/// The key list is hand-written. This pins it to the structs: a field
/// added to any config table without a `KEYS` entry fails here.
#[test]
fn every_key_the_profile_serialises_is_in_keys() {
    use mud_client::bank::BankConfig;
    use mud_client::bot::BotConfig;
    use mud_client::dialect::Target;
    use mud_client::farm::FarmConfig;
    use mud_client::profile::Profile;

    let full = Profile {
        target: Target::MbbsEmu,
        host: "h".into(),
        port: 1,
        username: "u".into(),
        password: "p".into(),
        pace_ms: Some(1),
        disable_evil_warnings: true,
        bot: Some(BotConfig {
            heal_spells: vec!["x".into()],
            ..Default::default()
        }),
        farm: Some(FarmConfig {
            finish_at: Some("1/1".into()),
            depart_at_percent: Some(1),
            ..Default::default()
        }),
        bank: BankConfig {
            at: Some("1/1".into()),
            ..Default::default()
        },
    };
    let text = toml::to_string(&full).unwrap();
    let table: toml::Table = text.parse().unwrap();
    let mut found = Vec::new();
    flatten(&table, "", &mut found);
    found.sort();
    let mut want: Vec<String> = KEYS.iter().map(|k| k.to_string()).collect();
    want.sort();
    assert_eq!(found, want);
}

fn flatten(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::Table(t) => flatten(t, &key, out),
            _ => out.push(key),
        }
    }
}
```

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test -p mud-client --test settings`
Expected: compile error, no `mud_client::settings`.

- [ ] **Step 4: Implement**

Create `crates/mud-client/src/settings.rs`:

```rust
//! Live settings: the profile as an editable TOML document.
//!
//! The document is the source of truth. `/set` writes into it, then the
//! whole document is re-parsed into a [`Profile`] and validated, so there
//! is one parser, one validator and no per-key setter. A `/save` writes
//! the document text, which is why the file's comments and key order
//! survive.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

use crate::profile::Profile;

/// Every key `/set` accepts, in file order. The listing walks it in this
/// order and completion offers it. Hand-written because Rust has no way
/// to enumerate struct fields, and pinned to the structs by the test
/// `every_key_the_profile_serialises_is_in_keys`.
pub const KEYS: &[&str] = &[
    "target",
    "host",
    "port",
    "username",
    "password",
    "pace_ms",
    "disable_evil_warnings",
    "bot.auto_combat",
    "bot.auto_heal",
    "bot.auto_get",
    "bot.take_keys",
    "bot.ignore_coins",
    "bot.auto_flee",
    "bot.minor_heal_at_percent",
    "bot.major_heal_at_percent",
    "bot.rest_at_percent",
    "bot.mana_rest_at_percent",
    "bot.rest_until_percent",
    "bot.meditate",
    "bot.flee_at_percent",
    "bot.rest_command",
    "bot.minor_heal_spell",
    "bot.major_heal_spell",
    "bot.hp_regen_spell",
    "bot.heal_spells",
    "bot.buffs",
    "bot.ignore",
    "bot.max_hp",
    "bot.max_mana",
    "bot.combat_idle_prompts",
    "bot.assist_play",
    "farm.content",
    "farm.start",
    "farm.circuit",
    "farm.finish_at",
    "farm.loops",
    "farm.max_seconds",
    "farm.stop_seconds",
    "farm.dwell_empty_seconds",
    "farm.depart_at_percent",
    "farm.slowdown_backoff_ms",
    "farm.idle_poke_ms",
    "farm.heal_retry_prompts",
    "farm.heal_refused",
    "farm.fight_while_travelling",
    "farm.interrupt_at_percent",
    "farm.travel_interrupts",
    "farm.max_rest_seconds",
    "farm.defend_seconds",
    "farm.nav.step_timeout_ms",
    "farm.nav.bash_doors",
    "farm.nav.search_hidden",
    "bank.auto_deposit",
    "bank.deposit_at_coins",
    "bank.deposit_on_weight_class",
    "bank.keep_gold",
    "bank.at",
];

/// Keys that were renamed, and what they are called now. Deserialisation
/// accepts both, so this exists to SAY SO: a profile that keeps working
/// while its vocabulary has moved on is a profile whose owner never finds
/// out about the new knob next to it. A warning, never a refusal.
const RENAMED_KEYS: [(&str, &str); 4] = [
    ("heal_at_percent", "rest_at_percent"),
    ("heal_command", "rest_command"),
    ("spell_at_percent", "minor_heal_at_percent"),
    ("heal_spells", "minor_heal_spell and major_heal_spell"),
];

/// The serde aliases: old spelling, current spelling. Setting the current
/// spelling removes the old one from the same table, because serde
/// rejects a table that carries both as a duplicate field.
const ALIASES: [(&str, &str); 3] = [
    ("heal_at_percent", "rest_at_percent"),
    ("heal_command", "rest_command"),
    ("spell_at_percent", "minor_heal_at_percent"),
];

pub struct Settings {
    doc: DocumentMut,
    profile: Profile,
    /// Where the text came from, or where it was last saved.
    path: Option<PathBuf>,
    /// Edited since the last load or save.
    dirty: bool,
}

impl Default for Settings {
    /// An empty document, which is the default profile.
    fn default() -> Self {
        Settings {
            doc: DocumentMut::new(),
            profile: Profile::default(),
            path: None,
            dirty: false,
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, String> {
        let doc: DocumentMut = text
            .parse()
            .map_err(|e: toml_edit::TomlError| e.to_string())?;
        let profile = profile_of(&doc)?;
        Ok(Settings {
            doc,
            profile,
            path: None,
            dirty: false,
        })
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The document as it would be saved.
    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Write one key. `text` is parsed as a TOML value. When that fails
    /// the whole of it is a string, so `rest` and `sit down` both work
    /// without quotes. Any failure, a bad type or a validator refusal,
    /// leaves the document as it was.
    pub fn set(&mut self, key: &str, text: &str) -> Result<(), String> {
        if !KEYS.contains(&key) {
            return Err(format!("unknown key {key}"));
        }
        let value: toml_edit::Value = match text.parse() {
            Ok(v) => v,
            Err(_) => toml_edit::Value::from(text),
        };
        let before = self.doc.clone();
        let (path, leaf) = split_key(key);
        let table = table_at(&mut self.doc, &path)?;
        table.insert(leaf, Item::Value(value));
        drop_aliases(table, leaf);
        self.reparse(before)
    }

    /// Remove one key so its default applies again. Removing a key that
    /// is not there is fine.
    pub fn unset(&mut self, key: &str) -> Result<(), String> {
        if !KEYS.contains(&key) {
            return Err(format!("unknown key {key}"));
        }
        let before = self.doc.clone();
        let (path, leaf) = split_key(key);
        if let Some(table) = existing_table(&mut self.doc, &path) {
            table.remove(leaf);
            drop_aliases(table, leaf);
        }
        self.reparse(before)
    }

    fn reparse(&mut self, before: DocumentMut) -> Result<(), String> {
        match profile_of(&self.doc) {
            Ok(profile) => {
                self.profile = profile;
                self.dirty = true;
                Ok(())
            }
            Err(e) => {
                self.doc = before;
                Err(e)
            }
        }
    }
}

/// The one parse and the one validation, shared by every edit and every
/// load. `Profile::load` used to hold this.
fn profile_of(doc: &DocumentMut) -> Result<Profile, String> {
    let mut profile: Profile =
        toml::from_str(&doc.to_string()).map_err(|e| e.message().to_string())?;
    if let Some(bot) = &mut profile.bot {
        bot.normalise();
        bot.validate()?;
    }
    profile.bank.validate()?;
    Ok(profile)
}

/// `bot.rest_at_percent` is the table path `["bot"]` and the leaf
/// `rest_at_percent`. A bare key has an empty path.
fn split_key(key: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let leaf = parts.pop().unwrap_or(key);
    (parts, leaf)
}

/// The table at `path`, created on the way down. A created table is
/// implicit, so `[farm]` gets no header of its own when only
/// `[farm.nav]` has keys.
fn table_at<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Result<&'a mut Table, String> {
    let mut table = doc.as_table_mut();
    for seg in path {
        let item = table.entry(seg).or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
        table = item
            .as_table_mut()
            .ok_or_else(|| format!("{seg} is not a table in this profile"))?;
    }
    Ok(table)
}

fn existing_table<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Table> {
    let mut table = doc.as_table_mut();
    for seg in path {
        table = table.get_mut(seg)?.as_table_mut()?;
    }
    Some(table)
}

fn drop_aliases(table: &mut Table, leaf: &str) {
    for (old, new) in ALIASES {
        if new == leaf {
            table.remove(old);
        }
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p mud-client --test settings`
Expected: all pass. If `every_key_the_profile_serialises_is_in_keys` fails, the assertion prints both lists. A key in `found` and not in `want` is a field the plan missed: add it to `KEYS` in its struct's file order. A key in `want` and not in `found` is a typo in `KEYS`.

Mutation check: in `set`, delete the `drop_aliases` call. `setting_the_new_name_drops_the_old_alias` fails. Revert. Then in `reparse`, delete `self.doc = before;`. `a_wrong_type_is_refused_and_nothing_changes` fails. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/Cargo.toml Cargo.lock crates/mud-client/src/lib.rs crates/mud-client/src/settings.rs crates/mud-client/tests/settings.rs
git commit -m "feat(client): settings edit the profile as a document"
```

---

### Task 3: Load, save, and the renamed-key warnings

**Files:**
- Modify: `crates/mud-client/src/settings.rs`
- Modify: `crates/mud-client/src/profile.rs:52-87`
- Test: `crates/mud-client/tests/settings.rs`

**Interfaces:**
- Consumes: `Settings::parse`.
- Produces:
  - `Settings::load(path: &Path) -> Result<Settings, String>`, remembers the path.
  - `Settings::save(&mut self, path: Option<&Path>) -> Result<PathBuf, String>`, writes, remembers the path, clears dirty.
  - `Settings::warnings(&self) -> Vec<String>`, one line per renamed key present.
  - `Profile::load` is now `Settings::load` plus printing the warnings.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/settings.rs`:

```rust
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("settings");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn save_keeps_comments_and_order() {
    let path = scratch("commented.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    let mut s = Settings::load(&path).unwrap();
    assert_eq!(s.path(), Some(path.as_path()));
    s.set("bot.rest_at_percent", "45").unwrap();
    let saved = s.save(None).unwrap();
    assert_eq!(saved, path);
    assert!(!s.dirty());
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        COMMENTED.replace("rest_at_percent = 60", "rest_at_percent = 45"),
        "every comment and every line stays where it was"
    );
}

#[test]
fn save_without_a_path_needs_one_once() {
    let mut s = Settings::default();
    s.set("host", "\"127.0.0.1\"").unwrap();
    let err = s.save(None).unwrap_err();
    assert!(err.contains("/save <file>"), "{err}");
    let path = scratch("fresh.toml");
    s.save(Some(&path)).unwrap();
    assert_eq!(s.path(), Some(path.as_path()));
    s.set("port", "2327").unwrap();
    s.save(None).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("port = 2327"));
}

#[test]
fn load_reports_renamed_keys_and_still_parses() {
    let s = Settings::parse(
        r#"
[bot]
heal_at_percent = 50
heal_command = "rest"
"#,
    )
    .unwrap();
    let warnings = s.warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("`heal_at_percent` is now `rest_at_percent`"));
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 50);
}

#[test]
fn a_missing_file_is_an_error_not_a_default() {
    assert!(Settings::load(&scratch("nowhere.toml")).is_err());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test settings save load`
Expected: compile error, no `load`, `save`, `warnings`.

- [ ] **Step 3: Implement**

Add to `impl Settings` in `settings.rs`:

```rust
    /// Read a profile file. The path is remembered for `save`.
    pub fn load(path: &Path) -> Result<Settings, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut settings = Settings::parse(&text)?;
        settings.path = Some(path.to_path_buf());
        Ok(settings)
    }

    /// Write the document. `None` means the remembered path, and with
    /// none remembered the caller has to name one. A path given here is
    /// remembered.
    pub fn save(&mut self, path: Option<&Path>) -> Result<PathBuf, String> {
        let path = match path.map(Path::to_path_buf).or_else(|| self.path.clone()) {
            Some(p) => p,
            None => return Err("no file to save to: /save <file>".into()),
        };
        std::fs::write(&path, self.doc.to_string())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.path = Some(path.clone());
        self.dirty = false;
        Ok(path)
    }

    /// One line per renamed key the document still uses.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        collect_renamed(self.doc.as_table(), &mut out);
        out
    }
```

Add after `drop_aliases`:

```rust
fn collect_renamed(table: &Table, out: &mut Vec<String>) {
    for (key, item) in table.iter() {
        if let Some((_, new)) = RENAMED_KEYS.iter().find(|(old, _)| *old == key) {
            out.push(format!("`{key}` is now `{new}` (still accepted)"));
        }
        if let Some(t) = item.as_table() {
            collect_renamed(t, out);
        }
    }
}
```

In `crates/mud-client/src/profile.rs`, delete `RENAMED_KEYS` and its doc comment, and replace the body of `Profile::load` with:

```rust
    /// Read and parse a profile, reporting renamed keys on stderr.
    ///
    /// The headless commands call this. The parse and the validation
    /// live in `settings`, which `play` uses directly so it can edit
    /// what it loaded.
    pub fn load(path: &std::path::Path) -> Result<Profile, String> {
        let settings = crate::settings::Settings::load(path)?;
        for warning in settings.warnings() {
            eprintln!("profile {}: {warning}", path.display());
        }
        Ok(settings.profile().clone())
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test settings` then `cargo test -p mud-client --test profile`
Expected: all pass.

Mutation check: in `save`, delete `self.dirty = false;`. `save_keeps_comments_and_order` fails. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/settings.rs crates/mud-client/src/profile.rs crates/mud-client/tests/settings.rs
git commit -m "feat(client): settings load and save the profile file, comments intact"
```

---

### Task 4: Listing by glob

**Files:**
- Modify: `crates/mud-client/src/settings.rs`
- Test: `crates/mud-client/tests/settings.rs`

**Interfaces:**
- Consumes: `KEYS`, `Settings::profile`.
- Produces:
  - `pub fn glob_match(pattern: &str, key: &str) -> bool`, `*` matches anything, a trailing `*` is implied.
  - `Settings::list(&self, pattern: &str) -> Vec<(String, String)>`, key and rendered value in `KEYS` order.
  - `Settings::value(&self, key: &str) -> Option<String>`, one rendered value.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/settings.rs`:

```rust
use mud_client::settings::glob_match;

#[test]
fn a_glob_matches_by_prefix_with_a_trailing_star_implied() {
    assert!(glob_match("bot", "bot.rest_at_percent"));
    assert!(glob_match("bot.*", "bot.rest_at_percent"));
    assert!(glob_match("bot.rest", "bot.rest_at_percent"));
    assert!(glob_match("bot.rest*", "bot.rest_until_percent"));
    assert!(glob_match("*heal*", "bot.minor_heal_at_percent"));
    assert!(glob_match("*heal*", "farm.heal_refused"));
    assert!(glob_match("", "host"));
    assert!(!glob_match("bot", "bank.at"));
    assert!(!glob_match("*heal*", "bank.at"));
}

#[test]
fn the_listing_walks_keys_in_file_order_and_renders_toml() {
    let s = Settings::parse(COMMENTED).unwrap();
    let rows = s.list("bot.rest");
    assert_eq!(
        rows,
        vec![
            ("bot.rest_at_percent".to_string(), "60".to_string()),
            ("bot.rest_until_percent".to_string(), "95".to_string()),
        ]
    );
    let all = s.list("");
    assert_eq!(all.len(), KEYS.len());
    assert_eq!(all[0].0, "target");
    assert_eq!(all[0].1, "\"mbbs\"");
}

#[test]
fn an_absent_table_lists_its_defaults() {
    let s = Settings::default();
    assert_eq!(s.value("bot.rest_at_percent").as_deref(), Some("60"));
    assert_eq!(s.value("bank.at").as_deref(), Some("unset"));
    assert_eq!(s.value("farm.circuit").as_deref(), Some("[]"));
}

#[test]
fn the_password_is_masked() {
    let s = Settings::parse(COMMENTED).unwrap();
    assert_eq!(s.value("password").as_deref(), Some("\"****\""));
    let empty = Settings::default();
    assert_eq!(empty.value("password").as_deref(), Some("\"\""));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test settings glob listing absent masked`
Expected: compile error, no `glob_match`, `list`, `value`.

- [ ] **Step 3: Implement**

Add to `settings.rs`, a free function:

```rust
/// irssi's `/set` pattern: `*` matches anything and a trailing `*` is
/// implied, so `bot` lists the bot table and `*heal*` finds every heal
/// key in every table.
pub fn glob_match(pattern: &str, key: &str) -> bool {
    let mut rest = key;
    for (i, part) in pattern.split('*').enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}
```

Add to `impl Settings`:

```rust
    /// Every key the pattern matches, with its value rendered as TOML,
    /// in `KEYS` order. Absent tables show their defaults, because that
    /// is what the character gets. The password shows as stars.
    pub fn list(&self, pattern: &str) -> Vec<(String, String)> {
        let table = self.effective_table();
        KEYS.iter()
            .filter(|key| glob_match(pattern, key))
            .map(|key| (key.to_string(), render(&table, key)))
            .collect()
    }

    /// One key's rendered value, or `None` for a key `/set` does not know.
    pub fn value(&self, key: &str) -> Option<String> {
        if !KEYS.contains(&key) {
            return None;
        }
        Some(render(&self.effective_table(), key))
    }

    /// The profile with its optional tables filled in, as a TOML table,
    /// so the listing reads what the character will actually get.
    fn effective_table(&self) -> toml::Table {
        let mut profile = self.profile.clone();
        profile.bot.get_or_insert_with(Default::default);
        profile.farm.get_or_insert_with(Default::default);
        let text = toml::to_string(&profile).expect("a profile serialises");
        text.parse().expect("a serialised profile parses")
    }
```

Add free functions:

```rust
fn render(table: &toml::Table, key: &str) -> String {
    match lookup(table, key) {
        Some(value) if key == "password" => match value.as_str() {
            Some("") => "\"\"".to_string(),
            _ => "\"****\"".to_string(),
        },
        Some(value) => value.to_string(),
        None => "unset".to_string(),
    }
}

fn lookup<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let mut value = table.get(parts.next()?)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    Some(value)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test settings`
Expected: all pass. If `the_listing_walks_keys_in_file_order_and_renders_toml` fails on the rendered string form, print `all[0].1` and match the test to what `toml::Value::to_string` produces for a string, which is the quoted form.

Mutation check: in `glob_match`, change `strip_prefix` to `contains`, so a pattern matches anywhere. The `!glob_match("bot", "bank.at")` line still holds but `bot.rest` listing gains nothing, so also check: change `rest.find(part)` to `Some(0)`. `*heal*` then matches `bank.at` and the glob test fails. Revert both.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/settings.rs crates/mud-client/tests/settings.rs
git commit -m "feat(client): list settings by glob, defaults shown for absent tables"
```

---

### Task 5: Completion

**Files:**
- Modify: `crates/mud-client/src/settings.rs`
- Modify: `crates/mud-client/src/tui.rs:30-90` (`InputEditor`)
- Test: `crates/mud-client/tests/settings.rs`, `crates/mud-client/tests/tui.rs`

**Interfaces:**
- Consumes: `KEYS`.
- Produces:
  - `pub struct Completion { pub start: usize, pub end: usize, pub text: String, pub list: Vec<String> }`, character offsets of the word to replace, its replacement, and the candidates to print when the word did not grow.
  - `pub fn complete(line: &str, cursor: usize, verbs: &[&str]) -> Option<Completion>`.
  - `pub fn common_prefix(words: &[String]) -> String`.
  - `InputEditor::replace(&mut self, start: usize, end: usize, text: &str)`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/settings.rs`:

```rust
use mud_client::settings::complete;

const VERBS: &[&str] = &["/set", "/save", "/quit", "/unset"];

#[test]
fn a_unique_verb_completes_with_a_space() {
    let c = complete("/sa", 3, VERBS).unwrap();
    assert_eq!((c.start, c.end, c.text.as_str()), (0, 3, "/save "));
    assert!(c.list.is_empty());
}

#[test]
fn a_unique_key_completes_after_set() {
    let c = complete("/set bot.ign", 12, VERBS).unwrap();
    assert_eq!((c.start, c.end, c.text.as_str()), (5, 12, "bot.ignore_coins "));
    assert!(c.list.is_empty());
    let c = complete("/unset bank.a", 13, VERBS).unwrap();
    assert_eq!(c.text, "bank.at ");
}

#[test]
fn several_candidates_grow_to_the_common_prefix_and_then_list() {
    let c = complete("/set bot.rest_", 14, VERBS).unwrap();
    assert_eq!(c.text, "bot.rest_");
    assert_eq!(
        c.list,
        vec![
            "bot.rest_at_percent".to_string(),
            "bot.rest_until_percent".to_string()
        ]
    );
    let c = complete("/set bot.min", 12, VERBS).unwrap();
    assert_eq!(c.text, "bot.minor_heal_", "the word grew, so nothing is listed");
    assert!(c.list.is_empty());
}

#[test]
fn completion_works_mid_line_and_only_on_slash_lines() {
    let c = complete("/set bot.ign 5", 12, VERBS).unwrap();
    assert_eq!((c.start, c.end), (5, 12));
    assert!(complete("set bot.ign", 11, VERBS).is_none());
    assert!(complete("/go bot.ign", 11, VERBS).is_none());
    assert!(complete("/set bot.rest_at_percent 6", 26, VERBS).is_none());
    assert!(complete("/zz", 3, VERBS).is_none());
}

#[test]
fn common_prefix_of_nothing_is_empty() {
    use mud_client::settings::common_prefix;
    assert_eq!(common_prefix(&[]), "");
    assert_eq!(common_prefix(&["abc".into(), "abd".into()]), "ab");
}
```

Append to `crates/mud-client/tests/tui.rs`, next to the editor tests:

```rust
#[test]
fn editor_replace_swaps_a_word_and_parks_the_cursor_after_it() {
    let mut e = InputEditor::new();
    for c in "/set bot.ign 5".chars() {
        e.insert(c);
    }
    e.replace(5, 12, "bot.ignore_coins ");
    assert_eq!(e.line(), "/set bot.ignore_coins  5");
    assert_eq!(e.cursor(), 22);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test settings complet common_prefix` and `cargo test -p mud-client --test tui editor_replace`
Expected: compile errors, no `complete`, `Completion`, `common_prefix`, `replace`.

- [ ] **Step 3: Implement**

Add to `settings.rs`:

```rust
/// What Tab does to the word under the cursor. `start..end` are
/// character offsets of that word. `text` replaces it. `list` is empty
/// unless the word could not grow, in which case it is the candidates
/// to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub list: Vec<String>,
}

/// Complete the word under the cursor. The first word of a slash line
/// completes against `verbs`. The second word after `/set` or `/unset`
/// completes against [`KEYS`]. Anything else is `None`.
pub fn complete(line: &str, cursor: usize, verbs: &[&str]) -> Option<Completion> {
    let chars: Vec<char> = line.chars().collect();
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |i| i + 1);
    let end = chars[cursor..]
        .iter()
        .position(|c| c.is_whitespace())
        .map_or(chars.len(), |i| cursor + i);
    let word: String = chars[start..end].iter().collect();
    let before: String = chars[..start].iter().collect();
    let index = before.split_whitespace().count();
    let verb = before.split_whitespace().next().unwrap_or_default();
    let candidates: Vec<String> = if index == 0 && word.starts_with('/') {
        verbs
            .iter()
            .filter(|v| v.starts_with(&word))
            .map(|v| v.to_string())
            .collect()
    } else if index == 1 && matches!(verb, "/set" | "/unset") {
        KEYS.iter()
            .filter(|k| k.starts_with(&word))
            .map(|k| k.to_string())
            .collect()
    } else {
        return None;
    };
    match candidates.as_slice() {
        [] => None,
        [one] => Some(Completion {
            start,
            end,
            text: format!("{one} "),
            list: Vec::new(),
        }),
        many => {
            let prefix = common_prefix(many);
            let list = if prefix.chars().count() > word.chars().count() {
                Vec::new()
            } else {
                many.to_vec()
            };
            Some(Completion {
                start,
                end,
                text: prefix,
                list,
            })
        }
    }
}

/// The longest prefix every word shares. Empty for no words.
pub fn common_prefix(words: &[String]) -> String {
    let Some(first) = words.first() else {
        return String::new();
    };
    first
        .chars()
        .enumerate()
        .take_while(|(i, c)| {
            words
                .iter()
                .all(|w| w.chars().nth(*i) == Some(*c))
        })
        .map(|(_, c)| c)
        .collect()
}
```

Add to `impl InputEditor` in `tui.rs`, after `end`:

```rust
    /// Replace the characters `start..end` with `text` and park the
    /// cursor after it. Completion's one edit.
    pub fn replace(&mut self, start: usize, end: usize, text: &str) {
        let end = end.min(self.chars.len());
        let start = start.min(end);
        self.chars.splice(start..end, text.chars());
        self.cursor = start + text.chars().count();
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test settings` and `cargo test -p mud-client --test tui`
Expected: all pass.

Mutation check: in `complete`, change `index == 1` to `index >= 1`. `completion_works_mid_line_and_only_on_slash_lines` fails on the third-word case. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/settings.rs crates/mud-client/src/tui.rs crates/mud-client/tests/settings.rs crates/mud-client/tests/tui.rs
git commit -m "feat(client): tab completion for slash verbs and setting keys"
```

---

### Task 6: A pure key handler and the new verbs

**Files:**
- Modify: `crates/mud-client/src/tui.rs` (`KeyOutcome`, `slash`, `help_text`, `handle_key`, the key arm in `play`)
- Test: `crates/mud-client/tests/tui.rs`

**Interfaces:**
- Consumes: `complete`, `InputEditor::replace`.
- Produces:
  - `pub const VERBS: &[&str]`, every verb `slash` claims.
  - `KeyOutcome` gains `Send(String)`, `Raw(Vec<u8>)`, `Note(String)`, `QuitNow`, `SetList { pattern: String }`, `Set { key: String, value: String }`, `Unset { key: String }`, `Save { file: Option<String> }`, `Load { file: String }`, `Connect { target: Option<String> }`, `Disconnect`.
  - `pub fn handle_key(key: &KeyEvent, editor: &mut InputEditor, passthrough: &mut bool, farming: bool) -> KeyOutcome`. No session. The caller sends.

- [ ] **Step 1: Rewrite the tests that hand the handler a session**

In `crates/mud-client/tests/tui.rs`, replace `typing_reaches_the_board_during_a_farm_run`, `ctrl_f_takes_the_keyboard_back_from_any_job` and `slash_commands_are_intercepted_not_sent_to_the_board` with:

```rust
#[test]
fn a_typed_line_comes_back_as_send_even_during_a_farm_run() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    for c in "gossip hello".chars() {
        assert_eq!(
            handle_key(&key(KeyCode::Char(c)), &mut editor, &mut passthrough, true),
            KeyOutcome::Continue
        );
    }
    assert_eq!(editor.line(), "gossip hello", "keys must reach the editor while farming");
    assert_eq!(
        handle_key(&key(KeyCode::Enter), &mut editor, &mut passthrough, true),
        KeyOutcome::Send("gossip hello".into())
    );
    assert_eq!(editor.line(), "");
}

#[test]
fn ctrl_f_takes_the_keyboard_back_from_any_job() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    assert_eq!(handle_key(&ctrl('f'), &mut editor, &mut passthrough, true), KeyOutcome::TakeOver);
    assert_eq!(handle_key(&ctrl('q'), &mut editor, &mut passthrough, true), KeyOutcome::QuitNow);
}

#[test]
fn slash_commands_are_claimed_and_everything_else_is_sent() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    let mut submit = |line: &str| {
        for c in line.chars() {
            handle_key(&key(KeyCode::Char(c)), &mut editor, &mut passthrough, false);
        }
        handle_key(&key(KeyCode::Enter), &mut editor, &mut passthrough, false)
    };
    for line in ["/bot", "/go Grungy Shop", "/go", "/farm", "/set", "/save"] {
        assert!(!matches!(submit(line), KeyOutcome::Send(_)), "{line} must not go to the board");
    }
    assert_eq!(submit("gossip hi"), KeyOutcome::Send("gossip hi".into()));
}

#[test]
fn passthrough_keys_come_back_as_raw_bytes() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    assert_eq!(handle_key(&ctrl('p'), &mut editor, &mut passthrough, false), KeyOutcome::Continue);
    assert!(passthrough);
    assert_eq!(
        handle_key(&key(KeyCode::Up), &mut editor, &mut passthrough, false),
        KeyOutcome::Raw(b"\x1b[A".to_vec())
    );
    assert_eq!(handle_key(&ctrl('q'), &mut editor, &mut passthrough, false), KeyOutcome::QuitNow);
}

#[test]
fn tab_completes_and_lists() {
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    for c in "/set bot.ign".chars() {
        handle_key(&key(KeyCode::Char(c)), &mut editor, &mut passthrough, false);
    }
    assert_eq!(handle_key(&key(KeyCode::Tab), &mut editor, &mut passthrough, false), KeyOutcome::Continue);
    assert_eq!(editor.line(), "/set bot.ignore_coins ");
    let mut editor = InputEditor::new();
    for c in "/set bot.rest_".chars() {
        handle_key(&key(KeyCode::Char(c)), &mut editor, &mut passthrough, false);
    }
    match handle_key(&key(KeyCode::Tab), &mut editor, &mut passthrough, false) {
        KeyOutcome::Note(text) => {
            assert!(text.contains("bot.rest_at_percent"));
            assert!(text.contains("bot.rest_until_percent"));
        }
        other => panic!("expected the candidates, got {other:?}"),
    }
    assert_eq!(editor.line(), "/set bot.rest_");
}

#[test]
fn the_settings_verbs_parse() {
    assert_eq!(slash("/set"), Some(KeyOutcome::SetList { pattern: String::new() }));
    assert_eq!(slash("/set bot.rest*"), Some(KeyOutcome::SetList { pattern: "bot.rest*".into() }));
    assert_eq!(
        slash("/set bot.rest_command sit down"),
        Some(KeyOutcome::Set { key: "bot.rest_command".into(), value: "sit down".into() })
    );
    assert!(matches!(slash("/set bot.* 5"), Some(KeyOutcome::Refuse(_))));
    assert_eq!(slash("/unset bank.at"), Some(KeyOutcome::Unset { key: "bank.at".into() }));
    assert!(matches!(slash("/unset"), Some(KeyOutcome::Refuse(_))));
    assert_eq!(slash("/save"), Some(KeyOutcome::Save { file: None }));
    assert_eq!(slash("/save chars/dan.toml"), Some(KeyOutcome::Save { file: Some("chars/dan.toml".into()) }));
    assert_eq!(slash("/load chars/dan.toml"), Some(KeyOutcome::Load { file: "chars/dan.toml".into() }));
    assert!(matches!(slash("/load"), Some(KeyOutcome::Refuse(_))));
    assert_eq!(slash("/connect"), Some(KeyOutcome::Connect { target: None }));
    assert_eq!(slash("/connect bbs.example.com:2327"), Some(KeyOutcome::Connect { target: Some("bbs.example.com:2327".into()) }));
    assert_eq!(slash("/disconnect"), Some(KeyOutcome::Disconnect));
}

#[test]
fn every_verb_in_the_completion_list_is_claimed_and_in_help() {
    for verb in mud_client::tui::VERBS {
        assert!(slash(verb).is_some() || slash(&format!("{verb} x")).is_some(), "{verb} is not claimed");
        assert!(help_text().contains(verb), "{verb} is not in /help");
    }
}
```

Add `help_text` to the `use mud_client::tui::{...}` line at the top of the test file, and make `help_text` `pub` in `tui.rs`. Delete `capture_board` and `session_to` only if nothing else in the file uses them. `entering_the_realm_arms_and_fills_the_sessions_purse` uses `session_to`, so keep it.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test tui`
Expected: compile errors, wrong arity on `handle_key`, missing variants.

- [ ] **Step 3: Implement**

In `tui.rs`, add before `KeyOutcome`:

```rust
/// Every verb `slash` claims, for completion. A verb here that `slash`
/// does not claim, or that `help_text` does not list, fails the test
/// `every_verb_in_the_completion_list_is_claimed_and_in_help`.
pub const VERBS: &[&str] = &[
    "/quit", "/farm", "/loop", "/bot", "/go", "/bank", "/where", "/room", "/map", "/help",
    "/set", "/unset", "/save", "/load", "/connect", "/disconnect",
];
```

Add these variants to `KeyOutcome`:

```rust
    /// A line for the board. The caller sends it: the handler has no
    /// session, so the lobby and play share it.
    Send(String),
    /// Passthrough bytes for the board, likewise.
    Raw(Vec<u8>),
    /// Print this and send nothing. Completion candidates, mostly.
    Note(String),
    /// Ctrl-Q: exit without the unsaved-settings check `/quit` makes.
    QuitNow,
    /// `/set` with a pattern or nothing: list.
    SetList {
        pattern: String,
    },
    /// `/set key value`: write.
    Set {
        key: String,
        value: String,
    },
    Unset {
        key: String,
    },
    Save {
        file: Option<String>,
    },
    Load {
        file: String,
    },
    /// `/connect [host[:port]]`. Unparsed here, `connect_target` parses.
    Connect {
        target: Option<String>,
    },
    Disconnect,
```

Add these arms to `slash`, before `_ => None`:

```rust
        "/set" => match rest.split_once(char::is_whitespace) {
            Some((key, _)) if key.contains('*') => Some(KeyOutcome::Refuse(format!(
                "set: {key} is a pattern, which lists; give one key a value"
            ))),
            Some((key, value)) => Some(KeyOutcome::Set {
                key: key.to_string(),
                value: value.trim().to_string(),
            }),
            None => Some(KeyOutcome::SetList {
                pattern: rest.to_string(),
            }),
        },
        "/unset" if rest.is_empty() => Some(KeyOutcome::Refuse(
            "unset: which key? try `/unset bank.at`".into(),
        )),
        "/unset" => Some(KeyOutcome::Unset {
            key: rest.to_string(),
        }),
        "/save" => Some(KeyOutcome::Save {
            file: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/load" if rest.is_empty() => Some(KeyOutcome::Refuse(
            "load: which file? try `/load chars/dan.toml`".into(),
        )),
        "/load" => Some(KeyOutcome::Load {
            file: rest.to_string(),
        }),
        "/connect" => Some(KeyOutcome::Connect {
            target: (!rest.is_empty()).then(|| rest.to_string()),
        }),
        "/disconnect" => Some(KeyOutcome::Disconnect),
```

Make `help_text` `pub fn` and add these lines after the `/help` line:

```text
/set [pattern]       list settings, all or those the glob matches (bot, bot.rest*, *heal*)
/set <key> <value>   change a setting now; the value is TOML, a bare word is a string
/unset <key>         remove a setting so its default applies
/save [file]         write the settings; the file is remembered
/load <file>         read settings from a file
/connect [host[:port]]  connect, setting host and port when given
/disconnect          close the line and return to the lobby
Tab                  complete a slash verb or a setting key
```

Rewrite `handle_key`:

```rust
/// One keystroke against the editor. Pure: what to send comes back as
/// `Send` or `Raw` and the caller sends it, so the lobby, which has
/// nothing to send to, and play share this. Public for the keyboard
/// contract tests.
pub fn handle_key(
    key: &KeyEvent,
    editor: &mut InputEditor,
    passthrough: &mut bool,
    farming: bool,
) -> KeyOutcome {
    if farming && key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('f') => KeyOutcome::TakeOver,
            KeyCode::Char('q') => KeyOutcome::QuitNow,
            _ => KeyOutcome::Continue,
        };
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
        *passthrough = !*passthrough;
        return KeyOutcome::Continue;
    }
    if *passthrough {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            return KeyOutcome::QuitNow;
        }
        return match key_bytes(key) {
            Some(bytes) => KeyOutcome::Raw(bytes),
            None => KeyOutcome::Continue,
        };
    }
    match key.code {
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return KeyOutcome::QuitNow;
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => editor.insert(c),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Up => editor.history_prev(),
        KeyCode::Down => editor.history_next(),
        KeyCode::Tab => {
            if let Some(c) = crate::settings::complete(&editor.line(), editor.cursor(), VERBS) {
                editor.replace(c.start, c.end, &c.text);
                if !c.list.is_empty() {
                    return KeyOutcome::Note(c.list.join("  "));
                }
            }
        }
        KeyCode::Enter => {
            let line = editor.take_line();
            return match slash(&line) {
                Some(outcome) => outcome,
                None => KeyOutcome::Send(line),
            };
        }
        _ => {}
    }
    KeyOutcome::Continue
}
```

Keep the long comment above the farming check that explains why typing works during a run. Move it, do not delete it.

In `play`'s key arm, change the call to `handle_key(&key, &mut editor, &mut passthrough, job.is_some())` and add arms to the `match outcome`:

```rust
                            KeyOutcome::Send(line) => session.send(&line),
                            KeyOutcome::Raw(bytes) => session.send_raw(&bytes),
                            KeyOutcome::Note(text) => note(&mut out, &text)?,
                            KeyOutcome::QuitNow => break Ok(()),
                            KeyOutcome::SetList { .. }
                            | KeyOutcome::Set { .. }
                            | KeyOutcome::Unset { .. }
                            | KeyOutcome::Save { .. }
                            | KeyOutcome::Load { .. }
                            | KeyOutcome::Connect { .. }
                            | KeyOutcome::Disconnect => {
                                note(&mut out, "-- not wired yet --")?;
                            }
```

Task 9 replaces that last arm. This task only keeps the build green.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test tui` then `cargo test -p mud-client`
Expected: all pass. `cargo clippy -p mud-client --all-targets` adds no warnings.

Mutation check: in `handle_key`, make `Tab` fall through to `_ => {}`. `tab_completes_and_lists` fails. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/tests/tui.rs
git commit -m "refactor(client): the key handler returns what to send, and claims the settings verbs"
```

---

### Task 7: The session's profile can change, and the session can close

**Files:**
- Modify: `crates/mud-client/src/session.rs`
- Modify: call sites in `src/tui.rs`, `src/mapview.rs`, `src/farm.rs`, `src/dialect.rs`, `src/script.rs`, and any other file the compiler names
- Test: `crates/mud-client/tests/session_close.rs` (create)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `Session::profile(&self) -> Profile`, a clone.
  - `Session::set_profile(&self, profile: Profile)`.
  - `Session::close(&self)`: shuts the socket's write side, stops the reader, wakes every pending expect with the closed error.
  - `Cmd::Close`.

- [ ] **Step 1: Write the failing test**

Create `crates/mud-client/tests/session_close.rs`:

```rust
//! A session's profile is replaced by `/set`, and `/disconnect` closes it.

use std::time::Duration;

use mud_client::profile::Profile;
use mud_client::session::Session;

async fn silent_board() -> (std::net::SocketAddr, tokio::sync::oneshot::Receiver<usize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 64];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let _ = tx.send(n);
    });
    (addr, rx)
}

fn profile_for(addr: std::net::SocketAddr) -> Profile {
    let mut p = Profile::default();
    p.host = addr.ip().to_string();
    p.port = addr.port();
    p.username = "dan".into();
    p.pace_ms = Some(0);
    p
}

#[tokio::test]
async fn set_profile_is_what_the_next_reader_sees() {
    let (addr, _) = silent_board().await;
    let session = Session::connect(&profile_for(addr), None).await.unwrap();
    assert_eq!(session.profile().username, "dan");
    let mut changed = session.profile();
    changed.username = "ann".into();
    session.set_profile(changed);
    assert_eq!(session.profile().username, "ann");
}

#[tokio::test]
async fn close_ends_the_streams_and_the_board_sees_the_line_drop() {
    let (addr, saw_close) = silent_board().await;
    let session = Session::connect(&profile_for(addr), None).await.unwrap();
    let mut raw = session.raw();
    session.close();
    assert!(
        matches!(raw.recv().await, Err(tokio::sync::broadcast::error::RecvError::Closed)),
        "the raw stream must end"
    );
    assert!(
        session.expect("anything", Duration::from_secs(2)).await.is_err(),
        "a pending expect must fail at once, not wait out its timeout"
    );
    assert_eq!(saw_close.await.unwrap(), 0, "the board reads end of file");
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p mud-client --test session_close`
Expected: compile error, no `set_profile`, no `close`, and `profile()` returns a reference.

- [ ] **Step 3: Implement**

In `session.rs`:

Change the field to `profile: std::sync::RwLock<Profile>,` and add a field `reader: tokio::task::AbortHandle,` with the doc comment:

```rust
    /// The reader task, so `close` can stop it. Dropping the read half
    /// is what closes the socket for good once the writer has shut its
    /// side.
    reader: tokio::task::AbortHandle,
```

Add to `Cmd`:

```rust
    /// Shut the socket's write side and stop the writer.
    Close,
```

In the writer task's `match cmd`, add:

```rust
                        Cmd::Close => {
                            use tokio::io::AsyncWriteExt as _;
                            let _ = write_half.shutdown().await;
                            break;
                        }
```

Bind the reader task: change the `tokio::spawn(async move {` that owns `read_half` to `let reader = tokio::spawn(async move {` and after its closing `});` add nothing else; then in the `Ok(Session { .. })` literal use `profile: std::sync::RwLock::new(profile.clone()),` and `reader: reader.abort_handle(),`.

Replace the accessor:

```rust
    /// The profile this session runs under. A clone: `/set` replaces it
    /// with [`Session::set_profile`], and a job reads it when it starts.
    pub fn profile(&self) -> Profile {
        self.profile.read().expect("profile lock").clone()
    }

    /// Replace the profile. The next job start reads the new one. Nothing
    /// running re-reads it.
    pub fn set_profile(&self, profile: Profile) {
        *self.profile.write().expect("profile lock") = profile;
    }

    /// Close the line: the writer shuts the socket's write side, the
    /// reader stops so the read half drops, and every pending expect
    /// wakes with the closed error. Idempotent.
    pub fn close(&self) {
        let _ = self.cmd_tx.send(Cmd::Close);
        self.reader.abort();
        self.shared.close();
    }
```

Fix the call sites the compiler reports. The forms: `session.profile().clone()` becomes `session.profile()`. `locator(session.profile())` becomes `locator(&session.profile())`. `apply_evil_preference(session, session.profile())` becomes `apply_evil_preference(session, &session.profile())`. `dialect::login(&s, s.profile())` becomes `dialect::login(&s, &s.profile())`. `session.profile().username.clone()` becomes `session.profile().username`. Where a `&session.profile().username` is held across a statement and the compiler complains about a dropped temporary, bind `let profile = session.profile();` first and use `&profile.username`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test session_close` then `cargo test -p mud-client`
Expected: all pass. `cargo clippy -p mud-client --all-targets` adds no warnings. Clippy may flag a now-redundant `.clone()` on a `profile()` call. Remove it.

Mutation check: in `close`, delete `self.shared.close();`. The expect assertion fails, the expect waits out its timeout. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src crates/mud-client/tests/session_close.rs
git commit -m "feat(client): a session's profile can be replaced, and the session closed"
```

---

### Task 8: Applying a settings command

**Files:**
- Modify: `crates/mud-client/src/tui.rs`
- Test: `crates/mud-client/tests/tui.rs`

**Interfaces:**
- Consumes: `Settings`, `KeyOutcome`.
- Produces:
  - `pub struct Applied { pub note: String, pub profile_changed: bool, pub bot_changed: bool }`.
  - `pub fn apply_settings(outcome: &KeyOutcome, settings: &mut Settings) -> Option<Applied>`, `None` when the outcome is not a settings command.
  - `pub fn assist_config_for(profile: &Profile) -> BotConfig`, the profile's bot table or the attack-and-loot default play uses.
  - `pub fn needs_username(profile: &Profile) -> Result<(), String>`.
  - `pub fn connect_target(arg: &str) -> Result<(String, u16), String>`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/tui.rs`:

```rust
use mud_client::settings::Settings;
use mud_client::tui::{apply_settings, assist_config_for, connect_target, needs_username};

#[test]
fn set_changes_the_profile_and_says_so_unsaved() {
    let mut s = Settings::default();
    let applied = apply_settings(
        &KeyOutcome::Set { key: "bot.rest_at_percent".into(), value: "45".into() },
        &mut s,
    )
    .unwrap();
    assert!(applied.profile_changed);
    assert!(applied.bot_changed);
    assert!(applied.note.contains("bot.rest_at_percent = 45"), "{}", applied.note);
    assert!(applied.note.contains("unsaved"), "{}", applied.note);
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 45);
}

#[test]
fn a_refused_set_changes_nothing_and_reports() {
    let mut s = Settings::default();
    let applied = apply_settings(
        &KeyOutcome::Set { key: "bot.rest_at_percent".into(), value: "soon".into() },
        &mut s,
    )
    .unwrap();
    assert!(!applied.profile_changed);
    assert!(applied.note.contains("set:"), "{}", applied.note);
    assert!(s.profile().bot.is_none());
}

#[test]
fn a_bank_key_does_not_touch_the_assist() {
    let mut s = Settings::default();
    let applied = apply_settings(&KeyOutcome::Set { key: "bank.keep_gold".into(), value: "5".into() }, &mut s).unwrap();
    assert!(applied.profile_changed);
    assert!(!applied.bot_changed);
}

#[test]
fn a_listing_is_one_row_per_key() {
    let mut s = Settings::default();
    let applied = apply_settings(&KeyOutcome::SetList { pattern: "bot.rest".into() }, &mut s).unwrap();
    assert_eq!(applied.note.lines().count(), 2, "{}", applied.note);
    assert!(applied.note.lines().all(|l| l.contains(" = ")));
    assert!(!applied.profile_changed);
    let none = apply_settings(&KeyOutcome::SetList { pattern: "zebra".into() }, &mut s).unwrap();
    assert!(none.note.contains("no setting matches"));
}

#[test]
fn load_replaces_everything_and_rebuilds_the_assist() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("tui");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loaded.toml");
    std::fs::write(&path, "username = \"ann\"\n[bot]\nauto_heal = true\n").unwrap();
    let mut s = Settings::default();
    let applied = apply_settings(&KeyOutcome::Load { file: path.display().to_string() }, &mut s).unwrap();
    assert!(applied.profile_changed);
    assert!(applied.bot_changed);
    assert_eq!(s.profile().username, "ann");
    assert_eq!(s.path(), Some(path.as_path()));
    let missing = apply_settings(&KeyOutcome::Load { file: dir.join("none.toml").display().to_string() }, &mut s).unwrap();
    assert!(missing.note.contains("load:"));
    assert_eq!(s.profile().username, "ann", "a failed load keeps what was there");
}

#[test]
fn save_reports_the_path_or_asks_for_one() {
    let mut s = Settings::default();
    let asked = apply_settings(&KeyOutcome::Save { file: None }, &mut s).unwrap();
    assert!(asked.note.contains("/save <file>"), "{}", asked.note);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("tui");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved.toml");
    let saved = apply_settings(&KeyOutcome::Save { file: Some(path.display().to_string()) }, &mut s).unwrap();
    assert!(saved.note.contains("saved"), "{}", saved.note);
    assert!(path.exists());
}

#[test]
fn other_outcomes_are_not_settings_commands() {
    let mut s = Settings::default();
    assert!(apply_settings(&KeyOutcome::Where, &mut s).is_none());
    assert!(apply_settings(&KeyOutcome::Send("look".into()), &mut s).is_none());
}

#[test]
fn a_profile_without_a_bot_table_gets_the_attack_and_loot_assist() {
    let cfg = assist_config_for(&Profile::default());
    assert!(cfg.auto_combat && cfg.auto_get && !cfg.auto_heal);
    let mut with = Profile::default();
    with.bot = Some(BotConfig { auto_heal: true, ..Default::default() });
    let cfg = assist_config_for(&with);
    assert!(!cfg.auto_combat && cfg.auto_heal);
}

#[test]
fn jobs_need_a_username() {
    let err = needs_username(&Profile::default()).unwrap_err();
    assert!(err.contains("/set username"), "{err}");
    let mut named = Profile::default();
    named.username = "dan".into();
    assert!(needs_username(&named).is_ok());
}

#[test]
fn connect_target_defaults_the_port_to_telnet() {
    assert_eq!(connect_target("bbs.example.com").unwrap(), ("bbs.example.com".into(), 23));
    assert_eq!(connect_target("127.0.0.1:2327").unwrap(), ("127.0.0.1".into(), 2327));
    assert!(connect_target("host:port").is_err());
    assert!(connect_target("").is_err());
}
```

Add `use mud_client::profile::Profile;` at the top of the test file if it is not there.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test tui`
Expected: compile errors, the four functions do not exist.

- [ ] **Step 3: Implement**

Add to `tui.rs`, after `help_text`:

```rust
/// What a settings command did, for `play` and the lobby to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub note: String,
    /// The session's copy needs replacing.
    pub profile_changed: bool,
    /// A `bot.*` key moved, or a whole file came in: rebuild the assist.
    pub bot_changed: bool,
}

/// Run one settings command against the settings. `None` when the
/// outcome is not one. The note is ready to print.
pub fn apply_settings(outcome: &KeyOutcome, settings: &mut crate::settings::Settings) -> Option<Applied> {
    let said = |note: String| {
        Some(Applied {
            note,
            profile_changed: false,
            bot_changed: false,
        })
    };
    match outcome {
        KeyOutcome::SetList { pattern } => {
            let rows = settings.list(pattern);
            if rows.is_empty() {
                return said(format!("-- no setting matches {pattern} --"));
            }
            let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
            let text = rows
                .iter()
                .map(|(k, v)| format!("{k:width$} = {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            said(text)
        }
        KeyOutcome::Set { key, value } => match settings.set(key, value) {
            Ok(()) => Some(Applied {
                note: format!(
                    "-- {key} = {} (unsaved: /save) --",
                    settings.value(key).unwrap_or_default()
                ),
                profile_changed: true,
                bot_changed: key.starts_with("bot."),
            }),
            Err(e) => said(format!("-- set: {e} --")),
        },
        KeyOutcome::Unset { key } => match settings.unset(key) {
            Ok(()) => Some(Applied {
                note: format!(
                    "-- {key} = {} (unsaved: /save) --",
                    settings.value(key).unwrap_or_default()
                ),
                profile_changed: true,
                bot_changed: key.starts_with("bot."),
            }),
            Err(e) => said(format!("-- unset: {e} --")),
        },
        KeyOutcome::Save { file } => {
            match settings.save(file.as_deref().map(std::path::Path::new)) {
                Ok(path) => said(format!("-- saved {} --", path.display())),
                Err(e) => said(format!("-- save: {e} --")),
            }
        }
        KeyOutcome::Load { file } => {
            match crate::settings::Settings::load(std::path::Path::new(file)) {
                Ok(loaded) => {
                    let mut note = format!("-- loaded {file}; connection keys apply at the next /connect --");
                    for warning in loaded.warnings() {
                        note.push_str(&format!("\n-- {warning} --"));
                    }
                    *settings = loaded;
                    Some(Applied {
                        note,
                        profile_changed: true,
                        bot_changed: true,
                    })
                }
                Err(e) => said(format!("-- load: {e} --")),
            }
        }
        _ => None,
    }
}

/// The assist's config: the profile's bot table, or attack and loot for
/// a profile without one, because attack and loot are the whole point of
/// asking for an assist.
pub fn assist_config_for(profile: &crate::profile::Profile) -> crate::bot::BotConfig {
    profile.bot.clone().unwrap_or(crate::bot::BotConfig {
        auto_combat: true,
        auto_get: true,
        ..Default::default()
    })
}

/// A job or the assist matches the character's own death line against
/// the username. Empty, they would never know the character died.
pub fn needs_username(profile: &crate::profile::Profile) -> Result<(), String> {
    if profile.username.is_empty() {
        return Err(
            "username is empty: /set username <name> (the runner needs it to see your own death)"
                .into(),
        );
    }
    Ok(())
}

/// `host` or `host:port`. The port defaults to telnet's 23.
pub fn connect_target(arg: &str) -> Result<(String, u16), String> {
    let (host, port) = match arg.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("connect: {port} is not a port"))?;
            (host, port)
        }
        None => (arg, 23),
    };
    if host.is_empty() {
        return Err("connect: no host: /connect host[:port]".into());
    }
    Ok((host.to_string(), port))
}
```

In `play`, replace the `assist_config` initialiser with `let mut assist_config = assist_config_for(&session.profile());`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test tui` then `cargo test -p mud-client`
Expected: all pass. Clippy clean.

Mutation check: in `apply_settings`, change `bot_changed: key.starts_with("bot.")` to `true`. `a_bank_key_does_not_touch_the_assist` fails. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/tests/tui.rs
git commit -m "feat(client): settings commands apply to the loaded profile"
```

---

### Task 9: The lobby, connect and disconnect

**Files:**
- Modify: `crates/mud-client/src/tui.rs` (`play`, new `run`, `lobby`, `lobby_step`, `ContentCache`)
- Modify: `crates/mud-client/src/cli.rs:20-23`
- Modify: `crates/mud-client/src/bin/mmc.rs` (`main`, `play_command`)
- Test: `crates/mud-client/tests/tui.rs`, `crates/mud-client/tests/cli.rs`

**Interfaces:**
- Consumes: `Settings`, `apply_settings`, `connect_target`, `needs_username`, `Session::close`, `Session::set_profile`.
- Produces:
  - `pub enum PlayEnd { Quit, Closed }`.
  - `pub enum LobbyStep { Stay, Connect, Quit }`.
  - `pub fn lobby_step(outcome: KeyOutcome, settings: &mut Settings, quit_armed: &mut bool) -> (LobbyStep, Option<String>)`.
  - `pub type World = (Arc<RoomGraph>, Arc<SpawnTable>, Arc<Content>)`, `pub struct ContentCache`, `ContentCache::world(&mut self, db: &Path) -> Option<World>`.
  - `pub async fn run(settings: Settings, capture: Option<Capture>) -> std::io::Result<()>`, the new entry point.
  - `play` becomes `async fn play(session: Arc<Session>, settings: &mut Settings, cache: &mut ContentCache, key_rx: &mut UnboundedReceiver<TermEvent>, out: &mut std::io::Stdout) -> std::io::Result<PlayEnd>`, private.
  - `Command::Play { profile: Option<PathBuf>, .. }`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/tui.rs`:

```rust
use mud_client::tui::{ContentCache, LobbyStep, lobby_step};

async fn banner_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\nUsername: ").await.unwrap();
        let mut hold = [0u8; 64];
        use tokio::io::AsyncReadExt;
        let _ = sock.read(&mut hold).await;
    });
    addr
}

/// The lobby's whole job: settings, then `/connect`, then a session that
/// sees the board. The terminal loop around it cannot be driven from a
/// test, so this drives the seam it is built on.
#[tokio::test]
async fn the_lobby_sets_host_and_port_then_connects_and_sees_the_banner() {
    let addr = banner_board().await;
    let mut settings = Settings::default();
    let mut armed = false;
    let (step, note) = lobby_step(KeyOutcome::Send("look".into()), &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    assert!(note.unwrap().contains("not connected"));
    let (step, _) = lobby_step(slash(&format!("/connect {}:{}", addr.ip(), addr.port())).unwrap(), &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Connect));
    assert_eq!(settings.profile().host, addr.ip().to_string());
    assert_eq!(settings.profile().port, addr.port());
    assert!(settings.dirty(), "the host and port are settings, so /save keeps them");
    let session = Session::connect(settings.profile(), None).await.unwrap();
    session.expect("Welcome to the Test Board", std::time::Duration::from_secs(5)).await.unwrap();
}

#[test]
fn the_lobby_refuses_to_connect_nowhere_and_refuses_jobs() {
    let mut settings = Settings::default();
    let mut armed = false;
    let (step, note) = lobby_step(KeyOutcome::Connect { target: None }, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    assert!(note.unwrap().contains("no host"));
    let (step, note) = lobby_step(KeyOutcome::StartFarm { loop_name: None }, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    assert!(note.unwrap().contains("not connected"));
}

#[test]
fn quit_in_the_lobby_asks_once_while_unsaved() {
    let mut settings = Settings::default();
    let mut armed = false;
    settings.set("host", "\"h\"").unwrap();
    let (step, note) = lobby_step(KeyOutcome::Quit, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    assert!(note.unwrap().contains("unsaved"));
    let (step, _) = lobby_step(KeyOutcome::Quit, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Quit));
    let mut armed = false;
    let (step, _) = lobby_step(KeyOutcome::QuitNow, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Quit), "Ctrl-Q never asks");
    let mut clean = Settings::default();
    let (step, _) = lobby_step(KeyOutcome::Quit, &mut clean, &mut armed);
    assert!(matches!(step, LobbyStep::Quit), "nothing unsaved, nothing to ask");
}

#[test]
fn a_command_between_two_quits_disarms_the_second() {
    let mut settings = Settings::default();
    settings.set("host", "\"h\"").unwrap();
    let mut armed = false;
    let (step, _) = lobby_step(KeyOutcome::Quit, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay));
    let _ = lobby_step(KeyOutcome::SetList { pattern: String::new() }, &mut settings, &mut armed);
    let (step, _) = lobby_step(KeyOutcome::Quit, &mut settings, &mut armed);
    assert!(matches!(step, LobbyStep::Stay), "the refusal is for two /quit in a row");
}

#[test]
fn a_missing_world_database_is_none_and_asked_again_next_time() {
    let mut cache = ContentCache::default();
    let nowhere = std::path::Path::new("nowhere/at/all.sqlite");
    assert!(cache.world(nowhere).is_none());
    assert!(cache.world(nowhere).is_none());
}
```

Append to `crates/mud-client/tests/cli.rs`:

```rust
#[test]
fn play_without_a_profile_opens_the_lobby() {
    let cli = Cli::parse_from(["mmc", "play"]);
    match cli.command {
        Command::Play { profile, capture } => {
            assert!(profile.is_none());
            assert!(capture.is_none());
        }
        _ => panic!("expected play"),
    }
    let cli = Cli::parse_from(["mmc", "play", "--profile", "chars/dan.toml"]);
    match cli.command {
        Command::Play { profile, .. } => assert_eq!(profile.unwrap().to_str(), Some("chars/dan.toml")),
        _ => panic!("expected play"),
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test tui lobby quit world` and `cargo test -p mud-client --test cli`
Expected: compile errors.

- [ ] **Step 3: Implement the seams**

Add to `tui.rs`:

```rust
/// Why `play` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayEnd {
    /// The operator quit. The program ends.
    Quit,
    /// The line closed, by the board or by `/disconnect`. Back to the
    /// lobby with the settings intact.
    Closed,
}

/// The lobby's answer to one submitted line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyStep {
    Stay,
    Connect,
    Quit,
}

/// One outcome against the lobby: settings commands, `/connect`, `/help`
/// and `/quit`. Everything that needs a board says so. `quit_armed` is
/// the unsaved-settings refusal: set by a refused `/quit`, cleared by
/// any other command, so two `/quit` in a row exit.
pub fn lobby_step(
    outcome: KeyOutcome,
    settings: &mut crate::settings::Settings,
    quit_armed: &mut bool,
) -> (LobbyStep, Option<String>) {
    if !matches!(outcome, KeyOutcome::Quit | KeyOutcome::Continue) {
        *quit_armed = false;
    }
    if let Some(applied) = apply_settings(&outcome, settings) {
        return (LobbyStep::Stay, Some(applied.note));
    }
    match outcome {
        KeyOutcome::Continue => (LobbyStep::Stay, None),
        KeyOutcome::QuitNow => (LobbyStep::Quit, None),
        KeyOutcome::Quit => {
            if settings.dirty() && !*quit_armed {
                *quit_armed = true;
                (LobbyStep::Stay, Some("-- unsaved settings: /save, or /quit again --".into()))
            } else {
                (LobbyStep::Quit, None)
            }
        }
        KeyOutcome::Connect { target } => {
            if let Some(target) = target {
                let (host, port) = match connect_target(&target) {
                    Ok(t) => t,
                    Err(e) => return (LobbyStep::Stay, Some(format!("-- {e} --"))),
                };
                if let Err(e) = settings
                    .set("host", &format!("{host:?}"))
                    .and_then(|()| settings.set("port", &port.to_string()))
                {
                    return (LobbyStep::Stay, Some(format!("-- connect: {e} --")));
                }
            }
            if settings.profile().host.is_empty() {
                return (LobbyStep::Stay, Some("-- connect: no host: /connect host[:port] --".into()));
            }
            (LobbyStep::Connect, None)
        }
        KeyOutcome::Help => (LobbyStep::Stay, Some(help_text().to_string())),
        KeyOutcome::Note(text) | KeyOutcome::Refuse(text) => (LobbyStep::Stay, Some(text)),
        KeyOutcome::Send(_)
        | KeyOutcome::Raw(_)
        | KeyOutcome::Disconnect
        | KeyOutcome::StartFarm { .. }
        | KeyOutcome::Loops { .. }
        | KeyOutcome::ImportLoop { .. }
        | KeyOutcome::TakeOver
        | KeyOutcome::ToggleAssist
        | KeyOutcome::Go { .. }
        | KeyOutcome::Bank
        | KeyOutcome::Where
        | KeyOutcome::Room { .. }
        | KeyOutcome::Map { .. } => (
            LobbyStep::Stay,
            Some("-- not connected: /connect host[:port] --".into()),
        ),
        KeyOutcome::SetList { .. }
        | KeyOutcome::Set { .. }
        | KeyOutcome::Unset { .. }
        | KeyOutcome::Save { .. }
        | KeyOutcome::Load { .. } => (LobbyStep::Stay, None),
    }
}

/// The world files one content path loads: graph, spawn table and the
/// content decoder.
pub type World = (
    Arc<crate::graph::RoomGraph>,
    Arc<crate::spawn::SpawnTable>,
    Arc<mud_core::content::Content>,
);

/// World files by content path, loaded once per path for the life of
/// the program. A second connection on the same database reuses the
/// first load. A path that fails to load is not remembered, so a file
/// that appears later is found.
#[derive(Default)]
pub struct ContentCache {
    worlds: std::collections::HashMap<std::path::PathBuf, World>,
}

impl ContentCache {
    pub fn world(&mut self, db: &std::path::Path) -> Option<World> {
        if let Some(world) = self.worlds.get(db) {
            return Some(world.clone());
        }
        let world = load_world(db)?;
        self.worlds.insert(db.to_path_buf(), world.clone());
        Some(world)
    }
}
```

Rename the existing `locator` to `load_world`, change its parameter to `db: &std::path::Path`, and delete its first line `let db = content_path(profile);`. Its doc comment stays, with the first sentence changed to "Loads the world files for one content path: the graph, the spawn table and the content decoder." The `finish_locator` signature is unchanged.

- [ ] **Step 4: Implement `run` and the lobby, and rewire `play`**

Add to `tui.rs`, replacing the doc comment and signature of `play`:

```rust
/// Run the client until the operator quits. A loop of two states: the
/// lobby, which has no session, and `play`, which has one. A profile
/// with a host connects at once. A line that closes lands in the lobby
/// with the settings intact. The terminal is set up once, here, and put
/// back once, here.
pub async fn run(mut settings: crate::settings::Settings, mut capture: Option<Capture>) -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if key_tx.send(ev).is_err() {
                break;
            }
        }
    });
    let mut out = std::io::stdout();
    let (_, rows) = crossterm::terminal::size()?;
    setup_region(&mut out, rows)?;
    for warning in settings.warnings() {
        note(&mut out, &format!("-- {warning} --"))?;
    }
    let mut cache = ContentCache::default();
    let mut connect_now = !settings.profile().host.is_empty();
    let result = loop {
        if !connect_now {
            match lobby(&mut out, &mut key_rx, &mut settings).await {
                Ok(LobbyStep::Connect) => {}
                Ok(_) => break Ok(()),
                Err(e) => break Err(e),
            }
        }
        connect_now = false;
        let profile = settings.profile().clone();
        // A capture names two files and creating them truncates, so it
        // records the first connection only.
        let session = match Session::connect(&profile, capture.take()).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                note(&mut out, &format!("-- connect {}:{}: {e} --", profile.host, profile.port))?;
                continue;
            }
        };
        // Interactive play is not paced. `pace_ms` is flood control,
        // which is for automation. The session keeps the real profile,
        // so `/farm` can put its pace back.
        session.set_pace(std::time::Duration::ZERO);
        match play(session, &mut settings, &mut cache, &mut key_rx, &mut out).await {
            Ok(PlayEnd::Quit) => break Ok(()),
            Ok(PlayEnd::Closed) => note(&mut out, "-- disconnected; /connect to go back --")?,
            Err(e) => break Err(e),
        }
    };
    let (_, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let _ = out.write_all(b"\x1b[r");
    let _ = out.write_all(format!("\x1b[{rows};1H\r\n").as_bytes());
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
    result
}

/// The client with no session: an input line that takes the settings
/// commands, `/connect`, `/help` and `/quit`.
async fn lobby(
    out: &mut std::io::Stdout,
    key_rx: &mut tokio::sync::mpsc::UnboundedReceiver<TermEvent>,
    settings: &mut crate::settings::Settings,
) -> std::io::Result<LobbyStep> {
    let (mut cols, mut rows) = crossterm::terminal::size()?;
    setup_region(out, rows)?;
    let mut editor = InputEditor::new();
    let mut passthrough = false;
    let mut quit_armed = false;
    lobby_paint(out, settings, &editor, cols, rows)?;
    loop {
        let Some(ev) = key_rx.recv().await else {
            return Ok(LobbyStep::Quit);
        };
        match ev {
            TermEvent::Resize(w, h) => {
                cols = w;
                rows = h;
                setup_region(out, rows)?;
            }
            TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                let outcome = handle_key(&key, &mut editor, &mut passthrough, false);
                // There is no board to pass keys through to.
                passthrough = false;
                let (step, text) = lobby_step(outcome, settings, &mut quit_armed);
                if let Some(text) = text {
                    note(out, &text)?;
                }
                if step != LobbyStep::Stay {
                    return Ok(step);
                }
            }
            _ => {}
        }
        lobby_paint(out, settings, &editor, cols, rows)?;
    }
}

/// The lobby's bar and input line. The same two rows `play` paints,
/// with nothing to report but where `/connect` would go.
fn lobby_paint(
    out: &mut std::io::Stdout,
    settings: &crate::settings::Settings,
    editor: &InputEditor,
    cols: u16,
    rows: u16,
) -> std::io::Result<()> {
    let profile = settings.profile();
    let mut status = format!("not connected  {}:{}", profile.host, profile.port);
    if settings.dirty() {
        status.push_str("  unsaved");
    }
    let width = cols as usize;
    let status: String = status.chars().take(width).collect();
    let status = format!("{status:width$}");
    let status_row = rows.saturating_sub(1).max(1);
    let input_row = rows.max(1);
    let line = editor.line();
    let cursor_col = 3 + editor.cursor() as u16;
    out.write_all(
        format!(
            "\x1b[{status_row};1H\x1b[2K\x1b[7m{status}\x1b[0m\
             \x1b[{input_row};1H\x1b[2K> {line}\x1b[{input_row};{cursor_col}H"
        )
        .as_bytes(),
    )?;
    out.flush()
}
```

Then change `play`:

1. Signature: `async fn play(session: Arc<Session>, settings: &mut crate::settings::Settings, cache: &mut ContentCache, key_rx: &mut tokio::sync::mpsc::UnboundedReceiver<TermEvent>, out: &mut std::io::Stdout) -> std::io::Result<PlayEnd>`. Not `pub`.
2. Delete `crossterm::terminal::enable_raw_mode()?;`, the key reader thread, and `let mut out = std::io::stdout();`. Keep `let (mut cols, mut rows) = crossterm::terminal::size()?;` and the `setup_region` call.
3. Delete the four lines after the loop that reset the region and disable raw mode. `run` does that. The function ends with `result`.
4. Replace `finish_locator(locator(session.profile()), &session)` with `finish_locator(cache.world(&content_path(&session.profile())), &session)`.
5. Every `break Ok(())`: the `raw_rx` closed arm and the `changed.is_err()` arm become `break Ok(PlayEnd::Closed)`. The `key_rx` closed arm, `KeyOutcome::QuitNow`, and the map view's `ViewAction::Quit` arm become `break Ok(PlayEnd::Quit)`.
6. Every `&mut out` stays as written, since `out` is now the `&mut Stdout` parameter. Where the code passes `&mut out` to a function taking `&mut impl Write`, `out` reborrows and compiles. If a call site complains, write `&mut *out`.
7. Add `let mut quit_armed = false;` beside `let mut passthrough = false;`.
8. In the key arm, before the `match outcome`, add:

```rust
                        if !matches!(outcome, KeyOutcome::Quit | KeyOutcome::Continue) {
                            quit_armed = false;
                        }
                        if let Some(applied) = apply_settings(&outcome, settings) {
                            note(&mut out, &applied.note)?;
                            if applied.profile_changed {
                                session.set_profile(settings.profile().clone());
                            }
                            if applied.bot_changed {
                                assist_config = assist_config_for(settings.profile());
                                if assist.is_some() {
                                    let (bot, heal) = new_assist(&session, &assist_config);
                                    assist = Some(bot);
                                    assist_heal_state = Some(heal);
                                    assist_watch = crate::farm::HealWatch::new(&assist_config, &crate::farm::FarmConfig::default());
                                    assist_book_seen = usize::MAX;
                                    note(&mut out, "-- bot assist rebuilt with the new settings --")?;
                                }
                            }
                        }
```

9. Replace the `-- not wired yet --` arm from Task 6 and the `Quit` arm with:

```rust
                            KeyOutcome::Quit => {
                                if settings.dirty() && !quit_armed {
                                    quit_armed = true;
                                    note(&mut out, "-- unsaved settings: /save, or /quit again --")?;
                                } else {
                                    break Ok(PlayEnd::Quit);
                                }
                            }
                            KeyOutcome::Disconnect => {
                                if let Some(j) = job.take() {
                                    j.handle.abort();
                                    phase_rx = None;
                                }
                                session.close();
                                break Ok(PlayEnd::Closed);
                            }
                            KeyOutcome::Connect { .. } => {
                                note(&mut out, "-- already connected: /disconnect first --")?;
                            }
                            KeyOutcome::SetList { .. }
                            | KeyOutcome::Set { .. }
                            | KeyOutcome::Unset { .. }
                            | KeyOutcome::Save { .. }
                            | KeyOutcome::Load { .. } => {}
```

10. The username gate. In the `StartFarm`, `Go` and `Bank` arms, and in the `ToggleAssist` arm's `if on` branch, wrap the existing body:

```rust
                                if let Err(why) = needs_username(&session.profile()) {
                                    note(&mut out, &format!("-- {why} --"))?;
                                } else {
                                    // the existing body, unchanged
                                }
```

For `ToggleAssist` the check goes before `let on = assist.take().is_none();`, and only when the assist is currently off: `if assist.is_none() && let Err(why) = needs_username(&session.profile()) { note; } else { existing body }`.

11. The `QuitNow` arm from Task 6 becomes `KeyOutcome::QuitNow => break Ok(PlayEnd::Quit),`.

Change `cli.rs`:

```rust
    /// Interactive session against a target board, or the lobby when no
    /// profile is given
    Play {
        /// Character profile (TOML). Without one the client starts in
        /// the lobby: `/connect host[:port]`, then `/save <file>`.
        #[arg(long)]
        profile: Option<PathBuf>,
        /// Capture basename: writes `<capture>.raw` and `<capture>_timing.log`
        #[arg(long)]
        capture: Option<PathBuf>,
    },
```

Change `mmc.rs`. In `main`: `Command::Play { profile, capture } => play_command(profile.as_deref(), capture.as_deref()),`. Replace `play_command`:

```rust
fn play_command(profile_path: Option<&std::path::Path>, capture: Option<&std::path::Path>) -> ExitCode {
    let settings = match profile_path {
        Some(path) => match mud_client::settings::Settings::load(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("profile {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => mud_client::settings::Settings::default(),
    };
    let capture = capture.map(|base| Capture {
        raw: base.with_extension("raw"),
        timing: Some(append_to_stem(base, "_timing.log")),
    });
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    rt.block_on(async {
        match mud_client::tui::run(settings, capture).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("terminal error: {e}");
                ExitCode::FAILURE
            }
        }
    })
}
```

Remove the now-unused `use std::sync::Arc;` and `use mud_client::session::Session;` from `mmc.rs` if the compiler says they are unused.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p mud-client --test tui` then `cargo test -p mud-client --test cli` then `cargo test -p mud-client` then `cargo clippy -p mud-client --all-targets`
Expected: all pass, no new warnings.

Then run the binary by hand once, from the repo root: `cargo run -p mud-client --bin mmc -- play`. Expect the lobby bar `not connected  :23`. Type `/set host` and see the row. Type `/quit`. Report what you saw.

Mutation check: in `lobby_step`, delete the `if settings.profile().host.is_empty()` refusal. `the_lobby_refuses_to_connect_nowhere_and_refuses_jobs` fails. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src crates/mud-client/tests
git commit -m "feat(client): a lobby before the connection, /connect and /disconnect"
```

---

### Task 10: Documentation

**Files:**
- Modify: `docs/mud-client.md`
- Modify: `docs/superpowers/specs/2026-09-06-live-settings-and-lobby-design.md` (one paragraph, see step 2)

- [ ] **Step 1: Write the doc**

In `docs/mud-client.md`:

1. In the command table near the top, change the `mmc play --profile P` row to `mmc play [--profile P]` with the text "Interactive terminal session. Without a profile, a lobby: `/connect host[:port]`, `/set`, then `/save <file>`."
2. After the `## Driving from inside \`play\`` section's existing subsections, add:

```markdown
## Live settings

Every profile key can be read and changed from inside the client, the way
irssi does it. The profile file is the source of truth: `/set` edits the
document, and `/save` writes it back with your comments and key order
intact.

| Command | Effect |
| --- | --- |
| `/set` | List every key with its value. The password shows as stars. |
| `/set <pattern>` | List the keys a glob matches. `*` matches anything and a trailing `*` is implied: `/set bot`, `/set bot.rest*`, `/set *heal*`. |
| `/set <key> <value>` | Change a key. The value is TOML: `60`, `true`, `["copper", "silver"]`. A bare word is a string, so `/set bot.rest_command rest` works. |
| `/unset <key>` | Remove a key so its default applies. The way back to none for `bank.at`, `farm.finish_at`, `farm.depart_at_percent` and `pace_ms`. |
| `/save [file]` | Write the settings. Started with `--profile`, no path is needed. Started bare, the first `/save` names the file and later ones remember it. |
| `/load <file>` | Replace the settings from a file. The connection stays open. |

Tab completes a slash verb or, after `/set` and `/unset`, a key:
`/set bot.ign<Tab>` gives `/set bot.ignore_coins`. Several candidates
grow to their common prefix, and a second Tab lists them.

When a change takes effect:

- A `bot.*` key rebuilds the assist on the spot when it is on.
- Everything a job reads, `[farm]`, `[bank]`, `pace_ms`, applies at the
  next `/farm`, `/go` or `/bank`. A running job keeps what it started
  with.
- Connection keys apply at the next `/connect`.

A refused value, a wrong type or a value the validator rejects, changes
nothing and says why. `/quit` with unsaved settings refuses once and
exits on the second `/quit`. Ctrl-Q exits without asking.

A profile with no `[bot]` table gives the assist attack and loot on. The
first `/set bot.anything` creates the table, and every other bot key then
takes its struct default, which is off for both. `/set bot` shows the
effective values either way.

## The lobby

`mmc play` with no `--profile` starts in the lobby: the input line with no
connection. It takes the settings commands, `/connect`, `/help` and
`/quit`. `/connect host[:port]` sets `host` and `port` and connects, port
23 by default, so a `/save <file>` afterwards keeps them. Started with a
profile whose host is set, the client connects at once.

Whenever the line closes, by the board or by `/disconnect`, the client
returns to the lobby with the settings intact. `/connect` with no
argument dials the current host and port.

Interactive play never logs in for you: you type the username and
password at the board's prompt. The `username` key still matters, because
the runner matches your own death line against it, so `/farm`, `/go`,
`/bank` and `/bot` refuse to start while it is empty and say which key to
set.
```

- [ ] **Step 2: Note the deferred struct in the spec**

In the spec, in the section "Lobby and connect", replace the paragraph that begins "The per-connection state in play, the assist, the job, the room fix" with:

```markdown
The per-connection state in play stays local to `play`, which is entered
once per connection, so it is rebuilt on every `/connect` as it stands.
Turning it into a struct is the windows design's first task, where the
window task needs it. The room database and graph are loaded through a
cache keyed by path, so that design can share one copy between sessions.
```

- [ ] **Step 3: Check the files end with a blank line and commit**

Run: `tail -c1 docs/mud-client.md | xxd | grep -q 0a && echo ok`
Expected: `ok`.

```bash
git add docs/mud-client.md docs/superpowers/specs/2026-09-06-live-settings-and-lobby-design.md
git commit -m "doc(client): live settings, completion and the lobby"
```

---

## Self-review notes

- Spec coverage: commands (Tasks 6, 8, 9), settings module (Tasks 2, 3, 4), session copy (Task 7), lobby and connect (Task 9), pure key handler (Task 6), completion (Tasks 5, 6), content cache (Task 9), tests as listed, docs (Task 10). The spec's per-connection struct is deferred to the windows design, recorded in Task 10.
- The content cache's hit path has no file-backed test, because no test builds a world database and none may read `re/`. The miss path is tested. The reviewer reads `ContentCache::world` for the hit.
- Names used across tasks: `Settings::{parse, load, save, set, unset, list, value, profile, dirty, path, text, warnings}`, `KEYS`, `glob_match`, `complete`, `Completion`, `common_prefix`, `InputEditor::replace`, `VERBS`, `KeyOutcome::{Send, Raw, Note, QuitNow, SetList, Set, Unset, Save, Load, Connect, Disconnect}`, `handle_key` without a session, `apply_settings`, `Applied`, `assist_config_for`, `needs_username`, `connect_target`, `lobby_step`, `LobbyStep`, `PlayEnd`, `ContentCache`, `World`, `run`, `Session::{profile, set_profile, close}`.
