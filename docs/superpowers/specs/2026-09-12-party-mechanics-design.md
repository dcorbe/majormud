# Party mechanics: party state, the telepath channel, `@bank`, `@wait` and `@ok`

**Status:** accepted 2026-09-12
**Scope:** `crates/mud-client`. A new `party.rs`, then `session.rs`,
`bot.rs`, `tui.rs`, `nav.rs`, `farm.rs`, `bank.rs`, `profile.rs`,
`settings.rs`, `docs/mud-client.md`.

## The problem this solves

Two or more characters run by this client can play as a party on the
board. The board drags a follower wherever the leader walks, and the
client already fights, sweeps and keeps stealth up in a room it was
dragged into. What it lacks is any idea that it is in a party, any way
for the characters to talk to each other, and any answer to the one
question a party of farmers asks every hour: when does the party go to
the bank.

MudPlay and MegaMud solve the talking part with a remote command
protocol. A telepath whose text starts with `@` is a command for the
receiving client. This design adopts that protocol and its wire shapes
so the characters stay compatible with MudPlay peers, and adds one
command MudPlay does not have. `@bank` lets a follower tell its leader
that its purse has tripped the deposit gate. The leader detours to a
bank, the followers are dragged in, every one of them deposits, and the
leader waits for them before walking on.

## Decisions

These were settled in the design conversation and are not open.

- **This cut is the party core plus three commands.** Party state, the
  telepath channel, `@bank`, `@wait` and `@ok`. The queries, the other
  channels and the remaining MudPlay commands are follow-ups on the same
  grammar.
- **Senders must be in the party.** There is no character database, so
  the sender of an `@` command must be the leader or on the member
  list. Anything else is dropped without a word.
- **Nothing sends `invite` or `follow`.** The operator types them. No
  auto invite, no auto join.
- **A follower is the assist plus party state.** No job. Every
  follower behaviour is assist behaviour under `/bot`, reading an
  existing profile key where one fits. Jobs that move refuse to start
  while following.
- **The leader detours at the end of the current stop.** A follower's
  `@bank` is acted on where the farm judges its own deposit gate.
- **`@wait` fires at once.** The telepath line itself is the
  interruption. No further step is sent after it.
- **The bank wait ends on every follower's `@ok` or the timeout.**
  Whichever comes first.
- **Telepaths are the only command channel in this cut.**
- **A follower deposits whenever it is dragged into a bank.** Whoever
  asked. The leader's own gate trip banks the whole party.
- **Party state lives in the session.** Not in the bot, which a flee
  recovery rebuilds mid-stop, and not in a job, which ends.

## The wordings

Every line below comes from the string table of the stock `WCCMMUD.DLL`.
None has been captured on the live board. Each is UNVERIFIED there and
the module says so at its top, the same caveat `bank.rs` carries. A
wording that never matches fails safe: the party is never seen and
nothing party-shaped runs.

| meaning | line |
|---|---|
| this character now follows X | `You are now following X` |
| X now follows this character | `X started to follow you.` |
| this character invited X | `You have invited X to follow you.` |
| this character stopped following X | `You are no longer following X.` |
| X was removed from this character's followers | `X has been removed from your followers.` |
| the party is over | `You are not in a party at the present time.` |
| roster header | `The following people are in your travel party:` |
| roster row | an indented row, name first, with `[Invited]` on an invited row |
| the drag | ` -- Following your Party leader <direction> --` |
| an inbound telepath | `Foo telepaths: text` |
| the echo of an outbound telepath | `--- Telepath Sent to Foo ---` |

The stock board has no line for the leader when a follower stops
following. The roster is how the leader finds out.

The outbound telepath is `/Foo text`. That is the wire form MudPlay
sends on the live board.

`set follow` has a BLIND mode that prints only the drag line and no room
block. A follower in that mode cannot see the room it was dragged into,
so it cannot fight, sweep, or recognise a bank.

## `party.rs`

Pure functions and one state machine. Nothing here sends anything.

### `PartyState`

```
pub enum Role { None, Leader, Follower }

pub struct Member { pub name: String, pub invited: bool }

pub struct PartyState {
    pub role: Role,
    pub leader: Option<String>,
    pub members: Vec<Member>,
}
```

`observe(&mut self, line: &str) -> Option<Change>` drives it off the
table above. `You are now following X` sets Follower with leader X and
an empty member list. `X started to follow you.` sets Leader and adds X,
clearing an invited flag X already had. `You have invited X to follow
you.` sets Leader and adds X as invited. `You are no longer following
X.` and `You are not in a party at the present time.` reset to None. `X
has been removed from your followers.` drops X. The roster block, from
its header to the next blank line or prompt, replaces the member list
wholesale, invited flags from the rows, so a stale roster heals on the
next `par`. A roster with no rows while the role is Leader resets to
None. A row is any indented line before the block ends. Its name is the
first word and it is invited when `[Invited]` appears anywhere on it.
The stock DLL prints name and class in two columns and MudPlay's board
printed class in brackets with health after it, and the parse must not
care which.

`Change` says what happened in words the lobby can print: began
leading, began following X, X joined, X left, the party ended, the
roster was refreshed.

### The telepath grammar

```
pub enum Remote { Bank, Wait, Ok }

pub struct Request { pub from: String, pub command: Remote }

pub fn remote(line: &str) -> Option<Request>
```

`remote` matches `^(\w+) telepaths: @(\S+)` on the ANSI-stripped line.
The word after `@` is matched case-insensitively against `bank`, `wait`
and `ok`. Any other word is `None`, so an unknown command is dropped
silently, as MudPlay drops them. A telepath without `@` is chat. The
send echo is not a command.

`permitted(state: &PartyState, sender: &str) -> bool` is true when the
sender is the leader or a member that is not invited. An invited
character is not in the party yet and cannot hold it. Names compare
case-insensitively.

`telepath(to: &str, text: &str) -> String` returns `/{to} {text}`.

### The hold set

```
pub struct Holds { by: BTreeMap<String, Instant> }
```

`hold(name, until)` inserts or extends. `release(name)` removes.
`expire(now) -> Vec<String>` removes and returns every name whose
deadline passed. `is_empty()`. `retain_members(&PartyState)` drops a
name no longer in the party. A hold is a follower the leader must not
walk away from, whether the follower said `@wait` or the leader put it
there for a bank wait.

### `BankRooms`

`bank_names(content) -> BTreeSet<String>` is the set of the five bank
room names from `bank::bank_rooms`. The names are unique across the
shipped world, checked 2026-09-12, so a room block naming one of them is
a bank arrival with no localisation.

## The session tracker

`session.rs` gains a `PartyTracker` fed in the same loop as the purse,
stats and contents trackers, so it sees every line in every mode and
survives a flee's bot rebuild.

```
struct PartyTracker {
    state: PartyState,
    holds: Holds,
    requests: Vec<Request>,
    changes: watch::Sender<PartyState>,
}
```

On each line it calls `observe`, and on a `Change` publishes the new
state and reports a lobby notice. Then it calls `remote`. A request
that is not `permitted` is dropped. `Wait` inserts a hold for the
sender with a deadline of now plus `[party].wait_secs`. `Ok` releases
the sender. `Bank` is pushed onto `requests`. The tracker prints a
notice for every request it accepts, naming the sender and the command.

Accessors:

- `session.party() -> PartyState`, a snapshot.
- `session.party_changes() -> watch::Receiver<PartyState>`.
- `session.take_party_requests() -> Vec<Request>`, which drains the
  queue. One consumer drains it at a time: the assist while no job
  runs, the job otherwise, which is already how the two share the
  session.
- `session.party_hold(name, until)`, `session.party_holds_clear() ->
  bool` and `session.party_expire_holds() -> Vec<String>`, for the
  leader's leg and errand.

The party ending clears the holds and the queue.

## Follower behaviour

All of it is assist behaviour in `assist_tick` and the `Bot`, runs only
while `session.party().role` is Follower, and is under `/bot`. With the
switch off none of it runs, including the `@ok` replies, so a follower
with the switch off costs its leader the full bank timeout. That is the
switch working as specified.

### The deposit gate

The bot counts the board's coin pickup confirmation, the same
`bot::picked_up` line the farm counts. On the first idle prompt after a
pickup the assist sends `i` and judges a `BankGate` with the `[bank]`
config, seeded from the realm-entry inventory reading. The rule is the
farm's: over `deposit_at_coins` and above the keep floor, or a weight
class crossing. On `Judgement::Deposit` it telepaths `@bank` to the
leader once and notes the time. It does not ask again until it has
deposited or five minutes have passed. Gated by `[bank].auto_deposit`,
whose meaning stays "judge the gate and act on it". For a follower,
acting is asking.

### The deposit on arrival

A `RoomSeen` whose name is in `bank_names`, while following, is a bank
arrival. The deposit is a three-command exchange, `i`, `deposit`, `i`,
and the assist tick handles one event at a time, so the exchange is
split out of `bank::errand` as `bank::deposit_here(session, cfg) ->
ErrandEnd`, the errand's at-the-bank half, and the errand calls it too.
On the arrival the tui runs `deposit_here` as a short automatic task
and pauses the assist for its duration, the same way it pauses the
assist for a job. When it returns, the tui telepaths `@ok` to the
leader whatever the outcome. Nothing above the floor means `@ok` at
once with no deposit. `DepositReply::NotABank` means the leader moved
on before the deposit landed. That is printed and `@ok` is still sent.
Same `auto_deposit` key. This task is automatic and under `/bot`, so
the rule that jobs do not run while following does not apply to it. A
fight that breaks out in the bank during the exchange is unattended
for those few seconds, as it is during the farm's errand.

### The wait handshake

When the assist's rest decision fires while following, it telepaths
`@wait` to the leader before the rest command. The `@ok` goes out on
the first prompt without the Resting status after a prompt that had it,
which covers the `rest_until` mark and a fight breaking the rest, or at
once when the board refused the rest, which `HealWatch` already
detects. Edge-triggered: one of each per rest. Under `auto_rest` like
the rest itself. Poison and blindness are not wait reasons in this cut.

### Following normal

`[party].follow_normal`, default true, sends `set follow normal` once
each time the character begins following. The tui sends it on the
began-following change, since the tracker only observes. It is a board
setting that persists on the character, so it sits beside
`disable_evil_warnings` as a profile key and not under `/bot`.

### Jobs that move refuse to start

`/go`, `/farm`, `/roam`, `/bank` and `/recover` check the party state
at their start and return with "following Foo; a job that moves does
not run in a party".

### A leader with no job

Only the assist running, a `Bank` or `Wait` from a follower is printed
as a lobby notice. The bank request stays queued for the next farm.
There is nothing to hold still when nothing moves.

## Leader behaviour

Two hooks in the farm and one new interrupt. Not in a party means every
one of them is inert and the farm runs exactly as today.

### Held on the way

`nav::Interrupt` gains `Held`. The farm's travel guard raises it on any
event while the session's hold set is non-empty, after expiring stale
holds. The `@wait` telepath line is itself an event, so the hold fires
the moment the line arrives. A step already sent cannot be recalled,
but no further step is sent, the way a death hands the walk back at
once rather than arming it the way a hurt threshold does.

`farm::travel` handles `Held` like its other interrupts: the leader
stands where it is, defends itself with the same defence a walk
interruption gets, and resumes the leg from its current room once
`party_holds_clear` is true. Waiting is a loop over session events that
expires holds on each pass, prints a notice naming each follower whose
hold expired, and honours `Live` changes and the bot switch. The leg's
entry checks the set too, so a hold that arrived during a stop is
honoured before the first step. Roams and circuits share the leg, so
both get it.

### The bank request

At the point where the farm judges its own gate after a stop, it drains
`take_party_requests`. A `Bank` marks the errand as asked and names the
follower in a notice. The errand runs when the gate said Deposit or a
follower asked. `bank::errand` takes `asked: bool`. When asked, the
leader arriving with nothing above its own keep floor deposits nothing,
says so, and deposits stay on. Only a bank no route reaches, or a walk
that fails, switches deposits off for the run, as today. There is no
reply telepath to the follower in this cut.

### The bank wait

After its own deposit, while `role` is Leader, the errand sends `par`
and waits up to five seconds for the roster refresh on
`party_changes`, keeping the roster it has if none comes, then calls
`party_hold` for every
member that is not invited, with a deadline of now plus
`[party].bank_wait_secs`. It then waits through the same hold loop the
leg uses. Every `@ok` releases one. The notice at the end names any
follower that did not reply. This happens on every errand while
leading, asked or not, since the leader's own gate trip drags the
followers into the bank too.

### The roster

`par` is sent at the bank and nowhere else. No periodic poll. A
follower that left without the leader hearing costs one bank wait's
timeout, and the `par` at the next bank corrects the roster.

## Configuration

One new table.

```toml
[party]
wait_secs = 90        # a follower's @wait holds the leader this long at most
bank_wait_secs = 15   # the leader waits at the bank this long for @ok replies
follow_normal = true  # send "set follow normal" when following begins
```

`PartyConfig` in `party.rs` with `Default`, `#[serde(default)]` on the
table, held on `Profile` beside `bank`. The three keys join
`settings::KEYS`, so `/set party.bank_wait_secs 30` works mid-run
through the same `Live` path `[bank]` uses. `wait_secs` and
`bank_wait_secs` of 0 are refused by `validate`.

## Operator surface

- A lobby notice on every role change, every request accepted, every
  request sent, every hold expired, and the outcome of every bank wait,
  in the wording the notices use today.
- The status bar shows `following Foo` or `leading 2` while in a party.
- No new slash commands. `invite`, `follow` and `par` are typed by hand.

## Error handling

- A wording that never matches fails safe. The party is never seen and
  the character behaves as today.
- A telepath from a sender not in the party is dropped without a word.
- An unknown `@` word is dropped without a word.
- A follower's deposit that gets "not in a bank" still sends `@ok`.
- A follower with no room database cannot recognise a bank. It sends
  `@bank` on the gate and never deposits. The notice at assist start
  says so, as the farm's item identity notice does.
- A leader whose errand cannot route reports it and switches deposits
  off for the run, as today.
- A hold that expires is dropped with a notice naming the follower.
- A death on either side goes through the existing death path. The
  follower's rest ends, so its `@wait` is released by the leader's
  timeout at worst. The leader's leg ends, so its holds are moot.
- `/bot` off stops every automatic send on both sides.

## Testing

No test reads `re/`. Bank room names come from a fixture content table,
as the banking tests do.

Unit tests in `party.rs`:

- the state machine over the six wordings and the roster block,
  including a roster that replaces a stale list, an invited row
  promoted by `started to follow you`, and a row in each of the two
  known formats
- the telepath grammar: a command, the three commands in any case, a
  non-`@` telepath, an unknown word, the send echo
- `permitted` for the leader, a member, a stranger, and an invited
  member that is not permitted
- the hold set: insert, release, expiry, a member dropping out

Scripted board tests, the pattern `tests/session_capabilities.rs` uses:

- a follower dragged into a bank room sends `i`, `deposit`, `i` and
  `/Leader @ok`, and the assist sends nothing of its own meanwhile
- a follower with nothing above the keep floor sends `/Leader @ok` and
  no deposit
- a follower's gate trip sends `/Leader @bank` once and not again on the
  next pickup
- a follower's rest sends `/Leader @wait` before the rest and `/Leader
  @ok` after
- a leader's leg stops on `Foo telepaths: @wait` before the next step
  and resumes on `@ok`
- a leader's errand sends `par`, waits for two followers, and leaves as
  soon as both reply
- a leader's errand leaves after the timeout with a notice naming the
  follower that did not reply
- a stranger's `@wait` does nothing
- `/bot` off: a follower dragged into a bank sends nothing

## Follow-ups, not in this spec

- The assist's backstab weapon swap, with a `[bot].backstab_weapon` key
  naming the weapon or meaning any capable one in the pack. Today the
  assist backstabs with whatever is wielded and never swaps. A follower
  is exactly the character that needs the swap.
- Gangpath and room speech as command channels.
- The reply-only queries: `@health`, `@where`, `@party`, `@wealth`,
  `@version`.
- Poison and blindness as wait reasons.
- A reply telepath to the follower when the leader's errand fails.
