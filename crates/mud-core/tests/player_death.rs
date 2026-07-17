//! Tests for player death, bleed/aid, and respawn
//! (`re/docs/death.md` + oracle transcripts: floor -200, miracle sequence).

use mud_core::content::{
    AttackForm, Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

/// Executioner: always hits hard (accuracy overwhelming, damage fixed high).
fn executioner() -> Monster {
    Monster {
        id: MonsterId(7),
        name: "executioner".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 5000,
        experience: 40,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 0,
        magic_resist: 0,
        bs_defence: 0,
        energy: 1000,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [
            AttackForm {
                kind: 1,
                accuracy: 500,
                weight: 100,
                min_damage: 60,
                max_damage: 60,
                hit_msg: None,
                miss_msg: None,
                energy: 200, // five swings per round: 300 damage
            },
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Arena".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_room(Room {
        id: RoomId { map: 1, room: 2190 },
        name: "Newhaven, Healer".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_monster(executioner());
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
        start_location: RoomId { map: 1, room: 1 },
        recall_location: RoomId { map: 1, room: 2190 },
        ..CoreConfig::default()
    }
}

fn create(core: &mut Core, name: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
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

fn fight_to_death(core: &mut Core, s: SessionId) -> String {
    core.input(s, "attack executioner");
    core.drain_events();
    let mut shown = String::new();
    for _ in 0..600 {
        core.tick();
        shown.push_str(&text_to(&core.drain_events(), s));
        if shown.contains("lives left") {
            break;
        }
    }
    shown
}

#[test]
fn death_fires_at_the_floor_with_the_miracle_sequence() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });

    let shown = fight_to_death(&mut core, s);
    assert!(shown.contains("You have been killed!"), "got: {shown:?}");
    assert!(
        shown.contains("But, due to a miracle, you have been saved."),
        "got: {shown:?}"
    );
    assert!(shown.contains("You have 8 lives left."), "got: {shown:?}");

    let p = core.player_snapshot(s);
    assert_eq!(p.lives, 8);
    assert_eq!(p.current_hp, 35, "respawn at full HP");
    assert_eq!(
        p.location,
        RoomId { map: 1, room: 2190 },
        "recalled to the healer"
    );
}

#[test]
fn death_drops_all_coins_in_the_death_room() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_copper(s, 55);
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    fight_to_death(&mut core, s);

    assert_eq!(core.player_snapshot(s).coins.copper, 0, "coins dropped");
    // Walk back (teleport via test hook is overkill: check the pile via a
    // second player's look).
    let w = create(&mut core, "Witness");
    core.input(w, "look");
    let shown = text_to(&core.drain_events(), w);
    assert!(
        shown.contains("You notice 55 copper farthings here."),
        "pile in the death room: {shown:?}"
    );
}

#[test]
fn out_of_lives_is_the_end() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.set_lives(s, 1);
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });

    core.input(s, "attack executioner");
    core.drain_events();
    let mut shown = String::new();
    let mut deleted = false;
    let mut disconnected = false;
    for _ in 0..600 {
        core.tick();
        for e in core.drain_events() {
            match e {
                Event::Output { session, text } if session == s => shown.push_str(&text),
                Event::DeleteCharacter(ref name) if name == "Dain" => deleted = true,
                Event::Disconnect(d) if d == s => disconnected = true,
                _ => {}
            }
        }
        if disconnected {
            break;
        }
    }
    assert!(
        shown.contains("You have no lives remaining!"),
        "got: {shown:?}"
    );
    assert!(deleted, "character deletion event");
    assert!(disconnected, "session closed");
}

#[test]
fn unaided_downed_player_bleeds_on_the_slow_tick() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.set_current_hp(s, 0); // downed, no attacker

    for _ in 0..30 {
        core.tick();
    }
    assert_eq!(core.current_hp(s), -1, "bleeds one per slow tick");
}

#[test]
fn aided_downed_player_stabilizes() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let helper = create(&mut core, "Bofur");
    core.set_current_hp(s, -3);
    core.input(helper, "aid dain");
    let shown = text_to(&core.drain_events(), helper);
    assert!(
        shown.contains("You have aided Dain"),
        "aid confirmation: {shown:?}"
    );

    for _ in 0..30 {
        core.tick();
    }
    assert_eq!(core.current_hp(s), -2, "recovers one per slow tick");
}

#[test]
fn aiding_the_healthy_is_rejected() {
    let mut core = Core::new(world(), config());
    let _dain = create(&mut core, "Dain");
    let helper = create(&mut core, "Bofur");
    core.input(helper, "aid dain");
    let shown = text_to(&core.drain_events(), helper);
    assert!(
        shown.contains("is in no need of assistance"),
        "got: {shown:?}"
    );
}
