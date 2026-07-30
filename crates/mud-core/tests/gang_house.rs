//! .HSE guild-house description streaming (gangs.md §4;
//! display_desc_from_file 0x39598): `Desc[0] == "FILE DESCRIPTION"` +
//! a filename in `Desc[1]` replaces the inline paragraphs with the
//! external file's lines; `Desc[2]` optionally selects the contiguous
//! block of lines carrying it as a prefix (stripped on output).

use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HOUSE: RoomId = RoomId { map: 15, room: 851 };

fn world(desc: Vec<String>) -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: HOUSE,
        name: "White House Entrance".into(),
        description: desc,
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
    content.add_house_text(
        "WCC85115.HSE",
        vec![
            "Entrance White House Room".into(),
            "A marble hall gleams.".into(),
            "N:northern annex".into(),
            "N:second annex line".into(),
            "footer line".into(),
        ],
    );
    content
}

fn visitor() -> Player {
    Player {
        name: "Salad".into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 5,
        current_hp: 30,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HOUSE,
        ..Default::default()
    }
}

fn look_output(content: Content) -> String {
    let mut core = Core::new(
        content,
        CoreConfig { start_location: HOUSE, ..CoreConfig::default() },
    );
    let s = core.attach_player(visitor());
    let events = core.drain_events();
    texts(&events, s)
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
fn file_description_streams_the_house_file() {
    let out = look_output(world(vec![
        "FILE DESCRIPTION".into(),
        "WCC85115.HSE".into(),
    ]));
    assert!(out.contains("Entrance White House Room"), "{out:?}");
    assert!(out.contains("A marble hall gleams."), "{out:?}");
    assert!(!out.contains("FILE DESCRIPTION"), "sentinel never renders: {out:?}");
}

#[test]
fn selector_prefix_picks_the_contiguous_block() {
    let out = look_output(world(vec![
        "FILE DESCRIPTION".into(),
        "WCC85115.HSE".into(),
        "N:".into(),
    ]));
    assert!(out.contains("northern annex"), "{out:?}");
    assert!(out.contains("second annex line"), "{out:?}");
    assert!(!out.contains("N:northern"), "prefix stripped: {out:?}");
    assert!(!out.contains("marble hall"), "{out:?}");
    assert!(!out.contains("footer line"), "block ends at first non-match: {out:?}");
}

#[test]
fn missing_file_renders_no_description() {
    // internal_error is a SYSOP-side log in the DLL (the FILE_ID.DIZ
    // fill-in set exists to silence it) — the player just gets no
    // paragraphs.
    let out = look_output(world(vec![
        "FILE DESCRIPTION".into(),
        "NOSUCH.HSE".into(),
    ]));
    assert!(out.contains("White House Entrance"), "room renders: {out:?}");
    assert!(!out.contains("NOSUCH"), "{out:?}");
    assert!(!out.contains("FILE DESCRIPTION"), "{out:?}");
}

#[test]
fn plain_descriptions_are_untouched() {
    let out = look_output(world(vec![
        "Just an ordinary room.".into(),
        "WCC85115.HSE".into(),
    ]));
    assert!(out.contains("Just an ordinary room."), "{out:?}");
    assert!(out.contains("WCC85115.HSE"), "second paragraph renders inline: {out:?}");
    assert!(!out.contains("marble hall"), "{out:?}");
}
