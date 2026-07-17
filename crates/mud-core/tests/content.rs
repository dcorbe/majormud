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
        room_type: 0,
        shop: None,
        placed_items: vec![],
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
        hitpoints: 9,
        experience: 1,
        exp_multi: 12,
        armour_class: 0,
        damage_resist: 1,
        magic_resist: 30,
        bs_defence: 0,
        energy: 1000,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: Default::default(),
    }
}

#[test]
fn resolved_exits_pass_validation() {
    let mut a = room(1, 1);
    a.exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        exit_type: 0,
        trigger_msg: None,
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
        trigger_msg: None,
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

#[test]
fn element_maps_ids_and_resist_abilities() {
    use mud_core::content::Element;
    assert_eq!(Element::from_i16(4), Some(Element::Magic));
    assert_eq!(Element::from_i16(7), None);
    assert_eq!(Element::Magic.resist_ability(), None); // no case 4 in get_spell_random_modifier
    assert_eq!(Element::Cold.resist_ability(), Some(Ability::from_id(3).unwrap())); // Rcol
    assert_eq!(Element::Fire.resist_ability(), Some(Ability::from_id(5).unwrap())); // Rfir
    assert_eq!(Element::Stone.resist_ability(), Some(Ability::from_id(65).unwrap())); // ResistStone
    assert_eq!(Element::Lightning.resist_ability(), Some(Ability::from_id(66).unwrap())); // Rlit
    assert_eq!(Element::Water.resist_ability(), Some(Ability::from_id(147).unwrap())); // ResistWater
    assert_eq!(Element::Poison.resist_ability(), Some(Ability::from_id(21).unwrap())); // ImmuPoison
}

#[test]
fn match_type_predicates_follow_spec_groupings() {
    use mud_core::content::MatchType;
    assert_eq!(MatchType::from_i16(14), None);
    assert_eq!(MatchType::from_i16(-1), None);
    let mt = |n| MatchType::from_i16(n).unwrap();
    // spellcasting.md §3/§4 groupings
    for n in [6, 7] { assert!(mt(n).is_item()); }
    for n in [3, 5, 9, 10, 11, 12, 13] { assert!(mt(n).room_wide()); }
    for n in [3, 5, 9, 11, 12] { assert!(mt(n).hits_monsters()); }
    for n in [3, 5, 9, 10] { assert!(mt(n).splits_magnitude()); }
    for n in [0, 1, 2, 4, 8] { assert!(!mt(n).room_wide() && !mt(n).is_item()); }
    assert!(!mt(10).hits_monsters());
    assert!(!mt(13).hits_monsters());
    assert!(!mt(11).splits_magnitude());
}

#[test]
fn target_mode_offensive_threshold_is_three() {
    use mud_core::content::TargetMode;
    assert!(TargetMode::from_i16(0).unwrap().is_offensive());
    assert!(TargetMode::from_i16(2).unwrap().is_offensive());
    assert!(!TargetMode::from_i16(3).unwrap().is_offensive());
    assert_eq!(TargetMode::from_i16(4), None);
}

#[test]
fn save_class_maps_typeofresists() {
    use mud_core::content::SaveClass;
    assert_eq!(SaveClass::from_i16(0), Some(SaveClass::None));
    assert_eq!(SaveClass::from_i16(1), Some(SaveClass::IfAntiMagic));
    assert_eq!(SaveClass::from_i16(2), Some(SaveClass::Always));
    assert_eq!(SaveClass::from_i16(3), None);
}

#[test]
fn scale_pair_guards_zero_denominator() {
    use mud_core::content::ScalePair;
    // Magic missile ships per=1, levels=0 — the engine's guard yields 0.
    assert_eq!(ScalePair { per: 1, levels: 0 }.scaled(10), 0);
    assert_eq!(ScalePair { per: 3, levels: 2 }.scaled(10), 15);
    assert_eq!(ScalePair { per: 1, levels: 3 }.scaled(8), 2); // integer division
    assert_eq!(ScalePair::NONE.scaled(50), 0);
}

#[test]
fn scale_pair_duration_divides_before_multiplying() {
    use mud_core::content::ScalePair;
    // §3 min/max: per * L / levels (multiply-first) vs
    // §5 duration: (L / levels) * per (divide-first). per=2, levels=3, L=8
    // distinguishes them: 2*8/3 = 5 but (8/3)*2 = 4.
    let p = ScalePair { per: 2, levels: 3 };
    assert_eq!(p.scaled(8), 5);
    assert_eq!(p.scaled_duration(8), 4);
    // Zero-denominator guard (spell+0xf9 == 0 contributes nothing).
    assert_eq!(ScalePair { per: 2, levels: 0 }.scaled_duration(8), 0);
    assert_eq!(ScalePair::NONE.scaled_duration(50), 0);
}
