# MudPlay Room Locator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give MudPlay a history-free room localizer so a lost character walks to find out where it is, instead of failing with zero moves sent.

**Architecture:** A pure `RoomLocator` (seed a candidate set from one room display; choose the exit that splits it furthest) plus a `LocatorWalk` pump that sends a direction and folds each landing into MudPlay's existing `FootprintMatcher`. Two drivers consume the pump: `EngineRecoveryGate` where it currently gives up, and a new engine-less coordinator that does move-free dead reckoning while no walker is attached.

**Tech Stack:** C# 13 / .NET 10 / Avalonia 12, xUnit, CommunityToolkit.Mvvm source generators. No DI container.

**Spec:** `docs/superpowers/specs/2026-08-22-mudplay-room-locator-design.md` (in `~/bbs`; the MudPlay working tree is `~/bbs/archive/FujiTerm/MudPlay`)

## Global Constraints

- **Working tree:** `/home/daniel/bbs/archive/FujiTerm/MudPlay`, branch `pathfinding-fixes` (already created off `main` at `d6bfdcb`). This repo has its own `.git` and its own remotes: `origin` = `Tehshortbus/MudPlay`, `fork` = `dcorbe/MudPlay`. Push to `fork`.
- **Zero-warning build.** `TreatWarningsAsErrors=true` and `EnforceCodeStyleInBuild=true`. A build that emits a warning is a failed build. Fix it; do not suppress it.
- **Build/test:** `dotnet build` and `dotnet test` from the working tree. Do not pass `--framework`, `--runtime`, or `-c`.
- **No DI container.** `Services/AppServices.cs` is a POCO holder that constructs services and exposes them as properties.
- **Single-writer invariant.** Every `[ObservableProperty]` field carries `[Owner(typeof(...))]`, enforced by a Mono.Cecil IL-scan test. **This plan adds no observable properties** — keep it that way.
- **Style:** file-scoped namespaces, `sealed` by default, `_camelCase` private fields, no `#region`, plain `//` comments in product code (tests use `///` XML docs, matching `MudPlay.Tests`), one public type per file.
- **Comment brevity.** Explain *why*, not *what*. Do not port `lost.rs`'s long prose header. A few lines per non-obvious decision.
- **Every file ends with a blank line.**
- **Commit messages** use `feat:` / `fix:` / `doc:` / `test:` / `refactor:` tags, and must **not** reference this plan or the spec.
- **Do not invent game mechanics.** If a MajorMUD behavior is unclear, stop and ask.
- **Direction enum order** is `N,S,E,W,NE,NW,SE,SW,U,D` = `0..9`, with `Teleport = 10` deliberately outside the cardinal `ExitMask`. Iterate `0..9` only.

---

## File Structure

**Create:**
- `Game/Map/LocateOutcome.cs` — the result type (outcome kind, room, candidate count, steps).
- `Game/Map/RoomLocator.cs` — pure seeding + splitting-exit choice. No sends, no state.
- `Game/Map/LocatorWalk.cs` — the send/land pump shared by both drivers.
- `Game/Map/PassiveRelocalizer.cs` — engine-less driver: move-free dead reckoning.
- `MudPlay.Tests/RoomLocatorTests.cs`
- `MudPlay.Tests/LocatorWalkTests.cs`
- `MudPlay.Tests/PassiveRelocalizerTests.cs`

**Modify:**
- `Game/Map/EngineRecoveryGate.cs` — forward tier at the two give-up points.
- `Game/Map/RoomTracker.cs` — expose `RecentSteps` read-only (currently private at line 61).
- `Services/AppServices.cs` — construct and hold `RoomLocator` + `PassiveRelocalizer`.
- `MudPlay.csproj`, `CHANGELOG.md`, `README.md` — version trio.
- `Assets/Help/guide.md` — Help entry.
- `Services/BugReportBuilder.cs` — locator state in the capture.

---

### Task 1: `RoomLocator.Seed` — candidate seeding

**Files:**
- Create: `Game/Map/RoomLocator.cs`
- Test: `MudPlay.Tests/RoomLocatorTests.cs`

**Interfaces:**
- Consumes: `RoomGraphManager.FindCandidates(string, IReadOnlySet<Direction>)`, `RoomGraphManager.FindByNameCoveringExits(string, IReadOnlySet<Direction>)` — both return `IReadOnlyList<RoomKey>`.
- Produces: `RoomLocator(RoomGraphManager graph, LogService? log = null)`; `IReadOnlyList<RoomKey> Seed(RoomObservation observation)`.

- [ ] **Step 1: Write the failing test**

Create `MudPlay.Tests/RoomLocatorTests.cs`. Study `MudPlay.Tests/HiddenExitRevealManagerTests.cs:60-80` for the `GameDataCache` + `RoomGraphManager` construction pattern already used in this suite, and follow it to build the fixture graph. The two behaviors under test:

```csharp
using System.Collections.Generic;
using MudPlay.Game.Map;
using Xunit;

namespace MudPlay.Tests;

/// <summary>
/// Seeding and splitting-exit choice for the history-free localizer.
/// </summary>
public sealed class RoomLocatorTests
{
    private static RoomObservation Obs(string name, params Direction[] exits)
        => new(name, new HashSet<Direction>(exits));

    [Fact]
    public void Seed_returns_the_exact_name_and_exit_match()
    {
        // Fixture: 1/1 "Narrow Road" exits N,S — the only room with that
        // (name, exit-set). Build via the suite's RoomGraphManager pattern.
        RoomLocator locator = BuildLocator();

        IReadOnlyList<RoomKey> seeded = locator.Seed(Obs("Narrow Road", Direction.N, Direction.S));

        Assert.Equal(new[] { new RoomKey(1, 1) }, seeded);
    }

    [Fact]
    public void Seed_widens_to_the_superset_match_when_exact_finds_nothing()
    {
        // Fixture: 1/2 "Shut Gate" has graph exits N,S,E but a closed door
        // hides E, so the observation lists only N,S. The exact bucket
        // (name, {N,S}) is empty; the superset path must still find it.
        RoomLocator locator = BuildLocator();

        IReadOnlyList<RoomKey> seeded = locator.Seed(Obs("Shut Gate", Direction.N, Direction.S));

        Assert.Equal(new[] { new RoomKey(1, 2) }, seeded);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `dotnet test --filter RoomLocatorTests`
Expected: FAIL — `RoomLocator` does not exist (compile error `CS0246`).

- [ ] **Step 3: Write the minimal implementation**

Create `Game/Map/RoomLocator.cs`:

```csharp
using MudPlay.Services;

namespace MudPlay.Game.Map;

// Answers "which rooms could this display be?" from the room graph, and
// picks the exit that best tells the survivors apart.
//
// Pure: it sends nothing, holds no state and owns no observable field.
// The narrowing itself is FootprintMatcher's job — this type only seeds
// the set and ranks directions.
public sealed class RoomLocator
{
    // Steps a walk may take before the room is called indistinguishable.
    // Past twelve the resolution curve is flat, and every step is a room
    // the character did not choose to be in.
    public const int DefaultBudget = 12;

    private readonly RoomGraphManager _graph;
    private readonly LogService? _log;

    public RoomLocator(RoomGraphManager graph, LogService? log = null)
    {
        ArgumentNullException.ThrowIfNull(graph);
        _graph = graph;
        _log = log;
    }

    // Every room consistent with one display. Exact (name, exit-set) first;
    // widen to the superset reading only if that admits nothing, since a
    // closed door or unsearched hidden exit drops a bit the graph still
    // carries — and "nowhere" is the wrong answer about a character that is
    // plainly somewhere.
    public IReadOnlyList<RoomKey> Seed(RoomObservation observation)
    {
        IReadOnlyList<RoomKey> exact = _graph.FindCandidates(observation.Name, observation.Exits);
        if (exact.Count > 0) return exact;

        IReadOnlyList<RoomKey> wide = _graph.FindByNameCoveringExits(observation.Name, observation.Exits);
        if (wide.Count > 0 && _log?.IsDebugEnabled == true)
            _log.Debug("RoomLocator",
                $"Seed('{observation.Name}'): exact empty, superset gave {wide.Count}.");
        return wide;
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `dotnet test --filter RoomLocatorTests`
Expected: PASS, 2 tests.

- [ ] **Step 5: Verify the build is warning-clean**

Run: `dotnet build`
Expected: `0 Warning(s)`, `0 Error(s)`.

- [ ] **Step 6: Commit**

```bash
git add Game/Map/RoomLocator.cs MudPlay.Tests/RoomLocatorTests.cs
git commit -m "feat: seed localizer candidates from one room display"
```

---

### Task 2: `RoomLocator.ChooseSplittingExit` — the direction choice

**Files:**
- Modify: `Game/Map/RoomLocator.cs`
- Test: `MudPlay.Tests/RoomLocatorTests.cs`

**Interfaces:**
- Consumes: `RoomGraphManager.GetRoom(RoomKey)` → `Room?`; `Room.Exits` is `IReadOnlyDictionary<Direction, RoomExit>`; `RoomExit.Target` is `RoomKey`; `Room.Name` is `string`; `Room.ExitMask` is `uint`.
- Produces: `Direction? ChooseSplittingExit(IReadOnlyCollection<RoomKey> candidates, RoomObservation here)`.

**Why `(Name, ExitMask)` as the shape key:** the ideal key is "what will this room look like when I arrive", i.e. name plus *visible* exits. MudPlay cannot compute visible exits reliably — `RoomExit.TryParseWire` classifies "Hidden/Passable" as `RoomExitHint.None` (`Game/Map/RoomExit.cs:292-304`), indistinguishable from an ordinary exit. `ExitMask` therefore slightly over-counts distinct shapes on the 3.4% of rooms carrying hidden exits. That only perturbs *ranking*; the real narrowing is `FootprintMatcher.Step`, so the imperfection is not a correctness bug. Do not add a hint-derived visible mask — it would be wrong.

- [ ] **Step 1: Write the failing tests**

Append to `MudPlay.Tests/RoomLocatorTests.cs`:

```csharp
    [Fact]
    public void ChooseSplittingExit_prefers_the_direction_with_the_most_distinct_neighbours()
    {
        // Candidates 1/10 and 1/11 both look like "Twin Hall" with exits N,E.
        // North leads from both into rooms named "Hall" with identical exits
        // (1 shape). East leads into "Larder" and "Cellar" (2 shapes).
        RoomLocator locator = BuildLocator();

        Direction? chosen = locator.ChooseSplittingExit(
            new[] { new RoomKey(1, 10), new RoomKey(1, 11) },
            Obs("Twin Hall", Direction.N, Direction.E));

        Assert.Equal(Direction.E, chosen);
    }

    [Fact]
    public void ChooseSplittingExit_skips_a_direction_a_candidate_lacks()
    {
        // 1/10 has N,E; 1/12 has only N. East is unusable — taking it would
        // presuppose which candidate we are, which is the open question.
        RoomLocator locator = BuildLocator();

        Direction? chosen = locator.ChooseSplittingExit(
            new[] { new RoomKey(1, 10), new RoomKey(1, 12) },
            Obs("Twin Hall", Direction.N, Direction.E));

        Assert.Equal(Direction.N, chosen);
    }

    [Fact]
    public void ChooseSplittingExit_still_moves_when_no_direction_splits_anything()
    {
        // The patient rule: both candidates' northern neighbours look
        // identical, so this step teaches nothing today — but it carries the
        // whole set forward to a room where the neighbours differ. Measured
        // over the shipped world, patient resolves 93.3% against 87.7%.
        RoomLocator locator = BuildLocator();

        Direction? chosen = locator.ChooseSplittingExit(
            new[] { new RoomKey(1, 20), new RoomKey(1, 21) },
            Obs("Long Corridor", Direction.N));

        Assert.Equal(Direction.N, chosen);
    }

    [Fact]
    public void ChooseSplittingExit_breaks_ties_in_compass_order()
    {
        // North and East split 1/30 and 1/31 equally well. North wins
        // because it comes first in the enum, so a walk is reproducible.
        RoomLocator locator = BuildLocator();

        Direction? chosen = locator.ChooseSplittingExit(
            new[] { new RoomKey(1, 30), new RoomKey(1, 31) },
            Obs("Crossing", Direction.N, Direction.E));

        Assert.Equal(Direction.N, chosen);
    }

    [Fact]
    public void ChooseSplittingExit_returns_null_when_no_listed_exit_is_usable()
    {
        // Nothing left to learn by moving — the walk must stop rather than
        // wander.
        RoomLocator locator = BuildLocator();

        Direction? chosen = locator.ChooseSplittingExit(
            new[] { new RoomKey(1, 40), new RoomKey(1, 41) },
            Obs("Dead End"));

        Assert.Null(chosen);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `dotnet test --filter RoomLocatorTests`
Expected: FAIL — `ChooseSplittingExit` is not defined (`CS1061`).

- [ ] **Step 3: Write the implementation**

Add to `Game/Map/RoomLocator.cs`, inside the class:

```csharp
    // The listed exit that splits the candidate set furthest.
    //
    // Every candidate must have the exit and the graph must know where it
    // leads — otherwise taking it presupposes which candidate we are, which
    // is the question being asked. Among the usable ones, best is whichever
    // reaches the most different-LOOKING rooms, since only a difference the
    // board can show is evidence.
    //
    // A direction that splits nothing is still worth taking: it carries the
    // whole set forward to a room where the neighbours do differ. Ties go to
    // the first direction in compass order so a walk is reproducible.
    //
    // Null when no listed exit is usable at all — the walk should stop.
    public Direction? ChooseSplittingExit(IReadOnlyCollection<RoomKey> candidates, RoomObservation here)
    {
        ArgumentNullException.ThrowIfNull(candidates);
        if (candidates.Count == 0) return null;

        Direction? best = null;
        int bestShapes = 0;

        // 0..9 only: Teleport (10) is synthesized and never a listed exit.
        for (int i = 0; i <= (int)Direction.D; i++)
        {
            var dir = (Direction)i;
            if (!here.Exits.Contains(dir)) continue;

            var shapes = new HashSet<(string Name, uint ExitMask)>();
            bool usable = true;

            foreach (RoomKey candidate in candidates)
            {
                Room? source = _graph.GetRoom(candidate);
                if (source is null || !source.Exits.TryGetValue(dir, out RoomExit exit))
                {
                    usable = false;
                    break;
                }
                Room? destination = _graph.GetRoom(exit.Target);
                if (destination is null)
                {
                    usable = false;
                    break;
                }
                shapes.Add((destination.Name, destination.ExitMask));
            }

            if (!usable) continue;
            if (best is null || shapes.Count > bestShapes)
            {
                best = dir;
                bestShapes = shapes.Count;
            }
        }

        return best;
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `dotnet test --filter RoomLocatorTests`
Expected: PASS, 7 tests.

- [ ] **Step 5: Mutate to prove the tests can fail**

Temporarily change `shapes.Count > bestShapes` to `shapes.Count >= bestShapes`.
Run: `dotnet test --filter RoomLocatorTests`
Expected: `ChooseSplittingExit_breaks_ties_in_compass_order` FAILS (East now wins the tie).
Then revert the mutation and re-run to confirm green. **A test that cannot fail is not evidence.**

- [ ] **Step 6: Commit**

```bash
git add Game/Map/RoomLocator.cs MudPlay.Tests/RoomLocatorTests.cs
git commit -m "feat: choose the exit that splits the candidate set furthest"
```

---

### Task 3: `LocatorWalk` — the send/land pump

**Files:**
- Create: `Game/Map/LocateOutcome.cs`, `Game/Map/LocatorWalk.cs`
- Test: `MudPlay.Tests/LocatorWalkTests.cs`

**Interfaces:**
- Consumes: `RoomLocator.Seed`, `RoomLocator.ChooseSplittingExit`, `RoomLocator.DefaultBudget`; `FootprintMatcher(Func<RoomKey,Direction,HopOutcome>, Func<RoomKey,RoomObservation,bool>, LogService?, int)` with `Reset(IEnumerable<RoomKey>)`, `Step(Direction, RoomObservation)`, `Candidates`, `IsConverged`, `IsExhausted`.
- Produces:
  - `enum LocateOutcomeKind { Converged, Ambiguous, Unknown }`
  - `readonly record struct LocateOutcome(LocateOutcomeKind Kind, RoomKey Room, int CandidateCount, int Steps)` with factories `Converged`, `Ambiguous`, `Unknown`.
  - `LocatorWalk(RoomLocator locator, FootprintMatcher matcher, Action<Direction> send, int budget = RoomLocator.DefaultBudget)`
  - `bool IsActive { get; }`, `int Steps { get; }`
  - `LocateOutcome? Begin(RoomObservation here)` — null means a move was sent and the caller must pump `OnLanding`.
  - `LocateOutcome? OnLanding(RoomObservation landed)` — same contract.

**Why a pump and not an async loop:** `EngineRecoveryGate` is an event-driven phase machine (`_tier3Phase`, `AdvanceReverseWalk` sends and returns, the landing arrives via `OnRoomObserved`). A pump matches that shape, needs no threading, and is testable synchronously with a fake sender.

- [ ] **Step 1: Write the failing tests**

Create `MudPlay.Tests/LocatorWalkTests.cs`:

```csharp
using System.Collections.Generic;
using MudPlay.Game.Map;
using Xunit;

namespace MudPlay.Tests;

/// <summary>
/// The send/land pump: it must send nothing when the first display already
/// settles the question, converge on a unique candidate, and report an
/// honest count when the rooms are genuinely indistinguishable.
/// </summary>
public sealed class LocatorWalkTests
{
    private static RoomObservation Obs(string name, params Direction[] exits)
        => new(name, new HashSet<Direction>(exits));

    [Fact]
    public void Begin_sends_nothing_when_one_display_already_settles_it()
    {
        var sent = new List<Direction>();
        LocatorWalk walk = BuildWalk(sent);

        LocateOutcome? outcome = walk.Begin(Obs("Narrow Road", Direction.N, Direction.S));

        Assert.Empty(sent);
        Assert.Equal(LocateOutcomeKind.Converged, outcome!.Value.Kind);
        Assert.Equal(new RoomKey(1, 1), outcome.Value.Room);
        Assert.Equal(0, outcome.Value.Steps);
    }

    [Fact]
    public void Begin_reports_unknown_when_the_graph_has_no_such_room()
    {
        var sent = new List<Direction>();
        LocatorWalk walk = BuildWalk(sent);

        LocateOutcome? outcome = walk.Begin(Obs("Nowhere At All", Direction.N));

        Assert.Empty(sent);
        Assert.Equal(LocateOutcomeKind.Unknown, outcome!.Value.Kind);
    }

    [Fact]
    public void A_landing_that_narrows_to_one_converges()
    {
        var sent = new List<Direction>();
        LocatorWalk walk = BuildWalk(sent);

        LocateOutcome? first = walk.Begin(Obs("Twin Hall", Direction.N, Direction.E));
        Assert.Null(first);                       // ambiguous — a move went out
        Assert.Equal(new[] { Direction.E }, sent);

        LocateOutcome? done = walk.OnLanding(Obs("Larder", Direction.W));

        Assert.Equal(LocateOutcomeKind.Converged, done!.Value.Kind);
        Assert.Equal(new RoomKey(1, 10), done.Value.Room);
        Assert.Equal(1, done.Value.Steps);
    }

    [Fact]
    public void An_exhausted_budget_reports_how_many_rooms_remain()
    {
        var sent = new List<Direction>();
        LocatorWalk walk = BuildWalk(sent, budget: 2);

        walk.Begin(Obs("Long Corridor", Direction.N));
        walk.OnLanding(Obs("Long Corridor", Direction.N));
        LocateOutcome? outcome = walk.OnLanding(Obs("Long Corridor", Direction.N));

        Assert.Equal(LocateOutcomeKind.Ambiguous, outcome!.Value.Kind);
        Assert.True(outcome.Value.CandidateCount > 1);
        Assert.Equal(2, outcome.Value.Steps);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `dotnet test --filter LocatorWalkTests`
Expected: FAIL — `LocatorWalk` / `LocateOutcome` do not exist (`CS0246`).

- [ ] **Step 3: Write `LocateOutcome`**

Create `Game/Map/LocateOutcome.cs`:

```csharp
namespace MudPlay.Game.Map;

// Why a walk stopped.
public enum LocateOutcomeKind
{
    // Exactly one room survived. Room carries it.
    Converged = 0,

    // Several rooms survived and walking cannot tell them apart.
    // CandidateCount carries how many: "one of 212" and "one of 2" are
    // different situations, and the user is entitled to know which.
    Ambiguous = 1,

    // No room in the graph matches. Walking cannot fix this — the world
    // loaded is not the world the character is standing in.
    Unknown = 2,
}

// Where a character turned out to be, and what it cost to find out. Steps
// is not decoration: it is how far the character was MOVED to answer the
// question, which the operator who asked is entitled to know.
public readonly record struct LocateOutcome(
    LocateOutcomeKind Kind,
    RoomKey Room,
    int CandidateCount,
    int Steps)
{
    public static LocateOutcome Converged(RoomKey room, int steps)
        => new(LocateOutcomeKind.Converged, room, 1, steps);

    public static LocateOutcome Ambiguous(int candidateCount, int steps)
        => new(LocateOutcomeKind.Ambiguous, default, candidateCount, steps);

    public static LocateOutcome Unknown(int steps)
        => new(LocateOutcomeKind.Unknown, default, 0, steps);
}
```

- [ ] **Step 4: Write `LocatorWalk`**

Create `Game/Map/LocatorWalk.cs`:

```csharp
namespace MudPlay.Game.Map;

// Drives a localizing walk: ask the locator which exit splits the surviving
// candidates furthest, send it, fold the landing back in, repeat.
//
// A pump rather than a loop, because its hosts are event-driven — the gate
// sends and returns, and the landing arrives later on a room-observed
// event. Begin/OnLanding return null while the walk is still going and an
// outcome when it is done.
//
// Sending is injected, so the same walk serves an attached engine
// (SendBacktrackMove) and an engine-less driver (the gated wire sender)
// without either being named here.
public sealed class LocatorWalk
{
    private readonly RoomLocator _locator;
    private readonly FootprintMatcher _matcher;
    private readonly Action<Direction> _send;
    private readonly int _budget;

    private RoomObservation _here;
    private Direction _lastSent;
    private bool _active;

    public LocatorWalk(
        RoomLocator locator,
        FootprintMatcher matcher,
        Action<Direction> send,
        int budget = RoomLocator.DefaultBudget)
    {
        ArgumentNullException.ThrowIfNull(locator);
        ArgumentNullException.ThrowIfNull(matcher);
        ArgumentNullException.ThrowIfNull(send);
        if (budget < 0) throw new ArgumentOutOfRangeException(nameof(budget));
        _locator = locator;
        _matcher = matcher;
        _send = send;
        _budget = budget;
    }

    // True while a move is outstanding and OnLanding is owed.
    public bool IsActive => _active;

    // Moves sent so far this walk.
    public int Steps { get; private set; }

    // Seed from the current display and take the first step if one is
    // needed. Returns an outcome when no walking is required at all.
    public LocateOutcome? Begin(RoomObservation here)
    {
        _here = here;
        Steps = 0;
        _active = false;
        _matcher.Reset(_locator.Seed(here));
        return Advance();
    }

    // Fold one landing into the candidate set and take the next step.
    public LocateOutcome? OnLanding(RoomObservation landed)
    {
        if (!_active) return null;
        _active = false;
        _matcher.Step(_lastSent, landed);
        Steps++;
        _here = landed;
        return Advance();
    }

    // Settle, or send the next splitting step. Null means a move went out.
    private LocateOutcome? Advance()
    {
        if (_matcher.Candidates.Count == 0) return LocateOutcome.Unknown(Steps);
        if (_matcher.IsConverged)
        {
            foreach (RoomKey only in _matcher.Candidates)
                return LocateOutcome.Converged(only, Steps);
        }
        if (Steps >= _budget) return LocateOutcome.Ambiguous(_matcher.Candidates.Count, Steps);

        // Candidates is IReadOnlySet<RoomKey>, which already satisfies the
        // IReadOnlyCollection parameter — no copy, no System.Linq needed.
        Direction? next = _locator.ChooseSplittingExit(_matcher.Candidates, _here);
        // Nothing left to learn by moving — stop rather than wander.
        if (next is not { } dir) return LocateOutcome.Ambiguous(_matcher.Candidates.Count, Steps);

        _lastSent = dir;
        _active = true;
        _send(dir);
        return null;
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `dotnet test --filter LocatorWalkTests`
Expected: PASS, 4 tests.

- [ ] **Step 6: Mutate to prove the tests can fail**

Temporarily change `if (Steps >= _budget)` to `if (Steps >= _budget + 5)`.
Run: `dotnet test --filter LocatorWalkTests`
Expected: `An_exhausted_budget_reports_how_many_rooms_remain` FAILS.
Revert and re-run to confirm green.

- [ ] **Step 7: Verify the build is warning-clean**

Run: `dotnet build`
Expected: `0 Warning(s)`.

- [ ] **Step 8: Commit**

```bash
git add Game/Map/LocateOutcome.cs Game/Map/LocatorWalk.cs MudPlay.Tests/LocatorWalkTests.cs
git commit -m "feat: pump a localizing walk from an injected sender"
```

---

### Task 4: Resolution-curve check against real game data

**Files:**
- Test: `MudPlay.Tests/RoomLocatorCurveTests.cs` (create)

**Interfaces:**
- Consumes: everything from Tasks 1-3.
- Produces: nothing consumed downstream. This is a measurement.

**Context:** the spec carries a resolution curve from `lost.rs`'s own simulation — 18.7% of rooms resolved from one display, 52.0% after one step, 93.3% at twelve. **Only the zero-step point has been independently reproduced.** This task re-derives the curve on MudPlay's own graph, validating both the ported rule and the claim the design rests on.

Real game data lives under the user's app folder (`~/.local/share/MudPlay/game data/{set}/Rooms.json`) and is not in the repo, so this test must **skip cleanly** when it is absent rather than fail. It is a measurement, not a gate.

- [ ] **Step 1: Write the test**

Create `MudPlay.Tests/RoomLocatorCurveTests.cs`. Locate the active set's `Rooms.json` via `MudPlay.Services.AppPaths`; if no set is present, return early so the suite stays green on a clean machine. For every room in the graph, simulate: seed from what the board would display for that room, then walk the splitting rule against the graph itself (no wire), recording how many steps it took to converge. Assert the aggregate:

```csharp
        // The curve the design rests on. Loose bounds — this guards against
        // a regression in the splitting rule, not against a specific graph.
        Assert.True(resolvedAtZero  >= 0.15, $"0 steps: {resolvedAtZero:P1}");
        Assert.True(resolvedAtOne   >= 0.45, $"1 step:  {resolvedAtOne:P1}");
        Assert.True(resolvedAtTwelve >= 0.85, $"12 steps: {resolvedAtTwelve:P1}");
```

Print the full curve through `ITestOutputHelper` so a run reports the measured numbers, not just pass/fail.

- [ ] **Step 2: Run it**

Run: `dotnet test --filter RoomLocatorCurveTests`
Expected: PASS. If game data is present, the output carries the measured curve — **record those numbers in the PR description**, since they are the evidence the feature works at all. If absent, the test skips and says so.

- [ ] **Step 3: Commit**

```bash
git add MudPlay.Tests/RoomLocatorCurveTests.cs
git commit -m "test: re-derive the localizer resolution curve on real data"
```

---

### Task 5: Driver A — forward tier in `EngineRecoveryGate`

**Files:**
- Modify: `Game/Map/EngineRecoveryGate.cs` (`AdvanceReverseWalk` at 640-661; `FailTier3`; the tier-3 landing path around 537-577)
- Test: `MudPlay.Tests/EngineRecoveryGateTests.cs` (extend the existing file)

**Interfaces:**
- Consumes: `LocatorWalk`, `LocateOutcome`, `RoomLocator` from Tasks 1-3. Existing gate internals: `_tier3` (a `FootprintMatcher`), `_engine`, `FinishTier3Success(RoomKey)`, `FailTier3(string)`, `_tier3Phase`.
- Produces: no new public surface. Behavior change only.

**The change:** today `AdvanceReverseWalk` calls `FailTier3("backtrack exhausted to anchor without convergence")` the moment `_executedSinceAnchor` is empty — terminal, zero moves sent. Replace that branch with a forward `LocatorWalk` over the same `_tier3` matcher, sending via `_engine.SendBacktrackMove`. Reverse-walk stays first (it is cheaper and returns the character toward a known anchor); the forward walk is what happens when reverse-walk has nothing left.

- [ ] **Step 1: Write the failing test**

Add to `MudPlay.Tests/EngineRecoveryGateTests.cs` a test asserting that a gate entering tier 3 with **no executed history** sends a move rather than failing:

```csharp
    [Fact]
    public void Tier3_with_no_history_walks_forward_instead_of_failing()
    {
        // The party case: PartyFollowerMovementGate held the engine all
        // session, so _executedSinceAnchor is empty. Reverse-walk has
        // nothing to un-walk; the forward locator must take over.
        // Follow the fixture pattern already used by this test class.
        var sent = new List<Direction>();
        EngineRecoveryGate gate = BuildGateWithNoHistory(sent, out FakeEngine engine);

        gate.ForceTier3ForTests("no history");

        Assert.NotEmpty(sent);
        Assert.False(engine.RecoveryFailed);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `dotnet test --filter EngineRecoveryGateTests`
Expected: FAIL — no move is sent and `AbortFromRecoveryFailure` fires.

- [ ] **Step 3: Implement**

In `AdvanceReverseWalk`, replace the `_executedSinceAnchor.Count == 0` terminal branch with a handoff to the forward walk. Construct the `LocatorWalk` lazily against the existing `_tier3` matcher and `_engine.SendBacktrackMove`. Route the landing that currently reaches `_tier3.Step(_lastBacktrackReverse, obs)` into `LocatorWalk.OnLanding` while the forward walk is active, and map the outcome:

- `Converged` → `FinishTier3Success(outcome.Room)`
- `Ambiguous` → `FailTier3($"one of {outcome.CandidateCount} rooms walking cannot tell apart")`
- `Unknown` → `FailTier3("no room in the loaded world matches this display")`

Keep the existing dark-landing and combat-clear handling intact — the forward walk sends the same kind of cardinal move, so those paths apply unchanged.

- [ ] **Step 4: Run the tests**

Run: `dotnet test --filter EngineRecoveryGateTests`
Expected: PASS, including every pre-existing test in the file.

- [ ] **Step 5: Run the whole suite**

Run: `dotnet test`
Expected: all green. This touches shared recovery machinery — a regression here breaks the walker, the loop runner and auto-lair at once.

- [ ] **Step 6: Commit**

```bash
git add Game/Map/EngineRecoveryGate.cs MudPlay.Tests/EngineRecoveryGateTests.cs
git commit -m "fix: walk forward when there is no history to un-walk"
```

---

### Task 6: Driver B — engine-less dead reckoning, with the party guard

**Files:**
- Create: `Game/Map/PassiveRelocalizer.cs`, `MudPlay.Tests/PassiveRelocalizerTests.cs`
- Modify: `Game/Map/RoomTracker.cs` (expose `RecentSteps`; `_recentSteps` is private at line 61), `Services/AppServices.cs`

**Interfaces:**
- Consumes: `RoomLocator`, `LocatorWalk`, `RoomTracker.StateChanged` (`event Action<RoomTransition>`), `RoomTracker.SetLocated(RoomKey)`, `MovementCoordinator` follower-gate state, `PartyState.IsInParty` / `SelfIsLeader`.
- Produces:
  - `RoomTracker.RecentSteps` → `IReadOnlyList<DirectionDto>`
  - `PassiveRelocalizer(RoomTracker tracker, RoomLocator locator, RoomGraphManager graph, MovementCoordinator coordinator, LogService? log = null)`, `IDisposable`.

**Behavior:** on a `StateChanged` transition into `Suspect` or `Lost` with no engine attached, replay `RoomTracker.RecentSteps` forward through a `FootprintMatcher` seeded by the locator. Pure computation — **zero bytes sent**. On convergence call `SetLocated`. Because follow-drags already reach `_recentSteps` (`RoomTracker.cs:322-333`), this is the party case.

**Party guard — the one real behavioral risk.** Never send a move while the follower gate is asserted, or the client marches the player out of formation. Read the gate from `MovementCoordinator` rather than re-deriving `IsInParty && !SelfIsLeader`, so there is one authority for "am I being driven". Walking at all is additionally behind the opt-in setting from Task 7; until that lands, this driver is computation-only.

- [ ] **Step 1: Expose the tracker's step record**

In `Game/Map/RoomTracker.cs`, beside `_recentSteps`:

```csharp
    // Read-only view of the step record, including party follow-drags —
    // the evidence a passive relocalizer replays. Writes stay with
    // AppendStep so the single-writer discipline holds.
    public IReadOnlyList<DirectionDto> RecentSteps => _recentSteps;
```

- [ ] **Step 2: Write the failing tests**

Create `MudPlay.Tests/PassiveRelocalizerTests.cs` with two tests:

```csharp
    [Fact]
    public void Replaying_follow_drags_narrows_without_sending_anything()
    {
        // A follower dragged N then E from an ambiguous start: replay alone
        // must pin the room, with no bytes on the wire.
    }

    [Fact]
    public void It_never_sends_a_move_while_the_follower_gate_is_asserted()
    {
        // Walking a follower out of formation is the one unacceptable
        // failure mode.
    }
```

Fill both in against the fixture pattern this suite already uses (see `MudPlay.Tests/PartyFollowerMovementGateTests.cs` for the `MovementCoordinator` setup).

- [ ] **Step 3: Run them to verify they fail**

Run: `dotnet test --filter PassiveRelocalizerTests`
Expected: FAIL — `PassiveRelocalizer` does not exist.

- [ ] **Step 4: Implement `PassiveRelocalizer`**

Create `Game/Map/PassiveRelocalizer.cs`. Subscribe to `RoomTracker.StateChanged` in the constructor, unsubscribe in `Dispose`. On a transition into `Suspect`/`Lost`: seed a `FootprintMatcher` from `RoomLocator.Seed` on the current observation, replay `RecentSteps` through `Step`, and call `tracker.SetLocated` on convergence. Log one Info line naming the outcome. Send nothing.

- [ ] **Step 5: Wire it into `AppServices`**

In `Services/AppServices.cs`, alongside the existing `RoomGraph` / `RoomTracker` construction (see `FollowMove` at line 2663 for the established pattern), add `RoomLocator` and `PassiveRelocalizer` properties and construct them. Dispose the relocalizer wherever per-character services are torn down.

- [ ] **Step 6: Run the tests**

Run: `dotnet test --filter PassiveRelocalizerTests`
Expected: PASS.

- [ ] **Step 7: Mutate the party guard**

Temporarily invert the follower-gate check so it permits sending.
Run: `dotnet test --filter PassiveRelocalizerTests`
Expected: `It_never_sends_a_move_while_the_follower_gate_is_asserted` FAILS.
Revert and re-run. **This is the guard that protects a live party — prove it works.**

- [ ] **Step 8: Full suite + build**

Run: `dotnet test` then `dotnet build`
Expected: all green, `0 Warning(s)`.

- [ ] **Step 9: Commit**

```bash
git add Game/Map/PassiveRelocalizer.cs Game/Map/RoomTracker.cs Services/AppServices.cs MudPlay.Tests/PassiveRelocalizerTests.cs
git commit -m "feat: relocalize a dragged follower from its own footsteps"
```

---

### Task 7: Definition-of-Done surfaces

**Files:**
- Modify: `MudPlay.csproj`, `CHANGELOG.md`, `README.md`, `Assets/Help/guide.md`, `Services/BugReportBuilder.cs`, plus the Settings model/tab for the opt-in toggle.

**Interfaces:**
- Consumes: everything above.
- Produces: user-facing surfaces. Nothing downstream.

MudPlay's `CLAUDE.md` makes each of these part of Definition of Done. A PR that changes behavior and leaves them stale is incomplete.

- [ ] **Step 1: Settings surface**

Add the opt-in "walk to find myself when lost" toggle and its step budget to the Settings model and the appropriate tab, routed through `SettingsResolver` like every other setting. Default the walk **off** — it moves the character, and that should be a choice. Wire `PassiveRelocalizer` to consult it.

- [ ] **Step 2: Logging surface**

Confirm each decision emits at the right severity: Info for "seeded N candidates", "walked K steps", and the final outcome; Debug for per-candidate drops. An operator reading the program log should be able to see the localizer work.

- [ ] **Step 3: Bug-report surface**

Add locator state to `Services/BugReportBuilder.cs` — current candidate count, steps taken, last outcome — beside the existing Movement section (`BuildMovement`, line 675). State that never reaches the capture is invisible when a user reports it broken.

- [ ] **Step 4: Help surface**

Add a short `Assets/Help/guide.md` entry explaining what the client now does when it loses track, and what the new setting controls.

- [ ] **Step 5: Version trio**

Per the semver policy this is a **MINOR** bump — a capability that did not exist before. In one commit:
1. Bump `<Version>` in `MudPlay.csproj` (x.y.z → x.(y+1).0).
2. Prepend a `## <version>` section to `CHANGELOG.md` — terse bullets, no subheads, no summary paragraph. No `bug reports addressed:` line; there is no report behind this.
3. Replace the `README.md` current-version block between the `<!-- current-version:start -->` / `<!-- current-version:end -->` markers to mirror it, minus the bookkeeping tail.

- [ ] **Step 6: Full verification**

Run: `dotnet build` then `dotnet test`
Expected: `0 Warning(s)`, all tests green.

- [ ] **Step 7: Commit and push**

```bash
git add -u
git commit -m "feat: surface the room locator in settings, help and bug reports"
git push -u fork pathfinding-fixes
```

Then open the PR against `Tehshortbus/MudPlay`, and put the Task 4 curve numbers in the description.

**Note on `git add`:** stage named paths, never `git add -A`. Parallel work in this tree has previously left stray experiment files behind, and a blanket add commits them silently.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| `RoomLocator` pure core — seeding | 1 |
| `RoomLocator` pure core — splitting choice, patient rule, ties | 2 |
| Walk loop, one implementation two senders | 3 |
| Measurements / curve claim | 4 |
| Driver A — forward tier | 5 |
| Driver B — move-free, engine-less | 6 |
| Party safety | 6 |
| Non-goal: no listed-exit index | Task 2 preamble states it and says why a hint-derived mask would be wrong |
| Non-goal: no look-sweep while dragged | Task 6 is computation-only |
| Definition-of-Done surfaces | 7 |
| PR shape | 7, Step 7 |

**Type consistency:** `LocateOutcome` / `LocateOutcomeKind` are defined in Task 3 and used in Tasks 5-6 under those names. `RoomLocator.Seed` and `ChooseSplittingExit` keep the signatures introduced in Tasks 1-2. `RoomTracker.RecentSteps` is introduced in Task 6 Step 1 before its use in Step 4. `LocatorWalk.Begin`/`OnLanding` return `LocateOutcome?` throughout.

**Known soft spots, deliberately left to the implementer:** Tasks 5 and 6 describe fixture construction rather than spelling out every fake, because both extend existing test classes whose fixture helpers are already established in-file. The implementer should read the surrounding test class first and match it. Task 4's data-presence check depends on the user's installed game data and must skip, never fail, when absent.

**Assumption carried from the spec:** whether a hidden or foliage party drag prints any hookable line is unconfirmed. Nothing in this plan depends on the answer — Driver B degrades to Driver A's walk when drags go unrecorded.
