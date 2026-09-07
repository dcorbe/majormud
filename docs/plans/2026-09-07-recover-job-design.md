# A death log and a `/recover` job (2026-09-07)

Base: `main` after the settings spec and the stealth spec. Client only. Third of three.

A full death drops everything the character carries into the room it died in. Getting
it back by hand from a room of baby green dragons is not going well. This adds a job
that sneaks in, searches once, picks up everything the search lists, and runs back to
where it started. It never attacks. It bails at the first broken sneak. And because
the client had no idea where the character died, it starts keeping a death log.

## Evidence

From `re/docs`, verified against the decompile notes, and from the content database.

- **Aggressive monsters acquire a target inside the 5 second combat round**, and skip
  any player flagged as having moved this round, per `monsters.md` §4. Pursuit runs once
  a second and refuses a target who moved this round. So a character that moves every
  round is not acquired and not followed. The run home needs no sneak. It needs no
  pauses.
- **One `sneak` arms it and every move rerolls.** A move that stayed sneaky answers
  with `Sneaking...`. A move that broke it answers with neither that line nor
  `You make a sound as you enter the room!`, per `theft.md` §11. The client already reads
  both lines per move in `Navigator::goto`.
- **`search` breaks a sneak** and costs a round, per `theft.md` §9. A bare `search`
  re-lists the room's items, hidden ones included, as `You notice ... here.`, or prints
  `Your search revealed nothing.`. In combat it prints `You may not search while
  attacking!`. The room parser already reads the `You notice` line into
  `RoomView::items`. The re-list wording is unverified on the live board.
- **The dragon rooms are dark.** All 38 Darkwood Forest rooms in the band that spawns
  baby green dragons have light -150. A blind search does nothing and prints nothing.
  The runners already light up from the stat sheet's light sources before stepping
  into darkness, and lighting breaks a sneak.
- **The client records no death.** A job that ends in a death carries no room. The
  window only learns of a death as a job ending. The board prints
  `You have been killed.` to the dying player, `<name> is dead.` to the room, and the
  prompt shows zero hitpoints. The client matches only the last two, and the first
  nowhere.
- **`get all` does not exist.** The board's `get` takes one floor item by word prefix
  or one coin denomination. There is no wording for an item too heavy to lift.

## 1. The death log

### The file

`~/.config/mmc/deaths.log`, beside the loop library, under the config base the
settings spec introduced. Append only. One line per death:

    2026-09-07T14:42:07Z beef 1/2810 Darkwood Forest confirmed
    2026-09-07T15:10:33Z beef 1/2811 Darkwood Forest stale
    2026-09-07T15:30:00Z beef - - unknown

Fields are separated by single spaces. The room name is last before the fix word and
may contain spaces. The fix word is `confirmed`, `stale` or `unknown`. A death with no
known room writes a line with `-` for the id and the name, so the death itself is
not lost.

### The trigger

Any of these, seen on the session:

- A prompt with hitpoints at zero.
- The line `You have been killed.`
- The line `<name> is dead.` where the name is the character's.

The first to arrive writes the line. The others are ignored until a prompt shows
hitpoints above zero again, which re-arms the trigger. One death, one line.

### The room

The character's last resolved fix at that moment. The window tracks the fix through
`lost::refix` on every room block, during a job as well as in hand play, so it is
current. The death line is written before the board renders the recall room, because
the killing prompt arrives first.

### Where it is written

- The window, in its event arm. That covers hand play, every job, and the map, which
  bounces back to the board on a death anyway.
- The headless `mmc farm`, from its own event loop, because a farm run there is where
  the character dies alone.

Both go through one function, `deathlog::record(character, fix, now)`. The character
name is `Session::character_name`, and the profile's username before a sheet exists.

### Reading it back

`deathlog::last(character) -> Option<Death>` returns the newest line for the character
that carries a room. It is what `/recover` with no argument uses.

## 2. Command and map key

### `/recover [room]`

`/recover <room>` takes the same target grammar as `/go`: a `map/room` id or a room
name. `/recover` alone takes the character's last logged death. The start room is
wherever the character stands, and that is also the safe room.

The verb joins `VERBS`, `slash` and `help_text`. A `KeyOutcome::Recover { target }`
carries the argument. The window handles it beside `Go`, through a `start_recover`
beside `start_go` in `tui.rs`.

### The map key

`R` with the cursor on a room returns `ViewAction::Recover(room)`. The window closes
the map and starts the job the way it does for `Go`. The panel's key line gains
`R recover`.

### Refusals

The job refuses to start, with one line saying why, when:

- Another job is running.
- The character's position is not confirmed.
- There is no target: `/recover` alone with no logged death that has a room.
- There is no route from here to the target.
- The stat sheet says stealth is zero.
- `bot.sneak` is off. The job is defined by the sneak in, so the switch refuses it
  rather than silently walking in visible.

## 3. The job

`recover::run_recover(session, graph, from, target, live, phase, notices) -> Result<RecoverEnd, FarmError>`
next to `run_go`. `live` is the settings receiver from the settings spec.

### Configs

Derived from the profile and reapplied on every live change:

- `BotConfig` with `auto_combat` false, `auto_flee` false, `auto_get` false. The job
  owns every pickup.
- `FarmConfig` from `go_config` with `fight_while_travelling` false,
  `interrupt_at_percent` zero, `bash_doors` false.
- `NavConfig` with `sneak` from the profile. `search_hidden` stays on, because a hidden
  exit on the route is the navigator's business.

`session.travel_fights()` is set false for the job's life and restored after.

### Phases

Published on the phase channel so the bar shows them: `Preparing`, `SneakingIn`,
`Sweeping`, `GoingHome`, then `Done`.

**Preparing.** In the start room. Light up if the target or any room on the route is
dark, through the same `ensure_lit` the farm uses. Cast the buffs in `bot.buffs`. Both
happen before the sneak because either breaks one. The stealth buff is the
navigator's, per the stealth spec, and is cast inside arming.

**Sneaking in.** The route is `graph.route(from, target)`. The job walks it one room
at a time, calling `nav.goto(session, here, next, guard, sneaking)` per hop and reading
`Arrival::sneaking` after each. Walking hop by hop is what makes every break visible.
A hop that arrives with `sneaking` false is a break, and the job turns for home from
that room. `You make a sound` on its own is not a break by the spec, and the job
treats it as one anyway. You said a failed sneak means bail, and the cost of a false
alarm is one walk.

The guard for this leg is a `FarmGuard` with the fight switch off and the hurt
interrupt off. A monster attacking mid route surfaces as `Interrupt::Attacked` from
the navigator, because the board refuses movement in combat. The job treats that as a
break too: turn for home, and keep sending the move until the round passes.

**Sweeping.** Section 4.

**Going home.** `nav.goto(session, here, from, guard, false)` under
`FarmGuard::death_only`, the guard the farm's own flee recovery uses, so blows do not
stop the walk. Unsneaked, because the sweep broke the sneak and re-arming costs a
round the character cannot stand still for. Every move is sent as soon as the previous
one is answered, which at the job's pace is inside the combat round.

### The pace

The job sets the pace from the profile at start, as every job does, and the settings
spec keeps it live. With `pace_ms` at zero every send goes at once.

## 4. The sweep

On arrival the character is still sneaking.

1. Send one bare `search`. The correlator gains a kind for the bare form that is
   completed by a room block, by `Your search revealed nothing.`, or by
   `You may not search while attacking!`. The directed form keeps its own kind.
2. `Your search revealed nothing.` means there is nothing to take. Go home.
   `You may not search while attacking!` means something already has the character.
   Go home.
3. The room block's `You notice ... here.` line is the list. Split on `, `. An entry
   that `bot::coin_pile` recognises is coins, taken by denomination. Anything else is
   an item, taken by name with any leading article removed. Entries that
   `items::resolve` cannot match are still attempted with their printed name, because
   the board matches by word prefix and the printed name is what it printed.
4. Gear first, coins last. Every entry is taken, including denominations in
   `bot.ignore_coins`, because the coins are the character's own. Gear goes first so a
   forced exit leaves coins behind rather than equipment.
5. One `get <name>` per entry, one in flight at a time. `You took <item>.` and
   `You picked up <n> <denomination>` retire the entry. `You don't see <name> here.`
   and `You don't see any <plural>` mean somebody else took it, and the entry is
   dropped. There is no wording for an item too heavy, so each entry gets three
   attempts and is then dropped, the way `LOOT_TRIES` works at a farm stop.
6. After every reply the job reads the prompt. Hitpoints under
   `bot.minor_heal_at_percent` of the known maximum end the sweep at once. A prompt at
   zero, or a death line, ends the job.
7. No second search. You asked for exactly one, and the board's search costs a round.

The maximum hitpoints come from `discover_vitals` at start when the session does not
already know them.

## 5. Ending and reporting

`RecoverEnd` carries what happened and where the character is:

| ending | phase text |
| --- | --- |
| swept and home | `recovered 5 of 7 items and 2 coin piles` |
| break on the way in | `sneak broke at 1/2810 Darkwood Forest, nothing taken` |
| hurt mid sweep | `hurt under 70%, back with 3 of 7 items` |
| empty search | `nothing there` |
| walk home did not complete | `stopped at 1/2812 Darkwood Forest with 3 of 7 items` |
| died | the `DIED` ending, no room |

The window adopts the ending's room as the character's fix, as it does for a go. The
items taken are printed one per line through `Notices` when the job ends, so they can
be checked against what was lost.

## Tests

Death log, in `tests/deathlog.rs` with a temporary config base:

- Each of the three triggers writes one line, and a second trigger before the
  hitpoints recover writes nothing.
- A stale fix writes `stale`. No fix writes `- - unknown`.
- `last` returns the newest line with a room for that character and skips the
  `unknown` one.
- The room name with spaces round trips.

Job, in `tests/recover_scripted.rs` on a scripted board, a corridor of three rooms with
the death room at the end:

- Happy path: `sneak`, two moves with `Sneaking...`, `search`, a block listing two
  items and a coin pile, three `get`s in gear first order, two moves home, and the
  ending names the counts.
- The second move answers without `Sneaking...`. The job sends no `search` and walks
  home from that room. The ending says where it broke.
- The search answers `Your search revealed nothing.` and the job walks home.
- A prompt under the mark after the first `get` ends the sweep with one item and the
  ending says so.
- `You don't see ... here.` drops an entry without a retry.
- An entry that never answers is attempted three times and dropped.
- A monster attacks during the sweep and the log holds no `a ` line, ever.
- The dragon room is dark and the job lights up in the start room before `sneak`.
- Stealth zero is refused before any send.

Map and command:

- `R` on a room yields `ViewAction::Recover(room)`.
- `/recover` is claimed by `slash` and listed by `help_text`.
- `/recover` with no logged death is refused with the reason.

## Out of scope

- A second search. A re-hide timer. Either is a later spec with a live capture behind
  it.
- Recovering worn equipment to its slots. The job puts things in the pack. Wearing them
  is the operator's.
- Recovery from a room the client has no route to.
- Marking the death room on the map. `/recover` alone already knows it.

