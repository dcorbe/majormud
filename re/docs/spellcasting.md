# MajorMUD spell-cast resolution & duration/maintenance (WG3-NT)

Behavioral spec derived from the 32-bit WG3-NT decompile
(`wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Cross-references
`abilities.md` (the 188-entry ability enum), `records.md` (struct offsets),
and `combat.md` (damage path). Ability ids are cited by decimal/hex; struct
offsets are byte offsets into the player, spell, or monster record.

Every spell encodes its effect as a parallel `(ability-id, value)` table of 10
entries:

- spell `+0xd8 + i*2` — ability **id** array (10 slots)
- spell `+0xa8 + i*2` — ability **value** array (10 slots)

`get_spell_ability_value(id, spellId, spellPtr)` and `spell_has_ability(...)`
walk these two arrays (up to 10 entries) and return the matching value / a
boolean. `get_spell_data(id)` resolves a spell id to its record via a hashed
slot cache (`mem_get_spell`, `load_spell_into_buffer`); every function below
takes either a resolved pointer or an id it resolves through this.

---

## 1. Spell record fields used by the cast path

Confirmed from absolute offsets in `cast_item_target` and the word-index math
in `cast_no_target`/`add_cast_spell_to_user`:

| offset | field | notes |
|--------|-------|-------|
| `+0x00` | spell id | hash key in `get_spell_data` |
| `+0x02` | name string | `prf(... , spell+2)` |
| `+0xa2` | **level cap** for scaling | caster level clamped to this before scaling value/duration |
| `+0xa8..` | ability **value** array (10) | see effect table |
| `+0xbc` | **round-action cost** | deducted from player `+0xba` round pool |
| `+0xbe` | **required caster power/level** | vs player `+0x94`; fail = "too powerful for you" |
| `+0xc0` | min base value | |
| `+0xc2` | max base value | |
| `+0xc4` | **target mode** | `<3` = offensive/combat-scoped; `>=3` = benign/self |
| `+0xc6` | **save class** (`typeofresists`) | 0 = no save; 1 = save only if the target has AntiMagic (51/0x33); 2 = target always gets a save (see §3 saving throw). Shipped data: 1121/167/91 spells |
| `+0xc8` | **base success chance %** | `>=200` ⇒ auto-succeed (skip roll) |
| `+0xca` | duration per-level multiplier (word) | duration = mult × level |
| `+0xcc` | **match/delivery type** | drives targeting (see `get_spell_match_type`) |
| `+0xce` | **duration** (base ticks) | `0` ⇒ instant spell; `!=0` ⇒ duration spell |
| `+0xd0` | **damage element** | 0 cold,1 fire,2 stone,3 lightning,5 water,6 poison |
| `+0xd6` | **class-gate group** | must equal class `+0x40` |
| `+0xd8..` | ability **id** array (10) | see effect table |
| `+0xf0` | **mana cost** | deducted from player `+0x602` |
| `+0xf2/+0xf3` | max-bound per-level numerator/denominator (bytes, `maxincrease/lvlsmaxincr`) | see §7 label note |
| `+0xf4` | **required level within class** | vs class `+0x42` |
| `+0xf6/+0xf7` | min-bound per-level numerator/denominator (bytes, `minincrease/lvlsminincr`) | see §7 label note |
| `+0xf8/+0xf9` | duration per-level numerator/denominator (bytes) | |

`get_spell_match_type(+0xcc)` cases: `0/1/2` single-scope (returns `0x82`, or
`2` if target-mode `+0xc4 > 2`); `3,5,9,10,0xb,0xc,0xd` area/room (return `0`);
`4` `0x801`, `6` `0xf837`, `7` `0x14`, `8` `0x803`. These codes are consumed by
the command parser to pick a cast entry point (`cast_no_target` vs
`cast_user_target` vs `cast_monster_target` vs `cast_item_target`) and required
target syntax.

**Player active-spell slots** (10 slots each), the maintained duration-spell state:

| offset | meaning |
|--------|---------|
| `+0x40 + i*2` | active spell id (`0` = empty slot) |
| `+0x54 + i*2` | stored value/potency (fed to upkeep & `update_dynamic_stats`) |
| `+0x68 + i*2` | remaining duration in ticks |

**Monster active-spell slots** (5 slots): id `+0x14a+i*2`, value `+0x154+i*2`,
duration `+0x15e+i*2`, plus a "dirty/recompute" byte at `+0x140`.

---

## 2. Eligibility (`user_can_use_spell`, inline checks in cast entry points)

`user_can_use_spell(player, spellPtr)` is the standalone predicate (used e.g.
when an item with **LearnSp** (42/0x2a) tries to teach a spell — line 17756).
The interactive cast entry points repeat the same gates inline. Gates, in order:

1. **Class group** — resolve `get_class_data(player+0x92)`. If spell `+0xd6 != 0`
   then class `+0x40` must equal spell `+0xd6` **and** class `+0x42`
   (max spell level the class can cast) must be `>=` spell `+0xf4`
   (required level within class). Otherwise ineligible.
2. **Spell power** — player `+0x94` (caster's spell-casting level/power) must be
   `>=` spell `+0xbe`, else "This spell is too powerful for you."
3. **Alignment** — `get_legal_level(player+0x542)` yields an alignment tier; the
   spell is rejected by cross-referencing alignment-flag abilities on the spell:
   **Good** (97/0x61), **Evil** (98/0x62), **NotGood** (110/0x6e),
   **NotEvil** (111/0x6f), **Neutral** (112/0x70). Low tiers reject Good/Evil-only
   spells; mid tiers reject Good/NotEvil/Neutral; high (evil) tiers require an
   Evil/NotGood/Neutral affinity. (The tier→flag mapping is an alignment lattice;
   the exact banding is in `user_can_use_spell` lines 17811-17846.)

Additional inline gates present in `cast_no_target`/`cast_item_target` before a
cast resolves:

- **MageBind** (79/0x4f) on the caster ⇒ "Your spell fails!" (blocked).
- For the **kai/mystic** caster class (class `+0x40 == 5`), **KaiBind**
  (159/0x9f) ⇒ "Your power fails!".
- **Fear** flag (player `+0x7c8 & 0x80`) or HP `< 1` ⇒ cannot cast.
- `check_confusion` may divert the action (Confusion, 71/0x47).
- Resource check (all three or fail): round pool `+0xba >= +0xbc`,
  mana `+0x602 >= +0xf0`, power `+0x94 >= +0xbe`.

Kai/mystic casters use the same fields but messages say "invoke a power" /
"kai" instead of "cast a spell" / "mana"; mana cost still reads spell `+0xf0`
and is deducted from `+0x602`.

---

## 3. Cast flow (`cast_no_target`, and the parallel `cast_*_target`)

`cast_no_target(spellPtr, terminal, mode)` is the reference implementation.
`cast_user_target`, `cast_monster_target`, `cast_item_target` are structurally
parallel but resolve an explicit target first (and `cast_item_target` requires
match type `+0xcc ∈ {6,7}`). `mode` (`param_3`): `0` = normal player cast,
`2` = chained/forced cast (from EndCast; skips confusion/fear/round checks).

Order of operations:

1. **Pre-checks** — confusion, HP/fear, MageBind/KaiBind (see §2). Clear
   per-cast "told" flags.
2. **Room & protection** — resolve room; reject in no-cast/protected rooms
   ("You are overcome with a feeling of..."), partially refunding the round pool.
3. **Target counting** — `count_valid_targets` fills counts and a target-flags
   byte; `0` targets ⇒ "Your spell has no effect in this...". For offensive
   spells (target mode `+0xc4 < 3`) this also drives **evil-warning** emission
   (`add_evil_warnings_to_room`) and auto-combat engagement.
4. **Resource check / deduction gate** — if any of round/mana/power is short,
   abort with the specific message (see §2). Under autocombat the round pool is
   zeroed and combat is (dis)engaged instead.
5. **Success roll** — `iVar11 = genrdn(0,100)`. If spell `+0xc8 < 200`, compute
   `chance = min(player.SC(+0x604) + spell.baseChance(+0xc8), 98)`; the cast
   **succeeds** when `roll < chance`, else **fails**. Base chance `>=200`
   auto-succeeds. So success probability ≈ `(playerSC + spellBase)` percent,
   capped at 98%. (Mystics additionally consume a per-round "already invoked"
   flag at `+0x700 & 4`.)
6. **On failure** — "You attempt to cast %s but fail"; broadcast to room;
   deduct the **full round cost** (`+0xba -= +0xbc`) but only **half** the mana
   (`+0x602 -= floor(manaCost/2)`). No effects applied.
7. **On success** — deduct **full round cost and full mana**. Then compute the
   effect magnitude and apply the ability table (§4).

**Target saving throw (targeted casts, success only).** After a successful
success roll and before any effect, `cast_user_target` (gate lines
41712-41733; resist message/cost block 42984-43009) and `cast_monster_target`
(structurally identical) give the **resolved target** a resist check driven by
spell `+0xc6`:

- The target's **SpellImmu** (139/0x8b) is read (line 41556) and the predicate
  `FUN_0043e3db` (72500) forces the resist outcome — working assumption:
  SpellImmu ⇒ auto-resist (flagged, needs one confirmation pass).
- Otherwise a save is rolled when `+0xc6 == 2` (always) or `+0xc6 == 1` **and**
  the target has **AntiMagic** (51/0x33): resisted when
  `genrdn(1,100) <= min(targetSC(+0xc2) / 2, 98)`.

A resisted cast deducts the full round cost but only **half mana** (like a
failed roll), applies no effects, and prints the "You resisted %s's %s" /
"%s resisted %s's %s" family (distinct from the plain-failure strings, which
for targeted casts read "You attempt to cast %s at %s, but…" /
"%s attempted to cast %s on you, b…"). Most spells (1121/1379) carry
`+0xc6 == 0` — no save. **This corrects §6, which previously claimed the
player-initiated cast path has no target resist check.**

**Magnitude roll (success only).** Effective level `L = min(player+0x94,
spell+0xa2)`. Per-level scaling: `minScale = spell+0xf2 * L / spell+0xf3`,
`maxScale = spell+0xf6 * L / spell+0xf7` (guarded against zero denominators).
Bounds `lo = min(spell+0xc0 + maxScale, spell+0xc2 + minScale)`,
`hi = max(...)`; rolled value `V = genrdn(0, hi-lo+1) + lo`. For area match types
(`3,5,9,10`) `V` is **divided by the number of targets** (area spells split their
magnitude); for single/room types it is not. A debug line prints Min/Max when the
caster has the spell-debug flag (`DAT_00479100 + terminal*0x14`).

**RemovesSpell / KillSpell pre-pass.** Before applying, the loop scans for
abilities **RemovesSpell** (122/0x7a) and **KillSpell** (153/0x99): the named
target spell (value slot) is located in the target's active-spell slots and
terminated via `perform_spell_termination_player_upkeep` — RemovesSpell runs the
spell's EndCast chain, KillSpell suppresses it (the `param_5` flag distinguishes
`0x7a` vs `0x99`). A successful pre-pass dispel **terminates the cast**
(`cast_no_target` 39472-39494, match types 1/2/6): the DLL prints
`display_spell_success`, clears the slot, runs the termination path, recomputes
secondary stats, and returns — the early return precedes the apply loop
(39573), so the caster's own spell is neither applied (instant effects
included) nor entered into a slot. The pre-pass is not duration-gated; it runs
for every cast that reaches the apply stage.

---

## 4. Effect application

The apply loop iterates the 10 ability slots. For each ability id
`spell+0xd8+i*2` it dispatches on **(ability id) × (match type `+0xcc`)**. The two
axes:

- **Match type `+0xcc`** selects *who*: `1/2/6` act on the single resolved
  target/caster; `3/5/9/10/0xb/0xc/0xd` iterate every valid player in the room
  (`is_valid_target`) and, for `3/5/9/0xb/0xc`, every monster
  (`is_valid_monster_target`) via the room monster list at `room+0x400+k*4`.
- **Instant vs duration** is decided by spell `+0xce` (duration). When
  `+0xce != 0` the effect is *not* applied directly; instead
  `add_cast_spell_to_user` (single) or `add_duration_spell_to_room` (area) enters
  it into the target's active-spell slots. When `+0xce == 0` the effect is
  applied immediately to live fields.

Selected instant handlers (ability id → field), player target:

| ability | id | instant effect |
|---------|----|----------------|
| Damage | 1 | HP `+0xb0 -= V`; routes through the combat kill path — `check_kill_user` / `distribute_experience` (matches `combat.md`). Resisted by element (see below). MR is ignored. |
| Damage(-MR) | 17/0x11 | Like Damage, but `V` is scaled by the target's **MR** first — the SAME stat the saving throw reads (monster: M.R.(36) modifiers + template `mr` word, floored at 1, `cast_monster_target` 43387-43392; player: `user+0xc2`). Both paths first boost `V` by the caster's **AlterSpDmg** (165/0xa5) percent (43940-43941; plain Damage gets the identical boost via the 39025-39030 helper). Without **AntiMagic** (51) on the target: `red = clamp((MR-50)/2, 0, 50)` (43946-43954); if `red == 0` the damage is instead **amplified**: `V' = V + V*(50-MR)/100` (43974-43975) — a floor-MR target takes +49%, MR 50 is the unchanged pivot; else `V' = V - V*red/100` (43982). With AntiMagic: `red = clamp(MR/2, 0, 75)` (43957-43968), no amplification (`red == 0` ⇒ `V' = V`, 43978). All divisions truncate toward zero. Monster-target body 43937-43993; player-target twin `cast_no_target` 40137-40198. **This is the damage path the shipped attack spells predominantly use**: of the 336 instant (`duration=0`) offensive (`spelltype<3`) spells, 171 carry 17 (magic missile — spell 1 — included, value 0 = rolled magnitude; no shipped 17 slot carries a fixed value) vs 98 carrying plain Damage(1). |
| Enslave | 6 | `silly_spell` placeholder in WG3-NT (charm on players not implemented here) |
| Drain | 8 | target HP `-= V`, caster HP `+= V` (capped at caster max `+0xae`); kill-checked |
| EnergyLevel | 11/0xb | round pool `+0xba += V` (capped at max `+0xb8`) |
| Summon | 12/0xc | `generate_monster` into the room, tagged owned by caster |
| Alterhunger | 15/0xf | `+0xce += V` |
| Alter thirst | 16/0x10 | `+0xd0 += V` |

Damage/negative spells scale by the target's **resistance** to the spell's
element (`get_spell_random_modifier`): it reads spell `+0xd0` and returns the
target's resistance ability value — **Rcol** (3), **Rfir** (5), **ResistStone**
(65/0x41), **Rlit** (66/0x42), **ResistWater** (147/0x93), **ImmuPoison**
(21/0x15). Applied as `V' = ((100 - resist) * V) / 100`. (The function name says
"random" but it is the *resistance* modifier.) Two boundary behaviors confirmed
from the function body (38634-38664): the switch has **no case for element 4**
— element-4 damage ("pure magic", e.g. magic missile) is unresistable and lands
at full value — and the whole modifier returns 0 unless the spell's target mode
`+0xc4 < 3`, so benign-mode spells never apply elemental resistance.

**Duration effects.** `add_cast_spell_to_user(terminal, spellId, duration,
value, spellPtr, casterTerminal, casterPtr, applyFlag)`:

1. Scales **duration** by caster level: `L = min(caster+0x94, spell+0xa2)`;
   if `spell+0xf9 != 0` add `(L / spell+0xf9) * spell+0xf8`; the max is
   `spell+0xca * L`; if base `< max`, `duration = genrdn(base, max+1)`. A final
   percentage from **AlterSpLength** (166/0xa6) scales it
   (`duration = (AlterSpLength + 100) * duration / 100`).
2. If the spell is **already active** in a slot, refresh value `+0x54` and
   duration `+0x68` (or, when `applyFlag==0`, report "already have" and return).
3. Else write into the first empty slot (`+0x40`, `+0x54`, `+0x68`).
4. `display_spell_success` unless the caster is in a "quiet" state (`+0x5f6`).

The stored `(id, value)` in the active slot is what makes the effect *persist*:
`update_dynamic_stats` recomputes all derived combat fields **from scratch** each
call, re-applying every active spell's ability list through
`update_dynamic_with_ability`, plus the player's 30-entry permanent/equipment
ability table at `+0x73a`(id)/`+0x776`(value). When a spell leaves the slot list,
its contribution simply vanishes on the next recompute — no explicit undo needed
for flag/modifier effects.

**`update_dynamic_with_ability(target, abilityId, value)`** — the dynamic-effect
dispatch. Accumulating modifiers (`+= value`) and flag bits it sets:

| ability | id | dynamic field / flag |
|---------|----|----------------------|
| AC | 2 | `+0x70c += v` |
| Max Damage | 4 | `+0x70e += v` |
| DR | 7 | `+0x7b6 += v` |
| AC(Blur) | 10/0xa | `+0x7bc += v` |
| RoomIllu | 14/0xe | `+0x6bc += v` |
| Accuracy | 22/0x16 | `+0x70a = max(v, cur)` (set-if-greater) |
| SeeHidden | 57/0x39 | `+0x6f4 |= 0x2000` |
| Crits | 58/0x3a | `+0x7b4 += v` |
| Fear | 60/0x3c | `+0x7c8 |= 0x80` |
| Quickness | 67/0x43 | `+0x7c8 |= 0x04` |
| Slowness | 68/0x44 | `+0x7c8 |= 0x02` |
| Confusion | 71/0x47 | `+0x6f4 |= 0x80` |
| HoldPerson | 74/0x4a | `+0x7c8 |= 0x20` |
| Paralyze | 75/0x4b | `+0x6f4 |= 0x800` |
| Mute | 76/0x4c | `+0x6f4 |= 0x100` |
| Speed | 87/0x57 | `+0x7ba` weighted-average toward `v` (first write sets it) |
| AlterDRpercent | 99/0x63 | `+0x7b8 += v` |
| DefenseModifier | 104/0x68 | `+0x7be += v` |
| Accuracy(2)/(3) | 105/106 | `+0x70a = max(v, cur)` |
| BlindUser | 107/0x6b | `+0x6f4 |= 0x1000` |
| HPRegen | 123/0x7b | `+0x7d8 += v` |
| ManaRgn | 145/0x91 | `+0x7da += v` |

> **Correction to `abilities.md`:** id 87 (Speed) writes `+0x7ba` (not `+0x7b8`);
> `+0x7b8` is written by id 99 (AlterDRpercent). Everything else in the
> abilities.md "dynamic effect" column is confirmed. Flag word `+0x6f4` also
> carries bit `0x02` = being-dragged (`user_being_dragged`); flag word `+0x7c8`
> bit `0x10` marks a player as an in-room live target during target sweeps.

`negate_dynamic_with_ability` is the inverse (zeros the field / clears the bit).
It exists for targeted dispel but is largely vestigial now that
`update_dynamic_stats` rebuilds from scratch; `update_dynamic_stats` still calls
it indirectly and treats ability **NegateAbility** (124/0x7c) specially
(suppresses re-application of that slot).

`add_duration_spell_to_room(spellPtr, value, terminal)` applies a duration spell
to **every** valid player (`add_cast_spell_to_user`) and, for area match types
`3/5/9/0xb/0xc`, every valid monster (`add_cast_spell_to_monster`) in the room,
each with its own resistance modifier `((100 - resist) * value)/100` keyed on
spell `+0xd0`.

---

## 5. Duration & maintenance

Duration is measured in **maintenance ticks** (one decrement per world upkeep
cycle). The player tick loop (in the "medium/routine update" pass) does, for each
non-empty slot `i` of the 10 (`+0x40+i*2 != 0`):

1. **Decrement** remaining duration `+0x68 += -1`.
2. **`perform_routine_spell_player_upkeep(terminal, player, spellPtr, slotValue)`**
   — apply the spell's recurring per-tick effect (below).
3. **If duration reached 0** — clear the slot (`+0x40=0`, `+0x54=0`) and call
   **`perform_spell_termination_player_upkeep(...)`** (reverse + chain, below).
4. `check_kill_user` (recurring damage can kill mid-loop).

`perform_routine_spell_player_upkeep` walks the spell ability table; value =
slot value unless the spell overrides per-ability at `+0xa8`. Per-tick handlers:

| ability | id | per-tick effect |
|---------|----|-----------------|
| Damage | 1 | HP `+0xb0 -= v`, prompt, death-check |
| Drain | 8 | HP `-= v`, death-check |
| EnergyLevel | 11/0xb | round pool `+0xba += v` (cap max `+0xb8`) |
| Alterhunger | 15/0xf | `+0xce += v` |
| Alter thirst | 16/0x10 | `+0xd0 += v` |
| Heal | 18/0x12 | HP `+= v` (cap max `+0xae`), prompt |
| Cure Poison | 20/0x14 | poison `+0xbe -= v` (floor 0) |
| Fear | 60/0x3c | `genrdn(0,100) < v` ⇒ flee a random exit (`move_user`) |
| HealMana | 150/0x96 | mana `+0x602 += v` (floor 0, cap max `+0x600`), prompt |

`perform_spell_termination_player_upkeep(terminal, player, spellPtr, value,
chainFlag)` runs once when a spell expires or is removed:

1. Print the spell's **end message** via **DescMsg** (115/0x73) → `get_message_data`.
2. Reverse the "hard" stat writes that were applied directly at cast (these are
   *not* recomputed by `update_dynamic_stats`, so they must be undone explicitly):
   - Poison (19/0x13): `+0xbe -= v`
   - Intel/Wisdom/Strength/Health/Agility/Charm (44-49 / 0x2c-0x31): stat `-= v`
     (fields `+0xa2..+0xac`)
   - Alter HP (88/0x58): both max HP `+0xae` and cur HP `+0xb0` `-= v`
   - GiveTempSpell (160/0xa0): `purge_spell_from_spellbook`
3. Read **EndCast** (151/0x97) target spell id and **CastOnEnd%** (164/0xa4)
   chance (default 100).
4. **Chaining:** if `chainFlag` set, EndCast id present, and `genrdn(0,100) <
   CastOnEnd%`, resolve that spell and `cast_no_target(..., mode 2)` — a forced
   follow-up cast. This is how multi-stage spells chain.
5. `calculate_secondary_stats` to fold everything back in.

The same termination path fires on death/reroll (line 10408-10419) and on
targeted removal (§3 RemovesSpell/KillSpell), clearing the slot first then
calling termination with the stored value.

**Maintenance-flag abilities are a *separate* item-cleanup sweep**, not part of
spell duration. In the room/item upkeep pass (around line 4690-4740):

- **Del@Maint** (119/0x77) — the ground item is deleted at cleanup (poofed),
  gated together with a hidden-timer field (`item+0x322`) and a zero refcount.
- **Remove@Maint** (149/0x95) — removes/poofs the item at cleanup (checked in a
  parallel inventory sweep at 4407/4440).
- **Visible@Maint** (154/0x9a) — forces the item to remain visible/un-merged at
  cleanup (suppresses the consolidate-and-hide branch).

These govern transient items — including spell-conjured or summoned drops — so
they persist or vanish each cycle. They act on items, not on the active-spell
slot arrays, and are documented here only because the enum groups them with the
`@Maint` family.

---

## 6. Monster casting

`monster_cast(monster, template, attackIdx, targetTerminal, targetPlayer,
spellId)` and its area sibling `monster_cast_area`. Differences from player
casting:

1. **Spell source** — the spell is selected from the monster template's spell-
   attack list at `template+0x12e+attackIdx*2`; when `attackIdx == -1` a spell id
   is passed directly (scripted cast).
2. **No mana / no round pool** — monsters do not pay `+0x602`/`+0xba`; there is
   no resource-shortage abort and no half-mana-on-fail.
3. **Success chance** — a flat per-spell percentage from
   `template+0x13e+attackIdx*2` (or 100 for a forced/`-1` cast), compared to
   `genrdn(0,100)`. There is no player-SC term.
4. **Saving throw** — the *target* gets a resist check (cf. the player-cast
   saving throw in §3, corrected — both paths save): **SpellImmu** (139/0x8b)
   and level (`spell+0xbe` vs the save DC at `template+0x190+attackIdx*2`),
   plus **AntiMagic** (51/0x33) and the target's `+0xc2` (SC/resist stat)
   halved as a roll. Success prints "You resisted %s's cast of %s" and
   halves/negates the effect. Whether this path also honors the spell's
   `+0xc6` save class is unverified.
5. **Effect entry** — `monster_add_cast_spell_to_user` writes the **same** player
   active-spell slots (`+0x40/+0x54/+0x68`) but with a simpler policy: refresh
   only if the new value **exceeds** the current slot value; no caster-level
   duration scaling, no AlterSpLength (duration passed as a fixed argument).
   `monster_add_duration_spell_to_room` / `monster_cast_area` are the room
   variants. `add_cast_spell_to_monster` (used when a spell targets a monster)
   writes the 5 monster slots and sets the dirty byte `+0x140`.
6. **Monster upkeep** — `medium_update_monster` ticks the 5 monster slots
   (decrement `+0x15e`, `perform_routine_spell_monster_upkeep`, terminate at 0).
   `perform_routine_spell_monster_upkeep` handles a reduced set: Damage (1) HP
   `+0x18 -= v`, EnergyLevel (11) `+0x16 += v` (cap `+0x114`), Heal (18) `+0x18
   += v`, Cure Poison (20) `+0x14 -= v`, Fear (60) random flee (`move_monster`).
   `perform_spell_termination_monster_upkeep` reverses only Enslave (6, releases
   charm: reset name/owner bit `+0x128 & ~1`, flags `+0x140`/`+0x116`) and Poison
   (19/0x13, `+0x14 -= v`). Monsters have **no** stat-buff reversal, mana, or
   EndCast chaining.

Monster-side critical hits do not exist (see `combat.md`: monster fighter crit is
hard-zeroed), and monster spell damage still routes through
`check_kill_monster` / `distribute_experience` like player-cast damage.

---

## 7. Unresolved / flagged

- **Scaling-pair labels resolved via editor columns:** the decoded `.VIR`
  columns name `+0xf2/+0xf3` `maxincrease/lvlsmaxincr` and `+0xf6/+0xf7`
  `minincrease/lvlsminincr` — the reverse of §3's minScale/maxScale variable
  names. Under the editor names §3's bound formula reads
  `lo = min(minBase + minScale, maxBase + maxScale)` — min pairs with min,
  max with max, which is self-consistent. Editor naming is adopted; §3's
  variable names are historical. Verified against `_raw` blobs for all 1379
  spells (byte pairs at those offsets equal the columns). Note magic missile
  ships `maxincrease=1, lvlsmaxincr=0` — the zero-denominator guard makes it
  contribute 0; zero denominators are legal shipped data, not a load error.
- **Spell-reference ability values** (EndCast 151, RemovesSpell 122,
  KillSpell 153, GiveTempSpell 160): value 0 is the "none" sentinel (14
  shipped slots carry it); every nonzero reference resolves to a real spell.
- **`FUN_0043e3db`** (72500) — the auto-resist predicate in the §3 saving
  throw; assumed to be the SpellImmu evaluation, needs a confirmation pass.
- **Player `+0x94`** — the cast path uses it consistently as the caster's
  spell-casting level/power (eligibility gate *and* value/duration scaling input).
  `records.md` labels `+0x94` "gender / reroll flag (checked `<2`)". Both cannot
  be the same field; either `records.md` mis-attributed `+0x94` or the byte is
  reused. This spec follows the cast-path meaning (spell level/power). **Needs a
  second look at `roll_stats`/`_GET_PLAYER` to reconcile.**
- **`use_spell_target`** is a stub in WG3-NT (calls `get_spell_data` and returns
  `0`); it appears vestigial. The live targeted entry points are
  `cast_user_target` / `cast_monster_target` / `cast_item_target`.
- The alignment lattice in `user_can_use_spell` (tier→Good/Evil/Neutral flag
  banding) is transcribed structurally but not fully reduced to a truth table;
  `get_legal_level`'s tier definition (`player+0x542` alignment score, threshold
  `0x28` seen in `is_valid_target`) would pin it down.
- `get_spell_match_type`'s numeric return codes (`0x82`, `0x801`, `0xf837`, …)
  are targeting descriptors for the command parser; their bit meanings are not
  decoded here.
- **16-bit vs WG3-NT:** the per-level *duration* scaling
  (`+0xf8/+0xf9/+0xca`) and the AlterSpLength percentage are WG3-NT; the classic
  16-bit build is believed to use a simpler fixed-duration model, but no 16-bit
  binary was read for this spec. WG3-NT is authoritative.

## 8. Learning & listings (oracle-measured)

Measured 2026-07-17 against WCCMMUD 1.11p under MBBSEmu (`tools/oracle/`).
Character: **Vexil Arcanum, Human Mage** (disposable; BBS account
`Vexil`/test123), rolled with default 40s (CP 100). Transcripts in
`re/oracle/`:

- `oracle_spell_learning.raw` — creation, first `spells`, shop listing,
  verb hunt, unknown-spell failures. (Driver: `oracle_spell_learning.py`.)
- `oracle_spell_cast.raw` — read-vs-use control, paid buy, too-high learn,
  blur self-casts, magic-missile combat casts. (`oracle_spell_cast.py`.)
- `oracle_spell_train.raw` — exp-patched train test, gate-is-level proof,
  insufficient-mana failure. (`oracle_spell_train.py`.)

### 8.1 The verdict: scrolls are the only path — training grants nothing

- Fresh Mage L1, first command (`oracle_spell_learning.raw`):

  ```
  spells
  You have no spells.
  ```

- After training L1→L2 with two spells known and the level-2 mage spell
  *illuminate* deliberately never learned (`oracle_spell_train.raw`): the
  `spells` output before and after `train` is **byte-identical** (blur +
  magic missile only). Illuminate did not appear. **Leveling never inserts
  spells; the LearnSp(42) scroll ability is the sole acquisition path.**

### 8.2 Caster baseline (creation defaults)

Human Mage, all stats 40 (`oracle_spell_learning.raw`): HP 26, **Mana
12/12**, Spellcasting 43, Lives/CP 9/100, exp for L2 = 1400. After training
to L2 (`oracle_spell_train.raw`): HP max 29, **Mana 18**, Spellcasting 45,
CP 110. Caster prompt carries mana: `[HP=26/MA=12]:` (updates immediately
on cast; a Warrior's prompt is `[HP=n]:` only). `health`:

```
Health:    26/26    [100%]  Mana:  12/12  [100%]
```

Race/class creation menus are flat numbered lists (no CP price per class);
Mage and Priest differ only in the exp-table modifier (both +40 per
WCCCLASS `exp`).

### 8.3 Shop listing — `(Too powerful)` / `(You can't use)` suffixes

Newhaven Spell Shop (room 1/2144, shop 48), Mage L1
(`oracle_spell_learning.raw`):

```
The following items are for sale here:

Item                          Quantity    Price
------------------------------------------------------
scroll of magic missile       120          Free
scroll of illuminate          100          4 gold crowns (Too powerful)
scroll of blur                105          Free
scroll of smite               60           8 gold crowns (Too powerful)
scroll of cause harm          125          Free (You can't use)
scroll of minor healing       140          Free (You can't use)
scroll of bless               110          4 gold crowns (You can't use)
scroll of vine strike         118          Free (You can't use)
scroll of starlight           95           Free (You can't use)
scroll of mend                80           8 gold crowns (You can't use)
songsheet of valour           100          4 gold crowns (You can't use)
songsheet of misfortune       82           8 gold crowns (You can't use)
songsheet of discord          65           8 gold crowns (You can't use)
```

- `(Too powerful)` = scroll teaches a same-class spell whose `level`
  (+0xbe, required power) exceeds the character's **level**: illuminate
  (level 2) and smite (level 3) at char L1. At char L2 illuminate's suffix
  disappears while smite's remains (`oracle_spell_train.raw`) — the gate is
  the character level, not Spellcasting (43→45 across the same test).
- `(You can't use)` = wrong magery group (priest/druid/bard scrolls to a
  mage). Same annotation the M4 equipment gates use.
- Free rows (`cost=0`) show `Free`; priced rows show shelf price in the
  item's own cost denomination (4 gold crowns = cost 4, costtype 2).

### 8.4 Learning verbs (checklist 4)

Owned scroll, eligible spell (`oracle_spell_learning.raw`,
`oracle_spell_cast.raw`):

- `learn <scroll>` is **not a verb** — falls through to say:
  `You say "learn scroll of magic missile"`.
- `use <scroll>` learns and consumes; prints the learn line plus a blank
  line:

  ```
  use scroll of magic missile
  You read scroll of magic missile and learn the spell magic missile.

  ```

- `read <scroll>` (owned) learns, consumes, and adds a destruction line
  (`oracle_spell_cast.raw`):

  ```
  read scroll of blur
  You read scroll of blur and learn the spell blur.
  Its magic used, the scroll disintegrates.
  ```

  Both verbs reach the same handler (identical learn sentence); only the
  epilogue differs.

  *Correction (2026-07-17, `oracle_blur_duration.raw`):* the disintegrate
  line is not verb-bound — `use scroll of blur` printed it too, while
  `use scroll of illuminate` in the same session printed only the learn
  line + blank. Which epilogue appears varies by scroll (or roll), not by
  `use` vs `read`.
- `read <scroll>` when you own none (shop shelf nearby) prints the item's
  description paragraph and learns nothing:

  ```
  This parchment is inscribed with runes of magic, but exactly what is written
  can only be learned by reading it.
  ```

- **Too-high scroll** (smite, level 3, char L1 and L2 both —
  `oracle_spell_cast.raw` / `oracle_spell_train.raw`):

  ```
  use scroll of smite
  You may not use that item!
  ```

  The scroll is **not consumed** (still in inventory) and the book is
  unchanged. The level gate fires at learn time; a too-high spell can never
  enter the book, so there is no separate "cast too high" state to reach.
  After training to L2 the same character's `use scroll of illuminate`
  succeeds — pinning the gate to level.

- **Already-known scroll** (magic missile while it is in the book —
  `oracle_use_verbs.raw`, 2026-07-17):

  ```
  use scroll of magic missile
  You realize that you already know this scroll!
  ```

  The scroll is **not consumed** and the book is unchanged. `read` on the
  same owned scroll prints the identical line (also not consumed).

- **`use` never reads the shop shelf.** With no owned match, `use {arg}`
  prints `You don't have {arg}.` — seen both for garbage (`use zzz`,
  `oracle_use_verbs.raw`) and for a real shelf item not carried
  (`use scroll of blur` immediately after the owned one was consumed,
  `oracle_spell_cast.raw`). Only `read` falls back to the visible
  shelf/floor item description. `read {arg}` with nothing owned and
  nothing visible prints `You do not see {arg} here!`
  (`read zzz`, `oracle_use_verbs.raw`).

- **Owned non-LearnSp item** with no use action of its own (club,
  `oracle_use_verbs2.raw`): both `use club` and `read club` print

  ```
  You may not use that item!
  ```

  and the item is kept — the same refusal the too-high scroll gets. Items
  that carry their own use semantics diverge before this refusal:
  `use torch` → `You lit the torch.` (light-source path,
  `oracle_use_verbs.raw`; charged/usable items are out of M5 scope).

- Purchases (M4 cross-check): free rows print
  `You just bought scroll of magic missile for nothing.`; paid rows print
  the deduct_currency multiset, e.g.
  `You just bought scroll of smite for 8 gold crowns, 1 silver noble, 6 copper farthings.`
  (markup on an 8-gold scroll at Charm 40).

### 8.5 `spells` listing format

After learning (`oracle_spell_train.raw`, final state):

```
spells
You have the following spells:
Level Mana Short Spell Name
  1   4    blur  blur                          
  1   1    mmis  magic missile                 
  2   4    illu  illuminate                    
```

- Columns: `Level` = spell `level` (+0xbe), `Mana` = mana cost, `Short` =
  4-char shortname column, `Spell Name` padded to a fixed width with
  trailing spaces.
- Ordering: level ascending, then name (blur before magic missile even
  though magic missile was learned first — not insertion order).
- The on-disk book mirrors the display order: WCCUSERS record word array at
  disk +0x474 read `[1]` after learning magic missile and `[129, 1]` after
  adding blur (blur spell id 129 inserted before mmis id 1).

### 8.6 Cast strings (checklist 7 & 8)

All from `oracle_spell_cast.raw` unless noted.

- Unknown / unlearned spell (both echo the argument verbatim; learned-book
  lookup, so an unlearned real spell reads the same as garbage):

  ```
  cast zzz
  You do not know how to cast zzz.
  ```

  (`c smit` while smite is unlearned gives the same sentence.) Resolution
  accepts the shortname (`c mmis`, `c blur`, `c illu`) and the full name
  (`cast magic missile`); `c mami` does not resolve.
- Offensive cast with no target in a room with only a friendly NPC
  (Rayth, the Newhaven spell-shop keeper — `oracle_spell_learning.raw`):

  ```
  cast magic missile
  You are overcome with a feeling of guilt and break off your attack.
  ```

- Successful offensive cast at a monster (engages combat like `attack`;
  re-casting mid-combat toggles `*Combat Off*` / `*Combat Engaged*`):

  ```
  c mmis filthbug
  *Combat Engaged*
  You fire a magic missile at nasty filthbug for 13 damage!
  ```

  Prompt mana dropped 6→5 (mmis costs 1). Kill epilogue:

  ```
  You fire a magic missile at nasty filthbug for 10 damage!
  The filthbug collapses, its legs curling tightly around it.
  You gain 12 experience.
  *Combat Off*
  ```

- **Failed cast roll** (SC check missed): `You attempt to cast magic
  missile, but fail.` — prompt mana unchanged (mmis mana 1; the
  half-mana-on-fail charge truncates to 0).
- Benign self-target (blur, match `target=2`, no argument):

  ```
  c blur
  You cast blur on Vexil!
  You are blurred!
  ```

  Prompt 12→8. The buff persists: `st` appends `You are blurred!` after the
  stat sheet (`oracle_spell_train.raw`). Illuminate (match `target=1`)
  prints just `You cast illuminate!`.
- **One spell per round**, even for `energy=0` spells (blur):

  ```
  c blur
  You have already cast a spell this round!
  ```

- **Insufficient mana** (`oracle_spell_train.raw`, mana 0 vs blur cost 4):

  ```
  c blur
  You do not have enough mana to cast that spell.
  ```

### 8.7 Train strings (checklist 6)

`oracle_spell_train.raw`, Adventurer's Guild (room 1/2147, shop 38 type 8,
markup 0):

```
train
You hand over 5 silver nobles and you receive training to attain level 2.
You receive the following:
10 additional character points
```

- Cost printed for L1→2 at markup 0 was **5 silver nobles** (50 copper) —
  the `(markup+100)*level*5/100` formula's result is denominated in
  **silver**, matching the healer-services finding (economy.md).
- No spell line in the receipt; `spells` before/after identical (§8.1).
- Retry without exp: `You do not have the required experience to train yet!`

### 8.8 Expedition staging notes (for future oracle runs)

- Fresh characters start with **0 coins** (`Wealth: 0 copper farthings`).
- WCCUSERS.DB staging offsets (DOS disk record, sqlite `data_t.data`,
  emulator stopped, `.bak` kept): coin drawers dword×5 at +0x603 high→low
  (economy.md), **experience dword at +0x3c** with a second copy at
  +0x46f (patch both), spellbook word array at +0x474. Purse and exp
  patches verified in-game (`Wealth: 5250`, `Exp: 1500 ... [107%]`).
- Disconnecting in combat skips the exit save but the module still
  persists exp/coins/spells; re-entry prints
  `Last time you were on, you disconnected while playing.` /
  `The gods have punished you appropriately.`

### 8.9 Cast edges (bare cast, abbreviations, targeting)

Measured 2026-07-17: `oracle_cast_edges.raw` (run 1, driver
`oracle_cast_edges.py`) and `oracle_cast_edges2.raw` (run 2,
`oracle_cast_edges2.py`). Vexil L2 (HP 29, Mana 18), book = blur / magic
missile / illuminate. Empty-room probes on Newhaven, Narrow Road (`look`
confirmed no `Also here:` before every cast); monster probes in the Arena.

- **Bare `cast` (and bare `c`) print a syntax line** — not an error, not
  speech:

  ```
  cast
  Syntax: CAST {spell} [{target}]
  ```

- **Name resolution = exact shortname OR per-word prefix of the full
  name.** Shortname matching is exact-only; name matching accepts a prefix
  of each typed word against the corresponding name word (one letter
  suffices):

  | probe | result |
  |---|---|
  | `c mm`  | `You do not know how to cast mm.` (shortname prefix ✗) |
  | `c mmi` | `You do not know how to cast mmi.` (run 2 — still ✗) |
  | `c mmis` | resolves (exact shortname, §8.6) |
  | `c m` / `c magic` / `c magic mi` | all resolve (name word-prefix) |
  | `c bl` / `c b` | resolve to blur |

  `c magic mi` proves the trailing `mi` is consumed by name matching, not
  read as a target: in the empty room it printed `You must specify a
  target...` (below), where an unmatched word would have printed
  `You do not see mi here!`. (`c mami` failing, §8.6, is consistent:
  `mami` is not a prefix of `magic`.)
- **Offensive bare cast NEVER auto-picks a monster** — unlike `attack`.
  In the empty room, in a room with a live (unengaged, even attacking-us)
  giant rat, and even while melee-ENGAGED with it, `c mmis` gives:

  ```
  c mmis
  You must specify a target for that spell!
  ```

  Mana unchanged. The engaged case additionally printed `*Combat Off*`
  before the refusal — the bare cast broke off the melee engagement, then
  failed target resolution (run 2).
- **The guilt line is the friendly-NPC-present case** of the same bare
  offensive cast. Run 2, Weapons Shop with only Nathaniel:

  ```
  c mmis
  You are overcome with a feeling of guilt and break off your attack.
  ```

  Mana unchanged. Matches §8.6's `cast magic missile` next to Rayth. So:
  friendly NPC in room → guilt; otherwise (empty OR monsters only) →
  `You must specify a target for that spell!`. There is no "no effect"
  family.

  **Correction (decompile):** the "friendly NPC present" inference above
  is superseded — the NPC was a confound. The gate is the ROOM's
  protected flag, `room+0x564 & 1` (the `attributes` column; Weapons
  Shop and the Spell Shop carry bit 1, Narrow Road and the Arena do
  not), checked in `cast_no_target` 39163-39195 for the bare form and
  again in `cast_monster_target` 43232 for the targeted form (guilt
  refusal at 44290-44297) — so `c mmis rat` in a protected room refuses
  with the same guilt line. The measured strings above stand as-is.
- **Trailing words are the target, not garbage.** Everything after the
  matched spell name is looked up as one target string among room
  entities:

  ```
  cast blur extra trailing words
  You do not see extra trailing words here!
  ```

  Mana unchanged (blur did not self-cast).
- **Prefix bonus (blur is the book's only b-spell):** `c bl` resolved and
  missed its roll — `You attempt to cast blur, but fail.` with prompt mana
  14→12: blur costs 4, fail charged 2 — **half-mana-on-fail confirmed for
  even costs** (complements §8.6's mmis 1→0 floor). `c b` resolved and
  cast (`You cast blur on Vexil!`, mana 12→8, full 4).
- **Incidental (slice 4 + Task 11):** blur expiry line is
  `The effects of blur wear off.` An engaged offensive cast **auto-repeats
  each combat round like `attack`**: after `c mmis rat` failed its roll,
  the next round fired `You fire a magic missile at big giant rat for 6
  damage!` with no further input, charging mana normally (run 2; the same
  unprompted re-fire is visible in `oracle_spell_cast.raw`).

### 8.10 Monster attack lines (oracle-measured)

Expedition 2026-07-17, Newhaven Arena (down from Narrow Road). Victim =
Vexil (Human Mage L2, unarmored, 29 HP — killed out of all nine lives by
the end of the expedition; the character no longer exists); observer =
Oracle (Dwarf Warrior, chain coif). Transcripts:
`oracle_monster_lines1.log`/`2.log` (runs 1-2 are stdout logs — the raw
byte captures were lost to unflushed buffers, since fixed in `mudlib.py`),
`oracle_monster_lines3*.raw` + `3_rescue.log`, `oracle_monster_lines4*.raw`,
`oracle_monster_lines5*.raw`, `oracle_monster_attacks*.raw` (runs 5-5b and
the cleanup passes), `oracle_dead_vexil.raw` (out-of-lives aftermath),
`oracle_gear_recovery.raw`; prior rat and filthbug misses in
`oracle_cast_edges*.raw` / `oracle_spell_cast.raw`. Complements
`combat_rounds.md` §"Monster swing execution" (attack-form weight table;
the verb comes from the form).

**Hit (victim view)** — `The <name> <form verb phrase> for %d damage!`,
prompt HP drops by exactly the printed amount. There is NO generic
"hits you" — the verb phrase is the attack-form's own string:

```
The acid slime whips you with its pseudopod for 9 damage!
The nasty acid slime whips you with its pseudopod for 4 damage!
The nasty kobold thief stabs you for 2 damage!
The thin giant rat bites you for 10 damage!
```

(171 slime pseudopod hits, 63+ kobold stabs on record; kobold damage 1-8,
slime 2-9. The rat's hit verb is "bites" while both its miss lines use
"lunges" — the hit and miss strings are fully independent. No filthbug
hit ever landed across all runs, so its hit verb remains unmeasured.)

**Hit rider** — the slime's form carries a second, separately-rolled line
after every pseudopod hit:

```
Acid burns you for 1 damage!            (victim)
Oracle is burned by acid for 2 damage!  (observer)
```

**Miss, plain** — same verb phrase, no damage clause, HP provably
unchanged (inline prompt):

```
The giant rat lunges at you!
The big giant rat lunges at you!
The large giant rat lunges at you!
The nasty filthbug swipes at you with its claws!
The acid slime flails at you!
The nasty kobold thief lunges at you with their shortsword!
```

**Miss, dodge** — `..., but you dodge out of the way!` (rat, filthbug,
slime) or the shorter `..., but you dodge!` (kobold):

```
The giant rat lunges at you, but you dodge out of the way!
The nasty filthbug claws at you, but you dodge out of the way!
The acid slime lashes at you, but you dodge out of the way!
The nasty kobold thief lunges at you with their shortsword, but you dodge!
```

Both miss framings exist for the same monster (kobold: 1 plain vs 24
dodge; rat: 9 plain vs 2 dodge), and the miss verb can differ from the
hit verb AND from the other miss's verb: slime hit "whips … pseudopod",
plain miss "flails", dodge "lashes"; filthbug "swipes … with its claws"
plain vs "claws" dodge; kobold "stabs" hit vs "lunges … shortsword" both
misses; rat "bites" hit vs "lunges" both misses. So hit / plain-miss /
dodge are independent per-form message strings, not one verb in a shared
template — matching the per-form hit/miss message ids in the monster
records. Two more monsters seen only as a bystander (carrion beast:
`The carrion beast snaps at Poop with its teeth!` plain / `…snaps at
Poop, but he dodges out of the way!` dodge; lashworm: `The lashworm
lunges at Poop!`) fit the same shapes.

**Name adjectives are per-spawn flavor, not distinct monsters.** The same
kobold thief spawned as "nasty/angry/tall/fat/small kobold thief", the
giant rat as "thin/small/large/big/nasty/angry giant rat", the slime as
"nasty/big acid slime" — and the DEATH line always uses the base name
("tall kobold thief" dies as `The kobold thief falls to the ground with a
shrill cry.`). Monster death lines are per-monster:

```
The giant rat falls to the ground with a tortured squeak.
The kobold thief falls to the ground with a shrill cry.
The acid slime dissolves into a puddle of bluish goo.
The filthbug collapses, its legs curling tightly around it.
The carrion beast falls to the ground with a yelp, and is still.
```

**Observer view** — identical strings with the victim's name substituted
for "you" (pronouns follow: "he dodges", "their"); observers DO see the
damage number:

```
The acid slime whips Oracle with its pseudopod for 2 damage!
The nasty acid slime whips Vexil with its pseudopod for 9 damage!
The acid slime lashes at Vexil, but he dodges out of the way!
Vexil is burned by acid for 3 damage!
```

**Glance family:** never observed in ~300 monster swings against both an
unarmored mage and a chain-coif warrior; the monster-swing equivalent of
the player's absorb line (`Your swing at <name> hits, but glances off its
armour.`, see `combat_rounds.md`) remains unmeasured — if it exists it
needs a properly armored victim to stage.

**Downing / death sequence** (incidental capture, both perspectives):

```
Vexil drops to the ground!                  (same line in own view and room)
You may not do that while you are mortally wounded!   (every command while down)
You have been killed!
But, due to a miracle, you have been saved.
You have 8 lives left.
Vexil is dead.                              (observer's death line)
```

While mortally wounded the prompt keeps counting down (`[HP=-136/MA=18]`;
`health` shows `-136/29 [-468%]`) and monsters keep swinging at the body
with the normal hit lines. Death fired at -203..-204 with maxhp 29 (-7x)
but at -208 with maxhp 35, so the threshold formula is not settled. The
miracle revives the character at the Newhaven Healer at full HP, minus one
life; everything carried drops where the body lay. Also measured: the next
entry after a mid-play disconnect prints `Last time you were on, you
disconnected while playing. / The gods have punished you appropriately.`
(one occurrence dropped the whole inventory on the spot; another left
inventory intact — the punishment is not a fixed item drop).

**Out of lives** (measured the hard way — this expedition burned all of
Vexil's): the last death prints

```
You have been killed!

You have no lives remaining!
```

followed by the game banner, a `MUD Internal Error - Please tell your
sysop / Invalid room 0/0>` hiccup, and then **Temple, Halls of the Dead**
(marble slabs, shrouded forms, prompt is a bare `>` with no HP block).
On the NEXT entry to the Realm the character is gone: the account lands
straight in character creation (`You must choose a valid race.`). Vexil
is dead; long live Vexil. Related healer string when broke:
`You do not have sufficient funds to buy full healing!` (partial funds
are taken as partial payment otherwise: `You hand over 8 copper farthings
and all your wounds are healed.` / `You hand over nothing and all your
wounds are healed.` at full HP).

**Spawn incidentals:** `A acid slime oozes into the room from nowhere.`
(bare "A" even before a vowel), `A nasty filthbug scuttles into the room
from nowhere.`, `A large giant rat creeps into the room from nowhere.`,
`A angry kobold thief sneaks into the room from nowhere.` — the entry
verb is per-monster too. Floor items — coin piles included — persist
across an MBBSEmu restart; live monsters do not.

**Bystander bonus strings** (another player "Poop" fought in view):
`Poop moves to attack giant rat.`, `Poop punches acid slime for 7
damage!`, `Poop critically punches giant rat for 21 damage!`, quarterstaff
verbs `whaps`/`beats`/`smacks` (`Poop critically whaps angry kobold thief
for 45 damage!`), miss `Poop swings at carrion beast!`, `Poop wields
quarterstaff!` / `Poop removes dagger.`, `Poop breaks off combat.`,
`Poop picks up quarterstaff.`, `Poop just disconnected!!!`, and the
third-person move-fail `Oracle ran into the wall to the up.`

### 8.11 Duration timing (oracle-measured)

Measured 2026-07-17. Character: **Zinvar Duskmere, Human Mage** (fresh
roll on the freed `Vexil`/test123 BBS account; the Given Name FSD field
comes prefilled with the account name and typing appends — backspace it
out first). Transcripts: `oracle_blur_duration.raw` (raw capture) and
`oracle_blur_duration_timing.log` (millisecond-timestamped clean lines —
the raw capture has no timestamps; the timing lives here). Driver:
`tools/oracle/oracle_blur_duration.py` (interactive FIFO session).
Blur record: duration 70, duration_per_level 0, duration_increase (0,0),
level_cap 0 — re-verified in `re/mmud_wgnt.sqlite` during this run, so
every measurement below is the same flat 70-tick spell.

**Tick length.** Wall clock from the `You cast blur on Zinvar!` line to
the async `The effects of blur wear off.` line:

| run | char level | emulator process | elapsed | s/tick (70) |
|---|---|---|---|---|
| 1 | L1 | original (tmux pty) | 211.97 s | 3.028 |
| 2 | L1 | original (tmux pty) | 213.78 s | 3.054 |
| 3 | L1, refreshed at +110.03 s | original (tmux pty) | 214.36 s from recast | 3.062 |
| 4 | L2 | restarted, GUI on non-tty | 369.43 s | 5.278 |
| 5 | L2 | restarted, GUI on non-tty | 268.12 s | 3.830 |
| 6 | L2 | restarted in tmux pty | 282.16 s | 4.031 |

Runs 1-3 are tightly grouped: **~3.03-3.06 s per tick, i.e. a ~3 s
upkeep cycle — NOT the 5 s energy round** (blur 70 ticks ≈ 3½ minutes).
**Runs 4-6 (all after emulator restarts, all at L2) did NOT converge**
(268-369 s), and the mana-regen cadence in the same windows shows the
emulator's world clock itself was the moving part: consecutive +2 regen
events arrived every **31 s, dead stable,** in the original process, but
39-66 s (run 4), ~40 s (run 5) and ~34 s (run 6) after restarts —
i.e. the whole game clock ran slow and erratically in the restarted
processes (worst with the console GUI writing to a non-tty), so the DLL
plausibly counted the same 70 ticks that simply arrived late. The regen
slowdown fully accounts for runs 4-5; run 6's excess (282 s against a
regen cadence only ~10% slow) is NOT fully explained — either the fresh
world's routine pass ran slower than its regen pass, or there is a real
level/world-state term in the duration that the record's all-zero
scaling fields don't show. **Best-supported reading: 70 ticks at ~3 s
per tick (3.03-3.06 measured over three consistent runs); the L2
divergence is emulator scheduling, with a residual unresolved wobble —
re-measure L1 vs L2 back-to-back in ONE long-lived process before
trusting any level dependence (ORACLE-VERIFY).** Recommendation for the
M5 slice-4 `Job::Upkeep` constant: **3 s nominal.** Input does not
alter tick flow: run 1 polled `look` every 60 s + an `st`, run 2 was
near-idle (one `look`), same duration within one tick.

**Cast-time line order** (raw bytes, first cast): command echo, then

```
You cast blur on Zinvar!            <- castmsgb, bright blue (1;34), CRLF
[HP=26/MA=8]:                       <- prompt, mana ALREADY deducted
\x1b[79D\x1b[K                      <- prompt line erased in place
You are blurred!                    <- DescMsg line3, bright blue, CRLF
[HP=26/MA=8]:                       <- fresh prompt
```

DescMsg line3 goes out through the async-message path (erase pending
prompt, print, re-prompt) — net visible order: castmsgb line, line3,
prompt. The wear-off line uses the same path, in yellow (0;33):
`The effects of blur wear off.` printed with no input pending.

**Recast while active = silent full refresh.** `c blur` mid-buff
(+110.03 s in) produced byte-identical output to a first cast — both
lines, no "already have" variant anywhere — and charged full mana
(12→8). The timer resets to full: expiry came 214.36 s after the
*recast* (324.38 s after the original cast), squarely one full duration
from the refresh. For slice 4: a normal player self-recast takes the
refresh-in-place branch; no special message exists.

**`st` line.** While active, the sheet appends `You are blurred!`
(DescMsg line3) directly after the `MagicRes` row; after expiry the line
is gone. Bonus: the sheet row labeled **Martial Arts rose by exactly the
stored magnitude** while blurred (13→18, blur value 5 = Dodge 34) and
reverted on expiry — the Dodge contribution is visible there, so slice 4
can assert on it.

**Failed cast charges half mana, truncated — nonzero case.** `You
attempt to cast blur, but fail.` (cyan 0;36) with prompt mana 14→12
(blur costs 4 → charged 2). Confirms §8.6's mmis-based truncation
reading; the deduction shows up via a prompt refresh a beat after the
fail line.

**Illuminate does NOT enter the status sheet — because it is not a
duration spell.** Trained L2 (exp patch per §8.8, `WCCUSERS.DB.bak-zinvar-preL2`
kept), bought the scroll (`You just bought scroll of illuminate for
4 gold crowns, 8 copper farthings.`), learned, cast with blur active:
`You cast illuminate!`, and `st` still showed only `You are blurred!`.
The record explains it: illuminate has **duration 0** with RoomIllu
(148) value 4012 — room light is its own mechanism, no active slot, and
no wear-off line was ever observed for it. The `st` suffix is strictly
"DescMsg line3 of each occupied active slot", not "any lingering
effect". (Multi-slot ordering with two DescMsg spells remains
ORACLE-VERIFY — no second slotted buff is learnable at L2 mage.)

Also observed: name validation on character save took ~30 s this time
(previous characters took ~10 min on the same DB — the polling walk is
evidently not a fixed cost), and `Poop just left the Realm.` is the
clean-logout counterpart to §8.10's `Poop just disconnected!!!`.
