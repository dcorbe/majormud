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

> **Disk-file check (2026-07-19):** the shipped template
> `wg_nt_ref/WCCNT8PJ/out/WCCGANG2.VIR` (28,672 B = 7 × 4096-B pages) parses
> with `vir_wg.py` as **logical record length `0x100` (256), physical `0x10a`
> (266, 10-B usage prefix)** — matching the `dfaOpen` size exactly. It contains
> 2 FCR + 2 PAT pages, 2 index pages, and **one data page with zero used
> slots**: it is an all-empty pre-created template, so the field offsets below
> could not be cross-checked against sample records (no gang data ships with
> the game; records are only created at runtime by `cmd_create`).

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
| `0x2000` (`+0x7d5 & 0x20`) | (non-gang) exp/character flag — unrelated (`restructure_experience` gate at login) |
| `0x4000` (`+0x7d5 & 0x40`) | **outstanding gang-shop paperwork** — blocks deed purchase (`buy_item` refusal 7, checked at 14463); **set by selling to a type-`0xc` deed shop** (14740) |
| `0x0008` (`+0x7d4 & 0x08`) | **roster view: online-only** — clear ⇒ `display_gang_members` (all), set ⇒ `display_online_gang_members` (`cmd_broadgang` 53865). Toggled by the **SET command** (54136-54143: set → "You will now only see online gang members.", clear → "You will now see all gang members."; bad arg → "Valid gang options: Online, All") |

The login/refresh handler (line ~10200) is the deferred-action processor. Full
confirmed order (decompile 10220-10285), each arm printing its notice and
self-clearing:

1. `0x0200` pending promote → set `0x0100`, clear `0x0200`,
   "You have been promoted to the rank of lieutenant."
2. `0x0400` pending demote → clear `0x0100` AND `0x0400`,
   "You have been demoted from the rank of lieutenant."
3. `0x0800` → clear, "Your ganghouse has been closed down!!"
4. `0x1000` → clear, "Gang house items have dissappeared from your inventory!" (sic)
5. **Membership validation** (`player+0x6c8` non-empty):
   - gang record **missing** → if Lieutenant, clear `0x0100` +
     "You have been stripped of your rank as lieutenant!"; then
     "You are no longer in the gang %s." (player's stored copy of the name)
     and clear `+0x6c8`.
   - gang **disbanded** (`+0x50 & 1`) → clear Lieutenant bit **silently**
     (no strip notice), decrement `gang+0x4e`, mark gang dirty,
     "Your gang, %s, has been disbanded!" (gang **display** name `+0x14`),
     clear `+0x6c8`. Offline members therefore drain the member count one by
     one at their next login.

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
  can — "You must be the gang leader to…"). Ejected member gets
  `"%s has exiled you from %s."` — **string confirmed in the DLL** (decompile
  52952: `prf(fmt, remover+0x1e, remover+0x6c8)`, i.e. args = remover's
  character name, gang display name).

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
**gang stock shop** (`shop+0xcc == 0xb`; polarity corrected 2026-07-30, see §3.1)
after a successful purchase. It looks up a **bankbook** via
`get_bankbook_data(shop+0x128, 8)` — keyed by a **name string stored in the shop
record at `shop+0x128`** (for a gang shop this slot is repurposed from the normal
max-stock array to hold the account-holder name — written by `cmd_stock` with the
stocker's BBS account user-id; see §7). **Bank id 8 = the bank's shop number**
(2026-07-30): `get_bankbook_data` (0x33190) matches bankbook records on
`(name @ +0x00, bank id u32 @ +0x20)`, and `deposit_gold` passes the bank shop's
number as that id — shipped shop **8 = "Bank of Godfrey"** (shoptype 7, the
Silvermere bank; consistent with the tax lifecycle's "OLD SILVERMERE BALANCE"
log line, §8). It adds `amount` copper to `bankbook+0x24` — with an overflow
guard `balance+amount > balance` under which **the deposit is skipped entirely**
(not clamped) — marks the bankbook dirty, logs
`"GANGSHOP DEPOSIT : %s : %s copper farthings."` (exact literal), and flushes
all bankbooks. So the **gold price** a member pays for a gang-shop item is
deposited into the gang's bank-8 account. **The deed purchase touches
neither** (corrected 2026-07-30, slice-7 implementation pass): the type-`0xc`
branch never calls the deposit, and the exp pool is a threshold GATE — it is
never debited (14416-14437 only read it).

### 2.3 Gang-house tax — `GHouseTax` (182 / 0xb5-adjacent abilities)

`GHouseDeed` (181 / 0xb5), `GHouseTax` (182) and `GHouseItem` (183) are **item
abilities** carried by the special gang-house merchandise (per `abilities.md`; all
flagged engine-referenced). They are the data that marks a shop item as a *deed*, a
*taxed item*, or a *house furnishing*, consumed by the gang-shop branches of
`buy_item` rather than applied to the player. The "already owns a gang item"
sentinel `buy_item` scans the buyer's inventory for is **`GHouseDeed` (0xb5 = 181)**
— an earlier revision of this doc mislabeled it `BadAttk 185`, conflating the hex
value with the wrong decimal id (corrected 2026-07-30). The three shipped deed
items **1008/1009/1010** (red/orange/yellow parchment deed) carry GHouseDeed with
**values 1/2/3** — deed tiers. (The precise per-item tax arithmetic lives in the
gang-shop pricing block of `buy_item`; the deposit half is §2.2.)

---

## 3. Guild houses

### 3.1 Buying a deed / house item — `buy_item`, shop types `0xb`/`0xc`

> **POLARITY CORRECTED 2026-07-30.** Earlier revisions of this section had the
> two types swapped. Direct read of `buy_item`'s branches (decompiled.c
> ~14394-14660) plus the shipped data (21 type-11 shops all named
> "Gang Shop #NNN"; one type-12 shop, #124 "Realm Deed Shop", stocked with the
> parchment deeds 1008-1010) settles it as below.

Gang-house commerce uses two shop types that reprice from the shop record
(restocking disabled — the restock walker skips them — pricing fields overloaded):

- **DEED shop (`shop+0xcc == 0xc`, shipped shop 124 "Realm Deed Shop")** —
  `buy_item` branch ~14394-14466. Gate order as compiled:
  1. resolve the buyer's gang (`get_gang_data(player+0x6c8)`) and require
     **leadership** (`sameas(gang+0x2c, player+0x1e)`) → refusal **6**
     "You must be a gang leader to purchase a gang house deed.";
  2. 100-slot inventory scan for any item with **GHouseDeed (0xb5)** → refusal
     **4** "You are already the owner of a gang house.";
  3. **gang experience pool** `gang+0x28 >= DAT_00482d10 * 10000` (= **GANGEXP**
     MSG option × 10000, default 1000 → **10,000,000 exp**; see §7) → refusal
     **5** "Your gang does not have enough experience for you to purchase a gang
     house now." **Saturated nuance:** if `gang+0x50 & 8` is set the pool is
     treated as exactly GANGEXP×10000 — automatically sufficient;
  4. **paperwork** `(player+0x7d5) & 0x40` is checked **last and overrides** →
     refusal **7** "Due to outstanding paper-work we are unable to provide you
     with another / property today. Please call back tomorrow!" (two lines).
  The paperwork bit's **setter** is the *sell* path: selling to a type-`0xc`
  shop sets word bit `0x4000` (14740) — i.e. selling a deed back files the
  paperwork that blocks another purchase until cleared (by the tax/cleanup
  lifecycle, §8 — not ported in M7).
- **Gang STOCK shop (`shop+0xcc == 0xb`, the 21 "Gang Shop #NNN")** — branch
  ~14520-14650. Player-stocked storefront: per-slot price denomination read from
  `shop+0x1a0 + slot*2` via `convert_currency`, a `(0x6e - CHA/5)`-style markup
  applied (~14571), `deposit_gangleaders_account(shop, price)` credited on
  **every** sale (§2.2), and the slot is **removed from the stock list when its
  quantity reaches 0** ("REMOVING ITEM FROM STOCK LIST" debug). Gang shops **buy
  nothing** (`sell_item`: "You may not sell items to a gang shop."). Stocking is
  `STOCK`/`UNSTOCK`/`MARKUP`, gated on carrying the `GShopItem` (184) controller
  whose value matches `room+0x46e` (§7).

**Where BUY works at all (2026-07-30):** the DLL's `cmd_buy` (0x539b7) requires
the room's own shop link (`room+0x43c == 1`) — the same storefront gate for every
shop type. Shop 124's storefront is **map 15 room 732** (`type` 1 in the shipped
DB); all 19 rooms holding type-11 gang shops are also `type` 1. The ~190 other
map-15 rooms that carry `shopnum` 124 in the extract are district metadata — BUY
there falls through to the cmdtext funnel, with one special arm: **`BUY ROOM` in
a non-storefront room prints the lease stub** "If you are a gang leader you may
lease a Gang House." (the same stub the `CREATE <direction>` build path prints).

### 3.2 Room ↔ gang-house link

A room is a gang house when `room+0x564 & 0x40` (the **Ganghouse** flag, printed as
"Ganghouse" in the sysop room dump; the shipped ganghouse rooms carry attributes
`0x42` = Ganghouse|patrollable). The house's **controlling room** number is
`room+0x5c4`. The house's shop is the room's own shop (`room+0x43c==1`, id
`room+0x440`) — type `0xb` for the gang stock shops; the deed vendor itself is
the type-`0xc` Realm Deed Shop (§3.1). The `CREATE <direction>`
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
the `FILE DESCRIPTION` sentinel mechanism, and gang houses are its headline user
(the error string is literally `"Can't display gang house file %s"`).

> **Sentinel spelling (corrected 2026-07-30):** the literal is `FILE DESCRIPTION`
> **with a space**. Earlier revisions wrote `FILE_DESCRIPTION` — an artifact of
> the Ghidra symbol name `s_FILE_DESCRIPTION_00483ecf`, which substitutes `_` for
> spaces. Confirmed against both the DLL string table and the shipped content DB
> (136 rooms with `desc_1 = 'FILE DESCRIPTION'`, e.g. map 15 room 851 →
> `WCC85115.HSE`).

### 4.1 The mechanism (`display_room_desc`, `display_LONG_room_desc`, `display_desc_from_file`)

Room description rendering checks:

```
if (sameas(room + 0x13a, "FILE DESCRIPTION") && room[0x181] != '\0')
    display_desc_from_file(room + 0x181, room + 0x1c8, ...);   // read external file
else
    print the inline Desc[] paragraphs;
```

- **`room + 0x13a` = Desc[0]** holds the literal sentinel string
  `"FILE DESCRIPTION"`.
- **`room + 0x181` = Desc[1]** (the second 71-byte paragraph) holds the **filename**
  of the external description text file.
- **`room + 0x1c8` = Desc[2]** is passed as the section/prefix selector into
  `display_desc_from_file`.

`display_desc_from_file(nameField, section, …)` opens that file with **`tfsopn`**
(the BBS text-file service, i.e. an ordinary on-disk text file) and streams its
lines; on failure it raises `internal_error("Can't display gang house file %s")`.
The same sentinel+filename convention is reused for item descriptions
(`display_item_desc`: `item+0xcb`=="FILE DESCRIPTION", filename at `item+0x108`).

### 4.2 Exact field that holds the description pointer (for extraction)

**On the ROOM record (WCCMP001), keyed by `(MapNumber, RoomNumber)`:**

- `Desc[0]` at record offset **`0x13a`** — must equal the ASCII string
  `"FILE DESCRIPTION"` for the file mechanism to engage.
- `Desc[1]` at record offset **`0x181`** (= `0x13a + 71`) — the **description
  filename** (a DOS text-file name). This is the field to read to recover a gang
  house's custom description.

Extraction recipe: scan WCCMP001 for rooms whose `Desc[0]@0x13a == "FILE DESCRIPTION"`
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

`TOPTEN`-style listing; the user-facing form is **`TOP <n> GANGS`** (the MAXTOP
MSG option, default 30, caps `n`). `display_top_gangs(N)` first flushes all gang
buffers, then walks WCCGANG2.DAT via `dfaAcqLock(buf, 0, 1, 0xc, 0)` +
`dfaQueryNP(0x38)` — i.e. in **Btrieve key-1 order** (the ordering index,
effectively experience-ranked) — and displays up to `N` gangs, **skipping**
records whose flag bits `0x1` (disbanded) or `0x4` (hidden — the sysop
`DISABLE`/`ENABLE` toggle, §7) are set. Header columns
(`display_top_gang_header`): **Rank, Gangname (`+0x14`), Leader (`+0x2c`),
Members (`+0x4e`), Created (`+0x4a` via `ncedat`)**, and an **Exp** column
(`+0x28`) shown only to privileged viewers (`EXP_IN_TOPGANG` config key). When
`gang+0x50 & 8` (pool saturated) the Exp cell additionally prints the secondary
pool `+0x58` and "×`+0x5c`+1 times". Empty result ⇒
`"There are no gangs currently established!"` (exact literal, with the bang).

> **Key-1 order, header evidence (2026-07-30):** the template's FCR key-spec
> region (`WCCGANG2.VIR` @0x110) shows a second key of **length 4 with the
> duplicates flag**, positioned at/near the `+0x28` exp pool — consistent with
> an experience index — but the spec bytes don't parse cleanly enough to read
> the descending flag (0x40) with confidence, and the populated mirror
> (`docs/mirrors/github-lucid2310-ReMUD/DATs/WCCGANG2.dat`) is byte-identical
> empty. **Port decision: exp-descending, documented divergence** (a "Top"
> listing ascending would be nonsense; if a live capture ever contradicts,
> re-pin).

### 5.2 Gang chat — `tell_gang` / `cmd_broadgang`

- **Channel semantics:** `tell_gang(gangNamePtr)` broadcasts the composed `prf`
  buffer to **every online player whose `player+0x6c8` matches the gang name** — the
  gang channel is defined entirely by shared membership string, with no separate
  subscription list.
- **Send:** `GANG <text>` / `GUILD <text>` (`cmd_broadgang` with `margc >= 2`)
  formats **`"%s gangpaths: %s%s"`** (exact literal, corrected 2026-07-30 from
  the DLL string table @VA 0x48a9bb) and calls `tell_gang(player+0x6c8)`.
  The three varargs are `(sender display name via FUN_00416c89, DAT_0048a9cf,
  message)`. `DAT_0048a9cf` is a raw ANSI blob:
  `1b 5b 5b 1b 5b 30 3b 33 33 6d 7c 20 08 20 08 20 08 20 08 5d 00`
  (≈ `ESC[` + `[` + `ESC[0;33m` + `| \b \b \b \b]` — a bracket/backspace
  compose; **rendered form ORACLE-VERIFY**, capture live before pinning text).
  A leading `'` or `-` on the message is stripped (emote-style prefix handling:
  message = `margv[0]+1` instead of the tail). Requires the sender to be in a
  gang — else `"You are not in a gang at the present!"`. `join_gang` and
  `remove_from_gang` also use `tell_gang` for join/leave announcements.
- **Roster:** `GANG`/`GUILD` with no argument shows the member roster (§1.6);
  with no argument and no gang, the same "not in a gang at the present!" line.

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
  account** in **bank 8** keyed by the BBS user-id stored at `shop+0x128`
  (written by `cmd_stock`; see §7), credited by
  `deposit_gangleaders_account` from gang-house purchases.
- **Guild house** = a room with `room+0x564 & 0x40`; deeds are bought at the
  type-`0xc` Realm Deed Shop (leader-gated, exp-pool priced), and each house
  runs a type-`0xb` gang stock shop whose sales deposit to bank 8. `GHouseDeed`
  (181)/`GHouseTax` (182)/`GHouseItem` (183)/`GShopItem` (184) tag the
  merchandise.
- **★ Custom description linkage:** the room's description is redirected to an
  **external text file** when **`Desc[0]` (WCCMP001 offset `0x13a`) == the literal
  `"FILE DESCRIPTION"`**, in which case **`Desc[1]` (offset `0x181`) holds the
  filename**, read at render time via `display_desc_from_file`/`tfsopn`. This is the
  exact field pair to key on for extraction. It is **not** a textblock number:
  WCCTEXT2.DAT (2024-B records, `get_text_block`) is used only for item/monster/spell
  long text, and WCCUPDA2.DAT is the DB-update feed.
- **Ranking** iterates gangs in Btrieve key-1 (experience) order, skipping disbanded.
  **Chat** targets every online player sharing the gang name (`tell_gang`).

## 7. Undetermined / flagged

- ~~**`GangHouseNumber @ room+0x62`**~~ **CLOSED (2026-07-30):** the number
  lives at **`room+0x46e`** — the extractor's `ganghousenumber` column, which
  matches the manifest's `ganghouse#` (1..10) on every sampled room, and is
  exactly what the `GShopItem` controller gate compares against (`cmd_stock`
  50646). Item 844 "red key" carries GShopItem value 1 = the Red house, whose
  shop room (15/973) carries `ganghousenumber` 1.
- **Player-side description editing.** No in-game command that *writes* the room
  `Desc[1]` filename or authors the external file was located; descriptions appear to
  be placed as external text files out-of-band. The `CREATE <direction>` house-build
  path is present but stubbed.
- ~~**Deed exp-price constant** `DAT_00482d10`~~ **CLOSED (2026-07-19).**
  `DAT_00482d10` is not a compiled constant: `reload__wccmmud` (0x11ff) sets
  `DAT_00482d10 = numopt(0x42, 0, 0x7fff)` — MSG option #66 of WCCMMUD.MSG,
  which is **`GANGEXP {Experience (x10000) required? 1000} N 0 32767`**
  (`wg_nt_ref/WCCNT8PJ/out/WCCMMUD.MSG`; the surrounding option indices
  0x41=GENDERGO/-1..32000, 0x44=MAXEPDAY/4..100, 0x45=EPCYCLE/15..360 all match
  their `numopt` ranges, confirming the numbering). So the deed gate is
  **sysop-configurable**, default `1000 × 10000 = 10,000,000` gang-pool exp
  (`buy_item` debug: "GANG EXP REQ BASE * 10000"). The GHouse per-item tax
  arithmetic remains only structurally read.
- **Top-gang key-1** is taken to be experience-descending; the 2026-07-30 FCR
  header read (§5.1 note) found a 4-byte duplicates-key consistent with an exp
  index but could not confirm sort direction — port ships exp-descending as a
  documented divergence.
- ~~The `+0x50 & 0x4` skip bit~~ **RESOLVED (2026-07-30):** the sysop command
  set includes `SYSOP DISABLE <gangname>` → "Gang %s has been disabled and will
  not appear on the top gangs." and `SYSOP ENABLE <gangname>` → "…will appear on
  the top gangs listing again." — bit `0x4` = **hidden from the top-gangs
  listing**. The DLL also ships `SYSOP DISBAND <gangname>` ("…will be deleted at
  cleanup."), `SYSOP GANGSIZE <n> <gangname>` ("Gang %s has been resized to %d."
  / "You cannot set the gangsize to 0" — writes `+0x4e` directly, consistent
  with member count being bookkeeping-only), `LIST GANG <gangname>`, and a gang
  rename path ("%s's gangname changed from %s to "). Related config keys:
  `SYS_DISABLE_GANGS`, `EXP_IN_TOPGANG`, `GANGBUF` (gang cache depth, default
  10), `MAXTOP` (TOP cap, default 30). **M7 port: schema-only** — the flag bit
  exists and the TOP walker honors it; no sysop command surface ships (USER
  DECISION 2026-07-30).
- **Command minimum abbreviations** for the gang verb set are compiled into
  `parse_command`'s per-character decision tree (0xdd44, see combat_rounds.md)
  and were not extracted; the port's VERBS entries carry `ORACLE-VERIFY`
  minimums pending the slice-8 live expedition.
- ~~**`deposit_gangleaders_account` account name** at `shop+0x128`~~ **CLOSED
  (2026-07-19).** The writer is **`cmd_stock`** (0x52b1e): on every successful
  `STOCK` into a GShop-controlled gang shop it does
  `strcpy(shop+0x128, player)` — copying the string at the **player record
  base**, i.e. the stocker's **BBS account user-id** (offset `+0x00`, 30 bytes;
  the in-game character name is the separate `+0x1e` field). It is **neither
  the gang name nor the leader's character name**: bank-8 bankbooks are keyed
  by BBS user-id, and `deposit_gangleaders_account` then resolves
  `get_bankbook_data(shop+0x128, 8)`. Stocking rights (and thus who the
  deposits accrue to) go to whoever is in the shop room carrying the item with
  ability `GShopItem` (0xb8/184) whose value matches `room+0x46e` — in practice
  the gang leader who bought the shop controller, but mechanically it is the
  **last player to stock an item**, since each stock overwrites `+0x128`.

## 8. Gang-house tax / eviction lifecycle — **PENDING (M8), evidence only, not ported in M7**

Absent from earlier revisions of this spec but clearly implemented in the DLL
(discovered 2026-07-30 via the string table; the arithmetic has not had a
decompile pass). There are **ten numbered gang houses** with colour names —
"Red Gang House", "Orange", "Yellow", "Green", "Violet", "Blue", "Black",
"Silver", "Gold", "White Gang House" (1..10, matching the `ganghouse#` column in
`re/exports/gang_house_description_files.txt`; e.g. room 850's .HSE reads
"Entrance White House Room" → house 10) — plus a "Gang Shop" label. A periodic
process charges each occupied house's leader a **house tax** against a bankbook
balance (the Silvermere bank — bank 8, §2.2):

```
GANGHOUSE : NMBR %d - LEADER %s - GANG %s - HOUSE TAX %s.
GANGHOUSE : Leader %s paid house tax. OLD SILVERMERE BALANCE %s NEW BALANCE %s.
GANGHOUSE : Gang leader %s couldn't pay his gang house tax. BALANCE %s.
Failed update of bankbook during Ganghouse Taxation
GANGHOUSE : HOUSE %s : House tax paid flag NOT set.
GANGHOUSE : HOUSE %s : Effective eviction flag set.
GANGHOUSE : HOUSE %s : Not Occupied.
GANGHOUSE : Adding gang house deed for gang house %d back into deed shop.
GANGHOUSE : Removing GANGHOUSE DEED %d from Room %s Map %s floor.
GANGHOUSE : DELETE_AT_CLEANUP : Skipping room cleanup. Room %s Map %s Gang house Number %s.
```

On non-payment: an eviction flag is set, the house's deed is re-shelved into the
deed shop, stray deeds are removed from floors, and the ex-occupants get the
deferred `player+0x7d4` notice bits — `0x0800` "Your ganghouse has been closed
down!!" and `0x1000` "Gang house items have dissappeared from your inventory!"
(§0). The `0x4000` paperwork bit (set by selling a deed back, §3.1) is presumably
cleared by this same cleanup cycle ("call back tomorrow"). **None of this ships
in M7** (USER DECISION 2026-07-30): the M7 port models the notice bits and the
paperwork refusal, and defers the tax scheduler, eviction, and deed re-shelving
to M8 with this section as the spec seed. Where the house-number lives on the
room/gang record remains unlocated (§7 first bullet).
</content>
</invoke>

## 9. As built — M7 slice 7 (2026-07-30)

The port (crates/mud-core `gang.rs` + the `game.rs` gang surfaces,
crates/mud-server persistence) ships everything in §0-§5 except the
deferrals below. Spec corrections discovered DURING implementation, all
decompile-read:

- **INVITE and UNINVITE gang arms require the `MEMBER` keyword**
  (`INVITE MEMBER <name>` / `UNINVITE MEMBER <name>`); the plain forms
  are the party follow system (unported, M8). §1.2/§1.4's bare-form
  aliases were wrong. The gang arm's not-found line prints margv[1] — so
  it literally says "You don't see member here!" (52753, ported).
- **JOIN has no GUILD alias** (cmd_join matches only "gang"), and ANY
  join attempt against an EXISTING gang clears EVERY pending invite for
  the user (`clear_gang_invitations(user, NULL)`); a nonexistent gang
  leaves them intact.
- **cmd_create**: separate too-LONG gate ("The name you have chosen is
  too LONG: %s") ahead of the 'None'/charset checks; `CREATE ROOM <arg>`
  is the lease-stub build path; other keyword pairs with args are a
  SILENT consume; fewer than two words falls to say (margc < 3). The
  in-use refusal appends "%s is the leader of %s." unless the holder is
  disbanded.
- **PROMOTE/DEMOTE are silent no-ops** for non-leaders, gangless actors,
  and multi-word names (margc == 2 gate). Self-promote prints the
  demote-yourself line (string reuse). Unknown OFFLINE names print the
  syntax line. Offline demote sets the pending bit with NO
  is-lieutenant check; offline promote does check already-lieutenant.
  "Gang member %s is not a lieutenant in your gang." is UNPLACED dead
  text (no call site found). Offline gang-mismatch wording differs per
  verb (invite-first vs the period-form demote line).
- **DISBAND GANG runs a yes/no continuation** (input state 0x88): one
  word starting with Y accepts; the gang hears BOTH lines ("The gang %s
  has now been disbanded." + "The name may not be used again until all
  members have entered the game!") via tell_gang before the silent
  online sweep. **tell_gang has no sender exclusion** — joiners hear
  their own join broadcast, gangpath senders their own line.
- **The deed shop's already-owner refusal (code 4) is DEAD CODE**: the
  pool check clobbers it to 5 (14435), so an owner sees the
  exp-insufficient line (ORACLE-VERIFY). The pool is never debited and
  no deposit fires on a deed sale (§2.2 correction). Selling a stocked
  item back to the type-0xc shop sets the 0x4000 paperwork bit and
  skips the shelf restock.
- **Gang shops**: ten slots, count cap 20, sold-out/emptied slots are
  DELISTED; every STOCK rewrites slot price/denomination (explicit args
  or the item's own +0x322/+0x429) and overwrites last_stocker; UNSTOCK
  pulls from the lowest-count slot; buy price carries the DLL's >100000
  1/100-precision quirk and deposits the FULL price to bank 8.

**Port divergences** (each cited at its call site): roster ALL-view
order is mirror order (boot scan + joins), not WCCUSERS record order;
the Btrieve check-vs-insert race arm of cmd_create is unportable;
top-gangs order is exp-descending by assumption (§5.1 note); the
gangpath ANSI blob renders as its visible intent; offline promote of a
GANGLESS player prints the syntax line (the mirror cannot see them —
the DLL would say the invite-first line); GANGEXP/MAXTOP are
compile-time defaults (1000/30) rather than MSG options; the bank-8
u32 skip-on-overflow guard is unreachable under u64 balances.

## 10. M7 deferrals

| item | target | note |
|---|---|---|
| Gang-house tax / eviction lifecycle | **M8** | §8 — evidence only; the paperwork bit ships with no clearer |
| Sysop gang commands (DISBAND/DISABLE/ENABLE/GANGSIZE, LIST GANG, rename) | M8 | flags bit 0x4 is schema-only; TOP skips it |
| Player TOP arm | M8 | needs board-wide account data; stub + marker |
| Party/group system (plain INVITE/UNINVITE, JOIN channels, cmd_follow, DISBAND PARTY) | M8 | gang arms fall through to say |
| .HSE editing | never | out-of-band even in the original (§4.3) |
| Gang war | M8+ | needs PvP combat |
| Limited-item / worn-second-copy STOCK gates | M8 | fields unmodeled; marker at cmd_stock port |
| Oracle expedition (strings, min-abbrevs, gangpath render, deed dead-code-4) | slice 8 | two-character program + .HSE render |
