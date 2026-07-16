//! Tests for the delayed exit (silent meditation) and the Lawful creation
//! prompt (oracle transcripts oracle_train2.raw / oracle_exit_cancel.raw /
//! oracle_full_creation.raw).

use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec![],
        shop: None,
        exits: Default::default(),
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
    });
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
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

fn create(core: &mut Core, name: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.input(s, "No"); // Lawful prompt
    core.drain_events();
    s
}

// --- Lawful prompt ---

#[test]
fn class_choice_leads_to_the_lawful_prompt() {
    let mut core = Core::new(world(), config());
    let s = core.attach_account(AccountProfile {
        name: "Dain".into(),
        gender: Gender::Male,
    });
    core.input(s, "2");
    core.drain_events();
    core.input(s, "1");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You must now choose if you want to be a truly 'lawful' citizen of the realm."),
        "got: {shown:?}"
    );
    assert!(
        shown.contains("Do you want to be Lawful?  [Yes/No]"),
        "two spaces before [Yes/No]: {shown:?}"
    );
    assert!(!shown.contains("Hits:"), "no sheet before the answer");
}

#[test]
fn lawful_answer_is_recorded_and_creation_completes() {
    let mut core = Core::new(world(), config());
    let s = core.attach_account(AccountProfile {
        name: "Dain".into(),
        gender: Gender::Male,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.input(s, "Yes");
    let events = core.drain_events();
    let persisted = events
        .iter()
        .find_map(|e| match e {
            Event::Persist(p) => Some(p),
            _ => None,
        })
        .expect("persisted");
    assert!(persisted.lawful);
    let shown = text_to(&events, s);
    assert!(shown.contains("Hits:"), "sheet after answer: {shown:?}");
}

#[test]
fn unrecognized_lawful_answer_reasks() {
    let mut core = Core::new(world(), config());
    let s = core.attach_account(AccountProfile {
        name: "Dain".into(),
        gender: Gender::Male,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.drain_events();
    core.input(s, "maybe");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Do you want to be Lawful?  [Yes/No]"),
        "got: {shown:?}"
    );
}

// --- delayed exit ---

#[test]
fn quit_starts_meditation_not_immediate_exit() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");

    core.input(s, "x");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    assert!(
        shown.contains("You will exit after a period of silent meditation."),
        "got: {shown:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::Disconnect(_))),
        "no immediate disconnect"
    );
}

#[test]
fn exit_completes_after_ten_dots() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let watcher = create(&mut core, "Watcher");
    core.drain_events();

    core.input(s, "x");
    core.drain_events();
    let mut dots = String::new();
    let mut disconnected = false;
    for _ in 0..12 {
        core.tick();
        for e in core.drain_events() {
            match e {
                Event::Output { session, text } if session == s => dots.push_str(&text),
                Event::Disconnect(d) if d == s => disconnected = true,
                Event::Output { session, text } if session == watcher => {
                    if text.contains("Dain just left the Realm.") {
                        assert!(disconnected || dots.matches('.').count() >= 10);
                    }
                }
                _ => {}
            }
        }
    }
    assert_eq!(dots.matches('.').count(), 10, "ten dots: {dots:?}");
    assert!(disconnected, "disconnects after meditation");
}

#[test]
fn commands_during_meditation_are_swallowed() {
    // Oracle: sending movement during the dots produces nothing and the
    // exit still completes.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "x");
    core.drain_events();

    core.tick();
    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        !shown.contains("Town Gates"),
        "look ignored while meditating: {shown:?}"
    );

    let mut disconnected = false;
    for _ in 0..11 {
        core.tick();
        disconnected |= core
            .drain_events()
            .iter()
            .any(|e| matches!(e, Event::Disconnect(d) if *d == s));
    }
    assert!(disconnected, "exit still completes");
}

#[test]
fn cancel_exit_aborts_the_meditation() {
    // The combat hook (M3): engagement cancels a pending exit.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "x");
    core.drain_events();

    core.tick();
    core.cancel_exit(s);
    let mut disconnected = false;
    for _ in 0..15 {
        core.tick();
        disconnected |= core
            .drain_events()
            .iter()
            .any(|e| matches!(e, Event::Disconnect(_)));
    }
    assert!(!disconnected, "cancelled exit never fires");

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("Town Gates"), "play continues: {shown:?}");
}
