# Party heal: the roster with numbers, `@heal`, `@cure` and `@iam`

**Status:** accepted 2026-09-14
**Scope:** `crates/mud-client`. `party.rs`, `session.rs`, `sheet.rs`,
`tui.rs`, `window.rs`, `settings.rs`, `docs/mud-client.md`.

## The problem this solves

The party walks as one and fights as one, but every member heals alone.
A member with no heal spell and a low pool sits down, tells the leader
`@wait`, and the party stops for it. Two members now carry major
healing, cure poison and healing rain, and nothing lets them spend
those on the member that needs them. The fragile member dies with a
healer standing next to it.

The board already prints what a healer needs to know. The `party`
reply lists every member with class, pool and health as percentages:

```
The following people are in your travel party:
  Blueberry                      (Mystic)     [K:100%] [H:100%]   - Frontrank
  Beef                           (Ninja)               [H:100%]   - Midrank
  Salad                          (Ranger)     [M:100%] [H: 86%]   - Midrank
```

The client keeps only the names. This design keeps the numbers, polls
them, lets a member say when it is hurt, and lets a healer act on both.

## Decisions

These were settled in the design conversation and are not open.

- **Every healer that can afford the spell casts.** Two healers may
  both heal one member in one round. That is accepted as the cost of
  speed. No election, no announcement of who heals.
- **Everyone polls `party` on a timer.** Every member sends `party`
  every `[party].poll_secs` seconds while in the realm and in a party.
  Every window knows the whole party's state.
- **Rain goes out when two or more members are under the mark.** The
  healer counts itself. Otherwise one member gets a single heal by
  name.
- **Requests are said aloud.** A telepath goes to one name and a member
  does not know who heals. The party stands in one room, so the member
  says the word and every healer hears it. The room hears
  `Celery says "@heal 35"`. Strangers saying it are dropped by the
  member rule that already guards telepaths.
- **The roster is the model; a request is a fresher row.** A healer
  decides off one table. A request overwrites the requester's number
  with a newer one. A poll refreshes every row.
- **Members introduce themselves.** On joining, and to any member they
  have not met, a member says `@iam <race> <class>`. Witchunters resist
  all magic, and a healer must not spend a cast on one. Class is also
  on the roster, so either source marks a witchunter. Race is kept for
  the rules that will need it.
- **Asking follows the local heal rules.** `[bot].auto_heal` and its
  minor and major marks decide when a member is hurt. It asks when it
  is hurt and its own heal machine has nothing to cast. Self first,
  then the party.
- **`/bot` is the master switch.** The assist runs only with the bot
  on, so none of this runs with it off. `[party].heal` turns answering
  off for one character while the rest of the bot runs.

## The roster with numbers

`party.rs`. The roster row regex reads the class in parentheses, an
optional pool tagged `K` or `M`, and the health, all as whole percents:

```
^\s+(\w+)\s+\(([\w -]+)\)\s+(?:\[([KM]):\s*(\d+)%\]\s+)?\[H:\s*(\d+)%\]
```

`Member` gains `class: Option<String>`, `hp: Option<u8>` and
`pool: Option<u8>`. The `[Invited]` suffix still marks an invited
member, and an invited row has no numbers. The own row is dropped as
today.

A roster that changed only numbers is `Change::Vitals`, not
`Change::Roster`. The window prints a party note for `Roster` and
nothing for `Vitals`, so a poll every twenty seconds is silent.

## The poll

`window.rs`, beside the `exp` poll. A `party` interval of
`[party].poll_secs` seconds fires while `in_realm` and
`session.party().role != Role::None`. The correlator has no grammar
for `party`; the reply is recognised by its header whoever asked, the
same as `exp`. `poll_secs` of 0 turns the poll off.

## Three new words

`party::remote` reads two shapes:

```
^(\w+) telepaths: @(\S+)(?: (.*))?$
^(\w+) says "@(\S+)(?: (.*))?"$
```

and `Remote` gains:

- `Heal(u8)` from `@heal <percent>`. A missing or unreadable percent
  reads as 0, which is "as hurt as it gets".
- `Cure` from `@cure`.
- `Iam { race: String, class: String }` from `@iam <race> <class>`.
  The race is one word, the class is the rest of the line.

`permitted` guards all three as it guards `@bank`. The own echo
`You say "@heal 35"` matches neither shape and is ignored.

The wire form for a said word is `say @heal 35`. `party::say(text)`
builds it beside `party::telepath`.

## The health table

`party::Health`, held in the session's `PartyTracker` beside the
holds. It holds facts from the board only:

```rust
pub struct Vitals {
    pub name: String,
    pub class: Option<String>,
    pub race: Option<String>,
    pub hp: Option<u8>,
    pub pool: Option<u8>,
    /// When `hp` was last written, by a roster or a request.
    pub seen: Instant,
    /// When the member last said `@cure`. `None` once a roster
    /// arrives after it, since the member says it again on the next
    /// poison tick if it still needs curing.
    pub poisoned: Option<Instant>,
}
```

keyed by lowercased name. `feed_party` writes it:

- A roster block sets every row's class, hp, pool and `seen`, and
  clears `poisoned`. Names not on the roster are dropped.
- `@heal n` from X sets X's hp to n and `seen` to now.
- `@cure` from X sets X's `poisoned` to now.
- `@iam race class` from X sets X's race and class.
- `Change::Ended` clears the table.

`Session::party_health()` returns a copy, read only when the assist is
about to decide, the same rule as `party()`.

`Vitals::resists_magic()` is true when the class, from either source,
is `Witchunter`. Every rule below skips such a row.

## Introductions

`AssistCasts` keeps `met: BTreeSet<String>`. On a prompt, for every
member of the party not in `met`, the character says
`@iam <race> <class>` once and adds every current member to `met`.
The race and class are the stat sheet's; with either missing the
character says nothing and tries again on the next prompt. `met` is
cleared when the party ends, so a re-formed party is greeted again.
This greets on joining, since the whole party is unmet then, and greets
each newcomer once, since the newcomer is unmet when the roster or the
join line first names it.

## Asking

`party::AskState` in `AssistCasts`, paced by the round clock. On a
prompt, when `[bot].auto_heal` is on, `heal_need` says minor or major
for the own percent, and `assist_heal` returned nothing for that
prompt, the character says `@heal <percent>`, at most once per round.

On the line `You feel ill.` the character is poisoned. If its book has
cure poison it casts it on itself through the party cast state below,
else it says `@cure`, at most once per round. The board prints the
line on every poison tick, so a request that went unanswered is made
again on the next tick without any timer of its own.

## Answering

`sheet::PartyHeal`, built with the self heal sources from the same
book, in `AssistCasts` as `party`:

- `singles`: the minor and major `HealSource`s the self machine uses.
- `area`: the first of `healing rain`, `major healing rain`, `greater
  healing rain` in the book.
- `cure`: `cure poison` in the book.
- `mana`, `pending`, `last_attempt` and `dead` as `HealState` keeps
  them.
- `healed: BTreeMap<String, Instant>`, when each member was last
  healed by this character. A row is due only if its `seen` is after
  its `healed`.
- `cured: BTreeMap<String, Instant>`, likewise for cure.

`attempt(now, clock, health, own_percent, cfg)` returns a
`CastAttempt`:

1. Nothing while a cast of this state or of the self heal is out, or
   inside the round after either's last cast.
2. The own row first: poisoned self with cure affordable is
   `cast cure`.
3. A poisoned, uncured, non-resisting row with cure affordable is
   `cast cure <name>`.
4. Due rows are those under `cfg.minor_heal_at_percent`, not resisting,
   with `seen` after `healed`. The count includes the character itself
   when its own percent is under the mark. Two or more with an
   affordable area is the area's command.
5. Otherwise the lowest due row gets `heal_need(cfg, hp)`'s kind, the
   major falling back to the minor when the pool cannot afford it,
   as `cast <short> <name>`.
6. Otherwise nothing.

`on_event` folds the pool from prompts and reads only lines the
correlator attributes to the pending cast, through one shared
`sheet::cast_outcome(line) -> Option<Outcome>` extracted from
`HealState::on_event` and used by both:

- `Outcome::Unknown` (`do not know how to cast`) retires the source.
- `Outcome::Cast` (`you cast `) clears pending and marks the targets
  healed or cured at now. A single cast marks its one name, an area
  cast marks every row that was counted, a self cure marks nothing.
- `Outcome::Failed` (the fizzle, pool and round lines) clears pending.

`new_visit` clears pending as `HealState` does.

In `assist_tick`, after `assist_heal` and `assist_buff` and before the
bot's own actions:
when `[party].heal` is on and `session.party().role != Role::None`,
`casts.party.attempt(...)` may send one command, registered with
`on_sent` and the rest watch like every other cast. The state survives
a rebuild through `carry_party`, since it carries the healed marks and
a cast in flight.

## Settings

- `[party].poll_secs`, `u64`, default 20. 0 turns the poll off.
- `[party].heal`, `bool`, default true. Off, the character neither
  answers requests nor heals off the roster. It still asks and still
  introduces itself.

Both reload live like the other party settings.

## Tests

Unit, in `tests/party.rs` and `tests/sheet.rs`, no board:

- A row with a pool, a row without, an invited row, a witchunter row.
- `@heal 35`, `@heal`, `@cure`, `@iam Human Witchunter` in both the
  telepath and the said shape; the own echo is not a request; a
  stranger's word is not permitted.
- The health table: a roster sets rows and drops the departed, a
  request overwrites one number and its `seen`, a roster after `@cure`
  clears it, `Ended` clears all.
- `AskState` says once per round and not while over the mark.
- `PartyHeal`: cure before heal, the witchunter is skipped in the count
  and as a target, two under the mark with rain is rain, one is the
  single by kind, the major falls back to the minor on the pool, a
  cast marks the targets and the next attempt is quiet until a fresher
  row, an unknown spell retires the source.
- `cast_outcome` on the three line families.

Window, one test per binary as the wait tests are:

- A healer that hears `Celery says "@heal 30"` sends
  `cast mahe celery` once, and again only after a fresh request.
- A member with no heal spell under its mark says `@heal 30` once per
  round, and stops once over the mark.
- A member says `@iam` on following, and again when the roster names
  a newcomer.
- `party` goes out on its interval and the roster's numbers reach
  `party_health()` without a party note.

## Out of this cut

- A leader running a job does not tick the assist, so it neither asks
  nor answers. The followers are the healers today.
- The status bar and the lobby draw nothing of the table.
- No rule reads race yet.
- Mana for others: no member asks for mana and nothing gives it.

## Wordings assumed until seen live

- A follower's `party` reply lists everyone. Only the leader's has
  been captured.
- `You cast major healing on Celery!` for a targeted heal. The spec
  shows `You cast blur on Vexil!` for a targeted buff.
- `You cast healing rain on the room!` for the area heal. The spec
  shows `casts stinking cloud on the room!` in the third person.
- The roster's class word for a witchunter is `Witchunter`.
- `Celery says "@heal 35"` is captured for a plain say; the `@` is
  assumed to pass through unchanged.
