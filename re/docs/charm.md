# MajorMUD — charm / pet system (monster-side), WG3-NT

Source: `re/wg_nt_ghidra/exports/WCCMMUD_decompiled.c` (line cites below are into that
file), function index `re/wg_nt_ghidra/exports/functions.txt`. Template field names are
disk-verified against the Nightmare `MonsterRecType` layout (`re/rectype.py`), which maps
the WG3-NT KNMSR record 1:1 (`vir_schemas.md`).

Player-target Enslave is a `silly_spell` placeholder in WG3-NT (`spellcasting.md` §4) —
everything below is the **monster-target** system.

## 0. The charm state triple

A pet is not one flag but a triple on the live monster record:

| field | meaning |
|-------|---------|
| `mon+0x1a` | **name link** (string) — the name of the user this monster is bound to. Dual-use: with `+0x116 == 0` it is a **grudge** target (monster attacks that user on sight and pursues); with `+0x116 != 0` it is the **owner/friend** |
| `mon+0x116` | **attack-suppression** byte — nonzero = will not swing at the named user (sysop debug view prints `Friend`, `display_monster_desc` 34268-34277) |
| `mon+0x128` bit 0 | **charmed** bit (int-idx `0x4a`) — full pet: assists the owner, skips the pursuit follow-roll, never wanders |

`mon+0x140` is the generic **dirty flag** (persistence), NOT an owner link — every state
transition below stamps `+0x140 = 1` alongside the real writes.

The charmed bit is set at exactly three sites in the whole DLL (grep `| 1` on int-idx
`0x4a`): Enslave instant apply (43806), Enslave duration apply (43820), and the
Summon-pet path in `cast_no_target` (40050). All other named/suppressed combinations
(grudge monsters, healed "friends", summoned hunters) are **not** charmed.

## 1. Acquisition — Enslave (ability 6) on a monster target

Handler: `cast_monster_target` (`0x47110`, 43112), ability-application switch `case 6:`
at **43796-43822**. The cast rides the standard machinery first (eligibility, success
roll, save — `spellcasting.md` §3); charm-specific deltas:

### 1.1 The save uses `charmres`, not MR

During the pre-application ability scan (43302-43316), a spell carrying ability 6
preloads the save stat from the **template**: `local_34 = knmsr+0x1a0` (short,
Nightmare column **`charmres`**; decompile `local_c[0x68]`, 43311). Non-Enslave spells
would instead get M.R.: ability 0x24 modifiers + `knmsr+0x70` floored at 1 (43387-43391).
The save itself is the standard one (43604-43617, and the same shape at
43596-43617 for the forced-cast branch): only when save class `spell+0xc6 == 2`, or
`== 1` with target AntiMagic (0x33); resisted when

```
genrdn(1,100) <= min(floor(charmres/2), 98)
```

Resist prints the "The %s resists your spell!" family (`s_..._00485e2e`) and applies
nothing. Note the shipped save-class distribution in `spellcasting.md` §3 — an Enslave
spell with `+0xc6 == 0` would be unsavable.

### 1.2 The application gate is `charmlvl` vs caster level

`case 6:` (43796) requires **both**:

* spell match type `spell+0xcc ∈ {4, 6, 8}` (the monster-targetable classes), and
* `knmsr+0x120` (short, Nightmare column **`charmlvl`**; decompile
  `(short)local_c[0x48]`) `<=` caster level `user+0x94` (43798-43800).

It is a plain level compare — no roll. **Failure is silent**: if the caster's level is
below `charmlvl`, the case body is skipped entirely (no message, no slot entry, mana
already paid).

### 1.3 State written on success

Both variants (43801-43807 instant, 43809-43821 duration) write the same triple:

```
mon+0x140 = 1                       ; dirty
strcpy(mon+0x1a, caster user+0x1e)  ; owner name link
mon+0x116 = 1                       ; suppressed toward owner
mon+0x128 |= 1                      ; charmed
```

There is **no rename** — the display name at `mon+0x8e` is untouched; only the internal
`+0x1a` link is written (cleared, not restored, on release — it held the grudge/owner
name, never the display name).

### 1.4 Duration slotting

* `spell+0xce == 0` (instant): charm state only — **no slot, no timer**. Permanent
  until a §4 release path fires.
* `spell+0xce > 0`: `add_cast_spell_to_monster` (`0x3ed64`, 38223) first, then the
  charm state. Slot array (5 slots): id `+0x14a+i*2`, **value** `+0x154+i*2`,
  **remaining ticks** `+0x15e+i*2` (38258-38279). Duration computed in
  `add_cast_spell_to_monster` from the base `spell+0xce` with the standard level
  scaling: `lvl = min(caster level, spell+0xa2 cap)`; `+ (lvl / spell+0xf9) *
  spell+0xf8` when the divisor is nonzero; if `base < spell+0xca * lvl` then
  `genrdn(base, spell+0xca*lvl + 1)` (**RNG draw**); finally `* (100 +
  SpellDuration ability 0xa6) / 100` (38236-38255). Same-id recast refreshes the
  existing slot in place (38259-38266).
* Slot-full edge: with all 5 slots occupied by other spells the function returns -1
  after printing the plain-failure pair ("You attempt to cast %s, but fail!",
  `0x4859f6`, 38283-38287) — but the caller does **not** check the return and still
  writes the charm triple (43812-43821): a fully-slotted monster gets a permanent,
  timerless charm.

The success message is the generic `display_spell_success` (caster/room strings from
the spell record, `spellcasting.md` §8.6), invoked from inside
`add_cast_spell_to_monster` (38263/38276) — i.e. only on the duration path. The
instant path prints nothing in the case body.

### 1.5 RNG draw order (Enslave cast, one attempt)

1. `genrdn(0,100)` — cast success roll (43414, per attempt-loop pass).
2. `genrdn(1,100)` — charm save, only when the save gate of §1.1 applies (43604).
3. `genrdn(0, max-min+1)` — spell value roll (43693-43695; value stored in the slot,
   otherwise unused — charm is binary).
4. `genrdn(base, perlvl*lvl+1)` — duration roll inside `add_cast_spell_to_monster`,
   only when `base < perlvl*lvl` (38249-38251).

(Forced/auto casts, `param_4 != 0`, may additionally draw the retaliation-lock
`genrdn(1,100)` vs `mon+0x108` before the loop — 43261 — which can set a grudge
`+0x1a`/`+0x116=0` on a passive target before the charm lands and overwrite-order makes
the later charm write win.)

## 2. Pet behaviour while charmed

### 2.1 Movement — never wanders, always pursues

* Random wander (`medium_update_monster` 19340-19378) requires `+0x1a` empty AND
  `(mon+0x128 & 1) == 0` — a pet does neither.
* Following is the standard name-pursuit tier (`fast_update_monster` 19411-19445,
  `monsters.md` §4 "Pursuit"): breadcrumb-walk toward the user named at `+0x1a`.
  The charmed bit **skips the follow roll** — the `genrdn(0,100) < mon+0x108` gate is
  only evaluated when `(mon+0x128 & 1) == 0` (19422-19423) — so a pet always follows,
  regardless of the template's aggression. All other refusal sources still bump the
  give-up counter `+0x124` (different map, target hidden w/o SeeHidden 0x39,
  moved-flag, no trail, `move_monster` refusal — and owner not online at all,
  19412-19415).

### 2.2 Assist — the pet attacks the owner's autocombat target

Combat driver `FUN_00423863` (`0x23863`, 20335 — the 5 s acquisition pass), named-link
branch (`+0x1a` nonempty, 20464+): resolve `get_user_number(mon+0x1a)`; if the user is
online and `+0x116 != 0` and the charmed bit is set, then when the owner
`is_inside_autocombat` → `FUN_0044cc65` (20512-20517). No RNG — deterministic each pass.

`FUN_0044cc65` (46917-46965) reads the owner's autocombat entry
(`DAT_004877e8 + usernum*0x14`: `[+0]` user target or -1, `[+4]` monster target or
0xffff):

* owner's target is a **user** → `attack_monster_user(pet, thatUser)` (46963) — the
  pet joins PvP.
* owner's target is a **monster** → `attack_monster_monster(pet, thatMonster)`
  (46958-46960) — see §3.
* owner's target is **the pet itself** → instant release: clear charmed bit, walk the
  5 slots and `perform_spell_termination_monster_upkeep` every spell carrying
  ability 6, zero those slots (46929-46953). See §4.3.

The `+0x12c == 5` special branch (20477-20493) is for **non-charmed** suppressed
followers of roam class 5: they counter-attack players who are in autocombat against
the named user. Charmed pets take the `FUN_0044cc65` branch instead; suppressed,
named, non-charmed monsters ("friends", §2.4) attack players *other than* the named
user, behaviour-mode-gated (20494-20511).

### 2.3 Target exemption — nobody targets a pet by accident

* **Monsters never acquire pets.** Monster target acquisition only ever produces
  users (`attack_monster_user`) or the explicit hunt link `mon+0x88`
  (`FUN_00423863` 20452-20460); there is no room-scan for monster victims, so a pet
  is only ever swung at by a §6 hunter that carries its id.
* **Hostile spells can't target your own pet.** `is_valid_monster_target`
  (`0x3f1e4`, 38430), match types 9/0xc: if the monster is charmed-or-suppressed and
  `mon+0x1a` equals *your* name → invalid (return 0, 38477-38488); the same compare
  on an unsuppressed monster makes your grudge-holder always-valid.
* **Threat scans ignore your pet.** `monster_could_attack` (`0x...`, 18209) counts a
  monster as a threat only if NOT (charmed AND named == you) AND `+0x116 == 0`
  (18238-18241) — your pet (and any "friend") never blocks rest-type actions.
* **Physical attacks are allowed** — and are a release path (§4.3): the engine does
  not block `attack_user_monster` against your own pet.

### 2.4 `+0x116` semantics (suppression) — set/clear inventory

Set to 1: Enslave apply (43805/43819); Summon-pet (40049); **healing a monster**
("befriending"): after a heal-type ability lands on a monster, `genrdn(1,100) >
knmsr+0x6e` (aggression) → `+0x1a = caster name`, `+0x116 = 1`, charmed bit NOT set
(44047-44052, 44147-44155) — a "friend", the state `display_monster_desc` labels
`Friend` (34268-34277).

Cleared to 0: grudge acquisition when a monster is damaged and survives —
`genrdn(1,100) < aggression` locks `+0x1a = attacker`, `+0x116 = 0`
(`attack_user_monster` 26515-26525; spell-damage twin 43758-43766; area copies
40377-40384 and 40607-40614); every §4 release; summoned hunters at birth (§6).

The spell-damage twin's roam-class arm is the OTHER half of that `if`, not a peer
outcome of the grudge. `check_kill_monster` returns 0 (43750) and then 43751 asks
`known_monster_data == NULL || knmsr+0x54 == 0x25`; **only on that branch** does
`sameas(mon+0x1a, attacker)` run, and its whole effect is `+0x116 = 0`
(43753-43756) — a re-hit by the monster's *current* name-holder unsuppresses it,
with no roll and no name write. A monster with an ordinary non-`0x25` template
takes the `else` at 43758 and never reaches the `sameas` at all. The two AREA
copies are the same shape with one clause missing: they test the INSTANCE's roam
class (`mon+0x12c == 0x25`, 40371 and 40601) and have no null-record branch.

Aggression source differs by twin, incidentally: 43760 reads the TEMPLATE's
`knmsr+0x6e`, while the melee (26516) and area (40379/40609) twins read the
instance's `mon+0x108`.

What it suppresses: the named-branch swing in `FUN_00423863` (20466-20476 fires only
when `+0x116 == 0`), the monster-vs-monster swing gate (20453), and the free flee
attack against the locked target (`give_monsters_a_free_attack` 23886-23895,
`monsters.md` §4).

## 3. Monster-vs-monster combat — `attack_monster_monster` (`0x2f6ae`, 27213)

The one m-v-m swing function; used by pets (§2.2) and hunters (§6). Preconditions
(27231-27234): attacker energy `mon+0x16 >= mon+0x114` (full-energy gate) and attacker
does NOT have ability 0x3c (Fear) — `monster_has_ability`, so an active spell slot or
a carried item counts, not just the template rows. There is NO room compare and no
safe-room check anywhere in the function: a swing can cross a room boundary, which is
what the §6 hunt arm relies on.

Formula path — **it is the ordinary combat pipeline**, both sides loaded as fighters:

1. `DAT_004877e4 = 5; DAT_004877e0 = 5` — attack mode 5 for both globals (27235-27236;
   the mode `combat.md` uses to reshape damage).
2. `FUN_0042a15c(template)` → attack-alignment code from `knmsr+0xae` (behaviour
   mode): modes 0/4 → 0, 1/2/6 → 2, else 1 (24410-24427); passed as the 4th arg of
   the attacker's `move_monster_to_fighter(&DAT_00496010, ...)` (27238) — which
   compares it against its OWN call of `FUN_0042a15c`, so the equal case skips the
   0x18/0x19 accuracy block and word `[2]` stays 0 (25109-25118); the defender is
   loaded with -1 (27240) and takes the same skip.
3. `piVar6 = calculate_attack(&DAT_00496010, 0x49625c)` — the full accuracy /
   dodge / damage engine of `combat.md` (all its RNG draws happen here, in its
   documented order). Result block: `[0]` result code (1 glance / 3 dodge /
   else-miss when damage < 1; hit otherwise), `[1]` damage, `[3]` floor for the
   defender's `+0x14` counter, `[4]` kill exp (worth × multiplier, `combat.md`
   §monster-fighter), `[5]` attacker energy cost. **`[3]` is dead**: `DAT_00495fdc`
   is zeroed on entry to `calculate_attack` (25246) and no path writes it, so the
   `+0x14` raise below can never fire (M7 slice 5 pass; not ported).
4. Attacker pays `result[5]` energy — the gate is checked AFTER the draws, so a form
   costing more than the pool burns rolls and lands nothing (27242-27243); defender HP
   `-= result[1]` (clamped to remaining HP, 27244-27247); defender `+0x14` raised to
   `result[3]`; both records dirtied (27248-27252).
5. `check_kill_monster(defender, -1)` (27254), the display name `strcpy`'d off
   `mon+0x8e` first (27253).
6. Post-damage **DamageShield** (defender ability 0x48): if present,
   `genrdn(1, max(val+1,1))` (**RNG**) is subtracted from the attacker's HP, then
   **capped at** `mon+0x104` (27293-27295 assigns the maximum down onto anything
   above it; the bite itself only ever subtracts). The attacker is never checked for
   death there. The two arms differ: the survivor block (27283-27296) sits inside the
   `damage >= 1` else-arm, but the kill block (27307-27320) has **no damage guard at
   all** — a kill that landed zero damage still draws.

The attacker fighter is built from attack-form slot 0 **whatever its kind byte** —
`move_monster_to_fighter` returns 0 only for a missing record or template
(25091-25099), so the `!= '\0'` guards at 27239-27240 are validity checks, not an
"is slot 0 a melee form" test. Its parry word `[8]` is Dodge(0x22) (25185-25186) on
BOTH sides; accuracy folds 0x16/0x69/0x6a (25188-25193), MaxDamage(4) raises both
damage bounds (25196-25198), and Speed(0x57) scales the energy cost `EU*val/100`
capped at `knmsr+0x7a` (25203-25211; the DLL does that multiply in `longlong`, and
`sphere of isolation` carries Speed 5000).

**Kind-0 slot 0 is NOT the same as a zeroed slot** (checked against `mmud_wgnt.sqlite`,
M7 slice 5 review): 125 of the 1101 templates have `attacktype_1 = 0`, and **28 of
those carry nonzero accuracy/min/max in that slot**. They swing with those words.
Notable ones:

| template | `charmlvl` | acc | min-max | note |
|---|---|---|---|---|
| `dark warlock` | 22 | 49 | 100-15 | min > max, so `calculate_attack` raises max to min → flat **100** |
| `dying master assassin` | 999 | 120 | 7-20 | |
| `Horner the Hide` | 999 | 160 | 50-100 | |
| `amazon battle master` | 9999 | 200 | 5-80 | |
| `Sharh'Kur` | 999 | 757 | 100-15 | flat 100, as above |

**73** of the 125 kind-0 templates sit under the 9999 charm floor, so this is live for
pets, not a curiosity.

**The EU trap** (matters for the §2.2 pet-assist driver): **21** templates carry
`attackenergy_1 > energy`, so the step-4 pay gate can never open — every driver pass
burns `calculate_attack`'s draws and lands nothing, forever. `bishop`, `priest` and
`boatman` are the sharp edge: pool 0, form cost 5, and **`charmlvl 0`, i.e. charmable
by anyone**. Any driver that calls `attack_monster_monster` on a schedule must expect
these to be permanent RNG sinks; the DLL does not special-case them.

Draw order per swing: `calculate_attack` internals first, then at most one
DamageShield `genrdn`.

Room strings (each `spr`'d into `DAT_004964a9`, first letter upcased, prefixed with
the combat color codes, `tell_room` to the defender's room — the kill line to the
ATTACKER's room, 27326). VERIFIED byte-for-byte against the shipped `.rdata`
(M7 slice 5): the earlier transcription of the glance line came from Ghidra's
mangled symbol name and was wrong in both slot count and spelling.

* hit: `%s just attacked %s!` (`0x481f87`, 27298-27304)
* glance (`result 1`): `%s's just glanced off of %s's armour.` (`0x481f9d`, 27259) —
  TWO slots, attacker then defender; the weapon is never named
* dodge (`result 3`): `%s just dodged an attack from %s.` (`0x481fc4`, 27267) —
  defender first
* miss: `%s just missed an attack against %s.` (`0x481fe7`, 27275)
* kill: `%s just killed %s.` (`0x481f73`, 27322-27326; name captured before the kill)

**Experience on a pet kill**: `distribute_experience(-1, result[4], -1, victimId,
map, room)` (27330) — no killer credit. `distribute_experience` (`0x4c990`, 46819)
with `param_1 == -1` splits `result[4]` evenly among users **in autocombat against the
victim** (plus idle-autocombat users in the room); shares round down (min 1 each);
users engaged on the victim get the share + autocombat break message. The owner gets a
share only if they were themselves fighting that monster; a pure pet solo-kill awards
**nobody** anything. `kill_autocombat_against_monster(victim)` cleans up engaged
players (27331).

No draw-order surprises beyond the above; the defender never counter-swings inside the
call (retaliation happens only via its own `+0x88` link, if any).

## 4. Release

### 4.1 Timer expiry — `medium_update_monster` (19308-19326)

Every medium tick (3 s), each nonempty slot: `+0x15e -= 1`,
`perform_routine_spell_monster_upkeep` (which has **no** case for ability 6 — charm
does nothing per-tick), and at 0 the slot id is cleared and
`perform_spell_termination_monster_upkeep(mon, spell, slotValue)` runs.

`perform_spell_termination_monster_upkeep` (`0x4a45d`, 44972), `case 6:` (44988-44995):

```
mon+0x140 = 1                       ; dirty (SET, not cleared)
*(byte *)(mon+0x1a) = 0             ; owner name emptied
mon+0x116 = 0                       ; suppression off
mon+0x128 &= ~1                     ; charmed bit off
```

No message to anyone. The released monster is neutral (`+0x1a` empty) and rejoins
normal wander/acquisition — it may immediately re-acquire the former owner via the
ordinary aggression rolls, but holds no grudge.

### 4.2 Leash give-up / owner logout — `fast_update_monster` (19446-19489)

When the pursuit give-up counter `+0x124` exceeds 15 (bumped once per fast tick by any
refusal, **including the owner not being online** — `get_user_number == -1`,
19412-19415 — so logout releases in ~16 s): non-roam-`0x25` monsters clear `+0x1a` and
the counter, and if charmed additionally clear the bit and terminate every slot whose
spell carries ability 6 (19455-19487, same slot sweep as §2.2). A charmed roam-`0x25`
monster instead despawns silently (`FUN_004298ec`, 19448-19450).

**The branch never writes `+0x116`.** 19451-19453 is the whole of its own state work
— `+0x140 = 1`, `+0x124 = 0`, `+0x1a = 0` — and the charmed arm at 19454-19455 adds
only `+0x128 &= ~1`. Suppression comes off through the slot TERMINATION and nowhere
else, so a **slotless** pet that ages out ends up nameless, uncharmed, and still
SUPPRESSED: `+0x116` stays 1 with nothing left in `+0x1a` for it to refer to. Since
the suppression consumers (§2.4) all pair `+0x116 == 0` with a name compare, the
monster is left permanently unable to swing on the named-branch path — a stuck,
harmless loiterer. This is reachable on shipped data by two routes: §1.4's
slot-full permanent charm, and Summon-born pets (§6), both of which carry the
charmed bit with no ability-6 slot behind it.

### 4.3 Owner attacks own pet

* **Melee/ranged** (`attack_user_monster` 26052, post-damage survivor branch
  26513-26564; the charmed `else` is 26527-26563): if the surviving target is charmed
  and `mon+0x1a` == attacker name: `+0x116 = 0`, charmed bit cleared, ability-6 slot
  sweep terminated (which also empties `+0x1a`). A *different* player attacking
  someone's pet triggers **nothing** — the charmed branch has no retaliation lock,
  no name overwrite.
  Note where this branch lives: it is inside the autocombat ROUND arm, the `else`
  (26241) of `if (DAT_004877f4 == '\0')` (26112). The engage-time lock at 26230 is in
  the other arm and cannot run in the same call, so a pet released here is left with
  an empty `+0x1a` and no fresh grudge — §4.1's neutral end state, on the melee path.
* **Autocombat** targeting the pet: released by the pet's own driver
  (`FUN_0044cc65` 46929-46953, §2.2) on the next combat pass — same sweep.
* Asymmetry for **slotless** pets (instant Enslave or Summon, §6): the sweep finds no
  ability-6 slot, so `+0x1a` keeps the owner's name. On the melee path `+0x116` was
  forced 0 first → the ex-pet is a full **grudge monster hostile to its former
  owner** (attacks and pursues). On the autocombat path `+0x116` stays 1 → it degrades
  to a "friend" that attacks *other* players (§2.4/§2.2).

### 4.4 Paths that do NOT release

* **Owner death**: `check_kill_user` (12992) contains no monster sweep — the pet
  persists (the respawned owner is still online under the same name, so pursuit
  resumes; a link-dead owner falls under §4.2).
* **Dispel**: no player-usable effect terminates monster slots; the only
  `perform_spell_termination_monster_upkeep` callers are 19321, 19470, 26548, 46944
  (the four paths above).
* `dismiss_users_angels` (`0x203db`, 17855) sweeps only `+0x116 == 0`, roam-`0x25`
  monsters named to the user (silently despawned when the user's legal level hits
  4/5) — hunters, not pets.

## 5. `cmd_tame` / `cmd_mesmerize` — dead verbs that turn into speech

`cmd_tame` (`0x528e3`, 50470-50477) and `cmd_mesmerize` (`0x528ea`, 50482-50493) are
`return 0` stubs, dispatched from `handle_commands` (`0x13655`) cases 0x1d/0x1e
(9243-9248) — so the verbs ARE in the parse table. `handle_commands` returns the
handler's 0 (9844-9846).

The caller `execute_input` (`0x...`, 48938) treats `handle_commands() == 0` as
"input not consumed" (49027): it `rstrin()`s the original input line and falls through
the full unknown-input chain (49028-49260):

1. named text exits of the room (type-10/type-0xc actions) — `move_user` if matched;
2. the room's special command (`perform_special_command`);
3. bareword spellbook match → `cmd_cast` (typing a spell name casts it);
4. profanity gate, `handle_ljn_actions` (emotes);
5. finally **speech**: the whole line is said aloud — `%s says "%s"` (`0x488792`) to
   the room (yell prefix handled at 49193-49222), or `Your command had no effect.`
   (`0x48882c`) when speech is impossible and nobody heard.

Net observable behaviour: `tame bear` makes the character *say* "tame bear" (unless a
room exit, spell, or emote happens to match the text). Not silent, no error string.

## 6. Summon ownership and the hunt variants (ability 12/0xc)

Four handlers, one `generate_monster(map, room, -1, templateId=value, 0, 65000, -1,
0, 1)` each; ownership is entirely in the §0 triple written right after:

| path | site | name link `+0x1a` | `+0x116` | charmed | net behaviour |
|------|------|-------------------|----------|---------|---------------|
| `cast_no_target` case 0xc (match 1/2/6) | 40035-40056 | **caster** | 1 | **yes** (40050) | full pet, timerless (release via §4.2/§4.3 only); success shown as `display_spell_success(..., "everyone", ...)` (40042) |
| `cast_user_target` case 0xc (match 0/2/6/8) | 42059-42086 | **target user** | 0 | no | hunter vs a player: spawns in the **caster's** room, pursues/attacks the named victim via the grudge machinery |
| `cast_monster_target` case 0xc (match 4/6/8) | 43902-43931 | (empty) | 0 | no | hunter vs a monster: victim link `summoned+0x88 = victim id` (43921), and the summon's id is pushed into the victim's 10-deep back-link array `victim+0x60+i*4` (43922-43928). `FUN_00423863` then walks the summon toward `+0x88` (`dir_monster_travelling_coord`) and `attack_monster_monster`s when co-located and unsuppressed (20452-20460) |
| `monster_cast` ability 0xc | 23251-23268 | **target user** | 0 | no | monsters summoning hunters against players; population caps from `knmsr+0x5c` passed instead of the 0/65000 pair |

Duration != 0 on any of these routes to `silly_spell` (summons are instant-only).
For our `game.rs` `summon_spawn(..., None)` placeholders: the owner argument should be
the §0 triple — `(owner_name, suppressed, charmed)` = (caster, true, true) for the
no-target pet form, (victim, false, false) for the player-hunt form, and the
monster-hunt form carries no name at all, only the `+0x88` victim id + back-link.

## 7. UNDETERMINED / flagged

* **Stale hunt links**: nothing clears `summoned+0x88` or the victim's `+0x60`
  back-links on either party's death (`check_kill_monster` 21245+ touches neither);
  monster ids are reused, so a long-lived hunter could redirect onto a recycled id.
  Not chased.
* **`+0x60` back-link consumers**: the victim-side array is written (43922-43928) but
  no reader was located in this pass; suspected despawn/cleanup bookkeeping. Open.
* **Instant-Enslave messaging**: the `spell+0xce == 0` apply path (43801-43807) prints
  nothing in the case body; whether shipped instant-charm spells surface any text
  beyond the generic cast lines needs an oracle run (no shipped-data survey done).
* **`charmlvl` semantics vs shipped data**: the gate is `charmlvl <= caster level`;
  templates with `charmlvl 0` are charmable by anyone, and no "uncharmable" sentinel
  (e.g. 0xffff read as -1 ⇒ always charmable, since the compare is signed on the
  template side: `(int)(short)`) was checked against the raws. A negative `charmlvl`
  always passes; whether editors used large positives as "never" is data, not code.
* **`FUN_0042a15c` return** (0/1/2 by behaviour mode) feeds
  `move_monster_to_fighter`'s 4th parameter; its effect inside the fighter build is
  documented as the alignment/verb selector in `combat.md` — not re-derived here.
* **Ability 0x3c** on the attacker template blocks `attack_monster_monster` entirely
  (27234); id 0x3c is the Fear/random-move ability in the upkeep table — the reuse
  here ("pacifist"?) is unexplained.
