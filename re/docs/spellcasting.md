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
`0x7a` vs `0x99`).

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
| Damage | 1 | HP `+0xb0 -= V`; routes through the combat kill path — `check_kill_user` / `distribute_experience` (matches `combat.md`). Resisted by element (see below). |
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
