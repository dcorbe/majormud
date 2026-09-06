# Windows: several clients in one process

**Status:** accepted 2026-09-06
**Scope:** `crates/mud-client`. `tui.rs`, `mapview.rs`, `settings.rs`,
`cli.rs`, `bin/mmc.rs`, new `window.rs` and `screen.rs`,
`docs/mud-client.md`. Builds on the live settings and lobby design of the
same date.

## The problem this solves

One `mmc` process is one character. Playing two means two processes, and
each loads the whole room database, which is 120 MB on disk and larger
in memory. The data set is the fixed cost. Sharing it between characters
needs both to live in one process, which needs a way to look at one
while the other keeps running.

irssi solves the same problem with numbered windows and a status window.
This client gets the same shape.

The rewrite also removes a fault in today's painter: every chunk of bytes
from the board redraws the status bar, erasing the row before rewriting
it, in several unbuffered writes. Over SSH the erase and the rewrite
arrive in different packets and the bar flashes on every line.

## Decisions

These were settled in the design conversation and are not open.

- **One task per window, one front end that owns the terminal.** A window
  holds its settings, an optional session with the per-connection state,
  an in-memory terminal screen, and an unread flag. The front end reads
  the keyboard, routes keys to the active window, paints the active
  window's screen, and draws the bar and input line.
- **Window 1 is the lobby.** It never connects. Its screen is the
  notification log. Its settings are the template every new window
  copies.
- **A terminal screen per window.** The board's bytes go into a `vt100`
  parser sized to the terminal minus the two reserved rows, with a
  scrollback capped at `scrollback_lines`. Switching repaints that screen
  exactly. Hidden windows keep updating.
- **Activity lights on major events only.** A death, a job or farm ending,
  a disconnect, the assist stopping on a refusal. Ordinary output never
  lights it. Looking at the window clears it.
- **The bar never flashes.** It is painted only when its text changes,
  by overwriting the row with a padded string, never by clearing it, and
  each frame is one buffered write.
- **The shared data set is a cache keyed by content path.** Two windows
  on the same content load the room database, graph and spawn table once.

## Windows and commands

Windows are numbered from 1 in creation order and renumbered when one
closes.

| Command | Effect |
| --- | --- |
| `/new [file]` | Open a window with a copy of the lobby's settings, or with that profile loaded, and switch to it. |
| `/1` .. `/9` | Switch to that window. |
| `/windows` | List each window's number, host, character and state. |
| `/close` | Close the current window. Refused while connected and refused for the lobby. |
| `/connect [host[:port]]` | In the lobby, open a window and connect it. Elsewhere, connect this window. |
| `/quit` | Refuse once if any window is connected or has unsaved settings, naming them. The second exits everything. |

Ctrl-Q exits at once. `/set`, `/save`, `/load`, `/unset`, `/help`, Tab
completion, Ctrl-F and Ctrl-P act on the current window. Passthrough is
per window. PageUp and PageDown scroll the current window and any other
key jumps back to the bottom.

`mmc play` opens the lobby alone. `mmc play --profile x` opens the lobby
and a second window connected with that profile, and switches to it.

Settings gain one top-level key, `scrollback_lines`, default 2000, read
when a window is created.

## The window task

`window.rs` holds the task. It is the play loop of the settings design
with three substitutions: keys arrive on a channel from the front end,
terminal writes become writes into the window's screen, and major events
are sent up the event channel. The per-connection struct from the
settings design is built on `/connect` and dropped on disconnect.

`screen.rs` wraps the `vt100` parser: feed bytes, feed a note as its own
line, resize, scroll by a page, jump to the bottom, and the text and
formatted contents for painting and for tests.

The screen sits behind a lock shared with the front end. After each write
the window signals the front end with a change notice that says whether
the write was a major event.

## The front end

One task owns the terminal. It holds the key reader thread, the windows,
the active window's number, the content cache, and the last frame it
drew. Its loop has three inputs.

- **A keystroke.** `/new`, `/close`, `/windows`, `/quit`, the switch
  commands and Ctrl-Q are the front end's own. Everything else goes down
  the active window's key channel.
- **A screen change.** From the active window, a repaint of the screen as
  a diff against the last frame. From a hidden window, the unread flag if
  the change was a major event.
- **A window event.** Connected, disconnected, died, job ended, farm
  stopped, assist stopped. Each prints a timestamped line in the lobby
  log naming the window and the character, and sets the unread flag when
  the window is not active.

A terminal resize resizes every window's parser and repaints in full.

The frame builder is a pure function from the active screen, the bar
text, the editor and the last frame to bytes. It emits the bar only when
its text changed and never emits a clear-line sequence.

The bar shows the active window's number, then what it shows today, then
`Act: 3,4` when other windows have unread events. The lobby's bar reads
`1: lobby`.

## The map view

The map view keeps drawing straight to the terminal on the alternate
screen. It already takes a key receiver and the raw and event streams.
When a window starts it, the window tells the front end, which stops
painting and forwards every key to the map view until it returns, then
repaints in full. Switching windows is refused while a map view is up.

## Testing

- **Screen.** Board bytes appear on the screen, the scrollback is capped
  at `scrollback_lines`, PageUp scrolls and a keystroke returns to the
  bottom, a note lands as its own line.
- **Window commands.** `/new` opens and switches, `/2` switches, `/close`
  refuses a connected window and the lobby, `/windows` lists, `/quit`
  refuses once and then exits, renumbering after a close.
- **Lobby.** `/connect host:port` from the lobby opens a window connected
  to the local fake board and the banner shows on that window's screen,
  not the lobby's. `/set` in the lobby is what `/new` copies.
- **Events.** A fake board dropping the line disconnects the window, the
  lobby log gets the line, and the activity list names the window while
  another is active. Same for a death line.
- **Painting.** An unchanged bar produces no bar bytes. No frame contains
  a clear-line sequence.
- **Shared data.** Two windows with the same content path get the same
  shared reference.
- **Existing tests.** The bar and editor tests stay. The tests that drove
  `play` through seams such as the realm-entry board move to driving a
  window.
- `docs/mud-client.md` gets a windows section and the activity bar.

## Out of scope

- Split screens, or more than one window visible at once.
- Alt-number switching. `/N` is the only switch.
- Per-window colours or themes.
- Trimming the session transcript, which is its own bugfix.
