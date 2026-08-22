# The map draws connections that do not exist

**Status:** accepted 2026-08-22
**Scope:** `crates/mud-client/src/map.rs`, `crates/mud-client/tests/map.rs`

## The problem this solves

`map.rs::connectors` decides whether to draw a link between two rooms by
asking whether a room is *sitting in the adjacent grid cell*, never whether an
exit joins them:

```rust
for (_dir, step) in COMPASS {
    if plane.room_at((cell.0 + step.0, cell.1 + step.1)).is_none() {
        continue;
    }
```

The room graph is available and is not consulted. Every pair of rooms that
happens to land side by side on the grid gets a connector, and the connector
is drawn identically to a real one.

### What it costs, measured

On the plane the client was actually displaying when this was reported —
anchored at `1/2146`, which the client's own sidebar labels `plane 1882 rooms,
102x146` with `40 rooms not placed`:

| | count |
|---|---|
| rooms placed | 1,882 |
| connectors drawn | 7,854 |
| backed by a real exit | 4,200 |
| **fabricated** | **3,654** (47%) |
| rooms drawn with all 8 connectors but ≤3 real exits | 212 (11.3%) |

`layout()` is a BFS from the anchor, so both the placed-room count and the
conflict set depend on which room it starts from. Anchored at `1/837` instead
the same component places 1,997 rooms and 3,635 of 8,066 connectors are
fabricated (45%). The defect is not an artefact of the anchor; only the exact
totals are. Every census in this document states its anchor.

Worldwide — every room laid out, each unvisited room anchoring a new plane,
1,307 planes in total (anchors are arbitrary here, so these are
order-of-magnitude figures, not a pinned census):

| | count |
|---|---|
| `╳` glyphs drawn (2×2 blocks) | 84,346 |
| blocks with two genuinely crossing diagonals | **6,503** |

So 92% of the crossings on the map are false.

The reported case: `1/837 Graveyard, East of Tomb` has exactly three exits —
`north → 1/838`, `south → 1/836`, `east → 1/823`, all type 0. The client's own
sidebar prints `exits: n s e`. The map draws that room enclosed on all eight
sides. A four-room square loop with **zero** diagonal exits still renders

```
□─□
│╳│
□─□
```

six connectors for three exits.

### Why the tests did not catch it

`tests/map.rs` has a section headed `// --- connectors must not lie ---` and a
test named `overview_draws_no_connectors_rather_than_misleading_ones` whose
comment reads *"a map that shows the wrong connections is worse than one that
shows none."* The intent was written down. But every assertion checks that a
connector is **present**; none checks that an absent exit stays unpainted.

Worse, `two_diagonals_through_one_cell_are_drawn_crossed` **asserts the bug**.
It builds `spokes(&[SouthEast, East])`, in which the neighbour rooms declare
no exits at all, and then requires a crossing to appear. The only thing that
can produce one is adjacency.

## Success criterion

**Every connector drawn corresponds to an exit in the room graph, and every
exit between two placed, grid-adjacent rooms is drawn.** Checked by a census
over the shipped world, not by example: fabricated == 0.

## Design

**Record real edges at layout time; stop inferring them at draw time.**

`layout()` already walks every exit of every room and already determines
whether the destination landed in the wanted cell. That is precisely the
information `connectors` needs, discovered at the only moment it is free.

Add to `Plane` a set of in-plane edges beside the existing `links` (which
records the *off*-plane hops):

```rust
edges: BTreeSet<(RoomId, Direction)>,
```

Insert into it in exactly the two places where a walked exit resolves to a
placed neighbour at the expected cell:

- the `Some(&there)` arm, when `there == want`
- the `None =>` arm that places the room

Conflicts are excluded for free, and correctly: a conflicting exit's
destination is *not* in that cell, so no line should be drawn to it.

`connectors()` then iterates the room's recorded edges instead of the compass.

### Why not thread the graph into `render`

`render(plane, styles, view, size, zoom, marks)` has no `&RoomGraph`, and
adding one would churn every caller in `mapview.rs`, `tui.rs` and the tests.
Recording the edges on `Plane` keeps `render` a pure function of `Plane` —
which is the property that makes the map testable at all.

### One-way exits

Only the declaring room records the edge, so the line is drawn once, in that
room's colour. This matches the existing rule in `connectors`' own comment: *"A
connector belongs to the room it leaves."* No reciprocity is inferred.

### `link()` is unchanged

The `╲`/`╱` → `╳` crossing logic stays exactly as written. It becomes honest
rather than wrong: after the fix a `╳` means two genuinely crossing diagonal
links, and there are 6,503 of those.

## Deliberately not doing: widening the canvas

Considered and rejected, after rendering the alternatives.

At a real crossing `╳` is already unambiguous — four corners with two crossing
diagonals admit exactly one pairing, NW↔SE and NE↔SW. Separating the two
diagonals into per-room stubs to avoid the glyph makes the picture *worse*:

```
real crossing            separated into stubs
□     □                  □     □
   ╳                      ╲   ╱
□     □                  □     □
```

Two lines that cross get drawn as two lines that converge. The stub form
destroys the one fact the glyph gets right.

Widening therefore buys legibility, not correctness, and costs rooms on
screen — on a 166-cell-wide plane that is a real price. Revisit deliberately
after the connectors are honest, if the map still reads as cramped.

## Also in scope: arrow keys move the canvas again

`mapview.rs::step` snaps the cursor to the nearest room in the pressed
direction (introduced `8ca3a68e`, 2026-08-02). In use it has proved hard to
drive — the cursor jumps an unpredictable distance, and ranking candidates by
off-axis deviation means a press can land somewhere the operator was not
aiming.

Restore the previous behaviour, which the same commit removed:

```rust
self.cursor = (self.cursor.0 + dx, self.cursor.1 + dy);
self.follow();
```

One cell per press. `follow()` still scrolls the viewport when the cursor
reaches an edge, so this pans the canvas rather than teleporting around it.

**Why the original objection no longer applies.** `8ca3a68e` argued a cell
cursor "spent most of its life on nothing" and vanished. It cannot vanish now:
`render()` paints the cursor **last and unconditionally**, over empty cells
included (`map.rs:550-562`), and `under_cursor()` puts a floor under the
reversed colour precisely so it stays findable on dark ground. That fix is
independent and stays.

The residual tradeoff is real and accepted: with the cursor on empty space the
side panel has no room to describe. That is the cost of aiming precisely, and
it is the operator's call — they have used both.

**Diagonals stay.** `8ca3a68e` also bound `yubn` to diagonal movement, and
that is not what is being reverted — MajorMUD streets run diagonally
constantly. `yubn` becomes a one-cell diagonal step. Removing it was not
asked for and would be a contraction.

## Testing

Written before the implementation.

1. **A 2×2 block joined only by orthogonal exits draws no diagonal and no
   `╳`.** This is the reported symptom in its smallest form.
2. **Two grid-adjacent rooms with no exit between them draw no connector.**
   Fixture: `A →east→ B`, `A →southeast→ D`; `B` and `D` are adjacent and
   unconnected, and today a `│` appears between them.
3. **`two_diagonals_through_one_cell_are_drawn_crossed` is rebuilt** so both
   rooms genuinely declare the crossing diagonals, and still asserts `╳`.
   Without this the suite would keep asserting the defect.
4. **`1/837` renders exactly three connectors** — north, south, east — and
   nothing else. The reported case, pinned.
5. **Census over the shipped world: fabricated == 0**, skipped with a clear
   message when `re/mmud_wgnt.sqlite` is absent (it is gitignored and
   auto-populated in worktrees).
6. The existing `every_compass_direction_is_drawn_at_the_connector_zooms` and
   `overview_draws_no_connectors_rather_than_misleading_ones` must stay green;
   `spokes()` declares each exit on the hub, so they should.
7. **One press moves the cursor exactly one cell**, in each of the eight
   directions, including onto an empty cell — and the cursor is still drawn
   there.

   Two existing tests assert the snap and must go **with** the mechanism, not
   be edited until they pass again:
   `tests/mapview.rs::the_cursor_moves_room_to_room` (line 91) and
   `a_gap_is_jumped_rather_than_stepped_into` (line 114). The second is the
   inverse of the new requirement — a gap must now be stepped into.

   Then re-read the tests that merely *used* the snap as a precondition —
   `the_viewport_follows_the_cursor_only_at_the_margin`,
   `zooming_keeps_the_cursor_room_on_screen` — and confirm each still asserts
   something. Removing a mechanism can quietly empty a test that used it to
   set up, leaving it green and vacuous; that has happened on this project
   before.

Mutation check before believing any of it: invert the edge test in
`connectors` and confirm tests 1, 2, 4 and 5 fail.

## Out of scope

**The 40 unplaced rooms** the sidebar reports on this plane are `Conflict`s —
real exits whose destination could not take its grid cell because the world is
not flat. That is the opposite defect: connections that exist and are
invisible. This design does not address them, and fixing adjacency does not
change their count.
