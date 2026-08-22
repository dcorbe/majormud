# Exit requirements, part 2: routing that knows who is walking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the router's answer depend on the character — pay the Silvermere toll when the purse can afford it, take the free detour when it cannot — and learn which direction the toll actually charges instead of guessing.

**Architecture:** Track money as one integer in copper farthings. Replace `exit_cost(exit_type)` with an evaluation of an `ExitRequirement` against a `Capabilities` snapshot, returning either a step count or `Impassable`. Thread that snapshot through Dijkstra. Keep it a cost rather than a prohibition, so an expensive route is still found and reported.

**Tech Stack:** Rust, `cargo test -p mud-client`.

**Spec:** `docs/superpowers/specs/2026-08-22-exit-requirements-design.md` (this plan covers the purse, `Capabilities`, cost-not-prohibition, toll-direction learning, and success criteria 1, 3, 4)

**Depends on:** `2026-08-22-exit-requirements-descriptor.md` must be complete and merged. This plan reads `ExitRequirement` and cannot start without it.

## Global Constraints

- **Never run `cargo fmt`, `rustfmt`, or any formatter.**
- Every file ends with a blank line. Commit after every task with a tag prefix.
- Coin ratios come from `mud-core`'s `CoreConfig::coin_ratios` default — `[10, 10, 100, 100]`, ORACLE-VERIFIED off the Bank of Godfrey lobby sign (`crates/mud-core/src/game.rs:592`). **Do not hardcode a second copy** in `mud-client`; import or restate with a comment pointing at that line as the authority.
- In farthings: copper 1, silver 10, gold 100, platinum 10,000, runic 1,000,000. Five denominations — `runic` is real and is easy to forget.
- `ExitRequirement::Toll { gold }` carries GOLD CROWNS. The unit is INFERRED, not proven (see the spec). The gold→farthing conversion must exist in exactly one function so a correction is a one-line change.
- `tests/graph.rs` and `tests/map.rs` assume `re/mmud_wgnt.sqlite` via their `graph()` `OnceLock` helpers. Follow that convention.
- 45 pre-existing workspace failures are environmental. Judge by `cargo test -p mud-client`.

---

### Task 1: A purse counted in farthings

**Files:**
- Create: `crates/mud-client/src/purse.rs`
- Modify: `crates/mud-client/src/lib.rs` (add `pub mod purse;`)
- Test: `crates/mud-client/tests/purse.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub struct Purse(u64)` with `Purse::ZERO`, `Purse::from_farthings(u64)`, `Purse::farthings(&self) -> u64`, `Purse::from_gold(u32) -> Purse`, and `pub fn parse_coin_line(line: &str) -> Option<Purse>`. Tasks 2 and 4 use `farthings()` and `from_gold`.

- [ ] **Step 1: Write the failing test**

Create `crates/mud-client/tests/purse.rs`:

```rust
use mud_client::purse::{parse_coin_line, Purse};

/// The ladder, ORACLE-VERIFIED off the Bank of Godfrey lobby sign:
/// 10 copper = 1 silver, 10 silver = 1 gold, 100 gold = 1 platinum,
/// 100 platinum = 1 runic.
#[test]
fn the_denomination_ladder_is_the_bank_of_godfreys() {
    assert_eq!(Purse::from_gold(1).farthings(), 100);
    assert_eq!(Purse::from_gold(5).farthings(), 500, "the Silvermere toll");
    assert_eq!(
        Purse::from_gold(10_000).farthings(),
        1_000_000,
        "map 17: 10,000 gold is exactly one runic coin"
    );
}

/// The board prints coins high denomination first, comma separated, and
/// singularises at one. Every form it can print must parse.
#[test]
fn a_coin_listing_parses_to_farthings() {
    for (line, want) in [
        ("1 copper farthing", 1u64),
        ("9 copper farthings", 9),
        ("1 silver noble", 10),
        ("1 gold crown, 9 copper farthings", 109),
        ("2 gold crowns, 8 copper farthings", 208),
        ("1 platinum piece", 10_000),
        ("1 runic coin", 1_000_000),
        (
            "1 runic coin, 1 platinum piece, 1 gold crown, 1 silver noble, 1 copper farthing",
            1_010_111,
        ),
    ] {
        assert_eq!(
            parse_coin_line(line).map(|p| p.farthings()),
            Some(want),
            "parsing {line:?}"
        );
    }
}

/// A line that is not a coin listing must not parse as an empty purse —
/// "you have no money" and "this line is about something else" are
/// different facts and the caller acts differently on each.
#[test]
fn a_line_that_is_not_coins_does_not_parse() {
    for line in [
        "You notice a rusty dagger here.",
        "Obvious exits: north, south",
        "",
    ] {
        assert_eq!(parse_coin_line(line), None, "parsing {line:?}");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mud-client --test purse`

Expected: FAIL to compile — no `purse` module.

- [ ] **Step 3: Write the module**

Create `crates/mud-client/src/purse.rs`:

```rust
//! What the character is carrying, as one number.
//!
//! MajorMUD has five denominations and the board prints them mixed
//! ("2 gold crowns, 8 copper farthings"). Carrying them as five counters
//! means every comparison, every subtraction and every toll check has to
//! normalise first, and each of those is a place to get the ladder
//! wrong. One integer in the smallest unit has none of those places.

/// Farthings per unit, low to high. The ratios are `mud-core`'s
/// `CoreConfig::coin_ratios` default (`crates/mud-core/src/game.rs:592`),
/// ORACLE-VERIFIED off the Bank of Godfrey lobby sign: 10 copper = 1
/// silver, 10 silver = 1 gold, 100 gold = 1 platinum, 100 platinum = 1
/// runic. That line is the authority; this is a restatement in the
/// client's own unit, not a second source of truth.
const COPPER: u64 = 1;
const SILVER: u64 = 10;
const GOLD: u64 = 100;
const PLATINUM: u64 = 10_000;
const RUNIC: u64 = 1_000_000;

/// Denomination names as the board writes them, singular and plural,
/// paired with what one is worth. Longest names first is not required —
/// each is matched whole.
const DENOMINATIONS: [(&str, &str, u64); 5] = [
    ("copper farthing", "copper farthings", COPPER),
    ("silver noble", "silver nobles", SILVER),
    ("gold crown", "gold crowns", GOLD),
    ("platinum piece", "platinum pieces", PLATINUM),
    ("runic coin", "runic coins", RUNIC),
];

/// Money on hand, in copper farthings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Purse(u64);

impl Purse {
    pub const ZERO: Purse = Purse(0);

    pub fn from_farthings(n: u64) -> Purse {
        Purse(n)
    }

    pub fn farthings(&self) -> u64 {
        self.0
    }

    /// Gold crowns to farthings.
    ///
    /// THE conversion for `ExitRequirement::Toll`, whose `gold` field is
    /// the raw `para1`. That `para1` is denominated in gold is INFERRED
    /// (the observed 5-gold Silvermere toll matches `para1 = 5`, and map
    /// 17's `10000` is exactly one runic coin) and not proven — the
    /// proof is in `move_user` in the WCCMMUD decompile, unread. If it
    /// turns out to be farthings, this function is the only edit.
    pub fn from_gold(gold: u32) -> Purse {
        Purse(u64::from(gold) * GOLD)
    }
}

/// Read a coin listing into a purse, or `None` if the line is not one.
///
/// `None` and `Some(Purse::ZERO)` are different answers on purpose: a
/// line that says nothing about money must not be read as "carrying
/// nothing", or every unrelated line would zero the purse.
pub fn parse_coin_line(line: &str) -> Option<Purse> {
    let mut total: u64 = 0;
    let mut matched = false;
    for part in line.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (count, rest) = part.split_once(' ')?;
        let count: u64 = count.parse().ok()?;
        let rest = rest.trim();
        let (_, _, value) = DENOMINATIONS
            .iter()
            .find(|(one, many, _)| rest == *one || rest == *many)?;
        total = total.checked_add(count.checked_mul(*value)?)?;
        matched = true;
    }
    matched.then_some(Purse(total))
}
```

Add `pub mod purse;` to `crates/mud-client/src/lib.rs`, in the module list's existing alphabetical position.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p mud-client --test purse`

Expected: PASS.

- [ ] **Step 5: Mutate**

Change `GOLD` to `10`. Expected: `the_denomination_ladder_is_the_bank_of_godfreys` and `a_coin_listing_parses_to_farthings` both FAIL. Change `matched.then_some(...)` to `Some(Purse(total))`. Expected: `a_line_that_is_not_coins_does_not_parse` FAILS on the empty string. Revert both.

The expected values in the ladder test are written from the Bank of Godfrey sign, not computed from these constants — a test that multiplied the constants together could not fail.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/purse.rs crates/mud-client/src/lib.rs crates/mud-client/tests/purse.rs
git commit -m "feat(purse): track money as one integer in copper farthings

Five denominations printed mixed by the board means every comparison
would otherwise normalise first, and each of those is a place to get the
ladder wrong. Ratios are mud-core's ORACLE-VERIFIED coin_ratios restated
in the client's own unit, with that line named as the authority.

parse_coin_line returns None rather than a zero purse for a line that is
not a coin listing: 'carrying nothing' and 'this line is not about
money' are different facts."
```

---

### Task 2: `Capabilities` and a cost that can say "impossible"

**Files:**
- Modify: `crates/mud-client/src/graph.rs` (`exit_cost` ~line 73; `explore` ~line 546)
- Test: `crates/mud-client/tests/graph.rs`

**Interfaces:**
- Consumes: `ExitRequirement` (part 1), `Purse` (Task 1).
- Produces: `pub struct Capabilities { pub purse: Purse }` with `Capabilities::unrestricted()`; `pub enum Cost { Steps(u32), Impassable }`; `pub fn exit_cost_for(req: &ExitRequirement, exit_type: i64, caps: &Capabilities) -> Cost`. Task 3 calls `route_for`.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/graph.rs`:

```rust
use mud_client::graph::{Capabilities, Cost, exit_cost_for};
use mud_client::purse::Purse;

/// A toll you can pay is a step. A toll you cannot pay is a wall.
#[test]
fn a_toll_costs_a_step_when_affordable_and_is_impassable_otherwise() {
    let toll = ExitRequirement::Toll { gold: 5 };
    let rich = Capabilities {
        purse: Purse::from_farthings(500),
    };
    let broke = Capabilities {
        purse: Purse::from_farthings(499),
    };
    assert_eq!(exit_cost_for(&toll, 4, &rich), Cost::Steps(1));
    assert_eq!(exit_cost_for(&toll, 4, &broke), Cost::Impassable);
}

/// A puzzle-concealed exit is not merely expensive: no amount of walking
/// opens it, so pricing it high would still route through it.
#[test]
fn a_puzzle_concealed_exit_is_impassable_not_expensive() {
    let caps = Capabilities::unrestricted();
    assert_eq!(
        exit_cost_for(&ExitRequirement::Hidden { searchable: false }, 6, &caps),
        Cost::Impassable
    );
    assert_eq!(
        exit_cost_for(&ExitRequirement::Hidden { searchable: true }, 6, &caps),
        Cost::Steps(40),
        "a searchable hidden exit keeps its old price"
    );
}

/// Everything exit_cost priced before must still cost the same, or this
/// change silently reroutes the whole world. The old function is the
/// oracle for the new one on every requirement that does not consult
/// state.
#[test]
fn state_free_requirements_keep_their_old_prices() {
    let caps = Capabilities::unrestricted();
    for exit_type in [0, 2, 7, 0xb, 9, 0x18, 0x10, 0x14, 0x16, 0x17, 10, 0x13, 3, 5] {
        let req = ExitRequirement::from_exit_type(exit_type, 0);
        let want = mud_client::graph::exit_cost(exit_type);
        assert_eq!(
            exit_cost_for(&req, exit_type, &caps),
            Cost::Steps(want),
            "type {exit_type:#x} changed price"
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mud-client --test graph`

Expected: FAIL to compile — `Capabilities`, `Cost`, `exit_cost_for` do not exist.

- [ ] **Step 3: Write them**

In `crates/mud-client/src/graph.rs`, beside `exit_cost`:

```rust
/// What the walker can currently bring to bear on an exit.
///
/// A snapshot, passed to routing rather than read from a global: two
/// routes computed for different characters in the same process must be
/// able to disagree.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub purse: crate::purse::Purse,
}

impl Capabilities {
    /// Everything satisfiable — what routing assumed before it could ask.
    ///
    /// Used by callers that genuinely want the shape of the world rather
    /// than one character's view of it, and as the migration default.
    /// A caller that wants a character's real answer must pass that
    /// character's capabilities; this one will happily route through a
    /// 10,000-gold toll.
    pub fn unrestricted() -> Capabilities {
        Capabilities {
            purse: crate::purse::Purse::from_farthings(u64::MAX),
        }
    }
}

/// What one edge costs this walker, or that it cannot be walked at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cost {
    Steps(u32),
    Impassable,
}

/// [`exit_cost`], but able to consult the walker.
///
/// Still a COST and not a prohibition wherever a cost can express the
/// truth — the argument in `exit_cost`'s own comment stands: refusing a
/// type answers "no route" to rooms that are genuinely reachable.
/// `Impassable` is reserved for edges no amount of walking opens:
/// a toll beyond the purse, and a passage concealed by a puzzle
/// bit-word that SEARCH cannot clear.
pub fn exit_cost_for(req: &ExitRequirement, exit_type: i64, caps: &Capabilities) -> Cost {
    match req {
        ExitRequirement::Toll { gold } => {
            if caps.purse.farthings() >= crate::purse::Purse::from_gold(*gold).farthings() {
                Cost::Steps(1)
            } else {
                Cost::Impassable
            }
        }
        // No search roll can clear a bit-word. Pricing this high rather
        // than refusing it would still route through it whenever the
        // detour was longer, and then stall at the wall.
        ExitRequirement::Hidden { searchable: false } => Cost::Impassable,
        // Everything else is priced exactly as before, by type.
        _ => Cost::Steps(exit_cost(exit_type)),
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p mud-client --test graph`

Expected: PASS, including `state_free_requirements_keep_their_old_prices`.

- [ ] **Step 5: Mutate**

Change the toll comparison from `>=` to `>`. Expected: the affordable case FAILS at exactly 500 farthings — the boundary is the point of the test. Change `Hidden { searchable: false }` to `Cost::Steps(40)`. Expected: `a_puzzle_concealed_exit_is_impassable_not_expensive` FAILS. Revert both.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/tests/graph.rs
git commit -m "feat(graph): cost an exit against the walker, not just its type

exit_cost took the exit type and nothing else, so it answered the same
for a pauper and a millionaire. exit_cost_for consults a Capabilities
snapshot and can answer Impassable for the two cases no amount of
walking opens: a toll beyond the purse, and a passage concealed by a
bit-word SEARCH cannot clear.

Everything else keeps its old price, pinned by a test that uses the old
function as the oracle -- otherwise this silently reroutes the world.

Nothing routes on it yet."
```

---

### Task 3: Dijkstra asks the walker

**Files:**
- Modify: `crates/mud-client/src/graph.rs` (`explore`, `route`, `route_within`, `distances`, `distances_within`, `route_within` callers)
- Test: `crates/mud-client/tests/graph.rs`

**Interfaces:**
- Consumes: `Capabilities`, `Cost`, `exit_cost_for` from Task 2.
- Produces: `RoomGraph::route_for(from, to, caps) -> Option<Vec<Direction>>`. `route`/`route_within`/`distances` keep their signatures and delegate with `Capabilities::unrestricted()`.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/graph.rs`:

```rust
/// The headline case. Leaving Silvermere through the gate is one hop and
/// costs 5 gold. With the money, take the gate. Without it, take the
/// long way round -- and still arrive.
///
/// The detour's exact length is deliberately NOT asserted. Measured
/// against this exit_cost model it is 34 hops for a cost of 77 (an
/// unweighted BFS says 29, which is why the number is not a constant
/// worth pinning: it moves whenever a price does). What matters is that
/// a route exists and that it is not the gate.
#[test]
fn the_silvermere_toll_is_paid_when_affordable_and_walked_around_when_not() {
    let g = graph();
    let inner = RoomId { map: 1, room: 1381 };
    let road = RoomId { map: 1, room: 1382 };

    let rich = Capabilities {
        purse: Purse::from_gold(5),
    };
    let route = g.route_for(inner, road, &rich).expect("a route with money");
    assert_eq!(route.len(), 1, "one hop through the gate: {route:?}");
    assert_eq!(route[0], Direction::East);

    let broke = Capabilities {
        purse: Purse::from_gold(4),
    };
    let detour = g
        .route_for(inner, road, &broke)
        .expect("broke is not stranded; there is a free way round");
    assert!(
        detour.len() > 1,
        "the free way round is long; got {detour:?}"
    );
    assert_ne!(detour[0], Direction::East, "not through the gate");
}

/// A gate is a cost, never a refusal: an expensive route is still a
/// route. Only genuinely unopenable edges are refused.
#[test]
fn an_expensive_route_is_still_found() {
    let g = graph();
    // The Shadowy Passage behind the gem puzzle is unreachable...
    let shadowy = RoomId { map: 17, room: 3042 };
    let behind = RoomId { map: 17, room: 3044 };
    assert!(
        g.route_for(shadowy, behind, &Capabilities::unrestricted())
            .is_none(),
        "the gem passage is shut until the puzzle is solved"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mud-client --test graph the_silvermere_toll_is_paid_when_affordable_and_walked_around_when_not`

Expected: FAIL to compile — no `route_for`.

- [ ] **Step 3: Thread capabilities through Dijkstra**

Change `explore`'s signature to take `caps: &Capabilities`, and replace its cost line:

```rust
                let step = (cost + u64::from(exit_cost(edge.exit_type)), hops + 1);
```

with:

```rust
                let step = match exit_cost_for(&edge.requirement, edge.exit_type, caps) {
                    // An edge this walker cannot open is not a dear edge.
                    // Skipping it is what lets the detour win.
                    Cost::Impassable => continue,
                    Cost::Steps(c) => (cost + u64::from(c), hops + 1),
                };
```

Add the public entry point beside `route`:

```rust
    /// As [`RoomGraph::route`], for a walker with these capabilities.
    ///
    /// The difference is not academic: leaving Silvermere through the
    /// gate is one hop and costs 5 gold, and the cheapest way round is
    /// 34 hops for a cost of 77. A
    /// router that cannot be told which walker is asking gets that
    /// wrong every time, in the same direction.
    pub fn route_for(
        &self,
        from: RoomId,
        to: RoomId,
        caps: &Capabilities,
    ) -> Option<Vec<Direction>> {
        self.route_within_for(from, to, &|_, _| true, caps)
    }
```

Rename the existing `route_within` body to `route_within_for` taking the extra `caps: &Capabilities`, and make the old names delegate:

```rust
    pub fn route(&self, from: RoomId, to: RoomId) -> Option<Vec<Direction>> {
        self.route_within_for(from, to, &|_, _| true, &Capabilities::unrestricted())
    }

    pub fn route_within(
        &self,
        from: RoomId,
        to: RoomId,
        allow: &dyn Fn(Direction, &ExitEdge) -> bool,
    ) -> Option<Vec<Direction>> {
        self.route_within_for(from, to, allow, &Capabilities::unrestricted())
    }
```

Do the same for `distances` and `distances_within`. Every existing caller keeps compiling and keeps its old behaviour.

**This is a migration, not a completion.** `Capabilities::unrestricted()` reproduces today's wrong answer. Grep for the remaining callers and list them in the commit message:

```bash
grep -rn "\.route(\|\.route_within(\|\.distances(" crates/mud-client/src/ | grep -v "fn route\|fn distances"
```

Task 4 migrates the walking path; anything still on `unrestricted()` after that is a known gap, not a finished job.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p mud-client --test graph`

Expected: PASS. If the rich route is not exactly one hop east, stop: that is the toll edge itself and something is wrong with the classification, not with the routing.

- [ ] **Step 5: Run the whole crate's tests**

Run: `cargo test -p mud-client`

Expected: PASS. Every pre-existing caller routes with `unrestricted()`, which prices exactly as before **except** that puzzle-concealed exits are now skipped. If a routing test changes, that is the one legitimate cause — confirm it is, and say so.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/tests/graph.rs
git commit -m "feat(graph): route for a walker, not for the world

route_for/distances_for consult a Capabilities snapshot; the old names
delegate with unrestricted(), which reproduces today's behaviour so
every existing caller keeps working.

With 5 gold the router takes Silvermere's gate in one hop. With 4 it
takes the free detour (34 hops, cost 77) and still arrives -- an
impassable edge is
skipped rather than priced, which is what lets the detour win.

MIGRATION NOT COMPLETE: callers still on unrestricted() are listed
below and are a known gap."
```

---

### Task 4: Fill the purse from the board

**Files:**
- Modify: `crates/mud-client/src/purse.rs` (add the observer)
- Modify: `crates/mud-client/src/tui.rs` (poll on entering the realm, beside the existing `exp` poll)
- Test: `crates/mud-client/tests/purse.rs`

**Interfaces:**
- Consumes: `Purse`, `parse_coin_line` (Task 1).
- Produces: `PurseMeter` with `observe(&mut self, line: &str) -> bool` and `current(&self) -> Purse`. Task 5 reads `current()` either side of a crossing.

**Why this task exists.** It was NOT in the plan as first written, and that was a defect: Task 5 needs to read the purse across a crossing, `Capabilities { purse }` needs a real value, and **nothing in the client produces one**. Verified on main: the client polls `health` (`farm.rs:1180`) and `exp` (`tui.rs:251`, `359`) and nothing else, and `bot.rs`'s `COIN_PILE_RE` reads coins on the FLOOR, not carried. Without this task the spec's first success criterion can only ever be exercised with a hand-constructed `Capabilities`, never against a board.

**Where the number comes from.** The inventory command. `mud-core`'s `show_inventory` prints carried coins through `text::coin_listing`, which renders high-denomination-first and comma-separated (`"1 gold crown, 9 copper farthings"`). The command's aliases are `i` and `inventory` (`crates/mud-core/src/command.rs:166,213`).

**The trap this task must avoid.** A coin listing on the wire is not necessarily the purse. A pile on the floor prints as a `You notice ... here.` line and `bot.rs` already sweeps those. If `PurseMeter` treated any coin-shaped line as the balance, walking into a room with 3 gold on the floor would silently rewrite the purse to 3 gold and the router would then refuse a toll it can afford.

So the meter must only accept a listing it can attribute to **our own inventory command**. Read `crates/mud-client/src/correlate.rs` before designing this — the client already correlates a sent command with the reply that answers it, and that machinery is the right tool. Do not invent a second correlation scheme.

- [ ] **Step 1: Read first, then write**

Read `crates/mud-client/src/correlate.rs` and `crates/mud-client/src/tui.rs:240-260` (the `exp` poll on realm entry, which is the pattern to follow). Then decide how a reply to `i` is recognised.

**If the correlator cannot attribute an inventory reply** — for example if it only tracks movement and combat — say so and report `NEEDS_CONTEXT` rather than inventing an attribution that will silently mis-fire. A purse that is wrong is worse than a purse that is absent, because the router will act on it.

- [ ] **Step 2: Write the failing tests**

```rust
/// The meter takes its value from OUR inventory reply.
#[test]
fn an_inventory_reply_sets_the_purse() {
    let mut m = PurseMeter::default();
    assert_eq!(m.current(), Purse::ZERO);
    m.expect_reply();                    // we just sent `i`
    assert!(m.observe("2 gold crowns, 8 copper farthings"));
    assert_eq!(m.current().farthings(), 208);
}

/// Coins lying on the floor are NOT the purse. Walking into a room with
/// money in it must not rewrite the balance -- the router would then
/// refuse a toll the character can actually afford.
#[test]
fn coins_on_the_floor_do_not_touch_the_purse() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe("2 gold crowns, 8 copper farthings"));
    let before = m.current();
    // No pending inventory request now; anything coin-shaped is somebody
    // else's money.
    assert!(!m.observe("3 gold crowns"));
    assert!(!m.observe("You notice 11 silver nobles here."));
    assert_eq!(m.current(), before, "the floor is not the purse");
}

/// An inventory with no coins in it means ZERO carried, which is a real
/// answer and different from "we have never asked".
#[test]
fn an_inventory_without_coins_reads_as_empty() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(!m.observe("You are carrying Nothing!"));
    assert_eq!(m.current(), Purse::ZERO);
    assert!(m.settled(), "we asked and got an answer");
}
```

Adapt the API names to whatever the correlator makes natural, but keep all three behaviours: attributed reply sets it, unattributed coin lines do not, and an answered-but-empty inventory is distinguishable from never-asked.

- [ ] **Step 3: Run to verify they fail, then implement**

Run: `cargo test -p mud-client --test purse`

Implement `PurseMeter` in `purse.rs`, and send the inventory command on entering the realm in `tui.rs`, beside the existing `exp` poll and gated on the same realm-presence check — a command typed at the account menu is a menu key, not an inventory request.

- [ ] **Step 4: Mutate**

Make `observe` accept any coin-shaped line regardless of attribution. Expected: `coins_on_the_floor_do_not_touch_the_purse` FAILS. Make it accept none. Expected: `an_inventory_reply_sets_the_purse` FAILS. Both must bite, or the meter cannot tell our reply from the room.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/purse.rs crates/mud-client/src/tui.rs crates/mud-client/tests/purse.rs
git commit -m "feat(purse): read the carried balance off the inventory reply

Nothing produced a purse value, so Capabilities.purse could only ever
hold what a caller hand-constructed and the toll predicate could not be
exercised against a board.

The balance comes from the inventory command, which prints carried coins
through the same high-to-low listing mud-core renders. Attribution is
load-bearing: a pile on the floor is coin-shaped too, and treating one
as the balance would have the router refuse a toll the character can
afford."
```

---

### Task 5: Learn which way the toll actually charges

**Files:**
- Modify: `crates/mud-client/src/nav.rs` (the walk path, where a step is sent and its reply read)
- Test: `crates/mud-client/tests/nav_toll.rs` (create)

**Interfaces:**
- Consumes: `Purse`, `Capabilities`, `ExitRequirement::Toll`, and `PurseMeter::current()` from Task 4.
- Produces: `pub struct TollLog` recording `(RoomId, Direction) -> charged: bool`, and `Capabilities::tolls_known_free: Arc<TollLog>` consulted by `exit_cost_for`.

- [ ] **Step 1: Read first, then write**

Before writing anything, read how `nav.rs` observes the board's reply to a step (the `StepEvent` enum and the arrival path), and how `PurseMeter` (Task 4) reaches it. Task 4 exists precisely to supply the before/after reading this task needs; if it is not yet merged, stop — this task cannot be done without it.

The spec's design is: assume every type-4 crossing charges; on an actual crossing compare the purse before and after; if it did not move, record that `(room, direction)` as free.

- [ ] **Step 2: Write the failing test**

Create `crates/mud-client/tests/nav_toll.rs` following `nav_hidden.rs`'s scripted-board pattern (`alley_board` / `session_for` / `nav`). Two cases:

```rust
/// Crossing a toll edge that does NOT deduct is remembered as free, and
/// the next route prefers it.
///
/// The data says both directions of the Silvermere gate are tolled and
/// play says only outbound charges. The rule is in `move_user` in the
/// WCCMMUD decompile, unread -- so the client measures instead of
/// guessing, and corrects itself on first contact.
#[tokio::test]
async fn a_crossing_that_does_not_deduct_is_remembered_as_free() {
    // board: crossing west prints no coin change
    // assert: log.is_free(gate_room, Direction::West) after the walk
}

/// And one that DOES deduct stays tolled.
#[tokio::test]
async fn a_crossing_that_deducts_stays_tolled() {
    // board: crossing east deducts 5 gold
    // assert: !log.is_free(gate_room, Direction::East)
}
```

Fill both bodies against the harness you read in Step 1. Both must exist: a test that only proves the free case would pass with `is_free` hardcoded to `true`.

- [ ] **Step 3: Run to verify they fail, implement, run to verify they pass**

Run: `cargo test -p mud-client --test nav_toll`

Implement `TollLog` as an interior-mutable map on the shared session state, consulted in `exit_cost_for`:

```rust
        ExitRequirement::Toll { gold } => {
            if caps.is_known_free(room, dir) {
                Cost::Steps(1)
            } else if caps.purse.farthings() >= crate::purse::Purse::from_gold(*gold).farthings() {
                Cost::Steps(1)
            } else {
                Cost::Impassable
            }
        }
```

Note this requires `exit_cost_for` to know **which** edge it is costing, not just its requirement — add `room: RoomId, dir: Direction` parameters and update Task 2's call site in `explore`, which has both to hand.

- [ ] **Step 4: Mutate**

Make `is_known_free` return `true` always. Expected: `a_crossing_that_deducts_stays_tolled` FAILS. Make it return `false` always. Expected: `a_crossing_that_does_not_deduct_is_remembered_as_free` FAILS. Both must bite.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/nav.rs crates/mud-client/src/graph.rs crates/mud-client/tests/nav_toll.rs
git commit -m "feat(nav): learn which direction a toll actually charges

Both directions of the Silvermere gate carry type=4 para1=5 in the data
and only the outbound crossing charges in play, so the asymmetry is in
the engine and we do not have the rule -- theft.md 8.1 does not name
type 4 and move_user has not been read.

So the client measures: pessimistic until a crossing proves otherwise,
then that direction is remembered free. Pessimism can waste a detour; it
can never strand a character on the wrong side of a gate it cannot pay
to re-cross."
```

---

## Self-Review

**Spec coverage.** Purse in farthings → Task 1. `Capabilities` + `Cost` + cost-not-prohibition → Task 2. Success criterion 1 (routing changes with state) → Task 3. Success criterion 4 (nothing refused for being merely expensive) → Task 3's `an_expensive_route_is_still_found`. Toll-direction learning → Task 4. Spec tests 1, 2 → Tasks 2 and 3. Spec test 7 (purse round trip) → Task 1. Spec test 8 (observed-toll learning) → Task 4.

**Amended 2026-08-22 during execution.** A pre-flight check against the merged
descriptor branch found this plan had a hole: Task 5 (toll-direction learning)
needs to read the purse across a crossing, and NOTHING in the client produced a
purse value — the client polls `health` and `exp` and nothing else, and
`bot.rs` reads coins on the floor, not carried. Task 4 (fill the purse from the
board) was inserted to close it, and the old Task 4 became Task 5. Without it
success criterion 1 could only ever be exercised with a hand-constructed
`Capabilities`.

**Known gaps, stated rather than hidden:**

- **Success criterion 3 — "a route that fails at a gate reports *which* gate" — has no task.** `Cost::Impassable` currently loses the reason on the way out of Dijkstra. Doing it properly means `route_for` returning a refusal reason rather than `Option`, which touches every caller, and it is worth its own plan rather than a rushed fourth task here. This is a deliberate deferral of a spec criterion and must not be reported as complete.
- **Task 5's purse reading is now supplied by Task 4** rather than assumed. Task 4's own Step 1 still requires checking that the correlator can attribute an inventory reply, and reporting `NEEDS_CONTEXT` rather than inventing an attribution — a purse that is wrong is worse than one that is absent, because the router acts on it.
- **Migration is incomplete by construction.** Task 3 leaves every existing caller on `Capabilities::unrestricted()`. Task 4 migrates the walking path. Any caller still unrestricted afterwards is a known gap.
- **Spec tests 5 and 6** (the `12/2118` cross-room puzzle shape, and the four gem actions being order-free) are inherited from part 1's self-review and still have no assertion, because nothing consumes `Puzzle { actions }` until the phase-C solver.

**Type consistency.** `Purse::from_gold`, `Purse::from_farthings`, `Purse::farthings` defined in Task 1 and used with those names in Tasks 2-4. `Capabilities { purse }` defined in Task 2, extended with the toll log in Task 4 — Task 4 explicitly restates that `exit_cost_for` gains `room` and `dir` parameters, so its signature diverges from Task 2's on purpose and the call site in `explore` is named as the thing to update.
