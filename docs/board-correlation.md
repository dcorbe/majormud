# Which command did this answer?

How to tell, on a MajorMUD board, which reply belongs to which command —
and why the client currently cannot, which is the root cause of the
position desyncs that end farm runs.

Status: **investigated and measured, not yet implemented.** The two fixes
in `7ddfd17` remove contributing causes; they do not address this.

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

## Shape of a permanent fix

Not implemented; recorded so it can be planned rather than guessed at.

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

### Not the fix

Counting unanswered sends at the session layer was tried and reverted
(see `7ddfd17`). Login lines are answered by menus rather than `[HP=]`
prompts, so the count inflates and never drains; and gating on it wrongly
makes the client discard the board legitimately saying "you did not
move", turning a clean re-localize into a step timeout. A proxy for
correlation is not correlation.
