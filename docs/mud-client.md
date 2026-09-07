# mud-client / `mmc`

The automated MajorMUD client. One engine, four uses:

| Command | What it does |
|---|---|
| `mmc play [--profile P]` | Interactive terminal session. Opens the lobby, and with a profile a second window connected to it. |
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
take_keys = true           # keys the ring lacks; independent of auto_get
ignore_coins = ["copper"]  # denominations the sweep leaves on the floor
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

[bank]
auto_deposit = true              # detour to a bank during a farm when a gate trips
deposit_at_coins = 1000          # raw coin count, every denomination counts one; 0 = off
deposit_on_weight_class = true   # a pickup lifted None->Light, Light->Medium, Medium->Heavy
keep_gold = 0                    # left in the purse, in gold crowns
# at = "1/297"                   # a fixed bank; unset = the nearest
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
35%   cast the major heal, and rest if the room is clear
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

A book whose only heal is the named regen spell casts nothing below the
major mark, where the slow regen is never wanted.

The old `heal_spells` list still parses: its first entry becomes the
minor and its last the major.

Mystics are handled: `powers` and `invoke` replace `spells` and `cast`, and
the Kai pool replaces mana. The client works out which from the board's own
redirect, so nothing needs configuring.

#### `buffs`

`bless` is **not** a heal — it is a 40-round buff worth +3, and it restores
no health. There is no HP mark that makes sense for it, so buffs are kept up
on a **duration budget** instead: cast in a quiet room, recast when the
rounds run out. Naming one in `minor_heal_spell` would do nothing useful.

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
first entry the minor and its last the major.

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
- **`depart_at_percent`** is unset by default and means never start a leg
  below this mark. Unset, the mark is the bot's `rest_until_percent`,
  which gates mana too and sends `meditate` when the switch is on. Set it
  to give this farm its own mark. Set it to `0` to disable the gate for
  this farm alone.
  **`interrupt_at_percent`**, 50 by default, stops a leg that gets hurt on
  the way. It must not exceed the departure mark. A farm that sets its own
  `depart_at_percent` has the pair rejected when the plan is built. A farm
  that does not is refused at run start instead, where the bot's mark is
  finally in hand.
  **`max_rest_seconds`**, 120 by default, caps how long the departure gate
  may spend recovering. A caster resting both pools to 95 usually reaches
  this cap first, since mana climbs by one tick every 15 or 30 seconds.
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
  not shift and the character cannot pick. Bashing costs HP (*"You take
  %d damage for bashing the door!"*) and needs a weapon, so it is a
  switch. Turning it off makes a locked door the character cannot pick a
  wall: routing goes around it, and a route that has no way around says
  `no route`.

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
  closed doors are no obstacle. A lock the character can pick is picked.
  A lock the character cannot pick is a wall for routing when bashing is
  off, so `/go` goes around it and says `no route` when there is no way
  around. The exception is a lock with a positive pick modifier, which
  never re-locks once anyone has picked it, so routing treats it as an
  ordinary door. If it turns out to be shut after all and the character
  cannot pick it, the walk stops there and names the door and the
  direction. Either way the walk never grinds bashes. Bash it by hand
  and `/go` again.

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

### `/bank`, deposit now

`/bank` walks the character to the nearest bank, or the one `[bank].at`
names, deposits everything above `keep_gold`, and stops there. It takes
the same job slot as `/go`, and Ctrl-F takes the keyboard back. It does
not consult the deposit gates: you asked.

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
them. Live on cwgaming, 2026-08-03: the board answered `open n` with
*"The door is locked."* and the walk sent **88 bashes across four
approaches, spending 36 HP of a 75-HP character** on a type-7 lock that
wanted Picklocks and was never going to yield to force. A roam always
has somewhere else to be, so skipping a door costs nothing and trying
one is paid for in health. A patrol with a circuit is a different
bargain, its stops were named by you and a door in the way has to be
opened, so `[farm.nav].bash_doors` still governs there.

**Item gates are inside a roam** for the character carrying the item
and a wall for one who is not. The region is flooded with the
character's own pack, the same way it is with the character's own
levers.

**Buttons and levers are inside a roam.** The area is flooded with the
character's own capabilities, so a passage this character can open is
part of the area and one whose button wants an item the pack lacks is
not. A lever room outside your walls is the one thing the flood cannot
see: the walk to it fails as `no route`, the leg ends with a puzzle
error, the room leaves the roam for the rest of the run, and the
rotation moves on.

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

## Live settings

Every profile key can be read and changed from inside the client, the way
irssi does it. The profile file is the source of truth: `/set` edits the
document, and `/save` writes it back with your comments and key order
intact.

| Command | Effect |
| --- | --- |
| `/set` | List every key with its value. The password shows as stars. |
| `/set <pattern>` | List the keys a glob matches. `*` matches anything and a trailing `*` is implied: `/set bot`, `/set bot.rest*`, `/set *heal*`. |
| `/set <key> <value>` | Change a key. The value is TOML: `60`, `true`, `["copper", "silver"]`. A bare word is a string, so `/set bot.rest_command rest` works. |
| `/unset <key>` | Remove a key so its default applies. The way back to none for `bank.at`, `farm.finish_at`, `farm.depart_at_percent` and `pace_ms`. |
| `/save [file]` | Write the settings. Started with `--profile`, no path is needed. Started bare, the first `/save` names the file and later ones remember it. |
| `/load <file>` | Replace the settings from a file. The connection stays open. |

Tab completes a slash verb or, after `/set` and `/unset`, a key:
`/set bot.ignore_c<Tab>` gives `/set bot.ignore_coins`. When several keys
match, Tab grows the word to what they share, and a second Tab lists
them: `/set bot.ign<Tab>` becomes `/set bot.ignore` and the next Tab
prints `bot.ignore_coins` and `bot.ignore`.

When a change takes effect:

- A `bot.*` key rebuilds the assist on the spot when it is on.
- Everything a job reads, `[farm]`, `[bank]`, `pace_ms`, applies at the
  next `/farm`, `/go` or `/bank`. A running job keeps what it started
  with.
- Connection keys apply at the next `/connect`.

A refused value, a wrong type or a value the validator rejects, changes
nothing and says why. `/quit` with unsaved settings refuses once and
exits on the second `/quit`. Ctrl-Q exits without asking.

A profile with no `[bot]` table gives the assist attack and loot on. The
first `/set bot.anything` creates the table, and every other bot key then
takes its struct default, which is off for both. `/set bot` shows the
effective values either way.

## The lobby

`mmc play` with no `--profile` starts in the lobby: the input line with no
connection. It takes the settings commands, `/connect`, `/help` and
`/quit`. `/connect host[:port]` opens a window and connects it, port 23
by default. The host and port become that window's settings, so a
`/save <file>` there keeps them. Started with a profile whose host is
set, the client connects at once.

Whenever the line closes, by the board or by `/disconnect`, the client
returns to the lobby with the settings intact. `/connect` with no
argument dials the current host and port.

Interactive play never logs in for you: you type the username and
password at the board's prompt. The automation needs the character's
name, because it matches your own death line against it. It reads that
name off the stat sheet, so once you are in the realm it has one whatever
the profile says. The `username` key only matters before that, and for
the headless `mmc farm` and `mmc run`, which log in for you. With no name
from either place, `/farm`, `/go`, `/bank`, `/where`, `/bot` and the map
view's own roam and go refuse to start and say so. A profile whose
`assist_play` is on gets the same refusal at connect, and the assist
stays off.

## Windows

Several characters run in one process, each in a numbered window with its
own settings, session and screen. Window 1 is the lobby: it never
connects, its `/set` edits the template every new window copies, and its
screen is a log of every window's major events. The room database is
loaded once per content path and shared by every window on it.

| Command | Effect |
| --- | --- |
| `/new [file]` | Open a window on a copy of the lobby's settings, or on that profile file, and switch to it. A copy whose settings name a host connects at once. The copy carries no file, so a `/save` there needs a name. |
| `/1` to `/9` | Switch to that window. |
| `/windows` | List each window: number, character and host, connected or not, unsaved or not. |
| `/close` | Close the current window. Refused while it is connected and refused for the lobby. |
| `/connect [host[:port]]` | In the lobby, open a window and connect it. The host and port go into that window's settings, not the lobby's. In any other window, connect that window. |
| `/quit` | Refuse once if any window is connected or has unsaved settings, naming them. The second `/quit` exits everything. Ctrl-Q exits at once. |

`/set`, `/save`, `/load`, `/unset`, `/help`, Tab, Ctrl-F and Ctrl-P act on
the current window. Passthrough is per window. PageUp and PageDown scroll
the current window through its scrollback, and any other key returns it
to the bottom. The scrollback keeps `scrollback_lines` rows, 2000 by
default, read when the window opens.

A hidden window keeps running: its farm, its assist and its output all
carry on. Its screen is kept in memory, so switching to it shows exactly
what it would have shown, colours included. A window that has
disconnected shows `not connected` and its host in the bar, and takes
`/connect` to go back.

### The activity list

The bar shows the active window's number, then what it showed before,
then `Act: 2,3` when other windows have an event you have not looked at.
Only major events count: a connect, a disconnect, a death, a job ending,
the assist refusing to start. Ordinary output never lights it. Switching
to the window clears it. The same events are written to the lobby's log
with a UTC time, the window number and the character.

The bar is painted only when its text changes, and never by clearing the
row first, so it no longer flashes over a slow link.

Started as `mmc play` with no profile, the client opens the lobby alone.
Started with `--profile`, it opens the lobby and window 2 connected to
that profile, and shows window 2. A capture, when given, records window
2's first connection.

## The status bar

In play the bar is prefixed with the window's number, and carries the
activity list described under Windows.

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

With the rate and the level known the bar also shows experience per
hour and the time to the next level at that rate.

```
 HP 38 MA 10 | Tick 3.2 | HP 12.3 | MA 21.0 | Newhaven, Narrow Road [1/2146] | 4200 xp/hr | L3->4 1h12m | mbbs
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

### Locks

Every door and gate carries a lock state and a pick modifier in the room
record, and the graph reads both. Most shipped doors are locked in the
data. The pick roll is `theft.md` §8.2: the skill
must be at least 1 and `genrdn(0,100) < modifier + Picklocks`. So a lock
is pickable at all only when `Picklocks >= 1` and `modifier + Picklocks
> 0`. Below that line the pick fails every time, and the walk never
spends a roll there.

Routing prices a lock by that formula. A door the character can pick
costs the door plus the expected rolls, capped at the price of a
searchable hidden exit. A lock it cannot pick costs the door when
`bash_doors` is on, since force is a separate roll, and is a wall when
it is off. The lock state is the boot state: a lock with a positive
modifier never re-locks once picked, so a door the data calls locked can
stand open all day. A lock with a positive modifier is priced as an
ordinary door for that reason. The walk tries `open` first regardless.

## Keys and item gates

A type 2 door wants a key, one of 88 key items, and the key ring is read
off the `i` reply along with the pack. With the key on the ring the door
is priced like any other and the walk opens it: `open e` says *"The door
is locked."*, then `use black star key east` with the key's full name
and the full direction word answers *"You successfully unlocked the
door."*, the same line a pick gives, and `open e` then opens it. Captured
at the Black House on Slum Street, 2026-09-05. `unlock` is not a
command. Most keys are spent by use, 37 of them on the first, so the
pack is re-read from the board after every key use rather than updated
by inference.

Without the key the door is a lock like any other, priced and picked by
the formula above, except that a key door nobody can pick is a wall
whether or not bashing is on. A key the board does not accept at a door
is answered with some other line, and the walk then treats the lock as
the story again.

A type 3 exit is passable only while carrying an item. One of them
wants nothing. Routing checks the pack and the walk takes it as a
plain step. Without the item it is a wall.

The bot picks up any key it sees on the floor that the ring lacks,
`[bot].take_keys`, on by default.

## Banking

A farm picks up every pile it kills over. Coins weigh a third of a unit
each, so a long run drifts the character up a weight class, and a death
loses the lot. So the run judges a deposit gate after any stop where the
board confirmed a coin pickup, on one fresh `i`, and detours when it
trips. The run reads the purse once at its start as well, so the first
weight-class crossing is measured from what the character carries now
rather than from a reading of unknown age.

Two gates, either one enough:

- **`deposit_at_coins`**, 1000 by default: the raw coin count is over
  the mark and the purse is above the keep floor, so a deposit can
  lower it.
- **`deposit_on_weight_class`**, on by default: the coins picked up
  since the last reading lifted the class a step, None to Light, Light
  to Medium, Medium to Heavy. The boundaries are the board's own, 33,
  66 and 100 percent. A class already raised by earlier coins does not
  fire again.

The errand walks to the bank with the same leg a circuit uses, so
fights on the way, interrupts and desync recovery are the leg's. At the
bank it reads the purse again, deposits everything above
**`keep_gold`**, which is 0 by default, and reads once more so routing
sees the money that is left. A circuit's next leg starts from the bank,
so there is no walk back. A roam walks back to the room it left,
because a roam picks its next room from inside its fence and the bank
is almost never in there. The walk to the bank ignores the fence for
the same reason: a run that cannot leave its region cannot put its coin
down.

An errand that deposits nothing switches deposits off for the rest of
the run and prints one line saying so. That covers a bank no route
reaches, a walk that fails, a deposit the board refuses, and a purse
with nothing above the keep floor.

**The nearest bank** is the one the fewest hops away along a route the
character can take. The world has five bank rooms: 1/297 Bank of
Godfrey, 6/1334 Bank of Khazarad, 2/2568 Bank of Rhudaur, 16/320 Bank
in the Lost City, 17/2435 Bank of Arlysia. Godfrey and Khazarad share
one account. The other three are separate accounts, so a run that uses
the nearest bank can spread its money over four. `[bank].at` names one
bank for every deposit instead. The Bank of Rhudaur sits behind a 5
gold toll on the way in and its exit is free. A toll the purse cannot
pay is a wall to routing, so a bank behind one is simply not a
candidate.

**Tolls after a deposit.** Routing prices a toll against the purse and
treats one the character cannot pay as a wall. A circuit that crosses a
toll needs `keep_gold` set to it, or the leg home goes round.

**`[bot].ignore_coins`** lists denominations the sweep leaves on the
floor, as `get` names them: `copper`, `silver`, `gold`, `platinum`,
`runic`. Every sweep site honours it, and an ignored pile never holds a
stop open.

Three wordings are from the stock oracle and unverified on the live
board: the deposit reply *"You deposit 10 silver nobles."*, the
inventory reply's *"You are carrying"* line, and the pickup line *"You
picked up 11 silver nobles"*. The first live `/bank` settles them.

## Buttons and levers

Some passages open on a phrase. A room's type 12 exit slot is not an
exit but a remote action: `push button` in 1/506 clears the concealment
bit on that room's own south wall, and `pull lever` in 1/1044 and 1/1038
together open 1/1056 north. 287 slots ship. 200 act on their own room,
the rest on another, and 86 want an item carried, 79 of them a titanium
fork. No search ever reveals one of these exits, which is how a roam
came to send `search s` two hundred times at 1/506 on 2026-09-04.

The graph decodes every slot at load and attaches it to the exit it
opens. The slot itself is never walked. Routing prices the exit by its
plan: the exit, then a phrase and a round trip per lever. An exit whose
plan needs an item the pack lacks, or a lever room no walk reaches, is
not priced at all, it is refused, and the router finds another way or
says `no route`.

The walk sends the direction first. If the passage is already open it
simply walks. On *"There is no exit in that direction!"* it performs the
plan: walk to each lever room in turn, speak the phrase, wait for the
reply or the next prompt, and walk back. Levers pull in descending
number order, which is the order an ordered puzzle needs and any order
is fine for the rest. Then the direction goes out again. A wall that
stays shut after a whole plan is tried once more, because the board
re-hides a passage 300 seconds after the last lever and a long detour
can lose that race, and then reported as a puzzle failure. The board
says an unknown phrase out loud, *"You say "push button""*, and the walk
reads that as the phrase doing nothing here and stops at once.

A lever on a gate toggles its lock instead. The walk finds that out the
ordinary way: `open n` answers *"The gate is locked."*, the lever is
pulled, and `open n` is sent again before any pick or bash.

An interrupt while the character is off pulling a lever reports the
lever room, not the room the step set out from. The farm resumes from
where the character actually is.

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
- `tests/dialect.rs`, `tests/nav_doors.rs`, `tests/nav_keys.rs`,
  `tests/farm_scripted.rs` — against scripted boards, for behaviour
  `mud-server` does not model (the MBBSEmu login, the open/bash command
  family, spell healing).
- `tests/sheet.rs` — the spellbook, and the three spell machines
  (lighting, healing, buff upkeep) as pure state.
- `tests/bot_corpus.rs`, `tests/farm_corpus.rs`, `tests/parse.rs` — replay
  the real transcripts in `re/oracle/`.

The corpus goldens count files in `re/oracle/`, which the **server** track
also writes into during oracle runs. They will fail spuriously while
another session is mid-capture.

