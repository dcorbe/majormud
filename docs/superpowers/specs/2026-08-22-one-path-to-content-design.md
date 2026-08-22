# One path to the content database — Design

**Status:** accepted 2026-08-22. Supersedes nothing.

## Goal

The client and the server both read `re/mmud_wgnt.sqlite`, and they decode it
with two independently hand-written sets of SQL. Collapse that to one decoder,
in `mud-core`, and let the client hold the same typed `Content` the server does.

The race/class question that started this — the client probes for a spellbook on
classes that have no magic — then stops being a feature to add and becomes two
fields on a structure the client already has.

## Origin

Daniel, 2026-08-22, after the picklock fix landed: *"The client is checking
spells on classes that have no magic"*, then, on being offered a narrow
client-only loader: *"I'll worry about memory optimization later. What I'm
worried about now is composability and adding in a second path to data is the
opposite of that. The mud server deliberately loads the database into memory, so
the client should too."* And on placement: *"no crate spam. Shared code can go in
mud-core"*, *"if there's anything in mud-server that we can depend on it should
come out of there and go into mud-core."*

## The duplication being removed

`crates/mud-client/src/graph.rs` hand-writes five queries. Every one hits a table
`crates/mud-server/src/content_db.rs` already decodes:

| `graph.rs` | line | duplicate of |
|---|---|---|
| `SELECT {cols} FROM room` | 490 | `load_rooms` |
| `SELECT number, messageline1 FROM message` | 579 | `load_messages` |
| `room ⋈ textblock` (remote actions) | 614 | `load_textblocks` |
| `select lower(name), experience, hitpoints from monster` | 720 | `load_monsters` |
| `select lower(name), duration from spell` | 763 | `load_spells` |

## Architecture

1. **`content_db.rs` moves `mud-server` → `mud-core`** as `mud_core::content_db`.
   It produces `mud_core::content::Content`, whose types already live there. The
   per-table loaders become `pub`.
2. **`mbbs-server`'s `mud-core` dependency is dropped.** It is declared in
   `crates/mbbs-server/Cargo.toml` and uses zero `mud_core` symbols — verified by
   grep over the whole crate. Left in place, it would make the MBBS host build a
   bundled C SQLite to use nothing.
3. **The client loads the full `Content`** via the same `load()` the server calls.
   Deliberately in memory, deliberately all of it. A narrower client-only entry
   point is the second path this design exists to remove.
4. **The five queries are deleted** and their structures rebuilt as views over
   `Content`.
5. **Race and class fall out.** No new subsystem.

### No new workspace dependency

`mud-core` currently declares an empty `[dependencies]`. This adds `rusqlite`
0.37 with `bundled`. That is not new to the workspace: `mud-server` and
`mud-client` — the only two crates that actually consume `mud-core` after step 2
— already depend on exactly that version and feature set.

## What the views must do

The model is a strict superset of what the client reads, so this is a port, not
a redesign. Two things do not come for free:

- **Name indices.** `Content` keys monsters and spells by numeric id. The
  `ThreatTable` and spell-duration maps are keyed by lowercased name. Both
  indices are built in one pass at load (1101 monsters, 1379 spells).
- **Row filtering moves into the view.** The loaders store raw rows; the client's
  queries filtered in SQL. The views must reproduce, exactly:
  - monsters: skip `name == ""` (1 such row); score `exp*1000 + hp`; **max** wins
    on a name collision (7 rows share "giant rat").
  - spells: skip `name == ""` and `duration <= 0` — 542 of 1379 rows
    qualify, collapsing to **453 distinct names** in the map; **min** wins
    on a collision. (The earlier "542 survive" read as map entries and was
    misleading; both figures are real, and they count different things.)
  - exit commands: entry only when `messageline1` is non-empty after trimming.

`content::Exit` preserves the raw `exit_type` and all four `para` slots, so
`ExitRequirement::from_exit_type` and `exit_cost` port directly.

## Race and class

`class.magictype` answers what the client currently discovers by probing:

| magictype | classes | client behaviour |
|---|---|---|
| 0 | Warrior, Witchunter, Ninja, Thief | send no spellbook probe at all |
| 1–4 | Gypsy, Warlock, Mage, Paladin, Cleric, Priest, Missionary, Druid, Ranger, Bard | probe `spells` |
| 5 | Mystic | go straight to `powers` |

The wire gives a class *name*; the table keys by *number*. The lookup is by name
and **returns `Option`**. A miss — a customised board, a truncated sheet, an
unexpected spelling — must mean *"I do not know"*, never *"no magic"*. On a miss
the client falls back to today's probe-and-read-the-refusal path.

**The board stays authoritative.** The database is a mirror of one content set
(WG3-NT). It may only ever let the client *skip* a probe it is confident about;
it may never override what the board says. `sheet.rs`'s `Casting::redirected()`
keeps working and keeps winning.

## Testing

- The view functions are pure over a `Content`, so they test offline with a
  hand-built `Content` — no database file, no board.
- **Equivalence tests are the core of this change.** For each of the five, build
  the structure the old SQL produced and the structure the view produces from the
  same database and assert they are identical. That is what proves this is a
  refactor. These are the tests to mutate hardest: a view that silently drops the
  blank-name filter or flips a max/min tie-break must fail them.
- `cargo test -p mud-server --test load_real_db` must stay green after the move.
- Do not connect to a live board.

## Risks, stated

- **Strict loader.** `content_db` errors rather than skipping on anything that
  does not fit its types, where the client's narrow queries tolerated more.
  Verified non-blocking today: the full load succeeds on the shipped database,
  and `graph.rs:497`'s placeholder-row skip discards zero rows against it. It
  remains a live property to watch if new content ships.
- **Memory.** The client will hold 26720 rooms, 1950 items, 1101 monsters, 1379
  spells, 3867 messages, 3267 textblocks. Deferred by Daniel, deliberately. The
  resident cost is to be **measured and reported** once it runs, not argued about
  in advance.
- **Modified boards.** Content that diverges from WG3-NT makes the mirror wrong.
  The `Option`-returning lookup and wire-authoritative rule above are the
  containment.

## On the reliability of our own specs

The documents this work rests on — `re/docs/theft.md` and its siblings — are
hypotheses derived from a decompile, not ground truth. They will contain errors,
and the errors surface only when something independent contradicts them: a live
board, or a second working client such as `archive/FujiTerm/MudPlay`.

The evidence ranking, when sources disagree:

1. Observed behaviour on a live board.
2. A second independent implementation that was built against a live board.
3. Our RE documents.
4. Our code.

A conflict between (1)/(2) and (3) means the document is wrong until shown
otherwise — not the observation. **When a document is proven wrong, correcting it
is part of the same change as correcting the code.** A doc left stale
reintroduces the defect at the next port off that section.

This applies here to the exit-decode rules and the meaning of the `para` slots,
which the views in this design carry over unchanged from `graph.rs`. Porting them
verbatim preserves whatever they get wrong; the equivalence tests prove the port
faithful, not the rules correct.

## Not in scope

- Auto-sneak and the sneak-persistence question — tracked separately.
- Memory optimisation of the loaded `Content`.
- Reading `para2`/`para3`/`para4` (lock state, pick modifier, relock delay) in
  the door logic. The move makes that data available for the first time; using it
  is its own change.

## Sequencing

Blocked on the `2026-08-22-session-knows-character` plan landing. Its Task 3
edits `Capabilities` in `graph.rs`, which this design restructures.
