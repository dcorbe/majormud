# Profile names, a sneak switch, and live settings for running jobs (2026-09-07)

Base: `main` at 55a0494. Client only. First of three specs. The stealth spec and the
recovery spec build on this one.

Three problems, all in how settings reach the client.

1. `/save beef` wrote a file named `beef` in the current directory. The profiles live
   in `~/.config/mmc`, and that is where a bare name should go.
2. Every walk sneaks when the character has any stealth. There is no way to say no.
3. A `/set` reaches the assist at once and a running job never. The docs say a job
   keeps what it started with. Restarting a farm to change a rest mark is a wall.

## 1. Profile names resolve under the config directory

### Rule

A profile argument with no path separator and no `.toml` suffix names a profile in the
config directory. `beef` means `~/.config/mmc/beef.toml`. Anything with a separator or
the suffix is a path and is used as given. `chars/dan.toml` and `./beef.toml` stay what
they are.

The config directory is `$XDG_CONFIG_HOME/mmc`, or `~/.config/mmc`. `loops::dir` already
computes the base for the loop library. The base moves to one function the profile
resolver and the loop directory both call.

### Where it applies

Every place a profile name is typed:

- `/save NAME`, `/load NAME` and `/new NAME` in `tui::slash` and the lobby.
- `--profile NAME` on `mmc play`, `mmc run` and `mmc farm` in `cli.rs`.

The resolver is one function, `profile::resolve(arg: &str) -> PathBuf`, and every site
calls it. No site keeps its own rule.

### Overwrite guard

`Settings::save` refuses a path that exists and is not the path the settings were
loaded from. The message names the file and says what to do:

    save: ~/.config/mmc/beef.toml exists and these settings were not loaded from it. /load it first, or pick another name.

A bare `/save` with a remembered path writes as it does today. A `/save NAME` where
`NAME` resolves to the remembered path writes. A `/save NAME` onto a file that does
not exist writes and remembers the path.

This guard is what makes the resolver safe. The stray `beef` in the repo root came
from a bare lobby with three keys. Under the resolver alone it would have replaced
the real Beef profile, password included.

There is no force flag. Deleting the file by hand is the override.

### Tests

- `resolve("beef")` ends in `mmc/beef.toml` under the config base.
- `resolve("chars/dan.toml")`, `resolve("./beef")` and `resolve("beef.toml")` are used as
  given.
- `save` onto an existing file the settings did not come from is refused and the file
  is untouched.
- `save` onto the remembered path, and onto a fresh path, both write.
- The `/save`, `/load`, `/new` arms in `slash` hand the argument to the resolver. One
  test per verb, asserting the path the outcome carries.

## 2. `bot.auto_sneak`

### Rule

One new profile key, `bot.auto_sneak`, a boolean, default true. False means no walk arms a
sneak and, once the stealth spec lands, no stealth spell is cast for one.

### Where it applies

`BotConfig` gains `sneak: bool` with a serde default of true. `NavConfig` gains
`sneak: bool`. Wherever a `Navigator` is built from the profile, `nav.sneak` is set from
`bot.auto_sneak`. `Navigator::arm_sneak` returns `Ok(false)` when `sneak` is off, the same
early return it already takes when the sheet's stealth is zero.

The key joins `settings::KEYS` so `/set bot.sne<Tab>` completes it. It is a `bot.*` key,
so the assist rebuilds on the spot as it does for every other bot key, and a running
job picks it up through section 3.

### Tests

- `bot.auto_sneak` is in `KEYS` and a profile without it parses with `sneak: true`.
- A navigator with `sneak: false` and a stealthy character walks a two room corridor
  and the board log holds no `sneak`.
- The same walk with `sneak: true` sends one `sneak` before the first move.

## 3. Live settings for running jobs

### The event

The session's profile becomes a `tokio::sync::watch` channel. `Session::set_profile`
sends. `Session::profile` clones the current value, as today. A new
`Session::profile_changes() -> watch::Receiver<Profile>` is the event.

The window already calls `set_profile` on every `/set`, `/unset` and `/load` that
changes the profile. Nothing new publishes. Nothing polls.

### The receiver in a job

Every job takes the receiver at start: `run_farm`, `run_go`, `run_bank`, the roam, and
later `run_recover`. The job keeps its derived configs in one value it owns, rebuilt
from the profile whenever the receiver reports a change:

- `BotConfig`, with the job's own forced fields reapplied after the rebuild. A go keeps
  `auto_combat` off, a bank keeps `auto_get` off, a recover keeps both off. The forcing
  is a function of the job, applied to whatever profile arrives.
- `FarmConfig`, with the same treatment. A go still forces `bash_doors` off,
  `depart_at_percent` to none and `max_seconds` to zero.
- `NavConfig`, and the `Navigator` rebuilt from it. The navigator is cheap to build.
  The graph is shared and the capabilities and backstab choice are carried over.

The `travel_fights` switch on the session is already live and stays as it is.

### Where the job wakes

Where a job waits on board events in a `select!`, the walk's step wait, the stop pump,
the bank errand, the recover sweep, a `changed()` arm on the receiver wakes it. The job
rebuilds its configs and continues the loop. It does not abandon the step, the pass or
the pickup in flight.

Where a loop is pass based rather than event based, the top of the pass asks the
receiver `has_changed()`. That is one relaxed atomic load. It is not a poll of the
file or of the settings.

### Which keys are live

Live, taking effect at the next decision that reads them:

- `bot.*`: the heal, rest and flee marks, the heal and buff spells, `ignore`,
  `ignore_coins`, `auto_get`, `take_keys`, `sneak`.
- `farm.rest_at_percent`, `farm.rest_until_percent`, `farm.mana_rest_at_percent`,
  `farm.fight_while_travelling`, `farm.travel_interrupts`, `farm.interrupt_at_percent`,
  `farm.defend_seconds`, `farm.nav.*`.
- `bank.*` except `bank.at`.
- `pace_ms`. The writer reads the pace live, so the window calls `set_pace` on a change
  while a job runs. With no job running the pace stays at zero, as it does today.

Fixed at start:

- The loop and its stops, `farm.start`, `farm.finish_at`, `farm.circuit`.
- The go target. The recover target and start room. `bank.at`.
- The connection keys.

The docs table under "When a change takes effect" is rewritten to these two lists.
The sentence "A running job keeps what it started with" goes.

### What the job says

On picking up a change the job prints one line through its `Notices`, so the window
shows it landed:

    -- farm: settings reloaded --

One line per change, not per key.

### Headless

`mmc farm` and `mmc run` build a session whose profile never changes, so the receiver
never fires. They take the receiver like any job and nothing else is different.

### Tests

- A scripted farm changes `bot.ignore_coins` through `set_profile` between two stops.
  The first stop's copper pile is taken and the second stop's is left.
- A scripted go changes `farm.fight_while_travelling` mid walk and the guard's next
  sighting is handled the new way.
- A scripted go changes the go target mid walk and the walk ends at the original
  target.
- Each job prints the reloaded line exactly once for one change.
- `set_pace` is called with the new pace when `pace_ms` changes during a job, and not
  when no job runs.

## Out of scope

- A `/sneak` toggle command. `/set bot.auto_sneak false` is the one way.
- A force flag on `/save`.
- Applying structural keys to a running job.

