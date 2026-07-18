# M5 Slice 6: Monster Casting — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Monsters cast — attack-form kind 2 through the combat driver, the
player-side saving throw, monster 5-slot effects with upkeep, offensive-
duration split, Summon/Fear/AlterSpDmg. This closes M5.

**Spec:** re/docs/spellcasting.md §6 (monster casting) + §8 measurements.
**Data:** 219 monsters carry cast forms; AttackForm already loads them
(kind 2: `accuracy` = spell id, `min_damage` = cast success %, `max_damage`
= cast level; the M3 comments say so). The `continue; // cast/rob forms
arrive in M5+` skip sits in `monster_attack_sequence`.

## Facts pinned during planning

- Cast-form examples: dark cleric 33 / dark priest 34 (spiritual hammer 16,
  instant offensive, 65/70%), tentacled abomination 37 (spits venom 79,
  duration 100 POISON — the live poison delivery), mummy 31 (breathes 84,
  duration 50), hellhound 36 (breathes flame 78, 100%).
- Spec §6: no mana/round pool for monsters; flat per-form success % vs
  genrdn; forced (idx −1) casts always fire; target saving throw =
  SpellImmu(139) + spell power vs the form's save DC (cast level) +
  AntiMagic(51) + target SC halved; "You resisted %s's cast of %s";
  `monster_add_cast_spell_to_user` refreshes only if the new value EXCEEDS
  the current; fixed duration (no caster scaling, no AlterSpLength);
  monster 5-slot array + dirty byte; reduced upkeep (Damage/EnergyLevel/
  Heal/Cure Poison/Fear-flee); monster termination reverses ONLY Enslave +
  Poison; no EndCast chaining; monster crits don't exist.
- Slice-5 leftover markers to close: monster active-spell slots (areas +
  offensive-duration), Summon(12) (87 carriers, all monster payloads),
  Fear(60) flee (all carriers monster payloads), AlterSpDmg(165)
  aggregation (monster-side), offensive-duration engagement split
  (engage-only is conditioned on duration==0 in the DLL).
- Poison delivery goes LIVE here: venom casts set the player poison
  counter (SET-IF-GREATER, ImmuPoison gate — machinery from slice 5).

## Task 1: Oracle expedition — monster casts at a player

Zinvar (Mage L8, Silvermere Docks — LOCKED OUT of Newhaven, stay in
Silvermere) hunts a casting monster; check placed spawns near Silvermere
(dark clerics/priests haunt the graveyard/sewers — survey room monster
placements in the DB first and pick the safest reachable caster; Zinvar
has 51 HP — flee early, heal often; kill -INT only). Measure:
1. The monster cast lines (victim + room if Kaimon can be ferried in —
   optional), hit AND resist outcomes; the resist line family.
2. A duration debuff landing (slot on the player? st line? wear-off?).
3. If a venom-caster is findable at survivable level: live poison
   (counter, "You feel ill." ticks, healer cure end-to-end).
4. Forced-cast/area monster casts if encountered (breath weapons).
Deliver: §8.14, transcripts, commit. If no caster is safely reachable,
capture what IS reachable and report honestly.

## Task 2: Monster cast dispatch + saving throw

`monster_attack_sequence` kind-2 branch: flat `genrdn(0,100)` vs cast %
(spec §6 — check the decompile for < vs <=), then the target save
(SpellImmu auto; cast level vs ... read the decompile's save formula at
the §6 cites — attack_monster_user's cast branch / monster_cast fn), then
route into the spell effect machinery with the MONSTER as caster: instant
offensive abilities at the player (Damage/DamageMR/Drain/Poison via the
existing per-target apply — factor what slice 5 built), castmsgb fan-out
(monster name as caster arg), no engagement changes (the monster is
already fighting). Resist prints the measured/§6 line, effect skipped.
Cast level = the magnitude L (replaces caster power for scaling).

## Task 3: Player-slot entry from monster casts + offensive-duration split

`monster_add_cast_spell_to_user` semantics: refresh only if new value >
current; duration fixed from the spell record (no scaling). Duration
debuffs (venom 79) enter the PLAYER's slots → poison hard-write on entry,
upkeep ticks it, termination reverses (all existing). ALSO: the
player-side offensive-duration split the slice-3 marker promised —
player casts of offensive-duration spells (none learnable; fixture) apply
the slot to the monster instead of engage-only? NO: monster slots — Task 4
decides; keep the engage-only + marker if monster slots make it moot.

## Task 4: Monster 5-slot array + upkeep + termination

`MonsterInstance` gains `[ActiveSpell; 5]` + recompute-dirty flag; player
duration casts at monsters (smite 7! learnable L3, match 2 offensive
duration 60) enter monster slots; slice-5 duration AREAS now apply to
monster slots (close that marker); monster upkeep in the 3s tick
(reduced handlers per spec §6: Damage/EnergyLevel/Heal/CurePoison/
Fear-flee); termination reverses only Enslave/Poison, no chains; monster
derived stats: the dirty flag + a monster ability_bag fold for slot
contributions (AC/DR debuffs affecting the M3 combat mapping — evasion=ac,
soak=dr*10). Smite at a monster goes end-to-end: slot, debuff, expiry.

## Task 5: Summon + Fear + AlterSpDmg

- Summon(12): monster cast spawns the named monster (value slot) into the
  room via the --spawn machinery, owner tag = aggression marker (M6).
- Fear(60): recurring-handler flee — the monster/player flees a random
  exit (move machinery exists; player-side "flee" = forced move).
- AlterSpDmg(165): caster-side damage boost aggregation (monster ability
  bag + player bag — closes the slice-4 divergence note).

## Task 6: Gates + M5 close-out

Goldens (fixture dark-cleric fight: cast lines, save, poison lifecycle,
smite debuff on monster); live gate (spawned caster vs a test char on our
server — hellhound or dark cleric); full suite + clippy; marker sweep
(zero SLICE 6 pending markers — retag genuine M6/M7 items); design-doc M5
COMPLETE + memory; final whole-M5 review; ORACLE-VERIFY inventory report
for the checkpoint. CHECKPOINT: M5 done — discuss merge to main
(finishing-a-development-branch) and M6.
