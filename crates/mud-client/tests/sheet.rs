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

use mud_client::sheet::{Inventory, LightSource, Spellbook};

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
        mud_client::sheet::light_sources(&Inventory::parse(text), &empty_book)
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
        mud_client::sheet::light_sources(&no_items, &book),
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
        mud_client::sheet::light_sources(&no_items, &dark).is_empty(),
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
        mud_client::sheet::light_sources(&inv, &Spellbook::default())
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
        mud_client::sheet::light_sources(&Inventory::default(), &book),
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
        mud_client::sheet::light_sources(&inv, &book),
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
    let sources = mud_client::sheet::light_sources(&inv, &book);
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
        mud_client::sheet::light_sources(&inv, &book),
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
    assert!(mud_client::sheet::light_sources(&inv, &book).is_empty());
}

/// Salad's real kit: no torch, but starlight in the book — one Spell
/// source whose mana cost comes from the book, which is the mana floor
/// the caster respects (no config key involved).
#[test]
fn the_real_character_derives_a_single_spell_source() {
    assert_eq!(
        mud_client::sheet::light_sources(
            &Inventory::parse(REAL_INVENTORY),
            &Spellbook::parse(REAL_SPELLBOOK)
        ),
        vec![LightSource::Spell {
            cmd: "cast star".into(),
            mana_cost: 4,
        }]
    );
}
