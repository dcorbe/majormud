# One engine, one walker, one channel, one log

Design for the mud-client's automatic behaviour, settled 2026-09-14.

## Why

Heal requests went unanswered on 2026-09-14 for the fourth session in a
row. Each time the heal code was right. The failures were a parser gap,
a missing config key, an assist that was never switched on, and a leader
whose farm has no party heal at all. None of them left a trace in the
timing log, so every one cost a replay of a capture to find.

For a follower to answer one `@heal` today, ten conditions across four
files must all hold: no job running, the assist built by `/bot` or
`assist_play`, the spell book read, no self cast this prompt, a
non-empty party book, `[party].heal` on, a parsed role, the asker in the
parsed member list, the row due, and mana plus the round clock. Nothing
logs any of them. The farm carries its own copy of the self heal, the
buffs, the light and the Bot, and no copy of the party heal. The assist
is rebuilt at four sites and a hand-written function lists which state
survives. This spec replaces that shape.

## Rules

These were decided in conversation and are not open in the plan.

1. One engine owns every automatic policy. Nothing automatic is job
   specific.
2. Every job drives the walker and sends nothing but steps. This is
   absolute: farm, go, bank and recover alike.
3. The engine never sends a step. A flee is a decision the engine
   publishes and the walker carries out by retracing the trail.
4. The timing log is the one log. Switch state, every decision, every
   skipped decision and every reply that did not fit are written next to
   the RX and TX lines.
5. The parser is not redesigned. Its gaps are made visible instead.
6. A contract violation crashes. A board surprise is logged.

## The engine

One value per window, built once at session start, alive until the
session closes, never rebuilt.

It holds: the switch, the `BotConfig` policy table, the party config,
the Bot core, the heal, buff, light and party cast states, the ask, wait
and deposit states, the set of members introduced to, the heal watch,
the round clock, the room model and the book length last read. This is
everything that is spread today over `Bot`, `AssistCasts`, the farm's
`Casts` and `StopState`, and the window's `assist_watch` and
`assist_book_seen`.

Its one entry point is `on_event`, called with every correlated event
whether or not a job runs. It returns the decisions it made. A decision
is a send with its reason or a skip with its reason. The session writes
each as a DECIDE line. The window prints the ones it prints today as
notes.

The switch is one boolean. `/bot` flips it and `bot.assist_play` sets it
at start. Off, `on_event` still folds every event, so the room model,
the party rows and the book stay current, but it decides nothing. One
DECIDE line is written when the switch changes. `BotConfig::switched_off`
is deleted.

A settings reload updates the table in place. Cast states are rebuilt
only when a spell choice changed, the rule `casts_need_rebuild` states
today. Nothing else is ever rebuilt. The book is re-read when the book
tracker reports a new length.

The engine publishes a status value on a `watch` channel, the way the
session publishes state today. The status holds:

- `idle`: no fight engaged, no cast out, not resting, no loot to sweep,
  no flee wanted.
- `quiet_since`: when the last attributed room block held nothing to
  fight or take, or none.
- `flee`: set while the engine wants to leave the room.
- `vitals`: hp and mana as percents, and whether they are known.

The engine never sends a step, never rebuilds itself, never reads the
profile file, and never treats a reply it did not ask for as an answer.

### Policies the engine owns

Fight, loot, keys, heal, rest, flee, sneak, hide, buffs, light, and the
party's asks, answers and introductions. Each is gated by its profile
key and by the switch, and by nothing else outside the engine.

### The party heal, restated

On a prompt, with the switch on, `[party].heal` on, a role other than
none and a non-empty party book: if no cast of this character's own went
out this prompt and none is in flight, the party cast state is asked
for a cast against the party's health rows. The rules inside that state
are unchanged. Every skip has a reason: switch off, no role, empty book,
held this round, in flight, no row due, no mana. This runs the same way
for a leader mid-farm as for a follower standing still, because nothing
stands between the event and the engine any more.

### Flee, restated

A flee is a decision about the room, not about the number. Below the
flee mark and the room holds a threat, meaning engaged or something
attackable listed here: the engine sets `flee` in its status and holds
it until the block that answers the retreat arrives. Below the mark and
the room holds nothing: the rest branch runs as it would at any low
number. Arriving in a room that holds a threat while still below the
mark: `flee` again. Arriving in an empty room: rest.

The old rule fled by the first exit on every arrival while below the
mark, because the fled latch was cleared by every room block and the
flee branch ran before the rest branch. The test
`flees_once_per_room_not_once_per_prompt` asserts that behaviour and is
rewritten to assert this one.

## The walker and the itineraries

The `Navigator` becomes a window-level value like the engine, alive for
the whole session. Jobs borrow it. It keeps what a step needs: legs,
steps, doors, keys, the sneak armed before a step and the backstab
opener on arrival.

### The trail

The walker keeps a trail: the direction of every Move released on the
channel, whoever sent it, with its reverse. A farm leg, a go, a drag and
a direction typed by hand all add to it. Reversing a direction needs no
map, so the trail works in hand play with no known position. A Move
with no reverse, a one-way exit or a drag with no direction, is an entry
that ends a retreat.

### The retreat

When the engine's status shows `flee`, the walker sends the reverse of
the last trail entry as a step and pops it. On arrival the engine judges
the room. If it wants to flee again the walker retreats again. If the
trail is empty or its last entry has no reverse, the walker sends the
first exit of the room instead, and a DECIDE line on the flee unit says
the trail ran out.

The room behind is the one just walked through, so it is the room the
character knows most about, and on a farm it was cleared on the way in.

### Displacement

The walker's `current` is trusted only while every Move on the channel
was its own. A room block nobody asked for that names a different room,
a drag, sets `displaced` with that block. The walker publishes
`displaced` on its own `watch` channel, next to the engine's. A flee the
walker sent never displaces anything. The itinerary answers displacement by waiting for
idle, asking the walker to localize from the block, and walking back.

### Itineraries

An itinerary is an async task with a plan, the borrowed walker, the
engine's status channel and the session for notes. Farm, go, bank and
recover are the four. Each is the same loop: pick the next room, wait
until the engine is idle, walk a leg, stand there until the plan says
leave, repeat. Everything that happens while standing is the engine's.

The four rules:

1. A leg starts only when the engine is idle.
2. A stop ends only when the engine has been quiet for the dwell. A
   leader's party hold overrides the dwell.
3. The departure gate reads the engine's vitals. Below the rest mark
   the itinerary waits, and the engine's own rest brings the number back.
4. Displaced is answered as above. Nothing else is rebuilt.

The farm's phases all fit: locating the start, legs, stops, party
holds, the bank errand and the finish walk. A hold is "do not start the
next leg yet". The go job is one leg and no stop.

## The channel

The farm's `Gate` moves into the session. `Session::send` keeps its
signature and returns the `CmdId` as now, so no call site changes shape.

Rules:

- One queue. Strict first in, first out. One command in flight.
- A command is released when the previous one was acknowledged by the
  correlator or expired by its deadline.
- An expiry writes an EXPECT line naming the command and the reply kind
  it waited for.
- Flood control is the Gate's backoff, unchanged.
- A resend of the in-flight command jumps the queue. This is the one
  exception, carried over from the Gate.
- Every Move released goes to the walker's trail with its sender.
- Every release goes to the engine's cast and watch states through the
  `on_sent` hook they already use.
- The keyboard's line goes through the same queue, so a typed command is
  ordered with everything else and logged the same way.
- No priorities. The engine sends at most one command per prompt and
  the walker one step at a time, so the queue is never deep enough for
  a priority to matter.

On death or a closed connection the queue is dropped and each dropped
command is logged once.

## The log

Two tags join RX, TX and `!!` in the timing log. Every line is the
timestamp, the tag, a unit, and a short payload with the reason in
brackets. Units: switch, fight, loot, heal, party, rest, flee, buff,
light, sneak, walk, channel.

```
1789414326.284 DECIDE party: cast mend blueberry (row 64 < 70, minor, mana 41)
1789414326.284 DECIDE party: skip blueberry (held, self cast this round)
1789414326.284 DECIDE party: skip salad (not due, healed since 62)
1789414317.005 DECIDE switch: off (profile assist_play=false)
1789414331.400 EXPECT party: roster header with no rows
1789414337.900 EXPECT channel: 'cast mend potato' expired unanswered after 6s
1789414331.400 EXPECT party: 'cast mend potato' refused ("You don't see potato here.")
1789414500.100 EXPECT walk: block for step 'n' named "Inner Ward", expected "Guard Post"
```

A send is written every time. A skip is written when its reason
changes, not on every prompt. This is the Bot's existing rule, fire once
and re-arm on a change of situation, applied to the log.

An EXPECT is written for: a modelled reply that never came, a reply the
correlator could not fit, a roster or room block that was structurally
empty where the situation says it cannot be, a cast the board refused
by name, and a step whose block named the wrong room. The raw line goes
in the brackets.

Not logged: bytes already in RX and TX, party notes that only echo a
DECIDE line, and the window's own drawing.

## The book tracker

`Session::set_sheet` has one caller today, the realm entry probe, which
collects the spell listing as a transcript bounded by three seconds. A
`spells` typed later is opaque to the correlator and nothing collects
it, so a spell trained mid-session is never castable until the window
is reopened.

The correlator gains a `Spells` kind for `spells`, `powers` and the
abbreviations the board accepts for them. The reply is completed by the
ordinary prompt that follows the listing, the same shape as `Stat`. The
session gains a book tracker shaped like `StatTracker`: armed by the
send, fed by attribution, and replacing the book when the listing
closes. The engine re-reads the sheet when the tracker reports a new
length, which it already does on a length change. The realm entry probe
becomes a send of the listing command and a wait on the tracker.

## The correlator's echo guard

A block completes a look only if the look's echo was seen first, and an
echo is credited to the oldest not yet echoed entry whose text matches.
A stale un-echoed `look` in the queue therefore takes the echo of the
next `look`, the block answers the stale id, and the sender waiting on
its own id never sees it. A job's opening look that meets this ends the
job with no room block after two tries.

The channel removes the cause: with one command in flight, no un-echoed
entry can sit ahead of a new send, because the previous entry was
acknowledged or expired first. The correlator additionally refuses to
credit an echo to an entry sent after the line arrived, which the
channel's ordering already guarantees, so a regression trips an
assertion rather than a stall. Expiry writes EXPECT, so the next stall
names the command that went unanswered.

No capture of a stalled start exists on disk, so the second candidate,
a block that never closed because its exits line was not last, stays
open. The EXPECT line is what will settle it.

## Error handling

- The engine never errors. A refused cast marks its source dead as now.
  An unanswered cast is given up on the next prompt as now. A missing
  spell is a refusal note at book read as now. Everything it cannot do
  is a DECIDE skip.
- The channel expires, logs, and releases the next.
- The walker's step failures keep their names: timeout, no such exit,
  combat blocked, blind, wrong room. Each is an EXPECT line and an error
  to the itinerary. A retreat with no trail is a DECIDE skip on the flee
  unit.
- An itinerary ends the way jobs end now: died, time up, too hurt,
  arrived, or the walker's error. It never rebuilds the engine or the
  walker.
- Contract violations are assertions: the engine handed a Move to send,
  a second command released while one is in flight, an itinerary
  starting a leg while the engine is not idle.

## Tests

Each rule gets its test before its code, and the test is seen to fail
first. For each gate in the engine the test also removes the gate and
shows the test catch it. The suite runs with one build job and one test
thread.

Engine, unit: switch off folds and decides nothing, one DECIDE line on
the change. Table reload keeps cast times unless a spell choice changed.
Flee only with a threat present, rest in an empty room below the flee
mark. The party heal decision with the inputs a leader has mid-farm.

Channel, unit: one in flight, first in first out, resend jumps. Expiry
writes EXPECT and releases the next. A typed command is ordered with an
engine send. A stale un-echoed look cannot take a job's echo.

Walker, unit: the trail grows from every released Move including a
typed direction. Retreat sends the reverse and pops. An entry with no
reverse ends the retreat. An engine look between steps leaves `current`
untouched. A drag's block sets `displaced` and localize resolves it.

Book tracker, unit: a second listing replaces the book, `powers` is read
the same way, the abbreviations are recognised, and the engine's next
tick can cast the new spell.

Scripted board, end to end: a follower without `assist_play` hears
`@heal`, the log shows DECIDE switch off, nothing is sent. The same
follower with the switch on casts. A leader on a farm hears `@heal`
between two steps and the cast goes out before the next step, with the
step's block still attributed to the step. The existing party scripted
tests stay.

Log, by replay: a test reads the timing log a scripted run produced and
asserts the tag shapes, and that a skip appears once per reason change.

Every test that reaches into the farm's `Casts`, `StopState` or its
private Bot goes with that code. The plan lists them by name, and the
behaviour each covered is restated against the engine or the itinerary
contract.

## What is deleted

- `AssistCasts`, `carry_party`, `new_assist` and its four call sites in
  window.rs, `assist_watch`, `assist_book_seen`.
- `BotConfig::switched_off`.
- The farm's `Casts`, its per-stop Bot, its `Gate`, and the fight, loot,
  heal, light and buff arms of `farm_stop`, including the block at
  farm.rs:3901.
- The same copies in bank and recover.
- The transcript read of the spell listing in `probe_sheet`.
- The name-mismatch flee detection in `farm_stop` and its rebuild of
  the Bot, Gate, watch and stop state.

## Live assumptions

Marked here because no capture shows them. The first live run's EXPECT
lines confirm or correct each.

- The wording the board uses to refuse a party cast at a name not in
  the room.
- Whether a roster ever prints its header with no rows in a state that
  is not a parser gap.
- The block a flee's arrival paints, and whether a retreat through a
  closed door is answered the way a walk's open is.
- The abbreviations the board accepts for `spells` and `powers`.

## Out of scope

- Reworking colour classification or the block builder in the parser.
- Priorities on the channel.
- The exit the fallback flee picks when the trail is empty. It stays the
  first exit.
- Any change to the party wire vocabulary.
