//! Real-content wander smoke test (M6 slice 2): a shipped zone-30 slime
//! drifts the actual sewers under the real zone map. Also documents the
//! diagnosis that roam classes 0/2 (dummies, town NPCs like Cygani) are
//! STATIONARY by data — a monster that "won't wander" live is usually
//! carrying one of those classes, not hitting a bug.

use mud_core::content::{ClassId, MonsterId, RaceId, RoomId};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player};
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
    }
}

#[test]
fn shipped_slime_wanders_the_real_sewers() {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    let content = content_db::load(&db).expect("load");
    let mut core = Core::new(content, CoreConfig::default());
    let start = RoomId { map: 9, room: 1 };
    let watcher = core.attach_player(player_at("Watcher", start));
    let m = core.spawn_monster(MonsterId(467), start).expect("spawn purple slime");
    core.drain_events();
    let mut heard = String::new();
    for t in 1..=300u32 {
        core.tick();
        for e in core.drain_events() {
            if let Event::Output { session, text } = e
                && session == watcher
            {
                heard.push_str(&text);
            }
        }
        let loc = core.monster_location(m);
        if loc != Some(start) {
            println!("purple slime moved at tick {t} to {loc:?}");
            println!("watcher heard: {heard:?}");
            assert!(
                heard.contains("purple slime just left"),
                "departure line missing: {heard:?}"
            );
            return;
        }
    }
    panic!("purple slime never left 9/1 in 300 ticks; heard: {heard:?}");
}
