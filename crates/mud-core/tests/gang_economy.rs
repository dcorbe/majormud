//! Gang economy (gangs.md §2.1, §5.1): the per-kill exp-pool feed and
//! TOP n GANGS.

use mud_core::content::{
    AttackForm, Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::gang::Gang;
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const ARENA: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: ARENA,
        name: "Arena".into(),
        ..Default::default()
    });
    content.add_monster(Monster {
        id: MonsterId(1),
        name: "giant rat".into(),
        hitpoints: 1,
        experience: 9,
        exp_multi: 1,
        energy: 1000,
        attacks: [AttackForm::default(); 5],
        behaviour: 3,
        ..Default::default()
    });
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 30,
        },
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
    content
}

fn person(name: &str, gang: &str) -> Player {
    let stats = StatBlock {
        intellect: 30,
        wisdom: 50,
        strength: 50,
        health: 50,
        agility: 30,
        charm: 30,
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 5,
        stats,
        base_stats: stats,
        current_hp: 40,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: ARENA,
        gang: gang.into(),
        ..Default::default()
    }
}

fn kill_rat(core: &mut Core, s: SessionId) -> Vec<Event> {
    let mut all = Vec::new();
    core.input(s, "attack rat");
    all.extend(core.drain_events());
    for _ in 0..100 {
        if all.iter().any(|e| matches!(e, Event::Output { text, .. } if text.contains("is dead"))) {
            break;
        }
        core.tick();
        all.extend(core.drain_events());
    }
    all
}

fn texts(events: &[Event], who: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session, text } if *session == who => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// gangs.md §2.1 (award block 11154): a gang member's kill share is
/// also added to the gang pool; a gangless killer feeds nothing.
#[test]
fn kill_share_feeds_the_gang_pool() {
    let config = CoreConfig {
        restored_gangs: vec![Gang::new("Iron Fist", "Salad", 0)],
        restored_gang_members: vec![("Salad".into(), "Iron Fist".into(), 0)],
        start_location: ARENA,
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let s = core.attach_player(person("Salad", "Iron Fist"));
    core.spawn_monster(MonsterId(1), ARENA);
    core.drain_events();
    let events = kill_rat(&mut core, s);
    assert_eq!(core.gang("Iron Fist").unwrap().exp_pool, 9, "the full share");
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistGang(g) if g.exp_pool == 9)),
        "pool persisted"
    );
}

#[test]
fn gangless_kill_feeds_nothing() {
    let config = CoreConfig {
        restored_gangs: vec![Gang::new("Iron Fist", "Salad", 0)],
        start_location: ARENA,
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let s = core.attach_player(person("Loner", ""));
    core.spawn_monster(MonsterId(1), ARENA);
    core.drain_events();
    kill_rat(&mut core, s);
    assert_eq!(core.gang("Iron Fist").unwrap().exp_pool, 0);
}

/// The saturation path rides the same live feed (§2.1).
#[test]
fn kill_share_saturates_live() {
    let mut gang = Gang::new("Iron Fist", "Salad", 0);
    gang.exp_pool = u32::MAX - 4;
    let config = CoreConfig {
        restored_gangs: vec![gang],
        restored_gang_members: vec![("Salad".into(), "Iron Fist".into(), 0)],
        start_location: ARENA,
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let s = core.attach_player(person("Salad", "Iron Fist"));
    core.spawn_monster(MonsterId(1), ARENA);
    core.drain_events();
    kill_rat(&mut core, s);
    let g = core.gang("Iron Fist").unwrap();
    assert_eq!(g.exp_pool, u32::MAX);
    assert_eq!(g.secondary_pool, 5, "excess = award - (MAX - pool) = 9 - 4");
    assert!(g.is_saturated());
}

// --- §5.1 TOP n GANGS ---

fn top_world() -> Core {
    let mut gangs = Vec::new();
    for (name, leader, exp, count, created) in [
        ("Alphas", "Aldo", 500u32, 3u16, 0i64),
        ("Bravos", "Brin", 9_000, 5, 86_400 * 20_300),
        ("Middle", "Mona", 700, 2, 86_400 * 20_301),
    ] {
        let mut g = Gang::new(name, leader, created);
        g.exp_pool = exp;
        g.member_count = count;
        gangs.push(g);
    }
    // A disbanded and a hidden gang never rank.
    let mut dead = Gang::new("Dead", "Gone", 0);
    dead.flags |= mud_core::gang::GANG_DISBANDED;
    dead.exp_pool = 99_999;
    gangs.push(dead);
    let mut hidden = Gang::new("Hidden", "Shy", 0);
    hidden.flags |= mud_core::gang::GANG_HIDDEN;
    hidden.exp_pool = 99_999;
    gangs.push(hidden);
    let config = CoreConfig {
        restored_gangs: gangs,
        start_location: ARENA,
        ..CoreConfig::default()
    };
    Core::new(world(), config)
}

#[test]
fn top_gangs_ranks_by_exp_descending() {
    let mut core = top_world();
    let s = core.attach_player(person("Watcher", ""));
    core.drain_events();
    core.input(s, "top gangs");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Top Gangs of the Realm"), "{out:?}");
    assert!(out.contains("Rank Gangname            Leader      Members Created"), "{out:?}");
    let b = out.find("Bravos").expect("Bravos listed");
    let m = out.find("Middle").expect("Middle listed");
    let a = out.find("Alphas").expect("Alphas listed");
    assert!(b < m && m < a, "exp-descending: {out:?}");
    assert!(!out.contains("Dead"), "disbanded skipped: {out:?}");
    assert!(!out.contains("Hidden"), "flag-4 skipped: {out:?}");
    // The row shape: "  1. Bravos              Brin              5    ..."
    assert!(out.contains("  1. Bravos"), "{out:?}");
}

#[test]
fn top_n_gangs_limits_and_caps() {
    let mut core = top_world();
    let s = core.attach_player(person("Watcher", ""));
    core.drain_events();
    core.input(s, "top 1 gangs");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Bravos") && !out.contains("Middle"), "{out:?}");
    // The reversed form parses too.
    core.input(s, "top gangs 2");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Middle") && !out.contains("Alphas"), "{out:?}");
}

#[test]
fn top_gangs_empty_realm() {
    let mut core = Core::new(world(), CoreConfig { start_location: ARENA, ..CoreConfig::default() });
    let s = core.attach_player(person("Watcher", ""));
    core.drain_events();
    core.input(s, "top gangs");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("There are no gangs currently established!"), "{out:?}");
}

#[test]
fn bare_top_stays_the_player_stub() {
    let mut core = top_world();
    let s = core.attach_player(person("Watcher", ""));
    core.drain_events();
    core.input(s, "top");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Top Heroes of the Realm"), "{out:?}");
    assert!(!out.contains("Gangname"), "{out:?}");
}
