# Exit requirements: what an edge costs depends on who is walking it

**Status:** accepted 2026-08-22
**Scope:** `crates/mud-client` — `graph.rs`, `nav.rs`, `sheet.rs`, new purse
and requirement modules
**Companion:** `2026-08-22-map-connectors-design.md` (independent; that one
makes the map honest, this one makes the router honest)

## The problem this solves

```rust
pub fn exit_cost(exit_type: i64) -> u32
```

The router's cost function takes the exit *type* and nothing else. Whether an
edge is worth taking, or possible at all, routinely depends on the character —
gold on hand, items carried, alignment, class, level, whether a puzzle has
been solved. None of that can reach this function, so the router answers the
same for a pauper and a millionaire.

Three consequences, all measured.

### 1. A toll the router cannot see

`roomtype == 4` is a **toll exit** and `para1` is the gold it demands. All 57
shipped type-4 exits carry `para1 ∈ {0, 5, 5000, 10000}` — a currency
distribution and nothing else. `para2` is 0 on 56 of the 57, so it is not a
denomination selector.

| para1 | count | where |
|---|---|---|
| 5 | 3 | `1/1381` ↔ `1/1382` Silvermere Gates (both directions), `2/2528` Temple Street |
| 0 | 2 | `17/2656`, `17/3340` |
| 5000 | 11 | map 17 Ebony / Crimson Passage |
| 10000 | 41 | map 17 Grand Hallway / Golden Passage / Crimson Passage |

`re/docs/theft.md` §8.1 — this project's authority for exit types — does not
name type 4, and `grep -rn toll` over `crates/mud-core/src` and `re/docs`
returns **zero hits**. Nothing here models it.

`exit_cost` prices type 4 among the ORACLE-OPEN defaults at **1, a plain
step**. So today the router walks through a 10,000-gold toll it cannot pay,
and always prefers the Silvermere gate: the toll edge is **1 hop**, and the
cheapest toll-free way round is **34 hops for a cost of 77** under this very
`exit_cost` model (an unweighted BFS puts it at 29 — the two differ because
the free route pays for doors, and it is the weighted figure the router
actually sees). That preference is not close, and it is made in total
ignorance of the purse.

### 2. A hidden exit that SEARCH can never reveal

`nav.rs:909-913` dispatches SEARCH whenever the graph says `exit_type == 6`.
For `17/3042`'s north exit that is a trap with no exit: the passage is
concealed by a **bit-word**, not a search roll. `para1 = 240`, and it opens
only when four `remoteaction` scripts in four other rooms have each cleared
their bit. No number of search rolls can succeed, so the walker searches
until something else interrupts it.

This is the same shape as the Silvermere alleyway incident already recorded in
`exit_cost`'s doc comment (live, 2026-08-02) — except that one was merely
expensive, and this one is unbounded.

### 3. Puzzles the client cannot represent at all

The shipped world's one multi-room ordered puzzle, on map 17:

| actor room | script |
|---|---|
| `17/3037` | `put diamond in hole … remoteaction 3042 0 1 0` |
| `17/3039` | `put pearl in hole … remoteaction 3042 0 2 0` |
| `17/3041` | `put moonstone in hole … remoteaction 3042 0 3 0` |
| `17/3043` | `put marble in hole … remoteaction 3042 0 4 0` |

All four target `17/3042`, north exit, `type=6`, `para1=240`, `para2=-4`.
Tracing our own `mud-core/src/game.rs::remote_lever`: `para2 < 0` sets
`unconditional`, which bypasses the `word & gate` check — so **the four gems
may be placed in any order**. Each also requires carrying a specific item
(`checkitem 1921/1922/1923/1924`). All bits clear → passage opens, 300 s
re-hide.

Census: 28 rooms carry `remoteaction` scripts, 82 directives, 62 same-room and
5 distinct cross-room actor→target pairs — the four above plus `12/2118
Plaster Hallway, Guardroom` → `12/2122 Narrow Precipice`.

`mud-client` has none of this. The quest VM and `exit_locks` live in
`mud-core`, the *server* reimplementation — a different program. The bot
consults only the static `RoomGraph`.

**Not a problem:** the Marble Chamber candle (`1/2252`, north → `2254`,
`type=10`, `para1=8586` = `"turn right candle left"`) is a plain
`COMMAND_EXIT`, one of 250. `nav.rs:622-646` already handles these — *"A
command exit is not walked, it is spoken."*

## Success criterion

**The router's answer changes with the character's state, and every refusal
names its cause.**

Concretely, all of:

1. Routing out of Silvermere with ≥ 5 gold takes the 1-hop gate; with less it
   takes the long way round and still arrives. Same code path, no special
   case. The detour's exact length is not a contract — it moves with any
   price in `exit_cost`.
2. The walker never sends SEARCH at a puzzle-concealed exit.
3. A route that fails at a gate reports *which* gate, not a timeout.
4. No route is refused for being expensive — only for being unsatisfiable.

## Design

### `ExitRequirement`, computed once at load

Derived from `roomtype` + `para1..3` + the room's `cmdtext` script, in *our*
decode. MudPlay's `RoomExitHint` is the idea worth taking; their MDB
parenthetical grammar is not, and will not transfer.

```
None
Door { pickable }            2, 7, 0xb
Hidden { searchable: bool }  6
Trap                         9, 0x18
Command(String)              10        (already carried on ExitEdge)
Toll { gold: u32 }           4
AlignmentGate                0x14
SpellGate                    0x16
AbilityGate                  0x17
Timed                        0x10
Puzzle { actions: Vec<PuzzleAction> }
```

`Hidden { searchable: false }` is set when any `remoteaction` targets that
exit — which is what stops the `17/3042` hang.

`Puzzle` is built by a load-time pass over the 810 rooms with a `cmdtext`,
extracting `remoteaction <room> <msg> <action> <exit>` and attaching the
requirement to the **target** exit. `mud-core` already parses this grammar, so
the rule has a second independent reader to disagree with us if we get it
wrong.

### Cost, not prohibition

```
exit_cost(&ExitRequirement, &Capabilities) -> Cost      // Cost::Steps(u32) | Cost::Impassable
```

`Capabilities` is what is currently known about the character: purse, level,
class, alignment, inventory, known spells.

This deliberately keeps the property `exit_cost`'s existing doc comment
defends — *"a route that fails at a gate says something truer than 'no
route'"*. MudPlay's `MovementFilter.IsExitBlocked` is a boolean that removes
the edge from the BFS frontier entirely; ours stays a cost, so an awkward but
real route can still be found and reported. That is the one place our router is
already better and it should stay that way.

The toll then needs no special case: purse ≥ toll → `Steps(1)` plus a pay step;
purse < toll → `Impassable`; Dijkstra finds the detour by itself.

### The purse

`Purse(u64)`, a single integer **in copper farthings**. One unit end to end;
denominations exist only at the parse and display boundaries.

Ratios come from `mud-core`'s `CoreConfig::coin_ratios` — `[10, 10, 100, 100]`,
ORACLE-VERIFIED off the Bank of Godfrey lobby sign — rather than a second
hardcoded copy:

| denomination | farthings |
|---|---|
| copper farthing | 1 |
| silver noble | 10 |
| gold crown | 100 |
| platinum piece | 10,000 |
| runic coin | 1,000,000 |

So the Silvermere toll is **500 farthings** and map 17 wants **1,000,000**.

Inputs we already have: `bot.rs` detects coin drops and `auto_get` sweeps
them, and `mud-core/src/text.rs:1174` documents the board's own listing
grammar (`"1 gold crown, 9 copper farthings"`), so the parser does not need
reverse-engineering. MudPlay's `Game/Cash/CashManager.cs` carries the same
five-slot ladder including `runic` and is a usable cross-check on wire
wordings.

### The toll direction: measure, do not guess

Both directions of the Silvermere gate carry `type=4, para1=5, para2=0,
para3=0`, but only the outbound crossing actually charges in play. The
asymmetry is in the engine, not the table, and we do not have the rule —
`theft.md` §8.1 does not cover type 4, and the answer is in `move_user` in the
WCCMMUD decompile, unread.

So the client **learns it**:

- Start pessimistic — assume every type-4 crossing charges. This can waste a
  detour; it can never strand the character on the wrong side of a gate it
  cannot pay to re-cross.
- On an actual crossing, compare the purse before and after. If it did not
  move, record that `(room, direction)` as free.

The unknown becomes a measurement that self-corrects on first contact. It also
protects against the inverse error — assuming inbound is free everywhere and
meeting a board where it is not.

Accepted cost: until an inbound crossing has been observed, routing *into*
Silvermere with under 5 gold takes the long way for no reason. Safe, wasteful,
self-repairing.

### Also unknown, and stated as such

`para1`'s unit is inferred to be **gold crowns**, on two supports: the observed
5-gold Silvermere toll matches `para1=5`, and map 17's `para1=10000` is exactly
10,000 gold = 100 platinum = 1 runic coin, a clean ladder number. This is
inference from one observation and one round number, not proof. `move_user`
would settle it. The conversion must live in exactly one place so a correction
is a one-line change.

## Phase C — the detour solver

Separate plan, after this one lands.

Given a `Puzzle` requirement, plan go-act-return per action: route to the
actor room, satisfy its item precondition, send the phrase, return, cross
within the 300 s window. Track what has already been done.

This is where we beat the reference implementation outright. MudPlay has **no
puzzle state at all** — no `PuzzleState`, `TriggerState`, `GateState` or
`LeverState` type exists anywhere in its tree — and re-runs every lever detour
on every crossing, relying on a server-side timer (`RemoteActionPathExpander`:
*"a game-side timer, not in the data"*). With four gem-fetches against a 300 s
re-hide, that approach would simply fail here.

C depends on inventory contents, which is why it is scoped separately.

## Testing

Fixtures are real and named, not invented.

1. **Silvermere, both ways.** Purse 500 farthings → the 1-hop gate. Purse 499
   → a route that exists and does not start east. The pair is the whole design
   in one test.
2. **Map 17 toll.** Purse 999,999 → `Impassable`. Purse 1,000,000 → passable.
3. **`17/3042` never gets a SEARCH.** The hang, pinned.
4. **`1/2252` candle** still routes as a spoken command exit — proof the
   change did not regress the 250 command exits that already work.
5. **`12/2118` → `12/2122`** produces a cross-room `Puzzle` requirement with
   one action, in the right target room.
6. **The gem puzzle parses to four actions, order-free** (`para2 = -4`), each
   with its item precondition.
7. **Purse round-trip:** every denomination string the board can print parses
   to farthings and back.
8. **Observed-toll learning:** a crossing with no purse change marks that
   direction free; the next route prefers it.

Mutation checks before believing the suite: force `Cost::Impassable` to
`Steps(1)` and confirm 1, 2 fail; force `searchable: true` and confirm 3
fails; break the farthing ratio and confirm 1, 2, 7 fail.

## Open questions

- The directional toll rule (`move_user`). Pessimistic-charge until read.
- `para1`'s unit (gold vs farthings). Inferred, isolated to one conversion.
- Whether `Inventory` in `sheet.rs` tracks item identity well enough for C's
  preconditions. Unverified; C's plan must check before relying on it.
