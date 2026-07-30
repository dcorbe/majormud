//! The gang membership state machine (gangs.md §1) — CREATE through
//! DISBAND, the roster, and the gangpath channel. Every string is the
//! DLL literal from the slice-7 verification pass unless tagged.

use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HALL: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: HALL,
        name: "Hall".into(),
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
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
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

fn person(name: &str) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 10,
        experience: 150_000,
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

fn core_with(players: &[&Player]) -> (Core, Vec<SessionId>) {
    let mut core = Core::new(world(), CoreConfig::default());
    let ids = players.iter().map(|p| core.attach_player((*p).clone())).collect();
    core.drain_events();
    (core, ids)
}

// --- §1.1 CREATE ---

#[test]
fn create_gang_success() {
    let (mut core, s) = core_with(&[&person("Salad")]);
    core.input(s[0], "create gang Iron Fist");
    let events = core.drain_events();
    let out = texts(&events, s[0]);
    assert!(out.contains("Gang created."), "{out:?}");

    let gang = core.gang("Iron Fist").expect("gang exists");
    assert_eq!(gang.leader, "Salad");
    assert_eq!(gang.member_count, 1);
    assert_eq!(gang.display, "Iron Fist");
    assert_eq!(gang.name_key, "IRON FIST");
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistGang(g) if g.display == "Iron Fist")),
        "gang persisted"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p) if p.gang == "Iron Fist")),
        "member row persisted"
    );
}

#[test]
fn create_gate_order_exp_before_membership() {
    // gangs.md §1.1 / cmd_create 53520-53534: the exp gate fires first.
    let mut poor = person("Scrub");
    poor.experience = 99_999;
    poor.gang = "Somewhere".into();
    let (mut core, s) = core_with(&[&poor]);
    core.input(s[0], "create gang Nope");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You are not experienced enough to start your own gang!"),
        "{out:?}"
    );
}

#[test]
fn create_refuses_second_gang() {
    let (mut core, s) = core_with(&[&person("Salad")]);
    core.input(s[0], "create gang First");
    core.drain_events();
    core.input(s[0], "create gang Second");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You are already in one gang.  You cannot create another one."),
        "{out:?}"
    );
    assert!(core.gang("Second").is_none());
}

#[test]
fn create_name_validation_chain() {
    // Order per cmd_create: too-long → 'None' → invalid character.
    let (mut core, s) = core_with(&[&person("Salad")]);

    core.input(s[0], "create gang This Name Is Way Too Long For A Gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("The name you have chosen is too LONG: This Name Is Way Too Long For A Gang"),
        "{out:?}"
    );

    core.input(s[0], "create gang none");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You may not use 'None' as a gang name."), "{out:?}");

    core.input(s[0], "create gang Caf\u{e9} Crew");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You have specified an invalid character in your gang name."),
        "{out:?}"
    );
    assert!(core.gang("none").is_none());
}

#[test]
fn create_duplicate_names_refused_with_leader_line() {
    let (mut core, s) = core_with(&[&person("Salad"), &person("Torgo")]);
    core.input(s[0], "create gang Iron Fist");
    core.drain_events();
    // Case-insensitive collision via the uppercase key.
    core.input(s[1], "create gang IRON fist");
    let out = texts(&core.drain_events(), s[1]);
    assert!(out.contains("The name you have chosen is already being used!"), "{out:?}");
    assert!(out.contains("Salad is the leader of Iron Fist."), "{out:?}");
}

#[test]
fn create_without_name_falls_to_say() {
    // margc < 3 → return 0 → the SAY fall-through (cmd_create 53476).
    let (mut core, s) = core_with(&[&person("Salad")]);
    core.input(s[0], "create gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You say"), "falls through to say: {out:?}");
    assert!(!out.contains("Gang created"), "{out:?}");
}

#[test]
fn create_room_prints_the_lease_stub_and_others_consume_silently() {
    let (mut core, s) = core_with(&[&person("Salad")]);
    core.input(s[0], "create room north");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("If you are a gang leader you may lease a Gang House."),
        "{out:?}"
    );
    // Non-keyword forms with args are consumed with no output (the
    // decompile's silent fall-off arm).
    // Non-keyword forms with args are consumed with no message (the
    // decompile's silent fall-off arm) — only the prompt comes back:
    // neither the say fall-through nor the lease stub fires.
    core.input(s[0], "create castle now");
    let out = texts(&core.drain_events(), s[0]);
    assert!(!out.contains("You say"), "not say: {out:?}");
    assert!(!out.contains("gang leader"), "not the lease stub: {out:?}");
}
