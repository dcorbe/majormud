//! Tests for engagement and the 5-second combat round
//! (`re/docs/combat_rounds.md` + oracle transcripts).

use mud_core::content::{
    AttackForm, Class, ClassId, Content, Monster, MonsterId, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

/// A punching bag: huge HP, hits back for exactly 1-8 like the kobold.
fn kobold() -> Monster {
    Monster {
        id: MonsterId(7),
        name: "kobold thief".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 5000,
        experience: 40,
        exp_multi: 1,
        armour_class: 10,
        damage_resist: 1,
        magic_resist: 10,
        bs_defence: 0,
        energy: 1000,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [
            AttackForm {
                kind: 1,
                accuracy: 15,
                weight: 100,
                min_damage: 1,
                max_damage: 8,
                hit_msg: None,
                miss_msg: None,
                energy: 666,
            },
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Arena".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_monster(kobold());
    content.add_race(Race {
        id: RaceId(2),
        name: "Dwarf".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 30,
        },
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
        abilities: vec![],
        hp_per_level: 6,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 6,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        ..CoreConfig::default()
    }
}

fn create(core: &mut Core, name: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.input(s, "No");
    core.drain_events();
    s
}

fn text_to(events: &[Event], session: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session: s, text } if *s == session => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn run_rounds(core: &mut Core, n: u64) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..(n * 5) {
        core.tick();
        all.extend(core.drain_events());
    }
    all
}

#[test]
fn fighter_numbers_match_the_decompile() {
    // Dwarf Warrior L1 (combat 6, Str 50, Agl 30), naked, unencumbered:
    // skill = 1 + 15 = 16
    // accuracy = 0 + 2*((6-1)*1 + 12 + 0 + 8 - 2) + (30-50)/6 = 46 - 3 = 43
    // damage 1-4 (Str 50 adds nothing); crit = dodge_base = 1
    // EU = 1200*1000 / ((6*1+45)*(30+150)*1500/9000) = 1200000/1530 = 784
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let (fighter, eu) = core.combat_debug(s);
    assert_eq!(fighter.accuracy, 43);
    assert_eq!(fighter.min_damage, 1);
    assert_eq!(fighter.max_damage, 4);
    assert_eq!(fighter.crit_rating, 1);
    assert_eq!(eu, 784);
}

#[test]
fn attack_engages_with_first_strike() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });

    core.input(s, "attack kobold");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
    // restart_autocombat: the opening swing happens immediately.
    assert!(
        shown.contains("You punch kobold thief for")
            || shown.contains("You swing at kobold thief!")
            || shown.contains("Your swing at kobold thief hits, but glances off its armour."),
        "first strike message: {shown:?}"
    );
}

#[test]
fn attack_matches_name_by_word_prefix() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack thief");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}

#[test]
fn unresolved_attack_falls_through_to_say() {
    // Oracle: "a kobold" in a kobold-less room was spoken aloud.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "a kobold");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You say \"a kobold\""),
        "got: {shown:?}"
    );
    assert!(!shown.contains("*Combat Engaged*"));
}

#[test]
fn bare_a_auto_picks_a_target() {
    // Player testimony: "just a and it picks a target for me".
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "a");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}

#[test]
fn bare_a_with_no_monster_is_said() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "a");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You say \"a\""), "got: {shown:?}");
}

#[test]
fn reengaging_prints_combat_off_first() {
    // Oracle: "at thief" while already fighting printed *Combat Off* then
    // *Combat Engaged*.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack kobold");
    core.drain_events();

    core.input(s, "at thief");
    let shown = text_to(&core.drain_events(), s);
    let off = shown.find("*Combat Off*");
    let on = shown.find("*Combat Engaged*");
    assert!(off.is_some() && on.is_some() && off < on, "got: {shown:?}");
}

#[test]
fn multi_word_target_names_resolve() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "a kobold thief");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}

#[test]
fn partial_multi_word_prefixes_resolve() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "a kob th");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}

#[test]
fn rounds_exchange_blows_every_five_seconds() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let m = core
        .spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 })
        .unwrap();
    core.input(s, "attack kobold");
    core.drain_events();

    let events = run_rounds(&mut core, 4);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("You punch kobold thief for")
            || shown.contains("You swing at kobold thief!")
            || shown.contains("glances off its armour"),
        "player swings across rounds: {shown:?}"
    );
    assert!(
        shown.contains("The kobold thief hits you for"),
        "monster retaliates: {shown:?}"
    );
    assert!(
        core.monster_hp(m).unwrap() < 5000 || core.current_hp(s) < 35,
        "damage flowed somewhere"
    );
}

#[test]
fn no_swings_without_engagement() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    let events = run_rounds(&mut core, 3);
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("punch") && !shown.contains("hits you"),
        "aggression is M6; passive monsters stay passive: {shown:?}"
    );
}

#[test]
fn moving_away_breaks_combat() {
    let mut content = world();
    content
        .rooms
        .get_mut(&RoomId { map: 1, room: 1 })
        .unwrap()
        .exits[mud_core::content::Direction::North as usize] =
        Some(mud_core::content::Exit {
            dest: RoomId { map: 1, room: 2 },
            exit_type: 0,
            trigger_msg: None,
        });
    content.add_room(Room {
        id: RoomId { map: 1, room: 2 },
        name: "Vestibule".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack kobold");
    core.drain_events();

    core.input(s, "n");
    core.drain_events();
    let events = run_rounds(&mut core, 3);
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("You punch") && !shown.contains("hits you for"),
        "combat torn down after leaving: {shown:?}"
    );
}

#[test]
fn being_attacked_cancels_a_pending_exit() {
    // The meditation-delay purpose (user-confirmed): you cannot exit the
    // Realm while being attacked.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack kobold");
    core.drain_events();

    core.input(s, "x"); // meditation starts mid-fight
    core.drain_events();
    let events = run_rounds(&mut core, 4);
    let disconnected = events
        .iter()
        .any(|e| matches!(e, Event::Disconnect(d) if *d == s));
    assert!(
        !disconnected,
        "combat swings must cancel the pending exit"
    );
}

#[test]
fn downed_player_stops_swinging_and_is_blocked() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack kobold");
    core.drain_events();
    core.set_current_hp(s, -5); // downed (death floor is -200)

    core.input(s, "n");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not do that while you are mortally wounded!"),
        "got: {shown:?}"
    );

    let events = run_rounds(&mut core, 2);
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("You punch"),
        "helpless players do not swing: {shown:?}"
    );
}
