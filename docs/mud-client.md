# mud-client / `mmc`

The automated MajorMUD client. One engine, four uses:

| Command | What it does |
|---|---|
| `mmc play --profile P` | Interactive terminal session with a status bar and bot toggles |
| `mmc run SCRIPT --profile P` | Headless Lua-scripted run (oracle captures, acceptance tests) |
| `mmc path FROM TO` | Print a route between two rooms, e.g. `mmc path 1/2146 1/2156` |
| `mmc farm --profile P` | Walk a patrol circuit, farming each stop |

`mmc play` is **never paced** — see [`pace_ms`](#pace_ms). It can also
drive the session you are already sitting in: type **`/farm`** to patrol
or **`/go <room>`** to travel, and **Ctrl-F** takes the keyboard back.
`mmc farm`
prints a live feed by default — where the character is, what
it is fighting, what it killed, and HP whenever it changes. The feed is the **full transcript** — everything the board sends;
`--brief` cuts it to notable lines only (arrivals, combat, kills, flood
control, HP changes) and `--quiet` prints nothing until the run ends. For a
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
minor_heal_at_percent = 70 # cast the minor heal below this; 0 = never
major_heal_at_percent = 40 # cast the major heal below this; 0 = never
rest_at_percent = 60       # rest below this
mana_rest_at_percent = 30  # rest, or meditate, below this mana
rest_until_percent = 95    # a rest or a meditation is over at this
flee_at_percent = 20       # run below this
meditate = false           # send meditate for mana. A quest ability, so you say.
rest_command = "rest"
minor_heal_spell = ""      # empty = the cheapest heal in the book
major_heal_spell = ""      # empty = the dearest
hp_regen_spell = ""        # a heal over time, cast between the two marks
buffs = ["bless"]          # kept up on a duration budget, not an HP mark
ignore = ["guard", "healer"]
# max_hp and max_mana omitted: the runner probes the board for them at startup

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

### The recovery marks

| mark | response | costs | works in a fight |
|---|---|---|---|
| `minor_heal_at_percent` | cast the minor heal, or the regen between the marks | mana | **yes** |
| `major_heal_at_percent` | cast the major heal | mana | **yes** |
| `rest_at_percent` | `rest` | nothing | no |
| `mana_rest_at_percent` | `rest`, or `meditate` with the switch on | nothing | no |
| `flee_at_percent` | walk out the first exit | the room | n/a |

They are two ladders and the loader refuses a profile whose marks are out
of order: `minor_heal >= major_heal >= flee`, and `rest_until >= rest_at
>= flee`, with `mana_rest_at` under `rest_until`. `0` means *off*.

As health falls with the defaults:

```
80%   nothing
65%   cast the minor heal, or the regen if one is named and not running
55%   cast a heal; rest as well, but only if the room is clear
35%   cast the major heal
15%   run, and nothing else
```

A rest or a meditation is over when the pools reach `rest_until_percent`,
HP and mana for a rest, mana for a meditation. The board has no command
to end one. The character stands on its next action, and with Stealth on
the sheet the assist's next action is `hide`. The defaults are MudPlay's.

#### Why casting works mid-fight and resting does not

Resting is suppressed whenever the room holds a fight, and that is not
caution — it is the fix for a measured death spiral (2026-08-01, HP 21/52
beside a cave bear). The board **disengages combat to rest**, so the bot
un-latched, the next room block re-engaged, the engage broke the rest, and
it alternated every round while the bear kept swinging. In an occupied room
the coherent choices are fight or flee.

Casting has none of that. It does not disengage, it does not interrupt the
swing, and mid-fight is exactly when it is worth the mana. So the spell mark
is the *only* recovery a character has while something is still hitting it —
which is the whole reason it exists.

Casts are paced to one per round, because the board refuses a second
(*"You have already cast a spell this round!"*), and each one is confirmed
from the board's own wording rather than assumed.

#### Choosing the spells

`minor_heal_spell` and `major_heal_spell` empty mean **discover**: the
client reads the character's own spellbook, takes the cheapest heal as
the minor and the dearest as the major, with the mana costs the book
reports. Name one to pin it. A name the book does not know is refused at
startup with the reason.

`hp_regen_spell` is only ever named. It is a heal over time, cast between
the minor and major marks when it is not already running, and its
duration comes from the shipped spell table in combat rounds, the same
way a buff's does. Below the major mark the instant heal is always
preferred, since a regen pays out a round later.

The old `heal_spells` list still parses: its first entry becomes the
minor and its last the major.

Mystics are handled: `powers` and `invoke` replace `spells` and `cast`, and
the Kai pool replaces mana. The client works out which from the board's own
redirect, so nothing needs configuring.

#### `buffs`

`bless` is **not** a heal — it is a 40-round buff worth +3, and it restores
no health. There is no HP mark that makes sense for it, so buffs are kept up
on a **duration budget** instead: cast in a quiet room, recast when the
rounds run out. Naming one in `heal_spells` would do nothing useful.

The duration comes from the shipped `spell` table via `[farm].content`, in
combat rounds, and it is a **floor**: the real duration scales with caster
level, so a recast on the table figure is always early and never late. That
is what lets this work off a timer at all.

There is a wear-off wording family (`The effects of %s wear off.`), and it
is used only as an *early trigger*: any wear-off expires every budget at
once. The `%s` is the spell's own free text, so it cannot be trusted to say
which buff lapsed — the same trap as the per-monster movement messages. One
redundant cast is the worst case; a buff silently down is not.

A buff the character does not know, or one with no duration, is refused at
startup with the reason printed. Nothing is skipped quietly: a buff that is
not being kept up looks exactly like one that is.

Buffs are never cast mid-fight. A buff bought during the fight it was meant
to help is mana spent too late to matter.

#### Renamed keys

`heal_at_percent` and `heal_command` named the rest mark and the rest
command back when resting was the only recovery there was. Both still parse
and still mean rest; the client says so once at load.

`spell_at_percent` is the old name of `minor_heal_at_percent`. `heal_spells`
is the old form of `minor_heal_spell` and `major_heal_spell` together, its
first entry the minor and its last the major. `depart_at_percent` under
`[farm]` is a farm-only override of `rest_until_percent`.

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
- **`depart_at_percent`** (unset) — never start a leg below this. Unset
  means the bot's `rest_until_percent`, which gates mana too and sends
  `meditate` when the switch is on. Set it to give this farm its own
  mark, or `0` to disable the gate for it.
  `interrupt_at_percent` (50) stops one that gets hurt on the way; it must
  not exceed `depart_at_percent` or the plan is rejected.
- **`idle_poke_ms`** (5000) — an idle board sends *nothing*, not even a
  prompt, for minutes at a stretch. The poke is the only thing that
  produces prompts at an empty stop, and it doubles as a respawn check.
  It is also how stale an observation may get before the runner re-asks.
- **`dwell_empty_seconds`** (0) — how long to hold a stop open once the
  room block has *proven* it empty, waiting for a respawn. 0 leaves the
  moment it is proven. This is a policy budget, not a state inference —
  see below.

### How a stop ends

A stop ends on **evidence**, not on elapsed time or a count of prompts.

The board answers "is there anything here to fight" outright, in every
room block, under `Also here:`. `StopState` keeps the last block accepted
for this stop and the runner leaves only when that block is fresh, lists
nothing the bot would swing at, no fight is outstanding, and nothing is
still owed to the board.

"Accepted" is attributed, not merely named: the session correlates every
reply to the send it answers (see `board-correlation.md`), and the only
block `StopState` believes is the one whose `Correlated::answers`
matches the `look` it is owed for. The Bot stays a pure `Event` core, so
the pump curates for it: an unsolicited render never reaches its latches
(one line in the stop pump, untested-by-design — the unit suites drive
Gate/StopState directly and no fixture scripts an unsolicited same-name
block; the live acceptance is its field test). An unsolicited block — somebody
else's render, a stale answer to a superseded ask — is never believed
and never settles the ask. Anything that changes the room (an arrival,
a blow landing on us, a kill) discards both the accepted block AND the
outstanding ask, because a block can be mid-render when the room
changes and its content then predates an event emitted before it.

While an ask is owed, the verdict is `Waiting`, never another `Ask`:
each look supersedes the last in the correlator's eyes, so stacking
would orphan the in-flight answer — and flooding looks at the board was
its own measured failure. An overdue ask (answer eaten) falls back to
`Ask` and the fresh id supersedes honestly. The owed ask also outranks
`Blind`: the light-recovery flow ends with a `look` whose echo idles
the gate one event before its answer, and blind-first walked out owing
the lit room's block every time lighting worked.

This replaced a rule that counted prompts with nothing to do. A prompt is
evidence that the board answered *something*; it says nothing about who
is standing in the room. That mismatch produced three separate failures —
walking out with three monsters still listed, abandoning a fight that was
still going, and hanging forever on an attack the board had refused — and
four successive patches tuned the threshold without addressing any of it.

Two things remain time-based, and both are honest about it:

- `dwell_empty_seconds` is a **judgement about this circuit**. No message
  can say how long a respawn is worth waiting for.
- An accepted block goes stale after `idle_poke_ms` and must be re-asked,
  because **nothing announces a respawn** — the board simply puts a
  monster in the room. Silence is not proof the room is unchanged. This
  forces a re-observation; it never decides anything by itself.

A stop that cannot be seen at all is `Blind`, not empty: a dark room
answers `look` with "you can't see anything" and never sends a block. The
runner tries the light plan a bounded number of times and then moves on,
except while defending, where the deadline governs.

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
a monster somebody else finished — `combat_idle_prompts` (12) prompts with
no blow struck either way. A fight in progress refreshes that on every
swing, hit or miss, so it only counts genuine silence.

It is now a true last resort. The room block clears the engaged latch
properly by finding the target gone, and the stop decision overrides the
counter outright: if it fires early in a genuinely slow fight, the last
accepted block still lists the monster, so the stop stays busy and the
runner does not walk off mid-fight. What is left for it to cover is the
case where no room block is coming at all.

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

## Driving from inside `play`

`/help` (or `/?`) prints the client's own commands and keybindings —
this section, condensed, without leaving the session.

`/farm` starts the patrol on the session you are already connected to,
and **Ctrl-F** stops it and hands the keyboard back. That is the reason
to want it: when a run does something you dislike you take over in one
keystroke, already connected and already where the character is, instead
of killing the process and logging in again while it stands in a lair.

### `/go <room>` — travel

`/go` walks the character from wherever it is standing to any room, and
Ctrl-F takes the keyboard back from it too. **There is one job slot**: a
`/go` and a `/farm` cannot run at once, because each owns the connection
outright (one sender, see below).

The target is a room id or a room name:

```
/go 1/2324          the id, always unambiguous
/go Grungy Shop     one match, so it walks
/go Slum Street     152 matches, so it refuses and lists the nearest five
```

Names are matched exactly first (ignoring case), then as a substring —
prefix matching would be useless here, because MajorMUD names are
area-prefixed (`Newhaven, Narrow Road`) and the fragment anyone
remembers is the tail. An ambiguous name is never a guess: `/go` prints
the nearest candidates with their ids and step counts and waits to be
asked again.

**Walk versus run is the `/bot` toggle**, not a second command:

| `/bot` | Behaviour |
|---|---|
| on | **Walk** — stops for anything the bot would attack or loot, clears the room, resumes |
| off | **Run** — keeps moving, engages only when the board refuses the move |

The assist recovers by the same marks a farm does: it rests, meditates
and casts heals, and after a rest it hides when the sheet shows Stealth.
Fleeing follows `auto_flee` as it does for a farm.

Run mode still fights, and has to: the board answers a move with *"You
may not enter that room while in combat"*, so a walk that would never
fight is a walk that stays stuck wherever something picked a fight. What
`/bot off` buys is that whiffs, passing monsters and floor loot no longer
stop the leg. After three interruptions the walk gives up and reports the
room it is standing in.

The toggle works mid-walk. Pressing `/bot` while a `/go` or a `/farm`
is running changes what the walk does from its next sighting, entry or
blow onward. It does not stop a step already sent, and a walk that has
already stopped to defend finishes that defence first. A `/farm` starts
from the profile's `fight_while_travelling` and follows the toggle from
then on.

When the walk ends with `/bot` on, the client sends one `look` before
handing the keyboard back. The walk consumed the block its last step
was answered with, and the assist is rebuilt fresh at that moment, so
without the look it would stand among whatever the destination lists
without ever having seen it.

One deliberate difference from `/farm`, because this answers a
keystroke rather than running unattended:

- **No door bashing.** `open` is still tried and still free, so ordinary
  closed doors are no obstacle; a *locked* one fails loudly instead of
  grinding sixty failed bashes. Type `bash <dir>` yourself.

The walk rests to the bot's mark before its first step, as a farm does.
Set `rest_until_percent = 0` for the old instant start.

`/go` needs the room database. The default path is **relative**
(`re/mmud_wgnt.sqlite`), so a `play` started outside the repo root will
refuse — and say which path it tried.

While the runner drives, the line editor is **locked** except for Ctrl-F
and Ctrl-Q. This is not politeness — `Gate` is built on being the only
sender, one command in flight acknowledged by the event that ANSWERS it
(its echo, or any later attributed reply — never a prompt), and a line
typed mid-leg consumes the prompt the navigator was waiting for. Two senders
desync the walk, which is the exact failure verified navigation exists to
prevent.

Errors are reported and the connection is kept: being told "no `[farm]`
table" while still logged in beats being thrown out. A run that stops
badly shows why in the bar (`stopped: ...`) rather than a bare "done".

### Roaming — fence an area instead of listing a circuit

A loop says where to go, in order. A **roam** says where *not* to go, and
the area falls out of that: the character wanders everything it can reach
from where it stands without crossing a wall you marked.

On the map (`/map`):

| key | |
|---|---|
| `x` | wall the room under the cursor — it paints red |
| `r` | leave the map and roam what the walls leave |
| `c` | clear stops **and** walls |

Walls and loop stops coexist; `enter` still marks a stop, and you can
build both in one sitting. A room marked as both paints as a wall,
because the fence is the half that has to win.

**Nothing is saved.** The walls exist for that one run and are gone when
it ends — there is no roam library, no name to type, no file to go stale.
The reasoning is that a wall which outlived its run would shape a later
one with nothing to look at and no reason to suspect it.

The area is worked out **once**, from where the character actually
stands. `r` with no walls at all is legitimate and means "this whole
plane".

**Up, down and cross-map exits are never inside a roam**, and that is not
configurable. It is the same rule the map uses to decide what belongs on
a drawn plane, so the area a roam covers is exactly the area you were
looking at when you placed the markers. It also means one marker on a
staircase is unnecessary — the stairs were never in.

The fence binds the **walk**, not just the destinations. A route that
could reach its target more cheaply by cutting through a wall takes the
long way instead, and a room the walls cut off entirely is simply
unreachable — `no route`, which is what you asked for.

**Doors are outside a roam, all of them**, whether or not you marked
them. The client has no key handling of any kind — keys in the inventory
are not parsed, exits' key ids are not read, and there is no `unlock`
verb — so its entire repertoire for a shut door is `open`, then bash
until a counter runs out. Live on cwgaming, 2026-08-03: the board
answered `open n` with *"The door is locked."* and the walk sent **88
bashes across four approaches, spending 36 HP of a 75-HP character** on a
type-7 lock that wanted Picklocks and was never going to yield to force.

A roam always has somewhere else to be, so skipping a door costs nothing
and trying one is paid for in health. A patrol with a circuit is a
different bargain — its stops were named by you and a door in the way has
to be opened — so `[farm.nav].bash_doors` still governs there and is
untouched. This is a roam rule, and a stopgap: it goes away when there is
something better than force to offer a lock.

Order is **least-recently-visited, nearest on ties**: a fresh roam sweeps
outward rather than settling, and after that each room gets the longest
recovery the area's size allows. Respawns are silent in this game, so
walking back in is the only way to find out; an empty room is left the
moment it proves empty and comes round again later.

A roam has no laps, so it ends on `[farm].max_seconds` or Ctrl-F, and
reports rooms worked rather than loops walked. Fencing yourself into a
single room is allowed — that is a vigil.

`mmc map` can mark walls but cannot roam: there is no connection to roam
with, so it says so rather than discarding the marking silently.

## The status bar

One renderer for both commands, so a session looks the same whichever
started it. `mmc farm` reserves the bottom terminal row and scrolls the
feed above it (DECSTBM, the same mechanism `mmc play` uses):

```
 attacking cave bear | HP 23 MA 8 | Small Cavern [1/2156] | mbbs
```

With no runner attached — ordinary interactive play — the activity is
absent, but the **position is still tracked**:

```
 HP 38 MA 10 | Newhaven, Narrow Road [1/2146] | mbbs
```

A room block is a room block whoever caused it, so the client localizes
every one through `localize_view` regardless of whether you typed the
move or the runner did. That handles the cold start too — the first block
after logging in has no previous room to hop from, so the global
name-and-exits search is what resolves it, and it resolves ambiguity
correctly: "Newhaven, Narrow Road" is two rooms, and the exit set picks
1/2146 over 1/2151.

While a runner is attached its own belief wins, since it knows which of
two same-named rooms it actually walked to.

While the character rests or meditates the board paints a word into its
prompt, and the bar repeats it after the vitals:

```
 HP 42 MA 12 (Resting) | Dark Cave [1/2160] | mbbs
```

The board has no wording for the end of a rest. The prompt is the only
signal, so the word is tracked on every prompt and disappears the moment
the board stops painting it.

Tracking needs the room database. The path comes from `[farm].content`
when the profile has one, else the same default `mmc path` uses; without
it the bar shows the name alone, as it always did.

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

### The tick clock

The board never says when it ticks. The client infers three clocks and
shows their countdowns in the bar:

```
 HP 42 MA 12 (Resting) | Tick 3.2 | HP 12.3/4.5 | MA 21.0 | Dark Cave [1/2160] | mbbs
```

`Tick` is the combat round, locked by any hit or miss line and 5.13
seconds long. `HP` and `MA` are the regen cycles. Passive HP and mana
share one 30 second pulse, which the client anchors on a pool rising
between two prompts. While the prompt says resting a second HP number
appears, the 20 second rest tick. While it says meditating a second MA
number appears, the 15 second meditate tick. A dash means nothing has
locked that clock yet. A pool rising within three seconds of one of your
own casts is the spell landing and moves no clock.

The cadence is MudPlay's measurement of the stock board. In `play` the
bar is repainted four times a second so the numbers move between
events. The headless `mmc farm` bar repaints on events only.

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

## Fighting on the way

`[farm].fight_while_travelling` (default **on**) stops a leg and clears
the room when something swings at the character, then resumes the walk.

This is not a nicety. The board **refuses movement outright while you are
in combat** — *"You may not enter that room while in combat."* (DLL
0xbc6a8) — so a runner that walked on regardless simply sent directions
that were rejected, then waited out its 15-second step deadline for a room
block that was never coming. A patrol crossing Newhaven Arena died exactly
that way.

The HP gate is not a substitute: a healthy character can be swarmed for a
long time without falling past `interrupt_at_percent`, and every one of
those rounds is a step that does not land.

Turn it off for legs whose point is to get somewhere. The character then
still stops for the HP gate and for dying — but note it can still be
pinned, because the refusal is the board's, not the client's.

The walk home fights too, for the same reason.

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
- `tests/dialect.rs`, `tests/nav_doors.rs`, `tests/farm_scripted.rs` —
  against scripted boards, for behaviour `mud-server` does not model (the
  MBBSEmu login, the open/bash command family, spell healing).
- `tests/sheet.rs` — the spellbook, and the three spell machines
  (lighting, healing, buff upkeep) as pure state.
- `tests/bot_corpus.rs`, `tests/farm_corpus.rs`, `tests/parse.rs` — replay
  the real transcripts in `re/oracle/`.

The corpus goldens count files in `re/oracle/`, which the **server** track
also writes into during oracle runs. They will fail spuriously while
another session is mid-capture.
