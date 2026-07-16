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
    assert!(
        gates.description[0].starts_with("You are before the massive town gates of Silvermere."),
        "got: {:?}",
        gates.description
    );

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
    assert_eq!(
        (dwarf.base_stats.intellect, dwarf.base_stats.wisdom, dwarf.base_stats.strength),
        (30, 50, 50)
    );
    assert_eq!((dwarf.max_stats.health, dwarf.max_stats.charm), (120, 85));
    assert_eq!(dwarf.cp, 100);

    let classes: Vec<&str> = content.classes.values().map(|c| c.name.as_str()).collect();
    assert_eq!(classes[0], "Warrior");
    assert!(classes.contains(&"Mystic"));

    // Monster combat fields: the giant rat (id 1).
    let rat = &content.monsters[&mud_core::content::MonsterId(1)];
    assert_eq!(rat.name, "giant rat");
    assert_eq!(rat.hitpoints, 12);
    assert_eq!(rat.experience, 9);
    assert_eq!(rat.exp_multi, 1);
    assert_eq!(rat.armour_class, 0);
    assert_eq!(rat.damage_resist, 1);
    assert_eq!(rat.magic_resist, 30);
    assert_eq!(rat.energy, 1000);
    let form = &rat.attacks[0];
    assert_eq!(form.kind, 1); // melee
    assert_eq!(form.accuracy, 10);
    assert_eq!(form.weight, 100);
    assert_eq!(form.min_damage, 2);
    assert_eq!(form.max_damage, 10);
    assert_eq!(form.energy, 1000);
    assert_eq!(rat.attacks[1].kind, 0); // empty slot

    // Every exit in the shipped data resolves (verified upstream: 62,352 exits).
    let exits: usize = content
        .rooms
        .values()
        .map(|r| r.exits.iter().flatten().count())
        .sum();
    assert_eq!(exits, 62352);
}
