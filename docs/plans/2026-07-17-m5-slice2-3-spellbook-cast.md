# M5 Slices 2+3: Spellbook & Cast Skeleton — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Slice 2 — persistent spellbook learned from LearnSp scrolls, `spells`
listing, shop suffixes. Slice 3 — the cast command through instant
single-target effects: a mage kills a monster with magic missile end to end.

**Architecture:** All game logic in `mud-core` (`game.rs` handlers,
`command.rs` parser rows, `text.rs` strings), persistence in
`mud-server::state_db`. Oracle-measured strings are law
(`re/docs/spellcasting.md` §8); formulas cite the spec §§2-4. Design:
`docs/plans/2026-07-16-m5-magic-design.md` slices 2-3.

**Tech Stack:** Rust workspace; sqlite state DB; seeded/injectable RNG
(M3 pattern) for roll determinism.

---

## Facts pinned during planning (do not re-derive)

**Oracle-measured (spellcasting.md §8, transcripts in re/oracle/):**

- Training grants NO spells; LearnSp(42) scrolls are the only path (§8.1).
- `learn` is not a verb. `use <scroll>` → `You read scroll of X and learn the
  spell Y.` + blank line. `read <scroll>` (owned) → same line +
  `Its magic used, the scroll disintegrates.` `read` unowned prints the item
  description paragraph and learns nothing (§8.4).
- Too-high scroll: `You may not use that item!` — NOT consumed; the book can
  never contain an uncastable-yet spell (§8.4).
- Shop suffixes (§8.3): `(You can't use)` = wrong magery group;
  `(Too powerful)` = same group but **character level** < spell `level`
  (+0xbe, `required_power`) — proven by the suffix flipping at L2 while
  Spellcasting changed 43→45 (gate is level, not SC).
- `spells` output (§8.5): `You have the following spells:` then header
  `Level Mana Short Spell Name`, rows sorted level ascending then name.
  Exact column spacing MUST be copied from `re/oracle/oracle_spell_train.raw`
  (markdown in §8.5 may not preserve trailing spaces). Empty book:
  `You have no spells.`
- Cast strings (§8.6): unknown/unlearned → `You do not know how to cast
  {arg}.` (argument echoed verbatim; learned-book lookup). Fail roll →
  `You attempt to cast magic missile, but fail.` Insufficient mana →
  `You do not have enough mana to cast that spell.` One cast per round, even
  energy-0 spells → `You have already cast a spell this round!` Offensive
  bare cast in a room with only a friendly NPC → `You are overcome with a
  feeling of guilt and break off your attack.` Successful attack cast:
  `*Combat Engaged*` + `You fire a magic missile at nasty filthbug for 13
  damage!`; kill adds M3's death/exp lines then `*Combat Off*`.
- Name resolution accepts full shortname (`c mmis`, `c illu`) and full name
  (`cast magic missile`); `c mami` does NOT resolve (§8.6).
- Caster prompt `[HP=n/MA=n]:` already works (M2); mana deltas confirmed live.

**Cast-message model (checked against mmud_wgnt.sqlite; AMENDED by Task-10
review):** `castmsga` is 1 (the empty message) on all sampled spells — render
`castmsgb` only, flag if a spell ships a non-1 `castmsga`. `castmsgb` message
record = 3 audience lines: line1 → caster, line2 → target, line3 → room.
**The arg orders below hold only for `msgstyle & 1 == 0` spells** (all slice-3
starters). ~441 spells with `msgstyle & 1 == 1` (fireball 120, deathtouch 58,
chaos storm 140, righteousness 347…) bind (target, damage)/(damage)/(target,
damage) with no spell-name slot — Task 11's caller must check `msgstyle` and
flag/refuse odd styles loudly (Spell doesn't load the column yet; add it when
first needed). Examples (msgstyle-even):
- msg 3242 (magic missile): `You fire a %s at %s for %d damage!` /
  `%s fires a %s at you for %d damage!` / `%s fires a %s at %s for %s damage!`
  (note line3's damage is `%s`).
- msg 2 (illuminate): `You cast %s!` / `%s casts %s!` / `%s casts %s!`
- msg 7 (blur): `You cast %s on %s!` / `%s casts %s upon you!` /
  `%s casts %s on %s!`

**Magnitude arithmetic (decompile-verified today):**
- Zero-denominator guard CONFIRMED at WCCMMUD_decompiled.c:41790-41796
  (`+0xf3 == 0 ⇒ scale 0`) — `ScalePair::scaled` is correct as shipped.
- The `+0xf2/f3` pair adds to **max_base** (+0xc2) and `+0xf6/f7` to
  **min_base** (+0xc0) (lines 41803-41806) — editor naming, matching our
  `max_increase`/`min_increase` fields.
- Roll: `hi = max_base + max_increase.scaled(L)`, `lo = min_base +
  min_increase.scaled(L)`, swap so lo ≤ hi, `V = genrdn(0, hi-lo+1) + lo`,
  then resist `V' = (100-resist)*V/100` (line 41809-41811).
  `L = min(player_level, spell.level_cap)` (41783-41789).
- Observed mmis damage of **13** at L1 (bounds 4..12) means `genrdn(a,b)` is
  inclusive of `b` (13 = hi+1 = 12 + max roll of `hi-lo+1`=9 over lo=4).
  Verify against M3's settled genrdn before implementing; if M3's genrdn is
  exclusive, the magnitude call must be `genrdn(0, hi-lo+1+1)` equivalent —
  match whatever makes 4..13 the mmis range.
- Success roll: `genrdn(0,100) < min(SC + base_chance, 98)`; `base_chance >=
  200` auto-succeeds (spec §3). SC = the Spellcasting derived stat (Vexil:
  43; mmis base 15 ⇒ 58%).

**Eligibility rule (spec §2 gate 1-2 + §8.3):** given `class =
content.classes[player.class]` and spell S:
- `S.class_gate_group != 0` ⇒ require `class.caster_group ==
  S.class_gate_group && class.casting_factor >= S.required_class_level`,
  else **WrongClass**.
- `player.level < S.required_power` ⇒ **TooPowerful**.
- Alignment gates (Good/Evil/... abilities) stay DEFERRED like M4's — none of
  the starter scrolls carry them; leave a doc note where they'd slot.

**Code anchors:**
- Parser: `crates/mud-core/src/command.rs` — `ALIASES` (exact shortcuts,
  :66), `VERBS` (name, min_abbrev, kind — :93), `parse` (:131),
  `Resolution::FallThrough` (:58). Dispatch: `game_command`
  `crates/mud-core/src/game.rs:960-1049`; arg-verbs fall through to
  `self.say(session, line.trim())`.
- Inventory lookup pattern: `word_prefix_match` (game.rs:28) +
  `iter().position(...)` + `inventory.remove(pos)` (see `drop_command`
  game.rs:1970).
- Shop list: `list_command` game.rs:1625; suffix at :1664-1667 via
  `user_can_use`; `CANT_USE_SUFFIX` text.rs:292-294 (its doc already
  promises the M5 `(Too powerful)` string).
- Class fields: `caster_group` (+0x40 `magictype`), `casting_factor`
  (+0x42 `magiclvl`) — content.rs:581-583.
- Per-round state lives on `Session::InGame` (game.rs:314-326: `energy`,
  `target`, `aided`...), refilled/driven by `energy_round` (game.rs:2177).
- Persistence: `player_item` table pattern state_db.rs:107-114;
  `save_player` :230 (delete+reinsert), `load_player` :345;
  `Event::Persist(Box<Player>)` game.rs:285-297 already carries the whole
  player — NO new Event variant needed.
- Monster damage/death path: M3's combat driver; reuse its kill/exp-split
  (death lines + `You gain N experience.` + `*Combat Off*` already exist).
- Vexil (Human Mage, acct Vexil/test123) lives in MBBSEmu for verification;
  Newhaven Spell Shop = room (1,2144), shop 48; scrolls of magic missile and
  blur are free.

---

## SLICE 2 — Spellbook

### Task 1: Spellbook state + persistence

**Files:**
- Modify: `crates/mud-core/src/game.rs` (Player struct ~:84-119, plus every
  `Player { ... }` literal — creation ~:2869 and test seats)
- Modify: `crates/mud-server/src/state_db.rs` (schema ~:54-114, save ~:230,
  load ~:345)
- Test: `crates/mud-server/tests/state_db.rs`

**Step 1: Write the failing persistence test** (follow the existing
save/load roundtrip test's shape in state_db.rs tests):

```rust
#[test]
fn spellbook_roundtrips() {
    let db = fresh_db(); // whatever helper the file uses
    let mut player = sample_player("Vexil"); // existing fixture helper
    player.spellbook.insert(SpellId(1), false);   // learned
    player.spellbook.insert(SpellId(129), true);  // temporary (GiveTempSpell)
    db.save_player(&player).unwrap();
    let loaded = db.load_player("Vexil").unwrap().unwrap();
    assert_eq!(loaded.spellbook, player.spellbook);
}
```

Run: `cargo test -p mud-server --test state_db spellbook 2>&1 | tail -5`
Expected: compile FAILURE — `Player` has no `spellbook`.

**Step 2: Add the field.** `Player` gains:

```rust
    /// Learned spells; `true` = temporary (GiveTempSpell 160, purged when
    /// the granting effect ends — wiring lands in slice 4). Display order
    /// is computed at render (level, then name), not storage order.
    pub spellbook: BTreeMap<SpellId, bool>,
```

Initialize `spellbook: BTreeMap::new()` at every construction site (character
creation, plus the `player_at`-style fixtures in mud-core tests — the
compiler will list them).

**Step 3: Schema + save/load.** In `state_db.rs` schema block:

```sql
CREATE TABLE IF NOT EXISTS player_spell (
    name TEXT NOT NULL COLLATE NOCASE,
    spell INTEGER NOT NULL,
    temporary INTEGER NOT NULL CHECK (temporary IN (0, 1)),
    PRIMARY KEY (name, spell)
) STRICT;
```

`save_player`: after the `player_item` delete+reinsert, do the same for
`player_spell` (`DELETE FROM player_spell WHERE name = ?` then insert each
`(name, spell.0, temporary as i64)`). `load_player`: read rows into the map.
`delete_player` (:339) must also purge `player_spell`.

**Step 4: Run** `cargo test 2>&1 | tail -5` — full workspace green (the
compiler-driven fixture updates are done when this passes).

**Step 5: Clippy + commit**

```bash
git add crates/mud-core/src/game.rs crates/mud-server/src/state_db.rs \
        crates/mud-server/tests/state_db.rs
git commit -m "feat: persistent per-character spellbook state"
```

### Task 2: `spells` command

**Files:**
- Modify: `crates/mud-core/src/command.rs` (Command variant + VERBS row)
- Modify: `crates/mud-core/src/game.rs` (dispatch + handler)
- Modify: `crates/mud-core/src/text.rs` (strings/renderer)
- Test: `crates/mud-core/tests/spellbook.rs` (new file)

**Step 1: Extract the exact listing format.** Before writing the test, pull
the verbatim block from the transcript:
`grep -n -A6 "You have the following spells" re/oracle/oracle_spell_train.raw`
— copy column spacing exactly (including trailing padding) into the test's
expected string. §8.5's reading: `Level` right-aligned width ~3, mana,
4-char shortname column, name padded with trailing spaces; verify against
the raw bytes.

**Step 2: Write the failing tests** in new `tests/spellbook.rs` (fixture
pattern: copy the `world()`/`config()`/`create()`/`text_to()` helpers from
`tests/shops.rs`; give the fixture `Content` two spells built with a local
`spell(n)` helper like `tests/content.rs`'s — set `required_power`,
`mana_cost`, `short_name`, `name` so ordering is observable):

```rust
#[test]
fn spells_lists_book_sorted_by_level_then_name() { /* golden block */ }

#[test]
fn empty_book_prints_you_have_no_spells() {
    // "You have no spells."
}

#[test]
fn spells_verb_min_abbreviation() {
    // "sp" resolves to spells (ORACLE-VERIFY: real minimum unmeasured;
    // chosen not to collide with "st"=status / "say")
}
```

Run to confirm compile failure (no `Command::Spells`).

**Step 3: Implement.** `command.rs`: `Spells` variant + VERBS row
`("spells", 2, Verb::Plain)` with an `// ORACLE-VERIFY min abbrev` comment.
`game.rs` dispatch arm → `spells_command(session)`: collect
`player.spellbook` keys, resolve each via `content.spells`, sort by
`(required_power, name)`, render. Renderer in `text.rs` next to the other
VERIFIED blocks, doc-tagged `VERIFIED (oracle §8.5)`.

**Step 4: Green + clippy, commit:**
`feat: spells command — oracle-exact listing, level-then-name order`

### Task 3: Learnability gate

**Files:**
- Modify: `crates/mud-core/src/game.rs` (near `user_can_use`, ~:1548)
- Test: `crates/mud-core/tests/spellbook.rs`

**Step 1: Failing unit tests** — fixture classes: mage
(`caster_group: 1, casting_factor: high`) and warrior (`caster_group: 0`);
fixture spells: same-group low level, same-group high `required_power`,
other-group. Assert the three verdicts.

**Step 2: Implement:**

```rust
/// Why a spell can('t) be learned/used by this character.
/// `user_can_use_spell` gates 1-2 (spellcasting.md §2); the alignment
/// lattice (gate 3) is deferred with M4's other alignment gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellGate {
    Ok,
    /// Wrong magery group, or the class can't ever cast this deep.
    WrongClass,
    /// Right class, character level below spell.required_power (+0xbe).
    /// Oracle-proven level gate (spellcasting.md §8.3).
    TooPowerful,
}

fn spell_gate(&self, player: &Player, spell: &Spell) -> SpellGate {
    let class = &self.content.classes[&player.class];
    if spell.class_gate_group != 0
        && (class.caster_group != spell.class_gate_group
            || class.casting_factor < spell.required_class_level)
    {
        return SpellGate::WrongClass;
    }
    if i32::from(player.level) < i32::from(spell.required_power) {
        return SpellGate::TooPowerful;
    }
    SpellGate::Ok
}
```

**Step 3: Green + clippy, commit:**
`feat: spell learnability gate — magery group + level vs required power`

### Task 4: `use` / `read` verbs + LearnSp handler

**Files:**
- Modify: `crates/mud-core/src/command.rs`, `crates/mud-core/src/game.rs`,
  `crates/mud-core/src/text.rs`
- Test: `crates/mud-core/tests/spellbook.rs`

**Step 0 (oracle, 5 minutes, do FIRST):** one unmeasured branch decides the
handler: using a scroll for an **already-known** spell. Vexil knows magic
missile and the shop sells the scroll free. Drive (per `tools/oracle/`
harness): buy + `use scroll of magic missile`, capture the string and
whether it consumes. Also capture `use dagger` (non-LearnSp item) if quick.
Append both to spellcasting.md §8.4 and commit
(`doc: oracle — use on known-spell scroll and non-LearnSp item`). Implement
whatever it says.

**Step 1: Failing tests:**

```rust
#[test] fn use_scroll_learns_and_consumes() { /* strings from §8.4 */ }
#[test] fn read_scroll_adds_disintegrate_line() { }
#[test] fn too_high_scroll_refused_not_consumed() {
    // "You may not use that item!" + scroll still in inventory + empty book
}
#[test] fn wrong_class_scroll_refused_not_consumed() { }
#[test] fn read_unowned_scroll_prints_description() { }
#[test] fn learned_spell_persists() { /* Event::Persist emitted */ }
#[test] fn already_known_scroll_behaves_like_oracle() { /* from step 0 */ }
```

**Step 2: Implement.** `command.rs`: `Use(String)`, `Read(String)`;
VERBS rows `("use", 2, WithArgs)`, `("read", 2, WithArgs)` — both
`// ORACLE-VERIFY min abbrev` ("u" is the up alias; "re" must not shadow
"remove" min 3 — parse() matches row-name prefixes so `rem` still hits
remove; add a `tests/command.rs` case pinning that).

`game.rs` handler skeleton (shared by both verbs, `epilogue` differs):

```rust
fn use_command(&mut self, session: SessionId, target: &str, read_verb: bool)
    -> Resolution
{
    // 1. inventory lookup via word_prefix_match (drop_command pattern)
    // 2. not owned:
    //    - read_verb: item visible on the shop shelf / floor?
    //      -> print its description paragraph (Resolution::Handled)
    //    - else FallThrough (parser's universal say fallback)
    // 3. owned, has LearnSp(42): value = taught SpellId
    //    - spell_gate != Ok -> "You may not use that item!" (keep item)
    //    - already known -> per Task 4 step 0 oracle result
    //    - else: book.insert(id, false); inventory.remove(pos);
    //      "You read {item} and learn the spell {spell}.";
    //      read_verb also prints "Its magic used, the scroll disintegrates.";
    //      emit Event::Persist
    // 4. owned, no LearnSp: "You may not use that item!"
    //    (charged-item `use` arrives with the item-charges slice; keep this
    //    arm as the obvious extension point)
}
```

**Step 3: Green + clippy, commit:**
`feat: use/read learn LearnSp scrolls — oracle strings, refusal keeps item`

### Task 5: Shop listing suffixes

**Files:**
- Modify: `crates/mud-core/src/text.rs` (TOO_POWERFUL_SUFFIX next to
  CANT_USE_SUFFIX :292-294, update the doc note that promised it)
- Modify: `crates/mud-core/src/game.rs` `list_command` :1638-1668
- Test: `crates/mud-core/tests/spellbook.rs` (or extend
  `tests/use_gates.rs` — it owns the suffix tests today, :160)

**Step 1: Failing test** — fixture shop selling three scrolls to a mage:
same-class castable → no suffix; same-class too-high → ` (Too powerful)`;
other-class → ` (You can't use)`. Mirror §8.3's real rows in shape.

**Step 2: Implement.** In the row loop: if the item carries LearnSp, suffix
by `spell_gate` on the taught spell (`WrongClass` → CANT_USE, `TooPowerful`
→ TOO_POWERFUL); else the existing `user_can_use` path unchanged.

**Step 3: Green + clippy, commit:**
`feat: shop rows annotate scrolls — (Too powerful) / (You can't use) by spell gate`

**SLICE 2 GATE:** full workspace green, clippy clean, then a live check:
`cargo run -p mud-server`, telnet in, roll a mage, buy + learn a scroll,
`spells`. Compare against §8 strings by eye before starting slice 3.

---

## SLICE 3 — Cast skeleton

### Task 6: Oracle top-up (unmeasured cast edges)

Drive Vexil (or a fresh caster) per `tools/oracle/`; append to
spellcasting.md §8 and commit `doc: oracle — cast edge cases`.

Capture: (1) bare `cast` with no argument; (2) abbreviation behavior —
`c mm`, `c magic`, `c magic mi` (is it shortname-prefix, name word-prefix,
both?); (3) offensive cast bare in a room WITH a live monster (auto-pick
like `attack`?); (4) offensive cast in a truly empty room (no NPC — expect
the "no effect" family vs the guilt line); (5) `cast blur extra words`
(trailing-garbage tolerance). Each answer feeds Task 7/11 directly.

### Task 7: Cast parsing + book resolution

**Files:** `command.rs` (+ `Cast(String)` variant; ALIASES row `("c", ...)`
— note ALIASES are exact-match shortcuts, so bare `c` and `c args` both must
route to Cast; VERBS row `("cast", 2, WithArgs)` ORACLE-VERIFY min),
`game.rs` dispatch + `resolve_spell_from_book`, tests in new
`tests/cast.rs`.

Resolution (MEASURED, §8.9): candidates = learned book only. A spell name
matches on **exact shortname OR per-word name prefix** (`c m`, `c magic`,
`c magic mi` all hit magic missile; `c mm`/`c mmi` MISS — shortname prefixes
do not match). Words consumed by the name match don't become the target.
Miss → `You do not know how to cast {arg}.` — **Handled, never
say-fallthrough**. Bare `cast`/`c` → `Syntax: CAST {spell} [{target}]`.
After the spell resolves, the ENTIRE remaining input is the target string —
trailing garbage is not tolerated (`cast blur extra trailing words` →
`You do not see extra trailing words here!`, no self-cast, no mana).

Tests: resolution hits/misses (`mmis`, full name, `mami` miss), unknown
echo, bare form.
Commit: `feat: cast command parsing — book-only resolution, oracle miss string`

### Task 8: Cast gates (order is behavior)

**Files:** `game.rs` (`Session::InGame` gains `cast_this_round: bool`,
cleared in `energy_round` :2177; handler `cast_command`), `text.rs`
(strings), `tests/cast.rs`.

Gate order for slice 3 (spec §3 order, restricted to what exists —
confusion/fear/MageBind arrive with their abilities in later slices; leave
one comment naming each future gate in spec order):

1. Mortally-wounded / dead gating — reuse M3's command gating.
2. `cast_this_round` → `You have already cast a spell this round!`
3. Resolve spell (Task 7).
4. Level: `player.level < spell.required_power` → `This spell is too
   powerful for you.` (unreachable via scroll-learned books; reachable in
   slice 4+ via temp spells — gate stays, spec §2.)
5. Round energy: `session.energy < spell.round_cost` → treat exactly like
   an M3 attack without energy (queued/no-op within the round loop — match
   `attack`'s behavior; do NOT invent a message, none was measured).
6. Mana: `player.current_mana < mana_cost` → `You do not have enough mana
   to cast that spell.`

Passing all gates sets `cast_this_round = true` **whether the roll then
succeeds or fails** (measured: a failed blur still blocked the round).

Tests: each gate's message, gate ordering (e.g. already-cast beats no-mana),
`cast_this_round` resets after `energy_round`.
Commit: `feat: cast gates — one per round, mana, energy, level, spec order`

### Task 9: Success roll + costs

**Files:** `game.rs`, `tests/cast.rs`. Injectable-roll seam: the M3 combat
pattern (`calculate_attack` takes injectable rolls) — same approach.

- `chance = min(SC + base_chance, 98)` where SC is the Spellcasting derived
  stat (check `stats.rs::Derived` for the field name; the stat sheet prints
  it, so it exists). `base_chance >= 200` skips the roll.
- Success: deduct full `mana_cost` + full `round_cost`.
- Failure: `You attempt to cast {name}, but fail.` + room broadcast (check
  §8/transcripts for the observer line; if unmeasured, ORACLE-VERIFY note
  and emit caster-only) — deduct full round cost, **half mana rounded down**
  (mmis mana 1 → 0 deducted, oracle-confirmed).

Tests (seeded rolls): success/fail branches, exact mana/energy deltas
including the floor(1/2)=0 case, auto-succeed at 200.
Commit: `feat: cast resolution roll — min(SC+base,98), half-mana fail`

### Task 10: Cast-message renderer

**Files:** `text.rs` (or a small `fn render_cast_msg` in game.rs), tests.

Render `castmsgb`'s three lines to the three audiences (caster / target /
room-others), substituting in order of appearance: spell name, target name,
damage (`%d` or `%s` — msg 3242 line3 uses `%s` for the number; format
integers for both). Spells with `castmsga != 1`: log/flag, render castmsgb
only (no sampled spell differs). Target-less messages (msg 2) simply consume
fewer args. Tests: golden lines for msgs 3242/2/7 shapes using fixture
messages.
Commit: `feat: cast message renderer — 3-audience castmsgb substitution`

### Task 11: Offensive cast → damage, engagement, kill

**Files:** `game.rs` (cast handler offensive branch), `tests/cast.rs` +
`tests/game_combat.rs` patterns.

- Target selection (MEASURED, §8.9 — the auto-pick assumption was WRONG):
  explicit arg → monster by `word_prefix_match` (M3 targeting); bare
  offensive cast NEVER auto-picks — friendly NPC present → the guilt line;
  empty room or monsters-only → `You must specify a target for that
  spell!`. Bare offensive cast while melee-ENGAGED prints `*Combat Off*`
  (breaks the engagement) and then the must-specify refusal.
- Engaged repeat: an offensive cast that engages combat auto-repeats every
  combat round like `attack` (measured: unprompted mmis line the round
  after a failed roll).
- Engagement: entering combat emits `*Combat Engaged*` and cast damage joins
  the M3 combat frame (`c mmis filthbug` engaged + fired in one round —
  reuse the attack-command engagement path).
- Magnitude: pinned formula above; L = min(level, level_cap); genrdn
  semantics reconciled with observed 4..13 mmis range.
- Resist: `Element::resist_ability()` value from the monster's AbilityBag,
  `V' = (100-resist)*V/100`; element Magic bypasses (mmis full damage).
- Save class: `spell.save_class != SaveClass::None` gives the monster its
  save (spec §3: AntiMagic for IfAntiMagic, roll vs SC/2 capped 98) — the
  starter mage spells are all `None`, so implement the structure with
  fixture tests and an ORACLE-VERIFY tag on the monster-side stat used
  (decompile cite: cast_monster_target 43165+, the `+0xc2`-halved roll).
- Kill: route through M3's monster-death path (damage → death lines →
  exp split `You gain N experience.` → `*Combat Off*`) — the oracle kill
  transcript already matches M3's existing strings.

Tests (seeded): damage within 4..13 for a L1 mmis fixture, resist scaling,
engagement lines, kill flow, guilt line.
Commit: `feat: offensive casts — magnitude, resist, engagement, M3 kill path`

### Task 12: Benign instant handlers

**Files:** `game.rs` apply-effects for instant (`duration == 0`) spells,
`tests/cast.rs`.

Handler set (spec §4 instant table, single-target only): Damage 1 (done in
Task 11), Heal 18 (cap at max HP), Drain 8 (kill-checked, caster capped),
EnergyLevel 11 (round pool, cap), Alterhunger 15 / Alterthirst 16. Fixture
spells per handler (base_chance 200 for determinism). Duration spells
(blur/illuminate!) must NOT apply — emit nothing beyond the cast message and
leave a `// slice 4: duration slots` marker; add a test asserting a
duration-spell cast does not change stats (it will start failing loudly in
slice 4, which is the point).

Commit: `feat: instant spell effects — heal, drain, energy, hunger/thirst`

### Task 13: End-to-end + verification sweep

1. Golden scenario test: mage learns mmis from fixture scroll, casts at a
   fixture monster with seeded rolls, transcript pinned (creation→kill).
2. `cargo test` + clippy, zero warnings.
3. Live hand-test: telnet to our server — roll mage, buy scroll, learn,
   `spells`, kill something with mmis. Compare side-by-side with Vexil doing
   the same in MBBSEmu.
4. Sweep the new code for ORACLE-VERIFY tags; resolve what Vexil can answer
   cheaply now, list the rest in the commit message.
5. Commit any calibration fixes; update
   `docs/plans/2026-07-16-m5-magic-design.md` slice status.

**CHECKPOINT — STOP HERE.** Review with Daniel; slice 4 (duration engine)
gets planned next — blur is already learnable and visibly does nothing,
which is the first thing slice 4 fixes.
