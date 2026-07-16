# MajorMUD Plus (WCCMMPLS) — add-on module

**MajorMUD Plus — Entertainment Edition**, by West Coast Creations / Metropolis, Inc.
An optional add-on DLL that layers extra player-facing / sysop features onto the base
game. From the module definition (`WCCMMPLS.MDF`): *Requires WCCMUD*, `Needs Me: WCCMMUD`,
one DLL (`WCCMMPLS`), one MSG, **1 Btrieve file** (2048-byte page). It is a small module —
121 functions in the 32-bit build; the named exports map 1:1 to the features below.

Sources: `WCCMMPLS.DOC`/`.AD` (feature list), `WCCMMPLS.MSG` (CNF config keys + prompts),
and the 32-bit decompile at `re/wccmmpls_ghidra/exports/`.

## Features (from the module's own manual)

1. **User-editable extended character descriptions** — players write a personal description,
   optionally for a fee. Sysop can view/delete a specific user's description
   (`begin_listing_outstanding_descriptions`), and descriptions can require approval.
2. **MajorMUD Registry** — 10 sysop-configurable info fields users fill in, with a global
   **`/mudreg`** lookup that combines real game data with the user-entered registry info.
3. **Purchase lives** — buy extra MajorMUD lives, charged in BBS credits and/or in-game
   copper and/or membership days.
4. **Offline action editor** — edit MajorMUD social actions offline.
5. **DMA-server billing path** — instead of charging credits directly (which fails on some
   DMA servers), send an email notification to a designated sysop user.

## Configuration surface (CNF keys, from `WCCMMPLS.MSG`, with stock defaults)

| key | meaning | default | range |
|-----|---------|---------|-------|
| `DESCCHRG` | credits to change description | 3600 | 0–1000000 (0 = free) |
| `DESCCOPP` | copper to change description | 0 | 0–1000000 |
| `DESCDAYS` | membership days to change description | 0 | 0–1000000 |
| `DESCLEN`  | max description length | — | — |
| `NEEDAPPR` / `APPRUSER` | description needs approval / approving user | — | — |
| `BUYLIFE`  | credits to buy a life (**−1 disables the feature**) | 28800 | −1–90000000 |
| `COPPLIFE` | copper to buy a life (disabled if BUYLIFE=−1) | 10000 | 0–99999999 |
| `DAYSLIFE` | membership days to buy a life | 0 | 0–60 |
| `MINYEAR` / `STARTAGE` | registry "age" display baseline | — | — |
| `REGDISP0..9` | the 10 registry field display templates | — | — |
| `REGCOLR` / `REGGLOB` | registry colour / global-lookup config | — | — |
| `SYSOPKEY` | sysop access key | — | — |

## Mechanics (from the decompile)

### Buy-a-life (`add_life_to_user` / `charge_user_to_buy_life`)
- Refuses if the user record can't be found, and **caps at 9 lives** — the same life cap the
  base game enforces (character lives at player `+0x6a6`; see `character_creation.md`,
  `death.md`). At 9 it prints "no more lives" and stops.
- Charging walks the three configured currencies in order: **BBS credits** (test with
  `otstcrd`, deduct with `odedcrd`, amount `BUYLIFE`), **in-game copper** (compared against the
  player's currency field), and **membership days** (`DAYSLIFE`). Any shortfall aborts the
  purchase with no charge. On success, `addon_adding_life_to_user` increments the base-game
  life counter.

### Descriptions (`plus_user_description`)
- Stored in the module's single Btrieve file (keyed by user), separate from the base game's
  room `FILE_DESCRIPTION` mechanism (`gangs.md`). Charged per `DESCCHRG`/`DESCCOPP`/`DESCDAYS`;
  length-limited by `DESCLEN`; optional sysop approval gate.

### Registry & `/mudreg`
- Ten user-entered fields (`REGDISP0..9`) merged with live character data (level, class, etc.)
  and shown via the global `/mudreg <name>` command — a cross-module registry lookup.

### Menus (`plus_menu_display` / `plus_menu_command`)
- A small text menu (`MAINMENU`/`LIFEOPT`/`SYSMENU`/`SYSMMENU`) fronts the description editor,
  life purchase, and sysop tools; config is (re)loaded by `reload_ini_settings`.

## Relationship to the base game
Purely additive and optional. It reads/writes the base player record for the life counter and
character data, but its descriptions and registry live in its **own** Btrieve file. Disabling
it (or setting `BUYLIFE=−1`) removes the paid features without affecting core gameplay. For a
faithful reimplementation it is a **separable, optional layer** — document/implement the base
game first; Plus is a bolt-on.
