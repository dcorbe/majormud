# MajorMUD (WG3-NT) — Character-creation spec

Behavioral spec derived from the 32-bit WG3-NT decompile
(`wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Cross-references `records.md`
(struct offsets), `leveling.md` (`roll_stats`, `calculate_secondary_stats`, the
`+0x94`=level finding), and `abilities.md` (stat ability ids 44-49). Offsets are
byte offsets into the player / race / class records.

Functions read: `create_player` (0x16234), `roll_stats` (0x1a2d1),
`calculate_secondary_stats` (0x1a424), `edit_character_stats` (0x487d), the
account-menu state machine `case 0x33/0x34/0x38/0x3a` (the enter-game driver at
~lines 2600-3540), the race/class setters `FUN_00416cfb`/`FUN_00416d5a`, the
name setter `FUN_00416ca2`, `get_user_currency` (0x1eb84), and the runtime
`gender` command (~line 54787, ~60786).

---

## 1. Creation flow

Character creation is a BBS **state machine** keyed on `usrptr+0x1c` (the menu
state). The relevant transitions:

1. **Name** — states `0x15 → 0x14`. The player enters a name; it is validated
   (≤10 chars, alphabetic only via `__ctype` class bits, not already taken via
   `find_player`, first letter force-capitalised) and copied to `player+0x1e`
   (`FUN_00416ca2` / case 0x15). `player+0x7c8 |= 1` marks "name pending
   validation"; a polling routine (`ljngame_validate_name_polling`) confirms it.
   The account/BBS handle is the default; `create_player` pre-seeds `+0x00` and
   `+0x1e` with `uacoff()` (the account name) before the player overrides it.

2. **`create_player(term)`** — fired from the enter-game menu (case 0x38) the
   first time a paid slot creates a character (`load_player` returns "no save",
   `pcVar7 == 0`). It **zero-initialises the whole 0x7ec-byte player record** and
   writes the defaults in §2/§3/§4 below. It then advances to state `0x33` and
   prints "Please choose a race…". **It does NOT set race, class, stats, gold,
   or items** — those come from the following steps.

3. **Race** — state `0x33` (`0x49` = one-time race change). Input is `_atol`'d and
   passed to `FUN_00416cfb`, which validates the id against the WCCRACE DFA menu
   block (`dfaSetBlk(DAT_00479114)` / `dfaAcqLock`) and, if legal, stores it in
   **`player+0x90`**. Invalid → "You must choose a valid race". On success it
   displays the class list and advances to state `0x34`.

4. **Class** — state `0x34` (`0x4a` = one-time class change). `FUN_00416d5a`
   validates against the WCCCLASS DFA block (`DAT_00479118`) and stores the id in
   **`player+0x92`**. Invalid → "You must choose a valid class".

5. **Alignment (optional)** — after a valid class, if `player+0x542 < 1`
   (not already evil) **and** `DAT_00482dcc` (a sysop flag enabling evil chars) is
   set, the game asks the good/evil question and moves to state `0x3a`:
   - **Y (evil):** `player+0x544 = 0xf6`, `player+0x542 = 0xffcd` (−51 alignment).
   - **N (good/neutral):** `player+0x544 = 0`; if `+0x542 < 0` it is reset to 0.
   Otherwise this prompt is skipped.

6. **`roll_stats(term)`** — invoked immediately after class/alignment (case
   0x34/0x4a and case 0x3a both call it). Copies the racial stat template, CP
   grant, HP-base seed, gender, and computes derived stats (§2).

7. **`edit_character_stats(1)`** — the interactive full-screen (FSD) stat/apparence
   editor (§2.4). The player spends character points to raise the six primaries
   within racial caps and picks cosmetic appearance. This is the last creation
   step before entering the Realm.

**Gender is not chosen in this flow.** It is inherited from the BBS account
(see §5).

---

## 2. Starting stats

### 2.1 `create_player` placeholder values
Before race is known, `create_player` writes throw-away defaults:

- **Level** `player+0x94 = 1` (word). New characters are **level 1**, not 0.
  (`leveling.md`: `+0x94` is the level; the `<2` test in `roll_stats` is the
  char-gen guard.)
- **Six current stats** `+0x96..+0xa0 = 5` and **six effective stats**
  `+0xa2..+0xac = 5` (word each). Placeholder — overwritten by `roll_stats`.
- **Max/current HP** `+0xae = +0xb0 = DAT_00482cc8` (config). Placeholder —
  overwritten by `calculate_secondary_stats`.
- Legacy exp total `+0x3c = 0`; current-exp counter `+0x470/+0x474 = 0`.

### 2.2 `roll_stats` — the six primaries are FIXED racial values, not rolled
Despite the name, `roll_stats` performs **no randomisation**. It copies the race
template deterministically:

- `race+0x20..+0x2a` (six words: Int, Wis, Str, Hea, Agl, Chm) → **effective
  stats** `player+0xa2..+0xac`, then mirrored into **current** `+0x96..+0xa0`.
  Two identical characters of the same race always start with identical stats.
- **Character points** `race+0x46` → both `player+0x6e2` (unspent CP) and
  `player+0x6fa` (lifetime CP). This is the pool the stat editor spends.
- **HP-base seed** — only because level `< 2`: `class+0x22` → `player+0x724`
  (byte). `+0x724` is the flat HP-base addend that `train_level` later grows.
- **Gender** `player+0x7d6 = account+0xd5` (see §5).
- **`player+0x7c` title** re-sprintf'd from a level/rank string
  (`create_player` had set it to "Apprentice"; `roll_stats` refreshes it).
- Calls **`calculate_secondary_stats(term, 0)`** — mode 0 = full recompute of max
  HP / max mana / all derived skills **and sets current = max** (a fresh
  character starts at full HP/mana).

### 2.3 Initial derived stats (via `calculate_secondary_stats`, level = 1)
All formulas are in `leveling.md §5`; at creation `L = 1`. The load-bearing ones:

- **Max HP** `+0xae` = `Health/2 + 1*(class[+0x20]+race[+0x2c]) + (Health−50)/16 +
  HPbase(+0x724) + …`. Class- and race-dependent; small at level 1.
- **Max mana** `+0x600` = 0 for non-casters; for caster groups
  `MaxManaAbil + class[+0x42]*1*2 + … + 6`; Kai group → `L−1 = 0`.
- Perception, stealth, thievery, traps, picklocks, tracking, dodge, MR, carry cap
  are all derived from the six primaries + level + class/race gates.
- Current HP `+0xb0 = +0xae` and current mana `+0x602 = +0x600` (mode 0 fills to
  full).

### 2.4 `edit_character_stats(1)` — spend CP, pick appearance
Opens an FSD form (`fsdroom`) showing name, race name (`raceRec+2`), class name
(`classRec+2`), and per stat the **race base** (`race+0x20..+0x2a`), **racial cap**
(`race+0x66..+0x70`), and **current** value (`+0xa2..+0xac`). The player spends
the `+0x6e2` CP pool to raise stats up to the racial caps (escalating CP cost, see
`leveling.md §6`). It also edits cosmetic appearance indices
`+0x6dc` (clamped ≤5), `+0x6dd` (≤9), `+0x6df` (≤0x10) — hair/eye/complexion-style
descriptors rendered from the `PTR_DAT_0048032c` / `PTR_s_black_*` string tables.

---

## 3. Starting resources

Everything below is what `create_player` leaves in the fresh record.

- **Currency: zero.** Coins live in five denomination dwords
  `player+0x610, +0x614, +0x618, +0x61c, +0x620` (read by `get_user_currency`
  via `convert_currency`). `create_player` zeroes all five. **New characters are
  broke.**
- **Inventory: empty.** The carried-item id array (`+0x268`, 100 words) is set to
  `-1`, and the parallel `+0x334` (0x32 dwords) / `+0x3fc` (0x32 words, `0xfffe`
  pattern) tables are cleared. No starting items.
- **Worn equipment: none.** The worn-slot arrays `+0x62c` (0x14 dwords, zeroed)
  and `+0x67c` (0x14 words, `-1`) are empty.
- **Spellbook: empty.** No `add_spell_to_spellbook` call occurs in creation.
  Casters gain spells later through training/leveling; Kai classes auto-learn
  level-gated powers in `update_kai_powers`.
- **Character points:** seeded in `roll_stats` from `race+0x46` into `+0x6e2`
  (unspent) and `+0x6fa` (lifetime) — the only "resource" a new character has.
- **Lives:** `player+0x6a6 = 9` (word). Confirmed lives field — death decrements
  it ("You have %d lives left", `+0x6a6 -= 1`; `<1` ⇒ character death/deletion);
  `train_level` adds `DAT_00482cd8` capped at 9; resurrection shops raise it up to
  9. **New characters start at the 9-life cap.**
- **Experience: 0**, and `player+0x7d4 |= 0x2000` is set at creation, i.e. new
  characters are **already flagged "experience restructured"** (`leveling.md §2.1`)
  so they can earn exp immediately without the migration prompt.
- Other notable non-zero seeds: `+0xb3 = 1`, `+0xd5 = 100`, `+0x330 = 50`,
  `+0x478 = 100`, `+0x5f4 = 1`, `+0x5f5 = 2`, `+0x6a4 = 1`, `+0x6e1 = 1`,
  `+0x6fe = +0x702 = +0x703 = 3`, `+0x700 = 0x10|0x400 = 0x410` (flag word),
  `+0x720 = 0x16`, `+0x7d4 |= 0x80`, `+0x7c6 = today()` (creation date),
  `+0x542 = get_saved_evil_points(account)` (alignment restored from account).
  `+0xce = +0xd0 = 0x3e8` (1000) are written but their player-record meaning is
  unconfirmed.

---

## 4. Starting location

`create_player` sets the newbie start position:

- `player+0xc4 = 1` (word) — realm/map field.
- `player+0xc6 = 0` (word).
- `player+0xc8 = DAT_00482cf8` (dword) — the **sysop-configured starting room
  number** (a global loaded from the game config; the exact value is not a literal
  in the DLL).
- `player+0x5f0 = 1` (byte) — current-map id, used by `display_room_desc` when the
  character first enters.

(Location field span `+0xc4..+0xca` per `records.md`.) **Tournament mode**
overrides this: when `DAT_004906c9 == 2` and `player+0x706 == 0`, entry code forces
`+0xc4 = 1` and `+0xc8 = 0xe0` (room 224) — the tournament start room.

---

## 5. Constraints, alignment, and the true gender field

### 5.1 Valid race × class combinations
Legality is **data-driven**, not hard-coded. `FUN_00416cfb` (race) and
`FUN_00416d5a` (class) validate the selected id against DFA menu blocks
(`WCCRACE` / `WCCCLASS` via `dfaAcqLock`); the class list presented
(`display_class_list`) is likewise filtered from the data files. The engine
advertises **13 races** (Human, Dwarf, Gnome, Halfling, Elf, Half-Elf, Dark-Elf,
Half-Orc, Half-Ogre, Goblin, Kang, Nekojin, Gaunt One) and **15 classes**
(Warrior, Witchunter, Paladin, Cleric, Priest, Missionary, Ninja, Thief, Bard,
Gypsy, Warlock, Mage, Druid, Ranger, +1). The specific allowed pairings live in
the WCCRACE/WCCCLASS `.VIR`/`.DFA` data, not in this DLL.

### 5.2 Alignment
`player+0x542` (signed word) = alignment / "evil points", restored from the
account at creation via `get_saved_evil_points`. The optional good/evil prompt
(state 0x3a, §1.5) sets it to −51 (evil) or clamps it to ≥0 (good/neutral).
`get_legal_level` (`leveling.md §1`) later maps this score to the 0-7 alignment
tier used for spell legality. `player+0x544` is a companion evil-state byte set
alongside it.

### 5.3 Gender — the true field is `player+0x7d6`
`records.md` originally guessed gender at `+0x94`; `leveling.md` proved `+0x94` is
the **level**. The **real gender field is `player+0x7d6`**, a single ASCII byte:
`'M'` (0x4d) or `'F'` (0x46). Evidence:

- `roll_stats` seeds it at creation from the **BBS account record**:
  `player+0x7d6 = account+0xd5` (`iVar2 = uacoff(term)`), i.e. gender is inherited
  from the caller's BBS sex/gender setting rather than chosen in the char-gen flow.
- The runtime `gender male|female` command writes `'M'`/`'F'` to `+0x7d6`
  ("You are now male/female"), and a sysop change command toggles the same byte
  and prints "…has been changed to a male/female".

So a new character's gender = whatever the BBS account carried; it can be changed
in-game later via the `gender` command (which may cost currency, `DAT_00482dc8`).

---

## 6. Summary — initial state of a brand-new character

| aspect | value |
|--------|-------|
| Name | account handle by default; ≤10 alpha chars, unique, first letter capitalised (`+0x1e`) |
| Race / Class | chosen from data-file menus; stored `+0x90` / `+0x92` |
| Level | **1** (`+0x94`) |
| Six primaries | **fixed racial base values** (`race+0x20..+0x2a`), no rolling; `+0xa2..+0xac` (+ mirror `+0x96..+0xa0`) |
| HP / Mana | full — computed by `calculate_secondary_stats` from race+class+level 1; current = max |
| Character points | `race+0x46`, in `+0x6e2`/`+0x6fa`; spent in the FSD editor within racial caps (`race+0x66..+0x70`) |
| HP-base seed | `class+0x22` → `+0x724` |
| Gold / coins | **0** (five denomination fields `+0x610..+0x620`) |
| Inventory / worn gear / spellbook | **all empty** |
| Lives | **9** (`+0x6a6`, at the cap) |
| Experience | **0**, already flagged "restructured" (`+0x7d4 |= 0x2000`) |
| Alignment | from account; optional evil prompt sets −51 else ≥0 (`+0x542`) |
| Gender | inherited from BBS account (`account+0xd5` → `+0x7d6`, ASCII M/F) |
| Location | realm/map `+0xc4 = 1`, room `+0xc8 = DAT_00482cf8` (config), map byte `+0x5f0 = 1`; tournament → room 0xe0 |
| Title | rank string in `+0x7c` (e.g. "Apprentice") |

---

## 6.5 Oracle addendum (MBBSEmu, WCCMMUD 1.11p DOS, 2026-07-16)

Live-transcript findings that refine or extend the decompile reading above
(raw transcript: `re/oracle/oracle_m1.raw`):

- **Starting room = Newhaven, Village Entrance**, i.e. `DAT_00482cf8` resolves
  to room **(map 1, room 2140)** in the stock data.
- **Lawful prompt**: after a valid class (before `roll_stats` output), the
  game asks "Do you want to be Lawful? [Yes/No]" — the PvP opt-out. This is
  distinct from (and in the DOS build asked instead of / in addition to) the
  sysop-gated evil prompt in §1.5. Irrevocable except by reroll.
- **Family name**: the FSD editor requires a *family* (last) name in addition
  to the given name ("You must enter a last name!"). The full name renders as
  "Given Family" on the entry stat sheet.
- **CP cost chart** (FSD editor): within a stat, each successive block of 10
  raised points costs 1/2/3/... CP per point (so +10 = 10 CP, +20 = 30,
  +30 = 60, +40 = 100, +50 = 150).
- **Name validation** is slow by design: the polling routine steps one DB
  record per cycle across player and monster names (~10 min under MBBSEmu).
- Entry stat sheet for Dwarf Warrior L1, base stats, no CP spent:
  Hits 35/35, AC 0/0, Perception 35, MagicRes 55, Martial Arts 10,
  Stealth/Thievery/Traps/Picklocks/Tracking 0 — reference points for the
  `calculate_secondary_stats` formulas (`leveling.md` §5).
- **Exit ('x')**: "You will exit after a period of silent meditation." — exit
  is delayed (anti combat-logout), then dots print until departure.

## 7. Undetermined / flagged

- **Exact starting room number** (`DAT_00482cf8`) and the other config constants
  (`DAT_00482cc8` initial-HP placeholder, `DAT_00482cd0`→`+0xb6/+0xb8/+0xba`,
  `DAT_00482cd4`→`+0xbc`) are runtime-loaded globals, not literals in the DLL;
  their values need the game config / a live dump to pin down.
- The meaning of player `+0xce/+0xd0 = 1000`, `+0x330 = 50`, `+0x478 = 100`,
  `+0xd5 = 100`, `+0x720 = 22`, and the flag word `+0x700 = 0x410` at creation is
  not established here (they are not consumed by the creation path).
- The precise valid race×class matrix lives in the WCCRACE/WCCCLASS data files and
  was not extracted from the DLL.
- `+0x544` is confirmed as an evil/alignment companion byte but its full semantics
  (vs. `+0x542`) were not traced.
