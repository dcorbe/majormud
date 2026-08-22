# MudPlay: history-free room localization

Port the candidate-set localizer from `crates/mud-client/src/lost.rs` into
MudPlay (`archive/FujiTerm/MudPlay`, upstream `Tehshortbus/MudPlay`) so a
character who does not know where it is can find out by walking, without
needing a movement history to un-walk.

## The defect

MudPlay's recovery ladder is real — three tiers plus a Paradigm `rm` fast
path — but it is **retrospective** and **engine-scoped**. Both properties
were read off the code, not inferred:

1. `EngineRecoveryGate` guards every entry point with
   `if (_engine is null) return;`. With no walker, loop-runner or auto-lair
   attached, the gate is inert.
2. Its only history feeder is `NoteEngineStepSent`
   (`Game/Map/EngineRecoveryGate.cs:237`), called when *the engine* sends a
   move.
3. `PartyFollowerMovementGate` holds the engines for as long as the player
   is a non-leader follower, so `_executedSinceAnchor` stays empty
   throughout a party.
4. Tier-3 recovery walks that list backwards. With it empty,
   `AdvanceReverseWalk` takes the `Count == 0` branch and calls
   `FailTier3("backtrack exhausted to anchor without convergence")`
   (`EngineRecoveryGate.cs:640-650`) — terminal failure, **zero moves
   sent**.

The user-visible result: leave a party, and the client reports itself lost
having tried nothing. It is not passive by design; it is structurally out
of input, because leaving a party is precisely the case with no moves of
its own to reverse.

Footsteps *are* tracked. `NoteMoveSentCore` calls `AppendStep` for follow
drags too, and even from `Unknown`/`Suspect`/`Lost`
(`Game/Map/RoomTracker.cs:322-333`), so `RoomTracker._recentSteps` holds
them. The history exists; the recovery ladder simply cannot see it, because
it lives in a different object.

## What is missing

`FootprintMatcher` already does candidate-set narrowing, and its
`Step(Direction, RoomObservation)` is pure — it does not care who sent the
move. `RoomGraphManager.FindCandidates` already seeds a candidate set from
one observation. The three genuinely absent pieces are:

- a **forward** direction choice that maximizes information gain, rather
  than reverse-of-history;
- a **cold-start** driver needing no anchor and no history;
- **availability without an attached engine**.

## Non-goals

- **A listed-exit index.** `Room.ExitMask` sets a bit for every parsed
  exit (`RoomGraphManager.cs:1216`) without consulting `RoomExitHint`, so
  the 895 rooms carrying a hidden or command exit can never match the
  exact `(Name, ExitMask)` bucket. Fixing that was measured and rejected:
  see Measurements. The locator seeds from the existing
  `FindCandidates` ∪ `FindByNameCoveringExits`, which already reaches those
  rooms through the superset path. No new index, no blast radius on the
  promotion path every localization depends on.
- **`look <dir>` sweeps while being dragged.** MudPlay's
  `RecoveryLookSweep` is move-free and would be free evidence, but sending
  anything while a leader drives the party is out of scope by decision.
  Dead reckoning only.
- **Inventing party mechanics.** See Assumptions.

## Measurements

Against `re/mmud_wgnt.sqlite`, 26,580 named rooms carrying 1,753 distinct
names. Reproduced in this session:

| | rooms pinned from one room block |
|---|---|
| MudPlay today (all-exit index, queried with a listed mask) | 4,695 — 17.7% |
| with a listed-exit index | 4,973 — 18.7% |

The listed-exit index gains **+278 rooms, about one percentage point** —
collapsing hidden exits also merges rooms that were previously distinct.
That does not justify touching `FindCandidates`, hence the non-goal.

The 18.7% figure reproduces `lost.rs`'s documented value exactly, which is
what makes the query trustworthy.

`lost.rs` further documents a simulated resolution curve — 18.7% at zero
steps, 52.0% at one, 71.4% at two, 82.4% at three, 93.3% at twelve — and a
*patient* versus *impatient* splitting rule at 93.3% versus 87.7%. **Only
the zero-step point has been independently reproduced here.** The rest are
carried from that module's own simulation. The port's test suite
re-derives the curve against MudPlay's graph, which validates the claim and
the port at once (see Testing).

## Design

### 1. `Game/Map/RoomLocator.cs` — the pure core

Stateless. Sends nothing, owns no observable state, so the `[Owner]`
IL-scan invariant is untouched.

- `Seed(RoomObservation)` → candidate list. Exact `FindCandidates` first;
  `FindByNameCoveringExits` when exact admits nothing. Widening rather than
  answering "nowhere" about a character that is plainly somewhere.
- `ChooseSplittingExit(candidates, here)` → `Direction?`. Among the exits
  the observation lists, the one whose destinations across all candidates
  fall into the most distinct `(Name, ExitMask)` shapes. Every candidate
  must have that exit with a known destination — otherwise taking it is a
  guess about the very question being asked. Ties break in fixed compass
  order so a walk is reproducible. Returns the best exit **even when it
  splits nothing this step**: a step that teaches nothing today still
  carries the whole set forward to a room where the neighbours differ.
- Narrowing is `FootprintMatcher.Step`. Not reimplemented.

Outcome type distinguishes converged, ambiguous-with-a-count, and
no-such-room. The count matters: "one of 212" and "one of 2" are different
situations, and the existing lost dialog currently reports neither.

### 2. The walk loop — one implementation, two senders

The forward walk is a single helper taking a `send` delegate and a
`FootprintMatcher`: ask the locator for a splitting exit, send it, feed the
landing into `Step`, repeat to budget. It never names an engine.

Driver A passes `engine.SendBacktrackMove`. Driver B passes the
`EngineSendGate`-wrapped wire sender that `RecoveryLookSweep` already uses.
This is why the design does not need a null-object `IRecoverableEngine`:
the walk depends on *sending a direction*, not on an engine. Tests pass a
fake sender and assert the emitted directions.

### 3. Driver A — forward tier in `EngineRecoveryGate`

At the two points that currently give up — `AdvanceReverseWalk`'s
empty-history branch, and tier-3 exhaustion — switch from reverse-walk to
forward splitting-walk, driving the shared walk loop. Budget 12, the
measured knee.

- Converged → `FinishTier3Success`.
- Budget exhausted, n > 1 → fail, but with the number, so the dialog says
  how many rooms the character is standing in one of.
- Candidates empty → fail: the loaded world is not the world the character
  is in, and walking cannot fix that.

### 4. Driver B — move-free, engine-less

A small coordinator wired in `AppServices`, subscribing to
`RoomTracker.StateChanged`. On `Suspect`/`Lost` with no engine attached:
replay `RoomTracker`'s own recent steps forward through a
`FootprintMatcher` seeded by the locator. Pure computation, zero bytes.
Because follow-drags already land in `_recentSteps`, this is the party
case.

Walking — Driver A's algorithm reached without an engine — is offered only
when the follower gate is clear **and** the user has opted in.

### 5. Party safety

The one real behavioral risk is walking a follower out of formation.

- Never send a move while `IsInParty && !SelfIsLeader`.
- Guard on `MovementCoordinator`'s follower gate rather than re-deriving
  the predicate, so there is one authority for "am I being driven".
- Opt-in setting gates walking even when the gate is clear.

On leaving a party the footprint has been narrowing throughout, so the
ladder resumes better localized than it started — the direct answer to
"as soon as I leave a party, I'm instantly lost".

## Assumptions (unverified)

**Hidden and foliage party drags may print no hookable line.** MudPlay's
`GAME_MECHANICS.md` flags this as a known gap: the follower's room changes
with nothing to feed `NoteMoveSent`. Whether the game prints *anything* on
such a drag is domain truth and has not been confirmed, so it is recorded
here rather than guessed.

The design does not rest on the answer:

- If drags print a hookable line, footprint replay keeps a follower located
  through the whole party at zero cost.
- If some drags are silent, the follower goes ambiguous during the party as
  it does today, and the history-free forward walk pins it on leaving.

The answer changes how often the free path applies, not whether the defect
is fixed. Confirm from a Bug Report capture's Scrollback section (raw
wire, 750 lines, `Services/BugReportBuilder.cs:1158`) taken after a drag
through a hidden exit.

## Definition of Done surfaces

MudPlay's `CLAUDE.md` requires each of these per PR:

- `MudPlay.csproj` `<Version>` — MINOR bump (new capability).
- `CHANGELOG.md` — new top `## <version>` section, terse bullets.
- `README.md` current-version block mirroring it.
- Settings surface: opt-in walk-when-lost, budget.
- `LogService` Info lines for locator decisions; Debug for per-candidate
  drops.
- `BugReportBuilder` fields for locator state.
- Help guide (`Assets/Help/guide.md`) entry.
- Zero-warning build (`TreatWarningsAsErrors`).

## Testing

xUnit in `MudPlay.Tests`, following its stated philosophy: test where
compile-time cannot catch it.

- `RoomLocator` pure unit tests — seeding, exact-then-superset fallback,
  splitting choice, tie determinism, the patient rule, unusable-direction
  handling.
- Curve re-derivation over MudPlay's own loaded graph, asserting the
  documented resolution rates. Validates both the port and the claim.
- Driver A: empty-history entry converges instead of failing.
- Driver B: footprint replay narrows with zero sends.
- Party guard: no move is emitted while the follower gate is asserted.

Per project preference, mutate before trusting: invert the splitting
choice and confirm a test fails; break the party guard and confirm the
no-walk test catches it. A test that cannot fail is not evidence.

## PR shape

Feature-scale, so its own PR per MudPlay's cadence rule. Branch
`pathfinding-fixes` on the `dcorbe/MudPlay` fork, PR to
`Tehshortbus/MudPlay`. This document stays in `~/bbs` and is not
referenced from commit messages.
