# Prompt status and recovery — Design

**Status:** accepted 2026-09-04. Supersedes the "no departure gate" rule for
`/go` in `docs/mud-client.md`.

## Goal

The client reads the game prompt with one regex that knows three shapes. The
board has more. While the character rests the prompt reads
`[HP=42 (Resting) ]:`, the regex misses it, and every consumer of prompts goes
quiet at once: the correlator never accepts the echo behind it, the room block
that follows is unattributed, HP stops updating, and a `/farm` started while
resting dies waiting for a look the board answered.

Replace the regex with a reader that knows the board's two prompt templates,
carry the status the prompt paints, track it at all times, and build the
recovery behaviour on top of it: rest and meditate to a mark, heal by spell at
two marks in combat, hide when idle, and a tick clock inferred from the board's
own regen and combat cadence, shown in the status bar.

## Origin

Daniel, 2026-09-04, after `/farm` timed out on a look while resting:
*"(Resting) is a status, like (Meditating) and others, we need to track that at
all times."* Then: *"a simple regex might not cut it."* On the behaviour:
*"stop resting if we're above the rest max threshold. When we stop resting, we
either need to resume what we were doing (go or farm?) or if we were idle and
our race can sneak, we need to hide until we're hidden."* Same for meditating.
*"The default rest/meditate until should be 95."* *"There are minor healing
spells and major healing spells, so minor healing and major healing are
separate values. While in combat, all of this is done in a loop. IE attack, if
minor or major heal are needed, cast a spell and then resume attacking. Don't
break combat to run until you're below the run threshold."* On timing: *"you're
gonna need to extract the tick detection algorithm from mudplay because
majormud doesn't tell you when it ticks. And you don't know the round timing
until something happens, IE you gain mana or health, or an attack round goes
by. We also need to track this info in the status bar."* And one more: *"XP/MIN
should be changed to XP/HR."*

## What the board sends

Everything below is read from `WCCMMUD.DLL` in the bbs backup, or measured in
captures on disk. None of it is in `re/docs`, whose `regeneration.md` claims
there is no player rest mode. That claim is wrong and will be corrected.

**Two prompt templates**, and they put the status in different places:

```
[HP=%s%d%s%s]:             HP only    ->  [HP=42 (Resting) ]:
[HP=%s%d%s/%s=%s%d%s]:%s   with pool  ->  [HP=36/MA=12]: (Resting) look
```

The `%s` slots are colour codes except the last one, which is the status,
` (Resting) ` or ` (Meditating) ` with the spaces. Those are the only two status
strings in the DLL. Both shapes appear live: 44 of 298 prompts in `test.raw`
carry the HP-only form, and `accept-run2.raw` carries the pool form glued to
the echo of `look`. The second form is why older code met `(Resting) look` as
an echo and once saw the marker glued onto a monster's name.

A third template, `{Energy=%d Snk JstEnt Hdn }:`, sits beside them in the DLL.
No capture has ever shown it. It is not modelled.

**No wording ends a rest.** The DLL has `You are now resting.` and `You are now
meditating.` and nothing for stopping. The prompt status is the only signal
that a rest or a meditation is still on. The board breaks a rest on most
commands. Measured live: `look`, `exp`, `health` and `help` do not break it.
`hide` and any move do. MudPlay never sends a command to end a rest, since
there is no stand command. The character stands on its next action.

**Hide is silent on success.** The DLL has `Attempting to hide...`, then
` You don't think you are hidden.` on a noticed failure and two refusals for
paralysis and stun. There is no success wording. Measured live 2026-09-04:

```
[HP=47 (Resting) ]:hide
Attempting to hide...
[HP=47]:
```

That was a successful hide, and it ended the rest.

**Regen ticks are not announced.** MudPlay infers them. Its measured cadence
on the stock board: passive HP and mana share one 30 second pulse, resting HP
ticks every 20 seconds, meditating mana every 15 seconds. The combat round is
5 seconds, and the client already measures it at 5.13. A spell cast on
yourself breaks a rest or a meditation. Poison refuses a rest.

**The `Health:` line carries max mana.** `Health:    42/47    [89%]` gains
`  Mana:  12/12  [100%]` or `Kai:` whenever a pool exists. The max HP probe
already reads this line.

## Part 1: prompt status

### Reading a prompt

`parse.rs` replaces `PROMPT_RE` with a prompt reader that runs on the
ANSI-stripped text and knows the two templates. It finds `[HP=`, reads the HP
as a signed integer, reads the optional `/MA=` or `/KAI=` pool, reads an
optional ` (Word) ` before `]:`, and after `]:` reads an optional ` (Word) `
only when the prompt had a pool. What follows on the physical line keeps its
current meaning: the echo, an async line, or the next prompt in a pileup. The
end of buffer rule accepts the same shapes with the trailing status, so a
dangling resting prompt still emits at once.

The status is a closed type:

```rust
pub enum Status {
    Resting,
    Meditating,
    /// A word the board painted that this client has never seen.
    Other(String),
}
```

An unknown word must never again hide a prompt. It flows through, shows in the
status bar, and is noticed the day it appears.

One known hole. The board writes a pool prompt and its status in one call, but
a chunk boundary can land between `]:` and ` (Resting) `. The prompt then
emits bare and the status word reaches the correlator as a decorated echo,
which `strip_decoration` already handles. The status is missing for that one
prompt and back on the next. Every rule in Part 3 is level triggered on the
status together with the vitals, never on a status vanishing, so a blink can
fire nothing. The tick clock does restart its rest cycle on one, which costs
that cycle one credited tick and nothing else.

### Carrying it

- `Event::Prompt` gains `status: Option<Status>`. Every construction site
  names it. About sixty lines, most in tests, and the compiler proves none was
  missed.
- `GameState` gains `status: Option<Status>`, set on every prompt. A change of
  status is a state change for anyone watching.
- `BotConfig` gains `max_mana`, probed beside `max_hp` from the `Health:`
  line. `discover_max_hp` becomes `discover_vitals` and returns both. Zero
  means no pool.
- The correlator keeps `strip_decoration` as defence. The "prompt pileup
  residue" case in its tests stops arising.

### Showing it

The status line shows the word after the vitals: `HP 42 (Resting)`.

## Part 2: the tick clock

Ported from MudPlay's `TickEngine`, `RegenTracker`, `RegenCycle` and
`RegenStat`, into `world.rs` beside the existing `RoundClock`.

### Cycles

| cycle | period | anchored by |
|---|---|---|
| combat round | 5 s | any hit or miss line, debounced 250 ms so one volley anchors once |
| HP natural | 30 s | HP rising between prompts |
| HP rest | 20 s | HP rising while the status is Resting |
| mana natural | 30 s | mana rising, and the HP natural pulse, which it shares |
| mana meditate | 15 s | mana rising while the status is Meditating |

Each cycle is a phase anchor and a period. A cycle with no anchor reports
nothing. There is no guess before the first observation.

### Rules

1. A pool rising between two prompts is a tick, unless a heal-shaped command
   went out in the last 3 seconds. Heal-shaped means any cast.
2. The gain credits whichever active cycle is due, meaning within 750 ms of a
   period since its anchor. If both the rest and the natural cycle are due,
   both take it. If neither is due the natural cycle anchors on it.
3. The rest and meditate cycles start when the status says so and stop when
   it stops saying so. No partial credit on stop.
4. An anchor rolls forward in exact period steps when asked for the next tick,
   so a silent tick at full HP keeps phase.
5. Every real observation re-anchors the cycle at that instant.
6. The amount per tick is a running average with weight 0.2 on the newest
   sample. A sample whose interval does not look like one to three periods is
   dropped.
7. The combat round re-anchors on every volley. Casting holds until the next
   round after the last cast, as `RoundClock` does today, and an observed
   round clears the hold.

The existing `RoundClock` keeps its 5.13 second period and its observe and
next_round_after shape. The port adds the regen cycles beside it and one
`TickClock` that owns all five, fed from the session's correlated events.

### Showing it

The status line shows the countdowns: `Tick 3.2 | HP 12.3/4.5 | MA 21.0/8.1`.
The second number in a pair is the rest or meditate cycle, shown only while it
runs. A cycle with no anchor shows `-`. The play loop repaints on events today,
so it gains a 250 ms repaint timer.

## Part 3: recovery

### The marks

All in `[bot]`, all percentages of the probed maxima, 0 means off. The loader
keeps refusing marks out of order. Defaults follow MudPlay's health settings.

| key | default | meaning |
|---|---|---|
| `rest_at_percent` | 60 | exists, was 50. Below this HP mark, out of combat, rest. |
| `mana_rest_at_percent` | 30 | new. Below this mana mark, out of combat, rest or meditate. |
| `rest_until_percent` | 95 | new. Recovery is over when HP is at or above it and, with a pool, mana too. |
| `meditate` | false | new. A mana-only recovery sends `meditate` instead of `rest`. |
| `minor_heal_at_percent` | 70 | replaces `spell_at_percent`, kept as an alias. Was 0, off. Below it, cast the minor heal. |
| `major_heal_at_percent` | 40 | new. Below it, cast the major heal. |
| `flee_at_percent` | 20 | exists, was 25. Leave a fight only below this. |
| `minor_heal_spell` | discovered | replaces `heal_spells`. Empty means the cheapest heal in the book. |
| `major_heal_spell` | discovered | empty means the dearest heal in the book. |
| `hp_regen_spell` | none | a regen over time spell, named by the player. |
| `max_mana` | probed | beside `max_hp`. |

`[farm].depart_at_percent` stays accepted. A profile that sets it and not
`rest_until_percent` gets it as the rest mark, with the rename notice the
loader already prints for the two older keys.

The defaults are MudPlay's, from its `HealthSettings`: `RestIfBelowHp` 60,
`RestIfBelowMa` 30, `RestMaxHp` and `RestMaxMa` 95, `MinorHealCombatTrigger`
70, `MajorHealCombatTrigger` 40, `RunIfBelowHp` 20, `UseMeditateAbility` off.
Two of them move existing client defaults, and one changes a policy: spell
healing was opt in so that upgrading never spent a profile's mana unasked. With
70 as the default, a profile whose spellbook holds a heal starts casting it
below 70 after upgrading. The loader says so once when a profile has heals in
the book and no heal mark of its own.

Meditate is a quest ability. The client cannot tell whether the character has
it, so `meditate` is a switch the player sets. The runner says so once and
ignores it on a character whose probe found no pool.

### Which recovery to send

Out of combat, with the room clear, on every prompt:

- HP below `rest_at_percent`: send `rest`. Rest restores both pools.
- HP fine, mana below `mana_rest_at_percent`: send `meditate` when the
  `meditate` switch is on, else `rest`.
- Both need recovery: send `rest`.

This is MudPlay's `ChooseRestCommand` without its `MeditateBeforeResting`
knob, which nobody has asked for.

A recovery is over when the status is Resting or Meditating and the pools are
at or above `rest_until_percent`, HP and mana both for a rest, mana for a
meditation. Being over means the recovery gate clears and the bot is free to
act. No command is sent to end it. The board stands the character on its next
action.

### The combat loop

On every prompt in a fight, in this order:

1. HP below `flee_at_percent`: flee, exactly as today. Nothing else leaves a
   fight.
2. A heal is due when HP is below a mark, the pool affords the spell, and no
   cast has gone out this round. Below `major_heal_at_percent` the major heal
   is due, falling back to the minor heal when there is no major or the pool
   does not afford it. Between the two marks, `hp_regen_spell` is due when it
   is named and not already running, else the minor heal. Above
   `minor_heal_at_percent` nothing is due.
3. A between-round cast makes the board print `*Combat Off*`. The bot's
   un-latch fires and the next room block or cooldown re-attacks. That is the
   attack, cast, resume loop.
4. Rest and meditate are never sent while the room has work, exactly as today.

The regen spell follows MudPlay's placement. It pays out a round later, so it
is kept out of the band below the major mark where an instant heal is wanted.
Once cast it is not recast until its duration in rounds has elapsed on the
tick clock, the same way buffs are tracked.

The heal marks also fire out of combat, as `spell_at_percent` does today: a
spell lands in one round and a rest tick takes twenty seconds. Rest follows
once nothing is due or affordable.

### Idle

When no job is running, only the assist acts, and only while `/bot` is on.
With `/bot` off the status is tracked and shown and left alone.

When a recovery is over and the sheet shows Stealth, the assist sends `hide`.
That ends the rest and hides in one command. It believes itself hidden unless
the board answers ` You don't think you are hidden.`, on which it sends `hide`
again, three tries at most. The belief clears when the bot sends anything
else, since nearly every command breaks hide. Without Stealth the character
stays rested until something else moves it.

Hide is not suppressed for a party. The client has no party model, and a
hidden member cannot be targeted by party heals. That is the first thing to
add when one arrives.

### The gates

The farm's departure gate keeps its shape and sends `rest` or `meditate` by
the rule above. It watches the game state, which is live during a rest now
that prompts parse, and leaves when both pools are at or above the mark. The
first step ends the recovery. `max_rest_seconds` stays as the cap.

`/go` gets the same gate. `go_config` stops zeroing the mark. A profile that
wants the old behaviour sets `rest_until_percent = 0`. The docs lose the "no
departure gate" difference.

## Part 4: XP per hour

The experience rate is computed per minute in `progress.rs`, shown as `xp/min`
in the status line, used by the level ETA label, and printed by the run mode
of the `mmc` binary. All four become per hour and the label becomes `xp/hr`.

## Testing

- **Parser.** Every prompt shape in the captures: both templates, pileups,
  dangling prompts, negative HP, a status with a pool, and an unknown word.
- **Correlator.** The exact resting look from `test.raw` at
  `1788503928.333`, asserting the room block is credited to the look.
- **Session.** The state follows the status, and the vitals probe fills both
  maxima.
- **Tick clock.** Recorded prompt sequences from the captures, one per cycle,
  plus the artifact window, the silent tick, and the dropped sample.
- **Bot.** The end of a rest and of a meditation, the meditate start rule, the
  heal order at every band with and without a regen spell, the one cast per
  round hold, the hide retry, and no action without Stealth.
- **Gates.** Farm and go both depart on both pools, and go honours a zero
  mark.
- **Replay.** All of `test.raw` through the replay examples, confirming the
  look at the failure point attributes.
- **Mutation.** Each guard dropped in turn to prove its test is the one that
  fails.

The engine in `mud-core` has no rest mode and is out of scope.

## Docs

`docs/mud-client.md`: the `[bot]` table and the recovery ladder, the
departure gate paragraph, the go differences list, the status line. The
prompt shapes in `docs/board-correlation.md`. The wrong claim in
`re/docs/regeneration.md`, corrected locally.

## Not in scope

- The `{Energy=...}:` prompt template.
- Party awareness.
- Poison. A rest refused by poison never shows Resting, so the gate waits out
  `max_rest_seconds` as it does today.
- Casting a self buff during a rest breaks it. The buff machine stays as it
  is, and the rest re-fires on the next prompt.

## Phases

Each phase is committed on its own and leaves the suite green.

1. **Prompt status.** Part 1, with the correlator test. This fixes the
   reported bug.
2. **Tick clock.** Part 2 and the status bar countdowns.
3. **Recovery.** Part 3: marks, combat loop, regen spell, idle hide, both
   gates.
4. **XP per hour.** Part 4.
