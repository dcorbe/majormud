# Profile Names, Sneak Switch and Live Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A bare profile name resolves under `~/.config/mmc` and a save never overwrites a file the settings did not come from, `bot.auto_sneak` turns sneaking off, and a running job picks up a `/set` without a restart.

**Architecture:** One resolver in `profile.rs` turns a typed name into a path, and every site that takes a profile name calls it. The session's profile becomes a `tokio::sync::watch` channel, so a change is an event. A new `farm::Live` owns the configs a job runs under together with the receiver and the job's own derivation rule. The jobs' deep functions take `&mut Live` where they took `&BotConfig` and `&FarmConfig`, ask it to refresh at the top of every leg, pass and stop, and select on it where they wait on the board.

**Tech Stack:** Rust 2024 edition, tokio `watch` and `select!`, serde, toml and toml_edit, the scripted-board harnesses in `tests/farm_scripted.rs`, `tests/go_scripted.rs` and `tests/sneak.rs`.

**Spec:** `docs/plans/2026-09-07-settings-live-reload-design.md`

## Global Constraints

- Never run `rustfmt` or `cargo fmt`.
- Every file ends with a blank line.
- No em dashes, parentheses or semicolons in prose you write: doc comments, commit messages, the doc. Code punctuation is code.
- Commit messages carry a tag with the crate in brackets as the repo does: `feat(client): ...`, `refactor(client): ...`, `test(client): ...`, `doc(client): ...`. No attribution lines. Do not mention the plan or the spec in commit messages.
- No test may read `re/`. Fixtures are hand-built. A test that needs a file puts it under `env!("CARGO_TARGET_TMPDIR")`, which cargo sets for integration tests. Never `/tmp`. A test that saves to a scratch path removes that path first, because the target directory survives between runs and the overwrite guard in Task 2 refuses a leftover.
- Each task's test must fail before the implementation and pass after. Each task also runs the named mutation check: change the rule, watch the test fail, revert.
- Run the crate's tests with `cargo test -p mud-client` from the repo root. Build with `cargo build -p mud-client`. Pass `--test <file>` while iterating and run the whole crate once before each commit. `cargo clippy -p mud-client --all-targets` must add no warnings. The count today is 13.
- Working directory for every command below is the repo root `/home/daniel/majormud/majormud`.
- Tests that set process environment do not exist in this plan. Every rule that reads the environment has a pure twin that takes the values as arguments, and the tests use the twin.
- Names later plans depend on, verbatim: `Session::profile_changes(&self) -> tokio::sync::watch::Receiver<Profile>`, `BotConfig::auto_sneak: bool`, `NavConfig::sneak: bool`, `profile::resolve(arg: &str) -> PathBuf`, `profile::config_dir() -> PathBuf`, `farm::Live`, `farm::nav_config`.

---

## File Structure

- `crates/mud-client/src/profile.rs` (modify): `config_dir`, `config_dir_from`, `resolve`, `resolve_in`.
- `crates/mud-client/src/loops.rs` (modify): `dir` becomes `config_dir().join("loops")`.
- `crates/mud-client/src/settings.rs` (modify): the overwrite guard in `Settings::save`, `"bot.auto_sneak"` in `KEYS`.
- `crates/mud-client/src/tui.rs` (modify): `apply_settings_in` with `apply_settings` calling it under `config_dir()`, the `/new` arm resolves its name, `pace_on_change`, the four starters build a `Live`, `spawn_run` takes one.
- `crates/mud-client/src/cli.rs` (modify): doc comments on the three `--profile` arguments.
- `crates/mud-client/src/bin/mmc.rs` (modify): `main` resolves every `--profile`, `farm_command` builds `Live::fixed`.
- `crates/mud-client/src/window.rs` (modify): a profile change re-paces a running job.
- `crates/mud-client/src/bot.rs` (modify): `BotConfig::auto_sneak`, `Bot::reconfigure`.
- `crates/mud-client/src/nav.rs` (modify): `NavConfig::sneak`, the `Navigator` field, the early return in `arm_sneak`.
- `crates/mud-client/src/session.rs` (modify): the profile as a `watch::Sender`, `profile_changes`.
- `crates/mud-client/src/farm.rs` (modify): `nav_config`, `Derive`, `Live`, and `run_farm`, `farm_loop`, `travel`, `farm_stop` on `Live`. The stop pump's wait becomes a `select!` with a `changed` arm.
- `crates/mud-client/src/go.rs` (modify): `run_go` on `Live`.
- `crates/mud-client/src/bank.rs` (modify): `errand` and `run_bank` on `Live`.
- `docs/mud-client.md` (modify): the `/save` row, the "When a change takes effect" lists, `bot.auto_sneak`.
- Tests: `tests/profile.rs`, `tests/loops.rs`, `tests/settings.rs`, `tests/tui.rs`, `tests/sneak.rs`, `tests/farm.rs`, `tests/bot.rs`, `tests/farm_scripted.rs`, `tests/go_scripted.rs`, `tests/bank_scripted.rs`, `tests/farm_live.rs`, `tests/go_live.rs`, `tests/session_capabilities.rs`.

### The live reload structure

The jobs pass `&BotConfig` and `&FarmConfig` into `farm_loop`, `travel`, `farm_stop` and `bank::errand`, and each of those builds guards, bots and watches from them at entry. Replacing the two references with one `&mut Live` is the least invasive change that lets the configs move: every read becomes `live.bot` or `live.farm`, and the four functions gain one refresh point each.

`Live::refresh` rebuilds the configs when the receiver has changed and bumps a generation counter. Each function remembers the generation it built its guard, bot or watch from and rebuilds those when the counter moves. `Bot::reconfigure` swaps a bot's policy without clearing its latches, so a change mid fight does not forget the fight.

Where a change lands:

- `farm_loop`: at the top of each stop. The navigator, the heal and buff casts, the bank policy and the fight switch are rebuilt.
- `travel`: at the top of each pass of its retry loop. The guard and the sighting bot are rebuilt. A change during one `goto` lands when that walk returns, at the next pass or the next leg.
- `farm_stop`: at the top of each pass. The bot is reconfigured and the rest watch rebuilt. The pump's wait selects on `Live::changed`, so a change wakes it and the next pass sees it.
- `errand`: passes `Live` down to its two `travel` calls.

Structural inputs never come from `Live`: the plan and its stops, the go target, the bank target and the content path are read once at start.

---

### Task 1: One config directory and one profile resolver

**Files:**
- Modify: `crates/mud-client/src/profile.rs`
- Modify: `crates/mud-client/src/loops.rs:194-205`
- Test: `crates/mud-client/tests/profile.rs`, `crates/mud-client/tests/loops.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub fn profile::config_dir() -> PathBuf`
  - `pub fn profile::config_dir_from(xdg: Option<OsString>, home: Option<OsString>) -> PathBuf`
  - `pub fn profile::resolve(arg: &str) -> PathBuf`
  - `pub fn profile::resolve_in(dir: &Path, arg: &str) -> PathBuf`
  - `loops::dir()` returns `config_dir().join("loops")`, same value as before.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/profile.rs`:

```rust
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mud_client::profile::{config_dir_from, resolve_in};

/// The loop library and the profiles share one base, and it is the one
/// the loop library already used: XDG first, then the home directory,
/// then the current directory when the machine says nothing.
#[test]
fn the_config_directory_prefers_xdg_then_home() {
    assert_eq!(
        config_dir_from(Some(OsString::from("/xdg")), Some(OsString::from("/home/x"))),
        PathBuf::from("/xdg/mmc")
    );
    assert_eq!(
        config_dir_from(None, Some(OsString::from("/home/x"))),
        PathBuf::from("/home/x/.config/mmc")
    );
    assert_eq!(config_dir_from(None, None), PathBuf::from("./mmc"));
}

/// `/save beef` means the profile called beef, which lives beside the
/// other profiles.
#[test]
fn a_bare_name_is_a_profile_in_the_config_directory() {
    let dir = Path::new("/cfg/mmc");
    assert_eq!(resolve_in(dir, "beef"), PathBuf::from("/cfg/mmc/beef.toml"));
    assert_eq!(resolve_in(dir, "salad-healtest"), PathBuf::from("/cfg/mmc/salad-healtest.toml"));
}

/// Anything that looks like a path is one. A separator anywhere, or the
/// suffix, and the argument is used exactly as typed.
#[test]
fn a_path_or_a_toml_suffix_is_used_as_given() {
    let dir = Path::new("/cfg/mmc");
    assert_eq!(resolve_in(dir, "chars/dan.toml"), PathBuf::from("chars/dan.toml"));
    assert_eq!(resolve_in(dir, "./beef"), PathBuf::from("./beef"));
    assert_eq!(resolve_in(dir, "beef.toml"), PathBuf::from("beef.toml"));
    assert_eq!(resolve_in(dir, "/abs/beef"), PathBuf::from("/abs/beef"));
}
```

Append to `crates/mud-client/tests/loops.rs`:

```rust
/// The library moved onto the shared config base. Same place as before,
/// one function fewer that knows where that is.
#[test]
fn the_loop_library_sits_under_the_config_directory() {
    assert_eq!(
        mud_client::loops::dir(),
        mud_client::profile::config_dir().join("loops")
    );
    assert!(mud_client::loops::dir().ends_with("mmc/loops"));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test profile config_directory bare_name toml_suffix`
Expected: compile error, `profile` has no `config_dir_from` or `resolve_in`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/profile.rs`, add to the imports:

```rust
use std::ffi::OsString;
use std::path::{Path, PathBuf};
```

and after `impl Profile { ... }` add:

```rust
/// Where the client keeps what is not any one profile's business: the
/// profiles themselves, the loop library, and the death log. This is
/// `$XDG_CONFIG_HOME/mmc`, or `~/.config/mmc`.
pub fn config_dir() -> PathBuf {
    config_dir_from(std::env::var_os("XDG_CONFIG_HOME"), std::env::var_os("HOME"))
}

/// The rule behind [`config_dir`], with the environment passed in so a
/// test can say what it is without touching the process.
pub fn config_dir_from(xdg: Option<OsString>, home: Option<OsString>) -> PathBuf {
    let base = xdg
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("mmc")
}

/// A profile argument as a path. A bare name is a profile in the config
/// directory, so `beef` is `~/.config/mmc/beef.toml`. Anything with a
/// separator or the suffix is a path and is used as given.
pub fn resolve(arg: &str) -> PathBuf {
    resolve_in(&config_dir(), arg)
}

/// The rule behind [`resolve`], against a directory the caller names.
pub fn resolve_in(dir: &Path, arg: &str) -> PathBuf {
    let is_path = arg.contains('/')
        || arg.contains(std::path::MAIN_SEPARATOR)
        || arg.ends_with(".toml");
    if is_path {
        PathBuf::from(arg)
    } else {
        dir.join(format!("{arg}.toml"))
    }
}
```

In `crates/mud-client/src/loops.rs`, replace the body of `dir`:

```rust
/// Where the library lives: `loops` under the client's config
/// directory, beside the character profiles and never inside one.
pub fn dir() -> PathBuf {
    crate::profile::config_dir().join("loops")
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test profile --test loops`
Expected: all pass.

Mutation check: in `resolve_in` change `arg.ends_with(".toml")` to `false`. `a_path_or_a_toml_suffix_is_used_as_given` fails on `beef.toml`. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/profile.rs crates/mud-client/src/loops.rs crates/mud-client/tests/profile.rs crates/mud-client/tests/loops.rs
git commit -m "feat(client): a bare profile name names a file in the config directory"
```

---

### Task 2: A save refuses to overwrite a file it did not come from

**Files:**
- Modify: `crates/mud-client/src/settings.rs:184-215`
- Test: `crates/mud-client/tests/settings.rs`, `crates/mud-client/tests/tui.rs:1161-1172`

**Interfaces:**
- Consumes: nothing new.
- Produces: `Settings::save` returns `Err` for a path that exists and is not `self.path`. The message is `"<path> exists and these settings were not loaded from it. /load it first, or pick another name."`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/settings.rs`:

```rust
/// The stray `beef` in the repo root was a bare lobby with three keys.
/// Under the resolver alone that save would have replaced the real
/// profile, password and all. A file the settings did not come from is
/// somebody's, and a save onto it is refused.
#[test]
fn a_save_onto_a_file_the_settings_did_not_come_from_is_refused() {
    let path = scratch("theirs.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    let mut s = Settings::default();
    s.set("host", "\"127.0.0.1\"").unwrap();
    let err = s.save(Some(&path)).unwrap_err();
    assert!(
        err.starts_with(&format!("{} exists and these settings were not loaded from it.", path.display())),
        "{err}"
    );
    assert!(err.contains("/load it first, or pick another name."), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED, "untouched");
    assert!(s.dirty(), "nothing was saved");
}

/// The remembered path is the file these settings came from, so a save
/// onto it by name is the same as a bare save.
#[test]
fn a_save_onto_the_remembered_path_by_name_writes() {
    let path = scratch("mine.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    let mut s = Settings::load(&path).unwrap();
    s.set("port", "2400").unwrap();
    s.save(Some(&path)).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("port = 2400"));
}

/// A name nobody has used is free, and the save remembers it.
#[test]
fn a_save_onto_a_fresh_name_writes_and_remembers() {
    let path = scratch("fresh-name.toml");
    let _ = std::fs::remove_file(&path);
    let mut s = Settings::default();
    s.set("host", "\"127.0.0.1\"").unwrap();
    s.save(Some(&path)).unwrap();
    assert_eq!(s.path(), Some(path.as_path()));
    assert!(!s.dirty());
}
```

Three existing tests in `tests/settings.rs` save parsed text onto a file that exists and now have to load it first. Replace the body of `a_save_replaces_the_file_and_leaves_no_temporary` with:

```rust
    let path = scratch("replaced.toml");
    let temp = scratch("replaced.toml.tmp");
    std::fs::write(&path, "host = \"old\"\n").unwrap();
    let mut s = Settings::load(&path).unwrap();
    s.set("host", "\"new\"").unwrap();
    s.save(None).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("host = \"new\""));
    assert!(!temp.exists(), "the temporary is renamed away, not left behind");
```

In `a_failed_write_reports_the_profile_and_drops_the_temporary`, replace

```rust
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("port", "2400").unwrap();
    let err = s.save(Some(&path)).unwrap_err();
```

with

```rust
    let mut s = Settings::load(&path).unwrap();
    s.set("port", "2400").unwrap();
    let err = s.save(None).unwrap_err();
```

In `a_failed_save_leaves_the_old_file_whole`, replace

```rust
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("port", "2400").unwrap();
    let result = s.save(Some(&path));
```

with

```rust
    let mut s = Settings::load(&path).unwrap();
    s.set("port", "2400").unwrap();
    let result = s.save(None);
```

In `save_without_a_path_needs_one_once`, add `let _ = std::fs::remove_file(&path);` right after `let path = scratch("fresh.toml");`.

In `crates/mud-client/tests/tui.rs`, in `save_reports_the_path_or_asks_for_one`, add `let _ = std::fs::remove_file(&path);` right after `let path = dir.join("saved.toml");`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test settings did_not_come_from`
Expected: FAIL, `unwrap_err` on an `Ok`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/settings.rs`, in `save`, right after the `let path = match ... };` block and before the `temp_path` line, add:

```rust
        // A file these settings did not come from is somebody's. The
        // resolver in `profile` turns a bare name into a path beside
        // every other profile, which is exactly where an unrelated
        // character's file already sits.
        if path.exists() && self.path.as_deref() != Some(path.as_path()) {
            return Err(format!(
                "{} exists and these settings were not loaded from it. /load it first, or pick another name.",
                path.display()
            ));
        }
```

Update the doc comment on `save`:

```rust
    /// Write the document. `None` means the remembered path, and with
    /// none remembered the caller has to name one. A path given here is
    /// remembered. A path that exists and is not the remembered one is
    /// refused: it is another character's file until it is loaded.
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test settings --test tui`
Expected: all pass.

Mutation check: change `path.exists() &&` to `false &&`. The refusal test fails. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/settings.rs crates/mud-client/tests/settings.rs crates/mud-client/tests/tui.rs
git commit -m "fix(client): a save never overwrites a profile the settings were not loaded from"
```

---

### Task 3: Every site that takes a profile name resolves it

**Files:**
- Modify: `crates/mud-client/src/tui.rs:1148-1200` (`apply_settings`), `crates/mud-client/src/tui.rs:611-619` (`/new`)
- Modify: `crates/mud-client/src/bin/mmc.rs:10-36`
- Modify: `crates/mud-client/src/cli.rs`
- Modify: `docs/mud-client.md:571`
- Test: `crates/mud-client/tests/tui.rs`

**Interfaces:**
- Consumes: `profile::resolve`, `profile::resolve_in`, `profile::config_dir`.
- Produces: `pub fn tui::apply_settings_in(outcome: &KeyOutcome, settings: &mut Settings, dir: &Path) -> Option<Applied>`. `apply_settings` keeps its signature and calls it with `config_dir()`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/tui.rs`:

```rust
use mud_client::tui::apply_settings_in;

fn profiles_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("profiles").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `/save beef` writes beef.toml beside the other profiles, and
/// `/load beef` reads it back from there. The note names the file.
#[test]
fn a_bare_name_saves_and_loads_under_the_config_directory() {
    let dir = profiles_dir("bare");
    let mut s = Settings::default();
    s.set("username", "\"beef\"").unwrap();
    let saved = apply_settings_in(&KeyOutcome::Save { file: Some("beef".into()) }, &mut s, &dir).unwrap();
    let expected = dir.join("beef.toml");
    assert!(expected.exists(), "{}", saved.note);
    assert!(saved.note.contains(&expected.display().to_string()), "{}", saved.note);

    let mut other = Settings::default();
    let loaded = apply_settings_in(&KeyOutcome::Load { file: "beef".into() }, &mut other, &dir).unwrap();
    assert_eq!(other.profile().username, "beef");
    assert!(loaded.note.contains(&expected.display().to_string()), "{}", loaded.note);
}

/// A path is a path. The resolver leaves it alone.
#[test]
fn a_path_given_to_save_is_used_as_given() {
    let dir = profiles_dir("path");
    let elsewhere = dir.join("elsewhere").join("dan.toml");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    let mut s = Settings::default();
    s.set("username", "\"dan\"").unwrap();
    apply_settings_in(&KeyOutcome::Save { file: Some(elsewhere.display().to_string()) }, &mut s, &dir).unwrap();
    assert!(elsewhere.exists());
    assert!(!dir.join("dan.toml").exists());
}

/// The overwrite guard reaches the keyboard with the `save:` prefix
/// every other refusal carries.
#[test]
fn a_save_onto_another_characters_profile_is_refused_at_the_keyboard() {
    let dir = profiles_dir("guard");
    std::fs::write(dir.join("beef.toml"), "username = \"beef\"\n").unwrap();
    let mut s = Settings::default();
    s.set("username", "\"salad\"").unwrap();
    let refused = apply_settings_in(&KeyOutcome::Save { file: Some("beef".into()) }, &mut s, &dir).unwrap();
    assert!(refused.note.starts_with("-- save: "), "{}", refused.note);
    assert!(refused.note.contains("/load it first"), "{}", refused.note);
    assert_eq!(std::fs::read_to_string(dir.join("beef.toml")).unwrap(), "username = \"beef\"\n");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test tui bare_name_saves used_as_given at_the_keyboard`
Expected: compile error, no `apply_settings_in`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/tui.rs`, replace the signature and the `Save` and `Load` arms of `apply_settings`:

```rust
/// Run one settings command against the settings. `None` when the
/// outcome is not one. The note is ready to print. Profile names
/// resolve under the config directory.
pub fn apply_settings(outcome: &KeyOutcome, settings: &mut crate::settings::Settings) -> Option<Applied> {
    apply_settings_in(outcome, settings, &crate::profile::config_dir())
}

/// [`apply_settings`] against a directory the caller names, which is
/// how a test says where the profiles are without touching the
/// process environment.
pub fn apply_settings_in(
    outcome: &KeyOutcome,
    settings: &mut crate::settings::Settings,
    dir: &std::path::Path,
) -> Option<Applied> {
```

The body is the old body with these two arms:

```rust
        KeyOutcome::Save { file } => {
            let path = file.as_deref().map(|f| crate::profile::resolve_in(dir, f));
            match settings.save(path.as_deref()) {
                Ok(path) => said(format!("-- saved {} --", path.display())),
                Err(e) => said(format!("-- save: {e} --")),
            }
        }
        KeyOutcome::Load { file } => {
            let path = crate::profile::resolve_in(dir, file);
            match crate::settings::Settings::load(&path) {
                Ok(loaded) => {
                    let mut note = format!(
                        "-- loaded {}; connection keys apply at the next /connect --",
                        path.display()
                    );
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
```

In the `KeyOutcome::NewWindow { file }` arm around line 611, replace

```rust
                    Some(f) => crate::settings::Settings::load(std::path::Path::new(&f)),
```

with

```rust
                    Some(f) => crate::settings::Settings::load(&crate::profile::resolve(&f)),
```

In `crates/mud-client/src/bin/mmc.rs`, replace the three arms of `main` that carry a profile:

```rust
        Command::Play { profile, capture } => {
            let profile = profile.map(|p| mud_client::profile::resolve(&p.to_string_lossy()));
            play_command(profile.as_deref(), capture.as_deref())
        }
        Command::Run {
            script,
            profile,
            capture,
        } => run_command(
            &script,
            &mud_client::profile::resolve(&profile.to_string_lossy()),
            capture.as_deref(),
        ),
```

and

```rust
        Command::Farm {
            profile,
            capture,
            content,
            brief,
            quiet,
        } => farm_command(
            &mud_client::profile::resolve(&profile.to_string_lossy()),
            capture.as_deref(),
            content.as_deref(),
            brief,
            quiet,
        ),
```

In `crates/mud-client/src/cli.rs`, change the three `profile` doc comments. `Play`:

```rust
        /// Character profile: a name under ~/.config/mmc, or a path.
        /// Without one the client starts in the lobby: `/connect
        /// host[:port]`, then `/save <name>`.
```

`Run` and `Farm`:

```rust
        /// Character profile: a name under ~/.config/mmc, or a path
```

with `Farm` keeping its `; needs a [farm] table` tail.

In `docs/mud-client.md`, replace the `/save` and `/load` rows:

```markdown
| `/save [name]` | Write the settings. Started with `--profile`, no name is needed. A bare name is a profile under `~/.config/mmc`, so `/save beef` writes `~/.config/mmc/beef.toml`. Anything with a slash or a `.toml` suffix is a path. A file these settings were not loaded from is refused: `/load` it first, or pick another name. |
| `/load <name>` | Replace the settings from a profile, named the same way. The connection stays open. |
```

and in the same section add one line after the table: `` `--profile` on `mmc play`, `mmc run` and `mmc farm` takes the same names. ``

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test tui --test cli`
Expected: all pass.

Mutation check: in the `Save` arm pass `file.as_deref().map(std::path::Path::new).map(Path::to_path_buf)` instead of resolving. `a_bare_name_saves_and_loads_under_the_config_directory` fails. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/src/bin/mmc.rs crates/mud-client/src/cli.rs crates/mud-client/tests/tui.rs docs/mud-client.md
git commit -m "feat(client): /save, /load, /new and --profile take a profile name"
```

---

### Task 4: `bot.auto_sneak`

**Files:**
- Modify: `crates/mud-client/src/bot.rs:185-200` (the field), `bot.rs:390-416` (the default)
- Modify: `crates/mud-client/src/settings.rs:26-32` (`KEYS`)
- Modify: `crates/mud-client/src/nav.rs:193-227` (`NavConfig`), `nav.rs:636-712` (`Navigator`), `nav.rs:2242-2250` (`arm_sneak`)
- Modify: `crates/mud-client/src/farm.rs` (`nav_config`, the walker in `farm_loop`), `crates/mud-client/src/go.rs` (`run_go`), `crates/mud-client/src/bank.rs` (`run_bank`)
- Modify: `docs/mud-client.md` (the `[bot]` example)
- Test: `crates/mud-client/tests/settings.rs`, `crates/mud-client/tests/sneak.rs`, `crates/mud-client/tests/farm.rs`, `crates/mud-client/tests/farm_scripted.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `BotConfig::auto_sneak: bool`, default true, serialised, key `bot.auto_sneak`.
  - `NavConfig::sneak: bool`, default true, `#[serde(skip)]`, so it is not a profile key.
  - `pub fn farm::nav_config(bot: &BotConfig, farm: &FarmConfig) -> NavConfig`.
  - `Navigator::arm_sneak` returns `Ok(false)` when `sneak` is off.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/settings.rs`:

```rust
/// The one way to walk without sneaking. A bot key, so it completes
/// under `/set bot.` and rebuilds the assist like the rest.
#[test]
fn bot_sneak_is_a_key_that_defaults_on() {
    assert!(KEYS.contains(&"bot.auto_sneak"));
    let s = Settings::default();
    assert_eq!(s.value("bot.auto_sneak").as_deref(), Some("true"));
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bot.auto_sneak", "false").unwrap();
    assert!(!s.profile().bot.as_ref().unwrap().sneak);
}
```

Append to `crates/mud-client/tests/sneak.rs`:

```rust
/// `bot.auto_sneak = false`. A stealthy character walks and never arms.
#[tokio::test]
async fn sneak_off_never_sends_sneak() {
    let (addr, log) = sneak_board("Attempting to sneak...", Board { arms: true, ..Board::default() }).await;
    let session = session_for(addr).await;
    let navigator = Navigator::new(
        graph_one_hop(),
        NavConfig { step_timeout_ms: 1500, sneak: false, ..NavConfig::default() },
    )
    .with_capabilities(Capabilities { stealth: 56, ..Capabilities::unrestricted() });
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(!arrival.sneaking);
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 0, "sneak off must not send sneak");
    assert_eq!(log.moves.load(Ordering::SeqCst), 1, "the walk still moves");
}
```

Append to `crates/mud-client/tests/farm.rs`:

```rust
/// The walk limits come from `[farm].nav` and the sneak switch from
/// `[bot]`. One function joins them, so no job can forget the switch.
#[test]
fn nav_config_copies_the_sneak_switch_from_the_bot_table() {
    let bot = BotConfig { sneak: false, ..BotConfig::default() };
    let farm = FarmConfig {
        nav: mud_client::nav::NavConfig { bash_doors: false, ..Default::default() },
        ..FarmConfig::default()
    };
    let nav = mud_client::farm::nav_config(&bot, &farm);
    assert!(!nav.sneak);
    assert!(!nav.bash_doors, "the farm's own nav limits still apply");
    let on = mud_client::farm::nav_config(&BotConfig::default(), &farm);
    assert!(on.sneak);
}
```

Append to `crates/mud-client/tests/farm_scripted.rs`, after `a_lap_of_quiet_stops_sneaks_once`:

```rust
/// The same lap with `bot.auto_sneak` off. The sheet says stealth 56 and the
/// run still never arms, because the switch reaches the walker through
/// the farm's nav config.
#[tokio::test]
async fn a_lap_with_sneak_off_never_arms() {
    let (addr, received) = scripted_board(vec![
        ("stat", NINJA_SHEET.into()),
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south"))),
        ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;
    assert_eq!(session.capabilities().stealth, 56, "the sheet must have been read");

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into(), "1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        sneak: false,
        ..BotConfig::default()
    };

    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the lap must finish: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap();
    assert_eq!(log.iter().filter(|l| *l == "n").count(), 2, "both legs walked: {log:?}");
    assert!(!log.iter().any(|l| l == "sneak"), "sneak off never arms: {log:?}");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test settings bot_sneak`
Expected: FAIL, `KEYS` does not contain `bot.auto_sneak`. The other three fail to compile.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/bot.rs`, after the `take_keys` field of `BotConfig`:

```rust
    /// Arm a sneak before walking, when the character has any stealth.
    /// Off means no walk sneaks and no stealth spell is cast for one.
    /// The recovery job refuses to start without it.
    pub sneak: bool,
```

and in `impl Default for BotConfig`, after `take_keys: true,`: `sneak: true,`.

In `crates/mud-client/src/settings.rs`, in `KEYS`, after `"bot.take_keys",` add `"bot.auto_sneak",`.

In `crates/mud-client/src/nav.rs`, add to `NavConfig` after `search_hidden`:

```rust
    /// Arm a sneak before a step. Copied from `bot.auto_sneak` by
    /// [`crate::farm::nav_config`] and not a profile key of its own:
    /// one switch, in the bot table, that every walker reads.
    #[serde(skip)]
    pub sneak: bool,
```

and in `impl Default for NavConfig` add `sneak: true,`. In `struct Navigator` add a field after `search_hidden`:

```rust
    /// `NavConfig::sneak`. Off is the operator's word, and it outranks
    /// the sheet's stealth.
    sneak: bool,
```

In `Navigator::new` add `sneak: cfg.sneak,` after `search_hidden: cfg.search_hidden,`. In `arm_sneak` replace

```rust
        if self.capabilities.stealth == 0 {
            return Ok(false);
        }
```

with

```rust
        if !self.sneak || self.capabilities.stealth == 0 {
            return Ok(false);
        }
```

In `crates/mud-client/src/farm.rs`, next to `check_departure_mark`, add:

```rust
/// The navigator's config for a job, from the two tables that own it:
/// the walk limits from `[farm].nav`, the sneak switch from `[bot]`.
pub fn nav_config(bot: &crate::bot::BotConfig, farm: &FarmConfig) -> crate::nav::NavConfig {
    crate::nav::NavConfig {
        sneak: bot.auto_sneak,
        ..farm.nav.clone()
    }
}
```

In `farm_loop`, in the `walker` closure, replace `crate::nav::Navigator::new(graph.clone(), cfg.nav.clone())` with `crate::nav::Navigator::new(graph.clone(), nav_config(bot_config, cfg))`. Note that at that point `bot_config` is still the borrowed parameter, which is what is wanted.

In `crates/mud-client/src/go.rs`, in `run_go`, replace `crate::nav::Navigator::new(graph.clone(), cfg.nav.clone())` with `crate::nav::Navigator::new(graph.clone(), crate::farm::nav_config(bot_config, cfg))`. In `crates/mud-client/src/bank.rs`, in `run_bank`, the same replacement.

In `docs/mud-client.md`, in the `[bot]` example after the `take_keys` line, add:

```toml
sneak = true               # arm a sneak before walking; false never sneaks
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test settings --test sneak --test farm --test farm_scripted --test profile`
Expected: all pass. `every_key_the_profile_serialises_is_in_keys` passes because `NavConfig::sneak` is skipped and `bot.auto_sneak` is in `KEYS`.

Mutation check: in `farm::nav_config` write `sneak: true` instead of `bot.auto_sneak`. `nav_config_copies_the_sneak_switch_from_the_bot_table` and `a_lap_with_sneak_off_never_arms` fail. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/bot.rs crates/mud-client/src/settings.rs crates/mud-client/src/nav.rs crates/mud-client/src/farm.rs crates/mud-client/src/go.rs crates/mud-client/src/bank.rs crates/mud-client/tests/settings.rs crates/mud-client/tests/sneak.rs crates/mud-client/tests/farm.rs crates/mud-client/tests/farm_scripted.rs docs/mud-client.md
git commit -m "feat(client): bot.auto_sneak turns sneaking off for every walk"
```

---

### Task 5: The session's profile is a watch channel, and a change re-paces a running job

**Files:**
- Modify: `crates/mud-client/src/session.rs:347` (the field), `session.rs:613` (construction), `session.rs:628-638` (`profile`, `set_profile`)
- Modify: `crates/mud-client/src/tui.rs` (`pace_on_change`)
- Modify: `crates/mud-client/src/window.rs:846-855`
- Test: `crates/mud-client/tests/tui.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `Session::profile_changes(&self) -> tokio::sync::watch::Receiver<Profile>`.
  - `pub fn tui::pace_on_change(job_running: bool, profile: &Profile) -> Option<Duration>`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/tui.rs`, after `jobs_need_a_name`:

```rust
/// A `/set` replaces the session's profile. That replacement is the
/// event a running job listens for.
#[tokio::test]
async fn a_profile_change_wakes_a_subscriber() {
    let addr = banner_board().await;
    let profile = Profile {
        host: addr.ip().to_string(),
        port: addr.port(),
        ..Default::default()
    };
    let session = Session::connect(&profile, None).await.unwrap();
    let mut changes = session.profile_changes();
    assert!(!changes.has_changed().unwrap(), "nothing has changed yet");
    session.set_profile(Profile { username: "dan".into(), ..session.profile() });
    tokio::time::timeout(Duration::from_secs(2), changes.changed())
        .await
        .expect("the change wakes the subscriber")
        .unwrap();
    assert_eq!(changes.borrow_and_update().username, "dan");
    assert_eq!(session.profile().username, "dan", "profile() reads the same value");
}

/// The writer reads the pace live. A change under a job follows at
/// once. With no job the operator's keystrokes stay unpaced.
#[test]
fn a_pace_change_reaches_the_writer_only_under_a_job() {
    let profile = Profile { pace_ms: Some(700), ..Default::default() };
    assert_eq!(pace_on_change(true, &profile), Some(Duration::from_millis(700)));
    assert_eq!(pace_on_change(false, &profile), None);
}
```

Add `pace_on_change` to the `use mud_client::tui::{...}` line at the top of the file, and `Duration` if `std::time::Duration` is not already imported there.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test tui wakes_a_subscriber reaches_the_writer`
Expected: compile error, no `profile_changes` and no `pace_on_change`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/session.rs`, change the field:

```rust
    /// The profile this session runs under. A watch channel, because a
    /// `/set` replacing it is the event a running job reloads on.
    profile: watch::Sender<Profile>,
```

In `connect`, change `profile: std::sync::RwLock::new(profile.clone()),` to `profile: watch::Sender::new(profile.clone()),`. Replace the two methods:

```rust
    /// The profile this session runs under. A clone: `/set` replaces it
    /// with [`Session::set_profile`], and a job reads it when it starts
    /// and again through [`Session::profile_changes`] while it runs.
    pub fn profile(&self) -> Profile {
        self.profile.borrow().clone()
    }

    /// Replace the profile. Every job holding a receiver from
    /// [`Session::profile_changes`] wakes.
    pub fn set_profile(&self, profile: Profile) {
        self.profile.send_replace(profile);
    }

    /// The event a job reloads its settings on. The receiver reports
    /// the current profile on `borrow` and wakes on every replacement.
    pub fn profile_changes(&self) -> watch::Receiver<Profile> {
        self.profile.subscribe()
    }
```

In `crates/mud-client/src/tui.rs`, next to `assist_config_for`:

```rust
/// The pace a settings change hands the writer. A running job is under
/// the profile's pace and follows a change at once. With no job the
/// operator's keystrokes stay unpaced, whatever the profile says.
pub fn pace_on_change(job_running: bool, profile: &crate::profile::Profile) -> Option<std::time::Duration> {
    job_running.then(|| profile.pace())
}
```

In `crates/mud-client/src/window.rs`, in the `WindowMsg::Outcome` arm, replace

```rust
                            if applied.profile_changed {
                                session.set_profile(w.settings.profile().clone());
                            }
```

with

```rust
                            if applied.profile_changed {
                                session.set_profile(w.settings.profile().clone());
                                if let Some(pace) = pace_on_change(job.is_some(), w.settings.profile()) {
                                    session.set_pace(pace);
                                }
                            }
```

and add `pace_on_change` to the `use crate::tui::{...}` import at the top of `window.rs`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test tui --test session_close --test session_capabilities`
Expected: all pass.

Mutation check: make `set_profile` a no-op. `a_profile_change_wakes_a_subscriber` fails on the timeout. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/session.rs crates/mud-client/src/tui.rs crates/mud-client/src/window.rs crates/mud-client/tests/tui.rs
git commit -m "feat(client): the session's profile is a watch channel a job can subscribe to"
```

---

### Task 6: `farm::Live` and `Bot::reconfigure`

**Files:**
- Modify: `crates/mud-client/src/farm.rs` (after `Notices` and `set_phase`)
- Modify: `crates/mud-client/src/bot.rs` (after `with_refusals`)
- Test: `crates/mud-client/tests/farm.rs`, `crates/mud-client/tests/bot.rs`

**Interfaces:**
- Consumes: `Session::profile_changes`, `Notices`.
- Produces:
  - `pub type farm::Derive = Arc<dyn Fn(&Profile) -> (BotConfig, FarmConfig) + Send + Sync>`.
  - `pub struct farm::Live { pub bot: BotConfig, pub farm: FarmConfig, .. }`.
  - `Live::new(session: &Session, what: &'static str, notices: Notices, bot: BotConfig, farm: FarmConfig, derive: Derive) -> Live`.
  - `Live::over(rx: watch::Receiver<Profile>, what, notices, bot, farm, derive) -> Live`.
  - `Live::fixed(bot: BotConfig, farm: FarmConfig) -> Live`, a receiver that never fires.
  - `Live::refresh(&mut self) -> bool`, `Live::generation(&self) -> u64`, `Live::profile(&self) -> Profile`, `Live::learned_vitals(&mut self, max_hp: i32, max_mana: i32)`, `async fn Live::changed(&mut self)`.
  - `Bot::reconfigure(&mut self, config: BotConfig)`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/farm.rs`:

```rust
use mud_client::farm::Live;
use mud_client::profile::Profile;

fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

fn collected() -> (mud_client::farm::Notices, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = std::sync::Arc::clone(&said);
    (
        std::sync::Arc::new(move |line: &str| into.lock().unwrap().push(line.to_string())),
        said,
    )
}

/// A go keeps combat off whatever the profile says. The derivation is
/// the job's, applied to every profile that arrives.
fn derive_like_a_go() -> mud_client::farm::Derive {
    std::sync::Arc::new(|p: &Profile| {
        let bot = BotConfig {
            auto_combat: false,
            ..p.bot.clone().unwrap_or_default()
        };
        (bot, p.farm.clone().unwrap_or_default())
    })
}

#[test]
fn refresh_is_false_until_the_profile_changes_and_true_once_after() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    assert!(!live.refresh());
    assert_eq!(live.generation(), 0);
    tx.send(Profile { bot: Some(BotConfig { ignore_coins: vec!["copper".into()], ..Default::default() }), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert_eq!(live.generation(), 1);
    assert_eq!(live.bot.ignore_coins, vec!["copper".to_string()]);
    assert!(!live.refresh(), "one change, one rebuild");
    assert_eq!(live.generation(), 1);
}

#[test]
fn a_rebuild_applies_the_jobs_forced_fields() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "go", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    tx.send(Profile { bot: Some(BotConfig { auto_combat: true, ..Default::default() }), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert!(!live.bot.auto_combat, "a go never fights, whatever the profile says");
}

#[test]
fn a_rebuild_keeps_discovered_vitals_when_the_profile_does_not_say() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    live.learned_vitals(120, 40);
    assert_eq!((live.bot.max_hp, live.bot.max_mana), (120, 40));
    tx.send(Profile { bot: Some(BotConfig::default()), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert_eq!((live.bot.max_hp, live.bot.max_mana), (120, 40), "the board's answer outlives a rebuild");
    tx.send(Profile { bot: Some(BotConfig { max_hp: 200, max_mana: 50, ..Default::default() }), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert_eq!((live.bot.max_hp, live.bot.max_mana), (200, 50), "a profile that says wins");
}

#[test]
fn the_reloaded_line_prints_once_per_change() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let (notices, said) = collected();
    let mut live = Live::over(rx, "farm", notices, BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    tx.send(Profile::default()).unwrap();
    live.refresh();
    live.refresh();
    assert_eq!(said.lock().unwrap().as_slice(), ["-- farm: settings reloaded --"]);
}

#[tokio::test]
async fn changed_resolves_after_a_send_and_a_fixed_live_never_fires() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(Profile::default()).unwrap();
    });
    tokio::time::timeout(Duration::from_secs(2), live.changed()).await.expect("a send wakes changed");
    assert!(live.refresh());

    let mut fixed = Live::fixed(BotConfig::default(), FarmConfig::default());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), fixed.changed()).await.is_err(),
        "a fixed live never wakes"
    );
    assert!(!fixed.refresh());
}
```

`tests/farm.rs` already imports `BotConfig`, `FarmConfig` and `Duration`. Append to `crates/mud-client/tests/bot.rs`:

```rust
/// A settings change mid stop must not forget the fight. The policy
/// moves, the latch stays.
#[test]
fn reconfigure_keeps_the_engaged_latch_and_changes_the_policy() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["kobold thief"]));
    assert_eq!(bot.engaged(), Some("kobold thief"));
    assert!(bot.wants_coin("copper"));
    bot.reconfigure(BotConfig {
        auto_combat: true,
        ignore_coins: vec!["copper".into()],
        ..BotConfig::default()
    });
    assert_eq!(bot.engaged(), Some("kobold thief"), "the fight is still on");
    assert!(!bot.wants_coin("copper"), "the new policy applies");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test farm refresh_is_false --test bot reconfigure`
Expected: compile error, no `Live` and no `reconfigure`.

- [ ] **Step 3: Implement**

In `crates/mud-client/src/farm.rs`, after `set_phase`, add:

```rust
/// What a job derives from a profile: its bot table and its farm table
/// with the job's own forced fields applied. A go keeps combat off and
/// its walk mode, a bank keeps looting off, whatever the profile says.
pub type Derive = std::sync::Arc<
    dyn Fn(&crate::profile::Profile) -> (crate::bot::BotConfig, FarmConfig) + Send + Sync,
>;

/// A job's settings, kept current while it runs.
///
/// The window replaces the session's profile on every `/set`, `/unset`
/// and `/load`. This holds the receiver, the configs the job runs
/// under, and the rule that derives one from the other. A job asks
/// [`Live::refresh`] at every decision that reads a config and selects
/// on [`Live::changed`] where it waits on the board, so a change lands
/// at the next step, pass or pickup and never part way through one.
///
/// The generation counter is for the guards, bots and watches a job
/// builds from the configs: each remembers the generation it was built
/// at and rebuilds when the counter moves, whichever caller did the
/// refresh.
pub struct Live {
    rx: tokio::sync::watch::Receiver<crate::profile::Profile>,
    derive: Derive,
    pub bot: crate::bot::BotConfig,
    pub farm: FarmConfig,
    /// What `discover_vitals` found, reapplied to every rebuild whose
    /// profile does not say.
    vitals: Option<(i32, i32)>,
    generation: u64,
    what: &'static str,
    notices: Notices,
    /// `fixed` keeps its own sender so the receiver never reports a
    /// closed channel.
    _pinned: Option<tokio::sync::watch::Sender<crate::profile::Profile>>,
}

impl Live {
    /// The live settings for a job on this session. `bot` and `farm`
    /// are what the job starts with, which for a named loop is not what
    /// `derive` would make of the profile.
    pub fn new(
        session: &crate::session::Session,
        what: &'static str,
        notices: Notices,
        bot: crate::bot::BotConfig,
        farm: FarmConfig,
        derive: Derive,
    ) -> Live {
        Live::over(session.profile_changes(), what, notices, bot, farm, derive)
    }

    /// As [`Live::new`], over a receiver the caller holds the sender
    /// of. What a test uses to change the settings under a job.
    pub fn over(
        rx: tokio::sync::watch::Receiver<crate::profile::Profile>,
        what: &'static str,
        notices: Notices,
        bot: crate::bot::BotConfig,
        farm: FarmConfig,
        derive: Derive,
    ) -> Live {
        Live {
            rx,
            derive,
            bot,
            farm,
            vitals: None,
            generation: 0,
            what,
            notices,
            _pinned: None,
        }
    }

    /// Settings that never change: the headless commands, and every
    /// test that is not about reloading.
    pub fn fixed(bot: crate::bot::BotConfig, farm: FarmConfig) -> Live {
        let (tx, rx) = tokio::sync::watch::channel(crate::profile::Profile::default());
        let (b, f) = (bot.clone(), farm.clone());
        let mut live = Live::over(
            rx,
            "job",
            std::sync::Arc::new(|_: &str| {}),
            bot,
            farm,
            std::sync::Arc::new(move |_: &crate::profile::Profile| (b.clone(), f.clone())),
        );
        live._pinned = Some(tx);
        live
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The profile as it stands, for the tables `derive` does not
    /// cover, like `[bank]`.
    pub fn profile(&self) -> crate::profile::Profile {
        self.rx.borrow().clone()
    }

    /// What the board said the pools are. Applied now and to every
    /// rebuild whose profile leaves `max_hp` at zero.
    pub fn learned_vitals(&mut self, max_hp: i32, max_mana: i32) {
        self.vitals = Some((max_hp, max_mana));
        self.bot.max_hp = max_hp;
        self.bot.max_mana = max_mana;
    }

    /// Rebuild the configs if the profile has changed since the last
    /// look. True when it did. One relaxed load when it did not.
    pub fn refresh(&mut self) -> bool {
        if !self.rx.has_changed().unwrap_or(false) {
            return false;
        }
        let profile = self.rx.borrow_and_update().clone();
        let (bot, farm) = (self.derive)(&profile);
        self.bot = bot;
        self.farm = farm;
        if let Some((hp, mana)) = self.vitals
            && self.bot.max_hp == 0
        {
            self.bot.max_hp = hp;
            self.bot.max_mana = mana;
        }
        self.generation += 1;
        (self.notices)(&format!("-- {}: settings reloaded --", self.what));
        true
    }

    /// Resolves when the profile changes. Never, once the sender is
    /// gone: a select arm that fired forever would spin the pump.
    pub async fn changed(&mut self) {
        if self.rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
```

In `crates/mud-client/src/bot.rs`, after `with_refusals`:

```rust
    /// Swap the policy and keep the memory. A settings change mid stop
    /// must not forget the fight in progress: a fresh bot reads an
    /// empty latch as an idle room, and the stop walks out on a monster
    /// still standing in it.
    pub fn reconfigure(&mut self, config: BotConfig) {
        self.config = config;
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test farm --test bot`
Expected: all pass.

Mutation check: in `refresh` drop the `vitals` block. `a_rebuild_keeps_discovered_vitals_when_the_profile_does_not_say` fails. Revert. Second mutation: make `reconfigure` build a fresh `Bot` and assign `*self`. The bot test fails on the latch. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/farm.rs crates/mud-client/src/bot.rs crates/mud-client/tests/farm.rs crates/mud-client/tests/bot.rs
git commit -m "feat(client): a job's settings are a Live value that rebuilds on a profile change"
```

---

### Task 7: Every job runs on a `Live`

This task changes signatures and call sites only. No job behaves differently yet. Every existing test passes at the end of it with `Live::fixed` where it passed two references.

**Files:**
- Modify: `crates/mud-client/src/farm.rs` (`run_farm`, `farm_loop`, `travel`, `farm_stop`)
- Modify: `crates/mud-client/src/go.rs` (`run_go`)
- Modify: `crates/mud-client/src/bank.rs` (`errand`, `run_bank`)
- Modify: `crates/mud-client/src/tui.rs` (`start_farm`, `start_roam`, `spawn_run`, `start_go`, `start_bank`)
- Modify: `crates/mud-client/src/bin/mmc.rs` (`farm_command`)
- Modify: `crates/mud-client/tests/farm_scripted.rs`, `tests/go_scripted.rs`, `tests/bank_scripted.rs`, `tests/farm_live.rs`, `tests/go_live.rs`, `tests/session_capabilities.rs`
- Test: the existing suites.

**Interfaces:**
- Consumes: `Live`, `nav_config`.
- Produces:
  - `pub async fn run_farm(session, graph: Arc<RoomGraph>, plan: &FarmPlan, live: Live, phase: PhaseSink<'_>, notices: &Notices) -> Result<(FarmEnd, FarmStats), FarmError>`
  - `pub async fn run_go(session, graph, hint: Option<RoomId>, to: RoomId, live: Live, phase, notices) -> Result<GoEnd, FarmError>`
  - `pub async fn run_bank(session, graph, hint: Option<RoomId>, live: Live, phase, notices) -> Result<ErrandEnd, FarmError>`
  - `pub(crate) async fn travel(session, nav, graph, current: &mut RoomId, stop: RoomId, live: &mut Live, threat, refusals, casts, clock, started, stats, phase, sneaking: bool)`
  - `async fn farm_stop(session, nav, graph, stop, live: &mut Live, threat, refusals, casts, clock, started, until, defending, arrival, sneaking, restore_weapon, stats, phase)`
  - `pub(crate) async fn errand(session, nav, graph, content, bank: &BankConfig, live: &mut Live, threat, refusals, casts, clock, started, stats, phase, notices, current, return_to)`
  - `fn spawn_run(session, graph, plan, live: Live, what, notices) -> Job`

- [ ] **Step 1: Change the signatures in `farm.rs`**

`run_farm`: replace the `bot_config: &crate::bot::BotConfig, cfg: &FarmConfig,` parameters with `live: Live,`. At the top of the body add `let mut live = live;` and then `let (bot_config, cfg) = (&live.bot, &live.farm);` is not possible across the later `&mut` use, so instead replace every read in `run_farm`'s body: `cfg` becomes `&live.farm` and `bot_config` becomes `&live.bot`. The body reads them in `check_departure_mark`, `travel_fights().set`, `deaths::init`, `load_spell_durations`, the `buffs.is_empty()` check, `sheet_from`, the three heal notices and the `minor_heal_at_percent` checks. The call to `farm_loop` becomes:

```rust
    let out = farm_loop(
        session, graph, plan, &mut live, phase, notices, &mut casts, &mut clock, &durations,
    )
    .await;
```

`farm_loop`: signature becomes

```rust
#[allow(clippy::too_many_arguments)]
async fn farm_loop(
    session: &crate::session::Session,
    graph: std::sync::Arc<RoomGraph>,
    plan: &FarmPlan,
    live: &mut Live,
    phase: PhaseSink<'_>,
    notices: &Notices,
    casts: &mut Casts,
    clock: &mut crate::world::RoundClock,
    durations: &BTreeMap<String, u32>,
) -> Result<(FarmEnd, FarmStats), FarmError> {
```

`durations` is unused until Task 8. Name it `_durations` for now so clippy stays quiet, and rename it in Task 8. In the body:

- `content_for(session, cfg, notices)` becomes `content_for(session, &live.farm, notices)`.
- The `walker` closure becomes a function of its config: `let walker = |nav_cfg: crate::nav::NavConfig| { let nav = crate::nav::Navigator::new(graph.clone(), nav_cfg).with_capabilities(session.capabilities()); ... }` and the two calls become `walker(nav_config(&live.bot, &live.farm))`.
- `load_threat(&cfg.content)` becomes `load_threat(&live.farm.content)`.
- The `let mut bot_config = bot_config.clone(); if bot_config.max_hp == 0 && let Some(vitals) = ...` block becomes:

```rust
    if live.bot.max_hp == 0
        && let Some(vitals) = discover_vitals(session).await
    {
        live.learned_vitals(vitals.max_hp, vitals.max_mana);
    }
```

- `time_up(started, cfg)` becomes `time_up(started, &live.farm)`.
- The `travel(...)` call passes `live` in place of `cfg, &bot_config`, so the argument list is `session, &nav, &graph, &mut current, stop, live, &threat, &refusals, casts, clock, started, &mut stats, phase, sneaking`.
- `cfg.stop_seconds` becomes `live.farm.stop_seconds`.
- The `farm_stop(...)` call passes `live` in place of `&bot_config` and drops the later `cfg` argument, so the argument list is `session, &nav, &graph, stop, live, &threat, &refusals, casts, clock, started, until, false, arrival, arm.0, arm.1, &mut stats, phase`.
- The `crate::bank::errand(...)` call passes `live` in place of `cfg, &bot_config`, so the list is `session, &bank_nav, &graph, content, &bank_cfg, live, &threat, &refusals, casts, clock, started, &mut stats, phase, notices, &mut current, roam.is_some().then_some(stop)`.
- `cfg.loops` becomes `live.farm.loops`.

`travel`: replace `cfg: &FarmConfig, bot_config: &crate::bot::BotConfig,` with `live: &mut Live,`. At the top of the body, before the guard is built, add `let bot_config = live.bot.clone(); let cfg = live.farm.clone();` and change every `bot_config.` and `cfg.` read to use these locals. The three `farm_stop(...)` calls pass `live` in place of `bot_config` and drop `cfg`, so each list reads `session, nav, graph, <room>, live, threat, refusals, casts, clock, started, Some(until), true, <arrival>, <sneaking>, <restore>, stats, phase`. `wait_for_departure_health(session, cfg, bot_config, &sight)` becomes `wait_for_departure_health(session, &cfg, &bot_config, &sight)`. `time_up(started, cfg)` becomes `time_up(started, &cfg)`.

`farm_stop`: replace `bot_config: &crate::bot::BotConfig,` with `live: &mut Live,` and delete the `cfg: &FarmConfig,` parameter. At the top of the body add `let bot_config = live.bot.clone(); let cfg = live.farm.clone();` and change every `bot_config` and `cfg` read to the locals, passing `&bot_config` and `&cfg` where a reference is wanted: `HealWatch::new(&bot_config, &cfg)` twice, `StopState::new(stop_name.clone(), &cfg)` twice, `wait_for_departure_health(session, &cfg, &bot_config, &flee_sight)`, `crate::bot::heal_need(&bot_config, percent)`, `time_up(started, &cfg)`.

- [ ] **Step 2: Change `go.rs` and `bank.rs`**

`run_go`: replace `bot_config: &crate::bot::BotConfig, cfg: &FarmConfig,` with `live: Live,`, add `let mut live = live;` first, and rewrite the body reads: `check_departure_mark(&live.farm, &live.bot)`, `travel_fights().set(live.farm.fight_while_travelling)`, `deaths::init(&live.farm.content)`, `content_for(session, &live.farm, notices)`, `Navigator::new(graph.clone(), crate::farm::nav_config(&live.bot, &live.farm))`, the vitals block becomes the `learned_vitals` form from Step 1, `load_threat(&live.farm.content)`, `sheet_from(session, &live.bot, &Default::default())`. The `travel(...)` call passes `&mut live` in place of `cfg, &bot_config`.

`run_bank`: the same rewrite. The `errand(...)` call passes `&mut live` in place of `cfg, &bot_config`.

`errand`: replace `cfg: &FarmConfig, bot_config: &crate::bot::BotConfig,` with `live: &mut Live,`. Its two `travel(...)` calls pass `live` in place of `cfg, bot_config`. It reads nothing else from them.

Add `use crate::farm::Live;` to `bank.rs` if `Live` is not reachable through the existing `crate::farm::` imports.

- [ ] **Step 3: Change the starters in `tui.rs` and the headless farm**

`spawn_run`: replace `bot: crate::bot::BotConfig, cfg: crate::farm::FarmConfig,` with `live: crate::farm::Live,` and the `run_farm` call passes `live` in place of `&bot, &cfg`.

`start_farm`: replace the last line with

```rust
    let live = crate::farm::Live::new(
        &session,
        "farm",
        notices.clone(),
        bot,
        cfg,
        std::sync::Arc::new(|p: &crate::profile::Profile| {
            (p.bot.clone().unwrap_or_default(), p.farm.clone().unwrap_or_default())
        }),
    );
    Ok(spawn_run(session, graph, plan, live, "farm", notices))
```

`start_roam`: the same, with `"roam"`.

`start_go`: build the live before the spawn:

```rust
    let live = crate::farm::Live::new(
        &session,
        "go",
        notices.clone(),
        bot,
        cfg,
        std::sync::Arc::new(move |p: &crate::profile::Profile| {
            let base = p.farm.clone().unwrap_or_else(|| crate::farm::FarmConfig {
                content: content_path(p),
                ..Default::default()
            });
            (assist_config_for(p), crate::go::go_config(&base, walking))
        }),
    );
```

and the `run_go` call passes `live` in place of `&bot, &cfg`. `start_bank`: the same with `"bank"` and `run_bank`.

In `crates/mud-client/src/bin/mmc.rs`, in `farm_command`, the `run_farm` call passes `mud_client::farm::Live::fixed(bot_config.clone(), farm_config.clone())` in place of `&bot_config, &farm_config`.

- [ ] **Step 4: Change the test call sites**

Every `run_farm(<session>, <graph>, &plan, &<bot>, &<cfg>, <phase>, <notices>)` becomes `run_farm(<session>, <graph>, &plan, Live::fixed(<bot>.clone(), <cfg>.clone()), <phase>, <notices>)`. Every `run_go(<session>, <graph>, <hint>, <to>, &<bot>, &<cfg>, <phase>, <notices>)` becomes `run_go(<session>, <graph>, <hint>, <to>, Live::fixed(<bot>, <cfg>), <phase>, <notices>)` where the two were temporaries, or with `.clone()` where they are reused. `run_bank` likewise. Add `use mud_client::farm::Live;` to each file. The files and their counts:

| file | calls |
| --- | --- |
| `tests/farm_scripted.rs` | 21 |
| `tests/go_scripted.rs` | 4 |
| `tests/bank_scripted.rs` | 1 |
| `tests/farm_live.rs` | 5 |
| `tests/go_live.rs` | 3 |
| `tests/session_capabilities.rs` | 1 |

The `walk` helper in `tests/go_scripted.rs` becomes:

```rust
        run_go(&session, graph, Some(START), STOP, Live::fixed(bot(), cfg(walking)), None, &quiet()),
```

- [ ] **Step 5: Build and run everything**

Run: `cargo build -p mud-client && cargo clippy -p mud-client --all-targets 2>&1 | grep -c "^warning"`
Expected: builds, and the count is 13 or lower.

Run: `cargo test -p mud-client`
Expected: every test passes. Nothing behaves differently yet.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src crates/mud-client/tests
git commit -m "refactor(client): every job carries its settings as a Live value"
```

---

### Task 8: A running job picks up a change

**Files:**
- Modify: `crates/mud-client/src/farm.rs` (`farm_loop`, `travel`, `farm_stop`)
- Test: `crates/mud-client/tests/farm_scripted.rs`, `crates/mud-client/tests/go_scripted.rs`

**Interfaces:**
- Consumes: `Live::refresh`, `Live::generation`, `Live::changed`, `Live::profile`, `Bot::reconfigure`, `sheet_from`, `nav_config`.
- Produces: no new names. The behaviour the spec describes.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/farm_scripted.rs`:

```rust
use mud_client::farm::{Derive, Live};

/// The farm's own derivation: the profile's two tables as they are.
fn farm_derive() -> Derive {
    Arc::new(|p: &Profile| (p.bot.clone().unwrap_or_default(), p.farm.clone().unwrap_or_default()))
}

/// Wait until the board has seen `line`, then hand it the new profile.
/// The send lands while the run is between that line and the next.
fn change_after(
    received: Arc<Mutex<Vec<String>>>,
    line: &'static str,
    tx: tokio::sync::watch::Sender<Profile>,
    profile: Profile,
) {
    tokio::spawn(async move {
        loop {
            if received.lock().unwrap().iter().any(|l| l == line) {
                let _ = tx.send(profile);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

/// `/set bot.ignore_coins ["copper"]` between two stops. The first
/// stop's pile is swept, the second stop's is left, and nothing was
/// restarted.
#[tokio::test]
async fn a_coin_rule_changed_mid_run_holds_at_the_next_stop() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        (
            "n",
            format!("\r\nn{}", room_block_items("Inner Ward", &["49 copper farthings"], "north south")),
        ),
        (
            "get copper",
            "\r\nget copper\r\nYou picked up 49 copper farthings\r\n[HP=30/MA=0]:".into(),
        ),
        (
            "n",
            format!("\r\nn{}", room_block_items("Keep", &["12 copper farthings"], "south")),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into(), "1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        auto_get: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let (notices, said) = collected();
    let live = Live::over(rx, "farm", notices, bot.clone(), cfg.clone(), farm_derive());
    change_after(
        Arc::clone(&received),
        "get copper",
        tx,
        Profile {
            bot: Some(BotConfig { ignore_coins: vec!["copper".into()], ..bot.clone() }),
            farm: Some(cfg.clone()),
            ..Profile::default()
        },
    );

    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, live, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!("the run must finish: {e:?}\nboard received: {:?}", received.lock().unwrap()),
    };
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap();
    assert_eq!(
        log.iter().filter(|l| *l == "get copper").count(),
        1,
        "the second pile is copper and copper is now ignored: {log:?}"
    );
    assert_eq!(
        said.lock().unwrap().iter().filter(|l| l.as_str() == "-- farm: settings reloaded --").count(),
        1,
        "one change, one line: {:?}",
        said.lock().unwrap()
    );
}

/// `/set farm.fight_while_travelling false` after the first stop. The
/// next leg walks past a whiff instead of turning to fight.
#[tokio::test]
async fn the_fight_switch_changed_mid_run_holds_at_the_next_leg() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south"))),
        (
            "n",
            format!(
                "\r\nn\r\nThe giant rat swings at you but misses!\r\n{}",
                room_block("Keep", Some("giant rat"), "south")
            ),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into(), "1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        stop_seconds: 1,
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: false,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let live = Live::over(rx, "farm", quiet(), bot.clone(), cfg.clone(), farm_derive());
    change_after(
        Arc::clone(&received),
        "n",
        tx,
        Profile {
            bot: Some(bot.clone()),
            farm: Some(FarmConfig { fight_while_travelling: false, ..cfg.clone() }),
            ..Profile::default()
        },
    );

    let (end, _) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, live, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!("the run must finish: {e:?}\nboard received: {:?}", received.lock().unwrap()),
    };
    assert_eq!(end, FarmEnd::LoopsDone);
    assert!(
        !session.travel_fights().get(),
        "the switch every leg reads follows the profile"
    );
    let log = received.lock().unwrap();
    assert!(!log.iter().any(|l| l.starts_with("a ")), "nothing was fought: {log:?}");
}
```

The second test's bot has `auto_combat: false` so a sighting can never swing. What it pins is the switch: with the old value the whiff's `Sighted` interrupt would hand the room to a defence that looks and waits out `stop_seconds`, and with the new value the leg ignores it. Add `use mud_client::profile::Profile;` if the file's imports do not already carry it. They do, in the `session_with_bank` helper.

Append to `crates/mud-client/tests/go_scripted.rs`:

```rust
use mud_client::farm::Derive;
use mud_client::profile::Profile;

/// A go's target is an argument, not a setting. A profile change mid
/// walk changes the walk's settings and nothing about where it goes.
#[tokio::test]
async fn a_profile_change_mid_walk_leaves_the_target_alone() {
    let mut script = up_to_the_whiff();
    script.push(("n", format!("\r\nn{}", room_block("Keep", None, "south"))));
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let derive: Derive = Arc::new(|p: &Profile| {
        let base = p.farm.clone().unwrap_or_default();
        (BotConfig { auto_combat: false, ..p.bot.clone().unwrap_or_default() }, go_config(&base, false))
    });
    let live = Live::over(rx, "go", quiet(), bot(), cfg(false), derive);
    let log_for_change = Arc::clone(&received);
    tokio::spawn(async move {
        loop {
            if log_for_change.lock().unwrap().iter().any(|l| l == "n") {
                let _ = tx.send(Profile {
                    farm: Some(FarmConfig { start: "9/9".into(), finish_at: Some("9/9".into()), ..Default::default() }),
                    ..Profile::default()
                });
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let end = tokio::time::timeout(
        Duration::from_secs(30),
        run_go(&session, corridor(), Some(START), STOP, live, None, &quiet()),
    )
    .await
    .expect("run_go should finish, not hang")
    .unwrap();
    assert_eq!(end, GoEnd::Arrived(STOP), "log: {:?}", received.lock().unwrap());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p mud-client --test farm_scripted changed_mid_run --test go_scripted leaves_the_target_alone`
Expected: `a_coin_rule_changed_mid_run_holds_at_the_next_stop` FAILS with two `get copper` lines, because nothing refreshes yet. `the_fight_switch_changed_mid_run_holds_at_the_next_leg` FAILS on the switch. The go test passes already, and stays as the pin.

- [ ] **Step 3: Implement the refresh points**

In `farm_loop`, rename `_durations` to `durations`. Replace `let nav = match &plan.roam { ... };` and `let bank_nav = walker();` with `let mut` bindings, and add `let mut built_at = live.generation();` after them. Change `let bank_cfg = session.profile().bank;` to `let mut bank_cfg = session.profile().bank;`. At the top of the `for &stop in &lap {` body, before the `time_up` check, add:

```rust
            // A change lands here, at the stop boundary: the navigator,
            // the casts and the bank policy are rebuilt from the new
            // tables, and the fight switch every leg reads follows the
            // new value. Mid stop and mid leg have their own points.
            if live.refresh() || built_at != live.generation() {
                built_at = live.generation();
                nav = match &plan.roam {
                    Some(walls) => walker(nav_config(&live.bot, &live.farm)).fenced(walls.clone(), plan.start.map),
                    None => walker(nav_config(&live.bot, &live.farm)),
                };
                bank_nav = walker(nav_config(&live.bot, &live.farm));
                let sheet = sheet_from(session, &live.bot, durations);
                casts.heal = crate::sheet::HealState::new(sheet.heals.0);
                casts.buff = crate::sheet::BuffState::new(sheet.buffs.0);
                bank_cfg = live.profile().bank;
                session.travel_fights().set(live.farm.fight_while_travelling);
            }
```

The `walker` closure borrows `graph`, `session` and `content` immutably, which is compatible with the reassignment.

In `travel`, turn the guard and the sighting bot into rebuildable state. Replace `let mut guard = FarmGuard::new(...)...;` and `let sight = ...;` with a small builder closure and the two bindings:

```rust
    let build = |bot_config: &crate::bot::BotConfig, cfg: &FarmConfig| {
        let guard = FarmGuard::new(
            bot_config.max_hp,
            cfg.interrupt_at_percent,
            &session.character_name().unwrap_or_default(),
        )
        .sighting(
            crate::bot::Bot::with_refusals(bot_config.clone(), threat.clone(), refusals.clone())
                .with_pack(session.pack_handle()),
        )
        .follows(session.travel_fights().clone());
        let sight = crate::bot::Bot::with_refusals(bot_config.clone(), threat.clone(), refusals.clone())
            .with_pack(session.pack_handle());
        (guard, sight)
    };
    let (mut guard, mut sight) = build(&bot_config, &cfg);
    let mut built_at = live.generation();
```

and at the top of `travel`'s `loop {` add:

```rust
        // A change lands at the next pass: a fresh guard and sighting
        // bot from the new tables. The walk in flight when it arrived
        // finished on the old ones, which is the honest reading of
        // "the next decision that reads them".
        if live.refresh() || built_at != live.generation() {
            built_at = live.generation();
            bot_config = live.bot.clone();
            cfg = live.farm.clone();
            let (g, s) = build(&bot_config, &cfg);
            guard = g;
            sight = s;
        }
```

`bot_config` and `cfg` from Task 7 become `let mut`. The `build` closure captures `session`, `threat` and `refusals` by reference and takes the configs as arguments, so the reassignment of the locals is fine.

In `farm_stop`, after `let mut rest_watch = HealWatch::new(&bot_config, &cfg);` add `let mut built_at = live.generation();`, and make `bot_config` and `cfg` `let mut`. At the top of its `loop {` add:

```rust
        // A change lands at the next pass. The bot keeps its latches
        // and takes the new policy, and the rest watch takes the new
        // marks. The stop's dwell budgets were read at entry and stay.
        if live.refresh() || built_at != live.generation() {
            built_at = live.generation();
            bot_config = live.bot.clone();
            cfg = live.farm.clone();
            stop_config = crate::bot::BotConfig {
                auto_get: false,
                ..bot_config.clone()
            };
            bot.reconfigure(stop_config.clone());
            rest_watch = HealWatch::new(&bot_config, &cfg);
        }
```

`stop_config` becomes `let mut`, so the lag recovery lower down rebuilds its bot from the current policy too.

Then make the wait select on the change. Replace

```rust
        let cor = match tokio::time::timeout_at(wake, events.recv()).await {
```

with

```rust
        let waited = tokio::select! {
            r = tokio::time::timeout_at(wake, events.recv()) => r,
            // A change wakes the pump. The next pass reads it.
            _ = live.changed() => continue,
        };
        let cor = match waited {
```

leaving the match arms as they are.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test farm_scripted --test go_scripted --test bank_scripted --test farm`
Expected: all pass, the two new farm tests included.

Run: `cargo test -p mud-client`
Expected: all pass.

Mutation check: delete the `live.refresh() ||` from the `farm_loop` refresh condition and the `bot.reconfigure` line from `farm_stop`. `a_coin_rule_changed_mid_run_holds_at_the_next_stop` fails with two sweeps. Revert both. Second mutation: delete the `session.travel_fights().set(...)` line from the `farm_loop` refresh block. `the_fight_switch_changed_mid_run_holds_at_the_next_leg` fails. Revert.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/farm.rs crates/mud-client/tests/farm_scripted.rs crates/mud-client/tests/go_scripted.rs
git commit -m "feat(client): a running job picks up a settings change at its next stop, leg or pass"
```

---

### Task 9: The docs say which keys are live

**Files:**
- Modify: `docs/mud-client.md:585-591`

**Interfaces:** none.

- [ ] **Step 1: Rewrite the section**

Replace the "When a change takes effect" list with:

```markdown
When a change takes effect:

- A `bot.*` key rebuilds the assist on the spot when it is on.
- A running job picks up a change at its next decision that reads the
  key: the next step, the next pass of a stop, the next pickup. It says
  so on the screen: `-- farm: settings reloaded --`. Live keys are
  `bot.*`, `farm.rest_at_percent`, `farm.rest_until_percent`,
  `farm.mana_rest_at_percent`, `farm.fight_while_travelling`,
  `farm.travel_interrupts`, `farm.interrupt_at_percent`,
  `farm.defend_seconds`, `farm.nav.*`, `bank.*` except `bank.at`, and
  `pace_ms`.
- Fixed at a job's start, so a change waits for the next `/farm`, `/go`
  or `/bank`: the loop and its stops, `farm.start`, `farm.finish_at`,
  `farm.circuit`, the go target, `bank.at`.
- Connection keys apply at the next `/connect`.
```

- [ ] **Step 2: Check the file ends with a blank line and commit**

```bash
tail -c2 docs/mud-client.md | xxd -p
git add docs/mud-client.md
git commit -m "doc(client): which settings a running job picks up"
```

Expected: `0a0a` or `0a`, with the file's last character a newline.

---

## Self-review

**Spec coverage.**

- §1 resolver and base: Task 1. Every site: Task 3 covers `/save`, `/load`, `/new`, `--profile` on play, run and farm. Overwrite guard and message: Task 2. Tests listed in the spec: Tasks 1 to 3.
- §2 `bot.auto_sneak`: Task 4 covers the field, `KEYS`, `NavConfig.sneak`, `arm_sneak`, `nav_config` at every navigator built from the profile, and the three tests. `start_where` builds `NavConfig::default()` and does not sneak today, so it is unchanged.
- §3 the event: Task 5. The receiver in a job, the forced fields, the navigator rebuilt: Tasks 6 to 8. The wake points: `farm_stop` selects on `changed`, `travel` and `farm_loop` refresh at the top of a pass and a stop. The bank errand refreshes through the `travel` it calls. Live keys: the bot marks and spells through `reconfigure` and the casts rebuild, the ignore lists through `reconfigure`, `bot.auto_sneak` through the navigator rebuild, `farm.*` through the config clones, `bank.*` through `bank_cfg`, `pace_ms` through Task 5. Fixed keys: never read from `Live`. The notice: `Live::refresh`. Headless: `Live::fixed`. Tests: Task 8 and Task 5.

**Deviations from the spec, stated.** The spec asks for a `changed` arm at the walk's step wait. That wait lives inside `Navigator::goto`, which cannot rebuild the job's configs, so a change during one `goto` lands when it returns. The spec's go test that changes `farm.fight_while_travelling` mid walk is written against a farm instead, because a go's fight mode is the `/bot` toggle by design and its derivation forces it. The spec's go test that changes the target is written as a profile change that leaves the target alone, because the target is not a setting.

**Placeholder scan.** None.

**Type consistency.** `Live::over` takes `(rx, what, notices, bot, farm, derive)` in Tasks 6 and 8. `Live::fixed(bot, farm)` in Tasks 6 and 7. `nav_config(&BotConfig, &FarmConfig)` in Tasks 4, 7 and 8. `travel` and `farm_stop` take `live: &mut Live` in Tasks 7 and 8. `run_farm`, `run_go`, `run_bank` take `live: Live` in Task 7 and the tests of Task 8.

