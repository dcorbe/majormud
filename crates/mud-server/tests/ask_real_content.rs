//! Real-content ask smoke (M7 slice 6): one shipped conversation driven
//! against `re/mmud_wgnt.sqlite`. This doubles as the design-doc risk
//! check on WCCTEXT2 continuation assembly — the kobold thief's
//! keyword table (block 31 -> default 32, `shit:80`) must have survived
//! extraction verbatim for either line to print.

use mud_core::content::{ClassId, MonsterId, RaceId, RoomId};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};
use mud_core::content_db;

/// "Cavern, Dead End" (map 10) — empty at boot, nothing else talks.
const ARENA: RoomId = RoomId { map: 10, room: 42 };
const KOBOLD_THIEF: MonsterId = MonsterId(7);

fn load() -> Core {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    let content = content_db::load(&db).expect("load");
    Core::new(content, CoreConfig::default())
}

fn seeker() -> Player {
    Player {
        name: "Seeker".into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 5,
        current_hp: 40,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: ARENA,
        ..Default::default()
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

#[test]
fn the_kobold_thief_conversation_round_trips() {
    let mut core = load();
    core.spawn_monster(KOBOLD_THIEF, ARENA).expect("shipped template");
    let s = core.attach_player(seeker());
    core.drain_events();

    // Bare ask: the default long text (block 32) with the spawn-composed
    // name substituted for its %s.
    core.input(s, "ask kobold");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("grins evilly at you, and eyes your purse!"),
        "block 32 verbatim: got {shown:?}"
    );

    // The shipped keyword line `shit:80` — substring-matched inside a
    // longer question, answered by block 80 verbatim.
    core.input(s, "ask kobold what is this shit");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("What a foul mouth!"),
        "block 80 verbatim: got {shown:?}"
    );

    // An unmatched question shrugs.
    core.input(s, "ask kobold about the weather");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("has nothing to tell you!"),
        "got: {shown:?}"
    );
}
