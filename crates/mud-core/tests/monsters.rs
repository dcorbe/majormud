//! Tests for live monster instances: fixture placement, room display.

use mud_core::content::{Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn rat_template() -> Monster {
    Monster {
        id: MonsterId(1),
        name: "giant rat".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 12,
        experience: 9,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 1,
        magic_resist: 30,
        bs_defence: 0,
        energy: 1000,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: Default::default(),
        // Lair mode: attackable without the mode-0/4 evil charge
        // (created characters ship Warn on Evil ON — crime.md §2.5).
        behaviour: 3,
        ..Default::default()
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Forest Path".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    content.add_monster(rat_template());
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

#[test]
fn spawned_monster_appears_in_the_room_display() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 })
        .expect("spawn");
    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Also here: giant rat."),
        "got: {shown:?}"
    );
}

#[test]
fn players_and_monsters_share_the_also_here_line() {
    // Oracle (M1): the weapons-shop NPC appears exactly like a player would.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let _bob = create(&mut core, "Bob");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 })
        .expect("spawn");
    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Also here: Bob, giant rat."),
        "players first, then monsters: {shown:?}"
    );
}

#[test]
fn two_instances_of_one_template_both_listed() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 })
        .expect("spawn");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 })
        .expect("spawn");
    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Also here: giant rat, giant rat."),
        "got: {shown:?}"
    );
}

#[test]
fn spawn_of_unknown_template_fails() {
    let mut core = Core::new(world(), config());
    assert!(core
        .spawn_monster(MonsterId(999), RoomId { map: 1, room: 1 })
        .is_none());
}

#[test]
fn spawned_instance_carries_template_hp() {
    let mut core = Core::new(world(), config());
    let id = core
        .spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 })
        .expect("spawn");
    assert_eq!(core.monster_hp(id), Some(12));
}
