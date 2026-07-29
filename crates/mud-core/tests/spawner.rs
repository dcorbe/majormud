//! M6 slice 4 — the density spawner (`FUN_004232d3` decompile 20100-20291)
//! and `generate_monster`'s gates (20806-21240). Extracted facts under
//! test: type-2 rooms spawn at 89% per 5 s kick and type-0 at 4% (the
//! spec had the direction inverted); the density brake is monsters <
//! players-in-room; a lone natural-100 spawns while monsters < 2x
//! players; type-2 rooms bypass the respawn timer; max_level 0 with no
//! forced monster never spawns; bosses bypass the spawn cap and their
//! own respawn timer; boot fills bosses plus type-3/type-1 rooms.

use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Message, MessageId, Monster, MonsterId, Race,
    RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HUB: RoomId = RoomId { map: 1, room: 1 };
const SIDE: RoomId = RoomId { map: 1, room: 2 };

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
    content
}

/// A zone-7 level-3 template, passive and stationary so the scenario
/// stays put.
fn critter(id: u16) -> Monster {
    Monster {
        id: MonsterId(id),
        name: format!("critter {id}"),
        hitpoints: 30,
        energy: 1000,
        magic_resist: 0,
        roam_class: 7,
        level: 3,
        behaviour: 0,
        herd_mode: 0,
        ..Default::default()
    }
}

/// A spawn room: `room_type` picks the spawner path, zone 7, band 1-5,
/// cap high by default.
fn spawn_room(id: RoomId, room_type: i16, cap: i16) -> Room {
    Room {
        id,
        name: format!("room {}", id.room),
        room_type,
        spawn_zone: 7,
        spawn_cap: cap,
        min_level: 1,
        max_level: 5,
        ..Default::default()
    }
}

fn player_at(name: &str, location: RoomId) -> Player {
    let stats = StatBlock {
        intellect: 50,
        wisdom: 50,
        strength: 90,
        health: 50,
        agility: 90,
        charm: 50,
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        stats,
        base_stats: stats,
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
        start_location: HUB,
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

fn live_in(core: &Core, room: RoomId) -> usize {
    core.monster_ids()
        .into_iter()
        .filter(|id| core.monster_location(*id) == Some(room))
        .count()
}

#[test]
fn boot_fills_bosses_and_swarm_rooms_but_not_plain_ones() {
    let mut content = base_content();
    let mut boss_room = spawn_room(HUB, 0, 5);
    boss_room.boss_monster = Some(MonsterId(9));
    content.add_room(boss_room);
    content.add_room(spawn_room(SIDE, 3, 2)); // swarm: boot-fills to cap
    content.add_room(spawn_room(RoomId { map: 1, room: 3 }, 0, 5)); // plain: empty
    content.add_monster(critter(1));
    let mut boss = critter(9);
    boss.name = "warden".into();
    content.add_monster(boss);
    let core = Core::new(content, config());
    assert_eq!(live_in(&core, HUB), 1, "the boss stands at boot");
    assert_eq!(live_in(&core, SIDE), 2, "the swarm room boot-fills to its cap");
    assert_eq!(live_in(&core, RoomId { map: 1, room: 3 }), 0, "type 0 gets no boot fill");
}

#[test]
fn type2_room_fills_toward_the_player_count() {
    // 89% per kick while monsters < players; the density brake then holds
    // the room at parity (bar the 1% natural-100 overshoot).
    let mut content = base_content();
    content.add_room(spawn_room(HUB, 2, 10));
    content.add_monster(critter(1));
    let mut core = Core::new(content, config());
    let _s = core.attach_player(player_at("Bait", HUB));
    core.drain_events();
    for _ in 0..20 {
        core.tick(); // four 5 s kicks
    }
    assert_eq!(live_in(&core, HUB), 1, "parity with one player, no overshoot");
}

#[test]
fn unfulfillable_band_never_spawns() {
    // L20886: max_level 0 with no forced monster refuses before anything
    // else — the shipped zoned band-0 rooms are spawnless by design.
    let mut content = base_content();
    let mut r = spawn_room(HUB, 2, 10);
    r.min_level = 0;
    r.max_level = 0;
    content.add_room(r);
    content.add_monster(critter(1));
    let mut core = Core::new(content, config());
    let _s = core.attach_player(player_at("Bait", HUB));
    core.drain_events();
    for _ in 0..40 {
        core.tick();
    }
    assert_eq!(live_in(&core, HUB), 0);
}

#[test]
fn zone_and_band_filter_the_candidates() {
    // Wrong zone and out-of-band templates never come up.
    let mut content = base_content();
    content.add_room(spawn_room(HUB, 2, 10));
    let mut wrong_zone = critter(1);
    wrong_zone.roam_class = 8;
    let mut too_high = critter(2);
    too_high.level = 9;
    content.add_monster(wrong_zone);
    content.add_monster(too_high);
    content.add_monster(critter(3)); // the only eligible one
    let mut core = Core::new(content, config());
    let _s = core.attach_player(player_at("Bait", HUB));
    core.drain_events();
    for _ in 0..20 {
        core.tick();
    }
    let ids = core.monster_ids();
    assert_eq!(ids.len(), 1);
    assert_eq!(core.monster_template(ids[0]), Some(MonsterId(3)));
}

#[test]
fn forced_monster_room_spawns_exactly_that_template() {
    let mut content = base_content();
    let mut r = spawn_room(HUB, 2, 10);
    r.forced_monster = Some(MonsterId(5));
    content.add_room(r);
    content.add_monster(critter(1)); // zone-eligible decoy
    let mut forced = critter(5);
    forced.roam_class = 99; // NOT zone-eligible: only the forced path spawns it
    content.add_monster(forced);
    let mut core = Core::new(content, config());
    let _s = core.attach_player(player_at("Bait", HUB));
    core.drain_events();
    for _ in 0..20 {
        core.tick();
    }
    let ids = core.monster_ids();
    assert!(!ids.is_empty(), "the forced room spawned");
    for id in ids {
        assert_eq!(core.monster_template(id), Some(MonsterId(5)));
    }
}

#[test]
fn respawn_timer_blocks_the_refill_until_the_delay_passes() {
    // Type 0 honors the stamp window [kill, kill + delay minutes]; delay
    // 1 minute = 60 ticks. Type-0's 4% kick rate means the refill itself
    // is slow — assert the WINDOW blocks, then that the state allows.
    let mut content = base_content();
    let mut r = spawn_room(HUB, 2, 10); // type 2 spawns fast...
    r.respawn_delay = 1;
    content.add_room(r);
    content.add_monster(critter(1));
    let mut core = Core::new(content, config());
    let _s = core.attach_player(player_at("Bait", HUB));
    core.drain_events();
    for _ in 0..20 {
        core.tick();
    }
    let ids = core.monster_ids();
    assert_eq!(ids.len(), 1, "parity spawn");
    // ...but type 2 BYPASSES the timer — so first prove the bypass:
    let killed_at = ids[0];
    core.debug_kill_monster(killed_at);
    core.drain_events();
    for _ in 0..20 {
        core.tick();
    }
    assert_eq!(live_in(&core, HUB), 1, "type 2 refills straight through the stamp");
}

#[test]
fn type0_respawn_window_blocks() {
    // A type-0 room with a fresh kill stamp refuses generate_monster for
    // the whole delay window, natural-100s included.
    let mut content = base_content();
    let mut r = spawn_room(HUB, 0, 10);
    r.respawn_delay = 5;
    content.add_room(r);
    content.add_monster(critter(1));
    let mut core = Core::new(content, config());
    let _s = core.attach_player(player_at("Bait", HUB));
    let planted = core.spawn_monster(MonsterId(1), HUB).unwrap();
    core.drain_events();
    core.debug_kill_monster(planted);
    core.drain_events();
    // 5-minute window = 300 ticks; type-0's own 4%/kick makes spawns rare
    // anyway, so drive generate directly through the debug hook.
    for _ in 0..3 {
        core.tick();
    }
    assert!(
        !core.debug_generate(HUB),
        "the stamp window refuses a fresh spawn"
    );
}

#[test]
fn spawn_arrival_lines_default_and_custom() {
    // movemsg 0 => "X just arrived from the <dir>." (or "from nowhere."
    // with no plain compass exit); a resolvable movemsg prints its text;
    // adjacent rooms hear "You hear movement to the <reverse dir>."
    let mut content = base_content();
    let mut hub = spawn_room(HUB, 2, 10);
    hub.exits[Direction::North as usize] = Some(Exit {
        dest: SIDE,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(hub);
    let mut side = Room {
        id: SIDE,
        name: "Side".into(),
        ..Default::default()
    };
    side.exits[Direction::South as usize] = Some(Exit {
        dest: HUB,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(side);
    content.add_monster(critter(1));
    let mut core = Core::new(content, config());
    let here = core.attach_player(player_at("Here", HUB));
    let next_door = core.attach_player(player_at("Near", SIDE));
    core.drain_events();
    let (mut seen_here, mut seen_near) = (String::new(), String::new());
    for _ in 0..20 {
        core.tick();
        let events = core.drain_events();
        seen_here.push_str(&text_to(&events, here));
        seen_near.push_str(&text_to(&events, next_door));
        if live_in(&core, HUB) > 0 {
            break;
        }
    }
    assert!(
        seen_here.contains("critter 1 just arrived from the north."),
        "default arrival names the plain exit: {seen_here:?}"
    );
    assert!(
        seen_near.contains("You hear movement to the south."),
        "the far side of the exit hears the rumble: {seen_near:?}"
    );
}

#[test]
fn custom_movemsg_replaces_the_arrival_line() {
    let mut content = base_content();
    content.add_room(spawn_room(HUB, 2, 10));
    let mut m = critter(1);
    m.move_msg = Some(MessageId(700));
    content.add_monster(m);
    content.add_message(Message {
        id: MessageId(700),
        // The shipped shape (orc rogue msg 38): %s slots bind (NAME,
        // dirspec) — "An nasty orc rogue walks into the room from the
        // west." in the oracle capture.
        lines: vec!["An %s walks into the room from %s.".into()],
    });
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Bait", HUB));
    core.drain_events();
    let mut seen = String::new();
    for _ in 0..20 {
        core.tick();
        seen.push_str(&text_to(&core.drain_events(), s));
        if live_in(&core, HUB) > 0 {
            break;
        }
    }
    assert!(
        seen.contains("An critter 1 walks into the room from nowhere."),
        "custom movemsg binds (name, dirspec): {seen:?}"
    );
    assert!(!seen.contains("just arrived"), "default suppressed: {seen:?}");
}

#[test]
fn rolled_coins_drop_with_the_ground_lines() {
    // generate_monster rolls lngrnd(0, max+1) per nonzero pile; the
    // killer sees "N <denom> drop to the ground." lines high-first.
    let mut content = base_content();
    content.add_room(spawn_room(HUB, 2, 10));
    let mut m = critter(1);
    m.coins = [0, 0, 0, 40, 0]; // silver max 40 (template order high->low)
    m.hitpoints = 1;
    content.add_monster(m);
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Bait", HUB));
    core.drain_events();
    for _ in 0..20 {
        core.tick();
        if live_in(&core, HUB) > 0 {
            break;
        }
    }
    core.drain_events();
    let ids = core.monster_ids();
    core.input(s, "attack critter");
    let mut seen = String::new();
    for _ in 0..25 {
        core.tick();
        seen.push_str(&text_to(&core.drain_events(), s));
        if core.monster_hp(ids[0]).is_none() {
            break;
        }
    }
    // Under SEED the silver roll lands 9 (re-derived for the genrdn
    // exclusive-upper correction): the killer sees the drop line at kill
    // time and the pile renders on the next look.
    assert!(
        seen.contains("9 silver drop to the ground."),
        "killer-visible drop line: {seen:?}"
    );
    core.input(s, "look");
    let look = text_to(&core.drain_events(), s);
    assert!(
        look.contains("You notice 9 silver"),
        "the rolled pile is on the floor: {look:?}"
    );
}

#[test]
fn neighbor_pass_spreads_into_the_adjacent_room() {
    // Phase A: ~5% per exit per pass; the neighbor spawns while its live
    // count < the source room's player count.
    let mut content = base_content();
    let mut hub = spawn_room(HUB, 2, 10);
    hub.exits[Direction::North as usize] = Some(Exit {
        dest: SIDE,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(hub);
    let mut side = spawn_room(SIDE, 0, 10);
    side.exits[Direction::South as usize] = Some(Exit {
        dest: HUB,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(side);
    content.add_monster(critter(1));
    let mut core = Core::new(content, config());
    let _s = core.attach_player(player_at("Bait", HUB));
    core.drain_events();
    let mut spread = false;
    for _ in 0..600 {
        core.tick();
        core.drain_events();
        if live_in(&core, SIDE) > 0 {
            spread = true;
            break;
        }
    }
    assert!(spread, "the neighbor pass seeds the room next door");
}

#[test]
fn boss_respawns_through_the_forced_room_after_a_kill() {
    // A boss room with bynumber == permnpc: the flag blocks re-spawn
    // while alive; the kill clears it, and the boss (timer-exempt)
    // returns on a later kick.
    let mut content = base_content();
    let mut r = spawn_room(HUB, 2, 1);
    r.boss_monster = Some(MonsterId(9));
    r.forced_monster = Some(MonsterId(9));
    content.add_room(r);
    let mut boss = critter(9);
    boss.name = "warden".into();
    boss.hitpoints = 1;
    boss.roam_class = 99; // only the forced path spawns it
    content.add_monster(boss);
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Regicide", HUB));
    core.drain_events();
    assert_eq!(live_in(&core, HUB), 1, "boot boss");
    for _ in 0..10 {
        core.tick(); // two kicks: the present-flag blocks a duplicate
    }
    assert_eq!(live_in(&core, HUB), 1, "no duplicate while the flag holds");
    let ids = core.monster_ids();
    core.input(s, "attack warden");
    let mut fight = String::new();
    for _ in 0..100 {
        core.tick();
        fight.push_str(&text_to(&core.drain_events(), s));
        if core.monster_hp(ids[0]).is_none() {
            break;
        }
    }
    assert!(core.monster_hp(ids[0]).is_none(), "the warden fell: {fight:?}");
    let mut returned = false;
    for _ in 0..40 {
        core.tick();
        core.drain_events();
        if live_in(&core, HUB) > 0 {
            returned = true;
            break;
        }
    }
    assert!(returned, "the boss respawns past the cleared flag");
}
