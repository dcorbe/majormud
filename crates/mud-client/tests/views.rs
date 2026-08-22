//! Equivalence tests for `crate::views`: the old hand-written SQL in
//! `graph.rs` (`load_threat`, `load_spell_durations`) against the new
//! views built over `mud_core::content_db::load()`'s `Content`, run
//! against the same shipped database (`2026-08-22-one-path-to-content`
//! Task 3). Proves the port faithful; nothing here argues the rules
//! themselves are correct.

use mud_client::graph::RoomGraph;
use mud_client::views;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

#[test]
fn threat_table_matches_the_old_sql() {
    let content = mud_core::content_db::load(&db_path()).expect("load content");
    let old = RoomGraph::load_threat(&db_path()).expect("old threat table");
    let new = views::threat_table(&content);

    assert!(!old.is_empty());
    assert_eq!(old, new);

    // Named checks against the plan's own evidence: 7 "giant rat" rows
    // collide, and the empty-name row (1 shipped) must not appear.
    assert!(new.contains_key("giant rat"));
    assert!(!new.contains_key(""));
}

#[test]
fn spell_durations_match_the_old_sql() {
    let content = mud_core::content_db::load(&db_path()).expect("load content");
    let old = RoomGraph::load_spell_durations(&db_path()).expect("old spell durations");
    let new = views::spell_durations(&content);

    // 453 of 1379 shipped spells survive the duration>0 / name!='' filter
    // against the real database -- the design doc's "542" does not match
    // measurement and is wrong; see the Task 3 report.
    assert_eq!(old.len(), 453);
    assert_eq!(old, new);

    // "rapid healing" is spell 138 and 831; the SHORTER duration wins.
    assert!(new.contains_key("rapid healing"));
    assert!(!new.contains_key(""));
}
