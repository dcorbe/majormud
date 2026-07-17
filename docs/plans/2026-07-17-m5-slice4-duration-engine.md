# M5 Slice 4: Duration Engine — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Duration spells live end-to-end: blur grants +Dodge for 70 ticks,
shows in `st`, decays on the upkeep tick, wears off with its message, and
EndCast chains fire. Closes the slice-3 loud-fail tripwire
(`duration_spell_cast_applies_no_stats`).

**Architecture:** Active-spell slots on `Player` (persisted), fed into the
existing `AbilityBag`→`derive()` recompute; a new upkeep job in the M2 tick
scheduler; termination with hard-write reversal and EndCast chaining.
Spec: re/docs/spellcasting.md §1 (slots), §4 (entry + dynamic table),
§5 (upkeep + termination), §8 (measured strings). Design doc slice 4.

**Prior art to reuse, not recreate:** `spell_magnitude`, `ScalePair::scaled_duration`
(divide-first, already correct), `cast_command`'s benign/offensive branches
(slice-3 markers show where slots go), `render_cast_line`, `monster_killed`,
`state_db` table patterns + migration machinery, seeded-RNG test drivers.

---

## Facts pinned during planning

- **Blur (spell 129):** duration 70, duration_per_level 0, duration_increase
  (0,0), level_cap 0; abilities `[(Dodge 34, 0), (DescMsg 115, 68),
  (RemovesSpell 122, 157)]`; magnitude bounds 5..5. Spell 157 is the
  amethyst pendant's effect — blur dispels it on cast (anti-stacking).
- **DescMsg message record (msg 68):** line1 = expiry line
  `The effects of blur wear off.` (measured §8.9); line2 = empty (room
  variant unmeasured); line3 = active line `You are blurred!` — printed at
  CAST (§8.6) and appended after the `st` sheet (§8.6). One record, three
  roles.
- **Duration formula (spec §4):** `L = min(caster_power, level_cap)` with
  cap ≤ 0 = uncapped (same clamp as magnitude);
  `base = duration + duration_increase.scaled_duration(L)`;
  `max = duration_per_level * L`; if `base < max` →
  `duration = genrdn(base, max+1)` (genrdn inclusive — check against the
  slice-3 finding; range likely base..=max+1); then AlterSpLength (166):
  `duration = (alter + 100) * duration / 100`. Blur: all scaling zero → 70
  flat.
- **Refresh (spec §4):** spell already in a slot → refresh value + duration
  in place; when `applyFlag == 0` report "already have" instead —
  which flag applies to a normal player cast is UNMEASURED (oracle task 1).
- **Slots:** 10 per player, `(spell_id, value, remaining_ticks)`; slot
  exhaustion is observable (11th buff finds no slot — behavior unmeasured,
  flag it). Monsters have 5 slots — SLICE 6, do not build now.
- **Tick length: UNKNOWN.** Candidates: the 5s energy round or a dedicated
  cycle. Blur = 70 ticks; oracle task 1 measures cast→wear-off wall-clock
  (70 ticks @5s ≈ 5m50s). The DLL runs upkeep in the "medium/routine
  update" pass (spec §5) — likely its own interval; the measurement decides.
- **Dynamic effects:** recompute-from-scratch already exists
  (`ability_bag` game.rs:843 sums race/class/worn/weapon; `derive_for`
  wraps). Slice 4 adds active-slot contributions to the bag. Dodge (34) and
  AC (2) must demonstrably flow into combat (monster hit rate) — check how
  `build_player_defender` consumes Derived. Add Derived fields ONLY for
  abilities the tests exercise; the full §4 dynamic table lands ability by
  ability as content needs it (list the unported rows in a comment).
- **Termination (spec §5):** print DescMsg line1; reverse hard writes
  (stat buffs 44-49, AlterHP 88, Poison 19 — Poison itself is slice 5, keep
  the arm with a marker; GiveTempSpell 160 → purge the temporary spellbook
  entry, the slice-2 `temporary` flag finally pays off); EndCast (151) +
  CastOnEnd% (164, default 100) → `genrdn(0,100) < pct` → forced cast
  (mode 2: skips confusion/fear/round gates — in our terms: skip
  cast_this_round + energy + the syntax path; mana/level gates per spec
  ambiguity — follow the decompile at cast_no_target's mode-2 branches,
  cite lines); recompute after.
- **Death terminates all slots** (spec §5: same path fires on death/reroll,
  clear slot first, then termination with stored value).
- **Offensive duration spells:** slice-3 comment marks the engage-only
  branch as conditioned on `duration == 0` in the DLL
  (cast_monster_target `param_1[0x67] == 0`). Slice 4 splits: offensive
  duration casts apply their slot to the monster? NO — monster slots are
  slice 6. Offensive duration casts at monsters: refuse with the msgstyle
  temp message? Better: check what shipped offensive-duration spells exist
  and whether any is learnable pre-slice-6; if none reachable, keep
  engage-only with a loud slice-6 marker. Decide in Task 3 from data.
- **msgstyle-odd arg table:** the slice-3 refusal stands; slice 4 only
  needs it if a duration starter spell is odd — blur/illuminate are even.
  Check learnable duration spells' msgstyle in Task 3; if all even, the
  arg table moves to slice 5 (update the markers).

---

## Task 1: Oracle expedition — tick length + refresh semantics

The Vexil BBS account is free (the character died); roll a NEW caster on it
(Human Mage, name your choice — record it in the report and memory).
MBBSEmu telnet 2327, harness tools/oracle/README.md. Fresh chars have 0
coins; blur scroll is free at the Newhaven Spell Shop (1/2144). Budget
wall-clock generously — the measurement IS elapsed time.

Measure:
1. **Tick length:** timestamp `c blur` success, idle (send periodic `look`
   or blank — note whether input affects ticks), timestamp
   `The effects of blur wear off.` Compute seconds/tick (expect ~350s total
   if 5s ticks). Repeat once for confidence.
2. **Recast while active:** `c blur` again mid-duration — refresh silently,
   "already have" message, or error? Exact string. Does the wear-off timer
   visibly reset (re-measure to expiry after refresh)?
3. **`st` line:** confirm the sheet appends `You are blurred!` while active
   and drops it after expiry.
4. **Cast-time print order:** capture the exact line order for a success:
   castmsgb caster line vs `You are blurred!` (DescMsg line3) vs prompt.
5. If time allows: learn illuminate (L2 needed — patch exp per §8.8 offsets,
   emulator stopped, backup kept) and observe whether RoomIllu/TextBlock
   spells enter the status sheet too — informs how generic the st line is.

Deliverables: raw transcripts in re/oracle/, findings as spellcasting.md
§8.11 "Duration timing (oracle-measured)", commit
`doc: oracle — duration tick length, refresh, status line`.

## Task 2: Active slots — state + persistence

- `Player.active_spells: [ActiveSpell; 10]` where
  `ActiveSpell { spell: Option<SpellId>, value: i16, remaining: i32 }`
  (array, not Vec — slot exhaustion is observable; a helper
  `first_free_slot()` / `find_active(spell_id)`).
- New `player_effect` table (name, slot, spell, value, remaining; PK
  name+slot) — rides the existing transactional save/load/delete/migration
  machinery (add to TABLES; the migration test pattern exists).
- TDD: state roundtrip incl. mutate-and-resave; migration-from-current
  schema opens clean.
- Commit: `feat: active-spell slots — state and persistence`

## Task 3: Slot entry on cast

- Benign duration branch (the slice-3 marker in `cast_command`): compute
  duration per the pinned formula (new `spell_duration` fn next to
  `spell_magnitude`, TDD the formula: flat, scaled, genrdn-rolled band,
  AlterSpLength percentage — AlterSpLength source: caster's AbilityBag).
- Entry per spec §4: already active → refresh value+duration (Task 1's
  measured semantics decide the message); else first free slot; slot FULL →
  unmeasured, pick refuse-with-no-slot-consumed + ORACLE-VERIFY.
- Cast-time output: castmsgb lines (existing) then DescMsg line3 to the
  target (measured order from Task 1).
- RemovesSpell/KillSpell pre-pass on the CASTER's own slots (blur→157):
  find named spell in slots, terminate it — RemovesSpell runs its EndCast
  chain, KillSpell suppresses (termination fn arrives Task 6; for this task
  a minimal clear-slot + TODO wiring is acceptable IF committed green with
  the test asserting the slot clears; full chain honored in Task 6).
- Offensive-duration + msgstyle-odd decisions per the pinned-facts bullets
  (data check first, document what you find).
- Commit: `feat: duration casts enter active slots — formula, refresh, dispel pre-pass`

## Task 4: Recompute integration + status line

- `ability_bag` adds each active slot's spell ability table (value-0 slots
  contribute the STORED slot value — spec §4: the stored value is what
  persists, decide how (ability, 0) pairs map: the stored V replaces the 0,
  mirroring add_cast_spell_to_user's stored potency; cite spec §4 "The
  stored (id, value) in the active slot is what makes the effect persist").
- Dodge (34) must flow to the defender: check stats.rs Derived + M3's
  `build_player_defender`; wire whatever is missing so blur measurably
  drops monster hit rate (seeded test: identical combat script with/without
  blur slot differs per the dodge % semantics — find Dodge's consumption in
  the decompile/combat.md if it isn't already modeled; it may already be,
  M4 items carry Dodge).
- `st` appends DescMsg line3 for each active slot (measured §8.6; ordering
  = slot order, ORACLE-VERIFY if multiple).
- DescMsg-less active spells: no line (verify a fixture).
- This task deletes the slice-3 tripwire test and replaces it with the
  real assertions (blur cast → dodge in Derived + st line + slot entry).
- Commit: `feat: active spells feed derived stats — blur dodges, st shows it`

## Task 5: Upkeep tick

- New `Job::Upkeep` at the Task-1-measured interval (const with the
  measurement cite). Per non-empty slot: decrement; recurring handlers
  (spec §5 table: Damage 1, Drain 8, EnergyLevel 11, Alterhunger 15,
  AlterThirst 16, Heal 18, Cure Poison 20 — marker only until slice-5
  poison, Fear 60 — marker until fear flag exists, HealMana 150);
  death-check mid-loop (recurring damage kills route through the M3 player
  death path); at 0 → clear slot + terminate (Task 6's fn; stub print of
  DescMsg line1 acceptable interim if Task 6 follows immediately in the
  same session).
- Recurring handlers use the STORED value with per-ability overrides
  (spec §5: "value = slot value unless the spell overrides per-ability at
  +0xa8" — i.e. nonzero ability value wins, same convention as instant).
- TDD with manual tick driving (existing test pattern drives Jobs).
- Commit: `feat: upkeep tick — duration decay, recurring effects, expiry`

## Task 6: Termination engine

- `terminate_active_spell(session, slot, honor_endcast: bool)`:
  DescMsg line1 to the owner (room line2 unmeasured — ORACLE-VERIFY,
  emit nothing to room for now); hard-write reversal (stats 44-49 —
  Intellect/Wisdom/Strength/Health/Agility/Charm deltas on `stats`,
  AlterHP 88 — max AND current HP; Poison 19 marker; GiveTempSpell 160 →
  remove temporary book entry + persist); EndCast chain when honored:
  CastOnEnd% roll then forced cast per the pinned mode-2 semantics
  (decompile-cite the gate skips); recompute + prompt refresh.
- Death path: player death terminates ALL slots (order: slot order),
  suppressing or honoring EndCast per the decompile (spec §5 cites
  death/reroll → same termination path; check the param_5/chain flag at the
  death call site, lines ~10408-10419 per spec).
- Wire Task 3's dispel pre-pass to the real fn (RemovesSpell honors chain,
  KillSpell suppresses).
- EndCast test: fixture A with EndCast→B, CastOnEnd% 100 → B's slot
  appears when A expires; CastOnEnd% 0 → no chain; KillSpell dispel of A →
  no chain; RemovesSpell dispel → chain.
- Commit: `feat: spell termination — hard-write reversal, EndCast chaining`

## Task 7: E2E + gates + checkpoint

1. Golden scenario extension: mage learns blur, casts (slot + st line +
   dodge in effect), ticks to expiry (wear-off line, stats revert), seeded.
2. Live gate on our server: fresh char, blur lifecycle by hand-script —
   watch a monster's hit rate visibly drop while blurred (spawn a kobold),
   `st` line present, expiry after the measured wall-clock (shorten via a
   test-only tick interval? NO — run the real interval once; patience is
   the test).
3. MBBSEmu side-by-side: same sequence with the Task-1 character.
4. Full suite + clippy; zero stale `slice 4:` markers left (grep); update
   design-doc status + memory; final code review of the whole slice.
5. CHECKPOINT with Daniel before slice 5.
