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

- Duration expedition (2026-07-17, blur timing) added more:
  - **The game clock's wall-time depends on the emulator's environment.**
    MBBSEmu restarted with its console GUI writing to a non-tty ran the
    whole game clock ~1.74x slow (blur 70-tick duration 212s → 369s, mana
    regen 30s/point → ~52s). Run MBBSEmu inside a real pty (tmux pane) for
    any wall-clock measurement, and cross-check with the mana-regen cadence.
  - Name validation is not always ~10 min: Zinvar's validation completed in
    ~30 s on the same DB where earlier characters took ~10 min.
  - The FSD stat editor's Given Name field arrives prefilled with the BBS
    account name and typing APPENDS — send backspaces first to replace it.

- Monster-casting expedition (2026-07-18, slice 6) added more:
  - **Flood control**: >~15 rapid commands trip `Why don't you slow down
    for a few seconds?` and DROP input — blind move sequences desync.
    Keep ≥1.5 s per command and verify by room name.
  - **Death threshold is ~flat −200 HP** (kills at −200/−202/−209/−225
    on a 56-maxhp char), not −7×maxhp; unattacked downed chars bleed
    ~1 HP/slow-tick. Revive is at the AREA deathroom (Silvermere →
    Temple Halls of the Dead 1/2189), not Newhaven.
  - Silver River rooms pulse `The river bashes you up against some
    rocks!` (10-18 dmg, ~10 s cycle, can fire ~1.5 s after entry) —
    don't loiter, don't fight there.
  - Spawn-in-room (`X appears right beside you!`) attacks within the
    same second. Never leave a character unattended in a spawn room —
    poll ≤5 s or use `oracle_mcast_babysit.py`.
  - Restart-despawn + disk-patch recipe: logout (or accept the
    disconnect), `kill -INT`, edit WCCUSERS.DB `data_t` (Zinvar id 3;
    exp +0x3c AND +0x46f + key_2, gold dword +0x60b, **curHP word
    +0xb0, maxHP +0xae, poison counter +0xbe, room +0xc8**), restart.
    Floor coin piles persist across restarts; live monsters don't.

- Dodge-parry expedition (2026-07-26, slice 8) added more:
  - **`sysop summon <item-or-spell>` needs the `WCCSYSOP` key.** Without it
    the verb falls through to the SAY fallback and you get
    `You say "sysop summon ..."` — which looks like a syntax error but is a
    permission failure. Grant it from the sysop account (`sysop` at the BBS
    login) with `/SYS ADDKEY <user> WCCSYSOP`; `/SYS LISTKEYS <user>`
    confirms. `/SYS` also has REMOVEKEY, RESETPW, LISTACCOUNTS, KICK.
  - A few items refuse to be conjured: `A strange force stops you from
    getting this item.` (seen on `platinum ring`) — the summon line still
    prints `<item> conjured.` first, so check for both.
  - **Check for Cursed (ability 82) / CURSED (83) before staging any item as
    an experimental variable.** A cursed item cannot be taken off once worn,
    so a configuration built on one can only be changed by DYING (which drops
    everything and costs a life). This bit the dodge-parry runs: every
    negative-accuracy item worth wearing for its size — `smoky black talisman`
    and `shining white talisman` (-20), `malachite ring` (-12), `spiked
    collar` (-5) — is cursed. The removable negatives are the shields
    (`tower shield` -6, `black shield` -5, `kite shield` -4, all 250-500
    weight, so they move the encumbrance band too) and `darkwood ring` (-3,
    weight 10).
  - **Host-side commands are the only reliable escape.** `/xgoto` is
    intercepted by MBBSEmu before the module sees it, so it works even while
    mortally wounded, and unlike a walked flee it cannot be broken by the
    free attack a monster gets on movement. `/xcash <n> [denom]` sets the
    purse; `/xwhere` dumps the location key.
  - **`/xexp` grants experience but does NOT level the character** — the exp
    total moves and the level does not. Levelling requires visiting a
    **trainer** (shop type 8). Room 1/289 "Halls of Training, Entrance"
    carries shop 39 "Sysop Trainer", `shopclasslimit` 0 (every class) and a
    level band 1..999, and it has no walking path from the world, so `/xgoto`
    is the only way in. The verb is `train`, and one level reads:

        You hand over 50 copper farthings and you receive training to
        attain level 2.
        You receive the following:
        10 additional character points
        2 additional lives

    So a level costs coin, and **grants lives** — L1→L2 took a Dwarf Warrior
    from 35 to 44 max HP, +10 CP and +2 lives. Levelling is therefore also the
    cheapest way to refill the life budget an expedition burns.
  - **Something in room 1/289 pulses `The river bashes you up against some
    rocks!`** (10-18 dmg on a ~4 s cadence) even though the room renders as a
    marble chamber. It killed a level-2 character standing at the trainer.
    Whether this is a property of the room (its `roomtype_2` is 7, but that
    flag is shared with casinos and jail cells, so it is not diagnostic) or a
    room-effect subsystem that `/xgoto` fails to resynchronise is UNRESOLVED —
    heal and train in short bursts, and leave immediately.
  - **Mortally wounded is a dead end.** At HP < 0 every recovery verb
    refuses with `You may not do that while you are mortally wounded!` —
    `buy healing`, drinking a conjured potion, everything. The only exits
    are bleeding to the −200 death threshold or being finished off; both
    cost one life. Budget lives, and set the HP floor generously.
  - **Death drops the ENTIRE inventory** (worn included) and costs one life;
    the character revives at full HP at the area deathroom. Any staged kit
    must therefore be re-summoned at the start of every run.
  - **Coins weigh about a third of a unit each**, so the purse is a real
    encumbrance term: 4000 copper carried as healing money silently pushed a
    run from 26% to 65% encumbrance, which crosses the `enc < 33` cutoff in
    `move_player_to_fighter` and changes the character's ACCURACY. That is
    useful as a deliberate lever (`/xcash` sets it exactly) but it will
    corrupt a run if unnoticed — assert the encumbrance band before
    collecting.
  - `buy healing` at the Newhaven healer (1/2190) is a full heal and prints
    `You hand over N copper farthings and all your wounds are healed.`
  - The status prompt is written INLINE ahead of game text and several game
    lines can share one physical line (`[HP=22]:You punch giant rat for 5
    damage!`). Split transcripts on `\[HP=-?\d+\]:` before matching, or
    every regex anchored at `^` silently fails. Match monster nouns on a
    word boundary too — `prepa*rat*ions` in room prose otherwise counts as a
    swing at a `rat`.

## Scripts

- `mudlib.py` — session driver (login, expect, capture).
- `oracle_m1_capture.py` — the M1 capture: creation, room display,
  movement, errors, quit. Produces `oracle_m1.raw` + sections JSON.
- `oracle_blur_duration.py` — interactive FIFO-driven session with
  millisecond-timestamped clean log (for wall-clock measurements); commands
  `send`/`raw`/`quit` echoed into a FIFO, raw capture + timing log split.
- `oracle_kai_mystic.py` — same FIFO driver parameterized by raw-file path
  (mystic/kai expedition, spellcasting.md §8.12). Lessons: the FSD editor's
  Enter is CR NUL (`\r\x00` — bare `\r` is swallowed); stop MBBSEmu with
  `kill -INT` (the TUI eats `^C`; SIGTERM loses game-DB rows).
- `oracle_mcast_survey.py` — sqlite survey: BFS reachable rooms from a
  start (type-15 exits blocked), spawn regions, and casting monsters
  spawnable in them (slice-6 monster-cast expedition, §8.14).
- `oracle_mcast_drive.py` — batch command sender for the FIFO driver
  (prints the clean-log delta).
- `oracle_mcast_babysit.py` — camp watcher: auto-flees bad spawns,
  auto-fights whitelisted casters, HP-floor flee (island cave camp).

## Relocating the oracle (permanent installation)

Connection info is environment-driven — no script edits needed:

    export MBBS_HOST=<new host>   # default 127.0.0.1
    export MBBS_PORT=<new port>   # default 2327

`mudlib.Session()` and the standalone FIFO drivers all honor these.
When the oracle moves to a fresh installation, all characters must be
re-rolled (fresh WCCUSERS): the character roster documented across
§8.x (Zinvar, Kaimon, Oracle) belongs to the OLD install and those
sections' character-state notes become historical. The measured game
BEHAVIOR is installation-independent and stays authoritative.
