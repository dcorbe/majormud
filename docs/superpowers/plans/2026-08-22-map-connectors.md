# Honest map connectors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** LANDED on main 2026-08-22, commits `ee6a9fd1..5cb44f3e` (8 commits).
All 4 tasks implemented, task-reviewed, and passed a whole-branch review (verdict:
ship). Merged `--ff-only`. The unticked checkboxes below are an artefact of
execution: implementers worked from extracted per-task briefs, not this file.
Two residuals parked — see the commit messages and
`a-comment-can-lie-about-which-layer-a-test-covers` in memory.

**Goal:** Make every line the map draws correspond to an exit that exists, and return the arrow keys to moving one cell per press.

**Architecture:** `layout()` already walks every exit and already decides whether the destination landed in the cell it wanted. Record that fact in a set on `Plane` at the moment it is discovered, and have `connectors()` read the set instead of inferring links from grid adjacency. `render()` stays a pure function of `Plane`, so no caller needs a `&RoomGraph`.

**Tech Stack:** Rust, `cargo test -p mud-client`, `std::collections::BTreeSet`.

**Spec:** `docs/superpowers/specs/2026-08-22-map-connectors-design.md`

## Global Constraints

- **Never run `cargo fmt`, `rustfmt`, or any formatter.** Project-wide rule.
- Every file ends with a blank line.
- Commit after every task, with a tag prefix (`feat:`, `fix:`, `refactor:`, `doc:`, `test:`).
- Run the whole crate's tests, not just the new one: `cargo test -p mud-client`.
- `re/mmud_wgnt.sqlite` is a 119 MB gitignored fixture, present in the main checkout and auto-populated in worktrees. `tests/map.rs` already assumes it via its `graph()` `OnceLock` helper (`loads_all_rooms` asserts 26,720 rooms), so follow that convention — do not add a skip path this file does not have.
- `Direction` and `RoomId` both derive `Ord` (`crates/mud-core/src/content.rs:13,47`), so they are valid `BTreeSet` keys.
- 45 pre-existing workspace test failures are environmental and unrelated to this work. Judge success by `-p mud-client` only.

---

### Task 1: `Plane` records the edges that actually exist

**Files:**
- Modify: `crates/mud-client/src/map.rs` (struct `Plane` ~line 99; `layout()` ~line 172-225)
- Test: `crates/mud-client/tests/map.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Plane::has_edge(&self, room: RoomId, dir: Direction) -> bool`. Task 2 calls this and nothing else from this task.

- [ ] **Step 1: Write the failing test**

Add to the end of `crates/mud-client/tests/map.rs`:

```rust
// --- the plane records real exits, not grid adjacency -----------------

/// `A --east--> B` and `A --southeast--> D`. That places B at (1,0) and D
/// at (1,1), which makes B and D vertically adjacent on the grid — with
/// nothing at all joining them. The plane must know the difference.
#[test]
fn the_plane_records_exits_and_not_adjacency() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let d = RoomId { map: 1, room: 4 };
    let mut ra = GraphRoom {
        name: "A".into(),
        ..Default::default()
    };
    ra.exits[Direction::East as usize] = Some(ExitEdge {
        dest: b,
        exit_type: 0,
        command: None,
    });
    ra.exits[Direction::SouthEast as usize] = Some(ExitEdge {
        dest: d,
        exit_type: 0,
        command: None,
    });
    let rooms = vec![
        (a, ra),
        (
            b,
            GraphRoom {
                name: "B".into(),
                ..Default::default()
            },
        ),
        (
            d,
            GraphRoom {
                name: "D".into(),
                ..Default::default()
            },
        ),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);

    assert!(plane.has_edge(a, Direction::East), "A really does go east");
    assert!(
        plane.has_edge(a, Direction::SouthEast),
        "A really does go south-east"
    );

    // B is at (1,0) and D at (1,1) — adjacent, and unconnected.
    assert_eq!(plane.cell_of(b), Some((1, 0)));
    assert_eq!(plane.cell_of(d), Some((1, 1)));
    assert!(
        !plane.has_edge(b, Direction::South),
        "nothing joins B to D; adjacency is not an exit"
    );
    assert!(
        !plane.has_edge(d, Direction::North),
        "and nothing joins D back to B"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mud-client --test map the_plane_records_exits_and_not_adjacency`

Expected: FAIL to compile — `no method named `has_edge` found for struct `Plane``.

- [ ] **Step 3: Add the field and the accessor**

In `crates/mud-client/src/map.rs`, add a field to `struct Plane` (it is a private field; the struct already holds `cells`, `at`, `extent`, `conflicts`, `links`):

```rust
    /// Every exit that both EXISTS in the graph and joins two rooms this
    /// plane placed in adjacent cells — recorded here at layout time,
    /// where the exits are already being walked.
    ///
    /// The renderer used to re-derive this at draw time by asking
    /// whether a room happened to sit in the neighbouring cell, which
    /// draws a line between any two rooms the grid puts side by side
    /// whether or not anything joins them. Keeping the answer here is
    /// what lets `render` stay a pure function of `Plane` — the
    /// alternative was threading a `&RoomGraph` through `render` and
    /// every one of its callers.
    ///
    /// Up and down are absent by construction: they never take a cell in
    /// the plane, so they land in `links` instead.
    edges: BTreeSet<(RoomId, Direction)>,
```

Add the accessor inside `impl Plane`, next to `room_at`:

```rust
    /// Does a real exit leave `room` in `dir` toward the room the plane
    /// placed in the adjacent cell?
    ///
    /// False for an exit whose destination ended up somewhere else (a
    /// [`Conflict`]) — that line would point at the wrong room.
    pub fn has_edge(&self, room: RoomId, dir: Direction) -> bool {
        self.edges.contains(&(room, dir))
    }
```

Initialise it in `layout()`'s constructor literal, alongside `links: Vec::new()`:

```rust
        edges: BTreeSet::new(),
```

Then record it in the two arms of `layout()`'s `match plane.at.get(&edge.dest)` where the destination genuinely occupies `want`. The `Some(&there)` arm becomes:

```rust
                Some(&there) => {
                    if there != want {
                        plane.conflicts.push(Conflict {
                            from,
                            dir,
                            dest: edge.dest,
                            cell: want,
                        });
                    } else {
                        plane.edges.insert((from, dir));
                    }
                }
```

and the placing arm becomes:

```rust
                None => {
                    plane.cells.insert(want, edge.dest);
                    plane.at.insert(edge.dest, want);
                    plane.edges.insert((from, dir));
                    queue.push_back(edge.dest);
                }
```

Leave the `None if plane.cells.contains_key(&want)` arm alone: that is a conflict, the destination is not in that cell, and no line should be drawn.

`BTreeSet` is already imported at the top of `map.rs` (`use std::collections::{BTreeMap, BTreeSet, ...}`); confirm it, and add it to the `use` if it is not.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mud-client --test map the_plane_records_exits_and_not_adjacency`

Expected: PASS.

- [ ] **Step 5: Mutate to prove the test can fail**

Temporarily change the `Some(&there)` arm's `else` to record unconditionally:

```rust
                    }
                    plane.edges.insert((from, dir));
```

Run the test again. Expected: still PASS — which tells you this mutation is not what the test catches. Now instead make `has_edge` return `self.cells.contains_key(&(0, 0)) || self.edges.contains(&(room, dir))`. Re-run: expected FAIL on the `!plane.has_edge(b, ...)` assertion. Revert both mutations.

Record in the commit message that the first mutation survived: it means conflict-vs-agreement is **not** yet covered, and Task 3's census is what covers it.

- [ ] **Step 6: Run the whole crate's tests**

Run: `cargo test -p mud-client`

Expected: PASS, unchanged from before this task — nothing reads `edges` yet.

- [ ] **Step 7: Commit**

```bash
git add crates/mud-client/src/map.rs crates/mud-client/tests/map.rs
git commit -m "feat(map): record real exits on the plane at layout time

layout() already walks every exit and already decides whether the
destination landed in the cell it wanted. Keep that answer in a set on
Plane rather than making the renderer guess it back from grid adjacency
later. Conflicts are excluded, correctly: a conflicting exit's
destination is not in that cell, so a line drawn to it would point at
the wrong room.

Nothing reads it yet. Noted for the record: mutating the agreement check
to record unconditionally does NOT fail this test -- only the census in
a later task covers conflict-vs-agreement."
```

---

### Task 2: `connectors()` draws only recorded edges

**Files:**
- Modify: `crates/mud-client/src/map.rs` (`connectors()` ~line 640-684; its call site ~line 534)
- Test: `crates/mud-client/tests/map.rs`

**Interfaces:**
- Consumes: `Plane::has_edge(room, dir) -> bool` from Task 1.
- Produces: no new public API. `connectors` becomes `fn connectors(plane: &Plane, buf: &mut [Vec<(char, Ink)>], room: RoomId, at: (i64, i64), zoom: Zoom, fg: &'static str)` — note the third parameter changes from `cell: Cell` to `room: RoomId`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/mud-client/tests/map.rs`:

```rust
/// Four rooms in a square, joined ONLY by orthogonal exits. Not one
/// diagonal exists, so not one diagonal may be drawn — and the crossed
/// glyph, which is two diagonals, least of all.
#[test]
fn a_square_loop_with_no_diagonal_exits_draws_no_diagonal() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let c = RoomId { map: 1, room: 3 };
    let d = RoomId { map: 1, room: 4 };
    let mk = |name: &str, exits: Vec<(Direction, RoomId)>| {
        let mut r = GraphRoom {
            name: name.into(),
            ..Default::default()
        };
        for (dir, dest) in exits {
            r.exits[dir as usize] = Some(ExitEdge {
                dest,
                exit_type: 0,
                command: None,
            });
        }
        r
    };
    let rooms = vec![
        (a, mk("A", vec![(Direction::East, b), (Direction::South, c)])),
        (b, mk("B", vec![(Direction::West, a), (Direction::South, d)])),
        (c, mk("C", vec![(Direction::East, d), (Direction::North, a)])),
        (d, mk("D", vec![(Direction::West, c), (Direction::North, b)])),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);
    for zoom in [Zoom::Detail, Zoom::Normal] {
        let frame = drawn(&plane, zoom);
        for glyph in ['\u{2573}', '\u{2572}', '\u{2571}'] {
            assert!(
                !frame.contains(glyph),
                "{zoom:?} drew {glyph:?} for a square with no diagonal exits:\n{frame}"
            );
        }
    }
}

/// Two rooms the grid puts side by side with nothing joining them get no
/// line. `A --east--> B` and `A --southeast--> D` leave B and D
/// vertically adjacent and unconnected.
#[test]
fn adjacent_rooms_with_no_exit_between_them_are_not_joined() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let d = RoomId { map: 1, room: 4 };
    let mut ra = GraphRoom {
        name: "A".into(),
        ..Default::default()
    };
    ra.exits[Direction::East as usize] = Some(ExitEdge {
        dest: b,
        exit_type: 0,
        command: None,
    });
    ra.exits[Direction::SouthEast as usize] = Some(ExitEdge {
        dest: d,
        exit_type: 0,
        command: None,
    });
    let rooms = vec![
        (a, ra),
        (
            b,
            GraphRoom {
                name: "B".into(),
                ..Default::default()
            },
        ),
        (
            d,
            GraphRoom {
                name: "D".into(),
                ..Default::default()
            },
        ),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);
    let frame = drawn(&plane, Zoom::Normal);
    // Exactly one horizontal (A-B) and one diagonal (A-D). The vertical
    // that used to appear between B and D was never an exit.
    assert_eq!(
        frame.matches('\u{2502}').count(),
        0,
        "a vertical was drawn where no exit exists:\n{frame}"
    );
    assert_eq!(frame.matches('\u{2500}').count(), 1, "A-B:\n{frame}");
    assert_eq!(frame.matches('\u{2572}').count(), 1, "A-D:\n{frame}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test map a_square_loop_with_no_diagonal_exits_draws_no_diagonal adjacent_rooms_with_no_exit_between_them_are_not_joined`

Expected: both FAIL. The first reports a `╳` in the frame; the second reports a vertical count of 2.

- [ ] **Step 3: Change `connectors()` to read the edges**

Replace the head of the loop in `connectors()`. It currently reads:

```rust
    for (_dir, step) in COMPASS {
        if plane.room_at((cell.0 + step.0, cell.1 + step.1)).is_none() {
            continue;
        }
        let (dx, dy) = (step.0 as i64, step.1 as i64);
```

with:

```rust
    for (dir, step) in COMPASS {
        if !plane.has_edge(room, dir) {
            continue;
        }
        let (dx, dy) = (step.0 as i64, step.1 as i64);
```

Change the signature's third parameter from `cell: Cell` to `room: RoomId`, and delete the now-unused `cell` binding. The neighbour's position no longer needs looking up: an edge is only recorded when the destination occupies `cell + step`, so the geometry below is unchanged.

Update the doc comment on `connectors` to say what it now does:

```rust
/// Draw the connectors leaving one room.
///
/// A connector is drawn for an exit the plane RECORDED (`Plane::edges`),
/// never for a room that merely happens to occupy the neighbouring cell.
/// Grid adjacency is not connectivity: the layout packs rooms onto a
/// square grid, so unrelated rooms end up side by side constantly, and
/// drawing those was 47% of every line on the map.
```

At the call site (~line 534) change:

```rust
                connectors(plane, &mut buf, (gx, gy), (col, row), zoom, style.sgr);
```

to:

```rust
                connectors(plane, &mut buf, id, (col, row), zoom, style.sgr);
```

`id` is already bound immediately above by `let Some(id) = plane.room_at((gx, gy))`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test map a_square_loop_with_no_diagonal_exits_draws_no_diagonal adjacent_rooms_with_no_exit_between_them_are_not_joined`

Expected: both PASS.

- [ ] **Step 5: Run the whole crate's tests and expect a KNOWN failure**

Run: `cargo test -p mud-client`

Expected: `two_diagonals_through_one_cell_are_drawn_crossed` now FAILS. This is correct and is fixed in Task 3 — that test builds a hub whose neighbours declare no exits and then requires a crossing, so the only thing that could ever have produced one is the adjacency bug. Do not "fix" it by reverting this task.

Note any other failures. `every_compass_direction_is_drawn_at_the_connector_zooms` and `overview_draws_no_connectors_rather_than_misleading_ones` use `spokes()`, which declares each exit on the hub, so they should stay green.

- [ ] **Step 6: Commit**

```bash
git add crates/mud-client/src/map.rs crates/mud-client/tests/map.rs
git commit -m "fix(map): draw a connector only where an exit exists

connectors() decided whether to draw a link by asking whether a room sat
in the adjacent grid cell, and never asked whether an exit joined them.
The room graph was available and unconsulted. On the plane the client
was displaying, 3,654 of 7,854 connectors were fabricated -- 47% -- and
212 rooms were drawn enclosed on all eight sides while having three
exits or fewer. 1/837 Graveyard, East of Tomb has exactly n/s/e and was
drawn boxed in.

two_diagonals_through_one_cell_are_drawn_crossed now fails: it asserts
the defect. Rebuilt in the next commit."
```

---

### Task 3: Rebuild the test that asserted the bug, and pin the world

**Files:**
- Modify: `crates/mud-client/tests/map.rs` (`two_diagonals_through_one_cell_are_drawn_crossed`)
- Test: `crates/mud-client/tests/map.rs`

**Interfaces:**
- Consumes: `Plane::has_edge` and the Task 2 renderer.
- Produces: nothing.

- [ ] **Step 1: Rebuild the crossing test around a real crossing**

Replace `two_diagonals_through_one_cell_are_drawn_crossed` entirely. The old body used `spokes(&[SouthEast, East])`, in which the neighbours declare nothing. Two rooms must each declare their own diagonal:

```rust
/// Both diagonals of a square share its centre character, so one used to
/// overwrite the other and was invisible everywhere. Crossed is the
/// honest answer — for a crossing that is REAL.
///
/// This test used to build a hub whose neighbours declared no exits at
/// all and then require a crossing to appear. The only thing that could
/// produce one was the adjacency bug, so the test asserted the defect it
/// was meant to guard. Both diagonals are now declared outright.
#[test]
fn two_real_diagonals_through_one_cell_are_drawn_crossed() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    // A(0,0) B(1,0) C(0,1) D(1,1). A goes south-east to D; B goes
    // south-west to C. The two links genuinely cross at the centre.
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let c = RoomId { map: 1, room: 3 };
    let d = RoomId { map: 1, room: 4 };
    let mk = |name: &str, exits: Vec<(Direction, RoomId)>| {
        let mut r = GraphRoom {
            name: name.into(),
            ..Default::default()
        };
        for (dir, dest) in exits {
            r.exits[dir as usize] = Some(ExitEdge {
                dest,
                exit_type: 0,
                command: None,
            });
        }
        r
    };
    let rooms = vec![
        (
            a,
            mk(
                "A",
                vec![(Direction::East, b), (Direction::SouthEast, d)],
            ),
        ),
        (b, mk("B", vec![(Direction::SouthWest, c)])),
        (c, mk("C", vec![])),
        (d, mk("D", vec![])),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);
    for zoom in [Zoom::Detail, Zoom::Normal] {
        let frame = drawn(&plane, zoom);
        assert!(
            frame.contains('\u{2573}'),
            "{zoom:?} lost one of two real crossing diagonals:\n{frame}"
        );
    }
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p mud-client --test map two_real_diagonals_through_one_cell_are_drawn_crossed`

Expected: PASS.

- [ ] **Step 3: Write the census that pins the whole shipped world**

Append to `crates/mud-client/tests/map.rs`:

```rust
/// The pin: over the shipped world, every connector the renderer would
/// draw is an exit that exists.
///
/// The example-based tests above each cover one shape. This covers the
/// shapes nobody thought of, which is where the original defect lived --
/// it survived a test section literally headed "connectors must not
/// lie" because every assertion there checked that a line was PRESENT
/// and none checked that an absent exit stayed unpainted.
#[test]
fn no_connector_in_the_shipped_world_is_fabricated() {
    // `graph()` is the file's existing OnceLock helper and already
    // expects the fixture to be present, as `loads_all_rooms` does.
    // Follow that convention rather than inventing a second one.
    let g = graph();
    // Anchored at Newhaven, Narrow Road -- the plane the client shows on
    // a stock login. layout() is a BFS from the anchor, so the placed
    // set depends on it; the anchor is named so the numbers are
    // reproducible.
    let plane = layout(g, RoomId { map: 1, room: 2146 });
    assert!(plane.len() > 1000, "expected a large plane, got {}", plane.len());

    let mut fabricated = Vec::new();
    let mut drawn = 0usize;
    for room in plane.rooms() {
        for dir in mud_client::graph::DIRECTIONS {
            let Some(step) = mud_client::map::step_of(dir) else {
                continue; // up/down leave the plane
            };
            if !plane.has_edge(room, dir) {
                continue;
            }
            drawn += 1;
            let cell = plane.cell_of(room).expect("placed");
            let neighbour = plane.room_at((cell.0 + step.0, cell.1 + step.1));
            let real = g
                .room(room)
                .and_then(|r| {
                    mud_client::graph::DIRECTIONS
                        .iter()
                        .position(|d| *d == dir)
                        .and_then(|i| r.exits[i].as_ref())
                })
                .map(|e| Some(e.dest) == neighbour)
                .unwrap_or(false);
            if !real {
                fabricated.push((room, dir));
            }
        }
    }
    assert!(drawn > 3000, "expected thousands of connectors, got {drawn}");
    assert!(
        fabricated.is_empty(),
        "{} of {drawn} connectors are not backed by an exit; first few: {:?}",
        fabricated.len(),
        &fabricated[..fabricated.len().min(5)]
    );
}
```

- [ ] **Step 4: Run the census**

Run: `cargo test -p mud-client --test map no_connector_in_the_shipped_world_is_fabricated -- --nocapture`

Expected: PASS. If it panics in `graph()` the sqlite fixture is missing — restore it before continuing; this is the task's whole deliverable.

- [ ] **Step 5: Mutate to prove the census bites**

In `connectors()`, temporarily restore the old adjacency condition in place of
the edge check — this is the exact defect being fixed, so it needs the room's
cell back for the duration of the mutation:

```rust
    let cell = plane.cell_of(room).expect("placed");
    for (dir, step) in COMPASS {
        let _ = dir;
        if plane.room_at((cell.0 + step.0, cell.1 + step.1)).is_none() {
            continue;
        }
```

Re-run the census.

Expected: FAIL, reporting thousands of fabricated connectors. This is the mutation that matters: it proves the census detects the exact defect this plan exists to fix, which the pre-existing tests did not. Revert the mutation.

Also re-run Task 1's mutation now (record `edges` unconditionally in the `Some(&there)` arm) and confirm the census FAILS on it — this is the conflict-vs-agreement coverage that Task 1 could not provide on its own. Revert.

- [ ] **Step 6: Run the whole crate's tests**

Run: `cargo test -p mud-client`

Expected: PASS, all green.

- [ ] **Step 7: Commit**

```bash
git add crates/mud-client/tests/map.rs
git commit -m "test(map): pin the shipped world against fabricated connectors

Rebuilds two_diagonals_through_one_cell_are_drawn_crossed so both rooms
declare their own diagonal -- the old fixture's neighbours declared no
exits, so the only thing that could draw a crossing was the adjacency
bug it was supposed to guard against.

Adds a census over the whole world anchored at 1/2146. Verified by
mutation: restoring the adjacency condition fails it with thousands of
fabricated connectors, and recording an edge for a conflicting exit
fails it too."
```

---

### Task 4: Arrow keys move one cell again

**Files:**
- Modify: `crates/mud-client/src/mapview.rs` (`step()` ~line 395-433)
- Modify: `crates/mud-client/tests/mapview.rs` (four snap tests, from ~line 85)
- Test: `crates/mud-client/tests/mapview.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks. This task is independent of Tasks 1-3 and may be done first if preferred.
- Produces: no API change. `MapView::step(&mut self, dx: i32, dy: i32)` keeps its signature; only its body changes.

- [ ] **Step 1: Delete the four tests that assert the snap**

In `crates/mud-client/tests/mapview.rs`, delete these four tests **entirely**, along with their doc comments:

- `the_cursor_moves_room_to_room` (~line 91)
- `a_gap_is_jumped_rather_than_stepped_into` (~line 114)
- `movement_finds_a_room_that_is_not_on_the_same_row` (~line 136)
- `movement_with_nothing_that_way_stays_put` (~line 164)

They are not adjusted to keep passing — each asserts a property the snap had and a one-cell cursor deliberately does not. `a_gap_is_jumped_rather_than_stepped_into` is the exact inverse of the new requirement.

- [ ] **Step 2: Write the replacement test**

Add in their place:

```rust
/// One press, one cell. The cursor moves the canvas rather than hopping
/// between rooms.
///
/// It used to snap to the nearest room in the pressed direction, ranking
/// candidates by how far OFF the axis they sat. That made a press land
/// an unpredictable distance away, and often somewhere the operator was
/// not aiming — hard to drive in practice, which is why it was reverted.
///
/// The original objection to a cell cursor was that it "spent most of
/// its life on nothing" and vanished. It cannot vanish: `render` paints
/// the cursor last and unconditionally, over empty cells included, and
/// `under_cursor` puts a floor under the reversed colour. The panel
/// being blank over a gap is the accepted cost of aiming precisely.
#[test]
fn one_press_moves_the_cursor_exactly_one_cell() {
    let mut v = view();
    for (code, (dx, dy)) in [
        (KeyCode::Right, (1, 0)),
        (KeyCode::Left, (-1, 0)),
        (KeyCode::Down, (0, 1)),
        (KeyCode::Up, (0, -1)),
        (KeyCode::Char('l'), (1, 0)),
        (KeyCode::Char('h'), (-1, 0)),
        (KeyCode::Char('j'), (0, 1)),
        (KeyCode::Char('k'), (0, -1)),
        (KeyCode::Char('y'), (-1, -1)),
        (KeyCode::Char('u'), (1, -1)),
        (KeyCode::Char('b'), (-1, 1)),
        (KeyCode::Char('n'), (1, 1)),
    ] {
        let from = v.cursor();
        press(&mut v, code);
        assert_eq!(
            v.cursor(),
            (from.0 + dx, from.1 + dy),
            "{code:?} should move exactly one cell"
        );
    }
}

/// A cell with no room in it is a place the cursor may stand. The snap
/// made this impossible by construction; it is now ordinary.
#[test]
fn the_cursor_may_stand_on_empty_space() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    // Two rooms two cells apart on the same row: A --east--> C is not
    // possible in one step, so build A -e-> B -e-> C and walk past C.
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let mut ra = GraphRoom {
        name: "A".into(),
        ..Default::default()
    };
    ra.exits[Direction::East as usize] = Some(ExitEdge {
        dest: b,
        exit_type: 0,
        command: None,
    });
    let g = std::sync::Arc::new(RoomGraph::from_rooms(vec![
        (a, ra),
        (
            b,
            GraphRoom {
                name: "B".into(),
                ..Default::default()
            },
        ),
    ]));
    let mut v = MapView::new(g, spawns(), a, Fix::Unknown, PaintCtx::default(), (100, 30));
    // A is at (0,0) and B at (1,0); (2,0) is empty.
    press(&mut v, KeyCode::Right);
    press(&mut v, KeyCode::Right);
    assert_eq!(v.cursor(), (2, 0));
    assert_eq!(v.cursor_room(), None, "standing on nothing is allowed");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p mud-client --test mapview one_press_moves_the_cursor_exactly_one_cell the_cursor_may_stand_on_empty_space`

Expected: both FAIL — the snap moves the cursor further than one cell, and refuses to stop on an empty cell.

- [ ] **Step 4: Replace `step()`'s body**

In `crates/mud-client/src/mapview.rs`, replace the whole of `step()` — doc comment included — with:

```rust
    /// Move the cursor one cell.
    ///
    /// This used to snap to the nearest room in the pressed direction,
    /// ranking candidates by deviation off the axis. It read well and
    /// drove badly: a press travelled an unpredictable distance and
    /// often landed somewhere nobody was aiming.
    ///
    /// A cell cursor cannot get lost the way the snap's rationale
    /// feared. `map::render` paints the cursor last and unconditionally,
    /// empty cells included, and `map::under_cursor` puts a floor under
    /// the reversed colour so it stays visible on dark ground. Standing
    /// on nothing leaves the side panel with nothing to describe, and
    /// that is the accepted cost of aiming precisely.
    ///
    /// `follow` scrolls the viewport once the cursor reaches the margin,
    /// so holding a direction pans the canvas.
    fn step(&mut self, dx: i32, dy: i32) {
        self.cursor = (self.cursor.0 + dx, self.cursor.1 + dy);
        self.follow();
    }
```

Leave the eight key bindings at lines ~296-307 exactly as they are: `yubn` stays bound to the diagonals, now as one-cell diagonal steps. Removing them was not asked for.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p mud-client --test mapview one_press_moves_the_cursor_exactly_one_cell the_cursor_may_stand_on_empty_space`

Expected: both PASS.

- [ ] **Step 6: Re-read the tests that used the snap as setup**

Open `crates/mud-client/tests/mapview.rs` and read these two in full:

- `the_viewport_follows_the_cursor_only_at_the_margin`
- `zooming_keeps_the_cursor_room_on_screen`

For each, answer in the commit message: does it still assert something? `zooming_keeps_the_cursor_room_on_screen` names a *room* under the cursor — if the cursor can now stand on empty space, check whether its setup still puts a room there, or whether the assertion has quietly become vacuous.

Removing a mechanism can empty a test that used it to set up, leaving it green and proving nothing. If either has gone vacuous, fix it by making the setup explicit (position the cursor on a known room deliberately) rather than by deleting the assertion.

- [ ] **Step 7: Run the whole crate's tests**

Run: `cargo test -p mud-client`

Expected: PASS.

- [ ] **Step 8: Verify by eye against a real board**

The status bar at the bottom of the map view already reads `move arrows/hjkl/yubn`, which stays accurate. Launch the map browser and drive it:

```bash
cargo run -p mud-client --bin mmc -- map 1/2146
```

(`map` takes the opening room as a required positional argument — `map/room`
or part of a name. `--content` defaults to `re/mmud_wgnt.sqlite`, so it only
needs passing when running from outside the repo root. `q` leaves.)

Confirm: one press moves one cell; the cursor is visible when it sits in a gap between streets; holding a direction pans the canvas at the margin; `yubn` moves diagonally one cell.

- [ ] **Step 9: Commit**

```bash
git add crates/mud-client/src/mapview.rs crates/mud-client/tests/mapview.rs
git commit -m "revert(mapview): arrow keys move one cell, not room to room

8ca3a68e snapped the cursor to the nearest room in the pressed
direction. In use it drove badly -- a press travelled an unpredictable
distance, and ranking candidates by off-axis deviation meant it often
landed somewhere nobody was aiming.

That commit's objection to a cell cursor no longer holds: render paints
the cursor last and unconditionally over empty cells, and under_cursor
floors the reversed colour, so it cannot vanish into a gap. The blank
side panel over empty space is the accepted cost.

yubn keeps its diagonals, now one cell at a time. The four tests
asserting snap semantics are deleted with the mechanism rather than
edited until they pass again."
```

---

## Self-Review

**Spec coverage.** Every section of `2026-08-22-map-connectors-design.md` maps to a task: the `Plane::edges` design → Task 1; `connectors()` reading it and `link()` left alone → Task 2; the rebuilt crossing test and the fabricated == 0 census → Task 3; "Also in scope: arrow keys move the canvas again" → Task 4. The spec's *deliberately not doing* (widening) correctly has no task. The spec's *out of scope* (the 40 unplaced `Conflict` rooms) correctly has no task.

**Test numbering.** The spec's testing section lists seven items. Spec 1 → Task 2 test 1. Spec 2 → Task 2 test 2 and Task 1's test. Spec 3 → Task 3 step 1. Spec 4 (`1/837` renders exactly three connectors) → **covered by Task 3's world census**, which checks every room including `1/837`; a separate single-room test would assert a strict subset. Spec 5 → Task 3 step 3. Spec 6 → Task 2 step 5 and Task 3 step 6. Spec 7 → Task 4.

**Type consistency.** `has_edge(&self, room: RoomId, dir: Direction) -> bool` is defined in Task 1 and called with exactly that signature in Tasks 2 and 3. `connectors`' third parameter changes from `cell: Cell` to `room: RoomId` in Task 2 and is not referenced elsewhere. `step_of(dir) -> Option<Cell>` is pre-existing (`map.rs:61`) and public; Task 3 uses it. `DIRECTIONS` is pre-existing (`graph.rs:15`) and public.

**Known-failing window.** Task 2 deliberately leaves the tree with one failing test, fixed in Task 3. Tasks 2 and 3 must land in that order; do not run them out of sequence or in parallel.
