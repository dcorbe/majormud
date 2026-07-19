# M6 Living World — Design

Milestone 6 of the MUD reimplementation
(`2026-07-16-mud-reimplementation-design.md`): density-driven spawning,
wander/leash, aggression, unique-spawn timers. Behavioral authority is
`re/docs/monsters.md` (WG3-NT decompile — §1 spawner, §2 generate_monster,
§3 move_monster/leash, §4 AI/aggression, §0/§5 tick tiers); cross-references
`death.md` §4 (respawn stamping), `combat_rounds.md` (tick metronome),
`vir_schemas.md` (disk maps). Spec citations below are to monsters.md unless
noted.

## Scope decisions

- **Charm/pets re-defer to M7.** M5 markers said "ships with M6 pets/charm",
  but the roadmap's M6 bullet is spawning/wander/aggression/timers only, and
  charm needs its own command surface (order/follow/dismiss), unpinned
  charm-flag semantics, and a separate oracle program. Every charm-family
  marker (Enslave's charm half, forced-cast mode, pet ownership) is retagged
  `M7 PENDING` in slice 6. The `charmlvl`/`charmres` columns load in slice 1
  but stay inert.
- **Summon follow-tags land in M6** — the directed-travel machinery is built
  in slice 3 anyway, so tagging a summon's victim/caster is nearly free.
- **The `--spawn` fixture flag survives** as the test/dev placement path; the
  density spawner is additive.
- **Interval constants are Ghidra-first, oracle-confirmed.** Spawning is
  player-driven, so live cadence measurement is confounded by presence;
  `.data` initializers are exact and cheap to dump. Passive line-timestamp
  observation (no commands sent — flood control throttles sends, not
  receives) confirms.

## Approach

Vertical slices in dependency order; each slice ends green, hand-testable,
and committed. Extraction goes up-front (slice 1) because every M6 mechanic
reads currently-unverified fields, and the page+6 `_raw` verification setup
is done once. Wander and aggression precede the spawner because fixture
spawns make them testable now, and spawner goldens inevitably involve
spawn-then-acquire (the oracle's "spawn-in-room attacks the same second"
quirk) — building the spawner first would mean re-goldening it when
aggression lands. Unique timers last: a spawner refinement plus the
milestone's only persistence work.

**Slice 1 COMPLETE (2026-07-18):** all offsets pinned and disk-verified (monsters.md
§2 copy map, vir_schemas.md spawn blocks) — behaviour←`alignment`@0xae,
aggression←`follow`@0x6e, roam/zone←`group`@0x54 (dual-use mongen region),
herd mode←`type`@0xaa, herd id←`something3`@0x6c, follower cap←`nothing2`@0xac,
forced monster = u4@0x468 (`bynumber>>16` — Nightmare misframe). Constants
extracted: spawn kick **5 s**, default respawn **5 min** (`delay` override,
minutes), respawn clock = minutes-since-midnight with +1440 midnight wrap
(`DAT_0047963a` = boot minute, not a threshold), fairness cap 3/medium-tick,
spawn-disable = crash-recovery flag only. Discoveries: lair/permanent monsters
(shopkeepers — `permnpc`) are generated at **module boot**, not player approach
(slice 4 must do the same); spawn-type-2 rooms bypass the respawn timer; monster
HP copied from the word@0x78 (doc corrected); `expmulti`@0x58 doubles as pack
herd rank. Room/Monster structs + loader extended with pin tests
(Slimy Sewer Tunnel 9/42, Gurbultis 27) and distribution tripwires; boss/forced
ids validate at boot (all 485+29 resolve).

## Slice 1 — Schemas & constants (zero behavior change)

- **Offset pinning:** from the decompile, pin which WCCKNMSR disk offsets
  feed `mon+0x106` (behaviour mode), `+0x108` (aggression), `+0x148` (herd
  mode) — only `+0x12c` ← `knmsr[0x15]` is pinned today (§7) — and which
  WCCMP001 offsets the spawner reads (`+0x560` zone, `+0x55c` cap,
  `+0x462/4` min/max level, `+0x468` forced monster, `+0x5bc` respawn
  override, `+0x5c0/4` linked rooms, `+0x5c8` boss).
- **Column↔offset confirmation** against `re/mmud_wgnt.sqlite` `_raw` at the
  page+6 frame. Candidates (Nightmare names are hypotheses — 24 B version
  drift — never authorities): room `type`↔`+0x43c` (reconcile the shop-active
  double-use in content.rs), `minindex`/`maxindex`, `bynumber`,
  `monstertype`, `maxregen`, `delay`, `controlroom`, `permnpc`; monster
  `group`, `index`, `follow`, `type`, `gamelimit`/`active`/`datekilled`/
  `timekilled`, `regentime`. Results recorded in `vir_schemas.md`.
- **`.data` dump:** spawn cadence `DAT_00482ca8`, respawn constants
  (`FUN_0046c3b8`, `DAT_0047963a`, the `0x5a0` nudge, `DAT_00482d0c`),
  wander fairness cap — close the monsters.md §7 open items.
- **Loader:** `content.rs` `Room` gains spawn_zone, spawn_cap, min/max
  level, forced_monster, respawn_override, linked_room, boss_monster;
  `Monster` gains behaviour, aggression, roam_class, herd_group, level,
  game_limit, regen_time (+ inert charm fields). Loaded in `content_db.rs`.
- **Validation (permanent, the schema tripwire):** aggression 0..=100;
  every nonzero room zone has ≥1 template in zone and level band (allowlist
  shipped exceptions, saracen precedent); roam special classes
  0/2/5/0x25/0x26 at plausible frequencies; boss ids resolve.

Deliverable: full-DB boot test proves the world data coherent; every later
slice reads typed fields with disk-verified provenance comments.

**Slice 2 COMPLETE (2026-07-18):** move_monster's full gate ladder + the
medium-tick wander decision live (15 wander tests + fear-flee), monster slow
tick gains HP regen + the last-move reset. Discoveries, all spec-corrected:
the walk-arrival broadcast is "walks into the room from ..." ("just arrived
from" appears in ZERO captures — it never was the walk line; divergent since
M1, fixed); monster wander arrivals use a THIRD string, "moves into the room
from the ..." (bare name); the anti-backtrack memory blocks two consecutive
SAME-direction steps (straight lines), not returning — ping-pong is legal;
herd semantics pinned (mode-2 held by any mode-1, mode-1 held by higher
rank, mode-1 drags mode-2 + lower ranks, rank = expmulti); water-path cap
quirks; confusion fumble line + ConfuseMsg override. M5's monster Fear-flee
marker closed. Exit model gains `param` (para1) + `door_closed` (para2).

## Slice 2 — Wander & leash (3 s medium tick)

- **Instance fields:** `MonsterInstance` gains aggression, behaviour,
  roam_class, herd, last_move_dir, location trail — copied from the template
  in `spawn_monster`.
- **`move_monster` port (§3):** gate ladder — immobility/prone; herd modes
  (`3` lair-stationary, `1`/`2` pack hold/drag with follower recursion capped
  by template count); zone leash (`dest.spawn_zone == mon.roam_class` or
  free-roam classes `5`/`0x25`/`0x26`); door/exit-type gating (blocked
  1,3,4,6,8,0xc; closed doors need `0x26`; secrets need `5`/`0x26`; damage
  exits 9/0x18 deal `genrdn(dmg/2, dmg+1)`); both-room messages.
- **Wander decision** folds into the existing 3 s `Job::Upkeep` after
  monster spell upkeep and env-death, mirroring `medium_update_monster`'s
  in-function order: roam-class switch (0/2 stationary), roll
  `genrdn(0,100) < (100-aggression)/2`, fairness cap 3/tick, direction via
  the existing `pick_valid_random_direction`, reject `last_move_dir`.
- **Oracle:** leave/arrive line templates (check existing `re/oracle/`
  captures first; the `movemsg` column exists) + confusion fumble line.

Tests: unit per gate; seeded golden (3 fixtures, N ticks, exact positions +
message stream); property test (zone-leashed wanderer never exits its zone
in 10k ticks over real content). Deliverable: `--spawn` a rat, watch it
drift around Newhaven and stop at zone borders and closed doors.

**Slice 3 COMPLETE (2026-07-18):** acquisition (FUN_00423863 port in the 5 s
round), the 1 s Job::Fast pursuit tier (breadcrumb chase, give-up > 15, 0x25
silent despawn), the flee free-attack, gated retaliation locks, the post-swing
lock re-roll, and the monster-cast summon victim pre-lock. 9 aggression + 7
pursuit tests; the two M5 seeded goldens re-pinned for the new draw order.
MAJOR SPEC CORRECTIONS (monsters.md §4 rewritten): an eligible monster ALWAYS
attacks someone (the anti-pile-on roll only picks who — fallback 20401); the
lock is written by the POST-SWING re-roll vs the follow word, aggressive modes
drop it on failure (this IS §8.14's "free retargeting", divergence resolved);
mode 6 SPARES fame ≥ 0x28 (was documented inverted); the criminal-hunter is
ROAM class 5 (base 100, only fame ≥ 0x28); +0x6f0 anti-pile-on taper resets
per MEDIUM tick and hard-gates the free attack; the free attack aborts the
move only on a LOST LIFE; no death/logout lock sweep — stale locks age out
through give-up. Player gains fame (+0x542, fed by M7 crime), the movement
breadcrumb trail, the moved-this-round flag (+0x6f4 bit 0x40). Charm-family
branches (pet assist, ward defence, charm-break) remain M7.

## Slice 3 — Aggression, pursuit, flee free-attack (new 1 s fast tick)

- **Acquisition** in the existing 5 s `Job::Energy` round (§4,
  `FUN_00423863`): per player, per room monster without target/travel order —
  behaviour gate (0/3/4 never; 6 only vs fame ≥ 0x28; else initiate),
  validity predicate (same room, attackable, hidden vs see-hidden 0x39),
  anti-pile-on `genrdn(0,100) < 50 - 5*jumped_count`, swing + lock.
- **`Job::Fast` (1 s, new):** gated to monsters holding a target (§0 — idle
  monsters cost nothing). Pursuit: direction toward target + `move_monster`;
  follow roll `genrdn(0,100) < aggression`; failures bump give-up counter —
  >15: class `0x25` despawns, others drop target.
- **Flee free-attack** (`give_monsters_a_free_attack`) wired into
  `move_player`: one room roll, first eligible monster by aggression ≥ roll
  and mode gate; a landed hit can abort the move.
- **M5 closures:** directed-travel field; `summon_spawn` victim/caster tags;
  live-cast-form and per-round retargeting divergence notes resolved against
  the now-real target model.
- **Oracle expedition:** initiation timing/lines passive vs aggressive
  templates; pursuit message sequence; free-attack on walk vs flee (reuse
  M4/M5 transcripts where they cover it). Give-up threshold is
  decompile-authoritative.

Tests: unit (mode-gate table incl. fame boundary, anti-pile-on, follow roll,
give-up, free-attack eligibility order); goldens (jump within one round of
entry; flee → pursued → caught; 16 failed follows → dropped/despawned; free
attack aborts the move). Deliverable: monsters jump you, chase you, and clip
you as you run.

**Slice 4 COMPLETE (2026-07-18):** the 5 s Job::Spawn (FUN_004232d3 port:
phase B per-user rooms building the player/monster cache, phase A ~5%-per-exit
neighbor pass, soft 9-cap with resumable cursors), generate_monster proper
(never-spawns band-0 gate, caps, respawn window, linked rooms, mongen
candidate walk with the 29/99 replace/stop draws, boss flag, entry-direction
pick, arrival + custom-movemsg lines, adjacent-room "You hear movement"
rumble, rolled coin piles with killer-visible drop lines), both-room respawn
stamping in monster_killed, and the BOOT POPULATION walk (bosses + type-3/
type-1 swarm fills — 7,854 monsters stand up on the real DB; Nathaniel is in
the Newhaven Weapons Shop with zero fixture flags, verified live). MAJOR SPEC
CORRECTIONS (monsters.md §1 rewritten): the threshold direction was INVERTED
(type-2 = 89%/kick, type-0 = 4%); the cache bytes are players/monsters per
room and the brake is monsters < players (density follows players, no quota);
a 1% natural-100 overshoots to <2x players; type-2 bypasses the respawn
timer; type-1 is boot-fill-only. The spawner runs its own RNG stream
(documented divergence — a background job on the main stream would reshuffle
every seeded golden per kick). Real-content smokes rewritten for a living
world (global wander cap shared by thousands). Deferred to slice 5 as
planned: the gamelimit/active throttle, cooldown jitter, first-kill loot
guarantee, persistence.

## Slice 4 — The density spawner + respawn timers

- **`Job::Spawn`** at the slice-1 cadence, porting `FUN_004232d3` (§1):
  per-session own-room pass, neighbor pass ~5%/exit (`genrdn(1,100) < 6`),
  per-room gate (`spawn_type` 0→threshold 5, 2→0x5a, else 0x19; 3→swarm
  loop), `live*2 <= max` brake, global cap 9/pass with a resumable cursor on
  `Core`, 15-slot room cap. Room cache iterates the `sessions` BTreeMap —
  deterministic, documented divergence from the original's user-table order.
- **`generate_monster` proper (§2):** pre-flight gates (global disable,
  spawn cap unless boss, room slots, respawn timer, linked-room cap);
  zone/level weighted template pick from a boot-built index (the
  `DAT_004790f0` table); instance creation reuses `spawn_monster` internals
  (HP copied, coins randomized, 10-slot carry rolls — already correct);
  arrival direction + line.
- **Respawn:** per-room ephemeral state (`RoomSpawnState`: live count,
  respawn_due) beside `room_coins`; `monster_killed` decrements and stamps
  the timer per death.md §4.1 with the nudge constants.
- **Oracle:** passive cadence confirmation — restart for a clean epoch, park
  a character in a known spawn room, timestamp arrival lines; respawn delay
  by killing the sole spawn in a single-cap room.

Tests: narrow seeded goldens (one room, one kick — RNG draw order is the M5
fragility lesson; draws strictly in decompile order); statistical rate tests
(10k passes: type thresholds, ~5% neighbor); cap/cursor/respawn-gate tests.
Deliverable: start with no `--spawn`, walk out the Town Gates, the world
fills in at the original's cadence.

**Slice 5 COMPLETE (2026-07-18):** the world-population throttle (refuse at
active >= gamelimit; active charged at generate, released + stamped at the
kill), the single-limit cooldown (base = regentime x 60 minutes, jitter
lngrnd(0, base/4) - base/8 drawn per attempt — refusal floor 52.5 min,
ceiling 67.5 min at regentime 1), the first-kill loot guarantee (a limited
template never killed carries every slot with NO draws — closes the
tests/loot.rs deferral), and persistence: monster_population + room_stamp
tables in state.sqlite (wall-clock stamps, upserted on kill events), restored
through CoreConfig as elapsed-seconds BEFORE the boot population walk — a
restart holds both the rare-monster cooldown and every room's refill window
(the restart-to-respawn exploit is dead). Fixture spawns stay exempt from the
throttle (dev tool). Full server-restart hand check rides the slice-6 close.

## Slice 5 — Unique-spawn timers, first-kill loot, persistence

- **Population throttle (§2 step 3):** template state on `Core` (active,
  remaining, last_kill) — refuse when exhausted; "1 remaining" real-time
  cooldown via the kill stamp (Ghidra for the minutes constant — oracle
  confirmation impractical for rare monsters). Updated in `monster_killed`
  per death.md §4.6.
- **Boss:** room boss id + present flag (`room+0x564` bit 8), cap bypass.
- **First-kill guarantee (addendum):** skip carry rolls when
  `game_limit != 0` and the last-kill stamp is zero — closes the
  `tests/loot.rs` deferral.
- **Persistence:** `state_db.rs` `monster_population` table (remaining +
  last-kill persist; active is ephemeral); room respawn timers persist too —
  matches the original's dirty-flag persistence and kills the
  restart-to-respawn exploit. Defaults read as a fresh world (remaining =
  game_limit, no stamp), preserving first-kill semantics.

Tests: goldens (cap refusal, cooldown gate, first-kill full loadout then
rolled); mud-server round-trip (kill rare → restart → cooldown holds).
Deliverable: a rare monster is actually rare, survives restarts, and pays
out its signature loot on first blood.

## Slice 6 — Close-out

Consolidated oracle expedition re-verifying M6 strings (spawn arrival
directional lines vs "appears right beside you!", wander leave/arrive,
free-attack, confusion fumble) → retag `text.rs`. Wire the cheap M5
leftovers: monster carried/wielded item terms in `monster_ability_value`;
fixture-only instant-area arms. Marker sweep: zero `M6 PENDING` in the tree
(charm family → `M7 PENDING` citing this doc); monsters.md §7 updated with
extracted constants and pinned offsets; roadmap status line.

## Testing & completion bar

- **Tier 1 — spec-cited unit tests** (TDD, red first): every gate, roll, and
  threshold above cites its monsters.md/death.md section.
- **Tier 2 — seeded scenario tests:** golden transcripts pinning gate order,
  message order, and RNG draw order per slice; regenerated per slice, never
  across slices.
- **Tier 3 — oracle expeditions:** deterministic surfaces diff against
  MBBSEmu (lines, timing bands); randomized internals carry decompile line
  citations instead (M3/M5 precedent). Quirk mitigations proven in M5:
  passive capture, babysitter polling, ≥1.5 s pacing, restart-as-reset.
- **Done:** six slices merged, `cargo test` green, clippy clean, zero `M6`
  markers, oracle scripts pass or carry a documented whitelist entry, and a
  hand telnet session — no `--spawn` flags, walk out of Newhaven, the world
  fills in, a monster jumps you, chases you when you flee, clips you on the
  way out; a rare monster stays rare across a restart.
