//! Spawn-table tests against the real WG3-NT database.
//!
//! Ground truths are SQL against `re/mmud_wgnt.sqlite`, cross-checked
//! with the field meanings in `re/docs/monsters.md` §1 and §4. The two
//! interesting cases are the ones a naive join gets wrong: region 0 is a
//! sentinel (Town Gates would otherwise spawn tapestries) and `bynumber`
//! is packed into the high word.

use mud_client::graph::{RoomGraph, SpawnKind};
use mud_client::spawn::{Dossier, SpawnTable, Standing, Threat};
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
    assert_eq!(bear.behaviour, 1, "mode 1: initiates on sight");
    assert_eq!(bear.aggression, 30);
    assert!(bear.initiates_against(&Standing::default()));

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
    assert_eq!(d.threat(&Standing::default()), Threat::Nothing);
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
    assert_eq!(dossier(1, 2156).threat(&Standing::default()), Threat::Aggressive);
    assert_eq!(dossier(1, 1).threat(&Standing::default()), Threat::Nothing);
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


// --- corrections from the first live check, 2026-08-02 -----------------

/// The room's PERMANENT occupant, which is what a shop, a healer or a
/// trainer actually contains. Missing this made `/room` at the healer
/// list 33 quest NPCs and not the healer.
#[test]
fn a_permanent_npc_is_the_rooms_real_occupant() {
    let d = dossier(1, 2190);
    assert_eq!(d.name, "Newhaven, Healer");
    let resident = d.resident.as_ref().expect("the healer lives here");
    assert_eq!(resident.name, "healer");
    assert_eq!(resident.number, 73);
    assert_eq!(d.shop, 4);
}

/// A boot-fill room is skipped by the periodic spawner entirely
/// (`re/docs/monsters.md` §1), so listing its region's roster as things
/// that spawn there is simply false.
#[test]
fn a_boot_fill_room_draws_nothing_from_its_region() {
    let d = dossier(1, 2190);
    assert_eq!(d.spawn.kind, SpawnKind::BootFill);
    assert!(
        d.candidates.is_empty(),
        "the healer's room spawns nothing; got {:?}",
        names(&d)
    );
}

/// A zero band is an unconfigured room, not a level-0 selector.
///
/// This reverses an earlier reading. The evidence for it was that
/// Darkwood Forest's zero band resolved to monsters that looked
/// plausible for the area; the evidence against is stronger. Every
/// zero-band room in Newhaven is named "Blank" — they are dev
/// placeholders — and the healer, which is one, contains exactly one NPC
/// named by `permnpc` rather than the 33 its region would draw.
#[test]
fn a_zero_band_draws_nothing() {
    for room in [109u16, 2178] {
        let d = dossier(1, room);
        assert_eq!(d.spawn.band, (0, 0));
        assert!(
            d.candidates.is_empty(),
            "1/{room} ({}) has a zero band; got {:?}",
            d.name,
            names(&d)
        );
    }
}

/// The rooms that really do spawn must be untouched by all of that.
#[test]
fn a_configured_spawn_room_still_lists_its_roster() {
    let arena = dossier(1, 2150);
    assert_eq!(arena.name, "Newhaven, Arena");
    assert_eq!(arena.spawn.kind, SpawnKind::Frequent);
    assert_eq!(arena.spawn.band, (1, 3));
    assert!(!arena.candidates.is_empty());
    assert_eq!(names(&dossier(1, 2156)), ["cave bear"]);
    assert_eq!(names(&dossier(1, 1072)), ["guardsman"]);
}

/// A shop is not a danger, and the danger paint must not say it is.
///
/// `Passive` means "there are spawns here you could farm", not "somebody
/// is standing here". Counting any occupant made every shop, healer and
/// trainer light up magenta on the danger map — reported live while
/// looking at Newhaven, where nothing spawns and the shops were the only
/// colour on the screen.
#[test]
fn a_harmless_resident_does_not_make_a_room_dangerous() {
    // The healer: one resident, aggression 0, nothing spawns.
    let healer = dossier(1, 2190);
    assert!(healer.resident.is_some());
    assert!(healer.candidates.is_empty());
    assert_eq!(healer.threat(&Standing::default()), Threat::Nothing);

    // Jael's Missile Weapons: a shopkeeper and no spawns.
    assert_eq!(dossier(1, 159).threat(&Standing::default()), Threat::Nothing);
    // A room with neither resident nor spawn is still empty.
    assert_eq!(dossier(1, 2151).threat(&Standing::default()), Threat::Nothing);
}

/// A resident that WILL start the fight is exactly what the paint is for.
/// Aggression 100 permanent NPCs are real: the tasloi chief, the night
/// hag, the Bloody Executioner.
#[test]
fn a_hostile_resident_still_paints_the_room_dangerous() {
    let hag = dossier(3, 37);
    assert_eq!(hag.resident.as_ref().expect("night hag").aggression, 100);
    assert_eq!(hag.threat(&Standing::default()), Threat::Aggressive);
}

/// Newhaven's shopkeepers are rated aggression 100 and never attack
/// anybody. The figure describes how hard they fight once provoked, not
/// whether they start — which is why two earlier readings of "will this
/// thing attack me" both painted every shop in town hostile.
///
/// What separates them is having no attack table at all.
#[test]
fn a_shopkeeper_with_aggression_100_does_not_initiate() {
    for room in [2141u16, 2142, 2144, 2145, 2147] {
        let d = dossier(1, room);
        let npc = d.resident.as_ref().expect("a shopkeeper lives here");
        assert_eq!(npc.aggression, 100, "{} rates 100", d.name);
        assert!(!npc.armed(), "{} has no attack", d.name);
        assert!(
            !npc.initiates_against(&Standing::default()),
            "{} must not read as hostile",
            d.name
        );
        assert_eq!(d.threat(&Standing::default()), Threat::Nothing, "{}", d.name);
    }
}

/// And the things that really do come for you still read as hostile.
///
/// The slum entrance is deliberately NOT in this list: guardsmen are
/// behaviour mode 4 and never initiate against a law-abiding character,
/// so that room is a target rather than a threat. An earlier reading had
/// it magenta, which is how the whole decode came to be re-examined.
#[test]
fn a_monster_that_initiates_still_paints_dangerous() {
    for (map, room) in [(1u16, 2156u16), (3, 37)] {
        let d = dossier(map, room);
        assert_eq!(
            d.threat(&Standing::default()),
            Threat::Aggressive,
            "{}/{} {}",
            map,
            room,
            d.name
        );
    }
}

// --- initiation computed from the spec, not from the board -------------

/// The behaviour-mode taxonomy of `re/docs/monsters.md` §4, checked
/// against templates whose real behaviour is known.
#[test]
fn the_behaviour_mode_decides_initiation() {
    let lawful = Standing::default();
    for (number, mode, initiates, who) in [
        (80i64, 1i64, true, "cave bear, mode 1"),
        (7, 2, true, "kobold thief, mode 2"),
        (54, 3, false, "kobold slave, mode 3 sentinel"),
        (14, 4, false, "guardsman, mode 4"),
        (73, 4, false, "healer, mode 4"),
    ] {
        let t = spawns().by_number(number).expect(who);
        assert_eq!(t.behaviour, mode, "{who}");
        assert_eq!(t.initiates_against(&lawful), initiates, "{who}");
    }
}

/// Mode 6 spares the famous; ROAM class 5 hunts them. These are the only
/// 19 templates whose answer depends on the character at all.
#[test]
fn the_fame_dependent_branches_need_your_standing() {
    let lawful = Standing::default();
    let outlaw = Standing { fame: 0x28 };
    assert!(!lawful.notorious());
    assert!(outlaw.notorious());

    // Balthazar, mode 6: comes for the unknown, spares the notorious.
    let monk = spawns().by_number(263).expect("Balthazar");
    assert_eq!(monk.behaviour, 6);
    assert!(monk.initiates_against(&lawful));
    assert!(!monk.initiates_against(&outlaw));

    // Crocodile, ROAM class 5: the criminal hunter, exactly inverted.
    let croc = spawns().by_number(390).expect("crocodile");
    assert_eq!(croc.roam_class, 5);
    assert!(!croc.initiates_against(&lawful));
    assert!(croc.initiates_against(&outlaw));
}

/// The tier word the board prints in WHO is enough: every branch tests
/// one threshold and each tier sits wholly on one side of it.
#[test]
fn standing_reads_back_from_the_legal_level_word() {
    for (word, notorious) in [
        ("Saint", false),
        ("Good", false),
        ("Neutral", false),
        ("Lawful", false),
        ("Seedy", false),
        ("Outlaw", true),
        ("Criminal", true),
        ("Villain", true),
        ("FIEND", true),
    ] {
        let s = Standing::from_legal_level(word).unwrap_or_else(|| panic!("{word}"));
        assert_eq!(s.notorious(), notorious, "{word}");
    }
    assert_eq!(Standing::from_legal_level("banana"), None);
}
