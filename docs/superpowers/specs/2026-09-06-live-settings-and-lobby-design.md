# Live settings and a lobby: `/set`, `/save`, `/load`, `/connect`

**Status:** accepted 2026-09-06
**Scope:** `crates/mud-client`. `profile.rs`, `session.rs`, `tui.rs`,
`cli.rs`, `bin/mmc.rs`, a new `settings.rs`, `docs/mud-client.md`.

## The problem this solves

Every setting the client has lives in a TOML profile that is read once,
before the connection opens. Tuning a heal mark or an ignore list means
quitting, editing the file, and logging back in. The client also cannot
start without a profile, so a new board or a new character needs a file
written by hand before the first connection.

irssi solves both with `/set`, `/save` and `/connect`. This client gets the
same three, plus tab completion so the key names do not have to be
remembered.

## Decisions

These were settled in the design conversation and are not open.

- **The profile text is the source of truth.** `/set` edits the TOML
  document, then the whole document is re-parsed into a `Profile` and
  validated. There is no per-key setter. Comments and key order in the
  file survive a `/save`, because the document is what gets written.
- **A value is a TOML value.** The text after the key is parsed as one. If
  that fails, the whole rest of the line is a string. So
  `/set bot.rest_at_percent 60`, `/set bot.meditate true`,
  `/set bot.buffs ["bless", "vigor"]` and `/set bot.rest_command rest`
  all work, and they read exactly as they would in the file.
- **A bare `/set` lists by glob.** `*` matches anything and a trailing `*`
  is implied. `/set bot` and `/set bot.*` list the bot table, `/set
  bot.rest` and `/set bot.rest*` list the two rest marks, `/set *heal*`
  finds every heal key in every table. A pattern with `*` and a value is
  refused.
- **`/save` needs a path once.** Started with `--profile`, `/save` writes
  back to that file. Started bare, the first `/save` must name a file.
  The path is remembered either way. `/load <file>` replaces the settings
  and makes that file the current path.
- **The assist rebuilds now, jobs read at their next start.** A change to
  a `bot.*` key rebuilds the assist on the spot when it is on. A running
  `/farm`, `/go` or `/bank` keeps the config it started with. Connection
  keys apply at the next `/connect`.
- **A disconnect lands in a lobby.** The lobby is the client with no
  session: settings commands, `/connect`, `/help`, `/quit`. The board
  dropping the line, or `/disconnect`, returns to it with settings intact.
- **Jobs and the assist refuse to start with an empty `username`.** The
  runner matches its own death line against the name. The refusal names
  the key to set.
- **`/quit` refuses once while settings are unsaved.** The second `/quit`
  exits. Ctrl-Q exits without asking.

## Commands

| Command | Effect |
| --- | --- |
| `/set` | List every key with its value, grouped by table. Password masked. |
| `/set <pattern>` | List the keys the glob matches. |
| `/set <key> <value>` | Write the key. The note prints the key, its new value, and `unsaved`. |
| `/unset <key>` | Remove the key from the file so its default applies. The only way back to none for `bank.at`, `farm.finish_at`, `farm.depart_at_percent` and `pace_ms`. |
| `/save [file]` | Write the document. No path and no current file is refused. |
| `/load <file>` | Replace the settings from a file. The connection stays open. |
| `/connect [host[:port]]` | Set host and port when given, port 23 by default, then connect. Refused while connected. |
| `/disconnect` | Close the line and return to the lobby. |

`/help` gains a line per command.

## The settings module

`crates/mud-client/src/settings.rs` holds one struct, `Settings`:

- the profile text as a `toml_edit::DocumentMut`,
- the `Profile` parsed from it,
- the path it was loaded from or last saved to, or none,
- a dirty flag.

Operations:

- `Settings::default()` is an empty document, parsing to a default
  `Profile`. `Profile` gains `Default` and `#[serde(default)]`: target
  mbbs, port 23, empty host, username and password. `mmc farm` and `mmc
  run` refuse an empty host, because they have no lobby to fall back to.
- `Settings::load(path)` reads and parses the file and prints the
  renamed-key warnings. `Profile::load` becomes a call through it.
- `set(key, text)` parses the value, writes it at the dotted path, creating
  tables as needed, then re-parses the document into a `Profile`, runs
  `normalise` and `validate`, and marks dirty. Any failure restores the
  document and returns the message. Setting a key deletes its renamed
  alias from the same table, so a file with `heal_at_percent` does not end
  up holding both spellings, which serde rejects as a duplicate.
- `unset(key)` removes the key and re-parses the same way.
- `list(pattern)` returns key and value pairs read from the parsed profile,
  so an absent table shows its effective defaults. Password masked.
- `save(path)`, `dirty()`, `path()`, `profile()`.
- `KEYS`: a constant slice of every settable key in file order. The
  listing and completion both read it. A test builds a profile with every
  optional filled, serialises it, flattens the keys and asserts equality
  with `KEYS`. A new config field that is not added to the list fails
  that test.

`toml_edit` becomes a direct dependency. It is already compiled as part
of `toml`.

The session keeps a copy of the profile for the jobs that start later.
`Session::profile()` returns a clone instead of a reference, and
`Session::set_profile` replaces the copy under a lock. The play loop calls
it after every successful `/set`, `/unset` and `/load`.

One consequence: a profile with no `[bot]` table gives the assist attack
and loot on. The first `/set bot.anything` creates the table, and every
other bot key then takes its struct default, which is off for both. The
listing shows the effective values, so this is visible.

## Lobby and connect

`mmc play` makes `--profile` optional. The TUI's entry point becomes a
loop over two states.

**Lobby.** Raw mode, the same scroll region and status bar as play, with
the bar reading `not connected` and the current host and port. The input
line takes the settings commands, `/connect`, `/help` and `/quit`. Any
other line, slash or not, gets a note that the client is not connected.
Tab completion works.

**Play.** `/connect` builds a session from the current profile and hands
it to the play loop, which gains the settings commands and `/disconnect`.
Play returns a reason: quit, or line closed. Quit exits the program. Line
closed returns to the lobby with a note. Started with a profile whose
host is set, the client connects at once, as today.

The key handler stops touching the session. A typed line comes back as
`KeyOutcome::Send(line)` and passthrough bytes as `KeyOutcome::Raw(bytes)`.
The caller sends. The handler is then a pure function of the keystroke and
the editor, which is what lets the lobby and play share it.
`KeyOutcome::Note(text)` is added for completion output. `Refuse` stays.

The per-connection state in play stays local to `play`, which is entered
once per connection, so it is rebuilt on every `/connect` as it stands.
Turning it into a struct is the windows design's first task, where the
window task needs it. The room database and graph are loaded through a
cache keyed by path, so that design can share one copy between sessions.

## Tab completion

Tab completes the word under the cursor when the line starts with a slash.

- First word: the slash verbs. `/se<Tab>` gives `/set `.
- Second word after `/set` or `/unset`: the keys in `KEYS`.
  `/set bot.ign<Tab>` gives `/set bot.ignore_coins `.
- Anything else: Tab is ignored. No path completion, no value completion.

Matching is by prefix. One candidate replaces the word and appends a
space. Several replace the word with their longest common prefix, and when
the word did not grow the candidates are printed as a note.

`complete(line, cursor) -> Completion` is a pure function in the settings
module. The editor gains one method to replace the current word.

## Testing

- `tests/settings.rs`: set a scalar, a list, a string with spaces, a key in
  an absent table, a nested `farm.nav` key. A bad type is refused and the
  document is unchanged. Unset returns a key to its default. Save keeps
  comments and order, checked by writing a commented file, setting one
  key, and comparing the text. The renamed alias is dropped on set. Glob
  listing for `bot`, `bot.rest*` and `*heal*`. The `KEYS` drift test.
- Completion: verb, key, common prefix, candidates note, Tab off a slash
  line.
- Key handler: `Send` and `Raw` replace the two tests that hand it a live
  session. The new verbs parse, including `/set key` against `/set key
  value`, and a glob with a value is refused.
- Lobby: a scripted test starts with no profile, sets host and port to a
  local fake board, connects, and sees the banner. Same fixture style as
  the realm-entry board tests in `tests/tui.rs`.
- `docs/mud-client.md` gets a settings section and the new `mmc play`
  synopsis.

## Out of scope

- Multiple windows. A separate design, which this one prepares for with
  the per-connection struct and the content cache.
- Pushing a changed config into a running job.
- Completion for file paths, loop names or values.
- Trimming the session transcript, which grows for the life of a session.
  A bugfix of its own.

