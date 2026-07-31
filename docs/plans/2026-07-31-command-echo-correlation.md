# Permanent fix: command-echo correlation for the MUD client

Worktree: `/home/daniel/bbs-stopstate`, branch `refactor/stop-state`. One commit per phase, tests first, full `cargo test` gate per phase.

## Context

The client's three consumers — `Gate` (farm.rs:441), `Navigator::wait_room` (nav.rs:708), `StopState` (farm.rs:544) — each independently guess which board reply answers which sent command. `Gate` acks on the next prompt (prompts arrive unsolicited in bursts); `Navigator` accepts the first `RoomSeen` whoever asked for it. Result: position desyncs on 3 of 4 live cave-bear runs (`desync: expected "Dungeon, Entrance", saw "Newhaven, Arena"`), pre-dating the stop-state work.

`docs/board-correlation.md` (this branch) has the diagnosis: **the board echoes every accepted command and the reply follows the echo** — measured 94.1% of 6,344 corpus room blocks, 97.3% on live captures. The echo already reaches consumers as `Event::Line(cmd)` and everyone ignores it. Status: "investigated and measured, **not yet implemented**." This plan implements it.

**Acceptance is fixed by the doc (§Acceptance, verbatim):** five consecutive 300s runs of circuit `["1/2150","1/2152","1/2156"]` on the **live board**, each ending `TimeUp` and walking to the finish room, **zero** `NavError`, board restarted before each run. Explicitly in scope: nav must open the door at 1/2150-north itself (no manual lua), and lighting comes before dead reckoning. "A run that dies at 80s is not a partial pass."

## Architecture decision — and why the doc's shape needed amending

The doc proposed each consumer watching for "its" echo. Live-capture evidence (verified directly in `~/mmc-probe/stopstate-run5_timing.log:955-1010`) breaks that:

- **A busy board echoes a command twice**: a *receipt echo* ~2ms after TX, and a *second execution echo* immediately before the reply when commands queue behind the round timer (`open n` echoed at 520.992 and again at 522.740, reply "The door is locked." right after). Idle case: the two coincide. Any first-match-pops correlator desyncs by one on exactly the busy case the fix exists for.
- **Replies are strictly FIFO** in every capture — order, not echo text, is the attribution key. Identical pipelined commands (four `n` in 7s → five `n` echo occurrences) make text-keyed maps dead on arrival.
- **Prompt adjacency identifies nothing** — async spawn lines land after redrawn prompts (`[HP=42/MA=5]:A acid slime oozes into the room…`), and echoes appear bare with no prompt. So the parser cannot mark echoes syntactically; detection needs the sent-command registry.
- **Single-command-in-flight is false** outside `farm_stop`: nav, heal pokes, `go_to_finish`, dialect login all send directly (nav.rs:361-665, farm.rs:858/1179/1257/1453, dialect.rs). Hanging correlation off `Gate` covers a minority of traffic and none of the navigator.
- **Per-consumer pending state dies at phase handoffs** — every phase subscribes a fresh broadcast receiver (farm.rs:1514, nav.rs:302), which is precisely how the stray `look`'s block orphans and desyncs the walk. Consumer-side correlation cannot see across that boundary; `Lagged` can split an echo from its block.

**Therefore: one correlator, owned by `Session`.** Registry fed in the writer task (wire order, session.rs:207-231); attribution computed in the reader task where events are born (session.rs:266-269); attribution carried **inside the broadcast event**. Consumers stop guessing by having zero of them correlate.

Frozen invariants (the micro-rules below are derived from the corpus, these are not negotiable):

1. **Echo = acceptance. FIFO order = attribution. Deadline = fallback.** Never attribute by echo text alone, never by prompt adjacency, never by count (the reverted `7ddfd17` counter — "a proxy for correlation is not correlation").
2. **A wrong association is worse than no association.** Ambiguity (worse-than-observed splits, missing echo) falls to the deadline, and consumers re-ask / re-localize — never accept.
3. **Unattributed events still flow** (`answers: None`). They must never satisfy a pending step, but flee detection, `localize_view`, and the TUI all consume unsolicited output today and must keep working (5.9% of blocks are legitimately unsolicited: summons, teleports, respawn-after-death, login render).
4. **`SlowDown` flushes the whole registry** — whether a flood-dropped command still echoes is unverified; flush + deadlines is the only safe response.

## Phase 1 — `correlate.rs`: a pure, replayable Correlator

New module `crates/mud-client/src/correlate.rs`. No I/O, no clocks it doesn't get injected — same discipline as `StopState`.

```rust
pub struct CmdId(u64);
pub struct Correlated { pub event: Event, pub answers: Option<CmdId> }

pub struct Correlator { /* ordered VecDeque<Entry>, each: id, text, echo_seen, deadline */ }
impl Correlator {
    pub fn sent(&mut self, id: CmdId, line: &str, now: Instant);   // writer-task feed, wire order
    pub fn on_event(&mut self, ev: Event, now: Instant) -> Correlated;
    pub fn flush(&mut self);                                        // SlowDown
}
```

The ONE echo definition, `is_echo(line, cmd)`: trim; strip a single leading parenthesized decoration (generalize `bot.rs::strip_status` — `(Resting)` is captured, hiding/sneaking are not, so strip any `(word)` token, and move the fn here with bot.rs delegating); exact match, else suffix-fragment match (cmd len ≥ 3, fragment len ≥ 2 — covers the ~0.4% split `l`+`ook`; a bare `n` never matches the tail of `open n`).

Attribution rule (starting point; the corpus is the oracle): an echo occurrence matches the **oldest un-retired entry** with that text; a duplicate occurrence of the already-echoed head re-confirms it, never advances. Events after the head's echo attribute to the head. The head retires on reply completion — `RoomSeen`, or a known single-line reply wording (the nav constants: door/exit/combat/dark lines, moved or referenced here), or deadline expiry. Unknown lines attribute but don't retire; deadlines govern.

Tests first (`tests/correlate.rs`):
- **The run5 double-echo/door sequence as a named fixture** (transcribed from `~/mmc-probe/stopstate-run5_timing.log:955-1010`): four pipelined `n` + `open n` + `bash n`; assert every reply attributes to the right send and the "Dungeon, Entrance" block attributes to the final `n`.
- The run7 dark sequence: echo `n` → TOO_DARK attributes to the step; echo `look` → TOO_DARK attributes to the look.
- Split echo `l`+async+`ook`; decorated echo; identical-text FIFO; deadline expiry; flush-on-SlowDown; unsolicited block → `answers: None`.
- Rewrite `tests/echo_correlation.rs` from characterization to mechanism: replay all 102 `re/oracle` raws through TX/RX reconstruction from the paired `_timing.log` files where present, assert ≥94% of blocks attributed, and **negative**: the login-banner entry render is never attributed, in any file.

### Phase 1 as-built amendments (post-review, two passes)

The review passes replayed the rules over ALL the captures and found the original retirement sketch produced a wrong association straight from run6 (a swing announce stole a retirement and a room block landed on the wrong move). As landed:

- **Retirement needs positive evidence.** An unknown line answers None and retires NOTHING (the board's unsolicited din is large: swing announces, spawn wordings the classifier misses, "You hear movement…", exp/loot lines). Retirement happens only when an event matches the pending command's **reply grammar** (`Kind`: Move/Look/Open/Bash/Cast/Light/Get/BuyHealing; commands outside it are Opaque and expire by deadline — missed, never wrong). The move-vs-look door wording split landed here (Phase 5 consumes it rather than introducing it).
- **FIFO cleanup:** when a reply retires an entry, everything older is dropped (replies are FIFO). **Progress refresh:** every acceptance/retirement refreshes all pending deadlines (a queued command hears nothing about itself for ~3s/round between receipt and execution echo).
- **parse.rs fix (unplanned, found by review):** a room name glued to a redrawn prompt on one physical line (the normal busy-room render — board uses cursor-back+erase, not newline) got `opening=None` and the whole block dissolved into Lines. `handle_line` now recovers the SGR after the last `]:`. Corpus tripwire moved 393→394.
- **Corpus mechanism test:** deferred honestly — 53 re/oracle raws DO have `*_timing.log` sidecars (TX interleaved), so a content-alignment replay is possible but expensive; the mechanism is pinned by the transcribed run5/6/7 fixtures, and end-to-end validation comes from Phases 2-3 (echoing fixture server) + Phase 10 live. Deferred: the corpus-wide "login-banner render never attributed" negative.
- **Known residuals (documented in module doc):** a chat line quoting a grammar wording verbatim can retire an entry early; two same-text pending entries where the older's echo was eaten can briefly mark the younger accepted. Both bounded by deadlines.

## Phase 2 — Session integration

`crates/mud-client/src/session.rs`:
- `send()` returns `CmdId` (atomic counter). `Cmd::Line` carries the id. `send_raw` excluded (FSD keystrokes).
- Writer task pushes `(id, line)` into the shared correlator state **before** `write_all` (push-before-write ⇒ registry order = wire order, and the entry exists before its echo can arrive). State is `Mutex<Correlator>` shared with the reader.
- Reader task: `for ev in parser.push(...)` → `correlator.on_event(ev)` → broadcast `Correlated`. Channel becomes `broadcast::Sender<Correlated>`; `drain()` and `events()` signatures follow. Registry entries get deadlines from birth so login/menu traffic (answered by menus, not realm grammar) can never inflate pending state — the `7ddfd17` lesson.
- Mechanical update of all subscribers (nav.rs:302, tui.rs:134, mmc.rs:251, farm.rs:1177/1255/1287/1514/1675, live_server.rs:120): destructure, ignore `answers` for now. Pure cores (`Bot`, `Parser`, `StopState`) keep taking `&Event`.

Gate: full workspace `cargo test` — zero behavior change intended in this phase.

## Phase 3 — mud-server echoes (fixture honesty + reimplementation fidelity)

The real board echoes every accepted line; `crates/mud-server/src/server.rs` (~:320, ~:423) does not — so every mud-server-backed test would otherwise exercise only the deadline fallback and prove nothing. Echo the accepted line back before dispatch (receipt echo). This is also a fidelity fix for the reimplementation itself — verify wording/position against a `re/oracle` raw. Execution-echo-under-round-timer modeling is a recorded follow-up; scripted TCP boards cover the double-echo case client-side.

Tests: new `live_server.rs` assertion (echo precedes room name in transcript); absorb fallout across the suite — review each failure, never blanket-weaken.

## Phase 4 — Gate acks on its echo

`farm.rs` Gate: **delete** the `Event::Prompt => in_flight = None` arm. Ack when the correlated event's `answers` matches the in-flight `CmdId` (Gate's send now keeps the id from `session.send`), or on its existing `ACK_TIMEOUT` fallback (keep — it is the echo-less escape). `SlowDown` requeue already exists; it now also coincides with the session-level flush.

Tests first (`tests/farm.rs` Gate section): unsolicited prompt bursts never ack; the echo acks; a different command's echo does not; timeout still expires.

## Phase 5 — Navigator: only the block that answers *its* step

`nav.rs`:
- `walk_step`/`arrival`/`wait_room` thread the `CmdId` of each send (dir word, `open {dir}`, `bash {dir}`, recovery `look`). `wait_room` classifies **only events attributed to that id**; unattributed `RoomSeen`s are observed (guard, state) but never satisfy the step. Step timeout unchanged → existing `localize`/failure machinery is the fallback.
- **Delete `DOOR_BLOCKED`** (nav.rs:147). Split into `MOVE_DOOR_BLOCKED` ("the door is closed!", "the gate is closed!", locked wordings — trailing `!` is load-bearing) vs `LOOK_DOOR_BLOCKED` ("door is closed in that direction") which must never read as a move refusal. Verify both against the DLL strings before landing.
- `blind_position`/`BlindContext` (nav.rs:501-531) becomes sound automatically: TOO_DARK attributed to a *move* id means the step landed; attributed to a *look* id means position unchanged — the last-sent guess is deleted.

Tests first: update `tests/nav_doors.rs` scripted boards to echo (they must model the real board), plus new scripts: the mechanized desync repro (unsolicited stale block + echoed step → lands correctly); double-echo busy-door script (straight from the run5 fixture); echo-less block never satisfies a step (times out); look-refusal provokes zero opens. `tests/nav.rs` (mud-server) should pass unchanged after Phase 3.

## Phase 6 — StopState reads attribution

`farm.rs` StopState: **delete** `changed: u64`, `asked_when`, `Seen.change`, `awaiting_look` — the hand-rolled generation counter is exactly what attribution replaces. Keep a `pending_look: Option<CmdId>`. A `RoomSeen` naming the stop is believed only when it answers `pending_look` (or an attributed nav arrival at stop entry). `invalidate()` now discards `seen` outright (strictly stronger). TOO_DARK sets `blind` only when attributed to our look/move. Update the field docs and `docs/mud-client.md` §How a stop ends.

Tests first (`tests/farm.rs` ~754-1255): update `look_and_see` helper to feed attribution; rewrite the race test (`a_room_block_that_raced_a_new_arrival_is_not_believed`); new: unsolicited block is never accepted and never clears the pending look.

## Phase 7 — Runner integration sweep

`verify_start`, `go_to_finish`, `ask`, heal pokes (farm.rs:858, 1166-1312, 1453): each send keeps its `CmdId` and waits for attributed answers — a login-banner render can never satisfy `verify_start`; `go_to_finish`'s look→light→look pipeline loses its bare 2s sleep. Confirm flee detection still triggers on **unattributed** off-name blocks (farm.rs:1655-1681) — that path must keep consuming `answers: None` events. Full workspace gate.

## Phase 8 — Lighting (before dead reckoning, per the doc)

Prereq (ops, not code): get a torch/lantern onto Salad — `SYSOP SUMMON` can conjure one. Prereq (RE): extract burn-out and cast-fail wordings from the DLL ("You lit the %s." = 0xdb52d is known).

New `LightState` (sheet.rs): plan re-derived on demand (not once at run start); light command's outcome line **attributed via the correlator** — "You lit the %s." marks lit, "…but fail." with low mana marks recoverable (rest, retry), still-dark-while-lit marks the source exhausted and re-derives; lit sources are not re-lit. Retire `MAX_LIGHT_ATTEMPTS` in favor of "try what can work, verify, only give up when nothing can" — `Blind`/dead-reckoning remains the floor for the genuinely unlightable. Thread `&mut LightState` through `travel`/`farm_stop`/`go_to_finish` replacing the static plan.

Tests first: `LightState` unit tests + scripted dark-stop boards (lit path, mana-fail-rest-retry path, exhausted path).

## Phase 9 — Door reliability at 1/2150-north

Prereq (RE): extract the bash-*failure* wording from the DLL — it is in neither `DOOR_YIELDED` nor the blocked set, which is the likely reason nav gives up today (unrecognized line → deadline). Read `~/mmc-probe/opendoor.lua` for what the manual script does differently. Implement bounded `bash` retries (`BASH_RETRIES`), each attempt echo-attributed, bash-damage line classified as chatter; exhaustion surfaces as a diagnosable NavError, not a hang. Scripted-board tests: door yields on third bash; door never yields → clean bounded error.

## Phase 10 — Live acceptance (the doc's protocol, exactly)

1. Prereqs: torch on Salad verified via `inventory`; `board-safe-to-restart` before every restart (another session may be capturing oracle transcripts).
2. Per run: restart board → run within the hour (spawner boot-fill drains) → `mmc farm`, circuit `["1/2150","1/2152","1/2156"]`, `max_seconds = 300`, `.raw` + timing capture on.
3. Pass = ends `TimeUp`, walks to finish, zero `NavError`, no "There is no exit" on the wire (grep TX log), door opened by nav, light verified lit. **Five consecutive.** Any failure: stop, diagnose from the timing log (attribution makes every association auditable), fix, restart the count from zero.
4. Afterwards: add the five raws to `re/oracle`; the corpus attribution assertion should rise toward 97.3%.

## Deferred (recorded, not dropped)

- **Render-preamble token** (Reset→`ESC[79D`→`ESC[K`→Colour through wire.rs): would make "room render" positive evidence, retire the parse.rs banner heuristic, and double-check the dark-entry case (entering dark carries the preamble; look-in-dark does not — verified in run7). Defer until after acceptance passes so any regression is attributable; echo attribution already makes unsolicited renders inert everywhere it matters.
- ~~mud-server execution-echo modeling under the round timer.~~ **LANDED**:
  `CoreConfig::command_round_seconds` gates a per-session FIFO queue, and a
  queued command is echoed from the core immediately before its reply. Measured
  from `oracle_blur_duration_timing.log` (1.15s/1.25s between consecutive queued
  moves; the queue window is completely silent — no prompt, no acknowledgement).
  Default 0 keeps every existing fixture on the run-immediately path; the shipped
  binary sets 1. `mud-server/tests/telnet_e2e.rs` now produces the double echo
  the client's scripted TCP boards used to be needed for.
- Optional live probe: does a flood-dropped command still echo? (Design is safe either way via flush+deadline.)

## Risk register (top items)

| Risk | Mitigation |
|---|---|
| Double/receipt-vs-execution echoes misattribute on a busy board | FIFO attribution + duplicate-confirms-head rule, pinned by the run5 fixture test — the exact captured sequence is the regression test |
| Suffix-fragment false positive → wrong association (the one forbidden outcome) | len floors (cmd ≥ 3, fragment ≥ 2), match only against pending entries, corpus negative assertions |
| Missing echo (flood, swallowed mid-room-block by parse.rs description accumulation) wedges a consumer | Deadlines from birth on every entry; every consumer keeps its re-ask/timeout path first-class |
| Envelope change breaks flee detection / TUI / localize (they need unsolicited events) | Invariant 3: `answers: None` events always flow; Phase 7 explicitly tests flee on unattributed blocks |
| mud-server echo breaks unrelated tests | Isolated Phase 3, full-suite gate, per-failure review |
| Deleting `asked_when`/`changed` reintroduces the raced-arrival bug | `invalidate()` discards the block outright (strictly stronger); rewritten race test pins it |
| DLL wordings guessed wrong (doors, lighting) | Both phases start with DLL string extraction + live capture; no invented wordings |
| Live acceptance blocked operationally | Torch via SYSOP SUMMON; `board-safe-to-restart` coordination; restarts scheduled per run |

## Verification

Per phase: `cargo test` (workspace) green before commit; commits tagged `feat:`/`fix:`/`refactor:`/`test:`/`doc:` per repo convention. End-to-end: the Phase 10 live protocol is the only thing that counts as done — plus `docs/board-correlation.md` status line flipped from "not yet implemented" to as-landed, and the run5/run7 sequences preserved as fixtures so the evidence outlives the captures.
