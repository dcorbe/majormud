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
permanent record use `--capture BASE`. `mmc run` and `mmc farm` write
`BASE.raw` (the raw socket bytes) and `BASE_timing.log`. `mmc play` writes
one pair per window, `BASE-NAME.raw` and `BASE-NAME_timing.log`, NAME being
the window's profile name, or its window number when it was opened without
one. Keep captures OUT of `re/oracle/` — two corpus tests count the files in
there.

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
auto_rest = true           # rest or meditate at the marks below; false never rests
auto_sneak = true          # arm a sneak before walking, and when idle; false never sneaks
auto_hide = false          # hide when idle instead of sneaking, and never backstab
ignore_coins = ["copper"]  # denominations the sweep leaves on the floor
minor_heal_at_percent = 70 # cast the minor heal below this; 0 = never
major_heal_at_percent = 40 # cast the major heal below this; 0 = never
rest_at_percent = 60       # rest below this
mana_rest_at_percent = 30  # rest, or meditate, below this mana
rest_until_percent = 95    # a rest or a meditation is over at this
flee_at_percent = 20       # run below this
meditate = false           # send meditate for mana. A quest ability, so you say.
rest_command = "rest"
attack_command = "a"       # the fight's verb: a, pu or ju for a mystic, "cast lbol" for a caster
minor_heal_spell = ""      # empty = the cheapest heal in the book; short names work
major_heal_spell = ""      # empty = the dearest
hp_regen_spell = ""        # a heal over time, cast between the two marks
buffs = ["bless"]          # kept up on a duration budget, not an HP mark; short names work
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
route = "short"            # or "safe": detour round rooms above the character

[bank]
auto_deposit = true              # detour to a bank during a farm when a gate trips
deposit_at_coins = 1000          # raw coin count, every denomination counts one; 0 = off
deposit_on_weight_class = true   # a pickup lifted None->Light, Light->Medium, Medium->Heavy
keep_gold = 0                    # left in the purse, in gold crowns
# at = "1/297"                   # a fixed bank; unset = the nearest

[party]
wait_secs = 90        # a follower's @wait holds the leader this long at most
bank_wait_secs = 15   # the leader waits at the bank this long for @ok replies
follow_normal = true  # send "set follow normal" when following begins
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

### `attack_command`

The verb a fight opens and continues with, sent as it is with the
target's noun after it. `a` swings. A mystic sets `pu` or `ju`. A caster
who fights with magic sets the whole cast, `"cast lbol"`, and the bot
sends `cast lbol rat`. The backstab opener is `bs` whatever this says,
since it is the sneak that asked for it. The board drops a backstab to
a plain attack once its first blow lands, so the verb is sent again on
that blow and the second round is fought with it. The board answers a
verb sent mid-fight with `*Combat Off*` and `*Combat Engaged*` back to
back; that pair is a mode switch, not the fight ending. The verb also reads the echo
that tells the bot its target was gone before the swing: the board
speaks a command it cannot resolve, and `You say "pu rat"` ends that
fight the same way `You say "a rat"` did.

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
the sheet the assist's next action is `hide` or `sneak`, whichever idle
time is spent under. The defaults are MudPlay's.

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
reports. A mystic's way of the swan is a heal, so a mystic discovers it
too and invokes it. Name one to pin it, by its full name or by the short
name the listing shows. A name the book does not know is refused at
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
not being kept up looks exactly like one that is. A buff may be named by
its full name or by the short name `cast` takes: `["shld", "blur"]` and
`["shockshield", "blur"]` are the same list.

The assist keeps the buffs up the same way a job does, standing in a
quiet room: no fight in progress, nothing in the room it would swing at,
and no rest or meditation under way, since a cast ends one. Until
2026-09-08 only a job built a buff state, and a character played by hand
with `/bot` on cast nothing.

Buffs are never cast mid-fight. A buff bought during the fight it was meant
to help is mana spent too late to matter.

A stealth spell the character knows is cast before every sneak a walk
arms, on the same budget a buff runs on. It is discovered from the spell
table by its stealth ability, so camouflage, way of the cat and shadowform
are found without being named. `bot.auto_sneak = false` turns the sneak and
the cast off together. A farm start prints what it found as
`stealth: camouflage (10 mana, 30 rounds)`, or `stealth: none known`.

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
- **`depart_at_percent`** is unset by default and is the mark a recovery
  before a leg runs to, HP and mana alike. Unset, the mark is the bot's
  `rest_until_percent`. The gate starts a recovery on the bot's own
  floors, HP under `rest_at_percent` or mana under `mana_rest_at_percent`,
  and sends `meditate` when only mana is short and the switch is on.
  Over both floors nothing was resting and the leg sets off as it
  stands. A recovery already showing on the prompt when the gate is
  reached is finished to the mark the same way. Set it to give this farm
  its own mark. Set it to `0` to disable the gate for this farm alone.
  **`interrupt_at_percent`**, 50 by default, stops a leg that gets hurt on
  the way. It must not exceed the bot's `rest_at_percent`, since that is
  the HP a leg may set off at. The plan cannot see the bot's floor, so
  the pair is refused at run start.
  **`max_rest_seconds`**, 180 by default, caps how long the departure gate
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
- **`route`** (`short`) — `safe` makes every walk price a room that can
  spawn something both hostile and above the character's level as 60
  extra steps, so a detour of up to sixty steps wins. It is a price, not
  a wall: the Slum Gates spawn level 17 townsfolk and are the only way
  out of the slums, and a wall there would say `no route` for every
  character under 18. Townsfolk do not count anyway, because only
  monsters that start fights are counted. The level is read off the
  `stat` sheet, and until one has been read `safe` routes like `short`.

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

**`/bot` is the master switch** for everything automatic, in every mode:
a `/go`, a `/farm`, a `/roam`, a `/bank`, a `/recover`, and the assist
beside a character walked by hand. Off, nothing automatic runs: no
fighting on the way, no looting, no resting, no heals, no buffs, no flee,
and no health gate before a leg departs. On, what the profile's `[bot]`
keys select runs, and only that. `auto_rest = false` never rests and
never gates a departure on health, `auto_heal = false` never casts a
heal. A job never turns a key on for itself. The switch does not touch
the profile: what `/set` chose is still there when the bot comes back on.

Whether a `/go` walks or runs comes from the same tables:

| `farm.fight_while_travelling` and `bot.auto_combat` | Behaviour |
|---|---|
| both on, with `/bot` on | **Walk** — stops for anything the bot would attack or loot, clears the room, resumes |
| either off, or `/bot` off | **Run** — keeps moving, engages only when the board refuses the move |

The assist recovers by the same marks a farm does: it rests and
meditates when `auto_rest` says so, casts heals when `auto_heal` does,
and it keeps stealth up while idle when the sheet shows Stealth.
Fleeing follows `auto_flee` as it does for a farm.

Every one of those marks is a percentage of the pools, and a profile
that leaves `bot.max_hp` out means "ask the board". The assist asks
with `health` once the character is in the realm, the same probe every
job runs, and the lobby notes the answer as
`-- assist: the board says 102 hits, 20 mana --`. Until the answer
lands the marks are off, and a board that never answers is said so and
asked again at the next realm entry. A profile that names `max_hp` is
believed over the board, as it is for a job. Until 2026-09-08 the
assist never asked, so with the recommended profile it never rested,
healed, fled or kept stealth up at all.

Idle means standing, not resting, with nothing in the room to fight.
Under `auto_hide` the assist sends `hide` whenever it finds itself idle
and believes it is not hidden. Otherwise, with `auto_sneak` on, it sends
`sneak`, so the next move goes unseen whether the character makes it or
a party leader does. Both are beliefs: the board's own echo confirms an
attempt, a named failure or a hard block clears it, any other send
forgets it, and a room block forgets a hide and keeps a sneak only when
the move printed "Sneaking...". A stealth command the board swallows
without a word, or answers with a failed roll, is tried again after
three quiet prompts, and after three sends the assist stops until the
situation changes: a new room, a recovery, or a command of its own. The
same room printed again is not a new situation. "You may not sneak
right now!" is not a roll and is not tried again until then. The idle
sneak is bare. The stealth spell a walk casts before its sneak is not
cast here.

A room the character sneaked into is opened with `bs`, off the same
"Sneaking..." line a walk's arrival reads. The board prints it for a
party follower on the leader's move, so a dragged character backstabs
what it is dragged into. Under `auto_hide` every fight opens with a
plain attack. The wielded weapon is used as it is. The swap to a
backstab-only weapon a farm's walk makes is not made here.

The assist acts on every room block the character is standing in,
whether or not the client asked for it. A party leader's move drags the
character into the next room and prints the block with nothing sent from
this side, and the assist engages what it finds there the same as after
a step of its own. A `look <direction>` block is the one exception: it
describes the neighbour, and the assist never fights the neighbour.

A run still fights, and has to: the board answers a move with *"You may
not enter that room while in combat"*, so a walk that would never fight
is a walk that stays stuck wherever something picked a fight. What a run
buys is that whiffs, passing monsters and floor loot no longer stop the
leg. After three interruptions the walk gives up and reports the room it
is standing in.

The switch works mid-job. Pressing `/bot` while a job is running reaches
it at its next hop, sighting, entry or wait, the same way a `/set` does,
and the job prints a notice saying which way it went. It does not stop a
step already sent, and a walk that has already stopped to defend finishes
that defence first.

When the walk ends with `/bot` on, the client sends one `look` before
handing the keyboard back. The walk consumed the block its last step
was answered with, and the assist is rebuilt fresh at that moment, so
without the look it would stand among whatever the destination lists
without ever having seen it.

One deliberate difference from `/farm`, because this answers a
keystroke rather than running unattended:

- **Doors as the profile says.** `open` is tried first and is free, so
  ordinary closed doors are no obstacle. A lock the character can pick
  is picked. A lock it cannot pick is bashed when `[farm.nav].bash_doors`
  is on, which it is by default, and is a wall for routing when it is
  off: `/go` then goes around it and says `no route` when there is no
  way around. The exception is a lock with a positive pick modifier,
  which never re-locks once anyone has picked it, so routing treats it
  as an ordinary door. A bash is minutes of rolls and costs HP per
  swing, so turn the switch off for a character with no weapon.

The walk rests before its first step as a farm does: a pool under its
floor starts a recovery, which runs to the bot's mark. Set
`rest_until_percent = 0` for the old instant start.

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
| `S` | mark the room under the cursor as the recovery's safe room; again clears it |
| `D` | mark it as the death room; again clears it |

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

## Recovering gear

| Command | What it does |
|---|---|
| `/recover [room]` | Sneak to a room, search it once, take everything listed, and run back to the safe room. Without a room, the death room marked on the map, else the room of the last logged death. |

A full death drops everything the character carries into the room it died
in. `/recover` goes and gets it back. The job runs to the safe room if the
character is not already standing in it, sneaks from there to the death
room one hop at a time, searches once, picks up everything the search
lists, and runs back to the safe room. It never attacks. On the map, `R`
with the cursor on a room does the same with that room as the death room.
On an empty cell it says there is no room under the cursor, and the
offline `mmc map` says recover needs a connection.

The two rooms are marked on the map: `S` on the safe room, `D` on the
death room, each a second time to clear. The map draws them as `S` and
`D` in their own colours and names them in the panel. The marks live as
long as the window does and are not saved. Without a safe mark the safe
room is wherever the character stands. Without a death mark, and with no
room typed, the target is the newest logged death. A typed room always
wins. The note that starts the job names both rooms:
`-- recovering from Keep [1/3] via the safe room Inner Ward [1/2] --`.

The run to the safe room is the run home in reverse: unsneaked, through
blows, and a move refused for combat is sent again once the round has
passed. A run that stops short ends the job as `stopped at` that room
with nothing taken.

The job refuses to start, and says why, when another job is running, when
the character's position is not confirmed, when there is no route to the
safe room or from it to the death room, or when the stat sheet says
Stealth is 0. `/recover` with no room refuses as well
when the log holds no death with a room. `bot.auto_sneak` and `/bot` do not
reach it: they say whether a walk sneaks on its own, and this sneak was
asked for. With `/bot` off the job still sneaks in and runs home, and only
the buffs from `bot.buffs` are skipped.

Before the sneak it lights up if the route or the death room is dark, and
casts the buffs in `bot.buffs`. Both break a sneak, so both come first. The
stealth spell the sheet found is cast by the navigator as part of arming.

On the way in, a hop that arrives without the board's `Sneaking...` line is
a break, and the job turns for home from that room without searching. A
move the board refuses for combat is treated the same way.

In the death room the job sends one bare `search`. A search the board
never answers ends as `nothing there`, the same as an empty search. The
floor is listed as `You notice ... here.`, and every entry is taken with
one `get` each, gear first and coins last, including denominations in
`bot.ignore_coins`. An entry the board says it does not see is dropped. An
entry the board says nothing about is tried three times and dropped, since
there is no wording for an item too heavy to lift. Every wait in the
sweep, the search and each of an entry's three attempts, is bounded by
`farm.nav.step_timeout_ms`, the same key that bounds a walk's steps. After
every reply the job reads the prompt, and hitpoints under
`bot.minor_heal_at_percent` end the sweep at once.

The run home is unsneaked and does not stop for blows. Aggressive monsters
acquire a target inside the combat round and skip a player who moved this
round, so a character that keeps moving is neither acquired nor followed.

The job ends the way a go does. The bar shows the reason, the window adopts
the room, and the items taken are printed one per line:

| ending | phase text |
| --- | --- |
| swept and home | `recovered 5 of 7 items and 2 coin piles` |
| break on the way in | `sneak broke at 1/2810 Darkwood Forest, nothing taken` |
| refused a move for combat | `attacked at 1/2810 Darkwood Forest, nothing taken` |
| hurt mid sweep | `hurt under 70%, back with 3 of 7 items` |
| empty search | `nothing there` |
| walk home did not complete | `stopped at 1/2812 Darkwood Forest with 3 of 7 items` |
| died | `died` |

The bare search's item line is unverified on the live board. The first live
run settles it.

### The death log

Every death writes one line to `~/.config/mmc/deaths.log`, beside the
profiles, from hand play, under a job, and under the map alike. `mmc farm`
writes it too. Append only. A line reads:

    2026-09-07T14:42:07Z beef 1/2810 Darkwood Forest confirmed

The character field is the name the stat sheet gave, or the profile's
username before a sheet exists. The last word is `confirmed`, `stale` or
`unknown`, and says how sure the client was of the room. A death with no
known room writes `- - unknown` for the id, the name and the fix, so the
death itself is not lost. `/recover` with no argument takes the newest
line for the character that has a room.

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
| `/save [name]` | Write the settings. Started with `--profile`, no name is needed. A bare name is a profile under `~/.config/mmc`, so `/save beef` writes `~/.config/mmc/beef.toml`. Anything with a slash or a `.toml` suffix is a path. A file these settings were not loaded from is refused: `/load` it first, or pick another name. |
| `/load <name>` | Replace the settings from a profile, named the same way. The connection stays open. |

`--profile` on `mmc play`, `mmc run` and `mmc farm` takes the same names.

Tab completes a slash verb or, after `/set` and `/unset`, a key:
`/set bot.ignore_c<Tab>` gives `/set bot.ignore_coins`. When several keys
match, Tab grows the word to what they share, and a second Tab lists
them: `/set bot.ign<Tab>` becomes `/set bot.ignore` and the next Tab
prints `bot.ignore_coins` and `bot.ignore`.

When a change takes effect:

- A `bot.*` key rebuilds the assist on the spot when it is on.
- A running job picks up a change at its next decision that reads the
  key: the next step, the next pass of a stop, the next pickup. It says
  so on the screen: `-- farm: settings reloaded --`. Live keys are
  `bot.*`, `farm.rest_at_percent`, `farm.rest_until_percent`,
  `farm.mana_rest_at_percent`, `farm.fight_while_travelling`,
  `farm.travel_interrupts`, `farm.interrupt_at_percent`,
  `farm.defend_seconds`, `farm.nav.*`, `bank.*`, `party.*`, and
  `pace_ms`.
  `farm.nav.*` and `bot.auto_sneak` reach a farm at its next leg, and
  reach a `/go` or a `/bank` only at the next command, because those two
  jobs build their navigator once.
- Fixed at a job's start, so a change waits for the next `/farm`, `/go`,
  `/bank` or `/recover`: the loop and its stops, `farm.start`,
  `farm.finish_at`, `farm.circuit`, `farm.content`, the go target, and the
  recover target and start room.
- Connection keys apply at the next `/connect`.

A refused value, a wrong type or a value the validator rejects, changes
nothing and says why. `/quit` with unsaved settings refuses once and
exits on the second `/quit`. Ctrl-Q exits without asking.

A profile with no `[bot]` table gives the assist every switch on, since
that is the struct default now. `take_keys` was already on and stays so.
The first `/set bot.anything` creates the table, and every other bot key
then takes its struct default. `/set bot` shows the effective values
either way.

`auto_combat`, `auto_heal`, `auto_get` and `auto_flee` default on now.
A `[bot]` table that left one of them out because off used to be the
default has it on from here. `auto_heal = false` no longer stops the
character resting, because `auto_rest` gates that on its own.
`auto_rest` and `auto_sneak` are the two new switches, and `auto_hide`
is a third, off by default. `/bot` turns every one of them off at once
and back on, without touching the profile.

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

A profile with `reconnect = true` dials again by itself when the line
closes, waits `reconnect_delay_seconds` first, ten by default, and logs
in with the profile's `username` and `password`. Both have to be set.
With either one empty the window says so and stays put, because a redial
that stopped at the username prompt would look connected and be useless.
A failed dial or a failed login is written on the window's screen and the
next try follows after the same wait. `/disconnect` while the wait runs
cancels it and leaves the window where it is, so `/connect` is then the
only thing that dials. A `/disconnect` typed while connected is the same
answer, and the window stays down rather than dialling back into a board
you just left. Every attempt goes into the lobby's log as `reconnecting,
attempt N`, and the count starts over once a session is playing or the
operator dials. `/set reconnect false` is read when the next line
closes, so it does not cancel a wait that is already running.

When a line closes on its own the window says why, from what the socket
read: `-- disconnected: end of stream --` is the board hanging up, and
anything else is the socket error, such as `Connection reset by peer`,
which is the path forgetting the line rather than the board ending it.
The same reason goes into the timing log as `!! connection closed:`.

Every connection keeps itself alive at the socket: after fifteen quiet
seconds the kernel sends empty probes ten seconds apart. They carry no
data, so the board sees nothing, and they are what keeps a NAT box from
dropping an idle line. If a line still drops while the character stands
idle, `keepalive_seconds = 30` sends a telnet no-op after every thirty
seconds without a send. The board's telnet layer eats the two bytes, so
nothing is printed and nothing is echoed. Zero, the default, sends none.
Each one is logged as `!! keepalive`. The farm never needs it: its own
`look` at an empty stop keeps the line busy.

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
the assist refusing to start, a rest. Ordinary output never lights it.
Switching to the window clears it. The same events are written to the
lobby's log with a UTC time, the window number and the character.

A rest is logged twice over. Every rest or meditate this client sends
says which part sent it and what it read, so the lobby answers "why is
it resting" without a guess:

```
14:02:11 window 2 salad@bbs:23 rest: assist: rest, hp 12/52 23%, mana 5/20 25%, rest_at 60%, mana_rest_at 30%
14:02:11 window 2 salad@bbs:23 rest: the prompt shows (Resting)
```

The sender is `assist`, `stop <room>` for a farm's or a roam's stop, or
`departure gate` for the wait before a leg sets off, with `after a flee`
when it follows one. The gate's line also names the mark the recovery
runs to, as `depart at 95%`. The second line is the board's own word, written
each time the prompt starts showing a recovery. A prompt line with no
send before it is a rest this client did not ask for.

The bar is painted only when its text changes, and never by clearing the
row first, so it no longer flashes over a slow link.

Started as `mmc play` with no profile, the client opens the lobby alone.
Started with `--profile`, it opens the lobby and window 2 connected to
that profile, and shows window 2. A capture, when given, records every
window's first connection, each under its own name (see `--capture`).

## The status bar

In play the bar is prefixed with the window's number, and carries the
activity list described under Windows.

One renderer for both commands, so a session looks the same whichever
started it. `mmc farm` reserves the bottom terminal row and scrolls the
feed above it (DECSTBM, the same mechanism `mmc play` uses):

```
 attacking cave bear | HP 23 MA 8 | Small Cavern [1/2156]
```

With no runner attached — ordinary interactive play — the activity is
absent, but the **position is still tracked**:

```
 HP 38 MA 10 | Newhaven, Narrow Road [1/2146]
```

With the rates and the level known the bar also shows experience per
hour, the coins picked up per hour valued in gold, and the time to the
next level at the experience rate. Income counts the board's "You picked
up 11 silver nobles" confirmations and nothing else: a pile the bot left
on the floor, a sale and a deposit all change nothing. `/go` and `/farm`
restart both rates together, so a job is measured from its own start.

```
 HP 38 MA 10 | Tick 3.2 | HP 12.3 | MA 12.3 | Newhaven, Narrow Road [1/2146] | 4200 xp/hr | 12.4 gold/hr | L3->4 1h12m
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
 HP 42 MA 12 (Resting) | Dark Cave [1/2160]
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
 HP 42 MA 12 (Resting) | Tick 3.2 | HP 12.3/4.5 | MA 12.3 | Dark Cave [1/2160]
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

## Parties

Two or more characters run by this client can play as a board party. The
leader walks and the board drags the followers along, printing each room
to them as though they had stepped into it themselves. The client learns
the party from the board's own lines: `You are now following Foo`, `Foo
started to follow you.`, the invite, the two wordings for leaving, and
the `par` roster block, which replaces the member list wholesale so a
stale list heals. Nothing here invites or follows for you. You type
`invite`, `follow` and `par` yourself and the client reads what comes
back.

The party lives in the session beside the purse and the stat sheet, so
it survives a flee's rebuild of the assist and it is there whether a job
runs or not. The status bar shows `following Foo` while following and
`leading 2` while leading. Every role change, every request sent and
every request accepted is a lobby note, as
`-- party: Foo asks @bank --`.

### The telepath channel

A telepath whose text starts with `@` is a command for the receiving
client rather than something to read. That is MudPlay's protocol and
this client speaks it, so a MudPlay peer understands what this client
sends. The wire form is the board's own `/Foo @ok`.

| Command | Meaning |
|---|---|
| `@bank` | the sender's purse has tripped its deposit gate and it wants the party taken to a bank |
| `@wait` | hold the leg, the sender is busy |
| `@ok` | the sender is done, release the hold |

The sender has to be in the party. A leader takes a command from a
member of its roster, a follower takes one from its leader, and anything
from anyone else is dropped without a word. An invited character has not
joined yet and cannot hold the party. An unknown word after the `@` is
dropped the same silent way, as MudPlay drops it. A telepath without an
`@` is chat and is left alone.

### Following

A follower is the assist plus the party state. There is no follower job.
Everything a follower does by itself is assist behaviour under
**`/bot`**, and with the switch off none of it runs, the `@ok` replies
included, so a follower with the bot off costs its leader the full bank
wait. The assist already fights, sweeps and keeps stealth up in a room
it was dragged into. The party adds three things.

**A deposit gate.** The follower counts the same coin pickups a farm
counts, spends one `i` on the next idle prompt, and judges the purse by
the same `[bank]` gates a farm uses. It cannot walk to a bank, so acting
on the gate means asking: it telepaths its leader `@bank` once and does
not ask again until it has deposited or five minutes have passed.
`[bank].auto_deposit` gates it.

**A deposit on arrival.** A room block naming one of the five banks,
while following, is a bank arrival. The deposit runs in the job slot as
a short automatic task, shown in the activity list as `party deposit`,
so the assist stays quiet for the few seconds it takes, the way it does
for a job you typed. It reads the purse, deposits everything above
`keep_gold`, reads once more, and telepaths the leader `@ok` whatever
happened. A leader who moved on before the deposit landed gets the `@ok`
anyway, and the board's refusal is printed on the follower's screen.
Nothing above the keep floor means `@ok` at once with no deposit,
because a leader standing at the bank must never be held by a follower
whose purse was empty. **Ctrl-F** during the task takes the keyboard
back and aborts it, and then no `@ok` goes out and the leader waits out
`bank_wait_secs`.

The arrival is recognised by the room's printed name, which is not the
shop's name for four of the five banks. The names are unique across the
world, so the block alone settles it without a route or a position. A
block that repeats the room the character is already standing in starts
no second deposit, which is what keeps the `look` a finished job sends
from running the exchange again.

**A wait before a rest.** When the assist decides to rest or meditate it
telepaths `@wait` first, so the leader stands still rather than dragging
the character out of its rest. The `@ok` goes out on the first prompt
without the Resting or Meditating word after one that had it, which
covers both the mark being reached and a fight breaking the rest, or at
once when the board refused the rest. One of each per rest, under
`auto_rest` like the rest itself.

Jobs that move refuse to start while following. `/go`, `/farm`, `/roam`,
`/bank` and `/recover` answer
`following Foo; a job that moves does not run in a party`. A walk of its
own fights the drag, and the two undo each other until the character is
nowhere either of them meant. `/where` still runs.

### Leading

A leader runs the jobs it always did. Two hooks and one interrupt make
them party-aware, and out of a party every one of them is inert.

**A hold stops the leg.** A follower's `@wait` puts a hold on its
sender, and the telepath line is itself the interruption, so the hold
lands before the next step goes out. A step already sent cannot be
recalled. The leader then stands where it is, defends itself with the
defence any walk interruption gets, and resumes the leg from the room it
is in once every hold is gone. A hold that expires is dropped and named:
`-- party: hold on foo expired --`. **`[party].wait_secs`** is how long
one lasts, 90 seconds by default. The leg looks for holds before its first
step as well, so one that arrived during a stop is honoured. `/go`,
`/farm` and `/roam` walk through the same leg, so a follower's `@wait`
holds all three.

**A `@bank` detours.** The leader drains the requests where a farm
judges its own deposit gate, at the end of a stop, so an ask and its own
gate tripping together make one errand rather than two. The detour runs
even with `[bank].auto_deposit` off, because an errand asked for by name
is not automatic behaviour. An earlier errand that failed and switched
deposits off for the run still stops it. Arriving with nothing above its
own keep floor is not a failure on an asked errand, and deposits stay
on.

**Then the wait.** After its own deposit the leader sends `par`, waits
up to five seconds for the roster, and puts a hold on every member that
is not invited. It stands at the bank until each has said `@ok` or
**`[party].bank_wait_secs`** passes, 15 seconds by default, and the notice
at the end names any follower that did not reply. This happens on every
errand while leading, asked or not, since the leader's own gate trip
drags the whole party into the bank. `par` is sent there and nowhere
else. A follower that left without the leader hearing costs one timeout,
and the roster at the next bank corrects it.

With no job running, only the assist, a `@bank` or a `@wait` is printed
as a note and nothing else happens. The bank request stays queued for
the next farm. There is nothing to hold still when nothing is moving.

### `[party]`

```toml
[party]
wait_secs = 90        # a follower's @wait holds the leader this long at most
bank_wait_secs = 15   # the leader waits at the bank this long for @ok replies
follow_normal = true  # send "set follow normal" when following begins
```

All three are live keys, so `/set party.bank_wait_secs 30` reaches a run
already going. A `wait_secs` or a `bank_wait_secs` of 0 is refused.

**`follow_normal`**, on by default, sends `set follow normal` once each
time the character begins following. The board keeps a follow mode per
character and this client assumes the normal one, where the leader's
move prints the room. `set follow blind` prints the drag line alone, and
a follower in that mode sees no room, so it cannot fight, sweep or
recognise a bank. The mode is a board setting that lives on the
character, so it sits beside `disable_evil_warnings` and is not under
`/bot`.

Every party wording this client matches comes from the string table of
the stock `WCCMMUD.DLL` and none of them has been captured on the live
board. A wording that never matches fails safe: the party is never seen
and nothing party-shaped runs. The first live party settles them.

The follow-ups are the assist's backstab weapon swap, with a
`[bot].backstab_weapon` key naming the weapon or meaning any capable one
in the pack, since a follower dragged into a room is exactly the
character that wants the swap. Then gangpath and room speech as command
channels, the reply-only queries `@health`, `@where`, `@party`,
`@wealth` and `@version`, poison and blindness as wait reasons, a reply
telepath to the follower when the leader's errand fails, and an `@ok`
when the operator takes the keyboard back during a deposit.

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

**In a party.** A follower cannot walk itself to a bank, so a follower
whose gate trips telepaths its leader `@bank` and the leader detours at
the end of its next stop. A follower dragged into a bank deposits there
on its own and tells the leader `@ok` when it is done. A leader's errand
holds every follower at the bank until each has replied or
`[party].bank_wait_secs` runs out, and that happens on every errand
while leading, whether a follower asked for it or the leader's own gate
tripped. An asked errand that finds nothing above the keep floor is not
a failure and leaves deposits on. See [Parties](#parties).

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
pinned, because the refusal is the board's, not the client's. The HP gate
is a rest, so it follows `bot.auto_rest` and the `/bot` switch: with
either off, nothing but dying stops the leg.

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

