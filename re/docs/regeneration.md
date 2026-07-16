# MajorMUD 1.11p — Regeneration: HP, mana, hunger/thirst (WG3-NT)

Behavioral spec of out-of-combat recovery, derived from the clean 32-bit WG3-NT
decompile (`wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Cross-references the tick
system in [`combat_rounds.md`](combat_rounds.md), the spell-upkeep tick in
[`spellcasting.md`](spellcasting.md), record offsets in [`records.md`](records.md),
and ability ids in [`abilities.md`](abilities.md). Byte offsets are into the player
record (`get_player`).

**Bottom line up front.** HP and mana regen are **one job on the 30-second slow
tick** (`slow_update_character`, `0x221ad`). There is *no* active-vs-resting regen
distinction — regen fires every slow tick regardless of combat. Hunger and thirst are
**vestigial counters** in this build: they tick down each slow tick but nothing reads
them to gate regen or damage the player. The recurring Heal / Drain / HPRegen etc.
*spell* effects are a **separate** system on the 3-second medium tick and must not be
double-counted against slow-tick regen.

---

## 1. Cadence and driver chain

Regen lives entirely in the **slow** background job. Chain (interval globals and their
`.data` defaults are from `combat_rounds.md §1`):

```
background_slow (0x215f6, every DAT_00482cbc = 30 s)
  └─ slow_update_characters (0x219f9)          // loops terminals 0..nterms
       └─ slow_update_character(user) (0x221ad) // for each in-game user
            ├─ hunger/thirst decrement
            ├─ poison damage
            ├─ near-death bleed / recovery
            ├─ HP regen
            └─ mana regen
```

`slow_update_characters` (`0x219f9`) walks every terminal (`_nterms_exref`) and calls
`slow_update_character` for each user that is in-game (`FUN_0046b93c` guard). So **the
regen cadence is 30 real seconds** — six combat rounds long (the round is 5 s,
`background_energy`; see `combat_rounds.md §1`).

> **Not HP/mana regen:** `background_energy` (5 s) regenerates only the *energy pool*
> (`+0xb8`/`+0xba`, the per-round attack budget) via `energy_update_character`
> (`0x2131f`). `background_medium` (3 s) runs the spell-upkeep tick (§6), not baseline
> regen. `background_fast` (1 s) does neither.

---

## 2. HP regeneration

In `slow_update_character`, after the poison and near-death branches (§4/§5), the HP
handler is the final arm of an `if/else-if` chain that runs **only when the player is
alive and not at full HP** (`0 < HP < maxHP`; HP `+0xb0`, maxHP `+0xae`):

```
base = ((level + 20) * Health) / 750          // level=+0x94, Health=+0xa8, 0x2ee=750
if base < 2: base = 1                          // floor: minimum regen is 1
if HPRegen(+0x7d8) != 0:                        // ability 123 accumulator, a percent
    base = ((HPRegen + 100) * base) / 100
if HP < maxHP:  HP += base
if HP > maxHP:  HP = maxHP                       // hard cap at max
if HP > 0:      clear aided-flag (+0x6a8 = 0)    // see §5
```

Evidence: lines 19546-19565 of the decompile.

**What scales HP regen**

| input | offset | effect |
|-------|--------|--------|
| **Level** | `+0x94` | linear: `(level + 20)` term. Same field `train_level` (`0x1e6e8`) compares against the exp curve and shop min/max level, i.e. it **is the character level** (see §8 on the `records.md` mislabel). |
| **Health** stat | `+0xa8` | linear multiplier. `+0xa8` is the **Health** entry of the max-stat array (§7 resolves the stat order). |
| **HPRegen** ability 123 | `+0x7d8` | percent bonus: `(HPRegen% + 100)/100`. Accumulated from equipment/spells by `update_dynamic_with_ability` (`spellcasting.md §4`). Item/spell driven; 0 for a bare character. |
| floor | — | result clamped up to **1** if the formula yields 0 or 1. |
| cap | `+0xae` | regen never overshoots max HP. |

So baseline HP/30 s ≈ `(level+20)·Health/750`, at least 1, times the HPRegen%
multiplier, capped at max. Class does **not** enter the HP formula (unlike mana).
Regen is **not** suppressed in combat and **not** gated by poison — a poisoned but
living player still regenerates in the same tick the poison damages them (poison is a
separate earlier `if`, HP regen is the later `else if`).

---

## 3. Mana regeneration

Same tick, immediately after HP. Gated on **current mana below max** (`+0x602 < +0x600`).
Non-casters have max mana `+0x600 == 0`, so the guard is false and the block is skipped
entirely — they never run this code. Evidence: lines 19567-19606.

```
class = get_class_data(+0x92)
if class == 0:                                   // no class record: trivial +1
    mana += 1
else:
    stat = switch(class[+0x40]):                 // class "spell group / casting stat"
        case 1 -> Intellect (+0xa2)
        case 2 -> Willpower (+0xa4)
        case 3 -> (Willpower + Intellect) / 2
        case 4 -> Charm     (+0xac)
        default -> 0
    regen = ((level + 20) * stat * (class[+0x42] + 2)) / 1650   // +0x94, 0x672=1650
    if class[+0x40] == 5:  regen = 1             // mystic/kai: fixed 1 per tick
    if ManaRgn(+0x7da) != 0:                      // ability 145 accumulator, a percent
        regen = ((ManaRgn + 100) * regen) / 100
    mana += regen
    if mana < 0:        mana = 0
    if mana > maxMana:  mana = maxMana            // cap at +0x600
```

**What scales mana regen**

| input | offset | effect |
|-------|--------|--------|
| **Level** | `+0x94` | linear `(level + 20)` term (same field as HP). |
| **Casting stat** | class `+0x40` selects | Intellect (`+0xa2`), Willpower (`+0xa4`), their average, or Charm (`+0xac`). Matches the classic class→stat map: mages Int, cleric/priest Wil, hybrids average, bard/gypsy Charm. |
| **Class casting tier** | class `+0x42` | linear `(tier + 2)` term. `+0x42` is the class's max castable spell level (`spellcasting.md §2`). Stronger caster classes regen faster. |
| **Mystic override** | class `+0x40 == 5` | ignores the formula; **fixed 1 per tick** (kai/mystic). Consistent with `show_health` and casting using a "Kai/power" label for this class. |
| **ManaRgn** ability 145 | `+0x7da` | percent bonus `(ManaRgn% + 100)/100`, item/spell driven. |
| cap | `+0x600` | clamped to `[0, maxMana]`. |

Baseline mana/30 s ≈ `(level+20)·stat·(tier+2)/1650` for a normal caster, ×ManaRgn%,
capped. The `class == 0` fallback (`mana += 1`) is a degenerate safety path; live caster
classes always resolve a class record.

`show_health` (`0x34af7`) is the reporting side: it prints HP `+0xb0`/`+0xae` and, when
`+0x600 != 0`, mana `+0x602`/`+0x600` with percentages, using a "Kai/power" caption for
class `+0x40 == 5` and "Mana" otherwise.

---

## 4. Poison (interacts with the same tick)

Before regen, if `+0xbe > 0` (poison counter) the tick prints "You feel ill" and does
`HP -= +0xbe`, with a death check (lines 19518-19533). Poison is decremented/cured by the
Cure-Poison spell upkeep (ability 20, §6) and reversed on the poison spell's termination
(`spellcasting.md §5`), not here. It does not stop HP regen: a living poisoned player is
damaged **and** regenerates in the same 30 s tick.

---

## 5. Near-death "recovery": bleeding vs AID (there is no player "rest mode")

The closest thing to a player recovery/rest mode is the **bleed-out / stabilize**
mechanic, keyed on the aided-flag `+0x6a8`. When a player is at **HP < 1** (down but not
yet removed), the slow tick branches (lines 19534-19545):

- **HP < 1 and `+0x6a8 == 0`** (not aided): `HP -= 1` each tick, then `check_kill_user` —
  the player **bleeds toward death**, one point per 30 s. The look/health display shows
  "mortally wounded ... and is bleeding heavily" (`0x484...`, line 33803).
- **HP < 1 and `+0x6a8 != 0`** (aided): `HP += 1` each tick — the player **stabilizes and
  slowly recovers** back above 0, at which point normal regen (§2) resumes and clears the
  flag.

**How the flag is set:** the **AID** command (`cmd_aid`, `0x58154`, body near line 53600) targets a
downed player (`target HP < 1`), sets `target+0x6a8 = 1`, and prints "You have aided %s;
%s's wounds are [bound]". Aiding an uninjured target is rejected ("is in no need of
assistance"). The flag is cleared to 0 on character (re)load (`_Dest[0x6a8]=0`, line 10948),
in `fast_update_character` (line 19903), and by normal regen once HP climbs above 0.

So the "recovery" the engine exposes for a character is **bleed-out control**, not a
rest/meditate speed-up. Regen rate does not change based on sitting still, fighting, or
resting.

### `display_recovery_mode_status` is a *server* mode, not player HP

`display_recovery_mode_status` (`0x2fab0`) prints "Please note: Recovery mode in
pr[ogress]" gated on the global `DAT_00482134`. This is the **world reinitialization /
crash-recovery** subsystem (see the adjacent `reinitialize_buffers`, `0x2fad2`, which
reports "Last Cycle Map/Room/Mon…"), i.e. the engine rebuilding the world after a restart.
It has **nothing to do** with HP/mana regen and must not be conflated with player recovery.

---

## 6. Hunger and thirst

Player hunger `+0xce` and thirst `+0xd0` (word counters; same fields the Alterhunger 15 /
Alterthirst 16 abilities write per `spellcasting.md`). Every slow tick they simply
decrement by 1 (lines 19516-19517):

```
hunger(+0xce) -= 1
thirst(+0xd0) -= 1
```

They are replenished by:

- **`cmd_eat`** (`0x673c3`, food item type `[0xbd]==4`): on a no-effect eat, `+0xce += 1`
  (line 62388); an item with `use_no_target` charges runs its abilities instead.
- **`cmd_drink`** (`0x67574`, drink type `[0xbd]==5`): on a no-effect drink, `+0xd0 += 1`
  (line 62477).
- **Alterhunger (15/0xf)** → `+0xce += V` and **Alterthirst (16/0x10)** → `+0xd0 += V`,
  applied instantly at cast and per-tick in the spell-upkeep loop (§6 of `spellcasting.md`).

**Do they gate regen? No.** A full search of every comparison/read of player `+0xce` and
`+0xd0` finds **no** consumer that penalizes low/zero hunger or thirst: the other hits on
those offsets are the spell record's own `+0xce` (spell duration) / `+0xd0` (damage
element), the shop training min/max at `iVar6+0xce/+0xd0`, and a save-state field compare.
There is **no starvation damage, no "you are hungry" warning, and no regen gate** in the
WG3-NT build. Hunger/thirst tick down but are effectively cosmetic here. (Regen in §2/§3
runs with no reference to `+0xce`/`+0xd0`.)

---

## 7. Stat offsets (resolved, corrects `records.md`)

The regen formulas need the exact stat identities. The character-creation stat display
(lines 1853-1863) labels the race/class stat block, and `roll_stats` (`0x1a2d1`,
lines 13317-13328) copies it verbatim into the player max-stat array `+0xa2..+0xac` (and
current `+0x96..+0xa0`). Resolved order:

| player max off | current off | stat |
|----------------|-------------|------|
| `+0xa2` | `+0x96` | **Intellect** |
| `+0xa4` | `+0x98` | **Willpower** (a.k.a. Wisdom) |
| `+0xa6` | `+0x9a` | **Strength** |
| `+0xa8` | `+0x9c` | **Health** |
| `+0xaa` | `+0x9e` | **Agility** |
| `+0xac` | `+0xa0` | **Charm** |

This is why HP regen (`+0xa8`) is **Health**-driven and mana regen selects Intellect
(`+0xa2`) / Willpower (`+0xa4`) / average / Charm (`+0xac`) by class.

> **Correction to `records.md`:** its tentative order "STR, INT, WIS, AGI, HEA, CHA" for
> `+0xa2..+0xac` is wrong. The disk/creation order is **INT, WIL, STR, HEA, AGL, CHM**.

---

## 8. Ability interactions — who owns what (avoid double-counting)

Two independent systems touch HP and mana. Keep them separate:

| system | tick / driver | HP | mana | hunger/thirst |
|--------|---------------|----|----|----|
| **Baseline regen** (this doc) | 30 s `slow_update_character` | `(level+20)·Health/750`, ×HPRegen%, cap | `(level+20)·stat·(tier+2)/1650`, ×ManaRgn%, cap | −1 each (decay only) |
| **Spell upkeep** (`spellcasting.md §5`) | 3 s `medium_update_character` → `perform_routine_spell_player_upkeep` | Heal 18 `+V`, Drain 8 `−V`, Damage 1 `−V` | HealMana 150 `+V` | Alterhunger 15 `+V`, Alterthirst 16 `+V` |

Key non-overlap facts:

- **HPRegen (123 / `+0x7d8`)** and **ManaRgn (145 / `+0x7da`)** are **not** per-tick
  healing effects. They are *percent modifiers* to the baseline slow-tick formulas only
  (§2/§3). `update_dynamic_with_ability` accumulates them into `+0x7d8`/`+0x7da` from
  active spells and worn equipment; they do nothing on the medium tick. Do not model them
  as a separate heal-over-time.
- **Heal (18)** and **HealMana (150)** *are* heal-over-time and live only in the 3-second
  spell-upkeep loop, applied per active duration-spell slot. They stack additively on top
  of baseline regen but are owned by the spell system, capped independently to max HP/mana.
- **Alterhunger / Alterthirst** are the only writers that raise `+0xce`/`+0xd0`; the slow
  tick only ever lowers them. Since nothing reads hunger/thirst, this interaction has no
  gameplay consequence in WG3-NT.
- Baseline regen and spell upkeep run on **different clocks** (30 s vs 3 s) and never share
  code, so there is no risk of the slow-tick formula being applied twice.

---

## 9. Unresolved / flagged

- **`+0x94` label.** The regen formulas, the spell path (`spellcasting.md §7`), and
  `train_level` all treat `+0x94` as the **character level**. `records.md` labels it
  "gender / reroll flag (checked `<2`)". The `<2` check appears only in `roll_stats` at
  creation (level 0/1), so the field almost certainly **is character level**; this spec
  follows that. `records.md` should be corrected. (Not independently re-derived here beyond
  the `train_level` cross-check.)
- **Second-mapping of the 30 s cadence** inherits the same caveat as `combat_rounds.md §1`:
  the interval value 30 (`DAT_00482cbc`) is read from the binary and certain; the absolute
  seconds rest on Worldgroup's `rtkick` seconds convention.
- **Mystic/kai (`class+0x40==5`)** mana regen is a flat 1/tick here. Whether Kai "power"
  has a separate faster-recovery path elsewhere (`update_kai_powers`, called from
  `train_level`) was not chased down; the slow-tick contribution is the flat 1.
- **`class+0x40` full taxonomy.** Cases 1-5 are decoded above from usage; the meaning of
  `+0x40` values outside 1-5 (default → stat 0 → mana += 0) is inferred as "non-mana class"
  and not separately confirmed.
