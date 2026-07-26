# MajorMUD (WG3-NT) — QUEST system spec

Behavioral spec derived from the 32-bit WG3-NT decompile
(`wg_nt_ghidra/exports/WCCMMUD_decompiled.c`, addresses are the file's
`/* 0x… */` RVA comments). Cross-references `abilities.md` (quest ability ids
50, 125-134, 156), `leveling.md` (`add_quest_exp`, exp storage), and
`vir_schemas.md` (ability tables on items/monsters/rooms).

**One-sentence model:** quests are **data-driven**, not hardcoded. They are
authored entirely in *text blocks* (WCCMSG/text-block DB) written in a small
imperative scripting language; the interpreter (`perform_matched_action`) reads
and writes a 30-slot **ability table on the player record** whose slots hold the
named quest-flag abilities (125-134) as integer progress counters. Rewards
(exp, items, class skills) and gates (level/class/alignment/ability checks) are
just verbs in the same language.

---

## 1. Quest representation — data-driven text-block scripts

There is **no per-quest struct.** A quest is an emergent behaviour of three data
mechanisms working over the shared ability system:

### 1.1 The player quest-flag table (`+0x73a` / `+0x776`)
`get_user_ability_value` (0x3d038) shows the player carries an innate ability
table of **30 slots**:

| offset | contents |
|--------|----------|
| `player+0x73a + i*2` | ability **id** (short), `i` in `0..29` |
| `player+0x776 + i*2` | ability **value** (short) |

`get_user_ability_value(id,…)` sums (or maxes) the value of every slot whose id
matches, and also folds in spell-granted abilities (spellbook at `+0x40`,
levels at `+0x54`), honouring the `NegateAbility` (0x7c) override. `user_has_ability`
(0x3d570) is the boolean form. This table is where the quest-flag abilities live:

| id | hex | ability | meaning of the stored value |
|----|-----|---------|-----------------------------|
| 50 | 0x32 | MageBaneQuest | MageBane quest progress |
| 125 | 0x7d | IceSorcQuest | Ice Sorcerer quest progress |
| 126 | 0x7e | GoodQuest | Good-path quest progress |
| 127 | 0x7f | NeutralQuest | Neutral-path quest progress |
| 128 | 0x80 | EvilQuest | Evil-path quest progress |
| 129 | 0x81 | DarkDruidQuest | Dark Druid quest progress |
| 130 | 0x82 | BloodChampQuest | Blood Champion quest progress |
| 131 | 0x83 | SheDragonQuest | She-Dragon quest progress |
| 132 | 0x84 | WereratQuest | Wererat quest progress |
| 133 | 0x85 | PhoenixQuest | Phoenix quest progress |
| 134 | 0x86 | DaoLordQuest | Dao Lord quest progress |

The value is a small integer "step counter" (0,1,2,3,…), not a bitmask — steps
advance it and completion is detected by comparing it to a threshold (§4).

### 1.2 Text blocks as quest scripts
A text block is a record with: id at `+0x8` (word 8), a default/long-text id at
`+0xa` (word 10), and a body string starting at `+0xc` (`get_text_block`,
0x3379c). The body is a newline-separated list of `head:tail` lines. Three
interpreters consume them:

- **`perform_text_block_as_special_command(user, blockId)`** (0x71d55) — runs a
  block *unconditionally*: every `:`-separated token on every line is passed to
  `perform_matched_action`. This is the "do these quest actions now" entry point.
- **`perform_special_command(user, blockId)`** (0x71a30) — the *input-matched*
  form: each line is `wildcard:action`; the head is `wildcard_match`ed against
  the player's raw typed input (`_input_exref`) and only matching lines' actions
  run. This is what the `TextBlock` ability (148) fires on items/rooms/monsters
  when a player types a command.
- **`ask_monster_a_question`** (0x20834) — the NPC-dialogue form (§3).

### 1.3 Where scripts are attached (the `abilities.md` connection)
Per `vir_schemas.md`, items/monsters/rooms carry `(ability,value)` tables.
Ability **148 `TextBlock`** on any of those objects names a block to execute;
ability **156 `QuestItem`** flags an item as a quest item; the quest-flag
abilities themselves can appear on objects. So a quest is assembled from: a
monster/room/item that triggers a text block, plus the block's scripted verbs
that read and mutate the player's quest-flag counters. Nothing about any
individual quest ("Dark Druid", "Phoenix", …) is compiled into the engine —
only the *vocabulary* (§2) and the *reward/enforcement thresholds* (§4) are.

---

## 2. The quest scripting language (`perform_matched_action`, 0x70209)

`perform_matched_action(user, line)` splits `line` on `:` and dispatches on the
first token (case-insensitive `sameto`). Verbs return a control code: **1** =
"line consumed, keep going", **2** = "condition failed — stop this block and
restore the un-parsed tail". Gate verbs that fail typically also print a
message and/or jump to a fail-block. The full verb set (resolved from the DLL
string table):

**State mutation (quest progress + rewards)**
| verb | effect | key code |
|------|--------|----------|
| `addability <id> <val>` | ensure player slot for `<id>` is **at least** `<val>` (raises value if lower, else creates a slot); id 0xa0 also adds a spell. *This is the primary "advance quest to step N" verb.* | writes `+0x73a`/`+0x776`, 0x71606-0x716xx |
| `giveability <id> <val>` | **adds** `<val>` to the slot for `<id>` (accumulate), or creates a slot; skips id 0xa0 | `FUN_0046c507` (0x46c507) |
| `removeability <id>` | zero every slot holding `<id>` (id 0xa0 purges its spell) | 0x715xx |
| `addexp <n>` | `add_quest_exp(user, n, 0)` — **uncapped** quest exp, silent | 0x70… → 0x6f291 |
| `giveitem <id>` | `add_item_to_inventory`; on failure drops to room | |
| `takeitem <id>` | `remove_item_from_inventory`; taken items are buffered and **rolled back** if a later verb in the block fails | |
| `givecoins <n>` | `player+0x620 += n` (a currency field) | |
| `addevil <n>` | `player+0x542 += n` (alignment / evil points) | |
| `learnspell <id>` | learn a spell | `FUN_0046fff6` |
| `cast <id>` | `cast_no_target` the spell as an effect | 0x71a… |
| `roomitem`/`hideitem`/`clearitem`/`roomtext`/`text`/`message` | spawn/hide/remove room items, print text/messages to user or room | |
| `summon`/`teleport`/`adddelay`/`random`/`remoteaction`/`price` | spawn monster, move player, timing, random branch, deduct currency | |

**Gates (require a condition, else fail-stop and optionally run a fail block)**
| verb | passes when |
|------|-------------|
| `checkability <id> <val>` / `testability <id> <val>` | player has ability `<id>` with value ≥ `<val>` |
| `failability <id> [val]` | player does **not** have `<id>` (or value below threshold) |
| `checkitem <id>` / `failitem <id>` | player has / lacks item |
| `checkitem`-style `roomitem`/`failroomitem` | item present / absent in room (scans room `+0x470` and `+0x4d8` slots) |
| `class <id>` | player class (`+0x92`) == `<id>` |
| `race <id>` | player race (`+0x90`) == `<id>` |
| `minlevel <n>` / `maxlevel <n>` | player level (`+0x94`) ≥ / ≤ `<n>` |
| `goodaligned <n>` / `evilaligned <n>` | alignment (`+0x542`) ≥ / ≤ `<n>` |
| `checkspell`/`checkskill`/`testskill` | player knows spell / skill or stat ≥ roll |
| `needmonster`/`nomonsters`/`monsters` | monster present / absent in room |
| `test_tournament` | tournament check |

So **quest progress is advanced by `addability`/`giveability`** (bumping the
125-134 counters) and **quest steps are gated by `checkability`/`failability`**
(and item/level/class/alignment checks) inside the same scripts. A typical quest
step is a text-block line like `KEYWORD:checkability 129 1:takeitem 400:addability 129 2:message 88`
— "if the player is at DarkDruid step 1 and hands over item 400, advance to
step 2 and print a message."

---

## 3. The ask-a-question mechanic (`ask_monster_a_question`, 0x20834)

Invoked when a player addresses a monster (ask/say). `param_1` = monster,
`param_2` = the player's words (upper-cased in place).

1. Resolve monster runtime data and its known/definition data
   (`get_known_monster_data`). The monster's **conversation block id** is at
   `knownmonster+0x11c`. If it is 0, print "*<mon> has nothing to tell you*".
2. Load that text block (`get_text_block`). Its body (`+0xc`) is a list of
   `KEYWORD:tail` lines; the block's default long-text is at word `+0xa`.
3. **No question given** (`param_2==NULL`, i.e. bare "ask <mon>"): display the
   default long-text (word +0xa), or "*doesn't understand you*" if none.
4. **Question given:** for each line, split on `:`, upper-case the keyword head,
   and `strstr` it against the player's input. On the **first keyword contained
   in the player's answer**:
   - `atol` the tail → a text-block number → `display_LONG_text` it (spoken by
     the monster), then **`perform_text_block_as_special_command(user, that#)`**
     — i.e. the matched answer runs an arbitrary quest script (§2).
   - If no keyword matches, "*<mon> has nothing to tell you*".

So the "quiz" is: builder authors a monster whose `+0x11c` block maps expected
answer-keywords to action blocks. Answering correctly runs a block that can
`checkability`/`takeitem`/`addability`/`addexp`/`giveitem` — validating the
answer and advancing/finishing the quest. Answer validation is plain
case-insensitive substring matching, keyword-by-keyword, first match wins.

---

## 4. Rewards and completion detection

### 4.1 Uncapped quest experience (`add_quest_exp`, 0x6f291)
`add_quest_exp(user, amount, tell)`: gated only on the "experience restructured"
flag (`player+0x7d5 & 0x20`); carries `amount` into the base-10⁹ exp counter
`+0x470`/`+0x474`. Unlike `add_experience`, it applies **no over-level cap and
no party split** — quest exp bypasses the "you have progressed too far" limit
(confirmed in `leveling.md` §2.3). Reached from the `addexp` verb with `tell=0`.

### 4.2 Quest items
`QuestItem` (156) merely tags an item; `giveitem`/`takeitem`/`roomitem` verbs
move them, and `checkitem`/`failitem` gate on possession. Quest logic (e.g.
"turn in the token") is scripted, not built in.

### 4.3 Stat-ability rewards and completion thresholds (`FUN_00414d23`, 0x414d23)
This function — reachable as **`load_player`** post-processing (0x15084 → call at
0x414d23) and via the god command **`verify <user> abilities`** (0x… → 0x414d23)
— is the completion detector. It scans the 30-slot table, reads each quest
counter, and applies **hardcoded thresholds** that grant permanent stat
abilities (`FUN_0046c507(player, abilityId, value)`) or penalise. The exact
`(abilityId, value)` push-args were recovered by disassembling 0x414d23
(`wg_nt_ghidra/exports/FUN_00414d23.asm`; call-site addresses cited per row):

| quest flag | value | grants `(id, value)` | asm call site |
|------------|-------|----------------------|---------------|
| IceSorc (0x7d) | == 2 | AC (0x02) +1 | 0x414e52 |
| Good (0x7e) > 7 **or** Neutral (0x7f) > 7 **or** Evil (0x80) > 3 | — | by class (`switch +0x92`), see below | 0x414e74+ |
| DarkDruid (0x81) | == 2 | S.C. (0x46) +1 | 0x414f66 |
| BloodChamp (0x82) | == 2 | Accuracy (0x16) +3 | 0x414f79 |
| SheDragon (0x83) | == 3 | Crits (0x3a) +1, S.C. (0x46) +2 | 0x414f8b, 0x414f98 |
| SheDragon (0x83) | == 2 | **penalty**: strip 35,000,000 exp (`0x2160ec0`), de-level (`FUN_00414c39`), clear the flag; "*You have been stripped of 35,000 [thousand] … exit and re-enter*" | 0x414fae |
| Wererat (0x84) | == 2 | Dodge (0x22) +1 | 0x415070 |

Alignment-path (Good/Neutral/Evil) per-class grants (jump table 0x414ea1):

| class (`+0x92`) | grants |
|-----------------|--------|
| 1, 2, 3, 0xf | MaxDamage (0x04) +1 |
| 4, 0xb | AC (0x02) +1, MaxMana (0x45) +6 |
| 5, 0xc, 0xd | S.C. (0x46) +1, MaxMana (0x45) +10 |
| 6, 9, 10 | BsMinDmg (0x75) +6, BsMaxDmg (0x76) +6, Stealth (0x1b) +1, MaxMana (0x45) +4 |
| 7, 8, 0xe | BsMinDmg (0x75) +10, BsMaxDmg (0x76) +10, Stealth (0x1b) +2 |
| 0 / others | nothing |

So **completion = the counter reaching a fixed value**, and the rewards are
**permanent stat boosts** (AC, MaxDamage, Accuracy, Dodge, Crits, S.C., MaxMana,
Stealth, backstab min/max damage — names per `abilities.md`), **not** the class
skills Smash/PerStealth/Meditate (that earlier inference was wrong; the class
skills are validated separately in §4.4). Idempotence comes from the scan loop
itself: before granting, any slot holding one of the reward ids
{0x02, 0x04, 0x16, 0x1b, 0x22, 0x3a, 0x45, 0x46, 0x75, 0x76} is zeroed
(`LAB_00414dd0`, asm 0x414dd0) — since `FUN_0046c507` *accumulates*, the pass
clears old grants and re-grants fresh on every run.

Note: MageBane (0x32), Phoenix (0x85) and DaoLord (0x86) counters are **not**
handled in this threshold switch — their rewards must be granted purely by the
data-side `giveability`/`addexp` scripts, or checked elsewhere (§6).

### 4.4 Login validation of quest-granted skills (`load_player`, 0x15084)
On every load, after the reward pass, `load_player` sweeps the ability table and
**removes** the granted class skills unless the player's current class+level
still qualifies:

- **Smash (0x20)** kept only for class∈{1,2,3,4,0xb,0xe} at level ≥ {0x16,0x14,0x19,0x1b,0x1b,0x16} respectively.
- **Perfect Stealth (0xba)** kept only for class∈{6,7,8,9,10,0xe,0xf} at their level gates.
- **Meditate (0xbb)** kept only for class∈{3,4,5,6,9,10,0xb,0xc,0xd,0xe} at their level gates.

This is the enforcement that quest-earned powers belong to specific advanced
classes at specific levels; change class or fall below level and the skill is
stripped on next login.

---

## 5. The class-quest connection

Despite the class-flavoured names, **completing a class-named quest does NOT
perform a class change.** Evidence:

- The only writer of the class field `player+0x92` from user action is
  **`one_time_class_change`** (0x6cee7), driven by the **offline character menu**
  (state `0x1c == 0x4a`, `WCCMMUD_decompiled.c:2928`: "*You have now completed
  your class change … there is a time [limit]*"). It sets `+0x92 = newclass`,
  marks the one-shot flag `+0x7d4 |= 0x80`, and resets CP (`+0x6e2=0xffff`) and
  HP-base (`+0x724=0`). This is a global, menu-driven, once-per-character reclass
  — **not** reachable from any quest text-block verb.
- The quest scripting language has **no set-class verb**. `class <id>` is a
  *gate* (require the player already be class X), never an assignment.

What the class-named quests actually do: their counters, on reaching threshold,
cause `FUN_00414d23` to **grant permanent stat abilities** (per the recovered
§4.3 table — e.g. DarkDruid → S.C. +1, BloodChamp → Accuracy +3, and the
alignment-path quests grant a class-flavoured stat package). In other words
these are **class-power / class-mastery quests** — the alignment-path package is
even re-derived from your *current* class on every pass — rather than
class-*change* quests. The actual advanced-class *transition* is the separate
one-time menu reclass. (The class skills Smash/PerStealth/Meditate are managed
by §4.4's login validation, not granted here.)

---

## 6. Undetermined / flagged

- ~~**Exact granted ability per quest.**~~ **CLOSED (2026-07-19).** The push
  sequences before each `call 0x46c507` in 0x414d23 were recovered by
  disassembly (`wg_nt_ghidra/exports/FUN_00414d23.asm`, generated with
  `DumpAsm.java`); the full `(abilityId, value)` table is now in §4.3. The
  earlier Smash/PerStealth/Meditate inference was wrong — the grants are
  permanent stat abilities (AC/MaxDamage/Accuracy/Dodge/Crits/S.C./MaxMana/
  Stealth/BsMinDmg/BsMaxDmg), and §4.4's class-skill validation is a separate
  mechanism.
- **MageBane (0x32), Phoenix (0x85), DaoLord (0x86)** counters exist and are set
  by scripts but are not consumed by the `FUN_00414d23` threshold switch. Their
  completion effect (if any beyond data-side `addexp`/`giveitem`/`giveability`)
  was not located; likely handled entirely by their own text-block scripts or by
  a gate elsewhere.
- **Precise control-code semantics** of the gate verbs (return 1 vs 2, and how a
  failed gate selects the fail-block vs. simply halting) are described
  behaviourally; the exact jump target of each `checkX`/`failX` on failure is a
  data-authoring detail of the specific block, not fixed in the engine.
- The peripheral verbs (`price`, `test_tournament`, `remoteaction`, `random`,
  `adddelay`, `checkskill`/`testskill`) are named and their handlers located but
  only summarised, as they are not quest-flag-specific.

---

## Function / offset index

| symbol | RVA | role |
|--------|-----|------|
| `get_user_ability_value` | 0x3d038 | read player ability table (`+0x73a`/`+0x776`) + spells |
| `user_has_ability` | 0x3d570 | boolean form |
| `ask_monster_a_question` | 0x20834 | NPC keyword→block dialogue quiz |
| `perform_matched_action` | 0x70209 | the quest scripting VM (all verbs) |
| `perform_text_block_as_special_command` | 0x71d55 | run a block unconditionally |
| `perform_special_command` | 0x71a30 | run a block matched vs player input |
| `get_text_block` | 0x3379c | load a text-block record |
| `add_quest_exp` | 0x6f291 | uncapped quest exp (`addexp` verb) |
| `FUN_0046c507` | 0x46c507 | grant/accumulate an ability into the table |
| `FUN_00414d23` | 0x414d23 | completion thresholds → class-skill rewards + SheDragon penalty (a.k.a. "verify abilities") |
| `FUN_00414c39` | 0x414c39 | de-level to match reduced exp (penalty helper) |
| `load_player` | 0x15084 | runs reward pass + strips class skills failing class/level gate |
| `one_time_class_change` | 0x6cee7 | menu-driven one-shot reclass (the real class change) |

### Player record offsets used by the quest system
`+0x40` spellbook ids · `+0x54` spell levels · `+0x73a[30]` quest/innate ability
ids · `+0x776[30]` their values · `+0x90` race · `+0x92` class · `+0x94` level ·
`+0x470`/`+0x474` experience (billions/remainder) · `+0x542` alignment/evil ·
`+0x620` currency (givecoins) · `+0x6e2` unspent CP · `+0x724` HP base ·
`+0x7d4`/`+0x7d5` flags (0x2000 restructured, 0x80 one-time-reclass used,
`+0x7d5 & 0x20` = restructured gate for `add_quest_exp`).
