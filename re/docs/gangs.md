# MajorMUD (WG3-NT) — Gang / Guild-house system spec

Behavioral spec derived from the 32-bit WG3-NT decompile
(`wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Cross-references `economy.md`
(gang-house / gang shops, `deposit_gangleaders_account`), `vir_schemas.md`
(WCCGANGS record, WCCMP001 room record), and `abilities.md` (GHouse abilities
`GHouseDeed` 181 / `GHouseTax` 182 / `GHouseItem` 183). "Gang" and "guild" are the
same feature — the parser accepts both keywords (`cmd_create` matches `"gang"` or
`"guild"`). Offsets are byte offsets into the gang / player / room / shop records.

Functions read: `get_gang_data` (0x3390a), `mem_get_gang`, `save_gang_from_buffer`,
`load_gang_into_buffer`, `join_gang` (0x4fef8), `invite_to_gang` (0x4fa7c),
`clear_gang_invitations` (0x4fb55), `remove_from_gang` (0x4fc2f),
`remove_offline_user_from_gang` (0x4fd25), `display_gang_members` (0x3c145),
`display_online_gang_members` (0x3bf6f), `display_top_gangs` (0x339dc),
`display_top_gang_header`, `display_a_top_gang` (0x34314),
`deposit_gangleaders_account` (0x73680), `tell_gang` (0x6ae71),
`cmd_broadgang` (0x585cc), `cmd_create` (0x57d4a), `cmd_disband` (0x58725),
`cmd_leave` (0x53cba), `cmd_join` (0x541fb), `cmd_invite`/`cmd_uninvite`,
`cmd_promote` (0x5adb4)/`cmd_demote`, `buy_item` GHouse/GShop branch (~0x1bd..),
the login/refresh handler (~0x28xx, gang-notification block at line 10200), the
per-kill exp-award block (line 11154), `display_room_desc` (0x36f5d),
`display_LONG_room_desc` (0x37774), `display_desc_from_file` (0x39598),
`display_LONG_text` (0x3bcca), `get_text_block` (0x3379c).

---

## 0. Record fields this spec relies on

### GANG record — WCCGANG2.DAT (runtime), 256-B physical (`0x100`), WCCGANGS on disk

Opened as `DAT_00479144 = dfaOpen("WCCGANG2.DAT", 0x100, 0)`. Layout confirmed from
`cmd_create` (which builds a fresh record byte-by-byte) and the display/membership
functions:

| off | width | field |
|-----|-------|-------|
| `+0x00` | 20 B | **uppercase name KEY** — Btrieve key 0; `get_gang_data` uppercases input and matches here |
| `+0x14` | 20 B | **display name** (mixed-case, ≤19 chars) — copied into `player+0x6c8` on join/create |
| `+0x28` | long | **experience pool** (primary) — the deed gate; shown as the "Exp" column |
| `+0x2c` | ~20 B | **leader name** (creator's `player+0x1e`) — sole ownership key |
| `+0x4a` | word | **creation date** (`today()`), rendered by `ncedat` in the top-gang list |
| `+0x4e` | word | **member count** (initialised to 1 at create; ++ on join, -- on leave/remove) |
| `+0x50` | dword | **flags**: bit `0x1` = **disbanded**; bit `0x8` = **exp-pool saturated / "secondary" active** |
| `+0x54` | byte | **dirty flag** (record needs saving; `save_gang_from_buffer` writes only if set) |
| `+0x58` | long | **secondary experience pool** — overflow catch once `+0x28` saturates |
| `+0x5c` | word | **overflow-wrap counter** ("×N times") for `+0x58` |

### PLAYER record — gang fields

| off | width | field |
|-----|-------|-------|
| `+0x1e` | str | player name (compared against `gang+0x2c` to test leadership) |
| `+0x6c8` | 20 B | **current gang** = the gang's display name (`gang+0x14`); empty ⇒ not in a gang |
| `+0x470/+0x474` | long,long | **experience** (hi/lo pair) — `≥100000` gates gang creation |
| `+0x7d4` | word | **channel/rank bitmask** (low byte `+0x7d4`, high byte `+0x7d5`) — see below |

**`+0x7d4` / `+0x7d5` gang bits** (the two bytes form one 16-bit word; `0x0100 =
+0x7d5 bit 0`, `0x0200 = +0x7d5 bit 1`, …):

| bit | meaning |
|-----|---------|
| `0x0100` (`+0x7d5 & 0x01`) | **Lieutenant** rank (persistent) |
| `0x0200` (`+0x7d5 & 0x02`) | pending **promote → Lieutenant** (set on an *offline* target; applied at login) |
| `0x0400` (`+0x7d5 & 0x04`) | pending **demote** (applied at login) |
| `0x0800` (`+0x7d5 & 0x08`) | pending "your **ganghouse has been closed**" notice |
| `0x1000` (`+0x7d5 & 0x10`) | pending "gang-house **items have disappeared**" notice |
| `0x2000` (`+0x7d5 & 0x20`) | (non-gang) exp/character flag — unrelated |

The login/refresh handler (line ~10200) is the deferred-action processor: it reads
these pending bits, mutates the Lieutenant bit `0x0100`, prints the matching
message, and self-clears the pending bit.

### ROOM record — WCCMP001, gang-house fields (in-memory struct from `get_room_data`)

The description block is `Desc[8]`, 71-byte paragraphs starting at `+0x13a`
(matches the disk `Desc[7]@0x13a`, 71 each, per `vir_schemas.md`). Gang-house
control fields (named from the sysop room-status dump around line 36395):

| off (mem) | field |
|-----|-------|
| `+0x105` | room **name** |
| `+0x13a` | **Desc[0]** — first description paragraph (**sentinel slot**, see §4) |
| `+0x181` | **Desc[1]** — second paragraph (**description-file NAME slot**, see §4) |
| `+0x1c8 … +0x32b` | Desc[2..8] — further paragraphs |
| `+0x43c` / `+0x440` | is-shop flag (word) / shop id (long) |
| `+0x444` | gang-house **owner** name (compared to `player+0x1e` in the create-exit path) |
| `+0x564` | room-flags byte: **bit `0x40` = Ganghouse**, `0x1` protected, `0x2` patrollable, `0x4` build-permitted |
| `+0x5c4` | **controlling room** number (`piVar4[0x171]`) |
| `+0x5c8` | specific-monster number (`piVar4[0x172]`) |

> On disk the same 71-byte `Desc[]` block begins at record offset `0x13a`; the
> memory and disk layouts agree for the description paragraphs, so §4's extraction
> recipe uses the disk offsets directly. (The `GangHouseNumber @ room+0x62` cited in
> the task's room-schema note was **not** found at `+0x62` in this build's in-memory
> struct — the gang-house linkage here is the `+0x564` bit-`0x40` flag plus the
> `+0x5c4` controlling-room pointer; see §6 Undetermined.)

---

## 1. Membership

### 1.1 Forming a gang — `cmd_create`

`CREATE GANG <name>` (or `CREATE GUILD <name>`). Gates:

- Player must have **experience `≥ 100000`** (`player+0x474 >= 100000 || +0x470 != 0`).
- Player must **not already be in a gang** (`player+0x6c8` empty).
- Name **< 20 chars**, printable ASCII only (0x20–0x7e), and not the literal `"None"`.
- Name must be unique (`get_gang_data` returns nothing; `dfaInsertDup` succeeds).

On success the record is initialised: `+0x00` = uppercased key, `+0x14` = display
name, `+0x28` exp = 0, `+0x2c` = creator name, `+0x4a` = `today()`, `+0x4e` member
count = **1**, `+0x50` flags = 0. The creator's `player+0x6c8` is set to the gang
name — **the creator becomes the leader** implicitly (leadership is defined purely
by `gang+0x2c == player+0x1e`; there is no separate leader flag on the gang).

### 1.2 Invitations — `invite_to_gang` / `clear_gang_invitations`

Invitations live in a **global in-memory linked list** (head `DAT_00488574`, tail
`DAT_00488578`), *not* on the gang record. Each 28-byte node = `{ invited user# @0,
gang name @+4 (19 B), next @+0x18 }`. `invite_to_gang(usernum, gangname)` appends a
node (deduping identical (user, gang) pairs). Nothing here caps membership.

Commands: `INVITE <name>` / `INVITE MEMBER <name>` (`cmd_invite`) — allowed to the
**leader or a Lieutenant** (`gang+0x2c==name || player+0x7d5 & 1`); it resolves the
target and calls `invite_to_gang`. `UNINVITE`/`UNINVITE MEMBER` (`cmd_uninvite`)
removes members (see §1.4). `clear_gang_invitations(usernum, gangname)` unlinks
matching pending invites (called after a successful join and on removal).

### 1.3 Joining — `join_gang` (via `cmd_join`: `JOIN GANG <name>`)

`join_gang(usernum, gangname)` walks the invitation list for a node matching
`(usernum, gangname)`. If found: `gang+0x4e`++ (member count), gang marked dirty,
`player+0x6c8` = `gang+0x14`, prints "You have joined the gang %s", and announces
"%s just joined your gang" to the gang via `tell_gang`. No match ⇒ "You have not
been invited…". `cmd_join` refuses if the player is already in a gang
("You may not join another gang"). **No member-count cap is enforced anywhere** —
`+0x4e` is a bookkeeping counter only.

### 1.4 Leaving / removal

- `cmd_leave` (`LEAVE GANG`): a **non-leader** leaves via `remove_from_gang`; the
  **leader may not leave** ("You are the leader… you may not"), and must `DISBAND`.
- `remove_from_gang(usernum, gangname)` — clears the online player's `+0x6c8`,
  clears the Lieutenant bit (`+0x7d4 & ~0x100`), decrements `gang+0x4e`, marks
  dirty, clears their invitations. **If the departing player is the leader**
  (`player+0x1e == gang+0x2c`), it sets `gang+0x50 |= 1` (**disbanded**) and sweeps
  every online terminal, ejecting all members whose `+0x6c8` matches this gang.
- `remove_offline_user_from_gang(name, gangname)` — the offline equivalent: loads
  the target's WCCUSERS record by name, clears their membership fields, decrements
  the count, and writes the player record back (`dfaInsertDup`/`dfaUpdateDup`). Used
  by `UNINVITE MEMBER <name>` against offline members.
- `cmd_uninvite` role rules: leader **or** Lieutenant may remove ordinary members;
  a **Lieutenant cannot remove another Lieutenant or the leader** (only the leader
  can — "You must be the gang leader to…"). Ejected member gets "%s has exiled you
  from %s".

### 1.5 Ranks / roles

Three roles, no explicit rank field on the gang record:

1. **Leader** — the single player whose name equals `gang+0x2c`. Can disband,
   promote/demote, invite, and remove anyone.
2. **Lieutenant** — `player+0x7d4 & 0x100` (`+0x7d5 & 0x01`). Can invite and remove
   ordinary members. Set/cleared by the leader (`cmd_promote`/`cmd_demote`):
   - **online** target → bit `0x100` toggled immediately;
   - **offline** target → pending bit `0x200` (promote) or `0x400` (demote) written
     to their saved record, applied at their next login by the refresh handler.
   Leader may not promote the leader or self-demote.
3. **Member** — in the gang (`+0x6c8` set), neither leader nor Lieutenant.

### 1.6 Roster display — `cmd_broadgang` with no argument

`GANG` / `GUILD` with no message shows the roster. With `player+0x7d5 & 8` clear it
calls `display_gang_members` (all members, paged via the WCCUSERS scan in
`FUN_0043c2a2`, showing `(Online)`/offline and the disbanded banner if
`gang+0x50 & 1`); otherwise `display_online_gang_members` (currently-online only).
Both print the **Leader** first, then **Lieutenants** (`+0x7d5 & 1`), then ordinary
members (those with `+0x7d4 & 0x100` clear).

---

## 2. Gang economy / experience pool & tax

There are **two distinct "banks":** a shared **experience pool** on the gang record,
and a **gold account** in a bankbook.

### 2.1 The gang experience pool (`gang+0x28` / `+0x58` / `+0x5c`)

Every time a **gang member** is awarded combat experience (award block, line 11154:
`player+0x6c8` non-empty and the gang loads), the same amount is **also added to the
gang's experience pool**:

- While `gang+0x50 & 8 == 0`: `gang+0x28 += award`. On 32-bit overflow, `+0x28`
  saturates to `0xFFFFFFFF`, the excess rolls into `+0x58`, and `gang+0x50 |= 8`.
- Once saturated (`+0x50 & 8`): accumulation continues in `+0x58`, with `+0x5c`
  counting each further wrap.

This pool is the currency for **buying a guild-house deed** (§3). It is *not* gold —
members "fund" the gang by adventuring.

### 2.2 The gang gold account — `deposit_gangleaders_account`

`deposit_gangleaders_account(shopPtr, amount)` is called from `buy_item` only for a
**GHouse shop** (`shop+0xcc == 0xb`) after a successful purchase. It looks up a
**bankbook** via `get_bankbook_data(shop+0x128, 8)` — i.e. **bank id 8** (the gang
bank), keyed by a **name string stored in the shop record at `shop+0x128`** (for a
GHouse shop this slot is repurposed from the normal max-stock array to hold the
account-holder name). It adds `amount` copper to `bankbook+0x24` (with a
`balance+amount > balance` overflow guard), marks the bankbook dirty, prints
`GANGSHOP DEPOSIT: <name> -> <n> copper`, and flushes all bankbooks. So the **gold
price** a member pays for a gang-house item is deposited into the gang's bank-8
account; the deed's **experience price** is charged against the pool in §2.1.

### 2.3 Gang-house tax — `GHouseTax` (182 / 0xb5-adjacent abilities)

`GHouseDeed` (181), `GHouseTax` (182) and `GHouseItem` (183) are **item abilities**
carried by the special gang-house merchandise (per `abilities.md`; all flagged
engine-referenced). They are the data that marks a shop item as a *deed*, a *taxed
item*, or a *house furnishing*, consumed by the GHouse (`0xb`) branch of `buy_item`
rather than applied to the player. `BadAttk` (185 / 0xb5) is the "already owns a gang
item" sentinel `buy_item` scans the buyer's inventory for. (The precise per-item tax
arithmetic lives in the GHouse pricing block of `buy_item`; the deposit half is §2.2.)

---

## 3. Guild houses

### 3.1 Buying a deed / house item — `buy_item`, shop types `0xb`/`0xc`

Per `economy.md`, gang-house commerce uses two shop types that reprice from the shop
record (restocking disabled, pricing fields overloaded):

- **GHouse (`shop+0xcc == 0xb`)** — deed/furnishing shop. To buy, the player must be
  a **gang leader** (`gang+0x2c == player+0x1e`), the **gang experience pool**
  `gang+0x28` (or the saturated path via `+0x50 & 8`) must reach
  `DAT_00482d10 * 10000`, and the buyer must not already own a gang item (no carried
  item with ability `BadAttk` 0xb5). Price = `shop+0x178[i]` in denomination
  `shop+0x1a0[i]`, then run through the standard Charm/markup buy formula. On success
  the stock slot is retired and the paid gold is routed to the gang's bank-8 account
  (`deposit_gangleaders_account`, §2.2). Refusal codes: `4`=already a gang owner,
  `5`=gang exp insufficient, `6`=must be gang leader, `7`=outstanding paperwork.
- **GShop (`shop+0xcc == 0xc`)** — gang shop; same leader gating, sets a
  pending-paperwork flag (`player+0x7d5 & 0x40` blocks further purchase). Gang shops
  **buy nothing** (`sell_item`: "You may not sell items to a gang shop").

### 3.2 Room ↔ gang-house link

A room is a gang house when `room+0x564 & 0x40` (the **Ganghouse** flag, printed as
"Ganghouse" in the sysop room dump). The house's **controlling room** number is
`room+0x5c4`. The house's shop (deed/furnishing vendor) is the room's own shop
(`room+0x43c==1`, id `room+0x440`) configured as type `0xb`. The `CREATE <direction>`
build path checks `room+0x564 & 4` (build-permitted) and `room+0x444 == player`
(owner) — but every arm of that path currently prints the same "If you are a gang
leader you may…" stub, so in-game room *construction* appears disabled in this build.

Gang-house lifecycle notices reach members through the deferred `player+0x7d5` bits:
`0x08` = "Your ganghouse has been closed down", `0x10` = "Gang house items have
disappeared" (both self-clearing at login).

---

## 4. ★ Custom room descriptions → external description FILE (the linkage)

**Finding: in this build a gang-house room's player-visible custom description is
NOT stored as a textblock id in a `.dat`. It is stored in an external DOS text file,
named by a filename written into the room's own description block.** The linkage is
the `FILE_DESCRIPTION` sentinel mechanism, and gang houses are its headline user
(the error string is literally `"Can't display gang house file %s"`).

### 4.1 The mechanism (`display_room_desc`, `display_LONG_room_desc`, `display_desc_from_file`)

Room description rendering checks:

```
if (sameas(room + 0x13a, "FILE_DESCRIPTION") && room[0x181] != '\0')
    display_desc_from_file(room + 0x181, room + 0x1c8, ...);   // read external file
else
    print the inline Desc[] paragraphs;
```

- **`room + 0x13a` = Desc[0]** holds the literal sentinel string
  `"FILE_DESCRIPTION"`.
- **`room + 0x181` = Desc[1]** (the second 71-byte paragraph) holds the **filename**
  of the external description text file.
- **`room + 0x1c8` = Desc[2]** is passed as the section/prefix selector into
  `display_desc_from_file`.

`display_desc_from_file(nameField, section, …)` opens that file with **`tfsopn`**
(the BBS text-file service, i.e. an ordinary on-disk text file) and streams its
lines; on failure it raises `internal_error("Can't display gang house file %s")`.
The same sentinel+filename convention is reused for item descriptions
(`display_item_desc`: `item+0xcb`=="FILE_DESCRIPTION", filename at `item+0x108`).

### 4.2 Exact field that holds the description pointer (for extraction)

**On the ROOM record (WCCMP001), keyed by `(MapNumber, RoomNumber)`:**

- `Desc[0]` at record offset **`0x13a`** — must equal the ASCII string
  `"FILE_DESCRIPTION"` for the file mechanism to engage.
- `Desc[1]` at record offset **`0x181`** (= `0x13a + 71`) — the **description
  filename** (a DOS text-file name). This is the field to read to recover a gang
  house's custom description.

Extraction recipe: scan WCCMP001 for rooms whose `Desc[0]@0x13a == "FILE_DESCRIPTION"`
(optionally filter to rooms flagged Ganghouse), read the filename from
`Desc[1]@0x181`, and open that text file from the game directory. There is **no
textblock number** and **no gang-record field** involved in the description linkage.

### 4.3 Why the textblock/wccupda2 hypothesis does not apply here

The engine *does* have a large textblock store, but it is a different file used for a
different purpose:

- **WCCTEXT2.DAT** — `DAT_00479148 = dfaOpen("WCCTEXT2.DAT", 0x7e8, 0)`, i.e.
  **2024-byte records** (the ~2020-byte "textblock" store). `get_text_block(id, seq,
  buf)` / `display_LONG_text(id)` read it, keyed by **`(seq @ offset 0, block# @
  offset 8)`**. It backs item **long** text (`item+0x414`), monster/spell scripted
  text, and the `TextBlock` (0x94) / `DeathText` (0x9b) abilities via
  `perform_text_block_as_special_command` — **not** room descriptions.
- **WCCUPDA2.DAT** — referenced only by `check_begin_updating` in
  `preload_and_generate_buffers`; it is the **database-update** feed, unrelated to
  descriptions.

So the two candidate "textblock" files (WCCTEXT2.DAT and WCCUPDA2.DAT) are not how
gang-house descriptions are stored in this build. If a player-facing in-game *editor*
for these files exists it was not located in the decompile (no `fopen("w")` /
`tfscre` writer targets the room `Desc[1]` filename); the descriptions are authored
as external text files and merely *referenced* from the room record.

---

## 5. Ranking and chat

### 5.1 Top-gang ranking — `display_top_gangs` / `display_a_top_gang`

`TOPTEN`-style listing. `display_top_gangs(N)` first flushes all gang buffers, then
walks WCCGANG2.DAT via `dfaAcqLock(buf, 0, 1, 0xc, 0)` + `dfaQueryNP(0x38)` — i.e.
in **Btrieve key-1 order** (the ordering index, effectively experience-ranked) — and
displays up to `N` gangs, **skipping** records whose flag bits `0x1` (disbanded) or
`0x4` are set. Header columns (`display_top_gang_header`): **Rank, Gangname
(`+0x14`), Leader (`+0x2c`), Members (`+0x4e`), Created (`+0x4a` via `ncedat`)**, and
an **Exp** column (`+0x28`) shown only to privileged viewers. When
`gang+0x50 & 8` (pool saturated) the Exp cell additionally prints the secondary pool
`+0x58` and "×`+0x5c`+1 times". Empty result ⇒ "There are no gangs currently
established."

### 5.2 Gang chat — `tell_gang` / `cmd_broadgang`

- **Channel semantics:** `tell_gang(gangNamePtr)` broadcasts the composed `prf`
  buffer to **every online player whose `player+0x6c8` matches the gang name** — the
  gang channel is defined entirely by shared membership string, with no separate
  subscription list.
- **Send:** `GANG <text>` / `GUILD <text>` (`cmd_broadgang` with `margc >= 2`)
  formats "%s gangpaths '%s'" and calls `tell_gang(player+0x6c8)`. Requires the
  sender to be in a gang. `join_gang` and `remove_from_gang` also use `tell_gang` for
  join/leave announcements.
- **Roster:** `GANG`/`GUILD` with no argument shows the member roster (§1.6).

(`cmd_broadcast`/`cmd_join <channel>` are the generic numbered chat channels — a
separate system from gangs.)

---

## 6. Summary

- **A gang is one WCCGANG2.DAT record.** Leadership is not a flag — it is
  `gang+0x2c == player name`. Members carry the gang's display name in
  `player+0x6c8`; Lieutenant rank is `player+0x7d4 & 0x100`. Ranks are Leader /
  Lieutenant / Member; there is **no coded member cap**.
- **Create** needs exp ≥ 100000 and a free membership slot; **leaving as leader
  disbands** (`gang+0x50 |= 1`) and ejects everyone. Promote/demote apply instantly
  to online targets, else via deferred `+0x7d5` bits at login.
- **Two funds:** a shared **experience pool** (`gang+0x28`, overflow → `+0x58`/`+0x5c`)
  fed by every member's kills and spent on **guild-house deeds**; and a **gold
  account** in **bank 8** keyed by a name in the GHouse shop record, credited by
  `deposit_gangleaders_account` from gang-house purchases.
- **Guild house** = a room with `room+0x564 & 0x40`; its deed/furnishing vendor is a
  type-`0xb` shop gated to gang leaders with a sufficient exp pool. `GHouseDeed`
  (181)/`GHouseTax` (182)/`GHouseItem` (183) tag the merchandise.
- **★ Custom description linkage:** the room's description is redirected to an
  **external text file** when **`Desc[0]` (WCCMP001 offset `0x13a`) == the literal
  `"FILE_DESCRIPTION"`**, in which case **`Desc[1]` (offset `0x181`) holds the
  filename**, read at render time via `display_desc_from_file`/`tfsopn`. This is the
  exact field pair to key on for extraction. It is **not** a textblock number:
  WCCTEXT2.DAT (2024-B records, `get_text_block`) is used only for item/monster/spell
  long text, and WCCUPDA2.DAT is the DB-update feed.
- **Ranking** iterates gangs in Btrieve key-1 (experience) order, skipping disbanded.
  **Chat** targets every online player sharing the gang name (`tell_gang`).

## 7. Undetermined / flagged

- **`GangHouseNumber @ room+0x62`** (cited from the room schema) was **not** the
  linkage found in code; the operative gang-house room fields here are the `+0x564`
  bit-`0x40` flag, `+0x5c4` controlling-room, and `+0x444` owner name. Whether the
  disk WCCMP001 record additionally stores a numeric gang-house id near `+0x62`
  (populated but unused by the paths read) was not disk-verified.
- **Player-side description editing.** No in-game command that *writes* the room
  `Desc[1]` filename or authors the external file was located; descriptions appear to
  be placed as external text files out-of-band. The `CREATE <direction>` house-build
  path is present but stubbed.
- **Deed exp-price constant** `DAT_00482d10` (× 10000) and the GHouse per-item tax
  arithmetic were read structurally from `buy_item`; exact numeric values are config
  globals not resolved here.
- **Top-gang key-1** is taken to be experience-descending from the "Top Gangs" +
  Exp-column context; the Btrieve index definition itself was not read from the file
  header. The `+0x50 & 0x4` skip bit's meaning (beyond "not shown") is unconfirmed.
- **`deposit_gangleaders_account` account name** at `shop+0x128` — confirmed to be a
  bank-8 bankbook key, but whether it is set to the gang name or the leader name at
  house-setup time was not traced to its writer.
</content>
</invoke>
