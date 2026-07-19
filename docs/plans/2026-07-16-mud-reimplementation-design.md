# MajorMUD Reimplementation — Design

## Context

The clean-room documentation phase in `re/` is complete: 17 behavioral spec docs
(`re/docs/`), the full extracted content database (`re/mmud_wgnt.sqlite`, 9 tables /
~35k rows, lossless, regenerable via `re/import_mmud.py`), the 188-ability enum
(`re/docs/ability_ids.tsv`), and gang-house files (`re/hse_files/`). This document
records the design for the from-scratch reimplementation.

## Decisions

| Decision | Choice |
|---|---|
| Deployment | Standalone telnet server |
| Language | Rust |
| Fidelity | Behaviorally exact; WG3-NT tuning wherever the spec flags a 16-bit split |
| Storage | Content: `mmud_wgnt.sqlite` read-only. Player/world state: separate `state.sqlite` |
| Verification | MBBSEmu + WCCMMUD 1.11p DOS as a live differential oracle from the start |

## Architecture

**Async edges, synchronous core.**

- tokio owns the telnet listener; one task per connection handles negotiation,
  line buffering, ANSI emission, word-wrap. No game logic at the edge.
- Sessions ↔ core via channels (input events in, output text out).
- The game core is a single thread owning all mutable state (`World`). No locks in
  game logic. The core is deterministic given (state, input sequence, RNG seed) —
  the property the test tiers below depend on.

**Crates:** `mud-core` (engine library, zero I/O deps), `mud-server` (telnet binary),
`mud-oracle` (differential-test harness).

### Engine internals

These mirror the spec's load-bearing patterns:

- **Tick scheduler** — `BinaryHeap` of self-rescheduling jobs keyed by absolute tick,
  replicating the 1 s metronome → 5 s combat / 3 s upkeep / 30 s regen-wander tiers
  (`re/docs/combat_rounds.md`). Ticks can run faster than wall-clock in tests.
- **Ability dispatch** — `enum Ability` generated from `re/docs/ability_ids.tsv`;
  every effect (spells, items, race/class, monsters) is an `(Ability, i16)` pair
  applied through one `apply_ability` fn. Exhaustive match means the compiler
  tracks unimplemented abilities.
- **Derived stats** — `update_dynamic_stats` recomputes all combat fields from
  scratch (permanent abilities + active spell slots + worn gear), as the original
  did — no incremental buff bookkeeping, no undo on expiry (`re/docs/leveling.md` §5).
- **Command layer** — verb table with the original's abbreviation/precedence rules
  (player-visible behavior — spec'd and tested).

### Data layer

- **Boot:** load all 9 content tables into typed structs (`RoomId`, `MonsterId`, ...);
  validate everything (exits resolve, ability ids known, message refs exist); fail
  boot loudly on any violation. The message table (3,905 rows) drives all game
  text — message-exact output is part of the fidelity bar. `.HSE` files load from
  a directory, as the original did.
- **`state.sqlite`:** players, gangs, banks, shop stock, quest state, unique-monster
  respawn stamps. Single writer (the core thread). Transactional writes on
  logout/level/death/trade plus a periodic sweep, matching original persistence
  points.
- **Ephemeral state** (spawned monsters, floor items, spell durations, engagement)
  is memory-only, evaporating on restart like the original's cleanup cycle.
- **RNG:** one seedable PRNG owned by the core. Exactness means formulas and
  distributions match — not the roll stream.

### Contained deviations

Standalone means no Worldgroup host, so two things must be invented:

- an `auth` module (name/password → the spec'd character-creation flow), and
- a `billing` stub (credits default unlimited; accounting hooks kept real).

Both are isolated modules. Everything after "you are logged in" is spec territory.

## Testing (three tiers)

1. **Formula unit tests** — TDD, mandatory: every spec formula written as a failing
   test from the doc before implementation.
2. **Scenario tests** — scripted sessions driving `mud-core` directly, seeded RNG,
   golden transcripts (creation, combat, shopping, death, leveling).
3. **Differential oracle** — `mud-oracle` feeds identical command scripts to MBBSEmu
   (real WCCMMUD) and our server, then diffs normalized transcripts. DOS-vs-WG3NT
   tuning differences are whitelisted with a doc citation; the spec wins.

Tiers 1–2 run per commit; the oracle runs on demand per system.

## Milestones

- **M0 Foundation** — cargo workspace, `Ability` codegen, content loader + full
  boot validation. *Deliverable: binary loads and validates the whole world.*
- **M1 Walkable world** — telnet server, auth, character creation, rooms/movement/
  look, message rendering, command parser; MBBSEmu stood up with first oracle
  scripts. *Deliverable: log in, roll a character, walk all 26,720 rooms.*
- **M2 The body** — tick scheduler, `update_dynamic_stats`, regen/hunger/thirst,
  leveling/training.
- **M3 Combat** — single-attack resolution → rounds/engagement → death/exp split
  (fixture-placed monsters until M6).
- **M4 Stuff** — inventory, equip/worn slots, item charges, shops, banking.
- **M5 Magic** — casting, effect application, duration/upkeep, monster casting.
- **M6 Living world** — density-driven spawning, wander/leash, aggression,
  unique-spawn timers. **COMPLETE 2026-07-18** (design + six slices:
  `2026-07-18-m6-living-world-design.md`; charm/pets re-deferred to M7).
- **M7 Content systems** — quest text-block VM, gangs + `.HSE` houses.
- **M8 Plus (optional)** — WCCMMPLS add-on.

Open spec items (unextracted `.data` constants) are resolved via Ghidra or oracle
measurement when their milestone arrives, not up front.

## Verification bar per milestone

- `cargo test` green, clippy clean, every formula traceable to a spec doc.
- M1 onward: oracle scripts pass, or carry a documented tuning whitelist.
- End-to-end: telnet in and exercise the new system by hand.
