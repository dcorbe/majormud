//! Per-user ANSI (M7 slice 2). The real board keys ANSI on the MBBS
//! account outside WCCMMUD.DLL (cmd_set has no ansi subcommand — slice-1
//! scan), so the in-game `ansi` toggle is OURS, a documented divergence;
//! the rendering on each side of the flag stays byte-exact. The per-user
//! flag overrides the `CoreConfig.ansi` server global at the output
//! funnel; sessions before character attach follow the global.

use mud_core::content::{Content, Room, RoomId};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player};

const HALL: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: HALL,
        name: "Hall".into(),
        ..Default::default()
    });
    content
}

fn dweller(name: &str, ansi: bool) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        level: 1,
        current_hp: 20,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HALL,
        ansi,
        ..Default::default()
    }
}

fn outputs_for(events: &[Event], who: mud_core::game::SessionId) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session, text } if *session == who => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn per_session_flag_overrides_the_global() {
    let config = CoreConfig { ansi: true, ..Default::default() };
    let mut core = Core::new(world(), config);
    let color = core.attach_player(dweller("Color", true));
    let plain = core.attach_player(dweller("Plain", false));
    core.drain_events();
    core.input(color, "say hello");
    let events = core.drain_events();
    let to_color = outputs_for(&events, color).join("");
    let to_plain = outputs_for(&events, plain).join("");
    assert!(to_color.contains('\x1b'), "ansi-on session keeps escapes: {to_color:?}");
    assert!(to_plain.contains("hello"), "plain session got the line: {to_plain:?}");
    assert!(!to_plain.contains('\x1b'), "ansi-off session is stripped: {to_plain:?}");
}

#[test]
fn ansi_command_toggles_reports_and_persists() {
    let config = CoreConfig { ansi: true, ..Default::default() };
    let mut core = Core::new(world(), config);
    let s = core.attach_player(dweller("Flip", true));
    core.drain_events();

    core.input(s, "ansi");
    let events = core.drain_events();
    let out = outputs_for(&events, s).join("");
    assert!(out.contains("ANSI colour is now OFF."), "toggle reports: {out:?}");
    let persisted = events.iter().any(|e| {
        matches!(e, Event::Persist(p) if p.name == "Flip" && !p.ansi)
    });
    assert!(persisted, "the flip persists: {events:?}");

    // Output after the flip is stripped — including the room render.
    core.input(s, "look");
    let out = outputs_for(&core.drain_events(), s).join("");
    assert!(!out.contains('\x1b'), "post-toggle output is plain: {out:?}");

    // And back on.
    core.input(s, "ansi");
    let out = outputs_for(&core.drain_events(), s).join("");
    assert!(out.contains("ANSI colour is now ON."), "re-toggle reports: {out:?}");
}
