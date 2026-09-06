# Keys and item doors (phase 3 of puzzles, keys and the pack) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Routing knows in advance which locked doors, key doors and item gates this character can get through, the walk opens a key door with the key on the ring, and a walk never spends a pick roll on a lock the formula says cannot give.

**Architecture:** The graph reads the lock fields the room record already carries and turns them into three requirements: `Door` with its lock state and pick modifier, `KeyDoor` with the key item and modifier, and `ItemGate` with the item. One pure function, `pickable`, answers whether a character can pick a modifier at all, and one price function turns the modifier into expected rolls. Routing prices a key door by the pack, a lock by the skill and the bashing switch, and an item gate by the pack. The walk gains one stage at a refused `open`: with the key on the ring it sends `use <key> <direction>`, reads the unlocked line the pick flow already knows, re-reads the pack because keys are spent, and opens.

**Tech Stack:** Rust workspace, tokio, `mud_core::content` for rooms and items, scripted TCP boards in tests.

**Spec:** `docs/superpowers/specs/2026-09-04-puzzles-keys-and-pack-design.md`, Part 3 and the "Key doors, item gates and the pick formula" section.

## Global Constraints

- No test reads `re/`. Graph fixtures are built with `RoomGraph::from_rooms` or `Content::default()` plus `add_room`, `add_item`.
- Never run rustfmt or cargo fmt.
- Commit after every task with a tag: `feat:`, `fix:`, `doc:`, `refactor:`. No references to plans or specs in commit messages. No attribution trailers of any kind in commit messages.
- Every file ends with exactly one trailing newline.
- Punctuation in new prose and comments: periods, commas, question marks, a colon before a list. No em dashes, parentheses, or semicolons in new prose. Existing prose is left as it is.
- Run the full crate suite before every commit: `cargo test -p mud-client 2>&1 | grep -E "^test result|FAILED|panicked"`. Every `test result:` line must say `0 failed`. Do not trust a `tail` of the output, the suite has over 50 binaries.
- The pick roll is `theft.md` sections 8.2 and 8.3, mirrored in `crates/mud-core/src/game.rs` `picklock_command`: the skill must be at least 1 and the roll is `genrdn(0,100) < modifier + skill`. So a lock is pickable at all only when `Picklocks >= 1 && modifier + Picklocks > 0`.
- The lock fields, from mud-core's `exit_lock_state` and `picklock_command`: type 2 keeps the key item id in `para1`, the lock state in `para2` and the pick modifier in `para3`. Types 7 and 0xb keep the lock state in `para1`, 2 meaning locked, and the pick modifier in `para2`. Type 3 keeps the required item id in `para1`.
- Live captures pin these strings: `use black star key east` answers `You successfully unlocked the door.`, the same line a pick gives, and the door then still wants `open e`, which answers `The door is now open.` The key ring line reads `You have the following keys:  black star key.` and `You have no keys.` when empty. `unlock` is not a command.
- Shipped door prices stay what they are: a door or gate is 5 steps, a searchable hidden exit 40. Those two numbers are `exit_cost(7)` and `exit_cost(6)` and the new pricing reads them from there.

## Rulings made while planning

- `Door.locked` is the shipped boot state, `para1 == 2`. A lock with a positive modifier never re-locks once picked, mud-core's `picklock_command` says so, so the Newhaven Arena door the data calls locked opened live to a plain `open`. The walk stays reactive and tries `open` first regardless. 416 of the 449 shipped type 7 doors are locked in the data, so this ruling decides most of the world's door prices.
- Routing needs to know whether bashing is on, because the spec prices a locked door nobody can pick as a wall when it is off. `Capabilities` gains `bash_doors`. The navigator sets it from its own `NavConfig.bash_doors` in `new`, `with_capabilities` and `fenced`, so routing and the walk cannot disagree. The session leaves it off: it has no config, and a roam, the only thing that routes with the session's own capabilities, never crosses a door.
- The reply to `use <key> <direction>` is read unattributed. `kind_of` in `correlate.rs` files `use` as opaque and a kind that never completes leaves an entry lingering to claim somebody else's line. `speak`, which already reads an opaque phrase's reply by skipping the echo and classifying lines up to the next prompt, is generalised into `ask` and both callers use it. The correlator learns no new verb.
- A key door with no key that the character cannot pick is `Impassable` even with bashing on. That is the spec's rule for `KeyDoor`, and the bashing exception applies to `Door` alone.
- A pickable lock is priced at the door plus the expected rolls whether or not bashing is on. The walk picks first when it can, so the rolls are what it will spend.
- 7 shipped item gates carry item id 0. They want nothing and read as `ExitRequirement::None`. No shipped key door carries key id 0, and one that did would read as a plain locked `Door` with the type 2 modifier.
- `can_pick` takes the lock's modifier. A lever gate lost its modifier to the puzzle and reads 0, which keeps today's rule for it: any Picklocks at all is worth a roll.
- Item gates fall inside a roam for free. `roam::passable` never excluded type 3 and the region flood already prices with the character's capabilities, so a gate whose item the pack lacks is now a wall for the region and one it holds is a step. One test pins it.

## File structure

| file | responsibility |
|---|---|
| `crates/mud-client/src/graph.rs` | `ExitRequirement::Door { locked, pick }`, `KeyDoor { key, pick }`, `ItemGate { item }`, `from_exit_type` reads three params, `pickable`, `pick_rolls`, `exit_cost_for` arms, `Capabilities.bash_doors`. |
| `crates/mud-client/src/nav.rs` | `Answer`, `ask`, `speak` over `ask`, `pick_modifier`, `can_pick(modifier)`, `key_name`, the key stage in `walk_step`, `bash_doors` into the capabilities. |
| `crates/mud-client/src/session.rs` | `capabilities()` sets `bash_doors: false`. |
| `crates/mud-client/tests/graph_content.rs` | the three requirements decoded from a fixture room. |
| `crates/mud-client/tests/graph.rs` | `pickable`, `pick_rolls`, the new prices. |
| `crates/mud-client/tests/nav_keys.rs` (new) | scripted boards: the key verb, the pack re-read, an unpickable lock spends no pick, a key door nobody can open is no route, a key that does nothing. |
| `crates/mud-client/tests/roam.rs` | a region and an item gate. |
| `crates/mud-client/tests/sneak.rs`, `nav_echo.rs`, `nav_hidden.rs`, `nav_doors.rs`, `backstab_opener.rs`, `nav_command_exit.rs` | the `from_exit_type` signature. |
| `docs/mud-client.md` | the Doors section, a Keys and item gates section, the roam paragraph, the `bash_doors` bullet. |

---

### Task 1: The three requirements in the graph

**Files:**
- Modify: `crates/mud-client/src/graph.rs:42-113` (the enum and `from_exit_type`), `crates/mud-client/src/graph.rs:550` (the loader call)
- Modify: `crates/mud-client/tests/sneak.rs`, `tests/nav_echo.rs`, `tests/graph.rs`, `tests/nav_hidden.rs`, `tests/nav_doors.rs`, `tests/backstab_opener.rs`, `tests/nav_command_exit.rs` (call sites)
- Test: `crates/mud-client/tests/graph_content.rs`

**Interfaces:**
- Consumes: `mud_core::content::{Exit, ItemId, Room}`, `RoomGraph::from_content`.
- Produces: `ExitRequirement::Door { locked: bool, pick: i32 }`, `ExitRequirement::KeyDoor { key: ItemId, pick: i32 }`, `ExitRequirement::ItemGate { item: ItemId }`, and `ExitRequirement::from_exit_type(exit_type: i64, para1: i64, para2: i64, para3: i64) -> ExitRequirement`.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/graph_content.rs`:

```rust
/// The lock fields the room record already carries become
/// requirements. The Black House door on Slum Street, 1/1224 east, is
/// the captured key door: type 2, key item 172, pick modifier -99. The
/// 88-bash door at 1/1119 north is a locked type 7 with modifier -30.
/// A type 3 exit wants an item carried, and one whose item id is 0
/// wants nothing.
#[test]
fn lock_fields_become_door_key_door_and_item_gate_requirements() {
    let mut content = Content::default();
    let mut room = Room {
        id: HERE,
        name: "Slum Street".into(),
        ..Default::default()
    };
    room.exits[Direction::East as usize] = Some(Exit {
        dest: THERE,
        exit_type: 2,
        param: 172,
        param2: 2,
        param3: -99,
        ..Default::default()
    });
    room.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 3,
        param: 1054,
        ..Default::default()
    });
    room.exits[Direction::South as usize] = Some(Exit {
        dest: THERE,
        exit_type: 3,
        param: 0,
        ..Default::default()
    });
    room.exits[Direction::West as usize] = Some(Exit {
        dest: THERE,
        exit_type: 7,
        param: 2,
        param2: -30,
        ..Default::default()
    });
    room.exits[Direction::Up as usize] = Some(Exit {
        dest: THERE,
        exit_type: 7,
        param: 0,
        param2: 30,
        ..Default::default()
    });
    room.exits[Direction::Down as usize] = Some(Exit {
        dest: THERE,
        exit_type: 0xb,
        param: 2,
        param2: -999,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: THERE,
        name: "Black House".into(),
        ..Default::default()
    });

    let graph = RoomGraph::from_content(&content);
    let exits = &graph.room(HERE).unwrap().exits;
    let req = |d: Direction| exits[d as usize].as_ref().unwrap().requirement.clone();
    assert_eq!(
        req(Direction::East),
        ExitRequirement::KeyDoor { key: ItemId(172), pick: -99 }
    );
    assert_eq!(req(Direction::North), ExitRequirement::ItemGate { item: ItemId(1054) });
    assert_eq!(req(Direction::South), ExitRequirement::None, "item 0 wants nothing");
    assert_eq!(req(Direction::West), ExitRequirement::Door { locked: true, pick: -30 });
    assert_eq!(req(Direction::Up), ExitRequirement::Door { locked: false, pick: 30 });
    assert_eq!(req(Direction::Down), ExitRequirement::Door { locked: true, pick: -999 });
}
```

Add `ItemId` to the file's `use mud_core::content::{...}` line if it is not already imported. `HERE` and `THERE` are the file's existing constants.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mud-client --test graph_content lock_fields 2>&1 | grep -E "^error|^test result|FAILED"`
Expected: a compile error, `ExitRequirement::KeyDoor` does not exist.

- [ ] **Step 3: Write the requirements**

In `crates/mud-client/src/graph.rs` replace the `Door` variant in `ExitRequirement`:

```rust
    /// A door or gate, types 7 and 0xb. `locked` is the shipped lock
    /// state, `para1 == 2`, the word mud-core's `exit_lock_state`
    /// reads. `pick` is the pick modifier, `para2`, negative on hard
    /// locks. The state is the boot state: a lock with a positive
    /// modifier never re-locks once picked, so a door the data calls
    /// locked can stand unlocked all day. The walk tries `open` first
    /// either way, this only prices the route.
    Door { locked: bool, pick: i32 },
    /// A key door, type 2. Always locked. `key` is the item that opens
    /// it, `para1`, and `pick` the modifier, `para3`. The walk says
    /// `use <key> <direction>` with the key on the ring and picks
    /// without it.
    KeyDoor { key: ItemId, pick: i32 },
    /// Passable only while carrying `item`, `para1`. Type 3, 173 of
    /// them. Walked as a plain step, routing does the checking.
    ItemGate { item: ItemId },
```

Replace `from_exit_type`:

```rust
    /// The requirement an exit type implies on its own, before the
    /// cmdtext pass gets a say.
    ///
    /// `para1`, `para2` and `para3` are the raw slot fields. Which one
    /// means what depends on the type, see the lock variants' docs,
    /// and a fixture that cares about none of them passes zeros.
    ///
    /// Shared with the navigator's test fixtures deliberately. Those
    /// fixtures build exits by type and care about the WALKER's
    /// behaviour, so they must classify exactly as `load` does or they
    /// test a world that cannot exist. The tests that pin the
    /// classification itself do NOT call this — they assert
    /// hand-written expectations against fixture rooms, so they can
    /// still fail when this is wrong.
    pub fn from_exit_type(exit_type: i64, para1: i64, para2: i64, para3: i64) -> ExitRequirement {
        let item = |id: i64| u16::try_from(id).ok().filter(|id| *id != 0).map(ItemId);
        let modifier = |m: i64| i32::try_from(m).unwrap_or(0);
        match exit_type {
            2 => match item(para1) {
                Some(key) => ExitRequirement::KeyDoor { key, pick: modifier(para3) },
                // No key exists for it, so the lock is all there is.
                None => ExitRequirement::Door { locked: true, pick: modifier(para3) },
            },
            // 7 shipped gates want item 0, which is no item at all.
            3 => match item(para1) {
                Some(item) => ExitRequirement::ItemGate { item },
                None => ExitRequirement::None,
            },
            7 | 0xb => ExitRequirement::Door {
                locked: para1 == 2,
                pick: modifier(para2),
            },
            // Searchable until a slot or a script targets it.
            6 => ExitRequirement::Hidden,
            9 | 0x18 => ExitRequirement::Trap,
            COMMAND_EXIT => ExitRequirement::Command,
            4 => ExitRequirement::Toll {
                gold: u32::try_from(para1).unwrap_or(0),
            },
            0x14 | 0x16 | 0x17 => ExitRequirement::Gate,
            0x10 => ExitRequirement::Timed,
            _ => ExitRequirement::None,
        }
    }
```

In `from_content`, the edge construction near line 550 becomes:

```rust
                    requirement: ExitRequirement::from_exit_type(
                        exit_type,
                        param,
                        i64::from(exit.param2),
                        i64::from(exit.param3),
                    ),
```

- [ ] **Step 4: Update the call sites**

Every test fixture passes zeros. From the repo root:

```bash
sed -i 's/from_exit_type(\([^,()]*\), 0)/from_exit_type(\1, 0, 0, 0)/' \
  crates/mud-client/tests/sneak.rs \
  crates/mud-client/tests/nav_echo.rs \
  crates/mud-client/tests/graph.rs \
  crates/mud-client/tests/nav_hidden.rs \
  crates/mud-client/tests/nav_doors.rs \
  crates/mud-client/tests/backstab_opener.rs \
  crates/mud-client/tests/nav_command_exit.rs
grep -rn "from_exit_type(" crates/mud-client/tests | grep -v ", 0, 0, 0)"
```

The grep must print nothing. `tests/graph_content.rs` mentions the name only in a comment.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p mud-client 2>&1 | grep -E "^error|^test result|FAILED|panicked"`
Expected: every `test result:` line says `0 failed`. `state_free_requirements_keep_their_old_prices` in `tests/graph.rs` still passes because `exit_cost_for` has not learned the new variants yet and prices them by type.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/tests
git commit -m "feat(client): the graph reads lock state, key and item off a door"
```

---

### Task 2: Pickable, the lock price and the bashing flag

**Files:**
- Modify: `crates/mud-client/src/graph.rs` (`pickable`, `pick_rolls`, `picked_cost`, `exit_cost_for`, `Capabilities`)
- Modify: `crates/mud-client/src/nav.rs:686-760` (`new`, `with_capabilities`, `fenced`)
- Modify: `crates/mud-client/src/session.rs:807-815` (`capabilities`)
- Test: `crates/mud-client/tests/graph.rs`

**Interfaces:**
- Consumes: the three variants from Task 1, `exit_cost`, `Capabilities::has_item`.
- Produces: `pub fn pickable(modifier: i32, picklocks: u32) -> bool`, `pub fn pick_rolls(modifier: i32, picklocks: u32) -> u32`, `Capabilities.bash_doors: bool`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/mud-client/tests/graph.rs`:

```rust
/// The pick formula, `theft.md` sections 8.2 and 8.3: the roll is
/// `genrdn(0,100) < modifier + skill` and needs a skill of at least 1.
/// The shipped key door modifiers are the cases: below the line the
/// pick fails every time and no retry budget changes that.
#[test]
fn pickable_is_a_line_the_skill_must_cross() {
    use mud_client::graph::pickable;

    assert!(!pickable(0, 0), "no skill, no pick, whatever the lock");
    assert!(pickable(0, 1));
    assert!(pickable(30, 1), "an easy lock still wants a skill of 1");
    assert!(!pickable(30, 0));
    for modifier in [-30, -60, -99, -100, -160, -290, -999] {
        let line = u32::try_from(-modifier).unwrap();
        assert!(!pickable(modifier, line), "{modifier} + {line} is not above zero");
        assert!(pickable(modifier, line + 1), "{modifier} + {} is", line + 1);
    }
    assert!(pickable(-999, u32::MAX), "the unrestricted walker picks anything");
}

/// The expected number of `picklock` commands: the inverse of the
/// chance, rounded up, never below one.
#[test]
fn pick_rolls_is_the_inverse_of_the_chance() {
    use mud_client::graph::pick_rolls;

    assert_eq!(pick_rolls(0, 100), 1, "a certain pick is one command");
    assert_eq!(pick_rolls(30, u32::MAX), 1);
    assert_eq!(pick_rolls(-30, 80), 2, "50 in 100");
    assert_eq!(pick_rolls(-60, 80), 5, "20 in 100");
    assert_eq!(pick_rolls(-99, 100), 100, "1 in 100");
    assert_eq!(pick_rolls(-90, 100), 10);
    assert_eq!(pick_rolls(-70, 100), 4, "30 in 100 rounds up");
}

/// The Black House door: free with the key on the ring, a lock without
/// it. The lock is priced by the expected rolls and capped at a
/// searchable hidden exit, and a lock this character cannot pick is a
/// wall whether or not bashing is on.
#[test]
fn a_key_door_is_free_with_the_key_and_a_lock_without() {
    use mud_client::pack::PackHandle;
    use mud_client::sheet::Inventory;
    use mud_core::content::{Content, Item, ItemId};
    use std::sync::Arc;

    let edge = RoomId { map: 1, room: 1224 };
    let door = ExitRequirement::KeyDoor { key: ItemId(172), pick: -99 };
    let mut content = Content::default();
    content.add_item(Item {
        id: ItemId(172),
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    let pack = PackHandle::new(Arc::new(content));
    pack.refresh(&Inventory {
        items: Vec::new(),
        keys: vec!["black star key".into()],
        encumbrance: None,
    });
    let with_key = Capabilities {
        picklocks: 0,
        pack: Some(pack),
        ..Capabilities::unrestricted()
    };
    assert_eq!(
        exit_cost_for(&door, 2, edge, Direction::East, &with_key),
        Cost::Steps(5),
        "the key makes it an ordinary door"
    );
    let thief = Capabilities {
        picklocks: 100,
        ..Capabilities::unrestricted()
    };
    assert_eq!(
        exit_cost_for(&door, 2, edge, Direction::East, &thief),
        Cost::Steps(40),
        "1 in 100 is 100 rolls, capped at the hidden exit price"
    );
    let strong = Capabilities {
        picklocks: 149,
        ..Capabilities::unrestricted()
    };
    assert_eq!(
        exit_cost_for(&door, 2, edge, Direction::East, &strong),
        Cost::Steps(7),
        "50 in 100 is two rolls on top of the door"
    );
    for bash_doors in [true, false] {
        let weak = Capabilities {
            picklocks: 99,
            bash_doors,
            ..Capabilities::unrestricted()
        };
        assert_eq!(
            exit_cost_for(&door, 2, edge, Direction::East, &weak),
            Cost::Impassable,
            "-99 + 99 is not above zero, bashing {bash_doors}"
        );
    }
}

/// A locked door is priced by its lock. Pickable, the door plus the
/// rolls. Not pickable, the door when bashing is on, since force is a
/// separate roll, and a wall when it is off. An unlocked door is a
/// door.
#[test]
fn a_locked_door_is_priced_by_the_lock() {
    let edge = RoomId { map: 1, room: 1119 };
    let locked = ExitRequirement::Door { locked: true, pick: -30 };
    let unlocked = ExitRequirement::Door { locked: false, pick: -30 };
    let thief = Capabilities {
        picklocks: 80,
        ..Capabilities::unrestricted()
    };
    assert_eq!(exit_cost_for(&locked, 7, edge, Direction::North, &thief), Cost::Steps(7));
    let basher = Capabilities {
        picklocks: 0,
        bash_doors: true,
        ..Capabilities::unrestricted()
    };
    assert_eq!(exit_cost_for(&locked, 7, edge, Direction::North, &basher), Cost::Steps(5));
    let neither = Capabilities {
        picklocks: 0,
        bash_doors: false,
        ..Capabilities::unrestricted()
    };
    assert_eq!(
        exit_cost_for(&locked, 7, edge, Direction::North, &neither),
        Cost::Impassable
    );
    assert_eq!(
        exit_cost_for(&unlocked, 7, edge, Direction::North, &neither),
        Cost::Steps(5),
        "an unlocked door only wants an open"
    );
    assert_eq!(
        exit_cost_for(&locked, 0xb, edge, Direction::North, &basher),
        Cost::Steps(5),
        "a gate is priced like a door"
    );
}

/// An item gate is a step with the item and a wall without it.
#[test]
fn an_item_gate_is_a_step_with_the_item_and_a_wall_without() {
    use mud_client::pack::PackHandle;
    use mud_client::sheet::Inventory;
    use mud_core::content::{Content, Item, ItemId};
    use std::sync::Arc;

    let edge = RoomId { map: 1, room: 1 };
    let gate = ExitRequirement::ItemGate { item: ItemId(1054) };
    assert_eq!(
        exit_cost_for(&gate, 3, edge, Direction::North, &Capabilities::unrestricted()),
        Cost::Impassable,
        "the unrestricted walker holds nothing"
    );
    let mut content = Content::default();
    content.add_item(Item {
        id: ItemId(1054),
        name: "silver pass".into(),
        ..Default::default()
    });
    let pack = PackHandle::new(Arc::new(content));
    pack.refresh(&Inventory {
        items: vec!["silver pass".into()],
        keys: Vec::new(),
        encumbrance: None,
    });
    let holder = Capabilities {
        pack: Some(pack),
        ..Capabilities::unrestricted()
    };
    assert_eq!(exit_cost_for(&gate, 3, edge, Direction::North, &holder), Cost::Steps(1));
}
```

In the same file, `state_free_requirements_keep_their_old_prices` loops over exit types. Remove `2` from its list and add a sentence to its doc comment: "Type 2 is gone from the list: a key door is priced by its lock now, see `a_key_door_is_free_with_the_key_and_a_lock_without`." Types 7 and 0xb stay, a fixture with zero params is an unlocked door and still costs 5. Type 3 stays, item 0 is `None` and still costs 1.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test graph 2>&1 | grep -E "^error|^test result|FAILED"`
Expected: compile errors, `pickable` and the `bash_doors` field do not exist.

- [ ] **Step 3: Write the formula and the prices**

In `crates/mud-client/src/graph.rs`, after `exit_cost` and before `Capabilities`:

```rust
/// Can this character ever pick a lock with this modifier?
///
/// `theft.md` sections 8.2 and 8.3, mirrored in mud-core's
/// `picklock_command`: the skill must be at least 1 and the roll is
/// `genrdn(0,100) < modifier + skill`. Below zero the roll never
/// passes, so the pick fails every time and no budget of retries
/// changes that. Shipped key doors carry -60, -99, -100, -160, -290
/// and -999, and the plain doors mostly 0, -20, -70 and -999. The one
/// place this line is drawn: routing asks it to price a lock and the
/// walk asks it before spending a roll.
pub fn pickable(modifier: i32, picklocks: u32) -> bool {
    picklocks >= 1 && i64::from(modifier) + i64::from(picklocks) > 0
}

/// How many `picklock` commands a lock is expected to take from a
/// character [`pickable`] says can open it. The roll passes with
/// chance `modifier + skill` in 100, so the expectation is the inverse,
/// rounded up. Never below one: a certain pick is still a command.
pub fn pick_rolls(modifier: i32, picklocks: u32) -> u32 {
    let chance = (i64::from(modifier) + i64::from(picklocks)).clamp(1, 100);
    u32::try_from((100 + chance - 1) / chance).unwrap_or(u32::MAX)
}

/// The price of a lock a pickable character will pick: the door, then
/// the expected rolls, never above a searchable hidden exit. The cap
/// keeps a 1 in 100 lock routable at all, and the walk's own retry
/// budget is what decides whether it gives.
fn picked_cost(modifier: i32, picklocks: u32) -> Cost {
    let door = exit_cost(7);
    Cost::Steps((door + pick_rolls(modifier, picklocks)).min(exit_cost(6)))
}
```

In `exit_cost_for`, add these arms before the `_ =>` arm:

```rust
        // An unlocked door only wants an open.
        ExitRequirement::Door { locked: false, .. } => Cost::Steps(exit_cost(exit_type)),
        // A lock: picked when the formula allows, else bashed when
        // bashing is on, since force is a separate roll, else a wall.
        ExitRequirement::Door { locked: true, pick } => {
            if pickable(*pick, caps.picklocks) {
                picked_cost(*pick, caps.picklocks)
            } else if caps.bash_doors {
                Cost::Steps(exit_cost(exit_type))
            } else {
                Cost::Impassable
            }
        }
        // The key makes it an ordinary door. Without it the lock is
        // all there is, and a key door nobody can pick is a wall
        // whether or not bashing is on.
        ExitRequirement::KeyDoor { key, pick } => {
            if caps.has_item(*key) {
                Cost::Steps(exit_cost(exit_type))
            } else if pickable(*pick, caps.picklocks) {
                picked_cost(*pick, caps.picklocks)
            } else {
                Cost::Impassable
            }
        }
        ExitRequirement::ItemGate { item } => {
            if caps.has_item(*item) {
                Cost::Steps(1)
            } else {
                Cost::Impassable
            }
        }
```

Update the `exit_cost_for` doc comment's sentence about `Impassable` to: "`Impassable` is reserved for edges no amount of walking opens: a toll beyond the purse, a lever exit this walker has no plan for, a lock it cannot pick and may not bash, a key door without the key or the skill, and an item gate without the item."

In `Capabilities`, replace the `picklocks` doc comment with:

```rust
    /// The character's own `stat`-sheet `Picklocks` skill. Routing
    /// prices a lock by it through [`pickable`] and [`pick_rolls`],
    /// and [`crate::nav::Navigator`] asks the same formula before
    /// spending a roll. Zero by default: an operator who never asked
    /// [`crate::session::Session::stats`] gets a character who cannot
    /// pick, the same safe direction an empty purse already takes for
    /// tolls.
    pub picklocks: u32,
```

Add the field after `pack`:

```rust
    /// Whether the walk may bash a door it can neither open nor pick.
    /// A navigator sets this from its own `bash_doors` config, the one
    /// switch there is, so routing and the walk agree on which locked
    /// doors are a wall. The session leaves it off: it has no config,
    /// and a roam, the one thing that routes with the session's own
    /// capabilities, never crosses a door at all.
    pub bash_doors: bool,
```

In `unrestricted()` add `bash_doors: true,` after the `pack: None,` line with the comment `// Every already existing cost is affordable, and bashing is a cost.`

In `crates/mud-client/src/session.rs` `capabilities()`, add `bash_doors: false,` after `pack: self.pack_handle(),`. Extend the method's doc comment with one sentence: "`bash_doors` is off, the navigator that walks for this session sets it from its own config."

In `crates/mud-client/src/nav.rs`:

`Navigator::new`:

```rust
            capabilities: crate::graph::Capabilities {
                bash_doors: cfg.bash_doors,
                ..crate::graph::Capabilities::unrestricted()
            },
```

`with_capabilities`:

```rust
    pub fn with_capabilities(mut self, caps: crate::graph::Capabilities) -> Self {
        // Routing must price a lock the way this walk will treat it,
        // so the bashing switch is this navigator's, not the caller's.
        self.capabilities = crate::graph::Capabilities {
            bash_doors: self.bash_doors,
            ..caps
        };
        self
    }
```

`fenced`, after `self.bash_doors = false;`:

```rust
        self.capabilities.bash_doors = false;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client 2>&1 | grep -E "^error|^test result|FAILED|panicked"`
Expected: every `test result:` line says `0 failed`. If a `Capabilities { .. }` literal without a spread fails to compile, add `bash_doors: false` to it. Every literal found while planning uses `..Default::default()` or `..Capabilities::unrestricted()`.

- [ ] **Step 5: Commit**

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/src/nav.rs crates/mud-client/src/session.rs crates/mud-client/tests/graph.rs
git commit -m "feat(client): routing prices a lock by the pick formula, the key and the item"
```

---

### Task 3: `speak` becomes `ask`

A refactor with no behaviour change. The key verb in Task 4 needs the same unattributed read `speak` does, so the read is pulled out first and `speak` becomes a caller.

**Files:**
- Modify: `crates/mud-client/src/nav.rs:1958-2033` (`speak`)
- Test: `crates/mud-client/tests/nav_puzzle.rs` (unchanged, must still pass)

**Interfaces:**
- Consumes: `SAID_ALOUD`, `ExpectError`, `TravelGuard`, `Interrupt`.
- Produces: `enum Answer { Replied, Prompted, SaidAloud }` and `async fn ask(&self, session: &Session, command: &str, reply: Option<&str>, guard: &mut impl TravelGuard, armed: &mut Option<Interrupt>) -> Result<Answer, NavErrorKind>`.

- [ ] **Step 1: Write the enum and `ask`**

Above `impl Navigator`'s `speak`, at module level next to `enum StepEvent`, add:

```rust
/// How the board answered a line the correlator cannot attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The wanted reply line arrived before the next prompt.
    Replied,
    /// The next prompt came without it.
    Prompted,
    /// The board said the line out loud. It was not a command here.
    SaidAloud,
}
```

Replace `speak` with the pair:

```rust
    /// Send a line the correlator files as opaque and read what comes
    /// back.
    ///
    /// A fresh receiver, subscribed before the send, so nothing an
    /// inner walk left behind can be mistaken for the reply. The reply
    /// body is unattributed, `kind_of` files a phrase or a `use` as
    /// `Opaque`, so the read is [`Navigator::arm_sneak`]'s: skip the
    /// echo, then classify what arrives before the next prompt.
    /// `reply`, lowercased, ends the read early when it arrives. The
    /// board saying the line out loud means it was not a command here.
    async fn ask(
        &self,
        session: &Session,
        command: &str,
        reply: Option<&str>,
        guard: &mut impl TravelGuard,
        armed: &mut Option<Interrupt>,
    ) -> Result<Answer, NavErrorKind> {
        let mut events = session.events();
        let id = session.send(command);
        let spoken = command.to_lowercase();
        let reply = reply.map(str::to_lowercase);
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
                        needle: format!("reply to {command}"),
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
                        return Ok(Answer::SaidAloud);
                    }
                    if reply.as_deref().is_some_and(|r| line.contains(r)) {
                        return Ok(Answer::Replied);
                    }
                }
                crate::events::Event::Prompt { .. } if echoed => return Ok(Answer::Prompted),
                _ => {}
            }
        }
    }

    /// Speak one action's phrase in the room it belongs to and read the
    /// answer. The slot's own reply line ends the read early. The board
    /// saying the phrase out loud means it was not a command here, and
    /// no retry can change that.
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
        match self
            .ask(session, phrase, action.reply.as_deref(), guard, armed)
            .await?
        {
            Answer::SaidAloud => Err(NavErrorKind::Puzzle {
                dir: dir.to_string(),
                tried: format!("{phrase:?} was said aloud, not acted on"),
            }),
            Answer::Replied | Answer::Prompted => Ok(()),
        }
    }
```

- [ ] **Step 2: Run the puzzle tests**

Run: `cargo test -p mud-client --test nav_puzzle 2>&1 | grep -E "^error|^test result|FAILED"`
Expected: `test result: ok` with every test passing, in particular `a_phrase_said_aloud_ends_the_step_at_once` and `a_same_room_button_is_pushed_when_the_wall_refuses`.

- [ ] **Step 3: Run the full suite and commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^error|^test result|FAILED|panicked"`
Expected: every `test result:` line says `0 failed`.

```bash
git add crates/mud-client/src/nav.rs
git commit -m "refactor(client): the unattributed read is one function"
```

---

### Task 4: The key verb in the walk

**Files:**
- Modify: `crates/mud-client/src/nav.rs` (`can_pick`, `walk_step`, `read_purse` doc)
- Create: `crates/mud-client/tests/nav_keys.rs`

**Interfaces:**
- Consumes: `ExitRequirement::KeyDoor`, `pickable`, `Answer`, `ask`, `DOOR_UNLOCKED`, `read_purse`, `crate::session::drain`, `PackHandle::has` and `PackHandle::content`, `mud_core::text::direction_shown`.
- Produces: `fn pick_modifier(requirement: &ExitRequirement) -> i32`, `fn can_pick(&self, modifier: i32) -> bool`, `fn key_name(&self, key: ItemId) -> Option<String>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/mud-client/tests/nav_keys.rs`:

```rust
//! Key doors.
//!
//! The Black House door on Slum Street, captured 2026-09-05: `unlock e`
//! is not a command, `use black star key east` answers "You
//! successfully unlocked the door.", the same line a pick gives, and
//! the door still needs `open e`. The board here says exactly that.
//! Its `i` reply is the four line inventory, so the walk's re-read of
//! the pack after a key use has something to read.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, NavErrorKind, Navigator, NoGuard};
use mud_client::pack::PackHandle;
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_client::sheet::Inventory;
use mud_core::content::{Content, Direction, Item, ItemId, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const STREET: RoomId = RoomId { map: 1, room: 1224 };
const HOUSE: RoomId = RoomId { map: 1, room: 1225 };
const KEY: ItemId = ItemId(172);
const PROMPT: &str = "\r\n[HP=30/MA=0]:";

#[derive(Default)]
struct DoorLog {
    lines: std::sync::Mutex<Vec<String>>,
    opens: AtomicUsize,
    uses: AtomicUsize,
    picks: AtomicUsize,
    bashes: AtomicUsize,
    inventories: AtomicUsize,
}

impl DoorLog {
    fn count(&self, line: &str) -> usize {
        self.lines.lock().unwrap().iter().filter(|l| l.as_str() == line).count()
    }
}

fn block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}{PROMPT}")
}

/// The door east of the street. `key_works` says whether the key is
/// the right one. `ring_after` is the key ring line the board prints
/// once asked, so a test can say the key was spent.
async fn house_board(
    key_works: bool,
    ring_after: &'static str,
) -> (std::net::SocketAddr, Arc<DoorLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(DoorLog::default());
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(block("Slum Street", "closed door east").as_bytes())
            .await
            .unwrap();
        let mut locked = true;
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
                let reply = match line.as_str() {
                    "e" | "east" if open => block("Black House", "open door west"),
                    "e" | "east" => format!("\r\nThe door is closed.{PROMPT}"),
                    "open e" | "open east" => {
                        seen.opens.fetch_add(1, Ordering::SeqCst);
                        if locked {
                            format!("\r\nThe door is locked.{PROMPT}")
                        } else {
                            open = true;
                            format!("\r\nThe door is now open.{PROMPT}")
                        }
                    }
                    "use black star key east" => {
                        seen.uses.fetch_add(1, Ordering::SeqCst);
                        if key_works {
                            locked = false;
                            format!("\r\nYou successfully unlocked the door.{PROMPT}")
                        } else {
                            format!("\r\nNothing happens.{PROMPT}")
                        }
                    }
                    "picklock e" | "picklock east" => {
                        seen.picks.fetch_add(1, Ordering::SeqCst);
                        locked = false;
                        format!("\r\nYou successfully unlocked the door.{PROMPT}")
                    }
                    "bash e" | "bash east" => {
                        seen.bashes.fetch_add(1, Ordering::SeqCst);
                        open = true;
                        format!("\r\nYou bashed the door open.{PROMPT}")
                    }
                    "i" => {
                        seen.inventories.fetch_add(1, Ordering::SeqCst);
                        format!(
                            "\r\nYou are carrying nothing.\r\n{ring_after}\r\nWealth: 0 copper farthings\r\nEncumbrance: 0/2400 - None [0%]{PROMPT}"
                        )
                    }
                    "look" => block("Slum Street", "closed door east"),
                    other => format!("\r\nYou say \"{other}\"{PROMPT}"),
                };
                sock.write_all(format!("\r\n{line}{reply}").as_bytes())
                    .await
                    .unwrap();
            }
        }
    });
    (addr, log)
}

/// The street and the house, joined by a door with `requirement`. A
/// key door is type 2, anything else here is a plain type 7 door.
fn house_graph(requirement: ExitRequirement) -> Arc<RoomGraph> {
    let exit_type = match requirement {
        ExitRequirement::KeyDoor { .. } => 2,
        _ => 7,
    };
    let door = |dest| ExitEdge {
        dest,
        exit_type,
        command: None,
        requirement: requirement.clone(),
    };
    let mut street = GraphRoom {
        name: "Slum Street".into(),
        ..Default::default()
    };
    street.exits[Direction::East as usize] = Some(door(HOUSE));
    let mut house = GraphRoom {
        name: "Black House".into(),
        ..Default::default()
    };
    house.exits[Direction::West as usize] = Some(door(STREET));
    Arc::new(RoomGraph::from_rooms(vec![(STREET, street), (HOUSE, house)]))
}

fn table() -> Arc<Content> {
    let mut content = Content::default();
    content.add_item(Item {
        id: KEY,
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    Arc::new(content)
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
    let session = Session::connect(&profile, None).await.unwrap();
    session.set_content(table());
    session
}

/// The session's own pack, holding the key or not.
fn ring(session: &Session, with_key: bool) -> PackHandle {
    let pack = session.pack_handle().expect("set_content gave the session a pack");
    pack.refresh(&Inventory {
        items: Vec::new(),
        keys: if with_key { vec!["black star key".into()] } else { Vec::new() },
        encumbrance: None,
    });
    pack
}

fn key_door() -> ExitRequirement {
    ExitRequirement::KeyDoor { key: KEY, pick: -99 }
}

fn nav(
    session: &Session,
    requirement: ExitRequirement,
    with_key: bool,
    picklocks: u32,
    bash_doors: bool,
) -> Navigator {
    Navigator::new(
        house_graph(requirement),
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors,
            ..NavConfig::default()
        },
    )
    .with_capabilities(Capabilities {
        picklocks,
        pack: Some(ring(session, with_key)),
        ..Capabilities::unrestricted()
    })
}

async fn walk(n: &Navigator, session: &Session) -> Result<RoomId, NavErrorKind> {
    tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(session, STREET, HOUSE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .map(|at| at.at)
    .map_err(|e| e.kind)
}

/// The captured flow: `open e` says locked, the key is used with its
/// full name and the full direction word, the unlocked line arrives,
/// `open e` now opens, and the step lands. No roll is spent.
#[tokio::test]
async fn a_key_door_is_unlocked_with_the_key_then_opened_and_walked() {
    let (addr, log) = house_board(true, "You have the following keys:  black star key.").await;
    let session = session_for(addr).await;
    let n = nav(&session, key_door(), true, 0, false);

    let at = walk(&n, &session).await.expect("the key opens it");

    assert_eq!(at, HOUSE);
    assert_eq!(log.count("use black star key east"), 1, "{:?}", log.lines.lock().unwrap());
    assert_eq!(log.opens.load(Ordering::SeqCst), 2, "one refused, one after the key");
    assert_eq!(log.picks.load(Ordering::SeqCst), 0);
    assert_eq!(log.bashes.load(Ordering::SeqCst), 0);
}

/// 37 shipped keys have one use. After the key is used the pack is
/// re-read from the board, never updated by inference, so a spent key
/// leaves the ring before routing trusts it again.
#[tokio::test]
async fn the_pack_is_re_read_after_the_key_is_used() {
    let (addr, log) = house_board(true, "You have no keys.").await;
    let session = session_for(addr).await;
    let n = nav(&session, key_door(), true, 0, false);
    let pack = session.pack_handle().unwrap();
    assert!(pack.has(KEY), "the walk starts with the key");

    walk(&n, &session).await.expect("the key opens it");

    assert!(log.inventories.load(Ordering::SeqCst) >= 1, "the pack was asked for");
    // The reader task refreshes the pack when the reply's last line
    // lands, which can be a moment after the walk returns.
    for _ in 0..20 {
        if !pack.has(KEY) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the spent key is still on the ring");
}

/// A lock the formula says cannot give is never picked. -99 against 10
/// Picklocks fails every roll, so the walk goes straight to force
/// rather than spending its whole pick budget first. A plain locked
/// door, because routing refuses a key door nobody can pick even with
/// bashing on, and the point here is the walk.
#[tokio::test]
async fn an_unpickable_lock_spends_no_pick() {
    let (addr, log) = house_board(true, "You have no keys.").await;
    let session = session_for(addr).await;
    let locked = ExitRequirement::Door { locked: true, pick: -99 };
    let n = nav(&session, locked, false, 10, true);

    let at = walk(&n, &session).await.expect("bashing is on");

    assert_eq!(at, HOUSE);
    assert_eq!(log.uses.load(Ordering::SeqCst), 0, "no key door, nothing to use");
    assert_eq!(log.picks.load(Ordering::SeqCst), 0, "-99 + 10 never passes");
    assert!(log.bashes.load(Ordering::SeqCst) >= 1);
}

/// Without the key, the skill or bashing there is nothing to try, and
/// routing knows it before a command is sent.
#[tokio::test]
async fn a_key_door_nobody_can_open_is_no_route() {
    let (addr, log) = house_board(true, "You have no keys.").await;
    let session = session_for(addr).await;
    let n = nav(&session, key_door(), false, 10, false);

    let err = walk(&n, &session).await.expect_err("nothing opens it");

    assert!(matches!(err, NavErrorKind::NoRoute), "{err}");
    assert_eq!(log.opens.load(Ordering::SeqCst), 0, "routing refused it first");
}

/// A key the board does not accept at this door is answered with some
/// other line and the next prompt. The walk reads that as the key
/// doing nothing and the lock is the story again: picking and bashing
/// get their turn.
#[tokio::test]
async fn a_key_that_does_nothing_falls_through_to_the_lock() {
    let (addr, log) = house_board(false, "You have the following keys:  black star key.").await;
    let session = session_for(addr).await;
    let n = nav(&session, key_door(), true, 100, false);

    let at = walk(&n, &session).await.expect("the pick opens it");

    assert_eq!(at, HOUSE);
    assert_eq!(log.uses.load(Ordering::SeqCst), 1);
    assert_eq!(log.picks.load(Ordering::SeqCst), 1, "-99 + 100 is worth a roll");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test nav_keys 2>&1 | grep -E "^error|^test result|FAILED|panicked"`
Expected: the tests compile and fail. `a_key_door_is_unlocked_with_the_key_then_opened_and_walked` fails because the walk never sends `use`, it picks or reports the door locked. `an_unpickable_lock_spends_no_pick` fails because the walk picks when `picklocks > 0`. `a_key_door_nobody_can_open_is_no_route` passes already, routing was finished in Task 2.

- [ ] **Step 3: Teach the walk the key**

In `crates/mud-client/src/nav.rs`, replace `can_pick`:

```rust
    /// The pick modifier of the lock in the way, for [`crate::graph::pickable`].
    /// A lever gate lost its modifier to the puzzle and reads 0, which
    /// keeps today's rule for it: any Picklocks at all is worth a roll.
    fn pick_modifier(requirement: &crate::graph::ExitRequirement) -> i32 {
        match requirement {
            crate::graph::ExitRequirement::Door { pick, .. }
            | crate::graph::ExitRequirement::KeyDoor { pick, .. } => *pick,
            _ => 0,
        }
    }

    /// Is picking this lock worth attempting right now?
    ///
    /// Two independent vetoes, either one enough to refuse: the
    /// formula may say the character cannot pick this modifier at all
    /// (`capabilities.picklocks`, read off the `stat` sheet, see
    /// [`crate::session::Session::stats`], against the lock's own
    /// modifier), or a fence may forbid it regardless of skill (see
    /// [`Navigator::fenced`]). A character the formula allows still
    /// just fails the roll sometimes, that is the board's own dice, not
    /// this gate.
    fn can_pick(&self, modifier: i32) -> bool {
        crate::graph::pickable(modifier, self.capabilities.picklocks) && !self.picking_fenced_off
    }

    /// The name to say in `use <key> <direction>`: the key's row in the
    /// item table, only when the pack holds it. `None` means there is
    /// nothing to use and the lock is what remains.
    fn key_name(&self, key: mud_core::content::ItemId) -> Option<String> {
        let pack = self.capabilities.pack.as_ref()?;
        if !pack.has(key) {
            return None;
        }
        pack.content().items.get(&key).map(|item| item.name.clone())
    }
```

In `walk_step`, immediately after the `match` on the first `open`'s reply, which ends with `StepEvent::DoorBlocked => {}` and a closing brace, and before the `// A lever on a gate toggles its lock` comment, insert:

```rust
        // A key door with the key on the ring. `use <key> <direction>`
        // answers with the same unlocked line a pick does and the door
        // still wants its open. The reply is unattributed, so it is
        // read the way a lever phrase is. The key may have been spent,
        // 37 shipped keys have one use, so the pack is re-read from
        // the board before routing trusts it again.
        if let crate::graph::ExitRequirement::KeyDoor { key, .. } = requirement {
            if let Some(name) = self.key_name(*key) {
                let command = format!("use {name} {}", mud_core::text::direction_shown(step));
                let answer = self
                    .ask(session, &command, Some(DOOR_UNLOCKED), guard, armed)
                    .await?;
                // Clear what the use queued so the open below is
                // answered by its own lines, and show every event to
                // the guard on the way out, exactly as after a plan.
                crate::session::drain(events, |ev| {
                    *armed = armed.take().or_else(|| guard.on_event(&ev.event));
                });
                if let Some(interrupt) = armed.take() {
                    return Err(NavErrorKind::Interrupted(interrupt));
                }
                if answer == Answer::Replied {
                    self.read_purse(session, events, guard, armed).await?;
                    let opened = session.send(&format!("open {dir}"));
                    return match self.wait_room(events, guard, armed, opened, sneak_seen).await? {
                        StepEvent::DoorYielded => {
                            let again = session.send(dir);
                            self.arrival(
                                here, expected, BlindContext::AfterMove, events, guard, armed,
                                again, sneak_seen,
                            )
                            .await
                            .map(StepOutcome::Arrived)
                        }
                        StepEvent::CombatBlocked => Err(NavErrorKind::Interrupted(Interrupt::Attacked {
                            by: "combat".into(),
                        })),
                        // Unlocked and still refusing to open is not
                        // something the key can help with.
                        _ => Err(NavErrorKind::DoorLocked {
                            dir: dir.to_string(),
                            tried: "used the key, but the door would not open".into(),
                        }),
                    };
                }
                // The key did nothing here. The lock is the story
                // again, and picking and bashing below get their turn.
            }
        }
```

Change the pick loop's guard from `if self.can_pick() {` to:

```rust
        let modifier = Self::pick_modifier(requirement);
        if self.can_pick(modifier) {
```

Replace the `if !self.bash_doors {` block that follows the pick loop:

```rust
        if !self.bash_doors {
            let key_door = matches!(requirement, crate::graph::ExitRequirement::KeyDoor { .. });
            return Err(NavErrorKind::DoorLocked {
                dir: dir.to_string(),
                tried: match (self.can_pick(modifier), key_door) {
                    (true, _) => format!("{PICK_RETRIES} picks, bashing off"),
                    (false, true) => "no key, can't pick, bashing off".into(),
                    (false, false) => "can't pick, bashing off".into(),
                },
            });
        }
```

Add one sentence to `read_purse`'s doc comment: "The same `i` reply refreshes the session's pack, which is why a key use reads the purse it does not need."

- [ ] **Step 4: Run the tests**

Run: `cargo test -p mud-client --test nav_keys --test nav_doors --test nav_puzzle 2>&1 | grep -E "^error|^test result|FAILED|panicked"`
Expected: three `test result:` lines, each `0 failed`. `a_character_with_no_picklocks_does_not_pick` in `nav_doors.rs` still passes, `pickable(0, 0)` is false.

- [ ] **Step 5: Run the full suite and commit**

Run: `cargo test -p mud-client 2>&1 | grep -E "^error|^test result|FAILED|panicked"`
Expected: every `test result:` line says `0 failed`.

```bash
git add crates/mud-client/src/nav.rs crates/mud-client/tests/nav_keys.rs
git commit -m "feat(client): the walk opens a key door with the key on the ring"
```

---

### Task 5: A roam and an item gate

No source change is expected. `roam::passable` never excluded type 3 and the region flood already prices edges with the character's capabilities, so this pins the behaviour Task 2 gave the roam for free.

**Files:**
- Test: `crates/mud-client/tests/roam.rs`

**Interfaces:**
- Consumes: `roam::region`, `ExitRequirement::ItemGate`, the file's `room`, `with_fork` and `empty_handed` helpers and its `POST`, `VAULT` and `FORK` constants.

- [ ] **Step 1: Write the test**

Append to `crates/mud-client/tests/roam.rs`:

```rust
/// A guard post whose north exit wants the fork carried.
fn gated_world() -> RoomGraph {
    let mut post = room("Guard Post", &[]);
    post.exits[Direction::North as usize] = Some(ExitEdge {
        dest: VAULT,
        exit_type: 3,
        command: None,
        requirement: ExitRequirement::ItemGate { item: FORK },
    });
    RoomGraph::from_rooms(vec![
        (POST, post),
        (VAULT, room("Vault", &[(Direction::South, POST)])),
    ])
}

/// An item gate is inside a roam for the character carrying the item
/// and a wall for one who is not. Doors stay outside either way.
#[test]
fn a_region_grows_through_an_item_gate_only_with_the_item() {
    let graph = gated_world();
    let walls = Walls::default();
    assert_eq!(
        region(&graph, POST, &walls, &with_fork()),
        BTreeSet::from([POST, VAULT])
    );
    assert_eq!(
        region(&graph, POST, &walls, &empty_handed()),
        BTreeSet::from([POST])
    );
}
```

If `Walls` has no `Default`, build it the way the file's existing region tests do.

- [ ] **Step 2: Run the test**

Run: `cargo test -p mud-client --test roam 2>&1 | grep -E "^error|^test result|FAILED|panicked"`
Expected: `test result: ok`, the new test passes on the first run. If it fails with the vault inside the empty handed region, `exit_cost_for`'s `ItemGate` arm from Task 2 is not being consulted and the fix belongs there, not here.

- [ ] **Step 3: Commit**

```bash
git add crates/mud-client/tests/roam.rs
git commit -m "test(client): a roam grows through an item gate only with the item"
```

---

### Task 6: Documentation

**Files:**
- Modify: `docs/mud-client.md` (the `bash_doors` bullet near line 338, the roam paragraph near line 498, the Doors section near line 620, the tests list near line 733)

- [ ] **Step 1: The `bash_doors` bullet**

Replace the bullet:

```markdown
- **`bash_doors`** (true) — fall back to bashing a door that `open` will
  not shift and the character cannot pick. Bashing costs HP (*"You take
  %d damage for bashing the door!"*) and needs a weapon, so it is a
  switch. Turning it off makes a locked door the character cannot pick a
  wall: routing goes around it, and a route that has no way around says
  `no route`.
```

- [ ] **Step 2: The roam paragraph**

Replace the paragraph beginning `**Doors are outside a roam, all of them**` and the one after it, up to and not including `**Buttons and levers are inside a roam.**`, with:

```markdown
**Doors are outside a roam, all of them**, whether or not you marked
them. Live on cwgaming, 2026-08-03: the board answered `open n` with
*"The door is locked."* and the walk sent **88 bashes across four
approaches, spending 36 HP of a 75-HP character** on a type-7 lock that
wanted Picklocks and was never going to yield to force. A roam always
has somewhere else to be, so skipping a door costs nothing and trying
one is paid for in health. A patrol with a circuit is a different
bargain, its stops were named by you and a door in the way has to be
opened, so `[farm.nav].bash_doors` still governs there.

**Item gates are inside a roam** for the character carrying the item
and a wall for one who is not. The region is flooded with the
character's own pack, the same way it is with the character's own
levers.
```

- [ ] **Step 3: The Doors section**

After the paragraph `Both direction forms work (open n and open north, verified live).` and before `## Buttons and levers`, add:

```markdown
### Locks

Every door and gate carries a lock state and a pick modifier in the room
record, and the graph reads both. 416 of the 449 shipped type 7 doors
are locked in the data. The pick roll is `theft.md` §8.2: the skill
must be at least 1 and `genrdn(0,100) < modifier + Picklocks`. So a lock
is pickable at all only when `Picklocks >= 1` and `modifier + Picklocks
> 0`. Below that line the pick fails every time, and the walk never
spends a roll there.

Routing prices a lock by that formula. A door the character can pick
costs the door plus the expected rolls, capped at the price of a
searchable hidden exit. A lock it cannot pick costs the door when
`bash_doors` is on, since force is a separate roll, and is a wall when
it is off. The lock state is the boot state: a lock with a positive
modifier never re-locks once picked, so a door the data calls locked can
stand open all day. The walk tries `open` first regardless.

## Keys and item gates

A type 2 door wants a key, one of 88 key items, and the key ring is read
off the `i` reply along with the pack. With the key on the ring the door
is priced like any other and the walk opens it: `open e` says *"The door
is locked."*, then `use black star key east` with the key's full name
and the full direction word answers *"You successfully unlocked the
door."*, the same line a pick gives, and `open e` then opens it. Captured
at the Black House on Slum Street, 2026-09-05. `unlock` is not a
command. Most keys are spent by use, 37 of them on the first, so the
pack is re-read from the board after every key use rather than updated
by inference.

Without the key the door is a lock like any other, priced and picked by
the formula above, except that a key door nobody can pick is a wall
whether or not bashing is on. A key the board does not accept at a door
is answered with some other line, and the walk then treats the lock as
the story again.

A type 3 exit is passable only while carrying an item. 173 ship, 7 of
them wanting nothing. Routing checks the pack and the walk takes it as a
plain step. Without the item it is a wall.

The bot picks up any key it sees on the floor that the ring lacks,
`[bot].take_keys`, on by default.
```

- [ ] **Step 4: The tests list**

In the tests list near line 733, on the line naming `tests/nav_doors.rs`, add `tests/nav_keys.rs` after it, keeping the line's existing form.

- [ ] **Step 5: Check the prose and commit**

Run: `grep -n "—\|;" docs/mud-client.md | sed -n 1,5p` to confirm the new paragraphs carry no em dashes or semicolons. Existing lines may. Then:

```bash
git add docs/mud-client.md
git commit -m "doc(client): locks, keys and item gates"
```

---

## Self-review

**Spec coverage.** Part 3's graph shapes are Task 1. `pickable` in one place with the shipped modifiers as cases is Task 2. Every cost rule in Part 3 is an arm in Task 2 with a test. The key verb, the unlocked reply and the fall through to pick and bash are Task 4. The item gate as a plain step is Task 4 by omission and Task 5 for roams. The pack re-read after a key use is Task 4's second test. Part 1's key pickup and Part 2's puzzles shipped in earlier phases. The spec's "Testing" list names `pickable` on both sides of the line, `exit_cost_for` with and without the pack and the `Impassable` cases, and a scripted key door, and each has a test above.

**Placeholders.** None. Every step carries the code or the command.

**Type consistency.** `from_exit_type(exit_type, para1, para2, para3)` in Task 1 is what the sed in Task 1 and the loader call produce. `pickable(modifier: i32, picklocks: u32)` and `pick_rolls` in Task 2 are what `can_pick(modifier)` in Task 4 and the tests call. `Answer` and `ask` in Task 3 are what the key stage in Task 4 uses. `Capabilities.bash_doors` in Task 2 is set in the three navigator methods named there and read in `exit_cost_for`. `ExitRequirement::KeyDoor { key, pick }` is spelled the same in Tasks 1, 2 and 4 and in `nav_keys.rs`.
