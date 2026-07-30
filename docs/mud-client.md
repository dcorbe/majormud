# mud-client / `mmc`

The automated MajorMUD client. One engine, four uses:

| Command | What it does |
|---|---|
| `mmc play --profile P` | Interactive terminal session with a status bar and bot toggles |
| `mmc run SCRIPT --profile P` | Headless Lua-scripted run (oracle captures, acceptance tests) |
| `mmc path FROM TO` | Print a route between two rooms, e.g. `mmc path 1/2146 1/2156` |
| `mmc farm --profile P` | Walk a patrol circuit, farming each stop |

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

### `[farm.nav]`

- **`step_timeout_ms`** (15000) — per-step arrival deadline.
- **`bash_doors`** (true) — fall back to bashing a door that `open` will
  not shift. Bashing costs HP (*"You take %d damage for bashing the
  door!"*) and needs a weapon, so it is a switch; turning it off makes a
  locked door a hard stop.

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
