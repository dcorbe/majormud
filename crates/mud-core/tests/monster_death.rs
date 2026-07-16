//! Tests for monster death: removal, coin drops, exp split
//! (`re/docs/death.md` §4/§5).

use mud_core::content::{
    AttackForm, Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

/// A pushover: 1 HP, no attacks, drops 43 copper + 7 silver, worth 9 exp.
fn dying_rat() -> Monster {
    Monster {
        id: MonsterId(1),
        name: "giant rat".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 1,
        experience: 9,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 0,
        magic_resist: 0,
        bs_defence: 0,
        energy: 1000,
        coins: [0, 0, 0, 7, 43], // high->low: 7 silver, 43 copper
        attacks: [AttackForm::default(); 5],
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Arena".into(),
        description: vec![],
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_monster(dying_rat());
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

/// Attack until the rat dies (first hit kills its 1 HP).
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

#[test]
fn killed_monster_is_removed_and_announced() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });

    let events = kill_rat(&mut core, s);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("The giant rat is dead."),
        "kill announcement: {shown:?}"
    );
    assert!(shown.contains("*Combat Off*"), "combat ends: {shown:?}");

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        !shown.contains("Also here: giant rat"),
        "corpse gone: {shown:?}"
    );
}

#[test]
fn kill_awards_experience() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });

    let events = kill_rat(&mut core, s);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("You gain 9 experience."),
        "exp line: {shown:?}"
    );
    assert_eq!(core.player_snapshot(s).experience, 9);
}

#[test]
fn dropped_coins_pile_in_the_room() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    kill_rat(&mut core, s);

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You notice 7 silver nobles, 43 copper farthings here."),
        "oracle floor-pile format: {shown:?}"
    );
}

#[test]
fn singular_coin_names() {
    let mut content = world();
    content.monsters.get_mut(&MonsterId(1)).unwrap().coins = [0, 0, 0, 0, 1];
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    kill_rat(&mut core, s);

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You notice 1 copper farthing here."),
        "singular form: {shown:?}"
    );
}

#[test]
fn exp_splits_equally_among_engaged_players() {
    // Two players both engaged on the rat: 9/2 = 4 each (integer floor).
    let mut content = world();
    content.monsters.get_mut(&MonsterId(1)).unwrap().hitpoints = 60;
    let mut core = Core::new(content, config());
    let a = create(&mut core, "Dain");
    let b = create(&mut core, "Bofur");
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });

    core.input(a, "attack rat");
    core.input(b, "attack rat");
    core.drain_events();
    for _ in 0..300 {
        core.tick();
        core.drain_events();
        if core.player_snapshot(a).experience > 0 || core.player_snapshot(b).experience > 0 {
            break;
        }
    }
    let (ea, eb) = (
        core.player_snapshot(a).experience,
        core.player_snapshot(b).experience,
    );
    assert_eq!(ea + eb, 8, "9 split two ways, floored: 4 + 4");
    assert_eq!(ea, 4);
    assert_eq!(eb, 4);
}
