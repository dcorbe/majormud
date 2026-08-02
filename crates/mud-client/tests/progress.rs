//! The live progress feed.
//!
//! `mmc farm` printed nothing at all until the run ended, so a farm that
//! was working and a farm that was wedged looked identical for as long as
//! it ran. These pin what an operator watching a run should see.

use mud_client::events::{Actor, Event, RoomView};
use mud_client::progress::ProgressView;

fn room(name: &str, also_here: &[&str]) -> Event {
    Event::RoomSeen(RoomView {
        name: name.into(),
        exits: vec!["south".into()],
        also_here: also_here.iter().map(|s| (*s).to_string()).collect(),
        items: vec![],
        also_here_sgr: Vec::new(),
    })
}

#[test]
fn arriving_somewhere_is_reported_with_who_is_there() {
    let mut view = ProgressView::new(false);
    let line = view.on_event(&room("Small Cavern", &["cave bear"])).unwrap();
    assert!(line.contains("Small Cavern"), "{line}");
    assert!(line.contains("cave bear"), "{line}");
}

#[test]
fn combat_damage_is_reported() {
    let mut view = ProgressView::new(false);
    let line = view
        .on_event(&Event::CombatHit {
            attacker: Actor::You,
            target: Actor::Other("The cave bear".into()),
            damage: 7,
        })
        .unwrap();
    assert!(line.contains("cave bear"), "{line}");
    assert!(line.contains('7'), "{line}");
}

/// HP is the number an operator is actually watching, but a prompt
/// arrives after almost every line and most of them repeat the same
/// value. Reporting each one would bury the run in noise.
#[test]
fn hp_is_reported_only_when_it_changes() {
    let mut view = ProgressView::new(false);
    assert!(view.on_event(&Event::Prompt { hp: 33, mana: None }).is_some());
    assert!(
        view.on_event(&Event::Prompt { hp: 33, mana: None }).is_none(),
        "an unchanged prompt is noise"
    );
    assert!(view.on_event(&Event::Prompt { hp: 21, mana: None }).is_some());
}

/// Flood control means the board DROPPED input. That is never noise.
#[test]
fn flood_control_is_always_reported() {
    let mut view = ProgressView::new(false);
    assert!(view.on_event(&Event::SlowDown).is_some());
}

/// A kill is the thing the run exists to produce, so it shows even in
/// the quiet feed, while an ordinary miss does not.
#[test]
fn a_kill_shows_but_a_miss_does_not() {
    let mut view = ProgressView::new(false);
    assert!(
        view.on_event(&Event::Line(
            "The cave bear falls to the ground with a grunt!".into()
        ))
        .is_some()
    );
    assert!(
        view.on_event(&Event::CombatMiss {
            line: "You swing at the cave bear and miss!".into()
        })
        .is_none()
    );
}

/// Watch mode is the firehose: everything the board said, verbatim.
#[test]
fn watch_mode_shows_every_line() {
    let mut view = ProgressView::new(true);
    assert!(
        view.on_event(&Event::CombatMiss {
            line: "You swing at the cave bear and miss!".into()
        })
        .is_some()
    );
    assert!(
        view.on_event(&Event::Line("The floor is littered with bones.".into()))
            .is_some()
    );
}

/// Description text is not progress. Without this the room block's prose
/// scrolls past on every arrival and the useful lines are lost in it.
#[test]
fn ordinary_prose_is_quiet_by_default() {
    let mut view = ProgressView::new(false);
    assert!(
        view.on_event(&Event::Line("The floor is littered with bones.".into()))
            .is_none()
    );
}
