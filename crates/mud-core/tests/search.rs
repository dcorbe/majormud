//! SEARCH — trap detection + the room broadcast (theft.md §9). Finding
//! a trap changes no state (it is pure information); the FindTraps roll
//! gates it. Bare `search` re-lists the room and tells the room you are
//! searching. (Hidden type-6 exit reveal rides the DISARM/trap-state
//! pass, which reworks the exit found-state overlay.)

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HALL: RoomId = RoomId { map: 1, room: 1 };
const NEXT: RoomId = RoomId { map: 1, room: 2 };

fn world() -> Content {
    let mut content = Content::default();
    let mut hall = Room {
        id: HALL,
        name: "Hall".into(),
        ..Default::default()
    };
    // North: an armed mechanical trap (type 9), damage rating in para1.
    hall.exits[Direction::North as usize] = Some(Exit {
        dest: NEXT,
        exit_type: 9,
        param: 30,  // damage rating
        param2: 0,  // trap state 0 = armed
        ..Default::default()
    });
    // East: a plain open exit.
    hall.exits[Direction::East as usize] = Some(Exit {
        dest: NEXT,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(hall);
    content.add_room(Room {
        id: NEXT,
        name: "Next".into(),
        ..Default::default()
    });
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
    // Sharp-eyed: FindTraps gate + a big bonus.
    content.add_class(Class {
        id: ClassId(1),
        name: "Scout".into(),
        abilities: vec![(Ability::from_id(0x28).unwrap(), 200)], // FindTraps
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    content.add_class(Class {
        id: ClassId(2),
        name: "Oaf".into(),
        abilities: vec![],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

fn person(name: &str, class: u16) -> Player {
    let stats = StatBlock {
        intellect: 60,
        wisdom: 40,
        strength: 40,
        health: 40,
        agility: 60,
        charm: 40,
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(class),
        level: 5,
        stats,
        base_stats: stats,
        current_hp: 30,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HALL,
        ..Default::default()
    }
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

#[test]
fn bare_search_tells_the_room() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Seeker", 1));
    let watcher = core.attach_player(person("Bystander", 2));
    core.drain_events();
    core.input(s, "search");
    let seen = texts(&core.drain_events(), watcher);
    assert!(seen.contains("Seeker is searching the area."), "{seen:?}");
}

#[test]
fn directional_search_finds_the_trap() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Seeker", 1)); // FindTraps huge
    let watcher = core.attach_player(person("Bystander", 2));
    core.drain_events();
    core.input(s, "search n");
    let events = core.drain_events();
    let own = texts(&events, s);
    let seen = texts(&events, watcher);
    assert!(seen.contains("Seeker is searching for exits."), "{seen:?}");
    assert!(own.contains("You found a trap to the north!"), "{own:?}");
}

#[test]
fn low_skill_notices_nothing() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Blind", 2)); // no FindTraps
    core.drain_events();
    core.input(s, "search n");
    let own = texts(&core.drain_events(), s);
    assert!(
        own.contains("You notice nothing different to the north."),
        "{own:?}"
    );
}

#[test]
fn searching_a_plain_exit_notices_nothing() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Seeker", 1));
    core.drain_events();
    core.input(s, "search e");
    let own = texts(&core.drain_events(), s);
    assert!(
        own.contains("You notice nothing different to the east."),
        "{own:?}"
    );
}

#[test]
fn searching_a_non_direction_is_refused() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Seeker", 1));
    core.drain_events();
    core.input(s, "search fountain");
    let own = texts(&core.drain_events(), s);
    assert!(own.contains("Why would you want to search that?"), "{own:?}");
}
