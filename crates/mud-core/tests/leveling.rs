//! Tests for the experience curve, exp command, and train_level
//! (`re/docs/leveling.md`, `re/docs/records.md` exp curve; oracle transcript
//! oracle_exp_train.raw / oracle_train2.raw for exact strings).

use mud_core::content::{
    Class, ClassId, Content, Race, RaceId, Room, RoomId, Shop, ShopId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};
use mud_core::stats::exp_needed;

#[test]
fn exp_curve_matches_the_oracle_seed() {
    // Dwarf Warrior: base = class.exp 0 + race.expchart 30 -> 1300 (oracle).
    assert_eq!(exp_needed(1, 30), 1300);
    // Level 2 = 1300 * 40/20.
    assert_eq!(exp_needed(2, 30), 2600);
    // Level 3 = 2600 * 44/24 (truncating).
    assert_eq!(exp_needed(3, 30), 4766);
}

#[test]
fn exp_curve_zero_base() {
    assert_eq!(exp_needed(1, 0), 1000);
    assert_eq!(exp_needed(2, 0), 2000);
}

#[test]
fn exp_curve_high_bands_taper() {
    // i in [26,53] -> 115/100; [54,56] -> 109/100; >=57 -> 108/100
    // (new_calc_exp_needed, decompile 0x73810).
    let base = 30u64;
    let e26 = exp_needed(26, base);
    assert_eq!(exp_needed(27, base), e26 * 115 / 100);
    let e54 = exp_needed(54, base);
    assert_eq!(exp_needed(55, base), e54 * 109 / 100);
    let e57 = exp_needed(57, base);
    assert_eq!(exp_needed(58, base), e57 * 108 / 100);
}

// --- game-level tests ---

fn world() -> Content {
    let mut content = Content::default();
    let mut entrance = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Village Entrance".into(),
        description: vec![],
        shop: None,
        exits: Default::default(),
    };
    entrance.exits[mud_core::content::Direction::North as usize] =
        Some(mud_core::content::Exit {
            dest: RoomId { map: 1, room: 2 },
            exit_type: 0,
        });
    content.add_room(entrance);
    let mut guild = Room {
        id: RoomId { map: 1, room: 2 },
        name: "Adventurer's Guild".into(),
        description: vec![],
        shop: None,
        exits: Default::default(),
    };
    guild.shop = Some(ShopId(38));
    content.add_room(guild);
    content.add_shop(Shop {
        id: ShopId(38),
        name: "Training Room".into(),
        shop_type: 8,
        min_level: 1,
        max_level: 10,
        markup: 0,
        class_limit: 0,
    });
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
    });
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        ..CoreConfig::default()
    }
}

fn create(core: &mut Core) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: "Dain".into(),
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

#[test]
fn exp_command_shows_the_oracle_line() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core);
    core.input(s, "exp");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Exp: 0 Level: 1 Exp needed for next level: 1300 (1300) [0%]"),
        "got: {shown:?}"
    );
}

#[test]
fn health_command_shows_the_oracle_line() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core);
    core.input(s, "health");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("Health:    35/35    [100%]"),
        "got: {shown:?}"
    );
}

#[test]
fn train_outside_a_trainer_room_is_rejected() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core);
    core.input(s, "train");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You must be in an appropriate training room to train!"),
        "got: {shown:?}"
    );
}

#[test]
fn train_without_exp_is_rejected_at_the_trainer() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core);
    core.input(s, "n");
    core.drain_events();
    core.input(s, "train");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You do not have the required experience to train yet!"),
        "got: {shown:?}"
    );
}

#[test]
fn train_without_money_is_rejected_after_the_exp_check() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core);
    core.add_experience(s, 1300);
    core.input(s, "n");
    core.drain_events();
    core.input(s, "train");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You do not have the money required for your training."),
        "got: {shown:?}"
    );
}

#[test]
fn successful_training_levels_up_with_all_effects() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core);
    core.add_experience(s, 1300);
    core.give_copper(s, 100); // cost at L1, markup 0: (0+100)*(1*5)/100 = 5
    core.input(s, "n");
    core.drain_events();

    core.input(s, "train");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    assert!(
        shown.contains("you receive training to attain level 2."),
        "got: {shown:?}"
    );

    let p = core.player_snapshot(s);
    assert_eq!(p.level, 2);
    // CP: level < 11 -> +10 (creation gave 100).
    assert_eq!(p.cp_unspent, 110);
    assert_eq!(p.cp_lifetime, 110);
    // HP-base roll: += genrdn(0, hp_seed+1), so within [4, 8].
    assert!(p.hp_base >= 4 && p.hp_base <= 8, "hp_base {}", p.hp_base);
    // Cost deducted.
    assert_eq!(p.coins.copper, 95);
    // Training does not heal: current HP untouched (still creation max 35).
    assert_eq!(p.current_hp, 35);
}

#[test]
fn training_needs_exp_for_the_current_level_only() {
    // After reaching L2, the next train needs exp_needed(2, 30) = 2600 total.
    let mut core = Core::new(world(), config());
    let s = create(&mut core);
    core.add_experience(s, 1300);
    core.give_copper(s, 100);
    core.input(s, "n");
    core.drain_events();
    core.input(s, "train");
    core.drain_events();

    core.input(s, "train");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You do not have the required experience to train yet!"),
        "1300 exp cannot buy level 3: {shown:?}"
    );

    core.add_experience(s, 1300); // total 2600
    core.input(s, "train");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("you receive training to attain level 3."),
        "got: {shown:?}"
    );
}

#[test]
fn shop_level_band_gates_training() {
    let mut core = Core::new(world(), config());
    {
        let shop = core.content_mut().shops.get_mut(&ShopId(38)).unwrap();
        shop.max_level = 1; // band [1,1]: level+1 = 2 is outside
    }
    let s = create(&mut core);
    core.add_experience(s, 1300);
    core.give_copper(s, 100);
    core.input(s, "n");
    core.drain_events();
    core.input(s, "train");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        !shown.contains("attain level"),
        "band-blocked trainer must refuse: {shown:?}"
    );
}
