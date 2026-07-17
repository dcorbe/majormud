# MajorMUD (WG3-NT) — Economy spec: shops, items, banking

Behavioral spec derived from the 32-bit WG3-NT decompile
(`wg_nt_ghidra/exports/WCCMMUD_decompiled.c`). Cross-references `vir_schemas.md`
(SHOP / ITEM disk records), `abilities.md` (item-ability ids), and `leveling.md`
(trainer shops `shop+0xcc==8`, the train-cost markup formula, the
`update_dynamic_stats` stat rebuild). Offsets are byte offsets into the
shop / item / player / bankbook records unless noted.

Functions read: `buy_item` (0x1b86f), `sell_item` (0x1c43c),
`get_shop_item_from_name` (0x1aeb8), `shop_sells_item` (0x1b83b),
`display_shop_items` (0x39eb5), `display_item_in_shop` (0x3bb08),
`get_shop_data` (0x32f5a), `cmd_buy` (0x539b7), `cmd_sell` (0x53be1),
`check_initiate_restocking` (0x5b58c), `restock_items` (0x5b6f6),
`wear_armour` (0x1c846), `remove_armour` (0x1ce18),
`update_allowed_worn_items` (0x14b38), `user_is_wearing` (0x1c815),
`user_can_use` (0x1fced), `equip_apply_item_abilities` (0x4b23c),
`unequip_apply_item_abilities` (0x4b2a5), `deduct_item_charge` (0x1b4d2),
`add_item_to_inventory` (0x1b111), `remove_item_from_inventory` (0x1b2c8),
`deduct_currency` (0x1edca), `check_currency` (0x1ed8a),
`convert_currency` (0x1ed10), `cleanup_currency` (0x1ebc0),
`get_coin_weight` (0x1f354), `get_max_weight` (0x1f3d0),
`deposit_gold` (0x1d4cc), `withdraw_gold` (0x1d590), `borrow_gold` (0x1d62a),
`repay_gold` (0x1d727), `get_users_bankbook_balance` (0x33462),
`display_bank_balance` (0x3b3df), `deduct_exempt_credits` (0x22675),
`proper_currency_name` (0x6bcd9).

---

## 0. Record fields this spec relies on

### SHOP record (WCCSHOPS, 500 B) — economy fields

| off | width | field |
|-----|-------|-------|
| `+0x02` | 29 B | shop name (per `vir_schemas.md`) |
| `+0xcc` | word | **shop type** (see below) |
| `+0xce` / `+0xd0` | word | trainer min / max level band (`leveling.md`) |
| `+0xd2` | word | **markup %** (buy price uplift; also the trainer fee input) |
| `+0xd6` | word | class restriction: 0 = any, else required class (`leveling.md`) |
| `+0xd8[20]` | long | **stock list** — item numbers (0x14 slots) |
| `+0x128[20]` | word | **max stock** (restock target / cap) per slot |
| `+0x150[20]` | word | **current stock** count per slot |
| `+0x178[20]` | word | **restock interval** per slot *(overloaded: GHouse price)* |
| `+0x1a0[20]` | word | **restock amount** per event *(overloaded: GHouse currency index)* |
| `+0x1c8[20]` | byte | **restock probability %** per check |
| `+0x1dc` | byte | dirty flag (shop record needs saving) |

**Shop type `+0xcc`** (observed switch values):
`5` = healer, `7` = bank, `8` = trainer/guild (`leveling.md`),
`0xa`(10) and the default = ordinary store, `0xb` = **gang house / GHouse**
(deed/tax shop, priced from the shop record itself), `0xc` = **gang shop /
GShop**. `check_initiate_restocking` and `restock_items` explicitly skip
`+0xcc==0xb`.

**Field overloading by shop type.** For ordinary/healer/trainer shops,
`+0x178`/`+0x1a0`/`+0x1c8` are the restock triple (interval / amount /
probability). For a **GHouse (`0xb`)** shop, `+0x178[i]` instead holds the item's
**base price** and `+0x1a0[i]` its **currency-denomination index** — restocking is
disabled for that type, so the slots are reused for pricing.

### ITEM record (WCCITEMS, 1061 B) — economy fields

| off | width | field |
|-----|-------|-------|
| `+0xad` | 29 B | item name |
| `+0x2f2` | word | **weight** |
| `+0x2f4` | word | **type** (0 = armor, 1 = weapon, 6 = light, 7 = stackable, 0xb = class-locked, …) |
| `+0x2f6[20]` | word | **ability-id table** (parallel to value table) |
| `+0x31e` | word | **default charges/uses** (seed copied into the carry slot) |
| `+0x322` | word | **cost value** (in the denomination named by `+0x429`) |
| `+0x324[10]` | word | **allowed class** list (0-terminated slots) |
| `+0x344[10]` | long | **allowed race** list |
| `+0x394` | word | weapon sub-type / hands (1 or 3 ⇒ two-handed, blocks off-hand) |
| `+0x396` | word | armor **strength/weight requirement** (vs class `+0x46`) |
| `+0x398` | byte | **worn-location code** (which slot; 0 ⇒ cannot be worn) |
| `+0x3e2[20]` | word | **ability-value table** (parallel to `+0x2f6`) |
| `+0x418` | long | removal-message id (WCCMSG) shown when the item is destroyed |
| `+0x429` | byte | **cost currency denomination** (0..4; 0 = copper, lowest) |
| `+0x42a` | byte | **"do not destroy at 0 charges"** flag (0 ⇒ item poofs at 0 uses) |

### PLAYER record — economy fields

| off | field |
|-----|-------|
| `+0x90` / `+0x92` / `+0x94` | race / class / level |
| `+0xac` | **Charm** (current effective) — the haggling stat |
| `+0xae` / `+0xb0` | max HP / current HP |
| `+0xb2` / `+0xb4` | carry capacity / carried item weight |
| `+0xd8[100]` | inventory item-number array; `+0x268[100]` per-item uses/charges |
| `+0x330` | count for the stackable (type-7) sub-list |
| `+0x334[50]` | stackable inventory array; `+0x3fc[50]` its per-item uses |
| `+0x542` | alignment (feeds `get_legal_level`) |
| `+0x610,+0x614,+0x618,+0x61c,+0x620` | **5 coin denominations, high→low** |
| `+0x620` | doubles as the pending-copper accumulator (proceeds land here) |
| `+0x624` | wielded weapon item number |
| `+0x62c[20]` | **worn-item array**; `+0x67c[20]` per-slot uses/charges |
| `+0x6ba` | index of the currently-lit light source (0xff = none) |

### BANKBOOK record (per user, per bank)

| off | field |
|-----|-------|
| `+0x20` | bank id / key |
| `+0x24` | **balance** (on deposit) |
| `+0x28` | **outstanding borrowed** amount |
| `+0x2c` | dirty flag |

---

## 1. Currency model

Five coin denominations, stored high→low at player `+0x610, +0x614, +0x618,
+0x61c, +0x620`. Denomination **index 0 is the lowest coin (copper farthing)**;
index 4 the highest. `proper_currency_name(count, idx)` returns the singular/plural
label for index `idx` (0 = "copper farthing(s)" …). Inter-coin ratios are four
config globals: `DAT_00482cec` (idx0↔idx1), `DAT_00482ce8` (1↔2),
`DAT_00482ce4` (2↔3), `DAT_00482ce0` (3↔4).

**`convert_currency(d4,d3,d2,d1,d0)`** collapses a 5-denomination amount into a
single **copper (idx-0) total**: it folds each higher coin down through its ratio
(`d3 += d4*ce0; d2 += d3*ce4; d1 += d2*ce8; d0 += d1*cec`) and returns `d0`, with
per-step overflow guards. All prices are computed in copper.

**`check_currency(user, a4,a3,a2,a1,a0)`** returns true if the player's coins
(converted to copper) cover the requested amount (same high→low arg order).
**`deduct_currency(user, a4,a3,a2,a1,a0, tell)`** removes that copper value,
greedily spending the largest coins first and **making change** (breaking a higher
coin into lower ones when a lower drawer is empty); with `tell` set it prints the
"you hand over N coppers, M silver…" line. **`cleanup_currency(user)`** is the
inverse: it repeatedly rolls the copper accumulator at `+0x620` up into higher
denominations by the ratios, then normalizes each drawer. Selling / withdrawing
deposit raw copper into `+0x620` and then call `cleanup_currency` to mint proper
coins.

**Coin weight — `get_coin_weight`.** Every coin, regardless of denomination,
weighs **1/3 unit**: weight = `Σ floor(coins_in_drawer / 3)` over the five drawers.
Carried coin weight is separate from item weight (`player+0xb4`);
`get_encumbrance_percent` sums `coin_weight + item_weight`, ×100 / carry-cap
(`get_max_weight`, default 800, otherwise `player+0xb2` = the Str-derived cap from
`leveling.md`, adjusted by the Encum ability 0x60), and stores the % at `+0x708`.

---

## 2. Shop buy — `buy_item(shopId, argText)`

Entry: `cmd_buy` requires the player's room to be a shop (`room+0x43c==1`) and
passes the room's shop id (`room+0x440`). `buy_item` fetches the shop
(`get_shop_data`) and the player, then branches:

- **Healer shop (`+0xcc==5`)** sells services, not items:
  - `buy healing` → cost = `(maxHP − curHP) * 2` **copper** (`check_currency(…,0,cost)`),
    then fully heals (`curHP = maxHP`).
  - `buy curing` / `buy cure poison` → flat `0x19`(25) copper if actually poisoned
    (clears poison and terminates poison spell effects), or `0xf`(15) copper if not
    poisoned (a wasted-purchase message). No Charm/markup on healer services.
- **Trainer shop (`+0xcc==8`)** with arg `training` → delegates to `train_level`
  (see `leveling.md`; fee = `(shop+0xd2 + 100) * level*5 / 100`).
- Otherwise it resolves the item by name in the shop's stock
  (`get_shop_item_from_name`, which matches against the `+0xd8[20]` list and
  disambiguates multiple matches), then computes a price and completes the sale.

### 2.1 Base price and the buy formula

The item's base cost is `item+0x322` expressed in denomination `item+0x429`;
`convert_currency` turns it into copper (`base`). Then (`buy_item`,
around the pricing block):

```
if (base > 100000) { base /= 100; scaled = 1; }          // 32-bit overflow guard
price = ( (110 - Charm/5)                                 // Charm haggle factor
          * ( ((markup + 100) * base) / 100 ) )           // shop markup
        / 100;
if (scaled) price *= 100;
```

where `Charm = player+0xac`, `markup = shop+0xd2`. In words:

**buy price (copper) = base_cost × (markup+100)/100 × (110 − Charm/5)/100.**

- The **markup** term matches the trainer-fee interpretation in `leveling.md`
  (`(markup+100)/100`). A markup of 0 sells at cost; 50 at 1.5×; etc.
- The **Charm** term is the haggle: Charm 50 is neutral (`110 − 10 = 100` ⇒ ×1.0),
  Charm 100 ⇒ ×0.9, Charm 0 ⇒ ×1.1, Charm 250 ⇒ ×0.6. Higher Charm = cheaper.
- The `>100000` guard trades precision for range on very expensive goods (divides
  the base by 100 before the multiply, restores after).

`check_currency(user,0,0,0,0,price)` gates affordability; on success
`add_item_to_inventory` gives the item (charges seeded from `item+0x31e`),
`deduct_currency(user,0,0,0,0,price,1)` takes the coins, the shop's stock slot
`+0x150[i]` is decremented, and the shop is marked dirty (`+0x1dc=1`).

> **List price ≠ paid price.** `display_shop_items` prints
> `cost × (markup+100)/100` **without** the Charm factor — the shelf price is the
> markup price; the Charm discount/surcharge only applies at the moment of purchase.
> The list also shows current quantity (`+0x150[i]`), "Free" when cost is 0, and a
> per-item "You can't use" / "Too powerful" tag from `user_can_use`.

### 2.2 Stock handling and the special shop types

Ordinary shops decrement `+0x150[i]` on each sale. A stock slot with a **logical
world-object** backing it (`item+6 != 0`) must be removed from the room's object
pool first (`remove_logical_from_room`); if that fails the sale is refused. Two
special buyers reprice from the shop record instead of the item:

- **GHouse (`+0xcc==0xb`)** — the gang-house/deed shop. Price = `shop+0x178[i]`
  in denomination `shop+0x1a0[i]`; the buyer must be a **gang leader**, the gang's
  banked exp (`gang+0x28`) must reach `DAT_00482d10 * 10000`, and the buyer must not
  already own a gang item (ability `BadAttk` 0xb5 on any carried item ⇒ refused).
  On success the sale amount is credited to the gang leader's account
  (`deposit_gangleaders_account`) and the stock slot is retired.
- **GShop (`+0xcc==0xc`)** — gang shop; similar gang-leader gating, sets a
  pending-paperwork flag on the buyer.

Refusal codes map to messages: not-a-known-item, "cannot buy X here", "cannot
afford X", already-a-gang-owner, gang-exp-insufficient, must-be-gang-leader,
outstanding-paperwork.

---

## 3. Shop sell — `sell_item(shopId, argText, quoteOnly)`

Entry: `cmd_sell` (room must be a shop). `quoteOnly` (`param_3`) non-zero means
**appraise only** — print "You would get N for your X" and change nothing.

- **Gang shops (`+0xcc==0xb`)** buy nothing ("You may not sell items to a gang
  shop").
- The item must be found in the player's inventory (`find_item_in_inventory`).
- **Cursed items** (ability `Cursed` 0x52 or `CURSED` 0x53) that the player is
  currently **wearing** and holds only one of cannot be sold.
- The shop must list the item in its `+0xd8[20]` stock (`local_9` flag); otherwise
  "You cannot sell X here." (There is no item-type whitelist beyond the stock
  list — a shop buys back exactly what it is willing to stock.)

### 3.1 Sellback formula

Base cost is again `item+0x322` in denomination `item+0x429`, converted to copper
(`base`). Then:

```
sellPrice = ( (Charm/2 + 25) * base ) / 100;             // Charm = player+0xac
```

**sell price (copper) = base_cost × (Charm/2 + 25)/100.**

- Charm-scaled, **no markup involved.** Charm 50 ⇒ ×0.5 (half cost), Charm 100 ⇒
  ×0.75, Charm 0 ⇒ ×0.25. So sellback runs from a quarter of cost up toward
  three-quarters at very high Charm — always below the buy price.
- Proceeds are added to the copper accumulator `player+0x620` and minted into
  coins by `cleanup_currency`; the sold item is removed from inventory
  (`remove_item_from_inventory`).

### 3.2 What returns to stock

When the shop's current stock for that slot is below its max
(`+0x150[i] < +0x128[i]`) the sold item is put **back on the shelf**
(`+0x150[i]++`), so re-buying it is possible. If the shop is already at max the
item is consumed (not restocked). GShop (`0xc`) sales instead set a player flag and
do not touch stock counts.

---

## 4. Restock — `check_initiate_restocking` + `restock_items`

Restocking regenerates each stock slot toward its **max `+0x128[i]`** using the
per-slot triple; it never exceeds the max. Gang-house shops (`+0xcc==0xb`) are
skipped entirely.

**`check_initiate_restocking(shopId)`** runs over the 20 slots. For a stocked slot
(`+0xd8[i]!=0`):

- If **interval `+0x178[i] == 0`** (no timer): if `current < max`, roll
  `genrdn(1,100)`; if the roll `< probability +0x1c8[i]`, add
  `amount +0x1a0[i]` to `current`, clamped to `max`. This is an **immediate,
  probabilistic top-up** performed when restocking is triggered. If `current > max`
  it is clamped down.
- If **interval `+0x178[i] != 0`** (timed): clamp any overflow down to max, then
  **schedule a delayed restock event** at a random offset `genrdn(1, max(2,
  interval))`. The event (shop id, slot index, due `today`/`now`) is pushed onto a
  global linked list; `FUN_0045b856` computes the due timestamp as roughly
  `now + interval*60 s` carried into date/time fields.

**`restock_items()`** is the periodic drainer of that scheduled list: for every due
event it re-checks `current < max`, rolls probability `+0x1c8[i]`, and on success
adds `amount +0x1a0[i]` clamped to `max`. `display_all_restocking_required` dumps
the pending queue for debugging.

**Summary:** restock cadence is per-slot — either a probability-gated top-up each
time the shop is poked (interval 0), or a self-rescheduling timer of `interval`
minutes (interval > 0). Each event adds `+0x1a0[i]` units, capped at `+0x128[i]`;
`+0x1c8[i]` is the % chance the event actually fires. Nothing regenerates a slot
above its configured max.

---

## 5. Equipment / worn slots — `wear_armour`, `user_can_use`

### 5.1 Slots

Worn items live in `player+0x62c[20]` (item numbers) with a parallel
`player+0x67c[20]` uses/charges array; the wielded weapon is separate at
`player+0x624`. `wear_armour` finds the first empty `+0x62c` slot; if all 20 are
full it prints "no more room to wear." Each item declares a **worn-location code**
at `item+0x398` (byte); code 0 ⇒ "may not be worn."

**One-per-location, with two dual-slot pairs.** Before equipping, `wear_armour`
scans currently-worn items and removes any that occupy the same location
(`item+0x398` match, or an already-worn item of location code 1 which conflicts
with everything). Two location groups permit **two** worn at once — codes `4`/`0xd`
form one pair (e.g. finger/ring slots) and `0xe`/`0x11` another (e.g. wrist/ear);
`wear_armour` tracks counts (`local_1e`/`local_1f`) and only auto-removes an
existing one when a **second** of that dual type is already worn. A worn item that
is **cursed** (0x52/0x53) blocks equipping anything that would displace it (and
blocks removal in `remove_armour`). Location code `0xc` (shield/off-hand) cannot be
worn while a two-handed weapon (`item+0x394 ∈ {1,3}`) is wielded.

### 5.2 Use restrictions — `user_can_use`

`wear_armour` calls `user_can_use` (also used to wield, and re-checked wholesale by
`update_allowed_worn_items` after level/class changes — items that become illegal
are auto-removed). The gates:

- **Class-locked items** (`item+0x2f4==0xb`): usable only by classes in a fixed
  engine list.
- **Alignment**, via `get_legal_level(player+0x542)` tier (see `leveling.md` §1):
  good/evil/neutral tiers reject items carrying the opposing alignment abilities
  (`Good` 0x61, `Evil` 0x62, `Neutral` 0x70, `NotGood` 0x6e, `NotEvil` 0x6f, …).
- **Witchunter** (has `AntiMagic` 0x33) cannot use any item bearing `Magical`
  (0x1c).
- **Class list** `item+0x324[10]` — if any entry is set, `player+0x92` must be in
  it. **Race list** `item+0x344[10]` — if any entry is set, `player+0x90` must be
  in it. (A matching class/race is an early "allowed" that bypasses the
  proficiency checks below.)
- **Level:** ability `MinLevel` (0x87) rejects if `player+0x94 < value`; `MaxLevel`
  (0x88) rejects if `value < player+0x94`.
- **Armor** (`type==0`): requires class strength rating `class+0x46 ≥ item+0x396`,
  and a per-class switch on `class+0x44` governs whether shields (`+0x398==0xc`) are
  allowed. **Weapons** (`type==1`): a class-weapon-proficiency switch on
  `class+0x44` vs the weapon sub-type `item+0x394` decides usability (with a few
  hard-coded item exceptions).

### 5.3 Applying worn-item effects — `equip_apply_item_abilities`

On a successful wear, `wear_armour` calls `equip_apply_item_abilities` and then
`calculate_secondary_stats(user, 2)`. Critically, **`equip_apply_item_abilities`
applies only two effects imperatively**, by walking the item's `+0x2f6`/`+0x3e2`
ability table (20 entries):

- **`Alter HP` (0x58, id 88):** adds the ability value to **both** max HP
  (`player+0xae`) and current HP (`player+0xb0`). (Unequip subtracts from both.)
- **`GiveTempSpell` (0xa0, id 160):** grants the spell to the player's spellbook
  (`add_spell_to_spellbook`; unequip purges it).

**Every other worn-item modifier — AC, accuracy, resistances, stat bonuses,
crits, speed, DR, etc. — is NOT applied here.** Those are folded into the player by
`update_dynamic_stats`, which `calculate_secondary_stats` invokes first (see
`leveling.md` §5 and `spellcasting.md`): the engine **rebuilds** the dynamic block
(`+0x70a` accuracy, `+0x70c` AC, `+0x7b4` crits, the resistance fields, the six
stats, …) from scratch each recompute by re-scanning all worn items and active
spells. So `wear_armour` calling `calculate_secondary_stats(…,2)` after
equip/unequip is what actually makes armor and stat gear take effect; the two
imperative effects above are the exceptions that must be poked directly because
they change persistent quantities (HP pool, spellbook) rather than derived stats.
This is why `equip_apply_item_abilities` is tiny and the heavy lifting lives in the
stat rebuild — worn gear is essentially **declarative** data consumed by
`update_dynamic_stats`.

---

## 6. Item charges / uses — `deduct_item_charge`

Charges are per-instance counts stored alongside each inventory item:
`player+0x268[i]` for the main inventory (`+0xd8[i]`), `player+0x3fc[i]` for the
stackable (type-7) sub-list (`+0x334[i]`), and `player+0x67c[i]` for a worn slot.
The item's **default** charge count is `item+0x31e`, copied into the slot when the
item is first acquired (`add_item_to_inventory` with the "use default" sentinel).

`deduct_item_charge(user, itemPtr, *usesPtr)` decrements the matching slot's count
by one and writes the new value back through `usesPtr`. A value of `-1` means
"infinite / not charge-tracked" and is left alone. **When the count reaches 0:**

- If **`item+0x42a == 0`** (destroy-at-zero, the normal case), the item is
  **removed**: its weight is subtracted from `player+0xb4`, the inventory/worn slot
  is cleared, any equip effects are reversed (`unequip_apply_item_abilities`, and
  the wielded/worn references at `+0x624`/`+0x62c` cleared), a lit light source
  (`+0x6ba`) is extinguished, the logical object is dropped, and a message prints —
  either `item+0x418`'s WCCMSG text or the default "Its uses gone, %s disappears."
- If **`item+0x42a != 0`**, the item **persists at 0 charges** (a rechargeable /
  permanent-shell item); only the count hits floor.

So charges deplete one per use and, by default, the item **poofs at zero**; the
`+0x42a` flag is the opt-out that keeps a depleted item in inventory (e.g. for the
`Recharge` ability 0x79 to refill later).

---

## 7. Banking and lending

Banks are shops with `+0xcc==7`; a player may hold an account at each bank, keyed
by name into a per-user **bankbook** record (`get_bankbook_data`). Balance lives at
`bankbook+0x24`, outstanding debt at `+0x28`.

- **`deposit_gold(bankId, user, amount)`** — requires the coins on hand
  (`check_currency`), then, guarding against 32-bit overflow of the stored balance
  (deposit only if `balance + amount > balance`), moves `amount` copper from coins
  (`deduct_currency`) into `+0x24`. Over-cap deposits are refused ("the bank cannot
  accept your deposit"). **No interest is credited.**
- **`withdraw_gold(bankId, user, amount)`** — if `amount ≤ balance`, adds `amount`
  to the copper accumulator `player+0x620` (minted by `cleanup_currency`) and
  subtracts from `+0x24`. **No fee.**
- **`get_users_bankbook_balance(user)`** sums `+0x24` across **all** of the
  player's bankbook records (multi-bank total). `display_bank_balance` shows one
  bank's balance formatted as major.minor (balance and `balance % 100`).

### 7.1 Borrow / repay are a BBS-credit ↔ gold exchange, not an interest loan

`borrow_gold` and `repay_gold` do **not** compute interest. They convert the host
BBS's **real online-account credits** into in-game gold and back, at a fixed rate
`DAT_0047d538` (credits per gold):

- **`borrow_gold(bankId, user, amount)`** — gated on `haskey(DAT_00482da0)` (a
  borrow-privilege key). Verifies the account has enough BBS credit
  (`otstcrd`), **charges `amount * rate` credits** (`odedcrd`), grants `amount`
  gold to `player+0x620`, and records the debt at `bankbook+0x28`.
- **`repay_gold(bankId, user, amount)`** — takes up to the outstanding debt
  (capped at `+0x28`), **refunds `repaid * rate` credits** to the account
  (`odedcrd` with a negative delta), removes the coins (`deduct_currency`), and
  reduces `+0x28`.

So the "loan" is a way to buy game gold with paid BBS credits and sell it back;
`bankbook+0x28` only bounds how much you can convert back. The `display_shop_items`
bank screen labels these as "Deposit Rate" / "Loan Rate," but the deposit path pays
no interest in code — the loan rate is this credit↔gold exchange rate.

### 7.2 `deduct_exempt_credits` — the play-time credit meter (context, not in-game gold)

`deduct_exempt_credits` is the door-game's real-credit metering, unrelated to gold
coins: unless the user holds an exemption key (`DAT_00482dc4`/`DAT_00482db4`) it
debits `DAT_00482c88` BBS credits (`odedcrd`) per charge tick, and when the account
runs dry it kicks the player ("You have run out of credits for MajorMUD"). It uses
the same `odedcrd`/`otstcrd` online-account primitives that `borrow_gold` taps,
which is the only economic tie between the BBS billing system and the in-game
purse.

---

## 8. Summary

- **Buy:** `price = cost × (markup+100)/100 × (110 − Charm/5)/100` copper. Markup is
  `shop+0xd2` (matches the trainer formula); Charm (`player+0xac`) haggles — 50 is
  neutral, higher is cheaper. The shelf/list price omits the Charm term. Payment
  deducts across five coin denominations (copper..runic) making change as needed;
  stock slot `+0x150[i]` decrements.
- **Sell:** `price = cost × (Charm/2 + 25)/100` copper — Charm-scaled, no markup,
  roughly a quarter of cost at Charm 50 and never above buy price. Shops buy back
  only items on their stock list; cursed worn items and gang-shop sales are blocked;
  sold items return to the shelf if below max stock.
- **Restock:** per-slot regen toward max `+0x128[i]`; each event adds `+0x1a0[i]`
  units with `+0x1c8[i]`% chance; interval `+0x178[i]==0` = probabilistic top-up on
  demand, else a timer of `interval` minutes. Never exceeds max. GHouse shops don't
  restock (those fields reused as price/denomination).
- **Equip:** 20 worn slots (`+0x62c`) + separate wield (`+0x624`), one item per
  location (`item+0x398`) with two dual-slot pairs allowing two. `user_can_use`
  enforces class/race lists, min/max-level abilities, alignment, Witchunter/Magical,
  armor strength (`class+0x46` vs `item+0x396`) and class weapon proficiency.
  `equip_apply_item_abilities` applies only Alter-HP (0x58) and GiveTempSpell (0xa0)
  imperatively; **all other gear bonuses are rebuilt by `update_dynamic_stats`
  inside `calculate_secondary_stats(…,2)`**, which `wear_armour` calls after every
  equip/unequip.
- **Charges:** one-per-use decrement of the per-instance count; item is destroyed at
  0 unless `item+0x42a` is set (rechargeable shell).
- **Banking:** deposit/withdraw are fee-free and interest-free; borrow/repay are a
  fixed-rate exchange of real BBS credits for in-game gold (debt tracked at
  bankbook `+0x28`), not an interest-bearing loan. Coins weigh 1/3 each regardless
  of denomination.

## 9. Undetermined / flagged

- Exact **coin ratios** (`DAT_00482ce0/ce4/ce8/cec`) and the highest-coin name are
  config constants not resolved to values here; the canonical MajorMUD set is
  copper/silver/gold/platinum/runic (idx 0→4), consistent with the code, but the
  numeric ratios were not read from data.
- The `>100000` copper **overflow guard** in `buy_item` loses low-order precision on
  very expensive goods; the exact threshold behavior for items priced near it is a
  minor rounding quirk, not separately verified against play.
- **Worn-location code meanings** (`item+0x398`: which body slot each number is) are
  inferred structurally (dual pairs 4/0xd and 0xe/0x11; 0xc = shield/off-hand;
  1 = full-cover/conflicts-with-all); the exact head/neck/finger labels are not
  pinned to WCCITEMS data.
- `class+0x44` (weapon/armor proficiency group) and `class+0x46` (armor strength
  rating) are named from `user_can_use` usage; a WCCCLASS disk pass would confirm
  widths.
- The **GHouse/GShop** gang-economy paths (deed price at `shop+0x178`, gang-exp
  gate `DAT_00482d10*10000`, `deposit_gangleaders_account`) are described from
  `buy_item` but the full gang-house lifecycle is out of scope here.
</content>
</invoke>

---

## Oracle addendum (Bank of Godfrey expedition, 2026-07-16)

- **Coin ratios VERIFIED** (the bank lobby prints them): 10 copper = 1 silver,
  10 silver = 1 gold, **100 gold = 1 platinum, 100 platinum = 1 runic** —
  i.e. `DAT_00482cec..ce0` = [10, 10, 100, 100], not uniform.
- `balance`: "Your balance at Bank of Godfrey (#8) is:" / "On deposit: N
  copper farthings [G. S gold crowns]". Outside a bank, balance prints
  nothing; deposit/withdraw print "You cannot DEPOSIT/WITHDRAW if you are
  not in a bank!".
- `deposit N` reports the coins actually handed over, **largest first**
  ("You deposit 10 silver nobles." for 100 from an 11s+49c purse — so
  `deduct_currency` spends whole large coins first). `withdraw N` prints
  "You withdrew N copper farthings." and the purse is minted upward
  (`cleanup_currency`) afterward. Over-withdrawal is **silent**; junk
  amounts get "Please specify a more reasonable amount.".
- Coin pickup: "You picked up 11 silver nobles" (no period observed).
  Coins render inside the carrying line, high→low, before items.
- **Exit type 10 = text-triggered exit**: hidden from Obvious exits; its
  `para1` is a message holding pipe-separated trigger phrases
  ("borrow skiff|go skiff|row skiff"); walking the direction plainly says
  "There is no exit in that direction!". The Newhaven→Silvermere ferry is
  one ("You climb into one of the skiffs, and row to Silvermere.").
- Newhaven is the tutorial pocket (14 rooms); Silvermere the first town
  (24+ prefixed rooms; Town Square = room 224 = the tournament start).

## Addendum — restock mechanism fully pinned (2026-07-16, decompile pass)

- **Boot**: `check_initiate_restocking` is called once for shops 0..199
  right after startup (guarded by `DAT_00482138` in the polling routine).
  So the interval-0 "probabilistic top-up when poked" happens at **boot
  only** — there is no per-transaction poke. On a MajorBBS board that
  meant once per nightly cleanup/restart.
- **Sweep cadence**: `restock_items()` hangs off `background_slow` behind
  counter `DAT_0047fb80` — it runs on every **21st slow tick, i.e. every
  630 s (~10.5 min)**. A due event on a live non-gang shop (type != 0xb)
  re-checks `current < max`, rolls `genrdn(1,100) < percent`, adds
  `amount` clamped to max, sets the shop-dirty byte (+0x1dc), then
  **reschedules itself +interval minutes whether or not the roll
  succeeded** (`FUN_0045b856` adds minutes to the packed DOS date/time
  pair at event +8/+0xc; events live forever on the `DAT_0048bdc0` list).
- **Rarity arithmetic**: expected time per unit = interval / percent.
  Slowest slots in the data: shadow cloak (Dreary Shop) 720 min @ 1% ≈
  50 days/unit; etched adamant warhammer/broadsword, ruby earrings,
  warpblade, prismatic gear all 720 min @ 2-3% (max 1 on the shelf).
  869 slots carry timers in total.
- **First-run state** (user testimony): on the first run of the world
  every item is available in its shop. The extracted `shopnow` columns
  disagree for 411 of 1,203 slots **because the .VIR is a played-board
  snapshot** — stock persists to Btrieve via the dirty byte. `shopnow`
  is runtime state, not content; a fresh world boots with `now = max`.

## Addendum — shelf persistence & the daily cleanup (2026-07-16)

- Shop stock **persists** in the original: restock/clamp paths set the
  shop dirty byte (+0x1dc) and the record is written back to Btrieve.
  The played-board .VIR (411 dented slots) is the proof.
- `check_initiate_restocking`'s run-once guard (`DAT_00482138`) is never
  reset within a process — the interval-0 "top-up" branch runs **once
  per module load**. Boards felt a nightly restock because Worldgroup's
  nightly cleanup restarted the module. A standalone server that never
  restarts must emulate this with a 24 h job re-running the same
  reconciliation (clamp overstock down to max; one probability-gated
  top-up for dented interval-0 slots). No timed slot's interval exceeds
  720 min, so leaving the event list running continuously instead of
  re-randomizing it daily is behaviorally indistinguishable.

## Addendum — M4 VERIFY tags cleared (2026-07-16, oracle_m4_verify.raw)

Oracle purse patched via MBBSEmu's sqlite-converted WCCUSERS.DB (drawers
found at disk 0x603..0x613 high→low — the stored record is the DOS
layout, offsets differ from the WG3-NT in-memory struct; anchored by the
known 1g 9c purse before patching).

- **Shop list rows**: `name %-30` + `qty %-10` + price VALUE right-aligned
  width 4 + ` ` + denomination label. The value is the shelf price in the
  item's OWN cost denomination: `cost × (markup+100)/100` ("4 gold
  crowns" for a 2-gold lantern at markup 100). Free rows: three-space
  gutter + "Free". Quantity column shows live stock.
- **Paid buy**: "You just bought X for <coins>." where <coins> is the
  multiset actually handed over — `deduct_currency` (0x1edca) exact:
  repeat { greedy high→low spend while one whole coin ≤ remainder; break
  ONE coin of the smallest non-empty denomination above copper } until
  paid. Change-making is player-visible: the same 104c sickle printed
  "9 silver nobles, 14 copper farthings" and then "1 gold crown, 4
  copper farthings" on consecutive buys (both traced and matched
  exactly). Buying does NOT re-mint the purse.
- **Sell**: "You sold X for N copper farthings." (proceeds always in
  copper; sellback = base × (Charm/2+25)/100), then `cleanup_currency`
  re-mints the ENTIRE purse greedily (496 gold became 4 platinum...).
- **Out of stock**: "You cannot buy X here!"
- **Equip**: arm/wield/equip all print "You are now holding X." (1H and
  2H alike); re-arm: "You do not have X left unequipped." Minimum
  abbreviations: ar=arm, wie=wield (wi says!), eq=equip, wea=wear
  (we says), rem=remove (re says). Wear: "You are now wearing X.";
  remove: "You have removed X."
- **Carrying line**: coins high→low, then WORN items suffixed with their
  wear-location name ("chain coif (Head)") from the location table at
  0x4801dc (Nowhere, Worn, Head, Hands, Finger, Feet, Arms, Back, Neck,
  Legs, Waist, Torso, Off-Hand, Wrist, Ears, Eyes, Face), then the armed
  weapon — "(Two handed)" iff type==1 && weapontype∈{1,3}, else
  "(Weapon Hand)" — then loose items grouped with a count prefix
  ("3 sickle", singular name).
- Still unverified: cannot-afford wording, crit message, caster
  prompt/Mana sheet line (M5), encumbrance descriptor bands.

## Addendum — healer services & user_can_use verified (2026-07-16)

- **Correction to §2**: the healer curing constants ride in the SILVER
  argument of check_currency — live cost when not poisoned is 15 silver
  = 150 copper ("You hand over 1 gold crown, 4 silver nobles, 10 copper
  farthings and find that you were not poisoned!"). Healing: "You hand
  over 8 copper farthings and all your wounds are healed." ((max−cur)×2
  copper; at full HP: "You hand over nothing and ..."). The hand-over
  list is deduct_currency's spent multiset.
- **Shops only operate in rooms with `type == 1`** (room+0x43c): the
  Silvermere Temple Healer (room 527, type 3, shopnum 4) refuses LIST
  ("You cannot LIST if you are not in a shop!") and lets `buy healing`
  fall to say; the healer NPC there answers said phrases instead (the
  textblock system — "The healer casts cure poison on you!"). The
  shop-active healer is Newhaven 2190 (type 1).
- **List rows**: zero-stock rows are HIDDEN and the header prints lazily
  on the first stocked row — an empty(-stock) shop lists nothing at all
  (why healer LIST is silent). Rows failing user_can_use append
  " (You can't use)" (spells get " (Too powerful)" — M5).
- **user_can_use (0x1fced)**: buying is NOT gated (a Warrior can buy the
  Paladin-only amulet); equipping is: wear → "You may not wear that
  item!", arm → "You may not use that weapon." Order: type 0xb needs a
  config class (no type-11 items ship); alignment ability gates
  (Good/Evil/NotGood/NotEvil/Neutral vs get_legal_level band — awaits
  the crime system); AntiMagic(51) player vs Magical(28) item; MinLevel
  135 / MaxLevel 136 (value arrays); item class_1..10 / race_1..10
  allowlists (a match BYPASSES the matrix, a miss refuses); then armor:
  class.armour >= item.armour, weapon codes 0/2/4 forbid Off-Hand
  (wornon 12); weapons: class.weapon code = capability set {0..3 that
  class, 4=1H, 5=2H, 6=sharp, 7=blunt, 8=all, 9=only config items
  quarterstaff(100)/dagger(68) — Mage/Mystic}. Other item types pass.
