//! Spawn-table tests against the real WG3-NT database.
//!
//! Ground truths are SQL against `re/mmud_wgnt.sqlite`, cross-checked
//! with the field meanings in `re/docs/monsters.md` §1 and §4. The two
//! interesting cases are the ones a naive join gets wrong: region 0 is a
//! sentinel (Town Gates would otherwise spawn tapestries) and `bynumber`
//! is packed into the high word.

use mud_client::graph::{RoomGraph, SpawnKind};
use mud_client::spawn::{Dossier, SpawnTable, Threat};
use mud_core::content::RoomId;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn graph() -> &'static RoomGraph {
    use std::sync::OnceLock;
    static G: OnceLock<RoomGraph> = OnceLock::new();
    G.get_or_init(|| RoomGraph::load(&db_path()).expect("load room graph"))
}

fn spawns() -> &'static SpawnTable {
    use std::sync::OnceLock;
    static S: OnceLock<SpawnTable> = OnceLock::new();
    S.get_or_init(|| SpawnTable::load(&db_path()).expect("load spawn table"))
}

fn dossier(map: u16, room: u16) -> Dossier {
    Dossier::of(graph(), spawns(), RoomId { map, room }).expect("room exists")
}

fn names(d: &Dossier) -> Vec<&str> {
    d.candidates.iter().map(|t| t.name.as_str()).collect()
}

#[test]
fn small_cavern_spawns_only_cave_bear() {
    // The farm room: swarm spawn, region 6, band 66..66.
    let d = dossier(1, 2156);
    assert_eq!(d.name, "Small Cavern");
    assert_eq!(d.spawn.kind, SpawnKind::Swarm);
    assert_eq!(d.spawn.region, 6);
    assert_eq!(d.spawn.band, (66, 66));
    assert_eq!(d.spawn.forced, None);
    assert_eq!(names(&d), ["cave bear"]);

    let bear = &d.candidates[0];
    assert_eq!(bear.number, 80);
    assert_eq!(bear.experience, 100);
    assert_eq!(bear.hitpoints, 50);
    assert_eq!(bear.gamelimit, 1, "one cave bear in the world at a time");
    assert_eq!(bear.alignment, 1);
    assert_eq!(bear.aggression, 30);
    assert!(bear.initiates(), "behaviour mode 3 is not unprovoked");

    assert!(d.dark, "light -200");
}

#[test]
fn slum_entrance_spawns_guardsmen() {
    let d = dossier(1, 1072);
    assert_eq!(d.name, "Slum Entrance");
    assert_eq!(d.spawn.region, 5);
    assert_eq!(d.spawn.band, (1, 1));
    assert_eq!(names(&d), ["guardsman"]);
    assert_eq!(d.candidates[0].aggression, 90);
}

#[test]
fn a_band_wider_than_one_lists_every_level_in_it() {
    // Region 5, band 1..4: guardsman(1), Sheriff Lionheart(2), elite
    // guardsman(4). Index 3 has no monster in this region, which is why
    // the band is a range rather than a count.
    let d = dossier(1, 101);
    assert_eq!(d.name, "Silver Street, Eastern End");
    assert_eq!(d.spawn.kind, SpawnKind::Timed);
    assert_eq!(d.spawn.band, (1, 4));
    assert_eq!(
        names(&d),
        ["guardsman", "Sheriff Lionheart", "elite guardsman"],
        "in level order"
    );
}

#[test]
fn a_forced_monster_is_read_out_of_the_high_word() {
    // `bynumber` is a 32-bit read of a 16-bit field: 1769472 >> 16 = 27.
    // The low word is zero in all 26,720 rooms.
    let d = dossier(1, 305);
    assert_eq!(d.name, "Skali's Fine Armour, Front Room");
    assert_eq!(d.spawn.forced, Some(27));
    assert_eq!(d.spawn.kind, SpawnKind::BootFill);
    assert_eq!(
        names(&d),
        ["Gurbultis"],
        "forced monster, not the whole region"
    );
}

#[test]
fn region_zero_is_a_sentinel_not_a_region() {
    // Read literally, region 0 / band 0..0 joins the 29 group-0 scenery
    // props and has Town Gates spawning ancient tapestries.
    let d = dossier(1, 1);
    assert_eq!(d.name, "Town Gates");
    assert_eq!(d.spawn.region, 0);
    assert!(
        d.candidates.is_empty(),
        "nothing spawns at Town Gates, got {:?}",
        names(&d)
    );
    assert_eq!(d.threat(), Threat::Nothing);
}

#[test]
fn a_zero_band_still_selects_level_zero_monsters() {
    // 1,354 rooms carry a region with a zero band and 205 monsters sit at
    // index 0. Unlike region 0 this one is plausible, so it is kept.
    let d = dossier(1, 109);
    assert_eq!(d.name, "Darkwood Forest");
    assert_eq!(d.spawn.region, 11);
    assert_eq!(d.spawn.band, (0, 0));
    let got = names(&d);
    assert!(
        got.contains(&"minotaur champion") && got.contains(&"spectral mage"),
        "got {got:?}"
    );
}

#[test]
fn a_shop_room_reports_its_shop_and_spawns_nothing() {
    let d = dossier(1, 2324);
    assert_eq!(d.name, "Grungy Shop");
    assert_eq!(d.shop, 67);
    assert!(d.candidates.is_empty());
}

#[test]
fn threat_separates_what_starts_a_fight_from_what_does_not() {
    // The map's danger paint reads exactly this.
    assert_eq!(dossier(1, 2156).threat(), Threat::Aggressive);
    assert_eq!(dossier(1, 1).threat(), Threat::Nothing);
}

#[test]
fn a_dossier_renders_without_control_characters() {
    // These lines are painted into a fixed-width panel; a stray newline
    // scrolls the terminal (the lesson `render_status` records).
    for line in dossier(1, 2156).lines() {
        assert!(
            !line.chars().any(char::is_control),
            "control character in {line:?}"
        );
    }
}

