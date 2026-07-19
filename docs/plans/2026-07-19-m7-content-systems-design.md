# M7 Content Systems — Design

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
doc); crime.md/theft.md/quests.md/gangs.md updated with as-built cites;
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
