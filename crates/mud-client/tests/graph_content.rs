//! Equivalence tests for `RoomGraph::from_content`, the view that
//! replaces `graph.rs`'s hand-written room/message/remoteaction SQL
//! (`2026-08-22-one-path-to-content` Task 4).
//!
//! `whole_graph_matches_the_sql_build` is the proof that matters: it
//! compares `RoomGraph::load` (built from the database) against
//! `RoomGraph::from_content` (built from the same database's decoded
//! `Content`), room for room, exit for exit. It was written and run
//! while `RoomGraph::load` still hand-rolled its own SQL, and it caught
//! nothing wrong -- the two builds agreed over all 26720 rooms. Once
//! `load` was switched to `content_db::load` + `from_content` (deleting
//! the SQL, per Task 4 Step 4), the two sides of this comparison became
//! the same code path; it is kept anyway as a wiring check -- it still
//! catches `load` drifting from calling `from_content` at all -- while
//! `tests/graph.rs`'s 600+ pinned assertions (unchanged, still green) are
//! the surviving regression proof of the original equivalence.
//!
//! The remaining tests here are offline, over hand-built `Content`
//! fixtures (no database), and are what actually mutation-guards the two
//! rules `from_content` add beyond a raw port: the command-exit
//! non-empty-`messageline1` filter, and the `remoteaction` puzzle scan.

use mud_client::graph::{ExitRequirement, RoomGraph};
use mud_core::content::{
    Content, Direction, Exit, Message, MessageId, Room, RoomId, TextBlock, TextBlockId,
};

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

#[test]
fn whole_graph_matches_the_sql_build() {
    let db_graph = RoomGraph::load(&db_path()).expect("sql/content build");
    let content = mud_core::content_db::load(&db_path()).expect("load content");
    let content_graph = RoomGraph::from_content(&content);

    assert_eq!(db_graph.len(), content_graph.len());
    let db_rooms: Vec<_> = db_graph.iter().collect();
    let content_rooms: Vec<_> = content_graph.iter().collect();
    assert_eq!(db_rooms, content_rooms);
}

const HERE: RoomId = RoomId { map: 1, room: 1 };
const THERE: RoomId = RoomId { map: 1, room: 2 };

/// A minimal room with one command exit (type 10, North) pointing at
/// `msg`, built the way `from_content` expects: `trigger_msg` already
/// resolved at load time, exactly as `content_db::load_rooms` does.
fn room_with_command_exit(msg: Option<MessageId>) -> Room {
    let mut room = Room {
        id: HERE,
        name: "Here".into(),
        ..Default::default()
    };
    room.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 10,
        trigger_msg: msg,
        ..Default::default()
    });
    room
}

#[test]
fn a_command_exit_takes_the_trimmed_first_line() {
    let mut content = Content::default();
    content.add_room(room_with_command_exit(Some(MessageId(1))));
    content.add_message(Message {
        id: MessageId(1),
        lines: vec!["  borrow skiff  ".into(), "alias".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let edge = graph.room(HERE).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap();
    assert_eq!(edge.command.as_deref(), Some("borrow skiff"));
}

/// The rule under mutation test: a command exit whose message's first
/// line is blank (after trim) carries no command at all -- never
/// `Some("")`. Flipping the `filter(|s| !s.is_empty())` in
/// `RoomGraph::exit_command` to a no-op must fail this.
#[test]
fn a_blank_first_line_leaves_the_command_exit_unspoken() {
    let mut content = Content::default();
    content.add_room(room_with_command_exit(Some(MessageId(1))));
    content.add_message(Message {
        id: MessageId(1),
        lines: vec!["   ".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let edge = graph.room(HERE).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap();
    assert_eq!(edge.command, None);
}

#[test]
fn a_command_exit_with_no_trigger_message_has_no_command() {
    let mut content = Content::default();
    content.add_room(room_with_command_exit(None));

    let graph = RoomGraph::from_content(&content);
    let edge = graph.room(HERE).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap();
    assert_eq!(edge.command, None);
}

/// A `remoteaction` script in one room's `cmdtext` conceals an exit in
/// ANOTHER room -- the two-room, two-pass shape `load`'s own doc comment
/// describes. Mirrors `nav_doors.rs`'s live-shape fixtures.
#[test]
fn a_remoteaction_script_conceals_the_named_exit() {
    let lever_room: RoomId = RoomId { map: 1, room: 3 };
    let mut content = Content::default();

    let mut target = Room {
        id: HERE,
        name: "Vault".into(),
        ..Default::default()
    };
    // A door exit (type 2) so the puzzle pass overwrites Door, not
    // Hidden -- exercising the "still a door" branch.
    target.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 2,
        ..Default::default()
    });
    content.add_room(target);

    let lever = Room {
        id: lever_room,
        name: "Lever Room".into(),
        command_block: Some(TextBlockId(9)),
        ..Default::default()
    };
    content.add_room(lever);
    content.add_text_block(TextBlock {
        id: TextBlockId(9),
        next: None,
        // remoteaction <target room> <msg> <action> <exit index>; North
        // is direction index 0.
        body: "pull lever:remoteaction 1 0 0 0".into(),
    });

    let graph = RoomGraph::from_content(&content);
    let edge = graph.room(HERE).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap();
    assert!(matches!(edge.requirement, ExitRequirement::Puzzle { .. }));
    let ExitRequirement::Puzzle { actions } = &edge.requirement else {
        unreachable!()
    };
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].room, lever_room);
    assert_eq!(actions[0].commands, vec!["pull lever".to_string()]);
}

/// A `cmdtext` block whose body never mentions `remoteaction` (the
/// overwhelming majority of the 810 shipped `cmdtext` rooms) leaves every
/// exit exactly as `from_exit_type` classified it.
#[test]
fn an_unrelated_cmdtext_block_leaves_exits_alone() {
    let mut content = Content::default();
    let mut target = Room {
        id: HERE,
        name: "Vault".into(),
        command_block: Some(TextBlockId(9)),
        ..Default::default()
    };
    target.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 6, // hidden
        ..Default::default()
    });
    content.add_room(target);
    content.add_text_block(TextBlock {
        id: TextBlockId(9),
        next: None,
        body: "look:some other quest script entirely".into(),
    });

    let graph = RoomGraph::from_content(&content);
    let edge = graph.room(HERE).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap();
    assert_eq!(edge.requirement, ExitRequirement::Hidden { searchable: true });
}
