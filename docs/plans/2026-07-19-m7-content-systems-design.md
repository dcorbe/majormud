# M7 Content Systems — Design

> **BRANCHING CHANGED, 2026-07-26.** Slices 1-5 and the slice-8 Dodge-parry
> expedition were merged to `main` **mid-milestone**, together with the
> `mud-client` track (C0-C9), and `main` is now a rolling trunk that both
> tracks develop on. Since M0 `main` had carried only completed milestones;
> that convention is retired. The reason is that the tracks were never really
> independent — `crates/mud-client`'s parser tests call `mud_core::text`
> directly, six of them boot `mud-server` in-process, and both write to
> `re/oracle/` — so separate branches only let that coupling drift unnoticed.
> The merge proved the point immediately: three client tests broke, one of
> them because M7's crime system made a passive fixture monster unattackable,
> and none of those breaks were visible while the branches were apart.
>
> Slices 6 (quest VM), 7 (gangs) and 8 (close-out) remain outstanding, and
> `M7 PENDING` markers are live on `main`.

Milestone 7 of the MUD reimplementation
(`2026-07-16-mud-reimplementation-design.md`): quest text-block VM, gangs +
`.HSE` guild houses, charm/pets, theft (full thief kit), crime/fame,
martial-arts verbs, per-user ANSI, name-generator adjectives. Behavioral
authorities: `re/docs/quests.md` (VM + interpreters + rewards),
`re/docs/gangs.md` (records, membership, economy, .HSE),
`re/docs/combat.md` (MA attack modes), `re/docs/monsters.md` §2.6 (name
roll), `re/docs/spellcasting.md` (Enslave semantics). Two new specs are
born in slice 1: `crime.md` (fame/legal levels) and `theft.md` (rob +
picklock). Decompile citations are to
`re/wg_nt_ghidra/exports/WCCMMUD_decompiled.c` line numbers.

**Decompile facts established during design** (verified against the
export; fold into the docs in slice 1):

- **`monster_rob_user` (0x295bd, decompiled.c:23792) is a stub —
  `return 0`** (read directly, confirmed). Monster robbery never happens
  in WG3-NT. The kind-3 attack form ships as an exact stub port; the real
  theft system is player-side: `cmd_rob` (0x4528fb, 510 lines) →
  `rob_monster` (0x41fc96, 87 lines) / `rob_user` (0x41f48d, 2037 lines),
  plus `cmd_picklock` (0x454856, 1907 lines).
- **`get_legal_level` (0x44e390) fully pinned** (read directly,
  confirmed): signed `player+0x542` → tier: `< -200 → 7`, `< -0x32 → 6`,
  `< 0x1e → 0`, `< 0x28 → 1`, `< 0x50 → 2`, `< 0x78 → 3`, `< 0xd2 → 4`,
  else `5`. Eight legal levels, negative = good side. M6's guardian
  (fame ≥ 0x28) and criminal-hunter (fame ≥ 0x50) raw compares are these
  boundaries. `evil_for_robbing` (0x44e89f, 56 lines) is a named fame
  writer.
- **`get_random_name` (0x424172, decompiled.c:20728) decoded** — the
  spawn-adjective source is a **WCCTEXT2 text block** (id =
  `knmsr+0x124`). Per line: `A:` suffix-after-base, `B:` prefix-before-
  base (the M6 "nasty orc rogue" capture), `F:` full replace, `N:` base,
  else the line verbatim; separator byte `DAT_00480efa`; per-line roll
  `genrdn(0,100)` accepts on ≤ 9 (10%), else keep walking (last composed
  candidate wins at end of block); 999-line cap, 29-char truncation.
  Name-gen is therefore data-gated on the WCCTEXT2 import.
- **`cmd_tame` (0x4528e3) and `cmd_mesmerize` (0x4528ea) are stubs**
  (`return 0`), and no order/dismiss/pet verbs exist in the export —
  charm has no command surface; it is purely Enslave spell state.

## Scope decisions

- **Quest VM only, zero quest authoring** (USER DECISION). The engine
  ships the vocabulary (~40 `perform_matched_action` verbs), the three
  interpreters, and the hardcoded completion thresholds (quests.md
  §2-§4). Every actual quest already exists as data in `wcctext2.vir` —
  importing it IS the content work. No hand-written quest content.
- **Full thief kit in M7** (USER DECISION — overrides the draft's M8
  deferral): `rob <monster>`, `rob <player>`, and `picklock` all land
  here. The monster-side rob form stays the WG3-NT `return 0` stub,
  closing the `game.rs` kind-3 marker divergence-free. `rob_user` (2037
  lines: victim notification, detection, forgive interplay) and
  `cmd_picklock` (1907 lines: locked exits, keys, traps) each get a full
  decompile pass in slice 1 before any code.
- **Charm is spell-state, not verbs.** Enslave-at-monster sets the charm
  bit; pets follow, assist, and revert on expiry/termination.
  `tame`/`mesmerize` parse and no-op exactly as the DLL stubs do.
  Player-target Enslave stays the silly-spell placeholder (already
  true). **Monster-vs-monster combat resolution comes into scope** —
  pet assist is unreachable without it.
- **Crime/fame lands before its feeders.** `+0x542` already has readers
  (M6 guardians/hunters); M7 adds the writer families (rob, `addevil`,
  plus whatever the slice-1 store-site enumeration finds). The deferred
  alignment gates (`user_can_use`, spell lattice) unlock here.
- **Gangs: full membership + economy + .HSE display; NO gang war, NO
  .HSE editing.** Gang-vs-gang hostility needs PvP combat (M8). .HSE
  authoring is out-of-band even in the original (gangs.md §7: the CREATE
  build path is stubbed and no in-game editor exists) — we stream the
  135 shipped files from `re/hse_files/` read-only.
- **Per-user ANSI = our own toggle command** (USER DECISION), persisted
  per character in state.sqlite, overriding the `CoreConfig.ansi` global
  at the output funnel. The real board keys ANSI on the MBBS account
  outside the DLL, so the toggle itself is a documented divergence; the
  rendering on each side of the flag stays byte-exact. Slice 1 greps
  `cmd_set` (0x458b60) to confirm the DLL has no in-game surface.
- **Backstab verb rides the MA slice.** `AttackType::Backstab` is
  already modeled, the hidden bare-attack divert exists, and
  `cmd_backstab` (0x45156f, 260 lines) is the last unparsed attack-mode
  verb — cheapest to land alongside kick/jumpkick.
- **Quest-flag persistence = the full 30-slot innate ability table**,
  not just the 11 quest ids — `giveability`/`addability` take arbitrary
  `(id, value)` pairs, so the schema is generic from day one.

## Approach

Eight vertical slices in dependency order; each ends green,
hand-testable, committed. Extraction/import is slice 1 because **three
systems are data-gated on WCCTEXT2** (quests, name-gen, ask/greet
dialogue) and six open questions need decompile passes before TDD can
cite a spec. Quick wins come immediately after: the MA verbs are
decompile-complete, the ANSI toggle is plumbing, and name-gen churns
existing goldens (`tests/cast_messages.rs`, `tests/spawner.rs` expect
bare names) — re-pin those before M7 stacks new goldens on top; name-gen
doubles as the WCCTEXT2 import's first consumer. Crime precedes theft
(theft writes fame) and quests (`addevil` writes fame; gates read legal
level). Theft precedes charm (contained, after the stub discovery);
charm precedes quests (the VM's `summon`/`cast` verbs want owner links
and monster-vs-monster resolution in place). Gangs go last of the big
slices: they touch nothing the other systems need, need two-character
oracle staging, and carry the milestone's largest persistence surface.

**Slice 1 COMPLETE (2026-07-19, 591 tests, commits e517353..01cc7f0):**
WCCTEXT2 solved — the set's only VARIABLE-length Btrieve file (shadow 'D'
head pages with generation liveness, logically-numbered 'V' text pages,
VRP page pointer, +0x20 byte-shift obfuscation; format in the importer
docstring + vir_schemas.md). 3267 blocks imported; 4 dangling next-links
allowlisted; TextBlock model + loader + boot validation landed.
**Column correction: monster `desctxt` = the name-generator block id
(knmsr+0x124), not a description.** New specs born: `crime.md` (all fame
via add_evil_points; 8 named tiers @0x4881a4; forgive/retaliation pair
timers; fame banks to the BBS account; NO kill-path fame, NO decay;
evil_for_robbing = dead code) and `theft.md` (711 lines). THEFT
SURPRISES: **`rob <monster>` is dead code in WG3-NT** — cmd_rob always
passes a NULL item arg so rob_monster never acts (slice 4 reproduces the
no-op; player theft = rob_user only); random item robs require LoyalItem
(ability 100) so keys are the practical loot; attempt_to_forgive has a
use-after-free bug (predecessor node freed — DO NOT clone, divergence
note); picklock gates on exit types 2/7/0xb (not the 9/0xc/0x10 disk
codes), lock-traps cast room+0x5fa, 300s/unit re-lock timers.
`charm.md` born: charmres@0x1a0 save + charmlvl<=level silent gate;
mon+0x1a shared name-link (grudge/owner by +0x116); pet assist =
owner's autocombat target each 5s pass; attack_monster_monster = full
calculate_attack forced mode 5; release on expiry/give-up(>15, incl.
owner logout)/owner-attacks-pet; owner death does NOT release. Quest
completion rewards recovered from disasm — PERMANENT STAT GRANTS with
clear-then-regrant idempotence (old skill-grant inference retracted).
GANGEXP deed price = MSG option 66 (default 1000 ×10000 exp);
DAT_00480efa separator = " "; gang bank-8 key = last STOCKER's BBS
account id; WCCGANG2.VIR ships empty (schema confirmed for state.sqlite);
cmd_set has no ANSI subcommand (our toggle = clean divergence).

## Slice 1 — Extraction, import, decompile passes (zero behavior change)

- **WCCTEXT2 importer:** extend `re/import_mmud.py` with a bespoke
  `textblock` table (not a Nightmare RecType — 2024-byte records keyed
  `(seq@0, block#@8)`, read by `get_text_block` 0x3379c; verify the body
  offset and continuation-record layout against the decompile before
  writing the reader). Source
  `re/wg_nt_ref/WCCNT8PJ/out/wcctext2.vir` (7.4 MB) via `re/vir_wg.py`;
  assemble continuation records by seq into one row per block id.
  Validate: monster `greettxt`/`desctxt` ids resolve; the orc-rogue name
  block (`knmsr+0x124`) contains the `B:` line matching the M6 "nasty
  orc rogue" capture.
- **Column↔offset pinning** (M6 slice-1 `_raw` precedent): the monster
  sqlite column for `knmsr+0x124` (name-gen block id) and `charmres`
  neighbor confirmation. Recorded in `vir_schemas.md`.
- **WCCGANG2 layout confirmation:** parse
  `re/wg_nt_ref/WCCNT8PJ/out/WCCGANG2.VIR` (28 KB) with `vir_wg.py`
  against the gangs.md §0 offsets (name key +0x00, display +0x14, exp
  +0x28, leader +0x2c, count +0x4e, flags +0x50, secondary pool
  +0x58/+0x5c). Informs the state.sqlite `gang` schema only — runtime
  gangs never read the .VIR.
- **Decompile/disasm passes** (each closes a named unknown):
  1. **`+0x542` writer enumeration** — every store site specced
     (`evil_for_robbing`, the `addevil` verb handler, NPC-kill sites).
     Output: new `re/docs/crime.md` with the legal-level table +
     alignment display-string pins.
  2. **Full thief kit** — `cmd_rob` (510) + `rob_monster` (87) +
     `rob_user` (2037) + `cmd_picklock` (1907): skill rolls vs
     THIEVERY/STEALTH/PICKLOCKS/FINDTRAPS, coin/item selection, victim
     notification + forgive interplay (`cmd_forgive` if present),
     detection → aggro/fame writes, locked-exit + key + trap semantics.
     Output: new `re/docs/theft.md`. Confirm the kind-3 caller
     fall-through (decompiled.c:26808).
  3. **Charm surface** — Enslave-at-monster handler (charmlvl/charmres
     gate, `mon+0x128 |= 1`, owner +0x140, suppression +0x116, rename),
     termination reversal, pet-assist target pick, and the
     monster-vs-monster swing path. Extends spellcasting.md/monsters.md.
     Pin what `return 0` from tame/mesmerize surfaces to the user.
  4. **Quest reward push-args** — `re/DumpAsm.java` over 0x414d23: the
     exact `(abilityId, value)` pushed per threshold before each
     `call 0x46c507` (quests.md §6 item 1).
  5. **Constants + odds and ends** — `cmd_set` ANSI scan;
     `DAT_00482d10` deed price and `DAT_00480efa` name separator via
     `re/DumpBytes.java`; gang bank-8 account-name writer
     (`shop+0x128`, gangs.md §7).
- **Loader:** `content.rs` gains `TextBlock` + monster `name_block`;
  `content_db.rs` loads the new table; boot validation that referenced
  block ids resolve.

Deliverable: regenerated `mmud_wgnt.sqlite` with `textblock`, crime.md +
theft.md born, quests.md §6 / gangs.md §7 items closed, zero behavior
change, full-DB boot test green.

**Slice 2 COMPLETE (2026-07-19, 607 tests, commits cec94a7..HEAD; code
side — the live oracle pass below is the open item):** punch/kick/
jumpkick verbs landed (cmd gates decompile-read: no ability → return 0 →
SAY fallthrough, same surface as tame/mesmerize; punch's hidden/sneak
divert to mode 4 wired in slice 4 with HIDE); attack mode stored on the
session (autocombat +8 model), fighter build + EU keyed off it; kick/
jumpkick damage confirmed to ride the mode-2/3 damage SEEDS (33/66 →
×1.33/×1.66 shown bands — test-derived). ARMED EU NOW HONORS WEAPON
SPEED (`+0x3de`; the M4 comment was stale and armed swings always ran at
1200 — 4 shipped 0-speed weapons like "flurry of blades" now hit the
6-swing cap, intentional data). Name-gen ported as pure
`text::generate_name` (decompile-read directly: per-line candidate,
accept ≤9, LAST candidate on walk-off, caps 28/29, trailing-newline tail
is the terminator; unit-pinned incl. the 1000-line ceiling), instance
display names on MonsterInstance (targeting + also-here + combat use
them; death lines re-read the template name); fixture spawns draw from
the main stream, density spawns from spawn_rng (draw position: after
item draws, before the direction pick — L21102). Per-user ANSI:
`Player.ansi` overrides the global at the funnel, `ansi` toggle command
(strings OURS), persisted column w/ backfill 1 for pre-M7 rows.
BACKSTAB MOVED TO SLICE 4: cmd_backstab gates on hidden/sneak state,
which theft.md owns. PENDING ORACLE (slice-8 sweep or earlier): MA verb
min-abbrevs, kick/jumpkick swing-verb strings (we reuse the fists verb
pools — unmeasured), live adjective spawn capture vs our walk.

## Slice 2 — Quick wins: MA verbs, backstab, name-gen, ANSI

- **Verbs** (`command.rs` VERBS + dispatcher): `punch` (mode 1 — math
  already in, just parse), `kick` (mode 2: max `L*V/6+7`, abilities
  0x5a/0x5d, speed 0x578), `jumpkick` (mode 3: max `L*V/6+8`, 0x5b/0x5e,
  speed 0x76c) per combat.md; `backstab` (mode 4, routes to the existing
  hidden-divert path); attack mode stored on the autocombat record.
  Min-abbreviations oracle-measured.
- **Name-gen:** port `get_random_name` at spawn; composed name stored on
  `MonsterInstance`; **death lines keep the base template name**
  (spellcasting.md:999). Draws from the spawner RNG stream (M6
  divergence precedent). Re-pin the two bare-name goldens.
- **ANSI:** `player.ansi` column (default = global server flag),
  per-session override at the funnel; toggle command per the slice-1
  `cmd_set` finding (ours if absent — documented divergence).

Tests: MA damage-band units citing combat.md; verb-abbreviation table;
seeded name-gen golden (fixture block, exact 10%-walk draws); ANSI
on/off funnel byte-diff. Oracle: MA hit/miss strings + speeds (Mystic),
adjective spawn lines (passive capture), backstab strings.

**Slice 3 COMPLETE (2026-07-20, 625 tests; leftovers landed: criminal
respawn split fame >= 0x28 → criminal_recall_location default room 142
ORACLE-VERIFY; permadeath banks fame×90% retention (option-0x32 default
unread, VERIFY) once per calendar day to the account, restore at
creation SKIPS the Lawful question when banked evil >= 1 and seeds fame
— negatives clamp 0, good standing never survives the account).**

Original progress banner (2026-07-19, 620 tests): crime.rs module
(get_legal_level exact chain, tier names/colors from 0x4881a4/0x488184,
charge_npc_evil with the §2.1 gate order/dark cloud/minimum-10 bump —
the §2.4 30000 add-guard is unreachable through this path since the 300
action ceiling refuses first); fame + warn_on_evil persisted (backfills
0/ON); passive-monster (mode 0/4, not-fighting-you) attack/targeted-
cast/area-cast writers with refusal-aborts; SET EVIL toggle (warn-ON
confirm = DLL string, OFF wording ORACLE-VERIFY); alignment lattice in
user_can_use AND spell gate 3 (Good/Evil/NotGood/NotEvil/Neutral;
NotNeutral dead as shipped); update_allowed_worn_items force-removal on
tier crossing (wording ORACLE-VERIFY); type-0x14 alignment exits (para1
= too-good bound, para2 = too-evil); creation Lawful = fame −51.
Fixture correction: attackable test monsters moved to behaviour 3
(lair) — created characters ship Warn on Evil ON and mode-0 punching
bags refuse every swing. LEFTOVERS (finish before slice 4): criminal
respawn split (fame >= 0x28 → the outlaw start, room defaults
ORACLE-VERIFY) and account-level evil banking/restore (GENBB analog:
bank on permadeath ×retention pct once/day, restore at creation —
server login plumbing). Pair timers/forgive/rob = slice 4 by design.

## Slice 3 — Crime, fame, legal levels

- **Fame persistence:** `player.fame` column (closes the state_db
  hardcode), riding the existing player snapshot persist.
- **`get_legal_level` port** with the eight pinned thresholds; lattice
  display strings per slice-1 pins.
- **Writers:** every slice-1-enumerated site belonging to shipped
  systems; `evil_for_robbing` lands with slice 4, `addevil` with
  slice 6 — each citing crime.md.
- **Gate unlock:** `user_can_use` alignment gates, spell alignment
  lattice. M6's raw fame compares get legal-level-boundary citations;
  behavior unchanged.

Tests: threshold table unit (all eight boundaries, both signs); gate
units per unlocked check; persistence round-trip. Oracle: SYSOP-staged
fame values → status display + guardian/hunter reaction flip.

**Slice 4 COMPLETE (2026-07-20, 656 tests) — full thief kit.** Final
additions past the 648 mark: DISARM (trap-state in the exit overlay,
DisarmTraps roll bands, trigger message records user-line-1/room-line-2,
damage genrdn(r/2,r+1) + drops/death, trapdoor forced relocation,
silent 300s re-arm); hidden type-6 exits (concealed from exits line +
movement until SEARCH's Perception-15 reveal, ~5 min re-hide to disk
state; 1470 shipped exits went dark with zero churn); the add_delay
command-delay (session units aged per fast tick; SNEAK/HIDE gate,
sneak/hide/search 1, picklock 2+2, rob 1 charges); the HIDE stash
(room hidden items/coins, bare-SEARCH reveal, get-by-name retrieval,
NotDroppable refusal). PENDING (tagged, slice 5/8): 0x18 spell traps +
the picklock lock-trap spell (room-cast plumbing + the room+0x5fa
column pin), mode-7 trapdoor spell arm, oracle passes on every VERIFY
string (delay unit length, closed-door wording, stash presentation).

**(Progress marker at 648 tests):** ALSO LANDED since the
633 mark — ROB <player> (thievery-roll outcomes bump/marginal/success,
quiet coin+item transfer with the LoyalItem/Robable gates, room never
told; the player-victim evil charge with victim-quality multipliers +
innocence gate; the 11-slow-tick evil-pair timer list keyed by name);
FORGIVE (exact refund via a clean node unlink — the DLL's predecessor-
free use-after-free is deliberately NOT cloned); PICKLOCK (Exit gained
param3/para4, loader re-indexed +20/+6-per-dir; runtime lock-state
overlay keyed (room,dir); movement blocks the 73 shipped locked doors;
reciprocal-exit unlock; 300s-per-unit re-lock timers via Job::ExitRelock
with the "just locked!" broadcast; positive-modifier 7/0xb locks never
re-lock; no crime consequence); SEARCH (trap detection via FindTraps
roll — informational, no state change — plus the room broadcasts and
the non-direction refusal). REMAINING in slice 4: DISARM (trap damage
genrdn(r/2,r+1)/death, trapdoor state-3 falls, spell-traps — needs the
trap-spell column room+0x5fa pinned) + hidden type-6 exit reveal (SEARCH
half — reworks how the 1470 shipped type-6 exits render/move) + the HIDE
item/coin stash + the add_delay command-delay system. Original 633 mark:

LANDED — monster rob
form (kind 3) stub port with the melee-slot-0 fallback (attack_monster_
user 26808-26813; closes the swing-loop marker); SNEAK/HIDE (§11:
stealth_chance helper exact, silent successes, perception-gated
self-doubt, perception-FILTERED sneak movement lines replacing the
normal broadcasts, hidden players leave also-here, non-sneak movement
clears the byte; runtime-only Player.hidden/sneak_armed); BACKSTAB
(cmd_backstab decompile-read: visible = silent plain attack,
hidden+unarmed or BSAccu-weapon = mode 4, non-BS weapon refuses then
attacks normally; mode-4 accuracy (Agl+Stealth)/2+Agl/2+BSAccu —
the +0x7d4 ±5/−15 flag mods untraced VERIFY; full-pool one-swing cost;
"surprise %s" verb wrap VERIFY; post-hit revert to normal; hidden
diverts wired into attack and punch per their decompiles). REMAINING —
evil-pair timers + FORGIVE + retaliation windows (crime.md §4-5, skip
the use-after-free), rob_user (§4), picklock/traps/SEARCH/DISARM
(§8-10) + the exit-state runtime model (lock/trap states, re-lock
timers), HIDE item/coin stash (room hidden storage), the add_delay
command-delay system (cross-cutting — sneak/hide/picklock/search all
charge it).

## Slice 4 — Theft (full kit)

- **Monster rob forms:** exact stub port — kind-3 swing calls a
  `monster_rob_user` that returns false, caller fall-through per
  decompiled.c:26808. Divergence-free no-op, closes the game.rs marker.
- **`rob <monster>`:** per theft.md — THIEVERY/STEALTH rolls, coin
  transfer, detection → aggro + `evil_for_robbing`.
- **`rob <player>`:** per theft.md — victim/room notification, detection,
  fame write, forgive interplay. (Robbery is item/coin interaction, not
  combat — no PvP combat core needed.)
- **`picklock`:** per theft.md — locked exit types, key bypass
  relationship, PICKLOCKS/FINDTRAPS rolls, trap consequences, failure
  states.

Tests: kind-3 golden (swing loop skips, next form fires); rob and
picklock success/failure/detection units citing theft.md; fame-write
integration. Oracle expedition: thief character (SYSOP-staged), rob a
monster + a second character, pick a Newhaven-reachable lock; capture
every string + fame delta via status.

**Slice 5 COMPLETE (2026-07-25, 764 tests, commits 66b3867..9fcd43b) —
charm & pets, plus one unplanned routing fix and one late blocker.**
Everything the slice scoped landed: `charmlvl`/`charmres` loader columns;
`attack_monster_monster` (form-0 fighter builds, mode-5 pipeline,
DamageShield, killer-less exp split) closing the M6 monster-vs-monster
marker; Enslave acquisition (the `charmres` save-stat swap with the
`charmres == 0` M.R. fallback, the silent `charmlvl` level gate, the §0
triple, and the slot-full case that yields a *permanent* timerless
charm because the caller ignores `add_cast_spell_to_monster`'s -1);
pet movement (follow-roll skipped, wander refused) and assist
(`FUN_0044cc65` — owner's monster target, self-release, idle-draw-free);
the pet targeting exemptions (the `0x800` find is an ORDERING, charmed
last, not a veto — corrected mid-slice; `is_valid_monster_target`'s area
gate is the only real veto); all four release paths (expiry reversal,
give-up/logout sweep with the roam-0x25 despawn variant, owner-melee
release with its grudge asymmetry, retaliation exemption); and the
Summon ownership links (`SummonLink::Pet`/`HuntUser`/`HuntMonster`, the
10-deep monster trail at `mon+0x38..0x5c`, and the `+0x88` hunt driver
arm whose cold-trail swing crosses room boundaries).

Deviations from the slice plan, all recorded in `re/docs/charm.md` §8:
`cast_user_target` 0xc IS implemented (it shares its body with
`cast_no_target` 0xc and needed no PvP, only the `target_id == session`
discriminator); the owner is keyed by `SessionId` rather than name, so a
re-login inside the ~16 s give-up window will not re-attach a pet; the
`attack_monster_monster` exp split pays only engaged sessions, not the
DLL's idle-autocombat bystanders; the victim-side `+0x60` back-links are
NOT ported (no reader exists, and the write loop has no `break`, so the
DLL puts the same id in every free slot — see charm.md §7); the hunt id
is a typed monotonic `MonsterInstanceId`, which closes charm.md §7's
stale-link hazard by construction.

**The late blocker: the Enslave eligibility scan was never ported**
(found by the whole-slice review after the first COMPLETE banner went
up; fixed in `f6b58ed`). `cast_monster_target`'s pre-application ability
scan (43295-43376) is a ten-row walk over the SPELL's abilities. The
slice ported exactly one arm of it — ability 6, the `charmres` save-stat
preload — and left three refusal siblings behind: AffectsAnimals(80),
which refuses a target without ability 78; AffectsUndead(23), which
tests the `undead` COLUMN (`knmsr+0xad`); and AffectsLiving(108), which
refuses a target carrying ability 109. All four shipped Enslave spells
carry exactly one of the three, so the omission was not academic:
`charm animal` was legal on all 1101 templates instead of 155 and
`control undead` on 1101 instead of 115, and in every illegal case the
player saw a SUCCESSFUL charm rather than the refusal line. The
`undead` column joined the loader as an `i16` — it is tri-valued in the
shipped data (0: 986, 1: 107, **-1**: 8) against a `!= 0` test, so a
`bool` would have silently dropped eight undead templates.

That the slice could ship without noticing is the interesting part: the
whole suite ran against hand-built fixtures whose spells carried no
gate rows, so nothing exercised the gates. The answer was
`crates/mud-server/tests/charm_real_content.rs` (`97585b4`, 10 tests) —
the first charm coverage driven by the SHIPPED content rather than
fixtures, running the entire Enslave chain against the real spells and
the real bestiary. It is the reason the remaining unported arms of that
scan (52/144/163, and Evil(98)'s absence) are now named rather than
merely absent (`f49d178`).

**Unplanned, and this doc did not know about it: the Task-3b routing
fix** (`493dff8` + `7a193d7`). Single-target casts were routed on
`spelltype`; the DLL routes on the MATCH type, and the 41434 self-target
divert sits above the acceptance gate. Correcting both sent 25 more
learnable benign spells (curse, blind, slow, hold person, the songs) at
monsters for the first time — which **widened an existing crime gap**:
those 25 carry `EvilInCombat(52)`, and `cast_monster_target` 43323-43347
charges 10 evil off the ABILITY with no `spelltype` test, then grudges
and un-suppresses the victim. We do neither. Logged against `crime.md`
§2.5 as `M7 PENDING` at `offensive_cast_attempt`'s fail arm; 16
learnable match-12 area carriers were already in the same hole.
`charge_passive_monster_evil` already implements the 43323 predicate
exactly — closing it is a call-site change (gate on the ability, not on
`is_offensive()`) plus the grudge/suppression writes.

**The close-out pass was about ANNOTATIONS, not code.** A 65-mutation
sweep over the slice killed 47 and left 18, and the survivors clustered
almost entirely on divergence notes, decompile cites and "this is the
DLL's shape" claims that nothing kept true — which, for a
reverse-engineering port, is the deliverable at least as much as the
code is. What that pass changed: charm.md §1.1 was stated flatly and
contradicted the code on the `charmres == 0` fallback; §3's unported
`+0x14` poison floor was described but never named or entered in the
§8.1 divergence ledger; the Dodge parry and the engage-lock relocation
were absent from the slice-8 carries entirely (both now head the list
below); a test comment claimed guardsman was "the `monstertype` of 487
rooms", conflating a template number with an unrelated spawn zone
(withdrawn, no replacement — see the comment for why a correct
derivation needs more than the naive query); and three code comments
claimed properties nothing can observe (the owner-release write ORDER,
which is an equivalent mutant because the slot sweep clears suppression
anyway; the m-v-m damage clamp, whose only reader is the kill test it
cannot change; and the Enslave arm's `match_ok` term, which is
unreachable defence in depth). Four mutation survivors were closed with
real tests instead: both wander arms' charm guard, the `charmlvl`
boundary, the signed-`charmlvl` compare, and the monster trail's
ten-entry bound. Each new pin was verified by applying its mutation and
watching it fail.

**Landed off the back of the parry expedition (2026-07-26, branch
`armour-columns`): the player armour column swap.** The expedition's second
anomaly — worn AC soaking far less than the port modelled — was filed in
charm.md §8.3 as an open "our soak may be 10x too strong". It was not a
scaling error. `move_player_to_fighter` (24788-24789) accumulates `+0x342`
(DB `ac`) into fighter `[1]` ÷10, the TO-HIT term, and `+0x39c` (DB `dr`)
into `[3]` raw, the damage soak; `build_player_defender` had them crossed
since M4. Both columns ship ×10 and are independent — **319 of the 565**
AC-bearing shipped items carry no DR at all — so the bug invented resistance
from the AC of essentially every piece of armour in the game, and denied
every geared character the evasion they should have had.

The transcripts settle it without a new capture, because the status line
prints both numbers: the acc-high set (Σac 125, Σdr 8) read
`Armour Class:  12/0`, and Σdr/10 = 0 is exactly the soak the giant bats
demonstrated. Two claims in §8.3 were corrected on the way through — no
transcript ever showed `Armour Class: 13`, and the smoky black talisman's
-20 is an AC(2) ability, not accuracy, so the acc-mid block varied AC too.
Also landed: `get_armour_rating` (the `st` line had shown a hardcoded `0/0`
since M4) and the AC(2)/DR(7) dynamic accumulators, which were applied for
monsters and nobody else. Left explicitly unported and cited at the call
site: the `×(+0x7b8+100)/100` DR percent, whose writing ability the
decompile does not readably identify.

The general lesson is slice 5's, again: the whole suite ran fixtures that
fought naked, so 950 tests could not see it. `crates/mud-server/tests/
armour_real_content.rs` is the answer, and reproduces the measured `12/0`
from the shipped columns.

**Carries to slice 8**, largest first:

1. **CLOSED (2026-07-30, charm.md §8.5) — the parry step function is
   measured at d=2/3/4/5 and stands as written.** The monster
   Dodge(0x22) parry on the player-attacks-monster path —
   the biggest live gameplay change in the slice, and the one most easily
   missed because it arrived as a side effect of Task 2. Adding the parry
   word to the shared `build_monster_defender` also armed it for the
   PLAYER's swings, because the DLL runs that same
   `move_monster_to_fighter` build for both. **167 of 1101** templates
   carry Dodge at 10..200; `parry*10/(accuracy/8)` (cap 95) converts
   roughly **28-80%** of connecting player swings into zero-damage
   parries. The formula came out of the 16-bit disassembly and has never
   been checked against a capture. **Capture a grind against a
   Dodge-carrying template first** — everything else on this list is
   smaller. (charm.md §8.2; ORACLE-VERIFY at `build_monster_defender`.)
2. **The engage retaliation lock's relocation** (`12e6178`) from the
   combat round to the ATTACK command — a change to all player melee that
   moved a `genrdn` draw earlier in the stream and moved a golden in
   `spell_scenario.rs`. Decompile-justified (26230 is in the other arm of
   the 26112 split), unmeasured. Confirm the timing and the draw order.
3. The live charm oracle expedition (charm a low monster with charm
   animal / song of charming, walk it, watch one assist round, attack it
   as the owner, let a second charm expire — pin every string and retag
   text.rs ORACLE → MEASURED).
4. The `EvilInCombat(52)` charge above.
4b. **The to-hit model**, which the parry expedition put in question
   (31/37 = 0.838 connecting against a predicted 0.67) and which the armour
   fix above did NOT settle — it has a different cause. It does now have a
   fair test for the first time: until the column swap was fixed, every
   geared player's evasion word was 0, so any earlier comparison ran against
   a defence the port was not applying.
   **The monster half is CLOSED (2026-07-26, charm.md §8.4):** template `ac`
   is whole units and `build_monster_defender` is right as written. Grey
   spiders (AC 20, no Dodge, so no parry channel) connected 2/21 = 0.0952
   against the port's 0.10 and a tenths reading's excluded 0.99 — which also
   put the first measurement on the `[10, 99]` clamp's FLOOR, a region no
   test had ever entered because every combat fixture fights an AC 0
   sandbag. Pinned, mutation-verified, in `game_combat.rs`.
   **The accuracy half CLOSED (2026-07-30, charm.md §8.5): delta = 0 —
   the derivation is right as written, and the 2026-07-26 acc-mid block
   is retired as contaminated (its 0.838 is the accuracy-29 rate
   exactly).** Historical framing below kept for the trail: The slice-8 fidelity fixes (clamp fall-through,
   strict comparison, genrdn's EXCLUSIVE upper bound) landed and the raws
   were re-fit under the corrected `P(connect) = (threshold-1)/99`. The
   acc-mid conflict survives on the same 37 swings: to-hit excludes
   accuracy 23 (wants ≥24), the parry channel excludes ≥24 (wants ≤23) —
   so at least one formula SHAPE is off, not (only) the accuracy constant.
   Candidate: the bat's effective defense word ≤ 8 rather than the raw
   `ac` 10. Next measurement: the two-front expedition — a parry-cliff
   pair (accuracy 23/21 vs the bat, cliffs at floor(acc/8)) that moves
   with accuracy alone, plus a kobold AC-30 pair at two accuracies whose
   connect RATIO cancels any constant defense offset. Budget more
   survivability than the control run had — it died at 21 swings,
   because a 10%-connect grind against a target you cannot kill is a long
   time under return fire.
4c. **The `Encumbrance: x/2880` denominator — CLOSED (2026-07-28, charm.md
   §8.3 tail).** The code was already right: `Core::carry_capacity` applies
   Encum(96)'s `(100+encum)/100` over `stats.rs`' `str*48` — 2400 × 1.2 =
   2880, pinned by the `Encumbrance: 0/2880` goldens in `tests/inventory.rs`.
   The doc trail, not the port, was behind. Remaining unmodeled scrap: the
   DLL's key-array weight (+0x334[50], decompile 67745-67775), inert unless
   keys are carried. (Still does NOT explain 4b's residue, since at the
   captured weight both denominators floor to the same `enc/10`.)
5. **The pet lifetime that `give_up` never resets.** `give_up` is zeroed
   only in `monster_attack`'s engage block (`game.rs:9773`, decompile
   26768-26777) — a path a pet essentially never takes, since a pet
   swings through `attack_monster_monster` (`pet_assist`) instead. (The
   one exception is the departure free-attack, which can route a pet into
   `monster_attack` against a NON-owner who leaves the room; that does
   reset the counter, but it is incidental, not the pet's own combat.)
   So in ordinary play a pet's give-up counter only ever
   climbs, and any 16 fast ticks that each fail to prosecute the follow
   (different map, no trail, a refused `move_monster`) release it —
   cumulatively, across its whole life, not consecutively. A pet can
   therefore be released after a handful of rooms travelled. This is
   DLL-shaped and pre-existing from M6, but slice 5 is what made it
   load-bearing: before charm, nothing cared how long a lock survived.
   Measure a real pet's travel range before deciding it is faithful.
6. **The unported ability-`0x39` (SeeHidden) chase escape** in that same
   pursuit predicate: the DLL bumps `give_up` when the quarry is hidden
   and the pursuer lacks `0x39`, which is how a thief breaks a chase. We
   do not implement it, so hiding does not shake a pet or a grudge
   monster. Lands with whichever slice wires hidden-target visibility
   into pursuit.
7. Three ORACLE-VERIFY items in charm.md §8.2 — instant-Enslave
   messaging (fixture-only, no shipped Enslave has duration 0),
   `is_valid_monster_target`'s roam/fame/behaviour-4 fall-through
   (exhaustive from the decompile, zero measured surface), and the
   match-10/0xd pet-command band (decompiled and implemented but
   unreachable, since those match types iterate players only and collect
   nothing).

## Slice 5 — Charm & pets

- **Monster-vs-monster combat** (prereq): the swing path per slice-1
  decompile; closes the M6 marker.
- **Enslave at monster:** charmlvl/charmres gate, `+0x128` bit 1, owner
  link (+0x140), attack suppression (+0x116); the instant branch;
  `summon_spawn` owner/victim tags.
- **Pet behavior:** always-pursue (skip follow roll), pet-assist in the
  owner's fights, charmed bit-0 lock exemption.
- **Expiry/termination:** upkeep reversal (name/owner reset, clear
  flags) replacing the current no-op arm.
- **Stubs stay stubs:** player-target Enslave, `tame`, `mesmerize`. Pets
  are ephemeral (instances don't persist — matches a board restart).

Tests: gate units (charmlvl vs level, charmres roll); assist/pursue
goldens; expiry reversal golden; suppression unit. Oracle expedition:
charm a low monster, walk it, watch it assist, let it expire — pin every
string.

## Slice 6 — Quest text-block VM (the core)

- **Interpreters** (quests.md §1.2): unconditional block runner;
  `perform_special_command` (wildcard-matched vs raw input, fired by
  TextBlock ability 148 on items/rooms/monsters — wired into command
  flow BEFORE the SAY fallback per the DLL dispatch point);
  `ask_monster_a_question` (keyword dialogue, new `ask` verb —
  `cmd_ask` 0x458306).
- **`perform_matched_action` verb set** (§2): mutations (addability,
  giveability, removeability, addexp, giveitem, takeitem-with-rollback,
  givecoins, addevil, learnspell, cast, roomitem, hideitem, clearitem,
  roomtext, text, message, summon, teleport, adddelay, random,
  remoteaction, price) and gates (checkability, testability,
  failability, checkitem, failitem, failroomitem, class, race, minlevel,
  maxlevel, goodaligned, evilaligned, checkspell, checkskill, testskill,
  needmonster, nomonsters, monsters, test_tournament); control codes
  1=continue / 2=fail-stop. `addevil` → slice-3 fame; `summon` →
  slice-5 tags; `addexp` → `add_quest_exp` uncapped/unsplit with the
  restructured-exp gate (§4.1).
- **Player ability table:** 30-slot `(id, value)` innate table
  (+0x73a/+0x776), folded into ability lookups; persisted as a generic
  `player_ability` state.sqlite table (player_effect pattern).
- **Completion detector** (0x414d23) at load + the login class/level
  re-strip (§4.3-4.4), reward args from the slice-1 disasm (Smash 0x20 /
  PerfectStealth 0xba / Meditate 0xbb thresholds).

Tests: per-verb units with fixture blocks (every gate's
pass/fail/control code); takeitem rollback golden; a synthetic
multi-step quest golden (trigger → gate → counter++ → threshold → skill
grant → re-strip on declassing); ability-table persistence round-trip.
Oracle expedition: one real Newhaven starter quest end-to-end on the
live board, diffing every emitted line + ask-dialogue strings.

## Slice 7 — Gangs & guild houses

- **State:** `gang` table in state.sqlite (schema from the confirmed
  WCCGANG2 layout) + `player.gang`/rank-bit columns (+0x6c8, +0x7d4/5);
  `Event::PersistGang`. Invitations ephemeral on `Core`; deferred
  login-notification bits persisted.
- **Membership** (gangs.md §1): create (exp ≥ 100000), invite/uninvite,
  join, leave, remove, promote/demote (lieutenant bit 0x100), disband;
  rosters; broadgang/tell_gang over the sessions map filtered on gang.
- **Economy** (§2): per-kill exp-pool feed (saturation → secondary pool
  + wrap counter, flag bit 0x8); gold account = bank-8 bankbook keyed
  per the slice-1 writer finding; top gangs by exp-descending
  (documented divergence unless the Btrieve key read settles order).
- **Guild houses** (§3-§4): room flag +0x564 & 0x40, deed purchase
  (GHouseDeed 181, price `DAT_00482d10`×10000), gang vendor shop type
  0xb leader-gated, GHouseTax 182 / GHouseItem 183; **.HSE streaming**:
  room `Desc[0] == "FILE_DESCRIPTION"` → stream
  `re/hse_files/<Desc[1]>`, error string
  `Can't display gang house file %s`. File I/O at the server edge — the
  core stays I/O-free (server preloads or resolves per look).

Tests: membership state-machine units (every transition + error
strings); exp-pool saturation golden; deed/shop gating; .HSE resolution
(fixture + missing-file error); gang persistence round-trip incl.
restart. Oracle expedition: two characters — create/invite/join/roster/
top/broadgang string pins; stock guild-house .HSE render; deed price.

## Slice 8 — Close-out

Consolidated oracle re-verification of all M7 strings → retag text.rs;
marker sweep to zero `M7 PENDING` (gang-war → `M8 PENDING` citing this
doc). Known survivors as of slice 5: the two `EvilInCombat(52)` markers
in `game.rs` (`cast_monster_target` 43323-43347's ability-gated 10-point
charge + grudge, widened by slice 5's routing fix — see the slice-5
banner) and `monster_could_attack`'s unported pet exemption, which is
not slice-scoped and lands with its first consumer.
crime.md/theft.md/quests.md/gangs.md updated with as-built cites;
roadmap status line. Hand session on the real DB: toggle ANSI, kick a
rat, watch "A nasty orc rogue" walk in, charm a pet and let it fight,
rob a monster, pick a lock, go Seedy and watch the guardian flip, ask an
NPC a quest question and complete a step, found a gang with a second
session and read a guild-house .HSE — with a restart in the middle
(gang, fame, quest progress all survive).

## Risks / unknowns

| Risk | Resolution |
|---|---|
| WCCTEXT2 continuation-record assembly wrong (multi-record blocks) | Slice-1 validation: known `greettxt` dialogue vs oracle `ask` captures; the orc-rogue `B:` line must reproduce the M6 capture |
| Quest reward push-args unreadable in decompile | DumpAsm disasm pass (slice 1); fallback = infer from load_player validation set, ORACLE-VERIFY tag |
| Monster-vs-monster damage formula unspecified | Slice-1 decompile of the pet-assist swing path — deterministic code, decompile-authoritative |
| `rob_user`/`picklock` spec surface larger than estimated (2037 + 1907 lines) | Slice-1 pass is spec-only; if either exposes systems out of scope (e.g. trap damage types unbuilt), the affected verb sub-part gets its own slice-4 gate with an explicit defer note |
| MageBane/Phoenix/DaoLord counters engine-unconsumed (quests.md §6) | Reproduce exactly: counters advance, no engine effect — data-side quests |
| Gang Btrieve key-1 order assumption (top gangs) | Exp-descending + documented divergence unless the slice-1 file-header read settles it |
| `DAT_00482d10` deed price / `DAT_00480efa` separator unresolved | DumpBytes .data dump, slice 1 — M6 precedent |
| ANSI toggle has no DLL surface | Server-edge toggle, documented divergence; rendering stays byte-exact both ways |
| Name-gen goldens fragile (extra spawner-stream draws) | Narrow one-spawn goldens, draws in decompile order; re-pin the two bare-name tests in slice 2 before new goldens stack |
| `+0x542` writer set incomplete | Slice-1 exhaustive store-site grep; unshipped writers (PvP kills) get `M8 PENDING` citations, not silence |

## Testing & completion bar

- **Tier 1 — spec-cited units** (TDD, red first): every verb, gate,
  roll, and threshold cites quests.md/gangs.md/crime.md/theft.md/
  combat.md section + offset.
- **Tier 2 — seeded goldens:** per-slice transcripts pinning message and
  RNG draw order (name roll, rob rolls, charm resist, VM `random`);
  regenerated per slice, never across slices.
- **Tier 3 — oracle expeditions:** six live programs (MA strings,
  fame/status display, theft incl. picklock, charm lifecycle, one real
  quest end-to-end, two-character gang program + .HSE render);
  randomized internals carry decompile citations instead.
- **Done:** eight slices on `m7-content`, `cargo test` green (586 +
  new), clippy clean, zero `M7` markers in the tree, oracle scripts pass
  or carry whitelist entries, state.sqlite migrations proven against a
  pre-M7 file, and the slice-8 hand session completes on the real DB
  with a mid-session restart.
