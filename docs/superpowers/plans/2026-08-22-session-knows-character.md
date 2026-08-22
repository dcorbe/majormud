# The session knows the character Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** The client learns who it is playing once, at login, instead of re-asking on every walk — and picking a lock becomes something the character can do rather than something an operator configures.

**Architecture:** The `Session` already owns polled character knowledge (the purse, read off `i` at realm entry and carried on `Capabilities` to every `Navigator`). Extend that pattern: a full `stat` sheet and the existing inventory/spellbook probe move to realm entry and live on the session. Nothing polls while a full-screen form is open.

**Tech Stack:** Rust, `cargo test -p mud-client`.

**Origin:** Daniel, 2026-08-22, after the locked-door fix landed: *"the client should know about everything in stat"*, *"it should do a stat once at login and then again after a train"*, *"the client checks spells every time I go or farm — that should be a login item too"*, and *"I don't think this is a config option the user should have to manage, if the character can pick, it should."*

## Global Constraints

- **Test scope is `cargo test -p mud-client` and nothing wider.** Never `--workspace`.
- **Never run `cargo fmt`, `rustfmt`, or any formatter.**
- Every file ends with a blank line. Commit with a tag prefix.
- Baseline on this branch: 50 binaries, 812 passed, 0 failed.
- `CARGO_BUILD_JOBS=3` — 7.5 GB machine. Run cargo in the FOREGROUND, one call.
- **Do not connect to any live board.**

## The constraint that shapes this whole plan

`train` opens a MajorMUD **FSD full-screen form**, and `mmc` corrupts those. Confirmed live 2026-08-02 (memory `mmc-cannot-pilot-fsd-screens`): the status bar, `-- notice --` lines, and **the 60-second `exp` poller** all write into the form's screen area and make it unreadable. Ctrl-P passthrough does not suspend any of them.

So a `stat` fired when the operator types `train` would land in the middle of the training form and corrupt it — trading a missing feature for a broken one. The re-poll must wait until the form has closed.

This also means the existing `exp` poller has that bug today. Task 5 fixes it with the same gate.

---

### Task 1: A model of the character sheet

**Files:** Create `crates/mud-client/src/stats.rs`; modify `lib.rs`; test `crates/mud-client/tests/stats.rs`.

**Produces:** `pub struct Stats` with every field the sheet carries, and `Stats::parse(&str) -> Stats` (partial parses allowed — an absent field stays `None`/0).

The sheet, captured live from MMud Reborn 2026-08-22:

```
Name: Beef                             Lives/CP:    9/100
Race: Dark-Elf    Exp: 0               Perception:     43
Class: Ninja      Level: 1             Stealth:        56
Hits:    22/22    Armour Class:   0/0  Thievery:        0
                                       Traps:          29
                                       Picklocks:      31
Strength:  40     Agility: 50          Tracking:       26
Intellect: 50     Health:  30          Martial Arts:   51
Willpower: 30     Charm:   40          MagicRes:       35
```

- [ ] **Step 1: Write the failing test** using that exact sheet as the fixture, asserting every field, including `picklocks == 31` and `class == "Ninja"`.
- [ ] **Step 2: Run it, see it fail.**
- [ ] **Step 3: Implement.** Parse **per field over the whole text**, not column-by-column — the columns do not align between rows (`Traps` and `Picklocks` sit alone on their rows). A field ends at a two-space gutter or the next `Label:`.

  **`Class` needs its own terminator.** On this board the class column is a SINGLE space from the following `Level:` label, unlike the two-space gutters elsewhere. A two-space-only rule silently drops it. MudPlay hit exactly this and solved it with a lookahead — see `archive/FujiTerm/MudPlay/Game/StatParser.cs`'s `ClassRx`, and its test `Class_SingleSpaceBeforeNextLabel_StillCaptured`.
- [ ] **Step 4: Run, see it pass. Mutate:** drop the `Class` lookahead and confirm the class assertion fails; change `picklocks`' expected value and confirm it fails.
- [ ] **Step 5: Commit.**

---

### Task 2: The session reads it off the board

**Files:** modify `correlate.rs`, `session.rs`; test `crates/mud-client/tests/stats.rs`.

**Consumes:** Task 1's `Stats`. **Produces:** `Session::stats() -> Stats`.

- [ ] **Step 1:** Add `Kind::Stat` to the correlator, classifying `stat` and `st`. **This is not optional plumbing** — `Kind::Opaque`'s `completes` always returns `false`, so an unclassified reply is never attributed and the consumer waits out its whole deadline. That is exactly how the locked door reported a phantom timeout (`b19f862d`).
- [ ] **Step 2:** The reply is **multi-line**, so the purse's "the line after the echo" trick does not transfer. Accumulate lines after the attributed echo and terminate on the prompt. Write the failing test first, including a test that an interleaved line does not truncate the sheet.
- [ ] **Step 3:** Mirror `PurseTracker`: a `StatTracker` on the `Session`, fed from the same reader-task hook as `feed_purse`, exposed as `Session::stats()`.
- [ ] **Step 4: Mutate** — make the accumulator stop at the first line and confirm the multi-line test fails.
- [ ] **Step 5: Commit.**

---

### Task 3: Picking is something the character can do

**Files:** modify `graph.rs` (`Capabilities`), `nav.rs`, `session.rs`, `go.rs`, `tests/nav_doors.rs`.

- [ ] **Step 1:** Add `picklocks: u32` to `Capabilities`, filled by `Session::capabilities()` from `Session::stats()`.
- [ ] **Step 2:** In `nav.rs`, gate the pick loop on `self.capabilities.picklocks > 0` and **delete `NavConfig::pick_locks` entirely**, along with its mentions in `go.rs` and `fenced`. A knob the operator has to manage is the thing being removed.
- [ ] **Step 3:** Update `tests/nav_doors.rs`. `forcing_nav` becomes "a character with no Picklocks" rather than "picking switched off" — same behaviour, honest name. The new picking tests supply a character who has the skill.
- [ ] **Step 4: Mutate** — give the picking test a character with `picklocks: 0` and confirm it stops picking; give the forcing test a skilled one and confirm it stops bashing. Both must bite, or the gate is not being read.
- [ ] **Step 5: Commit.**

---

### Task 4: The spellbook is read once, not per walk

**Files:** modify `farm.rs` (`read_sheet` and its two call sites), `go.rs:273`, `session.rs`, `tui.rs`.

`read_sheet` asks the board for `inventory` and `spells` at **farm.rs:1541** (farm start), **farm.rs:1867** (dark finish walk) and **go.rs:273** (every `/go`). Its own comment concedes it "runs at every farm start and on dark finish walks".

- [ ] **Step 1:** Move the probe to realm entry and hold the resulting `Sheet` on the `Session`. The three call sites read the session's copy.
- [ ] **Step 2:** Keep the mystic detection intact — `spells` answers `"You may not list your spells. You are KAI!"` and the redirect to `powers` IS the detection (`mud_core::text::KAI_NO_SPELLS`). Losing it would silently break every mystic.
- [ ] **Step 3:** A test proving a second `/go` in one session sends no `spells`, and one proving the mystic redirect still happens exactly once at login.
- [ ] **Step 4: Mutate** — make the session return a default `Sheet` and confirm a spellbook-dependent test fails.
- [ ] **Step 5: Commit.**

---

### Task 5: Nothing polls into a full-screen form

**Files:** modify `session.rs` or `tui.rs`; test `crates/mud-client/tests/tui.rs`.

- [ ] **Step 1: Read first.** Find how a full-screen form is recognisable on the wire. If there is no reliable signal, **report `NEEDS_CONTEXT`** rather than inventing one — a wrong guess here corrupts the operator's training screen, which is worse than the missing re-poll.
- [ ] **Step 2:** Gate every timed/automatic send behind "a form is not open": the `stat` re-poll, and **the existing `exp` poller, which has this bug today** (memory `mmc-cannot-pilot-fsd-screens`, culprit 3).
- [ ] **Step 3:** Arm a "stats are stale" flag when the operator sends `train`; fire the `stat` re-poll only once an ordinary game prompt returns — i.e. after the form has closed.
- [ ] **Step 4: Mutate** — remove the gate and confirm the test showing no poll during a form fails.
- [ ] **Step 5: Commit.**

## Self-Review

**Ordering:** 1 → 2 → 3 and 1 → 2 → 4 are hard dependencies; 5 depends on 3 and 4 existing but could be written against either. Run strictly in order.

**Known gap, stated:** the sheet's wording is verified against MMud Reborn only (one live capture, 2026-08-22) and against `mud-core`'s formatter. Other boards may word it differently; keep the field labels in one table so a differing board is a contained edit.
