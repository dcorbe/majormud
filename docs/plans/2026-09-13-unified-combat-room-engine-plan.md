# Unified combat-and-room engine — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate the bot's scattered combat-state fields and the farm's separate room model into one in-room engine that the assist and the farm both use, so combat state has a single source of truth and followers gain the room awareness only the farm had.

**Architecture:** A behaviour-preserving series. First collapse the bot's combat fields into one `CombatState` type. Then lift the farm's `Here` room model into a shared module. Then give the assist that model. Then make the farm consume the engine's queries and drop its duplicate sweep and occupancy tracking. Each step keeps the existing suite green and is proven by breaking a safety property before trusting the test.

**Tech Stack:** Rust, `crates/mud-client`. Tests are `#[test]` in `crates/mud-client/tests/*.rs` and inline `#[cfg(test)]`. Build and test with `CARGO_BUILD_JOBS=1 cargo test -p mud-client -- --test-threads=1` (the box OOMs otherwise).

**Spec:** `docs/plans/2026-09-13-unified-combat-room-engine-design.md`

## Global Constraints

- Never run `rustfmt`, `cargo fmt`, or any formatter.
- Every file ends with a blank line.
- Commit after every task with a tagged message (`feat:`, `fix:`, `refactor:`, `doc:`, `test:`); never reference a plan or spec document in a commit message.
- Do not check in anything under `re/`; no test may read from `re/` or `~/mmc`.
- Build/test with one build job and one test thread: `CARGO_BUILD_JOBS=1 cargo test -p mud-client -- --test-threads=1`.
- The whole existing client suite (82 test binaries) must be green at the end of every task. It is the primary correctness gate for a behaviour-preserving refactor.
- The replay harness fixture (`tests/replay.rs::the_beef_ping_pong_fixture_does_not_loop_today`) must stay green throughout — it is the regression net for the combat state.
- Verify every subagent's "green" claim yourself by rerunning the suite before accepting a task.

---

### Task 1: Extract `CombatState` in the bot, behaviour-preserving

Collapse `engaged`, `cooling`, `switch_watch`, and `quiet_prompts` into one `CombatState` value with named transitions. No consumer sees a difference: `Bot::engaged()` still returns `Option<&str>` and every existing test passes unchanged.

**Files:**
- Create: `crates/mud-client/src/combat.rs` (the `CombatState` type and its transitions)
- Modify: `crates/mud-client/src/bot.rs` (replace the four fields with one `CombatState`; route the ~ten mutation sites through it)
- Modify: `crates/mud-client/src/lib.rs` (add `pub mod combat;` in alpha order, after `pub mod cli;`)
- Test: `crates/mud-client/tests/combat.rs` (new, unit tests per transition)

**Interfaces:**
- Consumes: the existing `Event` and `target_word`/`is_combat_off`/`is_combat_engaged`/`is_kill_line` helpers in `bot.rs`.
- Produces:
  - `pub struct CombatState` with `pub fn new() -> Self`, `Default`.
  - `pub fn engaged(&self) -> Option<&str>` — the target under attack, or None.
  - `pub fn engage(&mut self, target: &str)` — enter the fight with `target`.
  - `pub fn on_combat_off(&mut self)` — the board un-latched; clears the target, arms the wander-out cooldown against it, and opens the one-event switch window (today's `switch_watch = Some(engaged.clone())` plus the cooldown-arming, moved verbatim).
  - `pub fn on_combat_engaged(&mut self)` — if the switch window is open, restore the target and cancel its cooldown (today's `switch_watch` restore).
  - `pub fn close_switch_window(&mut self)` — any event other than a Combat Engaged closes the window with the un-latch standing (today's `switch_watch.take()` on a non-engaged event).
  - `pub fn on_kill(&mut self)` — a kill/exp-award; clears the target with no cooldown.
  - `pub fn note_quiet_prompt(&mut self, idle_after: u32) -> bool` — increment the silence counter while engaged; returns true and clears the target when it reaches `idle_after` (today's `quiet_prompts` backstop).
  - `pub fn note_blow(&mut self)` — a blow involving the target resets the silence counter.
  - `pub fn cooling_noun(&self) -> Option<&str>` and `pub fn settle_cooling(&mut self, present: bool)` — the wander-out cooldown read and the per-block settle (today's `cooling` tuple and its `*listed += 1; if !present || *listed >= 2` rule).
  - `pub fn clear(&mut self)` — force to idle (used where a block shows the target absent, and by `disengaged_by_own_cast`).

**Notes for the implementer:** This is a pure move of existing logic. Read every current use of `self.engaged`, `self.cooling`, `self.switch_watch`, and `self.quiet_prompts` in `bot.rs` (there are about ten writes and several reads across `decide`, `engage`, `on_line`, `on_vitals`, and the `on_event` prompt arm). Each maps to one `CombatState` method above with the SAME condition and order. Do not change any behaviour, any wording, or any ordering. `backstab_open` stays a `Bot` field (it drives the second-round re-send, a separate concern) — leave it alone.

- [ ] **Step 1: Write the transition tests**

Create `crates/mud-client/tests/combat.rs` with unit tests for `CombatState` covering: engage sets the target; a lone `on_combat_off` clears it and arms cooling; `on_combat_off` then `on_combat_engaged` restores the target and clears cooling; `on_combat_off` then `close_switch_window` leaves it cleared with cooling armed; `on_kill` clears with no cooling; the quiet-prompt backstop clears at the threshold; cooling settles to None after two present blocks or one absent block.

```rust
use mud_client::combat::CombatState;

#[test]
fn a_lone_combat_off_clears_and_cools() {
    let mut c = CombatState::new();
    c.engage("kobold thief");
    c.on_combat_off();
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), Some("thief"));
}

#[test]
fn a_switch_pair_restores_the_target_and_cancels_cooling() {
    let mut c = CombatState::new();
    c.engage("dark goblin archer");
    c.on_combat_off();
    c.on_combat_engaged();
    assert_eq!(c.engaged(), Some("dark goblin archer"));
    assert_eq!(c.cooling_noun(), None);
}

#[test]
fn a_non_engaged_event_closes_the_switch_window() {
    let mut c = CombatState::new();
    c.engage("kobold thief");
    c.on_combat_off();
    c.close_switch_window();
    c.on_combat_engaged(); // too late: window closed
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), Some("thief"));
}

#[test]
fn a_kill_clears_with_no_cooldown() {
    let mut c = CombatState::new();
    c.engage("big skeleton");
    c.on_kill();
    assert_eq!(c.engaged(), None);
    assert_eq!(c.cooling_noun(), None);
}
```

- [ ] **Step 2: Run the tests, verify they fail to compile (no `combat` module yet)**

Run: `CARGO_BUILD_JOBS=1 cargo test -p mud-client --test combat -- --test-threads=1`
Expected: FAIL — unresolved import `mud_client::combat`.

- [ ] **Step 3: Write `combat.rs`**

Implement `CombatState` with the methods in Interfaces, moving the exact logic from `bot.rs`'s current `switch_watch`/`cooling`/`quiet_prompts`/`engaged` handling. `cooling` is `Option<(String, u32)>` internally exactly as today; the switch window is `Option<Option<String>>` exactly as today.

- [ ] **Step 4: Route `bot.rs` through `CombatState`**

Replace the four fields with `combat: CombatState`. Rewrite each mutation site to call the matching method. Keep `Bot::engaged()` delegating to `self.combat.engaged()`. Keep the `decide` top-of-function switch-window resolution calling `on_combat_engaged` / `close_switch_window`. Keep the `on_line` kill/combat-off branch calling `on_kill` / `on_combat_off`. Keep the prompt arm calling `note_quiet_prompt(self.config.combat_idle_prompts)`.

- [ ] **Step 5: Run the combat unit tests, verify they pass**

Run: `CARGO_BUILD_JOBS=1 cargo test -p mud-client --test combat -- --test-threads=1`
Expected: PASS.

- [ ] **Step 6: Run the full client suite, verify still green**

Run: `CARGO_BUILD_JOBS=1 cargo test -p mud-client -- --test-threads=1`
Expected: all binaries pass, including `backstab_opener`, `bot`, and `replay`.

- [ ] **Step 7: Prove the net has teeth**

Temporarily make `on_combat_engaged` a no-op, run `tests/replay.rs::the_beef_ping_pong_fixture_does_not_loop_today` and `tests/backstab_opener.rs::the_mode_switch_combat_off_ends_nothing`, confirm both FAIL, then restore.

- [ ] **Step 8: Clippy and commit**

Run: `CARGO_BUILD_JOBS=1 cargo clippy -p mud-client` — no new warnings in `combat.rs` or `bot.rs`.

```bash
git add crates/mud-client/src/combat.rs crates/mud-client/src/bot.rs crates/mud-client/src/lib.rs crates/mud-client/tests/combat.rs
git commit -m "refactor(client): the bot's combat latches become one CombatState"
```

---

### Task 2: Lift the room model into a shared module

Move the farm's `Here` room model out of `world.rs`'s farm-only role into a module the assist can also own. This is a relocation and a rename to a shared home, not a behaviour change: the farm keeps feeding it exactly as today.

**Files:**
- Modify: `crates/mud-client/src/world.rs` (this already holds `Here`; confirm it has no farm-only dependency that blocks the assist from constructing one — it does not, `Here::default()` plus `here.room = Some(id)` is the whole setup)
- Modify: `crates/mud-client/src/world.rs` — add `pub fn is_clear(&self, bot: &crate::bot::Bot) -> bool`: true when no occupant is a target the bot would attack and no pile is wanted. Built from the existing `aggressive_names`/`has_target_among` and `unswept_wanted`.
- Test: `crates/mud-client/tests/world.rs` (add `is_clear` cases)

**Interfaces:**
- Consumes: the existing `Here` API (`on_event`, `aggressive_names`, `unswept_wanted`, `seeded`), and `Bot::has_target_among`, `Bot::ignores_coin`.
- Produces: `Here::is_clear(&self, bot: &Bot) -> bool`.

**Notes:** `Here` already lives in `world.rs` and is already shared-capable. The only new surface is `is_clear`, which the farm's verdict (Task 4) and the assist (Task 3) will read. Do NOT move `Here` to a new file unless it has farm-only coupling; it does not, so this task is additive.

- [ ] **Step 1: Write the `is_clear` test**

```rust
// in tests/world.rs, using the file's existing Here/painted helpers
#[test]
fn is_clear_is_false_with_a_target_and_true_when_empty() {
    // build a Here with one aggressive monster and a wanted pile -> not clear
    // fold a kill + a pickup -> clear
}
```

Write it concretely against the helpers already in `tests/world.rs` (see the `painted`/`fold` helpers there).

- [ ] **Step 2: Run it, verify it fails** (`is_clear` not defined).

Run: `CARGO_BUILD_JOBS=1 cargo test -p mud-client --test world -- --test-threads=1`

- [ ] **Step 3: Implement `is_clear`** on `Here`, composing `has_target_among(self.aggressive_names())` and `unswept_wanted(LOOT_TRIES, ...)`.

- [ ] **Step 4: Run the world test, verify it passes.**

- [ ] **Step 5: Full suite green.**

Run: `CARGO_BUILD_JOBS=1 cargo test -p mud-client -- --test-threads=1`

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/world.rs crates/mud-client/tests/world.rs
git commit -m "feat(client): the room model answers is_clear"
```

---

### Task 3: Give the assist the room model

The assist (`assist_actions` in `tui.rs`, driven by the window loop in `window.rs`) currently runs only the bot. Give it a `Here` folded from the same correlated stream the window already has, so a follower and manual `/bot` know what is alive and on the floor, and stop line-guessing.

**Files:**
- Modify: `crates/mud-client/src/window.rs` (the play loop already folds correlated events; add a `Here` beside the assist bot, fold each event into it with the same attribution rule the assist uses — `cor.answers`/`cor.elsewhere`)
- Modify: `crates/mud-client/src/tui.rs` (`assist_actions` gains access to the assist's `Here` for sweeping decisions; the assist sweeps through the model like the farm does, rather than the bot's own `has_loot`)
- Test: `crates/mud-client/tests/assist_window.rs` (a dragged follower's room model tracks occupants; a switch pair does not loop — the harness fixture through the assist path)

**Interfaces:**
- Consumes: `Here` and `Here::is_clear` (Task 2), `CombatState` via the bot (Task 1), the window's existing `Correlated` stream.
- Produces: an assist that folds room state; no new public signature the farm depends on.

**Notes:** This is the behaviour-changing step and the attribution seam. The window is hard to drive end-to-end (raw terminal), so the seam is proven in `assist_window.rs` with a scripted board, the pattern that file already uses. Prove: a dragged follower whose leader moves gets a fresh room model (the block is attributed to the drag, not elsewhere); a `look <direction>` block is treated as elsewhere and does not overwrite the model.

- [ ] **Step 1:** Write the assist-room-model tests in `tests/assist_window.rs` (scripted board: arrival, a switch pair, assert no re-engage loop and the model tracks the occupant).
- [ ] **Step 2:** Run, verify they fail.
- [ ] **Step 3:** Add the `Here` fold to the window's assist path; route assist sweeping through it.
- [ ] **Step 4:** Run the new tests, verify they pass.
- [ ] **Step 5:** Full suite green.
- [ ] **Step 6:** Commit `feat(client): the assist knows what is alive in the room`.

---

### Task 4: The farm consumes the engine; drop the duplicate sweep and occupancy

Move `StopState.verdict` to read the room model's `is_clear` and the bot's `CombatState` instead of the farm's parallel occupancy tracking, and delete the stop-owned sweep.

**Files:**
- Modify: `crates/mud-client/src/farm.rs` (`StopState`: drop `note_occupancy`'s dependence on a private occupancy read; `verdict` reads `here.is_clear(&bot)`; remove `stop_config.auto_get = false` and the `Verdict::Loot` arm, letting the (now shared) sweep run through the room model)
- Test: `crates/mud-client/tests/farm.rs`, `crates/mud-client/tests/farm_live.rs` (existing verdict/loot tests must still pass; adjust only where they asserted the now-removed `Verdict::Loot` shape)

**Interfaces:**
- Consumes: `Here::is_clear` (Task 2), `CombatState` (Task 1).
- Produces: a smaller `StopState` (dwell timer, respawn budget, pending-look/get windows only).

**Notes:** This is the highest-risk task — `verdict` has many hard-won clauses (flee recovery, blind/light ordering, pending-look/get windows). Change ONLY the occupancy and loot clauses; leave flee, blind, and the pending windows exactly as they are. The full `farm` and `farm_live` suites are the gate. If a test asserted `Verdict::Loot`, the sweep now comes from the shared model — update the assertion to the new path, do not delete the coverage.

- [ ] **Step 1:** Read `verdict` and list every clause; identify only the occupancy (`has_target_among`, `note_occupancy`) and loot (`Verdict::Loot`) clauses.
- [ ] **Step 2:** Adjust the affected tests to the new shape (sweep via the model); run, verify they fail against current code.
- [ ] **Step 3:** Rewire `verdict` to `here.is_clear(&bot)`; remove the stop-owned sweep.
- [ ] **Step 4:** Run `farm` and `farm_live`, verify green.
- [ ] **Step 5:** Full suite green.
- [ ] **Step 6:** Commit `refactor(client): the farm reads the room model instead of its own occupancy`.

---

### Task 5: Wire `finish_at` in-session

When a `/farm` job ends (any exit but a death), walk the character to `finish_at` with the existing `go_to_finish`.

**Files:**
- Modify: `crates/mud-client/src/window.rs` (on a `Phase::Done` that is not a death, run `go_to_finish` before handing the keyboard back)
- Modify: `crates/mud-client/src/tui.rs` if the job-completion handling lives there
- Test: `crates/mud-client/tests/farm_live.rs` or `window.rs` (a finished job with `finish_at` set walks there; a death does not)

**Interfaces:**
- Consumes: `go_to_finish` (already public in `farm.rs`), the window's `Job` completion.
- Produces: in-session finish behaviour matching the doc.

- [ ] **Step 1:** Write the test (finished job with `finish_at` set ends at the finish room; a `FarmEnd::Died` does not walk).
- [ ] **Step 2:** Run, verify it fails.
- [ ] **Step 3:** Wire `go_to_finish` into the window's job-done handling.
- [ ] **Step 4:** Run the test, verify it passes.
- [ ] **Step 5:** Full suite green; update `docs/mud-client.md` to say `finish_at` works in-session.
- [ ] **Step 6:** Commit `feat(client): a finished farm walks to finish_at in-session`.

---

### Task 6: Extend the replay harness to the wrapper paths

The harness replays through the bot core today. Add a mode that replays through `assist_actions` (the look-on-Combat-Off wrapper) and one through the farm verdict, so wrapper loops like the cw-blueberry look-spam are caught.

**Files:**
- Modify: `crates/mud-client/src/replay.rs` (add `replay_assist` that drives `assist_actions`, collecting its emitted commands the same way)
- Test: `crates/mud-client/tests/replay.rs` (a fixture where the assist's look-on-Combat-Off would spin, asserting the extended harness catches it and today's code does not)

**Interfaces:**
- Consumes: `assist_actions` (`tui.rs`), the existing `Trace`/`find_loops`.
- Produces: `pub fn replay_assist(text: &str, config: BotConfig) -> Trace`.

- [ ] **Step 1:** Write a fixture + test for a wrapper loop through the assist path.
- [ ] **Step 2:** Run, verify it fails (the bot-core replay does not catch it).
- [ ] **Step 3:** Implement `replay_assist`.
- [ ] **Step 4:** Run, verify the wrapper loop is caught and today's code is clean.
- [ ] **Step 5:** Full suite green.
- [ ] **Step 6:** Commit `feat(client): the replay harness also drives the assist wrapper`.

---

## Self-review notes

- **Spec coverage:** Task 1 = CombatState; Task 2 = RoomModel/`is_clear`; Task 3 = assist gains the model (the operator-visible fix); Task 4 = farm as consumer + duplication deleted; Task 5 = `finish_at` in-session; Task 6 = harness extension. All six spec migration steps map to a task.
- **Sequencing:** 1 and 2 are pure consolidation and independent of each other; 3 depends on 2; 4 depends on 1, 2; 5 is independent; 6 depends on 3. Execute 1, 2, 3, 4, 5, 6 in order.
- **Refactor testing:** for the behaviour-preserving tasks (1, 2, 4) the existing suite is the gate, supplemented by new unit tests for new surface. The mutate-before-trust step in Task 1 proves the net; Tasks 3 and 6 add genuinely new behaviour with their own failing-first tests.
- **Open questions from the spec** (assist respawn budget; where `cooling` lives) are resolved within Tasks 1 and 3 as their notes direct.

