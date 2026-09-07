# Spell discovery by ability, and a stealth buff wherever a sneak is armed (2026-09-07)

Base: `main` after the settings spec, `2026-09-07-settings-live-reload-design.md`.
Client only. Second of three specs. The recovery spec builds on this one.

The client discovers a light spell today by matching the character's spellbook against
a list of three names in `sheet.rs`. Two of those names, "light" and "continual light",
are not in the shipped spell table. Only "starlight" matches. Nothing discovers a
stealth spell, and the character sneaks without one even when it knows camouflage.

## Evidence

Verified against `re/mmud_wgnt.sqlite` through `mud_core::content_db::load`, which
decodes each spell's ability list.

| spell | short | mana | stealth ability | target mode |
| --- | --- | --- | --- | --- |
| camouflage | camo | 10 | value 0 | 1, self |
| way of the cat | cat | 3 | value 0 | 1, self |
| shadowform | shad | 8 | value 0 | 1, self |
| glitterdust | glit | 6 | value 0 | 0, a monster |
| cross of vengeance | cross | 15 | value -15 | 1, self |
| stealth trap | none | 0 | value -200 | 11 |
| starlight | star | 4 | room illumination, value 0 | 1, self |
| bless | bles | 4 | none | 2, another player |

The ability value is level scaled in the table and reads as zero on every self buff.
Presence decides, not amount. A negative value is a penalty.

The target mode in the table above is the sqlite `target` column. The decoder puts that
column into `Spell::match_type`, whose variant for 1 is named `MatchType::Single1`. The
field named `Spell::target_mode` holds the `spelltype` column instead, which is 3 on
every spell. Both variant names are placeholders from the first decode. This spec does
not rename them. The rule below matches `match_type == MatchType::Single1` and says
"target mode 1" for it.

## 1. The discovery rule

A spell is a **stealth spell** when its record carries the `Stealth` ability with a
value that is not negative and its target mode is 1.

A spell is a **light spell** when its record carries the `RoomIllu` ability and its
target mode is 1.

Discovery compares the character's spellbook, parsed from the board's `spells` reply,
against the content spells by name. The spellbook gives the short name and the mana
cost. The content record gives the abilities, the target mode and the duration.

The three-name list `LIGHT_SPELLS` is deleted. A test pins that starlight is still
found.

### Where the spells come from

`farm::content_for` already loads the content database into the session for the pack
and the backstab. The sheet functions that discover spells take the content's spell
map. `sheet::light_sources` gains a spell map parameter. A new
`sheet::stealth_spells(spellbook, spells, durations, casting) -> (Vec<Buff>, Vec<String>)`
returns the stealth spells the character knows as buffs, with the shipped duration in
rounds, and one refusal line per stealth spell whose duration is zero.

Durations already come from `RoomGraph::load_spell_durations`, keyed by name.

### What the sheet reports

Beside the lines it prints for heals and light at job start, the sheet prints one line
naming the stealth spells found, and one line for anything refused:

    stealth: camouflage (10 mana, 30 rounds)
    stealth: none known

A character with no spellbook prints nothing new.

## 2. The stealth buff in the navigator

The sneak is armed in one place, `Navigator::arm_sneak`. That is where the buff goes,
so go, farm, roam, bank and recover all get it with one change.

### Construction

`Navigator::with_stealth(buffs: Vec<Buff>, clock: RoundClock)` stores a `BuffState`
built from the discovered stealth spells. A navigator built without it arms the sneak
as today.

### Arming

`arm_sneak` does, in order:

1. If `NavConfig::sneak` is off or the sheet's stealth is zero, return `Ok(false)`
   without sending anything. No spell is cast for a sneak that will not be armed.
2. Ask the buff state for a lapsed, affordable stealth spell. If there is one, send its
   cast and wait for the outcome the way `farm_stop` waits for a buff: success marks it
   cast, a fizzle or a refusal marks the attempt and moves on. Repeat until nothing is
   wanted. A cast breaks a sneak, which is why every cast comes before the `sneak`.
3. Send `sneak` and resolve it as today.

A cast that fails for mana, a fizzle or a refusal does not stop the walk. The sneak is
armed without the buff and the walk goes on.

### Re-arming mid walk

`goto` re-arms through the same function after a break, so a spell that lapsed during
the walk is recast at that step. A spell that lapses while the sneak still holds is
not recast until the next arming. Recasting would break the sneak to restore a bonus
the sneak is already surviving without.

### Mana

The buff state reads mana from the prompt, as it does for a farm's buffs. The
navigator feeds it every prompt it sees while waiting on a step, so the affordability
check is current.

## 3. `bot.sneak` off

With `bot.sneak` false the navigator arms nothing and casts nothing. This is the first
step of arming and needs no other gate.

## Tests

Discovery, over a hand-built spell map:

- camouflage, way of the cat and shadowform are found as stealth spells.
- glitterdust is excluded by its target mode.
- cross of vengeance is excluded by its negative value.
- starlight is found as a light spell.
- A spell the book does not know is absent, not an error.
- A stealth spell with a zero duration is refused with a line naming it.

Navigator, on the scripted sneak board in `tests/sneak.rs`:

- A stealthy character with camouflage sends `cast camo` and then `sneak` before the
  first move, in that order.
- The same walk with `bot.sneak` off sends neither.
- A walk that breaks on the second step and whose spell has lapsed recasts before
  re-arming.
- A walk whose spell has not lapsed does not recast on re-arm.
- A cast that fizzles is followed by `sneak` anyway and the walk arrives.
- A character with no stealth spell sends `sneak` and nothing else, as today.

Sheet:

- The report line names the stealth spell and its cost.
- A character with no stealth spell prints the "none known" line.

## Out of scope

- Renaming `TargetMode`'s variants. A separate change against the decoder, with its
  own evidence.
- Buffs other than stealth in the navigator. A farm's buffs stay at the stop.
- Casting a stealth spell for the assist's own `hide`.

