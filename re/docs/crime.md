# MajorMUD 1.11p — CRIME / FAME / LEGAL-LEVEL system (WG3-NT)

Behavioral spec of the evil-points ("fame") system: who writes `player+0x542`, what
each legal tier gates, the retaliation-timer machinery, forgiveness, and persistence.
Monster-side aggression consequences (guardian mode 6, roam-class-5 hunters) are
specified in [`monsters.md`](monsters.md) §aggression and only summarized here. The
quest-VM verbs (`addevil`, `goodaligned`/`evilaligned`) are specified in
[`quests.md`](quests.md) and cross-referenced. Record offsets cross-checked against
[`records.md`](records.md).

Reconstructed from the clean 32-bit **WG3-NT** decompile
(`../wg_nt_ghidra/exports/WCCMMUD_decompiled.c`, line numbers below refer to that file).
Function addresses are image-base 0x400000. Literal strings and the legal-level name
table were pulled from the binary itself
(`../wg_nt_ref/WCCNT8PJ/out/wccmmud.dll`), since the decompile export contains no
data section.

---

## 0. The fields

| field | type | meaning |
|-------|------|---------|
| `player+0x542` | **signed i16** | evil points ("fame"). Positive = evil, negative = good, 0 = neutral. Older docs call it alignment/evil; this offset is the single source of truth (it lives inside the player record — see `records.md` §character record, `+0x542 alignment/evil`). |
| `player+0x544` | i8 | way-of-life byte. `0` = normal, `-10` (0xF6) = **committed Lawful** (chosen at creation or set by sysop). Only ever written 0 or 0xF6 in this build (3519, 3535, 10883, 60608, 60621). Every read is `== -10 && DAT_00482dcc` (the Lawful feature toggle, §9). |
| `player+0x700 & 0x10` | flag | **"Warn on Evil"** user setting — when set, any action that would grant evil points is *refused* instead ("To do this action, you must turn off your evil warnings." 47457-47462). Toggled by `SET EVIL` in `cmd_set` (54203-54212: "You will now be warned and stopped from performing evil actions"), shown by `display_profile` ("Warn on Evil: …", 35441-35448) and by a `%w` substitution code (`substitute_string` 70786-70791). Set **on** for new characters (`create_player` writes `0x700 = 0x10`, line ~53795 region of the create path). |
| `player+0x700 & 0x40` | flag | set every time evil points are actually added (47484, 47571) — never read anywhere in this DLL (§10). |
| `player+0x7d2` | i16 | day-stamp (`today()`) written when evil points are banked to GENBB (13221-13222, 67657-67658); drives the once-per-day decay multiplication in `save_evil_points` (§8). Shared with the `WCC PERMANENT USER` record day-stamp (69998, 70161). |
| `DAT_00488180` | list head | in-memory **evil-pair timer list** (§4). Not persisted. |

---

## 1. Legal levels — `get_legal_level` (0x44e390, lines 47337-47368)

Pure threshold function on the signed fame value:

| fame range | level | display name | scan color (`PTR_DAT_00488184[i]`) |
|-----------:|:-----:|--------------|-------------------------------------|
| ≤ −201 | 7 | `Saint`   | bright white (`1;37`) |
| −200 … −51 | 6 | `Good` | bright white (`1;37`) |
| −50 … 29 | 0 | `Neutral` | cyan (`36`) — the WHO list prints *no* word for level 0 (34986-34999) |
| 30 … 39 | 1 | `Seedy` | white (`37`) |
| 40 … 79 | 2 | `Outlaw` | red (`31`) |
| 80 … 119 | 3 | `Criminal` | yellow (`33`) |
| 120 … 209 | 4 | `Villain` | bright yellow (`1;33`) |
| ≥ 210 | 5 | `FIEND` | bright red (`1;31`) |

Decompile literals: `< -200 → 7`, `< -0x32 → 6`, `< 0x1e → 0`, `< 0x28 → 1`,
`< 0x50 → 2`, `< 0x78 → 3`, `< 0xd2 → 4`, else `5` (47346-47367).

* Name table at `0x4881a4` (8 pointers, order Neutral, Seedy, Outlaw, Criminal,
  Villain, FIEND, Good, Saint — indexed directly by the level number). Color table
  immediately before it at `0x488184`. Both extracted from the DLL.
* **`Lawful` override**: everywhere a tier name is displayed, `player+0x544 == -10`
  (with the feature enabled) prints the separate string `Lawful` (`0x484d50`) instead
  of the tier word — WHO list (35004-35011, 35049-35056) and the public-userinfo API
  (`retrieve_public_userinfo` 71362-71368, which reads the saved record's copy of the
  two fields directly).
* Frequently-recurring raw compares in other code use the tier *boundaries* directly:
  `fame < 0x28` = "not yet a criminal" (Neutral/Seedy), `fame >= 0x50` = Criminal+.

---

## 2. The central writer — `add_evil_points` (0x4e49c, lines 47439-47583)

`add_evil_points(attacker_usernum, victim_usernum_or_-1, points, timer_rounds, rob_flag)`.
Every gameplay-driven fame increase goes through this one function. Return value:
**nonzero = the action was refused** (callers abort the attack/rob/cast), 0 = proceed.

Skipped entirely when `DAT_004906c9 == 2` (closed/demo game state, 47453).

### 2.1 Refusal gates (both the NPC and the player-victim path)

Checked in order; each prints its message and returns 1:

1. `player+0x700 & 0x10` (Warn on Evil) → "To do this action, you must turn off your
   evil warnings." (47457-47462, 47509-47514)
2. `fame > 300` → "You have progressed too far to the evil side to do this action."
   (47463-47468, 47515-47520). **300 is the action ceiling**: above it you cannot
   commit further evil at all. (Load-time hard cap is 312, §6.4.)
3. own `+0x544 == -10` (committed Lawful, feature on) → "You have chosen a way of life
   which does not allow this action." (47469-47474, 47521-47526)

Otherwise: "A dark cloud passes over you" (47475-47477, 47527-47529).

### 2.2 Victim = NPC (`victim == -1`, 47456-47485)

No pair timer, no multiplier. Apply §2.4 add.

### 2.3 Victim = player (47486-47572)

* `rob_flag == 0` (attack path): consult `should_give_evil` (§4.3). Result 0 →
  **return 0 immediately, no evil** — this is the free retaliation window. Result 1 →
  the existing rob-timer for the pair is replaced (`remove_single_timer` 47569-47571).
* `rob_flag != 0` (rob path): always proceeds; an existing "unnoticed rob" node
  upgrades the flag (47502-47506).
* **Victim innocence gate**: points are only awarded when the *victim's* fame `< 0x1e`
  (30, i.e. victim is inside the Neutral band) — otherwise `points = 0` (47508,
  47566-47567) though the pair timer is still (re)created.
* **Victim-quality multiplier** (47530-47555): victim committed-Lawful → ×3 (a 0-point
  rob becomes 10); victim Saint (≤ −201) → ×3; victim Good (≤ −51) → ×2. Each
  multiplication overflow-guards to ±32000.
* `add_evil_timer(attacker, victim, rounds, points, rob_flag)` records the pair (§4.1).

### 2.4 The add itself (both paths)

* **Minimum-10 bump**: a good-side attacker (fame < 0) whose `fame + points < 10`
  jumps straight to 10 (`points = 10 - fame`, 47478-47480, 47557-47559) — one evil act
  erases any Saint/Good standing.
* Cap: only applied if it increases fame and `fame < 30000` (47481-47483, 47560-47562).
* `+0x700 |= 0x40` (47484, 47571).
* If the add changed the legal level, `update_allowed_worn_items` re-validates equipped
  alignment-restricted gear (47574-47578).

### 2.5 Call sites (the complete evil-source table)

| action | call | points | lines |
|--------|------|-------:|-------|
| `attack <player>` (non-arena, PvP-eligible) | `attack_user_user` → `add_evil_points(a, v, 10, 0xb, 0)` | 10 | 25569 (gated at 25562-25568 by `FUN_0046c417`, §7.5, "Such an attack would result in a very unbalanced combat round.") |
| offensive spell at a player | `cast_user_target` ×3 | 10 | 41455, 41493, 41591 (same `FUN_0046c417` gate + arena exemption, 41440-41456) |
| `rob <player>` | `rob_user` → `add_evil_points(a, v, **1**, 0xb, 1 or 2)` | 1 | 17185 (bump-fail, noticed → flag 1), 17199 (skill-fail → flag 2), 17207 (success → flag 2) |
| `attack <monster>` that is passive (mode 0/4) and hasn't engaged you | `attack_user_monster` → `add_evil_points(a, **-1**, 10, 0xb, 0)` | 10 | 26116 (gate 26113-26116: skipped when mode ∉ {0,4}, when the monster is the player's own summon, or when `mon+0x12e` already targets this user) |
| offensive spell at a passive monster | `cast_monster_target` ×3 | 10 | 43255, 43330, 43417 — but see the note below: only **43255** and **43417** are `spelltype`-gated; **43330** keys off the spell's ability instead |
| area/no-target offensive spell | `cast_no_target` → `add_evil_warnings_to_room` (0x3fa18) | 10 once + 0-point pair timers | 39216, 39245, 39313 → 38774-38812. One 10-point NPC-style hit for the room (38774, 38784, 38806), then `add_evil_points(caster, victim, **0**, 0xb, 0)` per would-be-innocent player target (38790) so each victim gets a retaliation window without extra points. Uses `is_valid_target` (38294-38318: victim fame `< 0x28` **and** `should_give_evil`) and `is_valid_monster_target` (38430-38505; note 38489-38493: roam-class-5 and class-0x25 monsters are *excluded* as area-spell targets when the caster's fame `< 0x28` — innocents cannot accidentally aggro hunters/free-roamers). |

`rob_monster` (0x1fc96, 17442-17458) gives **no** evil points; its only crime check is
the committed-Lawful refusal (17454, "You have chosen a way of life which prevents
this action." — same message that blocks `rob_user` for Lawful players, 17163-17167).

#### 2.5.1 `cast_monster_target` 43330 — the ability-52 arm (**LANDED, M7 slice 8**)

The three `cast_monster_target` call sites are **not** the same gate. Two are
hostility-gated the way the row above implies; the middle one is not:

| line | enclosing gate |
|---|---|
| 43255 | `param_4 != 0` (autocombat re-fire) **and** `spelltype < 3` |
| 43417 | `spelltype < 3` (the engage-and-stop block) |
| **43330** | **neither** — it sits inside the spell's ability scan, on `ability == 0x34` |

43323-43347 in full:

```c
else if (((uVar7 == 0x34) && (room+0x43c != 5 || DAT_004790e8 == 0)) &&
         ((mon+0x106 == 0 || mon+0x106 == 4) && sameas(mon+0x1a, user+0x1e) == 0)) {
  if (add_evil_points(param_2, -1, 10, 0xb, 0) != 0) return 0;   // refusal
  if (/* not no-grudge, not class 0x25, roll/mode gate, not class-5-with-grudge */) {
    mon[0x50] = 1;                            // grudge flag
    strcpy(mon+0x1a, user+0x1e);              // remember the caster
    mon+0x116 = 0;                            // clear suppression
  }
}
```

Ability **0x34 (52) is `EvilInCombat`**. The predicate is otherwise identical to
`attack_user_monster`'s (26113-26116) and to 43255's: non-arena, monster mode
∈ {0, 4}, and the monster is not already holding a grudge against this caster.

So in the DLL, cursing a passive monster is a crime **because of what the spell
carries**, not because of its `spelltype`. Census (`re/mmud_wgnt.sqlite`, 207
LearnSp-taught spells): 45 learnable spells carry ability 52 — 27 at match 8,
2 at match 0, 16 at match 12. Of the 29 learnable **benign** (`spelltype` 3)
single-target match-4/6/8 spells, **25** carry it: curse, greater curse,
wrathful curse, blind, slow, hold person, confusion, sleep, entangle, mute,
senselessness, vulnerability, damnation, divine disfavour, rotting flesh,
partial petrification, burning aura, creeping doom and the seven songs.

**Landed (M7 slice 8).** The 43330 twin sits in `cmd_cast`'s monster arm ahead
of the SpellImmu gate (the DLL's scan-before-43380 order), keyed on the spell
carrying 52; `charge_passive_monster_evil` returns a tri-state so the
43335-43346 grudge body (`retaliation_lock` — the same function every other
twin uses) fires only on a real charge. The area side landed in the same
slice: the per-slot 39313 arm (one 10-point hit, whole-cast abort on refusal,
NO grudge writes — 38759-38812 has none) plus the 39164 protection gate's
has-52 half. Six single-target pins and four area pins in `crime.rs`.
KNOWN-DIVERGENCE (documented at `cast_eligibility_refused`): our refusal scan
runs before the 52 charge instead of slot-interleaved — one learnable
exception (35 poison bolt, slots 17/52/151/108: the DLL charges then refuses
at a passive NonLiving target; we refuse free), re-open if a content patch
makes another 52-before-refusal spell learnable.

### 2.6 Complete writer census of `player+0x542`

Every store found by exhaustive grep of `0x542` over the decompile:

| writer | lines | delta / value |
|--------|-------|---------------|
| `add_evil_points` | 47483, 47562 | `+points` per §2 |
| `attempt_to_forgive` | 48096, 48112 | `-banked points` (§5) |
| quest VM `addevil` (`perform_matched_action` 0x70209) | 68903-68904 | `+n` (signed; the *only* gameplay path that can lower fame besides forgiveness) — see `quests.md` §action verbs |
| `cmd_forgive` path | — | via `attempt_to_forgive` only |
| character creation good-path prompt (`ljngame_sttrou` state 0x3a) | 3519-3520 (`Y`: `+0x544=0xF6`, fame=**−51**), 3535-3537 (`N`: `+0x544=0`, negative fame clamped to 0) | asked only when fame < 1 and the Lawful feature is on (2954) |
| `create_player` | 10993-10994 | restore banked account evil: `fame = get_saved_evil_points(name)` (§8) |
| `load_player` version migrations | 10504-10546 (record version `+0x720` < 6: tier-bucket clamp 0x1d/0x27/0x4f/0x77/0xd1/299, "Some of your evil points have been forgiven…"), 10545-10546 (< 4: out-of-range → 300), 10617-10618 (< 10: 0x1e-0x27 band → 0x1d), 10626-10627 (< 0xb: `< -200 → -200`, "You will have to complete a quest to regain your Saint status.") | one-time record-format migrations, **not** recurring decay |
| `load_player` unconditional cap | 10660-10661 | `fame > 0x138 (312) → 0x138` on every load |
| sysop `SYSOP GOD <user> GOOD/SAINT/NEUTRAL/LAWFUL/UNLAWFUL` (`cmd_sysop`) | 60572 (−51), 60584 (−201), 60596 (0), 60609+60608 (Lawful: `+0x544=0xF6`, −51), 60621-60623 (unlawful: `+0x544=0`, fame raised to ≥ 0x28) | absolute sets; verb strings `good/saint/neutral/lawful/unlawful` at 0x48d6be-0x48d6d8 |
| sysop `SYSOP GOD <user> ADD EVIL <n>` | 60863-60864 (`+n`), 60869 (2-arg form: set 0) | |
| sysop self `… EVIL <n>` | 61117-61118 (`+n`, sets the EDITED flag `+0x6fc`) | |

No fame write exists in `check_kill_monster` / `distribute_experience` / any kill path:
**killing grants no evil — the evil is charged when the attack is initiated** (§2.5).
There is likewise **no tick-driven fame change**: `slow_update_character` (19497-19516)
only ages the pair timers (`decrement_evil_timers`, 19514) and makes a dead
`get_legal_level` call whose result is discarded (19515).

---

## 3. Blocking messages / strings (from the DLL, cited by address)

* `0x4882b0` "To do this action, you must turn off your evil warnings."
* `0x4882ea` "You have progressed too far to the evil side to do this action."
* `0x48832b` "You have chosen a way of life which does not allow this action."
* `0x488381` "A dark cloud passes over you"
* `0x47f77f` "You have chosen a way of life which prevents this action." (rob)
* `0x4817c4` "Such an attack would result in a very unbalanced combat round."
* `0x47f7df` "Such an action would result in a very unbalanced game." (rob)
* `0x47e31e` / `0x47e349` "You are too good/too evil to go through this exit!"
* `0x47de2a` "You may not enter that room during a retaliation time-period."
* `0x48a935/62/8c` forgiveness results (§5)

---

## 4. The evil-pair timer list (`DAT_00488180`)

### 4.1 Node layout — `add_evil_timer` (0x4e918, 47682-47716)

24-byte (`alczer(0x18)`, 47673) singly-linked nodes, appended at tail:

| off | field |
|----:|-------|
| +0x00 | attacker usernum |
| +0x04 | victim usernum |
| +0x08 | rounds remaining (i8; **always 0xb = 11** at every call site) |
| +0x0c | evil points banked by this act (0 for area-spell markers and refused-gate re-adds) |
| +0x10 | flags: bit0 = rob (vs attack), bit1 = *unnoticed* rob |
| +0x14 | next |

A node is only created if the pair has none (`already_evil` guard, 47691).

### 4.2 Aging — `decrement_evil_timers` (0x4e9f3, 47756-47776)

Called once per **attacker's** 30 s slow tick (`slow_update_character` 19514): all of
that attacker's nodes lose 1 round; expired nodes are freed (`FUN_0044e98e`,
47720-47748). 11 rounds × 30 s ⇒ the retaliation/forgiveness window is ≈ 5½ minutes
of the attacker's play time.

### 4.3 Queries

* `already_evil(a, v)` (0x4e859, 47587-47612): 0 = no node; 1 = attacked;
  2 = rob (noticed); 3 = rob (unnoticed).
* `should_give_evil(a, v)` (0x4e45f, 47409-47433): if the *victim* has a live node
  against the attacker → 0 (**retaliation is free**); else if the attacker already has
  an attack node vs the victim → 0 (already charged); a rob node → 1 (charge again,
  replace timer); no node → 2 (fresh charge).
* `evil_for_robbing(a, v)` (0x44e89f, 47618-47638): true iff a rob-flagged node
  (a→v) exists. **Dead code in this DLL — zero call sites** (the task brief's note
  that it writes fame is wrong: it neither writes `+0x542` nor is it called; it is a
  56-line list query, presumably kept for another module).
* `is_in_retaliation(u)` (48131-48147): u has *any* node **with nonzero banked
  points** as attacker. Gates: entering a lawful-flagged room (`room+0x564 & 1`,
  `user_allowed_in_room` 11715-11721 and arena entry 11738-11743, message 0x47de2a)
  and `SYSOP GOTO newhaven/silvermere/support` (60065-60067).
* `display_evil_star(a, viewer)` (0x4e8d7, 47646-47668): 0 (suppress) iff the a→viewer
  node has the *unnoticed-rob* bit; else 1.
* `display_evil_timers(viewer, u)` (0x4ea29, 47783-47812): sysop scan detail —
  "%s %s %s %d rounds to go." with verb `robbed`/`attacked` (0x4883ba/0x4883c1);
  invoked from the sysop deep-scan (`display_scan_of_users` mode 2, 35097).

### 4.4 The room-list star

`display_room_desc` (33247+) appends `*` (0x483536) to another player's name iff
(either-direction pair node exists **or** their fame ≥ 0x28) **and**
`display_evil_star` allows it (33340-33354, hidden-player variant 33366-33380). Net
effect: criminals and anyone who attacked/visibly robbed you are starred; a rob you
never noticed leaves no star.

---

## 5. Forgiveness — `cmd_forgive` (0x5846a, 53780-53834) / `attempt_to_forgive` (0x4ef1e, 48064-48127)

The **victim** types `FORGIVE <attacker>` while the attacker's timer is still live.
`attempt_to_forgive(attacker, victim)` finds the attacker→victim node and executes

```
attacker.fame -= node.points;  node.points = 0;
update_allowed_worn_items(attacker);  free(node);
```

(48096-48101 head-node case, 48112-48119 list case) — an exact reversal of what §2.4
added (including any victim-quality multiplier, since the *multiplied* value was
banked). Success: "The gods have forgiven you for your action." / broadcast
"The gods have forgiven %s for %s action."; no node → "The gods refuse to forgive %s
for %s actions." (53815-53827). Debug listing of the walk is printed for sysops
(48090-48094).

---

## 6. What the legal level / raw fame gates (reader census)

### 6.1 Item & spell alignment abilities — `user_can_use` (0x1fced, 17515-17539) and `user_can_use_spell` (0x2029b, 17811-17846)

Both apply the identical lattice on `get_legal_level(fame)` before any class/race
checks. Ability codes per [`ability_ids.tsv`](ability_ids.tsv): 97 `Good`, 98 `Evil`,
110 `NotGood`, 111 `NotEvil`, 112 `Neutral`, 113 `NotNeutral`.

Truth table — ✗ = object with that ability is **refused**:

| legal level | Good (0x61) | Evil (0x62) | NotGood (0x6e) | NotEvil (0x6f) | Neutral (0x70) | NotNeutral (0x71) |
|---|:-:|:-:|:-:|:-:|:-:|:-:|
| 0-1 Neutral/Seedy (`level < 2`) | ✗ | ✗ | ✓ | ✓ | ✓ | **✓ (not enforced!)** |
| 2-5 Outlaw…FIEND (`level-2 < 4`) | ✗ | ✓ | ✓ | ✗ | ✗ | ✓ |
| 6-7 Good/Saint (`level-6 < 2`) | ✓ | ✗ | ✗ | ✓ | ✗ | ✓ |

Decompile: neutral branch tests 0x62 then 0x61 (17517-17524 items / 17813-17827
spells); evil branch 0x61, 0x6f, 0x70 (17525-17532 / 17828-17838); good branch 0x62,
0x6e, 0x70 (17533-17539 / 17840-17846). **No code anywhere tests ability 0x71** — the
documented `NotNeutral` semantics are not implemented in this DLL (§10). The
committed-Lawful byte plays no part in equipment gating — only the fame-derived level.
`update_allowed_worn_items` re-runs these checks whenever a writer crosses a tier
boundary (§2.4, §5, load 10666).

### 6.2 Angel summons — `dismiss_users_angels` (0x203db, 17857-17882)

On logout/leave (`cleanup_when_user_leaves` 18325): if the player's legal level is 4
or 5 (Villain/FIEND), every live class-0x25 monster bound to their name is dismissed.

### 6.3 Movement gates

* **Alignment-restricted exits** (exit type 0x14, `move_user` 12433-12448):
  `fame < room.exitParamA[dir]` (`+0x374 + dir*4`) → "You are too good to go through
  this exit!"; `fame > room.exitParamB[dir]` (`+0x39c + dir*2`) → "You are too evil…".
* **Lawful rooms** (`room+0x564 & 1`): no entry while in combat or in retaliation
  (11709-11721); PvP/rob initiation inside is blocked (`attack_user_user` 25557-25560,
  `rob_user` 17181-17183) unless sysop/closed-game.
* **Arena** (`room+0x43c == 5` with `DAT_004790e8`): PvP is evil-free (25562, 41440,
  41454) and needs ≥ half health + no retaliation to enter (11726-11743).

### 6.4 Death, respawn, load

Respawn map by criminality — `fame < 0x28` → map `DAT_00482cfc` (init: 0x88d = 2189),
else map `DAT_00482d00` (init: 0x8e = 142, the outlaw start): `check_kill_user`
13020-13024, `commit_suicide` 67454-67458, `load_player` 10399-10403 and 10669,
room-desc fallback 33282. Both defaults hardcoded in `init__wccmmud` (558-559,
validated 605-613). Load-time clamps and the 312 cap: §2.6.

### 6.5 PvP eligibility — `FUN_0046c417` (0x46c417, 65805-65850)

Called before player-vs-player attacks/spells/robs ("unbalanced" messages §3):

1. `DAT_00482d8c == -1` (option 0x39) → PvP off entirely;
2. either side level (`+0x94`) < 4 → refuse;
3. live pair node (`already_evil`) → **allow** (retaliation bypasses balance);
4. defender is legal level 5 (**FIEND**) → allow regardless of level gap (65826-65828);
5. else require |level difference| ≤ `DAT_00482d8c` (65830-65840).

Related: `SYSOP LIGHTNING <user>` refuses non-FIEND targets unless option 0x40 is on
(60173-60180, "You may not lightning that user.").

### 6.6 Monster behavior (details in `monsters.md`)

* mode-6 monsters **spare** fame ≥ 0x28 in the aggro scan (`fast_update_character`
  20387-20390) and, in the roam-class-5 variant, invert to hunting fame < 0x28
  (20421-20435); flee free-attacks from mode 6 skip fame ≥ 0x50
  (`give_monsters_a_free_attack` 23882) — note the 0x28 vs 0x50 threshold
  inconsistency between the two paths, present in the original.
* roam-class-5 hunters only engage fame ≥ 0x28 (20435-20437) and will only finish a
  downed (HP < 1) player if fame ≥ 0x50 (`attack_monster_user` 26750-26757).
* area-spell target exclusion for innocent casters: §2.5 last row.

### 6.7 Displays

* **WHO / scan** (`display_scan_of_users` 34909+): per-tier color + name (or `Lawful`)
  as in §1 (34985-35090); the sysop variant prints raw points via `"%d"` (0x482f88,
  35090) plus per-pair timers (35097). Sysop limited-item audit prints fame as
  `"EP %d"` (`xref_users_items_polling_routine` 58476-58480).
* **Room description monster coloring** (`display_room_desc` 33413-33434): passive
  modes 0/3 print cyan (`0;36`); mode 4 prints plain white to viewers with fame < 0x28
  (else the hostile palette color); mode 6 the inverse (palette color to fame < 0x28,
  plain white to criminals) — i.e. the color quietly tells you whether it will attack
  *you*.
* **Public userinfo** export: §1.
* `FUN_0042a12e` (24385-24400) collapses the level to a side code (0 = good 6-7,
  1 = neutral 0-1, 2 = evil 2-5) passed into `move_player_to_fighter` /
  `move_monster_to_fighter` during combat setup (26120-26124); consumer semantics
  undetermined (§10).

### 6.8 Character-creation gate

The class-selection path re-asks the good-path question only if fame < 1 (2954,
§2.6) — a rerolling criminal (restored banked evil, §8) is not offered the Lawful
start, and answering **N** clamps negative fame to 0 (3535-3537).

---

## 7. Quest-VM integration (see `quests.md`)

Inside `perform_matched_action` (0x70209, fn at 68548):

* `addevil <n>` — `fame += n` (68903-68904); signed, so quest scripts are the
  legitimate fame-*lowering* mechanism (the Saint quest referenced by the migration
  message at 10627).
* conditional verbs: branch-if `fame < n` (69205-69216) and branch-if `fame > n`
  (69229-69240) — the `goodaligned` / `evilaligned` gates of `quests.md` §gates.

---

## 8. Persistence & decay

1. **Primary storage** is the player record itself: `+0x542` is saved/loaded with the
   whole character blob (see `records.md`; the same offset holds in WCCUSERS.DB — cf.
   the character-record layout memory note).
2. **Account-level banking** — GENBB record keyed `name` + `"WCC MAJOR MUD EVIL"`
   (0x4883d8), value at record `+0x38`:
   * written by `save_evil_points` (0x4eaa3, 47828-47871) on **final death** (lives
     exhausted, `check_kill_user` 13220) and **suicide** (`commit_suicide` 67656);
     both stamp `+0x7d2 = today()`;
   * **decay**: with `DAT_00482d70` (option 0x32, 0-100) = retention percent — if the
     bank day equals `+0x7d2` the value is saved unchanged (47855-47856), otherwise it
     is multiplied **once** by `pct/100` (47859) regardless of how many days elapsed;
     `pct == 0` banks 0, i.e. no carryover (47849-47851);
   * read back by `create_player` (10993-10994): a re-rolled character starts with the
     banked evil — crime follows the account;
   * deleted with the account (`delete_offline_mmud_user` 4917) and wiped en masse by
     the sysop console "C" maintenance command (`ljngame_sttrou` 2462 →
     `begin_clearing_evil_points` 48041-48060 → `clear_evil_points_routine`
     47953-48035, which walks GENBB deleting every `WCC MAJOR MUD EVIL` record).
3. **No in-play decay**: fame never changes on any tick (§2.6); only the pair timers
   age. The only downward paths are forgiveness, quest `addevil` with negative n,
   sysop edits, and the once-per-bank decay above.

---

## 9. Config knobs (set in `reload__wccmmud`, lines 109-133)

| global | source | meaning |
|--------|--------|---------|
| `DAT_00482dcc` | `ynopt(0x47)` | enable the committed-**Lawful** system (creation question, `Lawful` display, ×3 victim multiplier, rob prohibition). Forced 0 in the fallback path (114). |
| `DAT_00482d8c` | `numopt(0x39, -1, 100)` | PvP level-range limit; −1 disables PvP (§6.5). Fallback 100 (115). |
| `DAT_00482d70` | `numopt(0x32, 0, 100)` | evil retention percent on banking (§8). |
| `DAT_00482d99` | `ynopt(0x40)` | allow `SYSOP LIGHTNING` on non-FIENDs (§6.5). |
| `DAT_00482cfc` / `DAT_00482d00` | hardcoded 0x88d / 0x8e (558-559) | lawful / criminal respawn maps (§6.4). |

---

## 10. UNDETERMINED

* **`evil_for_robbing` (0x44e89f) has no callers in this DLL** — its purpose here is
  unresolved (export for MajorMUD Plus / the 16-bit build?). Confirmed: it is a
  read-only query of the pair list, not a fame writer.
* `player+0x700 & 0x40` ("has committed evil") is set on every gain (47484, 47571)
  but never read in this binary — external consumer unknown.
* Ability 0x71 `NotNeutral` is never enforced (§6.1) — either a doc-only ability or
  enforced elsewhere (editor?).
* The consumer of the `FUN_0042a12e` side code inside
  `move_player_to_fighter`/`move_monster_to_fighter` (death-message selection?) has
  not been traced.
* The creation-time Lawful question's exact prompt text lives in message block
  `DAT_0047910c` msgs 1/2 (2956-2959) — not extracted (MCV data, not in the DLL).
* Option-file text for options 0x32/0x39/0x40/0x47 (names as they appear in the MCV)
  not extracted; `wccmmud.ini` in `re/mudbins/` only shows `SYS_EDITEVIL_KEY=MMUDGOD`.
* The 0x28-vs-0x50 mode-6 threshold split (§6.6) is faithfully reproduced but the
  design intent is unknown.
* `load_player` migration messages reference a "new punishment associated with being
  evil" (0x47db50/0x47db93) — the 1.11 changelog context for the bucket clamp values
  (0x1d/0x27/0x4f/0x77/0xd1/299) is inferred from the code only.
