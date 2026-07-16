//! Tests for the content model and its cross-reference validation.

use mud_core::ability::Ability;
use mud_core::content::{
    Content, ContentError, Direction, Exit, Message, MessageId, Monster, MonsterId, Room, RoomId,
};

fn room(map: u16, num: u16) -> Room {
    Room {
        id: RoomId { map, room: num },
        name: format!("room {map}/{num}"),
        description: vec![],
        shop: None,
        exits: Default::default(),
    }
}

fn message(id: u16) -> Message {
    Message {
        id: MessageId(id),
        lines: vec!["text".into()],
    }
}

fn monster(id: u16) -> Monster {
    Monster {
        id: MonsterId(id),
        name: format!("monster {id}"),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
    }
}

#[test]
fn resolved_exits_pass_validation() {
    let mut a = room(1, 1);
    a.exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        exit_type: 0,
    });
    let mut content = Content::default();
    content.add_room(a);
    content.add_room(room(1, 2));
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn dangling_exit_is_reported() {
    let mut a = room(1, 1);
    a.exits[Direction::Up as usize] = Some(Exit {
        dest: RoomId { map: 9, room: 9 },
        exit_type: 0,
    });
    let mut content = Content::default();
    content.add_room(a);
    assert_eq!(
        content.validate(),
        vec![ContentError::UnresolvedExit {
            room: RoomId { map: 1, room: 1 },
            direction: Direction::Up,
            dest: RoomId { map: 9, room: 9 },
        }]
    );
}

#[test]
fn dangling_monster_message_is_reported() {
    let mut m = monster(5);
    m.death_msg = Some(MessageId(100));
    let mut content = Content::default();
    content.add_monster(m);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingMonsterMessage {
            monster: MonsterId(5),
            message: MessageId(100),
        }]
    );
}

#[test]
fn resolved_monster_message_passes() {
    let mut m = monster(5);
    m.move_msg = Some(MessageId(100));
    let mut content = Content::default();
    content.add_monster(m);
    content.add_message(message(100));
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn known_dangling_refs_are_allowlisted() {
    // The shipped 1.11p data has exactly two dangling death messages
    // (monsters 789 and 1017). They must not fail validation.
    let mut m = monster(789);
    m.death_msg = Some(MessageId(3551));
    let mut content = Content::default();
    content.add_monster(m);
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn known_dangling_spell_message_is_allowlisted() {
    // Spell 1055 "BCNS" references cast message 3499, which does not exist
    // in the shipped data.
    use mud_core::content::{Spell, SpellId};
    let mut content = Content::default();
    content.add_spell(Spell {
        id: SpellId(1055),
        name: "BCNS".into(),
        short_name: String::new(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(3499)),
        abilities: vec![],
    });
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn direction_has_ten_variants_with_opposites() {
    assert_eq!(Direction::ALL.len(), 10);
    for d in Direction::ALL {
        assert_eq!(d.opposite().opposite(), d);
    }
    assert_eq!(Direction::North.opposite(), Direction::South);
    assert_eq!(Direction::NorthEast.opposite(), Direction::SouthWest);
    assert_eq!(Direction::Up.opposite(), Direction::Down);
}

#[test]
fn ability_pairs_use_the_generated_enum() {
    let mut m = monster(1);
    m.abilities = vec![(Ability::from_id(21).unwrap(), 0)];
    let mut content = Content::default();
    content.add_monster(m);
    assert_eq!(content.validate(), vec![]);
}
