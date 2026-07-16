# MajorMUD 1.11p — behavioral specification & data

A reverse-engineered specification of **MajorMUD** (WCCMMUD, by Metropolis Inc.), rebuilt
for a from-scratch reimplementation on modern architecture. This directory documents *how
the game behaves*; the accompanying data + tooling capture *what the game contains*.

Everything here was derived from the game's own binaries and data files and validated
against a clean decompile. It is a behavioral spec — not the original source.

## Provenance & authority

MajorMUD shipped as three host-target builds (`WCC[DS|WG|NT]###`): DOS (16-bit),
Worldgroup (16-bit), and **Worldgroup-3/NT (32-bit PE)**. This project **targets the
WG3-NT build** because it is the final structure *and* the one whose 32-bit PE decompiles
cleanly (592 named exports — the naming key for everything else).

- **Engine logic** was read from the 32-bit `wccmmud.dll` (985 functions, `wg_nt_ghidra/`)
  and cross-checked against the 16-bit build. Core math is identical across builds; only
  peripheral *tuning* differs (flagged inline as `[16-bit / WG3-NT]`).
- **Data layouts** follow the WG3-NT records (`*2.vir`), whose sizes match the open-source
  Nightmare-Redux field maps exactly.
- Where a value is a `.data` literal an agent located but didn't extract, it's flagged
  "undetermined (mechanism certain, number not read)".

## Architecture at a glance

A few load-bearing patterns recur across the specs:

- **Tick system** (`combat_rounds.md`): a 1 s metronome drives self-rescheduling jobs —
  **5 s** combat rounds (energy), **3 s** spell-upkeep, **30 s** regen/wander. Nearly all
  time-based behavior hangs off these four tiers.
- **Derived-stat rebuild** (`leveling.md` §5): `update_dynamic_stats` recomputes *all*
  combat-relevant fields from scratch each call by replaying the player's permanent-ability
  table + active spell slots + worn gear through one dispatch. Buffs, gear, and level thus
  share one code path — and expiring effects need no "undo" (`spellcasting.md`, `economy.md`).
- **Shared ability system** (`abilities.md`): spells, monsters, items, races, and classes
  all encode effects as `(ability-id, value)` pairs against one 188-entry enum.
- **Data-driven content**: quests are scripts in the text-block DB (`quests.md`), not code;
  gang-house descriptions are external files (`gangs.md`). Much of "the game" is data.

## The specification (read in roughly this order)

### Data foundation
- **`vir_schemas.md`** — the `.VIR` Btrieve container format, record layouts, and the
  file catalog. The base for everything data.
- **`records.md`** — player / race / class / item / room record field offsets, the
  experience curve, and cross-build offset notes.

### Combat
- **`combat.md`** — single-attack resolution: the quadratic to-hit formula, damage, crits,
  parry, attack types (with the 16-bit/WG3-NT tuning split).
- **`combat_rounds.md`** — the round loop: cadence, attacks-per-round (energy ÷ EU),
  monster attack behavior, engage/break, result application.

### Character & progression
- **`character_creation.md`** — creation flow, starting state, the gender field resolution.
- **`leveling.md`** — training, per-level gains, and the full derived-stat formula set.
- **`regeneration.md`** — HP/mana regen (30 s tick), near-death bleed/stabilize, hunger/thirst.
- **`death.md`** — death trigger, penalties (drop-all + a life; **no exp loss**), respawn,
  monster loot, the equal-per-head exp split.

### Magic & abilities
- **`abilities.md`** (+ `ability_ids.tsv`) — the 188-ability enum with dispatch semantics.
- **`spellcasting.md`** — cast eligibility/flow, effect application, the duration/maintenance
  system, monster casting.

### World & economy
- **`monsters.md`** — player-driven spawning, `generate_monster`, wander/leash movement,
  the behavior-mode taxonomy.
- **`economy.md`** — shop buy/sell (Charm-based) pricing, restock, equipment/worn-slots,
  item charges, banking.

### Content systems
- **`gangs.md`** — membership/roles, gang funds, guild houses, and the external-`.HSE`
  room-description mechanism.
- **`quests.md`** — the data-driven text-block scripting VM, quest state, ask-a-question,
  rewards, and the class-quest connection.

### Add-on module
- **`majormud_plus.md`** — the optional WCCMMPLS add-on (descriptions, registry, buy-lives).

## Data & tooling artifacts (in `re/`)

| artifact | what it is |
|----------|-----------|
| `mmud_wgnt.sqlite` | The full dataset — 9 tables: room 26720, spell 1379, monster 1101, item 1950, message 3867, shop 178, class 15, race 13, action 67. Live records only (`_raw` blob per row). |
| `vir_wg.py` | Validated Btrieve 6.x reader for the WG3-NT `.vir` files. Filters deleted records (usage-word prefix ≠ 1) and stale shadow-page duplicates. |
| `rectype.py` / `import_mmud.py` | Nightmare RecType parser + the SQLite importer. |
| `room_graph_wg.py` | Builds the world graph: 26,720 rooms / 17 maps, 100% exits resolve, 98% reciprocity. |
| `ability_ids.tsv` | Raw ability-id → name/description table. |
| `hse_files/` | Default gang-house descriptions (134 files; 129/131 referenced rooms covered). |
| `exports/gang_house_description_files.txt` | room → `.HSE` filename manifest. |
| `wg_nt_ghidra/` | The 32-bit decompile (`WCCMMUD_decompiled.c`, functions/imports) + findings. |

## Known open items (trivia — nothing blocks understanding)

- A handful of `.data` literal constants (exact regen/spawn timers, death-floor, recall
  room ids, coin ratios) — mechanisms documented, raw numbers not extracted.
- Naming: stat 2 is "Wisdom" (ability enum) vs "Wil/Willpower" (Nightmare race field) — same stat.
- A few quest reward `(ability, value)` pairs need a raw-disassembly pass (Ghidra dropped
  the stack args); three quest counters set-but-unread by the engine (payoff data-side).
- ~2 gang-house descriptions (`RHUDAUR.ANS`, `WCC40044.HSE`) are board-custom, unrecoverable
  without that board's data.
