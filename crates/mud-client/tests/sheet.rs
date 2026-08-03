//! Inventory and spellbook parsing.
//!
//! Both formats are taken from real board output, not invented:
//!
//! - inventory, from `re/oracle/oracle_bank.raw` — note it WRAPS at the
//!   terminal width, mid-item, so "quarterstaff (Two handed)" arrives
//!   split across two lines.
//! - the spellbook, from `mud_core::text::SPELLS_HEADER` and
//!   `spell_row`, both marked VERIFIED against `oracle_spell_train.raw`:
//!   level right-aligned in 3, mana in 4, four spaces, short name in 6,
//!   spell name in 30.

use mud_client::sheet::{Casting, Inventory, LightSource, Spellbook};

#[test]
fn inventory_reads_the_carried_list() {
    let inv = Inventory::parse(
        "You are carrying 11 silver nobles, 49 copper farthings, quarterstaff\n\
         You have no keys.\n\
         Wealth: 159 copper farthings\n\
         Encumbrance: 119/2880 - None [4%]\n",
    );
    assert_eq!(
        inv.items,
        vec![
            "11 silver nobles".to_string(),
            "49 copper farthings".to_string(),
            "quarterstaff".to_string(),
        ]
    );
    assert_eq!(inv.encumbrance, Some((119, 2880)));
}

/// The board wraps the carried list at the terminal width, mid-item.
/// Splitting on lines rather than rejoining first would invent an item
/// called "handed)".
#[test]
fn inventory_rejoins_a_wrapped_item() {
    let inv = Inventory::parse(
        "You are carrying 49 copper farthings, quarterstaff (Two\n\
         handed)\n\
         You have no keys.\n\
         Wealth: 159 copper farthings\n",
    );
    assert_eq!(
        inv.items,
        vec![
            "49 copper farthings".to_string(),
            "quarterstaff (Two handed)".to_string(),
        ]
    );
}

#[test]
fn an_empty_pack_is_not_an_error() {
    let inv = Inventory::parse("You are carrying nothing.\nYou have no keys.\n");
    assert!(inv.items.is_empty());
}

/// The reason any of this exists: deciding whether darkness can be dealt
/// with. Matching is on the trailing noun so a "battered torch" still
/// counts, and it must not fire on a torch-shaped word like "torchbug".
#[test]
fn inventory_finds_a_light_source() {
    let empty_book = Spellbook::parse("You have no spells.\n");
    let first = |text: &str| {
        mud_client::sheet::light_sources(&Inventory::parse(text), &empty_book, Casting::Spells)
            .into_iter()
            .next()
    };
    assert_eq!(
        first("You are carrying a battered torch, 3 copper farthings\n"),
        Some(LightSource::Item {
            light_cmd: "light torch".into(),
            remove_cmd: "remove torch".into(),
        })
    );
    assert_eq!(
        first("You are carrying lantern\n"),
        Some(LightSource::Item {
            light_cmd: "light lantern".into(),
            remove_cmd: "remove lantern".into(),
        })
    );
    assert_eq!(first("You are carrying quarterstaff, 2 rations\n"), None);
    assert_eq!(
        first("You are carrying a torchbug in a jar\n"),
        None,
        "not a light source"
    );
}

#[test]
fn spellbook_reads_the_rows() {
    // Exactly the column layout of mud_core::text::spell_row.
    let book = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   2    star  starlight                     \n\
         \x20 3   6    heal  minor healing                 \n\n",
    );
    assert_eq!(book.spells.len(), 2);
    assert_eq!(book.spells[0].short, "star");
    assert_eq!(book.spells[0].name, "starlight");
    assert_eq!(book.spells[0].level, 1);
    assert_eq!(book.spells[0].mana, 2);
}

#[test]
fn an_empty_spellbook_is_not_an_error() {
    let book = Spellbook::parse("You have no spells.\n");
    assert!(book.spells.is_empty());
}

/// The caster's answer to a dark room. Cast by SHORT name, which is what
/// the board's cast command takes.
#[test]
fn spellbook_finds_a_light_spell() {
    let no_items = Inventory::parse("You are carrying nothing.\n");
    let book = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   2    star  starlight                     \n",
    );
    assert_eq!(
        mud_client::sheet::light_sources(&no_items, &book, Casting::Spells),
        vec![LightSource::Spell {
            cmd: "cast star".into(),
            mana_cost: 2,
        }]
    );

    let dark = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    lb    lightning bolt                \n",
    );
    assert!(
        mud_client::sheet::light_sources(&no_items, &dark, Casting::Spells).is_empty(),
        "lightning bolt is not a light spell"
    );
}

// --- against real board output ----------------------------------------
//
// Captured from Salad on 2026-07-30. The command is `inventory`; `inv`
// is NOT recognised and gets said out loud, which is how the board
// treats anything it does not know.

const REAL_INVENTORY: &str = "\
You are carrying 21 silver nobles, 56 copper farthings, padded pants (Legs),
padded vest (Torso), padded helm (Head), padded gloves (Hands), padded boots
(Feet), quarterstaff (Two handed)
You have no keys.
Wealth: 266 copper farthings
Encumbrance: 525/2400 - Light [21%]
";

const REAL_SPELLBOOK: &str = "\
You have the following spells:
Level Mana Short Spell Name
  1   4    star  starlight                     
  1   1    vine  vine strike                   
";

#[test]
fn the_real_inventory_parses() {
    let inv = Inventory::parse(REAL_INVENTORY);
    assert_eq!(
        inv.items,
        vec![
            "21 silver nobles",
            "56 copper farthings",
            "padded pants (Legs)",
            "padded vest (Torso)",
            "padded helm (Head)",
            "padded gloves (Hands)",
            // Wrapped across two lines mid-item, rejoined.
            "padded boots (Feet)",
            "quarterstaff (Two handed)",
        ]
    );
    assert_eq!(inv.encumbrance, Some((525, 2400)));
    // No torch and no lantern: the item route to solving darkness is not
    // available to this character, which is the answer the caller needs.
    assert!(
        mud_client::sheet::light_sources(&inv, &Spellbook::default(), Casting::Spells)
            .is_empty()
    );
}

#[test]
fn the_real_spellbook_parses_and_offers_a_light() {
    let book = Spellbook::parse(REAL_SPELLBOOK);
    assert_eq!(book.spells.len(), 2);
    assert_eq!(book.spells[0].name, "starlight");
    assert_eq!(book.spells[0].mana, 4);
    assert_eq!(book.spells[1].name, "vine strike");
    // The whole point: this character can light a dark room.
    assert_eq!(
        mud_client::sheet::light_sources(&Inventory::default(), &book, Casting::Spells),
        vec![LightSource::Spell {
            cmd: "cast star".into(),
            mana_cost: 4,
        }]
    );
}

// --- deciding how to light a room --------------------------------------
//
// Both verbs are DLL-confirmed: "Syntax: CAST {spell} [{target}]"
// (0xd984f) and the `light` verb at 0xdb07a, whose success is "You lit
// the %s." (0xdb52d).

#[test]
fn a_carried_light_is_preferred_over_a_spell() {
    // An item costs no mana and, once lit, keeps burning -- mana is
    // wanted for the fight the dark room is hiding. Items first, spell
    // last; the spell survives any number of burn-outs.
    let inv = Inventory::parse("You are carrying a battered torch\n");
    let book = Spellbook::parse(
        "You have the following spells:\nLevel Mana Short Spell Name\n  1   4    star  starlight\n",
    );
    assert_eq!(
        mud_client::sheet::light_sources(&inv, &book, Casting::Spells),
        vec![
            LightSource::Item {
                light_cmd: "light torch".into(),
                remove_cmd: "remove torch".into(),
            },
            LightSource::Spell {
                cmd: "cast star".into(),
                mana_cost: 4,
            },
        ]
    );
}

/// EVERY carried light item is a source — the single-Option shape was
/// exactly why one burn-out went dead-for-the-run while a second torch
/// sat in the pack (there are two on the Small Cavern floor alone).
#[test]
fn every_carried_light_item_is_a_source() {
    let inv = Inventory::parse("You are carrying a battered torch, brass lantern, torch\n");
    let book = Spellbook::parse("You have no spells.\n");
    let sources = mud_client::sheet::light_sources(&inv, &book, Casting::Spells);
    assert_eq!(sources.len(), 3, "{sources:?}");
    assert!(matches!(&sources[0], LightSource::Item { light_cmd, .. } if light_cmd == "light torch"));
    assert!(
        matches!(&sources[1], LightSource::Item { light_cmd, .. } if light_cmd == "light lantern")
    );
}

#[test]
fn a_caster_with_no_torch_casts() {
    let inv = Inventory::parse("You are carrying quarterstaff\n");
    let book = Spellbook::parse(
        "You have the following spells:\nLevel Mana Short Spell Name\n  1   4    star  starlight\n",
    );
    assert_eq!(
        mud_client::sheet::light_sources(&inv, &book, Casting::Spells),
        vec![LightSource::Spell {
            cmd: "cast star".into(),
            mana_cost: 4,
        }]
    );
}

/// Neither: say so rather than send something that will be spoken aloud.
#[test]
fn with_neither_there_are_no_sources() {
    let inv = Inventory::parse("You are carrying quarterstaff\n");
    let book = Spellbook::parse("You have no spells.\n");
    assert!(mud_client::sheet::light_sources(&inv, &book, Casting::Spells).is_empty());
}

/// Salad's real kit: no torch, but starlight in the book — one Spell
/// source whose mana cost comes from the book, which is the mana floor
/// the caster respects (no config key involved).
#[test]
fn the_real_character_derives_a_single_spell_source() {
    assert_eq!(
        mud_client::sheet::light_sources(
            &Inventory::parse(REAL_INVENTORY),
            &Spellbook::parse(REAL_SPELLBOOK),
            Casting::Spells,
        ),
        vec![LightSource::Spell {
            cmd: "cast star".into(),
            mana_cost: 4,
        }]
    );
}

// --- healing spells ---------------------------------------------------

use mud_client::correlate::{CmdId, Correlated};
use mud_client::events::Event;
use mud_client::sheet::{CastAttempt, HealSource, HealState};
use mud_client::world::RoundClock;

/// A book with three heals at different prices, plus one spell that is
/// not a heal at all.
fn healer_book() -> Spellbook {
    Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   2    star  starlight                     \n\
         \x20 8   9    maj   major healing                 \n\
         \x20 1   3    heal  minor healing                 \n\
         \x20 2   5    mend  mend                          \n",
    )
}

/// Discovery, and the ordering that matters: mana is the scarce
/// resource, so the CHEAPEST heal comes first. Sizing the spell to the
/// wound is not attempted — the shipped min/max are level-1 figures and
/// the real heal scales with caster level, so any such table would lie
/// by more the longer a character had been played.
#[test]
fn heals_are_discovered_cheapest_first() {
    let heals = healer_book().heal_spells(&[], Casting::Spells);
    assert_eq!(
        heals,
        vec![
            HealSource { name: "minor healing".into(), cmd: "cast heal".into(), mana_cost: 3 },
            HealSource { name: "mend".into(), cmd: "cast mend".into(), mana_cost: 5 },
            HealSource { name: "major healing".into(), cmd: "cast maj".into(), mana_cost: 9 },
        ],
        "starlight lights rooms; it does not heal"
    );
}

/// Naming spells in `[bot].heal_spells` pins the choice. A name the
/// character does not know is simply absent — the book is the authority
/// on what it knows, and inventing a `cast` for a spell it lacks would
/// buy one "You do not know how to cast..." per attempt.
#[test]
fn configured_heal_spells_override_discovery() {
    let book = healer_book();
    assert_eq!(
        book.heal_spells(&["Mend".into()], Casting::Spells),
        vec![HealSource { name: "mend".into(), cmd: "cast mend".into(), mana_cost: 5 }],
        "matched case-insensitively"
    );
    assert!(
        book.heal_spells(&["godheal".into()], Casting::Spells).is_empty(),
        "a spell that is not in the book is not a heal this character has"
    );
}

/// `rapid healing` reads like a heal and is not one: duration 60, and
/// its min/max of 200 is an ability value rather than hit points. It is
/// a regen buff and belongs in `[bot].buffs`. `blessed vision` is the
/// symmetric trap on the buff side of the same list.
#[test]
fn the_near_misses_are_not_heals() {
    let book = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x2011   8    rapd  rapid healing                 \n\
         \x2041   4    bvis  blessed vision                \n",
    );
    assert!(book.heal_spells(&[], Casting::Spells).is_empty());
}

/// Mystics are a whole vocabulary, not a spelling. The board's own
/// redirect is the detection (VERIFIED, mud_core::text §8.12), and every
/// command built afterwards uses the matching verb.
#[test]
fn a_mystic_invokes_powers() {
    assert_eq!(
        Casting::redirected(mud_core::text::KAI_NO_SPELLS),
        Some(Casting::Powers)
    );
    assert_eq!(
        Casting::redirected(mud_core::text::NON_KAI_NO_POWERS),
        Some(Casting::Spells)
    );
    assert_eq!(
        Casting::redirected("You have the following spells:"),
        None,
        "an ordinary listing says nothing either way"
    );

    // The powers listing differs in its header caption and in a
    // right-aligned short column; the row parse is by whitespace, so
    // both fall out.
    let powers = Spellbook::parse(
        "You have the following powers:\n\
         Level Kai  Short Spell Name\n\
         \x20 5   4     lay  lay hands                     \n",
    );
    assert_eq!(
        powers.heal_spells(&["lay hands".into()], Casting::Powers),
        vec![HealSource { name: "lay hands".into(), cmd: "invoke lay".into(), mana_cost: 4 }]
    );
    assert!(Spellbook::parse("You have no powers.\n").spells.is_empty());
}

// --- HealState --------------------------------------------------------

fn heal_state() -> HealState {
    HealState::new(healer_book().heal_spells(&[], Casting::Spells))
}

fn prompt(hp: i32, mana: i32) -> Correlated {
    Correlated {
        event: Event::Prompt { hp, mana: Some(mana) },
        answers: None,
    }
}

fn answering(line: &str, id: CmdId) -> Correlated {
    Correlated {
        event: Event::Line(line.into()),
        answers: Some(id),
    }
}

/// The floor is the book's price, not a configured number, and the pool
/// picks the spell: 9 mana buys the dearest, 4 buys only the cheapest,
/// 2 buys nothing at all. Below every price this says Nothing rather
/// than anything louder — the rest mark takes over, and resting restores
/// mana as well as health, so the two compose without either knowing
/// about the other.
#[test]
fn the_pool_picks_the_spell() {
    let clock = RoundClock::new();
    let now = std::time::Instant::now();

    let mut rich = heal_state();
    rich.on_event(&prompt(20, 9));
    assert_eq!(rich.attempt(now, &clock), CastAttempt::Send("cast heal".into()));

    let mut thin = heal_state();
    thin.on_event(&prompt(20, 4));
    assert_eq!(thin.attempt(now, &clock), CastAttempt::Send("cast heal".into()));

    let mut broke = heal_state();
    broke.on_event(&prompt(20, 2));
    assert_eq!(broke.attempt(now, &clock), CastAttempt::Nothing);

    // A pool that has never been seen affords nothing: a character whose
    // prompt carries no mana is not a caster.
    let mut unseen = heal_state();
    assert_eq!(unseen.attempt(now, &clock), CastAttempt::Nothing);
}

/// One cast per round, because the board refuses a second
/// ("You have already cast a spell this round!"). The hold is until the
/// next round boundary, not a fixed sleep.
#[test]
fn a_second_cast_in_one_round_is_held() {
    let clock = RoundClock::new();
    let now = std::time::Instant::now();
    let mut heal = heal_state();
    heal.on_event(&prompt(20, 9));

    assert_eq!(heal.attempt(now, &clock), CastAttempt::Send("cast heal".into()));
    // The outcome lands, so nothing is owed — but the round has not
    // turned over.
    heal.on_sent("cast heal", CmdId(1));
    heal.on_event(&answering("You cast minor healing!", CmdId(1)));
    assert!(matches!(heal.attempt(now, &clock), CastAttempt::Hold(_)));
}

/// A cast in flight suppresses the next one outright. Nothing is owed
/// twice.
#[test]
fn an_owed_outcome_suppresses_the_next_cast() {
    let clock = RoundClock::new();
    let now = std::time::Instant::now();
    let mut heal = heal_state();
    heal.on_event(&prompt(20, 9));
    heal.attempt(now, &clock);
    heal.on_sent("cast heal", CmdId(1));

    assert!(heal.in_flight());
    assert_eq!(heal.attempt(now, &clock), CastAttempt::Nothing);
}

/// Every cast failure is a roll, a pool or a round, and comes round
/// again — EXCEPT one. "You do not know how to cast %s." means the spell
/// is not in the book and never will be, so that source is retired and
/// the next-cheapest takes over.
#[test]
fn only_an_unknown_spell_kills_a_source() {
    let clock = RoundClock::new();
    let now = std::time::Instant::now();

    for fizzle in [
        "You attempt to cast minor healing, but fail.",
        "You do not have enough mana to cast that spell.",
        "You have already cast a spell this round!",
    ] {
        let mut heal = heal_state();
        heal.on_event(&prompt(20, 9));
        heal.attempt(now, &clock);
        heal.on_sent("cast heal", CmdId(1));
        heal.on_event(&answering(fizzle, CmdId(1)));
        heal.new_visit();
        assert_eq!(
            heal.attempt(now, &clock),
            CastAttempt::Send("cast heal".into()),
            "{fizzle:?} is temporary"
        );
    }

    let mut heal = heal_state();
    heal.on_event(&prompt(20, 9));
    heal.attempt(now, &clock);
    heal.on_sent("cast heal", CmdId(1));
    heal.on_event(&answering("You do not know how to cast heal.", CmdId(1)));
    heal.new_visit();
    assert_eq!(
        heal.attempt(now, &clock),
        CastAttempt::Send("cast mend".into()),
        "the dead source is skipped, the next-cheapest is tried"
    );
}

/// A monster casting at us is routine din and says nothing about our own
/// spell. The correlator's attribution is what separates them — the
/// wording alone cannot, because "%s attempted to cast %s at you, but
/// failed." contains the same "fail" our fizzle does.
#[test]
fn a_monsters_cast_is_not_our_outcome() {
    let clock = RoundClock::new();
    let now = std::time::Instant::now();
    let mut heal = heal_state();
    heal.on_event(&prompt(20, 9));
    heal.attempt(now, &clock);
    heal.on_sent("cast heal", CmdId(1));

    heal.on_event(&Correlated {
        event: Event::Line("The cave bear attempted to cast blindness at you, but failed.".into()),
        answers: None,
    });
    assert!(heal.in_flight(), "somebody else's failure is not ours");

    // And an outcome attributed to a DIFFERENT send of ours is not it
    // either.
    heal.on_event(&answering("You cast starlight!", CmdId(2)));
    assert!(heal.in_flight());
}

// --- buffs ------------------------------------------------------------

use mud_client::sheet::{Buff, BuffState};
use std::collections::BTreeMap;

/// The shipped figures: bless is spell 14, duration 40 rounds.
fn durations() -> BTreeMap<String, u32> {
    BTreeMap::from([
        ("bless".to_string(), 40),
        ("greater bless".to_string(), 40),
        ("minor healing".to_string(), 0),
    ])
}

fn buff_book() -> Spellbook {
    Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 2   4    bles  bless                         \n\
         \x20 1   3    heal  minor healing                 \n",
    )
}

/// A buff needs both halves: the character has to know it, and it has to
/// last. Anything missing either is refused OUT LOUD rather than dropped
/// — a buff silently not being kept up looks exactly like one that is.
#[test]
fn buffs_are_refused_with_a_reason() {
    let (kept, refused) = mud_client::sheet::buffs(
        &buff_book(),
        &["bless".into(), "shockshield".into(), "minor healing".into()],
        &durations(),
        Casting::Spells,
    );
    assert_eq!(
        kept,
        vec![Buff {
            name: "bless".into(),
            cmd: "cast bles".into(),
            mana_cost: 4,
            rounds: 40,
        }]
    );
    assert_eq!(refused.len(), 2, "{refused:?}");
    assert!(refused[0].contains("not in this character's book"), "{refused:?}");
    // A heal has duration 0: it happens and is over. Kept up, it would
    // be recast forever, because a budget of 0 rounds is always expired.
    assert!(refused[1].contains("no duration"), "{refused:?}");
}

/// The budget, end to end. Cast once, then nothing until the rounds run
/// out — and only a CONFIRMED cast starts the clock, because a fizzle
/// leaves the buff genuinely down.
#[test]
fn a_buff_is_recast_when_its_budget_runs_out() {
    let clock = RoundClock::new();
    let (kept, _) = mud_client::sheet::buffs(
        &buff_book(),
        &["bless".into()],
        &durations(),
        Casting::Spells,
    );
    let mut buffs = BuffState::new(kept);
    let t0 = std::time::Instant::now();
    buffs.on_event(&prompt(30, 20), t0);

    assert_eq!(buffs.attempt(t0, &clock), CastAttempt::Send("cast bles".into()));
    buffs.on_sent("cast bles", CmdId(1));

    // A fizzle does not start the budget: the buff is not up.
    buffs.on_event(&answering("You attempt to cast bless, but fail.", CmdId(1)), t0);
    buffs.new_visit();
    assert_eq!(
        buffs.attempt(t0 + clock.period(), &clock),
        CastAttempt::Send("cast bles".into()),
        "a failed cast leaves it down"
    );
    buffs.on_sent("cast bles", CmdId(2));
    buffs.on_event(&answering("You cast bless!", CmdId(2)), t0);

    // Now it is up, and stays up for its 40 rounds.
    buffs.new_visit();
    assert_eq!(
        buffs.attempt(t0 + clock.period() * 39, &clock),
        CastAttempt::Nothing,
        "still inside the budget"
    );
    assert_eq!(
        buffs.attempt(t0 + clock.period() * 40, &clock),
        CastAttempt::Send("cast bles".into()),
        "the budget ran out"
    );
}

/// The wear-off line is an EARLY TRIGGER, not the mechanism. Its `%s` is
/// the spell's own free text, so which buff lapsed cannot be read off
/// it — every budget expires and the next quiet moment re-establishes
/// whatever is actually missing. One redundant cast is the worst case.
#[test]
fn a_wear_off_line_expires_the_budget_early() {
    let clock = RoundClock::new();
    let (kept, _) = mud_client::sheet::buffs(
        &buff_book(),
        &["bless".into()],
        &durations(),
        Casting::Spells,
    );
    let mut buffs = BuffState::new(kept);
    let t0 = std::time::Instant::now();
    buffs.on_event(&prompt(30, 20), t0);
    buffs.attempt(t0, &clock);
    buffs.on_sent("cast bles", CmdId(1));
    buffs.on_event(&answering("You cast bless!", CmdId(1)), t0);
    buffs.new_visit();
    assert_eq!(buffs.attempt(t0, &clock), CastAttempt::Nothing);

    // Unsolicited, unattributed, and naming a spell by its own prose.
    buffs.on_event(
        &Correlated {
            event: Event::Line("The effects of blur wear off.".into()),
            answers: None,
        },
        t0,
    );
    buffs.new_visit();
    assert_eq!(
        buffs.attempt(t0, &clock),
        CastAttempt::Send("cast bles".into()),
        "recast early rather than trusting a wording to name the right spell"
    );
}

/// Mana gates upkeep as it gates healing, and from the same source: the
/// book's own cost, not configuration.
#[test]
fn a_buff_is_not_cast_without_the_mana_for_it() {
    let clock = RoundClock::new();
    let (kept, _) = mud_client::sheet::buffs(
        &buff_book(),
        &["bless".into()],
        &durations(),
        Casting::Spells,
    );
    let mut buffs = BuffState::new(kept);
    let t0 = std::time::Instant::now();

    buffs.on_event(&prompt(30, 3), t0);
    assert_eq!(buffs.attempt(t0, &clock), CastAttempt::Nothing, "bless costs 4");
    buffs.on_event(&prompt(30, 4), t0);
    assert_eq!(buffs.attempt(t0, &clock), CastAttempt::Send("cast bles".into()));
}
