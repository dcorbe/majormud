# Banking: auto deposits from a farm, and `/bank` in play

**Status:** accepted 2026-09-06
**Scope:** `crates/mud-client`. `purse.rs`, `bot.rs`, `farm.rs`, `go.rs`,
`profile.rs`, `correlate.rs`, a new `bank.rs`, `docs/mud-client.md`.

## The problem this solves

A farm run picks up every coin pile it kills over and never puts any of it
down. Coins weigh one third of a unit each, so a long run drifts the
character from the None weight class into Light and then Medium, and the
low-encumbrance combat bonus goes with it. A death then loses the lot.

Mudplay and MegaMud both detour to a bank when a wealth or coin-count gate
trips. This client has the pieces already: the purse meter reads the
inventory reply, the pack is refreshed from it, the navigator walks to any
room, and the farm loop has a clean seam between a stop ending and the next
leg starting. What is missing is the gate, the bank list, the errand, and
the config.

A second, smaller gap: the sweep takes every pile it sees. At higher levels
a copper pile is not worth the send, and on a shared board it is not worth
the contest.

## Decisions

These were settled in the design conversation and are not open.

- **The coin gate is a raw count.** Every coin of every denomination counts
  one. This is the axis that drives weight, so both triggers speak the same
  language. Default 1000.
- **The weight gate fires on a crossing.** It fires when the coins picked up
  since the last reading lifted the class one step: None to Light, Light to
  Medium, Medium to Heavy. It does not fire on a class that was already
  raised by earlier coins.
- **A keep floor, default 0.** The deposit leaves `keep_gold` in the purse.
  A toll on the circuit is the operator's reason to set it.
- **Tolls are routed around, not paid on credit.** The router already prices
  a toll exit against the purse and treats one the character cannot pay as
  a wall. The errand re-reads inventory after the deposit, so every route
  planned afterwards sees the post-deposit purse.
- **Nearest bank by default.** A configured bank room overrides it.
- **Automatic deposits run inside a farm only.** In play the operator is
  driving, and a detour would take the keyboard. `/bank` does the errand on
  demand there.

## The banks

Bank rooms are the shop-active rooms whose shop has type 7. The shipped
database has five:

| room | name | shop |
|---|---|---|
| 1/297 | Bank of Godfrey | 8 |
| 6/1334 | Bank of Khazarad | 8 |
| 2/2568 | Bank of Rhudaur | 83 |
| 16/320 | Bank | 116 |
| 17/2435 | Bank of Arlysia | 156 |

Godfrey and Khazarad share shop 8, so they share one account. The other
three are separate accounts. Depositing at the nearest bank can therefore
spread money over four accounts. A later withdrawal has to know which one
holds it. This spec does not add withdrawals.

The Bank of Rhudaur sits behind a 5 gold toll. Temple Street, Northern End
at 2/2528 charges 5 gold on its east exit into the bank. The bank's only
exit is west, back to Temple Street, and it is free. So the toll is paid on
the way in, when the character is carrying the coin it came to deposit, and
never on the way out. The nearest-bank search routes under the session's
capabilities, so a bank the purse cannot reach is not a candidate.

## Config

A new `[bank]` table in the profile. Absent means these defaults.

```toml
[bank]
auto_deposit = true
deposit_at_coins = 1000
deposit_on_weight_class = true
keep_gold = 0
at = "1/297"
```

- `auto_deposit` turns the farm gate on and off. `/bank` works either way.
- `deposit_at_coins` is the raw count. 0 disables the count gate.
- `deposit_on_weight_class` is the crossing gate.
- `keep_gold` is what the deposit leaves in the purse, in gold crowns.
- `at` is a fixed bank as `map/room`. Unset means the nearest. A room that
  is not a bank room is refused at profile load, with the five listed.

In `[bot]`, one new key:

```toml
ignore_coins = ["copper"]
```

Denominations the sweep leaves on the floor, named as `get` takes them:
`copper`, `silver`, `gold`, `platinum`, `runic`. Default empty. Any other
word is refused at profile load. Both sweep sites honour it: the kill's drop
line and the room render's "You notice" listing. An ignored pile is skipped
before the per-visit memo, so it is never claimed.

## Reading the coins

The purse is one number in farthings and stays that way. The gate needs the
five counts, because both the count and the weight depend on them.

`purse.rs` gains a `Coins` value holding the five counts. `leading_coins`
returns `Coins` and the purse is derived from it, so there is one parser
with two views. The inventory reading the session keeps gains the `Coins`
read off its carrying line, next to the encumbrance pair it already keeps.

From `Coins`:

- `count()` is the sum of the five.
- `weight()` is the sum of each count divided by 3, the integer division
  mud-core applies per denomination in `carried_weight`.

The weight class is a function of the carried and capacity numbers on the
`Encumbrance:` line: None below 33 percent, Light below 66, Medium below
100, Heavy at or above. It is restated in the client next to the coin
ratios, with a comment naming `text::encumbrance_descriptor` as the
authority.

## The gate

A `BankGate` in `bank.rs` holds the last reading it judged and the config.
It is fed a fresh inventory reading and answers `Deposit` or `Hold`.

It answers `Deposit` when either holds:

- the coin count is over `deposit_at_coins` and the purse is above the keep
  floor, so a deposit can actually lower the count. Without the second
  clause a floor above the mark would send the character to the bank after
  every stop.
- `deposit_on_weight_class` is on and the class of this reading is one or
  more steps above the class of the previous reading.

The previous reading is replaced by the new one on every judgement, and by
the reading taken after a deposit.

The gate is a pure function of two readings and the config, tested without
a session.

## When the gate is judged

The bot already re-reads inventory after a key pickup. Coins are not
re-read per pile. Instead the farm runner reads inventory once at the end
of any stop where the bot confirmed at least one coin pickup, and judges
the gate on that reading. A deposit can only happen when the stop ends, so
this decides the same outcomes as a per-pile check at one send per stop
instead of one per pile.

The stop pump counts confirmed coin pickups off the line events it already
feeds to the room model, using the `picked_up` parse that exists for it. The
count lives in the pump, not the bot: a flee recovery rebuilds the bot
mid-stop and would lose a counter kept there.

The seam is in `run_farm`'s lap, after `farm_stop` returns `Dwelt` and
before the next `travel`. A roam uses the same lap, so it gets the same
gate.

## The errand

`bank.rs` owns the errand. It takes the session, the navigator, the graph,
the content, the bank config, and the current room, and returns where the
character ended up and what happened.

1. **Choose the bank.** The configured `at` room, or the nearest bank room
   by route cost from the current room under the session's capabilities.
   No reachable bank logs a warning once per run and the errand returns
   without moving. The run carries on, the way a roam drops a stop it
   cannot open.
2. **Walk there.** Through `travel`, so fights on the way, interrupts, the
   time budget and desync recovery come free. The character can die or end
   the run too hurt on the way, and those end the run as they would on a
   leg. A leg that arrives is a leg that stood in the bank room.
3. **Read the purse.** Send `i` and wait for the reply, the way
   `probe_sheet` does. The deposit is computed from this reading, never
   from the one the gate judged: a toll on the way changed the purse and
   nothing observes tolls.
4. **Deposit.** `deposit <farthings>` for the purse minus the keep floor,
   when that is positive. Wait for the reply.
5. **Read again.** Send `i` again so the purse meter and the pack reflect
   the deposit before any route is planned from the bank.

The lap then continues with `current` set to the bank. The next leg walks
from the bank to the next stop. There is no walk back to where the errand
began.

A refusal at the bank ends the errand without a deposit. The two known
refusals are "You cannot DEPOSIT if you are not in a bank!" and "Please
specify a more reasonable amount." The first means the walk did not land
where the graph said, and counts as a desync for the run's report. Either
way the run continues.

A new `Phase::Banking { at }` shows on the status bar for the whole errand.
The run stats gain a deposit count and total, printed with the rest at the
end.

## The correlator

`correlate.rs` gains a `Deposit` kind for `deposit` and `dep`. It completes
on "You deposit", on the not-in-a-bank refusal and on the unreasonable
amount refusal. Without it the reply sits opaque and the wait burns its
deadline, the failure the locked door once had.

## `/bank` in play

`/bank` runs the errand under the `/go` job slot, from wherever the
character stands, with the profile's bank config. It reports the amount
deposited and the bank's name, or why nothing was deposited. Ctrl-F takes
the keyboard back, as it does from `/go`. It does not consult the gate: the
operator asked.

## Unverified wording

Three lines come from the stock oracle, not from the board this client
plays:

- "You deposit 10 silver nobles." from `oracle_bank3.raw`.
- The inventory reply's "You are carrying" wrapper, already flagged in
  `purse.rs`.
- "You picked up 11 silver nobles" from `oracle_bank.raw`.

Each stays flagged in the doc and in the code, the way the purse flags its
wrapper today. The first live `/bank` is the capture that settles them.

## Testing

Unit, each with a mutation check that the test fails when the rule is
removed:

- `Coins` parsing, `count()` and `weight()`, including the per-denomination
  integer division.
- The class boundaries at 32, 33, 65, 66, 99 and 100 percent.
- The gate: count over the mark with and without purse above the keep
  floor, a crossing from None to Light, a class already Light that does
  not fire, a two-step crossing, both gates off.
- Nearest bank over a small graph with content: the nearest wins, a bank
  behind an unaffordable toll is skipped, no bank reachable returns none,
  a configured `at` wins over a nearer bank.
- `ignore_coins` at both sweep sites: an ignored pile is not fetched and
  does not claim the memo, an unlisted pile still is.
- The correlator completes `deposit` on all three replies.
- Profile validation refuses an unknown denomination and a non-bank `at`.

Scripted, in the pattern of `tests/farm_scripted.rs`: a stop that picks up
a pile crossing the mark, then a walk to the bank, `i`, `deposit`, `i`, and
the next leg starting from the bank.

## Out of scope

Withdrawals, a balance query, stash rooms, and a return walk to where the
errand began. None of them is needed for the run to keep its coin safe.
