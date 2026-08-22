# Exit requirements, part 1: the descriptor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** LANDED on main 2026-08-22, commits `1161ad5b..94b648c8` (5 commits),
plus integration fix `e24ba194`. All 3 tasks implemented, task-reviewed, and passed
a whole-branch review (verdict: ship). Merged `--ff-only`. The unticked checkboxes
below are an artefact of execution: implementers worked from extracted per-task
briefs, not this file.

**HANDOFF to the routing plan:** `Puzzle { actions }` REPLACES `Door` on 10 shipped
lever-opened gates. Nothing is lost — `ExitEdge::exit_type` is retained — so routing
recovers door-ness with `nav::is_door(edge.exit_type) && matches!(edge.requirement,
Puzzle { .. })`. A consumer that reads only `requirement` will route a walker past a
gate it could have picked or bashed.

**Goal:** Give every exit a descriptor saying what it requires, computed once at load, and use it to stop the walker searching for a passage that no search can ever reveal.

**Architecture:** `RoomGraph::load` already reads `roomtype_*` and `para1_*` and already does a side pass over the `message` table for command-exit phrases. Add a second side pass over the rooms that carry a `cmdtext` script, so that a `remoteaction` targeting an exit marks that exit as concealed-by-puzzle rather than concealed-by-search. Store the result on `ExitEdge` as an `ExitRequirement`. Nothing in this plan consults character state — that is part 2.

**Tech Stack:** Rust, `rusqlite`, `cargo test -p mud-client`.

**Spec:** `docs/superpowers/specs/2026-08-22-exit-requirements-design.md` (this plan covers the `ExitRequirement` section, the `Puzzle` extraction, and success criterion 2; parts 1, 3 and 4 are the follow-on plan)

## Global Constraints

- **Never run `cargo fmt`, `rustfmt`, or any formatter.**
- Every file ends with a blank line.
- Commit after every task with a tag prefix.
- `re/mmud_wgnt.sqlite` is a gitignored 119 MB fixture; tests needing it skip with a message when absent.
- Exit type numbering is `re/docs/theft.md` §8.1, which is authoritative for this project **over** the stale display-code list in `re/docs/vir_schemas.md`. §8.1 does not name type 4; this work adds it as a toll on the evidence recorded in the spec.
- `exit_cost` keeps its existing behaviour in this plan. Do not change routing here — it is part 2's job, and changing both at once makes a regression impossible to attribute.
- 45 pre-existing workspace failures are environmental. Judge by `cargo test -p mud-client`.

---

### Task 1: `ExitRequirement`, and the toll it makes visible

**Files:**
- Modify: `crates/mud-client/src/graph.rs` (`ExitEdge` ~line 29; `RoomGraph::load` ~line 214-306)
- Test: `crates/mud-client/tests/graph.rs` (exists; append)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum ExitRequirement` and the field `ExitEdge::requirement: ExitRequirement`. Task 2 extends the enum's construction; Task 3 reads `Hidden { searchable }`. The follow-on plan reads `Toll { gold }`.

- [ ] **Step 1: Write the failing test**

`crates/mud-client/tests/graph.rs` already exists and already has a `graph()`
`OnceLock` helper that loads the shipped database and a `db_path()` beside it.
Use them; do not add a second loader. Extend the file's `use` line to
`use mud_client::graph::{ExitRequirement, RoomGraph, DIRECTIONS};` and append:

```rust
fn requirement(g: &RoomGraph, room: RoomId, dir: Direction) -> ExitRequirement {
    let i = DIRECTIONS.iter().position(|d| *d == dir).expect("compass");
    g.room(room)
        .and_then(|r| r.exits[i].as_ref())
        .map(|e| e.requirement.clone())
        .unwrap_or(ExitRequirement::None)
}

/// The Silvermere gates charge to pass. Both directions carry the same
/// `roomtype = 4, para1 = 5` in the data; whether both actually CHARGE is
/// an engine question this plan does not answer.
#[test]
fn the_silvermere_gate_is_a_toll() {
    let g = graph();
    let inner = RoomId { map: 1, room: 1381 }; // Town Gates, Inner Bailey
    let road = RoomId { map: 1, room: 1382 };  // Main Road, Silvermere Gates
    assert_eq!(
        requirement(g, inner, Direction::East),
        ExitRequirement::Toll { gold: 5 }
    );
    assert_eq!(
        requirement(g, road, Direction::West),
        ExitRequirement::Toll { gold: 5 }
    );
}

/// Every type-4 exit in the shipped world is a toll and no other type is.
/// 57 of them, and their amounts are a currency distribution -- which is
/// the evidence type 4 was identified from in the first place.
#[test]
fn every_toll_in_the_world_is_a_type_four_exit() {
    let g = graph();
    let mut amounts: Vec<u32> = Vec::new();
    for (_, room) in g.iter() {
        for edge in room.exits.iter().flatten() {
            match (&edge.requirement, edge.exit_type) {
                (ExitRequirement::Toll { gold }, 4) => amounts.push(*gold),
                (ExitRequirement::Toll { .. }, t) => {
                    panic!("a toll on exit type {t}, which is not 4")
                }
                (_, 4) => panic!("a type-4 exit that is not a toll"),
                _ => {}
            }
        }
    }
    amounts.sort_unstable();
    assert_eq!(amounts.len(), 57, "57 shipped type-4 exits");
    let distinct: std::collections::BTreeSet<u32> = amounts.iter().copied().collect();
    assert_eq!(
        distinct,
        [0, 5, 5000, 10000].into_iter().collect(),
        "a currency distribution, not room or message ids"
    );
}

/// The Marble Chamber candle is NOT a puzzle: it is one of 250 plain
/// command exits, and the walker already speaks those. Guards against
/// over-classifying narrative dressing as mechanism.
#[test]
fn the_crypt_candle_is_an_ordinary_command_exit() {
    let g = graph();
    let chamber = RoomId { map: 1, room: 2252 };
    assert_eq!(
        requirement(g, chamber, Direction::North),
        ExitRequirement::Command
    );
    let i = DIRECTIONS
        .iter()
        .position(|d| *d == Direction::North)
        .unwrap();
    let edge = g.room(chamber).and_then(|r| r.exits[i].as_ref()).unwrap();
    assert_eq!(edge.command.as_deref(), Some("turn right candle left"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test graph`

Expected: FAIL to compile — `ExitRequirement` does not exist and `ExitEdge` has no `requirement` field.

- [ ] **Step 3: Define the enum**

In `crates/mud-client/src/graph.rs`, above `ExitEdge`:

```rust
/// What an exit requires of whoever walks it.
///
/// Computed once at load from `roomtype`, the `para*` slots and the
/// room's `cmdtext` script, so that routing and walking share one
/// answer rather than each re-deriving it from a bare type number.
///
/// The taxonomy is ours, from `re/docs/theft.md` §8.1 plus the quest
/// VM's `remoteaction` verb. It deliberately says what an exit NEEDS and
/// not what it costs: cost depends on who is walking, and lives in
/// `exit_cost`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitRequirement {
    /// Nothing. Walk it.
    None,
    /// A door or gate: `open`, then possibly a bash chain. Types 2, 7, 0xb.
    Door,
    /// Concealed. `searchable` is true for the ordinary SEARCH-revealed
    /// exit and FALSE when the concealment is a puzzle bit-word that no
    /// search roll can clear -- see [`ExitRequirement::Puzzle`]. Type 6.
    Hidden { searchable: bool },
    /// A trap on the exit. The walk has no DISARM. Types 9, 0x18.
    Trap,
    /// Not walked but spoken: the phrase is on [`ExitEdge::command`].
    /// Type 10, 250 of them.
    Command,
    /// Costs money to pass. `gold` is in GOLD CROWNS, the unit `para1`
    /// carries. Type 4.
    ///
    /// INFERRED unit, on two supports: the observed 5-gold Silvermere
    /// toll matches `para1 = 5`, and map 17's `para1 = 10000` is exactly
    /// 100 platinum = 1 runic coin. Proof would come from `move_user` in
    /// the WCCMMUD decompile, which nobody has read. The conversion to
    /// the client's base unit lives in exactly one place so that a
    /// correction is a one-line change.
    Toll { gold: u32 },
    /// Gated on alignment, a known spell, or an ability. Types 0x14,
    /// 0x16, 0x17. Not yet distinguished from one another: the walk can
    /// satisfy none of them today, so one variant is as actionable as
    /// three.
    Gate,
    /// Opens and shuts on a timer of its own. Type 0x10, 2 of them.
    Timed,
    /// Concealed by a bit-word that `remoteaction` scripts clear. Filled
    /// in by the cmdtext pass; see the `Puzzle` task.
    Puzzle { actions: Vec<PuzzleAction> },
}
```

Add the field to `ExitEdge`:

```rust
    /// What this exit requires of whoever walks it.
    pub requirement: ExitRequirement,
```

`PuzzleAction` is defined in Task 2. For this task, define it as an empty placeholder so the enum compiles — **not** a stub to be left behind, but the type Task 2 fills:

```rust
/// One action that clears one bit of a puzzle exit's concealment word.
/// Populated by the cmdtext pass in the next task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PuzzleAction {
    /// The room the phrase must be spoken in.
    pub room: RoomId,
    /// The phrases that satisfy this step; any one of them.
    pub commands: Vec<String>,
}
```

- [ ] **Step 4: Classify at load**

First give the classification a name of its own, so that a test fixture cannot
drift from what `load` does. Add beside the enum:

```rust
impl ExitRequirement {
    /// The requirement an exit type implies on its own, before the
    /// cmdtext pass gets a say.
    ///
    /// Shared with the navigator's test fixtures deliberately. Those
    /// fixtures build exits by type and care about the WALKER's
    /// behaviour, so they must classify exactly as `load` does or they
    /// test a world that cannot exist. The tests that pin the
    /// classification itself do NOT call this — they assert
    /// hand-written expectations against the shipped database, so they
    /// can still fail when this is wrong.
    pub fn from_exit_type(exit_type: i64, para1: i64) -> ExitRequirement {
        match exit_type {
            2 | 7 | 0xb => ExitRequirement::Door,
            // Searchable until the cmdtext pass proves otherwise.
            6 => ExitRequirement::Hidden { searchable: true },
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
}
```

Then in `RoomGraph::load`'s per-direction loop, after `exit_type` and `para1` are read and before the `ExitEdge` is built, add:

```rust
                let requirement = ExitRequirement::from_exit_type(exit_type, para1);
```

For reference, that classification is:

```rust
                // match exit_type {
                    2 | 7 | 0xb => ExitRequirement::Door,
                    // Searchable until the cmdtext pass proves otherwise.
                    6 => ExitRequirement::Hidden { searchable: true },
                    9 | 0x18 => ExitRequirement::Trap,
                    COMMAND_EXIT => ExitRequirement::Command,
                    4 => ExitRequirement::Toll {
                        gold: u32::try_from(para1).unwrap_or(0),
                    },
                    0x14 | 0x16 | 0x17 => ExitRequirement::Gate,
                    0x10 => ExitRequirement::Timed,
                //     _ => ExitRequirement::None,
                // }
```

and set it on the constructed edge:

```rust
                graph_room.exits[d] = Some(ExitEdge {
                    dest: RoomId {
                        map: dmap,
                        room: dest,
                    },
                    exit_type,
                    command: (exit_type == COMMAND_EXIT)
                        .then(|| commands.get(&para1).cloned())
                        .flatten(),
                    requirement,
                });
```

**The fixture trap, and it is load-bearing.** `crates/mud-client/tests/nav_hidden.rs::graph_with_exit` builds `ExitEdge { dest, exit_type, command: None }` as an explicit literal with no `..Default::default()`. If you add `requirement: ExitRequirement::None` there, its type-6 fixture stops being a hidden exit, and Task 3 will make `a_hidden_exit_is_searched_until_it_is_found` fail for a reason that has nothing to do with the change. Fix that one by classifying:

```rust
    here.exits[Direction::South as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type,
        command: None,
        requirement: ExitRequirement::from_exit_type(exit_type, 0),
    });
```

and the same for the `there` edge. Any other explicit literal whose `exit_type` is non-zero needs the same treatment; literals that use `..Default::default()` are fine.

Rather than editing dozens of them, add a `Default` impl next to the struct:

```rust
impl Default for ExitRequirement {
    fn default() -> Self {
        ExitRequirement::None
    }
}
```

and check whether `ExitEdge` already derives `Default` (it does — `#[derive(Debug, Clone, PartialEq, Eq, Default)]`). Test literals that use `..Default::default()` then keep compiling. For literals that list every field explicitly, add `requirement: ExitRequirement::None`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test graph`

Expected: the toll tests PASS. `the_crypt_candle_is_an_ordinary_command_exit` PASSES.

- [ ] **Step 6: Mutate to prove the census bites**

Change `4 => ExitRequirement::Toll { ... }` to `4 => ExitRequirement::None`. Re-run.

Expected: `the_silvermere_gate_is_a_toll` FAILS and `every_toll_in_the_world_is_a_type_four_exit` FAILS on "a type-4 exit that is not a toll". Revert.

Then change the amount to `gold: 5` unconditionally. Expected: the distribution assertion FAILS. Revert. This is the check that matters — a fixture computing the expected value from the code under test could not fail, and the distinct-amounts set is written from the measurement, not from the implementation.

- [ ] **Step 7: Run the whole crate's tests**

Run: `cargo test -p mud-client`

Expected: PASS. Routing is untouched — `exit_cost` still reads `exit_type` and still prices type 4 as a plain step. That is deliberate and is part 2's job.

- [ ] **Step 8: Commit**

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/tests/graph.rs
git commit -m "feat(graph): give every exit a requirement descriptor

roomtype 4 is a toll and para1 is what it demands. All 57 shipped type-4
exits carry para1 in {0, 5, 5000, 10000} -- a currency distribution and
nothing else -- and para2 is 0 on 56 of them, so it is not a
denomination selector. theft.md 8.1 does not name type 4; this is the
evidence that closes it.

Nothing routes on this yet: exit_cost still reads the bare type and
still prices a toll as a free step. Changing both at once would make a
regression impossible to attribute."
```

---

### Task 2: The cmdtext pass — which hidden exits no search can reveal

**Files:**
- Modify: `crates/mud-client/src/graph.rs` (`RoomGraph::load`; new `load_remote_actions`)
- Test: `crates/mud-client/tests/graph.rs`

**Interfaces:**
- Consumes: `ExitRequirement`, `PuzzleAction` from Task 1.
- Produces: exits whose requirement is `Hidden { searchable: false }` or `Puzzle { actions }`. Task 3 reads the former.

- [ ] **Step 1: Write the failing test**

Append to `crates/mud-client/tests/graph.rs`:

```rust
/// 17/3042's north passage is concealed by a bit-word that four
/// `remoteaction` scripts clear -- one per gem, in four other rooms. No
/// number of SEARCH rolls can ever reveal it, so the walker must not try.
#[test]
fn the_gem_passage_is_concealed_by_a_puzzle_not_by_search() {
    let g = graph();
    let shadowy = RoomId { map: 17, room: 3042 };
    match requirement(g, shadowy, Direction::North) {
        ExitRequirement::Hidden { searchable } => {
            assert!(!searchable, "no search roll can clear a bit-word")
        }
        other => panic!("expected a hidden exit, got {other:?}"),
    }
}

/// The ordinary hidden exit is still searchable. Without this the fix
/// would be "never search anything", which strands the 1,383 shipped
/// hidden exits that SEARCH genuinely does reveal.
#[test]
fn an_ordinary_hidden_exit_stays_searchable() {
    let g = graph();
    let mut searchable = 0usize;
    let mut puzzle_locked = 0usize;
    for (_, room) in g.iter() {
        for edge in room.exits.iter().flatten() {
            if let ExitRequirement::Hidden { searchable: s } = edge.requirement {
                if s {
                    searchable += 1
                } else {
                    puzzle_locked += 1
                }
            }
        }
    }
    assert!(
        searchable > 1000,
        "most of the 1,383 hidden exits are genuinely searchable, got {searchable}"
    );
    assert!(
        puzzle_locked > 0,
        "at least the gem passage is puzzle-locked, got {puzzle_locked}"
    );
    assert!(
        puzzle_locked < 50,
        "only a handful of exits are puzzle-locked, got {puzzle_locked}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mud-client --test graph the_gem_passage_is_concealed_by_a_puzzle_not_by_search`

Expected: FAIL — `searchable` is still `true`, because nothing reads `cmdtext` yet.

- [ ] **Step 3: Write the loader**

Add to `impl RoomGraph`, beside `load_exit_commands`:

```rust
    /// Which exits are opened by a `remoteaction` script somewhere.
    ///
    /// The quest VM's `remoteaction <room> <msg> <action> <exit>` verb
    /// clears one bit of a concealment word on the TARGET room's exit --
    /// the actor may be standing somewhere else entirely. An exit
    /// concealed this way answers SEARCH exactly as an ordinary hidden
    /// exit does and can never be revealed by one, so the walker has to
    /// be able to tell them apart.
    ///
    /// Returns target `(room, exit index)` -> the actions that open it,
    /// keyed by the exit's 0-based direction index.
    ///
    /// 28 rooms in the shipped world carry such a script, between them
    /// 82 directives. `mud-core`'s `remote_lever` parses the same
    /// grammar, so the two must agree.
    fn load_remote_actions(
        conn: &rusqlite::Connection,
    ) -> Result<BTreeMap<(RoomId, usize), Vec<PuzzleAction>>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT r.mapnumber, r.roomnumber, t.body \
                 FROM room r JOIN textblock t ON t.number = r.cmdtext \
                 WHERE r.cmdtext > 0 AND t.body LIKE '%remoteaction%'",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut out: BTreeMap<(RoomId, usize), Vec<PuzzleAction>> = BTreeMap::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let map: i64 = row.get(0).map_err(|e| e.to_string())?;
            let actor_room: i64 = row.get(1).map_err(|e| e.to_string())?;
            let body: Option<String> = row.get(2).map_err(|e| e.to_string())?;
            let Some(body) = body else { continue };
            let (Ok(map16), Ok(actor16)) = (u16::try_from(map), u16::try_from(actor_room)) else {
                continue;
            };
            let actor = RoomId {
                map: map16,
                room: actor16,
            };
            // One script per line; the phrase is the head, the verbs
            // follow, colon-separated. A line may hold several phrases
            // for the same effect ("clear rubble" / "move rubble").
            for line in body.split(['\r', '\n']).filter(|l| !l.trim().is_empty()) {
                let mut parts = line.split(':');
                let Some(phrase) = parts.next().map(str::trim) else {
                    continue;
                };
                if phrase.is_empty() {
                    continue;
                }
                for verb in parts {
                    let mut w = verb.split_whitespace();
                    if w.next() != Some("remoteaction") {
                        continue;
                    }
                    let nums: Vec<i64> = w.filter_map(|n| n.parse().ok()).collect();
                    // remoteaction <room> <msg> <action> <exit>
                    let [target, _msg, _action, exit] = nums[..] else {
                        continue;
                    };
                    let (Ok(target), Ok(exit)) = (u16::try_from(target), usize::try_from(exit))
                    else {
                        continue;
                    };
                    if exit > 9 {
                        continue;
                    }
                    let key = (
                        RoomId {
                            map: map16,
                            room: target,
                        },
                        exit,
                    );
                    let entry = out.entry(key).or_default();
                    match entry.iter_mut().find(|a| a.room == actor) {
                        Some(a) => a.commands.push(phrase.to_string()),
                        None => entry.push(PuzzleAction {
                            room: actor,
                            commands: vec![phrase.to_string()],
                        }),
                    }
                }
            }
        }
        Ok(out)
    }
```

- [ ] **Step 4: Apply it in `load`**

Beside the existing `let commands = Self::load_exit_commands(&conn)?;` add:

```rust
        let remote_actions = Self::load_remote_actions(&conn)?;
```

Then, after the whole `rooms` map is built and before `Ok(RoomGraph { rooms })`, mark the targets:

```rust
        // Second pass: an exit a `remoteaction` opens is concealed by a
        // bit-word, not by a search roll. Done after the rooms are all
        // read because the actor and the target are different rooms and
        // the target may be read first.
        for ((target, exit), actions) in remote_actions {
            let Some(room) = rooms.get_mut(&target) else {
                continue;
            };
            let Some(edge) = room.exits[exit].as_mut() else {
                continue;
            };
            edge.requirement = match &edge.requirement {
                ExitRequirement::Hidden { .. } => ExitRequirement::Hidden { searchable: false },
                // A gate or door a lever throws is still a gate; the
                // actions are what opens it.
                _ => ExitRequirement::Puzzle { actions },
            };
        }
```

Note the asymmetry deliberately: a **hidden** target becomes `Hidden { searchable: false }` because that is what stops the search hang, which is this plan's success criterion. Carrying its actions as well is the follow-on plan's concern, when something can act on them.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p mud-client --test graph`

Expected: all PASS.

- [ ] **Step 6: Cross-check the parse against a second reader**

Print what the loader found and compare with the spec's table by eye:

```bash
cd ~/bbs && sqlite3 re/mmud_wgnt.sqlite "
  select r.mapnumber||'/'||r.roomnumber, replace(t.body, char(13), ' | ')
  from room r join textblock t on t.number = r.cmdtext
  where r.cmdtext > 0 and t.body like '%remoteaction%';" | head -40
```

Confirm the four map-17 gem rooms (3037, 3039, 3041, 3043) all target 3042 exit 0, and that `12/2118` targets `2122`. If the loader's count of puzzle-locked exits is far from a handful, the parse is wrong — say so rather than loosening the assertion.

- [ ] **Step 7: Run the whole crate's tests, then commit**

Run: `cargo test -p mud-client`

```bash
git add crates/mud-client/src/graph.rs crates/mud-client/tests/graph.rs
git commit -m "feat(graph): tell a puzzle-concealed exit from a searchable one

The quest VM's remoteaction verb clears one bit of a concealment word on
another room's exit. 17/3042's north passage is opened that way -- four
gems in four Shadowy Passage rooms -- and answers SEARCH exactly as an
ordinary hidden exit does while being impossible to reveal with one.

28 rooms carry such a script, 82 directives between them. mud-core's
remote_lever parses the same grammar, so the two have to agree."
```

---

### Task 3: The walker stops searching for what it cannot find

**Files:**
- Modify: `crates/mud-client/src/nav.rs` (line ~622 where `exit_type` is derived; the parameter at ~872; the dispatch at ~909)
- Test: `crates/mud-client/tests/nav_hidden.rs`

**Interfaces:**
- Consumes: `ExitRequirement::Hidden { searchable }` from Tasks 1-2.
- Produces: no public API change.

- [ ] **Step 1: Write the failing test**

`crates/mud-client/tests/nav_hidden.rs` already has the harness: `alley_board(reveal_on)` stands up a board that counts SEARCHes in a `SearchLog`, `graph_with_exit(exit_type)` builds the two-room fixture, and `a_refused_plain_exit_is_never_searched` is the exact shape to copy. Add beside `graph_with_exit`:

```rust
/// The alleyway fixture, but the hidden exit is concealed by a puzzle
/// bit-word rather than by a search roll — 17/3042's north passage.
fn graph_with_puzzle_exit() -> Arc<RoomGraph> {
    let g = graph_with_exit(6);
    let mut rooms: Vec<(RoomId, GraphRoom)> = g.iter().map(|(id, r)| (id, r.clone())).collect();
    for (_, room) in rooms.iter_mut() {
        for edge in room.exits.iter_mut().flatten() {
            edge.requirement = ExitRequirement::Hidden { searchable: false };
        }
    }
    Arc::new(RoomGraph::from_rooms(rooms))
}
```

and the test:

```rust
/// A type-6 exit whose concealment is a bit-word cannot be revealed by
/// any number of SEARCH rolls, so the walker must not spend any. It used
/// to search until something else interrupted it.
#[tokio::test]
async fn a_puzzle_concealed_exit_is_never_searched() {
    // reveal_on = usize::MAX: this board never yields to a search, which
    // is exactly what the real one does here.
    let (addr, log) = alley_board(usize::MAX).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_puzzle_exit());

    let result = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");

    assert!(result.is_err(), "the passage is shut; the walk cannot succeed");
    assert_eq!(
        log.searches.load(Ordering::SeqCst),
        0,
        "no search roll can clear a bit-word: not one may be spent"
    );
}
```

`GraphRoom` already derives `Clone` (`graph.rs:192`), so this compiles as written.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mud-client --test nav_hidden a_puzzle_concealed_exit_is_never_searched`

Expected: FAIL on the search count — the client spends its whole roll budget, because the dispatch tests only `is_hidden(exit_type)` and a puzzle-concealed exit is still type 6.

- [ ] **Step 3: Carry searchability down to the dispatch**

At `nav.rs:622`, beside the existing derivation:

```rust
                let exit_type = edge.map(|e| e.exit_type).unwrap_or(0);
```

add:

```rust
                // A type-6 exit concealed by a puzzle bit-word answers
                // SEARCH exactly as a searchable one does and can never
                // be revealed by it, so the graph has to say which this
                // is. Searching a puzzle exit is unbounded: the roll can
                // never succeed.
                let searchable_hidden = edge
                    .map(|e| {
                        matches!(
                            e.requirement,
                            crate::graph::ExitRequirement::Hidden { searchable: true }
                        )
                    })
                    .unwrap_or(false);
```

Thread `searchable_hidden: bool` as a parameter alongside the existing `exit_type: i64` (declared at ~line 872), passing it from the call site at ~line 650 where `exit_type` is already passed.

Change the dispatch at ~line 909 from:

```rust
            StepEvent::NoSuchExit if self.search_hidden && is_hidden(exit_type) => {
```

to:

```rust
            StepEvent::NoSuchExit if self.search_hidden && searchable_hidden => {
```

Leave `is_hidden` itself in place: the MegaMud room-id codec and other callers still want the type predicate, and its doc comment already explains why it is public.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p mud-client --test nav_hidden`

Expected: PASS, all seven — the new one plus the six that were there. `a_hidden_exit_is_searched_until_it_is_found` passing is the half of this that proves the fix is narrow.

- [ ] **Step 5: Mutate**

Change `searchable_hidden` to `true` unconditionally. Expected: `a_puzzle_concealed_exit_is_never_searched` FAILS. Change it to `false` unconditionally. Expected: `a_hidden_exit_is_searched_until_it_is_found` FAILS. **Both must bite.** If only one does, the pair cannot tell a searchable exit from a puzzle-locked one and the fix is indistinguishable from "never search anything". Revert.

- [ ] **Step 6: Run the whole crate's tests, then commit**

Run: `cargo test -p mud-client`

```bash
git add crates/mud-client/src/nav.rs crates/mud-client/tests/nav_hidden.rs
git commit -m "fix(nav): never search an exit no search can reveal

nav dispatched SEARCH on any type-6 exit. 17/3042's north passage is
type 6 and is concealed by a bit-word four remoteaction scripts clear,
so the roll can never succeed and the walker searched without bound --
the same shape as the 2026-08-02 Silvermere alleyway incident recorded
in exit_cost's doc comment, except that one merely cost steps.

is_hidden stays: the MegaMud room-id codec wants the type predicate."
```

---

## Self-Review

**Spec coverage.** `ExitRequirement` taxonomy → Task 1. `Puzzle` built from the cmdtext pass → Task 2. Success criterion 2 ("the walker never sends SEARCH at a puzzle-concealed exit") → Task 3. Spec test 4 (the candle still routes as a command exit) → Task 1's third test. Spec test 5 (`12/2118` → `12/2122` cross-room) and test 6 (four gem actions, order-free) → **partially covered**: Task 2's loader builds the actions and step 6 cross-checks them by eye, but no assertion pins the four-action shape, because nothing consumes it until the follow-on plan. That plan must add those assertions when it adds the consumer; flagged here so it is not lost.

**Deferred to the follow-on plan, by design:** success criteria 1, 3 and 4 (state-dependent routing, named refusals, expensive-vs-impossible), the purse, `Capabilities`, `exit_cost`'s new signature, and toll-direction learning. `exit_cost` is deliberately untouched here.

**Placeholder scan.** None. Every step carries its code, including Task 3's, which builds on `nav_hidden.rs`'s existing `alley_board` / `SearchLog` / `graph_with_exit` harness rather than inventing a second one. `GraphRoom` derives `Clone` (`graph.rs:192`), so Task 3's fixture helper compiles as written.

**The fixture-divergence trap is covered.** `ExitRequirement::from_exit_type` is shared between `load` and the navigator's fixtures on purpose — a fixture that classified differently would test a world that cannot exist. The tests that pin the classification itself (Task 1) do not call it; they assert hand-written expectations against the shipped database, so they can still fail when the classifier is wrong. That split is what keeps the sharing honest.

**Type consistency.** `ExitRequirement` variants used in Tasks 2 and 3 (`Hidden { searchable }`, `Puzzle { actions }`, `Toll { gold }`, `Command`) are all defined in Task 1. `PuzzleAction { room, commands }` is defined in Task 1 and constructed in Task 2 with those exact field names. `load_remote_actions` returns `BTreeMap<(RoomId, usize), Vec<PuzzleAction>>` and is consumed at that type in Task 2 step 4.
