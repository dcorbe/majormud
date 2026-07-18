# MajorMUD 1.11p — MONSTER spawning, movement & non-combat AI (WG3-NT)

Behavioral spec of how monsters come into the world, wander through it, acquire and
pursue targets, and are updated each tick. The *combat* side (aggression rolls that end
in a swing, the swing loop, damage) lives in [`combat_rounds.md`](combat_rounds.md) and is
referenced here, not re-derived. Death / respawn bookkeeping is in
[`death.md`](death.md). Record layouts cross-referenced against
[`vir_schemas.md`](vir_schemas.md).

Reconstructed from the clean 32-bit **WG3-NT** decompile
(`../wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Function addresses are image-base
0x400000. Interval globals (`DAT_00482ca8`, …) are `.data` defaults not assigned in the
decompiled code — mechanism is certain, literal values not extracted.

---

## 0. The four monster update tiers

Monsters are driven by four self-rescheduling background jobs (the same 1/3/5/30 s
metronome as combat, see `combat_rounds.md` §1). **The task brief's assumption that wander
and despawn live in the 30 s `slow` update is wrong** — the tiers split as follows:

| tier | period | per-monster fn | what it does for monsters |
|------|--------|----------------|---------------------------|
| **fast**   | 1 s  | `fast_update_monster` (`0x21f24`) | prone-recovery timer; **pursuit** of a locked target; give-up / despawn |
| **medium** | 3 s  | `medium_update_monster` (`0x21cbc`) | spell-effect upkeep; environmental-death check; **random wander** |
| **slow**   | 30 s | `slow_update_monster` (`0x21c06`) | **HP regen** and periodic HP drain only |
| **spawn**  | `DAT_00482ca8` | `background_monster_create` → `FUN_004232d3` (`0x218b7`/`0x232d3`) | **create new monsters** near players |

Each tier walks the live-monster table incrementally: a cursor (`DAT_0047fb84` slow,
`DAT_0047fb94` medium, `DAT_0047fb9c` fast) advances one monster per call and wraps at
`_DAT_00482c60` (table size), so every live monster is serviced once per cycle
(`slow_update_next_monster` `0x21ae2`, etc.). The fast tier is additionally gated so it
only runs for a monster that **has an aggro target** (`mon+0x1a != 0`, checked in
`fast_update_next_monster` `0x21bb1`) — idle monsters cost nothing on the 1 s tick.

---

## 1. Spawn model — `FUN_004232d3` (the periodic spawner)

`background_monster_create` (`0x218b7`) is a thin wrapper: it calls **`FUN_004232d3`** and
reschedules itself with `my_rtkick(DAT_00482ca8, …)`. All spawn logic is in
`FUN_004232d3`.

### Spawning is player-driven — the world does not populate empty regions

`FUN_004232d3` never sweeps the whole map. It builds and consumes a small room cache
(`DAT_004942d0`, 512 entries × 12 B: `[+0]` room#, `[+4]` map, `[+8]` byte max-count /
`[+9]` byte live-count) keyed off **where players currently are**:

* **Phase B** (per logged-in user): look at the user's *own* room and decide whether to
  spawn there.
* **Phase A** (the neighbour pass): for each room in the cache, walk its 10 exits and, with
  a **~5 % chance per exit** (`genrdn(1,100) < 6`), spawn into the *adjacent* room.

So monsters appear in occupied rooms and in the ring of rooms one step out from players.
Regions with nobody in them stay empty until a player arrives. A single spawner pass is
globally capped at ~9 new monsters (`local_20 < 9`; it returns early once `8 < local_20`,
saving its cursor in `DAT_0047fba4`/`DAT_0047fba8` to resume next pass).

### Per-room spawn gate

The room's spawn behaviour is selected by **`room+0x43c`** (spawn-type, read as
`room[0x10f]` int-index in phase A):

| `room+0x43c` | behaviour |
|--------------|-----------|
| `0` / `2` | **normal timed spawn** — roll against a per-type threshold and the live/max cap |
| `3` | **swarm** — loop `generate_monster` until it fails or 9 spawned this pass |
| other (incl. absent) | no spawn |

For the normal case the spawner counts live monsters in `room+0x400` (the 15-slot live
list, §5) and compares to the room's **max-monster cap** (byte cached from the room
record). Only if `live < max` does it roll `genrdn(1,100)` against a type threshold
(`room+0x43c==2 → 0x5a`, `==0 → 5`, else `0x19`) plus a secondary `live*2 ≤ max` brake,
then calls `generate_monster`. Net effect: room type 0 spawns aggressively (low
threshold), type 2 rarely (high threshold), and the room never exceeds its cap.

### The `generate_monster` call — room fields that drive it

Both phases call (roomKey and map first):

```
generate_monster(room#, map,
                 room+0x560,   // param_3 — monster REGION / spawn-class selector
                 room+0x468,   // param_4 — forced monster number (0 ⇒ random from region)
                 room+0x462,   // param_5 — min level
                 room+0x464,   // param_6 — max level
                 -1, 0, 0)
```

### Room spawn-control fields (WCCMP001, live in-memory room record)

| off | field |
|-----|-------|
| `+0x400` (15 × 4) | **`CurrentRoomMon`** — live monster active-numbers in this room (0 = empty slot) |
| `+0x43c` | **spawn type** (0/2 timed, 3 swarm) |
| `+0x462` | spawn **min level** |
| `+0x464` | spawn **max level** |
| `+0x468` | **forced monster number** (0 ⇒ pick randomly from the region table) |
| `+0x560` | **spawn region / zone id** — selects candidate monsters; also the leash key (§3) |
| `+0x55c` | **spawn-count cap** (byte) |
| `+0x606` | **current spawn count** (byte) — bumped by `generate_monster`, decremented on death |
| `+0x562` | **respawn timer** (set on a kill in `check_kill_monster`; gates re-spawn) |
| `+0x5bc` | respawn-interval override (else `DAT_00482d0c`) |
| `+0x5c0`/`+0x5c4` | linked-room live-count / linked spawn-room number |
| `+0x5c8` + `room+0x564` bit 8 | **unique / boss** monster id, and "boss present" flag |
| `+0x5f8` | dirty flag (needs save) |

### Respawn timing after a kill

On a monster's death `check_kill_monster` (`0x24eb7`, see `death.md` §4) decrements
`room+0x606` and stamps `room+0x562` with the kill time in **minutes-since-midnight**:
`FUN_0046c3b8(now())` = `(t>>11)*60 + ((t&0x7FF)>>5)` from the DOS-packed time (the
2-second field is discarded). The `+0x5a0` nudge is **1440 = minutes per day**: applied
when the stamp falls numerically before the module boot minute `DAT_0047963a` (set once
in `init__wccmmud` — it is the boot time, not a threshold), keeping the minute clock
monotonic across midnight. `generate_monster` normalizes "now" the same way
(lines 20912-20916) and refuses to spawn until more than `room+0x5bc` minutes — or
**`DAT_00482d0c` = 5 minutes** when zero — have elapsed past the stamp
(lines 20918-20922). Spawn-type-2 rooms (`room+0x43c == 2`) bypass the timer entirely
(line 20909). So a slain monster's slot refills after a per-room cooldown — the classic
MajorMUD "respawn timer," per-room, not per-monster.

**Caps, summarised:** at most **15 live monsters per room** (the `room+0x400` array size,
enforced by `add_monster_to_room`), further limited by the room's own spawn-count cap
(`room+0x55c`) and by the monster template's world population counter
(`knmsr+0xa8`/`+0xa6`, §2). Global cap per spawner pass ≈ 9.

---

## 2. Instance generation — `generate_monster` (`0x24361`)

Turns a template (WCCKNMSR record, fetched via `get_known_monster_data`) into a live
instance. Sequence:

1. **Pre-flight gates** (return 0 = no spawn):
   * `DAT_00482134` global spawn-disable.
   * room spawn-count cap reached (`room+0x55c ≤ room+0x606`) and this is not the room's
     boss id.
   * no free slot in `room+0x400` (room already holds 15).
   * respawn timer `room+0x562` not yet elapsed (§1).
   * linked-room cap (`room+0x5c4` → that room's `+0x5be`/`+0x5c0`).
2. **Template selection.** If `param_4` (forced monster) is 0, walk the global
   monster-generation table (`DAT_004790f0`, count `DAT_004790ec`, stride 0xc): a candidate
   qualifies when its region field (`+4`) matches `param_3` (= `room+0x560`) **and** its
   level (`+8`) lies in `[param_5, param_6]`. Selection is weighted-random across matches
   (`genrdn(1,100)` with 0x1d/0x62 thresholds — later matches can displace earlier ones).
   With `param_4` set, that exact monster number is used.
3. **World-population throttle.** If the template carries a spawn counter
   (`knmsr+0xa6 != 0`, the `gamelimit` column): refuse if the counter is exhausted
   (`knmsr+0xa6 ≤ knmsr+0xa8`, `active`), and for "1 remaining" apply a real-time
   cooldown of `knmsr+0xb2 × 60` minutes (`regentime`, line 20995) computed from the
   last-kill stamp (`knmsr+0xb4/+0xb6` = `datekilled`/`timekilled`, minutes via
   `calc_minutes_difference`). On success bump `knmsr+0xa8` and mark the template
   dirty. This is the mechanism behind limited-population / rare monsters.
   *(Corrects the earlier `+0x54`-as-active-count reading — `+0x54` is the roam/zone
   class, §3, and doubles as the mongen region.)*
4. **Allocate** an instance id (`get_unique_active_monster_number`), zero a 400-byte
   instance struct, then copy template fields into it (name, level, stats, attack table,
   energy, the behaviour fields of §3/§4 — full copy map below). **HP is copied
   straight from the template (the word at `knmsr+0x78`, `hitpoints`, → both instance
   current HP `+0x18` and max HP `+0x104`, lines 21019/21028); it is _not_ rolled.**
   The only randomised values at birth are the **five coin piles**
   (`mon+0xf0/f4/f8/fc/100` = runic/plat/gold/silver/copper), each
   `lngrnd(0, knmsr_maxCoin+1)` from the maxes at `knmsr+0x108..0x118`
   (lines 21042-21056). *(Corrects the earlier `knmsr[0x18]/[0x19]`-as-HP and
   `mon+0x3c` coin claims — the `+0x60`/`+0x64` dwords are the `something2`/
   `weaponnumber` pair copied to `mon+0xac/+0xb0`.)*
5. **Carried inventory.** For each of 10 template item slots (`knmsr+0x30·i`): unless a
   per-slot drop-chance roll (`genrdn(1,100)` vs `knmsr+0xfc+i`) fails, attach the item via
   `add_logical_to_monster` (this is the same loot the monster later drops on death).
6. **Name.** Copy `knmsr+0x36` (template name — one word past the earlier `+0x34`
   guess; lines 21104-21107), or if the dword at `knmsr+0x124` (name-generator id,
   `piVar5[0x49]`) is set, roll a random name via `get_random_name`.
7. **Insert & place.** Store the record (`dfaInsertDup`, retrying up to 4 fresh ids on
   collision) and `add_monster_to_room` (§5). Bump `room+0x606` (or set the boss flag if
   this is the room's unique). Pick a **random valid entry direction** (loop over 8 exits,
   `genrdn(1,10)` weighting) purely for the arrival flavour text ("*X just arrived from the
   south.*"), then `tell_room` + `display_entry_movement`.

Returns the new instance id, or 0 on any gate failure.

### Template → instance copy map (disk-verified 2026-07-18, slice M6-1)

Pinned from `generate_monster`'s copy block (decompile lines 21012-21111) and
cross-checked against the consumers (§3/§4) and the sqlite column layout
(Nightmare `MonsterRecType` — its offsets match the WG3-NT logical record;
`load_known_monster_into_buffer`/`save_known_monster_from_buffer` read/write the
raw Btrieve record with no repacking, so **disk offsets == in-memory offsets**;
independently proven by `load_monster_quickreferences` stepping raw records with
the same +0x54/+0x5c reads):

| mon | ← knmsr | column | meaning | line |
|-----|---------|--------|---------|------|
| `+0x08` | `+0x58` (u4) | `expmulti` | herd rank (dual-use with the exp multiplier) | 21013 |
| `+0x16`/`+0x114` | `+0x7a` | `energy` | current / max energy | 21018/21031 |
| `+0x18`/`+0x104` | `+0x78` | `hitpoints` | current / max HP | 21019/21028 |
| `+0x106` | `+0xae` | `alignment` | **behaviour mode** (§4 taxonomy) | 21029 |
| `+0x108` | `+0x6e` | `follow` | **aggression** 0-100 | 21030 |
| `+0x10a`/`+0x10c` | `+0x68`/`+0x6a` | `dr`/`ac` | damage resist / armour class | 21026/21025 |
| `+0x110` | `+0x74` | `experience` | exp worth (fed to `distribute_experience`) | 21024 |
| `+0x12c` | `+0x54` | `group` | **roam/zone class** = mongen region | 21032 |
| `+0x130` | `+0x7c` | `hpregen` | HP regen per slow tick | 21020 |
| `+0x148` | `+0xaa` | `type` | **herd/leash mode** (0 none, 1/2 pack, 3 lair) | 21033 |
| `+0xac`/`+0xb0` | `+0x60`/`+0x64` | `something2`/`weaponnumber` | combat pair | 21070/21071 |
| `+0xb4+4i` | `+0xc0+4i` | `itemnumber_i` | carried item (roll vs `+0xfc+i` `itemdropper_i`) | 21074-21093 |

Template fields read in place (not copied): `+0x6c` `something3` herd id and
`+0xac` `nothing2` follower cap (move_monster pack logic), `+0xa6`/`+0xa8`
`gamelimit`/`active` population pair, `+0xb2` `regentime` cooldown factor,
`+0xb4/+0xb6` kill stamps, `+0xb8` `movemsg` arrival-message id, `+0xbc`
`deathmsg`, `+0x5c` `index` level (mongen candidate table). PLAUSIBLE only:
`+0x1be`/`+0x1ae` spawn-/death-time triggers for `FUN_00429bca`; the
`mon+0x134/+0x136/+0x138/+0x13c ← knmsr+0x1b0..+0x1b8` copies (meaning unchased).

---

## 3. Movement / wander — `move_monster` (`0x252f3`) and the wander roll

### When a wander is *attempted* (`medium_update_monster`, 3 s)

A monster considers wandering only when it is **not** locked onto a target (`mon+0x1a`
empty) and **not** paralysed (`mon+0x22 == 0`). The decision is a switch on the monster's
**roam-class field `mon+0x12c`** (read as `(short)mon[0x4b]`):

```
switch (mon+0x12c) {
  case 0: case 2:  break;                 // stationary — never wanders
  case 5:          try-wander (water/roamer path, respects a spell-active flag)
  default:         if genrdn(0,100) < (100 - aggression)/2  → try-wander
}
```

So **wander chance falls as aggression rises** (`aggression` = `mon+0x108`): a highly
aggressive monster mostly sits and ambushes; a placid one drifts. Global fairness cap:
`DAT_0047fb90 < 3` limits how many monsters wander per medium tick. A wander first rolls
`check_monster_confusion` (`0x29812`; monster ability `0x47`) — a confused monster fumbles
("*looks around stupidly*") and does not move. The chosen direction comes from
`pick_valid_random_direction`, and is rejected if it equals `mon+0x132` (the
**last-move direction**, i.e. monsters avoid immediately doubling back).

If a monster instead has a **directed-travel order** (`mon+0x22 != 0`, a target
coordinate — used by patrols / summoned / monster-vs-monster) it steps toward that coord
via `dir_monster_travelling_coord` rather than wandering randomly.

### What `move_monster` will and won't do

`move_monster(monster#, direction, notifyUser, herdFlag)` gates a step through many checks;
any failure returns 0 (stayed put):

1. **Immobility.** Template ability `0x4a` ⇒ can never move. Ability `0x44` ⇒ 50 % chance
   the step is skipped this call (sluggish).
2. **Prone.** If the monster is prone (`mon+0x128` bit 8, set by a player's smash), it
   cannot move; instead its prone timer (`mon+0x168`) ticks down.
3. **Herd / leash by pack** (`mon+0x148`, the herd-mode field, values 1/2/3):
   * `3` = **fully stationary** — bound to its lair, never takes an exit.
   * `1`/`2` = **pack members** — a monster will refuse to leave if a same-herd packmate
     (matched on `knmsr+0x6c` herd id, with a `knmsr+0x58` rank test) is present and holds
     the room; conversely, when a leader *does* move it **drags followers** the same
     direction (recursive `move_monster(..., herdFlag=1)`, up to `knmsr+0xac` of them). Packs
     move as a unit.
4. **Leash by zone** — the core wander bound. The destination room is
   `room+0x338+dir*4` (the exit's dest room, per `vir_schemas.md`). The step is allowed
   only if the destination's **`room+0x560` zone id equals the monster's `mon+0x12c`**, or
   the monster is one of the special free-roam classes:
   * `mon+0x12c == 0x25` — free roamer: enters any room whose `room+0x564` flags are clear
     and that is not a type-5 room.
   * `mon+0x12c == 0x26` — free roamer that additionally **opens/passes doors and secret
     passages**.
   * `mon+0x12c == 5` — water/roamer class: needs `room+0x564 & 2` and a non-`0x13` exit.
   Any other `mon+0x12c` value is a plain **zone id**: the monster is confined to rooms
   tagged with that same zone. This is how monsters stay in their intended area.
5. **Doors / exit type** (`room+0x360+dir*2`, per `vir_schemas.md`):
   * types `1,3,4,6,8,0xc` (locked / gate / **map-change portal** / remote) — **blocked**;
     monsters never leave the map or take gated exits.
   * type `2` **door** — blocked if the door is **closed** (`room+0x39c+dir*2 != 0`), unless
     the monster is class `0x26`. **Ordinary monsters do not open doors.**
   * types `7`/`0xb` **secret** — blocked unless class `5`/`0x26`.
   * types `9`/`0x18` **damage exits** — passable, but the monster takes
     `genrdn(dmg/2, dmg+1)` damage crossing (can drop it to 0).

On a successful step: set `mon+0x132 = direction` (anti-backtrack), `take_monster_from_room`
old room, `add_monster_to_room` new room (rolled back if the new room is full), update
`mon+0x10` (current room), push the old room onto the monster's location **trail**
(`mon+0x38…`, a 9-deep history via `memmove`), and print "*X just left…*" / "*X moves into
the room from the …*" to both rooms.

---

## 4. AI / behaviour modes

### The `mon+0x106` behaviour-mode taxonomy (completed)

`mon+0x106` (a `short`, copied from the template) decides whether and whom a monster will
*initiate* against. Reading every branch of the aggression driver `FUN_00423863`
(`0x423863`) and `give_monsters_a_free_attack` (`0x29692`):

| `mon+0x106` | class | initiation behaviour |
|-------------|-------|----------------------|
| **0** | passive | Never initiates. Only fights back once attacked (already has `mon+0x1a` set). |
| **3** | passive / sentinel | Never initiates (identical treatment to 0 in every gate). |
| **4** | passive | Never initiates. On a player's flee it swings only if it was *already* fighting that player. |
| **6** | guardian / conditional | Initiates **only against high-threat players**: aggro driver requires `player+0x542 ≥ 0x28` (fame/notoriety ≥ 40) or the player is already fighting it; a low-fame player is ignored. (The flee free-attack uses the mirror bound `player+0x542 < 0x50`.) This is the "attacks only criminals / notorious characters" guard type. |
| **1, 2, 5, …** (any other) | aggressive | Initiates against **any** valid player present. Class `5` in `mon+0x12c` (roam) is treated as *extra* aggressive (rolls against a base of 100 rather than 50). |

The `≥40` fame threshold for mode 6 is the same alignment/fame line the death code uses for
temple recall (`death.md` §3).

### Acquiring a target — `FUN_00423863` (runs inside the 5 s combat round)

For each player (walked through the shuffled terminal map `DAT_004913fc` for fairness), the
driver scans the up-to-15 monsters in that player's room. A monster is a candidate to
*acquire* when it has **no current target** (`mon+0x1a` empty) and **no travel order**
(`mon+0x22 == 0`). It then, subject to its `mon+0x106` mode above, tests each nearby player
with **`FUN_004237de`** — the target-validity predicate: player is in the monster's exact
room, is attackable, and is **not hidden** unless the monster has see-hidden ability `0x39`
(`player+0x5f6` hidden flag; `player+0x6f4` bits 4/0x40 gate safe/no-aggro states). On a
valid target it rolls the anti-pile-on chance from `combat_rounds.md` §3:

```
genrdn(0,100) < 50 - 5 * player[+0x6f0]     // +0x6f0 = times already jumped this round
```

Success bumps `player+0x6f0` and calls `attack_monster_user` (the actual swing, in
`combat_rounds.md`), which also **sets the monster's target** `mon+0x1a` to that player's
name — the monster is now "locked on."

### Pursuit of a locked target — `fast_update_monster` (1 s)

Once `mon+0x1a` is set the fast tier drives the chase every second:

* Resolve the target user. If they have **left the monster's room**, compute the direction
  toward them (`dir_player_travelling_coord`) and `move_monster` that way — i.e. monsters
  **follow fleeing players room-to-room**, subject to all the door/zone gates of §3.
* Pursuit is refused when the target recalled/left the map, went hidden (and the monster
  lacks ability `0x39`), or a per-monster follow roll fails (`genrdn(0,100) < mon+0x108`
  aggression — a low-aggression monster may lose the trail).
* Every failed follow bumps the **give-up counter `mon+0x124`** (byte). When it exceeds
  `0xf` (15): a free-roam class-`0x25` monster **despawns** (`FUN_004298ec` — removes it
  from the room, decrements the spawn count, frees the instance); any other monster simply
  **drops the target** (`mon+0x1a = 0`, counter reset) and reverts to wandering.
* The fast tier also runs **prone recovery**: `mon+0x128` bit 8 with countdown `mon+0x168`;
  on expiry it prints "*…rises from the ground*" and clears the prone bit.

### Flee handling — `give_monsters_a_free_attack` (`0x29692`)

Called from `move_user` whenever a player walks or flees (`combat_rounds.md` §3). One
`genrdn(0,100)` roll for the whole room; the first monster whose **aggression `mon+0x108`
≥ that roll** and whose `mon+0x106` mode permits gets a single parting swing. Passive modes
(0/3/4, or roam-class `0x25`) only swing if already targeting the fleer; aggressive modes
swing regardless. Gated to once per round via `player+0x6f0`. A nonzero return (the player
lost a life / died) aborts the move — the monster's blow can stop the escape.

### Peripheral: `monster_update_room_users_stats` (`0x2635d`)

Not spawn/movement: for each player flagged `player+0x7c8 & 0x10` (secondary stats dirtied
by a monster aura/effect) it re-runs `calculate_secondary_stats(user, 2)`. Housekeeping so
monster stat-drain/aura effects reflect in the player's sheet.

---

## 5. Per-tick update — what each tier does per monster

### `slow_update_monster` (`0x21c06`, 30 s) — regen only

Guarded so it runs once per slow cycle per monster (`mon+0x144` stamp vs `DAT_0047fb8c`).
It then:

* resets `mon+0x132` (last-move direction) to `0xffff`;
* applies a **periodic HP drain**: if `mon+0x14 > 0`, `HP(mon+0x18) -= mon+0x14` (bleeding /
  decaying / summoned-monster decay);
* **regenerates HP**: if `HP < maxHP(mon+0x104)`, `HP += mon+0x130` (regen rate), clamped to
  max.

No wander, no despawn, no ability upkeep happen here.

### `medium_update_monster` (`0x21cbc`, 3 s)

* **Spell-effect upkeep** for the 5 effect slots (`mon+0x14a` spell id, `+0x154` caster,
  `+0x15e` duration): tick each duration, run `perform_routine_spell_monster_upkeep`, expire
  at 0.
* **Environmental-death check**: if `HP < 0`, call `check_kill_monster(mon,-1)` and, on
  death, `distribute_experience` + `kill_autocombat_against_monster` (a monster that walked
  into a damage exit or bled out dies here).
* **Wander decision** (§3) — the `mon+0x12c` switch and the `(100-aggression)/2` roll — or
  directed travel toward `mon+0x22`.
* Clears the per-tick "acted" bit `mon+0x128 & 2`.

### `fast_update_monster` (`0x21f24`, 1 s)

Prone recovery + target pursuit / give-up / despawn (§4). Only invoked for monsters that
hold a target.

---

## 6. Live monster-instance field map (in-memory)

Consolidated from the functions above (offsets are byte offsets into the instance record;
"int-idx" notes where the decompile indexes it as a dword for the same address).

| off | field |
|-----|-------|
| `+0x00` | active monster number (unique id) |
| `+0x04` | known-monster **template id** (WCCKNMSR number) |
| `+0x0c` | map number |
| `+0x10` | current room number |
| `+0x14` | periodic HP-drain per slow tick |
| `+0x16` | current energy (attack budget) — `combat_rounds.md` |
| `+0x18` | **current HP** (short) |
| `+0x1a` | **aggro target name** (string; empty = no target) |
| `+0x88` | directed-travel / paralysis word (int-idx 0x22; patrol coord; 0 = free) |
| `+0x38…` | room **location trail** (9-deep history) |
| `+0x8e` | monster **name** (display; terminator at `+0xab`) |
| `+0xf0…+0x100` | coin piles (runic/plat/gold/silver/copper; rolled at spawn) |
| `+0x104` | **max HP** (short) |
| `+0x106` | **behaviour mode** (§4 taxonomy) |
| `+0x108` | **aggression** rating 0–100 (int-idx 0x42) |
| `+0x110` | **experience worth** (← `knmsr+0x74`; fed to `distribute_experience`) |
| `+0x120` | home/spawn room (read by `check_kill_monster`) |
| `+0x12e` | engaged-user number (0xffff = none; set on attack, cleared per energy tick) |
| `+0x114` | max/regen **energy** — `combat_rounds.md` |
| `+0x116` | attack-suppression flag (byte) |
| `+0x124` | **pursuit give-up counter** (byte; >15 ⇒ drop target/despawn) |
| `+0x128` | status flags (int-idx 0x4a): bit 1 charmed/controlled, bit 2 acted-this-tick, **bit 8 prone** |
| `+0x12c` | **roam / zone class** (int-idx 0x4b): 0/2 stationary, 5 water-roamer, 0x25 free-roamer(despawns), 0x26 door-opener, else zone id matched to `room+0x560` |
| `+0x130` | HP **regen rate** (short) |
| `+0x132` | **last-move direction** (0xffff = none) |
| `+0x140` | dirty flag (int-idx 0x50) |
| `+0x144` | last slow-tick stamp |
| `+0x148` | **herd / leash mode** (int-idx 0x52): 3 stationary/lair, 1/2 pack |
| `+0x14a/+0x154/+0x15e` | 5 spell-effect slots (id / caster / duration) |
| `+0x168` | prone / daze timer (int-idx 0x5a) |

---

## 7. Open / uncertain items

*(Slice M6-1, 2026-07-18: the interval literals and the template→instance offset map are
CLOSED — values below and the §2 copy-map table; extraction method: static `.data` reads
from `wccmmud.dll` validated against the four known metronome globals, plus a full
decompile pass over `generate_monster`.)*

* **Interval literals — CLOSED.** Spawn cadence `DAT_00482ca8` = **5 s** (static `.data`;
  a sysop `configure genrate` override exists but is gated to BTURNO `07356801`, the
  Metropolis dev system). Default respawn `DAT_00482d0c` = **5 minutes** (`configure
  minwait`, same gate). `DAT_0047963a` = the module **boot minute** (not a constant);
  `0x5a0` = 1440 minutes/day midnight wrap; `FUN_0046c3b8` = DOS-packed-time →
  minutes-since-midnight (§1). Wander fairness cap: compare is `DAT_0047fb90 < 3`,
  reset to 0 in `medium_update_monsters` (0x21b31) once per 3 s tick; the case-5
  water/roamer path additionally bypasses the cap when `(char)mon[0x50] != 0`.
  `DAT_00482134` (global spawn disable) is the **crash-recovery flag**: set during the
  recovery rebuild in `preload_and_generate_buffers`, cleared when recovery completes —
  and temporarily zeroed around the **boot-time lair/permanent `generate_monster`
  calls** (lines 27702-27717), i.e. lair/permanent monsters are populated at module
  boot, not on player approach.
* **HP is not rolled.** `generate_monster` copies template HP directly; only coins (and
  which carried items attach) are randomised at birth. If a per-monster HP range exists it
  would have to live in the template as pre-rolled min/max the engine picks elsewhere — not
  seen in this function. Flagged for template-schema follow-up.
* **Template → instance offset map — CLOSED.** See the §2 copy-map table:
  `mon+0x106` ← `knmsr+0xae` (`alignment`), `+0x108` ← `+0x6e` (`follow`),
  `+0x12c` ← `+0x54` (`group` — dual-use as the mongen region matched against
  `room+0x560`), `+0x148` ← `+0xaa` (`type`), `+0x130` ← `+0x7c` (`hpregen`).
* **`mon+0x22` dual use.** It reads as both a paralysis gate (medium-tick wander skips when
  nonzero) and a directed-travel coordinate (moves toward it, or `attack_monster_monster`
  for monster-vs-monster). The two uses share the field; the disambiguating flag was not
  fully chased. (Byte offset is `+0x88` — the doc's `+0x22` is the decompile's int-index.)
* **`DAT_004906c9 == 2`** blocks spawning of roam classes 5/0x25 (line 20976) — some
  config/holiday mode, not identified.
* **`mon+0x116` and `mon+0x128` bit 1** ("suppress attack" / "charmed") are named from
  usage, not symbols.
* **Class-5 vs the tautological gate** in `FUN_00423863` line ~20371
  (`(x<5) || (x!=5)`) routes only `mon+0x12c == 5` into the more-aggressive branch; read as
  a compiler artefact of an original `x <= 5 && x != 5`-style test — treated here as "class
  5 is special," consistent with `move_monster`/`give_monsters_a_free_attack`.

## Addendum — loot mechanics pinned (2026-07-16, decompile pass)

- `itemdropper` is a **carry chance, rolled at spawn** (`generate_monster`
  step 5: `genrdn(1,100) <= knmsr+0xfc+i`), not a death-time drop roll.
  The monster genuinely holds the item all its life (this is what
  `rob_monster` steals from).
- `check_kill_monster` drops **everything carried, unconditionally and
  silently** — no message; the items simply join the room's
  "You notice … here." line after the coin-drop messages.
- The wielded `weaponnumber` is copied to the instance for combat but is
  **not in the death drop loop**. Monsters that "drop their weapon" list
  it again in a loot slot (157 templates do, e.g. guardsman's short
  sword at 10%).
- **First-kill guarantee**: the spawn roll is skipped (guaranteed carry)
  when the template is limited-population (`knmsr+0xa6 != 0`) and its
  last-kill stamp (`+0xb4`) is zero — a rare monster's first-ever kill
  on a board always yields its full loadout. Deferred to M6 with the
  population/respawn stamps.
- `dispose_of_item_in_room` retries, then recursively spills into
  adjacent rooms (skipping exit types 8 and 0xc) when the room's 10
  floor slots are full. Deferred with the floor-slot cap itself.
- Data: 409/1101 monsters carry loot; 141 have a 100% slot; one shipped
  dangling ref (saracen commander 602 → item 2078, allowlisted).
