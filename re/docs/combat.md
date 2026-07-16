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
| `[3]`     | 6    | **armor / AC** (`def+6`, dmg −= armor/10) | Σ worn-item armor `item[+0x39b]` |
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

1. Roll `attackRoll = genrnd(1,100)`.
2. Determine **hit threshold** `local_a` (with sign word `local_8`):
   - If `def[+0x14] < 0` and `genrnd(0,100) > def[+0x14]+100`: threshold = 99 (near-auto-hit path).
   - Else if type == 4 (backstab): threshold = `att[0] - def[+2]` (signed 32-bit).
   - Else (normal): **recovered formula** (see below):
     `threshold = 100 - 140 * defense² / accuracy²`.
3. **Clamp** threshold to `[10, 99]`.
4. **Hit if** `attackRoll <= threshold`.

## On a hit

1. **Critical:** `genrnd(0,100) < att[0x117]` and type ∉ {6,7} → critical, `result = 4`.
   Damage scaling **[16-bit / WG3-NT]**: 16-bit `min *= 2, max *= 4`; **WG3-NT**
   `min = 2·max_old, max = 4·max_old` (higher floor). Crit rating is diminishing-returns
   capped in both: if `rating > 40`, `rating = 40 + (rating−40)/3`.
2. Ensure `max >= min`.
3. **Damage:** `dmg = genrnd(0, max-min+1) + min - def[+6]/10`  (rand in `[min,max]` minus armor/10).
4. Type multipliers: type 6 → `dmg *= 3`; type 7 → `dmg *= 5`.
5. **Parry/riposte:** parry chance `p = (def[+0x14] * 10) / (att[0] / 8)`, clamped `[0,95]`.
   Backstab (type 4) reduces it: **[16-bit / WG3-NT]** 16-bit halves (`p/2`); **WG3-NT** `p/5`.
   If `def[+0x14] > 0` and `genrnd(0,100) < p` → `result = 3` (parried), damage 0, **return**.
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
    threshold = 5
else:
    threshold = 100 - (defense² / den)          # = 100 - 140 * defense² / accuracy²
threshold = clamp(threshold, 10, 99)
hit if roll(1..100) <= threshold
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
