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

Two adjacent fields belong to the SUMMON side rather than the charm triple, and are
documented in §6: the hunt link `mon+0x88` (a monster id, not a name), and the travel
trail `mon+0x38 .. mon+0x5c` — 10 room-id entries, ending exactly where the `+0x60`
back-link array begins.

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

**It is not a clean swap, and `charmres == 0` is NOT a free charm.** The preload writes
into the same `local_34` the M.R. default keys on. `local_34` is initialised to **0** at
**43170**, and the default arm at **43387** is guarded by `if (local_34 == 0)` — so a
template whose `charmres` is 0 preloads a 0, fails to suppress the default, and saves
with the ordinary M.R. stat (floored at 1) exactly like a non-Enslave spell. The swap
only takes effect for `charmres != 0`.

This is live on the shipped data, not a corner: **48** of the 1101 templates carry
`charmres = 0` (`SELECT COUNT(*) FROM monster WHERE charmres=0`), and **38** of those
have `charmlvl <= 20` — squarely inside the low-level charm band, where they are the
templates a player will actually try to enslave. Ported at
`Core::monster_cast_save_stat` and documented on `content::Monster::charm_resist`.

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

### 2.3 Target exemption — pets are DEPRIORITISED, not hidden

*(CORRECTED 2026-07-25, M7 slice 5 Task 6. The previous text collapsed two
mechanisms on two disjoint call paths into one "hostile spells can't target your
own pet", which is false of every single-target spell in the game.)*

There are **two** gates, and they never both run on the same cast.

* **The find ORDERING — `find_action_target` mask bit `0x800`** (`0x699fc`,
  63726). When the bit is set the room's monster block runs **twice**: pass 1
  skips `mon+0x128 & 1` (63776), pass 2 scans ONLY charmed monsters (63820).
  Charmed bodies are therefore searched **last, never excluded** — with no wild
  match in the room, pass 2 hands back the pet and the action lands on it. Both
  passes finish before the `0x2` user scan, so the bit orders monsters against
  monsters and never against a player. Carriers: `cmd_any_attack` `0x883`
  (49590) and `cmd_cast`'s preferred masks `0x801` (match 4), `0x803` (match 8)
  and `0xf837` (match 6). The dispatcher's universal retry `0xf037`
  (59265-59271) does NOT carry it — no observable consequence, since every match
  type `cast_monster_target` accepts already searches monsters in its preferred
  mask, so the retry only reaches a monster for match types that refuse it by
  kind anyway. Every other caller (`cmd_rob`, `cmd_track`, `cmd_follow`,
  `cmd_give` — all `0x83`; `cmd_use` `0xf037`) is charm-blind.
  Net effect: `cast mmis rat` with a pet rat and a wild rat present hits the
  **wild** one; with only the pet present it hits the **pet**.
  **Corollary (M7 slice 5 Task 7): pass 2 itself is unobservable on the CAST
  path.** Its only job is to hand back a lone pet, and on every match type
  `cast_monster_target` accepts the charm-blind `0xf037` retry would hand back
  the same body one step later — the two mechanisms are indistinguishable from
  outside. `cmd_any_attack` (`0x883`, 49590) has **no** retry, so melee ATTACK is
  the ONE place pass 2 is load-bearing — which is exactly what keeps the §4.3
  melee-release family reachable. A test that charms the only monster in the
  room and then casts at it pins the *outcome*, not the *pass*: deleting pass 2
  leaves it green.
* **The area VETO — `is_valid_monster_target`** (`0x3f1e4`, 38430). A real
  exclusion, but it lives **only on the AREA sweeps**: `count_valid_targets`
  (38610), `add_duration_spell_to_room` (38707), `add_evil_warnings_to_room`
  (38803) and the eight `cast_no_target` effect arms (39705-40724).
  `cast_monster_target` never calls it, so no single-target cast consults it.
  Its `switch` is on `spell+0xcc`, so the MATCH TYPE decides how much runs:
  - 0/1/2/7 → invalid; 3/5/0xb → valid outright (38455-38466), as does the
    `default` arm that would catch 4/6/8;
  - 10/0xd → valid ONLY for your own charmed pet (38501-38509) — the pet-command
    band; the `{3,5,9,0xb,0xc}` sweep guards never pass them, so it is dead;
  - **9 and 0xc only** → the charm arm (38477-38488): uncharged, `+0x116 == 0`
    and `mon+0x1a` == your name → valid at once (your grudge-holder is always
    fair game); charmed **or** suppressed and `mon+0x1a` == your name → INVALID.
    That covers "friends" (§2.4) on the same terms as pets. Falling through the
    name compare: instance roam class 5 or `0x25` with caster fame
    `player+0x542 < 0x28` → invalid, behaviour mode 4 → invalid, else valid
    (38489-38499).

  So only a match-9/12 AREA cast actually spares your own pet. Shipped and
  learnable: stinking cloud (131) is match 12.
* **Monsters never acquire pets.** Monster target acquisition only ever produces
  users (`attack_monster_user`) or the explicit hunt link `mon+0x88`
  (`FUN_00423863` 20452-20460); there is no room-scan for monster victims, so a pet
  is only ever swung at by a §6 hunter that carries its id.
* **Threat scans ignore your pet.** `monster_could_attack` (`0x20b51`, 18209)
  counts a monster as a threat only if NOT (charmed AND named == you) AND
  `+0x116 == 0` (18237-18240) — your pet (and any "friend") never blocks the
  actions gated on it. There are **four** callers in WG3-NT, not two
  (re-grepped M7 slice 5 Task 7): `can_sneak` 65462 and `cmd_hide` 62023
  (`theft.md` §11.1/§11.2), plus **`cmd_close` 52341** and **`cmd_lock` 53290**.
  `cmd_hide`, `cmd_close` and `cmd_lock` carry the identical four-term guard —
  `is_inside_autocombat() == 0 && is_being_attacked() == 0 && user+0x6f0 < 1 &&
  monster_could_attack(-1, user) == 0` — while `can_sneak` runs its own
  attacker-type/same-room pre-test first and then only `+0x6f0 < 1` before the
  call. No rest command is among them.
* **Physical attacks are allowed** — and are a release path (§4.3): the engine does
  not block `attack_user_monster` against your own pet. `cmd_any_attack` carries
  `0x800`, so a named swing prefers a wild body, but the second pass keeps the
  pet reachable and the whole §4.3 melee release family with it.

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
   defender's `+0x14` counter — the **periodic HP-drain (poison / bleed) counter**
   drained once per slow tick (`monsters.md` §2 field map, `slow_update_monster`
   19276-19279) — `[4]` kill exp (worth × multiplier, `combat.md`
   §monster-fighter), `[5]` attacker energy cost. **`[3]` is dead**: `DAT_00495fdc`
   is zeroed on entry to `calculate_attack` (25246) and no path writes it, so the
   raise-if-greater onto `+0x14` at 27248-27249 can never fire. **Not ported** (M7
   slice 5) — a monster-vs-monster swing in our port never poisons its defender,
   which is what the DLL does too, by accident rather than by design. Listed in §8.1.
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
| `cast_monster_target` case 0xc (match 4/6/8) | 43902-43931 | (empty) | 0 | no | hunter vs a monster: victim link `summoned+0x88 = victim id` (43921), and the summon's id is written into the victim's 10-deep back-link array `victim+0x60+i*4` (43922-43928, see §7 — it lands in *every* free slot). `FUN_00423863` then walks the summon toward `+0x88` (`dir_monster_travelling_coord`) and swings when the walk has **no step to offer** — see the trail note below |
| `monster_cast` ability 0xc | 23251-23268 | **target user** | 0 | no | monsters summoning hunters against players; population caps from `knmsr+0x5c` passed instead of the 0/65000 pair |

Duration != 0 on any of these routes to `silly_spell` (summons are instant-only).

### 6.1 The breadcrumb trail and the hunt arm (20448-20463)

The hunter walks its quarry's **travel trail**, a 10-entry ring of room ids at
`mon+0x38 .. mon+0x5c` inclusive — it ends exactly where the `+0x60` back-link array
begins. `move_monster` pushes it on every departure (21572-21574:
`memmove(mon+0x3c, mon+0x38, 0x24)` then `mon+0x38 = dest`), so **index 0 is the
current room** and index 1 the predecessor.

`dir_monster_travelling_coord` (15790-15816) reads it from index **1**: it scans the
quarry's trail for the hunter's own room at index `i` and returns the exit whose
destination is `trail[i-1]` — the room the quarry went to next. (The decompile spells
that second read as `mon+0x34 + i*4`, which is the same slot as `0x38 + (i-1)*4`; the
array base is 0x38, not 0x34.) It short-circuits to `-1` when the quarry is already in
the hunter's room (15797-15799).

The driver's arm is then two-way and **only** on that return value:

* `!= -1` → confusion check, then one `move_monster` step. No roll, one step per pass.
* `== -1` → if `+0x116 == 0`, `attack_monster_monster`. This is the **cold-trail**
  case, and it has no room compare — neither here nor inside
  `attack_monster_monster` (§3). A hunter that cannot find a step swings at its quarry
  from wherever it is standing, across a room boundary. Co-location is one way to
  reach this arm (via the 15797 short-circuit), not a condition on it.

The `+0x88` test at 20370 sits **ahead of** every acquisition arm: a monster with a
hunt link never picks up a player, whatever its behaviour mode or roam class.

### 6.2 As built (M7 slice 5)

`summon_spawn` takes an explicit `SummonLink` tag rather than a nullable session —
the four sites write genuinely different state, and a bool pair would not have said
so. `Pet(session)` for `cast_no_target` 0xc, `HuntUser(session)` for `monster_cast`
0xc **and** for `cast_user_target` 0xc, `HuntMonster(id)` for `cast_monster_target`
0xc, `None` elsewhere.

`cast_user_target` 0xc **is** implemented, contrary to the slice plan's assumption
that it needed PvP. It shares its whole body with `cast_no_target` 0xc in our tree
(both are `benign_success_effects`), and the discriminator is exactly
`target_id == session`: self → `Pet`, another player → `HuntUser(target)`. The DLL
uses this handler to sic a monster ON somebody, so tagging both arms `Pet` would hand
the caster a bodyguard for a spell that is meant to be an attack. Only the pet's
`attack_monster_user` half of `FUN_0044cc65` is genuinely M8 (game.rs `pet_assist`).

## 7. UNDETERMINED / flagged

* **Stale hunt links**: nothing clears `summoned+0x88` or the victim's `+0x60`
  back-links on either party's death (`check_kill_monster` 21245+ touches neither);
  monster ids are reused, so a long-lived hunter could redirect onto a recycled id.
  Not chased. **Closed by construction in our port** (M7 slice 5): our
  `MonsterInstanceId` is a monotonic u64, so an id is never reused and a dangling
  link is inert rather than misdirected. The DLL agrees on the observable in the
  simple case — `get_monster_data` fails inside both `dir_monster_travelling_coord`
  (15797) and `attack_monster_monster` (27226), making the arm a no-op — it is only
  the *recycled-id* case that diverges, and ours cannot occur.
* **`+0x60` back-link consumers**: the victim-side array is written (43922-43928) but
  no reader was located in this pass; suspected despawn/cleanup bookkeeping. Open.
  Two findings from the slice-5 re-read, both reasons **not** to port it speculatively:
  * The write loop has **no `break`**. It tests all ten slots and stores the summon
    id into *every* slot that is currently zero — so the first hunter cast at a
    virgin victim fills all ten entries with the same id, not one. Whatever the array
    was meant to be, it is not a working list of distinct hunters.
  * The victim's dirty flag `+0x140` is stamped **inside** that loop (43926), i.e.
    once per zeroed slot, alongside each write.

  Neither is ported. If a reader ever turns up, port the reader's expectation, not
  this loop.
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

## 8. As built (M7 slice 5)

The system above shipped in `crates/mud-core` on the `m7-content` branch. What follows
is the delta between this document and the code, so a future reader can tell a
deliberate divergence from a bug.

### 8.1 Divergences we chose

* **Owner keyed by `SessionId`, not name.** The DLL's `mon+0x1a` is a *string*; ours
  is an `Option<SessionId>`. Everything observable matches — the pursuit tier bumps
  `give_up` each fast tick on a dead session and releases past 15, the same ~16 s
  window as §4.2 — with one exception: a player who **re-logs in inside that window**
  gets a fresh `SessionId`, so the pet will not re-attach to them. The DLL's name key
  would. Judged the better trade (a name key would need name-uniqueness invariants we
  do not otherwise have), but it is a real behavioural difference.
* **Engaged-only experience split.** `attack_monster_monster`'s kill pays only the
  sessions engaged on the victim; the DLL's `distribute_experience(-1, ...)` also pays
  idle-autocombat users merely standing in the room. Recorded in §3 and at the
  function's doc comment.
* **The `+0x14` poison/bleed floor is not ported.** `attack_monster_monster`
  raises the defender's periodic HP-drain counter to result word `[3]`
  (27248-27249). Word `[3]` is `DAT_00495fdc`, zeroed on entry to
  `calculate_attack` (25246) and written by no path in it, so the raise is dead
  code in WG3-NT and porting it would only add a term that is always 0. See §3
  step 3. This is the second of the two divergences named at
  `attack_monster_monster`'s doc comment in `game.rs`.
* **`+0x60` back-links not ported.** See §7 — no reader exists, and the write loop is
  defective (no `break`; every free slot takes the same id). Porting a defect with no
  consumer buys nothing.
* **Typed hunt id (an improvement, not a divergence in the observable).** `mon+0x88`
  is a `MonsterInstanceId` from a monotonic u64 counter, which closes §7's stale-link
  hazard by construction. The DLL's arm is a no-op for a dead id too; only its
  id-recycling case is unreachable for us.
* **`monster_could_attack`'s pet exemption** (18237-18240) is unported because the
  predicate itself has no consumer in our tree — no rest gate, no `close`/`lock`
  guard. Noted at `sneak_command`, to land with whichever slice grows the first
  caller.

### 8.2 Open — needs the live board (slice 8 oracle expedition)

* ~~**THE BIG ONE — the monster Dodge(0x22) parry**~~ — **PARTLY MEASURED, see
  §8.3.** The 2026-07-26 expedition confirmed the formula at its 95 cap
  (28/31 connecting swings parried at a predicted 0.95) and found that the
  board words result 3 apart from a plain miss, which our port conflated. The
  LINEAR region of the step function is still open; so, newly, is the to-hit
  model, which the same transcripts put in question. Original statement of the
  problem retained below.

  Slice 5 gave the shared `build_monster_defender` its parry word (`[8]`
  ← Dodge(0x22), `move_monster_to_fighter` 25185-25186). That build is not
  m-v-m-specific: the DLL runs the same function for a player's swing at a
  monster, so wiring it here changed **ordinary player melee against a sixth of
  the bestiary**. **167** of the 1101 shipped templates carry Dodge(0x22), at
  values **10..200**, and `calculate_attack`'s parry block (25336-25360) turns
  `parry*10 / (accuracy/8)` — capped at 95 — of connecting swings into
  zero-damage parries: roughly **28-80%** across that band (giant bat, Dodge 20
  vs a ~45-accuracy character ≈ 40%). That formula was recovered from the 16-bit
  disassembly and has **never been checked against a capture**, so any error in
  it is now amplified across 167 templates. This is the single largest live
  gameplay change the slice made. Capture a grind against a Dodge-carrying
  template and compare the observed no-damage rate against the prediction.
  Cited at `game.rs`'s `build_monster_defender` (ORACLE-VERIFY) and pinned for
  shape — not for magnitude — by
  `game_combat.rs::monster_dodge_ability_parries_player_swings`.
* **The engage retaliation lock moved to the ATTACK command** (`12e6178`). The
  lock used to be taken on the combat round; the DLL takes it once, at
  engagement, from the ATTACK command (26230 lives in the other arm of the 26112
  split and cannot follow the round's post-damage branch). This is a change to
  **all** player melee, not just charm: it moved a `genrdn` draw earlier in the
  RNG stream and moved a golden in `spell_scenario.rs`. Decompile-justified but
  unmeasured — a capture of "attack, then let the round run" will confirm both
  the lock's timing and the draw order around it.
* **Instant-Enslave messaging** (§7) — **DECOMPILE-CLOSED (argument, 2026-07-28).**
  Closure standard, applied to this and the two items below: (a) a decompile
  citation, (b) a reproducible unreachability proof, (c) a fixture pinning
  current behaviour, (d) an explicit re-open condition.
  (a) `cast_spell_on_monster` 43806/43820: the `spell+0xce == 0` apply path
  prints nothing in the case body. (b) Exactly four shipped spells carry
  Enslave(6) — #49 song of charming (dur 100), #55 enslave (60), #88 control
  undead (80), #92 charm animal (60) — and
  `SELECT count(*) FROM spell WHERE 6 IN (abilitya_1..10) AND duration = 0`
  is **0**, so the instant arm has no shipped surface. (c) The fixture-only
  tests stay. (d) Re-open if any content update ships a duration-0 Enslave.
* **`is_valid_monster_target`'s fall-through** (38510-38560) — **DECOMPILE-CLOSED
  (argument, 2026-07-28).** (a) The roam-5 / fame / behaviour-4 sparing arms are
  implemented exhaustively from the decompile. (b) No shipped monster/room
  combination reaches them through the live sweep (zero measured surface after
  two expeditions' worth of transcripts; the arms gate on template fields whose
  shipped values bypass them). (c) Pinned by fixtures. (d) Re-open if content
  ever reaches a roam-5/fame/behaviour-4 arm live — the fixtures then need a
  capture behind them.
* **The match-10/0xd pet-command band** (38502-38509) — **DECOMPILE-CLOSED
  (argument, 2026-07-28).** (a) Decompiled and implemented as "valid only for
  your own charmed pet". (b) STRUCTURALLY unreachable: match types 10/0xd
  iterate players only, and with players excluded from the sweep they collect
  nothing and hit the no-effect refusal first — no input reaches the band.
  (c) The implementation and its fixtures stay as dead-faithful code.
  (d) Re-open only if the sweep is ever taught to include monsters.
* **The whole live lifecycle**: charm a low monster, walk it, watch one assist round,
  attack it as the owner, let a second charm expire — every string in the tests above
  is decompile- or inference-derived, and wants retagging ORACLE → MEASURED.

### 8.3 MEASURED (2026-07-26 expedition) — the Dodge parry

Transcripts: `re/oracle/oracle_dodge_parry_acc-mid{,2}.raw` (+ timing logs),
harness `tools/oracle/oracle_dodge_parry.py`, analysis
`tools/oracle/oracle_dodge_stats.py`.

**Method.** Oracle Delver (Dwarf Warrior, Str 50 / Agl 30, combat factor 6)
swings a summoned wooden hammer (1..1 damage, accuracy 0) at giant bats
(#71, AC 10, DR 1, Dodge(0x22) 20) in the caves under Newhaven. Str 50 adds
no damage bonus, so every connect lands at exactly `1 - DR*10/10 = 0`: the bat
never dies, and every connecting unparried swing renders as a glance. Accuracy
is set by worn NEGATIVE-accuracy gear rather than by level, which decouples the
step of `floor(accuracy/8)` being probed from the hit points the character
needs to survive.

**The rendering finding, which changes how this is measured at all.** The board
words result 3 on the player-attacks-monster path DISTINCTLY:

    You swing at giant bat who dodges your attack!

against the plain to-hit miss `You swing at giant bat!`. `WCCMMUD.DLL` carries
it verbatim at file offset **0xca40d**, `You %s %s who dodges your attack!`,
sitting one slot after the plain miss (0xca3ea) and one before the monster-side
result-3 pair (0xca430 victim view, 0xca464 room view) — so the player family
is the same hit/miss/dodge triple the monster family already has. We rendered
both outcomes as the plain miss; fixed, with `text::player_dodge`. The line's
COLOUR is still unmeasured (the capture was ANSI-stripped) and inherits the
plain miss's.

Because the two are worded apart, the parry rate is counted DIRECTLY over
connecting swings and does not depend on the to-hit model at all.

**Result — the 95 cap holds.** Level 2 wearing the smoky black talisman
(accuracy -20), encumbrance 24% → skill -7 → **accuracy 23**, so
`floor(23/8) = 2` and `20*10/2 = 100` caps to a predicted **95%**:

    swings 37:  dodge 28   glance 3   miss 6   hit 0
    parry = 28/31 connecting = 0.903,  95% CI [0.743, 0.980]

The interval contains 0.95 and excludes every lower step (d=3 → 0.66,
d=4 → 0.50, d=5 → 0.40), and the no-parry null. The data also favours the cap
being **95 rather than 100**: an uncapped `chance = 100` still fails on a roll
of exactly 100, i.e. ~1% of connects get through, where a 95 cap lets ~5%
through; we saw 3/31 = 9.7%, comfortable under 95 and unlikely (~0.3%) under
100. Evidence, not proof.

**Result — the linear region, and the accuracy dependence.** Same character,
same target, talisman off: level 2, encumbrance 24% → skill 14 → **accuracy
43**, so `floor(43/8) = 5` and `20*10/5` predicts **40%**
(`oracle_dodge_parry_acc-high.raw`):

    swings 60:  dodge 26   glance 32   miss 2   hit 0
    parry = 26/58 connecting = 0.448,  95% CI [0.317, 0.585]

The interval contains the predicted 0.40. It does NOT separate 0.40 from its
immediate neighbours — 0.50 (d=4) and 0.333 (d=6) are both inside — which
needs roughly 92 and 207 connecting swings respectively, against the 58
collected before the run aborted. What it does exclude decisively is 0.66
(d=3) and the 0.95 cap.

**That exclusion is the point.** Taken together the two blocks are a
two-point test of the formula's ACCURACY DEPENDENCE, which no single block
can give:

| block   | accuracy | floor(acc/8) | predicted | measured | 95% CI         |
|---------|----------|--------------|-----------|----------|----------------|
| acc-mid | 23       | 2            | 0.95      | 0.903    | [0.743, 0.980] |
| acc-high| 43       | 5            | 0.40      | 0.448    | [0.317, 0.585] |

The intervals are disjoint. Nothing changed between them but the character's
accuracy — same template, same Dodge 20, same weapon, same rooms — and the
parry rate moved from ~90% to ~45%, in the predicted direction and close to
the predicted magnitude. A formula that did not divide by accuracy cannot
produce that.

So: the shape is confirmed and the cap is confirmed; the exact denominator in
the linear region is consistent-but-not-pinned. Note that 0.448 sits nearer
d=4 (accuracy 32-39) than the d=5 we compute, which — if it survives a larger
sample — would indict our ACCURACY derivation rather than the parry formula,
and would sit alongside the two anomalies below as the same class of problem.

**RESOLVED (2026-07-26, same day): the armour anomaly was a column swap, not
a 10x.** This paragraph originally read: worn AC soaked far less damage than
the port models — a grey spider (3..12) bit for 3 and 9, and a giant bat
(2..5) for 2, 4 and 5, i.e. the full undiminished ranges — and guessed that
the DLL's armour word holds the DISPLAYED value, making our soak 10x too
strong. That guess was wrong, and two of the facts it rested on were wrong
with it.

`move_player_to_fighter` (24786-24815) accumulates **two different item
columns into two different fighter words**: `+0x342` (DB `ac`) into `[1]`,
÷10 at 24866, which is the TO-HIT term; `+0x39c` (DB `dr`) into `[3]`, raw,
which is the soak `calculate_attack` divides by 10 at 25335. The port had
them crossed. Both columns ship pre-multiplied by 10 and are genuinely
independent — 319 of the 565 AC-bearing shipped items carry no DR at all —
so the effect was not a rescale: it invented resistance out of the AC of
every piece of armour in the game.

The transcripts pin both numbers, because the status line prints the pair
(`get_armour_rating` 16956 → 31553-31556 divides both by 10). The acc-high
set was gilded robes 70/0, chain coif 45/8, displacer fur cloak 10/0, beaded
belt 0/0, violet orchid 0/0 — Σac **125**, Σdr **8** — and the board printed
`Armour Class:  12/0`. Σdr/10 = 0 is exactly the soak the bats demonstrated.
Σac/10 = 12 would have made that character immune.

Two corrections to the original text:

* **No transcript ever showed `Armour Class: 13`.** The three captures read
  `12/0` (acc-high), `0/0` (acc-mid) and `0/0` (acc-mid2). The 13 came from
  reasoning about a 130-`ac` tunic, not from a capture — and 130/13 turns out
  to be `rigid leather tunic`'s ac/dr pair, i.e. the very two columns at
  issue, which is how the coincidence went unnoticed.
* **The smoky black talisman's -20 is ability 2 (AC), not accuracy.** The
  item carries BOTH a -20 `accuracy` column and an AC(2) -20 ability, so the
  acc-mid block varied the character's AC as well as their accuracy — which
  is why its display reads `0/0` rather than `12/0`: 125 + (-20×10) = -75,
  clamped at 0 by 17045-17048. The parry result is unaffected either way,
  being counted directly over player swings.

Fixed on branch `armour-columns`; pinned by
`crates/mud-server/tests/armour_real_content.rs`, which reproduces `12/0`
from the shipped columns with no board. The AC(2)/DR(7) dynamic
accumulators (`+0x70c`/`+0x7b6`) were unported for players and now land too.

**The to-hit model — half settled, see §8.4.** The same transcripts give
31/37 = 0.838 connecting against a predicted 0.67, with the prediction just
outside the 95% interval [0.680, 0.938]. Borderline at n=37, and no longer
supported by the armour anomaly, which has a different cause. It now has a
fair test it did not have before — the player's evasion word was 0 for every
geared character until this fix, so any earlier to-hit comparison was made
against a defence the port was not applying. The monster side of the
question (whether the template AC term needs its own scale check) was
measured the same day and is CLOSED; the accuracy side is not.

~~Also unresolved: the board reports `Encumbrance: x/2880` for a Str-50
character where `stats.rs` computes `str * 48` = 2400.~~ **RESOLVED
(2026-07-28, carry 4c):** the denominator was already right in code —
`Core::carry_capacity` (game.rs) applies `get_max_weight`'s Encum(96)
percent, `(100 + encum)/100`, over `calculate_secondary_stats`' `str*48`
(stats.rs): `2400 × 1.2 = 2880`, pinned by `tests/inventory.rs` (the
`Encumbrance: 0/2880` goldens). The only unmodeled scrap of the chain is
the DLL's key-array weight (`+0x334[50]`, decompile 67745-67775) — inert
unless keys are carried, and the oracle character carries none. (Still
true that it does not explain §8.4's residue: at the captured 711 units
both denominators floor to the same `enc/10 = 2`.)

### 8.4 MEASURED (2026-07-26) — the monster AC scale, and the miss colour

Transcript `re/oracle/oracle_dodge_parry_control.raw` (+ timing log), harness
config `control` in `tools/oracle/oracle_dodge_parry.py`.

**The question.** `build_monster_defender` feeds the RAW template `ac` column
into the evasion word, while the player's own side of that word divides its
item column by ten (24866). Shipped monster `ac` runs 0..9999 with a mean of
**101** against a geared player's ~12, and defense enters the threshold
SQUARED, so if the monster column were also in tenths every monster in the
game would be far easier to hit than we model.

**Method.** The same Oracle Delver, kit and rooms as §8.3, with one change:
the target. Grey spider #30 is AC 20, DR 2 and carries **no** Dodge(0x22), so
the parry channel is absent and every connect is a glance — the run measures
to-hit alone. Swapping AC 10 for AC 20 at accuracy 23 separates the two
readings by nearly the whole range, because `100 - defense²/(accuracy²/14/10)`
truncates at every divide: the raw column gives 400/3 = 133 over 100, i.e. the
clamp FLOOR of 10, where a tenths reading gives defense 2 and a threshold of
99.

    swings 21:  glance 2   miss 19   dodge 0   hit 0
    connect = 2/21 = 0.0952,  95% CI [0.0117, 0.3038]

**Result: the column is whole units and the port is right as written.** 0.10
is essentially the point estimate; 0.99 is excluded outright. This is also the
first live measurement of the **clamp floor** itself, which no test had ever
exercised — every combat fixture fights an AC 0 sandbag, where the threshold
clamps to 99 at the other end and the to-hit term never bites. Pinned by
`game_combat.rs::a_monsters_armour_class_is_a_whole_unit_not_a_tenths_scale`,
verified by applying the /10 mutation and watching it fail.

The run ended early — the character was killed at HP −18. At a 10% connect
rate against a spider that bites for 9-12, a sandbag target the player cannot
kill takes a very long time to hit back at, and the healer cycle could not
keep up. Not a problem for this measurement, which needed ~20 swings, but any
future high-AC block wants a bigger HP buffer or a lower-damage target.

**Correction to §8.3, found on the way through: the raws are NOT
ANSI-stripped.** §8.3 filed the parry line's colour as unmeasurable on that
premise. The raws carry ANSI throughout, and reading the attribute byte ahead
of each line — counting only segments whose entire content is the line in
question, so the code cannot belong to a neighbour — gives, with no
exceptions across all three captures:

| line | wording | colour | count |
|---|---|---|---|
| plain miss | `You swing at giant bat!` | `0;36` | 24 |
| parry | `You swing at ... who dodges your attack!` | `0;36` | 46 |
| glance | `Your ... glances off ...` | `0;31` | 36 |

So §8.3's guess that the parry inherits the plain miss's colour was right, but
the plain miss itself was painted with the glance's red: our one
"miss/glance family" constant was two thirds wrong. A swing that never
connected is cyan, like the incoming monster lines; only the
connected-but-soaked glance is red. Fixed, with `text::color::YOUR_GLANCE`
carrying the red.

**What remains open (carry 4b's residue): the accuracy derivation.** With the
monster scale settled, the acc-mid block's 0.838-against-0.670 still wants an
explanation, and the two channels of that same block disagree about the
cause: the parry rate wants accuracy ≤23 (denominator 2), while to-hit wants
≥25. This control run bounds it from the other side — at AC 20 the threshold
sits on the clamp floor for any accuracy in 13..25 and rises to 0.34 by
accuracy 29, which the interval excludes. A joint fit across all three blocks
favours accuracy ~25-27 where we compute 23, i.e. our derivation reading a
couple of points LOW. That is a two-point discrepancy inferred from three
small blocks, not a finding. The clean next measurement is a high-AC target
at an accuracy well clear of the clamp, where the threshold is steep in
accuracy rather than pinned.

**Re-fit under the corrected engine model (2026-07-28).** Before spending
board time on that measurement, the slice-8 fidelity fixes landed and the
existing raws were re-fit under the corrected to-hit: the [10,99] clamp
falls through to the den==0 arm, the comparison is STRICT, and genrdn's
upper bound is EXCLUSIVE (theft.md:37 was wrong; MBBSEmu implements the
ordinal as `_random.Next(min, max)`), so `P(connect) = (threshold-1)/99`
and the AC-20 clamp floor is 9/99 = 0.0909 — tighter against the control's
0.0952 than the old model was. The gate was: 4b closes iff one accuracy
sits inside all five intervals. It does not — the conflict SHARPENS:

- acc-high (60 swings): to-hit 0.929 in [0.885, 0.996], parry step d=5
  0.40 in [0.317, 0.585] — consistent, and insensitive to ±1 accuracy.
- control (21 swings): floor 0.0909 vs 2/21 = 0.0952 — consistent for any
  accuracy ≤ 28 at this config.
- acc-mid (37 swings, both channels on the SAME swings): to-hit 0.667 at
  accuracy 23 is EXCLUDED by [0.680, 0.938] — wants true accuracy ≥ 24;
  the parry step d=3 (0.66) is EXCLUDED by [0.743, 0.980] — wants true
  accuracy ≤ 23. No single accuracy satisfies both.

So the residue is not (only) a constant offset in the derivation; at least
one formula SHAPE is off. One candidate that fits the acc-mid to-hit at
accuracy 23: the bat's effective defense word is ≤ 8, not the raw `ac` 10
(a negative `word[2]`, or an AC-adjacent term we have not traced). The
expedition design discriminates this from an accuracy offset: the B1/B2
parry-cliff pair moves with accuracy alone, while the A3/A4 kobold pair
varies accuracy against a FIXED defense so the ratio cancels any constant
defense offset. Carry 4b stays open; the expedition is gated IN.

### 8.5 MEASURED (2026-07-29/30 campaign) — the to-hit model and the Dodge parry step function CLOSE

Transcripts: `re/oracle/oracle_dodge_parry_{a1,a12,a2,a22,a3,a4,a42,b1,b12,b2,b3}.raw`
(+ timing logs), ~1,300 swings across seven design points, two characters'
worth of tuition, and one board circadian lesson. Analyzer:
`tools/oracle/oracle_accuracy_fit.py` (joint binomial log-likelihood over
`delta` = accuracy-derivation error and `w` = untraced defense offset).
All predictions use the corrected engine model (§8.4 tail: strict roll
over genrdn's [1,99], clamp over every arm).

**The design.** Phase A (no cursed gear): a1/a2 put the d=5 and d=4 parry
steps on the giant bat at accuracies 43/33; a3/a4 probed the to-hit curve
against the kobold's AC 30 at accuracies 39/43 — a no-Dodge target at
FIXED defense, so the a3/a4 connect RATIO cancels any constant defense
offset and separates `delta` from `w`. Phase B (heavy band, `skill =
ratings`): b1/b2 slid the floor(acc/8) parry cliff across 23/21, and b3
(malachite alone, light band, accuracy 29) filled the d=3 step.

**The verdict: delta = 0, w = 0.** Every clean block sits on the model in
both channels:

| block | acc | target | connect obs/model | parry obs/model (d) |
|---|---|---|---|---|
| a1 (+acc-high) | 43 | bat 10 | .929 / .929 | .429 / .40 (5) |
| a2 | 33 | bat 10 | .845 / .859 | .542 / .50 (4) |
| a3 | 39 | kobold 30 | **.087 / .091** | — |
| a4 | 43 | kobold 30 | .380 / .303 (CI ok) | — |
| b1 | 23 | bat 10 | .708 / .667 | .927 / .95 (2) |
| b2 | 21 | bat 10 | .757 / .667 (CI ok) | .960 / .95 (2) |
| b3 | 29 | bat 10 | **.841 / .838** | **.653 / .66 (3)** |

The a3/a4 ratio observed .248 against the delta=0 model's .300 — any
accuracy offset of +1 pushes the ratio past .60. The joint fit puts
(0,0)/(0,-1) statistically tied at the top (the tie is entirely the
retired block below) with every other cell 13+ nats behind.

**Carry 4b CLOSES.** `move_player_to_fighter`'s accuracy derivation and
`calculate_attack`'s threshold are right as written. The 2026-07-26
acc-mid block (0.838 connect at a computed accuracy 23) is RETIRED as
contaminated: b1 re-measured the same design point with guard-verified
staging and landed on the model (.708 vs .667), while b3 showed that
0.838 is the accuracy-29 rate EXACTLY (.8407 measured at 29, .8384
modeled) — that block was evidently not at accuracy 23, and its
smoky-black-talisman staging (whose failure modes §8.3's own field notes
document) is the likely culprit.

**Carry 1 CLOSES.** The parry step function
`min(95, dodge*10 / floor(accuracy/8))`, recovered from the 16-bit
disassembly and never before captured, is measured at four denominators:
the .95 cap (d=2, pooled 176/187 = .941 across b1/b2/acc-mid) and the
linear steps .66 (d=3: 162/248 = .653), .50 (d=4: .542), .40 (d=5:
pooled 115/263 = .437). No rival formula shape fits those four points.

**Field notes** (each encoded as a harness guard): staging must happen at
the healer (map-6 login mauled a character mid-summon); the enc-33 cliff
and the enc-30 band edge move `skill`, so every config lands an accuracy
invariant across its nearest edge and EXPECT_ACC aborts on drift; the
two rings share ONE Finger slot; `/xcash` after each heal keeps the purse
weight staged AND the heal budget bottomless; bats are nocturnal
(02:00-08:00), giant rats never spawned in three 152-room laps, and
populations diffuse out of their spawn rooms with board uptime, so
spawn-dependent expeditions restart the board first; running out of
lives DELETES the character, and a creation-screen `look` reads exactly
like an empty world to a sweeping script.

### 8.6 MEASURED (2026-07-30) — the charm lifecycle, by suppression

Transcripts: `re/oracle/oracle_charm_lifecycle6.raw` (E4 bulk),
`oracle_charm_finish.raw` (E5), `oracle_charm_definitive.raw` (the
confirmed charm + expiry), `oracle_charm_strand.raw` (the strand test),
plus five earlier takes whose failures taught the method. Character:
Bard Minstrel (Human Bard L12→L14, songs #49/#96 via sysop summon).
Pet: the plain kobold #404 (`charmlvl` 12, aggression 30) — map-6 rooms
722-751 are an exclusive spawn band (zone 24 at level exactly 19).

**The method that finally worked: SUPPRESSION.** Two generations of
follow-detection false-positived (attack lines counted as follows, then
pursuit arrivals counted as follows — an aggro'd kobold chases a
fleeing Bard between rooms and "arrives" exactly like a pet). The
unambiguous signal is the charm's own state write: `mon+0x116`
suppresses the pet's attacks on the owner. The definitive driver finds
a SINGLE-kobold room, lets it prove hostility (it attacks first), sings
until the attacks cease for a full 25 s probe window in melee range,
and treats attacks resuming as the expiry stopwatch.

**E4 (carry 3) — measured:**
- Cast success line, verbatim: `You sing the song of charming to
  kobold!` — printed on EVERY successful cast roll, including casts
  whose charm application then fails silently (observed cleanly: a
  sung-at kobold kept attacking through the full probe window; the
  §1.1/§1.2 save and gate refuse without a message beyond the resist
  arm's `resists your spell`).
- Resist wording captured (charmres 60 ≈ 30% per cast under the
  corrected genrdn).
- Charm CONFIRMED by suppression (two independent runs, sings 2 and 0).
- Follow: walked-room arrival lines captured in the 10-room walk
  (lifecycle6) and the 3-room walk (definitive).
- Attack-own-pet exchange captured (lifecycle6; cost a death — the
  release turns an aggression-30 pet hostile).
- **Duration**: attacks resumed at +234 s from the confirmed charm;
  probe granularity brackets the true duration in ≈[209, 261] s. At the
  blur-measured 3.03 s tick that is ≈69-86 ticks against spell #49's
  base 100 with level scaling — the bracket is consistent with the
  `add_cast_spell_to_monster` formula and pins the scale; a tighter
  stopwatch (10 s probes) would pin the exact tick count.

**E5 (carry 4) — strings measured, charge closes on the decompile:**
- `You sing the song of foolishness to kobold!` at a PASSIVE target →
  immediate retaliation (the benign-cast grudge, measured: it stabbed
  back within the same exchange).
- Casting at your OWN ENGAGED target first prints `*Combat Off*` (the
  cast disengages autocombat) — an unlooked-for wording pin.
- The EvilInCombat(52) charge itself has NO live observable: `st`
  carries no alignment/evil surface (verified before/after both casts,
  byte-identical). The charge's bookkeeping closes DECOMPILE-CLOSED
  under §8.2's argument standard, with the behavioral strings above as
  the measured shell around it.

**E6 (carry 5) — the give_up model is consistent:**
- Ordinary walked following does NOT release: 13 followed rooms across
  two runs without a release.
- A STRANDED pet (teleport away — no breadcrumbs to prosecute) was
  gone within ONE 40 s exile (+77 s from charm, far under expiry). A
  charmed monster never wanders (§2.1), so its absence means give_up
  released it mid-exile — consistent with the port's cumulative
  16-failed-fast-tick model (~16 s to release, then aggression-30
  wandering resumes). The port's model stands as faithful.

**E3 (carry 2) — protocol built, measurement starved:** the engage-lock
trials (P0 aggression baseline / P1 attack-and-leave / P2 zero-damage
retaliation with the 0..0 flurry of blades / P3 control) are fully
scripted in `tools/oracle/oracle_engage_lock.py` with subject switching
and every survival guard this campaign produced — but every launch
window found the subject rooms empty or lethal, and the lock's
relocation stays DECOMPILE-JUSTIFIED (26230 in the other arm of the
26112 split) with the protocol on the shelf for a future session.

**The tuition ledger** (why five takes): instance-disambiguator
adjectives ("nasty kobold" IS #404); pursuit arrivals mimic follows;
dark rooms read as empty worlds without a lantern; `monster.index` is
LEVEL and `monster.group` is the spawn zone (the "kobold slave" plan
died on this — it is one rare candidate among the orc gang's band);
the spawner is player-driven so fast sweeps are spawn-proof; two
characters permadeathed before the lives floor existed.
