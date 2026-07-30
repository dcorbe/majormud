//! Real-content .HSE coverage (gangs.md §4): every FILE DESCRIPTION
//! room in the shipped DB resolves against re/hse_files, except the two
//! board-custom references the mirror set never carried (README:102).

use std::path::Path;

const CONTENT: &str = "../../re/mmud_wgnt.sqlite";
const HOUSES: &str = "../../re/hse_files";

/// Board-custom files known unrecoverable (the ODS fill-in set doesn't
/// have them); the rooms render descriptionless like the DLL's
/// sysop-error case.
const ALLOWLIST: &[&str] = &["RHUDAUR.ANS", "WCC40044.HSE"];

#[test]
fn every_file_description_room_resolves() {
    if !Path::new(CONTENT).exists() {
        eprintln!("skipping: {CONTENT} not present");
        return;
    }
    let mut content = mud_server::content_db::load(Path::new(CONTENT)).expect("content");
    let houses = mud_server::content_db::load_house_dir(Path::new(HOUSES)).expect("hse dir");
    assert!(houses.len() >= 130, "the mirror set is present");
    for (name, lines) in houses {
        assert!(!lines.is_empty(), "{name} parsed");
        content.add_house_text(&name, lines);
    }
    let mut missing = Vec::new();
    let mut checked = 0;
    for room in content.rooms.values() {
        let redirect = room
            .description
            .first()
            .is_some_and(|d| d.eq_ignore_ascii_case("FILE DESCRIPTION"));
        if !redirect {
            continue;
        }
        checked += 1;
        let file = room.description.get(1).cloned().unwrap_or_default();
        let key = file.to_uppercase();
        if !content.house_texts.contains_key(&key) && !ALLOWLIST.contains(&key.as_str()) {
            missing.push((room.id, file));
        }
    }
    assert!(checked >= 130, "the shipped sentinel rooms are present: {checked}");
    assert!(missing.is_empty(), "unresolved house files: {missing:?}");
}
