//! Integration test: load and validate the full shipped 1.11p content database.

use mud_core::ability::Ability;
use mud_core::content::{RaceId, RoomId};
use mud_server::content_db;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

#[test]
fn magic_missile_cast_fields_load_exactly() {
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, SpellId, TargetMode};
    let content = content_db::load(&db_path()).expect("load content db");
    let mm = &content.spells[&SpellId(1)];
    assert_eq!(mm.name, "magic missile");
    assert_eq!(mm.mana_cost, 1);
    assert_eq!(mm.required_power, 1);
    assert_eq!((mm.min_base, mm.max_base), (4, 12));
    assert_eq!(mm.round_cost, 1000);
    assert_eq!(mm.base_chance, 15);
    assert_eq!(mm.level_cap, 6);
    assert_eq!(mm.class_gate_group, 1);
    assert_eq!(mm.required_class_level, 1);
    assert_eq!(mm.target_mode, TargetMode::Offensive0);
    assert_eq!(mm.save_class, SaveClass::None);
    assert_eq!(mm.match_type, MatchType::Special8);
    assert_eq!(mm.element, Element::Magic);
    assert_eq!(mm.duration, 0);
    assert_eq!(mm.duration_per_level, 0);
    assert_eq!(mm.max_increase, ScalePair { per: 1, levels: 0 });
    assert_eq!(mm.min_increase, ScalePair::NONE);
    assert_eq!(mm.duration_increase, ScalePair::NONE);
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
    assert_eq!(rat.weapon, None);
    assert!(rat.loot.is_empty());

    // Loot slots: the thug (id 10) wields a spiked club (never dropped)
    // and carries a severed arm 1% of the time.
    let thug = &content.monsters[&mud_core::content::MonsterId(10)];
    assert_eq!(thug.weapon, Some(mud_core::content::ItemId(92)));
    assert_eq!(
        thug.loot,
        vec![mud_core::content::LootSlot {
            item: mud_core::content::ItemId(1356),
            uses: -1,
            dropper: 1,
        }]
    );

    // The one dangling loot ref shipped in the data (allowlisted): the
    // saracen commander's second slot names nonexistent item 2078.
    let saracen = &content.monsters[&mud_core::content::MonsterId(602)];
    assert_eq!(saracen.loot.len(), 2);
    assert_eq!(saracen.loot[1].item, mud_core::content::ItemId(2078));
    assert_eq!(rat.attacks[1].kind, 0); // empty slot

    // Item economy/combat fields: the quarterstaff (id 100).
    let staff = &content.items[&mud_core::content::ItemId(100)];
    assert_eq!(staff.name, "quarterstaff");
    assert_eq!(staff.weight, 100);
    assert_eq!(staff.item_type, 1); // weapon
    assert_eq!(staff.min_damage, 2);
    assert_eq!(staff.max_damage, 12);
    assert_eq!(staff.speed, 1200);
    assert_eq!(staff.cost, 0);
    assert_eq!(staff.weapon_type, 1, "1 or 3 = two-handed (oracle: '(Two handed)')");
    assert!(staff.hit_msg.is_some());

    // The newbie manual is placed at the Village Entrance but not gettable.
    let manual = &content.items[&mud_core::content::ItemId(1098)];
    assert_eq!(manual.name, "newbie manual");
    assert_eq!(manual.gettable, 0);
    let entrance = &content.rooms[&RoomId { map: 1, room: 2140 }];
    assert!(
        entrance.placed_items.iter().any(|p| p.item.0 == 1098),
        "manual placed at the entrance"
    );

    // Weapons shop stock (id 45): quarterstaff 31, club 26 (oracle list).
    let weapons = &content.shops[&mud_core::content::ShopId(45)];
    let slot = &weapons.stock[0];
    assert_eq!(slot.item.map(|i| i.0), Some(100));
    assert_eq!(slot.max, 31);
    assert_eq!(slot.now, 31);
    assert_eq!(weapons.stock[1].item.map(|i| i.0), Some(90));
    assert_eq!(weapons.stock[1].now, 26);

    // Every exit in the shipped data resolves (verified upstream: 62,352 exits).
    let exits: usize = content
        .rooms
        .values()
        .map(|r| r.exits.iter().flatten().count())
        .sum();
    assert_eq!(exits, 62352);
}
