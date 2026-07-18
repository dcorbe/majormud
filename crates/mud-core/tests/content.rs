//! Tests for the content model and its cross-reference validation.

use mud_core::ability::Ability;
use mud_core::content::{
    Content, ContentError, Direction, Exit, Message, MessageId, Monster, MonsterId, Room, RoomId,
    Spell, SpellId,
};

fn room(map: u16, num: u16) -> Room {
    Room {
        id: RoomId { map, room: num },
        name: format!("room {map}/{num}"),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
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
        ..Default::default()
    }
}

fn spell(n: u16) -> Spell {
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, TargetMode};
    Spell {
        id: SpellId(n),
        name: format!("spell {n}"),
        short_name: String::new(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![],
        level_cap: 0,
        round_cost: 0,
        required_power: 0,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200, // >= 200 = auto-succeed: cast fixtures are deterministic by default
        duration_per_level: 0,
        match_type: MatchType::Single0,
        duration: 0,
        element: Element::Cold,
        class_gate_group: 0,
        mana_cost: 0,
        max_increase: ScalePair::NONE,
        required_class_level: 0,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0, // even = the render_cast_line contract
    }
}

#[test]
fn resolved_exits_pass_validation() {
    let mut a = room(1, 1);
    a.exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
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
        ..Default::default()
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
    let mut content = Content::default();
    let mut s = spell(1055);
    s.name = "BCNS".into();
    s.cast_msg_b = Some(MessageId(3499));
    content.add_spell(s);
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn dangling_boss_monster_is_reported() {
    // monsters.md §1/§2: room+0x5c8 (`permnpc`) names the room's unique/boss
    // template; a nonzero id must resolve.
    let mut r = room(1, 1);
    r.boss_monster = Some(MonsterId(77));
    let mut content = Content::default();
    content.add_room(r);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingRoomMonster {
            room: RoomId { map: 1, room: 1 },
            field: "permnpc",
            monster: MonsterId(77),
        }]
    );
}

#[test]
fn dangling_forced_monster_is_reported() {
    // monsters.md §1: room+0x466 (`bynumber`) forces the spawn template.
    let mut r = room(1, 1);
    r.forced_monster = Some(MonsterId(88));
    let mut content = Content::default();
    content.add_room(r);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingRoomMonster {
            room: RoomId { map: 1, room: 1 },
            field: "bynumber",
            monster: MonsterId(88),
        }]
    );
}

#[test]
fn resolved_room_monster_refs_pass() {
    let mut r = room(1, 1);
    r.boss_monster = Some(MonsterId(77));
    r.forced_monster = Some(MonsterId(88));
    let mut content = Content::default();
    content.add_room(r);
    content.add_monster(monster(77));
    content.add_monster(monster(88));
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn room_spawn_fields_default_inert() {
    // A default room neither spawns nor leashes: no zone, no caps, no boss.
    let r = Room::default();
    assert_eq!(r.spawn_zone, 0);
    assert_eq!(r.spawn_cap, 0);
    assert_eq!(r.min_level, 0);
    assert_eq!(r.max_level, 0);
    assert_eq!(r.respawn_delay, 0);
    assert_eq!(r.forced_monster, None);
    assert_eq!(r.boss_monster, None);
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

#[test]
fn dangling_spell_reference_fails_validation() {
    use mud_core::content::ContentError;
    let mut content = Content::default();
    let mut s = spell(1);
    // EndCast (151) pointing at a spell that doesn't exist.
    s.abilities = vec![(Ability::from_id(151).unwrap(), 999)];
    content.add_spell(s);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingSpellRef {
            spell: SpellId(1),
            ability: Ability::from_id(151).unwrap(),
            referenced: 999,
        }]
    );
}

#[test]
fn negative_spell_reference_is_dangling() {
    use mud_core::content::ContentError;
    let mut content = Content::default();
    let mut s = spell(1);
    // Negative values can never name a spell (ids are u16); they must be
    // reported structurally, not wrapped through `as u16`.
    s.abilities = vec![(Ability::from_id(151).unwrap(), -1)];
    content.add_spell(s);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingSpellRef {
            spell: SpellId(1),
            ability: Ability::from_id(151).unwrap(),
            referenced: -1,
        }]
    );
}

#[test]
fn dangling_give_temp_spell_reference_fails_validation() {
    use mud_core::content::ContentError;
    let mut content = Content::default();
    let mut s = spell(1);
    // GiveTempSpell (160) pointing at a spell that doesn't exist.
    s.abilities = vec![(Ability::from_id(160).unwrap(), 999)];
    content.add_spell(s);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingSpellRef {
            spell: SpellId(1),
            ability: Ability::from_id(160).unwrap(),
            referenced: 999,
        }]
    );
}

#[test]
fn zero_spell_reference_is_the_none_sentinel() {
    // 14 shipped slots carry EndCast/RemovesSpell value 0 = "none".
    let mut content = Content::default();
    let mut s = spell(1);
    s.abilities = vec![(Ability::from_id(151).unwrap(), 0)];
    content.add_spell(s);
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn resolving_spell_references_pass() {
    let mut content = Content::default();
    let mut s = spell(1);
    s.abilities = vec![
        (Ability::from_id(122).unwrap(), 2), // RemovesSpell -> spell 2
        (Ability::from_id(153).unwrap(), 2), // KillSpell -> spell 2
        (Ability::from_id(160).unwrap(), 2), // GiveTempSpell -> spell 2
    ];
    content.add_spell(s);
    content.add_spell(spell(2));
    assert_eq!(content.validate(), vec![]);
}
