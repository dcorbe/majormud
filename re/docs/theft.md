# MajorMUD 1.11p — The player thief kit (WG3-NT): ROB, PICKLOCK, SEARCH, DISARM, SNEAK, HIDE, FORGIVE

Behavioral spec of the complete thief toolset: robbing players/monsters, picking
locks, finding/disarming traps, sneaking, hiding, and the crime/forgiveness system
they feed. Reconstructed from the clean 32-bit **WG3-NT** decompile
(`../wg_nt_ghidra/exports/WCCMMUD_decompiled.c`, function addresses image-base
0x400000, addresses per `../wg_nt_ghidra/exports/functions.txt`). Where the
decompiler dropped varargs or stack args the byte-level disassembly of
`re/wg_nt_ref/WCCNT8PJ/out/wccmmud.dll` was consulted directly; all strings below
are extracted verbatim from that DLL.

Cross-references: player-record basics in [`records.md`](records.md), stat order in
[`regeneration.md`](regeneration.md) §7, lawful-choice prompt in
[`character_creation.md`](character_creation.md), room record in
[`vir_schemas.md`](vir_schemas.md), item Robable flag confirmed against
Nightmare-Redux `modFieldmaps.bas` (`ItemRecType.Robable`).

| function | address | decompile line | role |
|---|---|---|---|
| `cmd_rob` | `0x4528fb` | 50497 | ROB command: parse, target resolve, dispatch |
| `rob_user` | `0x41f48d` | 17138 | player robs player (coins / items / keys) |
| `rob_monster` | `0x41fc96` | 17444 | player robs monster — **effectively a no-op** (§6) |
| `monster_rob_user` | `0x4295bd` | 23792 | **stub `return 0`** (§7) |
| `cmd_picklock` | `0x454856` | 51735 | PICKLOCK command |
| `cmd_search` / `search_for_hidden_exits` | `0x54fc9` / `0x20597` | 51977 / 17968 | SEARCH: hidden exits + traps |
| `cmd_disarm` | `0x468bed` | 63242 | DISARM TRAP command |
| `cmd_sneak` / `can_sneak` / `sneak` | `0x454641` / `0x46bd91` / `0x4172dc` | 51645 / 65443 / 11637 | SNEAK |
| `cmd_hide` | `0x466b1f` | 61989 | HIDE self / item / coins |
| `cmd_forgive` / `attempt_to_forgive` | `0x45846a` / `0x44ef1e` | 53782 / 48066 | FORGIVE command |
| `add_evil_points` / `add_evil_timer` | `0x44e49c` / `0x44e918` | 47442 / 47682 | crime bookkeeping |
| `evil_for_robbing` | `0x44e89f` | 47620 | rob-timer predicate — **zero callers** (§3.4) |
| `get_legal_level` | `0x44e390` | 47339 | evil points → legal tier |
| `background_update_exits` / `add_exit_to_list` | `0x44f45e` / `0x44fa1d` | 48419 / 48585 | door re-lock / trap re-arm ticker |
| `calculate_secondary_stats` | `0x41a424` | 13356 | derives all seven thief skills |
| `FUN_0046cc43` | `0x46cc43` | 66203 | shared sneak/hide success-chance helper |

RNG: `genrdn(lo,hi)` is the imported Galacticomm RNG — **EXCLUSIVE on the
upper bound**: `[lo, hi)`, `lo` when the span is empty. (CORRECTED
2026-07-28; this line long claimed "inclusive on both bounds", wrongly.
Three independent proofs: MBBSEmu — the oracle board — implements the
ordinal as .NET `_random.Next(min, max)`; WCCMMUD's own damage idiom
`genrdn(0,(max-min)+1)+min` only reaches the item column's max under an
exclusive top; and the Nekojin Mystic capture measured punch damage 2..6
where the formula max is exactly 6.) So `genrdn(1,100) < threshold` hits
with probability `(threshold-1)/99`, and every `genrdn(0,100) < chance`
gate is `chance` in 100.
Every roll below is listed in draw order — a reimplementation must match.

Message prefixes: nearly every line is preceded by the standard prompt-clear
escape (`ESC[[ ESC[79D ESC[K | \b...\b ]`) plus a color set. Below only the color
is noted: **[red]**=`1;31`, **[red40]**=`1;31;40`, **[yellow]**=`1;33`,
**[gray]**=`1;30`, **[white]**=`1;37`, **[green]**=`0;32`, **[dkyellow]**=`0;33`,
**[plain]**=no color (prompt-clear only).

---

## 1. The seven thief skills — `calculate_secondary_stats` (`0x41a424`, lines 13385–13545)

All are 16-bit shorts on the live player record, recomputed by
`calculate_secondary_stats`. Stats: Int `+0xa2`, Wil `+0xa4`, Str `+0xa6`,
Hea `+0xa8`, Agi `+0xaa`, Chm `+0xac` (effective/max array); level `+0x94`.

Two level scalings appear:

```
effLvl  = level                 if level < 16
        = (level - 15)/2 + 15   otherwise            (half rate past 15)
lvlScl  = level * 2             if level < 16
        = (level - 15) + 30     otherwise            (stealth only)
```

| offset | skill | gate (must have ability, else skill = 0) | formula (then clamp ≥ 0) |
|---|---|---|---|
| `+0x5f8` | **Perception** | none | `(Int*5 + Wil*2 + Chm) >> 3` `+ value(0x4d Percep)` |
| `+0x5fa` | **Stealth** | `0x66 RaceStealth` or `0x67 ClassStealth` | `Chm/6 + Agi/4 + lvlScl + Int/8 + 20` then: race-only **−15**, both **+10**, class-only +0; `+ value(0x1b Stealth)` |
| `+0x5fe` | **Thievery** | `0x27 Thievery` | `(Agi + Int + Chm + effLvl*24) / 6` `+ value(0x27)` |
| `+0x606` | **FindTraps** | `0x28 FindTraps` | `(Int + Agi + Chm*2 + effLvl*28) / 7` `+ value(0x28)` `+ value(0xb3 FindTrapsValue)` |
| `+0x608` | **DisarmTraps** | `0x28 FindTraps` (sic — gated on FindTraps) | same base as FindTraps `+ value(0x29 DisarmTraps)` |
| `+0x60a` | **Picklocks** | `0x25 Picklocks` | `((Agi + Int + effLvl*10) * 2) / 7` `+ value(0x25)` `+ value(0xb4 PickLocksValue)` |
| `+0x60c` | **Tracking** | `0x26 Tracking` | `(Int*2 + Wil + Chm + effLvl*40) >> 3` `+ value(0x26)` |

Stealth side effects: sets flag word `+0x7d4` bits `0x20` (race stealth) / `0x40`
(class stealth) / `0x60` (both); clears them and zeroes `+0x5fa` when neither
ability is present.

(Decompiler note: Ghidra renders the `get_user_ability_value` returns as re-adding
the base; the disassembly confirms the intended `base + ability-value` shown above.
`get_user_ability_value(id1, -1, plr, id2, &out)` returns the summed value of `id1`
and stores the summed value of `id2` in `out` — that is how one call feeds both
FindTraps(0x28) and DisarmTraps(0x29 via `out`).)

Related state bytes:

| offset | meaning |
|---|---|
| `+0x5f6` (byte) | **hidden** flag (set by successful HIDE, cleared by nearly every command and on non-sneak movement) |
| `+0x6f4` (word) bit `0x0004` | **sneak-armed**: next queued movement is executed via `sneak()` |
| `+0x6f4` bit `0x2000` | can-see-hidden (checked when robbing a hidden player) |
| `+0x6f4` bit `0x0400`, `+0x7c8` bit `0x0100` | cleared on entry by every command in this kit (generic command-entry clears) |
| `+0x6bb` (byte) | action-delay counter; `add_delay(usrnum,n)` adds n, `delay_done` ⇔ 0 |
| `+0x708` (short) | encumbrance percent (`get_encumbrance_percent` `0x41f430`) |
| `+0x542` (short) | **evil points** (negative = good side) |
| `+0x544` (byte) | `0xf6` (−10) = chose **Lawful** at creation (see `character_creation.md`; Y sets `0x544=0xf6`, `0x542=−51`) |
| `+0x700` bit `0x10` | "evil warnings on" — blocks all evil actions until turned off |
| `+0x700` bit `0x40` | set whenever evil is committed |

---

## 2. Crime bookkeeping — `add_evil_points` (`0x44e49c`) and the evil-timer list

### 2.1 The evil-timer node (list head `DAT_00488180`)

Allocated by `add_evil_timer(actor, victim, ticks, points, mode)`:

| dword | field |
|---|---|
| `[0]` | actor usrnum |
| `[1]` | victim usrnum |
| `[2]` (byte) | tick countdown — **always 0xb (11)** for both robs and PvP kills |
| `[3]` | evil points charged (refunded on FORGIVE) |
| `[4]` | flags: bit0 set when mode ≠ 0 (a rob), bit1 additionally set when mode == 2 (**undetected** rob) |
| `[5]` | next |

`already_evil(actor, victim)` → 0 no node, 1 node with bit0 clear (kill/aggression
timer), 2 rob-detected node (flags==1), 3 rob-undetected node (flags==3).
`decrement_evil_timers(usrnum)` runs from `slow_update_character` (`0x221ad`, line
19514) — one tick per slow update per online actor; at 0 the node is silently freed
(`FUN_0044e98e`) **without refund**: the crime becomes unforgivable.

### 2.2 `add_evil_points(actor, victim, points, 0xb, mode)` — gates and scaling

Returns **1 = action blocked** (caller prints nothing further), 0 = proceeded.
Globally inert when `DAT_004906c9 == 2` (closed/demo mode).

Blocking gates (each prints and returns 1). For `victim >= 0` they apply only while
the victim's evil `+0x542 < 30`:

- `actor+0x700 & 0x10` → **[yellow]** `To do this action, you must turn off your evil warnings.\r`
- `actor evil > 300` → **[yellow]** `You have progressed too far to the evil side to do this action.\r`
- actor Lawful (`+0x544 == 0xf6 && DAT_00482dcc`) → **[yellow]** `You have chosen a way of life which does not allow this action.\r`

Otherwise prints **[gray]** `A dark cloud passes over you\r` and charges points:

```
if victim evil >= 30 (0x1e):            points = 0        (robbing an outlaw is free)
else scale by the victim's goodness:
    victim Lawful (0x544==0xf6 & opt):  points *= 3
    victim evil < -200:                 points *= 3
    victim evil < -50:                  points *= 2
    (overflow guard: if points*n > 32000 use points *= 32000)
first crime bump: if actor evil < 0 and actor+points < 10:  points = 10 - actor_evil
apply: actor_evil += points  (only while < 30000)
set actor+0x700 |= 0x40
```

Then for robs (mode 1 or 2): if an existing rob node was **detected** (flags==1),
the new mode is downgraded to 1; the old node is removed and a fresh node added
(timer refresh — there is **no repeat-rob limit**, only accumulating evil).
Finally, if `get_legal_level` changed tier, `update_allowed_worn_items` runs
(legal-tier item restrictions kick in immediately).

### 2.3 `get_legal_level(evil)` (`0x44e390`)

| evil points | tier |
|---|---|
| < −200 | 7 |
| −200 … −51 | 6 |
| −50 … 29 | 0 (lawful/neutral band) |
| 30 … 39 | 1 |
| 40 … 79 | 2 |
| 80 … 119 | 3 |
| 120 … 209 | 4 |
| ≥ 210 | 5 (maximum — always attackable, see §2.5) |

### 2.4 `evil_for_robbing(actor, victim)` (`0x44e89f`) — dead code

Walks the timer list for a node `(actor, victim)` with flags bit0 set (any rob
timer) and returns 1/0. **It does not write anything and a full byte-scan of the
CODE section finds zero `call` sites** — it exists only as export ordinal 402
(presumably for WCCMMPLS). It is *not* called by `rob_monster` or anything else in
this DLL. The task-brief assumption that it is a fame write is wrong.

### 2.5 `FUN_0046c417(actor_rec, victim_rec, actor_usrnum, victim_usrnum)` — PvP-range gate

(args recovered from disassembly at `0x41f548`)

```
if DAT_00482d8c == -1:                          deny   (LVLDIFF option = -1: PvP off)
if actor.level < 4 or victim.level < 4:         deny   (newbie protection)
if already_evil(victim, actor) != 0:            allow  (they wronged you first)
if get_legal_level(victim.evil) == 5:           allow  (tier-5 criminals always fair game)
if |actor.level - victim.level| > DAT_00482d8c: deny
else                                            allow
```

`DAT_00482d8c = numopt(0x39, -1, 100)` — the **LVLDIFF** BTRIEVE option
(`Maximum level difference for PVP? 9`). `DAT_00482dcc = ynopt(0x47)` — the
**ENABLAWF** option (`Enable the choice of Lawful? YES`). Closed mode
(`DAT_004906c9==2`) forces `dcc=0, d8c=100`.

---

## 3. `cmd_rob` (`0x4528fb`, line 50497)

1. Entry clears `+0x6f4 &= ~0x400`, `+0x7c8 &= ~0x100`.
2. `margc == 1` → **[red40]** `Syntax: ROB {user/monster}\r`, done.
3. `find_action_target(map, room, margv[1], &kind, …, 0x83)`. Kind `0x20` =
   ambiguous → `terminate_multiple_matches(0)`, return. Otherwise:
   - **default** (nothing found): **[red40]** `You don't see that anywhere!\r`
   - **kind 1 (player)**: clear own sneak-armed bit (`+0x6f4 &= ~4`) and own
     hidden byte (`+0x5f6 = 0`). If the target is hidden (`target+0x5f6 != 0`) and
     the robber lacks see-hidden (`+0x6f4 & 0x2000` — note: tested *after* the
     robber's own bit-4 clear), print **[red40]** `You don't see that anywhere!\r`.
     Else `add_delay(usrnum, 1)` and call `rob_user(target_usrnum, NULL)`.
   - **kind 2 (monster)**: same flag clears, `add_delay(usrnum, 1)`,
     `rob_monster(monster, 0)`.
   - **kind 4 / 8 / 0x10** (items etc.): **[red40]** `Why would you want to rob from that?\r`
4. Returns 1 (handled).

**Confirmed at byte level (`0x452a62`–`0x452ac7`): both the `margc < 4` and
`margc >= 4` branches push literal 0 as `rob_user`/`rob_monster`'s second
argument.** The named-item robbery path inside `rob_user` (§4.4) is therefore
**unreachable from the command table in this build** — dead code, spec'd anyway
because the engine contains it.

---

## 4. `rob_user(victim_usrnum, item_name)` (`0x41f48d`, line 17138)

`robber` = self (`_usrnum_exref`), `victim` = param. Both must resolve via
`get_player`.

### 4.1 Gates (in order, each prints only to the robber)

1. Lawful robber (`+0x544 == 0xf6 && DAT_00482dcc`) **or** evil-warnings flag
   (`+0x700 & 0x10`):
   **[plain]** `You have chosen a way of life which prevents this action.\r`
2. Robbing yourself: `Why would you want to rob yourself?\r` (no prefix).
3. Room must resolve (`get_room_data(robber map/room)`).
4. PvP-range gate `FUN_0046c417` (§2.5) fails **and** the per-user sysop/debug
   flag `DAT_00479100[usrnum*0x14]` is clear:
   **[plain]** `Such an action would result in a very unbalanced game.\r`
5. Safe room (`room+0x564 & 1`) **or** arena room (`room+0x43c == 5` while arena
   enabled `DAT_004790e8`):
   **[red]** `You are overcome with a feeling of guilt and return your hands to your own pockets\r`

### 4.2 The skill roll — Thievery vs d100 (**draw 1**: `genrdn(1,100)`)

Let `T = robber+0x5fe` (Thievery):

| result | outcome |
|---|---|
| `roll > T + 10` | **detected** ("bump"): `add_evil_points(robber, victim, 1, 0xb, mode=1)`; if not blocked: robber sees **[red]** `You bump %s as you try to rob %s.\r` (victim name, him/her pronoun of victim); victim sees **[red]** `%s bumps you as %s tries to rob you!\r` (robber name, he/she pronoun of robber) + fresh prompt. **The room sees nothing.** |
| `T < roll <= T + 10` | **marginal fail**: `add_evil_points(…, mode=2)`; if not blocked: **[red]** `Your skills fail as you try to rob %s.\r` (victim name). Victim sees nothing. |
| `roll <= T` | **success path** — continue below. `add_evil_points(…, mode=2)` is charged *before* the loot is determined; if it blocks (lawful/warnings/too-evil), nothing further happens. |

Evil charge: 1 point per attempt (success or fail), tripled/doubled per §2.2, zero
against tier-1+ victims. The 11-tick evil timer records detected (flags=1) vs
undetected (flags=3) — used by FORGIVE display and by `should_give_evil` when a
later kill happens.

Pronoun helpers: `FUN_0041d89d` he/she/it, `FUN_0041d8dd` him/her/it,
`FUN_0041d91d` his/her/its (gender char at `+0x7d6`, `'M'`/`'F'`).

### 4.3 Loot selection, no item argument (the live path)

- **draw 2**: `genrdn(1,100)`. If `< 50` → **coins**: **draw 3** `genrdn(0,4)`
  picks the currency index (see table below) — even one the victim has none of.
- else **items**: walk the victim's 100-slot inventory (`victim+0xd8 + i*4`,
  i = 0…99). For **every non-zero slot** draw `genrdn(1,100)`; if `< 50` that
  item becomes the current candidate (`get_item_data`), and it stays selected
  **only if `item_has_ability(100 LoyalItem)`** — a non-Loyal hit overwrites the
  candidate pointer but clears the selected flag. The **last** qualifying slot
  wins. (Yes: random robbery from the pack can only ever select **Loyal** items —
  verified in the disassembly at `0x41f7d1`–`0x41f7e4`. See UNDETERMINED.)
- If nothing selected: walk the victim's 50-slot **key ring**
  (`victim+0x334 + i*4`, i = 0…49; array identity confirmed via
  `display_users_keys` `0x43a910`). One `genrdn(1,100)` per non-zero slot,
  `< 50` selects (last hit wins), **no Loyal requirement**. Keys are the
  realistic random loot.

### 4.4 Loot selection, with item argument (dead code — never reached from `cmd_rob`)

`match_string(arg, PTR_s_copper_farthings_00480248)` against the currency-name
table; a match makes it a coin rob of that type. Otherwise
`find_item_in_inventory(victim, arg, …, mode=8)` (+ `terminate_multiple_matches(1)`)
searches the victim's belongings by name.

### 4.5 Item transfer

For a selected item (either path):

1. `item+0x42b` (**Robable** byte, Nightmare `ItemRecType.Robable`) must be
   non-zero, else **[red]** `Your skills fail as you try to rob %s.\r`.
2. Count copies across inventory (100 slots) + key ring (50 slots). If exactly
   **one** copy and its id appears in a worn-equipment slot
   (`victim+0x62c + j*4`, j = 0…19) or is the readied weapon (`victim+0x624`)
   → fail with the same string (you cannot steal the only copy of something
   equipped).
3. `remove_item_from_inventory(victim, id, &qty, 0)` then
   `add_item_to_inventory(robber, 0, qty, item)`. If the add fails the item is
   given back and the same failure string prints.
4. Success: **[plain]** `You successfully stole %s from %s.\r` (item name
   `item+0xad`, victim name). **The victim is never told; the room is never told.**

### 4.6 Coin transfer

Currency index → live player offsets (table `0x480248`; names from the DLL):

| idx | name | player offset | room hidden-coin offset (HIDE, §11.2) |
|---|---|---|---|
| 0 | `copper farthings` | `+0x620` | `+0x558` |
| 1 | `silver nobles` | `+0x61c` | `+0x554` |
| 2 | `gold crowns` | `+0x618` | `+0x550` |
| 3 | `platinum pieces` | `+0x614` | `+0x54c` |
| 4 | `User Defined Currency Type ....` (runic) | `+0x610` | `+0x548` |

**draw** `genrdn(0, max(victim_coins, 0))` — amount stolen. 0 → **[red]**
`Your skills fail as you try to rob %s.\r`. Otherwise transfer the full rolled
amount (`victim -= amt; robber += amt`) and print
`You stole %s %s from %s.\r` = (`spr("%lu", amt)`, `proper_currency_name(amt, idx)`,
victim name) — no color prefix beyond the standard clear. Victim silent, room
silent. An out-of-range currency index prints the failure string (defensive
default).

**Total RNG draws in order**: (1) skill d100 → (2) coin-vs-item d100 → (3) either
currency `genrdn(0,4)`, or one d100 per occupied inventory slot then (if no
selection) one d100 per occupied key slot → (4) coin amount `genrdn(0,coins)` if
coins. Failure branches stop the sequence where noted.

---

## 5. Victim notification and FORGIVE interplay

Only the **bump** outcome informs the victim. The victim (or any wronged party) can
later `FORGIVE <player>`:

### `cmd_forgive` (`0x45846a`, line 53782)

Requires the criminal **present in the room** (`find_action_target(…, 0x82)`,
kind 1 = player; kind 0 → **[red40]** `You do not see %s here!\r`).
Calls `attempt_to_forgive(criminal_usrnum, self_usrnum)`:

- success (a node existed): criminal is told
  `The gods have forgiven you for your action.\r` (+prompt); forgiver sees
  `The gods have forgiven %s for %s action.\r` (him/her, his/her pronouns of the
  criminal).
- no node: forgiver sees `The gods refuse to forgive %s for %s actions.\r`.

### `attempt_to_forgive(actor, victim)` (`0x44ef1e`, line 48066)

Finds the `(actor, victim)` timer node; on match **refunds the evil**:
`actor_evil(+0x542) -= node[3]`, `update_allowed_worn_items`, node unlinked and
freed, returns 1. With the sysop/debug flag set it also dumps
`Forgiving: (%d) %s To Forgive: (%d) %s\r` and per-node
`List: Attacker (%d) %s Target (%d) %s\r`.

Because the node dies naturally after 11 slow ticks (§2.1), forgiveness has a
window; after expiry the evil is permanent (until the separate EPCYCLE/EPAMOUNT
option-driven decay, out of scope here).

---

## 6. `rob_monster(monster, arg)` (`0x41fc96`, line 17444) — a no-op

The complete body:

```c
if (robber && monster && arg != 0 && robber_is_lawful && DAT_00482dcc) {
    prf("You have chosen a way of life which prevents this action.\r");
    tell_user(usrnum);
}
```

`cmd_rob` **always passes `arg = 0`** (§3), so even the lawful message never
prints. `ROB <monster>` costs one action delay and does absolutely nothing else:
no roll, no loot, no evil, no aggro, and **no call to `evil_for_robbing`** (which
has no callers at all, §2.4). Robbing monsters was designed out of this build.

---

## 7. `monster_rob_user` (`0x4295bd`) and the kind-3 fall-through

`monster_rob_user()` is **confirmed `return 0;`** (7 asm bytes — line 23792).

Caller: the monster swing loop in `attack_monster_user` (line 26665; the kind-3
branch at line 26808). Attack-form selection, per swing attempt (max 6 loop
iterations):

```
roll = genrdn(0,100)
pick lowest i in 0..4 with roll < byte tmpl[0x138+i]   (cumulative % table;
      if none matches, keep the previous selection)
kind = byte tmpl[0x128 + i]        // 0 unused, 1 melee, 2 cast, 3 rob
kind 0 → re-roll (loop)
kind 1 → melee swing with form i
kind 2 → monster_cast(...); nonzero result → re-roll (2 = stop);
         zero result → fall through: kind = tmpl[0x128] (form 0), i = 0,
         melee only if that byte == 1, else re-roll
kind 3 → uVar6 = monster_rob_user()   // always 0
         since (kind != 3 || uVar6 != 0) is false, DO NOT re-roll; instead
         re-read kind = *(char*)(tmpl + 0x128)   // form slot 0's kind byte
         set i = 0
         if kind == 1 → proceed to the melee swing using form 0
         else        → re-enter the selection loop
```

So a monster whose form table rolls a "rob" attack (kind 3) **degrades to its
form-0 melee attack when form 0 is melee, and otherwise wastes the pick and
re-rolls** — exactly the same fall-through as a failed spell cast. Reimplementation
must preserve the re-read of `tmpl+0x128` (form 0's kind), not the rolled form's.

---

## 8. `cmd_picklock` (`0x454856`, line 51735)

Syntax errors (no direction, or `parse_command` fails, or dir ≥ 10):
**[red40]** `Syntax: PICKLOCK {direction}\r`. Directions 0–9 =
N,S,E,W,NE,NW,SE,SW,up,down (name table `PTR_0047febc`, opposite-direction table
`DAT_0047fe94` = `[1,0,3,2,7,6,5,4,9,8]`).

Entry: generic flag clears; breaks autocombat
(`display_autocombat_broken` + `kill_autocombat`). Then: destination
`room[0x338 + d*4]` must be non-zero, else **[red]**
`Your skill fails you this time.\r` (the same string masks "no such exit").
Clears sneak-armed bit and hidden byte; `add_delay(usrnum, 2)`.

### 8.1 Exit-field layout (runtime room record, per direction `d`)

Exit type = `word room[0x360 + d*2]`. The per-direction slots are **unions keyed
by type**:

| slot | type 2 ("door") | type 7 ("door") / 0xb ("gate") | type 6 hidden | type 9 trap | type 0x18 spell-trap |
|---|---|---|---|---|---|
| `word 0x39c+d*2` | lock **state**: 2 locked, 1 picked | **pick modifier** (added to skill; ≥1 also disables auto-relock) | re-hide difficulty code | trap **state**: 0/3 armed (3 = trapdoor variant), 1/4 disarmed | same as type 9 |
| `dword 0x374+d*4` | — | door **state**: 2 locked, 1 unlocked | found-state flags (2 hidden, 4 found; ticker codes `0x10…0x3ff0`) | **damage rating** | **trap spell id** |
| `dword 0x3b0+d*4` | **pick modifier** | **re-lock delay** (units) | — | — | success message id |
| `dword 0x3d8+d*4` | **re-lock delay** (units) | — | — | trigger message id | trigger message id |

Byte `room+0x5f8` = room-dirty flag (set on every state change so the room is
persisted). **Word `room+0x5fa` = per-room lock-trap spell** (§8.4).

(The disk-side type list in `vir_schemas.md` ("0=open, 2=door, 6=gate, 7=secret,
9/0xc/0x10 locked variants") came from the 16-bit build's display code; the WG3-NT
*runtime* semantics above are authoritative for this project: pickable types are
**2, 7, 0xb**; 6 = hidden exit; 9/0x18 = traps; 0x10 = timed exit; 0x14 =
alignment gate; 0x16/0x17 = spell/ability gates — the latter four per `move_user`
and `background_update_exits`, not pickable.)

### 8.2 Type 2 lock

Skill = `player+0x60a` (Picklocks). **Roll `genrdn(0,100)`** (only if skill ≥ 1):

- **success** iff `roll < room[0x3b0+d*4] + skill`.
- failure (or skill < 1): a **second** `add_delay(usrnum, 2)` (total 4) and
  **[red]** `Your skill fails you this time.\r`. No trap on type 2.

On success: `room[0x39c+d*2] = 1` (picked), dirty flag,
`add_exit_to_list(room#, map, d, max(room[0x3d8+d*4], 1))` schedules the re-lock.
If the destination room's opposite exit points back, it is opened too (type 7/0xb
→ `0x374 dword = 1`, its own re-lock from its `0x3b0`; type 2 → `0x39c word = 1`,
re-lock from its `0x3d8`).

### 8.3 Type 7 / 0xb lock

Requires `room[0x374+d*4] == 2` (locked); otherwise **[red]**
`Your skill fails you this time.\r` (also for any other exit type).
**Roll `genrdn(0,100)`** (skill ≥ 1): **success** iff
`roll < (short)room[0x39c+d*2] + skill` (the modifier is typically negative —
hard locks).

On success: `room[0x374+d*4] = 1`, dirty, re-lock via
`add_exit_to_list(…, max(room[0x3b0+d*4], 1))`, reciprocal exit opened when
type 7/0xb (state=1, its own timer). Then the messages (§8.5).

### 8.4 Failure trap (type 7/0xb only)

On a failed roll: second `add_delay(usrnum,2)`, then if the **room's** lock-trap
spell `word room+0x5fa` is non-zero →
`room_cast_on_user(usrnum, spell)` (the spell's own output; no fail string),
else **[red]** `Your skill fails you this time.\r`.
There is **no evil/crime/legal consequence for picking locks**, success or fail.

### 8.5 Success messages

Word choice: `gate` when the exit type is 0xb, else `door`. Room first (excluding
the picker, `tell_room(…, 0xff)`), then the picker:

- room, d = 8: `You see %s pick the lock on the %s above you.\r` (picker name, door/gate — chosen by dir-8's own type word at `0x370`)
- room, d = 9: `You see %s pick the lock on the %s below you.\r`
- room, else: `You see %s pick the lock on the %s to the %s.\r` (+ direction name)
- picker: `You successfully unlocked the %s.\r`

### 8.6 Re-lock — `add_exit_to_list` (`0x44fa1d`) + `background_update_exits` (`0x44f45e`)

Node: `[map, room, dir, delay-byte, next]`; enqueue schedules
`my_rtkick(DAT_004884c0 * delay, background_update_exits)` with
**`DAT_004884c0 = 300`** (.data initial value) — i.e. 300 s (5 min) per delay
unit. Each kick decrements the **head** node's delay; at 0 it processes by exit
type and frees the node:

- type 2: if state ≠ 2 → state = 2, dirty, room hears **[white]**
  `The door to the %s just locked!\r`
- type 7: only if pick-modifier `0x39c < 1` and state ≠ 2 → state = 2, dirty,
  `The door to the %s just locked!\r` (positive-modifier locks never re-lock)
- type 0xb: same with `The gate to the %s just locked!\r`
- type 6 (hidden exit found by SEARCH): re-hides — state bit4 path resets
  `0x374` to a code from the `0x39c` difficulty (`±1…±10` →
  `0x10,0x30,0x70,0xf0,0x1f0,0x3f0,0x7f0,0xff0,0x1ff0,0x3ff0`), else state = 2
- type 9 / 0x18 (traps): `0x39c` 1→0, 4→3 (**re-arm**)
- type 0x10 (timed exit): toggles `0x374 dword` 0↔2 with
  `The exit to the %s just opened!`/`closed!` (or the exit's custom message
  record), and **re-queues itself** with delay `0x39c` — a perpetual cycler.

If the room fails to load, the node is silently re-queued with its original delay.

---

## 9. SEARCH — `cmd_search` (`0x454fc9`) / `search_for_hidden_exits` (`0x420597`)

`cmd_search`: generic clears + breaks hide/sneak (`+0x5f6=0`, `+0x6f4 &= ~4`).
Blind (`can_see` fails) → just `add_delay(1)`. In combat →
`You may not search while attacking!\r`. No argument → re-lists room items
(`display_items_in_room`) and the room hears
`%s is searching the area.\r`; `add_delay(1)`. With a direction argument
(`parse_command`, dir < 10) → `search_for_hidden_exits(usrnum, dir)`;
non-directions → `Why would you want to search that?\r`. Either way
`add_delay(usrnum, 1)`.

`search_for_hidden_exits`: room first hears `%s is searching for exits.\r`.
Then by exit type:

- **type 6, state `0x374` has bit 2** (hidden): **roll `genrdn(0,100)`**, success
  iff `roll < max(Perception(+0x5f8) − 15, 3)` →
  state = 4 (found), dirty, `add_exit_to_list(…, delay 1)` (re-hides in ~5 min),
  message [plain] `You found an exit upwards!\r` / `You found an exit downwards!\r`
  / `You found an exit to the %s!\r`.
- **type 9** (trap): **roll `genrdn(0,100)`**, success iff
  `roll < FindTraps(+0x606)` → `You found a trap above you!\r` /
  `You found a trap below you!\r` / `You found a trap to the %s!\r`. Finding does
  **not** change any state. (Type 0x18 spell-traps are **not** findable.)
- anything else, or a failed roll:
  `You notice nothing different above you.\r` / `…below you.\r` /
  `…to the %s.\r`.

---

## 10. DISARM — `cmd_disarm` (`0x468bed`, line 63242)

Parses the direction from **`margv[2]`** (syntax `DISARM TRAP <direction>`);
dir < 10 and room must load, else silent return 1. Trap must be armed
(`0x39c` state 0 or 3); a disarmed/absent trap or wrong exit type gives
**[red]** `You failed to disarm any trap to the %s.\r`.

**Roll `genrdn(0,100)`** vs DisarmTraps `D = player+0x608`:

### Type 9 (mechanical trap)

- `roll < D` → **success**: `You successfully disarmed the trap to the %s.\r`
  (no prefix), state 0→1 / 3→4, dirty, `add_exit_to_list(…, delay 1)` (re-arms).
- `D <= roll < D+10` → **near miss** (safe): **[red]**
  `You failed to disarm any trap to the %s.\r`.
- `roll >= D+10` → **triggered**: message record `room[0x3d8+d*4]` broadcast —
  room-view text (`msg+0x56`, formatted with `"%s%s"` = green prefix + player
  name) to this room *and* the far room (`room[0x338+d*4]`), then the user-view
  text (`msg+4`) to the actor **[green]**. Then:
  - state 3 (trapdoor): `move_user(usrnum, d, mode 7)` — you fall through
    (mode-7 handling in `move_user`'s type-0x18 case, incl. its own
    `room_cast_on_user(room[0x374])` and forced room entry).
  - state 0: damage `genrdn(rating/2, rating+1)` where
    `rating = room[0x374+d*4]`; subtract from HP `+0xb0`; if it crossed to < 1,
    the death handler `FUN_0043c91d` fires.

### Type 0x18 (spell trap)

- success: as above, plus its success message record `room[0x3b0+d*4]`
  (both rooms + self), then `add_exit_to_list(…, 1)`.
- **any** failure: **[red]** fail string, then trigger message `room[0x3d8+d*4]`
  (both rooms + self), then state 3 → `move_user(usrnum, d, 7)` else
  `room_cast_on_user(usrnum, room[0x374+d*4])` (spell damage; death check after).

No evil/crime consequence. No FindTraps involvement in DISARM itself (FindTraps
only gates the *skill calc* and SEARCH detection).

---

## 11. SNEAK and HIDE

### 11.1 `cmd_sneak` (`0x454641`) — arm stealth for the next move

Gates via `can_sneak` (`0x46bd91`): fails if being attacked by a same-room
attacker (except attacker-type 4), or `player+0x6f0 >= 1` (combat-engagement
counter), or `monster_could_attack(-1, player)`. Failure:
`You may not sneak right now!\r` (no prefix) + `add_delay(1)`.
`margc == 2` → return 0 (unhandled). `margc > 2` → **[red40]** `Syntax: SNEAK\r`.

With no argument: breaks autocombat; delay gate (**[plain]**
`You must wait before you may do that!\r` if not ready); `add_delay(1)`;
prints `Attempting to sneak...` then:

- ability `0xba PerStealth` → auto-success.
- else `chance = FUN_0046cc43(player, 95, usrnum)` (§11.3);
  **roll `genrdn(0,100)`**: `roll < chance` → success.
- success: set `+0x6f4 |= 4` (sneak-armed) and print just `\r` — **the player is
  never told sneaking worked**.
- failure: **roll `genrdn(0,100)`** vs Perception `+0x5f8`: `roll < P` →
  `You don't think you're sneaking.\r`, else the same silent `\r`.

Sysop-debug flag + PerStealth + armed → extra `PERFECT STEALTH\r` line.

The armed bit is consumed by the queued-movement dispatcher in
`fast_update_character` (line 20045): if bit 4 is set the pending move runs as
`sneak(usrnum, dir)` (`0x4172dc`), which is just
`DAT_0047d648 = 1; move_user(...); DAT_0047d648 = 0;`.

`move_user`'s sneak branch (lines 12574+): clears bit 4, **rolls `genrdn(0,100)`**
vs own Perception — `roll < P` prints (red) `You make a sound as you enter the room!\r`
to the sneaker (self-awareness only, no actual effect on concealment). Both rooms
then get **perception-filtered** notifications (`tell_room` with the
perception-filter flag set, **[dkyellow]**):
`You notice %s sneaking out upwards%s.\r` / `…downwards%s.\r` /
`You notice %s sneaking out to the %s%s.\r` and on the far side
`You notice %s sneak in from above%s.\r` / `…below%s.\r` /
`You notice %s sneak in from the %s%s.\r` (trailing `%s` = a ` \b` no-op pair).
Normal leave/arrive broadcasts are suppressed.

### 11.2 `cmd_hide` (`0x466b1f`)

**No argument — hide self.** Gates: in-autocombat / being-attacked / `+0x6f0 ≥ 1`
/ `monster_could_attack` → prints `Attempting to hide...` +
` You don't think you are hidden.\r` (note leading space) + `add_delay(1)` —
i.e. an unconditional fake failure. Paralysis (`+0x7c8` bit 0x20) →
`You can't seem to move anywhere to hide!\r`; stun (`+0x7c9` bit 2) →
`You are too stunned to move anywhere to hide!\r`; delay gate →
**[plain]** `You must wait before you may do that!\r`. Otherwise `add_delay(1)`,
`Attempting to hide...`, `chance = FUN_0046cc43(player, 95, usrnum)`,
**roll `genrdn(0,100)`**:

- `roll < chance`: `+0x5f6 = 1` (hidden) and print just `\r` (silent success).
- else **roll `genrdn(0,100)`** vs Perception: `< P` →
  ` You don't think you are hidden.\r`, else silent `\r`.

(No PerStealth shortcut here, unlike SNEAK.) Ends with
`calculate_secondary_stats(usrnum, 2)`.

**With arguments — hide items/coins in the room** (the thief's stash mechanic):

- `HIDE <item>`: `find_item_in_inventory(…, mode 4)`; refused for
  `item+0x428` (NotDroppable) → `You may not hide that item!\r`; refused for a
  worn wearable (`0x52`/`0x53` ability + `user_is_wearing`) unless a second copy
  exists in inventory; `can_see` required. If it is the readied weapon it is
  removed first. `remove_item_from_inventory` +
  `add_item_to_room(map, room, id, hidden=1, qty)`: success →
  `You hid %s.\r`; room full → item returned,
  `There is no room to hide %s here.\r`.
- `HIDE <n> <currency>`: `n = atol(margv[1]) > 0`, currency matched against the
  table in §4.6 (else `Syntax: HIDE %s {Currency}\r` with n). Sufficient funds →
  transfer to the room's hidden-coin fields (§4.6 table), dirty flag,
  `You hid %s %s.\r`; else `You don't have %s %s to hide!\r`. An impossible
  index reaches `internal_error("hide currency")`.

### 11.3 The shared stealth-chance helper — `FUN_0046cc43(player, cap, self_usrnum)`

(args from disassembly: both callers push `(player_rec, 0x5f, usrnum)`)

```
chance = Stealth (+0x5fa)
encumbrance (+0x708):  >= 67% → −10;  35–66% → −5
+0x6f5 bit 0x80 set (bit 15 of the +0x6f4 flag word) → chance = chance*2/3
clamp to 100
−1 per other online player in the same room
−1 per live monster in the room (room+0x400 list, 15 slots)
clamp to cap (95 from both callers), then clamp ≥ 0
```

---

## 12. Config globals referenced

| global | source | meaning |
|---|---|---|
| `DAT_00482dcc` | `ynopt(0x47)` = **ENABLAWF** (`Enable the choice of Lawful? YES`) | lawful path enforced |
| `DAT_00482d8c` | `numopt(0x39,-1,100)` = **LVLDIFF** (`Maximum level difference for PVP? 9`) | −1 disables PvP/robbery |
| `DAT_004906c9` | module mode; `== 2` disables the evil system, forces dcc=0/d8c=100 |
| `DAT_004790e8` | arena system enabled (rooms with `+0x43c == 5`) |
| `DAT_00479100 + usrnum*0x14` | per-user sysop/debug flag byte (bypasses PvP-range gate; extra debug output) |
| `DAT_004884c0` | `.data` initial **300** — seconds per exit-timer delay unit |

---

## UNDETERMINED

- **Why random item robbery requires `LoyalItem` (ability 100).** Byte-verified
  (§4.3), but the design intent is unclear — it makes pack items essentially
  unstealable by random rob (keys are the real loot). Possibly intentional
  (loyal items survive death, so theft is their only sink), possibly an inverted
  test that shipped. Oracle test: rob a player carrying only a non-loyal Robable
  item; expect perpetual "Your skills fail".
- The named-item rob path (§4.4) is dead in this build; whether any other build
  (WCCMMPLS?) passes `margv` through is unverified. Same for `evil_for_robbing`'s
  intended external caller.
- `find_item_in_inventory` mode flags (4 vs 8) — 8 presumably includes the key
  ring; not read.
- `+0x6f5` bit `0x80` (stealth ×2/3 penalty, §11.3) — which condition sets it
  (suspected light-source/combat state) not traced.
- Exit type-6 re-hide difficulty codes (`0x39c` ±1…±10 → `0x10…0x3ff0` state
  values) — the meaning of the individual state bits beyond bit2=hidden /
  bit4=found is not decoded.
- `should_give_evil` / kill-path evil (10 points, mode 0) is only sketched here;
  full PvP-kill fame belongs in a combat doc.
- Room word `+0x5fa` (lock-trap spell, §8.4) has no disk-side confirmation in
  `vir_schemas.md` yet; needs a .VIR cross-check.
- ~~`attempt_to_forgive` unlink~~ — **promoted to a confirmed engine bug**, byte
  verified at `0x44f04c`–`0x44f053`: for a non-head match the code does
  `pred->next = matched->next` and then `galfree(pred)` — it **frees the
  predecessor node instead of the matched node**, leaking the matched node
  (points already zeroed, so a second FORGIVE refunds nothing) and, when the
  predecessor is the list head, leaving `DAT_00488180` dangling
  (use-after-free). Only the head-match path is correct. A reimplementation
  should free the matched node; do not clone the original behavior.

---

## As built — M7 slice 4 + close-out (2026-07-31)

Implementation: rob/picklock/search/disarm/sneak/hide in `game.rs`
(spec sections above, cited per-arm); slice-8 live pins in
`slice8_abbrevs.raw` / `slice8_abbrevs2.raw` / `slice8_gang1b.raw`.

### Live-measured (slice-8 expedition)

- `rob` minimum 2 ("ro"), bare form prints "Syntax: ROB {user/monster}";
  an absent target refuses "You don't see that anywhere!" (no say
  fall-through, unlike attack/ask).
- **`picklock` is pattern-parsed**: every bare prefix through the full
  word falls to SAY; with a direction argument it parses from 2 ("pi n"),
  and the two-word "pick lock n" form prints "Syntax: PICKLOCK
  {direction}". A skill-0 attempt at a real locked door (15/850 south)
  fails "Your skill fails you this time."; `open` on the same door prints
  "The door is locked." — both strings live.
- `search` shortest live form "sea" (se belongs to southeast); a bare
  search in an empty room prints "Your search revealed nothing."
- `disarm` owns di/dis live (silent no-op with no trap present).
- `sneak`/`hide`: "Attempting to sneak..." / "Attempting to hide...";
  the failed-sneak reveal "You don't think you're sneaking." captured.
- Closed-door render: "Obvious exits: north, closed door south"
  (`slice8_gang1b.raw`) — the slice-4 closed-door wording pin.

### Residue (documented, not blocking)

- Success-path rob/picklock (skilled thief) unmeasured — no thief-class
  character exists on the current oracle install; the decompile citations
  (§1-§8) remain authoritative for the roll/branch structure.
- The `attempt_to_forgive` engine bug (UNDETERMINED above) stays
  deliberately un-cloned.
