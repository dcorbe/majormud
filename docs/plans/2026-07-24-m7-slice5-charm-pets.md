# M7 Slice 5 — Charm & Pets Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Land the charm/pet system — monster-vs-monster combat, Enslave
acquisition, pet follow/assist, release paths, and summon ownership links —
per `re/docs/charm.md`, closing the M6/M7 monster-vs-monster markers.

**Architecture:** All behavior lands in `crates/mud-core` (`game.rs` +
`content.rs`), loader columns in `crates/mud-server/src/content_db.rs`.
Charm state is the §0 triple on the live instance: the existing
`target`/`suppress` fields plus a new `charmed` bool. Pets are ephemeral
(instances don't persist — matches a board restart). No state.sqlite
changes. Work on the `m7-content` branch (no worktrees).

**Tech Stack:** Rust workspace, `cargo test` (656 green at start), TDD
red-first, behavioral authority `re/docs/charm.md` + decompile cites into
`re/wg_nt_ghidra/exports/WCCMMUD_decompiled.c`.

**Key model mapping (charm.md §0 → our code):**

| DLL field | our field (`MonsterInstance`, game.rs:868) |
|---|---|
| `mon+0x1a` name link (grudge/owner) | `target: Option<SessionId>` (game.rs:881) |
| `mon+0x116` attack-suppression | `suppress: bool` (game.rs:929) |
| `mon+0x128` bit 0 charmed | **new** `charmed: bool` |
| `mon+0x88` hunt link | **new** `hunt: Option<MonsterInstanceId>` (Task 7) |
| `mon+0x34..0x5c` travel trail | **new** `trail: Vec<RoomId>` 10-deep (Task 7) |

The DLL keys the owner by NAME; we key by SessionId. Logout invalidates the
session, the pursuit tier bumps `give_up` each tick (game.rs:2399 already
handles the dead-session arm), and >15 releases — same ~16 s observable
window as the DLL (§4.2). Documented divergence: a re-login mid-window gets
a new SessionId, so the pet won't re-attach; the DLL's name key would. Note
this in charm.md's as-built section (Task 8).

**Shipped-data facts (re/mmud_wgnt.sqlite, verified 2026-07-24):**
- Exactly four Enslave(6) spells, ALL monster-target (`target`=4), ALL
  duration>0, ALL save class 2 (always savable): 49 song of charming
  (dur 100), 55 enslave (dur 60, min/max 0/0), 88 control undead (dur 80),
  92 charm animal (dur 60). The instant-Enslave arm (§1.4 `+0xce == 0`) is
  fixture-only.
- `charmlvl`: 54 templates at 0 (anyone), 381 at 9999 (never in practice —
  plain signed compare), rest in between. Pin: giant rat (template 1)
  charmlvl 1, charmres 40.
- Player-cast Summon(12) is fixture-only (zero learnable carriers — the
  slice-4 note at game.rs:5139 stands). The hunt-link machinery in Task 7
  is decompile-faithful but only fixture/monster-cast reachable.

---

### Task 1: Content columns — `charmlvl` / `charmres`

**Files:**
- Modify: `crates/mud-core/src/content.rs` (Monster struct, ~line 244)
- Modify: `crates/mud-server/src/content_db.rs` (`load_monsters`, line 253)
- Test: `crates/mud-server/tests/` — find the existing full-DB load test
  (grep `mmud_wgnt.sqlite` under `crates/mud-server`) and extend it; if
  none asserts monster columns, add the assertion to whatever boot test
  loads the real DB.

**Step 1: Write the failing test**

In the existing real-DB load test, pin the new columns:

```rust
// charm.md §1.1/§1.2: knmsr+0x120 charmlvl, +0x1a0 charmres.
let rat = &content.monsters[&MonsterId(1)];
assert_eq!(rat.charm_level, 1);
assert_eq!(rat.charm_resist, 40);
```

**Step 2: Run it — must fail to compile** (no such fields):
`cargo test -p mud-server` → expected: compile error `charm_level`.

**Step 3: Implement**

`content.rs` — add to `Monster` after `magic_resist` (keep doc-comment
style of the neighbors):

```rust
/// `knmsr+0x120` (`charmlvl`) — Enslave application gate: charmable when
/// `charm_level <= caster level`, plain signed compare (charm.md §1.2).
pub charm_level: i16,
/// `knmsr+0x1a0` (`charmres`) — the Enslave SAVE stat, replacing MR for
/// ability-6 spells; no floor (charm.md §1.1).
pub charm_resist: i16,
```

`content_db.rs` `load_monsters` — add `charmlvl, charmres` to the SELECT
and wire `to_i16` reads. CAREFUL: this loader indexes columns positionally
(the slice-1 "shift pattern") — append at the END of the SELECT and use the
next indices; do not insert mid-list.

**Step 4: `cargo test -p mud-server` → PASS; `cargo test -q` all green.**

**Step 5: Commit** — `feat: load charmlvl/charmres monster columns (charm.md §1)`

---

### Task 2: Monster-vs-monster combat — `attack_monster_monster`

The prereq (design doc slice 5, first bullet). Port of `0x2f6ae`
(decompiled.c 27213-27340) + the monster arm of `move_monster_to_fighter`
(25087-25230). Closes the "monster-vs-monster — M7" half of the marker at
game.rs:8228 (the hunt-branch half closes in Task 7).

**Files:**
- Modify: `crates/mud-core/src/game.rs` (new fns near `monster_attack`,
  line 8497; test hook near `monster_defense_debug`, line 1754)
- Modify: `crates/mud-core/src/text.rs` (five room strings)
- Test: `crates/mud-core/tests/monster_vs_monster.rs` (new)

**Behavior to port exactly (charm.md §3, decompile-verified this pass):**

1. Preconditions (27231-27235): attacker current energy >= its max
   (full-energy gate — same shape as `monster_attack`'s gate at
   game.rs:8521), and attacker template does NOT carry ability 0x3c.
   Either failure → silent return, no draws.
2. Fighter builds — BOTH sides from **attack-form slot 0** (decompile
   25087: `move_monster_to_fighter(..., param_3 = 0)`); a monster whose
   form 0 has kind 0 never swings and is never swung at (the `!= '\0'`
   guards at 27239-27243 — `move_monster_to_fighter` returns 0 there).
   - Attacker: accuracy = form-0 accuracy + abilities 0x16/0x69/0x6a
     (25185-25193), damage = form-0 min/max + ability 4 on both bounds
     (25196-25198), EU = form-0 energy, scaled by ability 0x57 as
     `EU*val/100` capped at `knmsr+0x7a` (25203-25211). Attack type =
     mode 5 → our `AttackType::Normal` (no seed, no acc mod; monster
     crit_rating 0 keeps crits off).
   - Defender: reuse `build_monster_defender` (game.rs:10645) but add
     `parry` = Dodge(0x22) ability (25199-25200 `param_2[8]`) — check
     whether the monster-vs-player path should also carry it and, if so,
     fix `build_monster_defender` itself in this task (it currently
     hard-zeros parry; combat.md's monster-fighter row says `[8]` =
     Dodge). One shared build, no fork.
3. `calculate_attack` (crates/mud-core/src/combat.rs:83) does all its
   draws in documented order. Then (27244-27254): abort silently if
   `result EU > attacker energy`; else pay EU, clamp damage to the
   defender's remaining HP, apply, raise defender poison floor
   (set-if-greater from result[3] — mirror what the player path does with
   it), dirty both.
4. Kill path (27311-27334): capture the defender's DISPLAY name first,
   DamageShield(0x48) `genrdn(1, max(val+1,1))` against the attacker,
   then `monster_killed(defender, None)` — our fn already does the
   check_kill + killer-less exp split among engaged sessions +
   break_combat, which is the DLL's `check_kill_monster(-1)` +
   `distribute_experience(-1, worth*multi)` + `kill_autocombat` trio
   (worth*multi == our `exp` at game.rs:10293) — then broadcast
   `%s just killed %s!` to the ATTACKER's room. Divergence note (doc
   comment): the DLL also shares to idle-autocombat users in the room;
   our split is engaged-only — cite charm.md §3.
5. Survivor path (27285-27308): DamageShield draw only when damage >= 1;
   then ONE room line to the DEFENDER's room, first letter upcased:
   - hit: `%s just attacked %s!`
   - glance (`Outcome::NoDamage`): `%s's %s just glanced off of %s's armor!`
     — middle `%s` is the attacker's weapon name (empty for unarmed;
     check the decompile at 27298 for the exact arg order before wiring)
   - dodge (`Outcome::Parried` — result 3): `%s just dodged an attack from %s!`
   - miss (`Outcome::Dodged`): `%s just missed an attack against %s!`
6. RNG order per swing: calculate_attack internals, then at most one
   DamageShield draw. No retaliation by the defender inside the call.

**Step 1: Write failing tests** (`tests/monster_vs_monster.rs`, harness
cribbed from `tests/aggression.rs`):

- `full_energy_gate_and_ability_0x3c_block` — no swing, no output.
- `hit_applies_damage_and_room_line` — seeded RNG, observer session in the
  defender's room sees `Beast 1 just attacked beast 2!` (upcased), HP
  delta matches the scripted roll.
- `kill_splits_exp_to_engaged_users` — a session engaged on the defender
  gets `worth*multi` exp, combat broken, kill line in the attacker's room.
- `miss_line` / `glance_line` — scripted outcome strings.

Drive via a new test hook:

```rust
/// Test hook: one m-v-m swing (`attack_monster_monster`, charm.md §3).
pub fn debug_monster_attack_monster(&mut self, a: MonsterInstanceId, d: MonsterInstanceId) { ... }
```

**Step 2: `cargo test -p mud-core --test monster_vs_monster` → FAIL** (hook missing).

**Step 3: Implement** `fn attack_monster_monster(&mut self, attacker, defender)`
+ `fn build_monster_attacker_form0(&self, id) -> Option<(Fighter, i32 /*EU*/)>`
+ text.rs strings. Cite decompile lines in comments as the codebase does.

**Step 4: Test green; full `cargo test -q` green (no golden churn — new
RNG draws only inside the new fn).**

**Step 5: Commit** — `feat: attack_monster_monster — form-0 fighter builds, mode-5 pipeline, DamageShield, killer-less exp split (charm.md §3)`

---

### Task 3: Enslave acquisition — the charm apply

Port of `cast_monster_target` case 6 (43796-43822) + the charmres save
preload (43302-43316). Lands in `fire_monster_cast` (the offensive
monster-target path, game.rs:~7050-7270).

**Files:**
- Modify: `crates/mud-core/src/game.rs` — `MonsterInstance` gains
  `charmed: bool` (init false at both spawn sites, lines ~1366/~1646);
  save-stat selection; a new `Ability::Enslave` arm in the apply loop
  (currently falls to the default duration-slot arm at game.rs:7253).
- Test: `crates/mud-core/tests/charm.rs` (new)

**Behavior (charm.md §1):**

1. **Save stat swap** (§1.1): when the spell's ability list carries
   Enslave(6), the save stat is the template's `charm_resist` — raw, NO
   `.max(1)` floor (charmres 0 never resists: 0/2 = 0) — instead of
   `monster_save_stat`. Same `monster_save_resists` formula (game.rs:569),
   same save-class gate. Resist prints the existing resist line and
   applies nothing.
2. **Apply gate** (§1.2): dedicated `Ability::Enslave` match arm:
   - spell match type ∈ {4, 6, 8} AND
     `template.charm_level <= caster level` (plain signed compare, no roll).
   - Gate fails → the arm does NOTHING (silent — no message, no slot
     entry, mana already paid). It must NOT fall through to the default
     slot-entry arm.
3. **On pass** (§1.3/§1.4): duration != 0 → drive the one slot entry
   exactly like the default arm (`entered = Some(enter_monster_spell_slot(...))`);
   THEN write the triple unconditionally — even when the slot entry
   returned false (the DLL ignores the -1: a fully-slotted monster gets a
   permanent timerless charm, §1.4):

   ```rust
   m.target = Some(session);   // owner link (+0x1a ← caster name)
   m.suppress = true;          // +0x116 = 1
   m.charmed = true;           // +0x128 |= 1
   m.needs_recompute = true;   // +0x140 dirty
   ```

   No rename — display name untouched (§1.3). Instant (duration 0) →
   triple only, no slot. ORACLE-VERIFY tag on instant messaging (§7).
4. **RNG order** (§1.5): success roll → save → magnitude roll → duration
   roll. This is the order `fire_monster_cast` already draws in — verify
   with a scripted-RNG test, don't reorder.
5. A resisted or failed Enslave sets no retaliation lock (duration != 0 —
   the existing gate at game.rs:7108 already skips it). A SUCCESSFUL charm
   must not then have the post-loop harm path lock the pet on the caster —
   confirm `retaliation_lock` at game.rs:7323 is behind `harms` (it is —
   Enslave sets no `harms`).

**Step 1: Failing tests** (fixture: giant-rat-alike template,
charm_level 1, charm_resist 40; bard/mage caster with a duration-60
Enslave spell):

- `charm_level_gate_silent` — caster level below charm_level: mana paid,
  monster unslotted, `charmed` false, NO output line beyond the generic
  cast lines.
- `charm_success_writes_triple_and_slots` — triple set, slot entered,
  success lines = the generic display_spell_success pair (byte-pin).
- `charmres_save_not_mr` — template MR 200 / charmres 0: never resists;
  MR 0 / charmres 196: resist threshold 98 — script both sides of the
  roll.
- `slot_full_charm_still_lands_permanent` — 5 foreign slots: fail line
  prints, no success lines, triple STILL written, no Enslave slot.

Read `charmed` via a small test hook
(`pub fn debug_monster_charm(&self, id) -> Option<(bool, bool, Option<SessionId>)>`
— charmed/suppress/target).

**Step 2: FAIL** (no `charmed` field/hook).

**Step 3: Implement** per above.

**Step 4: All green** — the new save-stat branch must not disturb
non-Enslave casts (no golden churn expected; if `cast_messages` goldens
move, the save-stat selection leaked — fix, don't re-pin).

**Step 5: Commit** — `feat: Enslave charm apply — charmres save, charmlvl gate, §0 triple, slot-full permanent charm (charm.md §1)`

---

### Task 4: Release paths

Port of §4 — expiry, give-up/logout, owner-attacks-pet, and the
retaliation exemption.

**Files:**
- Modify: `crates/mud-core/src/game.rs`:
  - termination arm game.rs:2266-2271 (currently `Ability::Enslave => {}`)
  - give-up path in `pursue_monster` game.rs:2410-2417
  - `retaliation_lock` game.rs:8331 (the "charmed bit-0 exemption is M7"
    comment at 8330)
  - the melee survivor branch in `player_attack_sequence` (post-damage,
    near the `retaliation_lock` calls at game.rs:8481/8487)
- Test: extend `crates/mud-core/tests/charm.rs`

**Behavior:**

1. **Shared release helper** — the §4.1 reversal (44988-44995):

   ```rust
   /// perform_spell_termination_monster_upkeep case 6 (charm.md §4.1).
   fn release_charm(&mut self, id: MonsterInstanceId) {
       // target = None (+0x1a emptied), suppress = false, charmed = false,
       // needs_recompute = true. No message to anyone.
   }
   ```

   Wire it into the termination arm at game.rs:2266 (replaces the no-op).
   The released monster rejoins normal wander/acquisition — no grudge.
2. **Ability-6 slot sweep** (used by 3-5 below): walk the 5 slots,
   terminate every slot whose spell carries Enslave (clear slot, run the
   termination = the release), matching the §2.2/46929-46953 sweep. A
   slotless pet (instant/summon) is the §4.3 asymmetry — the sweep finds
   nothing, `target` KEEPS the owner.
3. **Give-up / logout** (§4.2, 19446-19489): in `pursue_monster`'s
   `give_up > 15` branch — non-0x25: clear lock + counter as today, and
   if charmed additionally clear the bit and sweep ability-6 slots.
   Roam-0x25 charmed: despawn (already the code path). Also §2.1: the
   follow ROLL is skipped for charmed (19422-19423) — in
   `pursue_monster`, the `self.rng.roll(0,100) >= aggression` bump arm
   (game.rs:2384) must not DRAW for a charmed monster (RNG-order
   sensitive; pets always follow).
4. **Owner melee release** (§4.3, 26513-26563): in the post-swing
   survivor branch, before the retaliation lock: if target monster is
   charmed —
   - attacker == owner (`m.target == Some(session)`): `suppress = false`
     FIRST, then clear charmed + sweep (slotless ex-pet → full grudge
     hostile to the ex-owner: target kept, suppress false).
   - attacker != owner: NOTHING (no lock, no overwrite).
5. **Autocombat release** — in the Task 5 assist branch: owner's target
   == the pet itself → clear charmed + sweep; `suppress` STAYS true
   (slotless → degrades to a "friend", §4.3).
6. **Retaliation exemption**: `retaliation_lock` gains an early return
   when `m.charmed`. Decompile-check first (systematic-debugging rule —
   read before writing): the cast-damage twin at 43750-43765 — pin
   whether a charmed target's spell-damage retaliation differs from the
   melee branch (charm.md §2.4 says the attacker==name case only clears
   `+0x116`); cite what you find, implement what the decompile says.
7. **What does NOT release** (§4.4): owner death — no sweep anywhere in
   `player_killed` (verify none of your new calls leak into it; add a
   test).

**Step 1: Failing tests:**
- `expiry_releases_silently` — charm with duration, tick it out: triple
  cleared, no output, ex-pet wanders again.
- `logout_releases_via_give_up` — drop the owner session, run ~16 fast
  ticks: released; roam-0x25 variant despawns.
- `owner_melee_attack_releases` / `slotless_pet_becomes_grudge` /
  `other_player_attack_no_lock`.
- `charmed_pet_follow_skips_roll` — scripted RNG: no draw consumed on the
  pet's pursuit step.
- `owner_death_keeps_pet`.

**Steps 2-4:** red → implement → all green.

**Step 5: Commit** — `feat: charm release paths — expiry reversal, give-up/logout sweep, owner-attack release + grudge asymmetry, retaliation exemption (charm.md §4)`

---

### Task 5: Pet behavior — assist & the driver branch

Port of `FUN_0044cc65` (46917-46965) into `monster_consider`'s locked
branch (game.rs:8205-8225, replacing the "charmed pet-assist branch is M7
charm" note at 8208).

**Files:**
- Modify: `crates/mud-core/src/game.rs` (`monster_consider`,
  `wander_monster` game.rs:2306)
- Test: extend `crates/mud-core/tests/charm.rs`

**Behavior (§2.2):** in the `Some(victim)` branch, FIRST — when
`suppress && m.charmed`:
- owner session gone/not in game → return (pursuit owns the aging).
- owner in game, `target: Some(t)`:
  - `t == id` (the pet) → autocombat release (Task 4 item 5).
  - else → `attack_monster_monster(id, t)` — deterministic, no roll, every
    driver pass. (Owner-target-is-a-user = PvP, M8 — our target is always
    a monster id.)
- owner in game, no target → return (a pet never falls through to the
  suppressed-aggressive "attack others" arm — that arm is for non-charmed
  friends, §2.2 last paragraph).

**Wander literalism** (§2.1): `wander_monster` returns early on
`target.is_some()`; add `|| m.charmed` to match 19340-19378's explicit
bit check (unreachable in practice, cheap to be literal).

**Step 1: Failing tests:**
- `pet_assists_owner_target` — owner engaged on monster X; driver pass →
  pet swings X (m-v-m room line seen).
- `pet_idle_when_owner_idle` — no swing, no draws.
- `pet_never_attacks_others` — second player in room, pet suppressed +
  charmed: the suppressed-aggressive arm must NOT fire.
- `owner_autocombat_on_pet_releases_as_friend` — owner targets the pet:
  released, suppress stays true.

**Steps 2-4:** red → implement → green.

**Step 5: Commit** — `feat: pet assist — FUN_0044cc65 branch, owner-target attack, autocombat self-release, wander bit gate (charm.md §2)`

---

### Task 6: Targeting exemptions

Port of §2.3 — pets are DEPRIORITIZED in targeting, not hidden.

**CORRECTED after the Task-3b routing work** (verified twice against
`find_action_target` 63726): the `0x800` mask bit means **charmed monsters
are searched LAST, not excluded**. `find_action_target` runs two passes —
the first skips `mon+0x128 & 1` when `0x800` is set, the second scans ONLY
charmed monsters. `0x800` rides masks `0x801` (match 4), `0x803` (match 8)
and `0xf837` (match 6), but is ABSENT from the universal retry `0xf037`.
So `cast mmis rat` with a pet rat and a wild rat present hits the WILD one;
with only the pet present, pass 1 finds nothing, pass 2 finds the pet, and
**the pet gets hit**. The original "exclude your own pet" premise was
wrong — do not implement it.

**Files:**
- Modify: `crates/mud-core/src/content.rs` — `FindScope` gains the
  charmed-last ordering flag (a fourth field or an ordering enum; it
  currently models only inclusion).
- Modify: `crates/mud-core/src/game.rs` — `find_cast_target`'s monster
  pass (the Task-3b shared resolver), and the area-cast monster sweep
  (`area_cast` / the `monster_count_valid_targets` port at ~game.rs:9452).
- Test: extend `crates/mud-core/tests/charm.rs` and/or
  `crates/mud-core/tests/cast_routing.rs`

**Behavior:**
1. **Two-pass monster resolution**: implement the ordering in the shared
   resolver. Pass 1 skips charmed monsters when the mask carries `0x800`;
   pass 2 scans only charmed ones. The universal retry does NOT carry
   `0x800`, so it treats charmed and wild alike — reproduce that
   asymmetry exactly, it is what makes a lone pet targetable.
2. **`is_valid_monster_target`** (38430, 38477-38488, match 9/0xc): read
   this separately — it is a DIFFERENT gate from the find ordering, and
   §2.3 claims it makes your own pet an invalid target for hostile spells.
   Determine from the decompile which shipped call paths actually consult
   it and whether it survives the routing model; implement what the
   decompile says, and reconcile §2.3's wording with the two-pass finding.
3. **Area casts**: determine from the decompile whether the area sweep
   honors charm at all (it may simply not consult `0x800`) rather than
   assuming the single-target rule carries over.
4. Melee ATTACK must keep hitting pets either way (§2.3: physical attacks
   are allowed — they are a release path).
3. **Threat scans** (`monster_could_attack` 18238-18241): no consumer in
   our tree yet (no rest gate ported) — leave a one-line
   `M7 slice5: monster_could_attack pet exemption lands with its consumer`
   note where rest lands, or nothing if no anchor exists. Do not build
   speculative plumbing (YAGNI).
4. **Flee free-attack**: decompile 23865-23895 re-read this pass — the
   gate is name+suppression only, NO charmed check; our port at
   game.rs:11123 already matches. No change; add the §2.4 cite to its
   comment.

**Step 1: Failing tests:**
- `hostile_cast_cannot_target_own_pet` — `cast mmis pet-name` → do-not-see
  line, nothing charged.
- `hostile_cast_hits_your_grudge_holder` — unsuppressed monster locked on
  you: valid target.
- `area_cast_skips_own_pet` — pet unhurt, other monsters hit.
- `melee_attack_still_hits_pet` (release covered in Task 4; here just the
  targeting).

**Steps 2-4:** red → implement → green.

**Step 5: Commit** — `feat: pet targeting exemptions — hostile single/area casts skip own charmed/suppressed monsters (charm.md §2.3)`

---

### Task 7: Summon ownership links + the hunt branch

Port of §6 — the `summon_spawn` triple/victim tags (replacing the
`// pet links M7` at game.rs:5229 and `// hunt links M7` at game.rs:7246)
and the `+0x88` hunt arm of the driver (the remaining half of the marker
at game.rs:8227).

**Files:**
- Modify: `crates/mud-core/src/game.rs` — `summon_spawn` (game.rs:5681),
  its three call sites (5229, 7246, 9245), `monster_consider` A1 branch,
  `move_monster` (trail push), `MonsterInstance` (+`hunt`, +`trail`)
- Test: `crates/mud-core/tests/summon_links.rs` (new)

**Behavior (§6 table):**

1. Replace `summon_spawn`'s `lock: Option<SessionId>` with an explicit
   link enum — one obvious way, no boolean soup:

   ```rust
   enum SummonLink {
       Pet(SessionId),                 // cast_no_target 0xc: target=caster, suppress=true, charmed=true
       HuntUser(SessionId),            // monster_cast 0xc: target=victim, suppress=false (today's Some(victim))
       HuntMonster(MonsterInstanceId), // cast_monster_target 0xc: hunt=victim, no name link
       None,
   }
   ```

   - game.rs:5229 (player bare Summon) → `Pet(session)` — a full
     timerless pet (release via §4.2/§4.3 only; duration on any summon
     route is silly_spell already).
   - game.rs:9245 (monster cast at player) → `HuntUser(victim)` —
     unchanged behavior, new spelling.
   - game.rs:7246 (player Summon at a monster) → `HuntMonster(target)`.
     The victim's 10-deep back-link array (+0x60, 43922-43928) has NO
     located reader (charm.md §7) — do not port it; doc-comment the
     omission.
2. **Monster trail**: push the departed room onto a 10-deep
   `trail: Vec<RoomId>` in `move_monster` (index 0 = current room's
   predecessor — mirror the player-trail convention at game.rs:746).
3. **Hunt driver arm** (20448-20463): in `monster_consider`'s no-target
   branch, before the roam-5/aggro acquisition — when
   `m.hunt == Some(victim)` and the victim is alive:
   - `dir_monster_hunt` = the `dir_monster_travelling_coord` port
     (15790-15816): victim in the SAME room → None; else find own room in
     the VICTIM's trail at index i (scanning from 1), return the exit of
     the own room whose dest == trail[i-1]; miss → None.
   - Some(dir) → confusion check, then `move_monster` (one step, no
     roll).
   - None → if `!suppress` → `attack_monster_monster(id, victim)`. Yes:
     the DLL swings on a cold trail even cross-room (verified 20451-20456
     this pass) — mirror it, cite it, note reachability (fixture-only).
   - Dead/despawned victim: the DLL never clears `+0x88` (§7 stale-link
     flag) — ours holds a typed id; if the instance is gone, skip the arm
     (id reuse impossible with our u64 counter — divergence-free
     improvement, note in the doc comment).
4. Stub arms stay stubs: `cast_user_target` 0xc (player-hunt-player)
   needs PvP targeting — unreachable (no learnable Summon) — leave with
   an `M8 PENDING` cite.

**Step 1: Failing tests:**
- `bare_summon_is_pet` — fixture no-target Summon spell: spawned monster
  has the full triple toward the caster; assists like Task 5.
- `monster_summon_hunts_player` — existing behavior re-pinned through the
  new enum (crib from whatever slice-4 test covers game.rs:9245).
- `summon_at_monster_walks_and_kills` — hunter follows the victim's trail
  one room and swings on arrival (scripted RNG).
- `hunter_cold_trail_swings_cross_room` — decompile-literal arm.

**Steps 2-4:** red → implement → green. Watch `tests/spawner.rs` /
name-gen goldens: `summon_spawn` must not add RNG draws before the spawn
draw sequence.

**Step 5: Commit** — `feat: summon ownership links — Pet/HuntUser/HuntMonster tags, monster trail, hunt driver arm (charm.md §6)`

---

### Task 8: Stubs pin, marker sweep, docs

**Files:**
- Test: extend `crates/mud-core/tests/charm.rs`
- Modify: `re/docs/charm.md` (as-built cites), 
  `docs/plans/2026-07-19-m7-content-systems-design.md` (slice 5 COMPLETE
  banner)
- Sweep: game.rs markers at 2266, 5229, 5679, 7139, 7246, 8208, 8227,
  8330 — every `M7`/charm-pending comment either deleted (landed) or
  re-cited to its real home (M8 PvP, slice-8 oracle).

**Step 1: Stub tests** (charm.md §5 — `tame`/`mesmerize` are parse-table
stubs whose return 0 falls through to speech):
- `tame_and_mesmerize_fall_to_say` — `tame bear` → `You say "tame bear"`
  + room line (our unparsed-input path already does this — the test PINS
  that we never grow a handler).
- `player_target_enslave_stays_silly` — pin the existing silly_spell
  surface for a player-target Enslave fixture.

**Step 2:** Likely already green (that's fine — they're pins, not new
behavior; confirm they fail if a naive `tame` verb is added by flipping
the assertion once, then restore).

**Step 3: Docs.** charm.md gains an "As built (M7 slice 5)" note: the
SessionId-vs-name owner-key divergence, the engaged-only exp split, the
skipped +0x60 back-links, the typed-hunt-id stale-link improvement.
Design doc gets the slice-5 COMPLETE banner (test count, commit range,
leftovers → slice 8: ORACLE-VERIFY instant-Enslave messaging, live charm
lifecycle strings per the Testing section's oracle expedition).

**Step 4: Full gates:** `cargo test -q` all green, `cargo clippy
--workspace` clean, `grep -rn "M7" crates/ | grep -i charm` → empty.

**Step 5: Commit** — `doc: M7 slice 5 COMPLETE banner — charm & pets landed`

---

## Oracle expedition (slice 8 or opportunistic)

Design-doc Tier 3, needs the live board (run `board-safe-to-restart`
FIRST if a restart is wanted; see memory note): charm a giant rat with
charm animal / song of charming (SYSOP SUMMON the scroll if needed), walk
it, watch one assist round, attack it as the owner, let a second charm
expire — pin every string against Tasks 3-5's test expectations, retag
text.rs lines ORACLE→MEASURED.

## Risks

| Risk | Mitigation |
|---|---|
| Save-stat swap leaks into non-Enslave casts | Task 3 step 4: goldens must not move; fix, never re-pin |
| Follow-roll skip shifts RNG draw order for existing pursuit goldens | The skip only fires for `charmed` — impossible before Task 3; `tests/pursuit.rs` stays byte-stable |
| `build_monster_defender` parry fix (Dodge) churns monster-vs-player goldens | Decide in Task 2: if shipped data has Dodge-carrying monsters in any golden path, land the fix as its own commit with the re-pin isolated |
| Cast-damage retaliation twin (43750-43765) differs from melee | Task 4 item 6 reads the decompile BEFORE coding; cite lines either way |
| `content_db` positional-index shift | Append-only SELECT change (Task 1) |
