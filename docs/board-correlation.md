# Which command did this answer?

How to tell, on a MajorMUD board, which reply belongs to which command —
and why the client currently cannot, which is the root cause of the
position desyncs that end farm runs.

Status: **IMPLEMENTED and accepted, 2026-07-31.** The session owns a
`Correlator` (`correlate.rs`): the writer task registers each send in
wire order, the reader attributes every parsed event, and
`Correlated::answers` rides inside the broadcast. Gate, Navigator, and
StopState stopped guessing by having zero of them correlate — they match
their own send ids by equality. The acceptance below passed five
consecutive times on the live board (accept-run1..5 in `re/oracle`),
board restarted before each run, zero `NavError`, the 1/2150 door
opened by the walk itself, the dark rooms lit and confirmed lit.

The shape below was amended in the building — the live captures
disproved parts of the original sketch (a busy board echoes a command
TWICE, receipt then execution; replies are strictly FIFO; the board's
unsolicited din must complete nothing, so retirement runs on a closed
reply grammar keyed by our own send vocabulary; wrong-vs-missed decides
every ambiguity). The as-built rules live in `correlate.rs`'s module
doc and its transcribed run5/6/7 fixtures.

## The failure

A farm run ends with:

```
farm: at 1/2150: desync: expected "Dungeon, Entrance", saw "Newhaven, Arena"
```

The walk stepped north out of the Arena and the *Arena's own* room block
satisfied the step, so `Navigator::goto` recorded an arrival that never
happened. Its `current` then ran ahead of the character, and every
following step compounded it — ending in a stray `s` into a wall
("There is no exit in that direction!").

Reproduced on the live board on 3 of 4 runs of the Newhaven cave-bear
circuit, and on the pre-refactor client too. It is not new, and it is not
caused by the stop-state work; that work made dark rooms get visited more
often, which made it frequent enough to notice.

## Why the client cannot tell

Two independent gaps, both in the same place: **nothing associates a
reply with the command that caused it.**

1. **`Gate` acknowledges on the next prompt.** A prompt is not evidence
   that our command was answered. The board sends them in bursts, and it
   re-prompts unsolicited whenever async output disturbs a dangling
   prompt (`[HP=31]:[HP=32]:` on one physical line — `mud_core::game::
   reprompt_disturbed` models this faithfully). In a room with a fight in
   it, a prompt from somebody else's blow clears our in-flight command.

   The prompt has two templates. HP only paints its status inside the
   frame, `[HP=42 (Resting) ]:`. With a pool it paints the status after
   the frame, glued to the echo, `[HP=36/MA=12]: (Resting) look`. The
   parser reads both and the echo reaches the correlator clean.

2. **`Navigator` takes the next room block it sees.** `wait_room` returns
   on the first `RoomSeen`, whoever asked for it. `goto` drains the
   channel before sending a step, which removes blocks already *received*
   — it cannot remove one the board has not sent yet.

Together: the runner sends `look`, believes it is settled (a stray prompt
cleared the gate), hands over to the walk; the walk sends its step; the
`look`'s block arrives and satisfies it.

## What the board actually gives us

**The board echoes every line it is sent, and the reply follows the
echo.** Verbatim from a capture:

```
[HP=51/MA=10]:d                        <- echo of our command
ESC[0;37;40m ESC[79D ESC[K ESC[1;36m   <- room-render preamble
Newhaven, Arena
...
Obvious exits: open door north, up
```

Our own parser already emits this. It is in the event stream today and
every consumer ignores it:

```
Prompt(hp=52)
Line("n")                       <- the echo
RoomSeen("Dungeon, Entrance")   <- the block that answers it
```

### Measured

`tests/echo_correlation.rs`, over the 102-file `re/oracle` corpus:

| | |
|---|---|
| room blocks | 6344 |
| introduced by a known command echo | 5967 (**94.1%**) |

On the eight recent live cave-bear captures: 437 of 449 (**97.3%**).

The shortfall is understood, not noise:

- **Login banners.** `=-=-=-=-...` introduces the game-entry render,
  which no command asked for. Correctly *unsolicited*.
- **The `>` menu prompt** outside the realm.
- **Split echoes**, ~0.4%: async output arrives mid-echo and the line
  breaks up, so `look` reaches the parser as `l` + `ook`. Seen live as
  `[HP=47/MA=6]:l` `The cave bear swipes at you...` `ook`.

A consumer must therefore treat "no echo" as **unsolicited**, never as
"assume it answers my last command" — assuming is precisely today's bug.

## How the reference clients do it

`docs/mirrors/github-RonPenton-OmegaMUD` — `Parsing/RoomParseState.cs`
carries the render grammar as a comment, and it distinguishes renders by
what triggered them:

```
Enter Game:  Back->Erase->Color->Text->Newline
Hit enter:   Back->Erase->Color->Text->Newline
Look:        Reset->Back->Erase->Color->Text->Newline
Move <dir>:  Reset->Back->Erase->Color->Text->Newline
Look <dir>:  Back->Erase->Color->Text->Newline
```

Confirmed on our board: `ESC[0;37;40m` (Reset) `ESC[79D`
(CursorBackward 79) `ESC[K` (EraseLine) `ESC[1;36m` (room-name colour).

Two things follow.

- **OmegaMUD parses the ANSI control stream, not stripped text.** Cursor
  and erase tokens are first-class (`MUDTokenType.CursorBackward`,
  `EraseLine`). Our `wire.rs` strips ANSI before classification and keeps
  only the opening SGR, so the render preamble is discarded. That is also
  why `parse.rs` needs a workaround for banner art painting `1;36` and
  masquerading as a room name — the preamble would have settled it.
- **OmegaMUD tracks what it asked.** `MainWindow.OutputStateMachine.cs`
  keeps `isMoving` / `isLooking` and clears them on wordings that are
  *different for each*: a look is refused with `"The door is closed in
  that direction!"`, a move with `"The door is closed!"`.

  Our `nav::DOOR_BLOCKED` contains `"the door is closed"`, which matches
  **both**. The look-refusal is currently read as a move-refusal.

`ESC[79D` on its own is not a room marker — it appears 3568 times across
the live captures as the board's general line redraw (the same mechanism
`wire.rs` resolves as anti-bot backspacing). Only the full
Reset→Back→Erase→Colour signature marks a render.

## Shape of a permanent fix (original sketch; see Status for as-built)

As-built: items 1-3 and 5 landed (amended — see Status). Item 4, the
render-preamble token, is the one live deferral: carrying
Reset→`ESC[79D`→`ESC[K`→Colour through `wire.rs` as a token would make
"this is a room render" positive evidence and retire the banner-art
heuristic in `parse.rs`. Deferred deliberately until after the
acceptance so any regression stays attributable; echo attribution
already makes unsolicited renders inert at every consumer.

The original sketch, kept as written; the paragraph above says what became of it.

## Server side: what mud-server now speaks, and what it still owes

Written 2026-07-31, after reproducing the measured protocol in
`mud-server`/`mud-core`. Each item below was verified against the corpus
in `re/oracle`, not taken from these notes.

**Landed.** Telnet ECHO/SGA negotiation with password suppression; CP437
in both directions (one shared table in `mud_core::cp437`, so client and
server cannot drift); BS/DEL line editing; the two render preambles
(`ESC[0;37;40m ESC[79D ESC[K ESC[1;36m` for look and move,
without the reset for game entry and the bare-Enter re-show) and
`ESC[79D ESC[K` as the generic burst erase; `SET WARNING ON|OFF`; the
round-timer command queue and its execution echo; the anti-bot junk +
backspace in direction words.

**Wordings the client's reply grammar expects that we cannot emit yet.**
The audit found no case where `mud-core` prints a *wrong* wording — every
gap is a command that does not exist, so nothing reaches the string:

| Wording | Blocked on |
|---|---|
| `The door is closed in that direction!` (look) vs `The door is closed!` (move) | `look <direction>` — `Command::Look` takes no argument |
| `There is a closed door in that direction!` | same |
| the OPEN/CLOSE replies (`is now open`, `was already open`, `successfully unlocked`, `The door is locked`) | no `open`/`close` verb |
| the BASH roll (`Your attempts to bash through fail!`, `You bash the door open and walk through`, `You bashed the door open.`) | no `bash` verb |
| `The room is %s - you can't see anything` | darkness and light are unmodeled; no `light` verb |
| `You may not enter that room while in combat.` | movement is not refused during combat |

The client's side of this contract is `mud_client::correlate`'s wording
tables — the closed reply grammar it retires pending commands on. Adding
any of these commands means matching those strings exactly, including the
look-vs-move door split, which is load-bearing: one shared constant
matching both once read a look-refusal as a move-refusal.

**A divergence worth knowing about.** `move_player` gates on the exit's
LOCK state, never on `exit.door_closed`. `exit_entry` does read it, so
the exits line renders `closed door north` for a closed-but-unlocked
type-2 door and walking north then succeeds silently. Monster roaming
reads it too. Closing that gap belongs with the OPEN command family,
since a door you cannot open is not one you should be stopped by.

1. **Correlate on the echo, not the prompt.** A command is answered when
   its echo appears and the reply that follows it completes. `Gate`
   should acknowledge on the echo; `Navigator` should accept the room
   block that follows the echo of the direction *it* sent; `StopState`
   should consider a `look` answered by the block after *its* echo. All
   three then stop guessing independently.
2. **Treat an echo-less render as unsolicited.** It is a real event — a
   game-entry render, someone else's doing — and it must never satisfy a
   pending step.
3. **Handle the split echo.** ~0.4%, and the fragment is a suffix of the
   command (`ook` of `look`). Tolerate a fragment match, or fall back to
   the deadline rather than to a wrong association.
4. **Consider keeping the render preamble.** Carrying
   Reset→Back→Erase→Colour through `wire.rs` as a token would make "this
   is a room render" positive evidence instead of a colour heuristic, and
   would retire the banner-art workaround in `parse.rs`.
5. **Split the door wordings.** `"the door is closed in that direction"`
   (look) and `"the door is closed"` (move) mean different things and
   currently collapse onto one constant.

## Acceptance

**100% success on the Newhaven Arena / cave bear loop.** It is the
simplest loop in the game — three adjacent rooms, one door, one boss —
and if position tracking cannot survive it, the approach is wrong and
should be abandoned rather than tuned.

Concretely, and all of it on the live board, not the fixture server:

- `circuit = ["1/2150", "1/2152", "1/2156"]`, `max_seconds = 300`
- **five consecutive runs**, each ending `TimeUp` and walking to the
  finish room
- **zero** `NavError` of any kind — no `Desync`, no
  `Expect(Timeout { needle: "room block after movement" })`, no
  `There is no exit in that direction!` reaching the board
- board restarted before each run (the spawner is player-driven and the
  boot-fill drains within the hour)

Two things this criterion drags into scope that are easy to wave away:

- **The door at 1/2150 north is part of the loop.** A board restart
  re-locks it and the navigator's own open/bash currently gives up, so
  today it takes a manual `~/mmc-probe/opendoor.lua`. Needing a hand-run
  script to start the simplest loop in the game is a failure of the same
  system. Either nav opens it reliably or we know precisely why it
  cannot.
- **The dungeon rooms are dark**, so two of the three stops send no room
  block at all. Position there is dead reckoning by definition — the
  correlation work has to make that dead reckoning *sound*, which is what
  `blind_position` started and did not finish.

A run that dies at 80s is not a partial pass. The measurements in this
document are diagnostics, not a score.

### Lighting is part of it, and comes first

Walking a dark room blind is the LAST resort, permitted only when there
is genuinely no way to light it — no torch, no lamp, no lantern, no
spell, or every one of them exhausted. Anything else is stumbling from
room to room and guessing at where we ended up, which is the bug this
whole document is about, wearing a different hat.

The pieces already exist and are the right shape (`sheet.rs`):

- `LIGHT_ITEMS` — torch (175, 800 uses), lantern (176, 2400), brass lamp
  (286, 1800), moon-lamp (1153, 4000), scaled lantern (1233, 6000)
- `LIGHT_SPELLS` — starlight (26), light, continual light
- `light_plan()` prefers a carried source over a spell, because an item
  costs no mana. That preference is right; keep it.

What is wrong is how they are used:

1. **The plan is read ONCE, at run start** (`read_light_plan`). A torch
   that burns through its uses mid-run, or mana that comes back after a
   rest, never changes the answer. The plan has to be re-derived when the
   situation changes, not cached for the life of the run.
2. **Nothing verifies the light took.** The live failure was `cast star`
   against `MA=7`, answered "You attempt to cast starlight, but fail." —
   and the runner carried on into the dark anyway. The board says whether
   it worked ("You lit the %s.", DLL 0xdb52d) and that answer must gate
   what happens next.
3. **A lit source is treated as a per-visit command.** A torch is lit
   once and then burns; re-issuing `light torch` at every stop is waste
   and noise. Track that it is already lit.
4. **Exhaustion is not modelled.** Out of mana is recoverable — rest and
   retry. A torch with no uses left is not; it needs a different source.
   These need different responses and currently get the same one.
5. **`MAX_LIGHT_ATTEMPTS = 1` is the wrong lever.** It was set to 1 to
   stop the runner flooding the board with `cast`/`look` pairs, which was
   the right call for that symptom. But the real rule is not "try once",
   it is "try what can actually work, verify it, and only give up when
   nothing can". Retrying a spell that failed for want of mana is
   pointless; resting and then casting is not.

So the order is: light the room, confirm from the board that it is lit,
then navigate by room block like anywhere else. Dead reckoning
(`blind_position`) stays as the floor for the genuinely unlightable case
— and when it is used, it has to be sound, which is what the correlation
work is for.

Salad carried no light source when this was written, which is why the
live runs fell through to `cast star`; a torch was conjured on for the
acceptance (`SYSOP SUMMON torch`), and getting one onto a character
remains an operational prerequisite, not a code change.

### Not the fix

Counting unanswered sends at the session layer was tried and reverted
(see `7ddfd17`). Login lines are answered by menus rather than `[HP=]`
prompts, so the count inflates and never drains; and gating on it wrongly
makes the client discard the board legitimately saying "you did not
move", turning a clean re-localize into a step timeout. A proxy for
correlation is not correlation.
