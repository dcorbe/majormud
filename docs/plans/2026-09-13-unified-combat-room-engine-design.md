# Unified combat-and-room engine (2026-09-13)

## Problem

Combat has been patched four times in quick succession, each for a new
way the board can end or continue a fight:

- a self-cast `*Combat Off*` read as the target leaving (`d31efc6`),
- the backstab second-round verb never re-sent (`ab61c75`),
- a target-switch `*Combat Off*` / `*Combat Engaged*` pair read as a
  fight ending, which spun the cw-beef ping-pong (`ac231e6`),
- and before those, the mystic redirect and the wander-out cooldown.

The shape is always the same. The bot infers combat state from a noisy
event stream one line at a time, and `*Combat Off*` alone carries at
least five meanings: a kill, a wander-out, a self-cast ending, a target
switch, and the backstab mode revert. Each meaning has its own guard
(`is_kill_line`, `cooling`, `disengaged_by_own_cast`, `switch_watch`,
`backstab_open`), plus `quiet_prompts` as a catch-all. `engaged` is
written in about ten places across three functions.

Underneath that is a duplication. Three separate structures track
overlapping combat and room state:

- **`Bot`** — `engaged` and its qualifiers, line-guessed and
  attribution-blind.
- **`Here`** (the farm's room model) — the living occupants and the coin
  piles, attributed and folded per event. It already knows whether
  monsters are alive: it removes a corpse on a kill line, a leaver on a
  departure, and adds an arrival, all without a re-look.
- **`StopState`** — the dwell and respawn budget.

`engaged` and `Here`'s occupant list are two answers to nearly the same
question — "is there a live monster to fight here" — kept in different
places, fed by different inputs, free to disagree. Every combat fix has
patched the line-guessing side. The attributed side is better and
already exists, but only on the farm: the assist (what a party follower
and manual `/bot` run) has no `Here` at all, which is why the cw-beef
ping-pong happened in a follower.

## Goals

- One in-room engine that owns combat state, the living-occupant model,
  sweeping, and stealth, with a single answer to "am I fighting", "what
  is alive here", and "what is on the floor".
- The assist gains that engine, so followers and manual play stop
  line-guessing.
- The farm becomes a navigation-and-scheduling consumer of the engine
  rather than a second copy of it.
- `finish_at` becomes a real in-session feature instead of the
  headless-only one it was until the `mmc farm` removal.
- The replay harness is extended to cover wrapper loops, not just the
  bot core.

## Non-goals

- No change to the parser, the correlator, or the navigator's routing.
- No new combat mechanics. This is a consolidation of state tracking and
  a relocation of ownership, not a behavioural change the operator would
  notice beyond fewer loops.
- No change to the `[farm]`, `[bot]`, or `[bank]` config surface, except
  wiring `finish_at` into the in-session path.

## Current architecture

```
mmc play window
  ├─ assist:  Bot (engaged + qualifiers)          -- no room model
  └─ /farm job: run_farm
        ├─ Bot (engaged + qualifiers)             -- attribution-blind core
        ├─ Here (living occupants, piles)         -- attributed room model
        └─ StopState (dwell / respawn budget)     -- when to walk on
```

The pump in `run_farm` feeds each event to all three. `Bot` decides the
next command; `Here` maintains occupancy; `StopState.verdict` decides
whether to look, loot, wait, or leave. The farm deliberately turns the
bot's own sweeping off (`stop_config.auto_get = false`) and sweeps
through `Here` and `Verdict::Loot` instead — a duplication that exists, a
code comment says, only because the assist's bot "has no `Here` at all".

## Target architecture

```
mmc play window
  ├─ assist:  Engine                              -- room-aware in-room engine
  └─ /farm job: run_farm
        ├─ Engine  (combat state + RoomModel + sweep + stealth)
        └─ Schedule (dwell / respawn budget + walk + finish)  -- reads the Engine
```

One `Engine` owns everything that happens *inside* a room. The farm owns
everything *between* rooms: which stop is next, when the current room is
finished, the respawn-and-dwell budget, the walk, and the finish. The
farm asks the Engine "is this room done" instead of maintaining its own
occupancy.

### Component: `RoomModel`

Generalise today's `Here` into the engine's room model, owned by the
Engine and used by both the assist and the farm.

- **What it does:** maintains the living occupants and the coin piles
  from the attributed event stream, removing corpses and leavers and
  adding arrivals without a re-look. Answers `aggressive_names`,
  `has_target`, `unswept_wanted`, and `is_clear` (nothing this policy
  would fight and nothing it would loot).
- **How it is used:** the assist and the farm both fold their event
  stream into it. The farm's `Verdict` reads `is_clear` to decide the
  room is finished; the assist reads it to decide there is nothing to do.
- **What it depends on:** the correlated event stream and the bot's
  policy (what counts as attackable, what coins are wanted).

`Here`'s existing reconcile-and-divergence counters stay — they are the
evidence that the model has not lost occupants, and they earned it the
right to replace the re-look in the first place.

### Component: `CombatState`

Replace the scattered `engaged` / `cooling` / `switch_watch` /
`quiet_prompts` with one state machine, and let it lean on `RoomModel`
rather than re-derive occupancy from lines.

- **States:** `Idle`, `Engaging { target }`, `Fighting { target }`,
  `Ending { pending }`. Each board signal is one transition, tested on
  its own.
- **Inputs:** a small classified `CombatSignal` (fight-started,
  fight-ended, target-switch, our-blow-landed, kill, refusal) rather
  than raw lines. The target-switch that today needs `switch_watch` is a
  first-class signal here, not a lookahead flag.
- **What it replaces:** the ten `engaged =` sites collapse to the
  transitions of one type. `cooling` stays as a property of `Ending`
  (a wander-out is an end whose target must not be re-engaged for a
  couple of blocks), but it is one place, not a field threaded through
  `decide`, `on_line`, and the RoomSeen arm.

### Component: the Engine's query surface

The farm consumes the Engine through a small, stable interface:

- `is_fighting() -> bool` — a fight is in progress; do not rest, do not
  walk on.
- `is_clear(room) -> bool` — nothing to fight and nothing to loot; the
  room is finished as far as the Engine is concerned.
- `next_actions(event) -> Vec<Command>` — the commands the Engine wants
  sent for this event (engage, sweep, heal, hold stealth).

The farm never reads `engaged` or an occupant list directly again; it
asks these three questions.

### Component: the farm as a consumer

`run_farm` keeps the walk, the leg interrupts, the party hooks, and the
schedule. It drops:

- its own `Here` (the Engine owns the room model now),
- its stop-owned sweep (`stop_config.auto_get = false` and
  `Verdict::Loot` go away; the Engine sweeps, the farm reads `is_clear`),
- the occupancy half of `StopState` (`has_target_among`, the
  `note_occupancy` budget input), which becomes "ask the Engine".

`StopState` shrinks to the part that is genuinely the farm's: the dwell
timer, the respawn budget, and the pending-look bookkeeping that bounds
"the room went quiet, wait `recheck`, then look again". These are
scheduling concerns the Engine has no stake in.

### The assist gains the Engine

The assist becomes the Engine plus the party state, with no second
implementation of combat or sweeping. This is the change that fixes the
side where the loops actually happened. Under `/bot` the Engine runs;
with it off, nothing does.

## The one hard part: the attribution seam

The farm's room model is **attributed**: it trusts a block only when the
block answers its own `look`, and it knows when a block describes the
room next door (`cor.elsewhere`, the `look <direction>` case). The assist
watches a stream it does not fully drive: it is dragged by a party
leader and has to handle "the room changed under me".

So the `RoomModel` must work in both modes. The design keeps attribution
as an input to the fold, not a property of the owner: the pump tells the
model whether a block is attributed and whether it is elsewhere, exactly
as `run_farm` does today, and the assist supplies the same two facts from
its own context (a dragged move is attributed to the drag; a
`look <direction>` is elsewhere). The model's rules do not change; only
who feeds it does. This is the seam most likely to leak bugs, so it is
the first thing the extended harness must cover.

## `finish_at` wired in-session

With `mmc farm` gone, `go_to_finish` and `finish_at` have no production
caller. Rather than delete a useful feature, wire it into the in-session
path: when a `/farm` job ends (any exit but a death), the window walks
the character to `finish_at` using the existing `go_to_finish`. This
turns `finish_at` from headless-only into the feature the docs already
describe, and it belongs in this work because the window's job-completion
handling is exactly the farm-as-consumer boundary being drawn.

## Harness extension

The replay harness (`crate::replay`, built first) replays through the
bot core today and catches bot-core loops like the ping-pong. Extend it
to replay through the assist path (`assist_actions`) and the farm verdict
so it also catches wrapper loops — the assist's `look` on every Combat
Off, and the farm's `Ask` spam that spun cw-blueberry. The unified
Engine makes this easier: one thing to drive, one thing to watch.

## Testing

- The existing bot, farm, and world suites stay green throughout; each
  migration step is behaviour-preserving and proven so before the next.
- Per-transition `CombatState` tests, one board signal each.
- The replay harness fixtures for the ping-pong and the look-spam, run
  through the Engine and (extended) through the assist and farm wrappers.
- `Here`'s reconcile-and-divergence counters guard the `RoomModel`
  migration: if the folded model ever loses an occupant the counters
  catch it, the same way they earned `Here` the right to replace the
  re-look.
- Mutate-before-trust: each safety property (a lone Combat Off still
  un-latches, a wander-out still cools, a switch still holds) is proven
  by breaking it and watching the test fail, as the target-switch fix
  was.

## Migration plan

Incremental, each step shippable and green:

1. **Extract `CombatState`** inside `Bot`, behaviour-preserving. Collapse
   the ten `engaged =` sites and the four qualifiers into one state
   machine with the same outputs. No consumer changes.
2. **Lift `RoomModel`** out of the farm into a shared module the Engine
   owns. The farm keeps feeding it; nothing else changes yet.
3. **Give the assist the `RoomModel`.** Followers and manual `/bot` gain
   room awareness. This is the change with operator-visible value and the
   attribution seam to prove.
4. **Introduce the Engine query surface** (`is_fighting`, `is_clear`,
   `next_actions`) and move the farm's `Verdict` to consume it. Delete
   the farm's stop-owned sweep and the occupancy half of `StopState`.
5. **Wire `finish_at` in-session.**
6. **Extend the harness** to the assist and farm wrappers.

Steps 1 and 2 are pure consolidation. Step 3 is the one that changes
behaviour (the assist gets smarter). Step 4 is the one that deletes the
duplication. They can land as separate commits with the suite green
between each.

## Risks and open questions

- **`farm.rs` is large and intricate**, and `StopState.verdict` has many
  hard-won clauses (flee recovery, blind/light ordering, the pending-look
  and pending-get windows). Moving occupancy out without disturbing those
  is the main risk. Mitigation: the harness first, and step-by-step
  migration with the suite green.
- **The attribution seam** (self-driven vs dragged) is where a shared
  `RoomModel` is most likely to misbehave. Mitigation: the divergence
  counters, and harness fixtures for a dragged follower.
- **Open: does the assist need the respawn budget?** The farm dwells and
  walks on; the assist just sits. Likely the budget stays farm-only and
  the assist simply acts when the Engine says there is work. To confirm
  in step 3.
- **Open: `cooling` under the Engine.** Whether the wander-out cooldown
  belongs in `CombatState.Ending` or in the `RoomModel` (as a property of
  an occupant that is leaving) is a step-1-versus-step-2 boundary
  question. Decide when step 1 lands.

