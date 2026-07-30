# mud-client / `mmc`

The automated MajorMUD client. One engine, four uses:

| Command | What it does |
|---|---|
| `mmc play --profile P` | Interactive terminal session with a status bar and bot toggles |
| `mmc run SCRIPT --profile P` | Headless Lua-scripted run (oracle captures, acceptance tests) |
| `mmc path FROM TO` | Print a route between two rooms, e.g. `mmc path 1/2146 1/2156` |
| `mmc farm --profile P` | Walk a patrol circuit, farming each stop |

`mmc play` is **never paced** — see [`pace_ms`](#pace_ms). `mmc farm`
prints a live feed by default — where the character is, what
it is fighting, what it killed, and HP whenever it changes. `--watch`
turns it into the firehose (every line the board sends); `--quiet`
restores the old behaviour of printing nothing until the run ends. For a
permanent record use `--capture BASE`, which writes `BASE.raw` (the raw
socket bytes) and `BASE_timing.log`. Keep captures OUT of `re/oracle/` —
two corpus tests count the files in there.

It targets the live MBBSEmu board (WCCMMUD 1.11p-WG) and the in-repo
`mud-server` reimplementation. The two differ in more than the login
dance — see [Target differences](#target-differences).

> **`mmc farm` has no dry run.** Building the plan validates the whole
> configuration, but the command then connects and starts farming as soon
> as the start room checks out. Running it "just to check the config"
> will farm.

## Profiles

A profile is TOML describing one character. Keep it **outside this repo**:
it holds a plaintext password and there is no ignore rule that would catch
a stray `*.toml`.

```toml
target = "mbbs"            # "mbbs" (live board) | "rust" (mud-server)
host = "127.0.0.1"
port = 2327
username = "rangerdan"     # BBS account, not the character name
password = "..."
pace_ms = 2500             # minimum ms between sends

disable_evil_warnings = true

[bot]
auto_combat = true
auto_heal = true
auto_flee = true
auto_get = true            # coins only; floor items are never announced
heal_at_percent = 60
flee_at_percent = 30
heal_command = "rest"
ignore = ["guard", "healer"]
# max_hp omitted: the runner probes the board for it at startup

[farm]
content = "/abs/path/re/mmud_wgnt.sqlite"
start = "1/2146"
circuit = ["1/2156"]
finish_at = "1/2146"
loops = 0                  # 0 = until stopped
max_seconds = 3600         # 0 = unlimited

[farm.nav]
step_timeout_ms = 15000
bash_doors = true
```

### `pace_ms`

Flood control is measured: **eight sends 1.3s apart** earns *"Why don't
you slow down for a few seconds?"*. The MbbsEmu default of 1500 is only
just outside that — fine for a burst, too fast to sustain. 2500 buys
margin for a long unattended run. The Rust server needs no pacing.

**`mmc play` ignores this entirely.** Pacing is flood control and flood
control is for automation; a person typing is their own rate limiter, and
applying a farm-tuned 2500ms to a human delays every command after the
first in a burst by the full interval — 2.5 seconds per step when
walking. If interactive play ever does trip the board's limit, the board
says so and you can slow down.

### `disable_evil_warnings`

Turns the character's Warn-on-Evil flag off at login. The board refuses
attacks on unprovoked (behaviour 0/4) monsters while it is on, which is
most of the shipped bestiary.

This is **a real change to the character**: evil acts then land and accrue
fame, moving the legal level toward Criminal. Opt-in, and at the profile's
top level rather than under `[bot]` because it mutates persistent state
whether or not the bot ever swings.

Note the two targets spell it differently, which the client handles: the
board takes `SET WARNING ON|OFF` (an explicit setter, so idempotent),
while `mud-server` implements a bare `set evil` toggle. That divergence is
the reimplementation's, not the board's.

### `[farm]`

- **`start`** — where the character *stands at login*, not where the
  farming happens. It is verified by room name, then the runner walks the
  first leg onto the circuit itself. Beware: names repeat, and
  `verify_start` compares names only. "Newhaven, Narrow Road" is two rooms
  (1/2146 and 1/2151).
- **`circuit`** — the stops, in order; the lap wraps from last to first.
  A one-room circuit is legitimate and is the right shape for a dead-end
  lair.
- **`finish_at`** — where to leave the character when the run stops.
  Without it a run ends wherever it happened to be, which for a lair
  circuit means standing among the monsters, linkdead, until somebody
  walks it out. It fires on **every** route out including Ctrl-C, and is
  skipped only on a death. Validated at build time from every stop the run
  can end at, not just the start.
- **`depart_at_percent`** (80) — never start a leg below this.
  `interrupt_at_percent` (50) stops one that gets hurt on the way; it must
  not exceed `depart_at_percent` or the plan is rejected.
- **`idle_poke_ms`** (5000) — an idle board sends *nothing*, not even a
  prompt, for minutes at a stretch. The poke is the only thing that
  produces prompts at an empty stop, and it doubles as a respawn check.

### `combat_idle_prompts`

How the bot decides a fight has ended when nothing recognisable said so.

Death lines are per-template **prose**. Of the 1085 monsters shipping a
death record, **67** say "falls to the ground" and **1018** say something
else — *"The filthbug collapses, its legs curling tightly around it."*,
*"The skeleton crumbles into a pile of dust."* A further 14 have no death
record at all. Matching them all would mean carrying a thousand strings.

So the bot ends a fight on three signals instead: the classic death
phrase, the experience line that follows any kill of ours ("You gain %s
experience.", DLL 0xbc65f), and — as the backstop for an exp-less kill or
a monster somebody else finished — `combat_idle_prompts` (3) prompts with
no blow struck either way. A fight in progress refreshes that on every
swing, hit or miss, so it only counts genuine silence.

This matters more than it sounds: the farm runner reads a latched bot as
a fight in progress, so it stops poking the room and stops counting the
stop as idle. A missed kill does not merely lose a target — it wedges the
whole run.

### `[farm.nav]`

- **`step_timeout_ms`** (15000) — per-step arrival deadline.
- **`bash_doors`** (true) — fall back to bashing a door that `open` will
  not shift. Bashing costs HP (*"You take %d damage for bashing the
  door!"*) and needs a weapon, so it is a switch; turning it off makes a
  locked door a hard stop.

## Full-screen board screens (`train stats`)

Some board screens are **keystroke**-driven rather than line-driven —
`train stats` opens one, via `edit_character_stats` and an FSD room — and
they are navigated with ANSI cursor keys. MBBSEmu's FSD reads `\x1b[A`
and friends directly, so nothing else will move between fields.

`mmc play`'s line editor claims the arrow keys for its own cursor and
history, which is right for typing commands and useless here. **Ctrl-P**
swaps between the two:

- **line mode** (default) — type commands, arrows move the cursor and walk
  history, Enter sends a line with CRLF.
- **passthrough** — every key goes straight to the board as raw bytes:
  arrows as `\x1b[A`/`[B`/`[C`/`[D`, Enter as a *bare CR* (an FSD screen
  is not line-oriented, and the LF of a CRLF reads as a second key),
  Backspace, Tab, Esc, Home/End and Ctrl-letters as their control codes.

Ctrl-Q still quits from either mode, and the toggle itself is never
forwarded — otherwise leaving passthrough would drop a stray byte into
whichever field was selected.

If you are ever stuck on one of these screens with no way to drive it,
**disconnecting is safe**: `train_level` banks the character points into
`player+0x6e2`/`+0x6fa` before the stat screen ever opens, so abandoning
it loses nothing and `train stats` can be re-entered later.

## The status bar

`mmc farm` reserves the bottom terminal row and scrolls the feed above it
(DECSTBM, the same mechanism `mmc play` uses):

```
 attacking cave bear  |  HP 23  MA 8  |  Small Cavern [1/2156]
```

The activity is **published by the runner**, not guessed from board
output. That distinction matters: a watcher can infer "attacking" from
combat lines, but waiting-to-depart, travelling and wedged all look
identical from outside — and telling those apart is the entire point. See
`farm::Phase`.

The room *number* comes from the graph, since the board only ever prints
the name. The runner's own position is preferred; failing that the name
is resolved, which is only possible when it is unambiguous.

The bar is skipped when stdout is not a terminal, so piping the feed to a
file gives lines rather than escape sequences.

## Doors

A route through a door or gate opens it rather than stopping. Door and
gate exit types are **2, 7 and 0xb** (`re/docs/theft.md` §8.1); types 6
(hidden), 9/0x18 (traps), 0x10 (timed), 0x14 (alignment) and 0x16/0x17
(spell/ability gates) are *not* doors and never provoke an `open`.

`open` is tried before `bash` because it is free — the board answers *"The
door was already open."* when there was nothing to do. Only the graph
decides an exit is a door; reacting to the board's *"the door is closed"*
alone would mean sending `open` at exits that have none, one wasted
command per step against flood control.

Both direction forms work (`open n` and `open north`, verified live).

## Working out where the character is

Two answers, because they have different information:

- **`Navigator::localize(at, name)`** — from a room name alone. Limited to
  `at` and its immediate neighbours on purpose: names repeat, so a global
  name search would confidently return the wrong room.
- **`Navigator::localize_view(at, room_block)`** — from a whole room
  block. Searches the entire graph and narrows by the observed exits,
  which is what separates same-named rooms: 1/2146 shows
  north/east/west/down where its twin 1/2151 shows only north. Answers
  only when exactly one room survives, and says nothing when several do.

Observed exits are matched as a **subset**, never an equal set: hidden and
action exits live in the graph but are deliberately absent from the
board's "Obvious exits" line.

Exits are display tokens, not commands — `closed door north`, and for
trapdoors `above`/`below` rather than up/down.

## Target differences

| | live board (`mbbs`) | `mud-server` (`rust`) |
|---|---|---|
| Login | `Username:` → `Password:` → menu `A` → `[MAJORMUD]:` → `E` | `Account:` → `Password:` |
| Evil warnings | `SET WARNING ON\|OFF` (setter) | `set evil` (toggle) |
| Unknown command | **said out loud**, not an error | falls through to say |

That last row is the one that bites. The board does not reject a command
it does not know — it says it, and leaves the world untouched. A wrong
verb is therefore a silent no-op, so any fake-board test fixture must
model the say-it-back behaviour or it will accept a wrong verb and pass
while the feature does nothing.

## Testing

- `tests/bot.rs`, `tests/farm.rs` — the pure decision cores, no sockets.
- `tests/nav.rs`, `tests/farm_live.rs`, `tests/live_server.rs` — against
  the in-process `mud-server`.
- `tests/dialect.rs`, `tests/nav_doors.rs` — against scripted boards, for
  behaviour `mud-server` does not model (the MBBSEmu login, the open/bash
  command family).
- `tests/bot_corpus.rs`, `tests/farm_corpus.rs`, `tests/parse.rs` — replay
  the real transcripts in `re/oracle/`.

The corpus goldens count files in `re/oracle/`, which the **server** track
also writes into during oracle runs. They will fail spuriously while
another session is mid-capture.
