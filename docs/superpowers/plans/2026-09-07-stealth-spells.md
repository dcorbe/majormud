# Stealth Spells Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Light and stealth spells are discovered from the content database by ability instead of a name list, and a discovered stealth spell is cast right before every `sneak` the navigator arms.

**Architecture:** Two predicates in `sheet.rs` read a spell record's ability list and its `target` column, and the spellbook is matched against the content's spell map by name. `sheet_from` reads that map from the content the session already holds. The navigator gains an optional stealth buff state behind a mutex, and `arm_sneak` casts what is lapsed and affordable before it sends `sneak`, feeding mana from every prompt it sees.

**Tech Stack:** Rust 2024 edition, tokio, the generated `mud_core::ability::Ability` enum, `mud_core::content::Spell`, the scripted sneak board in `tests/sneak.rs`.

**Spec:** `docs/plans/2026-09-07-stealth-spells-design.md`

## Global Constraints

- Never run `rustfmt` or `cargo fmt`.
- Every file ends with a blank line.
- No em dashes, parentheses or semicolons in prose you write: doc comments, commit messages, the doc. Code punctuation is code.
- Commit messages carry a tag with the crate in brackets as the repo does: `feat(client): ...`, `refactor(client): ...`, `test(client): ...`, `doc(client): ...`. No attribution lines. Do not mention the plan or the spec in commit messages.
- No test may read `re/`. Fixtures are hand-built. A test that needs a file puts it under `env!("CARGO_TARGET_TMPDIR")`, which cargo sets for integration tests. Never `/tmp`.
- Each task's test must fail before the implementation and pass after. Each task also runs the named mutation check: change the rule, watch the test fail, revert.
- Run the crate's tests with `cargo test -p mud-client` from the repo root. Build with `cargo build -p mud-client`. Pass `--test <file>` while iterating and run the whole crate once before each commit. `cargo clippy -p mud-client --all-targets` must add no warnings.
- Working directory for every command below is the repo root `/home/daniel/majormud/majormud`.
- This plan runs after the settings plan. `NavConfig::sneak: bool` and `BotConfig::sneak: bool` exist. Do not add them again.
- The spec says "target mode 1". In `mud_core::content::Spell` the sqlite `target` column is decoded into `match_type: MatchType`, and the field named `target_mode` is the `spelltype` column, which is 3 on every spell in the table. The rule below reads `match_type == MatchType::Single1`. The variant name is the decoder's placeholder. Do not rename it in this plan.

---

## File Structure

- `crates/mud-client/src/sheet.rs` (modify): delete `LIGHT_SPELLS` and `Spellbook::light_spell_with_cost`. Add `SELF_CAST`, `is_light_spell`, `is_stealth_spell`, `Spellbook::known_matching`, the spell map parameter on `light_sources`, `stealth_spells`, `stealth_lines`, and `BuffState::in_flight`, `BuffState::mana`, `BuffState::seed_mana`.
- `crates/mud-client/src/session.rs` (modify): `Session::content` returns the content the realm entry probe stored.
- `crates/mud-client/src/farm.rs` (modify): `Sheet` gains `stealth`. `sheet_from` reads the spell map from `Session::content`. `run_farm` prints the stealth report lines. `stealth_buffs` reads the session's sheet and content for a navigator. The walker closure and the finish walk call `with_stealth`.
- `crates/mud-client/src/go.rs`, `crates/mud-client/src/bank.rs` (modify): the navigator is built `with_stealth`. Durations come from the loaded content instead of an empty map.
- `crates/mud-client/src/nav.rs` (modify): `StealthCasts`, the `stealth` field, `with_stealth`, `cast_stealth`, `note_event`, the new `arm_sneak` order, prompts fed in `wait_room` and the per-step drain.
- `docs/mud-client.md` (modify): one paragraph on stealth spells under the bot section.
- Tests: `crates/mud-client/tests/sheet.rs` (modify), `crates/mud-client/tests/sneak.rs` (modify).

---

### Task 1: Light spells are discovered from the spell table

**Files:**
- Modify: `crates/mud-client/src/sheet.rs:22-27` (`LIGHT_SPELLS`), `:370-378` (`light_spell_with_cost`), `:876-896` (`light_sources`)
- Modify: `crates/mud-client/src/session.rs:941-943` (beside `pack_handle`)
- Modify: `crates/mud-client/src/farm.rs:2410-2431` (`sheet_from`)
- Test: `crates/mud-client/tests/sheet.rs`

**Interfaces:**
- Consumes: `mud_core::content::{Spell, SpellId, MatchType}`, `mud_core::ability::Ability`, `crate::pack::PackHandle::content`.
- Produces:
  - `pub fn sheet::is_light_spell(spell: &Spell) -> bool`
  - `pub fn sheet::is_stealth_spell(spell: &Spell) -> bool`
  - `pub fn Spellbook::known_matching<'a>(&'a self, spells: &'a BTreeMap<SpellId, Spell>, keep: fn(&Spell) -> bool) -> Vec<(&'a KnownSpell, &'a Spell)>`
  - `pub fn sheet::light_sources(inventory: &Inventory, spellbook: &Spellbook, spells: &BTreeMap<SpellId, Spell>, casting: Casting) -> Vec<LightSource>`
  - `pub fn Session::content(&self) -> Option<Arc<mud_core::content::Content>>`

- [ ] **Step 1: Write the failing tests**

Add a spell fixture builder and the discovery tests to `crates/mud-client/tests/sheet.rs`. Put the builder and the map near the top, after the existing `use` line, so every later test in the file can reach them:

```rust
use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    Element, MatchType, SaveClass, ScalePair, Spell, SpellId, TargetMode,
};

/// One spell record with only the fields discovery reads set to
/// something: the ability list and the `target` column, decoded as
/// `match_type`. Everything else is zero.
fn spell(
    number: u16,
    name: &str,
    short: &str,
    mana: i16,
    duration: i16,
    target: MatchType,
    abilities: Vec<(Ability, i16)>,
) -> Spell {
    Spell {
        id: SpellId(number),
        name: name.into(),
        short_name: short.into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities,
        level_cap: 0,
        round_cost: 0,
        required_power: 0,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 0,
        duration_per_level: 0,
        match_type: target,
        duration,
        element: Element::Magic,
        class_gate_group: 0,
        mana_cost: mana,
        max_increase: ScalePair::NONE,
        required_class_level: 0,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

/// The shipped records this plan was measured against, by decoded
/// ability and `target` column. Values are the table's: the self buffs
/// carry 0 because the amount is level scaled.
fn spells() -> BTreeMap<SpellId, Spell> {
    let list = vec![
        spell(26, "starlight", "star", 4, 80, MatchType::Single1, vec![(Ability::RoomIllu, 0)]),
        spell(14, "bless", "bles", 4, 40, MatchType::Single2, vec![(Ability::Accuracy, 0)]),
        spell(39, "way of the cat", "cat", 3, 60, MatchType::Single1, vec![(Ability::Stealth, 0)]),
        spell(130, "shadowform", "shad", 8, 30, MatchType::Single1, vec![(Ability::Stealth, 0)]),
        spell(147, "glitterdust", "glit", 6, 30, MatchType::Single0, vec![(Ability::Stealth, 0)]),
        spell(448, "cross of vengeance", "cross", 15, 600, MatchType::Single1, vec![(Ability::RoomIllu, 200), (Ability::Stealth, -15)]),
        spell(1314, "camouflage", "camo", 10, 30, MatchType::Single1, vec![(Ability::Stealth, 0)]),
        spell(893, "stealth trap", "", 0, 1, MatchType::AreaB, vec![(Ability::RoomIllu, 0), (Ability::Stealth, -200)]),
        spell(2, "lightning bolt", "lb", 4, 0, MatchType::Single0, vec![(Ability::Damage, 10)]),
    ];
    list.into_iter().map(|s| (s.id, s)).collect()
}

#[test]
fn a_light_spell_carries_room_illumination_and_targets_self() {
    let spells = spells();
    assert!(mud_client::sheet::is_light_spell(&spells[&SpellId(26)]), "starlight");
    assert!(
        !mud_client::sheet::is_light_spell(&spells[&SpellId(893)]),
        "the stealth trap lights a room but is not a cast"
    );
    assert!(!mud_client::sheet::is_light_spell(&spells[&SpellId(2)]), "lightning bolt");
}

#[test]
fn a_stealth_spell_raises_stealth_and_targets_self() {
    let spells = spells();
    for id in [39, 130, 1314] {
        assert!(
            mud_client::sheet::is_stealth_spell(&spells[&SpellId(id)]),
            "{}",
            spells[&SpellId(id)].name
        );
    }
    assert!(
        !mud_client::sheet::is_stealth_spell(&spells[&SpellId(147)]),
        "glitterdust targets a monster"
    );
    assert!(
        !mud_client::sheet::is_stealth_spell(&spells[&SpellId(448)]),
        "cross of vengeance lowers stealth"
    );
    assert!(
        !mud_client::sheet::is_stealth_spell(&spells[&SpellId(14)]),
        "bless carries no stealth ability"
    );
}
```

Then change every `light_sources` call in the file to pass the map. There are nine. Each becomes the same call with `&spells()` inserted before the casting argument, for example:

```rust
mud_client::sheet::light_sources(&no_items, &book, &spells(), Casting::Spells)
```

In `spellbook_finds_a_light_spell`, the `lightning bolt` book stays as it is. Its assertion now proves the ability rule, not the name list. Add one more assertion at the end of that test so the deleted names are pinned as gone:

```rust
    let unknown = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    cl    continual light               \n",
    );
    assert!(
        mud_client::sheet::light_sources(&no_items, &unknown, &spells(), Casting::Spells).is_empty(),
        "a name the spell table does not carry is not a light spell"
    );
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test sheet 2>&1 | grep -E "^error|test result" | head`
Expected: compile errors naming `is_light_spell`, `is_stealth_spell` and the arity of `light_sources`.

- [ ] **Step 3: Write the discovery predicates and the map lookup**

In `crates/mud-client/src/sheet.rs`, delete the `LIGHT_SPELLS` constant and its doc comment at lines 22 to 27. Add, in their place:

```rust
use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{MatchType, Spell, SpellId};

/// The `target` column of the spell table, decoded as
/// [`MatchType`]. Value 1 is a cast on oneself: starlight, camouflage
/// and every other self buff carry it, bless carries 2 for another
/// player, glitterdust 0 for a monster. The variant name is the
/// decoder's placeholder from the first pass over the table, and the
/// field called `target_mode` is a different column. This is the one
/// discovery reads.
const SELF_CAST: MatchType = MatchType::Single1;

/// Does casting this on oneself light the room? Room illumination, on a
/// self cast. The old three-name list found one of the same spells and
/// named two that the shipped table does not carry.
pub fn is_light_spell(spell: &Spell) -> bool {
    spell.match_type == SELF_CAST
        && spell.abilities.iter().any(|(a, _)| *a == Ability::RoomIllu)
}

/// Does casting this on oneself raise stealth? The stealth ability with
/// a value that is not negative, on a self cast. The value reads as zero
/// on every shipped self buff because the amount is level scaled, so
/// presence decides. A negative value is a penalty, which keeps cross of
/// vengeance and the stealth trap out.
pub fn is_stealth_spell(spell: &Spell) -> bool {
    spell.match_type == SELF_CAST
        && spell
            .abilities
            .iter()
            .any(|(a, v)| *a == Ability::Stealth && *v >= 0)
}
```

Check the existing `use` lines at the top of `sheet.rs` first. If `BTreeMap` is already imported, do not import it twice.

In `impl Spellbook`, delete `light_spell_with_cost` at lines 370 to 378 and add:

```rust
    /// The spells this character knows that `keep` accepts, matched
    /// against the content's spell map by name, in the book's order.
    /// A known spell the map does not carry is skipped: the board's
    /// listing is the truth about what is castable, the map is the
    /// truth about what a cast does, and discovery needs both.
    pub fn known_matching<'a>(
        &'a self,
        spells: &'a BTreeMap<SpellId, Spell>,
        keep: fn(&Spell) -> bool,
    ) -> Vec<(&'a KnownSpell, &'a Spell)> {
        self.spells
            .iter()
            .filter_map(|known| {
                let lower = known.name.to_lowercase();
                spells
                    .values()
                    .find(|s| s.name.to_lowercase() == lower)
                    .filter(|s| keep(s))
                    .map(|s| (known, s))
            })
            .collect()
    }
```

Change `light_sources` to take the map and use the predicate:

```rust
pub fn light_sources(
    inventory: &Inventory,
    spellbook: &Spellbook,
    spells: &BTreeMap<SpellId, Spell>,
    casting: Casting,
) -> Vec<LightSource> {
    let mut sources: Vec<LightSource> = inventory
        .light_items()
        .into_iter()
        .map(|item| LightSource::Item {
            light_cmd: format!("light {item}"),
            remove_cmd: format!("remove {item}"),
        })
        .collect();
    if let Some((known, _)) = spellbook.known_matching(spells, is_light_spell).first() {
        sources.push(LightSource::Spell {
            cmd: casting.command(&known.short),
            mana_cost: known.mana as i32,
        });
    }
    sources
}
```

- [ ] **Step 4: Give the session a content accessor and feed the map to `sheet_from`**

In `crates/mud-client/src/session.rs`, after `pack_handle` at line 943:

```rust
    /// The content database the realm entry probe stored, if it did.
    /// The pack holds it because the pack was its first reader. Spell
    /// discovery is its second.
    pub fn content(&self) -> Option<Arc<mud_core::content::Content>> {
        self.pack
            .lock()
            .expect("pack lock")
            .as_ref()
            .map(|h| Arc::clone(h.content()))
    }
```

In `crates/mud-client/src/farm.rs`, change `sheet_from`:

```rust
pub(crate) fn sheet_from(
    session: &crate::session::Session,
    bot: &crate::bot::BotConfig,
    durations: &BTreeMap<String, u32>,
) -> Sheet {
    let (inventory, book, casting) = session.raw_sheet();
    // The spell map comes from the content the session already holds.
    // Before realm entry there is none, and then there is no light
    // spell either, which matches the empty book that comes back with
    // it.
    let content = session.content();
    let empty = BTreeMap::new();
    let spells = content.as_ref().map(|c| &c.spells).unwrap_or(&empty);
    Sheet {
        light: crate::sheet::light_sources(&inventory, &book, spells, casting),
        heals: book.heal_spells(
            crate::sheet::HealChoice {
                minor: &bot.minor_heal_spell,
                major: &bot.major_heal_spell,
                regen: &bot.hp_regen_spell,
            },
            durations,
            casting,
        ),
        buffs: crate::sheet::buffs(&book, &bot.buffs, durations, casting),
    }
}
```

Build and fix any other caller of `light_sources` the compiler names. Search first:

Run: `grep -rn "light_sources(" crates/mud-client/src crates/mud-client/tests | grep -v "tests/sheet.rs"`
Expected: only `farm.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test sheet 2>&1 | grep -E "^error|test result|panicked"`
Expected: `test result: ok.` with every test passing.

- [ ] **Step 6: Mutation check**

In `is_light_spell`, change `Ability::RoomIllu` to `Ability::Illu`. Run the sheet tests. `a_light_spell_carries_room_illumination_and_targets_self` and `spellbook_finds_a_light_spell` fail. Revert. In `is_stealth_spell`, change `*v >= 0` to `true`. Run. `a_stealth_spell_raises_stealth_and_targets_self` fails on cross of vengeance. Revert.

- [ ] **Step 7: Run the crate and clippy, then commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "test result" | awk '{p+=$4; f+=$6} END {print p" passed, "f" failed"}'`
Expected: `0 failed`.

Run: `cargo clippy -p mud-client --all-targets 2>&1 | grep -c "^warning\|^error"`
Expected: the same count as before this task. Note the count before you start.

```bash
git add crates/mud-client/src/sheet.rs crates/mud-client/src/session.rs crates/mud-client/src/farm.rs crates/mud-client/tests/sheet.rs
git commit -m "refactor(client): light spells are discovered from the spell table, not a name list

Two of the three names in the old list are not in the shipped table, and
the one that is, starlight, is found the same way camouflage will be:
room illumination on a self cast."
```

---

### Task 2: The sheet discovers stealth spells and says what it found

**Files:**
- Modify: `crates/mud-client/src/sheet.rs` (after `buffs` at line 812)
- Modify: `crates/mud-client/src/farm.rs:2255-2270` (`Sheet`), `:2410-2431` (`sheet_from`), `:1756-1767` (the buff report in `run_farm`)
- Test: `crates/mud-client/tests/sheet.rs`

**Interfaces:**
- Consumes: `Spellbook::known_matching`, `is_stealth_spell`, `Buff`, `Casting::command` from Task 1 and the existing sheet.
- Produces:
  - `pub fn sheet::stealth_spells(spellbook: &Spellbook, spells: &BTreeMap<SpellId, Spell>, durations: &BTreeMap<String, u32>, casting: Casting) -> (Vec<Buff>, Vec<String>)`
  - `pub fn sheet::stealth_lines(spellbook: &Spellbook, found: &[Buff], refused: &[String]) -> Vec<String>`
  - `Sheet::stealth: (Vec<Buff>, Vec<String>)` in `farm.rs`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/sheet.rs`:

```rust
// --- stealth spells ---------------------------------------------------

fn ranger_book() -> Spellbook {
    Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    star  starlight                     \n\
         \x20 3  10    camo  camouflage                    \n\
         \x20 2   4    bles  bless                         \n",
    )
}

fn stealth_durations() -> BTreeMap<String, u32> {
    BTreeMap::from([
        ("starlight".to_string(), 80),
        ("camouflage".to_string(), 30),
        ("bless".to_string(), 40),
        ("shadowform".to_string(), 30),
    ])
}

#[test]
fn a_known_stealth_spell_is_a_buff_with_the_shipped_duration() {
    let (found, refused) = mud_client::sheet::stealth_spells(
        &ranger_book(),
        &spells(),
        &stealth_durations(),
        Casting::Spells,
    );
    assert_eq!(
        found,
        vec![Buff {
            name: "camouflage".into(),
            cmd: "cast camo".into(),
            mana_cost: 10,
            rounds: 30,
        }]
    );
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn a_stealth_spell_the_book_lacks_is_absent_not_refused() {
    let book = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    star  starlight                     \n",
    );
    let (found, refused) =
        mud_client::sheet::stealth_spells(&book, &spells(), &stealth_durations(), Casting::Spells);
    assert!(found.is_empty());
    assert!(refused.is_empty(), "shadowform is in the table and not in the book: nothing to say");
}

#[test]
fn a_stealth_spell_with_no_duration_is_refused_by_name() {
    let mut durations = stealth_durations();
    durations.insert("camouflage".to_string(), 0);
    let (found, refused) = mud_client::sheet::stealth_spells(
        &ranger_book(),
        &spells(),
        &durations,
        Casting::Spells,
    );
    assert!(found.is_empty());
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(refused[0].contains("camouflage"), "{refused:?}");
    assert!(refused[0].contains("no duration"), "{refused:?}");
}

#[test]
fn a_mystic_invokes_its_stealth_power() {
    let (found, _) = mud_client::sheet::stealth_spells(
        &ranger_book(),
        &spells(),
        &stealth_durations(),
        Casting::Powers,
    );
    assert_eq!(found[0].cmd, "invoke camo");
}

#[test]
fn the_report_names_the_stealth_spell_and_its_cost() {
    let book = ranger_book();
    let (found, refused) =
        mud_client::sheet::stealth_spells(&book, &spells(), &stealth_durations(), Casting::Spells);
    assert_eq!(
        mud_client::sheet::stealth_lines(&book, &found, &refused),
        vec!["stealth: camouflage (10 mana, 30 rounds)".to_string()]
    );
}

#[test]
fn the_report_says_none_known_for_a_book_without_one() {
    let book = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    star  starlight                     \n",
    );
    assert_eq!(
        mud_client::sheet::stealth_lines(&book, &[], &[]),
        vec!["stealth: none known".to_string()]
    );
}

#[test]
fn the_report_is_silent_for_a_character_with_no_spellbook() {
    let book = Spellbook::parse("You have no spells.\n");
    assert!(mud_client::sheet::stealth_lines(&book, &[], &[]).is_empty());
}

#[test]
fn the_report_carries_a_refusal_as_its_own_line() {
    let book = ranger_book();
    let refused = vec!["`camouflage` raises stealth but has no duration in the spell table".to_string()];
    assert_eq!(
        mud_client::sheet::stealth_lines(&book, &[], &refused),
        vec!["stealth: `camouflage` raises stealth but has no duration in the spell table".to_string()]
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test sheet 2>&1 | grep -E "^error|test result" | head`
Expected: compile errors naming `stealth_spells` and `stealth_lines`.

- [ ] **Step 3: Write `stealth_spells` and `stealth_lines`**

In `crates/mud-client/src/sheet.rs`, after `buffs` at line 812:

```rust
/// The stealth spells this character knows, as buffs to keep up on a
/// sneak. Discovery, not configuration: the book says what is castable
/// and the spell map says what raises stealth. A spell the map carries
/// and the book does not is simply absent. One the book carries with no
/// duration in the table is refused out loud, the same way a buff is,
/// because a budget of zero rounds is always lapsed.
pub fn stealth_spells(
    spellbook: &Spellbook,
    spells: &BTreeMap<SpellId, Spell>,
    durations: &BTreeMap<String, u32>,
    casting: Casting,
) -> (Vec<Buff>, Vec<String>) {
    let mut out = Vec::new();
    let mut refused = Vec::new();
    for (known, _) in spellbook.known_matching(spells, is_stealth_spell) {
        match durations.get(&known.name.to_lowercase()) {
            Some(&rounds) if rounds > 0 => out.push(Buff {
                name: known.name.clone(),
                cmd: casting.command(&known.short),
                mana_cost: known.mana as i32,
                rounds,
            }),
            _ => refused.push(format!(
                "`{}` raises stealth but has no duration in the spell table",
                known.name
            )),
        }
    }
    (out, refused)
}

/// What a job says about stealth at startup, beside its heal and light
/// lines. One line per spell found, one per refusal, and "none known"
/// for a book with no stealth spell in it. A character with no book at
/// all gets nothing, because nothing was looked for.
pub fn stealth_lines(spellbook: &Spellbook, found: &[Buff], refused: &[String]) -> Vec<String> {
    if spellbook.spells.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = refused.iter().map(|r| format!("stealth: {r}")).collect();
    for b in found {
        lines.push(format!(
            "stealth: {} ({} mana, {} rounds)",
            b.name, b.mana_cost, b.rounds
        ));
    }
    if lines.is_empty() {
        lines.push("stealth: none known".to_string());
    }
    lines
}
```

- [ ] **Step 4: Carry it on the sheet and print it at farm start**

In `crates/mud-client/src/farm.rs`, add a field to `Sheet` at line 2255:

```rust
    /// The stealth spells the book carries, and one line for each
    /// that has no duration.
    pub stealth: (Vec<crate::sheet::Buff>, Vec<String>),
```

In `sheet_from`, add after the `buffs:` field, using the `spells` binding Task 1 introduced:

```rust
        stealth: crate::sheet::stealth_spells(&book, spells, durations, casting),
```

In `run_farm`, after the buff report loop that ends at line 1767 with `let buff = crate::sheet::BuffState::new(kept);`, print the stealth lines. The book is needed for the "none known" rule, so read it from the session:

```rust
    // Stealth, in the same voice: what the book carries that raises
    // stealth, cast before every sneak by the navigator.
    let (_, book, _) = session.raw_sheet();
    for line in crate::sheet::stealth_lines(&book, &sheet.stealth.0, &sheet.stealth.1) {
        notices(&line);
    }
```

Place it before `let mut casts = Casts { light, heal, buff };`. `sheet.light`, `sheet.heals` and `sheet.buffs` are moved out earlier in that function, so keep `sheet.stealth` reachable: partial moves out of a struct binding are allowed as long as `sheet` itself is not used whole afterwards. If the compiler objects, bind `let stealth = sheet.stealth;` right after `let sheet = sheet_from(...)` at line 1719 and use `stealth.0` and `stealth.1` here.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test sheet 2>&1 | grep -E "^error|test result|panicked"`
Expected: `test result: ok.`

- [ ] **Step 6: Mutation check**

In `stealth_spells`, change `Some(&rounds) if rounds > 0` to `Some(&rounds)`. Run the sheet tests. `a_stealth_spell_with_no_duration_is_refused_by_name` fails, because a zero duration is now kept. Revert.

- [ ] **Step 7: Run the crate and clippy, then commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "test result" | awk '{p+=$4; f+=$6} END {print p" passed, "f" failed"}'`
Expected: `0 failed`.

Run: `cargo clippy -p mud-client --all-targets 2>&1 | grep -c "^warning\|^error"`
Expected: unchanged.

```bash
git add crates/mud-client/src/sheet.rs crates/mud-client/src/farm.rs crates/mud-client/tests/sheet.rs
git commit -m "feat(client): the sheet discovers stealth spells and says what it found

Camouflage, way of the cat and shadowform are found by the stealth
ability on a self cast. A farm start prints what it found the way it
prints its heals."
```

---

### Task 3: A stealth spell is cast before every sneak

**Files:**
- Modify: `crates/mud-client/src/sheet.rs` (`BuffState`, lines 675-802)
- Modify: `crates/mud-client/src/nav.rs:636-650` (struct), `:698-760` (builders), `:944-946` (the per-step drain in `goto`), `:2242-2300` (`arm_sneak`), `:2384-2420` (`wait_room`)
- Modify: `crates/mud-client/src/farm.rs:1804-1815` (the walker), `:2137-2138` (the finish walk), and a new `stealth_buffs` beside `sheet_from`
- Modify: `crates/mud-client/src/go.rs:248-253`, `:291`
- Modify: `crates/mud-client/src/bank.rs:488-494`, `:512`
- Modify: `docs/mud-client.md` (the bot section, near `buffs`)
- Test: `crates/mud-client/tests/sneak.rs`

**Interfaces:**
- Consumes: `NavConfig::sneak: bool` from the settings plan. `Buff`, `BuffState`, `CastAttempt`, `RoundClock`, `Session::state`, `Session::content`, `views::spell_durations`, `stealth_spells` from Task 2.
- Produces:
  - `pub fn BuffState::in_flight(&self) -> bool`
  - `pub fn BuffState::mana(&self) -> Option<i32>`
  - `pub fn BuffState::seed_mana(&mut self, mana: i32)`
  - `pub fn Navigator::with_stealth(self, buffs: Vec<Buff>, clock: RoundClock) -> Self`
  - `pub(crate) fn farm::stealth_buffs(session: &Session) -> Vec<Buff>`

- [ ] **Step 1: Teach the scripted sneak board to answer a cast**

In `crates/mud-client/tests/sneak.rs`, the board's prompt carries `MA=0`, which makes every spell unaffordable. Change `room_block` and both prompt strings in the board to `MA=20`:

```rust
fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=20]:")
}
```

and in `sneak_board`, `"\r\n{sneak_reply}\r\n[HP=30/MA=0]:"` becomes `"\r\n{sneak_reply}\r\n[HP=30/MA=20]:"` and the `other` arm's prompt likewise.

Add a cast reply and an ordered log to the board:

```rust
#[derive(Default)]
struct SneakLog {
    sneaks: AtomicUsize,
    moves: AtomicUsize,
    casts: AtomicUsize,
    /// Every line the client sent, in order.
    lines: std::sync::Mutex<Vec<String>>,
}
```

Add to `Board`:

```rust
    /// What a `cast camo` is answered with, the raw body between the
    /// echo and the prompt. `None` answers it as an unknown command.
    cast_reply: Option<&'static str>,
```

In the board's read loop, record every line first, then add the cast arm before `other`:

```rust
            counter.lines.lock().unwrap().push(line.clone());
            let reply = match line.as_str() {
                "sneak" => { /* unchanged */ }
                "n" | "north" => { /* unchanged */ }
                "cast camo" if board.cast_reply.is_some() => {
                    counter.casts.fetch_add(1, Ordering::SeqCst);
                    format!("\r\n{}\r\n[HP=30/MA=20]:", board.cast_reply.unwrap())
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=20]:"),
            };
```

Add two navigator builders beside `nav_with_timeout`:

```rust
use mud_client::sheet::Buff;
use mud_client::world::RoundClock;

fn camouflage(rounds: u32) -> Vec<Buff> {
    vec![Buff {
        name: "camouflage".into(),
        cmd: "cast camo".into(),
        mana_cost: 10,
        rounds,
    }]
}

fn nav_with_stealth(graph: Arc<RoomGraph>, stealth: u32, clock: RoundClock, rounds: u32) -> Navigator {
    nav(graph, stealth).with_stealth(camouflage(rounds), clock)
}

/// The lines this plan's tests reason about, in the order they were
/// sent. Anything else the session sends on its own is left out so the
/// order assertions read one thing.
fn sent(log: &SneakLog) -> Vec<String> {
    log.lines
        .lock()
        .unwrap()
        .iter()
        .filter(|l| *l == "sneak" || *l == "n" || l.starts_with("cast "))
        .cloned()
        .collect()
}
```

- [ ] **Step 2: Write the failing tests**

Append to `crates/mud-client/tests/sneak.rs`:

```rust
/// The buff comes first and the sneak second, because a cast breaks a
/// sneak. Mutation target: send `sneak` before the cast and the order
/// assertion fails.
#[tokio::test]
async fn a_stealth_spell_is_cast_before_the_sneak() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board { arms: true, cast_reply: Some("You cast camouflage!"), ..Board::default() },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav_with_stealth(graph_one_hop(), 56, RoundClock::new(), 30);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(arrival.sneaking);
    assert_eq!(
        sent(&log),
        vec!["cast camo".to_string(), "sneak".to_string(), "n".to_string()]
    );
}

/// `bot.sneak` off means no walk arms a sneak and nothing is cast for
/// one.
#[tokio::test]
async fn sneak_off_casts_nothing_and_arms_nothing() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board { arms: true, cast_reply: Some("You cast camouflage!"), ..Board::default() },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = Navigator::new(
        graph_one_hop(),
        NavConfig { step_timeout_ms: 1500, sneak: false, ..NavConfig::default() },
    )
    .with_capabilities(Capabilities { stealth: 56, ..Capabilities::unrestricted() })
    .with_stealth(camouflage(30), RoundClock::new());
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(!arrival.sneaking);
    assert_eq!(sent(&log), vec!["n".to_string()]);
}

/// A break on the first step re-arms before the second. The spell has
/// lapsed by then, on a one round budget against a one millisecond
/// round, so it is recast first.
#[tokio::test]
async fn a_lapsed_spell_is_recast_before_the_rearm() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board {
            arms: true,
            break_on_move: Some(0),
            cast_reply: Some("You cast camouflage!"),
            ..Board::default()
        },
    )
    .await;
    let session = session_for(addr).await;
    let clock = RoundClock::with_period(std::time::Duration::from_millis(1));
    let navigator = nav_with_stealth(graph_two_hop(), 56, clock, 1);
    let arrival = navigator
        .goto(&session, HERE, FAR, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(arrival.sneaking);
    assert_eq!(
        sent(&log),
        vec![
            "cast camo".to_string(),
            "sneak".to_string(),
            "n".to_string(),
            "cast camo".to_string(),
            "sneak".to_string(),
            "n".to_string(),
        ]
    );
}

/// The same break with a spell still running: no recast, just the
/// re-arm.
#[tokio::test]
async fn a_running_spell_is_not_recast_on_the_rearm() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board {
            arms: true,
            break_on_move: Some(0),
            cast_reply: Some("You cast camouflage!"),
            ..Board::default()
        },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav_with_stealth(graph_two_hop(), 56, RoundClock::new(), 30);
    let arrival = navigator
        .goto(&session, HERE, FAR, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(arrival.sneaking);
    assert_eq!(log.casts.load(Ordering::SeqCst), 1, "cast once, still running at the re-arm");
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 2);
}

/// A fizzle does not stop anything: the sneak goes out unbuffed and the
/// walk arrives.
#[tokio::test]
async fn a_fizzle_is_followed_by_the_sneak_anyway() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board {
            arms: true,
            cast_reply: Some("You attempt to cast camouflage, but fail."),
            ..Board::default()
        },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav_with_stealth(graph_one_hop(), 56, RoundClock::new(), 30);
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(arrival.sneaking, "the sneak itself took");
    assert_eq!(arrival.at, THERE);
    assert_eq!(
        sent(&log),
        vec!["cast camo".to_string(), "sneak".to_string(), "n".to_string()],
        "one attempt, then on with the sneak"
    );
}

/// A cast the board never answers is given up at the step deadline and
/// the sneak still goes out.
#[tokio::test]
async fn an_unanswered_cast_does_not_hang_the_walk() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board { arms: true, cast_reply: Some(""), ..Board::default() },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav_with_timeout(graph_one_hop(), 56, 400)
        .with_stealth(camouflage(30), RoundClock::new());
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert_eq!(arrival.at, THERE);
    assert_eq!(log.sneaks.load(Ordering::SeqCst), 1);
}

/// No stealth spell known: the walk is exactly what it was.
#[tokio::test]
async fn no_stealth_spell_means_sneak_and_nothing_else() {
    let (addr, log) = sneak_board(
        "Attempting to sneak...",
        Board { arms: true, cast_reply: Some("You cast camouflage!"), ..Board::default() },
    )
    .await;
    let session = session_for(addr).await;
    let navigator = nav(graph_one_hop(), 56).with_stealth(Vec::new(), RoundClock::new());
    let arrival = navigator
        .goto(&session, HERE, THERE, &mut NoGuard, false)
        .await
        .unwrap();
    assert!(arrival.sneaking);
    assert_eq!(sent(&log), vec!["sneak".to_string(), "n".to_string()]);
}
```

Note on the unanswered cast test: with `cast_reply: Some("")` the board answers the cast with only a prompt. `BuffState::on_event` resolves a pending cast on a line, never on a prompt, so the navigator waits out its step deadline of 400 milliseconds and moves on.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test sneak 2>&1 | grep -E "^error|test result" | head`
Expected: compile errors naming `with_stealth`.

- [ ] **Step 4: Add the buff state accessors**

In `crates/mud-client/src/sheet.rs`, inside `impl BuffState` after `buffs()` at line 702:

```rust
    /// A cast has gone out and the board has not said how it went.
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    pub fn mana(&self) -> Option<i32> {
        self.mana
    }

    /// Mana from somewhere other than a prompt this state saw: the
    /// session's last prompt, for a state built after it passed.
    /// Only fills a blank, so a prompt already seen is never overwritten
    /// by an older reading.
    pub fn seed_mana(&mut self, mana: i32) {
        if self.mana.is_none() {
            self.mana = Some(mana);
        }
    }
```

- [ ] **Step 5: Give the navigator its stealth casts**

In `crates/mud-client/src/nav.rs`, add to the `Navigator` struct after `backstab`:

```rust
    /// The stealth spells to cast before a sneak, with the clock their
    /// budget runs on. Behind a mutex because `goto` takes `&self` and
    /// the state moves. Never held across an await.
    stealth: Option<std::sync::Mutex<StealthCasts>>,
```

And the struct, after `BackstabPrep`:

```rust
struct StealthCasts {
    buffs: crate::sheet::BuffState,
    clock: crate::world::RoundClock,
}
```

In `Navigator::new`, add `stealth: None,` to the literal. Add the builder after `with_backstab`:

```rust
    /// Cast these before every sneak this navigator arms. An empty list
    /// arms the sneak as before.
    pub fn with_stealth(
        mut self,
        buffs: Vec<crate::sheet::Buff>,
        clock: crate::world::RoundClock,
    ) -> Self {
        if !buffs.is_empty() {
            self.stealth = Some(std::sync::Mutex::new(StealthCasts {
                buffs: crate::sheet::BuffState::new(buffs),
                clock,
            }));
        }
        self
    }

    /// Let the stealth budget see an event: mana from a prompt, a wear
    /// off line, the answer to a cast of its own.
    fn note_event(&self, cor: &crate::correlate::Correlated) {
        if let Some(stealth) = &self.stealth {
            stealth
                .lock()
                .expect("stealth lock")
                .buffs
                .on_event(cor, std::time::Instant::now());
        }
    }
```

Feed events in the two places the navigator reads them for a step. In `goto`, the drain before each step at line 944 becomes:

```rust
                crate::session::drain(&mut events, |ev| {
                    self.note_event(ev);
                    armed = armed.take().or_else(|| guard.on_event(&ev.event));
                });
```

In `wait_room`, right after `let cor = match ev { ... };` and before `if cor.answers != Some(awaiting)`, add:

```rust
            self.note_event(&cor);
```

- [ ] **Step 6: Cast before arming**

Add `cast_stealth` before `arm_sneak` in `nav.rs`:

```rust
    /// Cast whatever stealth spell is lapsed and affordable, one attempt
    /// each, and wait for the board's word on it. A cast breaks a sneak,
    /// so this runs before `sneak` and never after it.
    ///
    /// One attempt per spell per arming. A fizzle, a refusal or an
    /// unanswered cast leaves the spell lapsed, and the next arming
    /// tries again. Looping on it here would spend rounds standing
    /// still that the walk has better uses for.
    async fn cast_stealth(
        &self,
        session: &Session,
        events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<(), NavErrorKind> {
        let Some(stealth) = &self.stealth else {
            return Ok(());
        };
        // A state built after the last prompt passed knows no mana and
        // would cast nothing. The session remembers the prompt.
        if let Some(mana) = session.state().borrow().mana {
            stealth.lock().expect("stealth lock").buffs.seed_mana(mana);
        }
        let deadline = tokio::time::Instant::now() + self.step_timeout;
        loop {
            let attempt = {
                let mut held = stealth.lock().expect("stealth lock");
                // A plain `&mut` so the two fields can be borrowed apart.
                // Through the guard itself the borrow checker refuses.
                let s = &mut *held;
                let now = std::time::Instant::now();
                s.buffs.attempt(now, &s.clock)
            };
            let cmd = match attempt {
                crate::sheet::CastAttempt::Send(cmd) => cmd,
                // A cast already went out this round, or nothing is
                // wanted. Either way the sneak is next.
                crate::sheet::CastAttempt::Hold(_) | crate::sheet::CastAttempt::Nothing => {
                    return Ok(());
                }
            };
            let id = session.send(&cmd);
            stealth.lock().expect("stealth lock").buffs.on_sent(&cmd, id);
            loop {
                let ev = tokio::time::timeout_at(deadline, events.recv()).await;
                if let Ok(Ok(ev)) = &ev {
                    match guard.on_event(&ev.event) {
                        Some(Interrupt::Died) => {
                            return Err(NavErrorKind::Interrupted(Interrupt::Died));
                        }
                        Some(hurt) => *armed = armed.take().or(Some(hurt)),
                        None => {}
                    }
                }
                let cor = match ev {
                    // Unanswered past the deadline: forget the cast and
                    // go on to the sneak. The budget stays lapsed.
                    Err(_) => {
                        stealth.lock().expect("stealth lock").buffs.new_visit();
                        return Ok(());
                    }
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                    Ok(Err(_)) => {
                        return Err(NavErrorKind::Expect(ExpectError::Closed {
                            tail: String::new(),
                        }));
                    }
                    Ok(Ok(cor)) => cor,
                };
                let settled = {
                    let mut held = stealth.lock().expect("stealth lock");
                    held.buffs.on_event(&cor, std::time::Instant::now());
                    !held.buffs.in_flight()
                };
                if settled {
                    break;
                }
            }
        }
    }
```

Change the head of `arm_sneak` so the order is gate, cast, sneak:

```rust
        if !self.sneak || self.capabilities.stealth == 0 {
            return Ok(false);
        }
        self.cast_stealth(session, events, guard, armed).await?;
        const MAY_NOT_SNEAK: &str = "You may not sneak right now!";
```

The `self.sneak` field is the one the settings plan added from `NavConfig::sneak`. If that plan stored it under another name, use that name and do not add a second field.

- [ ] **Step 7: Run the sneak tests**

Run: `cargo test -p mud-client --test sneak 2>&1 | grep -E "^error|test result|panicked|left|right"`
Expected: `test result: ok.` with the seven new tests and every old one passing. The old tests still pass with `MA=20` because none of them reads mana.

- [ ] **Step 8: Mutation check**

In `arm_sneak`, move the `self.cast_stealth(...)` line to after `let id = session.send("sneak");`. Run the sneak tests. `a_stealth_spell_is_cast_before_the_sneak` fails on order. Revert. In `cast_stealth`, delete the `seed_mana` block. Run. `a_stealth_spell_is_cast_before_the_sneak` fails because no prompt has passed through the navigator before the first arm. Revert.

- [ ] **Step 9: Wire every walker**

In `crates/mud-client/src/farm.rs`, beside `sheet_from`:

```rust
/// The stealth buffs a navigator casts before a sneak, read from the
/// session's own book and content. Durations come from the content in
/// hand rather than a second decode of the database. Empty before
/// realm entry, and empty for a character with no stealth spell, which
/// is what `Navigator::with_stealth` treats as "arm the sneak as
/// before".
pub(crate) fn stealth_buffs(session: &crate::session::Session) -> Vec<crate::sheet::Buff> {
    let Some(content) = session.content() else {
        return Vec::new();
    };
    let (_, book, casting) = session.raw_sheet();
    let durations = crate::views::spell_durations(&content);
    crate::sheet::stealth_spells(&book, &content.spells, &durations, casting).0
}
```

The walker closure at line 1805 becomes:

```rust
    let walker = || {
        let nav = crate::nav::Navigator::new(graph.clone(), cfg.nav.clone())
            .with_capabilities(session.capabilities())
            .with_stealth(stealth_buffs(session), crate::world::RoundClock::new());
        match &content {
```

The finish walk at line 2137:

```rust
    let nav = crate::nav::Navigator::new(graph.clone(), cfg.nav.clone())
        .with_capabilities(session.capabilities())
        .with_stealth(stealth_buffs(session), crate::world::RoundClock::new());
```

In `crates/mud-client/src/go.rs` at line 249:

```rust
    let nav = crate::nav::Navigator::new(graph.clone(), cfg.nav.clone())
        .with_capabilities(session.capabilities())
        .with_stealth(crate::farm::stealth_buffs(session), crate::world::RoundClock::new());
```

In `crates/mud-client/src/bank.rs` at line 488:

```rust
    let nav = crate::nav::Navigator::new(graph.clone(), cfg.nav.clone())
        .with_capabilities(session.capabilities())
        .with_stealth(crate::farm::stealth_buffs(session), crate::world::RoundClock::new())
        .with_backstab(
```

`go.rs:291` and `bank.rs:512` pass `&Default::default()` as durations to `sheet_from`. Leave them. The stealth list for the navigator no longer goes through that sheet.

- [ ] **Step 10: Document it**

In `docs/mud-client.md`, in the `[bot]` section near the `buffs` line, add a paragraph:

```markdown
A stealth spell the character knows is cast before every sneak a walk
arms, on the same budget a buff runs on. It is discovered from the spell
table by its stealth ability, so camouflage, way of the cat and shadowform
are found without being named. `bot.sneak = false` turns the sneak and
the cast off together. A farm start prints what it found as
`stealth: camouflage (10 mana, 30 rounds)`, or `stealth: none known`.
```

- [ ] **Step 11: Run the crate and clippy, then commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "test result" | awk '{p+=$4; f+=$6} END {print p" passed, "f" failed"}'`
Expected: `0 failed`.

Run: `cargo clippy -p mud-client --all-targets 2>&1 | grep -c "^warning\|^error"`
Expected: unchanged.

Run: `for f in crates/mud-client/src/nav.rs crates/mud-client/src/sheet.rs crates/mud-client/src/farm.rs crates/mud-client/tests/sneak.rs docs/mud-client.md; do tail -c1 "$f" | xxd -p; done`
Expected: `0a` on every line.

```bash
git add crates/mud-client/src/nav.rs crates/mud-client/src/sheet.rs crates/mud-client/src/farm.rs crates/mud-client/src/go.rs crates/mud-client/src/bank.rs crates/mud-client/tests/sneak.rs docs/mud-client.md
git commit -m "feat(client): a stealth spell is cast before every sneak

The navigator is the one place a sneak is armed, so go, farm, roam and
bank all get it. A cast breaks a sneak, which is why it comes first.
One attempt per arming: a fizzle or silence leaves the spell lapsed and
the walk goes on unbuffed."
```

---

## Self-review

Spec coverage:

- Discovery rule for stealth and light by ability and self cast: Task 1.
- `LIGHT_SPELLS` deleted, starlight pinned: Task 1.
- `light_sources` takes the spell map: Task 1.
- `stealth_spells` with the spec's signature and the zero duration refusal: Task 2.
- The two report lines with their exact text, and silence for no book: Task 2.
- `Navigator::with_stealth(buffs, clock)`: Task 3.
- Arming order gate, cast, sneak, with the cast outcome handling and the one attempt rule: Task 3.
- Re-arm after a break recasts a lapsed spell and not a running one: Task 3 tests.
- Mana fed from prompts inside the navigator, plus the seed from the session: Task 3.
- `bot.sneak` off casts nothing and arms nothing: Task 3 test.
- Every scripted board test the spec lists: Task 3.

Type consistency: `stealth_spells` returns `(Vec<Buff>, Vec<String>)` in Task 2 and `stealth_buffs` reads `.0` of it in Task 3. `with_stealth` takes `Vec<Buff>` and `RoundClock` in both the builder and every call site. `known_matching` takes a `fn(&Spell) -> bool` and both predicates are plain functions.

One departure from the spec's wording, recorded in Global Constraints: the field is `match_type`, not `target_mode`.

