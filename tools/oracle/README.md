# MBBSEmu oracle harness

Runs the original WCCMMUD 1.11p (DOS build) under MBBSEmu as a live
behavioral oracle. Captured transcripts live in `re/oracle/`.

## Setup (done once, 2026-07-16)

1. MBBSEmu linux-x64 release unzipped to `~/mbbsemu/` (v1.0-alpha-010225).
2. Module files from `mudbins/WCCMMUD_1.11p_DOS_MBBSEmu_v2.zip` unzipped to
   `~/mbbsemu/wccmmud/`.
3. `appsettings.json`: `Telnet.Port` 2327, `Rlogin.Enabled` false.
4. First-run database: `printf 'PW\nPW\n' | ./MBBSEmu -DBRESET`
   (it prompts twice; a single answer crashes the reset).
5. Run: `cd ~/mbbsemu && ./MBBSEmu -M WCCMMUD -P wccmmud/`.

BBS account used by the scripts: `Oracle` / `test123` (signup via `NEW`).
Character: Oracle Delver, Dwarf Warrior, not Lawful, default stats.

## Quirks learned the hard way

- **Name validation** (`ljngame_validate_name_polling`) steps one database
  record per poll cycle (~2.5/s) comparing the new name against every player
  and monster record — expect **~10 minutes** of spinner on character save.
  Disconnecting during validation discards the character.
- The FSD stat editor is driven with: CR (accept given name), `<family>\r`,
  then CR through the remaining fields; the Exit field defaults to SAVE.
- Output embeds junk-char+backspace pairs inside words (anti-bot trick, e.g.
  `nP\x08orth` renders as "north"). Transcript diffs must apply backspace
  semantics before comparing.
- Output is CP437 with ANSI; the module is in demo mode (unregistered),
  which caps usage but not the M1-relevant behavior.

## Scripts

- `mudlib.py` — session driver (login, expect, capture).
- `oracle_m1_capture.py` — the M1 capture: creation, room display,
  movement, errors, quit. Produces `oracle_m1.raw` + sections JSON.
