//! Tests for the character-creation flow (`re/docs/character_creation.md`).
//!
//! Standalone adaptation: the account (name + gender) arrives authenticated;
//! creation runs race -> class -> roll_stats, then enters the Realm. The
//! good/evil prompt is skipped (sysop flag defaults off) and the FSD stat
//! editor is deferred (CP arrives unspent).

use mud_core::content::{
    Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 40,
            wisdom: 40,
            strength: 40,
            health: 40,
            agility: 40,
            charm: 40,
        },
        max_stats: StatBlock {
            intellect: 100,
            wisdom: 100,
            strength: 100,
            health: 100,
            agility: 100,
            charm: 100,
        },
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
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
        max_stats: StatBlock {
            intellect: 90,
            wisdom: 120,
            strength: 110,
            health: 120,
            agility: 90,
            charm: 85,
        },
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


fn test_config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        ..CoreConfig::default()
    }
}

fn profile(name: &str) -> AccountProfile {
    AccountProfile {
        name: name.into(),
        gender: Gender::Female,
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

fn start(core: &mut Core, name: &str) -> SessionId {
    core.attach_account(profile(name))
}

#[test]
fn new_account_is_prompted_for_race() {
    let mut core = Core::new(world(), test_config());
    let s = start(&mut core, "Alice");
    let shown = text_to(&core.drain_events(), s);
    // Oracle-exact list format (MBBSEmu transcript 2026-07-16): "[N]" padded
    // to four columns, then the name; blank line; prompt with no newline.
    assert!(
        shown.contains("Please choose a race from the following list:\n[1]  Human\n[2]  Dwarf\n\nPlease choose your race [ ? for help ] :"),
        "got: {shown:?}"
    );
}

#[test]
fn race_list_number_padding_matches_oracle() {
    // Two-digit ids get one space after the bracket ("[10] Half-Ogre").
    let mut content = world();
    content.add_race(mud_core::content::Race {
        id: RaceId(10),
        name: "Half-Ogre".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 0,
        hp_per_level: 0,
        exp_chart: 30,
    });
    let mut core = Core::new(content, test_config());
    let s = start(&mut core, "Alice");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("[10] Half-Ogre\n"), "got: {shown:?}");
}

#[test]
fn invalid_race_choice_is_rejected() {
    let mut core = Core::new(world(), test_config());
    let s = start(&mut core, "Alice");
    core.drain_events();

    core.input(s, "99");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You must choose a valid race. [ ? for help ]"),
        "got: {shown:?}"
    );
}

#[test]
fn valid_race_advances_to_class_choice() {
    let mut core = Core::new(world(), test_config());
    let s = start(&mut core, "Alice");
    core.drain_events();

    core.input(s, "2");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Please choose a class from the following list:\n[1]  Warrior\n\nPlease choose your class [ ? for help ] :"),
        "got: {shown:?}"
    );
}

#[test]
fn blank_input_at_race_prompt_uses_the_choose_variant() {
    // Oracle: bare CR at the race prompt yields "You must choose a race.",
    // not the "valid race" wording used for wrong entries.
    let mut core = Core::new(world(), test_config());
    let s = start(&mut core, "Alice");
    core.drain_events();

    core.input(s, "");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You must choose a race. [ ? for help ]"),
        "got: {shown:?}"
    );
}

#[test]
fn invalid_class_choice_is_rejected() {
    let mut core = Core::new(world(), test_config());
    let s = start(&mut core, "Alice");
    core.input(s, "2");
    core.drain_events();

    core.input(s, "77");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You must choose a valid class. [ ? for help ]"),
        "got: {shown:?}"
    );
}

#[test]
fn completed_creation_matches_the_spec_initial_state() {
    let mut core = Core::new(world(), test_config());
    let s = start(&mut core, "Alice");
    core.input(s, "2"); // Dwarf
    core.input(s, "1"); // Warrior
    core.input(s, "No"); // Lawful prompt
    let events = core.drain_events();

    let persisted = events
        .iter()
        .find_map(|e| match e {
            Event::Persist(p) => Some(p),
            _ => None,
        })
        .expect("new character is persisted");

    assert_eq!(persisted.name, "Alice");
    assert_eq!(persisted.gender, Gender::Female);
    assert_eq!(persisted.race, RaceId(2));
    assert_eq!(persisted.class, ClassId(1));
    assert_eq!(persisted.level, 1);
    // roll_stats copies the racial template verbatim - no randomisation.
    assert_eq!(persisted.stats.intellect, 30);
    assert_eq!(persisted.stats.wisdom, 50);
    assert_eq!(persisted.stats.strength, 50);
    assert_eq!(persisted.stats.health, 50);
    assert_eq!(persisted.stats.agility, 30);
    assert_eq!(persisted.stats.charm, 30);
    assert_eq!(persisted.cp_unspent, 100);
    assert_eq!(persisted.cp_lifetime, 100);
    assert_eq!(persisted.lives, 9);
    assert_eq!(persisted.experience, 0);
    assert_eq!(persisted.location, RoomId { map: 1, room: 1 });
}

#[test]
fn completed_creation_enters_the_realm() {
    let mut core = Core::new(world(), test_config());
    let s = start(&mut core, "Alice");
    core.input(s, "1");
    core.input(s, "1");
    core.input(s, "No");
    let shown = text_to(&core.drain_events(), s);
    // Oracle: first entry shows the stat sheet, not the room.
    assert!(shown.contains("Hits:"), "sheet shown: {shown:?}");

    // A second player creating a character is announced to the first.
    let s2 = start(&mut core, "Bob");
    core.drain_events();
    core.input(s2, "1");
    core.input(s2, "1");
    core.input(s2, "No");
    let events = core.drain_events();
    let to_alice = text_to(&events, s);
    assert!(
        to_alice.contains("Bob just entered the Realm."),
        "got: {to_alice:?}"
    );
}

#[test]
fn creation_input_is_not_game_commands() {
    let mut core = Core::new(world(), test_config());
    let s = start(&mut core, "Alice");
    core.drain_events();

    core.input(s, "look"); // not a race number
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You must choose a valid race. [ ? for help ]"),
        "got: {shown:?}"
    );
}
