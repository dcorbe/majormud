//! Tests for the content model and its cross-reference validation.

use mud_core::ability::Ability;
use mud_core::content::{
    Content, ContentError, Direction, Exit, Message, MessageId, Monster, MonsterId, Room, RoomId,
    Spell, SpellId, TextBlock, TextBlockId,
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
    for n in [6, 7] { assert!(mt(n).accepts_item()); }
    for n in [3, 5, 9, 10, 11, 12, 13] { assert!(mt(n).room_wide()); }
    for n in [3, 5, 9, 11, 12] { assert!(mt(n).hits_monsters()); }
    for n in [3, 5, 9, 10] { assert!(mt(n).splits_magnitude()); }
    for n in [0, 1, 2, 4, 8] { assert!(!mt(n).room_wide() && !mt(n).accepts_item()); }
    assert!(!mt(10).hits_monsters());
    assert!(!mt(13).hits_monsters());
    assert!(!mt(11).splits_magnitude());
}

#[test]
fn match_type_acceptance_sets_match_the_cast_entry_points() {
    use mud_core::content::MatchType;
    let mt = |n| MatchType::from_i16(n).unwrap();
    // cast_monster_target 43205 -> {4, 6, 8}.
    for n in 0..=13 {
        assert_eq!(mt(n).accepts_monster(), [4, 6, 8].contains(&n), "monster gate, match {n}");
    }
    // cast_user_target 41460 -> {0, 2, 6, 8}. Match 1 is the self-only
    // buff band (barkskin, stoneskin) and is deliberately excluded.
    for n in 0..=13 {
        assert_eq!(mt(n).accepts_user(), [0, 2, 6, 8].contains(&n), "user gate, match {n}");
    }
    // cast_item_target 44367 -> {6, 7}.
    for n in 0..=13 {
        assert_eq!(mt(n).accepts_item(), [6, 7].contains(&n), "item gate, match {n}");
    }
}

#[test]
fn preferred_find_mirrors_get_spell_match_type() {
    use mud_core::content::{FindScope, MatchType};
    let mt = |n| MatchType::from_i16(n).unwrap();
    // get_spell_match_type 45018-45053, modelled bits only: 0x1 monsters,
    // 0x2 users, 0x4 carried items.
    let users = FindScope { monsters: false, users: true, items: false };
    for n in [0, 1, 2] {
        assert_eq!(mt(n).preferred_find(), users, "0x02/0x82, match {n}");
    }
    // 0x801
    assert_eq!(
        mt(4).preferred_find(),
        FindScope { monsters: true, users: false, items: false }
    );
    // 0xf837
    assert_eq!(mt(6).preferred_find(), FindScope::UNIVERSAL);
    // 0x14
    assert_eq!(
        mt(7).preferred_find(),
        FindScope { monsters: false, users: false, items: true }
    );
    // 0x803
    assert_eq!(
        mt(8).preferred_find(),
        FindScope { monsters: true, users: true, items: false }
    );
    // The seven area types return 0 — nothing is searched, so the
    // dispatcher's universal retry does all the work (MEASURED §8.13).
    for n in [3, 5, 9, 10, 11, 12, 13] {
        assert!(mt(n).preferred_find().is_empty(), "area mask 0, match {n}");
        assert!(!mt(n).accepts_monster() && !mt(n).accepts_user() && !mt(n).accepts_item());
    }
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

// --- M7 slice 1: text blocks (WCCTEXT2) ---

fn text_block(n: u16) -> TextBlock {
    TextBlock {
        id: TextBlockId(n),
        next: None,
        body: format!("block {n}\n"),
    }
}

#[test]
fn dangling_monster_text_block_is_reported() {
    let mut m = monster(5);
    m.name_block = Some(TextBlockId(2000));
    let mut content = Content::default();
    content.add_monster(m);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingMonsterTextBlock {
            monster: MonsterId(5),
            block: TextBlockId(2000),
        }]
    );
}

#[test]
fn resolved_monster_text_blocks_pass() {
    let mut m = monster(5);
    m.name_block = Some(TextBlockId(2000));
    m.greet_block = Some(TextBlockId(31));
    m.talk_block = Some(TextBlockId(32));
    let mut content = Content::default();
    content.add_monster(m);
    content.add_text_block(text_block(2000));
    content.add_text_block(text_block(31));
    content.add_text_block(text_block(32));
    assert_eq!(content.validate(), vec![]);
}

#[test]
fn dangling_text_block_next_is_reported() {
    let mut b = text_block(10);
    b.next = Some(TextBlockId(11));
    let mut content = Content::default();
    content.add_text_block(b);
    assert_eq!(
        content.validate(),
        vec![ContentError::DanglingTextBlockNext {
            block: TextBlockId(10),
            next: TextBlockId(11),
        }]
    );
}

#[test]
fn known_dangling_text_block_next_links_are_allowlisted() {
    // The shipped data has exactly four dangling seq-0 next-links
    // (133->134, 440->441, 2962->2963, 9637->9638).
    let mut content = Content::default();
    for (id, next) in [(133, 134), (440, 441), (2962, 2963), (9637, 9638)] {
        let mut b = text_block(id);
        b.next = Some(TextBlockId(next));
        content.add_text_block(b);
    }
    assert_eq!(content.validate(), vec![]);
}
