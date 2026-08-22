# One path to the content database — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development.
> Steps use checkbox (`- [ ]`) syntax. Run tasks strictly in order.

**Spec:** `docs/superpowers/specs/2026-08-22-one-path-to-content-design.md`

**Goal:** One decoder for `re/mmud_wgnt.sqlite`, living in `mud-core`. The client
holds the same typed `Content` the server does, and its five hand-written queries
are deleted.

## Global Constraints

- **Targeted test binaries only.** `cargo test -p <crate> --test <name>`. Do NOT
  run the full suite; Daniel accepted the tradeoff that regressions may surface
  later. The one exception is Task 1, where `-p mud-server` must be run whole
  because the move touches every loader.
- **Never run `cargo fmt`, `rustfmt`, or any formatter.**
- `CARGO_BUILD_JOBS=3`, foreground, one cargo call at a time. 7.5 GB machine.
- Every file ends with a blank line. Commit per task, tag-prefixed.
- **Never `git add -A` or `git add .`** — stage named files only.
- Do not connect to a live board.
- **Blocked** until `2026-08-22-session-knows-character` lands: it is editing
  `Capabilities` in `graph.rs`, which Task 4 restructures.

## The equivalence discipline

This is a refactor, so the proof is equivalence, not plausibility. For each view,
build the structure the old SQL produced and the structure the view produces
**from the same database**, and assert they are identical. Write that test BEFORE
deleting the query it replaces. A view that quietly drops a filter or flips a
tie-break must fail it.

---

### Task 1: The decoder moves to mud-core

**Files:** `crates/mud-server/src/content_db.rs` → `crates/mud-core/src/content_db.rs`;
`crates/mud-core/Cargo.toml`; `crates/mud-core/src/lib.rs`; `crates/mud-server/src/lib.rs`
(or wherever the module is declared); `crates/mbbs-server/Cargo.toml`.

- [ ] **Step 1:** Move the file. Add `rusqlite = { version = "0.37", features = ["bundled"] }`
      to `mud-core`. Make the per-table loaders (`load_rooms`, `load_monsters`,
      `load_spells`, `load_messages`, `load_textblocks`, `load_races`,
      `load_classes`, …) `pub`. `load()` keeps its exact current behaviour.
- [ ] **Step 2:** Update `mud-server`'s call sites to `mud_core::content_db`.
      Nothing about its behaviour changes.
- [ ] **Step 3:** Delete the `mud-core` line from `crates/mbbs-server/Cargo.toml`.
      It is declared and never used — verified by grep over the whole crate. If
      that grep now finds a use, STOP and report; do not force it.
- [ ] **Step 4:** `CARGO_BUILD_JOBS=3 cargo test -p mud-server` — whole crate,
      the one exception to the targeted-test rule. `load_real_db` (11 tests) must
      stay green. Also confirm `cargo build -p mbbs-server` still compiles.
- [ ] **Step 5: Mutate** — break one column name in `load_races` and confirm a
      `mud-server` test fails. If nothing fails, the loader is untested and that
      is a finding: report it.
- [ ] **Step 6: Commit.**

---

### Task 2: The client holds Content

**Files:** `crates/mud-client/Cargo.toml`, `graph.rs` or a new module, `cli.rs`.

- [ ] **Step 1:** Load `Content` once via `mud_core::content_db::load()` at the
      same point the client currently opens the database, and hold it. The
      existing `--content` paths and defaults are unchanged.
- [ ] **Step 2:** **Measure and report** the resident cost of the loaded
      `Content` (26720 rooms, 1950 items, 1101 monsters, 1379 spells, 3867
      messages, 3267 textblocks). Daniel deferred optimising it but asked for the
      number. Report it; do not act on it.
- [ ] **Step 3: Commit.**

---

### Task 3: The by-name views

**Files:** new view module; `crates/mud-client/tests/` (new test binary).

`Content` keys by numeric id; the client needs lowercased-name maps.

- [ ] **Step 1: Write the equivalence tests first** — old SQL vs view, same
      database, assert identical, for the threat table and spell durations.
- [ ] **Step 2: Implement**, reproducing the filters exactly:
      - monsters: skip `name == ""`; score `exp*1000 + hp`; **max** wins on a
        name collision (7 rows share "giant rat").
      - spells: skip `name == ""` and `duration <= 0` (542 of 1379 survive);
        **min** wins on a collision.
- [ ] **Step 3: Mutate** — flip max to min in the monster tie-break, and drop the
      blank-name filter. Both must fail the equivalence tests. If either passes,
      the test is not discriminating: strengthen it and say so.
- [ ] **Step 4: Commit.**

---

### Task 4: The room graph becomes a view

**Files:** `graph.rs`; its test binary.

- [ ] **Step 1: Equivalence test first.** `RoomGraph` built from SQL vs from
      `Content`, over the whole database, asserted identical.
- [ ] **Step 2: Implement.** `content::Exit` keeps the raw `exit_type` and all
      four `para` slots, so `ExitRequirement::from_exit_type` and `exit_cost`
      port directly. `dest_map` (`exit_type == 8 → para1`) and `forced_monster`
      (`bynumber >> 16`) are already identical in both decoders — confirm rather
      than re-derive.
- [ ] **Step 3:** Exit commands (entry only when `messageline1` is non-empty
      after trimming) and remote actions (`command_block` + textblock body) as
      views, each with its own equivalence test.
- [ ] **Step 4: Delete all five queries** and the now-dead placeholder-row skip
      at `graph.rs:497`, which discards zero rows against the shipped database.
- [ ] **Step 5: Mutate** — drop the `messageline1` non-empty rule and confirm the
      exit-command equivalence test fails.
- [ ] **Step 6: Commit.**

---

### Task 5: The spellbook probe consults magictype

**Files:** `sheet.rs`, `session.rs`; test binary.

- [ ] **Step 1:** Resolve the class name from `Session::stats()` against
      `Content.classes`. The lookup **returns `Option`**. A miss means "I do not
      know" and MUST fall through to today's probe.
- [ ] **Step 2:** `magictype == 0` (Warrior, Witchunter, Ninja, Thief) → send no
      spellbook probe. `magictype == 5` (Mystic) → go straight to `powers`.
      Everything else → probe `spells` as today.
- [ ] **Step 3:** `Casting::redirected()` stays and stays authoritative. The
      database may only skip a probe, never override the board.
- [ ] **Step 4: Mutate** — make the lookup return `Some(magictype 0)` for an
      unknown class and confirm a test proving the fallback fails. This is the
      one that matters: a miss silently meaning "no magic" is the failure mode
      the spec exists to prevent.
- [ ] **Step 5: Commit.**

## Self-Review

**Ordering:** 1 → 2 → 3 → 4 and 1 → 2 → 5 are hard dependencies. 3 and 4 are
independent of each other but both need 2.

**Known gap, stated:** the equivalence tests prove the port faithful, not the
underlying exit-decode rules correct. Those rules come from our RE docs and carry
their own error bars — see the spec's "On the reliability of our own specs".
