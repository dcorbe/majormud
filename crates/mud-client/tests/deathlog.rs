//! One line per death, kept across sessions, and read back for
//! `/recover`.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mud_client::deathlog::{Death, FixWord, death_of, last_in, record_in, stamp};
use mud_client::lost::Fix;
use mud_core::content::RoomId;

fn log_in(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("deathlog").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("deaths.log")
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn the_stamp_is_utc_to_the_second() {
    assert_eq!(stamp(at(1_788_792_127)), "2026-09-07T14:42:07Z");
    assert_eq!(stamp(at(951_782_400)), "2000-02-29T00:00:00Z", "a leap day");
    assert_eq!(stamp(at(0)), "1970-01-01T00:00:00Z");
}

#[test]
fn a_line_round_trips_with_a_room_name_that_has_spaces() {
    let death = Death {
        stamp: "2026-09-07T14:42:07Z".into(),
        character: "beef".into(),
        room: Some(RoomId { map: 1, room: 2810 }),
        name: "Darkwood Forest".into(),
        fix: FixWord::Confirmed,
    };
    assert_eq!(death.line(), "2026-09-07T14:42:07Z beef 1/2810 Darkwood Forest confirmed");
    assert_eq!(Death::parse(&death.line()), Some(death));
}

#[test]
fn a_death_with_no_room_still_writes_a_line() {
    let death = death_of("beef", Fix::Unknown, |_| unreachable!("no room to name"), at(0));
    assert_eq!(death.line(), "1970-01-01T00:00:00Z beef - - unknown");
    let back = Death::parse(&death.line()).unwrap();
    assert_eq!(back.room, None);
    assert_eq!(back.fix, FixWord::Unknown);
}

#[test]
fn a_stale_fix_is_written_as_stale() {
    let death = death_of(
        "beef",
        Fix::Stale(RoomId { map: 1, room: 2811 }),
        |id| format!("Room {}", id.room),
        at(0),
    );
    assert_eq!(death.line(), "1970-01-01T00:00:00Z beef 1/2811 Room 2811 stale");
}

#[test]
fn garbage_does_not_parse() {
    assert_eq!(Death::parse(""), None);
    assert_eq!(Death::parse("2026-09-07T14:42:07Z beef"), None);
    assert_eq!(Death::parse("2026-09-07T14:42:07Z beef 1/2810 Darkwood Forest maybe"), None);
}

#[test]
fn last_is_the_newest_line_with_a_room_for_that_character() {
    let path = log_in("last");
    let deaths = [
        death_of("beef", Fix::Confirmed(RoomId { map: 1, room: 1 }), |_| "One".into(), at(10)),
        death_of("salad", Fix::Confirmed(RoomId { map: 1, room: 2 }), |_| "Two".into(), at(20)),
        death_of("beef", Fix::Stale(RoomId { map: 1, room: 3 }), |_| "Three".into(), at(30)),
        death_of("beef", Fix::Unknown, |_| String::new(), at(40)),
    ];
    for d in &deaths {
        record_in(&path, d).unwrap();
    }
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 4, "append only: {text}");
    let last = last_in(&path, "beef").unwrap();
    assert_eq!(last.room, Some(RoomId { map: 1, room: 3 }), "the unknown one is skipped");
    assert_eq!(last.name, "Three");
    assert_eq!(last_in(&path, "salad").unwrap().room, Some(RoomId { map: 1, room: 2 }));
    assert_eq!(last_in(&path, "nobody"), None);
}

#[test]
fn a_missing_log_has_no_last_death() {
    let path = log_in("missing");
    assert_eq!(last_in(&path, "beef"), None);
}


use mud_client::deathlog::DeathWatch;
use mud_client::events::Event;

fn prompt(hp: i32) -> Event {
    Event::Prompt {
        hp,
        mana: None,
        status: None,
    }
}

fn line(text: &str) -> Event {
    Event::Line(text.to_string())
}

/// The board says a death three ways, and all three usually arrive for
/// one death. The first fires, the rest are held until the character
/// is alive again.
#[test]
fn the_first_of_the_three_wordings_fires_and_the_rest_are_held() {
    let mut w = DeathWatch::default();
    assert!(w.on_event(&line("You have been killed."), "Beef"));
    assert!(!w.on_event(&line("Beef is dead."), "Beef"));
    assert!(!w.on_event(&prompt(0), "Beef"));
    assert!(!w.on_event(&prompt(-3), "Beef"));
}

#[test]
fn each_wording_fires_on_its_own() {
    for ev in [line("You have been killed."), line("Beef is dead."), prompt(0)] {
        let mut w = DeathWatch::default();
        assert!(w.on_event(&ev, "Beef"), "{ev:?}");
    }
}

#[test]
fn a_prompt_above_zero_re_arms_the_watch() {
    let mut w = DeathWatch::default();
    assert!(w.on_event(&prompt(0), "Beef"));
    assert!(!w.on_event(&prompt(0), "Beef"));
    assert!(!w.on_event(&prompt(22), "Beef"), "alive again is not a death");
    assert!(w.on_event(&prompt(0), "Beef"), "the next death counts");
}

#[test]
fn somebody_elses_death_is_not_ours() {
    let mut w = DeathWatch::default();
    assert!(!w.on_event(&line("Salad is dead."), "Beef"));
    assert!(!w.on_event(&line("The giant rat is dead."), "Beef"));
    assert!(!w.on_event(&prompt(22), "Beef"));
}

#[test]
fn a_death_marked_from_outside_is_not_fired_again() {
    let mut w = DeathWatch::default();
    w.mark_dead();
    assert!(!w.on_event(&prompt(0), "Beef"));
    assert!(!w.on_event(&prompt(22), "Beef"));
    assert!(w.on_event(&prompt(0), "Beef"));
}

