//! Real-content armour test: the AC/DR chain driven against
//! `re/mmud_wgnt.sqlite` — shipped item columns, shipped monster damage
//! ranges, no fixtures.
//!
//! This file exists because of how the ac/dr column swap survived a whole
//! milestone. The port fed `Item::ac` (`+0x342`) into the fighter's soak
//! word and `Item::dr` (`+0x39c`) into its evasion word — exactly
//! transposed against `move_player_to_fighter` 24788-24789 — and not one
//! of the 950 tests in the suite noticed, because every fixture in it
//! fought naked. A geared player taking a hit was simply never a case.
//!
//! The 2026-07-26 Dodge-parry oracle expedition tripped over the
//! consequence and logged it in `re/docs/charm.md` §8.3 as an unexplained
//! "our soak may be 10x too strong". It was never a scaling error: the two
//! columns are different stats, and the transcripts pin both of them.
//!
//! MEASURED (`re/oracle/oracle_dodge_parry_acc-high.raw`). Oracle Delver
//! wore gilded robes, chain coif, displacer fur cloak, beaded belt and a
//! violet orchid:
//!
//! | item | # | `ac` (`+0x342`) | `dr` (`+0x39c`) |
//! |---|---|---|---|
//! | gilded robes | 410 | 70 | 0 |
//! | chain coif | 34 | 45 | 8 |
//! | displacer fur cloak | 1483 | 10 | 0 |
//! | beaded belt | 1218 | 0 | 0 |
//! | violet orchid | 2016 | 0 | 0 |
//! | **Σ** | | **125** | **8** |
//!
//! The board printed `Armour Class:  12/0` — 125/10 and 8/10, both
//! numbers — and giant bats (2..5 damage) bit for the full undiminished
//! 2..5, i.e. a soak of 8/10 = 0. A soak of Σac/10 = 12 would have made
//! that character immune to every bat in the caves.

use mud_core::content::{ClassId, ItemId, MonsterId, RaceId, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Player, SessionId};
use mud_core::content_db;

/// "Cavern, Dead End" (map 10) — the same empty arena
/// `charm_real_content.rs` uses: `attributes` 0, `monstertype` 0,
/// `permnpc` 0, so nothing spawns into it on boot.
const ARENA: RoomId = RoomId { map: 10, room: 42 };

/// `giant bat` — AC 10, DR 1, and a **2..5** attack form. The template the
/// expedition ground against, and the one whose full undiminished range in
/// the transcript is the evidence that worn `ac` is not the soak.
const GIANT_BAT: MonsterId = MonsterId(71);

const GILDED_ROBES: ItemId = ItemId(410);
const CHAIN_COIF: ItemId = ItemId(34);
const DISPLACER_FUR_CLOAK: ItemId = ItemId(1483);
const BEADED_BELT: ItemId = ItemId(1218);
const VIOLET_ORCHID: ItemId = ItemId(2016);
/// `rigid leather tunic` — ac 130 / dr 13, the cleanest single item for
/// showing that the two columns are a ~10:1 pair and NOT the same number
/// at two scales.
const RIGID_LEATHER_TUNIC: ItemId = ItemId(12);

const ORACLE_SET: [ItemId; 5] = [
    GILDED_ROBES,
    CHAIN_COIF,
    DISPLACER_FUR_CLOAK,
    BEADED_BELT,
    VIOLET_ORCHID,
];

fn load() -> Core {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    let content = content_db::load(&db).expect("load");
    Core::new(content, CoreConfig::default())
}

fn text_to(events: &[Event], session: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == session => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// A Dwarf warrior alone in the dead end, CARRYING `gear`. Nothing is worn
/// yet — [`seat`] puts it on through the `wear` command so the slot codes
/// come from each item's shipped `+0x398` rather than from this fixture.
/// HP is padded because two of these tests take real bat swings.
fn delver(gear: &[ItemId]) -> Player {
    Player {
        name: "Oracle Delver".into(),
        race: RaceId(2),
        class: ClassId(1),
        level: 2,
        current_hp: 5000,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: ARENA,
        stats: StatBlock {
            strength: 50,
            intellect: 30,
            wisdom: 50,
            agility: 30,
            health: 50,
            charm: 30,
        },
        inventory: gear.iter().map(|id| (*id, -1)).collect(),
        ..Default::default()
    }
}

fn seat(gear: &[ItemId]) -> (Core, SessionId) {
    let mut core = load();
    let s = core.attach_player(delver(gear));
    core.drain_events();
    for id in gear {
        let name = core.content().items[id].name.clone();
        core.input(s, &format!("wear {name}"));
    }
    core.drain_events();
    (core, s)
}

/// The measured pair, reproduced from the shipped columns with no board.
#[test]
fn the_oracle_set_renders_the_measured_armour_class_pair() {
    let (mut core, s) = seat(&ORACLE_SET);
    core.input(s, "stat");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Armour Class:  12/0"),
        "MEASURED oracle_dodge_parry_acc-high.raw: {shown:?}"
    );
}

/// The same set, one layer down: Σ`+0x342` = 125 reaches the evasion word
/// as 12 (÷10 at 24866) and Σ`+0x39c` = 8 reaches the soak word raw.
#[test]
fn the_oracle_set_fills_the_two_fighter_words() {
    let (core, s) = seat(&ORACLE_SET);
    let d = core.defender_debug(s);
    assert_eq!(d.evasion_a, 12, "Σ worn +0x342 ÷ 10");
    assert_eq!(d.armor, 8, "Σ worn +0x39c, raw");
}

/// The observation that started the investigation: `calculate_attack`
/// subtracts `armor/10` (25335), so Σdr 8 soaks **nothing** and the giant
/// bat's whole 2..5 band lands intact. Under the swapped wiring the soak
/// was 12 and this character could not have been scratched.
#[test]
fn the_giant_bat_band_survives_the_oracle_sets_soak() {
    let (core, s) = seat(&ORACLE_SET);
    let soak = core.defender_debug(s).armor / 10;
    assert_eq!(soak, 0, "Σdr 8 → 0 points of soak");

    let bat = &core.content().monsters[&GIANT_BAT];
    let form = bat
        .attacks
        .iter()
        .find(|f| f.kind != 0)
        .expect("giant bat has an attack form");
    assert_eq!(
        (form.min_damage, form.max_damage),
        (2, 5),
        "the template the expedition ground against"
    );
    assert_eq!(
        i32::from(form.min_damage) - soak,
        i32::from(form.min_damage),
        "every point of the 2..5 band arrives, as captured"
    );
}

/// One item, both columns, so the ~10:1 relationship between them is
/// visible without any summing: `rigid leather tunic` is ac 130 / dr 13
/// and reads `13/1`. Two numbers from two columns — not one number at two
/// scales, which is what the "10x" framing in charm.md §8.3 assumed.
#[test]
fn a_single_tunic_shows_the_two_columns_are_independent() {
    let (mut core, s) = seat(&[RIGID_LEATHER_TUNIC]);
    core.input(s, "stat");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Armour Class:  13/1"),
        "ac 130 → 13, dr 13 → 1: {shown:?}"
    );

    let d = core.defender_debug(s);
    assert_eq!(d.evasion_a, 13);
    assert_eq!(d.armor, 13, "raw; one point of soak after calculate_attack");
}

/// Shipped-data reachability, which is why this file is not fixtures: the
/// two columns disagree across most of the shipped gear, so a port that
/// reads one for the other is wrong on the bulk of it rather than on a
/// corner case.
#[test]
fn the_shipped_columns_are_not_redundant() {
    let core = load();
    let (mut ac_only, mut both) = (0, 0);
    for item in core.content().items.values() {
        match (item.evasion > 0, item.damage_resist > 0) {
            (true, false) => ac_only += 1,
            (true, true) => both += 1,
            _ => {}
        }
    }
    assert!(
        ac_only > both,
        "most AC-bearing gear carries NO damage resistance at all \
         ({ac_only} ac-only vs {both} carrying both) — reading `ac` as the \
         soak invents resistance for every one of them"
    );
}
