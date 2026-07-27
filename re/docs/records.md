# MajorMUD 1.11p — Record layouts (in-progress)

Offsets recovered from decompiled accessors (`_ROLL_STATS`, `_GET_RACE_DATA`,
`_GET_CLASS_DATA`, `_GET_PLAYER`). Byte offsets into each struct. Incomplete —
fields fill in as more accessors are read.

## Player record (returned by `_GET_PLAYER`)

| offset | field |
|--------|-------|
| `+0x90` | race id (index into WCCRACE) |
| `+0x92` | class id (index into WCCCLASS) |
| `+0x94` | **character level** (CONFIRMED: incremented +1 by `train_level`; read as an int by exp/stat/mana/regen code. NOT gender/reroll — the `< 2` test in `_ROLL_STATS` is a char-generation guard.) |
| `+0x96 .. +0xa0` | **current** stats ×6: order **Int, Wis, Str, Hea, Agl, Chm** (CONFIRMED = ability ids 44–49; Nightmare labels stat 2 "Wil"/Willpower — same stat) |
| `+0xa2 .. +0xac` | **max/effective** stats ×6 (same order) |
| `+0x6d3`, `+0x6e8` | (16-bit-era offsets; WG3-NT uses `+0x6e2`/`+0x6fa`) copied from race `+0x46` — that race field is the **starting character-point grant**, not hitpoints (race HP/level is race `+0x2c`) |
| `+0x70d` | byte, from class `+0x22` |
| `+0x7b9` | byte, from an accessor at `_ROLL_STATS`+... (`+0xd5` of another struct) |
| `+0x7c`  | name buffer (sprintf target in `_ROLL_STATS`) |
| `+0x117` (as word array) | crit rating in the attack profile — NB different struct |

## Race record (WCCRACE.VIR, 126-byte records; `_GET_RACE_DATA`)

13 races: Human, Dwarf, Gnome, Halfling, Elf, Half-Elf, Dark-Elf, Half-Orc,
Half-Ogre, Goblin, Kang, Nekojin, Gaunt One.

| offset | field |
|--------|-------|
| `+0x00` | name (null-terminated, space-padded, ~25 bytes) |
| `+0x20 .. +0x2a` | base stats ×6 (words) — copied to player current+max in `_ROLL_STATS` |
| `+0x46` | **starting character-point grant** (CORRECTED — not hitpoints); race HP-per-level is `+0x2c` |

## Class record (WCCCLASS.VIR, 153-byte records; `_GET_CLASS_DATA`)

Classes: Warrior, Witchunter, Paladin, Cleric, Priest, Missionary, Ninja, Thief,
Bard, Gypsy, Warlock, Mage, Druid, Ranger (+ more).

| offset | field |
|--------|-------|
| `+0x00` | name (null-terminated, space-padded, ~25 bytes) |
| `+0x22` | byte copied to player `+0x70d` |

## Item record (WCCITEMS.VIR; `_GET_ITEM_DATA`) — partial

| offset | field |
|--------|-------|
| `+0x33e` | weapon **min damage** |
| `+0x340` | weapon **max damage** |
| `+0x39b` | **damage resistance** — summed into fighter `[3]`, the SOAK word (dmg −= `[3]`/10). NOT the AC/to-hit stat: that is a separate column (WG3-NT `+0x342`, DB `ac`) which feeds fighter `[1]` ÷10. Both are called "armour" in the data and both ship ×10; see the warning in `combat.md`'s WG3-NT fighter section. |

Player worn-equipment slots reference items by (id-lo, id-hi) pairs, e.g.
`player[+0x617]/[+0x619]` (weapon), `player[+0x61f]/[+0x621]` (a worn slot).

## More player-record offsets (from `_MOVE_PLAYER_TO_FIGHTER`)

| offset | field |
|--------|-------|
| `+0xb0` | current hitpoints (`<1` ⇒ fighter dodge set to −1, i.e. helpless) |
| `+0x6fd` | crit-rating byte (added into fighter crit `[0x117]`) |
| `+0x79b` | crit-rating word (added into fighter crit `[0x117]`) |
| `+0x617`/`+0x619` | equipped weapon item id (lo/hi) |

Ability IDs seen (via `_GET_USER_ABILITY_VALUE(id,...)`): `0x18`/`0x19` weapon accuracy
(main/off hand), `0x1d`/`0x1e`/`0x23` martial-arts damage tiers, `0x22` dodge,
`0x74` overall accuracy, `0x25`/`0x1d` unarmed. (IDs index the ability system; names TBD.)

## Monster records (from `_MOVE_MONSTER_TO_FIGHTER`)

Two structures: the **template** (`_GET_KNOWN_MONSTER_DATA` → `iVar3`, static per
monster type, from WCCKNMSR/WCCMP001) and the **live instance** (`param_1`, the monster
in the room). A monster has multiple attacks selected by `param_3` (attack index),
indexing parallel arrays with stride 2 (words).

### Template (per-attack arrays indexed `+ param_3*2`)

| offset | field |
|--------|-------|
| `+0x123 + p*2` | attack **accuracy** (base) → fighter `[0]` |
| `+0x132 + p*2` | attack **min damage** → fighter `[8]` |
| `+0x13c + p*2` | attack **max damage** → fighter `[9]` |
| `+0x182 + p*2` | attack **message id** (32-bit) → fighter `[6]/[7]` (giant rat = 1000) |
| `+0x6e` (byte)  | stat used in backstab-mode level scaling |
| `+0x0d`, `+0x36`, `+0x5f` | name/short/long strings (`+0x34` is the disk name field, verified) |

> Correction: the `+0x404/+0x406` and `+0x408/+0x40a` attack-message pointers that
> feed fighter `[4]/[5]` come from the **wielded-item** record (`iVar4 = _GET_ITEM_DATA`,
> 1061 B), NOT the monster template — an earlier mis-attribution. The monster template
> offsets above are disk-verified against WCCKNMSR (see `vir_schemas.md`).

### Live instance (`param_1`)

| offset | field |
|--------|-------|
| `+0xb0/+0xb2` | wielded item id (lo/hi); if set, overrides `[6]/[7]` from item data |
| `+0x10a` | armor base → fighter `[3] = base*10` |
| `+0x10c` | level/power; → fighter `[1]`; backstab: `[1]=(level/2 + tmpl[+0x6e]*2)/2` |
| `+0x121` (byte) | status flags; bit `0x02` set ⇒ accuracy zeroed / `-= DAT_1150_0078` |

### Monster ability modifiers (layered onto template bases)

`_GET_MONSTER_ABILITY_VALUE(id,...)` adds to fighter fields:
accuracy `[0]` += abilities `0x16`, `0x69`, `0x6a`; backstab `[1]` += `2`;
min/max dmg `[8]/[9]` += `4`; armor `[3]` += `7`; dodge `[10]` = ability `0x22`
(**same dodge ability id as players** — consistent); `[0x118]` = ability `0x68`;
ability `0x57` gates a further special block.

**Finding: monster fighter crit `[0x117]` is hard-set to 0** → the critical-hit branch
in `_CALCULATE_ATTACK` never fires for a monster attacker. **Criticals are a
player-only mechanic.**

## Experience curve — VERIFIED from disassembly (`_CALC_EXP_NEEDED`)

Signature is **two args** (`_CALC_EXP_NEEDED(level, base)`) — the decompiler modeled only
one and hid the seed input. Callers pass `base = classData[+0x24] + raceData[+0x62]`,
so the exp base is class- and race-dependent. (A newer `_NEW_CALC_EXP_NEEDED` variant exists.)

Verified algorithm (disasm `1008:2497`, `mult`/`div` per-level ratio applied iteratively):

```
seed  = 10 * (base + 100)          # = 10*base + 1000
E = seed
for i in 0 .. level-1:
    (mult, div) = ratio(i)
    E = E * mult / div             # unsigned long; overflow-guarded (see note)
return E                           # exp needed to reach `level`
```

`ratio(i)` for `i < 26` comes from an in-DLL table at `DS 0x1110:0x0004` (word pairs
`{mult, div}`, stride 4). **Extracted values:**

| i (level idx) | mult/div | ratio | | i | mult/div | ratio |
|---|---|---|---|---|---|---|
| 0 | 1/1 | 1.000 | | 13 | 65/45 | 1.444 |
| 1 | 40/20 | 2.000 | | 14 | 70/50 | 1.400 |
| 2 | 44/24 | 1.833 | | 15 | 70/50 | 1.400 |
| 3 | 44/24 | 1.833 | | 16 | 75/55 | 1.364 |
| 4 | 48/28 | 1.714 | | 17 | 50/40 | 1.250 |
| 5 | 48/28 | 1.714 | | 18 | 50/40 | 1.250 |
| 6 | 52/32 | 1.625 | | 19 | 50/40 | 1.250 |
| 7 | 52/32 | 1.625 | | 20 | 50/40 | 1.250 |
| 8 | 56/36 | 1.556 | | 21 | 50/40 | 1.250 |
| 9 | 56/36 | 1.556 | | 22 | 50/40 | 1.250 |
| 10 | 60/40 | 1.500 | | 23 | 50/40 | 1.250 |
| 11 | 60/40 | 1.500 | | 24 | 50/40 | 1.250 |
| 12 | 65/45 | 1.444 | | 25 | 23/20 | 1.150 |

For `i >= 26` the ratio is hard-coded (`div = 100`): **i 26–52 → 115/100 (1.15)**,
**i 53–60 → 110/100 (1.10)**, **i 61+ → 105/100 (1.05)**. The table's own i=25 value
(1.15) matches the following band, so the handoff is smooth.

> **[16-bit / WG3-NT]** The above is `_calc_exp_needed` (both builds share it, and the
> seed + L1–25 table are byte-identical). But the **live WG3-NT engine calls
> `_new_calc_exp_needed`** — same seed and low-level table, but the high-level bands taper
> to **~108/100** instead of 105 at the top end. For the WG3-NT target, use
> `new_calc_exp_needed`. (See `../wg_nt_ghidra/wg_nt_findings.md`.)

**Overflow guard (decompiler noise, not math):** when `E*mult` would exceed 32 bits, the
code divides `E` by 100 (up to twice), does the step, then multiplies back by 100 —
a fixed-point rescale. It does not change the result except for precision at very high
exp; the repeated `÷100`/`×100` in the decompiled C is *this*, not part of the curve.

Earlier decompiler-based reading (`mult/100`) was directionally right on the 115/110/105
bands but **missed** the low-level table, the `10*(base+100)` seed, and the class/race
base input. All now confirmed against the disassembly.
