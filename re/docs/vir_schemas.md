# MajorMUD 1.11p — `.VIR` data file schemas

The `.VIR` files are **Btrieve 6.x** fixed-length-record databases. Reader:
`re/vir.py` (validated against RE-known field offsets). This documents the container
format, a catalog of all files, and the field maps decoded so far.

## Container format

- **FCR** at page 0 (offset 0): page size @`+0x08` (word), physical record length
  @`+0x16`, logical length @`+0x18`.
- Pages 0..k are FCR + index pages; **data pages** follow.
- Each data page = **8-byte header** then records packed at `rec_len` stride.
  Records **never cross a page boundary**, so a page holds
  `floor((page_size - 6) / rec_len)` records (often just 1 when two won't fit,
  which is why e.g. WCCSPELS uses one 253-byte record per 512-byte page).
- Record N of a data page begins at **`page_off + 6 + N*rec_len`** (6-byte header).

**FRAME CORRECTION (important):** the data-page header is **6 bytes**, not 8. Verified:
at `page+6` the record's Number field (offset 0) reads sequential ids (1,2,3,…) for every
file; at `page+8` it is garbage. Earlier work in this doc used `page+8`, which landed 2
bytes into each record — right on the Name field (offset 2), so **names decoded fine but
all numeric-field offsets were shifted +2 and several were mislabeled.** Every offset
below is now in the correct `page+6` frame. `re/vir.py` uses `PAGE_HDR=6`.

**Cross-check vs Nightmare-Redux (`modFieldmaps.bas`):** Nightmare's stock field maps are
for a **newer MajorMUD build** — its record buffers are larger than 1.11p's
(RACE +0, CLASS +3, SPELL +7, ITEM +9, ROOM +16, MONSTER +24 bytes). Fields in the shared
**common prefix** align and are validated below; **later fields drift** and need
1.11p-specific offsets (Nightmare supports version selection but its constants here don't
match 1.11p sizes). Net: RACE, ITEM (name/type/damage), and ROOM (name/exits) are
confirmed for 1.11p; SPELL (past ~0xa2), CLASS (HP fields), and MONSTER (attack block)
are **not** — flagged per-section.

**Combat-code vs disk caveat still stands:** engine in-memory structs (`_GET_ITEM_DATA`
etc.) are not identical to disk records; keep the two maps distinct.

## File catalog

| file | rec len | page | records | content |
|------|--------:|-----:|--------:|---------|
| WCCRACE  | 126  | 512  | 13     | **Races** (Human … Gaunt One) |
| WCCCLASS | 153  | 1536 | 14     | **Classes** (Warrior … Ranger) |
| WCCSPELS | 253  | 512  | 689    | **Spells** (name @+0, one/page) |
| WCCITEMS | 1061 | 1536 | 2148   | **Items** (name @+0xab, one/page) |
| WCCKNMSR | 732  | 1536 | 1269   | **Monster definitions** (name @+0x34, 2/page) |
| WCCMP001 | 1528 | 1536 | 29231  | **World map / rooms** (room name @+0x103, one/page) |
| WCCSHOPS | 500  | 512  | 210    | **Shops** (name @+0x02) |
| WCCACTS  | 1010 | 1024 | 68     | **Social actions / emotes** (nod, hug, kiss) |
| WCCMSG   | 253  | 512  | 2188   | **Game message strings** (combat/verb text, printf-style) |
| WCCTEXT  | 22   | 2048 | 66775  | Tiny records — link/index table (exits? text refs) |
| WCCBANKS | 72   | 512  | 1      | Bank config |
| WCCGANGS | 253  | 512  | 2      | Gang/guild config |
| WCCITOWN | 65   | 1536 | 2      | Item-town / spawn config |
| WCCACMSR | 386  | 1536 | 1      | Action-master config |
| WCCUSERS | 1998 | 2048 | 3      | Player records (name not at fixed low offset) |
| NEWMP001 | 1544 | 4096 | 8      | Newer map variant (patch) |

Counts are populated-slot scans; a few include header/placeholder rows
(e.g. WCCSHOPS row 0 = "Leave this blank", WCCMSG has empty leading slots).

## Field maps (decoded)

### WCCRACE (126 B) — CORRECTED via Nightmare + page+6 frame (validated)
- `+0x00` word = race **Number** (1..13).
- `+0x02` (29 B) = **name**.
- `+0x20..+0x2a` six **min stats** (words), order **Int, Wil, Str, Hea, Agl, Chm**.
  Validated: Human all 40; Dwarf 30/50/50/50/30/30; Half-Ogre Str 70, Hea 60.
- `+0x2c` = HP bonus. `+0x46` = starting CP (=100 for playable races). `+0x66..` = max stats.
- (Prior version of this doc read stats at the wrong frame with the wrong order — fixed.)

### WCCCLASS (153 B) — partial (record is 3 B smaller than Nightmare's; late fields drift)
- `+0x00` Number, `+0x02` name. `+0x24` = **Exp %** penalty (Witchunter 30, Paladin/Cleric 120).
- HP fields near `+0x20/+0x22` don't cleanly match Nightmare's newer map — need 1.11p offsets.

### WCCSPELS (253 B) — NEEDS 1.11p offsets (record 7 B smaller; fields drift past ~0xa2)
- `+0x00` Number, `+0x02` name (both validated).
- The ability-table / level / mana offsets I derived earlier were in the wrong (page+6-vs-8)
  frame **and** Nightmare's newer-version offsets (SLevelCap@0xa2, SMana@0xf0) don't align
  for 1.11p (magic missile shows mana 0). **Re-derive 1.11p spell offsets** — earlier
  `+0xbb reqlevel` / `+0xd3 ability-ids` claims are NOT reliable.

**Design finding still holds:** spell mechanics are **data-driven via a shared ability
system** — the `(id → value)` scheme backs `_GET_SPELL_ABILITY_VALUE`,
`_GET_MONSTER_ABILITY_VALUE`, `_GET_USER_ABILITY_VALUE`. Individual ability-id meanings TBD.

### WCCITEMS (1061 B) — CORRECTED via Nightmare (common prefix validated)
- `+0x00` word = item **Number**.
- `+0xad` (29 B) = **name**; description text inline after.
- `+0x2f4` word = **item Type** (weapon=1, armor=0, …). *(Corrects earlier "type@+0x02".)*
- `+0x33e` word = **min damage**, `+0x340` word = **max damage** — both validated:
  dagger 1–5, mace 3–12, quarterstaff 2–12, armor (grey robes) 0/0.
  *(Corrects the earlier single mis-framed "damage" field and the +0x340 "dual-use" note.)*
- Armor-class value and the `+9 B` of newer-version fields are past the common prefix — TBD.

### WCCKNMSR (732 B) — monster definitions — DISK-VERIFIED
Record = raw Btrieve record (same cache/`obtbtvl` path as spells). The combat offsets
ARE disk offsets (earlier "exceeds 732" worry was a mis-attribution — the `+0x404/+0x408`
message refs belong to the wielded-**item** record, not the monster). Per-attack fields
are parallel arrays indexed by attack number `p` (word stride 2):
- `+0x34` = monster **name**; description text inline after.
- `+0x6e` (byte) = stat used in backstab-mode level scaling.
- `+0x123 + p*2` = attack **accuracy** — verified: giant rat 70, lashworm 50.
- `+0x132 + p*2` = attack **min damage** — verified: giant rat 4, lashworm 1.
- `+0x13c + p*2` = attack **max damage** — verified: giant rat 20, lashworm 5.
- `+0x182 + p*2` = attack **message id** (giant rat = 1000).

Monster stats/hp/exp/level and ability entries (via `_GET_MONSTER_ABILITY_VALUE`,
the shared ability system) occupy other offsets — full map TBD.

> **Disk == memory CONFIRMED (slice M6-1, 2026-07-18).**
> `load_known_monster_into_buffer` (WG3-NT decompile 29915) `dfaAcqLock`s the raw
> Btrieve record straight into the template cache slot and
> `save_known_monster_from_buffer` writes the same buffer back — no repacking. So
> every knmsr offset in `monsters.md` is a disk offset, and the Nightmare
> `MonsterRecType` column layout maps the **WG3-NT** logical record 1:1 (the sqlite
> import already used it; M5's combat-field pins validated the prefix). The old
> "24 B smaller" warning applies to the **DOS 1.11p** file only — the oracle's data,
> not the engine's. Spawn/behaviour columns (disk-verified via the `generate_monster`
> copy map, monsters.md §2): `group`@0x54 roam/zone class + mongen region,
> `index`@0x5c level, `something3`@0x6c herd id, `follow`@0x6e aggression 0-100,
> `hitpoints`@0x78 (word — current AND max HP at spawn), `hpregen`@0x7c,
> `gamelimit`@0xa6 / `active`@0xa8 population pair, `type`@0xaa herd/leash mode,
> `nothing2`@0xac follower cap (byte), `alignment`@0xae behaviour mode,
> `regentime`@0xb2 unique-respawn cooldown (×60 min), `datekilled`/`timekilled`
> @0xb4/0xb6, `movemsg`@0xb8 arrival message, `deathmsg`@0xbc. Template **name is
> @0x36** (not 0x34); name-generator id dword @0x124. `expmulti`@0x58 doubles as the
> pack herd rank.

### WCCMP001 (1528 B) — world map / rooms — largely mapped
Record = raw Btrieve record. Layout:
- `+0x02`, `+0x08` = small flags (room type; `+0x08`=14 common).
- `+0x103` = **room name** (Town Gates, Mountain Stair, …). 29231 rooms.
- `+0x138` = **description** text (inline ANSI colour codes embedded).
- `+0x329` = **ANSI art filename** for the room (e.g. `WCCMAP01.ANS`).

**Room address key — SOLVED** (via the open-source Nightmare-Redux field map,
`github.com/syntax53/Nightmare-Redux`, `modFieldmaps.bas`). Key = **(MapNumber,
RoomNumber)**, a 2-field Btrieve composite. NOTE the record data starts at **page+6**
for this file (6-byte data-page header), not page+8 — everything below is relative to
that. Authoritative layout (fields from offset 0):

| off | field | | off | field |
|-----|-------|-|-----|-------|
| 0   | MapNumber (Long) | | 824 | RoomExit[10] (Long) — dest room# |
| 4   | RoomNumber (Long) | | 864 | RoomType[10] (Int) — exit type |
| 8   | (empty, 253 B) | | 884 | Para1[10] (Long) — dest map if type 8 |
| 261 | Name (53) | | 924 | Para2[10] (Int) |
| 314 | Desc[7] (71 each) | | 944 | Para3[10] (Long) |
| 811 | AnsiMap (13) | | 984 | Para4[10] (Long) |

Exit `d`: destination room = `RoomExit[d]` (>0); destination map = `Para1[d]` when
`RoomType[d]==8` (map-change portal), else the room's own `MapNumber`. Special
`RoomType`s (from Nightmare): 6 = hidden, 8 = map change, 12 = remote action.

**Not keyed on board serial / install key** (a hypothesis worth ruling out): Nightmare
reads any board's files portably; monsters/items/spells key on a plain integer number,
rooms on (map, room). Per-board registration (`bturno`) gates the running game only.

**Direction order — CONFIRMED** from the direction-name string table and the
`_DISPLAY_VISIBLE_EXITS` switch order (index `d`):

| d | dir | d | dir | d | dir |
|---|-----|---|-----|---|-----|
| 0 | North | 4 | Northeast | 8 | Up |
| 1 | South | 5 | Northwest | 9 | Down |
| 2 | East  | 6 | Southeast | | |
| 3 | West  | 7 | Southwest | | |

Opposite pairs: N↔S (0,1), E↔W (2,3), NE↔SW (4,7), NW↔SE (5,6), U↔D (8,9).

**Exit table**, per direction `d` (offsets from `_VISIBLE_EXIT`/`_DISPLAY_VISIBLE_EXITS`):
- `+0x338 + d*4` = **RoomExit[d]**, a single 4-byte Long = destination **room number**
  (0 ⇒ no exit). Destination **map** = `Para1[d]` (`+0x374 + d*4`) when the type is a
  map-change, else the room's own MapNumber. (These code offsets equal the disk offsets
  824/884 in the page+6 frame.) An earlier "high word / low word, 97% reciprocity" note
  was **wrong** — it split the Long into two words and used a collision-collapsed key;
  the true graph resolves 99.9% of exits with 98% opposite-direction reciprocity.
- `+0x360 + d*2` = exit **type** — CONFIRMED from door/gate/secret strings:
  0 = open, 2 = door, 6 = gate, 7 = secret passage (9/0xc/0x10/0xb = further
  hidden/locked variants gated by `_SECRET_VISIBLE_EXIT`).
- `+0x374 + d*4` = exit **flags** (byte; bits 4/8 = secret-door found/open state).
- `+0x39c + d*2` = door **state** (word; type-2/9 doors — open vs closed).
- `+0x3b0 + d*4` = **key/lock** field (type-0x10 locked exits require a key item).
- `+0x3d8 + d*4` = secret-passage **message** id (per direction).
- `+0x4ae..` / `+0x50c..` = `0xFFFF`/`0xFFFE` sentinel runs (empty list slots).

**Room spawn-control block — disk-verified (slice M6-1, 2026-07-18).**
`load_room_into_buffer`/`save_room_from_buffer` (decompile 29372/29326) move the raw
record with no repacking, so the in-memory offsets `monsters.md` §1 documents are disk
offsets, and they line up with the Nightmare `RoomRecType` columns exactly:
`currentroommon[15]`@0x400 live-monster list, `type`@0x43c spawn type (dual-use: 1 =
shop-active), `minindex`/`maxindex`@0x462/0x464 spawn level band, **forced monster =
the u4 @0x468** (Nightmare's `bynumber` Long@0x466 straddles it by two bytes — the id
is that column's high word), `maxregen`@0x55c spawn cap (1-15), `monstertype`@0x560
spawn zone / wander leash key, `unknown69`@0x562 respawn timer (runtime, 0 on disk),
`attributes`@0x564 flags (bit 8 boss-present, bit 2 water, bit 1 safe), `delay`@0x5bc
respawn-delay minutes, `maxarea`@0x5be linked-room cap, u2@0x5c0 linked live count
(runtime), `controlroom`@0x5c4 linked spawn room, `permnpc`@0x5c8 boss/unique id,
`nummons`@0x606 current spawn count (runtime, 0 on disk). Item drops: `roomitems`/
`roomitemqty`/`placeditems` columns (already loaded since M4).

## Next
- Disk-verify KNMSR and MP001 field maps by reading `_GET_KNOWN_MONSTER_DATA`,
  `_GET_ROOM_DATA`, `_GET_SPELL_DATA` accessors and cross-checking on disk.
- Identify WCCTEXT's 22-byte record structure (exits table is the likely candidate).
