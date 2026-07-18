//! Real-content aggression smoke test (M6 slice 3): a shipped purple
//! slime (behaviour 2 aggressive, aggression 30) jumps a player parked
//! in the real sewers and chases them along the breadcrumb when they
//! run — the full acquisition + pursuit stack over shipped data.

use mud_core::content::{ClassId, MonsterId, RaceId, RoomId};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};
use mud_server::content_db;

fn player_at(name: &str, location: RoomId) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        current_hp: 400,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location,
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
fn shipped_slime_jumps_and_chases_through_the_real_sewers() {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    let content = content_db::load(&db).expect("load");
    let mut core = Core::new(content, CoreConfig::default());
    let start = RoomId { map: 9, room: 1 };
    let s = core.attach_player(player_at("Bait", start));
    let m = core.spawn_monster(MonsterId(467), start).expect("spawn purple slime");
    core.drain_events();
    // Acquisition: an aggressive monster attacks within a few rounds
    // (roll < 50 each round, fallback guarantees on the rolled candidate).
    let mut jumped = String::new();
    for _ in 0..15 {
        core.tick();
        jumped.push_str(&text_to(&core.drain_events(), s));
        if jumped.to_lowercase().contains("purple slime") {
            break;
        }
    }
    assert!(
        jumped.to_lowercase().contains("purple slime"),
        "unprovoked attack in the sewers: {jumped:?}"
    );
    // Run for it — the slime chases along the breadcrumb (aggression 30:
    // the follow roll needs a few fast ticks).
    core.input(s, "n"); // 9/1 -> 9/2 (both zone 30)
    core.drain_events();
    let mut chased = false;
    for _ in 0..40 {
        core.tick();
        core.drain_events();
        if core.monster_location(m) == Some(core.player_snapshot(s).location) {
            chased = true;
            break;
        }
        if core.monster_target(m).is_none() {
            break; // gave up — legal at aggression 30, but count it
        }
    }
    assert!(chased, "the slime followed its lock through the sewer");
}
