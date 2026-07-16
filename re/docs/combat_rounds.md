# MajorMUD 1.11p — Combat ROUND / auto-combat system (WG3-NT)

Behavioral spec of the round loop that *drives* combat. Single-attack resolution
(to-hit, damage, crit, parry) lives in [`combat.md`](combat.md) and is called from here
via `calculate_attack`; this document does not re-derive it. Ability ids referenced are
from [`abilities.md`](abilities.md); record offsets from [`records.md`](records.md).

Reconstructed from the clean 32-bit **WG3-NT** decompile
(`wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Timer defaults were read directly from
the PE `.data` section of `wg_nt_ref/WCCNT8PJ/out/wccmmud.dll` (image base 0x400000).
WG3-NT is authoritative; a few word→dword layout notes vs the 16-bit build are flagged.

---

## 1. Round cadence

### Two-layer timer system

* **`rtkick(nsec, fn)`** — the Worldgroup real-time kick (host API). First arg is in
  **seconds**. Used only to reschedule `background_fast`.
* **`my_rtkick(delay, fnaddr)`** (`0x21831`) — an in-DLL scheduler. It enqueues a callback
  at `DAT_0047fb70 + delay` into a sorted list (`DAT_0047fb74`). `DAT_0047fb70` is a
  free-running tick counter **incremented once per `background_fast`**, and the list is
  drained by **`perform_kicks`** (`0x217ec`), which `background_fast` calls every tick.
  So `my_rtkick` delays are expressed in *fast-ticks* (= seconds, since `background_fast`
  runs at 1 s).

### The four background jobs (self-rescheduling)

`background_fast` (`0x21719`) is the metronome: it runs `perform_kicks`,
`fast_update_characters`, `fast_update_monsters`, and reschedules itself with
`rtkick(DAT_00482cc4, background_fast)`. Each of the other three reschedules itself with
`my_rtkick`. Interval globals (never reassigned in code → their `.data` defaults hold):

| job | reschedule interval | default (`.data`) | period | drives |
|-----|--------------------|-------------------|--------|--------|
| `background_fast`   (`0x21719`) | `DAT_00482cc4` | **1**  | **1 s**  | tick counter, per-tick char/monster updates |
| `background_medium` (`0x216e6`) | `DAT_00482cc0` | **3**  | **3 s**  | medium char/monster updates |
| **`background_energy`** (`0x2122f`) | `DAT_00482d30` | **5** | **5 s** | **the combat round** |
| `background_slow`   (`0x215f6`) | `DAT_00482cbc` | **30** | **30 s** | item restock, random events, world upkeep |

### The combat round = `background_energy`, every 5 s

`background_energy` is the round tick. Each firing, in order:

1. **Regenerate energy** — for every logged-in user `energy_update_character(i)`
   (`0x2131f`), for every live monster `energy_update_monster(m)` (`0x213ab`).
   Energy is the per-round attack budget (see §2).
2. **Resolve combat** — with the two drivers run in a **coin-flipped order** (so neither
   players nor monsters get a systematic first-strike advantage across rounds):

   ```
   if genrdn(0,100) < 60:  do_autocombat();  FUN_00423863();   // players first
   else:                   FUN_00423863();  do_autocombat();    // monsters first
   ```
   * `do_autocombat` (`0x4c5bb`) = **player/auto-combat** driver (§2, §4).
   * `FUN_00423863` = **monster aggression** driver (§3).
3. Set per-user "energy pulse" flags (`player+0x700 |= 4`, tweak `+0x6f4`) so clients
   refresh the round display.

> **Round period is therefore 5 real seconds.** The 1/3/5/30 interval values are certain
> (read from the binary); the absolute second mapping rests on `rtkick`'s Worldgroup
> convention of seconds, which `background_fast` reschedules at `1`.

---

## 2. Attack sequencing — attacks per round

### One dispatch per engaged combatant per round

`do_autocombat` walks a ring buffer of "users currently in autocombat"
(`DAT_004969ec`, 0x226 = 550 slots; dequeued by `FUN_0044c555`) and calls
`do_autocombat_for_user(i)` (`0x4c5e9`) for each. That function reads the user's
**autocombat record** and dispatches exactly one of:

* `attack_user_monster(user, monsterId)`   — PvE melee (target record `+0x00 == -1`)
* `attack_user_user(user, targetUser, 1)`  — PvP melee
* `cast_*_target` / `cast_no_target`       — if the record's spell flag is set

So each combatant makes **one driver call per round**; *multiple attacks happen inside
that call*.

**Autocombat record** — base `DAT_004877e8`, stride `0x14` (20 B), indexed by user number
(`init_autocombat` `0x4c4d6`):

| off | field | notes |
|-----|-------|-------|
| `+0x00` | target **user** number | `-1` (0xffffffff) ⇒ not a PvP target |
| `+0x04` | target **monster** id | `0xffff` ⇒ no monster target |
| `+0x08` | **attack type** | default `5`; `get_my_attack_type` returns this |
| `+0x0c` | spell flag (byte) | 0 = melee, else cast |
| `+0x10` | spell id | when `+0x0c` set |

### The energy model (governs attack count)

Each combatant has an **energy pool** that is spent per swing:

* Player: `player+0xb8` = **max / per-round regen**, `player+0xba` = **current**
  (both default `1000` = `DAT_00482cd0`; set on character load, `_Dest+0xb8/+0xba`).
* Monster: `mon+0x114` = max/regen, `mon+0x16` = current.

`energy_update_character` (each round) does `cur += max` and, **unless the user is inside
autocombat**, clamps `cur` to `max`. While actively fighting the clamp is skipped, so a
weapon slower than one round lets energy accumulate over several rounds until a swing is
affordable — yielding a *fractional* attacks-per-round. `energy_update_monster` always
clamps `cur` to `max` (monsters never bank energy).

**Cost per swing (EU)** is produced by `calculate_attack` in the result struct
(field `[5]`, labeled `EU` in the engine's own debug print). For a player it is computed
in `move_player_to_fighter` via **`compute_energy_used`** (`0x2a0c8`):

```
i   = agility*classWeaponFactor + 45              // agility = player+0x94
den = i * (stat+150) * 1500 / 9000                // stat = player+0xaa
EU  = attackSpeedConst * 1000 / den               // attackSpeedConst per weapon / attack-type
```

EU **falls** as agility, the speed stat, and weapon speed rise → more swings. In effect
**attacks/round ≈ pool(+0xb8) / EU**, capped at 6 (below). Special attack types set
`EU = full pool` (`param_2[5] = player+0xb8`), i.e. exactly **one** swing that round
(backstab, bash, smash). Monster EU comes from the known-monster per-attack speed array
(`knmsr+0x190+form*2`), percent-scaled by haste.

### The swing loop (identical shape in all three attack fns)

`attack_user_monster` (`0x2cfab`), `attack_user_user` (`0x42bd26`), and
`attack_monster_user` (`0x42e34b`) each run:

```
count = 0
while (!doneFlag && !targetDead && count <= 5) {   // 5 < count  ⇒ stop  → max 6 swings
    count++
    result = calculate_attack(&attackerFighter, defenderFighter)   // one resolution
    if (attackerEnergy < result[5]) break            // can't afford → round ends
    attackerEnergy -= result[5]                       // spend EU
    ... apply result (see §5) ...
}
```

So **a round grants up to 6 swings**, the actual number gated by `energy / EU`. The hard
cap of 6 is the loop bound `5 < count`.

### Attack-type selection (feeds `get_my_attack_type` / `calculate_attack`)

The pending type sits in globals `DAT_004877e4` (and mirror `DAT_004877e0`), copied into
record `+0x08` by `engage_autocombat`. It is chosen by the initiating command, then passed
to `calculate_attack` (see the attack-type table in `combat.md`):

| cmd | type | meaning |
|-----|------|---------|
| `cmd_attack` (`0x514f1`) | **5** | normal weapon; **1** if unarmed w/ martial-arts (abil `0x1d`); **4** if unarmed **and hidden** |
| `cmd_backstab` (`0x5156f`) | **4** | backstab (requires weapon ability `0x74`); falls back to **5** if not hidden |
| martial-arts styles | **1 / 2 / 3** | fists-of-fury / kick styles (`move_player_to_fighter` builds the unarmed profile) |
| `cmd_bash` (`0x516f1`) | **6** | bash (×3 damage per `combat.md`) |
| `cmd_smash` (`0x5167a`) | **7** | smash (requires ability `0x20`; ×5 damage; can knock monster prone) |

**Backstab is one-shot**: after the first swing the loop forces `DAT_004877e4 = 4→5`
(so only the opening strike is a backstab), and `clear_backstab_attack_type` (`0x4bd37`)
resets record `+0x08` from 4→5 for subsequent rounds. Monster melee always sets type 5
(`attack_monster_user` line ~26776); the *form* (bite/claw) is chosen separately (§3).

---

## 3. Monster behavior

### Aggression driver — `FUN_00423863` (runs inside the round, §1)

Iterates all logged-in users through a **shuffled** terminal map (`DAT_004913fc`,
reshuffled by `shuffle_mappings` each slow tick — spreads aggro fairly). For each user's
room it scans the up-to-15 monster slots (`room+0x400`, stride 4). A monster is a
candidate to attack when it is not already committed to a named target (`mon+0x1a`
empty), not paralyzed (`mon+0x22 == 0`), and its behavior mode (`mon+0x106`) is
aggressive. It then rolls:

```
chance = 50 - 5 * player[+0x6f0]     // 0x32 - 5·(attacks already taken this round)
if genrdn(0,100) < chance:
    player[+0x6f0]++                  // pile-on counter
    attack_monster_user(monsterId, user)
```

`player+0x6f0` counts how many times the player has already been jumped this round, so
each additional attacker is progressively less likely — this is the **anti-pile-on /
aggro-spreading** rule. A monster already locked onto a named target (`mon+0x1a` set)
attacks that target directly (separate branches). `mon+0x106` modes seen: `0`, `3`, `4`
behave as non-initiating/sentinel; `6` is conditional on player fame/threat
(`player+0x542`); the exact taxonomy is only partially recovered.

### Monster swing execution — `attack_monster_user` (`0x42e34b`)

Same 6-swing energy loop as players (`5 < count`, energy `mon+0x16` vs EU). Each swing:

1. `genrdn(0,100)`, then walk the **attack-form weight table** `knmsr+0x138` (5 cumulative
   byte thresholds) to pick a form index `0..4`.
2. Read the form's action byte at `knmsr+0x128+form`:
   `0` = no attack (reroll), `1` = **melee** (→ `calculate_attack`), `2` = **cast**
   (`monster_cast`), `3` = **rob/steal** (`monster_rob_user`).
3. Melee builds fighters (`move_monster_to_fighter(mon, ..., form, ...)`) and calls
   `calculate_attack`; monster **crit rating is hard-zeroed** (see `records.md`), so
   monsters never crit.

Monster energy `mon+0x16` gates swings exactly like the player pool; when
`mon+0x16 < EU` the round ends for that monster.

### Free attack on movement — `give_monsters_a_free_attack` (`0x29692`)

Called from **`move_user`** (`0x417692`, i.e. walking **or fleeing**). Rolls once, scans
the room's monsters, and the **first** eligible aggressive monster (roll `≤ mon+0x42`
aggression, correct `mon+0x106` mode, not already busy) gets a **single**
`attack_monster_user` swing, then breaks. Gated by `player+0x6f0` (`0 < +0x6f0` ⇒ skip —
one free attack per round). A nonzero return breaks the move: the monster's blow can stop
the escape. This is the classic "monsters get a swing when you flee."

### `monster_could_attack` (`0x20b51`)

A **predicate**, not an action: scans the room and returns true if some monster would
attack the given player (keys off aggression and ability `0xb9`). Used to warn/gate
player actions (e.g. resting, safe commands) when combat is imminent.

---

## 4. Combat start / break

### Engage — `engage_autocombat` (`0x4c85f`)

`engage_autocombat(user, targetUser, targetMonster, spellFlag, spellId)` writes the
autocombat record (`+0x00..+0x10`, taking `+0x08` from `DAT_004877e4`) and enqueues the
user into the `DAT_004969ec` ring (`FUN_0044c58d`). Command handlers call it as
`(user, 0xffffffff, monster, …)` for PvE or `(user, targetUser, 0xffff, …)` for PvP.

### First strike — `restart_autocombat` (`0x4c167`)

Runs the **opening swing immediately** (not waiting for the next 5 s tick). It validates
first: backstab requires a stabbing weapon (item `+0x394` type check; rejects with "You
cannot backstab with this weapon"), the target must be **present in the same room**
(monster `+0x10/+0xc` vs player `+0xc8/+0xc4`; user analog), else "You don't see your
target here." On success it dispatches the same `attack_user_monster` /
`attack_user_user` / cast path as a normal round.

### Per-round validation — `validate_auto_combat` (`0x4c096`)

Called each round from `energy_update_character`. For a monster target: if the monster is
gone or has changed rooms → `kill_autocombat(user,1)` + `display_autocombat_broken`. For
a target that moved: if the player is no longer co-located it sets the **pursuit flag**
`player+0x6f4 |= 0x200` (and clears it once co-located again) — the combat persists while
chasing.

### Break conditions

* **Target dies** — `check_kill_monster` / `check_kill_user` nonzero ⇒ set the loop's done
  flag, award exp (`distribute_experience`), `kill_autocombat_against_monster(target)`.
* **Attacker dies** — player HP `player+0xb0 < 1` at round entry ⇒
  `display_autocombat_broken`; mid-loop lethal damage sets the done flag.
* **Target left / unreachable** — `validate_auto_combat` / `restart_autocombat` checks above.
* **Manual break / logoff** — `kill_autocombat` (`0x4c8c2`) scrubs the ring and clamps the
  user's banked energy back to max; `clear_autocombat_info` (`0x4bd00`) blanks the record;
  `display_autocombat_broken` (`0x4bd6e`) prints `*Combat Off*` to the user and
  "`%s breaks off combat.`" to the room.
* **Flee** = `move_user` → `give_monsters_a_free_attack` (§3); on a successful move the
  target is simply no longer in range and `validate_auto_combat` tears combat down.

### State-query helpers

`is_inside_autocombat` (ring membership), `is_combatting_user` / `is_combatting_monster`
(record target match + inside-autocombat), `is_being_attacked` (any user targeting me),
`is_in_pvp_combat` (mutual user targeting), `attacker_type` (the attack type of whoever is
hitting me), `match_combat_target` (resolve a name against my current target).

---

## 5. Result application

`calculate_attack(&attacker, &defender)` returns a pointer to the shared **result struct**
(32-bit dword indices; the engine's `AV/DV/DR/DG/EU/D/CR` debug print confirms the roles):

| idx | field | use in the round loop |
|-----|-------|-----------------------|
| `[0]` | **result code** | `1` = glance / absorbed (no damage), `2` = hit, `3` = dodge/parry, `4` = **critical** |
| `[1]` | **damage** | subtracted from target HP (`monster+0x0c`→`puVar7[6]`, or `player+0xb0`); clamped to remaining HP first |
| `[2]` | crit / damage-descriptor value | fed to `get_damage_descriptor` and the "critically …" message |
| `[3]` | last-hit value | stored to `target+0x14` (`puVar7[5]`) for display/threat |
| `[4]` | experience / kill value | `distribute_experience(user, result[4], …)` on a kill |
| `[5]` | **EU (energy used)** | spent from the attacker's energy pool (§2) |

> Layout note vs `combat.md` (16-bit): that doc lists the result code at byte `+0x04`
> with damage at `+0x06/+0x08`. In WG3-NT the struct is dword-indexed and the loops read
> code at `[0]`, damage at `[1]`. Same fields, 32-bit stride — consistent with the
> word→dword widening documented for the fighter struct.

**Per-swing application** (from `attack_user_monster`, mirrored in the other two):

1. Spend `EU` from the attacker pool; if unaffordable, end the round.
2. Clamp `result[1]` to the target's current HP, subtract it, store `result[3]`.
3. Branch on `result[0]`: `1` → "glances off" flavor; `3` → dodge/parry flavor;
   `2`/`4` → hit/critical message via `get_damage_descriptor` (crit prefixes "critically").
   Messages go to the attacker (`tell_user`) and the room (`tell_room`).
4. **On-hit side effects**: weapon leech (item ability `8` restores attacker HP by the
   damage dealt, capped at max), weapon proc (ability `0x72` → `use_monster_target`);
   monster **damage shields** (item/spell ability `0x48`) deal `genrdn(1,n+1)` back to the
   player each hit. Type-7 smash can knock the monster prone (`mon+0x4a |= 8`).
5. `check_kill_monster` / `check_kill_user`; on death award exp + tear down autocombat and
   set the loop's done flag so the round stops early.

---

## Open / uncertain items

* **Absolute second mapping** rests on `rtkick` using seconds (Worldgroup convention).
  The interval *values* (1/3/5/30) are read from the binary and certain; a 5 s round is
  the high-confidence reading but not provable from the decompile alone.
* **`mon+0x106` behavior taxonomy** (modes 0/3/4/6) is only partially recovered — enough
  to see which are aggressive/initiating, not every nuance.
* **`compute_energy_used` argument names** (`player+0xaa` as the speed stat, `class+0x48`
  as the weapon-speed factor) are inferred from usage, not from symbols.
* **Bash/smash stun constants** `DAT_00482d24` / `DAT_00482d28` (prone duration / energy
  drain) are `.data` defaults not assigned in the decompiled code; values not extracted.
* **Attacks-per-round display stat**: the engine exposes attacks/round to clients via the
  energy-pulse flags in `background_energy`; the exact `pool/EU` rounding shown to players
  was not chased down.
