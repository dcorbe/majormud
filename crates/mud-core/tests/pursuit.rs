//! M6 slice 3 — the 1 s pursuit tier (`fast_update_monster` decompile
//! 19393-19489, `dir_player_travelling_coord` 15657-15686). A locked
//! monster chases its target along the player's movement breadcrumb, one
//! step per fast tick, gated by the follow roll `genrdn(0,100) <
//! aggression`; every unprosecutable tick bumps the give-up counter, and
//! past 15 the lock drops (roam class 0x25 silently despawns instead).

use mud_core::content::{
    AttackForm, Class, ClassId, Content, Direction, Exit, Monster, MonsterId, Race, RaceId, Room,
    RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };
const B: RoomId = RoomId { map: 1, room: 2 };
const C: RoomId = RoomId { map: 1, room: 3 };
/// A room on another map — pursuit never crosses maps.
const FAR: RoomId = RoomId { map: 2, room: 1 };

fn melee_form() -> AttackForm {
    AttackForm {
        kind: 1,
        accuracy: 100,
        weight: 100,
        min_damage: 1,
        max_damage: 2,
        energy: 200,
        ..Default::default()
    }
}

fn monster(id: u16, behaviour: i16, aggression: i16) -> Monster {
    Monster {
        id: MonsterId(id),
        name: format!("hunter {id}"),
        hitpoints: 500,
        energy: 1000,
        magic_resist: 0,
        behaviour,
        aggression,
        attacks: [
            melee_form(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
        ..Default::default()
    }
}

fn plain_exit(dest: RoomId) -> Option<Exit> {
    Some(Exit {
        dest,
        exit_type: 0,
        ..Default::default()
    })
}

/// A corridor A -north-> B -north-> C, all zone 0; FAR sits on map 2 off
/// C's east (a map-change portal type 8 for the player's escape).
fn world() -> Content {
    let mut content = Content::default();
    let mut a = Room {
        id: A,
        name: "Gate".into(),
        ..Default::default()
    };
    a.exits[Direction::North as usize] = plain_exit(B);
    let mut b = Room {
        id: B,
        name: "Hall".into(),
        ..Default::default()
    };
    b.exits[Direction::South as usize] = plain_exit(A);
    b.exits[Direction::North as usize] = plain_exit(C);
    let mut c = Room {
        id: C,
        name: "Sanctum".into(),
        ..Default::default()
    };
    c.exits[Direction::South as usize] = plain_exit(B);
    c.exits[Direction::East as usize] = Some(Exit {
        dest: FAR,
        exit_type: 8,
        ..Default::default()
    });
    let far = Room {
        id: FAR,
        name: "Elsewhere".into(),
        ..Default::default()
    };
    content.add_room(a);
    content.add_room(b);
    content.add_room(c);
    content.add_room(far);
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
    content
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
        start_location: A,
        ..CoreConfig::default()
    }
}

fn text_to(events: &[Event], session: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == session => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Lock the monster on the player by letting it attack once (aggression
/// 100 makes the post-swing lock re-roll land), then return.
fn lock(core: &mut Core, m: mud_core::game::MonsterInstanceId, s: SessionId) {
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.monster_target(m), Some(s), "fixture lock");
}

#[test]
fn locked_monster_chases_along_the_breadcrumb() {
    let mut content = world();
    content.add_monster(monster(1, 2, 100));
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Runner", A));
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    lock(&mut core, m, s);
    core.input(s, "n"); // A -> B (free attack may swing; survivable)
    core.drain_events();
    // moved_this_round shields the runner on the immediate fast tick;
    // the medium/energy clear frees the chase within ~3 ticks, and the
    // hunter steps to B on the next fast tick after that.
    let mut caught_up = 0;
    for t in 1..=8 {
        core.tick();
        core.drain_events();
        if core.monster_location(m) == Some(B) {
            caught_up = t;
            break;
        }
    }
    assert!(
        (1..=5).contains(&caught_up),
        "chase lands once the moved-flag clears (the medium tick may fall
         in the same second as the move), tick {caught_up}"
    );
}

#[test]
fn pursuit_prints_the_wander_line_family() {
    let mut content = world();
    content.add_monster(monster(1, 2, 100));
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Runner", A));
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    lock(&mut core, m, s);
    core.input(s, "n");
    core.drain_events();
    let mut seen = String::new();
    for _ in 0..8 {
        core.tick();
        seen.push_str(&text_to(&core.drain_events(), s));
        if core.monster_location(m) == Some(B) {
            break;
        }
    }
    assert!(
        seen.to_lowercase().contains("hunter 1 moves into the room from the south."),
        "pursuit arrival uses the moves-into family: {seen:?}"
    );
}

#[test]
fn map_change_breaks_the_chase_and_the_lock_gives_up() {
    // A different map can't be chased (19418-19423); every tick bumps
    // give-up until > 15 clears the lock (19446-19453).
    let mut content = world();
    content.add_monster(monster(1, 2, 100));
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Runner", C));
    let m = core.spawn_monster(MonsterId(1), C).unwrap();
    core.drain_events();
    lock(&mut core, m, s);
    core.input(s, "e"); // through the portal to map 2
    core.drain_events();
    assert_eq!(core.player_snapshot(s).location, FAR);
    for _ in 0..20 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.monster_location(m), Some(C), "no cross-map pursuit");
    assert_eq!(core.monster_target(m), None, "gave up after 16 dry ticks");
}

#[test]
fn roam_0x25_despawns_when_it_gives_up() {
    let mut content = world();
    let mut angel = monster(1, 2, 100);
    angel.roam_class = 0x25;
    content.add_monster(angel);
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Runner", C));
    let m = core.spawn_monster(MonsterId(1), C).unwrap();
    // Combat never locks a class 0x25 (the re-roll and retaliation both
    // skip it) — its locks are written at summon creation. Pre-lock like
    // generate_monster's param_7 does.
    core.debug_lock_monster(m, s);
    core.drain_events();
    core.input(s, "e");
    core.drain_events();
    for _ in 0..20 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.monster_hp(m), None, "class 0x25 silently despawns");
}

#[test]
fn zero_aggression_lock_gives_up_instead_of_chasing() {
    // The follow roll genrdn(0,100) < aggression never passes at 0; each
    // failure bumps the counter — the monster never leaves and the lock
    // dies of old age.
    let mut content = world();
    content.add_monster(monster(1, 3, 0)); // passive: locks without a roll
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Prodder", A));
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    core.input(s, "attack hunter");
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.monster_target(m), Some(s), "passive retaliation lock");
    core.input(s, "n");
    core.drain_events();
    for _ in 0..20 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.monster_location(m), Some(A), "no follow at aggression 0");
    assert_eq!(core.monster_target(m), None, "the lock expired");
}

#[test]
fn logged_off_target_expires_the_lock() {
    // A stale lock (the DLL keeps the NAME of a departed player) bumps
    // give-up every fast tick until it clears — there is no logout hook.
    let mut content = world();
    content.add_monster(monster(1, 2, 100));
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Quitter", A));
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    lock(&mut core, m, s);
    core.detach(s);
    for _ in 0..20 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.monster_target(m), None, "stale lock aged out");
    assert!(core.monster_hp(m).is_some(), "ordinary classes stay");
}

#[test]
fn two_room_chase_follows_the_full_trail() {
    // The breadcrumb walk (15657-15686): the monster finds its own room
    // in the trail and steps toward the room the player entered NEXT —
    // repeated fast ticks walk the whole path A -> B -> C.
    let mut content = world();
    content.add_monster(monster(1, 2, 100));
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Runner", A));
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    lock(&mut core, m, s);
    core.input(s, "n");
    core.drain_events();
    core.input(s, "n"); // A -> B -> C back to back
    core.drain_events();
    let mut reached = false;
    for _ in 0..12 {
        core.tick();
        core.drain_events();
        if core.monster_location(m) == Some(C) {
            reached = true;
            break;
        }
    }
    assert!(reached, "the hunter walks the breadcrumb to the sanctum");
}
