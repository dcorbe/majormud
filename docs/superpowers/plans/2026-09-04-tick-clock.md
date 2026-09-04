# Tick Clock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The client infers the board's combat round and its four regen cycles from what it observes, carries them in the game state, and shows the countdowns in the status bar.

**Architecture:** `world.rs` gains `RegenCycle`, a phase anchor with a period, and `TickClock`, which owns the existing `RoundClock` and four regen cycles and is fed one correlated event at a time. `GameState` carries a `TickClock` and the session's reader task feeds it beside the vitals. `render_status` takes the current instant and prints the countdowns, and the play loop repaints every 250 ms so they move.

**Tech Stack:** Rust 2024 edition, tokio, the existing `mud-client` test crates. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-04-prompt-status-and-recovery-design.md`, Part 2. This plan is Phase 2 of that spec. Prior art: `~/majormud/MudPlay/Game/RegenTracker.cs`, `RegenCycle.cs`, `TickEngine.cs`.

## Global Constraints

- Never run `rustfmt` or `cargo fmt`.
- Every file ends with one newline.
- Commit after every task with a tagged message. Do not mention plans or specs in commit messages. End each commit message with a blank line and `Claude-Session: https://claude.ai/code/session_0183JngSdxae1Eg4EDaso8H1`.
- Do not commit `.claude/`, `CLAUDE.md`, or anything under `.superpowers/`.
- Do not use `/tmp`.
- No test may read from `re/`.
- Prose in comments and docs: plain words, one idea per sentence, no em dashes, no parentheses as asides, no semicolons.
- Every clock rule is pure and clock injected: functions take `now: Instant`, tests pass synthetic instants, nothing sleeps except the one scripted board test.
- Periods on the stock realm: combat round is the existing `world::ROUND` of 5.13 s, HP and mana natural regen 30 s sharing one pulse, resting HP 20 s, meditating mana 15 s. Cast window 3 s. Claim grace 750 ms.

## Rulings that shape this plan

Three places the plan departs from the spec's literal text, each with the reason:

1. **The round cycle is the existing `RoundClock`, debounced at half a period, not MudPlay's 250 ms.** The spec says `RoundClock` keeps its shape. Its half period debounce is what keeps a second blow one second into a round from sliding the phase. It subsumes the 250 ms.
2. **The artifact window opens on the attributed echo of a cast, read from the event stream.** The board echoes every accepted command and the correlator credits the echo to our send, so `TickClock` sees `Line("cast heal")` with `answers` set and needs no plumbing from the writer task. A split echo misses the window on rare occasions, and the window is a heuristic.
3. **The per tick amount average is deferred.** Nothing displays or consumes it yet, and carrying a float would cost `GameState` its `Eq`. The anchors and phase are the whole of tick detection. The sample drop rule in the spec belongs to the amount and goes with it.

---

### Task 1: Regen cycles beside the round clock

**Files:**
- Modify: `crates/mud-client/src/world.rs:18-86` (constants, `RoundClock`)
- Test: `crates/mud-client/tests/world.rs`

**Interfaces:**
- Produces: `pub const REGEN_NATURAL`, `REGEN_REST`, `REGEN_MEDITATE`, `CAST_WINDOW`, `CLAIM_GRACE` in `mud_client::world`.
- Produces: `pub struct RegenCycle` with `new(period)`, `period()`, `active()`, `start(now)`, `stop()`, `is_due(now)`, `observe(now)`, `time_to_next(now) -> Option<Duration>`. Derives `Debug, Clone, PartialEq, Eq`.
- Produces: `RoundClock` derives `Debug, Clone, PartialEq, Eq` and gains `pub fn locked(&self) -> bool`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/world.rs`. Extend the `use mud_client::world::{...}` line with `RegenCycle, REGEN_NATURAL, CLAIM_GRACE`.

```rust
// ---------------------------------------------------------------------
// RegenCycle: one phase anchor with a period. Pure and clock-injected.
// ---------------------------------------------------------------------

#[test]
fn an_unstarted_cycle_has_no_next_tick() {
    let c = RegenCycle::new(REGEN_NATURAL);
    assert!(!c.active());
    assert_eq!(c.time_to_next(Instant::now()), None);
    assert!(!c.is_due(Instant::now()));
}

#[test]
fn start_anchors_once_and_stop_forgets() {
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.start(t0);
    // A second start does not move a running anchor.
    c.start(t0 + Duration::from_secs(5));
    assert_eq!(c.time_to_next(t0 + Duration::from_secs(5)), Some(Duration::from_secs(25)));
    c.stop();
    assert!(!c.active());
    assert_eq!(c.time_to_next(t0), None);
}

#[test]
fn time_to_next_counts_in_period_steps_from_the_anchor() {
    // A silent tick at full HP moves no anchor. The projection still
    // lands on the board's cadence.
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.observe(t0);
    assert_eq!(c.time_to_next(t0 + Duration::from_secs(70)), Some(Duration::from_secs(20)));
    assert_eq!(c.time_to_next(t0), Some(REGEN_NATURAL));
}

#[test]
fn a_cycle_is_due_a_period_after_its_anchor_less_the_grace() {
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.observe(t0);
    assert!(!c.is_due(t0 + Duration::from_secs(10)));
    assert!(c.is_due(t0 + REGEN_NATURAL - CLAIM_GRACE));
    assert!(c.is_due(t0 + REGEN_NATURAL + Duration::from_secs(40)));
}

#[test]
fn observe_reanchors_a_running_cycle() {
    let mut c = RegenCycle::new(REGEN_NATURAL);
    let t0 = Instant::now();
    c.observe(t0);
    let t1 = t0 + Duration::from_secs(31);
    c.observe(t1);
    assert_eq!(c.time_to_next(t1), Some(REGEN_NATURAL));
}

#[test]
fn the_round_clock_says_whether_it_is_locked() {
    let mut clock = RoundClock::new();
    assert!(!clock.locked());
    clock.observe(Instant::now());
    assert!(clock.locked());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test world cycle 2>&1 | tail -5`
Expected: compile error, `RegenCycle` not found.

- [ ] **Step 3: Add the cycle**

In `crates/mud-client/src/world.rs`, after the `ROUND` constant:

```rust
/// Cadence of the board's regen ticks on the stock realm, measured by
/// MudPlay. Passive HP and mana share one 30 second pulse, resting HP
/// ticks every 20 seconds, meditating mana every 15 seconds. The board
/// announces none of them. They are inferred from a pool rising
/// between two prompts.
pub const REGEN_NATURAL: Duration = Duration::from_secs(30);
pub const REGEN_REST: Duration = Duration::from_secs(20);
pub const REGEN_MEDITATE: Duration = Duration::from_secs(15);
/// A pool rising this soon after one of our own casts is the spell
/// landing, not a tick.
pub const CAST_WINDOW: Duration = Duration::from_secs(3);
/// How early a gain may land and still be read as the cycle's own tick.
pub const CLAIM_GRACE: Duration = Duration::from_millis(750);

/// One regen cadence: a phase anchor and a period.
///
/// Ported from MudPlay's `RegenCycle`. The anchor is the instant of the
/// last observed tick, or of the start when nothing has been observed
/// yet. The next tick is projected in exact period steps from it, so a
/// tick nobody could see, a pool already full, keeps the phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegenCycle {
    period: Duration,
    anchor: Option<Instant>,
}

impl RegenCycle {
    pub fn new(period: Duration) -> Self {
        RegenCycle { period, anchor: None }
    }

    pub fn period(&self) -> Duration {
        self.period
    }

    pub fn active(&self) -> bool {
        self.anchor.is_some()
    }

    /// Anchor here unless already running.
    pub fn start(&mut self, now: Instant) {
        if self.anchor.is_none() {
            self.anchor = Some(now);
        }
    }

    pub fn stop(&mut self) {
        self.anchor = None;
    }

    /// Is a gain at `now` this cycle's own tick: running, and a period
    /// has passed since the anchor, less the grace.
    pub fn is_due(&self, now: Instant) -> bool {
        match self.anchor {
            Some(anchor) => now.saturating_duration_since(anchor) + CLAIM_GRACE >= self.period,
            None => false,
        }
    }

    /// A real tick landed: re-anchor on it.
    pub fn observe(&mut self, now: Instant) {
        self.anchor = Some(now);
    }

    /// Time until the next projected tick, in exact period steps from
    /// the anchor. None while the cycle is not running.
    pub fn time_to_next(&self, now: Instant) -> Option<Duration> {
        let anchor = self.anchor?;
        let elapsed = now.saturating_duration_since(anchor);
        let steps = (elapsed.as_nanos() / self.period.as_nanos()) as u32;
        let next = anchor + self.period * (steps + 1);
        Some(next.saturating_duration_since(now))
    }
}
```

Give `RoundClock` derives and a `locked` method:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundClock {
```

```rust
    /// Has a volley ever locked the phase.
    pub fn locked(&self) -> bool {
        self.last_burst.is_some()
    }
```

- [ ] **Step 4: Run the world suite**

Run: `cargo test -p mud-client --test world 2>&1 | tail -3`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/world.rs crates/mud-client/tests/world.rs
git commit -m "feat(client): a regen cycle is a phase anchor with a period"
```

---

### Task 2: The tick clock reads ticks off events

**Files:**
- Modify: `crates/mud-client/src/world.rs` (after `RegenCycle`)
- Modify: `crates/mud-client/src/correlate.rs:184-188` (beside `is_movement`)
- Test: `crates/mud-client/tests/world.rs`

**Interfaces:**
- Consumes: `RegenCycle`, `RoundClock::locked`, the constants from Task 1. `Status` from `mud_client::events`. `Correlated` from `mud_client::correlate`.
- Produces: `pub fn is_cast(cmd: &str) -> bool` in `mud_client::correlate`.
- Produces: `pub struct TickClock` in `mud_client::world` with public fields `round: RoundClock`, `hp_natural`, `hp_rest`, `mana_natural`, `mana_meditate: RegenCycle`, and methods `new()`, `on_event(&mut self, cor: &Correlated, now: Instant)`, `time_to_round(&self, now) -> Option<Duration>`. Derives `Debug, Clone, PartialEq, Eq, Default`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/world.rs`. Extend the imports: `use mud_client::events::{Event, RoomView, Status};` and add `TickClock, REGEN_REST, REGEN_MEDITATE, CAST_WINDOW` to the world import.

```rust
// ---------------------------------------------------------------------
// TickClock: the board never announces a tick. A pool rising between
// two prompts is one, a volley is a round, and our own cast is neither.
// ---------------------------------------------------------------------

fn prompt(hp: i32, mana: Option<i32>, status: Option<Status>) -> Correlated {
    unsolicited(Event::Prompt { hp, mana, status })
}

fn hit() -> Event {
    Event::CombatHit {
        attacker: mud_client::events::Actor::Other("The giant rat".into()),
        target: mud_client::events::Actor::You,
        damage: 3,
    }
}

#[test]
fn the_first_prompt_only_sets_the_baseline() {
    let mut c = TickClock::new();
    c.on_event(&prompt(30, None, None), Instant::now());
    assert!(!c.hp_natural.active());
}

#[test]
fn hp_rising_between_prompts_anchors_the_natural_cycle() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(32, None, None), t1);
    assert_eq!(c.hp_natural.time_to_next(t1), Some(REGEN_NATURAL));
    // Natural HP and natural mana ride one server pulse.
    assert_eq!(c.mana_natural.time_to_next(t1), Some(REGEN_NATURAL));
}

#[test]
fn hp_falling_anchors_nothing() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    c.on_event(&prompt(25, None, None), t0 + Duration::from_secs(5));
    assert!(!c.hp_natural.active());
}

#[test]
fn a_gain_a_period_later_is_the_cycles_own_tick() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(32, None, None), t1);
    let t2 = t1 + REGEN_NATURAL - Duration::from_millis(500);
    c.on_event(&prompt(34, None, None), t2);
    assert_eq!(c.hp_natural.time_to_next(t2), Some(REGEN_NATURAL));
}

#[test]
fn a_gain_well_before_the_period_reanchors_the_natural_cycle() {
    // MudPlay's rule: a gain no running cycle can claim anchors the
    // natural cycle on itself. The board's cadence corrects it on the
    // next real tick.
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(32, None, None), t1);
    let t2 = t1 + Duration::from_secs(10);
    c.on_event(&prompt(33, None, None), t2);
    assert_eq!(c.hp_natural.time_to_next(t2), Some(REGEN_NATURAL));
}

#[test]
fn resting_starts_the_rest_cycle_and_a_bare_prompt_stops_it() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, Some(Status::Resting)), t0);
    assert!(c.hp_rest.active());
    assert!(!c.mana_meditate.active());
    c.on_event(&prompt(30, None, None), t0 + Duration::from_secs(1));
    assert!(!c.hp_rest.active());
}

#[test]
fn a_due_gain_while_resting_credits_the_rest_cycle_not_the_natural_one() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, Some(Status::Resting)), t0);
    let t1 = t0 + REGEN_REST;
    c.on_event(&prompt(33, None, Some(Status::Resting)), t1);
    assert_eq!(c.hp_rest.time_to_next(t1), Some(REGEN_REST));
    assert!(!c.hp_natural.active());
}

#[test]
fn a_gain_just_after_our_cast_is_the_spell_not_a_tick() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, Some(10), None), t0);
    let cast = t0 + Duration::from_secs(1);
    c.on_event(&answering(Event::Line("cast heal".into()), ASK), cast);
    c.on_event(&prompt(40, Some(6), None), cast + Duration::from_secs(1));
    assert!(!c.hp_natural.active());
    // Past the window a gain counts again.
    let later = cast + CAST_WINDOW + Duration::from_secs(1);
    c.on_event(&prompt(42, Some(6), None), later);
    assert_eq!(c.hp_natural.time_to_next(later), Some(REGEN_NATURAL));
}

#[test]
fn a_cast_line_nobody_answered_opens_no_window() {
    // A monster's "attempted to cast" din is unattributed and is not
    // our spell.
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, None, None), t0);
    c.on_event(&unsolicited(Event::Line("cast heal".into())), t0 + Duration::from_secs(1));
    let t1 = t0 + Duration::from_secs(2);
    c.on_event(&prompt(32, None, None), t1);
    assert_eq!(c.hp_natural.time_to_next(t1), Some(REGEN_NATURAL));
}

#[test]
fn a_volley_locks_the_round() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    assert_eq!(c.time_to_round(t0), None);
    c.on_event(&unsolicited(hit()), t0);
    assert_eq!(c.time_to_round(t0 + Duration::from_secs(1)), Some(ROUND - Duration::from_secs(1)));
}

#[test]
fn meditating_starts_the_meditate_cycle_and_a_due_gain_credits_it() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, Some(10), Some(Status::Meditating)), t0);
    assert!(c.mana_meditate.active());
    let t1 = t0 + REGEN_MEDITATE;
    c.on_event(&prompt(30, Some(12), Some(Status::Meditating)), t1);
    assert_eq!(c.mana_meditate.time_to_next(t1), Some(REGEN_MEDITATE));
    assert!(!c.mana_natural.active());
}

#[test]
fn mana_rising_anchors_the_natural_mana_cycle_and_the_hp_pulse() {
    let mut c = TickClock::new();
    let t0 = Instant::now();
    c.on_event(&prompt(30, Some(10), None), t0);
    let t1 = t0 + Duration::from_secs(5);
    c.on_event(&prompt(30, Some(12), None), t1);
    assert_eq!(c.mana_natural.time_to_next(t1), Some(REGEN_NATURAL));
    assert_eq!(c.hp_natural.time_to_next(t1), Some(REGEN_NATURAL));
}

#[test]
fn a_room_block_moves_no_clock() {
    let mut c = TickClock::new();
    let before = c.clone();
    c.on_event(&unsolicited(Event::RoomSeen(view(&[]))), Instant::now());
    assert_eq!(c, before);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test world 2>&1 | tail -5`
Expected: compile error, `TickClock` not found.

- [ ] **Step 3: Add `is_cast`**

In `crates/mud-client/src/correlate.rs`, after `is_movement`:

```rust
/// Is this command a spell cast? The tick clock uses it to keep a
/// cast's own healing from reading as a regen tick.
pub fn is_cast(cmd: &str) -> bool {
    matches!(kind_of(cmd), Kind::Cast)
}
```

- [ ] **Step 4: Add the tick clock**

In `crates/mud-client/src/world.rs`, after `RegenCycle`. Add `use crate::correlate::Correlated;` and `use crate::events::{Event, Status};` to the imports if `Event` is not already imported there, merging with whatever the file imports today.

```rust
/// The board's clocks, inferred. It announces no tick, so the round is
/// read off volleys and the regen cycles off a pool rising between two
/// prompts. Ported from MudPlay's `TickEngine` and `RegenTracker`.
///
/// The rules, in the order `on_event` applies them:
///
/// 1. Any hit or miss line is a volley and locks the round.
/// 2. The attributed echo of one of our own casts opens a window in
///    which a rising pool is the spell landing, not a tick.
/// 3. On every prompt the rest and meditate cycles follow the status:
///    running while it says so, stopped when it stops.
/// 4. A pool rising outside the cast window credits whichever running
///    cycle is due, both if both are. If none is, the natural cycle
///    anchors on the gain. Natural HP and natural mana share one server
///    pulse, so a natural credit on one starts the other if it is not
///    running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickClock {
    pub round: RoundClock,
    pub hp_natural: RegenCycle,
    pub hp_rest: RegenCycle,
    pub mana_natural: RegenCycle,
    pub mana_meditate: RegenCycle,
    last_hp: Option<i32>,
    last_mana: Option<i32>,
    last_cast: Option<Instant>,
}

impl Default for TickClock {
    fn default() -> Self {
        TickClock::new()
    }
}

impl TickClock {
    pub fn new() -> Self {
        TickClock {
            round: RoundClock::new(),
            hp_natural: RegenCycle::new(REGEN_NATURAL),
            hp_rest: RegenCycle::new(REGEN_REST),
            mana_natural: RegenCycle::new(REGEN_NATURAL),
            mana_meditate: RegenCycle::new(REGEN_MEDITATE),
            last_hp: None,
            last_mana: None,
            last_cast: None,
        }
    }

    /// Fold one correlated event at `now`.
    pub fn on_event(&mut self, cor: &Correlated, now: Instant) {
        match &cor.event {
            Event::CombatHit { .. } | Event::CombatMiss { .. } => self.round.observe(now),
            Event::Line(line)
                if cor.answers.is_some()
                    && crate::correlate::is_cast(crate::correlate::strip_decoration(line.trim())) =>
            {
                self.last_cast = Some(now);
            }
            Event::Prompt { hp, mana, status } => self.on_prompt(*hp, *mana, status.as_ref(), now),
            _ => {}
        }
    }

    /// Time until the next round. None until a volley has locked one.
    pub fn time_to_round(&self, now: Instant) -> Option<Duration> {
        self.round
            .locked()
            .then(|| self.round.next_round_after(now).saturating_duration_since(now))
    }

    fn on_prompt(&mut self, hp: i32, mana: Option<i32>, status: Option<&Status>, now: Instant) {
        match status {
            Some(Status::Resting) => self.hp_rest.start(now),
            _ => self.hp_rest.stop(),
        }
        match status {
            Some(Status::Meditating) => self.mana_meditate.start(now),
            _ => self.mana_meditate.stop(),
        }
        let quiet = self
            .last_cast
            .is_none_or(|cast| now.saturating_duration_since(cast) >= CAST_WINDOW);

        let hp_rose = self.last_hp.is_some_and(|last| hp > last);
        self.last_hp = Some(hp);
        if hp_rose && quiet {
            let rest = self.hp_rest.is_due(now);
            if rest {
                self.hp_rest.observe(now);
            }
            let mut natural = self.hp_natural.is_due(now);
            if natural {
                self.hp_natural.observe(now);
            }
            if !rest && !natural {
                self.hp_natural.observe(now);
                natural = true;
            }
            if natural {
                self.mana_natural.start(now);
            }
        }

        let Some(mana) = mana else {
            return;
        };
        let mana_rose = self.last_mana.is_some_and(|last| mana > last);
        self.last_mana = Some(mana);
        if mana_rose && quiet {
            let meditate = self.mana_meditate.is_due(now);
            if meditate {
                self.mana_meditate.observe(now);
            }
            let mut natural = self.mana_natural.is_due(now);
            if natural {
                self.mana_natural.observe(now);
            }
            if !meditate && !natural {
                self.mana_natural.observe(now);
                natural = true;
            }
            if natural {
                self.hp_natural.start(now);
            }
        }
    }
}
```

- [ ] **Step 5: Run the world suite and the whole client suite**

Run: `cargo test -p mud-client --test world 2>&1 | tail -3; cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: all pass.

- [ ] **Step 6: Mutation check**

Temporarily change `if hp_rose && quiet` to `if hp_rose`. Run `cargo test -p mud-client --test world just_after_our_cast`. Expected: `a_gain_just_after_our_cast_is_the_spell_not_a_tick` fails. Revert and confirm it passes.

- [ ] **Step 7: Commit**

```bash
git add crates/mud-client/src/world.rs crates/mud-client/src/correlate.rs crates/mud-client/tests/world.rs
git commit -m "feat(client): the tick clock reads the round and the regen cycles off events"
```

---

### Task 3: The game state carries the tick clock

**Files:**
- Modify: `crates/mud-client/src/session.rs:84-93` (`GameState`), `:1028-1040` (`apply_event`), and its two call sites in the reader task (`state_tx.send_if_modified(|s| apply_event(s, &cor))`)
- Modify: every `GameState { .. }` literal in `crates/mud-client/tests` (the compiler lists them)
- Test: `crates/mud-client/tests/session_correlate.rs`

**Interfaces:**
- Consumes: `TickClock` from Task 2.
- Produces: `GameState.ticks: TickClock`. `apply_event(state, cor, now: Instant) -> bool` returns true when the vitals, the room, the status, or any clock changed.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/session_correlate.rs`:

```rust
/// A board whose HP rises between two prompts, then rests.
async fn regen_board() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"\r\n[HP=30]:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sock.write_all(b"\r\n[HP=32]:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sock.write_all(b"\r\n[HP=32 (Resting) ]:").await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    addr
}

#[tokio::test]
async fn the_game_state_carries_the_tick_clock() {
    let addr = regen_board().await;
    let session = session_for(addr).await;
    let mut state = session.state();
    state_reaches(&mut state, "natural cycle anchored", |s| s.ticks.hp_natural.active()).await;
    state_reaches(&mut state, "rest cycle running", |s| s.ticks.hp_rest.active()).await;
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mud-client --test session_correlate carries_the_tick_clock`
Expected: compile error, no field `ticks`.

- [ ] **Step 3: Add the field and feed it**

In `crates/mud-client/src/session.rs`:

```rust
pub struct GameState {
    pub hp: i32,
    pub mana: Option<i32>,
    pub room: Option<RoomView>,
    /// The word the last prompt painted: resting, meditating, or
    /// nothing. Tracked on every prompt, because the board has no
    /// wording for the end of a rest. The prompt is the only signal.
    pub status: Option<crate::events::Status>,
    /// The board's round and regen cycles, inferred from what arrives.
    /// See [`crate::world::TickClock`].
    pub ticks: crate::world::TickClock,
}
```

```rust
/// Fold an event into the rolling state; returns whether it changed.
fn apply_event(state: &mut GameState, cor: &Correlated, now: Instant) -> bool {
    let ticks_before = state.ticks.clone();
    state.ticks.on_event(cor, now);
    let ticked = state.ticks != ticks_before;
    let changed = match &cor.event {
        Event::Prompt { hp, mana, status } => {
            let changed =
                state.hp != *hp || state.mana != *mana || state.status != *status;
            state.hp = *hp;
            state.mana = *mana;
            state.status = status.clone();
            changed
        }
        Event::RoomSeen(room) => {
            if cor.elsewhere {
                return ticked;
            }
            state.room = Some(room.clone());
            true
        }
        _ => false,
    };
    changed || ticked
}
```

Keep the existing comment on the `RoomSeen` arm about peeks. At both call sites in the reader task, the batch loop and the finish tail, change the line to:

```rust
                            let now = Instant::now();
                            let cor = guard.on_event(ev, now);
                            state_tx.send_if_modified(|s| apply_event(s, &cor, now));
```

using the same `now` the correlator was given, so the two agree.

Then build. Add `ticks: mud_client::world::TickClock::new(),` to every `GameState { .. }` literal the compiler names in the tests.

- [ ] **Step 4: Run the suites**

Run: `cargo test -p mud-client --test session_correlate 2>&1 | tail -3; cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/session.rs crates/mud-client/tests
git commit -m "feat(client): the game state carries the tick clock"
```

---

### Task 4: The status bar shows the countdowns

**Files:**
- Modify: `crates/mud-client/src/tui.rs:998-1015` (`redraw_bottom`), `:1038-1070` (`render_status`), `:1222-1245` (`repaint`), the play loop's timers near `:201` and the `tokio::select!` at `:234`
- Modify: `crates/mud-client/src/bin/mmc.rs:446-459` (the one other `render_status` caller)
- Modify: every `render_status(` call in `crates/mud-client/tests/tui.rs` (19 sites, mechanical)
- Modify: `docs/mud-client.md` "The status bar" section
- Test: `crates/mud-client/tests/tui.rs`

**Interfaces:**
- Consumes: `GameState.ticks`, `TickClock::time_to_round`, `RegenCycle::time_to_next`, `RegenCycle::active`.
- Produces: `render_status(state, now: Instant, target, phase, room_id, exp_per_min, level, assist, width)`. The `now` parameter sits second.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/tui.rs`. Add `use std::time::{Duration, Instant};` and `use mud_client::world::{TickClock, ROUND};` and `use mud_client::correlate::Correlated;` and `use mud_client::events::{Actor, Event};` to the imports, merging with the existing `events` import.

```rust
fn ticks_after(events: &[(Event, Duration)], t0: Instant) -> TickClock {
    let mut clock = TickClock::new();
    for (ev, at) in events {
        let cor = Correlated { event: ev.clone(), answers: None, elsewhere: false };
        clock.on_event(&cor, t0 + *at);
    }
    clock
}

#[test]
fn status_line_shows_dashes_until_a_clock_is_locked() {
    let state = GameState {
        hp: 42,
        mana: None,
        room: None,
        status: None,
        ticks: TickClock::new(),
    };
    let s = render_status(&state, Instant::now(), "mbbs", None, Fix::Unknown, None, None, false, 120);
    assert!(s.contains("| Tick - | HP - |"), "{s}");
    // No pool, no mana countdown.
    assert!(!s.contains("MA "), "{s}");
}

#[test]
fn status_line_counts_down_the_round_and_the_regen_cycles() {
    let t0 = Instant::now();
    let hit = Event::CombatHit {
        attacker: Actor::Other("The giant rat".into()),
        target: Actor::You,
        damage: 2,
    };
    let ticks = ticks_after(
        &[
            (Event::Prompt { hp: 30, mana: Some(10), status: None }, Duration::ZERO),
            (Event::Prompt { hp: 32, mana: Some(12), status: None }, Duration::from_secs(1)),
            (hit, Duration::from_secs(2)),
        ],
        t0,
    );
    let state = GameState { hp: 32, mana: Some(12), room: None, status: None, ticks };
    let now = t0 + Duration::from_secs(3);
    let s = render_status(&state, now, "mbbs", None, Fix::Unknown, None, None, false, 120);
    let round = (ROUND - Duration::from_secs(1)).as_secs_f64();
    assert!(s.contains(&format!("Tick {round:.1} | HP 28.0 | MA 28.0")), "{s}");
}

#[test]
fn status_line_shows_the_rest_cycle_beside_the_natural_one_while_resting() {
    let t0 = Instant::now();
    let ticks = ticks_after(
        &[
            (Event::Prompt { hp: 30, mana: None, status: None }, Duration::ZERO),
            (Event::Prompt { hp: 32, mana: None, status: None }, Duration::from_secs(1)),
            (Event::Prompt { hp: 32, mana: None, status: Some(Status::Resting) }, Duration::from_secs(2)),
        ],
        t0,
    );
    let state = GameState { hp: 32, mana: None, room: None, status: Some(Status::Resting), ticks };
    let now = t0 + Duration::from_secs(4);
    let s = render_status(&state, now, "mbbs", None, Fix::Unknown, None, None, false, 120);
    assert!(s.contains("HP 27.0/18.0"), "{s}");
}
```

- [ ] **Step 2: Rewrite the existing calls mechanically**

Every existing call in `crates/mud-client/tests/tui.rs` passes the state first. Run from the repo root:

```bash
perl -pi -e 's/render_status\((&\w+),/render_status($1, Instant::now(),/g' crates/mud-client/tests/tui.rs
```

Then confirm with `grep -c "Instant::now()" crates/mud-client/tests/tui.rs` that the 19 old calls plus the new ones carry it. Add `ticks: TickClock::new(),` to every existing `GameState { .. }` literal in that file if Task 3 did not already.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test tui 2>&1 | tail -5`
Expected: compile error, `render_status` takes 8 arguments but 9 were supplied.

- [ ] **Step 4: Render the countdowns**

In `crates/mud-client/src/tui.rs`, add `now: std::time::Instant` as the second parameter of `render_status` and of `redraw_bottom`, and pass it through. In `repaint`, pass `std::time::Instant::now()` to `redraw_bottom`. In `render_status`, after the status segment and before the room:

```rust
    s.push_str(&format!(" | {}", tick_readout(state, now)));
```

Add the helper next to `render_status`:

```rust
/// The clock countdowns: the round, then HP, then mana when the
/// character has a pool. A cycle nobody has observed yet shows `-`. The
/// second number in a pair is the rest or meditate cycle, shown only
/// while it runs.
fn tick_readout(state: &GameState, now: std::time::Instant) -> String {
    fn secs(d: Option<std::time::Duration>) -> String {
        match d {
            Some(d) => format!("{:.1}", d.as_secs_f64()),
            None => "-".into(),
        }
    }
    let t = &state.ticks;
    let mut s = format!("Tick {} | HP {}", secs(t.time_to_round(now)), secs(t.hp_natural.time_to_next(now)));
    if t.hp_rest.active() {
        s.push_str(&format!("/{}", secs(t.hp_rest.time_to_next(now))));
    }
    if state.mana.is_some() {
        s.push_str(&format!(" | MA {}", secs(t.mana_natural.time_to_next(now))));
        if t.mana_meditate.active() {
            s.push_str(&format!("/{}", secs(t.mana_meditate.time_to_next(now))));
        }
    }
    s
}
```

In `crates/mud-client/src/bin/mmc.rs`, pass `std::time::Instant::now()` as the second argument of `render_status`.

- [ ] **Step 5: Repaint on a timer**

In the play loop in `tui.rs`, beside `let mut level_tick = tokio::time::interval(LEVEL_POLL);`:

```rust
    // The countdowns in the bar move between events, so the bar is
    // repainted on its own short timer as well as on every event.
    let mut tick_paint = tokio::time::interval(std::time::Duration::from_millis(250));
```

And a new arm in the `tokio::select!`, placed right before the `level_tick` arm, calling `repaint` with exactly the arguments the `level_tick` arm's neighbours use:

```rust
            _ = tick_paint.tick() => {
                repaint(&mut out, &state_rx, target, job.as_ref(), here, exp.per_minute(exp_since.elapsed()), level, assist.is_some(), &editor, cols, rows)?;
            }
```

- [ ] **Step 6: Run the suites**

Run: `cargo build --workspace 2>&1 | grep -E "^(error|warning)"; cargo test -p mud-client --test tui 2>&1 | tail -3; cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: no errors or warnings, all pass.

- [ ] **Step 7: Document it**

In `docs/mud-client.md`, at the end of "The status bar" section and before `## Doors`, add:

```markdown
### The tick clock

The board never says when it ticks. The client infers three clocks and
shows their countdowns in the bar:

```
 HP 42 MA 12 (Resting) | Tick 3.2 | HP 12.3/4.5 | MA 21.0 | Dark Cave [1/2160] | mbbs
```

`Tick` is the combat round, locked by any hit or miss line and 5.13
seconds long. `HP` and `MA` are the regen cycles. Passive HP and mana
share one 30 second pulse, which the client anchors on a pool rising
between two prompts. While the prompt says resting a second HP number
appears, the 20 second rest tick. While it says meditating a second MA
number appears, the 15 second meditate tick. A dash means nothing has
locked that clock yet. A pool rising within three seconds of one of your
own casts is the spell landing and moves no clock.

The cadence is MudPlay's measurement of the stock board. The bar is
repainted four times a second so the numbers move between events.
```

- [ ] **Step 8: Commit**

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/src/bin/mmc.rs crates/mud-client/tests/tui.rs docs/mud-client.md
git commit -m "feat(client): the status bar counts down the round and the regen ticks"
```

---

### Task 5: Verify and ship

**Files:**
- None modified. This task gates the phase.

- [ ] **Step 1: Whole workspace**

Run: `cargo test --workspace 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c`
Expected: every line reads `ok`, zero `FAILED`.

- [ ] **Step 2: Replay the capture**

Run: `cargo run -q -p mud-client --example replay_assist -- test ~/.config/mmc/test.toml 2>&1 | tail -3`
Expected: the replay still runs to the end of the capture without panicking. It does not print the clocks.

- [ ] **Step 3: Release build**

Run: `cargo build --release -p mud-client 2>&1 | tail -1; readlink -f ~/.local/bin/mmc`
Expected: a fresh `target/release/mmc`, symlinked.

- [ ] **Step 4: Report**

State the commits, the test counts, and that a live `mmc play` session shows the countdowns in the bar after restart.
