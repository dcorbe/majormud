# The client knows what it carries — Design

**Status:** accepted 2026-08-22.
**Depends on:** `2026-08-22-one-path-to-content-design.md` (`Content.items` must be
loaded client-side before the item-identity layer can exist).

## Goal

Give the client a model of what the character carries and what is in each
equipment slot, resolved against the item data it already loads, so it can open a
fight with a backstab when it has the weapon for it.

## Origin

Daniel, 2026-08-22: *"If the user has a weapon in their inventory that can be
used to BS (which is also a property in the data), they should be doing BS as
long as they enter a room while still sneaking. That's why I opened this session
with the question, should we have an entire subsystem dedicated to inventory."*
Then: *"if we have a backstab weapon in inventory, we need to switch to it BEFORE
we begin the attack. Backstab is a once only thing, so after the backstab round,
we need to switch back to our primary weapon"*, and *"there are also weapons that
can have a dual purpose as primary and backstab tool… so we do need to check the
wielded weapon too."*

## Established facts

Verified in this session against the code and the WG3-NT binary, not assumed.

**The backstab gate is on the WIELDED weapon** (`mud-core/src/game.rs:6267`):

```rust
let stealthy = player.hidden || player.sneak_armed;
match player.weapon {
    None => AttackType::Backstab,      // unarmed backstabs successfully
    Some((id, _)) => { /* item must carry ability 0x74 (BSAccu) */ }
}
```

- A carried-but-unwielded backstab weapon does nothing.
- A wielded weapon without `0x74` prints `"You cannot backstab with this
  weapon!"` and degrades to a normal attack — the opening round is spent.
- **Unarmed backstabs.** A wielded non-BS weapon is strictly worse than no weapon
  for the opener.
- Either `hidden` or `sneak_armed` satisfies the stealth half.

**Sneak is consumed by one move.** Byte-verified against
`re/wg_nt_ref/WCCNT8PJ/out/wccmmud.dll`: `cmd_sneak` sets bit 4; the move clears
it unconditionally; and a *second* independent stealth roll at move time can fail
and drop the sneak silently. The client must therefore re-arm before **every**
step, and may never treat an arm as a guarantee of arrival unseen.

**Equipment facts** (MudPlay `GAME_MECHANICS.md` §Equipment & gear, `[CONFIRMED]`):

- `eq <item>` is universal; `wield` weapons, `wear` armour, `rem` removes.
- **Trade-places**: equipping into an occupied slot swaps, and the displaced item
  returns to the pack. No `rem` first. One command each way.
- A weapon swap prints exactly one line, `You are now holding <new>.`, and
  **names no displaced item**. The client must remember what it took off.
- **Nothing in the game force-unequips gear.** Worn state changes only from
  commands the player or client issues.
- Equipping breaks sneak.

## Architecture — three layers, different truth models

The central insight: equipment and contents are *not* the same kind of knowledge.

| layer | truth model | source |
|---|---|---|
| **Equipment** | authoritative; deterministic given our own command history | our commands, confirmed by `You are now holding <new>.` |
| **Contents** | best-effort; allowed to be stale | refreshed from `i` |
| **Item identity** | `Option` — never a guess | name → `Content.items` → ability arrays |

Equipment can be modelled with confidence *because* the game never force-unequips.
Contents cannot: loot, sales and consumables drift them constantly. Conflating
the two is what makes the current `sheet::Inventory` (`Vec<String>` plus
encumbrance) unable to answer anything useful.

### Item identity resolution

The board prints display strings; `Content.items` has 1950 rows. Resolution
handles articles, plurals and quantities, and **returns `Option`**. A miss means
*"I do not know"* and suppresses the optimisation. It must never resolve to a
*wrong* row — confidently wielding a weapon that cannot backstab is worse than
not trying, because it costs the opening round and is invisible until the refusal
line arrives.

## The backstab decision

Evaluated **before** arming sneak, because equipping breaks it:

| wielded weapon | action |
|---|---|
| carries `0x74` (dual-purpose) | no swap; sneak → move → `bs`; stay wielded |
| lacks `0x74`, BS weapon in pack | `eq <bs>` → sneak → move → `bs` → `eq <primary>` after the opening round |
| lacks `0x74`, none carried | **do nothing.** Disarming to enable an unarmed backstab is deliberately NOT implemented — it trades the whole fight's damage for one opener. Revisit only on request. |
| identity unresolved | do nothing; fall through to a normal opening attack |

Backstab lands only on the opening round, so there is no second chance in a room.

The swap-back is safe: by then the character is in combat and the sneak is
already spent.

## Failure modes and their cost

Stated because they set how aggressive the feature may be:

- `bs` while **not** actually stealthy → a silent plain attack. Cheap. This
  matters because the per-move re-roll means the client can never be certain it
  arrived sneaking.
- `bs` with the **wrong weapon** wielded → refusal line, opening round wasted.
  This is the expensive one, and the reason identity resolution must not guess.
- Swap issued but refused (class/level/slot) → the wire says so; the client must
  not assume its own command succeeded.

## Testing

Per Daniel's instruction for this session, **targeted test binaries only** — run
`cargo test -p mud-client --test <name>`, not the full suite. Accepted tradeoff:
regressions elsewhere may be found later.

- The decision table is a pure function of (wielded item, carried items,
  stealth state) and is tested exhaustively offline — one case per row, plus the
  unresolved-identity case.
- Equipment-model tests drive command/echo sequences, including a swap whose
  displaced item is never named, proving the client tracked it rather than read it.
- Identity resolution is tested against real `Content.items` rows for both a hit
  and a deliberate near-miss that must return `None`.
- Mutation is mandatory on the decision table: flip the dual-purpose row to swap
  anyway and confirm a test fails.
- Do not connect to a live board.

## Risks

- **Name resolution is the whole risk.** Everything else degrades gracefully; a
  wrong identity costs the opening round silently.
- **Contents staleness** is accepted by design. The equipment layer, which the
  decision actually depends on, is not stale.
- **The per-move re-roll** means arrival-unseen is never certain. The design
  tolerates this by making a mistaken `bs` cheap.

## Not in scope

- Auto-hide (a distinct stealth state with its own party hazards).
- Disarming to enable an unarmed backstab.
- Encumbrance, item level requirements, and the Witchunter anti-magic rule —
  all future consumers of this same join.
- Auto-sneak policy itself; this design assumes stealth state is available.

## Sequencing

Blocked on `2026-08-22-session-knows-character` landing (it is editing
`Capabilities` in `graph.rs`) and on the content design providing
`Content.items` client-side.
