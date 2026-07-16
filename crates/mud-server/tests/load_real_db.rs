//! Integration test: load and validate the full shipped 1.11p content database.

use mud_core::ability::Ability;
use mud_core::content::{RaceId, RoomId};
use mud_server::content_db;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

#[test]
fn full_database_loads_and_validates() {
    let content = content_db::load(&db_path()).expect("load content db");

    assert_eq!(content.rooms.len(), 26720);
    assert_eq!(content.monsters.len(), 1101);
    assert_eq!(content.spells.len(), 1379);
    assert_eq!(content.items.len(), 1950);
    assert_eq!(content.messages.len(), 3867);
    assert_eq!(content.shops.len(), 178);
    assert_eq!(content.races.len(), 13);
    assert_eq!(content.classes.len(), 15);

    assert_eq!(content.validate(), vec![]);
}

#[test]
fn known_content_spot_checks() {
    let content = content_db::load(&db_path()).expect("load content db");

    let gates = &content.rooms[&RoomId { map: 1, room: 1 }];
    assert_eq!(gates.name, "Town Gates");
    assert!(gates.exits.iter().flatten().count() > 0);

    let dwarf = content
        .races
        .values()
        .find(|r| r.name == "Dwarf")
        .expect("Dwarf race");
    assert_eq!(dwarf.id, RaceId(2));
    assert!(
        dwarf
            .abilities
            .contains(&(Ability::from_id(13).unwrap(), 75)),
        "Dwarf has Illu 75 (infravision), got {:?}",
        dwarf.abilities
    );

    let classes: Vec<&str> = content.classes.values().map(|c| c.name.as_str()).collect();
    assert_eq!(classes[0], "Warrior");
    assert!(classes.contains(&"Mystic"));

    // Every exit in the shipped data resolves (verified upstream: 62,352 exits).
    let exits: usize = content
        .rooms
        .values()
        .map(|r| r.exits.iter().flatten().count())
        .sum();
    assert_eq!(exits, 62352);
}
