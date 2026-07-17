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
- Combat expeditions (2026-07-17, monster-attack-lines runs) added more:
  - Kill scripted sessions only as a last resort: SIGTERM used to lose the
    whole raw capture to unflushed buffers (mudlib now flushes per recv),
    and the game punishes mid-play disconnects (`The gods have punished
    you appropriately` — can drop the character's whole inventory on the
    spot).
  - The prompt HP goes NEGATIVE while mortally wounded — parse
    `\[HP=(-?\d+)`. A downed character can do nothing; monsters keep
    swinging until death at roughly -7x maxhp, then a "miracle" revives
    them at the Newhaven Healer at full HP, minus one **life** (chars
    start with ~9; check before risking more).
  - Monsters get a free attack on movement and it can BREAK the move, so
    blind `u`/`w` walk sequences desync. Verify every step by room name
    and retry (see `oracle_monster_cleanup.py::move`).
  - Newhaven arena spawns include acid slimes (~10 dmg/round pairs) that
    will burst a L2 mage between two guard polls. `buy healing` at the
    Newhaven healer (west of Narrow Road) is a full heal for 2cp/HP.
  - Floor items persist across an MBBSEmu restart; live monsters do not —
    restarting is the clean way to defuse a monster-camped room.
  - When driving two sessions, pump both sockets in strict interleave;
    any blocking wait on one leaves the other's character unattended in
    combat (this killed a character twice).

## Scripts

- `mudlib.py` — session driver (login, expect, capture).
- `oracle_m1_capture.py` — the M1 capture: creation, room display,
  movement, errors, quit. Produces `oracle_m1.raw` + sections JSON.
