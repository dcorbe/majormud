//! Tests for the 30-second slow-tick regeneration
//! (`re/docs/regeneration.md`): HP, mana, hunger/thirst decay.

use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
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
        max_stats: StatBlock {
            intellect: 90,
            wisdom: 120,
            strength: 110,
            health: 120,
            agility: 90,
            charm: 85,
        },
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 40,
            wisdom: 40,
            strength: 40,
            health: 40,
            agility: 40,
            charm: 40,
        },
        max_stats: StatBlock {
            intellect: 100,
            wisdom: 100,
            strength: 100,
            health: 100,
            agility: 100,
            charm: 100,
        },
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
    content.add_class(Class {
        id: ClassId(12),
        name: "Mage".into(),
        abilities: vec![],
        hp_per_level: 3,
        hp_seed: 3,
        caster_group: 1,
        casting_factor: 3,
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

fn create(core: &mut Core, name: &str, race: &str, class: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
        saved_evil: 0,
    });
    core.input(s, race);
    core.input(s, class);
    core.input(s, "No");
    core.drain_events();
    s
}

fn ticks(core: &mut Core, n: u64) {
    for _ in 0..n {
        core.tick();
    }
}

fn persisted_on_quit(core: &mut Core, s: SessionId) -> mud_core::game::Player {
    core.detach(s); // carrier drop persists immediately (no meditation)
    core.drain_events()
        .into_iter()
        .find_map(|e| match e {
            Event::Persist(p) => Some(*p),
            _ => None,
        })
        .expect("quit persists")
}

#[test]
fn hp_regen_fires_on_the_30th_tick_not_before() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain", "2", "1");
    core.set_current_hp(s, 10);

    // Dwarf Warrior L1: base = (1+20)*50/750 = 1 per slow tick.
    ticks(&mut core, 29);
    assert_eq!(core.current_hp(s), 10, "no regen before the slow tick");
    ticks(&mut core, 1);
    assert_eq!(core.current_hp(s), 11, "one point on tick 30");
    ticks(&mut core, 30);
    assert_eq!(core.current_hp(s), 12, "cadence repeats");
}

#[test]
fn hp_regen_caps_at_max_and_stops() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain", "2", "1");
    core.set_current_hp(s, 34);
    ticks(&mut core, 30);
    assert_eq!(core.current_hp(s), 35);
    ticks(&mut core, 30);
    assert_eq!(core.current_hp(s), 35, "full players do not regen");
}

#[test]
fn mana_regen_uses_the_casting_stat_formula() {
    // Human Mage L1: stat = Int 40, tier 3.
    // regen = (1+20)*40*(3+2)/1650 = 4200/1650 = 2 per slow tick; max 12.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Merlin", "1", "12");
    core.set_current_mana(s, 0);
    ticks(&mut core, 30);
    assert_eq!(core.current_mana(s), 2);
    ticks(&mut core, 150);
    assert_eq!(core.current_mana(s), 12, "capped at max mana");
}

#[test]
fn non_casters_never_gain_mana() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain", "2", "1");
    ticks(&mut core, 60);
    assert_eq!(core.current_mana(s), 0);
}

#[test]
fn hunger_and_thirst_tick_down_from_1000() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain", "2", "1");
    ticks(&mut core, 60);
    let p = persisted_on_quit(&mut core, s);
    assert_eq!(p.hunger, 998);
    assert_eq!(p.thirst, 998);
}

#[test]
fn downed_players_bleed_instead_of_regenerating() {
    // The HP-regen handler is gated on 0 < HP < max; the near-death band
    // bleeds one per slow tick instead (death.md §5).
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain", "2", "1");
    core.set_current_hp(s, 0);
    ticks(&mut core, 30);
    assert_eq!(core.current_hp(s), -1);
}
