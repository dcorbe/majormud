# MajorMUD 1.11p — Combat resolution (`_CALCULATE_ATTACK`)

Reconstructed from Ghidra decompilation. Offsets below are for the **16-bit DOS build**
(`WCCMMUD.DLL`, `_CALCULATE_ATTACK`, 562-byte fighter, word-indexed).

> ## ⚖ TWO-BUILD RECONCILIATION (read first)
> Cross-checked against the clean **32-bit WG3-NT build** (`wccmmud.dll`, 592 named
> exports — see `../wg_nt_ghidra/wg_nt_findings.md`). **The project targets WG3-NT.**
>
> **Core math — CONFIRMED IDENTICAL in both builds** (strong validation): the to-hit
> quadratic `threshold = 100 − 140·defense²/accuracy²` (clamp [10,99], `den==0→5`),
> backstab threshold, helpless-gate, damage `rand(min..max) − armor/10`, and the parry
> base formula are byte-equivalent. The WG3-NT fighter struct is the same layout widened
> word→dword (588 B vs 562 B) — every field role preserved at 2× stride.
>
> **Peripheral tuning — DIFFERS between builds.** Where a section is marked
> **[16-bit / WG3-NT]**, the WG3-NT value is authoritative for this project:
> - **Attack-type table** is re-indexed (see the table below — WG3-NT column).
> - **Damage seed** is a percentage *bonus* in WG3-NT (`dmg *= (seed+100)/100`), NOT the
>   inverse `dmg *= 100/seed` this doc originally recorded for the 16-bit build. The
>   cleaner 32-bit decompile makes the WG3-NT reading the reliable one.
> - **Crit floor**: WG3-NT sets `min = 2·max_old` (higher floor); 16-bit doc had `min *= 2`.
> - **Backstab parry**: WG3-NT divides parry chance by 5; 16-bit halved it.
> - **Exp**: the live WG3-NT engine calls `_new_calc_exp_needed` (same seed + L1–25 table,
>   but high-level bands taper to ~108, not 105 — see `records.md`).

Field names are inferred from behavior. Treat as a working spec.

## Inputs — the FIGHTER struct

`_CALCULATE_ATTACK(param_1, param_2)` takes two **562-byte "fighter" structs**, not
the raw player/monster records. They are static globals in segment `0x1140`:
attacker at `0x1140:0x003a`, defender at `0x1140:0x026c` (`0x26c-0x3a = 0x232 = 562`).
Each is built by `_MOVE_PLAYER_TO_FIGHTER` / `_MOVE_MONSTER_TO_FIGHTER` before the call.
Same layout is used whether the fighter is attacking or defending.

Fighter struct (16-bit **word** indices; byte offset = index×2). Origins from
`_MOVE_PLAYER_TO_FIGHTER`:

| word | byte | field | populated from |
|------|------|-------|----------------|
| `[0]`     | 0    | **overall accuracy** (used as `att[0]`; parry denom `att[0]/8`) | `ability(0x74)/2 + hit-bonuses` |
| `[1]`     | 2    | backstab compare value (`def+2`) | (context-dependent) |
| `[2]`     | 4    | **weapon to-hit** rating | `ability(0x18 main / 0x19 off)`, `+10` if `ability(9)` |
| `[3]`     | 6    | **armor** = DAMAGE RESISTANCE (`def+6`, dmg −= armor/10) — **not** the AC/to-hit stat, which is `[1]`; see the warning under the WG3-NT build below | Σ worn-item DR `item[+0x39b]` |
| `[4]`     | 8    | attack **message id** (`def+8`) | weapon/verb |
| `[5]`     | 10   | attack **message id 2** (`def+10`) | weapon/verb |
| `[6]/[7]` | 0xc  | weapon ref / passthrough (`att[6]`→`DAT_1140_0010`) | `_GET_ITEM_DATA` (32-bit) |
| `[8]`     | 0x10 | **min damage** (default 1) | weapon `item[+0x33e]`, or martial-arts formula; `+= STR bonus` |
| `[9]`     | 0x12 | **max damage** (default 4) | weapon `item[+0x340]`, or MA tiers `base+6/7/8`; `+= dmg bonus` |
| `[10]`    | 0x14 | **dodge/parry** (`def+0x14`) | `ability(0x22) + (stat[+0xac]−50)/5 + field[+0x94]/5 + (stat[+0xaa]−50)/3`; **−1 if HP<1** |
| `[0x117]` | 0x22e| **crit rating** (min 1) | `player[+0x6fd](byte) + player[+0x79b](word)` |
| `[0x118]` | 0x230| crit flag scratch | cleared to 0 |

Notes:
- The **HP<1 → dodge = −1** case is exactly the "negative dodge" auto-hit branch in
  `_CALCULATE_ATTACK` step 2: a dead/incapacitated defender is trivially hit.
- Martial-arts (unarmed) min damage = `(gender_field × ability(0x1d)) >> 3 + 2`
  (or `ability×20/8 + 2`); this is why `field[+0x94]` (the `<2` gender/reroll field)
  scales barehanded damage.

- `DAT_1180_0002` — **attack type selector** (see table).

## Attack type table  **[16-bit / WG3-NT]** — differs between builds

Each type sets a damage seed, an accuracy modifier (added to accuracy in to-hit), whether
the attacker's crit rating is zeroed, and (types 6/7) a separate on-hit damage multiplier.
The **accuracy penalties are re-indexed** in WG3-NT — shifted down one slot, with an extra
type 8 carrying the −75. WG3-NT is authoritative for this project.

| type | seed | acc mod (16-bit) | acc mod (**WG3-NT**) | crit? | dmg mult |
|------|------|------------------|----------------------|-------|----------|
| 1 | 0   | 0   | 0    | yes | — |
| 2 | 33  | 0   | 0    | yes | — |
| 3 | 66  | 0   | 0    | yes | — |
| 4 | 0/10| −15 | **0** | no | — (backstab: special threshold + parry ÷5) |
| 5 | 0   | 0   | 0    | yes | (default) |
| 6 | 10/20 | −25 | **−15** | no | **×3** |
| 7 | 20/125| −75 | **−25** | no | **×5** |
| 8 | 0/125 | 0  | **−75** | no | — |

(Seed values also shifted between builds; the ×3/×5 gating and crit-disable stay keyed to
types 6/7 in both.)

**Damage-seed rescale — DIFFERS:**
- **WG3-NT (authoritative):** when `seed != 0`, `min/max *= (seed+100)/100` — a percentage
  **bonus** (type 2 → +33%, type 8 → +125%), applied *before* the ×3/×5 multipliers.
- **16-bit (this doc's original read):** `dmg *= 100/seed` (inverse). The cleaner 32-bit
  decompile makes the WG3-NT percentage-bonus reading the reliable one; the 16-bit inverse
  may have been a misread of the same intent.

## To-hit

> **genrdn's bounds (CORRECTED 2026-07-28):** `genrdn(lo,hi)` spans
> `[lo, hi)` — the upper bound is EXCLUSIVE (see theft.md's RNG note for
> the three-way proof: MBBSEmu's `_random.Next`, the DLL's own
> `(max-min)+1` damage idiom, the Nekojin Mystic 2..6 capture). So the
> attack roll spans [1, 99] and every `genrdn(0,100)` gate is
> chance-in-100 exactly.

1. Roll `attackRoll = genrnd(1,100)` — spans **[1, 99]**.
2. Determine **hit threshold** `local_a` (with sign word `local_8`):
   - If `def[+0x14] < 0` and `genrnd(0,100) > def[+0x14]+100`: threshold = 99 (near-auto-hit path).
   - Else if type == 4 (backstab): threshold = `att[0] - def[+2]` (signed 32-bit).
   - Else (normal): **recovered formula** (see below):
     `threshold = 100 - 140 * defense² / accuracy²`.
   - The `den == 0` arm yields a raw 5 — which the clamp below lifts to 10.
3. **Clamp** threshold to `[10, 99]` — the clamp sits OUTSIDE the whole
   if/else (decompile 25315; 16-bit `1f5b` falls through to `1f60`), so it
   covers the den==0 arm and the backstab difference alike.
4. **Hit if** `attackRoll < threshold` — STRICT (decompile 25324
   `iVar3 < iVar2`): **P(hit) = (threshold−1)/99**. A threshold-99 swing
   misses only on a rolled 99 (~1%); the clamp floor connects 9/99 ≈ 0.0909.

## On a hit

1. **Critical:** `genrnd(0,100) < att[0x117]` and type ∉ {6,7} → critical, `result = 4`.
   Damage scaling **[16-bit / WG3-NT]**: 16-bit `min *= 2, max *= 4`; **WG3-NT**
   `min = 2·max_old, max = 4·max_old` (higher floor). Crit rating is diminishing-returns
   capped in both: if `rating > 40`, `rating = 40 + (rating−40)/3`.
2. Ensure `max >= min`.
3. **Damage:** `dmg = genrnd(0, max-min+1) + min - def[+6]/10` — rand in
   `[min, max]` exactly: genrdn's exclusive top is WHY the idiom carries
   the `+1`. A fixed-magnitude row (12..12) always deals 12.
4. Type multipliers: type 6 → `dmg *= 3`; type 7 → `dmg *= 5`.
5. **Parry/riposte:** parry chance `p = (def[+0x14] * 10) / (att[0] >> 3)`, capped at `0x5f`
   (95). The guard is on the **accuracy**, not on the shifted denominator: `att[0] < 9` →
   `p = 0` outright (so accuracy 8 is unparryable rather than dividing by 1).
   Backstab (type 4) reduces it: **[16-bit / WG3-NT]** 16-bit halves (`p/2`); **WG3-NT** `p/5`.
   If `def[+0x14] > 0` → **draw** `genrnd(0,100)`; `< p` → `result = 3` (parried), damage 0,
   **return**. The draw is taken whenever the defender has any parry rating — including when
   `p` is 0 — so it is part of the RNG stream either way.
6. If `dmg < 1` → `result = 1` (no damage / absorbed), damage 0.
   Else → `result = 2` (normal) unless already crit (4); copy message ids from `def[+8]/+10`.

## Result block (globals at `DAT_1140`)

| offset | meaning |
|--------|---------|
| `+0x04` | result code: 1=no damage, 2=hit, 3=parried/dodged, 4=critical |
| `+0x06` / `+0x08` | damage dealt |
| `+0x0c` | attack message id (`def+8`) |
| `+0x0e` | attack message id 2 (`def+10`) |
| `+0x10` | attacker `[6]` passthrough |

## ✅ RESOLVED: the to-hit formula (recovered from disassembly)

Recovered by reading the raw disassembly of `_CALCULATE_ATTACK` at `1040:1ea2..1f4c`
and tracing the register operands the decompiler dropped. The `f_lxmul`/`f_ldiv`
16-bit helpers use the **register** ABI (`DX:AX op CX:BX → DX:AX`); the `14`/`10`
immediates the decompiler mis-attributed to `f_lxmul` are actually the two divisors
consumed by the following `f_ldiv` calls. Earlier hypotheses (a linear `1.4×`
weighting) were **wrong** — the real relationship is **quadratic**:

```
accuracy = attacker.word[0] + typeAccMod       # typeAccMod from attack type (below)
defense  = defender.word[1] + defender.word[2]

den = accuracy² / 14 / 10                       # = accuracy² / 140
if den == 0:                                    # accuracy ≲ 11
    threshold = 5                               # ...then clamps up to 10
else:
    threshold = 100 - (defense² / den)          # = 100 - 140 * defense² / accuracy²
threshold = clamp(threshold, 10, 99)            # OUTSIDE the if/else: every arm
hit if roll < threshold                         # roll = genrdn(1,100) in [1,99], STRICT
```

Disassembly evidence (segment `0x1040`):
- `1ea2-1ea8`: pushes divisors `14` then `10` (the `/14/10`).
- `1eaa-1ed9`: loads `DX:AX = CX:BX = accuracy` where `accuracy = att.word[0] + [BP-2]`
  (`[BP-2]` = `typeAccMod`, set by the attack-type switch).
- `1eda f_lxmul` → `accuracy²`; `1ee1`/`1ee8 f_ldiv` → `/14`, `/10` → `den` (`lVar7`).
- `1f01-1f38`: loads `DX:AX = CX:BX = def.word[1] + def.word[2]` (the defense sum).
- `1f39 f_lxmul` → `defense²`; `1f40 f_ldiv` → `defense² / den`.
- `1f45-1f4c`: `100 - that` → threshold.

**Field roles clarified** by this (the fighter struct is symmetric; role depends on
attack/defend):
- `word[0]` = **offensive accuracy** (used when attacking).
- `word[1] + word[2]` = **defensive evasion** (used when defending) — *this* is the
  real dodge term in normal to-hit, **not** `word[10]`.
- `word[10]` = the **helpless gate** (`< 0`, e.g. defender HP<1 ⇒ near-auto-hit branch)
  **and** the **parry** rating; it is not the primary evasion.

Sanity check: acc 200 vs defense 100 → `100 - 140·10000/40000 = 65%`; defense 150 →
21%; defense 50 → 91%. Evenly matched (acc = defense) → `100-140 = -40` → clamped 10%
(you must out-accuracy the target's evasion to hit reliably — a known MajorMUD trait).

`typeAccMod` (`[BP-2]`, from the attack-type switch): type 4 = **−15**, type 6 = **−25**,
type 7 = **−75**, all others = **0** (special attacks are harder to land).

> **MEASURED 2026-07-26** (`re/oracle/oracle_dodge_parry_control.raw`,
> `charm.md` §8.4). Two parts of this formula now have live confirmation.
>
> **The defender's `word[1]` is in WHOLE UNITS on the monster side** — the
> template `ac` column reaches it RAW, where the player's item column reaches
> it ÷10 (24866). The two look inconsistent and are not: shipped monster `ac`
> runs 0..9999 (mean 101) against a geared player's ~12, and reading the
> monster column in tenths as well is excluded by capture. A level-2 character
> at accuracy 23 swinging at grey spiders (AC 20, no Dodge, so no parry
> channel) connected **2/21 = 0.0952**, CI [0.0117, 0.3038]: the raw column
> predicts 400/3 = 133 over 100 → the clamp floor (0.0909 under the
> corrected strict-roll model), and a tenths reading predicts 0.99.
>
> **The [10, 99] clamp's floor is real**, and that run is the first thing to
> exercise it — the quadratic goes sharply negative once defense approaches
> accuracy, and a level-2 character against a common low-level monster is
> already deep in that region. Note every divide TRUNCATES (`den` first, then
> `defense²/den`), which is what puts accuracy 13..25 all on the same floor
> against AC 20.
>
> Still ORACLE-VERIFY: the ACCURACY side (`word[0]`, the
> `move_player_to_fighter` derivation below). The 2026-07-28 re-fit under
> the corrected model SHARPENED the conflict: on the acc-mid block's own
> 37 swings, to-hit excludes accuracy 23 (wants ≥24) while the parry
> channel excludes ≥24 (wants ≤23) — at least one formula shape is off,
> not only the constant. See `charm.md` §8.4 tail; the slice-8 expedition
> (parry-cliff pair + fixed-defense ratio pair) discriminates the two.

---

## Recovered in full (WG3-NT decompile pass, 2026-07-16)

`move_player_to_fighter` (0x2a19d) / `compute_energy_used` (0x2a0c8) /
`move_monster_to_fighter` (0x2b43e) read end-to-end:

- **EU** = `speed*1000 / ((class[+0x48]*level + 45) * (Agl+150) * 1500/9000) + weaponBonus(item+0x320)`;
  divide-by-zero guard returns 50. `class+0x48` = the `combat` column. Unarmed
  speeds: fists **1200** (0x4b0), fists-of-fury 1150, kicks 1400, jumpkick 1900
  (each has a higher variant gated on player flag `+0x7c8 & 2`, untraced).
  Weapon speed = `item+0x3de` (×3/2 when the flag is set); heavy weapons
  (Str < item+0x3a0) scale EU by `((need-Str)*3+200)/200`.
- **Accuracy (normal)** = `(Str-50)/3 + 2*((combat-1)*isqrt(level) + 2*combat
  + level/2 + skill/2 - 2) + (Agl-50)/6 + dyn(+0x70a)`, where `skill` =
  weapon+worn `+0x39a` ratings (floor 1) `+ 15 - enc/10` when encumbrance < 33.
  Stance (+0x6ac front/back ±15/±10), darkness −10 modifiers follow.
- **Backstab accuracy** = `(Agl + stealth)/2 (±hidden mods) + Agl/2`; defender
  backstab-compare `[1]` = `(weaponRating/10 + perception)/2`.
- **Unarmed damage** 1–4; MA styles: min `(min(level,20)*abil)>>3 + 2`, max
  per-style (`(level+3)*abil>>2+6` fists / `level*abil/6+7` kicks / `+8`
  jumpkick). **Str bonuses (all attacks):** max += `(Str-50)/10`; min +=
  `2*(Str-100)/10` when positive; min clamped ≤ max, both ≥ 0.
- **Crit rating** `[0x91]` = `byte(+0x710 dodge base) + word(+0x7b4)`, min 1.
- **Parry** `[8]` = `dodgeAbil(0x22) + (Chm-50)/5 + level/5 + (Agl-50)/3
  + (10 - enc/10 when enc < 33)`; −1 when HP < 1.
- **Player defense** `[1]` = `(Σ item+0x342)/10 + dyn(+0x70c)`, `[2]` = weapon
  to-hit ability (naked: both 0). Armor `[3]` = Σ worn `+0x39c` + dyn(+0x7b6),
  ×(+0x7b8+100)/100.

  > **⚠ THE TWO ITEM ARMOUR COLUMNS ARE DIFFERENT STATS, AND BOTH ARE
  > CALLED "ARMOUR".** `+0x342` (DB column `ac`) is the TO-HIT stat and
  > reaches `[1]` **÷10**; `+0x39c` (DB column `dr`) is the DAMAGE SOAK and
  > reaches `[3]` **raw**, the ÷10 happening later inside
  > `calculate_attack`. Both ship pre-multiplied by 10 (rigid leather tunic
  > ac 130 / dr 13), and 319 of the 565 AC-bearing shipped items carry no
  > DR at all — so reading one for the other is not a scaling error, it
  > invents resistance for most of the gear in the game.
  >
  > The `st` line prints BOTH as `Armour Class: A/B` = `Σ+0x342/10` and
  > `Σ+0x39c/10` (`get_armour_rating` 16956 → 31553-31556). MEASURED
  > 2026-07-26: Σac 125 / Σdr 8 → `12/0`, with giant bats (2..5) biting for
  > the full undiminished band.
  >
  > **As built:** the port had these transposed from M4 until 2026-07-26
  > (`build_player_defender`); see `re/docs/charm.md` §8.3 and
  > `crates/mud-server/tests/armour_real_content.rs`. The evasion
  > accumulator is seeded by the WIELDED weapon (24683) before the worn
  > loop adds to it (24797) — the soak word gets no such seed. Not ported:
  > the `×(+0x7b8+100)/100` DR percent, whose writing ability is not
  > readable from the decompile (see the call site in `game.rs`).
- **Monster fighter**: `[0]` = per-form accuracy (knmsr+0x12e) + Accuracy
  abilities; `[1]` evasion = instance AC (+0x10c) + AC ability; `[3]` armor =
  **DR(+0x10a) × 10** + DR ability; `[4]` kill exp = worth × multiplier;
  `[5]`/`[6]`/`[7]` per-form EU/min/max; `[8]` = Dodge ability;
  crit `[0x91]` = 0 (players-only crits confirmed again).

## Unarmed attack modes (slice M6+ arena expedition, 2026-07-19)

`move_player_to_fighter` (24520-24660) initializes damage `[6]=1, [7]=4`
(plain fists) and then reshapes per the global attack mode `DAT_004877e4`
(set by the attack-command family, stored per-fighter in the autocombat
table `+8`, restored each round by `restart_autocombat` 46442):

| mode | verb / selection | ability (V) | min `[6]` | max `[7]` | EU speed |
|------|------------------|-------------|-----------|-----------|----------|
| 1 "fists of fury" | `punch`, or **bare `attack` unarmed when the user has Punch** (cmd_attack 49712-49720; hidden/sneak diverts to mode 4) | Punch 0x1d | `L*V/8 + 2` | `(L+3)*V/4 + 6` | 0x47e = 1150 (0x6d6 when +0x7c8 bit 2) |
| 2 "lightning feet" | `kick` (needs Kick) | Kick 0x1e | `L*V/8 + 2` | `L*V/6 + 7` | 0x578 (2000 flagged) |
| 3 "flying feet" | `jumpkick` (needs JumpKick) | JumpKick 0x23 | `L*V/8 + 2` | `L*V/6 + 8` | 0x76c (0xa5a flagged) |
| else, unarmed | plain fists | — | 1 | 4 | 0x4b0 = 1200 |

`L` = level capped at 20 (the >=0x14 branch substitutes 20, and mode 1's
max uses 0x17 = 20+3). Post-block add-ons (24890-24916): mode 1 adds
PunchACY (0x59) to accuracy and PunchDmg (0x5c) to both damage bounds;
modes 2/3 use the Kick/JumpK pairs (0x5a/0x5d, 0x5b/0x5e). The usual
Strength adjustments then apply. The Mystic class record carries
Punch/Kick/JumpKick at value 1 (items/spells raise V through the
ability fold). ORACLE pin (oracle_m6_arena_fight.raw): Nekojin Mystic
L1 V1 Str40 — raw 2..6, observed 1..5 through the giant rat's DR 1,
exactly {1,1,1,4,5}. Mode 4 = backstab (its own block); modes 6/7 =
surprise/ranged variants, unextracted.
