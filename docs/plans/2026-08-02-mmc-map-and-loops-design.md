# Interactive map, room dossiers, and a portable loop format — design (2026-08-02)

Base: `main` at 466fc86. Client-only; no `mud-core` or `mud-server` changes.

`/go` proved that the client knowing the world graph is worth a lot: you type a room
name and the navigator walks you there. Everything *above* that — deciding **where** to
farm, **what** lives there, **which circuit** to walk — still happens in the operator's
head and lands in the profile as a hand-typed `[farm].circuit` of `map/room` strings.
There is no way to see the world, no way to see what a room spawns without querying
sqlite by hand, and no way to keep a library of routes.

This adds a room **dossier**, an **interactive map** you scroll around to pick stops
with the route drawn in, and a **loop file format** that is hand-editable, mmc-first,
and can import MegaMud `.mp` paths.

## Evidence

Both findings were verified against `re/mmud_wgnt.sqlite` while planning.

### 1. "What spawns here" is a static query

The room row carries `monstertype` (spawn region), `minindex`/`maxindex` (a band of
selector ordinals — see below, they are NOT levels), `bynumber` (forced monster) and
`type` (spawn behaviour). The candidate list is:

```sql
SELECT * FROM monster
 WHERE "group" = room.monstertype
   AND "index" BETWEEN room.minindex AND room.maxindex
```

Checked: `1/2156 Small Cavern` (monstertype 6, band 66..66) → `cave bear` (group 6,
index 66). `1/1072 Slum Entrance` (monstertype 5, band 1..1) → `guardsman`.

Three refinements found while writing the tests:

- **`bynumber` is packed.** The forced monster number is `bynumber >> 16`; the low word
  is zero in all 26,720 rooms, so the column is a 32-bit read of a 16-bit field. Room
  `1/305 Skali's Fine Armour, Front Room` gives monster 27 `Gurbultis`, and that room's
  region+band path resolves to the same monster independently. Two paths agreeing is
  the strongest validation the decode has.
- **`monstertype == 0` is a sentinel, not region zero.** Taken literally the join makes
  `1/1 Town Gates` spawn `ancient tapestry` and `mirror portal` — the 29 group-0
  monsters are scenery props. A room spawns from its region only when `monstertype > 0`.
- **Band `0..0` is real and is kept.** 1,354 rooms carry a region with a zero band, and
  205 monsters sit at index 0. `1/109 Darkwood Forest` (region 11, band 0..0) resolves
  to minotaur champion, chest and spectral mage, which is plausible for that area — so
  a zero band is a level-0 selector, not a second sentinel.

- **The band is not a level range.** `re/docs/monsters.md` §1 calls `+0x462`/`+0x464`
  min and max *level*, but the values do not behave like one: monster `index` runs to
  666, 999 and 9999, and experience at every index spans the whole 0–65,000 range. It is
  an ordinal within the region's roster. Nothing that reasons about difficulty may use
  it; the map's third paint mode uses experience instead.

Field meanings from `re/docs/monsters.md` §1 — `room+0x560` region, `+0x462`/`+0x464`
the selector band, `+0x468` forced monster, `+0x43c` spawn type (0 = timed ~4 % per 5 s kick,
2 = timed ~89 %, 3 = swarm, 1 = boot-fill only). Aggression is the `follow` column, a
0–100 rating (`mon+0x108`, §4 table at line 266); guardsman 90, kobold slave 10, kobold
thief 20. The behaviour mode is `something3` (`mon+0x12c`), where 0 and 4 are unprovoked
and everything else initiates.

**ORACLE-OPEN:** the behaviour-mode reading is from the decompile, not from live play.
Guardsman comes out as mode 1 = "initiates" although guardsmen only attack criminals, so
the mode almost certainly gates on fame or legal status somewhere untraced. The danger
palette below is therefore provisional until live-checked.

### 2. MegaMud's room ID is a checksum, not an identity

`docs/mirrors/gitlab-beckersource-OmegaMUD/src/mega/OMUD_MEGA.java` specifies it: 3 hex
digits of a room-name hash (Σ char × 1-based position, last 3 hex digits of the sum)
followed by 5 nibbles of exit signature (`u_d`, `se_sw`, `ne_nw`, `e_w`, `n_s`; first
direction of each pair = 1, second = 4, doubled for a door).

Computed over all 26k rooms and tested against the 156 mirrored `.mp` files in
`docs/mirrors/megamud.net/www.megamud.net/paths/`. On a sample of 18 loops:

| outcome | count |
|---|---|
| start room resolves uniquely | 10 |
| walks end-to-end on our graph | 6 |
| every step hash also matches | 1 |

`A0700050` alone matches 38 rooms; 8,312 distinct IDs cover 26k rooms. The failures are
not parser bugs — `delfcity.mp`, `elfloop.mp`, `madwloop.mp` start in rooms that do not
exist in stock 1.11p, because those are other realms' custom maps.

So the importable core of a `.mp` is **"start here, then walk these directions"**, and
the per-step hashes are a *verification* signal saying where a foreign path diverges
from our world. The divergence report is the feature.

### 3. Plane sizes

*(Corrected once the layout was implemented. The first figures here came from a Python
probe that let planes merge across maps; the real rule keeps a plane inside one map and
gives smaller planes and a higher conflict rate.)*

Splitting the world at vertical exits and map boundaries puts the **median plane at 8
rooms** and the **largest at 2,420, in a 65 × 118 cell extent**. No plane anywhere is
wider than **160** cells or taller than **157**, which is what the overview zoom was
sized against. Grid layout from the exit graph is sound — `re/slum_map.py` closes 160
slum rooms with zero conflicts. A whole-plane BFS is microseconds, so the layout is not
radius-bounded; a radius would put a wall in the middle of the thing being scrolled.

**Conflicts are ~6 %, not ~1 %.** The largest plane cannot place 145 of its 2,420 rooms.
That is what drawing a world on a grid it was never built on costs, and it is why an
unplaced room is reported rather than drawn over its neighbour.

A plane count is deliberately not pinned: exits are directed, so "reachable from here"
is not an equivalence relation and any sweep's tally depends on where it starts.

## Decisions

| Decision | Choice |
|---|---|
| Where the map lives | In-client, full alternate screen, entered with `/map` |
| Interaction | Scroll a cursor around, mark stops, route drawn in gold |
| Loop model | Stops, routed by the existing `route()`, with optional `via` to force a leg |
| Loop storage | `~/.config/mmc/loops/<name>.toml`, one loop per file; profile never rewritten |
| Deferred | `/where <monster>` reverse lookup |

Full-screen means being blind to the board while planning, so the view bounces out on a
death or an HP-gate trip, and Ctrl-Q still quits. Board bytes are buffered while the map
is up and flushed into the scroll region on exit.

## Loop file format

One loop per file at `~/.config/mmc/loops/<name>.toml`. A directory rather than one
library file, because importing somebody's path pack means dropping files in.

```toml
name   = "slum-sweep"
note   = "guardsmen, lvl 5+"
world  = "mmud-1.11p"
origin = "mmc"                 # or "megamud:slm2loop.mp"

[[stop]]
at   = "1/1076"
name = "Slum Street, Crossroads"

[[stop]]
at   = "1/1123"
name = "Slum Street, Intersection"
rest = false                   # don't rest here

[[stop]]
at    = "1/1195"
name  = "Dark Alley"
light = true                   # needs a light source
via   = "w w s"                # force this leg instead of routing it

finish = "1/1072"              # park here when the run stops
```

**`finish` is written above the stops, not below.** TOML puts every bare key before the
first table header, so a scalar declared after `[[stop]]` would serialise INTO the last
stop. `profile.rs` records the same trap; the loop file must not fall into it twice.

`name` on a stop is a real optional key rather than a trailing comment: a comment
cannot survive a TOML round-trip and cannot be verified, a key can. On load a `name`
that disagrees with the graph is a loud warning naming both — that is what catches a
loop written against a different realm. Omitting it is legal.

`rest`, `light`, `fight` are the MegaMud step flags mmc can act on (`0002`/`0004` rest,
`0001`/`0009` dark, `0040` don't-attack). Flags it cannot act on (stash point, disarm
trap, pick lock, re-learn) are **not** stored — they would be dead fields — but the
import summary lists every one it dropped.

## Slices

### 1. Spawn dossier — `spawn.rs` (new), `/room`

- `SpawnTable::load(&Path)` reads `monster` once into `group → index → Vec<Template>`,
  same shape and read-only-open style as `RoomGraph::load_threat` (`graph.rs:189`).
- `GraphRoom` gains the spawn columns (`type`, `monstertype`, `minindex`, `maxindex`,
  `bynumber`, `shopnum`), loaded in the existing single `SELECT` (`graph.rs:73`) —
  extra columns there are free, a second query is not.
- `Dossier::of(graph, spawns, id)`: name, id, light, spawn behaviour, level band,
  candidates with exp/hp/level/alignment/`gamelimit`/aggression/initiates, exits
  including `ExitEdge::command` phrases, shop number. One renderer,
  `Dossier::lines()`, shared by `/room`, the map panel and the danger paint mode.
- `/room [id|name]` in `tui::slash`, defaulting to `here`, resolving names through
  `go::resolve` so ambiguity reporting is already solved.

### 2. Layout engine — `map.rs` (new), non-interactive `/map`

- `layout(graph, center) -> Plane`: BFS over the centre's plane, one grid cell per
  compass exit. Returns cell→room, extent, the conflict list and the links off-plane.
- Up/down and map-portals do not move the cursor in-plane. A plane is `(map, z)`; only
  the centre's plane is drawn, and rooms with a vertical or cross-map exit carry a
  marker.
- A cell whose wanted position is occupied is **not placed**; the source room is marked
  as having an undrawn link. Never silently overlap.
- `render(plane, view, zoom, paint)` is viewport-clipped, so cost tracks the terminal.

**Zoom** (`+`/`-`), cell footprint in characters:

| zoom | cell | shows | widest plane (160 × 157) |
|---|---|---|---|
| detail | 4 × 2 | glyph, `───` `│` `╲` `╱` connectors | 640 × 314 |
| normal (default) | 2 × 1 | glyph plus one horizontal connector char | 320 × 157 |
| overview | 1 × 1 | one coloured glyph per room | 160 × 157 |

Overview fits the widest plane on a 160-column terminal and still scrolls vertically. A
half-block level (1 × ½, `▀` carrying two rooms per character row, 160 × 79) is
deliberately out of scope until overview proves too tall.

Glyphs say structure, colour says state: `@` the character, `$` a shop, `+` a stairwell
or portal (a link off the plane), `·` an ordinary room — drawn `█` at overview, where a
single cell cannot carry a connector and colour has to do the work.

**Scrolling.** Arrows/`hjkl` move the cursor, viewport following at a two-cell margin;
Shift-arrows/`HJKL` pan a screenful without moving the cursor; `Home` recentres; `/`
searches by name. The panel shows `view (x,y) of WxH`.

**Colour.** Foreground says what the room *is*; background says what you *marked it
as*. They compose rather than fight, which a single-colour scheme cannot: a dangerous
room on your route has to show both.

Foreground is the paint mode, cycled with `m`:

| mode | meaning |
|---|---|
| terrain (default) | `1;30` needs light, `1;36` shop, `0;37` otherwise |
| danger | `1;35` spawns something that initiates, `0;35` unprovoked only, plain none |
| worth | best experience on offer: green → cyan → yellow → magenta at 0 / 100 / 1k / 10k |

Bright magenta is the exact SGR the board paints an aggressive monster in
(`bot.rs:234`, `events.rs:45`), so "kill me" means the same thing in both places.

**The third mode is worth, not the spawn band.** `minindex`/`maxindex` and the monster
`index` they select on are a within-region ordinal, not a difficulty scale: the values
run to 666, 999 and 9999, and experience at every index spans the whole 0–65,000 range.
A ramp built on them would be decoration. Experience is the number a farm spot is
actually chosen on, and it is real.

Background is the marks: `43` gold on-route, `47;30` for a stop, `42` for where the
character stands.

**The warning overlay** takes the foreground from whatever mode is active, in **bright
red `1;31`**, and red is reserved for it alone — which is why red is absent from the
worth ramp. It fires only on conditions the client can determine; there is no combat
model behind it and there must not appear to be one. `PaintCtx::default()` knows nothing
about the character and therefore warns about nothing, because a warning that is always
on is a warning nobody reads.

| trigger | source |
|---|---|
| the toughest spawn has more hitpoints than the character's maximum | `monster.hitpoints` against `BotConfig::max_hp`; 0 = unknown, no warning |
| the richest spawn is at or above an experience ceiling the operator set | `warn_above_exp`; 0 = never |
| a dark room while the character is known to carry no light source | `sheet::LightState` with an empty source list |

### 3. Interactive view — `mapview.rs` (new)

`async fn run(...)` called from `play`'s `/map` arm; owns the terminal until it returns.

- Alternate screen; on exit restore, `setup_region`, flush buffered board bytes,
  repaint.
- Keep draining `raw_rx` into a buffer (so the broadcast never lags) and `events` (so
  the assist and exp meter keep working).
- Beyond slice 2's keys: `>` follows a stairwell or portal under the cursor to that
  plane and `<` walks back the way it came, `g` leaves and starts a `/go` to the cursor
  room, `q`/Esc leave, Ctrl-Q quits. Esc closes a search rather than the map — one Esc,
  one thing.
- No Tab-to-next-room: the arrows move the cursor and `/` finds a room by name, and a
  third way to move the cursor is a third thing to remember.
- Right panel is `Dossier::lines()` for the cursor room, behind a ruled edge — the
  map's own right margin is ragged and the panel read as part of it without one.
- Bounce out and restore on a death event or an HP-gate trip.
- `mmc map --content <db> --at <map/room>` in `cli.rs`: the view needs the graph, not a
  connection, so offline planning between sessions is nearly free.

### 4. Loop library — `loops.rs` (new)

- `Loop`, `Stop` serde types; `load_dir`, `load`, `save`.
- `Loop::to_farm(&self, base)` fills `circuit`/`finish_at`, then validates with the
  existing `CircuitPlan::build` (`farm.rs:217`) so every leg is proven walkable before a
  file is written. Failure is refused with the planner's own error.
- Route highlight: for each consecutive stop pair plus the closing pair, `route()`, then
  replay the directions marking every room passed. A leg leaving the visible plane is
  marked at its departure room rather than drawn.
- Map keys: Enter/Space toggles a stop, `c` clears, `s` names and saves. The view builds
  the `Loop` value and hands it back as a `ViewAction`; the caller writes the file, so
  every key the view handles stays testable and the view never touches the filesystem.
- `/loop` lists the library, `/loop <name>` shows one loop's stops with a `!!` on any
  whose recorded name disagrees with the world. `/farm <name>` walks a named loop.
- **A named loop replaces the circuit, not the policy.** The profile's `[farm]` knobs —
  hp gates, dwell budgets, nav limits — still apply. The library holds routes, not
  settings, which is what lets one loop be walked by any character.
- No `/loop drop`: deleting a file is what `rm` is for, and a destructive verb behind a
  slash command in a terminal is a mis-keystroke away from losing work.

### 5. MegaMud import — `mega.rs` (new)

- `room_id(name, exits)` per `OMUD_MEGA.java`, plus an `Index` from id → candidates.
- `parse_mp`: tolerate CRLF, the optional `[name][author]` line, the optional
  `[PREFIX:Group:Node]` lines, and the `start:end:count:-1:gold::` header. Real mirrored
  files omit the group lines entirely.
- `import(graph, mp) -> (Loop, Report)`: resolve the start by hash (unique → take it,
  ambiguous → list candidates, unknown → fail naming the hash), replay the directions,
  verify each step's hash. One stop per step with `via` = that direction, consecutive
  duplicates collapsed.
- `Report`: steps walked of steps present, where it diverged, hash mismatches, flags
  dropped.
- `/loop import <file>` and `mmc loop import <file>`.

## Files

New under `crates/mud-client/src/`: `spawn.rs`, `map.rs`, `mapview.rs`, `loops.rs`,
`mega.rs`. Modified: `graph.rs` (spawn columns), `tui.rs` (slash verbs, the `/map` arm),
`cli.rs` (subcommands), `lib.rs`. Unchanged on purpose: `farm.rs`, `nav.rs`, `go.rs` —
this work consumes `travel`, `route`, `CircuitPlan` and `resolve` rather than growing
parallel copies.

## Verification

Unit and corpus tests in `crates/mud-client/tests/`:

- `spawn.rs` — `1/2156` → cave bear (100 exp, 50 hp, gamelimit 1), `1/1072` →
  guardsman, a `bynumber` room, a room with no spawn.
- `map.rs` — the Slum Entrance plane reports its conflicts rather than swallowing them;
  the largest plane is 2,420 rooms in 65 × 118 and the widest anywhere is 160 × 157 (a
  guard on the layout rule itself); the largest plane's 145 unplaced rooms are pinned
  too, so the honest conflict rate cannot quietly drift; a vertical exit consumes no
  grid cell; a plane never crosses into another map; at every zoom the render never
  exceeds the terminal and a viewport at the extent edge clips instead of panicking;
  danger paint is `1;35` for an initiating spawn and dim magenta for unprovoked-only;
  an unknown character warns about nothing.
- `loops.rs` — TOML round-trip; a `name` disagreeing with the graph warns; an
  unwalkable stop list is refused by `CircuitPlan`.
- `mega.rs` — corpus over the mirrored `.mp` files asserting the measured baseline
  (`slm2loop.mp` start → `1/1076`; `rocsloop.mp` walks 72/72 with every hash matching;
  `elfloop.mp` fails with "start room not in this world"). This locks in the honest
  numbers rather than pretending import is total.

Live, in the existing tmux session, on Salad:

1. `/room` in a known room; check the dossier against what actually spawns.
2. `/map`, zoom to overview, pan a screenful, zoom back and confirm the cursor held;
   scroll to Small Cavern and confirm the vertical marker and plane hop.
3. Danger paint in a slum street, checked against what actually attacks — this is the
   test that settles the ORACLE-OPEN behaviour-mode reading above.
4. Mark 3 slum stops, watch the gold route appear, save, read the file.
5. `/farm slum-sweep` walks exactly those stops.
6. `/loop import .../rocsloop.mp`, read the report, `/map` it and eyeball the route.
7. The map bounces out on an HP-gate trip.

Board etiquette: `board-safe-to-restart` before any restart, and the spawner drains in
1–2 h, so restart immediately before a field run.

## Follow-on, not in this plan

`/where <monster|item>` — reverse index over the spawn table ranked by
`RoomGraph::distances`, and the `/spots exp>N within M steps not-dark` filter it
enables.
