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

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mud_core::ability::Ability;
use mud_core::content::{
    Element, MatchType, SaveClass, ScalePair, Spell, SpellId, TargetMode,
};

use mud_client::sheet::{Casting, Inventory, LightSource, Spellbook};

/// One spell record with only the fields discovery reads set to
/// something: the ability list and the `target` column, decoded as
/// `match_type`. Everything else is zero.
fn spell(
    number: u16,
    name: &str,
    short: &str,
    mana: i16,
    duration: i16,
    target: MatchType,
    abilities: Vec<(Ability, i16)>,
) -> Spell {
    Spell {
        id: SpellId(number),
        name: name.into(),
        short_name: short.into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities,
        level_cap: 0,
        round_cost: 0,
        required_power: 0,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 0,
        duration_per_level: 0,
        match_type: target,
        duration,
        element: Element::Magic,
        class_gate_group: 0,
        mana_cost: mana,
        max_increase: ScalePair::NONE,
        required_class_level: 0,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

/// The shipped records this plan was measured against, by decoded
/// ability and `target` column. Values are the table's: the self buffs
/// carry 0 because the amount is level scaled.
fn spells() -> BTreeMap<SpellId, Spell> {
    let list = vec![
        spell(26, "starlight", "star", 4, 80, MatchType::Single1, vec![(Ability::RoomIllu, 0)]),
        spell(14, "bless", "bles", 4, 40, MatchType::Single2, vec![(Ability::Accuracy, 0)]),
        spell(39, "way of the cat", "cat", 3, 60, MatchType::Single1, vec![(Ability::Stealth, 0)]),
        spell(130, "shadowform", "shad", 8, 30, MatchType::Single1, vec![(Ability::Stealth, 0)]),
        spell(147, "glitterdust", "glit", 6, 30, MatchType::Single0, vec![(Ability::Stealth, 0)]),
        spell(448, "cross of vengeance", "cross", 15, 600, MatchType::Single1, vec![(Ability::RoomIllu, 200), (Ability::Stealth, -15)]),
        spell(1314, "camouflage", "camo", 10, 30, MatchType::Single1, vec![(Ability::Stealth, 0)]),
        spell(893, "stealth trap", "", 0, 1, MatchType::AreaB, vec![(Ability::RoomIllu, 0), (Ability::Stealth, -200)]),
        spell(2, "lightning bolt", "lb", 4, 0, MatchType::Single0, vec![(Ability::Damage, 10)]),
    ];
    list.into_iter().map(|s| (s.id, s)).collect()
}

#[test]
fn a_light_spell_carries_room_illumination_and_targets_self() {
    let spells = spells();
    assert!(mud_client::sheet::is_light_spell(&spells[&SpellId(26)]), "starlight");
    assert!(
        !mud_client::sheet::is_light_spell(&spells[&SpellId(893)]),
        "the stealth trap lights a room but is not a cast"
    );
    assert!(!mud_client::sheet::is_light_spell(&spells[&SpellId(2)]), "lightning bolt");
}

#[test]
fn a_stealth_spell_raises_stealth_and_targets_self() {
    let spells = spells();
    for id in [39, 130, 1314] {
        assert!(
            mud_client::sheet::is_stealth_spell(&spells[&SpellId(id)]),
            "{}",
            spells[&SpellId(id)].name
        );
    }
    assert!(
        !mud_client::sheet::is_stealth_spell(&spells[&SpellId(147)]),
        "glitterdust targets a monster"
    );
    assert!(
        !mud_client::sheet::is_stealth_spell(&spells[&SpellId(448)]),
        "cross of vengeance lowers stealth"
    );
    assert!(
        !mud_client::sheet::is_stealth_spell(&spells[&SpellId(14)]),
        "bless carries no stealth ability"
    );
}

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
        mud_client::sheet::light_sources(&Inventory::parse(text), &empty_book, &spells(), Casting::Spells)
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
        mud_client::sheet::light_sources(&no_items, &book, &spells(), Casting::Spells),
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
        mud_client::sheet::light_sources(&no_items, &dark, &spells(), Casting::Spells).is_empty(),
        "lightning bolt is not a light spell"
    );

    let unknown = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    cl    continual light               \n",
    );
    assert!(
        mud_client::sheet::light_sources(&no_items, &unknown, &spells(), Casting::Spells).is_empty(),
        "a name the spell table does not carry is not a light spell"
    );
}

/// Two spells in the book light a room and the cheaper one wins. Cross
/// of vengeance carries room illumination on a self cast, so it is a
/// light spell by the table, but it costs 15 mana against starlight's
/// 4 and it is listed FIRST here, so book order would take the wrong
/// one.
///
/// Mutation target: take the first match in book order and this picks
/// `cast cross` at 15 mana.
#[test]
fn the_cheapest_light_spell_is_the_light_source() {
    let no_items = Inventory::parse("You are carrying nothing.\n");
    let book = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 8  15    cross cross of vengeance            \n\
         \x20 1   4    star  starlight                     \n",
    );
    assert_eq!(
        mud_client::sheet::light_sources(&no_items, &book, &spells(), Casting::Spells),
        vec![LightSource::Spell {
            cmd: "cast star".into(),
            mana_cost: 4,
        }]
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
        mud_client::sheet::light_sources(&inv, &Spellbook::default(), &spells(), Casting::Spells)
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
        mud_client::sheet::light_sources(&Inventory::default(), &book, &spells(), Casting::Spells),
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
        mud_client::sheet::light_sources(&inv, &book, &spells(), Casting::Spells),
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
    let sources = mud_client::sheet::light_sources(&inv, &book, &spells(), Casting::Spells);
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
        mud_client::sheet::light_sources(&inv, &book, &spells(), Casting::Spells),
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
    assert!(mud_client::sheet::light_sources(&inv, &book, &spells(), Casting::Spells).is_empty());
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
            &spells(),
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
use mud_client::sheet::{
    CastAttempt, HealChoice, HealKind, HealNeed, HealSource, HealState, PartyHeal, PartyKind,
};
use mud_client::world::{RoundClock, ROUND};

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

fn no_choice() -> HealChoice<'static> {
    HealChoice { minor: "", major: "", regen: "" }
}

/// Discovery, and the split that matters: mana is the scarce resource, so
/// the CHEAPEST heal is the minor and the dearest is the major. Sizing
/// the spell to the wound is not attempted — the shipped min/max are
/// level-1 figures and the real heal scales with caster level, so any
/// such table would lie by more the longer a character had been played.
#[test]
fn heals_are_discovered_cheapest_as_minor_and_dearest_as_major() {
    let (heals, refused) = healer_book().heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells);
    assert!(refused.is_empty());
    let minor = heals.iter().find(|h| h.kind == HealKind::Minor).expect("a minor heal");
    let major = heals.iter().find(|h| h.kind == HealKind::Major).expect("a major heal");
    assert!(minor.mana_cost < major.mana_cost);
    assert!(heals.iter().all(|h| !matches!(h.kind, HealKind::Regen { .. })));
}

/// Naming a spell in `[bot].minor_heal_spell`/`major_heal_spell` pins the
/// choice. A name the character does not know is refused out loud —
/// inventing a `cast` for a spell it lacks would buy one "You do not
/// know how to cast..." per attempt.
#[test]
fn named_heals_override_discovery_and_an_unknown_name_is_refused() {
    let book = healer_book();
    let (heals, refused) = book.heal_spells(
        HealChoice { minor: "major healing", major: "", regen: "" },
        &BTreeMap::new(),
        Casting::Spells,
    );
    assert_eq!(heals.iter().find(|h| h.kind == HealKind::Minor).unwrap().name, "major healing");
    assert!(refused.is_empty());
    let (_, refused) = book.heal_spells(
        HealChoice { minor: "cure light wounds", major: "", regen: "" },
        &BTreeMap::new(),
        Casting::Spells,
    );
    assert_eq!(refused.len(), 1, "{refused:?}");
}

/// The regen is only ever named, never discovered, and needs a duration
/// from the spell table so it is not recast while running.
#[test]
fn a_regen_spell_needs_a_duration_and_is_a_third_kind() {
    let mut book = healer_book();
    book.spells.push(mud_client::sheet::KnownSpell {
        level: 5,
        mana: 6,
        short: "regn".into(),
        name: "regeneration".into(),
    });
    let mut durations = BTreeMap::new();
    let (_, refused) = book.heal_spells(
        HealChoice { minor: "", major: "", regen: "regeneration" },
        &durations,
        Casting::Spells,
    );
    assert_eq!(refused.len(), 1, "no duration known: {refused:?}");
    durations.insert("regeneration".into(), 20);
    let (heals, refused) = book.heal_spells(
        HealChoice { minor: "", major: "", regen: "regeneration" },
        &durations,
        Casting::Spells,
    );
    assert!(refused.is_empty());
    let regen = heals.iter().find(|h| matches!(h.kind, HealKind::Regen { rounds: 20 })).expect("regen");
    assert_eq!(regen.cmd, "cast regn");
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
    assert!(book.heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0.is_empty());
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
        powers
            .heal_spells(
                HealChoice { minor: "lay hands", major: "", regen: "" },
                &BTreeMap::new(),
                Casting::Powers,
            )
            .0,
        vec![HealSource {
            name: "lay hands".into(),
            cmd: "invoke lay".into(),
            mana_cost: 4,
            kind: HealKind::Minor,
        }]
    );
    assert!(Spellbook::parse("You have no powers.\n").spells.is_empty());
}

// --- HealState --------------------------------------------------------

fn heal_state() -> HealState {
    HealState::new(healer_book().heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0)
}

fn prompt(hp: i32, mana: i32) -> Correlated {
    Correlated {
        event: Event::Prompt { hp, mana: Some(mana), status: None },
        answers: None,
        elsewhere: false,
    }
}

fn answering(line: &str, id: CmdId) -> Correlated {
    Correlated {
        event: Event::Line(line.into()),
        answers: Some(id),
        elsewhere: false,
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
    rich.on_event(&prompt(20, 9), now);
    assert_eq!(rich.attempt(now, &clock, HealNeed::Minor), CastAttempt::Send("cast heal".into()));

    let mut thin = heal_state();
    thin.on_event(&prompt(20, 4), now);
    assert_eq!(thin.attempt(now, &clock, HealNeed::Minor), CastAttempt::Send("cast heal".into()));

    let mut broke = heal_state();
    broke.on_event(&prompt(20, 2), now);
    assert_eq!(broke.attempt(now, &clock, HealNeed::Minor), CastAttempt::Nothing);

    // A pool that has never been seen affords nothing: a character whose
    // prompt carries no mana is not a caster.
    let mut unseen = heal_state();
    assert_eq!(unseen.attempt(now, &clock, HealNeed::Minor), CastAttempt::Nothing);
}

/// One cast per round, because the board refuses a second
/// ("You have already cast a spell this round!"). The hold is until the
/// next round boundary, not a fixed sleep.
#[test]
fn a_second_cast_in_one_round_is_held() {
    let clock = RoundClock::new();
    let now = std::time::Instant::now();
    let mut heal = heal_state();
    heal.on_event(&prompt(20, 9), now);

    assert_eq!(heal.attempt(now, &clock, HealNeed::Minor), CastAttempt::Send("cast heal".into()));
    // The outcome lands, so nothing is owed — but the round has not
    // turned over.
    heal.on_sent("cast heal", CmdId(1));
    heal.on_event(&answering("You cast minor healing!", CmdId(1)), now);
    assert!(matches!(heal.attempt(now, &clock, HealNeed::Minor), CastAttempt::Hold(_)));
}

/// A cast in flight suppresses the next one outright. Nothing is owed
/// twice.
#[test]
fn an_owed_outcome_suppresses_the_next_cast() {
    let clock = RoundClock::new();
    let now = std::time::Instant::now();
    let mut heal = heal_state();
    heal.on_event(&prompt(20, 9), now);
    heal.attempt(now, &clock, HealNeed::Minor);
    heal.on_sent("cast heal", CmdId(1));

    assert!(heal.in_flight());
    assert_eq!(heal.attempt(now, &clock, HealNeed::Minor), CastAttempt::Nothing);
}

/// Every cast failure is a roll, a pool or a round, and comes round
/// again — EXCEPT one. "You do not know how to cast %s." means the spell
/// is not in the book and never will be, so that source is retired.
/// Retiring it costs only that one kind: the major heal is a separate
/// source and is untouched.
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
        heal.on_event(&prompt(20, 9), now);
        heal.attempt(now, &clock, HealNeed::Minor);
        heal.on_sent("cast heal", CmdId(1));
        heal.on_event(&answering(fizzle, CmdId(1)), now);
        heal.new_visit();
        assert_eq!(
            heal.attempt(now, &clock, HealNeed::Minor),
            CastAttempt::Send("cast heal".into()),
            "{fizzle:?} is temporary"
        );
    }

    let mut heal = heal_state();
    heal.on_event(&prompt(20, 9), now);
    heal.attempt(now, &clock, HealNeed::Minor);
    heal.on_sent("cast heal", CmdId(1));
    heal.on_event(&answering("You do not know how to cast heal.", CmdId(1)), now);
    heal.new_visit();
    assert_eq!(
        heal.attempt(now, &clock, HealNeed::Minor),
        CastAttempt::Nothing,
        "the dead minor has no fallback of its own kind"
    );
    assert_eq!(
        heal.attempt(now, &clock, HealNeed::Major),
        CastAttempt::Send("cast maj".into()),
        "a different kind is unaffected"
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
    heal.on_event(&prompt(20, 9), now);
    heal.attempt(now, &clock, HealNeed::Minor);
    heal.on_sent("cast heal", CmdId(1));

    heal.on_event(
        &Correlated {
            event: Event::Line("The cave bear attempted to cast blindness at you, but failed.".into()),
            answers: None,
            elsewhere: false,
        },
        now,
    );
    assert!(heal.in_flight(), "somebody else's failure is not ours");

    // And an outcome attributed to a DIFFERENT send of ours is not it
    // either.
    heal.on_event(&answering("You cast starlight!", CmdId(2)), now);
    assert!(heal.in_flight());
}

// --- which heal ----------------------------------------------------------

#[test]
fn the_need_picks_the_kind_and_falls_back_to_the_minor() {
    let clock = RoundClock::new();
    let now = std::time::Instant::now();
    let mut s = heal_state();
    s.on_event(&prompt(20, 9), now);
    let major_cmd = healer_book()
        .heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells)
        .0
        .into_iter()
        .find(|h| h.kind == HealKind::Major)
        .unwrap()
        .cmd;
    assert_eq!(s.attempt(now, &clock, HealNeed::Major), CastAttempt::Send(major_cmd));
    // Too poor for the major: the minor goes out instead.
    let mut poor = heal_state();
    poor.on_event(&prompt(20, 4), now);
    assert_eq!(poor.attempt(now, &clock, HealNeed::Major), CastAttempt::Send("cast heal".into()));
    // No regen named: the minor.
    let mut none = heal_state();
    none.on_event(&prompt(20, 9), now);
    assert_eq!(none.attempt(now, &clock, HealNeed::Regen), CastAttempt::Send("cast heal".into()));
}

#[test]
fn a_running_regen_is_not_recast_until_its_rounds_run_out() {
    let clock = RoundClock::new();
    let t0 = std::time::Instant::now();
    let mut book = healer_book();
    book.spells.push(mud_client::sheet::KnownSpell { level: 5, mana: 6, short: "regn".into(), name: "regeneration".into() });
    let mut durations = BTreeMap::new();
    durations.insert("regeneration".to_string(), 4);
    let (heals, _) = book.heal_spells(HealChoice { minor: "", major: "", regen: "regeneration" }, &durations, Casting::Spells);
    let mut s = HealState::new(heals);
    s.on_event(&prompt(20, 9), t0);
    assert_eq!(s.attempt(t0, &clock, HealNeed::Regen), CastAttempt::Send("cast regn".into()));
    s.on_sent("cast regn", CmdId(1));
    s.on_event(&answering("You cast regeneration on yourself.", CmdId(1)), t0);
    // Running: the next round in the band falls back to the minor.
    let t1 = t0 + ROUND;
    s.on_event(&prompt(20, 9), t1);
    assert_eq!(s.attempt(t1, &clock, HealNeed::Regen), CastAttempt::Send("cast heal".into()));
    // Rounds out: the regen again.
    let t2 = t0 + ROUND * 5;
    s.on_event(&prompt(20, 9), t2);
    assert_eq!(s.attempt(t2, &clock, HealNeed::Regen), CastAttempt::Send("cast regn".into()));
}

// --- buffs ------------------------------------------------------------

use mud_client::sheet::{Buff, BuffState};

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
/// last. Anything missing either is refused OUT LOUD rather than dropped.
/// A buff silently not being kept up looks exactly like one that is.
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

/// A buff may be named by its short name, the one `cast` takes and the
/// book's listing shows, as well as its full name. `["shld", "blur"]`
/// in a profile cast nothing, refused as not in the book, because only
/// the full name was matched (2026-09-08).
#[test]
fn a_buff_may_be_named_by_its_short_name() {
    let (kept, refused) = mud_client::sheet::buffs(
        &buff_book(),
        &["bles".into(), "BLESS".into()],
        &durations(),
        Casting::Spells,
    );
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(|b| b.cmd == "cast bles"), "{kept:?}");
}

/// The budget, end to end. Cast once, then nothing until the rounds run
/// out. Only a CONFIRMED cast starts the clock, because a fizzle leaves
/// the buff genuinely down.
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
            elsewhere: false,
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

/// The ring line, live 2026-09-05 with one key: two spaces after the
/// colon and a trailing period. It follows the carried list, which
/// wraps mid-item, so the parser has to close the carried list on it.
#[test]
fn inventory_reads_the_key_ring() {
    let inv = Inventory::parse(
        "You are carrying 38 runic coins, ninjato (Weapon Hand), white gold\n\
         ring\n\
         You have the following keys:  black star key.\n\
         Wealth: 38982274 copper farthings\n\
         Encumbrance: 430/1680 - Light [25%]\n",
    );
    assert_eq!(
        inv.items,
        vec![
            "38 runic coins".to_string(),
            "ninjato (Weapon Hand)".to_string(),
            "white gold ring".to_string(),
        ]
    );
    assert_eq!(inv.keys, vec!["black star key".to_string()]);
}

/// Two keys are comma separated like the carried list.
#[test]
fn inventory_reads_several_keys() {
    let inv = Inventory::parse(
        "You are carrying nothing.\n\
         You have the following keys:  black star key, bone key.\n\
         Wealth: 0 copper farthings\n",
    );
    assert_eq!(
        inv.keys,
        vec!["black star key".to_string(), "bone key".to_string()]
    );
}

#[test]
fn an_empty_key_ring_reads_as_no_keys() {
    let inv = Inventory::parse("You are carrying nothing.\nYou have no keys.\n");
    assert!(inv.keys.is_empty());
}

/// The inventory reply is where the gate reads its counts from.
#[test]
fn inventory_reads_its_coins() {
    let inv = Inventory::parse(
        "You are carrying 11 silver nobles, 49 copper farthings, quarterstaff\n\
         You have no keys.\n\
         Wealth: 159 copper farthings\n\
         Encumbrance: 119/2880 - None [4%]\n",
    );
    assert_eq!(inv.coins().counts, [49, 11, 0, 0, 0]);
    let none = Inventory::parse("You are carrying nothing.\nYou have no keys.\n");
    assert_eq!(none.coins().count(), 0);
}

// --- stealth spells ---------------------------------------------------

fn ranger_book() -> Spellbook {
    Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    star  starlight                     \n\
         \x20 3  10    camo  camouflage                    \n\
         \x20 2   4    bles  bless                         \n",
    )
}

fn stealth_durations() -> BTreeMap<String, u32> {
    BTreeMap::from([
        ("starlight".to_string(), 80),
        ("camouflage".to_string(), 30),
        ("bless".to_string(), 40),
        ("shadowform".to_string(), 30),
    ])
}

#[test]
fn a_known_stealth_spell_is_a_buff_with_the_shipped_duration() {
    let (found, refused) = mud_client::sheet::stealth_spells(
        &ranger_book(),
        &spells(),
        &stealth_durations(),
        Casting::Spells,
    );
    assert_eq!(
        found,
        vec![Buff {
            name: "camouflage".into(),
            cmd: "cast camo".into(),
            mana_cost: 10,
            rounds: 30,
        }]
    );
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn a_stealth_spell_the_book_lacks_is_absent_not_refused() {
    let book = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    star  starlight                     \n",
    );
    let (found, refused) =
        mud_client::sheet::stealth_spells(&book, &spells(), &stealth_durations(), Casting::Spells);
    assert!(found.is_empty());
    assert!(refused.is_empty(), "shadowform is in the table and not in the book: nothing to say");
}

#[test]
fn a_stealth_spell_with_no_duration_is_refused_by_name() {
    let mut durations = stealth_durations();
    durations.insert("camouflage".to_string(), 0);
    let (found, refused) = mud_client::sheet::stealth_spells(
        &ranger_book(),
        &spells(),
        &durations,
        Casting::Spells,
    );
    assert!(found.is_empty());
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(refused[0].contains("camouflage"), "{refused:?}");
    assert!(refused[0].contains("no duration"), "{refused:?}");
}

#[test]
fn a_stealth_spell_missing_from_durations_is_refused_by_name() {
    let mut durations = stealth_durations();
    durations.remove("camouflage");
    let (found, refused) = mud_client::sheet::stealth_spells(
        &ranger_book(),
        &spells(),
        &durations,
        Casting::Spells,
    );
    assert!(found.is_empty());
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(refused[0].contains("camouflage"), "{refused:?}");
    assert!(refused[0].contains("no duration"), "{refused:?}");
}

#[test]
fn a_mystic_invokes_its_stealth_power() {
    let (found, _) = mud_client::sheet::stealth_spells(
        &ranger_book(),
        &spells(),
        &stealth_durations(),
        Casting::Powers,
    );
    assert_eq!(found[0].cmd, "invoke camo");
}

#[test]
fn the_report_names_the_stealth_spell_and_its_cost() {
    let book = ranger_book();
    let (found, refused) =
        mud_client::sheet::stealth_spells(&book, &spells(), &stealth_durations(), Casting::Spells);
    assert_eq!(
        mud_client::sheet::stealth_lines(&book, &found, &refused),
        vec!["stealth: camouflage (10 mana, 30 rounds)".to_string()]
    );
}

#[test]
fn the_report_says_none_known_for_a_book_without_one() {
    let book = Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   4    star  starlight                     \n",
    );
    assert_eq!(
        mud_client::sheet::stealth_lines(&book, &[], &[]),
        vec!["stealth: none known".to_string()]
    );
}

#[test]
fn the_report_is_silent_for_a_character_with_no_spellbook() {
    let book = Spellbook::parse("You have no spells.\n");
    assert!(mud_client::sheet::stealth_lines(&book, &[], &[]).is_empty());
}

#[test]
fn the_report_carries_a_refusal_as_its_own_line() {
    let book = ranger_book();
    let refused = vec!["`camouflage` raises stealth but has no duration in the spell table".to_string()];
    assert_eq!(
        mud_client::sheet::stealth_lines(&book, &[], &refused),
        vec!["stealth: `camouflage` raises stealth but has no duration in the spell table".to_string()]
    );
}


/// A mystic's book, as the oracle listing shows it
/// (`re/oracle/oracle_kai_mystic_timing.log`): kai for mana, and
/// every power named "way of the ...".
fn mystic_book() -> Spellbook {
    Spellbook::parse(
        "You have the following powers:\n\
         Level Kai  Short Spell Name\n\
         \x20 2   1    swan  way of the swan               \n\
         \x20 3   2     owl  way of the owl                \n",
    )
}

/// Way of the swan heals (ability 18 in the shipped table), so a
/// mystic's minor heal is discovered the same way a cleric's is, and
/// invoked rather than cast.
#[test]
fn a_mystic_discovers_way_of_the_swan_as_the_minor_heal() {
    let (heals, refused) = mystic_book().heal_spells(no_choice(), &BTreeMap::new(), Casting::Powers);
    assert!(refused.is_empty(), "{refused:?}");
    let minor = heals.iter().find(|h| h.kind == HealKind::Minor).expect("a minor heal");
    assert_eq!(minor.name, "way of the swan");
    assert_eq!(minor.cmd, "invoke swan");
    assert!(heals.iter().all(|h| h.kind != HealKind::Major));
}

/// `swan` is what the operator types at the board and what the listing
/// shows, so it names the heal too, the way a buff may be named by its
/// short name.
#[test]
fn a_heal_may_be_named_by_its_short_name() {
    let (heals, refused) = healer_book().heal_spells(
        HealChoice { minor: "mend", major: "maj", regen: "" },
        &BTreeMap::new(),
        Casting::Spells,
    );
    assert!(refused.is_empty(), "{refused:?}");
    let minor = heals.iter().find(|h| h.kind == HealKind::Minor).expect("a minor heal");
    let major = heals.iter().find(|h| h.kind == HealKind::Major).expect("a major heal");
    assert_eq!(minor.name, "mend");
    assert_eq!(major.name, "major healing");
}

#[test]
fn the_outcome_grammar_reads_the_three_families() {
    use mud_client::sheet::{Outcome, cast_outcome};
    assert_eq!(cast_outcome("You do not know how to cast mahe."), Some(Outcome::Unknown));
    assert_eq!(cast_outcome("You cast major healing on Celery!"), Some(Outcome::Cast));
    assert_eq!(cast_outcome("You cast healing rain on the room!"), Some(Outcome::Cast));
    assert_eq!(cast_outcome("You invoke way of the swan!"), Some(Outcome::Cast));
    assert_eq!(cast_outcome("You attempt to cast major healing at Celery, but fail."), Some(Outcome::Failed));
    assert_eq!(cast_outcome("You do not have enough mana to cast that spell!"), Some(Outcome::Failed));
    assert_eq!(cast_outcome("You have already cast a spell this round!"), Some(Outcome::Failed));
    assert_eq!(cast_outcome("Celery is looking around the room."), None);
    assert_eq!(cast_outcome("A rat attempted to cast fire at you, but failed."), None, "the caller gates by attribution, this only names the family");
}

// --- PartyHeal ---------------------------------------------------------

use mud_client::bot::BotConfig;
use mud_client::party::{Health, Member};

fn party_book() -> Spellbook {
    Spellbook::parse(
        "You have the following spells:\n\
         Level Mana Short Spell Name\n\
         \x20 1   2    mihe  minor healing                 \n\
         \x20 8   6    mahe  major healing                 \n\
         \x2010   5    rain  healing rain                  \n\
         \x20 7   8    cure  cure poison                   \n",
    )
}

fn party_state() -> PartyHeal {
    let book = party_book();
    let heals = book.heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0;
    PartyHeal::new(book.party_heals(&heals, Casting::Spells))
}

fn marks() -> BotConfig {
    BotConfig { minor_heal_at_percent: 70, major_heal_at_percent: 40, ..BotConfig::default() }
}

fn row(name: &str, class: &str, hp: u8) -> Member {
    Member { name: name.into(), invited: false, class: Some(class.into()), hp: Some(hp), pool: None }
}

fn health(rows: &[Member], at: Instant) -> Health {
    let mut h = Health::new();
    h.on_roster(rows, at);
    h
}

#[test]
fn the_book_yields_singles_area_and_cure() {
    let book = party_book();
    let heals = book.heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0;
    let sources = book.party_heals(&heals, Casting::Spells);
    let kinds: Vec<PartyKind> = sources.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![PartyKind::Single(HealKind::Minor), PartyKind::Single(HealKind::Major), PartyKind::Area, PartyKind::Cure]
    );
    assert_eq!(sources[1].cmd, "cast mahe");
    assert_eq!(sources[2].cmd, "cast rain");
    assert_eq!(sources[3].cmd, "cast cure");
    let plain = healer_book();
    let heals = plain.heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0;
    assert!(plain.party_heals(&heals, Casting::Spells).iter().all(|s| matches!(s.kind, PartyKind::Single(_))));
}

#[test]
fn one_hurt_member_gets_the_single_by_the_marks() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Ninja", 100)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 60)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mihe celery".into()));
}

#[test]
fn the_major_falls_back_to_the_minor_on_the_pool() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 4), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mihe celery".into()));
    let mut broke = party_state();
    broke.on_event(&prompt(100, 1), t0, &clock);
    assert_eq!(broke.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Nothing);
    let mut unseen = party_state();
    assert_eq!(unseen.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Nothing, "a pool never seen affords nothing");
}

#[test]
fn two_under_the_mark_is_rain_and_the_healer_counts_itself() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Ninja", 50)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast rain".into()));
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(50), &marks()), CastAttempt::Send("cast rain".into()));
    let mut p = party_state();
    p.on_event(&prompt(100, 4), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Ninja", 50)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mihe celery".into()), "rain unaffordable, the lowest gets what the pool buys");
}

#[test]
fn a_witchunter_is_neither_counted_nor_healed() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Witchunter", 20)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    let h = health(&[row("Beef", "Witchunter", 20)], t0);
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Nothing);
}

/// UNVERIFIED. No self cure has been seen on the live board. The
/// captured targeted form is `You cast blur on Vexil!`, and this
/// assumes the board names the caster the same way it names anyone
/// else.
const SELF_CURE: &str = "You cast cure poison on Yourself!";

/// The self cure is not a party cast. A character with `[party].heal`
/// off, or one in no party at all, still cures itself, so the caller
/// reads this on every prompt ahead of the gated party block.
#[test]
fn the_self_cure_is_its_own_attempt() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    assert_eq!(p.attempt_self_cure(t0, &clock), CastAttempt::Nothing, "nothing to cure");
    p.poison_self(t0);
    assert_eq!(p.attempt_self_cure(t0, &clock), CastAttempt::Send("cast cure".into()));
    p.on_sent("cast cure", CmdId(1));
    assert_eq!(p.attempt_self_cure(t0, &clock), CastAttempt::Nothing, "one cast out at a time");
    p.on_event(&answering(SELF_CURE, CmdId(1)), t0, &clock);
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt_self_cure(t1, &clock), CastAttempt::Nothing, "cured, and no timer clears it");

    // A pool that cannot buy the cure says nothing rather than
    // spending the round on a cast that never goes out.
    let mut broke = party_state();
    broke.on_event(&prompt(100, 4), t0, &clock);
    broke.poison_self(t0);
    assert_eq!(broke.attempt_self_cure(t0, &clock), CastAttempt::Nothing);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(
        broke.attempt(t0, &clock, &h, Some(100), &marks()),
        CastAttempt::Send("cast mihe celery".into()),
        "the round was not spent"
    );
}

#[test]
fn cure_comes_before_heal_and_self_before_others() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let mut h = health(&[row("Celery", "Mage", 30)], t0);
    h.on_cure("Celery", t0);
    p.poison_self(t0);
    // The caller asks for the self cure first, and it takes the round.
    assert_eq!(p.attempt_self_cure(t0, &clock), CastAttempt::Send("cast cure".into()));
    assert!(
        matches!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Hold(_)),
        "the round is spent on the self cure"
    );
    p.on_sent("cast cure", CmdId(1));
    p.on_event(&answering(SELF_CURE, CmdId(1)), t0, &clock);
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt_self_cure(t1, &clock), CastAttempt::Nothing, "the poison is cured");
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast cure celery".into()));
    p.on_sent("cast cure celery", CmdId(2));
    p.on_event(&answering("You cast cure poison on Celery!", CmdId(2)), t1, &clock);
    let t2 = t1 + Duration::from_secs(10);
    assert_eq!(p.attempt(t2, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()), "cured, the heal is next");
}

#[test]
fn a_cast_marks_its_targets_until_a_fresher_row() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let mut h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(1));
    assert!(p.in_flight());
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Nothing, "one cast out at a time");
    p.on_event(&answering("You cast major healing on Celery!", CmdId(1)), t1, &clock);
    assert!(!p.in_flight());
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Nothing, "healed since the row was seen");
    let t2 = t1 + Duration::from_secs(10);
    h.on_heal("Celery", 35, t2);
    assert_eq!(p.attempt(t2, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()), "a fresher row is due again");
}

#[test]
fn rain_marks_every_counted_row() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30), row("Beef", "Ninja", 50)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast rain".into()));
    p.on_sent("cast rain", CmdId(1));
    p.on_event(&answering("You cast healing rain on the room!", CmdId(1)), t0, &clock);
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Nothing);
}

#[test]
fn a_failure_frees_the_next_round_and_an_unknown_spell_retires_the_source() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(1));
    p.on_event(&answering("You attempt to cast major healing at Celery, but fail.", CmdId(1)), t0, &clock);
    assert!(matches!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Hold(_)), "the round is spent");
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(2));
    p.on_event(&answering("You do not know how to cast mahe.", CmdId(2)), t1, &clock);
    let t2 = t1 + Duration::from_secs(10);
    assert_eq!(p.attempt(t2, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mihe celery".into()), "the major is dead, the minor answers");
}

#[test]
fn a_self_cast_holds_the_party_cast_a_round() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    p.hold_round(t0);
    assert!(matches!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Hold(_)));
    let mut s = heal_state();
    s.on_event(&prompt(20, 9), t0);
    s.hold_round(t0);
    assert!(matches!(s.attempt(t0, &clock, HealNeed::Minor), CastAttempt::Hold(_)));
    assert!(s.affords(HealNeed::Minor));
    let mut broke = heal_state();
    broke.on_event(&prompt(20, 1), t0);
    assert!(!broke.affords(HealNeed::Major));
}


/// A name the room does not hold used to wedge the healer for the rest
/// of the run: the reply was attributed to nothing, `pending` stayed
/// set, and every later attempt returned nothing. The reply now
/// completes the cast, and a cast nobody answered at all is given up
/// on after two rounds.
#[test]
fn a_missing_target_does_not_wedge_the_healer() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let h = health(&[row("Celery", "Mage", 30)], t0);

    // The attribution gate still holds: the same wording from the room
    // at large answers nothing.
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(1));
    let stray = Correlated {
        event: Event::Line("You don't see celery here.".into()),
        answers: None,
        elsewhere: false,
    };
    p.on_event(&stray, t0, &clock);
    assert!(p.in_flight(), "unattributed, so it says nothing about our cast");

    // Attributed, it is a failure and the round comes round again.
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(1));
    p.on_event(&answering("You don't see celery here.", CmdId(1)), t0, &clock);
    assert!(!p.in_flight());
    let t1 = t0 + ROUND * 2;
    assert_eq!(p.attempt(t1, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));

    // And an answer that never came at all frees the cast two rounds
    // later, once a prompt says so.
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(1));
    assert_eq!(p.attempt(t0 + ROUND, &clock, &h, Some(100), &marks()), CastAttempt::Nothing, "one round on, still out");
    p.on_event(&prompt(100, 20), t0 + ROUND * 2, &clock);
    assert!(!p.in_flight(), "two rounds on, a prompt gives up on it");
    assert_eq!(
        p.attempt(t0 + ROUND * 2, &clock, &h, Some(100), &marks()),
        CastAttempt::Send("cast mahe celery".into()),
        "two rounds on, given up on"
    );
}

/// The unknown-spell outcome is terminal and the absent-target one is
/// not, so the shared grammar must tell them apart.
#[test]
fn the_absent_target_reply_is_a_failure() {
    use mud_client::sheet::{Outcome, cast_outcome};
    assert_eq!(cast_outcome("You don't see celery here."), Some(Outcome::Failed));
}


/// A rebuild hands the party state on through `carry_party`, and the
/// first tick after it re-reads the sheet. Reading the sheet must not
/// undo the handover: a member healed a moment ago would be healed
/// again on the next prompt.
#[test]
fn a_reload_keeps_the_healed_marks() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut p = party_state();
    p.on_event(&prompt(100, 20), t0, &clock);
    let h = health(&[row("Celery", "Mage", 30)], t0);
    assert_eq!(p.attempt(t0, &clock, &h, Some(100), &marks()), CastAttempt::Send("cast mahe celery".into()));
    p.on_sent("cast mahe celery", CmdId(1));
    p.on_event(&answering("You cast major healing on Celery!", CmdId(1)), t0, &clock);

    let book = party_book();
    let heals = book.heal_spells(no_choice(), &BTreeMap::new(), Casting::Spells).0;
    p.reload(book.party_heals(&heals, Casting::Spells));
    assert!(!p.in_flight(), "the book is new, so the source a cast named is not");

    // The pool crossed the reload: a member this character has never
    // healed gets a cast with no fresh prompt to seed one.
    let t1 = t0 + ROUND * 2;
    let stranger = health(&[row("Beef", "Ninja", 30)], t0);
    assert_eq!(
        p.attempt(t1, &clock, &stranger, Some(100), &marks()),
        CastAttempt::Send("cast mahe beef".into())
    );
    p.new_visit();

    // And so did the healed mark.
    let t2 = t1 + ROUND * 2;
    assert_eq!(
        p.attempt(t2, &clock, &h, Some(100), &marks()),
        CastAttempt::Nothing,
        "Celery was healed after its row was seen, reload or not"
    );
}

