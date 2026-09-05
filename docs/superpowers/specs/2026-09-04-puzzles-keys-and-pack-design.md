# Puzzles, keys and the pack: design

**Status:** accepted 2026-09-04. This is the "Phase C" that
`2026-08-22-exit-requirements-design.md` deferred, widened by a data
discovery that spec did not have.
**Scope:** `crates/mud-client`: `graph.rs`, `nav.rs`, `bot.rs`, `roam.rs`,
`purse.rs`, a new `pack.rs`, a new `puzzle.rs`.

## Goal

The walker can open what the world opens with a phrase, a lever, a key or a
carried item, and routing knows in advance what this character can and cannot
get through. A roam or a walk never stands at a wall sending a command that
can never succeed.

## Origin

Daniel, 2026-09-04, after a roam sent `search s` two hundred times at 1/506
Secret Passage: *"let's do the proper fix, because we're going to run into
this issue a lot. There are push buttons and levers to pull all over the
place."* On keys: *"there's going to be a lot of places where keys are
required. If we don't have them and our pick strength isn't high enough
(which can be calculated) to open the door without a key, the client needs to
know it's going to be blocked. We also need to pick up any keys we see along
the way that we don't already have in inventory. That should be an automatic
default-on thing."* On scope: *"everything."* On roams: they solve puzzles
too. On the pick calculation: *"If that number is too low, the pick will fail
100% of the time. It's an actual calculation buried in a formula somewhere. I
don't think MudPlay has prior art for this either."*

## What the data says

### Type 12 slots are buttons and levers

The earlier spec counted puzzles by reading `remoteaction` lines in room
command text and found 25 targets. That missed the main mechanism. A room's
exit slot with `roomtype = 12` is not an exit. It is a remote action, decoded
from the Nightmare-Redux map source (`frmMap.frm` near line 21705, in the bbs
backup):

| field | meaning |
|---|---|
| `roomexit` | the target room, usually the room itself |
| `para1` | message id. Line 1 is the phrase, line 2 an alias |
| `para2` | below 10: exit index, action 0. Otherwise action = `para2 / 10`, exit index = `para2 % 10` |
| `para3` | message id of the response. Line 1 to the actor, line 2 to the room |
| `para4` | item id the actor must carry, 0 for none |

Census over the shipped world, 287 slots:

| kind | slots |
|---|---|
| acts on its own room | 200 |
| acts on another room | 87 |
| action 0, full reveal | 179 |
| action 1 | 51 |
| action 2 and above, ordered puzzles | 57 |
| needs an item, 79 of them a titanium fork | 86 |

Targets: 248 hidden exits, 19 gates, 4 doors, 12 plain exits, 1 trap, 3
missing. Every slot has a response message. 240 of the 248 hidden targets
carry a custom reveal line in the exit's `para3`, and the exit's `para4` is
the label the exits line shows once revealed: "secret passage %s", "dark
passageway %s", "cramped passage %s". The walker's exit token reader already
takes the trailing word, so "secret passage south" reads as south.

The room that started this, 1/506, has a west slot: phrase "push button",
action 1 on exit 1 which is south, response "You push the button." The south
exit is type 6 with state 16, reveal "A wooden panel in the wall slides open."
No search can open it.

### The concealment word

From `theft.md` section 9 and our own `mud-core` `remote_lever`, which
follows the DLL at 65973 to 66079. A type 6 exit's `para1` is a state word.
Bit 2 means hidden and searchable. Bits 0x10 through 0x2000 are puzzle bits.
Action n clears bit n+3. Action 0 clears every puzzle bit. Action 10 clears
0x2000. Unless the exit's `para2` is negative, action n only works when bit
n+4 is already clear, so levers pull in descending order. When no puzzle bit
remains the exit opens, the target room hears the reveal line, and a timer
re-hides it after 300 seconds. Only the target room hears the reveal. The
actor hears the slot's own response line.

Shipped state words on puzzle targets: 16 on 166, 48 on 29, 240 on 21, 2032
on 14, 1008 on 6, 112 on 6, 496 on 5. Every one is a run of bits starting at
0x10, so the plan for a word is the actions 1 through k where k is the bit
count, or one action 0.

The Crypt pair Daniel named: 1/1044 holds "pull lever" as action 1 and
1/1038 holds it as action 2, both on 1/1056 north, state 48 with `para2 = -2`,
so either order works. Captured 2026-09-05: each pull answered "You pull the
lever. Off in the distance you hear a small click." and the lever room heard
nothing else. The target hallway then listed "dark passageway north" on its
exits line, the exit's `para4` label with the direction filled in, and
walking it landed in 1/1063.

### Search only clears bit 2

`theft.md` section 9: a search rolls `genrdn(0,100)` against
`max(Perception - 15, 3)` only when the state word has bit 2. Anything else
prints "You notice nothing different". So a type 6 exit with a puzzle word is
never searchable, and the existing 100 roll budget is spent for nothing.

Open question: 495 type 6 exits ship with state 1, most of them on maps 17 and 7. The
rule above says they are never searchable either. Nothing in play has tested
one. Until a capture does, the client keeps treating them as searchable,
which is today's behaviour.

### Key doors, item gates and the pick formula

Nightmare names exit type 2 "Key" and type 3 "Item". The shipped data:

| type | count | `para1` | `para2` | `para3` | `para4` |
|---|---|---|---|---|---|
| 2 key door | 70 | key item id | 2 locked | pick modifier | re-lock delay |
| 3 item gate | 173 | required item id | message | message | |
| 7 door, 11 gate | 1519 | 2 locked or 0 | pick modifier | re-lock delay | |

The pick roll, from `theft.md` sections 8.2 and 8.3 and mirrored in
`mud-core` `picklock_command`: skill must be at least 1, and
`genrdn(0,100) < modifier + skill`. The modifier is negative on hard locks.
Shipped key doors carry modifiers like -60, -100, -160, -290 and -999. So a
door is pickable at all only when `Picklocks >= 1` and
`modifier + Picklocks > 0`. Below that the pick fails every time. That is the
calculation Daniel remembered, and it is a pure function of two numbers the
client already has.

Item type 7 is the key type, 88 items. The inventory reply carries a key
ring line after the carrying line. Live it reads "You have no keys." when
empty and "You have the following keys:  black star key." with one on the
ring, two spaces after the colon (test.raw 2026-09-05).

### What is not known

None of these are in `re/docs`, the oracle transcripts or `mud-core`:

- What the board prints when a key is used on a key door, and whether `open`
  uses the key by itself or a verb is needed.
- What the board says at an item gate without the item.
- Whether a state 1 hidden exit answers a search.

Each is pinned by one capture. The design isolates each unknown in one
constant so the capture changes one line.

## Part 1: the pack

### The type

`pack.rs`:

```rust
pub struct Pack {
    pub carried: Vec<(ItemId, u32)>,    // loose items with counts
    pub worn: Vec<ItemId>,
    pub keys: Vec<ItemId>,
    pub unresolved: Vec<String>,        // names the resolver could not place
}
```

`Pack::has(item)` answers over carried, worn and keys. Names resolve through
`items::resolve`, exact match only, as the backstab work already does. A
name that does not resolve is kept as text so a bad parse is visible in
`/where` style output rather than silently missing.

### Reading it

The `i` reply is four lines: carrying, keys, wealth, encumbrance. The purse
reader already sends `i` and reads the carrying line for coins. It becomes a
pack reader that fills both. The carrying line is split on commas, the
trailing worn suffix and leading count are stripped as today. The key line is
everything after the first colon when it is not the empty ring line, split
on commas, trailing period dropped.

### Sharing it

`Capabilities` gains `pack: Arc<Mutex<Pack>>`, the same shape as the toll
log and for the same reason: one handle shared by every navigator and bot for
one character, updated by whichever walk changes it. `unrestricted()` gets an
empty pack, because an unlimited pack would route every walker through item
gates it cannot pass.

The pack is re-read from the board after every pickup and every key use,
never updated by inference. The board can refuse a pickup silently.

### Key pickup

`BotConfig.take_keys`, default true, separate from `auto_get` which sweeps
coins and defaults false. When a room block lists an item that resolves to a
type 7 item and the pack does not hold it, the bot sends `get <name>` and
waits for the pickup confirmation. Items print "You picked up a silver holy
amulet", already seen live, so the existing coin regex widens to items. After
a confirmed pickup the bot re-reads the pack.

## Part 2: buttons and levers

### The graph

`puzzle.rs` and `graph.rs`. At load, each type 12 slot becomes:

```rust
pub struct PuzzleAction {
    pub room: RoomId,
    pub number: u8,              // 0 clears all, 1..=10 one bit
    pub phrases: Vec<String>,    // message line 1, then line 2 if present
    pub item: Option<ItemId>,
    pub reply: Option<String>,   // the actor line of the response message
}
```

and attaches to the target exit, keyed by target room and exit index. Command
text `remoteaction` lines feed the same map, replacing the pass that exists
today. The slot itself never becomes an edge.

The target exit's requirement becomes:

```rust
ExitRequirement::Puzzle(Puzzle {
    word: u32,               // the exit's para1
    ordered: bool,           // para2 >= 0
    actions: Vec<PuzzleAction>,
    detour: u32,             // hops, see cost
})
```

`Hidden { searchable: bool }` collapses to `Hidden`: a type 6 exit is
searchable unless something targets it, in which case it is a puzzle. The
raw `exit_type` stays on the edge so the walker knows a revealed puzzle on a
type 7 door still needs `open`.

### The plan

`Puzzle::plan(&self, pack) -> Option<Vec<&PuzzleAction>>`, a pure function:

1. The bits set in `word & 0x3ff0` name the actions needed.
2. If an action 0 exists and its item is carried, the plan is that one action.
3. Otherwise every needed bit must have an action whose item is carried or
   absent. Missing one means `None`.
4. When `ordered`, actions run from the highest number down. Otherwise the
   order is by distance and is left to the walker.

### Cost

`exit_cost_for` on a `Puzzle`: `None` plan is `Impassable`. A plan costs one
step plus, per action, one step for the phrase and `2 * hops` from the exit's
room to the action room. `hops` is precomputed at load with a plain breadth
first search from each puzzle exit's room, stored as `detour` for the whole
puzzle so the router's inner loop stays a lookup. An action room the search
cannot reach makes the puzzle `Impassable`. The 40 step price of a searchable
hidden exit is unchanged.

Because the item test is per walker, the precomputed detour is the sum over
every action and the plan decides passability. That over-prices a puzzle with
an unused action 0, which is acceptable: the router still prefers it to a
long detour.

### The walk

In `goto`, a step whose edge is a `Puzzle`:

1. If the block the walk holds for this room lists the direction, the passage
   is open. Walk it.
2. Take the plan. `None` is a `NavErrorKind::Puzzle` error, not a re-localize,
   since routing should have refused the edge.
3. For each action in order: when its room is not the current room, run an
   inner `goto` to it with the same guard, boxed for the recursion. Send the
   phrase. Wait for the action's reply line or the next prompt, within the
   step timeout. An unknown command line or "you don't see that" fails the
   step with the puzzle error.
4. Inner `goto` back to the exit's room. Send the direction. A room block
   means through. "There is no exit in that direction" after a completed plan
   is a puzzle failure, tried once more from step 2 because the 300 second
   re-hide can beat a long detour, then reported.

A puzzle whose target is a door or gate ends with the door flow the walker
already has, since the lever unlocks it rather than removing it.

Interrupts from inner walks propagate as they do from a step.

### Roams

`roam::passable` allows puzzle edges. The region flood and the rotation take
the character's real `Capabilities` instead of `unrestricted()`, so the
region only grows through puzzles this character can solve. Doors stay
outside a roam as today.

## Part 3: key and item doors

### The graph

```rust
ExitRequirement::Door { locked: bool, pick: i32 }       // types 7, 0xb
ExitRequirement::KeyDoor { key: ItemId, pick: i32 }     // type 2
ExitRequirement::ItemGate { item: ItemId }              // type 3
```

`pick` is `para2` on a door or gate and `para3` on a key door.

### Pickable

`pickable(modifier, picklocks) -> bool` is `picklocks >= 1 && modifier +
picklocks > 0`, in one place, with the shipped modifiers as test cases.

### Cost

- `KeyDoor` with the key in the pack: the door price, 5.
- `KeyDoor` without it, pickable: 5 plus the expected rolls, capped at the
  hidden exit price.
- `KeyDoor` without it, not pickable: `Impassable`.
- `Door` locked and not pickable with bashing off: `Impassable`. With
  bashing on, the door price, since force is a separate roll.
- `ItemGate` with the item: 1. Without it: `Impassable`.

### The walk

At a key door with the key: `open <dir>`. If the board answers with the
locked line, send the key verb. That verb and its reply are one constant
each, pinned by capture. A successful reply is followed by the existing
unlocked then open then step flow. Without the key the existing pick and bash
flow runs unchanged, and routing has already checked that picking can
succeed.

An item gate is walked as a plain step. Routing has already checked the
item.

## Testing

Fixture graphs and scripted boards, as the navigator tests do today. No test
reads `re/`.

- `puzzle.rs`: plan over each shipped state word shape, ordered and not,
  with and without items, action 0 present and absent.
- `graph.rs`: a fixture room with a type 12 slot yields a puzzle on the
  target and no edge for the slot. A command text `remoteaction` line yields
  the same shape. A type 6 exit nobody targets stays `Hidden`.
- `pickable`: the five shipped modifiers against skills on both sides of the
  line.
- `exit_cost_for`: each new requirement with and without the pack, and the
  `Impassable` cases.
- `pack.rs`: the four line reply with and without keys, unresolved names
  kept.
- `nav`: a scripted board where the direction is refused, the phrase is
  spoken, the reply arrives, and the direction then lands. A same room
  button. A two room lever pair. A plan that fails after the phrase. A
  puzzle the walk finds already open from the exits line.
- `bot`: a room block listing a key the pack lacks sends a get, and one it
  holds does not.
- `roam`: a region grows through a solvable puzzle and not through an
  unsolvable one.

Live captures pin the constants. Held: the Crypt levers and the inventory
reply with a key. Owed: one key door on map 1 with its key held, and one
search at a state 1 hidden exit.

## Not in scope

- `mud-core` does not learn type 12 slots, key doors or item gates here.
- Trap disarming stays absent.
- Roams still refuse doors, keyed or not.
- Selling, dropping or organising the pack.

## Phases

1. The pack: `pack.rs`, the reader, `Capabilities.pack`, key pickup.
2. Buttons and levers: `puzzle.rs`, the graph pass, cost, the walk, roams.
3. Key and item doors: the requirements, `pickable`, cost, the key verb.
