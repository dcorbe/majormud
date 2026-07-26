//! M6 slice 2 — wander & leash (monsters.md §3, §5-medium).
//!
//! Fixture-spawned monsters drift on the 3 s medium tick through
//! `move_monster`'s gate ladder: roam-class switch, wander roll
//! `genrdn(0,100) < (100-aggression)/2`, fairness cap 3/tick, zone leash,
//! herd modes, door/exit-type gating, no immediate backtracking.

use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Message, MessageId, Monster, MonsterId, Race, RaceId,
    Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };
const B: RoomId = RoomId { map: 1, room: 2 };
const C: RoomId = RoomId { map: 1, room: 3 };

fn room(id: RoomId, zone: i16) -> Room {
    Room {
        id,
        name: format!("room {}", id.room),
        spawn_zone: zone,
        ..Default::default()
    }
}

fn exit(dest: RoomId, exit_type: u16) -> Option<Exit> {
    Some(Exit {
        dest,
        exit_type,
        trigger_msg: None,
        param: 0,
        param2: 0,
        param3: 0,
        param4: 0,
        door_closed: exit_type == 2, // shipped doors are closed (para2=2)
    })
}

fn monster(id: u16, roam: i16, aggression: i16) -> Monster {
    Monster {
        id: MonsterId(id),
        name: format!("beast {id}"),
        hitpoints: 50,
        energy: 1000,
        magic_resist: 0,
        roam_class: roam,
        aggression,
        // Lair mode: attackable without the mode-0/4 evil charge
        // (created characters ship Warn on Evil ON — crime.md §2.5).
        behaviour: 3,
        ..Default::default()
    }
}

/// A/B share zone 9 (A -north-> B, B -south-> A); C is zone 8 off A's east.
fn world() -> Content {
    let mut content = Content::default();
    let mut a = room(A, 9);
    a.exits[Direction::North as usize] = exit(B, 0);
    a.exits[Direction::East as usize] = exit(C, 0);
    let mut b = room(B, 9);
    b.exits[Direction::South as usize] = exit(A, 0);
    let mut c = room(C, 8);
    c.exits[Direction::West as usize] = exit(A, 0);
    content.add_room(a);
    content.add_room(b);
    content.add_room(c);
    content.add_race(Race {
        id: RaceId(2),
        name: "Dwarf".into(),
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

fn config() -> CoreConfig {
    CoreConfig {
        start_location: A,
        ..CoreConfig::default()
    }
}

fn create(core: &mut Core, name: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
        saved_evil: 0,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.input(s, "No");
    core.drain_events();
    s
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

fn run_ticks(core: &mut Core, n: u32) {
    for _ in 0..n {
        core.tick();
    }
    core.drain_events();
}

#[test]
fn stationary_roam_classes_never_wander() {
    // §3: roam-class 0 and 2 are the stationary cases of the wander switch.
    let mut content = world();
    content.add_monster(monster(1, 0, 0));
    content.add_monster(monster(2, 2, 0));
    let mut core = Core::new(content, config());
    let m0 = core.spawn_monster(MonsterId(1), A).unwrap();
    let m2 = core.spawn_monster(MonsterId(2), A).unwrap();
    run_ticks(&mut core, 300);
    assert_eq!(core.monster_location(m0), Some(A));
    assert_eq!(core.monster_location(m2), Some(A));
}

#[test]
fn lair_monster_never_leaves() {
    // §3 gate 3: herd mode 3 = lair, fully stationary even with a valid zone.
    let mut content = world();
    let mut lair = monster(1, 9, 0);
    lair.herd_mode = 3;
    content.add_monster(lair);
    let mut core = Core::new(content, config());
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    run_ticks(&mut core, 300);
    assert_eq!(core.monster_location(m), Some(A));
}

#[test]
fn zone_wanderer_moves_but_respects_the_leash() {
    // §3 gate 4: zone 9 wanderer drifts A<->B but never enters zone-8 C.
    let mut content = world();
    content.add_monster(monster(1, 9, 0));
    let mut core = Core::new(content, config());
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut visited_b = false;
    for _ in 0..600 {
        core.tick();
        core.drain_events();
        let loc = core.monster_location(m).unwrap();
        assert_ne!(loc, C, "zone leash must keep the wanderer out of zone 8");
        visited_b |= loc == B;
    }
    assert!(visited_b, "an aggression-0 wanderer (50% per medium tick) moves");
}

#[test]
fn max_aggression_monster_never_wanders() {
    // §3: wander chance is (100 - aggression)/2 — at 100 the roll
    // genrdn(0,100) < 0 can never pass.
    let mut content = world();
    content.add_monster(monster(1, 9, 100));
    let mut core = Core::new(content, config());
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    run_ticks(&mut core, 600);
    assert_eq!(core.monster_location(m), Some(A));
}

#[test]
fn wander_broadcasts_departure_and_arrival_lines() {
    // Oracle: "big guardsman just left to the east." (bare name) and
    // "happy guardsman moves into the room from the east." (bare name;
    // wander arrivals use "moves into", NOT the spawn line's "walks into").
    let mut content = world();
    content.add_monster(monster(1, 9, 0));
    let mut core = Core::new(content, config());
    let watcher_a = create(&mut core, "Alice");
    let watcher_b = create(&mut core, "Bob");
    core.input(watcher_b, "n"); // Bob observes from B
    core.drain_events();
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut saw_leave = String::new();
    let mut saw_arrive = String::new();
    for _ in 0..300 {
        core.tick();
        let events = core.drain_events();
        saw_leave.push_str(&text_to(&events, watcher_a));
        saw_arrive.push_str(&text_to(&events, watcher_b));
        if core.monster_location(m) == Some(B) {
            break;
        }
    }
    assert!(
        saw_leave.contains("beast 1 just left to the north."),
        "departure line: {saw_leave:?}"
    );
    assert!(
        saw_arrive.contains("beast 1 moves into the room from the south."),
        "arrival line: {saw_arrive:?}"
    );
}

#[test]
fn same_direction_twice_needs_a_slow_reset_between() {
    // move_monster stores the moved direction in mon+0x132 and the wander
    // pick is rejected when it EQUALS it — no two consecutive steps in
    // the same direction until the 30 s slow tick clears the memory
    // (decompile 19357/21550/19274; ping-ponging back is allowed). In a
    // north corridor A->B->C, reaching C requires two norths, so a slow
    // boundary must fall while the monster waits in B.
    let mut content = Content::default();
    let mut a = room(A, 9);
    a.exits[Direction::North as usize] = exit(B, 0);
    let mut b = room(B, 9);
    b.exits[Direction::South as usize] = exit(A, 0);
    b.exits[Direction::North as usize] = exit(C, 0);
    let mut c = room(C, 9);
    c.exits[Direction::South as usize] = exit(B, 0);
    content.add_room(a);
    content.add_room(b);
    content.add_room(c);
    content.add_monster(monster(1, 9, 0));
    let mut core = Core::new(content, config());
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut arrived_b_at: Option<u32> = None;
    let mut reached_c = false;
    let mut prev = A;
    for t in 1..=1800u32 {
        core.tick();
        core.drain_events();
        let loc = core.monster_location(m).unwrap();
        if loc != prev {
            if loc == B && prev == A {
                arrived_b_at = Some(t);
            }
            if loc == C {
                let entered_b = arrived_b_at.expect("must pass through B");
                assert!(
                    (entered_b..t).any(|x| x % 30 == 0),
                    "second north step at {t} without a slow reset since {entered_b}"
                );
                reached_c = true;
                break;
            }
            prev = loc;
        }
    }
    assert!(reached_c, "the corridor is passable across slow windows");
}

#[test]
fn closed_door_blocks_ordinary_wanderers_but_not_class_0x26() {
    // §3 gate 5: type-2 exits with the door closed block everything except
    // roam class 0x26 (the door-opening free roamer).
    let mut content = Content::default();
    let mut a = room(A, 9);
    a.exits[Direction::North as usize] = exit(B, 2);
    let mut b = room(B, 9);
    b.exits[Direction::South as usize] = exit(A, 2);
    content.add_room(a);
    content.add_room(b);
    content.add_monster(monster(1, 9, 0));
    content.add_monster(monster(2, 0x26, 0));
    let mut core = Core::new(content, config());
    let ordinary = core.spawn_monster(MonsterId(1), A).unwrap();
    let roamer = core.spawn_monster(MonsterId(2), A).unwrap();
    let mut roamer_moved = false;
    for _ in 0..600 {
        core.tick();
        core.drain_events();
        assert_eq!(
            core.monster_location(ordinary),
            Some(A),
            "ordinary monsters do not open doors"
        );
        roamer_moved |= core.monster_location(roamer) != Some(A);
    }
    assert!(roamer_moved, "class 0x26 passes closed doors");
}

#[test]
fn free_roamer_0x25_needs_flagless_rooms() {
    // §3 gate 4: class 0x25 enters only rooms whose attributes word is 0
    // (and non-type-5), regardless of zone.
    let mut content = Content::default();
    let mut a = room(A, 1);
    a.exits[Direction::North as usize] = exit(B, 0);
    a.exits[Direction::East as usize] = exit(C, 0);
    let mut b = room(B, 2); // foreign zone, attributes 0 — enterable
    b.exits[Direction::South as usize] = exit(A, 0);
    let mut c = room(C, 1);
    c.attributes = 1; // flagged — blocked for 0x25
    c.exits[Direction::West as usize] = exit(A, 0);
    content.add_room(a);
    content.add_room(b);
    content.add_room(c);
    content.add_monster(monster(1, 0x25, 0));
    let mut core = Core::new(content, config());
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut visited_b = false;
    for _ in 0..600 {
        core.tick();
        core.drain_events();
        let loc = core.monster_location(m).unwrap();
        assert_ne!(loc, C, "0x25 never enters a flagged room");
        visited_b |= loc == B;
    }
    assert!(visited_b, "0x25 ignores the zone leash");
}

#[test]
fn water_class_needs_the_water_flag_and_moves_every_tick() {
    // §3: class 5 takes the try-wander path with no aggression roll, but
    // only into rooms with attributes bit 2.
    let mut content = Content::default();
    let mut a = room(A, 1);
    a.attributes = 2;
    a.exits[Direction::North as usize] = exit(B, 0);
    a.exits[Direction::East as usize] = exit(C, 0);
    let mut b = room(B, 1);
    b.attributes = 2; // water — enterable
    b.exits[Direction::South as usize] = exit(A, 0);
    let mut c = room(C, 1); // dry — blocked
    c.exits[Direction::West as usize] = exit(A, 0);
    content.add_room(a);
    content.add_room(b);
    content.add_room(c);
    content.add_monster(monster(1, 5, 100)); // aggression irrelevant on the water path
    let mut core = Core::new(content, config());
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut visited_b = false;
    for _ in 0..120 {
        core.tick();
        core.drain_events();
        let loc = core.monster_location(m).unwrap();
        assert_ne!(loc, C, "water class stays in the water");
        visited_b |= loc == B;
    }
    assert!(visited_b, "water class wanders without the aggression roll");
}

#[test]
fn fairness_cap_limits_three_wanders_per_medium_tick() {
    // §3: DAT_0047fb90 < 3 — at most 3 monsters wander per 3 s tick.
    // Five water-class monsters all want to move every tick; exactly 3 may.
    let mut content = Content::default();
    let mut a = room(A, 1);
    a.attributes = 2;
    a.exits[Direction::North as usize] = exit(B, 0);
    let mut b = room(B, 1);
    b.attributes = 2;
    b.exits[Direction::South as usize] = exit(A, 0);
    content.add_room(a);
    content.add_room(b);
    content.add_monster(monster(1, 5, 0));
    let mut core = Core::new(content, config());
    let ids: Vec<_> = (0..5)
        .map(|_| core.spawn_monster(MonsterId(1), A).unwrap())
        .collect();
    // Advance exactly one medium tick (3 s).
    for _ in 0..3 {
        core.tick();
    }
    core.drain_events();
    let moved = ids
        .iter()
        .filter(|id| core.monster_location(**id) == Some(B))
        .count();
    assert_eq!(moved, 3, "fairness cap: exactly three wanders per tick");
}

#[test]
fn confused_monster_fumbles_instead_of_moving() {
    // check_monster_confusion (0x29812): ability 0x47 value beats
    // genrdn(0,100) => "%s looks around stupidly and foams at the mouth!"
    // and no movement. A 101-value carrier always fumbles.
    use mud_core::ability::Ability;
    let mut content = world();
    let mut m = monster(1, 9, 0);
    m.abilities = vec![(Ability::from_id(0x47).unwrap(), 101)];
    content.add_monster(m);
    let mut core = Core::new(content, config());
    let watcher = create(&mut core, "Alice");
    let id = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut heard = String::new();
    for _ in 0..120 {
        core.tick();
        heard.push_str(&text_to(&core.drain_events(), watcher));
        assert_eq!(core.monster_location(id), Some(A));
    }
    assert!(
        heard.contains("beast 1 looks around stupidly and foams at the mouth!"),
        "fumble line: {heard:?}"
    );
}

#[test]
fn confusion_custom_message_overrides_the_stupid_line() {
    // check_monster_confusion: ability 0x65 names a message whose first
    // line replaces the default fumble text (%s = instance name).
    use mud_core::ability::Ability;
    let mut content = world();
    let mut m = monster(1, 9, 0);
    m.abilities = vec![
        (Ability::from_id(0x47).unwrap(), 101),
        (Ability::from_id(0x65).unwrap(), 500),
    ];
    content.add_monster(m);
    content.add_message(Message {
        id: MessageId(500),
        lines: vec!["%s spins in a circle, drooling.".into()],
    });
    let mut core = Core::new(content, config());
    let watcher = create(&mut core, "Alice");
    let _id = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut heard = String::new();
    for _ in 0..30 {
        core.tick();
        heard.push_str(&text_to(&core.drain_events(), watcher));
    }
    assert!(
        heard.contains("beast 1 spins in a circle, drooling."),
        "custom fumble line: {heard:?}"
    );
    assert!(!heard.contains("stupidly"), "default suppressed: {heard:?}");
}

#[test]
fn pack_follower_is_dragged_and_never_strays() {
    // §3 herd modes: a lone mode-1 leader wanders freely and drags mode-2
    // packmates (same herd id, up to the follower cap) the same direction;
    // a mode-2 follower never moves alone.
    let mut content = world();
    let mut leader = monster(1, 9, 0);
    leader.herd_mode = 1;
    leader.herd_id = 4;
    leader.exp_multi = 10; // herd rank
    leader.follower_cap = 2;
    let mut follower = monster(2, 9, 0);
    follower.herd_mode = 2;
    follower.herd_id = 4;
    follower.exp_multi = 1;
    content.add_monster(leader);
    content.add_monster(follower);
    let mut core = Core::new(content, config());
    let l = core.spawn_monster(MonsterId(1), A).unwrap();
    let f = core.spawn_monster(MonsterId(2), A).unwrap();
    for _ in 0..600 {
        core.tick();
        core.drain_events();
        assert_eq!(
            core.monster_location(l),
            core.monster_location(f),
            "the pack moves as a unit"
        );
        if core.monster_location(l) == Some(B) {
            return; // dragged together — done
        }
    }
    panic!("pack never moved");
}

#[test]
fn damage_exit_wounds_the_crossing_monster() {
    // §3 gate 5, type 9: passable, costs genrdn(dmg/2, dmg+1) HP.
    let mut content = Content::default();
    let mut a = room(A, 9);
    a.exits[Direction::North as usize] = Some(Exit {
        dest: B,
        exit_type: 9,
        trigger_msg: None,
        param: 10,
        param2: 0,
        param3: 0,
        param4: 0,
        door_closed: false,
    });
    let mut b = room(B, 9);
    b.exits[Direction::South as usize] = exit(A, 0);
    content.add_room(a);
    content.add_room(b);
    content.add_monster(monster(1, 9, 0));
    let mut core = Core::new(content, config());
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    assert!(core.debug_move_monster(m, Direction::North));
    assert_eq!(core.monster_location(m), Some(B));
    let hp = core.monster_hp(m).unwrap();
    assert!(
        (50 - 11..=50 - 5).contains(&hp),
        "crossing cost genrdn(5, 11): {hp}"
    );
}

#[test]
fn secret_passage_needs_class_5_or_0x26() {
    // §3 gate 5, types 7/0xb: blocked when the secret parameter is set,
    // unless roam class 5 or 0x26.
    let mut content = Content::default();
    let mut a = room(A, 9);
    a.exits[Direction::North as usize] = Some(Exit {
        dest: B,
        exit_type: 7,
        trigger_msg: None,
        param: 3,
        param2: 0,
        param3: 0,
        param4: 0,
        door_closed: false,
    });
    let mut b = room(B, 9);
    b.attributes = 0;
    b.exits[Direction::South as usize] = exit(A, 0);
    content.add_room(a);
    content.add_room(b);
    content.add_monster(monster(1, 9, 0));
    content.add_monster(monster(2, 0x26, 0));
    let mut core = Core::new(content, config());
    let plain = core.spawn_monster(MonsterId(1), A).unwrap();
    let roamer = core.spawn_monster(MonsterId(2), A).unwrap();
    assert!(!core.debug_move_monster(plain, Direction::North));
    assert!(core.debug_move_monster(roamer, Direction::North));
}

#[test]
fn item_carried_abilities_fold_into_the_monster_value() {
    // get_monster_ability_value (0x3d71f, tail): after slots + template
    // rows the DLL folds the 10 carried items, the wielded weapon
    // (mon+0xb0) and the worn item (mon+0xac <- knmsr+0x60 `something2`,
    // grey robes on the shipped NPCs). A Confusion(101) carrier makes the
    // wander fumble — observable through the stock fumble line.
    use mud_core::ability::Ability;
    use mud_core::content::{Item, ItemId, LootSlot};
    let mut content = world();
    let mut m = monster(1, 9, 0);
    m.loot = vec![LootSlot { item: ItemId(50), uses: -1, dropper: 100 }];
    content.add_monster(m);
    content.add_item(Item {
        id: ItemId(50),
        name: "cursed bauble".into(),
        uses: -1,
        abilities: vec![(Ability::from_id(0x47).unwrap(), 101)],
        ..Default::default()
    });
    let mut core = Core::new(content, config());
    let watcher = create(&mut core, "Alice");
    let id = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut heard = String::new();
    for _ in 0..60 {
        core.tick();
        heard.push_str(&text_to(&core.drain_events(), watcher));
        assert_eq!(core.monster_location(id), Some(A), "carried confusion pins it");
    }
    assert!(
        heard.contains("beast 1 looks around stupidly and foams at the mouth!"),
        "item-borne confusion folds in: {heard:?}"
    );
}

#[test]
fn worn_item_abilities_fold_into_the_monster_value() {
    use mud_core::ability::Ability;
    use mud_core::content::{Item, ItemId};
    let mut content = world();
    let mut m = monster(1, 9, 0);
    m.worn_item = Some(ItemId(51));
    content.add_monster(m);
    content.add_item(Item {
        id: ItemId(51),
        name: "dizzy helm".into(),
        uses: -1,
        abilities: vec![(Ability::from_id(0x47).unwrap(), 101)],
        ..Default::default()
    });
    let mut core = Core::new(content, config());
    let watcher = create(&mut core, "Alice");
    let _id = core.spawn_monster(MonsterId(1), A).unwrap();
    let mut heard = String::new();
    for _ in 0..30 {
        core.tick();
        heard.push_str(&text_to(&core.drain_events(), watcher));
    }
    assert!(
        heard.contains("looks around stupidly"),
        "worn-item confusion folds in: {heard:?}"
    );
}
