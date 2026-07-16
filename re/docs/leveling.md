# MajorMUD (WG3-NT) — Level-up & character-progression spec

Behavioral spec derived from the 32-bit WG3-NT decompile
(`wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Cross-references `records.md`
(struct offsets, exp curve), `spellcasting.md` (the `+0x94` conflict), and
`abilities.md` (stat ability ids 44-49 = Int/Wis/Str/Hea/Agl/Chm). Offsets are
byte offsets into the player / race / class / shop records.

Functions read: `roll_stats` (0x1a2d1), `calculate_secondary_stats` (0x1a424),
`train_level` (0x1e6e8), `cmd_train` (0x56f54), `edit_character_stats` (0x487d),
`add_experience` (0x16818), `add_quest_exp` (0x6f291),
`distribute_experience` (0x4c990), `restructure_experience` (0x73a30),
`get_legal_level` (0x4e390).

---

## 0. Resolved stat-block layout (primary stats)

`roll_stats` copies race base stats `race+0x20..+0x2a` (stride 2) into
`player+0xa2..+0xac`, then mirrors that into `player+0x96..+0xa0`.
`edit_character_stats` labels each field explicitly (race base / race max /
player current). The definitive layout:

| stat | player current (effective) | player base copy | race base | race **max/cap** | class HP field |
|------|------|------|------|------|------|
| Intellect | `+0xa2` | `+0x96` | `+0x20` | `+0x66` | |
| Willpower/Wisdom | `+0xa4` | `+0x98` | `+0x22` | `+0x68` | |
| Strength | `+0xa6` | `+0x9a` | `+0x24` | `+0x6a` | |
| Health | `+0xa8` | `+0x9c` | `+0x26` | `+0x6c` | |
| Agility | `+0xaa` | `+0x9e` | `+0x28` | `+0x6e` | |
| Charm | `+0xac` | `+0xa0` | `+0x2a` | `+0x70` | |

Order = **Int, Wis, Str, Hea, Agl, Chm**, matching `abilities.md` ids 44-49 exactly.

`+0xa2..+0xac` are the **current effective** stats: they are what the training
editor raises (capped at `race+0x66..+0x70`), what temporary stat-buff spells
(abilities 44-49) add to and reverse (see `spellcasting.md` termination), and
what nearly every derived-stat formula reads. The `+0x96..+0xa0` copy is a
parallel un-modified base; only **current Health `+0x9c`** is consumed downstream
(the max-HP `/2` term), so a Health *buff* on `+0xa8` does **not** raise max HP —
a real quirk of the engine.

---

## 1. `+0x94` is the CHARACTER LEVEL — conflict resolved

`records.md` labelled `player+0x94` as "gender / reroll flag (checked `<2`)";
`spellcasting.md` flagged that the cast path uses `+0x94` as caster
spell-level/power. **`+0x94` is the character's level.** Evidence:

- `train_level` (the level-up path) does `*(short *)(iVar4 + 0x94) = *(...) + 1`
  after a successful train (line 16489) — it *increments* `+0x94` by one per
  level. A gender/reroll flag is never incremented.
- `calculate_secondary_stats` reads `+0x94` as an integer level throughout:
  `class[+0x42] * player[+0x94] * 2` (mana), `player[+0x94] * 2` (SC),
  `player[+0x94] * class[+0x20]` (HP per level), `player[+0x94] / 10` (dodge),
  and growth-rate breakpoints at `< 0x10` (level 16). None of that is a boolean.
- `train_level` gates on `+0x94` vs shop min/max level (`+0xce`/`+0xd0`) and vs
  the global level cap `DAT_00482d90`.

The `< 2` test in `roll_stats` (line 13331) is a *character-generation* guard, not
the field's meaning: during a fresh roll (level 0 or 1) it copies the class HP-base
byte `class+0x22 → player+0x724`; it does not re-apply that on an already-levelled
character. So `records.md` mis-attributed `+0x94`. **Correct: `+0x94` = level
(word).** The cast-path reading in `spellcasting.md` stands.

> Note: `get_legal_level` (0x4e390), despite the name, has **nothing** to do with
> `+0x94`. It maps an *alignment* score (called with `player+0x542`) to a tier 0-7
> (`< -200 → 7`, `< -50 → 6`, `< 30 → 0`, `< 40 → 1`, `< 80 → 2`, `< 120 → 3`,
> `< 210 → 4`, else 5). It is the alignment-lattice input for spell legality, not a
> level accessor.

---

## 2. Experience storage, award, and the level-up trigger

### 2.1 Experience is stored as a base-10⁹ two-word integer
Current experience lives in **`player+0x470` (count of billions)** and
**`player+0x474` (remainder 0..999,999,999)**. `player+0x3c` is a legacy 32-bit
running total kept in parallel. All exp-add functions carry from `+0x474` into
`+0x470` at 10⁹.

`restructure_experience(playerPtr)` is a one-time migration: it sets
`+0x474 = +0x3c`, normalizes into the `+0x470`/`+0x474` billions/remainder form,
and sets the "restructured" flag `+0x7d4 |= 0x2000` (= byte `+0x7d5 & 0x20`).
**`add_experience` and `add_quest_exp` refuse to award exp until this flag is
set** ("Your experience has not been restructured — please exit and re-enter").

### 2.2 `add_experience(term, amount, tellFlag)`
1. Requires the restructured flag; else aborts with the re-enter message.
2. **Over-level cap:** computes `new_calc_exp_needed(level + DAT_00482d08, base)`
   with `base = class[+0x24] + race[+0x62]` (the same seed inputs as `records.md`).
   If the player's exp already meets/exceeds that look-ahead threshold and
   `tellFlag` is set, it *refuses* the award ("You have progressed too far with…").
   This caps how far ahead of your trainable level you may bank exp
   (`DAT_00482d08` levels of headroom).
3. Adds `amount` to the gang exp pool (`get_gang_data`, offset `+0x28`/`+0x58`).
4. Adds `amount` to `+0x3c` (legacy total) and to the `+0x470`/`+0x474` current exp.
5. "You gain %s experience" when `tellFlag`.

### 2.3 `add_quest_exp(term, amount, tellFlag)`
Identical carry logic into `+0x470`/`+0x474`, gated on the restructured flag, but
**no over-level cap and no gang split** — quest exp bypasses the "progressed too
far" limit that combat exp obeys.

### 2.4 `distribute_experience(killerTerm, amount, monsterId, userId, map, room)`
The combat-kill award splitter. It counts every terminal engaged in autocombat
against the same target (plus the killer), **divides `amount` evenly** among them
(`amount /= participants`, floored to a minimum of 1), then calls
`add_experience(term, share, 1)` for each participant and tears down their
autocombat state. So party members in the same fight split kill exp equally.

### 2.5 The trigger: **experience never auto-levels**
Nothing in `add_experience` / `distribute_experience` raises `+0x94`. Gaining exp
only fills the `+0x470`/`+0x474` counter. **Level only advances inside
`train_level`, which the player must invoke at a trainer** — MajorMUD's classic
"go to a guild and train" model.

---

## 3. `cmd_train` — the TRAIN command

`cmd_train` (invoked as `train`) branches on argument count:

- **`train` (no arg)** → `train_level(user)` — spend banked exp to gain a level.
- **`train stats`** → the interactive stat-point editor. Preconditions:
  - must be in a room whose shop is a **trainer** (`room+0x43c==1`,
    `shop+0xcc==8`) and class-matched (`shop+0xd6==0` or `==player class`);
  - **no active stat-buff** may be running (rejects if any of abilities
    `0x2c..0x31` = Intel/Wisdom/Strength/Health/Agility/Charm is present —
    "Your stats are unnaturally altered"); otherwise you could bank buffed points.
  - On success it `save_player`s, drops the user to offline mode, and calls
    `edit_character_stats(0)` (returns 2 = went to a menu).
- Training of any kind is blocked while `DAT_004906c9 == 2` (a global lockout,
  e.g. combat/newbie state).

---

## 4. `train_level` — the actual level-up, step by step

`train_level(term)`:

1. **Location gate.** Player's room must be a shop (`room+0x43c==1`) whose
   `shop+0xcc == 8` (trainer/guild); else "You must be in an appropriate training
   [location]". Class gate: `shop+0xd6 == 0` or `== player class (+0x92)`.
2. **Kai refresh.** If class is a Kai/mystic class (`class+0x40 == 5`),
   `update_kai_powers` first.
3. **Shop level band.** `level+1` must be within `[shop+0xce, shop+0xd0]`
   ("not progressed far enough" / "progressed too far to use this trainer").
4. **Global level cap.** Allowed only if `level < DAT_00482d90`, **or** the player
   `haskey(DAT_00482db0)` (a key/flag that unlocks training past the soft cap);
   else "You may not train any further…".
5. **Experience check.** `new_calc_exp_needed(level, class[+0x24] + race[+0x62])`
   returns the 64-bit exp required for the *next* level; it is compared against the
   player's `+0x470`/`+0x474`. Insufficient ⇒ "You do not have the required
   experience". (Debug lines print NEW EXP diagnostics when the terminal's debug
   byte is set.)
6. **Cost.** `cost = (shop+0xd2 + 100) * (level * 5) / 100` currency units
   (`shop+0xd2` = the trainer's markup %). `check_currency`; insufficient ⇒ "You do
   not have the money required". On success `deduct_currency`.
7. **Level up:** `player+0x94 += 1`; "you receive training to attain level %d".
8. **Character points granted** (the currency for `train stats`):
   - `level < 11` → 10 CP
   - `level < 21` → 15 CP
   - else → `((level-1)/10)*5 + 10` CP
   Added to **both** `player+0x6e2` (unspent CP) and `player+0x6fa` (lifetime CP).
9. **HP-base roll:** `roll = genrdn(0, class[+0x22]+1)`; if nonzero,
   `player+0x724 += roll`. `+0x724` (the class HP-base byte, seeded from
   `class+0x22` at roll time) is a flat addend in the max-HP formula, so each level
   adds a random 0..`class[+0x22]` permanent HP.
10. **Lives:** `player+0x6a6 += DAT_00482cd8`, capped at 9; reports "%d additional
    lives" if it changed.
11. **Recompute:** `calculate_secondary_stats(term, 2)` — mode 2 recomputes max HP
    / max mana / all derived stats but **does not** refill current HP/mana (training
    does not heal). Kai classes refresh powers again. Returns success (1).

**Stat values themselves do not change at level-up.** The only automatic gains are
+1 level, +CP, +random HP-base, +lives. Raising the six primary stats is a separate
player-directed spend of CP via `train stats` → `edit_character_stats` (§6).

---

## 5. `calculate_secondary_stats(term, mode)` — the derived-stat formula set

Called with `mode` bit0 = "don't recompute max HP / preserve", bit1 = "don't reset
current HP/mana to max". `roll_stats` calls it with `mode 0` (full, sets
current = max); `train_level` with `mode 2` (recompute max, keep current).
It first calls `update_dynamic_stats` (which folds in equipment + active-spell
ability modifiers, e.g. accuracy `+0x70a`, AC `+0x70c`, crits `+0x7b4` — see
`spellcasting.md`), then computes:

Let `L = player+0x94` (level). A recurring **level growth term** `g(L)` appears in
the skill formulas: `g = L` for `L < 16`, else `g = 15 + (L-15)/2` (growth halves
past level 15). Stats below are the current effective values `+0xa2..+0xac`
(Int, Wis, Str, Hea, Agl, Chm) unless noted.

| field | offset | formula |
|-------|--------|---------|
| **Max HP** | `+0xae` | `Health_cur(+0x9c)/2 + L*(class[+0x20] + race[+0x2c]) + (Health_cur-50)*L/16 + HPbase(+0x724) + AlterHP(abil 0x58) + equipHP(+0x468)`. (Only recomputed when `mode` bit0 = 0.) |
| **Max mana** | `+0x600` | switch on `class[+0x40]` (caster group): groups 1-4 → `MaxMana(abil 0x45) + class[+0x42]*L*2 + manaBonus(+0x46c) + 6`; group 5 (Kai) → `L-1`; group 0/default → `0` (non-caster). |
| **SC (spell skill)** | `+0x604` | `L*2 + statTerm + class[+0x42]*5 + class[+0x42]` where `statTerm` depends on caster group: g1 `(Int*3+Wis)/6`, g2 `(Wis*3+Int)/6`, g3 `(Int+Wis)/3`, g4 `(Chm*3+Wis)/6`, g5 `500`, default `-150`. Then `+ S.C. ability (0x46)`. |
| **MR (magic resist)** | `+0xc0`/`+0xc2` | `(Int + Wis*3)/4 + M.R. ability(0x24)`. `+0xc0` current, `+0xc2` max. |
| **Perception** | `+0x5f8` | `(Int*5 + Wis*2 + Chm)/8 + Percep ability(0x4d)`. |
| **Stealth** | `+0x5fa` | gated by RaceStealth(0x66)/ClassStealth(0x67); `= Chm/6 + Agl/4 + levelTerm + Int/8 + 0x14` (levelTerm = `L*2` if `L<16` else `30 + (L-15)`), with a per-class ±adjust, `+ Stealth ability`; floored at 0. Non-stealth classes → 0. |
| **Thievery (rob)** | `+0x5fe` | gated by Thievery(0x27); `(Agl + Int + Chm + g*24)/6 + Thievery ability`; floored 0. |
| **Find/Disarm traps** | `+0x606`/`+0x608` | gated by FindTraps(0x28); `(Int + Agl + Chm*2 + g*28)/7`, `+0x606` += FindTrapsValue(0xb3) and DisarmTraps(0x29) bonus, `+0x608` = disarm partner value; floored 0. |
| **Picklocks** | `+0x60a` | gated by Picklocks(0x25); `((Agl + Int + g*10)*2)/7 + PickLocksValue(0xb4) + ability`; floored 0. |
| **Tracking** | `+0x60c` | gated by Tracking(0x26); `(Int*2 + Wis + Chm + g*40)/8 + Tracking ability`; floored 0. |
| **Dodge base** | `+0x710` | `L/10 + (Int-50)/10 + (Agl-50)/20 + (Chm-50)/30`, clamped to `[1, 75]` (byte). |
| **Dodge (combat)** | `+0x5fc` | `2*dodgeBase(+0x710) + crits(+0x7b4) + Chm/10 + Agl/5 + L/5 + Dodge ability(0x22)`; if JumpKick(0x23): `(that + L)*2`. |
| **Carry/encumbrance cap** | `+0xb2` | `Str*48`; if `Str > 100`, `+= Str*36 - 3600`. (Strength-derived weight limit — label tentative.) |

After computing, if `mode` bits allow (`bit0==0 && bit1==0`) it sets current mana
`+0x602 = +0x600` and current HP `+0x0b0 = +0xae` (fresh char full). In preserve
modes it instead clamps max ≥ current rather than overwriting current.

**Accuracy** is not set here directly; it is the province of `update_dynamic_stats`
(field `+0x70a`, fed by equipment + Accuracy abilities 0x16/0x69/0x6a and weapon
accuracy abilities), which `calculate_secondary_stats` invokes first.

---

## 6. Stat caps & the `train stats` editor

`edit_character_stats(reroll)` is the FSD (full-screen) stat editor entered from
`train stats`. For each of the six stats it presents **race base**
(`race+0x20..+0x2a`), **racial max / cap** (`race+0x66..+0x70`), and the player's
**current** value (`+0xa2..+0xac`), and lets the player spend CP to raise stats:

- **Racial max** `race+0x66` (Int), `+0x68` (Wis), `+0x6a` (Str), `+0x6c` (Hea),
  `+0x6e` (Agl), `+0x70` (Chm) is the hard ceiling; the editor will not raise a
  stat above it. This is the "race `+0x66` region" `records.md` referenced.
- **CP cost is escalating** (from the editor's own help text): the 1st +10 above
  base costs 1 CP each, the 2nd +10 costs 2 CP each, the 3rd +10 costs 3 CP each;
  jumping "+10 to base stat" costs 10 CP and "+40 to base stat" costs 100 CP.
- Available CP shown = `player+0x6e2` (unspent pool). `player+0x6fa` is the lifetime
  CP total. Both are seeded at character generation from **`race+0x46`** (the
  starting CP grant) and incremented each level by `train_level` (§4.8).

> **Correction to `records.md`:** in WG3-NT, `roll_stats` copies `race+0x46` into
> `player+0x6e2`/`+0x6fa` (the **character-point** pools), and `race+0x2c` is the
> race's **HP-per-level**. `records.md`'s "`race+0x46` → hitpoints base" and the
> `+0x6d3`/`+0x6e8` player offsets are from the older 16-bit layout; the WG3-NT
> offsets are `+0x6e2`/`+0x6fa` (CP) and the HP fields differ accordingly.

---

## 7. Summary of the progression model

1. Kill/quest awards fill a base-10⁹ experience counter (`+0x470`/`+0x474`); combat
   exp is split among the party and capped a fixed number of levels ahead of your
   trainable level; quest exp is uncapped.
2. Experience **never** auto-levels. The player must `train` at a class-matched
   trainer shop (type 8) within the shop's level band and under the global cap,
   pay a level-scaled fee, and have enough exp for the next level.
3. Training grants exactly: **+1 level** (`+0x94`), a batch of **character points**
   (10 / 15 / escalating), a **random HP-base increment** (`0..class[+0x22]` into
   `+0x724`), and **+lives** (capped 9). It does not heal or auto-raise stats.
4. Primary stats are raised separately by spending CP in `train stats` /
   `edit_character_stats`, bounded by racial max caps (`race+0x66..+0x70`), with
   escalating CP cost.
5. `calculate_secondary_stats` then recomputes max HP, max mana, SC, MR,
   perception, the four thief skills, and dodge from the six primary stats, the
   level, and class/race record fields (see §5 for exact formulas).

## 8. Undetermined / flagged

- `+0xb2` (Str*48) is inferred as a carry/encumbrance capacity from its Strength
  derivation; not confirmed against a consumer.
- The exact per-class ±adjust to stealth (`+0x5fa`) and the class-group→mana-stat
  mapping (`class+0x40 ∈ {1,2,3,4,5}` → Int/Wis/hybrid/Chm/Kai) are transcribed
  from the switch but not cross-checked against WCCCLASS.VIR data.
- `class+0x20` (HP/level), `class+0x22` (HP-roll max & initial `+0x724`),
  `class+0x40` (caster group), `class+0x42` (casting level factor) and
  `race+0x2c` (HP/level), `race+0x46` (starting CP) are named from usage here; a
  disk-schema pass on WCCCLASS/WCCRACE would confirm the field widths.
- The two AlterHP-ability additions in the max-HP block appear once as a decompiler
  artifact (the `sVar2` term is added twice around the `get_user_ability_value(0x58)`
  call); the intended contribution is a single AlterHP addend.
