//! Reading MegaMud path files, against the real corpus.
//!
//! The numbers pinned here are measurements, not aspirations. Most of
//! these paths were written for other realms' custom maps and cannot
//! fully resolve against stock 1.11p; a test that demanded they did would
//! be a test of nothing. What must hold is that the importer says
//! honestly how far each one got.

use mud_client::graph::RoomGraph;
use mud_client::mega::{Index, ImportError, import, name_hash, parse_mp, room_id, slug};
use mud_core::content::RoomId;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn graph() -> &'static RoomGraph {
    use std::sync::OnceLock;
    static G: OnceLock<RoomGraph> = OnceLock::new();
    G.get_or_init(|| RoomGraph::load(&db_path()).expect("graph"))
}

fn index() -> &'static Index {
    use std::sync::OnceLock;
    static I: OnceLock<Index> = OnceLock::new();
    I.get_or_init(|| Index::build(graph()))
}

fn paths_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/mirrors/megamud.net/www.megamud.net/paths")
}

fn mp_files() -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(paths_dir())
        .expect("the mirrored path corpus")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mp"))
        .collect();
    files.sort();
    files
}

fn read(name: &str) -> String {
    std::fs::read_to_string(paths_dir().join(name)).expect("corpus file")
}

const CROSSROADS: RoomId = RoomId { map: 1, room: 1076 };

// --- the codec --------------------------------------------------------

#[test]
fn the_name_hash_is_the_last_three_hex_digits_of_the_weighted_sum() {
    // 'A' is 65, times position 1.
    assert_eq!(name_hash("A"), "041");
    // Short sums are zero-padded to three.
    assert_eq!(name_hash(""), "000");
    assert_eq!(name_hash("Slum Street, Crossroads"), "ABC");
}

/// The whole id, against a room whose file id is known: `slm2loop.mp`
/// opens on `ABC00055`, and that is the slum crossroads.
#[test]
fn a_room_id_matches_the_one_in_a_real_path_file() {
    let room = graph().room(CROSSROADS).expect("the crossroads");
    assert_eq!(room_id(&room.name, &room.exits), "ABC00055");
}

/// The point of the module: an id says what a room LOOKS like, so plenty
/// of rooms share one. Anything that treated it as a key would be wrong.
#[test]
fn a_room_id_is_a_checksum_and_collides_freely() {
    assert!(
        index().get("A0700050").len() > 20,
        "got {}",
        index().get("A0700050").len()
    );
    assert!(index().len() < graph().len() / 2, "{} ids", index().len());
}

// --- the parser -------------------------------------------------------

#[test]
fn a_path_with_both_header_lines_parses() {
    let mp = parse_mp(&read("rocsloop.mp")).expect("parse");
    assert_eq!(mp.name, "Narrow Plateau");
    assert_eq!(mp.author.as_deref(), Some("Connor Macleod"));
    assert_eq!(mp.start, "F0601040");
    assert_eq!(mp.steps.len(), 72);
    assert_eq!(mp.steps[0].command, "w");
}

/// `slm2loop.mp` has one bracket group and no author, and its goto-tree
/// lines are missing entirely. A bare title still has to be read as one.
#[test]
fn a_path_with_only_a_title_parses() {
    let mp = parse_mp(&read("slm2loop.mp")).expect("parse");
    assert_eq!(mp.name, "Slum loop with HO's in it");
    assert_eq!(mp.author, None);
    assert_eq!(mp.steps.len(), 82);
}

/// `delfcity.mp` is LF-only with an unbalanced bracket in its title.
#[test]
fn a_path_with_neither_crlf_nor_a_tidy_title_parses() {
    let mp = parse_mp(&read("delfcity.mp")).expect("parse");
    assert_eq!(mp.steps.len(), 71);
}

#[test]
fn something_that_is_not_a_path_is_refused() {
    assert!(parse_mp("hello\nworld\n").is_err());
    assert!(parse_mp("").is_err());
}

#[test]
fn a_title_becomes_a_usable_file_name() {
    assert_eq!(slug("Narrow Plateau"), "narrow-plateau");
    assert_eq!(slug("Dark Elf City)"), "dark-elf-city");
    assert_eq!(slug("Slum loop with HO's in it"), "slum-loop-with-ho-s-in-it");
    assert_eq!(slug("!!!"), "imported");
}

// --- importing --------------------------------------------------------

/// The best case, and proof the codec is right end to end: every step
/// walks, and every step lands where the file said it would.
#[test]
fn a_path_written_for_this_world_walks_and_verifies_completely() {
    let mp = parse_mp(&read("rocsloop.mp")).expect("parse");
    let (l, report) = import(graph(), index(), &mp, None).expect("import");
    assert_eq!(report.steps, 72);
    assert_eq!(report.walked, 72);
    assert_eq!(report.hash_matches, 72, "every room verified");
    assert_eq!(report.stopped_at, None);

    assert_eq!(l.name, "narrow-plateau");
    assert_eq!(l.origin.as_deref(), Some("megamud:Narrow Plateau"));
    assert_eq!(l.note.as_deref(), Some("by Connor Macleod"));
    // One stop per step, less the closing one, which is the first again.
    assert_eq!(l.stops.len(), 72);
    // Every stop records the direction that reached it, so the import
    // walks exactly the route the file walked rather than a shortest path
    // that happens to join the same rooms.
    assert!(l.stops[1].via.is_some());
    assert!(l.check_names(graph()).is_empty());
}

/// A foreign path that starts in a room we do not have. This is the
/// commonest outcome in the corpus and is not a failure of the importer.
#[test]
fn a_path_from_another_realm_says_so_instead_of_guessing() {
    let mp = parse_mp(&read("low1loop.mp")).expect("parse");
    match import(graph(), index(), &mp, None) {
        Err(ImportError::UnknownStart(id)) => assert_eq!(id, mp.start),
        other => panic!("expected UnknownStart, got {other:?}"),
    }
}

/// Divergence mid-path is reported with the step and the room it was
/// standing in — enough to go and look.
#[test]
fn a_path_that_stops_agreeing_says_where() {
    let mp = parse_mp(&read("goldloop.mp")).expect("parse");
    let (_, report) = import(graph(), index(), &mp, None).expect("import");
    assert_eq!(report.steps, 358);
    assert_eq!(report.walked, 280);
    let (step, at) = report.stopped_at.expect("it stops");
    assert_eq!(step, 280);
    assert_eq!(at, RoomId { map: 6, room: 2753 });
    assert!(
        report.lines().iter().any(|l| l.contains("6/2753")),
        "{:?}",
        report.lines()
    );
}

/// Flags mmc cannot act on are dropped, but never silently.
#[test]
fn dropped_flags_are_named_in_the_report() {
    let mp = parse_mp(&read("shadloop.mp")).expect("parse");
    let (_, report) = import(graph(), index(), &mp, None).expect("import");
    assert_eq!(report.dropped_flags, ["stash point"]);
    assert!(report.lines().iter().any(|l| l.contains("stash point")));
}

/// An ambiguous start is the operator's call, not a coin toss.
#[test]
fn an_ambiguous_start_lists_the_candidates_and_can_be_overridden() {
    let mp = parse_mp(&read("ccweloop.mp")).expect("parse");
    let rooms = match import(graph(), index(), &mp, None) {
        Err(ImportError::AmbiguousStart { rooms, .. }) => rooms,
        other => panic!("expected AmbiguousStart, got {other:?}"),
    };
    assert!(rooms.len() > 1);
    let (_, report) = import(graph(), index(), &mp, Some(rooms[0])).expect("import with a pick");
    assert!(report.walked > 0);
}

/// An imported loop has to be walkable by the runner that will walk it.
#[test]
fn an_imported_loop_validates_as_a_farm_circuit() {
    let mp = parse_mp(&read("rocsloop.mp")).expect("parse");
    let (l, _) = import(graph(), index(), &mp, None).expect("import");
    l.to_farm(&mud_client::farm::FarmConfig::default(), graph())
        .expect("every leg walkable");
}

/// The measured state of the whole corpus. These numbers are what the
/// importer actually achieves against stock 1.11p, and pinning them means
/// a change to the codec, the door test or the decode cannot quietly make
/// it worse.
#[test]
fn the_corpus_imports_as_well_as_it_measurably_can() {
    let files = mp_files();
    let (mut unique, mut ambiguous, mut unknown) = (0, 0, 0);
    let (mut full_walk, mut fully_verified) = (0, 0);
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let Ok(mp) = parse_mp(&text) else { continue };
        match import(graph(), index(), &mp, None) {
            Err(ImportError::UnknownStart(_)) => unknown += 1,
            Err(ImportError::AmbiguousStart { .. }) => ambiguous += 1,
            Ok((_, r)) => {
                unique += 1;
                if r.walked == r.steps {
                    full_walk += 1;
                    if r.hash_matches == r.steps {
                        fully_verified += 1;
                    }
                }
            }
        }
    }
    assert_eq!(files.len(), 19, "mirrored .mp files");
    assert_eq!(unique, 14, "paths whose start room resolves uniquely");
    assert_eq!(ambiguous, 2);
    assert_eq!(unknown, 3, "paths written for another realm's map");
    assert_eq!(full_walk, 7, "paths that walk end to end here");
    assert_eq!(fully_verified, 2, "paths where every room also verifies");
}
