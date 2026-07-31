//! `SET WARNING ON|OFF` — the board's evil-warning setter.
//!
//! The board takes an explicit argument (`WARNING` is in the SET list at
//! DLL 0xd8027; a bad argument answers "Valid warning options: ON, OFF"
//! at 0xd7d68). We used to implement a bare `set evil` TOGGLE instead,
//! which a caller has to read back: if the first one turned the warning
//! ON, the character already had it off and a second puts it back. The
//! client carried a whole extra branch to cope with that.

use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};
use mud_core::text;

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

fn player() -> Player {
    Player {
        name: "Alice".into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        current_hp: 10,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HALL,
        ..Default::default()
    }
}

fn said_to(events: &[Event], session: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == session => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn persisted(events: &[Event]) -> bool {
    events.iter().any(|e| matches!(e, Event::Persist(_)))
}

/// A setter, not a toggle: saying OFF twice leaves it off and says so
/// both times. This is the property the client's read-back branch exists
/// to work around.
#[test]
fn set_warning_off_is_idempotent() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player());
    core.drain_events();

    for attempt in 1..=2 {
        core.input(alice, "set warning off");
        let events = core.drain_events();
        assert!(
            said_to(&events, alice).contains(text::SET_EVIL_WARN_OFF),
            "attempt {attempt} confirms OFF"
        );
    }
}

#[test]
fn set_warning_on_is_idempotent() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player());
    core.drain_events();
    core.input(alice, "set warning off");
    core.drain_events();

    for attempt in 1..=2 {
        core.input(alice, "set warning on");
        let events = core.drain_events();
        assert!(
            said_to(&events, alice).contains(text::SET_EVIL_WARN_ON),
            "attempt {attempt} confirms ON"
        );
    }
}

#[test]
fn changing_the_setting_persists_the_character() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut warned = player();
    warned.warn_on_evil = true;
    let alice = core.attach_player(warned);
    core.drain_events();

    core.input(alice, "set warning off");
    assert!(persisted(&core.drain_events()), "a change is saved");

    // Asking again lands on the state it is already in, so there is
    // nothing new to write.
    core.input(alice, "set warning off");
    assert!(
        !persisted(&core.drain_events()),
        "a no-op setting does not re-save"
    );
}

#[test]
fn a_missing_or_bad_argument_lists_the_options() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player());
    core.drain_events();

    for args in ["set warning", "set warning maybe"] {
        core.input(alice, args);
        let shown = said_to(&core.drain_events(), alice);
        assert!(
            shown.contains(text::SET_WARNING_VALID),
            "{args:?} lists the options, got: {shown:?}"
        );
        assert!(
            !shown.contains(text::SET_EVIL_WARN_OFF) && !shown.contains(text::SET_EVIL_WARN_ON),
            "{args:?} changes nothing, got: {shown:?}"
        );
    }
}

/// The retired spelling gets the board's treatment for anything it does
/// not recognize: it is said out loud and the setting is left alone.
#[test]
fn the_old_set_evil_toggle_is_gone() {
    let mut core = Core::new(world(), CoreConfig::default());
    let alice = core.attach_player(player());
    core.drain_events();
    core.input(alice, "set evil");
    let shown = said_to(&core.drain_events(), alice);
    assert!(
        !shown.contains(text::SET_EVIL_WARN_OFF) && !shown.contains(text::SET_EVIL_WARN_ON),
        "`set evil` no longer sets anything: {shown:?}"
    );
    assert!(
        shown.contains("You say \"set evil\""),
        "an unknown command is said out loud: {shown:?}"
    );
}
