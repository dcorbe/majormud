# M5 Magic — Design

Milestone 5 of the MUD reimplementation (`2026-07-16-mud-reimplementation-design.md`):
casting, effect application, duration/upkeep, monster casting. Behavioral
authority is `re/docs/spellcasting.md` (WG3-NT decompile); cross-references
`abilities.md`, `combat.md`, `records.md`. Spec citations below are to
spellcasting.md sections.

## Scope decisions

- **Full spellbook subsystem** — persistent per-character spellbook, learned
  via guild-shop `LearnSp` scrolls (and trainer grants if the oracle confirms
  they exist). No auto-know shortcut.
- **Monster casting is in scope**, tested with fixture-placed casters; M6
  spawning inherits it.
- **All deferred `M5` hooks close in this milestone**: poison as a live
  mechanic, the healer cure-poison path, dynamic accuracy/AC accumulators in
  derived stats, the `" (Too powerful)"` listing suffix, and the `cast`
  command forms. `rob` stays out (thief skill, not magic).

## Approach

Vertical slices in dependency order; each slice ends green, hand-testable,
and committed. Engine-first was rejected: no playable feedback until late,
big-bang integration risk.

**Status (2026-07-17):** Slices 1-4 COMPLETE on branch m5-magic — content
layer (all 1379 spells load/validate), spellbook (scroll-only learning,
oracle-proven), cast skeleton (deferred-fire combat model, Damage(-MR),
protected rooms, saves), duration engine (active slots, upkeep,
termination + EndCast chaining) — live gates passed against the real DB.
Slice 4's upkeep tick is oracle-measured at ~3 s (§8.11: blur's 70 ticks
≈ 3.5 min) and player death terminates every slot with the EndCast chain
SUPPRESSED (decompile 13053-13066 — unlike reroll's honored chain).
Implementation-corrected details live in the slice plans and
spellcasting.md §8; where this doc's slice 2/3 bullets disagree with those
(auto-pick targeting, friendly-NPC guilt inference, magnitude swap), the
measured/decompiled versions win.

Slice 5 COMPLETE (2026-07-17): kai powers + invoke, player-target benign
casts, area casts, poison + healer cure, msgstyle-odd, item-target
skeleton — two oracle expeditions (§8.12/§8.13) plus goldens. Key
discoveries: kai powers are TRAINER-GRANTED into the ordinary spellbook
(one per level, announced in the train receipt — the class-keyed
complement of §8.1's "a mage's train grants nothing"); players are NEVER
area targets — measured alone, with players present, and with an explicit
player word (the DLL's player sweep is flag-gated, so match-12 areas have
no PvP path); and train's cost formula is SILVER-denominated — the value
converts through `ratios[0]` like the healer services (§8.12's
copper-only mystic paid 50/100 copper for the receipts' 5/10; we had been
charging the raw value as copper). Next: slice 6 (monster casting).

Slice 6 STATUS (2026-07-18) — landed, with ONE known open item. What
landed: the kind-2 cast dispatch in the monster attack driver (flat
success % as a strict `genrdn(0,100) <` compare, the whole cast inside
the form's energy gate — full cost on a landed cast, half floored at 1
on a resist/fizzle, silent skip when unaffordable); the player-side
saving throw (SpellImmu auto-resist independent of the chance roll,
`min(mr/2, 97)` — the 97 cap is one BELOW the player-cast path's 98);
the instant-effect table at the player (Damage/DamageMR/Drain/Poison/
Summon/EnergyLevel/hunger/thirst) and the duration entry via the
`monster_add_cast_spell_to_user` semantics (refresh only if the new
value EXCEEDS the stored one, fixed duration, display only on a real
write, full refund + silent abort on a rejected entry, poison
hard-write AFTER the entry — these §6 spec corrections are commit
4952ce0); monster 5-slot arrays with reduced upkeep and a termination
that reverses only Enslave + Poison, no EndCast chain; live poison
delivery end to end; Summon spawns; player-side Fear-flee; AlterSpDmg
folds. Oracle §8.14 measured the hit fan-out live (victim castmsgb line
WITH the damage number, room line WITHOUT — a simultaneous pair; no
cast-level scaling beyond the record band for spells 82/359), the full
poison lifecycle (`st` "You are Poisoned!", "You feel ill." ticks net
of regen, temple-healer cure), the flat -200 death floor, and free
per-round cast retargeting (an M6-tagged divergence note sits in the
driver); the resist and fizzle lines stay decompile-only — every
reachable caster carried a 100% form. Goldens: tests/monster_cast.rs
(30 tests) + the spell_scenario slice-6 companion (65% caster fight +
venom lifecycle, seeded).

RESOLVED (commit 71e2ae5): `monster_cast_area` is IMPLEMENTED — all 101
area/match-1 cast forms live (dragonfire, hellstorm, chaos storm...).
Decompile-faithful quirks: players-only iteration, silent per-target
saves at count time (97 cap, SpellImmu never read), silent fizzle with
half charge, no-refund room-entry abort, instant Damage(1) only for
match 5/10/13 (the 12 shipped match-12 Damage carriers are dead rows),
dispel-drops-victim, self-slot for match {1,2,4,6}. §8.14's dragonfish
line reconciled: a single-occupant area cast is indistinguishable from
a targeted one. The `st` "You are Poisoned!" sheet line is rendered
(bare counter, appended after the active lines — ordering ORACLE-OPEN).

**M5 COMPLETE (2026-07-18):** all six slices landed; 509 tests; final
review passed. Live hand-verification along the way: slice-2 shop/learn
byte-diff vs the oracle, slice-3 giant-rat kill, slice-4 blur wear-off
at +213.4s (oracle band 211.97-214.36s), slice-5 two-session blur,
slice-6 moaning-spirit drain line. Remaining ORACLE-VERIFY inventory
(80 in src) is the M6 punch list — headliners: FUN_0043e3db worn-item
immunity predicate, area fan-out with >1 player, monster resist/fizzle
lines (no reachable <100% caster), healer 25-silver poisoned price.

## Slice 1 — Content layer: the full Spell record

`Spell` in `content.rs` grows from 6 fields to the full cast-path record,
mapped 1:1 from spec §1's offset table:

- **Gating:** `class_gate_group` (+0xd6), `required_class_level` (+0xf4),
  `required_power` (+0xbe), `level_cap` (+0xa2)
- **Costs:** `mana_cost` (+0xf0), `round_cost` (+0xbc)
- **Resolution:** `base_chance` (+0xc8; ≥200 = auto-succeed), `target_mode`
  (+0xc4), `match_type` (+0xcc)
- **Magnitude:** `min_base`/`max_base` (+0xc0/+0xc2), min/max per-level
  numerator/denominator byte pairs (+0xf2/f3, +0xf6/f7)
- **Duration:** `duration` (+0xce; 0 = instant), per-level num/denom
  (+0xf8/f9), per-level multiplier (+0xca)
- **Element:** `element` (+0xd0) as an enum
  (Cold/Fire/Stone/Lightning/Water/Poison)

`match_type` and `target_mode` are enums; unknown values fail boot
validation. Additional validation: zero scaling denominators are LEGAL
shipped data (magic missile ships one; the runtime guard yields 0 — corrected
during slice 1, do not re-introduce a load error here); `EndCast` (151)
ability values must reference existing spell
ids (same dangling-reference treatment as messages). Deliverable: full-DB
boot test proving every real spell validates (the M0 pattern). No behavior.

## Slice 2 — Spellbook: state, persistence, learning

- **State:** `Player` gains a spellbook — `BTreeMap<SpellId, bool>` where the
  bool marks `GiveTempSpell` (160) temporaries (purged at effect end, wired
  in slice 4).
- **Persistence:** new `player_spell` table
  (`player_id, spell_id, temporary`), loaded with the character, written on
  learn — same shape as `player_item`.
- **Learning:** items with `LearnSp` (42) teach their value-slot spell on
  use, gated by `user_can_use_spell` (spec §2: class group, spell power,
  alignment lattice). Guild shops already sell scrolls via M4.
- **Open question resolved first:** whether training a level auto-grants
  class spells is not pinned down. First oracle expedition answers it (roll a
  mage in MBBSEmu, train, watch `spells`) before implementation. The spec
  wins; no guessing.
- **Commands:** `spells` lists the book in the oracle-captured format.
  Trainer/shop scroll listings gain `" (Too powerful)"` from the same
  predicate.

## Slice 3 — Cast pipeline: command to instant effect

- **Parser:** `cast <spell>` / `cast <spell> <target>` plus abbreviations
  (`c minor at rat`). Names resolve against the spellbook only, by
  short-name prefix. Match type picks the entry path (no-target /
  user-target / monster-target / item-target); wrong syntax rejected.
- **Gate order is behavior** (spec §2/§3) — each failure has a distinct
  message and cost: confusion divert → HP/fear → MageBind/KaiBind → room
  no-cast protection (partial round refund) → target counting (zero = "no
  effect in this…") → resource check with per-shortage messages.
- **Resolution:** success = `genrdn(0,100) < min(SC + base_chance, 98)`;
  base ≥200 auto-succeeds. Failure: full round cost, **half mana**, no
  effect. Success: full costs, then magnitude
  `L = min(power, level_cap)`, scaled bounds from the fraction pairs,
  `V = genrdn(0, hi-lo+1) + lo`.
- **Target saving throw** (found during planning; spec §3 corrected): on a
  successful targeted cast, the target saves per the spell's save class
  (`typeofresists` +0xc6): 0 = never, 1 = only if target has AntiMagic,
  2 = always — roll `genrdn(1,100) <= min(targetSC/2, 98)`; SpellImmu
  auto-resists. Resist costs the caster like a failure (full round, half
  mana) with the "You resisted %s's %s" message family. Applies to both
  user- and monster-target player casts.
- **Instant handlers (single-target):** Damage (element-resisted, routed
  through the M3 kill/exp-split path), Heal, Drain, EnergyLevel,
  hunger/thirst.
- **Messaging:** caster line, room broadcast, kai/mystic variants ("invoke a
  power"/"kai"); kai casters consume the once-per-round invoke flag.

## Slice 4 — Duration engine: active slots, upkeep, termination

- **Active slots:** fixed **10-slot array** of
  `(spell_id, value, remaining_ticks)` on `Player` — an array because slot
  exhaustion is observable. Entry per `add_cast_spell_to_user` (spec §4):
  duration scaled by `L = min(caster_power, level_cap)` with fraction pair
  and multiplier cap, `genrdn(base, max+1)` when base < max, then the
  `AlterSpLength` (166) percentage. Already-active spells refresh in place.
  Slots persist to sqlite; buffs survive logout.
- **Dynamic effects:** `derive()` already rebuilds from an `AbilityBag`, so
  recompute walks active slots and feeds each spell's ability table into the
  bag — no undo logic. This lands the deferred accumulators (AC, MaxDamage,
  DR, Accuracy set-if-greater, Quickness/Slowness flags, HPRegen, ManaRgn, …
  per the spec §4 table) in `Derived`, consumed by existing combat math.
- **Tick upkeep** (spec §5): per-slot decrement; recurring handlers (Damage,
  Drain, Heal, EnergyLevel, hunger/thirst, Cure Poison, Fear-flee,
  HealMana); death-check mid-loop; at zero: clear slot, terminate.
- **Termination** (also on death; later via dispels): DescMsg end message;
  explicit reversal of hard writes recompute can't undo (Poison, the six
  stat buffs 44–49, AlterHP, GiveTempSpell purge); then EndCast chaining —
  `CastOnEnd%` (164) roll, forced mode-2 cast skipping pre-check gates;
  recompute.

## Slice 5 — Breadth: area targeting, dispels, poison

- **Area/room casts:** match types 3/5/9/10/0xb/0xc/0xd iterate valid
  players, and (3/5/9/0xb/0xc) room monsters. Area magnitude **divides by
  target count** before per-target resistance; single-target never splits.
  Offensive area casts emit evil warnings and auto-engage combat (M3
  machinery). Duration area spells enter each target's slots individually.
- **Item targets:** `cast_item_target` for match types 6/7. Handler set kept
  to what the loaded DB actually uses — checked, not speculated.
- **Dispels:** RemovesSpell (122) / KillSpell (153) pre-pass — find the named
  spell in the target's slots, terminate; RemovesSpell honors the EndCast
  chain, KillSpell suppresses it.
- **Summon (12):** `generate_monster` into the room, owned by caster; reuses
  M3/M4 monster placement.
- **Poison live:** Poison (19) as a hard write reversed at termination;
  per-tick Cure Poison (20); poison counter ticking damage. Healer
  cure-poison completes: 25-gold poisoned path replaces the
  15-silver-only stub (`game.rs:1801`), matching oracle-verified pricing.
- **Resistances** extend to all negative effects:
  `V' = (100 − resist) × V / 100`, element vs
  Rcol/Rfir/ResistStone/Rlit/ResistWater/ImmuPoison.

## Slice 6 — Monster casting (spec §6)

- **Source:** cast-type attacks in the template attack list
  (`attackaccuspell_i` is the spell id for those slots); M3 attack selection
  gains the cast branch.
- **Economics:** no mana, no round pool, no half-cost-on-fail. Flat
  per-attack success % vs `genrdn(0,100)`; forced casts (index −1) always
  fire.
- **Saving throw**: like the player-cast save (slice 3) but extended with
  the template's per-attack save DC vs spell power, and not gated on the
  spell's save class (unverified — check during this slice). Save prints
  "You resisted %s's cast of %s" and halves/negates.
- **Effect entry:** same player 10-slot array, simpler policy — refresh only
  if the new value exceeds the current; fixed duration, no caster scaling,
  no AlterSpLength. Spells targeting monsters use the monster **5-slot**
  array plus the recompute-dirty byte.
- **Monster upkeep:** reduced handler set (Damage, EnergyLevel, Heal, Cure
  Poison, Fear-flee); termination reverses only Enslave and Poison; no
  EndCast chaining.
- **Testing:** fixture-placed caster with a real spell attack from the DB.

## Testing & completion bar

- **Tier 1 — spec-cited unit tests** (TDD, red first): 98% chance cap,
  half-mana-on-fail, magnitude bounds with zero-denominator guards, duration
  scaling with AlterSpLength, area split, resistance arithmetic, save-throw
  math. Each cites its spec section.
- **Tier 2 — seeded scenario tests:** golden transcripts — learn-from-scroll
  session, success/fail casts with exact resource deltas, buff persisting
  across recompute, expiry with end message, EndCast chain, poison
  tick-to-cure-at-healer, monster caster round. These pin gate/message
  order.
- **Tier 3 — oracle expeditions:** deterministic surfaces diff against
  MBBSEmu — `spells` format, eligibility-failure messages, scroll listings
  with `(Too powerful)`, cast/broadcast shapes, healer pricing. Randomized
  outcomes carry decompile line citations instead (M3 precedent). The
  trainer-vs-scroll question is the first expedition. MBBSEmu data may be
  edited to stage test conditions, and oracle characters are disposable —
  roll as many as needed.
- **Done:** six slices merged, `cargo test` green, clippy clean, zero `M5`
  markers left in the tree, oracle scripts pass or carry a documented
  whitelist entry, and a hand telnet session — roll a mage, buy and learn a
  scroll, cast a damage spell at a fixture monster, watch a buff expire.
