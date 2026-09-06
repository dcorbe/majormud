//! Tests for `RoomGraph::from_content` over hand-built `Content` fixtures
//! (no database). They guard the two rules `from_content` adds beyond a
//! raw port: the command-exit non-empty-`messageline1` filter, and the
//! `remoteaction` puzzle scan.

use mud_client::graph::{ExitRequirement, RoomGraph};
use mud_core::content::{
    Content, Direction, Exit, ItemId, Message, MessageId, Room, RoomId, TextBlock, TextBlockId,
};

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
    // A gate exit, type 7, so the puzzle pass overwrites Door. A type 2
    // key door would be left alone: mud-core's remote action dispatch
    // only answers for types 6, 7 and 0xb.
    target.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 7,
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
    let ExitRequirement::Puzzle(puzzle) = &edge.requirement else {
        panic!("expected a puzzle, got {:?}", edge.requirement);
    };
    assert_eq!(puzzle.actions.len(), 1);
    assert_eq!(puzzle.actions[0].room, lever_room);
    assert_eq!(puzzle.actions[0].number, 0);
    assert_eq!(puzzle.actions[0].phrases, vec!["pull lever".to_string()]);
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
    assert_eq!(edge.requirement, ExitRequirement::Hidden);
}

/// The room that started this, 1/506 Secret Passage. Its west slot is a
/// type 12 remote action: para1 names the phrase message, para2 = 11
/// is action 1 on exit 1 which is south, para3 names the response. The
/// south exit is type 6 with state 16, which no search clears. The slot
/// is a control, not an exit, so the graph has no west edge at all.
#[test]
fn a_type_12_slot_becomes_a_puzzle_on_its_target_and_no_edge_of_its_own() {
    use mud_client::puzzle::PuzzleAction;

    let passage = RoomId { map: 1, room: 506 };
    let hallway = RoomId { map: 1, room: 507 };
    let mut content = Content::default();
    let mut room = Room {
        id: passage,
        name: "Secret Passage".into(),
        ..Default::default()
    };
    room.exits[Direction::South as usize] = Some(Exit {
        dest: hallway,
        exit_type: 6,
        param: 16,
        ..Default::default()
    });
    room.exits[Direction::West as usize] = Some(Exit {
        dest: passage,
        exit_type: 12,
        param: 8259,
        param2: 11,
        param3: 8260,
        param4: 0,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: hallway,
        name: "Wooden Hallway".into(),
        ..Default::default()
    });
    content.add_message(Message {
        id: MessageId(8259),
        lines: vec!["push button".into(), "press button".into(), "".into()],
    });
    content.add_message(Message {
        id: MessageId(8260),
        lines: vec!["You push the button.".into(), "%s pushes a button.".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let room = graph.room(passage).unwrap();
    assert!(room.exits[Direction::West as usize].is_none(), "a slot is not an edge");
    let ExitRequirement::Puzzle(puzzle) = &room.exits[Direction::South as usize]
        .as_ref()
        .unwrap()
        .requirement
    else {
        panic!("the south wall should be a puzzle");
    };
    assert_eq!(puzzle.word, 16);
    assert_eq!(
        puzzle.actions,
        vec![PuzzleAction {
            room: passage,
            number: 1,
            phrases: vec!["push button".into(), "press button".into()],
            item: None,
            reply: Some("You push the button.".into()),
            hops: Some(0),
        }]
    );
}

/// The Crypt pair. 1/1044 holds `pull lever` as action 1 and 1/1038 as
/// action 2, both on 1/1056 north with state 48. Each lever room is one
/// step from the hallway, so both actions count one hop and sort by
/// room id between themselves.
#[test]
fn a_lever_in_another_room_counts_its_hops() {
    let hall = RoomId { map: 1, room: 1056 };
    let beyond = RoomId { map: 1, room: 1063 };
    let lever_a = RoomId { map: 1, room: 1044 };
    let lever_b = RoomId { map: 1, room: 1038 };
    let mut content = Content::default();

    let mut hall_room = Room {
        id: hall,
        name: "Crypt, Stone Hallway".into(),
        ..Default::default()
    };
    hall_room.exits[Direction::North as usize] = Some(Exit {
        dest: beyond,
        exit_type: 6,
        param: 48,
        param2: -2,
        ..Default::default()
    });
    hall_room.exits[Direction::East as usize] = Some(Exit {
        dest: lever_a,
        ..Default::default()
    });
    hall_room.exits[Direction::West as usize] = Some(Exit {
        dest: lever_b,
        ..Default::default()
    });
    content.add_room(hall_room);
    content.add_room(Room {
        id: beyond,
        name: "Crypt, Dark Passage".into(),
        ..Default::default()
    });

    let mut alcove = |id: RoomId, back: Direction, para2: i32| {
        let mut room = Room {
            id,
            name: "Crypt, Alcove".into(),
            ..Default::default()
        };
        room.exits[back as usize] = Some(Exit {
            dest: hall,
            ..Default::default()
        });
        room.exits[Direction::South as usize] = Some(Exit {
            dest: hall,
            exit_type: 12,
            param: 100,
            param2: para2,
            param3: 101,
            ..Default::default()
        });
        content.add_room(room);
    };
    // para2 = 10 is action 1 on exit 0, north. para2 = 20 is action 2.
    alcove(lever_a, Direction::West, 10);
    alcove(lever_b, Direction::East, 20);
    content.add_message(Message {
        id: MessageId(100),
        lines: vec!["pull lever".into()],
    });
    content.add_message(Message {
        id: MessageId(101),
        lines: vec!["You pull the lever. Off in the distance you hear a small click.".into()],
    });

    let graph = RoomGraph::from_content(&content);
    for id in [lever_a, lever_b] {
        assert!(
            graph.room(id).unwrap().exits[Direction::South as usize].is_none(),
            "the slot in {id:?} must not be an edge"
        );
    }
    let ExitRequirement::Puzzle(puzzle) = &graph.room(hall).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap()
        .requirement
    else {
        panic!("the north wall should be a puzzle");
    };
    assert_eq!(puzzle.word, 48);
    let summary: Vec<(RoomId, u8, Option<u32>)> =
        puzzle.actions.iter().map(|a| (a.room, a.number, a.hops)).collect();
    assert_eq!(
        summary,
        vec![(lever_b, 2, Some(1)), (lever_a, 1, Some(1))],
        "equal hops sort by room id"
    );
    let plan = puzzle
        .plan(&mud_client::graph::Capabilities::unrestricted())
        .expect("both levers are free");
    assert_eq!(plan.iter().map(|a| a.number).collect::<Vec<_>>(), vec![2, 1]);
}

/// A slot whose para4 names an item records it, so the plan can refuse
/// the exit to a walker without one.
#[test]
fn a_slot_that_needs_an_item_records_it() {
    let here = RoomId { map: 1, room: 1 };
    let there = RoomId { map: 1, room: 2 };
    let mut content = Content::default();
    let mut room = Room {
        id: here,
        name: "Fork Room".into(),
        ..Default::default()
    };
    room.exits[Direction::North as usize] = Some(Exit {
        dest: there,
        exit_type: 6,
        param: 16,
        ..Default::default()
    });
    room.exits[Direction::Up as usize] = Some(Exit {
        dest: here,
        exit_type: 12,
        param: 5,
        param2: 10,
        param4: 500,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: there,
        name: "Beyond".into(),
        ..Default::default()
    });
    content.add_message(Message {
        id: MessageId(5),
        lines: vec!["use fork".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let ExitRequirement::Puzzle(puzzle) = &graph.room(here).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap()
        .requirement
    else {
        panic!("expected a puzzle");
    };
    assert_eq!(puzzle.actions[0].item, Some(mud_core::content::ItemId(500)));
    assert_eq!(puzzle.actions[0].reply, None, "no response message, no reply");
}

/// mud-core's remote action dispatch does nothing for a plain exit, so
/// a slot aimed at one leaves it plain. The slot still vanishes.
#[test]
fn a_slot_onto_a_plain_exit_changes_nothing() {
    let here = RoomId { map: 1, room: 1 };
    let there = RoomId { map: 1, room: 2 };
    let mut content = Content::default();
    let mut room = Room {
        id: here,
        name: "Plain Room".into(),
        ..Default::default()
    };
    room.exits[Direction::North as usize] = Some(Exit {
        dest: there,
        exit_type: 0,
        ..Default::default()
    });
    room.exits[Direction::Up as usize] = Some(Exit {
        dest: here,
        exit_type: 12,
        param: 5,
        param2: 10,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: there,
        name: "Beyond".into(),
        ..Default::default()
    });
    content.add_message(Message {
        id: MessageId(5),
        lines: vec!["push button".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let room = graph.room(here).unwrap();
    assert!(room.exits[Direction::Up as usize].is_none());
    assert_eq!(
        room.exits[Direction::North as usize].as_ref().unwrap().requirement,
        ExitRequirement::None
    );
}

/// A lever room no walk reaches from the exit's room leaves the action
/// with no hops, and routing then refuses the exit.
#[test]
fn an_unreachable_lever_room_has_no_hops_and_the_exit_is_impassable() {
    use mud_client::graph::{Capabilities, Cost, exit_cost_for};

    let here = RoomId { map: 1, room: 1 };
    let there = RoomId { map: 1, room: 2 };
    let island = RoomId { map: 1, room: 3 };
    let mut content = Content::default();
    let mut room = Room {
        id: here,
        name: "Gate Room".into(),
        ..Default::default()
    };
    room.exits[Direction::North as usize] = Some(Exit {
        dest: there,
        exit_type: 6,
        param: 16,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: there,
        name: "Beyond".into(),
        ..Default::default()
    });
    let mut lever = Room {
        id: island,
        name: "Island".into(),
        ..Default::default()
    };
    lever.exits[Direction::Down as usize] = Some(Exit {
        dest: here,
        exit_type: 12,
        param: 5,
        param2: 10,
        ..Default::default()
    });
    content.add_room(lever);
    content.add_message(Message {
        id: MessageId(5),
        lines: vec!["pull lever".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let edge = graph.room(here).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap();
    let ExitRequirement::Puzzle(puzzle) = &edge.requirement else {
        panic!("expected a puzzle");
    };
    assert_eq!(puzzle.actions[0].hops, None);
    assert_eq!(
        exit_cost_for(&edge.requirement, 6, here, Direction::North, &Capabilities::unrestricted()),
        Cost::Impassable
    );
}

/// The cmdtext form names a message pair whose second line is what the
/// actor hears, and the action number is the directive's own.
#[test]
fn a_remoteaction_script_records_the_actor_line_and_the_number() {
    let lever_room: RoomId = RoomId { map: 1, room: 3 };
    let mut content = Content::default();
    let mut target = Room {
        id: HERE,
        name: "Vault".into(),
        ..Default::default()
    };
    target.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 6,
        param: 32,
        ..Default::default()
    });
    target.exits[Direction::East as usize] = Some(Exit {
        dest: lever_room,
        ..Default::default()
    });
    content.add_room(target);
    let mut lever = Room {
        id: lever_room,
        name: "Lever Room".into(),
        command_block: Some(TextBlockId(9)),
        ..Default::default()
    };
    lever.exits[Direction::West as usize] = Some(Exit {
        dest: HERE,
        ..Default::default()
    });
    content.add_room(lever);
    content.add_text_block(TextBlock {
        id: TextBlockId(9),
        next: None,
        body: "pull lever:remoteaction 1 7 2 0".into(),
    });
    content.add_message(Message {
        id: MessageId(7),
        lines: vec!["%s pulls a lever.".into(), "You pull the lever.".into()],
    });

    let graph = RoomGraph::from_content(&content);
    let ExitRequirement::Puzzle(puzzle) = &graph.room(HERE).unwrap().exits[Direction::North as usize]
        .as_ref()
        .unwrap()
        .requirement
    else {
        panic!("expected a puzzle");
    };
    assert_eq!(puzzle.word, 32);
    assert_eq!(puzzle.actions[0].number, 2);
    assert_eq!(puzzle.actions[0].reply.as_deref(), Some("You pull the lever."));
    assert_eq!(puzzle.actions[0].hops, Some(1));
}

/// The lock fields the room record already carries become
/// requirements. The Black House door on Slum Street, 1/1224 east, is
/// the captured key door: type 2, key item 172, pick modifier -99. The
/// 88-bash door at 1/1119 north is a locked type 7 with modifier -30.
/// A type 3 exit wants an item carried, and one whose item id is 0
/// wants nothing.
#[test]
fn lock_fields_become_door_key_door_and_item_gate_requirements() {
    let mut content = Content::default();
    let mut room = Room {
        id: HERE,
        name: "Slum Street".into(),
        ..Default::default()
    };
    room.exits[Direction::East as usize] = Some(Exit {
        dest: THERE,
        exit_type: 2,
        param: 172,
        param2: 2,
        param3: -99,
        ..Default::default()
    });
    room.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 3,
        param: 1054,
        ..Default::default()
    });
    room.exits[Direction::South as usize] = Some(Exit {
        dest: THERE,
        exit_type: 3,
        param: 0,
        ..Default::default()
    });
    room.exits[Direction::West as usize] = Some(Exit {
        dest: THERE,
        exit_type: 7,
        param: 2,
        param2: -30,
        ..Default::default()
    });
    room.exits[Direction::Up as usize] = Some(Exit {
        dest: THERE,
        exit_type: 7,
        param: 0,
        param2: 30,
        ..Default::default()
    });
    room.exits[Direction::Down as usize] = Some(Exit {
        dest: THERE,
        exit_type: 0xb,
        param: 2,
        param2: -999,
        ..Default::default()
    });
    content.add_room(room);
    content.add_room(Room {
        id: THERE,
        name: "Black House".into(),
        ..Default::default()
    });

    let graph = RoomGraph::from_content(&content);
    let exits = &graph.room(HERE).unwrap().exits;
    let req = |d: Direction| exits[d as usize].as_ref().unwrap().requirement.clone();
    assert_eq!(
        req(Direction::East),
        ExitRequirement::KeyDoor { key: ItemId(172), pick: -99 }
    );
    assert_eq!(req(Direction::North), ExitRequirement::ItemGate { item: ItemId(1054) });
    assert_eq!(req(Direction::South), ExitRequirement::None, "item 0 wants nothing");
    assert_eq!(req(Direction::West), ExitRequirement::Door { locked: true, pick: -30 });
    assert_eq!(req(Direction::Up), ExitRequirement::Door { locked: false, pick: 30 });
    assert_eq!(req(Direction::Down), ExitRequirement::Door { locked: true, pick: -999 });
}
