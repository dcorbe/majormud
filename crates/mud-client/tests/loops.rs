//! The loop file format: a hand-editable library of routes that is also
//! the target an imported MegaMud path lands in.
//!
//! Two properties matter more than the rest. A loop must survive a
//! round-trip through TOML unchanged, or hand-editing one is a gamble;
//! and a loop must not be saved unless every leg of it is walkable, or
//! the failure surfaces halfway round a lap with a live character in it.

use mud_client::loops::{Loop, Stop};
use mud_core::content::RoomId;

/// A scratch directory inside `target/`, never `/tmp`.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-loops")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

const CROSSROADS: RoomId = RoomId { map: 1, room: 1076 };
const INTERSECTION: RoomId = RoomId { map: 1, room: 1123 };

fn slum_sweep() -> Loop {
    Loop {
        name: "slum-sweep".into(),
        note: Some("guardsmen".into()),
        finish: Some("1/1072".into()),
        stops: vec![
            Stop::at(CROSSROADS),
            Stop {
                rest: Some(false),
                ..Stop::at(INTERSECTION)
            },
        ],
        ..Loop::new("slum-sweep")
    }
}

#[test]
fn a_loop_round_trips_through_toml() {
    let before = slum_sweep();
    let text = before.to_toml().expect("serialise");
    let after: Loop = Loop::from_toml(&text).expect("parse");
    assert_eq!(before, after);
}

/// TOML puts every bare key before the first table header, so a scalar
/// declared after the stops would serialise INTO the last one. The
/// profile learned this the hard way; the loop file must not relearn it.
#[test]
fn finish_is_written_before_the_stops_not_into_one() {
    let text = slum_sweep().to_toml().expect("serialise");
    let finish = text.find("finish").expect("finish is written");
    let first_stop = text.find("[[stop]]").expect("stops are written");
    assert!(
        finish < first_stop,
        "finish must precede the stop tables:\n{text}"
    );
    let reparsed: Loop = Loop::from_toml(&text).expect("parse");
    assert_eq!(reparsed.finish.as_deref(), Some("1/1072"));
    assert_eq!(reparsed.stops.len(), 2);
}

#[test]
fn a_loop_reads_the_shape_a_human_would_type() {
    // Terse on purpose: an id and nothing else is a legal stop.
    let text = r#"
name = "cavebear"
note = "1/2156, needs a bash north to get in"
finish = "1/2151"

[[stop]]
at = "1/2156"
"#;
    let l: Loop = Loop::from_toml(text).expect("parse");
    assert_eq!(l.name, "cavebear");
    assert_eq!(l.stops.len(), 1);
    assert_eq!(l.stops[0].at, "1/2156");
    assert_eq!(l.stops[0].name, None);
    assert_eq!(l.stops[0].rest, None);
}

#[test]
fn the_rooms_of_a_loop_resolve_to_ids() {
    let l = slum_sweep();
    assert_eq!(l.rooms().expect("ids"), vec![CROSSROADS, INTERSECTION]);
}

#[test]
fn a_bad_room_id_is_refused_by_name() {
    let mut l = slum_sweep();
    l.stops.push(Stop {
        at: "not-a-room".into(),
        ..Stop::at(CROSSROADS)
    });
    let err = l.rooms().expect_err("must refuse");
    assert!(err.contains("not-a-room"), "{err}");
}

// --- the library on disk ---------------------------------------------

#[test]
fn a_saved_loop_can_be_listed_and_loaded_back() {
    let dir = scratch("save-and-load");
    let path = slum_sweep().save(&dir).expect("save");
    assert_eq!(path.file_name().unwrap(), "slum-sweep.toml");

    assert_eq!(mud_client::loops::list(&dir), vec!["slum-sweep".to_string()]);
    let back = mud_client::loops::load(&dir, "slum-sweep").expect("load");
    assert_eq!(back, slum_sweep());
}

#[test]
fn loading_a_loop_that_is_not_there_says_so() {
    let dir = scratch("missing");
    let err = mud_client::loops::load(&dir, "nope").expect_err("must fail");
    assert!(err.contains("nope"), "{err}");
}

/// The name is a file name. Anything that could climb out of the library
/// directory is refused rather than sanitised, because a silently
/// renamed loop is worse than a refused one.
#[test]
fn a_name_that_is_not_a_file_name_is_refused() {
    let dir = scratch("bad-names");
    for bad in ["../escape", "with/slash", "", "."] {
        let l = Loop {
            name: bad.into(),
            stops: vec![Stop::at(CROSSROADS)],
            ..Loop::new(bad)
        };
        assert!(l.save(&dir).is_err(), "{bad:?} should be refused");
    }
}

#[test]
fn listing_an_absent_directory_is_empty_not_an_error() {
    let dir = scratch("gone").join("not-created");
    assert!(mud_client::loops::list(&dir).is_empty());
}

/// The library moved onto the shared config base. Same place as before,
/// one function fewer that knows where that is.
#[test]
fn the_loop_library_sits_under_the_config_directory() {
    assert_eq!(
        mud_client::loops::dir(),
        mud_client::profile::config_dir().join("loops")
    );
    assert!(mud_client::loops::dir().ends_with("mmc/loops"));
}

