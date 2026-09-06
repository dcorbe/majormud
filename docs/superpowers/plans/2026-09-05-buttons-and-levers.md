# Buttons and levers (phase 2 of puzzles, keys and the pack) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The graph knows which exits a button or lever opens and what it takes, routing prices those exits by the walk to the lever and refuses the ones this character cannot open, the walker performs the plan and comes back, and a roam grows through the puzzles the character can solve.

**Architecture:** A room's type 12 exit slot is a remote action, decoded at load into a `PuzzleAction` and attached to the exit it targets as `ExitRequirement::Puzzle`. The slot itself is never an edge. `puzzle.rs` holds the shape and the one pure question both routing and walking ask, `Puzzle::plan`, which turns the exit's concealment word and the walker's pack into an ordered list of actions or `None`. The navigator handles a puzzle exit the way it handles a hidden one: send the direction, and on the board's refusal perform the plan with inner `goto` calls, come back, and send the direction again. Roams flood and rotate with the character's real capabilities so the region is what the character can reach.

**Tech Stack:** Rust workspace, tokio, `mud_core::content` for rooms and messages, scripted TCP boards in tests.

**Spec:** `docs/superpowers/specs/2026-09-04-puzzles-keys-and-pack-design.md`, Part 2.

## Global Constraints

- No test reads `re/`. Graph fixtures are built with `RoomGraph::from_rooms` or `Content::default()` plus `add_room`, `add_message`, `add_text_block`, `add_item`.
- Never run rustfmt or cargo fmt.
- Commit after every task with a tag: `feat:`, `fix:`, `doc:`, `refactor:`. No references to plans or specs in commit messages. No attribution trailers of any kind in commit messages.
- Every file ends with exactly one trailing newline.
- Punctuation in new prose and comments: periods, commas, question marks, a colon before a list. No em dashes, parentheses, or semicolons in new prose. Existing prose is left as it is.
- Run the full crate suite before every commit: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked" `. Every `test result:` line must say `0 failed`. Do not trust a `tail` of the output, the suite has over 50 binaries.
- The bit rules are mud-core's `remote_lever` in `crates/mud-core/src/game.rs`: action n clears bit n+3 of the word, action 0 clears every bit in `0x3ff0`, action 10 clears `0x2000`. Levers pull in descending number order, which is valid whether or not the exit's `para2` is negative, so the plan never records whether the puzzle is ordered.
- A lever on a gate toggles its lock, mud-core's `remote_gate_toggle`, so one action of any number unlocks a gate. Only exit types 6, 7 and 0xb answer a remote action. A slot aimed at any other exit type changes nothing about it.
- Live captures pin these strings: the lever reply `You pull the lever. Off in the distance you hear a small click.`, the revealed exit token `dark passageway north`, and the board saying an unknown phrase aloud as `You say "pull lever"`.
- The retry budget for a puzzle step is 2 whole plans, because the board re-hides a revealed passage after 300 seconds and a long detour can lose that race once.

## Rulings made while planning

- `Puzzle` has no `ordered` field and `PuzzleAction` carries its own `hops`. The spec stores one `detour` per puzzle and an `ordered` flag. Descending order is valid for both kinds of puzzle, so the flag would never be read. Per action hops price exactly the actions the plan uses, which the spec noted its puzzle wide sum could not.
- The walk is reactive, not block driven. The spec's step 1 checks the held room block for the direction before deciding the passage is open. Sending the direction first asks the same question of the board itself, which is how hidden exits already work, and it needs no block.
- Removing type 12 slots from the graph changes the MegaMud room id of the 228 rooms that carry one. `mega.rs` says a room id is a checksum used as a verification signal only and 38 rooms already share one id, so this is accepted.
- `FarmPlan::roaming`'s "the fence leaves nowhere to roam" branch is dead, a region always contains its start room, and Task 6 removes it rather than thread capabilities into a check that cannot fire.

## File structure

| file | responsibility |
|---|---|
| `crates/mud-client/src/puzzle.rs` (new) | `PuzzleAction`, `Puzzle`, `Puzzle::needed`, `Puzzle::plan`, `Puzzle::cost`. Pure. |
| `crates/mud-client/src/graph.rs` | `ExitRequirement::Hidden` loses its flag, `ExitRequirement::Puzzle(Puzzle)`, slot decoding, hop counting, `exit_cost_for`, `distances_within_for`. |
| `crates/mud-client/src/nav.rs` | `NavErrorKind::Puzzle`, `open_puzzle`, `solve_puzzle`, `walk_to`, `speak`, the gate stage in `walk_step`. |
| `crates/mud-client/src/roam.rs` | `region` and `Rotation::next` take `Capabilities`. |
| `crates/mud-client/src/farm.rs` | passes the session's capabilities to the roam, drops the dead region check. |
| `crates/mud-client/src/lost.rs` | `is_remote_action` keeps working over fixtures that still hold a slot. No change. |
| `crates/mud-client/tests/puzzle.rs` (new) | the plan and the cost. |
| `crates/mud-client/tests/graph_content.rs` | slot decoding, hops, the cmdtext form. |
| `crates/mud-client/tests/graph.rs`, `tests/lost.rs`, `tests/nav_hidden.rs` | the `Hidden` rename and the new price. |
| `crates/mud-client/tests/nav_puzzle.rs` (new) | scripted boards: same room button, lever pair, already open, stays shut, said aloud, interrupt in a lever room, a lever on a gate. |
| `crates/mud-client/tests/roam.rs` (new) | the region and the rotation with and without the item. |
| `docs/mud-client.md` | a Buttons and levers section, the roam section. |

---

### Task 1: The puzzle module

**Files:**
- Create: `crates/mud-client/src/puzzle.rs`
- Modify: `crates/mud-client/src/lib.rs` (add `pub mod puzzle;` between `pub mod purse;` and `pub mod roam;`)
- Test: `crates/mud-client/tests/puzzle.rs`

**Interfaces:**
- Consumes: `crate::graph::Capabilities` and its `has_item(ItemId) -> bool`, `crate::graph::Cost`.
- Produces:
  - `pub const PUZZLE_BITS: u32 = 0x3ff0`
  - `pub struct PuzzleAction { pub room: RoomId, pub number: u8, pub phrases: Vec<String>, pub item: Option<ItemId>, pub reply: Option<String>, pub hops: Option<u32> }`
  - `impl PuzzleAction { pub fn bit(&self) -> u32; pub fn available(&self, caps: &Capabilities) -> bool }`
  - `pub struct Puzzle { pub word: u32, pub actions: Vec<PuzzleAction> }`
  - `impl Puzzle { pub fn needed(&self) -> Vec<u8>; pub fn plan(&self, caps: &Capabilities) -> Option<Vec<&PuzzleAction>>; pub fn cost(&self, base: u32, caps: &Capabilities) -> Cost }`

- [ ] **Step 1: Write the failing tests**

Create `crates/mud-client/tests/puzzle.rs`:

```rust
//! The plan over a puzzle exit's concealment word: which levers, in
//! which order, and what routing pays for the trip.

use std::sync::Arc;

use mud_client::graph::{Capabilities, Cost};
use mud_client::pack::PackHandle;
use mud_client::puzzle::{Puzzle, PuzzleAction};
use mud_client::sheet::Inventory;
use mud_core::content::{Content, Item, ItemId, RoomId};

const EXIT_ROOM: RoomId = RoomId { map: 1, room: 1056 };
const LEVER_A: RoomId = RoomId { map: 1, room: 1044 };
const LEVER_B: RoomId = RoomId { map: 1, room: 1038 };
const FORK: ItemId = ItemId(500);

fn action(room: RoomId, number: u8, item: Option<ItemId>, hops: Option<u32>) -> PuzzleAction {
    PuzzleAction {
        room,
        number,
        phrases: vec!["pull lever".into()],
        item,
        reply: None,
        hops,
    }
}

/// A walker whose pack holds the titanium fork, the item 79 shipped
/// slots ask for.
fn with_fork() -> Capabilities {
    let mut content = Content::default();
    content.add_item(Item {
        id: FORK,
        name: "titanium fork".into(),
        ..Default::default()
    });
    let pack = PackHandle::new(Arc::new(content));
    pack.refresh(&Inventory {
        items: vec!["titanium fork".into()],
        keys: Vec::new(),
        encumbrance: None,
    });
    Capabilities {
        pack: Some(pack),
        ..Capabilities::unrestricted()
    }
}

/// A walker with no pack at all, which is what routing assumes until a
/// session hands one over.
fn empty_handed() -> Capabilities {
    Capabilities::unrestricted()
}

fn rooms(plan: &[&PuzzleAction]) -> Vec<RoomId> {
    plan.iter().map(|a| a.room).collect()
}

fn numbers(plan: &[&PuzzleAction]) -> Vec<u8> {
    plan.iter().map(|a| a.number).collect()
}

/// The shipped state words are runs of bits from 0x10 up, so the
/// actions a word needs are 1 through its bit count.
#[test]
fn the_word_names_the_actions_it_needs() {
    let needed = |word| {
        Puzzle {
            word,
            actions: Vec::new(),
        }
        .needed()
    };
    assert_eq!(needed(16), vec![1]);
    assert_eq!(needed(48), vec![1, 2]);
    assert_eq!(needed(240), vec![1, 2, 3, 4]);
    assert_eq!(needed(2032), vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(needed(0x2000), vec![10]);
    assert_eq!(
        needed(4),
        Vec::<u8>::new(),
        "bit 2 is the searchable flag, not a puzzle bit"
    );
}

/// Each action clears one bit. Action 0 clears every puzzle bit.
#[test]
fn an_action_knows_the_bit_it_clears() {
    assert_eq!(action(LEVER_A, 1, None, None).bit(), 0x10);
    assert_eq!(action(LEVER_A, 2, None, None).bit(), 0x20);
    assert_eq!(action(LEVER_A, 10, None, None).bit(), 0x2000);
    assert_eq!(action(LEVER_A, 0, None, None).bit(), 0x3ff0);
}

/// The Crypt pair: 1/1044 is action 1 and 1/1038 is action 2 on a word
/// of 48. The plan pulls the higher number first, which the ordered
/// rule requires and the unordered rule allows.
#[test]
fn a_crypt_lever_pair_pulls_the_higher_number_first() {
    let puzzle = Puzzle {
        word: 48,
        actions: vec![
            action(LEVER_A, 1, None, Some(1)),
            action(LEVER_B, 2, None, Some(1)),
        ],
    };
    let plan = puzzle.plan(&empty_handed()).expect("both levers are free");
    assert_eq!(rooms(&plan), vec![LEVER_B, LEVER_A]);
}

/// An action 0 clears the whole word in one phrase, so it beats pulling
/// four levers.
#[test]
fn an_action_zero_does_the_whole_job_in_one_phrase() {
    let puzzle = Puzzle {
        word: 240,
        actions: vec![
            action(LEVER_A, 1, None, Some(1)),
            action(LEVER_A, 2, None, Some(1)),
            action(LEVER_A, 3, None, Some(1)),
            action(LEVER_A, 4, None, Some(1)),
            action(EXIT_ROOM, 0, None, Some(0)),
        ],
    };
    let plan = puzzle.plan(&empty_handed()).expect("the button is free");
    assert_eq!(numbers(&plan), vec![0]);
    assert_eq!(rooms(&plan), vec![EXIT_ROOM]);
}

/// A word that needs a bit no action clears is a wall for everyone.
#[test]
fn a_missing_bit_has_no_plan() {
    let puzzle = Puzzle {
        word: 48,
        actions: vec![action(LEVER_A, 1, None, Some(1))],
    };
    assert_eq!(puzzle.plan(&with_fork()), None);
}

/// 86 shipped slots want an item, 79 of them a titanium fork. Without
/// it the action is not available and the plan fails. With it the plan
/// is the same as for a free lever.
#[test]
fn an_action_needing_an_item_counts_only_when_the_pack_holds_it() {
    let puzzle = Puzzle {
        word: 16,
        actions: vec![action(EXIT_ROOM, 1, Some(FORK), Some(0))],
    };
    assert_eq!(puzzle.plan(&empty_handed()), None);
    let plan = puzzle.plan(&with_fork()).expect("the fork is carried");
    assert_eq!(numbers(&plan), vec![1]);
}

/// An action 0 the walker cannot use is skipped in favour of the bits,
/// and used as soon as the pack allows.
#[test]
fn an_unavailable_action_zero_falls_back_to_the_bits() {
    let puzzle = Puzzle {
        word: 48,
        actions: vec![
            action(EXIT_ROOM, 0, Some(FORK), Some(0)),
            action(LEVER_A, 1, None, Some(1)),
            action(LEVER_B, 2, None, Some(1)),
        ],
    };
    let bare = puzzle.plan(&empty_handed()).expect("the levers are free");
    assert_eq!(numbers(&bare), vec![2, 1]);
    let forked = puzzle.plan(&with_fork()).expect("the button is now usable");
    assert_eq!(numbers(&forked), vec![0]);
}

/// A gate's word carries no puzzle bits: the lever toggles its lock, so
/// any one action does. The graph lists actions nearest first, and the
/// plan takes the first it can use.
#[test]
fn a_word_with_no_puzzle_bits_opens_on_the_nearest_usable_action() {
    let puzzle = Puzzle {
        word: 0,
        actions: vec![
            action(LEVER_A, 0, Some(FORK), Some(1)),
            action(LEVER_B, 0, None, Some(3)),
        ],
    };
    let bare = puzzle.plan(&empty_handed()).expect("the far lever is free");
    assert_eq!(rooms(&bare), vec![LEVER_B]);
    let forked = puzzle.plan(&with_fork()).expect("the near lever is usable");
    assert_eq!(rooms(&forked), vec![LEVER_A]);
    let nothing = Puzzle {
        word: 0,
        actions: Vec::new(),
    };
    assert_eq!(nothing.plan(&with_fork()), None);
}

/// The price is the exit's own base, then one step for each phrase and
/// two per hop for the walk to the lever and back.
#[test]
fn the_cost_is_the_base_plus_a_phrase_and_a_round_trip_per_action() {
    let button = Puzzle {
        word: 16,
        actions: vec![action(EXIT_ROOM, 1, None, Some(0))],
    };
    assert_eq!(button.cost(1, &empty_handed()), Cost::Steps(2));
    let pair = Puzzle {
        word: 48,
        actions: vec![
            action(LEVER_A, 1, None, Some(1)),
            action(LEVER_B, 2, None, Some(1)),
        ],
    };
    assert_eq!(pair.cost(1, &empty_handed()), Cost::Steps(7));
    assert_eq!(pair.cost(5, &empty_handed()), Cost::Steps(11), "a gate keeps its door price");
}

/// Two ways to be a wall: no plan, or a plan through a room no walk
/// reaches.
#[test]
fn no_plan_or_an_unreachable_lever_room_is_impassable() {
    let needs_fork = Puzzle {
        word: 16,
        actions: vec![action(EXIT_ROOM, 1, Some(FORK), Some(0))],
    };
    assert_eq!(needs_fork.cost(1, &empty_handed()), Cost::Impassable);
    let unreachable = Puzzle {
        word: 16,
        actions: vec![action(LEVER_A, 1, None, None)],
    };
    assert_eq!(unreachable.cost(1, &with_fork()), Cost::Impassable);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test puzzle`
Expected: compile error, `mud_client::puzzle` does not exist.

- [ ] **Step 3: Write the module**

Create `crates/mud-client/src/puzzle.rs`:

```rust
//! Buttons and levers: the remote actions that open an exit somewhere.
//!
//! A room's exit slot with type 12 is not an exit. It is a control: a
//! phrase spoken in that room clears bits of another exit's concealment
//! word, or toggles a gate's lock. The graph decodes every slot at load
//! and hangs the result on the exit it opens as
//! [`crate::graph::ExitRequirement::Puzzle`]. This module holds the
//! shape and the one pure question routing and walking both ask: which
//! actions does this walker have to perform, and in what order.
//!
//! The bit rules are mud-core's `remote_lever`, which follows the DLL at
//! 65973 to 66079. Action n clears bit n+3 of the word, so action 1
//! clears 0x10 and action 10 clears 0x2000. Action 0 clears every
//! puzzle bit at once. Unless the exit's para2 is negative, action n
//! only works while bit n+4 is already clear, so levers pull in
//! descending order. Descending is valid either way, which is why the
//! plan never asks whether the puzzle is ordered.
//!
//! A gate is different: `remote_gate_toggle` flips its lock on every
//! action regardless of number, and its word carries no puzzle bits. The
//! plan reads that as "any one action".

use mud_core::content::{ItemId, RoomId};

use crate::graph::{Capabilities, Cost};

/// The bits of a hidden exit's state word that a lever clears. Bit 2 is
/// the searchable flag and is not among them: a search never touches a
/// puzzle bit and a lever never touches bit 2.
pub const PUZZLE_BITS: u32 = 0x3ff0;

/// The highest action number a word can call for, the one that clears
/// bit 0x2000.
const MAX_ACTION: u8 = 10;

/// One phrase spoken in one room, and what it does to the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PuzzleAction {
    /// The room the phrase must be spoken in.
    pub room: RoomId,
    /// 0 clears every puzzle bit. 1 to 10 clear one bit each.
    pub number: u8,
    /// Line 1 of the slot's message, then line 2 when it is not blank.
    /// The walker speaks the first. The rest are aliases the board also
    /// accepts.
    pub phrases: Vec<String>,
    /// An item the actor must carry, when the slot names one.
    pub item: Option<ItemId>,
    /// Line 1 of the slot's response message: what the actor hears.
    pub reply: Option<String>,
    /// Steps from the exit's own room to `room`, counted at load.
    /// `None` when no walk reaches it. Zero for a button in the exit's
    /// own room, which is 200 of the 287 shipped slots.
    pub hops: Option<u32>,
}

impl PuzzleAction {
    /// The word bits this action clears.
    pub fn bit(&self) -> u32 {
        match self.number {
            0 => PUZZLE_BITS,
            n => 0x10 << (n - 1),
        }
    }

    /// Can this walker perform the action: no item needed, or the item
    /// in the pack.
    pub fn available(&self, caps: &Capabilities) -> bool {
        self.item.is_none_or(|id| caps.has_item(id))
    }
}

/// Everything that opens one exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Puzzle {
    /// The exit's para1. On a hidden exit it is the concealment word.
    /// On a gate it is not a word at all and carries no puzzle bits.
    pub word: u32,
    /// Every action that targets this exit, nearest first.
    pub actions: Vec<PuzzleAction>,
}

impl Puzzle {
    /// The action numbers the word calls for, ascending.
    pub fn needed(&self) -> Vec<u8> {
        (1..=MAX_ACTION)
            .filter(|n| self.word & (0x10 << (n - 1)) != 0)
            .collect()
    }

    /// Which actions this walker performs, in order. `None` means the
    /// walker cannot open the exit at all.
    ///
    /// A word with no puzzle bits, which is every gate and a hidden exit
    /// nobody armed, opens on any one action. Otherwise an available
    /// action 0 does the whole job in one phrase, and failing that every
    /// needed bit must have its own available action. The order is
    /// descending by number.
    pub fn plan(&self, caps: &Capabilities) -> Option<Vec<&PuzzleAction>> {
        let needed = self.needed();
        if needed.is_empty() {
            return self
                .actions
                .iter()
                .find(|a| a.available(caps))
                .map(|a| vec![a]);
        }
        if let Some(all) = self
            .actions
            .iter()
            .find(|a| a.number == 0 && a.available(caps))
        {
            return Some(vec![all]);
        }
        needed
            .iter()
            .rev()
            .map(|n| {
                self.actions
                    .iter()
                    .find(|a| a.number == *n && a.available(caps))
            })
            .collect()
    }

    /// What routing pays to take the exit: `base` for the exit itself,
    /// then one step per phrase and the walk to each action room and
    /// back. `Impassable` when there is no plan, or when a planned
    /// action sits in a room no walk reaches.
    pub fn cost(&self, base: u32, caps: &Capabilities) -> Cost {
        let Some(plan) = self.plan(caps) else {
            return Cost::Impassable;
        };
        let mut total = base;
        for action in plan {
            let Some(hops) = action.hops else {
                return Cost::Impassable;
            };
            total += 1 + 2 * hops;
        }
        Cost::Steps(total)
    }
}
```

Add `pub mod puzzle;` to `crates/mud-client/src/lib.rs` after `pub mod purse;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test puzzle`
Expected: 10 passed.

- [ ] **Step 5: Run the whole suite and commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: every line `0 failed`.

```bash
git add crates/mud-client/src/puzzle.rs crates/mud-client/src/lib.rs crates/mud-client/tests/puzzle.rs
git commit -m "feat(client): the puzzle plan over a concealment word"
```

---

### Task 2: The graph speaks the new requirement

`Hidden { searchable }` collapses to `Hidden` and `Puzzle { actions }` becomes `Puzzle(puzzle::Puzzle)`. The cmdtext `remoteaction` pass is ported onto the new shape so the crate compiles at the end of this task. Type 12 slots, replies, items and hops arrive in Task 3.

**Files:**
- Modify: `crates/mud-client/src/graph.rs` (`ExitRequirement`, remove the old `PuzzleAction`, `from_exit_type`, `exit_cost_for`, `from_content`'s second pass, `remote_actions_from_content`)
- Modify: `crates/mud-client/src/nav.rs:900-911` (the `searchable_hidden` computation)
- Modify: `crates/mud-client/tests/graph.rs`, `crates/mud-client/tests/graph_content.rs`, `crates/mud-client/tests/lost.rs:281`, `crates/mud-client/tests/nav_hidden.rs:127-138`

**Interfaces:**
- Consumes: `crate::puzzle::{Puzzle, PuzzleAction}` from Task 1.
- Produces:
  - `ExitRequirement::Hidden` (unit variant) and `ExitRequirement::Puzzle(crate::puzzle::Puzzle)`.
  - `exit_cost_for` prices `Hidden` at `exit_cost(6)`, 40, and `Puzzle` at `puzzle.cost(base, caps)` where `base` is `exit_cost(exit_type)` for a door or gate and 1 otherwise.
  - `RoomGraph::from_content` builds a `Puzzle` only on exits of type 6, 7 and 0xb.

- [ ] **Step 1: Update the tests that name the old shapes**

In `crates/mud-client/tests/graph.rs`, replace the test `a_puzzle_concealed_exit_is_impassable_not_expensive` with:

```rust
/// A puzzle exit is priced by its plan: the exit itself, then a phrase
/// and a round trip per lever. A searchable hidden exit keeps the
/// search price. A puzzle this walker cannot solve is a wall, not a
/// dear edge: pricing it high would still route through it whenever the
/// detour was longer, and then stall at the wall.
#[test]
fn a_puzzle_exit_is_priced_by_its_plan() {
    use mud_client::puzzle::{Puzzle, PuzzleAction};
    use mud_core::content::ItemId;

    let caps = Capabilities::unrestricted();
    let edge = RoomId { map: 1, room: 506 };
    let button = |item| Puzzle {
        word: 16,
        actions: vec![PuzzleAction {
            room: edge,
            number: 1,
            phrases: vec!["push button".into()],
            item,
            reply: None,
            hops: Some(0),
        }],
    };
    assert_eq!(
        exit_cost_for(&ExitRequirement::Puzzle(button(None)), 6, edge, Direction::South, &caps),
        Cost::Steps(2),
        "a revealed passage is a step, plus the phrase"
    );
    assert_eq!(
        exit_cost_for(&ExitRequirement::Puzzle(button(None)), 0xb, edge, Direction::South, &caps),
        Cost::Steps(6),
        "a lever on a gate keeps the door price underneath"
    );
    assert_eq!(
        exit_cost_for(
            &ExitRequirement::Puzzle(button(Some(ItemId(500)))),
            6,
            edge,
            Direction::South,
            &caps
        ),
        Cost::Impassable,
        "an action needing an item the pack lacks is a wall"
    );
    assert_eq!(
        exit_cost_for(&ExitRequirement::Hidden, 6, edge, Direction::North, &caps),
        Cost::Steps(40),
        "a searchable hidden exit keeps its old price"
    );
}
```

In `crates/mud-client/tests/graph_content.rs`, change the test `a_remoteaction_script_conceals_the_named_exit`: the target exit becomes type 7 with the comment updated, and the assertions read the new shape.

```rust
    // A gate exit, type 7, so the puzzle pass overwrites Door. A type 2
    // key door would be left alone: mud-core's remote action dispatch
    // only answers for types 6, 7 and 0xb.
    target.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 7,
        ..Default::default()
    });
```

and at the end of that test:

```rust
    let ExitRequirement::Puzzle(puzzle) = &edge.requirement else {
        panic!("expected a puzzle, got {:?}", edge.requirement);
    };
    assert_eq!(puzzle.actions.len(), 1);
    assert_eq!(puzzle.actions[0].room, lever_room);
    assert_eq!(puzzle.actions[0].number, 0);
    assert_eq!(puzzle.actions[0].phrases, vec!["pull lever".to_string()]);
```

In the same file, `an_unrelated_cmdtext_block_leaves_exits_alone` ends with `assert_eq!(edge.requirement, ExitRequirement::Hidden);`.

In `crates/mud-client/tests/lost.rs:281`, `requirement: ExitRequirement::Hidden { searchable: false },` becomes `requirement: ExitRequirement::Hidden,`.

In `crates/mud-client/tests/nav_hidden.rs`, replace `graph_with_puzzle_exit` with:

```rust
/// The alleyway fixture, but the exit is concealed by a puzzle word
/// whose one action needs an item this walker does not carry. Routing
/// refuses the edge outright, so the walk never reaches the wall.
fn graph_with_puzzle_exit() -> Arc<RoomGraph> {
    use mud_client::puzzle::{Puzzle, PuzzleAction};
    use mud_core::content::ItemId;

    let g = graph_with_exit(6);
    let mut rooms: Vec<(RoomId, GraphRoom)> = g.iter().map(|(id, r)| (id, r.clone())).collect();
    for (id, room) in rooms.iter_mut() {
        for edge in room.exits.iter_mut().flatten() {
            edge.requirement = ExitRequirement::Puzzle(Puzzle {
                word: 16,
                actions: vec![PuzzleAction {
                    room: *id,
                    number: 1,
                    phrases: vec!["push button".into()],
                    item: Some(ItemId(500)),
                    reply: None,
                    hops: Some(0),
                }],
            });
        }
    }
    Arc::new(RoomGraph::from_rooms(rooms))
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test graph --test graph_content --test lost --test nav_hidden`
Expected: compile errors, `ExitRequirement::Hidden` takes fields and `Puzzle` is not a tuple variant.

- [ ] **Step 3: Change the requirement**

In `crates/mud-client/src/graph.rs`, replace the `Hidden` and `Puzzle` variants of `ExitRequirement` and their doc comments:

```rust
    /// Concealed, and a search can reveal it. Type 6, unless a button
    /// or lever targets it, in which case it is
    /// [`ExitRequirement::Puzzle`] and no search roll ever clears it.
    Hidden,
```

```rust
    /// Opened by a button or a lever: a type 12 slot or a cmdtext
    /// `remoteaction` line targets this exit. The slot itself is never
    /// an edge. The exit is a hidden passage, type 6, or a gate, type 7
    /// or 0xb, and [`ExitEdge::exit_type`] still says which, so the walk
    /// knows a gate a lever unlocked still wants an `open`. The plan and
    /// the price live in [`crate::puzzle::Puzzle`].
    Puzzle(crate::puzzle::Puzzle),
```

Delete the `PuzzleAction` struct from `graph.rs` and its doc comment.

In `from_exit_type`, `6 => ExitRequirement::Hidden { searchable: true },` becomes `6 => ExitRequirement::Hidden,` and its comment line `// Searchable until the cmdtext pass proves otherwise.` becomes `// Searchable until a slot or a script targets it.`

In `exit_cost_for`, replace the `Hidden { searchable: false }` arm and its comment with:

```rust
        // A lever exit costs what its plan costs. No plan is a wall:
        // pricing it high would still route through it whenever the
        // detour was longer, and then stall at the wall.
        ExitRequirement::Puzzle(puzzle) => {
            // The exit itself once open: a gate still wants an open, a
            // revealed passage is a step. The search price never
            // applies, no roll is spent.
            let base = if crate::nav::is_door(exit_type) {
                exit_cost(exit_type)
            } else {
                1
            };
            puzzle.cost(base, caps)
        }
```

Update the doc comment above `exit_cost_for` so its last sentence reads: `Impassable` is reserved for edges no amount of walking opens: a toll beyond the purse, and a lever exit this walker has no plan for.

Replace `from_content`'s second pass:

```rust
        // Second pass, because the target exit may be read before or
        // after the room that opens it.
        for ((target, exit), actions) in remote_actions {
            let word = content
                .rooms
                .get(&target)
                .and_then(|r| r.exits[exit].as_ref())
                .map(|e| e.param)
                .unwrap_or(0);
            let Some(edge) = rooms
                .get_mut(&target)
                .and_then(|r| r.exits[exit].as_mut())
            else {
                continue;
            };
            // Only a hidden exit or a gate answers a lever. mud-core's
            // remote action dispatch does nothing for any other type,
            // so a slot aimed at a plain exit changes nothing about it.
            if !matches!(edge.exit_type, 6 | 7 | 0xb) {
                continue;
            }
            edge.requirement = ExitRequirement::Puzzle(crate::puzzle::Puzzle {
                word: u32::try_from(word).unwrap_or(0),
                actions,
            });
        }
```

In `remote_actions_from_content`, change the return type and the entry building to the new action shape. The signature becomes:

```rust
    fn remote_actions_from_content(
        content: &Content,
    ) -> BTreeMap<(RoomId, usize), Vec<crate::puzzle::PuzzleAction>> {
        let mut out: BTreeMap<(RoomId, usize), Vec<crate::puzzle::PuzzleAction>> = BTreeMap::new();
```

and the parse of one directive becomes:

```rust
                    let nums: Vec<i64> = w.filter_map(|n| n.parse().ok()).collect();
                    // remoteaction <room> <msg> <action> <exit>
                    let [target, _msg, action, exit] = nums[..] else {
                        continue;
                    };
                    let (Ok(target), Ok(exit), Ok(number)) =
                        (u16::try_from(target), usize::try_from(exit), u8::try_from(action))
                    else {
                        continue;
                    };
                    if exit > 9 || number > 10 {
                        continue;
                    }
                    let key = (
                        RoomId {
                            map: actor.map,
                            room: target,
                        },
                        exit,
                    );
                    let entry = out.entry(key).or_default();
                    match entry
                        .iter_mut()
                        .find(|a| a.room == actor && a.number == number)
                    {
                        Some(a) => a.phrases.push(phrase.to_string()),
                        None => entry.push(crate::puzzle::PuzzleAction {
                            room: actor,
                            number,
                            phrases: vec![phrase.to_string()],
                            item: None,
                            reply: None,
                            hops: None,
                        }),
                    }
```

In `crates/mud-client/src/nav.rs`, replace the `searchable_hidden` computation at lines 900 to 911 with:

```rust
                // A type 6 exit concealed by a puzzle word answers
                // SEARCH exactly as a searchable one does and can never
                // be revealed by it, so the graph has to say which this
                // is. Searching a puzzle exit is unbounded: the roll can
                // never succeed.
                let searchable_hidden = edge
                    .map(|e| matches!(e.requirement, crate::graph::ExitRequirement::Hidden))
                    .unwrap_or(false);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test graph --test graph_content --test lost --test nav_hidden`
Expected: all pass. `a_puzzle_concealed_exit_is_never_searched` still passes: the route is `None` so no search is spent.

- [ ] **Step 5: Run the whole suite and commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: every line `0 failed`.

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/src/nav.rs crates/mud-client/tests/graph.rs crates/mud-client/tests/graph_content.rs crates/mud-client/tests/lost.rs crates/mud-client/tests/nav_hidden.rs
git commit -m "refactor(client): a puzzle exit carries its plan and a hidden exit is always searchable"
```

---

### Task 3: Type 12 slots, replies, items and hops

**Files:**
- Modify: `crates/mud-client/src/graph.rs` (`from_content`, new `slot`, `message_lines`, `hops_from`, new `REMOTE_ACTION_EXIT`)
- Modify: `crates/mud-client/src/nav.rs:358-360` (`is_remote_action` reads the constant)
- Test: `crates/mud-client/tests/graph_content.rs`

**Interfaces:**
- Consumes: `crate::puzzle::{Puzzle, PuzzleAction}`, `mud_core::content::Exit::{param, param2, param3, param4, dest}`, `Content::messages`.
- Produces:
  - `pub const REMOTE_ACTION_EXIT: i64 = 12;` in `graph.rs`.
  - A loaded graph has no edge for a type 12 slot. The target exit's `Puzzle.actions` are sorted by `(hops, room, number)` with `hops` filled by a breadth first search from the target room, `None` when unreachable.
  - Slot decoding: `para2 < 10` is exit index `para2` with action 0, otherwise action `para2 / 10` on exit `para2 % 10`. `phrases` are the first two non-blank trimmed lines of message `para1`. `reply` is the first non-blank line of message `para3`. `item` is `para4` when non-zero.
  - cmdtext form: `reply` is the second non-blank line of the directive's `<msg>` message, because that pair prints the room line first and the actor line second, mud-core's `quest_remote_message`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/graph_content.rs`:

```rust
/// The room that started this, 1/506 Secret Passage. Its west slot is a
/// type 12 remote action: para1 names the phrase message, para2 = 11
/// is action 1 on exit 1 which is south, para3 names the response. The
/// south exit is type 6 with state 16, which no search clears. The slot
/// is a control, not an exit, so the graph has no west edge at all.
#[test]
fn a_type_12_slot_becomes_a_puzzle_on_its_target_and_no_edge_of_its_own() {
    use mud_client::puzzle::PuzzleAction;

    let passage = RoomId { map: 1, room: 506 };
    let hallway = RoomId { map: 1, room: 507 };
    let mut content = Content::default();
    let mut room = Room {
        id: passage,
        name: "Secret Passage".into(),
        ..Default::default()
    };
    room.exits[Direction::South as usize] = Some(Exit {
        dest: hallway,
        exit_type: 6,
        param: 16,
        ..Default::default()
    });
    room.exits[Direction::West as usize] = Some(Exit {
        dest: passage,
        exit_type: 12,
        param: 8259,
        param2: 11,
        param3: 8260,
        param4: 0,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: hallway,
        name: "Wooden Hallway".into(),
        ..Default::default()
    });
    content.add_message(Message {
        id: MessageId(8259),
        lines: vec!["push button".into(), "press button".into(), "".into()],
    });
    content.add_message(Message {
        id: MessageId(8260),
        lines: vec!["You push the button.".into(), "%s pushes a button.".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let room = graph.room(passage).unwrap();
    assert!(room.exits[Direction::West as usize].is_none(), "a slot is not an edge");
    let ExitRequirement::Puzzle(puzzle) = &room.exits[Direction::South as usize]
        .as_ref()
        .unwrap()
        .requirement
    else {
        panic!("the south wall should be a puzzle");
    };
    assert_eq!(puzzle.word, 16);
    assert_eq!(
        puzzle.actions,
        vec![PuzzleAction {
            room: passage,
            number: 1,
            phrases: vec!["push button".into(), "press button".into()],
            item: None,
            reply: Some("You push the button.".into()),
            hops: Some(0),
        }]
    );
}

/// The Crypt pair. 1/1044 holds `pull lever` as action 1 and 1/1038 as
/// action 2, both on 1/1056 north with state 48. Each lever room is one
/// step from the hallway, so both actions count one hop and sort by
/// room id between themselves.
#[test]
fn a_lever_in_another_room_counts_its_hops() {
    let hall = RoomId { map: 1, room: 1056 };
    let beyond = RoomId { map: 1, room: 1063 };
    let lever_a = RoomId { map: 1, room: 1044 };
    let lever_b = RoomId { map: 1, room: 1038 };
    let mut content = Content::default();

    let mut hall_room = Room {
        id: hall,
        name: "Crypt, Stone Hallway".into(),
        ..Default::default()
    };
    hall_room.exits[Direction::North as usize] = Some(Exit {
        dest: beyond,
        exit_type: 6,
        param: 48,
        param2: -2,
        ..Default::default()
    });
    hall_room.exits[Direction::East as usize] = Some(Exit {
        dest: lever_a,
        ..Default::default()
    });
    hall_room.exits[Direction::West as usize] = Some(Exit {
        dest: lever_b,
        ..Default::default()
    });
    content.add_room(hall_room);
    content.add_room(Room {
        id: beyond,
        name: "Crypt, Dark Passage".into(),
        ..Default::default()
    });

    let mut alcove = |id: RoomId, back: Direction, para2: i32| {
        let mut room = Room {
            id,
            name: "Crypt, Alcove".into(),
            ..Default::default()
        };
        room.exits[back as usize] = Some(Exit {
            dest: hall,
            ..Default::default()
        });
        room.exits[Direction::South as usize] = Some(Exit {
            dest: hall,
            exit_type: 12,
            param: 100,
            param2: para2,
            param3: 101,
            ..Default::default()
        });
        content.add_room(room);
    };
    // para2 = 10 is action 1 on exit 0, north. para2 = 20 is action 2.
    alcove(lever_a, Direction::West, 10);
    alcove(lever_b, Direction::East, 20);
    content.add_message(Message {
        id: MessageId(100),
        lines: vec!["pull lever".into()],
    });
    content.add_message(Message {
        id: MessageId(101),
        lines: vec!["You pull the lever. Off in the distance you hear a small click.".into()],
    });

    let graph = RoomGraph::from_content(&content);
    for id in [lever_a, lever_b] {
        assert!(
            graph.room(id).unwrap().exits[Direction::South as usize].is_none(),
            "the slot in {id:?} must not be an edge"
        );
    }
    let ExitRequirement::Puzzle(puzzle) = &graph.room(hall).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap()
        .requirement
    else {
        panic!("the north wall should be a puzzle");
    };
    assert_eq!(puzzle.word, 48);
    let summary: Vec<(RoomId, u8, Option<u32>)> =
        puzzle.actions.iter().map(|a| (a.room, a.number, a.hops)).collect();
    assert_eq!(
        summary,
        vec![(lever_b, 2, Some(1)), (lever_a, 1, Some(1))],
        "equal hops sort by room id"
    );
    let plan = puzzle
        .plan(&mud_client::graph::Capabilities::unrestricted())
        .expect("both levers are free");
    assert_eq!(plan.iter().map(|a| a.number).collect::<Vec<_>>(), vec![2, 1]);
}

/// A slot whose para4 names an item records it, so the plan can refuse
/// the exit to a walker without one.
#[test]
fn a_slot_that_needs_an_item_records_it() {
    let here = RoomId { map: 1, room: 1 };
    let there = RoomId { map: 1, room: 2 };
    let mut content = Content::default();
    let mut room = Room {
        id: here,
        name: "Fork Room".into(),
        ..Default::default()
    };
    room.exits[Direction::North as usize] = Some(Exit {
        dest: there,
        exit_type: 6,
        param: 16,
        ..Default::default()
    });
    room.exits[Direction::Up as usize] = Some(Exit {
        dest: here,
        exit_type: 12,
        param: 5,
        param2: 10,
        param4: 500,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: there,
        name: "Beyond".into(),
        ..Default::default()
    });
    content.add_message(Message {
        id: MessageId(5),
        lines: vec!["use fork".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let ExitRequirement::Puzzle(puzzle) = &graph.room(here).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap()
        .requirement
    else {
        panic!("expected a puzzle");
    };
    assert_eq!(puzzle.actions[0].item, Some(mud_core::content::ItemId(500)));
    assert_eq!(puzzle.actions[0].reply, None, "no response message, no reply");
}

/// mud-core's remote action dispatch does nothing for a plain exit, so
/// a slot aimed at one leaves it plain. The slot still vanishes.
#[test]
fn a_slot_onto_a_plain_exit_changes_nothing() {
    let here = RoomId { map: 1, room: 1 };
    let there = RoomId { map: 1, room: 2 };
    let mut content = Content::default();
    let mut room = Room {
        id: here,
        name: "Plain Room".into(),
        ..Default::default()
    };
    room.exits[Direction::North as usize] = Some(Exit {
        dest: there,
        exit_type: 0,
        ..Default::default()
    });
    room.exits[Direction::Up as usize] = Some(Exit {
        dest: here,
        exit_type: 12,
        param: 5,
        param2: 10,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: there,
        name: "Beyond".into(),
        ..Default::default()
    });
    content.add_message(Message {
        id: MessageId(5),
        lines: vec!["push button".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let room = graph.room(here).unwrap();
    assert!(room.exits[Direction::Up as usize].is_none());
    assert_eq!(
        room.exits[Direction::North as usize].as_ref().unwrap().requirement,
        ExitRequirement::None
    );
}

/// A lever room no walk reaches from the exit's room leaves the action
/// with no hops, and routing then refuses the exit.
#[test]
fn an_unreachable_lever_room_has_no_hops_and_the_exit_is_impassable() {
    use mud_client::graph::{Capabilities, Cost, exit_cost_for};

    let here = RoomId { map: 1, room: 1 };
    let there = RoomId { map: 1, room: 2 };
    let island = RoomId { map: 1, room: 3 };
    let mut content = Content::default();
    let mut room = Room {
        id: here,
        name: "Gate Room".into(),
        ..Default::default()
    };
    room.exits[Direction::North as usize] = Some(Exit {
        dest: there,
        exit_type: 6,
        param: 16,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: there,
        name: "Beyond".into(),
        ..Default::default()
    });
    let mut lever = Room {
        id: island,
        name: "Island".into(),
        ..Default::default()
    };
    lever.exits[Direction::Down as usize] = Some(Exit {
        dest: here,
        exit_type: 12,
        param: 5,
        param2: 10,
        ..Default::default()
    });
    content.add_room(lever);
    content.add_message(Message {
        id: MessageId(5),
        lines: vec!["pull lever".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let edge = graph.room(here).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap();
    let ExitRequirement::Puzzle(puzzle) = &edge.requirement else {
        panic!("expected a puzzle");
    };
    assert_eq!(puzzle.actions[0].hops, None);
    assert_eq!(
        exit_cost_for(&edge.requirement, 6, here, Direction::North, &Capabilities::unrestricted()),
        Cost::Impassable
    );
}

/// The cmdtext form names a message pair whose second line is what the
/// actor hears, and the action number is the directive's own.
#[test]
fn a_remoteaction_script_records_the_actor_line_and_the_number() {
    let lever_room: RoomId = RoomId { map: 1, room: 3 };
    let mut content = Content::default();
    let mut target = Room {
        id: HERE,
        name: "Vault".into(),
        ..Default::default()
    };
    target.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 6,
        param: 32,
        ..Default::default()
    });
    target.exits[Direction::East as usize] = Some(Exit {
        dest: lever_room,
        ..Default::default()
    });
    content.add_room(target);
    let mut lever = Room {
        id: lever_room,
        name: "Lever Room".into(),
        command_block: Some(TextBlockId(9)),
        ..Default::default()
    };
    lever.exits[Direction::West as usize] = Some(Exit {
        dest: HERE,
        ..Default::default()
    });
    content.add_room(lever);
    content.add_text_block(TextBlock {
        id: TextBlockId(9),
        next: None,
        body: "pull lever:remoteaction 1 7 2 0".into(),
    });
    content.add_message(Message {
        id: MessageId(7),
        lines: vec!["%s pulls a lever.".into(), "You pull the lever.".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let ExitRequirement::Puzzle(puzzle) = &graph.room(HERE).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap()
        .requirement
    else {
        panic!("expected a puzzle");
    };
    assert_eq!(puzzle.word, 32);
    assert_eq!(puzzle.actions[0].number, 2);
    assert_eq!(puzzle.actions[0].reply.as_deref(), Some("You pull the lever."));
    assert_eq!(puzzle.actions[0].hops, Some(1));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test graph_content`
Expected: the new tests fail. The slot test panics on `a slot is not an edge`, the hops tests see `hops: None`, the cmdtext test sees `reply: None`.

- [ ] **Step 3: Decode the slots and count the hops**

In `crates/mud-client/src/graph.rs`, add after `pub const COMMAND_EXIT: i64 = 10;`:

```rust
/// Exit type for a remote action slot, a button or a lever. Never an
/// edge: the graph decodes it into a [`crate::puzzle::PuzzleAction`] on
/// the exit it opens and drops the slot.
pub const REMOTE_ACTION_EXIT: i64 = 12;
```

Add `use std::collections::VecDeque;` to the `std::collections` import line, and `ItemId` to the `mud_core::content` import.

Change `from_content` so the room loop collects slots and the second pass counts hops. The whole function becomes:

```rust
    pub fn from_content(content: &Content) -> Self {
        let mut remote_actions = Self::remote_actions_from_content(content);
        let mut rooms = BTreeMap::new();
        for (id, room) in &content.rooms {
            let mut graph_room = GraphRoom {
                name: room.name.clone(),
                exits: Default::default(),
                shop: room.shop.map(|s| i64::from(s.0)).unwrap_or(0),
                spawn: Spawn {
                    kind: SpawnKind::from_column(i64::from(room.room_type)),
                    region: i64::from(room.spawn_zone),
                    band: (i64::from(room.min_level), i64::from(room.max_level)),
                    forced: room.forced_monster.map(|m| i64::from(m.0)),
                    resident: room.boss_monster.map(|m| i64::from(m.0)),
                },
                light: i64::from(room.light),
            };
            for exit in room.exits.iter().flatten() {
                let exit_type = i64::from(exit.exit_type);
                if exit_type != REMOTE_ACTION_EXIT {
                    continue;
                }
                // A button or lever. Not an edge: it is recorded against
                // the exit it opens and the slot itself vanishes.
                if let Some((key, action)) = Self::slot(content, *id, exit) {
                    remote_actions.entry(key).or_default().push(action);
                }
            }
            for (d, exit) in room.exits.iter().enumerate() {
                let Some(exit) = exit else { continue };
                let exit_type = i64::from(exit.exit_type);
                if exit_type == REMOTE_ACTION_EXIT {
                    continue;
                }
                let param = i64::from(exit.param);
                graph_room.exits[d] = Some(ExitEdge {
                    dest: exit.dest,
                    exit_type,
                    command: (exit_type == COMMAND_EXIT)
                        .then(|| Self::exit_command(content, exit.trigger_msg))
                        .flatten(),
                    requirement: ExitRequirement::from_exit_type(exit_type, param),
                });
            }
            rooms.insert(*id, graph_room);
        }
        // Second pass, because the target exit may be read before or
        // after the room that opens it, and because the hop count needs
        // every room in place.
        for ((target, exit), mut actions) in remote_actions {
            let word = content
                .rooms
                .get(&target)
                .and_then(|r| r.exits[exit].as_ref())
                .map(|e| e.param)
                .unwrap_or(0);
            let wanted: BTreeSet<RoomId> = actions.iter().map(|a| a.room).collect();
            let hops = Self::hops_from(&rooms, target, &wanted);
            for action in &mut actions {
                action.hops = hops.get(&action.room).copied();
            }
            // Nearest first, so a plan that may pick any one action
            // picks the closest. Unreachable rooms sort last.
            actions.sort_by_key(|a| (a.hops.unwrap_or(u32::MAX), a.room, a.number));
            let Some(edge) = rooms
                .get_mut(&target)
                .and_then(|r| r.exits[exit].as_mut())
            else {
                continue;
            };
            // Only a hidden exit or a gate answers a lever. mud-core's
            // remote action dispatch does nothing for any other type,
            // so a slot aimed at a plain exit changes nothing about it.
            if !matches!(edge.exit_type, 6 | 7 | 0xb) {
                continue;
            }
            edge.requirement = ExitRequirement::Puzzle(crate::puzzle::Puzzle {
                word: u32::try_from(word).unwrap_or(0),
                actions,
            });
        }
        RoomGraph { rooms }
    }
```

Add these three associated functions after `exit_command`:

```rust
    /// Decode one type 12 slot: which exit it opens and what doing so
    /// takes. Nightmare's map source decodes the same fields, frmMap.frm
    /// near line 21705 in the bbs backup. `None` for a slot with no
    /// phrase or an unusable action number.
    fn slot(
        content: &Content,
        actor: RoomId,
        exit: &mud_core::content::Exit,
    ) -> Option<((RoomId, usize), crate::puzzle::PuzzleAction)> {
        // para2 below 10 is a bare exit index for action 0. Otherwise
        // the tens are the action and the ones the exit index.
        let para2 = u32::try_from(exit.param2).ok()?;
        let (number, index) = if para2 < 10 {
            (0, para2)
        } else {
            (para2 / 10, para2 % 10)
        };
        let number = u8::try_from(number).ok().filter(|n| *n <= 10)?;
        let index = usize::try_from(index).ok()?;
        let phrases: Vec<String> = Self::message_lines(content, exit.param).take(2).collect();
        if phrases.is_empty() {
            return None;
        }
        let reply = Self::message_lines(content, exit.param3).next();
        let item = u16::try_from(exit.param4)
            .ok()
            .filter(|id| *id != 0)
            .map(ItemId);
        Some((
            (exit.dest, index),
            crate::puzzle::PuzzleAction {
                room: actor,
                number,
                phrases,
                item,
                reply,
                hops: None,
            },
        ))
    }

    /// The non-blank lines of a message, trimmed, in order. Empty when
    /// the id is zero or unknown.
    fn message_lines(content: &Content, id: i32) -> impl Iterator<Item = String> + '_ {
        u16::try_from(id)
            .ok()
            .filter(|m| *m != 0)
            .and_then(|m| content.messages.get(&MessageId(m)))
            .into_iter()
            .flat_map(|m| m.lines.iter())
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
    }

    /// Steps from `from` to each of `wanted` by the fewest edges, over
    /// the rooms built so far. Stops as soon as every wanted room is
    /// found. A room it never reaches is absent from the answer.
    fn hops_from(
        rooms: &BTreeMap<RoomId, GraphRoom>,
        from: RoomId,
        wanted: &BTreeSet<RoomId>,
    ) -> BTreeMap<RoomId, u32> {
        let mut found = BTreeMap::new();
        let mut seen = BTreeSet::from([from]);
        let mut frontier = VecDeque::from([(from, 0u32)]);
        while let Some((at, hops)) = frontier.pop_front() {
            if wanted.contains(&at) {
                found.insert(at, hops);
                if found.len() == wanted.len() {
                    break;
                }
            }
            let Some(room) = rooms.get(&at) else { continue };
            for edge in room.exits.iter().flatten() {
                if rooms.contains_key(&edge.dest) && seen.insert(edge.dest) {
                    frontier.push_back((edge.dest, hops + 1));
                }
            }
        }
        found
    }
```

In `remote_actions_from_content`, the directive's message now fills `reply`. Replace `let [target, _msg, action, exit] = nums[..]` with `let [target, msg, action, exit] = nums[..]`, and in the `None =>` arm set `reply: Self::message_lines(content, i32::try_from(msg).unwrap_or(0)).nth(1),` with this comment above the arm:

```rust
                        // The directive's message pair prints the room
                        // line first and the actor line second, which
                        // is the other way round from a slot's para3.
```

Update the doc comment of `remote_actions_from_content` to say it feeds the same map as the type 12 slots and that the slots are the main mechanism.

In `crates/mud-client/src/nav.rs`, `is_remote_action` becomes:

```rust
pub fn is_remote_action(exit_type: i64) -> bool {
    exit_type == crate::graph::REMOTE_ACTION_EXIT
}
```

and add one sentence to its doc comment: A loaded graph carries no slot at all since the graph decodes them into puzzles, so this only matters for a fixture built by hand.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test graph_content --test puzzle --test graph`
Expected: all pass.

- [ ] **Step 5: Check the shipped world loads and the 1/506 slot decodes**

Run this once by hand. It reads `re/`, so it is not a test.

```bash
cat > /home/daniel/majormud/majormud/crates/mud-client/examples/slot_census.rs <<'EOF'
use mud_client::graph::{ExitRequirement, RoomGraph};
use mud_core::content::{Direction, RoomId};

fn main() {
    let g = RoomGraph::load(std::path::Path::new("re/mmud_wgnt.sqlite")).unwrap();
    let mut puzzles = 0;
    for (_, room) in g.iter() {
        for edge in room.exits.iter().flatten() {
            if matches!(edge.requirement, ExitRequirement::Puzzle(_)) {
                puzzles += 1;
            }
            assert_ne!(edge.exit_type, 12, "a slot survived as an edge");
        }
    }
    println!("puzzle exits: {puzzles}");
    let passage = g.room(RoomId { map: 1, room: 506 }).unwrap();
    println!("1/506 south: {:?}", passage.exits[Direction::South as usize]);
}
EOF
cargo run -p mud-client --example slot_census
rm /home/daniel/majormud/majormud/crates/mud-client/examples/slot_census.rs
rmdir /home/daniel/majormud/majormud/crates/mud-client/examples 2>/dev/null || true
```

Expected: `puzzle exits:` in the region of 270, the spec counted 248 hidden plus 19 gates plus 4 doors with 3 targets missing and some shared. `1/506 south` prints a `Puzzle` with `word: 16`, one action in room 506, number 1, phrases starting `push button`, `hops: Some(0)`. Record the printed count in the commit message body.

- [ ] **Step 6: Run the whole suite and commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: every line `0 failed`.

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/src/nav.rs crates/mud-client/tests/graph_content.rs
git commit -m "feat(client): type 12 slots become puzzles on the exits they open"
```

---

### Task 4: The walk opens a hidden puzzle exit

**Files:**
- Modify: `crates/mud-client/src/nav.rs` (`NavErrorKind`, `Display`, constants, `goto`'s call into `walk_step`, `walk_step`, new `open_puzzle`, `solve_puzzle`, `walk_to`, `speak`)
- Test: `crates/mud-client/tests/nav_puzzle.rs`

**Interfaces:**
- Consumes: `ExitRequirement::Puzzle(Puzzle)`, `Puzzle::plan`, `PuzzleAction::{room, phrases, reply}`, `crate::session::drain`, `Session::{send, events}`.
- Produces:
  - `NavErrorKind::Puzzle { dir: String, tried: String }`.
  - `walk_step` takes `requirement: &ExitRequirement` in place of `searchable_hidden: bool`, and `current: &mut RoomId` after `here`. Inner walks move `current`, so `NavError::at` is where the character really is.
  - private `const PUZZLE_TRIES: u32 = 2;` and `const SAID_ALOUD: &str = "you say \"";`.
  - private `async fn solve_puzzle(&self, session, current: &mut RoomId, dir: &str, puzzle: &Puzzle, guard, armed) -> Result<(), NavErrorKind>`, reused by Task 5.

- [ ] **Step 1: Write the failing tests**

Create `crates/mud-client/tests/nav_puzzle.rs`:

```rust
//! Buttons and levers: exits a phrase opens from here or from another
//! room.
//!
//! Live incident, 2026-09-04: a roam walked to 1/506 Secret Passage and
//! sent `search s` two hundred times. The south wall is a type 6 exit
//! with state 16, which no search roll clears, and the room's west slot
//! is a type 12 remote action: `push button`, action 1 on exit 1. The
//! Crypt pair captured 2026-09-05 is the two room shape: `pull lever` in
//! 1/1044 and 1/1038 each answer "You pull the lever. Off in the
//! distance you hear a small click." and 1/1056 then lists "dark
//! passageway north".
//!
//! The board here is a small world with a position, so an inner walk to
//! a lever room and back is a real walk: a direction the current room
//! lacks is refused, and the concealed exit is refused until the pulls
//! are in. The live board says an unknown phrase out loud rather than
//! rejecting it, and so does this one.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{Interrupt, NavConfig, NavErrorKind, Navigator, NoGuard, TravelGuard};
use mud_client::profile::Profile;
use mud_client::puzzle::{Puzzle, PuzzleAction};
use mud_client::session::Session;
use mud_core::content::{Direction, ItemId, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PASSAGE: RoomId = RoomId { map: 1, room: 506 };
const HALLWAY: RoomId = RoomId { map: 1, room: 507 };
const HALL: RoomId = RoomId { map: 1, room: 1056 };
const LEVER_A: RoomId = RoomId { map: 1, room: 1044 };
const LEVER_B: RoomId = RoomId { map: 1, room: 1038 };
const DARK_PASSAGE: RoomId = RoomId { map: 1, room: 1063 };

const PROMPT: &str = "\r\n[HP=51/MA=9]:";
const CLICK: &str = "You pull the lever. Off in the distance you hear a small click.";
const PUSHED: &str = "You push the button.";

/// One room as the scripted board knows it.
struct BoardRoom {
    name: &'static str,
    /// Plain exits: the command word, the word the exits line shows,
    /// and the room it leads to.
    exits: Vec<(&'static str, &'static str, &'static str)>,
    /// The concealed exit, refused until the pulls are in.
    concealed: Option<(&'static str, &'static str, &'static str)>,
    /// Whether a lever phrase does anything here. Elsewhere it is said
    /// aloud.
    lever: bool,
    /// A line printed before the lever reply, for the guard tests.
    ambush: Option<&'static str>,
}

struct World {
    rooms: BTreeMap<&'static str, BoardRoom>,
    /// Pulls needed before the concealed exit opens.
    pulls_needed: usize,
    /// What a lever phrase answers.
    reply: &'static str,
}

#[derive(Default)]
struct BoardLog {
    lines: Mutex<Vec<String>>,
    pulls: AtomicUsize,
}

impl BoardLog {
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    fn count(&self, line: &str) -> usize {
        self.lines().iter().filter(|l| l == line).count()
    }
}

fn render(world: &World, at: &str, opened: bool) -> String {
    let room = &world.rooms[at];
    let mut shown: Vec<&str> = room.exits.iter().map(|(_, shown, _)| *shown).collect();
    if opened {
        if let Some((_, label, _)) = room.concealed {
            shown.push(label);
        }
    }
    format!(
        "\r\n\x1b[1;36m{}\r\nObvious exits: {}{PROMPT}",
        room.name,
        shown.join(" ")
    )
}

async fn world_board(world: World, start: &'static str) -> (std::net::SocketAddr, Arc<BoardLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(BoardLog::default());
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut at = start;
        let opened = |seen: &BoardLog| seen.pulls.load(Ordering::SeqCst) >= world.pulls_needed;
        sock.write_all(render(&world, at, opened(&seen)).as_bytes())
            .await
            .unwrap();
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
                seen.lines.lock().unwrap().push(line.clone());
                let room = &world.rooms[at];
                let reply = if line == "look" {
                    render(&world, at, opened(&seen))
                } else if line == "pull lever" || line == "push button" {
                    if room.lever {
                        seen.pulls.fetch_add(1, Ordering::SeqCst);
                        match room.ambush {
                            Some(hit) => format!("\r\n{hit}\r\n{}{PROMPT}", world.reply),
                            None => format!("\r\n{}{PROMPT}", world.reply),
                        }
                    } else {
                        format!("\r\nYou say \"{line}\"{PROMPT}")
                    }
                } else if let Some(dest) = room
                    .exits
                    .iter()
                    .find(|(cmd, _, _)| *cmd == line)
                    .map(|(_, _, dest)| *dest)
                {
                    at = dest;
                    render(&world, at, opened(&seen))
                } else if let Some((_, _, dest)) = room.concealed.filter(|(cmd, _, _)| *cmd == line) {
                    if opened(&seen) {
                        at = dest;
                        render(&world, at, opened(&seen))
                    } else {
                        format!("\r\nThere is no exit in that direction!{PROMPT}")
                    }
                } else if line.starts_with("search ") {
                    format!("\r\nYou notice nothing different.{PROMPT}")
                } else if line.starts_with("open ") {
                    format!("\r\nThere is no door in that direction!{PROMPT}")
                } else if [
                    "n", "s", "e", "w", "ne", "nw", "se", "sw", "u", "d",
                ]
                .contains(&line.as_str())
                {
                    format!("\r\nThere is no exit in that direction!{PROMPT}")
                } else {
                    format!("\r\nYou say \"{line}\"{PROMPT}")
                };
                sock.write_all(format!("\r\n{line}{reply}").as_bytes())
                    .await
                    .unwrap();
            }
        }
    });
    (addr, log)
}

fn room(name: &str, exits: &[(Direction, RoomId)]) -> GraphRoom {
    let mut r = GraphRoom {
        name: name.into(),
        ..Default::default()
    };
    for (d, dest) in exits {
        r.exits[*d as usize] = Some(ExitEdge {
            dest: *dest,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
    }
    r
}

fn puzzle_edge(dest: RoomId, puzzle: Puzzle) -> ExitEdge {
    ExitEdge {
        dest,
        exit_type: 6,
        command: None,
        requirement: ExitRequirement::Puzzle(puzzle),
    }
}

fn button(item: Option<ItemId>) -> Puzzle {
    Puzzle {
        word: 16,
        actions: vec![PuzzleAction {
            room: PASSAGE,
            number: 1,
            phrases: vec!["push button".into()],
            item,
            reply: Some(PUSHED.into()),
            hops: Some(0),
        }],
    }
}

/// 1/506 and the hallway behind its south wall.
fn passage_graph(item: Option<ItemId>) -> Arc<RoomGraph> {
    let mut passage = room("Secret Passage", &[]);
    passage.exits[Direction::South as usize] = Some(puzzle_edge(HALLWAY, button(item)));
    Arc::new(RoomGraph::from_rooms(vec![
        (PASSAGE, passage),
        (HALLWAY, room("Wooden Hallway", &[(Direction::North, PASSAGE)])),
    ]))
}

fn passage_world(pulls_needed: usize, lever: bool) -> World {
    let mut rooms = BTreeMap::new();
    rooms.insert(
        "Secret Passage",
        BoardRoom {
            name: "Secret Passage",
            exits: Vec::new(),
            concealed: Some(("s", "south", "Wooden Hallway")),
            lever,
            ambush: None,
        },
    );
    rooms.insert(
        "Wooden Hallway",
        BoardRoom {
            name: "Wooden Hallway",
            exits: vec![("n", "north", "Secret Passage")],
            concealed: None,
            lever: false,
            ambush: None,
        },
    );
    World {
        rooms,
        pulls_needed,
        reply: PUSHED,
    }
}

/// The Crypt: the hallway, an alcove each side holding a lever, and the
/// passage north that both levers open.
fn crypt_graph() -> Arc<RoomGraph> {
    let lever = |room, number| PuzzleAction {
        room,
        number,
        phrases: vec!["pull lever".into()],
        item: None,
        reply: Some(CLICK.into()),
        hops: Some(1),
    };
    let mut hall = room(
        "Crypt, Stone Hallway",
        &[(Direction::East, LEVER_A), (Direction::West, LEVER_B)],
    );
    hall.exits[Direction::North as usize] = Some(puzzle_edge(
        DARK_PASSAGE,
        Puzzle {
            word: 48,
            actions: vec![lever(LEVER_B, 2), lever(LEVER_A, 1)],
        },
    ));
    Arc::new(RoomGraph::from_rooms(vec![
        (HALL, hall),
        (LEVER_A, room("Crypt, East Alcove", &[(Direction::West, HALL)])),
        (LEVER_B, room("Crypt, West Alcove", &[(Direction::East, HALL)])),
        (DARK_PASSAGE, room("Crypt, Dark Passage", &[(Direction::South, HALL)])),
    ]))
}

fn crypt_world(ambush_in_west: Option<&'static str>) -> World {
    let mut rooms = BTreeMap::new();
    rooms.insert(
        "Crypt, Stone Hallway",
        BoardRoom {
            name: "Crypt, Stone Hallway",
            exits: vec![("e", "east", "Crypt, East Alcove"), ("w", "west", "Crypt, West Alcove")],
            concealed: Some(("n", "dark passageway north", "Crypt, Dark Passage")),
            lever: false,
            ambush: None,
        },
    );
    rooms.insert(
        "Crypt, East Alcove",
        BoardRoom {
            name: "Crypt, East Alcove",
            exits: vec![("w", "west", "Crypt, Stone Hallway")],
            concealed: None,
            lever: true,
            ambush: None,
        },
    );
    rooms.insert(
        "Crypt, West Alcove",
        BoardRoom {
            name: "Crypt, West Alcove",
            exits: vec![("e", "east", "Crypt, Stone Hallway")],
            concealed: None,
            lever: true,
            ambush: ambush_in_west,
        },
    );
    rooms.insert(
        "Crypt, Dark Passage",
        BoardRoom {
            name: "Crypt, Dark Passage",
            exits: vec![("s", "south", "Crypt, Stone Hallway")],
            concealed: None,
            lever: false,
            ambush: None,
        },
    );
    World {
        rooms,
        pulls_needed: 2,
        reply: CLICK,
    }
}

async fn session_for(addr: std::net::SocketAddr) -> Session {
    let profile = Profile {
        target: mud_client::dialect::Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "testuser".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        disable_evil_warnings: false,
        bot: None,
        farm: None,
    };
    Session::connect(&profile, None).await.unwrap()
}

fn nav(graph: Arc<RoomGraph>) -> Navigator {
    Navigator::new(
        graph,
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
            ..NavConfig::default()
        },
    )
}

/// The bug, fixed: the south wall of 1/506 is refused, the button in
/// the same room is pushed, and the direction then lands. Not one
/// search is spent.
#[tokio::test]
async fn a_same_room_button_is_pushed_when_the_wall_refuses() {
    let (addr, log) = world_board(passage_world(1, true), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("the button opens the wall");

    assert_eq!(at.at, HALLWAY);
    assert_eq!(log.lines(), vec!["s", "push button", "s"]);
}

/// A passage somebody opened minutes ago simply walks. Sending the
/// direction first is what makes that free.
#[tokio::test]
async fn an_already_open_passage_is_walked_without_a_word() {
    let (addr, log) = world_board(passage_world(0, true), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("nothing was in the way");

    assert_eq!(at.at, HALLWAY);
    assert_eq!(log.lines(), vec!["s"]);
    assert_eq!(log.pulls.load(Ordering::SeqCst), 0);
}

/// The Crypt pair: the plan pulls action 2 first, so the walk goes west,
/// pulls, crosses to the east alcove, pulls, comes back to the hallway
/// and only then sends north again.
#[tokio::test]
async fn a_lever_pair_is_pulled_higher_number_first_and_the_walk_comes_back() {
    let (addr, log) = world_board(crypt_world(None), "Crypt, Stone Hallway").await;
    let session = session_for(addr).await;
    let n = nav(crypt_graph());

    let at = tokio::time::timeout(
        Duration::from_secs(30),
        n.goto(&session, HALL, DARK_PASSAGE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("two pulls open the passage");

    assert_eq!(at.at, DARK_PASSAGE);
    assert_eq!(
        log.lines(),
        vec!["n", "w", "pull lever", "e", "e", "pull lever", "w", "n"]
    );
    assert_eq!(log.pulls.load(Ordering::SeqCst), 2);
}

/// A wall that stays shut after a whole plan is tried once more, since
/// the re-hide can beat a long detour, and then reported as a puzzle
/// failure from the room the walk still stands in. Never a desync, and
/// never a search.
#[tokio::test]
async fn a_passage_that_stays_shut_is_reported_after_two_plans() {
    let (addr, log) = world_board(passage_world(usize::MAX, true), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the wall never opens");

    assert!(matches!(err.kind, NavErrorKind::Puzzle { .. }), "{err:?}");
    assert_eq!(err.at, PASSAGE);
    assert_eq!(log.count("push button"), 2, "{:?}", log.lines());
    assert_eq!(log.lines().iter().filter(|l| l.starts_with("search")).count(), 0);
}

/// The live board says an unknown phrase out loud. That is the only sign
/// the button did nothing, and no retry can change it, so the step ends
/// at once.
#[tokio::test]
async fn a_phrase_said_aloud_ends_the_step_at_once() {
    let (addr, log) = world_board(passage_world(1, false), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("speech opens nothing");

    assert!(matches!(err.kind, NavErrorKind::Puzzle { .. }), "{err:?}");
    assert_eq!(err.at, PASSAGE);
    assert_eq!(log.count("push button"), 1, "{:?}", log.lines());
}

/// Routing refuses a puzzle whose action needs an item the pack lacks,
/// so the walk never sets out.
#[tokio::test]
async fn a_plan_the_pack_cannot_fill_is_no_route() {
    let (addr, log) = world_board(passage_world(1, true), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(Some(ItemId(500))))
        .with_capabilities(Capabilities::unrestricted());

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("no fork, no plan");

    assert!(matches!(err.kind, NavErrorKind::NoRoute), "{err:?}");
    assert!(log.lines().is_empty(), "{:?}", log.lines());
}

/// An interrupt while the character stands in a lever room three rooms
/// from the step it set out on has to report the lever room. A caller
/// that resumes from the step's own room would be routing from a lie.
#[tokio::test]
async fn an_interrupt_in_the_lever_room_reports_the_lever_room() {
    struct ArmOnHit;
    impl TravelGuard for ArmOnHit {
        fn on_event(&mut self, ev: &mud_client::events::Event) -> Option<Interrupt> {
            match ev {
                mud_client::events::Event::CombatHit { .. } => {
                    Some(Interrupt::Attacked { by: "ghoul".into() })
                }
                _ => None,
            }
        }
    }

    let (addr, log) = world_board(
        crypt_world(Some("The crypt ghoul slashes you for 5 damage!")),
        "Crypt, Stone Hallway",
    )
    .await;
    let session = session_for(addr).await;
    let n = nav(crypt_graph());

    let err = tokio::time::timeout(
        Duration::from_secs(30),
        n.goto(&session, HALL, DARK_PASSAGE, &mut ArmOnHit, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the guard takes the walk back");

    assert!(
        matches!(err.kind, NavErrorKind::Interrupted(Interrupt::Attacked { .. })),
        "{err:?}"
    );
    assert_eq!(err.at, LEVER_B, "the walk stands in the west alcove");
    assert_eq!(log.lines(), vec!["n", "w", "pull lever"]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test nav_puzzle`
Expected: compile error, `NavErrorKind::Puzzle` does not exist. After adding only the variant they would fail on behaviour: the wall is refused, the walk looks, re-plans and desyncs.

- [ ] **Step 3: Add the error kind and the constants**

In `crates/mud-client/src/nav.rs`, add to `NavErrorKind` after `DoorLocked`:

```rust
    /// The exit is opened by a button or lever and the walk could not
    /// open it: the plan was performed and the way stayed shut, the
    /// phrase was said aloud instead of acted on, or the pack changed
    /// under the walk and there is no plan left.
    ///
    /// Its own variant for the same reason `DoorLocked` is: the walk
    /// knows exactly where it stands and what it tried, and a desync
    /// would send the caller off re-localizing a position that is not
    /// in doubt.
    Puzzle {
        /// The direction of the exit, as the walk words it.
        dir: String,
        /// What was tried, so the operator can act on it.
        tried: String,
    },
```

Add to the `Display` impl after the `DoorLocked` arm:

```rust
            NavErrorKind::Puzzle { dir, tried } => {
                write!(f, "the lever for {dir} did not open it: {tried}")
            }
```

Add after `const SEARCH_ROLLS: u32 = 100;`:

```rust
/// Whole plans performed for one puzzle step before the walk gives up
/// on it. Two, because the board re-hides a revealed passage 300
/// seconds after the last lever, and a long detour to a far lever can
/// lose that race once.
const PUZZLE_TRIES: u32 = 2;

/// The board saying a phrase out loud instead of acting on it, lowercased.
/// An unknown command is never an error on the live board, it is speech,
/// so this is the only sign a button phrase did nothing here.
const SAID_ALOUD: &str = "you say \"";
```

- [ ] **Step 4: Thread the requirement and the position into `walk_step`**

In `goto`, replace the `searchable_hidden` computation from Task 2 with:

```rust
                // What the exit needs of the walk, cloned off the graph
                // so the step can hold it across its own awaits. A
                // puzzle's plan is a handful of actions, cheap to copy.
                let requirement = edge
                    .map(|e| e.requirement.clone())
                    .unwrap_or_default();
```

In the call to `walk_step` in `goto`, replace the argument `searchable_hidden,` with `&requirement,` and add `&mut current,` immediately after `&here_name,`.

Change `walk_step`'s signature:

```rust
    async fn walk_step(
        &self,
        step: Direction,
        exit_type: i64,
        requirement: &crate::graph::ExitRequirement,
        sent: crate::correlate::CmdId,
        expected: &str,
        here: &str,
        current: &mut RoomId,
        session: &Session,
        events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
        sneak_seen: &mut SneakSeen,
    ) -> Result<StepOutcome, NavErrorKind> {
```

Add to its doc comment: `current` is the walk's position. A puzzle step walks to lever rooms and back with inner walks, and every one of them writes where it ended, so an error raised from a lever room names that room.

Replace the two `StepEvent::NoSuchExit` arms of the first `match` in `walk_step` with one:

```rust
            // "There is no exit in that direction!" is what a HIDDEN exit
            // says too, and it is the only thing it says. The graph is
            // the only witness that this wall is a passage, so it
            // decides: pull the lever for a puzzle, search a hidden
            // exit, re-localize everywhere else.
            StepEvent::NoSuchExit => {
                if let crate::graph::ExitRequirement::Puzzle(puzzle) = requirement {
                    return self
                        .open_puzzle(
                            step, puzzle, expected, here, current, session, events, guard, armed,
                            sneak_seen,
                        )
                        .await;
                }
                if self.search_hidden
                    && matches!(requirement, crate::graph::ExitRequirement::Hidden)
                {
                    return self
                        .find_hidden(step, expected, here, session, events, guard, armed, sneak_seen)
                        .await;
                }
                let ask = session.send("look");
                return self
                    .arrival(here, expected, BlindContext::AfterLook, events, guard, armed, ask, sneak_seen)
                    .await
                    .map(StepOutcome::StayedPut);
            }
```

- [ ] **Step 5: Write `open_puzzle`, `solve_puzzle`, `walk_to` and `speak`**

Add after `find_hidden`:

```rust
    /// Open a puzzle exit the board has just denied, then walk it.
    ///
    /// Reactive for the same reason [`Navigator::find_hidden`] is: a
    /// passage somebody opened minutes ago simply walks, and the
    /// refusal is the one signal that it has not been. Each attempt
    /// performs the whole plan, comes back, and sends the direction
    /// again. A second refusal after a complete plan is tried once
    /// more, because the 300 second re-hide can beat a long detour,
    /// and then reported as a puzzle failure rather than a desync: the
    /// walk knows exactly where it stands.
    #[allow(clippy::too_many_arguments)]
    async fn open_puzzle(
        &self,
        step: Direction,
        puzzle: &crate::puzzle::Puzzle,
        expected: &str,
        here: &str,
        current: &mut RoomId,
        session: &Session,
        events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
        sneak_seen: &mut SneakSeen,
    ) -> Result<StepOutcome, NavErrorKind> {
        let dir = dir_word(step);
        for _ in 0..PUZZLE_TRIES {
            self.solve_puzzle(session, current, dir, puzzle, guard, armed)
                .await?;
            // Everything the inner walks produced is still queued on
            // this receiver, already shown to the guard by those walks.
            // Drop it so the resent step is answered by its own lines.
            crate::session::drain(events, |_| {});
            let again = session.send(dir);
            match self.wait_room(events, guard, armed, again, sneak_seen).await? {
                StepEvent::Arrived(room) => {
                    return Ok(StepOutcome::Arrived(Sighting::Block(room)));
                }
                StepEvent::Blind => {
                    return Ok(StepOutcome::Arrived(Sighting::Dark(
                        Navigator::blind_position(BlindContext::AfterMove, expected, here)
                            .to_string(),
                    )));
                }
                StepEvent::CombatBlocked => {
                    return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                        by: "combat".into(),
                    }));
                }
                // Still shut, or a wording a plain step has no business
                // producing: spend an attempt on it.
                _ => {}
            }
        }
        Err(NavErrorKind::Puzzle {
            dir: dir.to_string(),
            tried: format!("performed the plan {PUZZLE_TRIES} times and the way stayed shut"),
        })
    }

    /// Perform the plan for one puzzle exit, starting from the room the
    /// exit is in, and come back to it.
    ///
    /// `current` is the walk's position and this moves it: every inner
    /// walk writes where it ended, on success and on failure alike, so
    /// an interrupt in a lever room three rooms away reports that room
    /// and not the one the step set out from. A caller that resumes
    /// from `NavError::at` is then resuming from somewhere true.
    ///
    /// The guard is heard after every phrase, the way it is between
    /// bash rolls: a plan can be a long way round, and a character
    /// pulling levers is a character standing still while something
    /// swings at it.
    ///
    /// The inner walks start with no sneak belief and this never reads
    /// what they learned. The resent step's own lines settle the belief
    /// the moment it lands, the same correction every step gets.
    async fn solve_puzzle(
        &self,
        session: &Session,
        current: &mut RoomId,
        dir: &str,
        puzzle: &crate::puzzle::Puzzle,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<(), NavErrorKind> {
        let Some(plan) = puzzle.plan(&self.capabilities) else {
            // Routing refuses an edge with no plan, so reaching one
            // means the pack changed under the walk. Say so rather than
            // re-localize: the position is not in doubt.
            return Err(NavErrorKind::Puzzle {
                dir: dir.to_string(),
                tried: "no plan: an action needs an item the pack lacks".into(),
            });
        };
        let home = *current;
        for action in plan {
            self.walk_to(session, current, action.room, guard).await?;
            self.speak(session, action, dir, guard, armed).await?;
            if let Some(interrupt) = armed.take() {
                return Err(NavErrorKind::Interrupted(interrupt));
            }
        }
        self.walk_to(session, current, home, guard).await
    }

    /// An inner walk on behalf of a puzzle step. Boxed because it is
    /// `goto` calling itself: the lever room may sit behind a puzzle of
    /// its own.
    async fn walk_to(
        &self,
        session: &Session,
        current: &mut RoomId,
        to: RoomId,
        guard: &mut impl TravelGuard,
    ) -> Result<(), NavErrorKind> {
        if *current == to {
            return Ok(());
        }
        match Box::pin(self.goto(session, *current, to, guard, false)).await {
            Ok(arrival) => {
                *current = arrival.at;
                Ok(())
            }
            Err(err) => {
                *current = err.at;
                Err(err.kind)
            }
        }
    }

    /// Speak one action's phrase in the room it belongs to and read the
    /// answer.
    ///
    /// A fresh receiver, subscribed before the send, so nothing an inner
    /// walk left behind can be mistaken for the reply. The reply body is
    /// unattributed, `kind_of` files a phrase as `Opaque`, so the read
    /// is [`Navigator::arm_sneak`]'s: skip the echo, then classify what
    /// arrives before the next prompt. The slot's own reply line ends
    /// the read early. The board saying the phrase out loud means it
    /// was not a command here, and no retry can change that.
    async fn speak(
        &self,
        session: &Session,
        action: &crate::puzzle::PuzzleAction,
        dir: &str,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<(), NavErrorKind> {
        let phrase = action.phrases.first().ok_or_else(|| NavErrorKind::Puzzle {
            dir: dir.to_string(),
            tried: "the slot has no phrase".into(),
        })?;
        let mut events = session.events();
        let id = session.send(phrase);
        let spoken = phrase.to_lowercase();
        let reply = action.reply.as_deref().map(str::to_lowercase);
        let mut echoed = false;
        let deadline = tokio::time::Instant::now() + self.step_timeout;
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
                Err(_) => {
                    return Err(NavErrorKind::Expect(ExpectError::Timeout {
                        needle: format!("reply to {phrase}"),
                        tail: String::new(),
                    }));
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => {
                    return Err(NavErrorKind::Expect(ExpectError::Closed { tail: String::new() }));
                }
                Ok(Ok(cor)) => cor,
            };
            match &cor.event {
                crate::events::Event::Line(line) => {
                    if cor.answers == Some(id) {
                        echoed = true;
                        continue;
                    }
                    let line = line.to_lowercase();
                    if line.starts_with(SAID_ALOUD) && line.contains(&spoken) {
                        return Err(NavErrorKind::Puzzle {
                            dir: dir.to_string(),
                            tried: format!("{phrase:?} was said aloud, not acted on"),
                        });
                    }
                    if reply.as_deref().is_some_and(|r| line.contains(r)) {
                        return Ok(());
                    }
                }
                crate::events::Event::Prompt { .. } if echoed => return Ok(()),
                _ => {}
            }
        }
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test nav_puzzle --test nav_hidden --test nav_doors --test nav_command_exit`
Expected: all pass. If `a_lever_pair_is_pulled_higher_number_first_and_the_walk_comes_back` records an extra `look`, an inner walk mis-stepped: check the board's exit tables against the graph before touching the navigator.

- [ ] **Step 7: Run the whole suite and commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: every line `0 failed`.

```bash
git add crates/mud-client/src/nav.rs crates/mud-client/tests/nav_puzzle.rs
git commit -m "feat(client): the walk pulls the levers a passage needs and comes back"
```

---

### Task 5: A lever on a gate

**Files:**
- Modify: `crates/mud-client/src/nav.rs` (`walk_step`, between the `open` match and the picking loop)
- Test: `crates/mud-client/tests/nav_puzzle.rs`

**Interfaces:**
- Consumes: `solve_puzzle` from Task 4.
- Produces: a locked gate whose requirement is `Puzzle` gets its lever pulled before any pick or bash.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/nav_puzzle.rs`:

```rust
const GATE_ROOM: RoomId = RoomId { map: 1, room: 20 };
const COURTYARD: RoomId = RoomId { map: 1, room: 21 };

/// A lever on a gate toggles its lock, so the walk pulls it after `open`
/// says locked and before any pick or bash is spent, then opens and
/// walks. The board here is locked until the lever is pulled once.
#[tokio::test]
async fn a_lever_on_a_gate_unlocks_it_before_any_pick_or_bash() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(BoardLog::default());
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let block = |name: &str, exits: &str| {
            format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}{PROMPT}")
        };
        sock.write_all(block("Gate Room", "closed gate north").as_bytes())
            .await
            .unwrap();
        let mut open = false;
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
                seen.lines.lock().unwrap().push(line.clone());
                let unlocked = seen.pulls.load(Ordering::SeqCst) >= 1;
                let reply = match line.as_str() {
                    "n" if open => block("Courtyard", "south"),
                    "n" => format!("\r\nThe gate is closed!{PROMPT}"),
                    "open n" | "open north" if unlocked => {
                        open = true;
                        format!("\r\nThe gate is now open.{PROMPT}")
                    }
                    "open n" | "open north" => format!("\r\nThe gate is locked.{PROMPT}"),
                    "pull lever" => {
                        seen.pulls.fetch_add(1, Ordering::SeqCst);
                        format!("\r\n{CLICK}{PROMPT}")
                    }
                    "look" => block("Gate Room", "closed gate north"),
                    other => format!("\r\nYou say \"{other}\"{PROMPT}"),
                };
                sock.write_all(format!("\r\n{line}{reply}").as_bytes())
                    .await
                    .unwrap();
            }
        }
    });

    let mut gate_room = room("Gate Room", &[]);
    gate_room.exits[Direction::North as usize] = Some(ExitEdge {
        dest: COURTYARD,
        exit_type: 0xb,
        command: None,
        requirement: ExitRequirement::Puzzle(Puzzle {
            word: 0,
            actions: vec![PuzzleAction {
                room: GATE_ROOM,
                number: 0,
                phrases: vec!["pull lever".into()],
                item: None,
                reply: Some(CLICK.into()),
                hops: Some(0),
            }],
        }),
    });
    let graph = Arc::new(RoomGraph::from_rooms(vec![
        (GATE_ROOM, gate_room),
        (COURTYARD, room("Courtyard", &[(Direction::South, GATE_ROOM)])),
    ]));
    let session = session_for(addr).await;
    let n = nav(graph);

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, GATE_ROOM, COURTYARD, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("the lever unlocks the gate");

    assert_eq!(at.at, COURTYARD);
    assert_eq!(log.lines(), vec!["n", "open n", "pull lever", "open n", "n"]);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mud-client --test nav_puzzle a_lever_on_a_gate`
Expected: FAIL. The navigator built by `nav` cannot pick, `Capabilities::unrestricted` can, so after `open n` says locked the walk sends `picklock n`, the board says it aloud, and the log has picks in it before any lever.

- [ ] **Step 3: Add the gate stage**

In `walk_step`, immediately after the second `match` block ends, the one that waits on `opened`, and before the comment that begins `// Picking first:`, insert:

```rust
        // A lever on a gate toggles its lock, mud-core's
        // `remote_gate_toggle`, so a locked gate the graph calls a
        // puzzle gets its lever pulled before any pick or bash is spent
        // on it. The gate is still shut afterwards and wants the same
        // `open` a picked lock does.
        if let crate::graph::ExitRequirement::Puzzle(puzzle) = requirement {
            self.solve_puzzle(session, current, dir, puzzle, guard, armed)
                .await?;
            crate::session::drain(events, |_| {});
            let opened = session.send(&format!("open {dir}"));
            match self.wait_room(events, guard, armed, opened, sneak_seen).await? {
                StepEvent::DoorYielded => {
                    let again = session.send(dir);
                    return self
                        .arrival(here, expected, BlindContext::AfterMove, events, guard, armed, again, sneak_seen)
                        .await
                        .map(StepOutcome::Arrived);
                }
                StepEvent::CombatBlocked => {
                    return Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                        by: "combat".into(),
                    }));
                }
                // Still locked after the lever: the lock is the story
                // again, and picking and bashing below get their turn.
                _ => {}
            }
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test nav_puzzle --test nav_doors`
Expected: all pass.

- [ ] **Step 5: Run the whole suite and commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: every line `0 failed`.

```bash
git add crates/mud-client/src/nav.rs crates/mud-client/tests/nav_puzzle.rs
git commit -m "feat(client): a locked gate with a lever gets the lever before a pick"
```

---

### Task 6: Roams grow through the puzzles the character can solve

**Files:**
- Modify: `crates/mud-client/src/graph.rs` (`distances_within`, new `distances_within_for`)
- Modify: `crates/mud-client/src/roam.rs` (`region`, `Rotation::next`, module and `passable` docs)
- Modify: `crates/mud-client/src/farm.rs:256-275` (`FarmPlan::roaming`) and `crates/mud-client/src/farm.rs:1818-1852` (the roam setup and the rotation call in `run_farm`)
- Test: `crates/mud-client/tests/roam.rs`

**Interfaces:**
- Consumes: `Session::capabilities()`, `exit_cost_for` over `Puzzle` from Task 2.
- Produces:
  - `RoomGraph::distances_within_for(&self, from: RoomId, allow: &dyn Fn(Direction, &ExitEdge) -> bool, caps: &Capabilities) -> BTreeMap<RoomId, usize>`.
  - `roam::region(graph: &RoomGraph, from: RoomId, walls: &Walls, caps: &Capabilities) -> BTreeSet<RoomId>`.
  - `Rotation::next(&self, graph, from, region, walls, caps: &Capabilities) -> Option<RoomId>`.
  - `FarmPlan::roaming(start, walls, graph)` keeps its signature and no longer computes a region.

- [ ] **Step 1: Write the failing tests**

Create `crates/mud-client/tests/roam.rs`:

```rust
//! The region a roam covers grows through the puzzles this character
//! can solve and stops at the ones it cannot.
//!
//! Before this the flood ran as an unrestricted walker, so a puzzle
//! whose lever wanted a titanium fork was inside every roam and the
//! rotation could pick a room the walk then could not reach.

use std::collections::BTreeSet;
use std::sync::Arc;

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::pack::PackHandle;
use mud_client::puzzle::{Puzzle, PuzzleAction};
use mud_client::roam::{Rotation, Walls, region};
use mud_client::sheet::Inventory;
use mud_core::content::{Content, Direction, Item, ItemId, RoomId};

const POST: RoomId = RoomId { map: 1, room: 1 };
const VAULT: RoomId = RoomId { map: 1, room: 2 };
const FORK: ItemId = ItemId(500);

fn room(name: &str, exits: &[(Direction, RoomId)]) -> GraphRoom {
    let mut r = GraphRoom {
        name: name.into(),
        ..Default::default()
    };
    for (d, dest) in exits {
        r.exits[*d as usize] = Some(ExitEdge {
            dest: *dest,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
    }
    r
}

/// A guard post whose north wall is a puzzle opened by a button in the
/// post itself, needing `item` if any.
fn world(item: Option<ItemId>) -> RoomGraph {
    let mut post = room("Guard Post", &[]);
    post.exits[Direction::North as usize] = Some(ExitEdge {
        dest: VAULT,
        exit_type: 6,
        command: None,
        requirement: ExitRequirement::Puzzle(Puzzle {
            word: 16,
            actions: vec![PuzzleAction {
                room: POST,
                number: 1,
                phrases: vec!["push button".into()],
                item,
                reply: None,
                hops: Some(0),
            }],
        }),
    });
    RoomGraph::from_rooms(vec![
        (POST, post),
        (VAULT, room("Vault", &[(Direction::South, POST)])),
    ])
}

fn with_fork() -> Capabilities {
    let mut content = Content::default();
    content.add_item(Item {
        id: FORK,
        name: "titanium fork".into(),
        ..Default::default()
    });
    let pack = PackHandle::new(Arc::new(content));
    pack.refresh(&Inventory {
        items: vec!["titanium fork".into()],
        keys: Vec::new(),
        encumbrance: None,
    });
    Capabilities {
        pack: Some(pack),
        ..Capabilities::unrestricted()
    }
}

fn empty_handed() -> Capabilities {
    Capabilities::unrestricted()
}

#[test]
fn a_region_grows_through_a_solvable_puzzle() {
    let g = world(None);
    let got = region(&g, POST, &Walls::default(), &empty_handed());
    assert_eq!(got, BTreeSet::from([POST, VAULT]));
}

#[test]
fn a_region_stops_at_a_puzzle_the_pack_cannot_solve() {
    let g = world(Some(FORK));
    assert_eq!(
        region(&g, POST, &Walls::default(), &empty_handed()),
        BTreeSet::from([POST]),
        "no fork, no vault"
    );
    assert_eq!(
        region(&g, POST, &Walls::default(), &with_fork()),
        BTreeSet::from([POST, VAULT]),
        "the fork opens the vault"
    );
}

/// The rotation asks the same question the flood did. A region seeded
/// by a walker with the fork, handed to a rotation without it, never
/// sends the character at the vault.
#[test]
fn the_rotation_never_picks_a_room_it_cannot_reach() {
    let g = world(Some(FORK));
    let both = region(&g, POST, &Walls::default(), &with_fork());
    let rotation = Rotation::new();
    assert_eq!(
        rotation.next(&g, POST, &both, &Walls::default(), &with_fork()),
        Some(VAULT)
    );
    assert_eq!(
        rotation.next(&g, POST, &both, &Walls::default(), &empty_handed()),
        None
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test roam`
Expected: compile error, `region` takes 3 arguments and `next` takes 4.

- [ ] **Step 3: Thread the capabilities through**

In `crates/mud-client/src/graph.rs`, replace `distances_within` with:

```rust
    /// As [`RoomGraph::distances`], over the edges `allow` accepts.
    pub fn distances_within(
        &self,
        from: RoomId,
        allow: &dyn Fn(Direction, &ExitEdge) -> bool,
    ) -> BTreeMap<RoomId, usize> {
        self.distances_within_for(from, allow, &Capabilities::unrestricted())
    }

    /// As [`RoomGraph::distances_within`], for a walker with these
    /// capabilities. An edge the walker cannot open is not counted, so
    /// a room behind it is absent unless another way reaches it. A roam
    /// floods its region with this, since a region containing a room
    /// the walk then cannot reach would send the character at a wall.
    pub fn distances_within_for(
        &self,
        from: RoomId,
        allow: &dyn Fn(Direction, &ExitEdge) -> bool,
        caps: &Capabilities,
    ) -> BTreeMap<RoomId, usize> {
        self.explore(from, None, allow, caps)
            .into_iter()
            .map(|(id, reached)| (id, reached.hops))
            .collect()
    }
```

In `crates/mud-client/src/roam.rs`, add `use crate::graph::Capabilities;` alongside the existing graph import, then:

```rust
/// Every room the character can reach from `from` without crossing a
/// wall, leaving the plane, or meeting an exit it cannot open.
///
/// `caps` is the character's own: a button that wants an item the pack
/// lacks is a wall for this character and the room behind it is not in
/// the region. Always contains `from`, so an empty answer is impossible
/// and callers need no special case for one. A region of exactly one
/// room IS possible, walls on every side, and is a legitimate answer
/// meaning "you fenced yourself in", not a failure.
pub fn region(graph: &RoomGraph, from: RoomId, walls: &Walls, caps: &Capabilities) -> BTreeSet<RoomId> {
    if graph.room(from).is_none() {
        return BTreeSet::new();
    }
    let allow = passable(from.map, walls);
    graph.distances_within_for(from, &allow, caps).into_keys().collect()
}
```

and `Rotation::next`:

```rust
    /// Where to go next from `from`, within `region`, for a walker with
    /// `caps`. Same capabilities as the flood that built the region, or
    /// a room the flood counted could be one this walker cannot reach.
    ///
    /// `None` means there is nowhere else to be: the region is the one
    /// room the character already stands in. The caller should keep
    /// working that room rather than treat it as an ending, since a
    /// fenced-in single room is a vigil, which is a coherent thing to
    /// ask for.
    pub fn next(
        &self,
        graph: &RoomGraph,
        from: RoomId,
        region: &BTreeSet<RoomId>,
        walls: &Walls,
        caps: &Capabilities,
    ) -> Option<RoomId> {
        let allow = passable(from.map, walls);
        let hops = graph.distances_within_for(from, &allow, caps);
```

with the rest of the body unchanged.

In the `passable` doc comment, add one paragraph before `Revisit when there is something better than force to offer a lock.`:

```rust
/// **Buttons and levers are inside a roam.** A puzzle exit is costed,
/// not refused: the region floods with the character's own
/// capabilities, so a passage this character can open is in and one
/// that wants an item the pack lacks is out. A lever room outside the
/// fence is the one case the flood cannot see, the walk then fails the
/// leg with a puzzle error and the rotation moves on.
```

In `crates/mud-client/src/farm.rs`, `FarmPlan::roaming` drops the region computation. Replace its body from `let region = ...` to the end with:

```rust
        Ok(FarmPlan {
            start,
            circuit: Vec::new(),
            finish: None,
            roam: Some(walls),
        })
```

and change its doc comment's last sentence: A region always contains its start room, so there is no empty region to refuse. The region itself is flooded by the run, from where the character actually stands and with what it actually carries.

In `run_farm`, the roam setup becomes:

```rust
    let mut roam = plan.roam.as_ref().map(|walls| {
        let region = crate::roam::region(&graph, current, walls, &session.capabilities());
        (walls, region, crate::roam::Rotation::new())
    });
```

and the rotation call becomes:

```rust
                vec![
                    rotation
                        .next(&graph, current, region, walls, &session.capabilities())
                        .unwrap_or(current),
                ]
```

Extend the comment above the roam setup with: Flooded with the character's own capabilities, the same ones the fenced navigator routes with, so the region and the walk agree about which puzzles are open to it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test roam --test farm_scripted --test nav`
Expected: all pass.

- [ ] **Step 5: Run the whole suite and commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: every line `0 failed`.

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/src/roam.rs crates/mud-client/src/farm.rs crates/mud-client/tests/roam.rs
git commit -m "feat(client): a roam floods its region with the character's own capabilities"
```

---

### Task 7: Documentation

**Files:**
- Modify: `docs/mud-client.md` (a new section after `## Doors`, one paragraph in the Roaming section)

- [ ] **Step 1: Add the Buttons and levers section**

Insert after the `## Doors` section, before `## Fighting on the way`:

```markdown
## Buttons and levers

Some passages open on a phrase. A room's type 12 exit slot is not an
exit but a remote action: `push button` in 1/506 clears the concealment
bit on that room's own south wall, and `pull lever` in 1/1044 and 1/1038
together open 1/1056 north. 287 slots ship. 200 act on their own room,
the rest on another, and 86 want an item carried, 79 of them a titanium
fork. No search ever reveals one of these exits, which is how a roam
came to send `search s` two hundred times at 1/506 on 2026-09-04.

The graph decodes every slot at load and attaches it to the exit it
opens. The slot itself is never walked. Routing prices the exit by its
plan: the exit, then a phrase and a round trip per lever. An exit whose
plan needs an item the pack lacks, or a lever room no walk reaches, is
not priced at all, it is refused, and the router finds another way or
says `no route`.

The walk sends the direction first. If the passage is already open it
simply walks. On *"There is no exit in that direction!"* it performs the
plan: walk to each lever room in turn, speak the phrase, wait for the
reply or the next prompt, and walk back. Levers pull in descending
number order, which is the order an ordered puzzle needs and any order
is fine for the rest. Then the direction goes out again. A wall that
stays shut after a whole plan is tried once more, because the board
re-hides a passage 300 seconds after the last lever and a long detour
can lose that race, and then reported as a puzzle failure. The board
says an unknown phrase out loud, *"You say "push button""*, and the walk
reads that as the phrase doing nothing here and stops at once.

A lever on a gate toggles its lock instead. The walk finds that out the
ordinary way: `open n` answers *"The gate is locked."*, the lever is
pulled, and `open n` is sent again before any pick or bash.

An interrupt while the character is off pulling a lever reports the
lever room, not the room the step set out from. The farm resumes from
where the character actually is.
```

- [ ] **Step 2: Amend the Roaming section**

After the paragraph that begins `A roam always has somewhere else to be, so skipping a door costs nothing`, insert:

```markdown
**Buttons and levers are inside a roam.** The area is flooded with the
character's own capabilities, so a passage this character can open is
part of the area and one whose button wants an item the pack lacks is
not. A lever room outside your walls is the one thing the flood cannot
see: the walk to it fails as `no route`, the leg ends with a puzzle
error, and the rotation moves on.
```

- [ ] **Step 3: Commit**

```bash
git add docs/mud-client.md
git commit -m "doc(client): buttons and levers"
```

---

## Self-review

**Spec coverage, Part 2.**
- The graph: type 12 slots decoded with para1, para2, para3, para4 and attached to the target exit, Task 3. cmdtext `remoteaction` lines feed the same map, Task 3. The slot never becomes an edge, Task 3. `Hidden { searchable }` collapses to `Hidden`, Task 2. The raw `exit_type` stays on the edge, unchanged.
- The plan: bits of `word & 0x3ff0`, action 0 shortcut, every needed bit or `None`, descending order, Task 1. The `ordered` flag is dropped by ruling.
- Cost: `Impassable` on no plan, one step plus per action a phrase and `2 * hops`, precomputed at load with a breadth first search, unreachable action room is `Impassable`, the 40 step search price unchanged, Tasks 1 to 3. Per action hops by ruling.
- The walk: direction first, plan, inner `goto` per action room boxed for recursion, phrase and reply, back to the exit's room, direction again, one retry, puzzle error, Task 4. Door and gate targets end in the door flow, Task 5. Interrupts propagate from inner walks with the true position, Task 4.
- Roams: `passable` already allows puzzle edges, the flood and the rotation take the real capabilities, doors stay outside, Task 6.
- Testing list: `puzzle.rs` plans over the shipped word shapes, `graph.rs` slot and cmdtext fixtures and the untargeted type 6, `exit_cost_for` with and without the pack, `nav` scripted boards for a same room button, a two room lever pair, a plan that fails after the phrase, a puzzle found already open, and `roam` regions with and without the item. All present. The `bot` and `pickable` items in the spec's list belong to phases 1 and 3.

**Placeholder scan.** Every code step carries its code. No task refers to another for its content.

**Type consistency.** `PuzzleAction { room, number, phrases, item, reply, hops }` is used with those six fields in Tasks 1 through 6. `Puzzle { word, actions }` throughout. `Puzzle::plan(&Capabilities) -> Option<Vec<&PuzzleAction>>` and `Puzzle::cost(u32, &Capabilities) -> Cost` in Tasks 1 to 3. `solve_puzzle(session, current, dir, puzzle, guard, armed)` in Tasks 4 and 5. `region(graph, from, walls, caps)` and `next(graph, from, region, walls, caps)` in Task 6. `walk_step`'s new parameters `requirement: &ExitRequirement` and `current: &mut RoomId` in Tasks 4 and 5.
