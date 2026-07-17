# M5 Slice 5: Breadth — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Casts at other players, area casts, kai/mystic wording, poison +
the healer cure path, targeted dispels, Summon, the msgstyle-odd message
table. After this slice only monster casting (slice 6) remains in M5.

**Architecture:** Extends the existing cast pipeline (game.rs) — no new
subsystems. Spec: re/docs/spellcasting.md §3/§4 (area + targeting), §8
measurements. Two oracle expeditions front-load the unmeasured strings.

**Prior art:** slot entry/termination (slice 4), render_cast_line +
fill_message, monster targeting (word_prefix_match), healer services (M4),
save-class machinery, exp-patch staging recipe (§8.8), tmux'd MBBSEmu.

---

## Facts pinned during planning (do not re-derive)

- **Kai powers (magery group 5): 18 spells, ZERO taught by any LearnSp
  item.** Mystics must acquire them another way — likely trainer-granted at
  level (the §8.1 "training grants nothing" finding was measured on a MAGE).
  Expedition A settles it. Lowest: way of the swan L2, owl L3, cat L4.
- **Learnable area spells:** mage flash L7 (match 12, ms even), stinking
  cloud L8 (match 12, duration 20, ms even); priest chant L6 (match 13);
  bard songs L2-3 (match 12/13 — bard class support NOT in scope).
  Match types 11/12/13 do NOT split magnitude (only 3/5/9/10 do — spec §3);
  11/12 hit monsters too, 13 is players-only. No learnable 3/5/9/10 spell
  exists in shipped data — the magnitude SPLIT is fixture-tested only.
- **Blur is match-type 2** — single explicit-target benign; `c blur oracle`
  is real behavior we currently refuse ("You do not see..."). Player-target
  benign casts are the slice's centerpiece. Minor healing (match 2,
  instant), bless/alertness (match 2, duration) are priest/druid analogues.
- **Poison(19) carriers are all monster-attack payload spells**
  (spits/bites/mermex). Slice 5 implements the mechanics (hard-write,
  counter, tick damage, CurePoison, healer path); live delivery is slice 6.
  Cure-side learnables exist NOW: antidote L6, cure poison L7 (instant,
  RemovesSpell-carrying — the slice-4 pre-pass already dispels; the
  POISON-counter side is what's new).
- **msgstyle-odd binding** (from the Task-10 review analysis, msg 8524
  family): caster line binds (target, damage), target line (damage), room
  line (target, damage) — NO spell-name slot. Confirm against the decompile
  msgstyle branch before implementing; lowest learnable odd DURATION spell
  is L19, odd instant spells unreachable until slice 6 — implement from
  decompile + tag ORACLE-VERIFY.
- **Item-target spells (match 6/7):** check data during Task 6; zero
  learnable ones were found in earlier sweeps — likely markers only.
- Healer cure stub: game.rs:1801-ish "Not-poisoned path only until M5
  brings poison: 15 silver" — the poisoned path is 25 GOLD (doc-corrected:
  constants ride the silver arg; M4 memory).
- Oracle staging: Zinvar Duskmere (acct Vexil/test123, Mage L2, Newhaven
  Spell Shop) — exp patch offsets §8.8 (0x3c + 0x46f, emulator stopped,
  .bak kept); Oracle (Dwarf Warrior, 2 lives, Newhaven Healer) as the
  second body for player-target/observer captures. MBBSEmu in tmux
  `mbbsemu`, port 2327. The user's char "poop" — do not touch, do not
  interfere if online.

## Task 1: Oracle expedition A — mystic

New BBS account (e.g. Kaimon/test123), roll a Human Mystic. Measure:
1. Power acquisition: `spells` at L1; train to L2-3 (exp patch); `spells`
   after each — do kai powers auto-grant at level? Any guild interaction?
2. Wording: bare `cast`, unknown power, insufficient-mana ("kai"?),
   one-per-round string, success lines for way of the swan (self buff),
   the `spells` listing header for powers, st behavior.
3. The per-round invoke flag (spec: mystics consume `+0x700 & 4`) — cast
   then attack same round? melee + invoke interplay if observable.
Deliver: transcripts, §8.12 in spellcasting.md, commit.

## Task 2: Oracle expedition B — targets and areas

Zinvar (patch to L8, stopped-emulator recipe; learn flash + stinking cloud
at the Silvermere/level-appropriate guild — find scroll shops; budget
ferry travel) + Oracle as second body:
1. `c blur oracle` — caster/target/room lines (all three views: drive both
   sessions interleaved per README), slot on the TARGET (Oracle's st),
   refresh-on-target, save-class vs players (blur ships save 0 — note).
2. Heal-type at a target if a priest scroll is learnable cross-class —
   else skip (mage has none; don't force it).
3. Area casts: flash (instant area) and stinking cloud (duration area) in
   a room with Oracle + a monster — who gets hit, line fan-out, whether
   Oracle (a player) is affected (PvP flag semantics?), per-target lines.
4. `cast blur zzz` unmatched-target string re-check with a player present.
Deliver: transcripts, §8.13, commit.

## Task 3: Kai wording + acquisition

Implement expedition A's findings: message variants keyed on
class.caster_group == 5 throughout the cast pipeline (the slice-4 marker
at game.rs ~3133), power acquisition per measurement (if trainer-granted:
grant on train per the class/level table — check how class spells map),
`spells` header variant, invoke flag semantics per measurement.

## Task 4: Player-target benign casts

`cast <spell> <player>` for match 1/2 benign: resolve players in room by
name prefix (existing player-listing helpers); slot entry/refresh on the
TARGET (Event::Persist for the target); instant benign (heal family) on
the target; save-class check vs player targets (spec §3 — the machinery
exists for monsters; player MR analog per decompile 41712-41733, already
spec'd); strings per expedition B; self-name targeting = self-cast.

## Task 5: Area casts

Match 11/12/13 (+3/5/9/10 machinery, fixture-tested): iterate valid
players (12/13) and monsters (11/12 per spec groupings — re-verify the
sets against §4 before coding); magnitude split ONLY for 3/5/9/10;
per-target resistance + save; duration areas enter each target's slots
individually; offensive areas engage combat (evil warnings = crime system,
SLICE 7 marker); line fan-out per expedition B.

## Task 6: Poison + healer cure + leftovers

- `Player.poison: u16` counter (+0xbe analog), Poison(19) hard-write at
  application + reversal at termination (slice-4 arm), poison tick damage
  in the upkeep/slow tick (find the DLL's poison tick site — regen docs),
  CurePoison(20) instant + recurring handlers (slice-4 markers).
- Healer: poisoned path = 25 gold "curing" service replacing the
  15-silver-only stub; strings per M4 healer capture.
- msgstyle-odd arg table per the pinned binding (decompile-confirm, then
  implement in render_cast_line as a second order table keyed on
  `msg_style & 1`; remove the temporary refusal; ORACLE-VERIFY tags).
- Item-target (6/7) + Summon(12): data check; implement Summon via the
  existing spawn machinery (owned tag = SLICE 6 aggression marker) if any
  learnable spell carries it, else markers.

## Task 7: Gates + checkpoint

Golden scenario additions (player-target blur between two sessions; area
fixture), full suite + clippy, live gate (two telnet sessions: blur a
second character, watch both views), marker sweep, design-doc + memory
updates, final whole-slice review, CHECKPOINT with Daniel before slice 6.
