//! M6 slice 5 — the world-population throttle, the single-limit respawn
//! cooldown, and the first-kill loot guarantee (generate_monster
//! 20981-21008 + item draws 21072-21097; check_kill_monster stamps).
//!
//! Extracted rules: a `gamelimit != 0` template refuses to spawn while
//! `active >= gamelimit`; a gamelimit-1 template with a kill stamp and a
//! nonzero `regentime` waits `base + lngrnd(0, base/4) - base/8` minutes
//! (base = regentime*60) before returning; a limited template that has
//! NEVER been killed carries its full loadout with no drop rolls.

use mud_core::content::{
    Class, ClassId, Content, ItemId, LootSlot, Monster, MonsterId, Race, RaceId, Room, RoomId,
    StatBlock,
};
use mud_core::game::{Core, CoreConfig, Gender, Player};

const DEN_A: RoomId = RoomId { map: 1, room: 1 };
const DEN_B: RoomId = RoomId { map: 1, room: 2 };

fn base_content() -> Content {
    let mut content = Content::default();
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
        abilities: vec![],
        hp_per_level: 6,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 6,
        weapon_code: 8,
        armour_code: 9,
    });
    for i in 1..=3u16 {
        content.add_item(mud_core::content::Item {
            id: ItemId(i),
            name: format!("relic {i}"),
            uses: -1,
            ..Default::default()
        });
    }
    content
}

fn spawn_room(id: RoomId) -> Room {
    Room {
        id,
        name: format!("den {}", id.room),
        room_type: 2, // 89%/kick, respawn-timer exempt
        spawn_zone: 7,
        spawn_cap: 10,
        min_level: 1,
        max_level: 5,
        ..Default::default()
    }
}

/// A limited template: zone 7 level 3, `game_limit`/`unique_cooldown` per
/// test, three 1%-dropper relics.
fn rare(game_limit: i16, cooldown: i16) -> Monster {
    Monster {
        id: MonsterId(9),
        name: "basilisk".into(),
        hitpoints: 10,
        energy: 1000,
        magic_resist: 0,
        roam_class: 7,
        level: 3,
        behaviour: 0,
        game_limit,
        unique_cooldown: cooldown,
        loot: vec![
            LootSlot { item: ItemId(1), uses: -1, dropper: 1 },
            LootSlot { item: ItemId(2), uses: -1, dropper: 1 },
            LootSlot { item: ItemId(3), uses: -1, dropper: 1 },
        ],
        ..Default::default()
    }
}

fn player_at(name: &str, location: RoomId) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        current_hp: 400,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location,
        ..Default::default()
    }
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: DEN_A,
        ..CoreConfig::default()
    }
}

#[test]
fn gamelimit_caps_the_world_population() {
    let mut content = base_content();
    content.add_room(spawn_room(DEN_A));
    content.add_room(spawn_room(DEN_B));
    content.add_monster(rare(2, 0));
    let mut core = Core::new(content, config());
    // Drive generates directly: the third refuses at the world cap.
    assert!(core.debug_generate(DEN_A));
    assert!(core.debug_generate(DEN_B));
    assert!(!core.debug_generate(DEN_A), "active 2 >= gamelimit 2 refuses");
    // A kill releases a slot (active decrements)...
    let victim = core.monster_ids()[0];
    core.debug_kill_monster(victim);
    // ...but DEN_A now carries a kill stamp in its rooms; type-2 rooms
    // bypass the timer, so the release is immediately usable.
    assert!(core.debug_generate(DEN_A), "the freed slot spawns again");
}

#[test]
fn single_limit_cooldown_blocks_then_allows() {
    // gamelimit 1 + regentime 1: base 60 min. Refused before
    // base - base/8 = 52.5 min; guaranteed past base + base/4 - base/8
    // = 67.5 min (jitter upper bound).
    let mut content = base_content();
    content.add_room(spawn_room(DEN_A));
    content.add_monster(rare(1, 1));
    let mut core = Core::new(content, config());
    assert!(core.debug_generate(DEN_A));
    let victim = core.monster_ids()[0];
    core.debug_kill_monster(victim);
    core.drain_events();
    // Just after the kill: hard refusal.
    assert!(!core.debug_generate(DEN_A), "fresh kill refuses");
    // Advance 50 minutes — still inside the guaranteed-refusal window.
    for _ in 0..(50 * 60) {
        core.tick();
    }
    core.drain_events();
    assert!(!core.debug_generate(DEN_A), "50 min < 52.5 min floor");
    // Advance to 70 minutes total — past the worst-case jitter.
    for _ in 0..(20 * 60) {
        core.tick();
    }
    core.drain_events();
    assert!(core.debug_generate(DEN_A), "70 min > 67.5 min ceiling");
}

#[test]
fn first_kill_carries_the_full_loadout_then_rolls() {
    let mut content = base_content();
    content.add_room(spawn_room(DEN_A));
    content.add_monster(rare(1, 0)); // no cooldown: refills freely
    let mut core = Core::new(content, config());
    let _watcher = core.attach_player(player_at("Looter", DEN_A));
    core.drain_events();
    assert!(core.debug_generate(DEN_A));
    let first = core.monster_ids()[0];
    // Never killed: every 1%-dropper slot is carried with NO draws.
    core.debug_kill_monster(first);
    core.drain_events();
    let floor = core.debug_room_items(DEN_A);
    assert_eq!(floor.len(), 3, "first kill drops the whole loadout: {floor:?}");
    // The next spawn rolls the 1% droppers — under seed, none carry.
    assert!(core.debug_generate(DEN_A));
    let second = core.monster_ids()[0];
    core.debug_kill_monster(second);
    core.drain_events();
    let floor = core.debug_room_items(DEN_A);
    assert_eq!(floor.len(), 3, "no new items joined the pile: {floor:?}");
}

#[test]
fn restored_cooldown_survives_a_restart() {
    // A kill 10 minutes ago (restored through CoreConfig) still blocks a
    // gamelimit-1 regentime-1 template at boot and after.
    let mut content = base_content();
    content.add_room(spawn_room(DEN_A));
    content.add_monster(rare(1, 1));
    let mut core = Core::new(
        content,
        CoreConfig {
            start_location: DEN_A,
            restored_population: vec![(MonsterId(9), 10 * 60)],
            ..CoreConfig::default()
        },
    );
    assert!(
        !core.debug_generate(DEN_A),
        "a 10-minute-old kill still cools down across the restart"
    );
}

#[test]
fn restored_room_stamp_survives_a_restart() {
    // A type-0 room killed 1 minute ago refuses through the restored
    // stamp (delay 5 min default).
    let mut content = base_content();
    let mut r = spawn_room(DEN_A);
    r.room_type = 0; // honors the timer
    content.add_room(r);
    content.add_monster(rare(0, 0));
    let mut core = Core::new(
        content,
        CoreConfig {
            start_location: DEN_A,
            restored_room_stamps: vec![(DEN_A, 60)],
            ..CoreConfig::default()
        },
    );
    assert!(!core.debug_generate(DEN_A), "the restart does not reset the window");
}
