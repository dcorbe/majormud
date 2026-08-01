# Server darkness + light sources — implementation plan (2026-08-01)

Base: create branch `server-darkness` at 9cbee66 in ~/bbs (fast-forward
main to 9cbee66 separately when convenient). No mud-client changes.
Companion client work already landed on `farm-mob-detection`
(worktree ~/bbs-farm-fix): the client folds exactly the wordings below.

## DLL evidence (re/exports/WCCMMUD_named.c; offsets into WCCMMUD.DLL)

### _GET_LIGHT_LEVEL (1008:d672, line 8088)
level = room.light (+0x468)
      + GET_USER_ABILITY_VALUE(Illu=0xd, viewer)      // personal
      + SUM over every player in room: player+0xcc (lit-item light)
          + player+0x6ad (dynamic: RoomIllu from spells/equipment)
cap at 900; BlindingLight(0x35) added AFTER cap (ORACLE-OPEN, skip).
Bands (fenceposts -0xc9/-0x97/-0x65/-1/199/899; strings 0xBDFD6..0xBE01B):
pitch black <= -201 | very dark -200..-151 | barely visible -150..-101 |
dimly lit -100..-1 | Regular Light 0..199 | Daylight 200..899 | >=900 ORACLE-OPEN.

### _CAN_SEE (line 66340) — THE RULE
level < -150 (-0x96): emit "The room is %s - you can't see anything"
(0xDF37E, NO trailing period) and refuse; level < 901 → visible;
>= 901 too-bright branch ORACLE-OPEN.
Gated callers (complete list): _DISPLAY_ROOM_DESC (34796),
_CMD_LOOK (49279), _CMD_SEARCH (51201), _CMD_HIDE (63822),
exits cmd (_HANDLE_COMMANDS case 0x49, 15351).
Movement/attack/get NOT gated (dark rooms are walked blind).

### _CMD_LIGHT (10b0:7071, line 63864), in order
no args → "The current light level is %s" (0xDB4B8);
not found → fall through to say-aloud (ORACLE-OPEN);
found-elsewhere/uses==0&type6 → "You must recharge that before you may light it again." (0xDB568);
USER_CAN_USE fail → "You may not light that item!" (0xDB4EE);
type != 6 → "You cannot light %s!" (0xDB550);
already lit (char+0x6ab != -1) → "You already have something lit!" (0xDB50C);
success → lit slot = inv index, IlluTarget(54) added to char+0xcc,
"You lit the %s." (0xDB52D), room "%s lights %s %s." (0xDB53E).

### Burn — _MEDIUM_UPDATE_CHARACTER (1028:224d, lines 19368-19427)
1 use per 3s medium tick unconditionally while lit (mud-core
Job::Upkeep, UPKEEP_INTERVAL=3, game.rs:1104). At 0:
- distructmsg present (torch→8603 "Your torch flickers and goes out.",
  lamps→8604, moon-lamp→1995): line1 to user, line2 (if any) to room;
- else generic pair: "%s is no longer lit!" (0xC781E) + "It's uses
  gone, %s disappears from your inventory!" (0xBD121; order
  ORACLE-OPEN) ; room "%s's %s just went out." (0xC7834);
- retainafteruses==0 (all five shipped lights) → destroy; clear lit.

### Extinguish
remove <lit item> (_REMOVE_ARMOUR type-6 branch, 5835-5854): clear lit,
subtract IlluTarget, "%s is no longer lit!" + room "%s's %s just went
out.". remove of unlit type-6 → refusal at 0x19d3 (wording unrecovered,
ORACLE-VERIFY placeholder). Any other inventory removal clears lit
SILENTLY (_REMOVE_ITEM_FROM_INVENTORY 4494-4497).

### Starlight = spell 26 (mmud_wgnt.sqlite)
star/starlight, mana 4, level 1, spelltype 3, match target 1,
duration 80 + durincrease(1,1) → 81 ticks at L1 (~4 min at 3s tick),
levelcap 32, min=max=175, castmsgb 8249 ("You cast %s!"/"%s casts %s!"),
abilities (RoomIllu=14, 0 → rolled 175) + (DescMsg=115, 2092).
Message 2092: line1 "Your starlight spell fades away." (wear-off via
existing terminate_active_spell, game.rs:8433), line3 "You are
surrounded by a shimmering light!" (cast-time, existing path 8401).
RoomIllu→+0x6ad is INFERRED (jump table 39598 unrecoverable) — comment.
quest_ability_value (game.rs:3627) already implements value-0 →
stored-slot magnitude, so (RoomIllu,0) yields 175 for free.

### Light items (item table)
175 torch Illu 100 uses 800 msg 8603 | 176 lantern 175/2400/8604 |
286 brass lamp 175/1800/8604 | 1153 moon-lamp 200/4000/1995 |
1233 scaled lantern 200/6000/0(generic). All type 6, retainafteruses 0.
17,432 rooms light<0; 1,443 <= -201.

## TDD slices
1. Content: Room.light i16, Item.destruct_msg Option<MessageId>;
   content_db.rs SELECT additions. Tests in mud-server
   tests/load_real_db.rs: small_cavern_loads_its_light_value (-200),
   the_torch_loads_its_destruct_message (8603).
2. light_level(viewer) + check_can_see mirroring _CAN_SEE; gate
   render_room (16643, preamble still emitted), show_exits_line
   (16616), search (10238), hide/stash (10087/10914). New
   crates/mud-core/tests/darkness.rs: too-dark line exact w/ band
   descriptor, arrival blind (leave/arrive broadcasts still fire),
   hit-enter dark, pitch-black at -201, fencepost -150 renders /
   -151 dark, exits/search/hide refused, light-0 unaffected.
3. Player.lit Option<ItemId> (self-healing: lit id gone from inv →
   clear silently); command.rs ("light",5,WithArgs); light_command
   ladder per DLL order (not-found → FallThrough); remove_command
   type-6 pre-branch; state_db lit column. Tests: lighting lights the
   dark room ("You lit the torch." + room line + look renders),
   no-arg level report, second light refused, non-light refused,
   uses-0 recharge line, remove extinguishes + dark again, save/load
   roundtrip.
4. Burn in upkeep_update: decrement while lit; at 0 emit per rules,
   destroy unless retain. Tests: 1 use per tick, unlit never burns,
   flicker-out message path, generic-pair path (the client fixture
   shape), retained item survives at 0.
5. Starlight e2e: fixture spell mirroring record 26 + msgs 8249/2092.
   Tests: cast lights dark room (175-200=-25 visible), fades after 81
   ticks + fade line + dark again, lights room for a second spell-less
   player, fizzle charges half mana, recast mid-glow refreshes.
6. text.rs constants (all VERIFIED w/ offsets above); colors
   ORACLE-OPEN (emit plain like NO_EXIT).

## Client fixture recipe (for later client-side dark tests)
room.light = -200; Item{id 175, type 6, uses 3, (IlluTarget,100),
destruct_msg None}; messages 8249 + 2092; spell 26 mirror; torch in
inventory, starlight in spellbook.

## Existing tests
None should move (light defaults 0; gate fires < -150). Watch: free-text
"light ..." lines (none found), load_real_db room assertions.

## Verification
cargo test --workspace per slice (baseline ~1381 at 9cbee66; wire layer
untouched). Oracle wishlist (marked in code): burn-out pair order,
unlit-remove refusal wording, light abbreviation, dark-line ANSI, no-arg
band fenceposts — one Small Cavern torch capture settles most.
