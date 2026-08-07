# Client Position Tracking Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Stop the client's idea of where the character is standing from silently going wrong, and make the map show that position live.

**Architecture:** Three defects share one cause — the client turns *any* room block into a position, with no record of what the block was evidence *of* or whether the answer was actually resolved. We fix it in two moves. First, the correlator learns that a `look <direction>` block describes the room *next door*, so a peek can no longer be read as an arrival. Second, the tracked position stops being an `Option<RoomId>` (which cannot say "I am unsure") and becomes a `Fix` with `Confirmed`/`Stale`/`Unknown` states, so the compiler forces `/go`, `/roam`, the status bar and the map each to say which they can live with. The live map then falls out cheaply, because there is finally a trustworthy position to push into it.

**Tech Stack:** Rust, tokio (`watch` + `broadcast` channels), crossterm for raw terminal I/O, hand-written ANSI for all rendering. Tests are integration tests under `crates/mud-client/tests/`; there are no inline `#[test]` modules in `src/`.

---

## Background: three symptoms, one cause

Reported by the operator:

1. The map does not follow the character during manual play.
2. `/go` or a loop started after manual movement desyncs and ends up lost.
3. Looking in a direction makes the client think the character moved.

Symptom 3 is the trigger and is confirmed in code. `kind_of` in `crates/mud-client/src/correlate.rs` files both look forms under one kind:

```rust
if cmd == "look" || cmd.starts_with("look ") { return Kind::Look; }
```

`completes` accepts a `RoomSeen` as the reply to `Kind::Look`, so the neighbour's block becomes `state.room`. `Navigator::localize` (`crates/mud-client/src/nav.rs:843-850`) then searches the current room **plus every neighbour** by name, matches the neighbour, and the tracked position steps one room in the direction looked. Every peek shifts it.

Symptom 1 has a second cause: `here = track(nav, here, &room).or(here)` (`crates/mud-client/src/tui.rs:294`). When localization fails — and `localize_view` only answers uniquely for **18.7% of the world** (`crates/mud-client/src/nav.rs:788`) — the `.or(here)` keeps the previous value, and a stale position is byte-identical to a confirmed one.

Symptom 2 follows from either: `/go` passes `here` to `lost::place`, whose name-only one-hop shortcut returns `Placed { at, steps: 0 }` on a hit with no exit check (`crates/mud-client/src/lost.rs:293-304`). A wrong hint gets rubber-stamped and the whole walk routes from a room the character was never in. The farm already avoids this trap deliberately (`crates/mud-client/src/farm.rs:2326-2338`); `/go` and `locate_start` do not.

**Rejected alternative:** an earlier proposal was to make `/go` call `lost::relocalize` unconditionally instead of `place`. Do not do this. `place` short-circuits at zero steps; `relocalize` answers without moving only for the 18.7% unique-on-sight rooms and otherwise **walks the character up to `BUDGET = 12` steps** doing candidate elimination. That would make `/go` wander before every walk from any non-unique room — most of the world — mid-play, into whatever is standing there. The defect is not that `place` is used; it is that a hint of unknown trustworthiness is handed to it. Fix the hint's provenance and the fast path stays.

**Measured on the live board, 2026-08-07.** No capture in this repo contained a directional look (3553 bare `look`s, zero directional ones) because the client's automation never sends one — only an operator does, by hand. That gap is now closed by `crates/mud-client/scripts/look_vantage.lua` and `look_vantage2.lua`, run against the live board as Salad. Standing in **Slum Street**, `look north` returned:

```text
look north
Slum Entrance
    The narrow path is barricaded heavily by a makeshift construction of heavy
wooden beams. Guards flank the only opening in this wall, ...
Also here: guardsman, large guardsman, large guardsman.
Obvious exits: north, south
[HP=79/MA=20]:
```

and the bare `look` immediately after reported **Slum Street** again. So, confirmed on this board:

- A directional look prints a **full room block for the adjacent room** — cyan name, description, `Also here:`, `Obvious exits:` — byte-shaped exactly like an arrival. `parse.rs` will therefore emit `Event::RoomSeen` for it, and nothing in the block itself says it is a peek.
- **The character does not move.** Every `look <dir>` in the probe was bracketed by bare looks that reported the same room.
- The block carries the **neighbour's occupants**. This is why Task 1 gates `state.room` rather than only gating position: `RoomView::also_here` feeds `bot::Bot::aggressive_here`, so an ungated peek can report three guardsmen as standing next to you when they are a room away. A position that drifts is bad; an assist bot that opens on a monster in another room is worse.
- The refusal wording for a direction with no exit is **`There are no exits to the east!`**. The existing `Kind::Look` arm in `completes` already matches on `there are no exits`, so widening that arm to `Kind::Look | Kind::LookDir` (Task 1, Step 5) is sufficient — do not invent a new wording for it.

Note also that the `ESC[0;37;40m` preamble the OmegaMUD reference client uses to tell a look from a move is **not reliable here** — cwrun2 and cwrun7 emit zero of them across 699 room renders. That is why detection is done on the command text we sent rather than on the wire bytes, and why no further capture is needed.

---

## Prerequisites

**Work in a worktree.** Another Claude session shares this checkout on `main`.

```bash
cd "$(git rev-parse --show-toplevel)"
git worktree add .worktrees/client-position -b client-position
cd .worktrees/client-position
```

`.worktrees/` is already covered by the global gitignore. Do not add it to the project `.gitignore`.

**House rules for this repo — all of these matter:**

- **Never run `cargo fmt`, `rustfmt`, or any formatter.** A bare `cargo fmt` rewrites 159 files across every crate in this workspace. Format by hand, matching surrounding style.
- **Never `git add -A` or `git add .`** in this repo. Root-level capture files contain a live account password. Always `git add` explicit paths, exactly as spelled out in each task.
- Every file ends with a blank line.
- Commit messages use tags: `feat:`, `fix:`, `test:`, `refactor:`, `docs:`, `chore:`.
- Commit after every task.

**On executing this with a smaller model:** the plan is written to be safe for one. Tasks 1, 2 and 5 are mechanical — every edit is quoted in full. Tasks 3, 4 and 6 are compiler-driven: you make one type change and then fix the sites the compiler names. That is exactly where the danger is, because there are two ways to silence each error and only one is right. `Fix::last_known()` returns `Some` whenever any history exists, so reaching for it makes the error disappear *and* restores the bug this plan removes, with no test failure. **When a call site will not compile, the question is never "which method makes this build" — it is "does this site route, or does it display?"** Routing takes `confirmed()`. Display takes `last_known()`. Review Pass 1 exists to catch this specific mistake; do not skip it.

**Baseline:** confirm the suite is green before you start.

```bash
cargo test -p mud-client 2>&1 | tail -20
```
Expected: all tests pass. If anything fails here, stop and report — it is not your change.

---

## Task 1: A directional look must not move the character

The correlator learns the difference between "the room I am in" and "the room I peeked at", and the session refuses to adopt a peek as the current room.

**Files:**
- Modify: `crates/mud-client/src/correlate.rs`
- Modify: `crates/mud-client/src/session.rs`
- Modify: `crates/mud-client/src/farm.rs` (two sites)
- Modify: test helpers in `crates/mud-client/tests/` (compiler will list them)
- Test: `crates/mud-client/tests/correlate.rs`

### Step 1: Write the failing tests

Append to `crates/mud-client/tests/correlate.rs`. The file already has `room(name)` and `line(l)` helpers and a `TTL` const — reuse them.

```rust
/// A `look <direction>` prints the NEIGHBOUR's room block. It retires the
/// look like any other reply, but it is evidence about the room next
/// door, never about where the character stands — so it is flagged
/// `elsewhere` and position tracking drops it.
#[test]
fn directional_look_block_is_elsewhere() {
    let mut c = Correlator::new(TTL);
    let now = Instant::now();
    c.sent(CmdId(1), "look north", now);
    let _ = c.on_event(line("look north"), now);
    let cor = c.on_event(room("Sewer Tunnel"), now);
    assert_eq!(cor.answers, Some(CmdId(1)), "the block still retires the look");
    assert!(cor.elsewhere, "a directional look describes the room next door");
}

/// A bare `look` describes the room the character is standing in, so it
/// must keep feeding position tracking.
#[test]
fn bare_look_block_is_here() {
    let mut c = Correlator::new(TTL);
    let now = Instant::now();
    c.sent(CmdId(1), "look", now);
    let _ = c.on_event(line("look"), now);
    let cor = c.on_event(room("Sewer Tunnel"), now);
    assert_eq!(cor.answers, Some(CmdId(1)));
    assert!(!cor.elsewhere);
}

/// A move's block is an arrival, the strongest position evidence there
/// is.
#[test]
fn movement_block_is_here() {
    let mut c = Correlator::new(TTL);
    let now = Instant::now();
    c.sent(CmdId(1), "n", now);
    let _ = c.on_event(line("n"), now);
    let cor = c.on_event(room("Sewer Tunnel"), now);
    assert_eq!(cor.answers, Some(CmdId(1)));
    assert!(!cor.elsewhere);
}

/// `l` is the board's look alias, so `l n` is a directional look too.
#[test]
fn short_look_alias_takes_a_direction() {
    let mut c = Correlator::new(TTL);
    let now = Instant::now();
    c.sent(CmdId(1), "l n", now);
    let _ = c.on_event(line("l n"), now);
    let cor = c.on_event(room("Sewer Tunnel"), now);
    assert_eq!(cor.answers, Some(CmdId(1)));
    assert!(cor.elsewhere);
}
```

### Step 2: Run the tests to verify they fail

```bash
cargo test -p mud-client --test correlate 2>&1 | tail -20
```
Expected: FAIL — compile error, `no field 'elsewhere' on type 'Correlated'`.

### Step 3: Add the `LookDir` kind

In `crates/mud-client/src/correlate.rs`, add a variant to `enum Kind`, directly after `Look`:

```rust
    Look,
    LookDir,
```

### Step 4: Classify the directional forms in `kind_of`

Replace this block:

```rust
    if cmd == "look" || cmd.starts_with("look ") {
        return Kind::Look;
    }
```

with:

```rust
    if cmd == "look" || cmd == "l" {
        return Kind::Look;
    }
    // `look <direction>` and its `l <direction>` alias answer with a full
    // room block for the NEIGHBOUR (vendor relnotes: "LOOK <dir> will now
    // give the same detail as moving"). Same reply shape as a move, other
    // vantage — so it needs its own kind rather than sharing `Look`,
    // whose block IS where the character stands.
    if let Some(rest) = cmd.strip_prefix("look ").or_else(|| cmd.strip_prefix("l ")) {
        if DIRS.contains(&rest.trim()) {
            return Kind::LookDir;
        }
    }
    if cmd.starts_with("look ") {
        return Kind::Look;
    }
```

`DIRS` is the const array already declared at the top of `kind_of`; it holds both the short and long spellings.

### Step 5: Let a room block complete a `LookDir`

Two edits in `completes`. In the `Event::RoomSeen(_)` arm:

```rust
            return matches!(kind, Kind::Move | Kind::Look | Kind::LookDir | Kind::Bash);
```

And widen the refusal arm, since `_cmd_look` emits the same wording for both look forms:

```rust
        Kind::Look | Kind::LookDir => {
            has(DARK) || has("door is closed in that direction") || has("there are no exits")
        }
```

### Step 6: Carry the vantage on `Correlated`

Add the field to `pub struct Correlated`:

```rust
    /// True when `event` is a room block describing a room the character
    /// is NOT standing in — the answer to a `look <direction>`.
    ///
    /// Without this a peek is indistinguishable from an arrival, and
    /// `Navigator::localize` searches the current room PLUS its
    /// neighbours by name, so the neighbour matches and the client's
    /// position walks one room per peek. Consumers that model WHERE the
    /// character is — position tracking, the occupancy model — must
    /// ignore such a block; consumers that model what the world looks
    /// like may still read it.
    pub elsewhere: bool,
```

### Step 7: Report the retired command's kind

Change `retire` to hand back what it retired:

```rust
    fn retire(&mut self, ev: &Event, now: Instant) -> Option<(CmdId, Kind)> {
        let pos = self
            .queue
            .iter()
            .position(|e| e.echoed && completes(e.kind, ev))?;
        let id = self.queue[pos].id;
        let kind = self.queue[pos].kind;
        self.queue.drain(..=pos);
        self.refresh(now);
        Some((id, kind))
    }
```

And in `on_event`, replace the `Event::RoomSeen(_)` arm and the final construction:

```rust
    pub fn on_event(&mut self, event: Event, now: Instant) -> Correlated {
        self.expire(now);
        let mut elsewhere = false;
        let answers = match &event {
            Event::SlowDown => {
                self.flush();
                None
            }
            Event::Line(l) => {
                let l = l.clone();
                self.on_line(&l, now)
            }
            Event::RoomSeen(_) => match self.retire(&event, now) {
                Some((id, kind)) => {
                    elsewhere = kind == Kind::LookDir;
                    Some(id)
                }
                None => None,
            },
            Event::Prompt { .. }
            | Event::CombatHit { .. }
            | Event::CombatMiss { .. }
            | Event::ActorEntered { .. }
            | Event::ActorLeft { .. } => None,
        };
        Correlated { event, answers, elsewhere }
    }
```

### Step 8: Fix every construction site the compiler names

```bash
cargo check -p mud-client --tests 2>&1 | grep -E "^error" | head -30
```

Add `elsewhere: false` to each `Correlated { .. }` literal. Known sites:

- `crates/mud-client/src/farm.rs:2639` (the `here.on_event` seed)
- `crates/mud-client/tests/world_corpus.rs` (2), `tests/sheet.rs` (4), `tests/drain.rs` (1), `tests/tui.rs` (2), `tests/world.rs` (2), `tests/farm.rs` (1) — most are inside one or two small helper fns per file, so the real edit count is low.

One site is a **pattern**, not a literal — `crates/mud-client/src/farm.rs:1883`. Add `..` rather than a field:

```rust
            Ok(Ok(Correlated { event: Event::RoomSeen(room), answers, .. }))
```

### Step 9: Refuse a peek as the current room

In `crates/mud-client/src/session.rs`, change `apply_event` to take the correlation, and gate the room:

```rust
fn apply_event(state: &mut GameState, cor: &Correlated) -> bool {
    match &cor.event {
        Event::Prompt { hp, mana } => {
            let changed = state.hp != *hp || state.mana != *mana;
            state.hp = *hp;
            state.mana = *mana;
            changed
        }
        Event::RoomSeen(room) => {
            // The block a `look <direction>` answers describes the room
            // NEXT DOOR. `state.room` means the room the character is
            // standing in — it feeds position tracking and the occupant
            // list — so adopting a peek would both walk the client's
            // position and report the neighbour's occupants as present.
            if cor.elsewhere {
                return false;
            }
            state.room = Some(room.clone());
            true
        }
        _ => false,
    }
}
```

Update the single call site in the same file (in the reader task, alongside `events_tx.send(cor)`):

```rust
                            let cor = guard.on_event(ev, Instant::now());
                            state_tx.send_if_modified(|s| apply_event(s, &cor));
                            let _ = events_tx.send(cor);
```

Add `use crate::correlate::Correlated;` to the imports if it is not already there.

### Step 10: Run the tests

```bash
cargo test -p mud-client 2>&1 | tail -20
```
Expected: PASS, including the four new correlate tests.

### Step 11: Commit

```bash
git add crates/mud-client/src/correlate.rs crates/mud-client/src/session.rs \
        crates/mud-client/src/farm.rs crates/mud-client/tests/
git commit -m "fix(mud-client): a look in a direction is not a step in it"
```

---

## Task 2: Make an unsure position say so

A pure type with no wiring, so it can be tested on its own.

**Files:**
- Modify: `crates/mud-client/src/lost.rs`
- Create: `crates/mud-client/tests/fix.rs`

### Step 1: Write the failing test

Create `crates/mud-client/tests/fix.rs`:

```rust
//! A tracked position is worth exactly what its provenance is worth.
//! `Option<RoomId>` could not say "unsure", so an unresolvable block left
//! a stale id that read as confirmed — and `lost::place`'s name-only
//! shortcut then confirmed it right back. `Fix` makes the difference a
//! type so no consumer can forget to ask.

use mud_client::lost::Fix;
use mud_core::content::RoomId;

const A: RoomId = RoomId { map: 1, room: 2324 };

#[test]
fn only_a_confirmed_fix_may_be_routed_from() {
    assert_eq!(Fix::Confirmed(A).confirmed(), Some(A));
    assert_eq!(Fix::Stale(A).confirmed(), None);
    assert_eq!(Fix::Unknown.confirmed(), None);
}

#[test]
fn a_stale_fix_still_says_where_we_last_were() {
    assert_eq!(Fix::Confirmed(A).last_known(), Some(A));
    assert_eq!(Fix::Stale(A).last_known(), Some(A));
    assert_eq!(Fix::Unknown.last_known(), None);
}

#[test]
fn demotion_keeps_the_room_but_drops_the_claim() {
    assert_eq!(Fix::Confirmed(A).demote(), Fix::Stale(A));
}

#[test]
fn demotion_is_idempotent_and_cannot_invent_history() {
    assert_eq!(Fix::Stale(A).demote(), Fix::Stale(A));
    assert_eq!(Fix::Unknown.demote(), Fix::Unknown);
}

#[test]
fn a_fresh_client_knows_nothing() {
    assert_eq!(Fix::default(), Fix::Unknown);
}
```

### Step 2: Run it to verify it fails

```bash
cargo test -p mud-client --test fix 2>&1 | tail -20
```
Expected: FAIL — `unresolved import mud_client::lost::Fix`.

### Step 3: Add the type

Add to `crates/mud-client/src/lost.rs`:

```rust
/// What the client believes about where the character is standing, and
/// how much that belief is worth.
///
/// `Option<RoomId>` could not tell "a room block resolved to this id"
/// apart from "no block has resolved since, so this is the last id that
/// did". Both were handed to [`place`] as hints of equal standing, and
/// its name-only one-hop shortcut then CONFIRMED the stale one — a wrong
/// origin that routes an entire walk from a room the character was never
/// in. Making the difference a type means every consumer has to say
/// which of the two it can live with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fix {
    /// Nothing has resolved yet: before the first block after login, or
    /// after a demotion chain with no history behind it.
    #[default]
    Unknown,
    /// A room block resolved to this id. Safe as a routing origin and as
    /// a trusted hint.
    Confirmed(RoomId),
    /// The last id that DID resolve, kept only to say where the
    /// character last was. Never a routing origin, never a hint.
    Stale(RoomId),
}

impl Fix {
    /// The id a caller may route from, or hand to [`place`] as a trusted
    /// hint. `None` unless the belief is current.
    pub fn confirmed(self) -> Option<RoomId> {
        match self {
            Fix::Confirmed(at) => Some(at),
            _ => None,
        }
    }

    /// The best id to SHOW, current or not. Display only — never feed
    /// this to a router or a localizer.
    pub fn last_known(self) -> Option<RoomId> {
        match self {
            Fix::Confirmed(at) | Fix::Stale(at) => Some(at),
            Fix::Unknown => None,
        }
    }

    /// Demote on an unresolvable block: the character is somewhere the
    /// graph could not name, so whatever was believed is now only
    /// history. Idempotent — a stale fix does not decay further, and an
    /// unknown one cannot invent a room to be stale about.
    pub fn demote(self) -> Fix {
        match self {
            Fix::Confirmed(at) | Fix::Stale(at) => Fix::Stale(at),
            Fix::Unknown => Fix::Unknown,
        }
    }
}

/// Fold one room block into a positional fix.
///
/// THE place a block becomes a position, so the client and the map
/// cannot disagree about what a block meant. Callers must have already
/// dropped blocks that describe somewhere else (`Correlated::elsewhere`).
pub fn refix(nav: &Navigator, fix: Fix, seen: &RoomView) -> Fix {
    // Only a CONFIRMED fix may seed the one-hop shortcut. Seeding it with
    // a stale id is exactly how a stale id used to confirm itself; the
    // impossible id forces the global search instead.
    let hint = fix.confirmed().unwrap_or(RoomId { map: 0, room: 0 });
    match nav.localize_view(hint, seen) {
        Some(at) => Fix::Confirmed(at),
        None => fix.demote(),
    }
}
```

### Step 4: Run the tests

```bash
cargo test -p mud-client --test fix 2>&1 | tail -20
```
Expected: PASS (5 tests).

### Step 5: Commit

```bash
git add crates/mud-client/src/lost.rs crates/mud-client/tests/fix.rs
git commit -m "feat(mud-client): a position that can admit it is only a memory"
```

---

## Task 3: The client tracks a `Fix`, not an `Option`

**Files:**
- Modify: `crates/mud-client/src/tui.rs`

### Step 1: Change the tracked variable

At `crates/mud-client/src/tui.rs:159`, replace the declaration:

```rust
    // Where the client believes the character is, and how much that
    // belief is worth. A room block is a room block: it says as much when
    // the operator typed the move as when the runner did — but a block
    // the graph cannot name says nothing at all, and that used to be
    // indistinguishable from agreement.
    let mut here = crate::lost::Fix::Unknown;
```

### Step 2: Replace the silent stale fallback

At `crates/mud-client/src/tui.rs:294`, replace:

```rust
                    here = track(nav, here, &room).or(here);
```

with:

```rust
                    here = crate::lost::refix(nav, here, &room);
```

and update the guard just below it, which wants a room id to seed the shadow model:

```rust
                    if let Some(id) = here.confirmed() {
                        model.note_room(id);
                    }
```

### Step 3: Delete the now-dead `track` helper

Remove `fn track(...)` at `crates/mud-client/src/tui.rs:1519` and its doc comment. `lost::refix` replaces it, and leaving both would give two answers to one question.

### Step 4: Adopt a finished job's answer as confirmed

At `crates/mud-client/src/tui.rs:334`:

```rust
                    if let Some(at) = placed {
                        here = crate::lost::Fix::Confirmed(at);
                        model.note_room(at);
                    }
```

### Step 5: Build and let the compiler find the consumers

```bash
cargo check -p mud-client 2>&1 | grep -E "^error" | head -30
```
Expected: FAIL, with errors at every `here` consumer. That list is Task 4's work — do not fix them yet, just confirm the compiler names them (`/go`, `/where`, `/room`, `/map`, `/roam`, `repaint`).

---

## Task 4: Every consumer says which kind of position it needs

This is the payoff: routing takes only a confirmed fix, display takes the last known one.

**Files:**
- Modify: `crates/mud-client/src/tui.rs`

### Step 1: Routing consumers take `confirmed()`

In the `/go` arm (around `crates/mud-client/src/tui.rs:421-437`):

```rust
                                        Some(g) => match crate::go::resolve(g, here.confirmed(), &target) {
```
```rust
                                                let steps = here
                                                    .confirmed()
                                                    .and_then(|f| g.route(f, to))
```
```rust
                                                let started = start_go(
                                                    session.clone(),
                                                    g.clone(),
                                                    here.confirmed(),
```

In the `/where` arm:

```rust
                                        let started = start_where(session.clone(), g.clone(), here.confirmed());
```

`/go` still passes a hint, and `lost::place` still takes its zero-step shortcut when that hint is good — we have only stopped an *unconfirmed* hint from reaching it. This is what keeps `/go` from having to walk to work out where it is.

### Step 2: Display consumers take `last_known()`

In the `/room` and `/map` arms:

```rust
                                    (Some(g), Some(s)) => match here_or(g, here.last_known(), &target) {
```

and in the `MapView::new(...)` call:

```rust
                                                here.last_known(),
```

### Step 3: `/roam` refuses an unconfirmed start

In `start_roam` (around `crates/mud-client/src/tui.rs:1190-1204`), change the parameter type to `crate::lost::Fix` and the refusal:

```rust
    here: crate::lost::Fix,
```
```rust
    let start = here.confirmed().ok_or(
        "nobody knows with any confidence where you are standing; /where first, then roam",
    )?;
```

The fence is measured from this room (`FarmPlan::roaming` validates "standing on its own fence" against it), so a stale start would anchor the fence somewhere nobody is. Update the two call sites the compiler names to pass `here` rather than `here`'s inner option.

### Step 4: The status bar shows the doubt

In `repaint` (around `crates/mud-client/src/tui.rs:1129`), replace the `room_id` line:

```rust
    // The runner's own belief wins while it drives -- it knows which of
    // two same-named rooms it walked to -- and the client's tracking
    // covers everything else.
    let room_id = match phase.as_ref().and_then(|p| p.room()) {
        Some(at) => crate::lost::Fix::Confirmed(at),
        None => here,
    };
```

Change `repaint`'s and `redraw_bottom`'s `here`/`room_id` parameters from `Option<RoomId>` to `crate::lost::Fix`, then in `render_status` (around `crates/mud-client/src/tui.rs:963-968`):

```rust
    if let Some(room) = &state.room {
        s.push_str(&format!(" | {}", room.name));
        if let Some(id) = room_id.last_known() {
            // A trailing `?` is the whole point of the type reaching the
            // bar: the operator can see the client has lost the thread
            // before /go routes from a room nobody is in.
            let sure = if room_id.confirmed().is_some() { "" } else { "?" };
            s.push_str(&format!(" [{}/{}{sure}]", id.map, id.room));
        }
    }
```

### Step 5: Build and test

```bash
cargo check -p mud-client 2>&1 | tail -20
cargo test -p mud-client 2>&1 | tail -20
```
Expected: clean build, all tests pass. Some tests in `crates/mud-client/tests/tui.rs` may need their expected status-bar strings updated — if a test now expects `[1/2324?]`, verify by hand that the `?` is *correct* for that fixture before changing the expectation. A `?` appearing where the fixture really did confirm the room is a bug in Task 3, not a stale test.

### Step 6: Commit

```bash
git add crates/mud-client/src/tui.rs crates/mud-client/tests/tui.rs
git commit -m "fix(mud-client): route from a position only when it is still true"
```

---

## Review Pass 1: the position model, before anything is built on it

Everything after this point assumes `Fix` is honest. Run this review **before** Task 5, because a mistake here is invisible — a wrong position does not crash, it just walks you into a wall twenty minutes later.

**REQUIRED SUB-SKILL:** Use @superpowers:code-reviewer to review the diff from Task 1 through Task 4 against this checklist. Do not accept "looks fine" — each item below is a grep or a specific question with a right answer.

### The one failure mode that matters most

`Fix::last_known()` returns `Some` whenever there is *any* history, confirmed or not. So the fastest way to make a compile error go away is to reach for `last_known()` — and doing that in a routing position silently restores the exact bug this plan removes, with no test failure and no crash. Check every call site by hand:

```bash
grep -rn "last_known()" crates/mud-client/src/
```

Every hit must be **display only**: the status bar, `/room`, `/map`'s anchor, `MapView`'s marker. If `last_known()` appears anywhere near `go::resolve`, `graph.route`, `start_go`, `start_where`, `start_roam`, `lost::place`, or `FarmPlan::roaming`, that is the bug — it must be `confirmed()`.

```bash
grep -rn "confirmed()" crates/mud-client/src/
```
Expected: `/go`'s resolve, route and `start_go`; `/where`; `/roam`'s refusal; `lost::refix`'s hint; the status bar's `?` test.

### The rest of the checklist

- **The silent fallback is gone.** `grep -rn "\.or(here)" crates/mud-client/src/` returns nothing.
- **There is exactly one way to turn a block into a position.** `grep -rn "localize_view" crates/mud-client/src/` should show only `nav.rs` (the definition), `lost.rs` (`refix` and `place`). The old `fn track` in `tui.rs` must be deleted, not merely unused.
- **A peek cannot become the current room.** `apply_event` in `session.rs` returns `false` on `cor.elsewhere` *before* touching `state.room`.
- **`Kind::LookDir` is actually reachable.** `kind_of` returns it for `look n` / `l n`, and `completes` accepts a `RoomSeen` for it. If `completes` were missed, the look would never retire and would linger to claim someone else's block — worse than the original bug.
- **`Fix::Stale` is actually reachable.** It should be produced by `refix`'s `None` branch via `demote()`. A `Stale` that can never occur means the `?` never shows and the demotion is dead code.
- **`refix` seeds the hint from `confirmed()`, never from `last_known()`.** Seeding the one-hop shortcut with a stale id is precisely how a stale id used to confirm itself.
- **No formatter ran.** `git diff --stat main` must list only the files these tasks name. If it shows 100+ files, `cargo fmt` was run against the workspace — reset and redo the task by hand.
- **Nothing from the repo root is staged.** `git status --short` must not show `*.raw` or `*_timing.log`. Those carry a live password.

### Then run the suite

```bash
cargo test -p mud-client 2>&1 | tail -20
```
Expected: all pass. Fix anything this review found before continuing, and commit the fixes separately with a `fix:` tag so the review is visible in history.

---

## Task 5: `/go`'s arrival tells the client where it landed

`Phase::room()` returns `None` for `Done`, so a finished `/go` never writes back the room it just walked to — the destination exists only inside a formatted string.

**Files:**
- Modify: `crates/mud-client/src/farm.rs` (the `Phase` enum and `Phase::room`)
- Modify: `crates/mud-client/src/go.rs`
- Test: `crates/mud-client/tests/go.rs`

### Step 1: Write the failing test

Add to `crates/mud-client/tests/go.rs`, matching that file's existing style for building a `Phase`:

```rust
/// A walk that finished knowing where it stands is the best position
/// evidence there is. Reporting only a sentence threw it away, and the
/// client fell back to whatever the last room block happened to localize
/// to.
#[test]
fn a_finished_walk_reports_the_room_it_reached() {
    let at = RoomId { map: 1, room: 2324 };
    let done = Phase::Done { why: "arrived".into(), at: Some(at) };
    assert_eq!(done.room(), Some(at));
}

#[test]
fn a_walk_that_ended_without_arriving_reports_no_room() {
    let done = Phase::Done { why: "gave up".into(), at: None };
    assert_eq!(done.room(), None);
}
```

### Step 2: Run it to verify it fails

```bash
cargo test -p mud-client --test go 2>&1 | tail -20
```
Expected: FAIL — `struct variant Phase::Done has no field named at`.

### Step 3: Carry the room on `Done`

In `crates/mud-client/src/farm.rs`, change the variant:

```rust
    /// The run ended. `at` is where it ended IF it knows — an arrival
    /// knows, a giving-up usually does not.
    Done { why: String, at: Option<RoomId> },
```

and add the arm to `Phase::room`:

```rust
            Phase::Done { at, .. } => *at,
```

### Step 4: Fill in every construction site

```bash
cargo check -p mud-client --tests 2>&1 | grep -E "^error" | head -30
```

Add `at: None` to each `Done { why: ... }`, **except** `/go`'s arrival in `crates/mud-client/src/go.rs` (the one whose `why` reads `arrived at {map}/{room}`), which becomes:

```rust
            Phase::Done { why: format!("arrived at {}/{}", to.map, to.room), at: Some(to) }
```

Use whatever the destination binding is actually called at that site — read the surrounding lines rather than assuming `to`.

### Step 5: Run the tests

```bash
cargo test -p mud-client 2>&1 | tail -20
```
Expected: PASS.

### Step 6: Commit

```bash
git add crates/mud-client/src/farm.rs crates/mud-client/src/go.rs crates/mud-client/tests/go.rs
git commit -m "feat(mud-client): a walk that arrived says where, not just that"
```

---

## Task 6: The map follows the character

`MapView::here` is set once in the constructor and has no setter, and `mapview::run` repaints only on key press and terminal resize — so the `@` is a snapshot from the moment `/map` was typed. The view also owns the screen while it is up, so the outer loop is not tracking during that time; the view must do it, using the same `lost::refix` so the two cannot disagree.

**Files:**
- Modify: `crates/mud-client/src/mapview.rs`
- Modify: `crates/mud-client/src/tui.rs`

### Step 1: Take a `Fix`, not an `Option`

In `crates/mud-client/src/mapview.rs`, change the field on `pub struct MapView`:

```rust
    /// Where the character stands, and whether that is still true.
    here: crate::lost::Fix,
```

and the constructor parameter on `MapView::new` (the body's `here,` shorthand needs no change):

```rust
        here: crate::lost::Fix,
```

In `MapView::lines()`, the `Marks` construction currently reads `here: self.here,`. Change it to:

```rust
            here: self.here.last_known(),
```

Showing the last known room is right here — a map with no `@` at all is worse than one the operator can see is uncertain, and Task 4 already puts the `?` on the status bar, which stays visible underneath.

### Step 2: Add the accessor and setter

Add to `impl MapView`, next to the other small accessors:

```rust
    /// What the view currently believes, so the client can adopt it when
    /// the map closes.
    pub fn here(&self) -> crate::lost::Fix {
        self.here
    }

    pub fn set_here(&mut self, fix: crate::lost::Fix) {
        self.here = fix;
    }
```

### Step 3: Carry the answer out on `ViewExit`

Add the field to `pub struct ViewExit`:

```rust
    /// Where the view last resolved the character to. `tui::play`'s own
    /// tracking arm does not run while the view owns the screen, so
    /// without this every step taken with the map open is forgotten the
    /// moment it closes.
    pub here: crate::lost::Fix,
```

`run` has **four** `return Ok(ViewExit { .. })` sites — the disconnect, the death, the `keys.recv()` `None`, and the key action. Every one of them gains `here: view.here(),`. The compiler will find any you miss.

### Step 4: Give `run` a navigator and track on every block

Change the signature of `pub async fn run` to take the navigator, after `events`:

```rust
    events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
    nav: Option<&crate::nav::Navigator>,
    on_event: &mut dyn FnMut(&crate::correlate::Correlated),
```

Then replace the whole `ev = events.recv()` arm with:

```rust
            ev = events.recv() => {
                if let Ok(cor) = &ev {
                    on_event(cor);
                    // The view owns the screen, so `tui::play`'s tracking
                    // arm is not running: the map must advance the same
                    // fix the same way, through the same `refix`, or the
                    // two disagree the moment the map closes. `elsewhere`
                    // is honoured here for the same reason it is in the
                    // session — a peek at the next room must not walk the
                    // marker into it.
                    if let (Some(nav), crate::events::Event::RoomSeen(seen)) = (nav, &cor.event)
                        && !cor.elsewhere
                    {
                        let next = crate::lost::refix(nav, view.here(), seen);
                        if next != view.here() {
                            view.set_here(next);
                            paint(&mut out, view)?;
                        }
                    }
                    if let crate::events::Event::Line(line) = &cor.event
                        && crate::farm::is_player_death(line, &username)
                    {
                        return Ok(ViewExit {
                            action: ViewAction::Leave,
                            buffered,
                            interrupted: Some("you died; the map is closed".into()),
                            here: view.here(),
                        });
                    }
                }
            }
```

Note this repaints only when the fix actually changed — the board sends room blocks constantly, and repainting the whole alternate screen on each one would flicker for no gain.

### Step 5: Wire it up in the client

In `crates/mud-client/src/tui.rs`, the `MapView::new(...)` call already passes `here.last_known()` from Task 4 — change it back to plain `here`, since the view now takes a `Fix`:

```rust
                                                here,
```

Pass the navigator into the `crate::mapview::run(...)` call, matching the new parameter position:

```rust
                                                nav.as_ref(),
```

And after the view exits — alongside the existing code that re-establishes the scroll region and flushes `exit.buffered` — adopt what it learned:

```rust
                                            here = exit.here;
```

Check the two offline callers too: `run_offline` in `mapview.rs` and `mmc map` in `crates/mud-client/src/bin/mmc.rs:213` construct a `MapView` with no position. Those become `crate::lost::Fix::Unknown` rather than `None`.

### Step 6: Build and test

```bash
cargo check -p mud-client 2>&1 | tail -20
cargo test -p mud-client 2>&1 | tail -20
```
Expected: clean build, all tests pass.

### Step 7: Commit

```bash
git add crates/mud-client/src/mapview.rs crates/mud-client/src/tui.rs crates/mud-client/src/bin/mmc.rs
git commit -m "feat(mud-client): the map keeps up with the character"
```

---

## Review Pass 2: the whole change, before it goes near the live board

**REQUIRED SUB-SKILL:** Use @superpowers:code-reviewer on the full diff (`git diff main`), then work the checklist below. This pass is about the seams between tasks, which no single task's tests cover.

### Two positions, one truth

The map and the client now each hold a `Fix`, and they must never diverge:

- Both must advance through `lost::refix` and nothing else. `grep -rn "refix\|localize_view" crates/mud-client/src/` — no third implementation, no open-coded `localize_view` in `tui.rs` or `mapview.rs`.
- Both must honour `elsewhere`. Confirm `mapview.rs`'s events arm checks it; a peek taken with the map open must not move the `@` any more than it moves the status bar.
- `ViewExit.here` is populated on **all four** return paths in `run`. Count them: `grep -c "ViewExit {" crates/mud-client/src/mapview.rs` and check each has a `here:`.
- `tui.rs` actually adopts `exit.here`. If it does not, walking with the map open is silently forgotten — the original bug, reintroduced through a new door.

### Repeat the Pass 1 greps

The routing/display split is the invariant most likely to have been broken by later tasks, especially Task 6, where `last_known()` legitimately appears for the first time inside `MapView`:

```bash
grep -rn "last_known()" crates/mud-client/src/
```
Expected hits and nowhere else: the status bar's `?` line, `here_or` for `/room` and `/map`, and `MapView::lines`'s marker. Anything else is a regression.

### Behaviour the tests do not cover

Reason through each and say why it is right, in the review notes:

- **A dark room.** `refix` gets a block the graph cannot name, returns `demote()`, the bar shows `?`. `/go` then localizes for itself instead of trusting it. Correct.
- **Twin rooms.** Standing in one Newhaven Narrow Road and walking to the other: the one-hop hint is seeded from `confirmed()`, so it resolves by neighbour rather than by name collision. Confirm Task 3 did not accidentally pass `last_known()` here.
- **A job running.** `repaint` prefers `phase.room()` over `here` and wraps it as `Fix::Confirmed`. A runner that knows where it is should never render a `?`.
- **Login.** `here` starts `Unknown`, the bar shows no id at all until the first block resolves — not `0/0`.

### Housekeeping

- Every touched file ends with a blank line.
- Every commit is tagged (`fix:`, `feat:`, `test:`).
- `git diff --stat main` lists only the files this plan names.
- `git status --short` shows no `*.raw` or `*_timing.log`.

```bash
cargo test -p mud-client 2>&1 | tail -20
```
Expected: all pass. Only then go to live verification.

---

## Verification

Run the whole suite once more:

```bash
cargo test -p mud-client 2>&1 | tail -20
```

Then verify by hand against the live board, which is the only place the reported symptoms appear. Start the board first — the spawner drains within an hour or two of boot, and these checks want a populated world:

1. `/where` to get a confirmed position. The status bar shows `[m/r]` with no `?`.
2. Type `look north` (a direction with a real exit). **The bar's room id must not change.** Before this work it stepped one room north.
3. Walk manually several rooms, quickly. The bar tracks, or shows `?` when it genuinely cannot resolve — never a confident wrong id.
4. With a `?` showing, run `/go <somewhere>`. It must localize for itself rather than routing from the stale room.
5. `/map`, then let a `/roam` run underneath. The `@` moves.

Capture files are never committed — do not `git add` anything at the repo root.

## Known gaps, deliberately out of scope

- **`look <item>` still classifies as `Kind::Look`**, so its (nonexistent) room block leaves the entry to expire, and an unrelated block could retire it. Same defect family as this plan's Task 1, but it needs its own evidence about what the board prints for an item look; `Kind::Opaque` is the likely answer.
- **The `watch` channel still collapses**: `state_rx` observes only the latest `RoomView`, so several very fast typed moves can be seen as one. Position tracking would be strictly better driven from the `Correlated` broadcast stream, which drops nothing. Left out because it restructures `tui::play`'s select loop and this plan is already load-bearing; `lost::refix` is deliberately shaped to make that migration a small change when it happens.
- **`farm::locate_start`** still takes `plan.start` as its hint, and `FarmConfig::start`'s doc comment still claims the runner "refuses to walk from a position it cannot confirm", which contradicts the module header. Worth reconciling; not a correctness bug, since `locate_start` does a real server `look` first.
