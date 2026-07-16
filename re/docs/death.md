# MajorMUD 1.11p — DEATH / DYING / respawn system (WG3-NT)

Behavioral spec of what happens when a player or monster reaches 0 HP: the death
trigger, the penalties applied, where and how the player respawns, and how a killed
monster drops loot and its experience is split among attackers.

Reconstructed from the clean 32-bit **WG3-NT** decompile
(`../wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Death is *detected* by the round loop
in [`combat_rounds.md`](combat_rounds.md) §4/§5 — when a swing drops the target's HP the
loop calls `check_kill_user` / `check_kill_monster`; a nonzero return means "died", and
the loop then awards exp via `distribute_experience` and tears down autocombat. Record
offsets are consistent with [`records.md`](records.md); the exp curve
(`new_calc_exp_needed`) is in [`leveling.md`](leveling.md) / `records.md`.

Function addresses (image base 0x400000): `check_kill_user` `0x19a33`,
`check_kill_monster` `0x24eb7`, `distribute_experience` `0x4c990`,
`add_experience` `0x16818`.

---

## 1. Player-death trigger — `check_kill_user(playerPtr, userNum)`

`check_kill_user(param_1, param_2)`: `param_1` is the player record (0 ⇒ look it up via
`get_player(param_2)`), `param_2` is the user number. It is called every place damage is
applied to a player: the PvE monster driver `attack_monster_user`, the PvP driver
`attack_user_user`, damage-shield / proc paths, and spell/DoT upkeep. Returns **1 = the
player died and was handled**, **0 = still alive** (the round loop uses this as its
break flag).

**The 0-HP gate is two-stage — there is a "downed but not dead" band:**

```
if (player == 0 || player[+0xb0] > 0)   return 0;      // HP>0 ⇒ alive, nothing to do
// HP <= 0 from here:
if ( (room[+0x10f]==5 && DAT_004790e8)   // died in a "collision"/hazard room (type 5)
     || player[+0xb0] <= DAT_00482cf0 )  // HP fell to/below the death floor
{  ...full death handling, return 1;  }
else { player[+0x6f8] = 0xffff;  return 0; }            // HP<=0 but above floor ⇒ NOT dead yet
```

* `player+0xb0` = current HP (`records.md`).
* **`DAT_00482cf0` is the death floor**, a (negative) `.data`-configured threshold. A
  player at HP in the open interval `(DAT_00482cf0, 0]` is at/under zero but **not
  killed** — the function just sets `player+0x6f8 = 0xffff` and returns 0. Actual death
  requires HP to reach the floor (or a hazard-room death). This is the engine's
  incapacitation / negative-HP grace window. (The exact floor value is a `.data` default
  not assigned in code; not extracted.)
* **Hazard/"collision" rooms**: `room+0x10f == 5` is a room type that, when the global
  `DAT_004790e8` is enabled, kills on contact regardless of the HP floor and routes to the
  lighter penalty branch (§3).

Before branching, on *any* death the handler unconditionally:
`stop_following`, `clear_search_flags`, prints **"You have been killed."**, optionally
`_MMLog` (if `DAT_004906d4==1`), `cleanup_when_user_leaves`, clears combat/target flags
(`+0x6f4 &= ~2`, `+0x6f8=0xffff`, `+0x6f6=0xffff`, `+0x6ba=0xff`, `+0x6b8=0`, `+0x6bb=0`,
`+0xcc=0`), announces **"%s is dead."** to the room, then **strips every active
spell/buff**: it walks the 10 buff slots (`+0x40` spell ids, `+0x54` durations, `+0x68`)
and calls `perform_spell_termination_player_upkeep` on each, zeroing them. Combat energy
`+0xbe` is zeroed.

---

## 2. Death penalties

There are **two penalty tiers**, selected at `0x419e5c`:

```
if (DAT_004790e8 == 0 || room[+0x10f] != 5)   → FULL death (drops + life lost)
else                                          → COLLISION death (light; no drops)
```

### 2a. Full death (normal case)

Everything the player carries is **dropped into the room** (or the death room):

* **All money.** The 5 coin denominations are added to the room's coin piles and the
  player's are zeroed:
  `room[0x14d..0x151] (= room+0x534..0x544) += player[+0x610..+0x620]`, then
  `player+0x610/+0x614/+0x618/+0x61c/+0x620 = 0`. These map to
  **runic / platinum / gold / silver / copper** (same pile offsets a dead monster uses,
  §4).
* **All carried inventory** (up to 100 slots at `player+0xd8`, state words at `+0x268`):
  each is removed and placed with `dispose_of_item_in_room`; if the room can't hold it,
  fall back to `dispose_of_item_in_trail`, then to a default limbo room `(map 0xa4, coord 1)`.
  Items flagged "return to owner/home" (`item[0x10b] != 0`) are sent home instead
  ("Your %s has returned to its rightful …").
* **All keys** (50 slots at `player+0x334`, state at `+0x3fc`): dropped the same way.
* **Worn equipment** (20 slots `player+0x62c`, weapon `player+0x624`): each is
  `unequip_apply_item_abilities`'d (its stat/ability bonuses removed) and the worn-slot
  reference cleared.

**Item-retention rule when the player has more than one life left:** in the inventory
loop, if `player[+0x6a6] (lives) == 1` **every** item drops. Otherwise an item is kept
(not dropped) only if it has ability **0x64** or **0x53** ("keep on death"); all other
items still drop. (So on your *last* life you always lose everything; with spare lives you
retain 0x64/0x53-flagged items.)

**Death-trigger item:** an item with ability **0x9b** yields a value (`get_item_ability_value(0x9b,…)`)
stashed in `local_10`; after respawn the engine runs it as a special text-block command
(`perform_text_block_as_special_command`) — a "on-death" scripted effect (e.g. curse/teleport).

**Life lost:** `player+0x6a6` ("lives") is **decremented by 1**.

> **No experience is lost on death.** `check_kill_user` contains no exp subtraction, and
> `add_experience` (`0x16818`) only ever *adds* (`param_2 > 0` required; player exp lives
> at `+0x470/+0x474`). The MajorMUD death penalty is **drop-everything + a life**, never
> de-leveling. (Corrects the "% of level" assumption in the task brief — there is no exp
> penalty in this build.)

### 2b. Collision / hazard-room death (light)

If you die in a type-5 room with `DAT_004790e8` on: **no items or money drop, no life is
consumed.** HP is set from `player+0x712` (a stored "collision-survival HP"), mana
restored (`+0x602 = +0x600`), you are moved to the recall target (§3), and the message is
**"But, because you were in a colli[sion]…  You have %d lives left."**

### 2c. Permadeath (out of lives)

After decrementing, `if (player+0x6a6 < 1)` the character is **deleted**:
`save_evil_points`, stamp death date `+0x7d2 = today()`, `save_permanent_info`,
`remove_from_gang`, `remove_users_bankbooks`, **`delete_player` + `clear_player`**, print
**"You have no lives remaining."**, show the main menu, and disconnect the session
(`usroff(user)+0x1c = 0x38`; `rstmbk`). The character record is gone.

> Whether "lives" are effectively infinite depends on the sysop's starting-lives config;
> the mechanism above is what the code does when the counter is finite.

---

## 3. Respawn

On a **surviving** full death (lives ≥ 1) the player is rebuilt at
`0x419f6a`:

* **HP fully restored:** `player+0xb0 = player+0xae` (current = max HP).
  (Collision branch instead uses `player+0x712`.)
* **Mana fully restored:** `player+0x602 = player+0x600`.
* Status `+0xb4 = 0`; `update_weight_carried`; `calculate_secondary_stats`.
* **Moved to the recall room:** `player+0xc4 = local_c` (coord), `player+0xc8 = local_8`
  (map/region) — see `records.md` player-location offsets `+0xc4/+0xc8`.
* `save_player`, prompt refresh, and to the room: **"%s appeared on the floor in the
  m[iddle]…"**. Then the on-death 0x9b special command fires if present.

**No respawn timer or incapacitation delay** — you are placed in the recall room, alive at
full HP/mana, immediately. (The only "delay" is the negative-HP grace band of §1 before
death is finalized.)

### Recall-target selection

The destination `(local_c coord, local_8 map)` is chosen at the top of the handler:

```
local_c = 1;                                   // default coordinate
local_8 = (player[+0x542] < 0x28) ? DAT_00482cfc   // alignment/fame < 40  → temple A
                                  : DAT_00482d00;   //                >=40  → temple B
```

* `player+0x542` is the alignment / fame-threat value (see `combat_rounds.md` §3).
  Below 40 you recall to one temple (`DAT_00482cfc`); at/above 40 to another
  (`DAT_00482d00`) — i.e. good vs. evil (or low- vs. high-notoriety) recall points. Both
  are `.data`-configured map ids (values not extracted).

**Per-room DeathRoom override:** immediately before dropping loot, the *room the player
died in* can redirect the recall:

```
if (room[0x15b] /* room+0x56c */ != 0) { local_8 = room[0x56c];  local_c = room[0]; }
```

So a room record carrying a nonzero **DeathRoom field (`room+0x56c`)** overrides the
default temple — the death destination becomes that room (its paired coordinate read from
`room[0]`). This is the `DeathRoom` concept from `records.md`: dungeons/zones can pin
where their victims respawn. *(The exact coordinate source `room[0]` is inferred from the
assignment; flagged as the one soft spot in the respawn-target read.)*

---

## 4. Monster death — `check_kill_monster(monsterPtr, userNum)`

`check_kill_monster(param_1, param_2)`: `param_1` = live monster instance, `param_2` =
killer user number (or **-1** for a monster/environmental kill — suppresses the
per-user output via `clrprf` instead of `tell_user`). Guard:
`if (monster==0 || (short)monster[6] > 0) return 0;` — **`monster+0x18` (`param_1[6]`) is
current HP**; > 0 ⇒ not dead. Returns **1** on death.

Sequence:

1. **Respawn bookkeeping** (via the monster's home room, keyed `monster[0x48]/monster[3]`):
   decrement the room's live-spawn count (`room+0x5c0`), set the room's **respawn timer**
   `room+0x562 = now()+FUN_0046c3b8()` (nudged up by `0x5a0` if below `DAT_0047963a`), and
   flag the room dirty (`room+0x5f8 = 1`). A "boss"/unique slot (`room+0x5c8`) and a linked
   spawn room (`room+0x5c4`) are handled specially. This is what lets the world-upkeep
   spawner regenerate the monster later.
2. **Remove from the current room:** clear the monster's slot in the room's 15-entry
   monster array (`room+0x400`, stride 4).
3. **Drop money:** the monster's 5 coin fields `monster[0x3c..0x40]` (**runic / platinum /
   gold / silver / copper**) are added to the same room coin piles `room+0x534..0x544`,
   each with a "%s <coin> drop to the ground." message.
4. **Death message:** if the known-monster template (`get_known_monster_data`) has a return
   message (`knmsr+0xbc`), print it to the room ("Returning …").
5. **Drop loot items:** loop 10 carried slots `monster[0x2d + i]` (item ids; state word at
   `monster+0xdc + i*2`). For each present item, `remove_logical_from_monster` — **and only
   if that returns nonzero** is the item placed in the room via `dispose_of_item_in_room`.
   That predicate is the drop gate: whether a given carried item actually drops is decided
   inside `remove_logical_from_monster` (loot-table / drop-chance logic — the numeric drop
   % is *not* a literal in this handler; it lives in the item/monster loot data, cf.
   `vir_schemas.md`).
6. **Spawn-limit accounting:** if `knmsr+0xa8` (remaining-spawns counter) is set, decrement
   it and stamp last-kill `knmsr+0xb4 = today()`, `knmsr+0xb6 = now()`.
7. **Free the instance:** `delete_monster`, and push the id onto the recycled-monster free
   list (`DAT_00480ea8/DAT_00480eac`).

**Monsters award no exp inside `check_kill_monster`.** The kill's exp value is produced by
the attack and paid out by the *caller* (§5).

---

## 5. Experience award & split — `distribute_experience`

Called by the round loop right after a kill returns 1:

```
distribute_experience(killerUser, expAmount, victimUser, victimMonsterId, victimMap, victimCoord)
```

* PvE monster kill: `distribute_experience(-1, result[4], -1, monsterId, map, coord)`
  (`param_3==-1`, `param_4==monsterId`). `expAmount = result[4]` is the kill/experience
  field of the `calculate_attack` result struct (`combat_rounds.md` §5); for
  environmental/DoT monster deaths the monster's own worth `monster[0x44]` (`+0x110`) is
  passed instead.
* PvP kill: `distribute_experience(-1, result[4], victimUser, 0xffff, map, coord)`
  (`param_3==victimUser`, `param_4==0xffff`).

Guard: acts only if there is a real victim (`param_3 != -1 || param_4 != 0xffff`) **and**
`expAmount != 0`.

### The split is an EQUAL division among all engaged participants — not damage-weighted

Two symmetric branches (monster victim keyed on autocombat-record target-monster
`rec+0x04`; user victim keyed on target-user `rec+0x00`; record base `DAT_004877e8`,
stride 0x14, per `combat_rounds.md` §4). Both do:

**Pass 1 — count recipients** (`local_8` starts at **1** for the killer):
for every terminal, `local_8++` if the user
  * is targeting the victim in autocombat (`rec+0x04 == victimMonster`, or
    `rec+0x00 == victimUser`) **and** `is_inside_autocombat` **and** isn't the killer; **or**
  * is a **co-located assister** — in autocombat but with no explicit target
    (`rec+0x04 == 0xffff && rec+0x00 == -1`), `is_inside_autocombat`, and standing in the
    victim's room (`player+0xc8 == victimMap && player+0xc4 == victimCoord`).

**Divide:** `expAmount = expAmount / local_8;  if (expAmount == 0) expAmount = 1;`
(each share is at least 1).

**Pass 2 — pay out:** every counted user (including the killer) gets
`add_experience(user, share, 1)`. Direct attackers additionally get
`kill_autocombat(user,1)` + `display_autocombat_broken(user,0)` (their combat ends since
the target is dead); co-located assisters just receive exp.

> **Split rule:** total kill exp is divided **equally** among the killer plus everyone
> engaged on that target (and co-located assisters who are in autocombat) — a flat
> per-head split, each share ≥ 1. **There is no "most-damage-dealer" weighting and no
> larger cut for the killblow.** Solo kills give the full value to one player (`local_8`
> stays 1). This is party/leech-friendly: proximity + being in autocombat is enough to
> share.

`add_experience` (`0x16818`) then adds the share to `player+0x470/+0x474`, capped against
the next-level requirement (`new_calc_exp_needed`, `leveling.md`) — it refuses exp that
would push you past a level you haven't earned ("You have progressed too far…") and warns
if the character's exp table hasn't been reset.

---

## 6. Edge cases

* **Incapacitation / negative-HP band.** HP `≤ 0` does **not** mean dead. Death is
  finalized only at `HP ≤ DAT_00482cf0` (a negative floor) or in a hazard room. In the
  band `(floor, 0]` the player survives with `+0x6f8 = 0xffff` set and
  `check_kill_user` returns 0. (§1)
* **PvE vs PvP death.** Both route through `check_kill_user` identically for penalties and
  respawn; the only difference is upstream — the killer/exp-split call uses
  `param_3=victimUser, param_4=0xffff` for PvP vs `param_3=-1, param_4=monsterId` for PvE
  (§5). The victim's item drop / life loss / recall is the same in both.
* **Killblow / who gets credit.** No special killblow bonus. The player who lands the
  fatal swing is `param_1` (always in the recipient set), but exp is split evenly with all
  co-engaged players (§5).
* **Hazard/collision rooms** (type `0x10f == 5` with `DAT_004790e8`): kill on contact,
  skip the HP floor, and use the *no-drop, no-life-loss* penalty branch (§2b).
* **Return-to-owner items** (`item[0x10b] != 0`) and quest/keys go home rather than to the
  room; retention-flagged items (ability 0x64 / 0x53) survive death while you still have a
  spare life (§2a).
* **On-death scripted item** (ability 0x9b) fires a text-block command after respawn.
* **Monster/environmental kills** pass killer `= -1`; `check_kill_monster` then suppresses
  per-user prints, and the exp call still splits among any engaged players in the room.
* **Followers.** Death calls `stop_following(user,-1)` up front — a dying player is removed
  from any follow relationship before being moved.

---

## Open / uncertain items

* **Death-floor value** `DAT_00482cf0`, **recall maps** `DAT_00482cfc` / `DAT_00482d00`,
  hazard-room global `DAT_004790e8`, and collision-survival HP `player+0x712` are all
  `.data` defaults not assigned in the decompiled code — mechanism is certain, literal
  values not extracted.
* **DeathRoom paired coordinate.** `room+0x56c` overriding the recall map is clear; the
  paired coordinate being `room[0]` is read straight from the assignment but the field's
  wider semantics weren't chased into the room schema.
* **Item drop probability.** Whether a monster's carried item drops is gated by
  `remove_logical_from_monster`'s return; the numeric drop chance / loot-table format lives
  in the item & monster data (`vir_schemas.md`), not in `check_kill_monster`.
* **"Lives" (`player+0x6a6`).** The decrement-and-permadelete logic is certain; the
  starting value / whether it is effectively unlimited is a config/creation concern not in
  these functions.
* **`monster+0x18` HP vs. the `monster+0x0c` reference** in `combat_rounds.md` §5: the two
  cite different offsets because §5 reads the *fighter* struct mid-swing while
  `check_kill_monster` reads the *live-instance* HP word — same quantity, different view;
  not reconciled field-by-field here.
</content>
</invoke>
