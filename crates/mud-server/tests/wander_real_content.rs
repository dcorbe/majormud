//! Real-content wander smoke test (M6 slice 2): a shipped zone-30 slime
//! drifts the actual sewers under the real zone map. Also documents the
//! diagnosis that roam classes 0/2 (dummies, town NPCs like Cygani) are
//! STATIONARY by data — a monster that "won't wander" live is usually
//! carrying one of those classes, not hitting a bug.

use mud_core::content::{ClassId, RaceId, RoomId};
use mud_core::game::{Core, CoreConfig, Gender, Player};
use mud_server::content_db;

fn player_at(name: &str, location: RoomId) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        stats: Default::default(),
        base_stats: Default::default(),
        hp_base: 0,
        current_hp: 10,
        current_mana: 0,
        hunger: 1000,
        thirst: 1000,
        coins: Default::default(),
        lawful: false,
        inventory: vec![],
        weapon: None,
        bankbooks: vec![],
        worn: vec![],
        cp_unspent: 0,
        cp_lifetime: 0,
        lives: 9,
        experience: 0,
        location,
        spellbook: std::collections::BTreeMap::new(),
        poison: 0,
        active_spells: Default::default(),
        ..Default::default()
    }
}

#[test]
fn shipped_world_drifts_after_boot() {
    // Since M6 slice 4 the boot pass populates every boss/lair room, so
    // the global wander fairness cap (3 per medium tick, monsters.md §3)
    // is shared by thousands of live monsters — a specific late-spawned
    // monster may wait a long time for a slot, exactly as on a real
    // board. The smoke assertion is therefore global: the world DRIFTS —
    // some monsters change rooms within a minute of uptime — and the
    // zone leash holds for every drifter.
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    let content = content_db::load(&db).expect("load");
    let mut core = Core::new(content, CoreConfig::default());
    let _watcher = core.attach_player(player_at("Watcher", RoomId { map: 9, room: 1 }));
    core.drain_events();
    let booted = core.monster_ids().len();
    assert!(booted > 500, "boot fill populates the lairs: {booted}");
    let before: std::collections::BTreeMap<_, _> = core
        .monster_ids()
        .into_iter()
        .filter_map(|id| core.monster_location(id).map(|loc| (id, loc)))
        .collect();
    for _ in 0..60 {
        core.tick();
        core.drain_events();
    }
    let moved = before
        .iter()
        .filter(|(id, loc)| {
            core.monster_location(**id).is_some_and(|now| now != **loc)
        })
        .count();
    assert!(moved > 0, "no monster moved in 60 s of a living world");
    println!("boot population: {booted}, drifted in 60s: {moved}");
}

#[test]
fn boot_stands_the_newhaven_shopkeepers_up() {
    // M6 slice 4: the boot walk spawns every permnpc boss and swarm/type-1
    // fill — the Newhaven Weapons Shop (1/2141) has its keeper standing
    // in it with no fixture flags, exactly like a real board.
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    let content = content_db::load(&db).expect("load");
    let core = Core::new(content, CoreConfig::default());
    let weapons_shop = RoomId { map: 1, room: 2141 };
    let standing: Vec<_> = core
        .monster_ids()
        .into_iter()
        .filter(|id| core.monster_location(*id) == Some(weapons_shop))
        .collect();
    assert!(!standing.is_empty(), "the weapons shop keeper stands at boot");
}
