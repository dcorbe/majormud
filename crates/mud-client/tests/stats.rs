//! `Stats::parse` against the `stat` sheet.
//!
//! The fixture in [`parses_every_field_from_the_beef_sheet`] is a live
//! capture from MMud Reborn, 2026-08-22 (see
//! `.superpowers/sdd/2026-08-22-session-knows-character/task-1-brief.md`).
//! Its columns do not line up between rows — `Traps` and `Picklocks` sit
//! alone on theirs — so a column-position parser would either misread or
//! silently drop them; this only proves out if every field is asserted.

use mud_client::stats::Stats;

#[test]
fn parses_every_field_from_the_beef_sheet() {
    let sheet = "\
Name: Beef                             Lives/CP:    9/100
Race: Dark-Elf    Exp: 0               Perception:     43
Class: Ninja      Level: 1             Stealth:        56
Hits:    22/22    Armour Class:   0/0  Thievery:        0
                                       Traps:          29
                                       Picklocks:      31
Strength:  40     Agility: 50          Tracking:       26
Intellect: 50     Health:  30          Martial Arts:   51
Willpower: 30     Charm:   40          MagicRes:       35
";

    let stats = Stats::parse(sheet);

    assert_eq!(stats.name.as_deref(), Some("Beef"));
    assert_eq!(stats.race.as_deref(), Some("Dark-Elf"));
    assert_eq!(stats.class.as_deref(), Some("Ninja"));
    assert_eq!(stats.level, Some(1));
    assert_eq!(stats.exp, Some(0));
    assert_eq!(stats.lives, Some(9));
    assert_eq!(stats.cp, Some(100));
    assert_eq!(stats.hits, Some(22));
    assert_eq!(stats.max_hits, Some(22));
    assert_eq!(stats.armour_class, Some(0));
    assert_eq!(stats.max_armour_class, Some(0));
    assert_eq!(stats.perception, Some(43));
    assert_eq!(stats.stealth, Some(56));
    assert_eq!(stats.thievery, Some(0));
    assert_eq!(stats.traps, Some(29));
    assert_eq!(stats.picklocks, Some(31));
    assert_eq!(stats.tracking, Some(26));
    assert_eq!(stats.martial_arts, Some(51));
    assert_eq!(stats.magic_res, Some(35));
    assert_eq!(stats.strength, Some(40));
    assert_eq!(stats.agility, Some(50));
    assert_eq!(stats.intellect, Some(50));
    assert_eq!(stats.health, Some(30));
    assert_eq!(stats.willpower, Some(30));
    assert_eq!(stats.charm, Some(40));
}

/// A board can bring the class column right up against the next label:
/// with a 10-character class name the fixed column position leaves only
/// ONE space before `Level:`, unlike the multi-space gutter the "Beef"
/// sheet happens to have. `MudPlay`'s `StatParser` hit exactly this (see
/// `archive/FujiTerm/MudPlay/Game/StatParser.cs`'s `ClassRx` and
/// `MudPlay.Tests/StatParserTests.cs`'s
/// `Class_SingleSpaceBeforeNextLabel_StillCaptured`) — a two-space-only
/// terminator silently drops the class here instead of erroring.
#[test]
fn class_survives_a_single_space_before_the_next_label() {
    let stats =
        Stats::parse("Class: Missionary Level: 2             Stealth:        65");

    assert_eq!(stats.class.as_deref(), Some("Missionary"));
    assert_eq!(stats.level, Some(2));
    assert_eq!(stats.stealth, Some(65));
}

#[test]
fn a_partial_sheet_parses_to_what_it_has() {
    let stats = Stats::parse("Name: Beef\nPicklocks: 31\n");

    assert_eq!(stats.name.as_deref(), Some("Beef"));
    assert_eq!(stats.picklocks, Some(31));
    assert_eq!(stats.race, None);
    assert_eq!(stats.class, None);
    assert_eq!(stats.level, None);
}

#[test]
fn empty_text_parses_to_an_entirely_absent_sheet() {
    let stats = Stats::parse("");

    assert_eq!(stats, Stats::default());
}
