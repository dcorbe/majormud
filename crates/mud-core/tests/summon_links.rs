//! M7 slice 5 Task 7 — Summon(12) ownership links and the `+0x88` hunt
//! branch (`re/docs/charm.md` §6/§7). Facts under test:
//! - the four summon sites stamp DIFFERENT §0 state (charm.md §6's
//!   table), so the link is an explicit tag and not a nullable session:
//!   `cast_no_target` 0xc (40035-40056) writes the full pet triple,
//!   `cast_user_target` 0xc (42059-42086) a bare grudge toward the target
//!   player, `cast_monster_target` 0xc (43902-43931) the `+0x88` victim
//!   id with NO name link, and `monster_cast` 0xc (23251-23268) a bare
//!   grudge toward the victim player;
//! - the monster breadcrumb trail (`mon+0x38..+0x60`, pushed by
//!   `move_monster` 21572-21574) and `dir_monster_travelling_coord`
//!   (15790-15816) reading it from index 1;
//! - the combat driver's hunt arm (20450-20463): a usable trail step
//!   MOVES, a cold trail SWINGS — even across a room boundary;
//! - the medium tick's travel arm (19376-19380), which pre-empts the
//!   wander arms entirely.
//!
//! FIXTURE SHAPE: player-cast Summon has zero learnable carriers in the
//! shipped data (all 87 rows are monster-attack payloads), so every
//! player-side arm here is fixture-reachable only. The behaviour is
//! decompile-faithful regardless — the monster-cast arm ships.

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Direction, Element, Exit, MatchType, Message, MessageId,
    Monster, MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId,
    StatBlock, TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, MonsterInstanceId, Player, SessionId};

const HALL: RoomId = RoomId { map: 1, room: 1 };
const MID: RoomId = RoomId { map: 1, room: 2 };
const FAR: RoomId = RoomId { map: 1, room: 3 };
const MAGE: ClassId = ClassId(1);
const HUMAN: RaceId = RaceId(1);

/// The pet form: a real form-0 fighter whose swing is FREE (form EU 0
/// against a 1000 pool), so the 27231 full-energy gate never closes it.
const HOUND: MonsterId = MonsterId(1);
/// The hunter form — HOUND's twin. Roam class 0 on purpose: a class-0
/// body NEVER wanders (19341-19344), so any movement it makes is the
/// travel arm and nothing else.
const STALKER: MonsterId = MonsterId(2);
/// The quarry: 500 HP, behaviour 3 (lair) and no attack form, so it
/// never initiates, never swings back and never moves on its own. Every
/// combat line it takes belongs to its hunter.
const DUMMY: MonsterId = MonsterId(3);

/// Benign match-1 Summon → the `cast_no_target` PET arm.
const SUMPET: SpellId = SpellId(700);
/// Benign match-4 Summon → the `cast_monster_target` HUNT arm.
const SUMHUNT: SpellId = SpellId(710);

fn spell(id: SpellId, name: &str, short: &str) -> Spell {
    Spell {
        id,
        name: name.into(),
        short_name: short.into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(901)),
        abilities: vec![],
        level_cap: 0,
        round_cost: 100,
        required_power: 1,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200, // auto-success: >= 200 skips the roll
        duration_per_level: 0,
        match_type: MatchType::Single1,
        // Summons are instant-only; any duration routes to silly_spell.
        duration: 0,
        element: Element::Magic,
        class_gate_group: 1,
        mana_cost: 4,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

fn fighter(id: MonsterId, name: &str) -> Monster {
    Monster {
        id,
        name: name.into(),
        hitpoints: 500,
        experience: 12,
        exp_multi: 1,
        energy: 1000,
        charm_level: 1,
        charm_resist: 40,
        aggression: 100,
        behaviour: 1,
        attacks: [AttackForm::default(); 5],
        ..Default::default()
    }
}

fn plain_exit(dest: RoomId) -> Option<Exit> {
    Some(Exit {
        dest,
        exit_type: 0,
        ..Default::default()
    })
}

fn world() -> Content {
    let mut content = Content::default();
    let mut hall = Room {
        id: HALL,
        name: "Hall".into(),
        ..Default::default()
    };
    hall.exits[Direction::North as usize] = plain_exit(MID);
    let mut mid = Room {
        id: MID,
        name: "Middle".into(),
        ..Default::default()
    };
    mid.exits[Direction::South as usize] = plain_exit(HALL);
    mid.exits[Direction::North as usize] = plain_exit(FAR);
    let mut far = Room {
        id: FAR,
        name: "Far".into(),
        ..Default::default()
    };
    far.exits[Direction::South as usize] = plain_exit(MID);
    content.add_room(hall);
    content.add_room(mid);
    content.add_room(far);

    let mut hound = fighter(HOUND, "war hound");
    hound.attacks[0] = AttackForm {
        kind: 1,
        accuracy: 500,
        weight: 100,
        min_damage: 7,
        max_damage: 7,
        energy: 0,
        ..Default::default()
    };
    content.add_monster(hound);
    let mut stalker = fighter(STALKER, "shade stalker");
    stalker.attacks[0] = AttackForm {
        kind: 1,
        accuracy: 500,
        weight: 100,
        min_damage: 7,
        max_damage: 7,
        energy: 0,
        ..Default::default()
    };
    content.add_monster(stalker);
    let mut dummy = Monster {
        id: DUMMY,
        name: "straw dummy".into(),
        hitpoints: 500,
        experience: 12,
        exp_multi: 1,
        energy: 0,
        charm_level: 9999,
        charm_resist: 40,
        behaviour: 3,
        attacks: [AttackForm::default(); 5],
        ..Default::default()
    };
    dummy.aggression = 0;
    content.add_monster(dummy);

    content.add_message(Message {
        id: MessageId(901),
        lines: vec![
            "You cast %s on %s!".into(),
            "%s casts %s upon you!".into(),
            "%s casts %s on %s!".into(),
        ],
    });
    content.add_race(Race {
        id: HUMAN,
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: MAGE,
        name: "Mage".into(),
        abilities: vec![],
        hp_per_level: 2,
        hp_seed: 4,
        caster_group: 1,
        casting_factor: 3,
        exp_base: 0,
        combat_factor: 2,
        weapon_code: 8,
        armour_code: 9,
    });

    // The ability VALUE is the template id (`generate_monster`'s
    // `templateId = value` argument at 40044 / 43911).
    let mut sumpet = spell(SUMPET, "summon pet", "pet");
    sumpet.abilities = vec![(Ability::Summon, HOUND.0 as i16)];
    let mut sumhunt = spell(SUMHUNT, "summon hunter", "hunt");
    sumhunt.abilities = vec![(Ability::Summon, STALKER.0 as i16)];
    sumhunt.match_type = MatchType::Special4;
    for s in [sumpet, sumhunt] {
        content.add_spell(s);
    }
    content
}

fn caster_at(location: RoomId) -> Player {
    let book: BTreeMap<SpellId, bool> = [SUMPET, SUMHUNT].into_iter().map(|s| (s, false)).collect();
    Player {
        name: "Zin".into(),
        gender: Gender::Male,
        race: HUMAN,
        class: MAGE,
        stats: StatBlock {
            strength: 100,
            ..StatBlock::default()
        },
        level: 3,
        current_hp: 200,
        current_mana: 100,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location,
        spellbook: book,
        ..Default::default()
    }
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: HALL,
        ..CoreConfig::default()
    }
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

fn setup() -> (Core, SessionId) {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(caster_at(HALL));
    core.drain_events();
    (core, s)
}

fn cast(core: &mut Core, s: SessionId, line: &str) -> String {
    core.input(s, line);
    text_to(&core.drain_events(), s)
}

/// The one instance of `template` currently alive.
fn instance_of(core: &Core, template: MonsterId) -> MonsterInstanceId {
    core.monster_ids()
        .into_iter()
        .find(|id| core.monster_template(*id) == Some(template))
        .expect("instance alive")
}

// --- §6 the ownership tags ---

#[test]
fn bare_summon_is_a_full_pet() {
    // `cast_no_target` case 0xc (40044-40051): name link = the CASTER,
    // `+0x116 = 1`, `+0x128 |= 1` — the §0 triple in full, timerless
    // (release only via §4.2/§4.3). It carries NO hunt link.
    let (mut core, s) = setup();
    let shown = cast(&mut core, s, "cast pet");
    assert!(
        shown.contains("You cast summon pet on everyone!"),
        "the 40042 'everyone' display: {shown:?}"
    );
    let pet = instance_of(&core, HOUND);
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((true, true, Some(s))),
        "charmed + suppressed + owned by the caster"
    );
    assert_eq!(
        core.debug_monster_hunt(pet),
        Some(None),
        "a pet hunts nothing"
    );
}

#[test]
fn a_summoned_pet_assists_its_owner() {
    // The Pet tag is the SAME state Enslave writes, so the §2.2 assist
    // branch picks a summoned pet up with no extra wiring.
    let (mut core, s) = setup();
    let quarry = core.spawn_monster(DUMMY, HALL).expect("fixture template");
    cast(&mut core, s, "cast pet");
    let pet = instance_of(&core, HOUND);
    core.input(s, "attack dummy");
    core.drain_events();
    let before = core.monster_hp(quarry).expect("quarry lives");
    core.debug_monster_consider(pet);
    assert!(
        core.monster_hp(quarry).is_some_and(|hp| hp < before),
        "the pet swings at its owner's target"
    );
}

#[test]
fn summon_at_a_monster_writes_the_hunt_link_and_no_name_link() {
    // `cast_monster_target` case 0xc (43915-43930): `+0x88 = victim id`,
    // `+0x116 = 0`, and the name link is left EMPTY — a hunter is not a
    // pet and holds no grudge against any player.
    //
    // The victim's 10-deep back-link array (`victim+0x60+i*4`,
    // 43922-43928) is deliberately NOT ported: charm.md §7 records that
    // no reader was ever located for it.
    let (mut core, s) = setup();
    let quarry = core.spawn_monster(DUMMY, HALL).expect("fixture template");
    cast(&mut core, s, "cast hunt dummy");
    let hunter = instance_of(&core, STALKER);
    assert_eq!(
        core.debug_monster_hunt(hunter),
        Some(Some(quarry)),
        "+0x88 = the victim"
    );
    assert_eq!(
        core.debug_monster_charm(hunter),
        Some((false, false, None)),
        "no charm, no suppression, no name link"
    );
}

// --- the breadcrumb trail (mon+0x38..+0x60) ---

#[test]
fn a_moving_monster_pushes_a_ten_deep_trail() {
    // 21572-21574: `mon+0x10 = dest`, memmove the array down one slot,
    // `trail[0] = dest`. Index 0 is therefore the CURRENT room and
    // index 1 the predecessor — the convention
    // `dir_monster_travelling_coord` scans from 1.
    let (mut core, _s) = setup();
    let walker = core.spawn_monster(DUMMY, HALL).expect("fixture template");
    assert_eq!(
        core.debug_monster_trail(walker),
        Some(vec![HALL]),
        "seeded at spawn"
    );
    // DUMMY is behaviour 3 (lair) — `move_monster` gate 3 refuses it, so
    // walk the STALKER instead.
    let hunter = core.spawn_monster(STALKER, HALL).expect("fixture template");
    core.debug_move_monster(hunter, Direction::North);
    core.debug_move_monster(hunter, Direction::North);
    core.drain_events();
    assert_eq!(
        core.debug_monster_trail(hunter),
        Some(vec![FAR, MID, HALL]),
        "newest first"
    );
}

// --- §6 the hunt driver arm (20450-20463) ---

#[test]
fn a_hunter_walks_the_victims_trail_then_swings() {
    // 20451: `dir_monster_travelling_coord` finds the hunter's room at
    // index i of the VICTIM's trail and hands back the exit toward
    // `trail[i-1]` — one step per driver pass, no roll. Once co-located
    // the direction goes cold (`victim room == own room` -> -1, 15799)
    // and 20453-20455 swings instead.
    let (mut core, s) = setup();
    let quarry = core.spawn_monster(STALKER, HALL).expect("fixture template");
    cast(&mut core, s, "cast hunt shade");
    let hunter = core
        .monster_ids()
        .into_iter()
        .find(|id| *id != quarry && core.monster_template(*id) == Some(STALKER))
        .expect("hunter spawned");
    assert_eq!(core.debug_monster_hunt(hunter), Some(Some(quarry)));
    // The quarry walks; the hunter is still in the hall.
    core.debug_move_monster(quarry, Direction::North);
    core.drain_events();
    core.debug_monster_consider(hunter);
    assert_eq!(
        core.monster_location(hunter),
        Some(MID),
        "one trail step, no roll"
    );
    let before = core.monster_hp(quarry).expect("quarry lives");
    core.debug_monster_consider(hunter);
    assert_eq!(
        core.monster_location(hunter),
        Some(MID),
        "co-located: no further step"
    );
    assert!(
        core.monster_hp(quarry).is_some_and(|hp| hp < before),
        "and swings on arrival"
    );
}

#[test]
fn a_cold_trail_swings_across_the_room_boundary() {
    // The decompile-literal arm: 20452-20455 tests ONLY the direction
    // and the suppression byte — there is no room compare here and none
    // in `attack_monster_monster` either (charm.md §3). A hunter that
    // cannot find a trail step swings from wherever it stands.
    let (mut core, s) = setup();
    let quarry = core.spawn_monster(DUMMY, HALL).expect("fixture template");
    cast(&mut core, s, "cast hunt dummy");
    let hunter = instance_of(&core, STALKER);
    // The HUNTER moves away; the quarry never moved, so its trail is
    // just [HALL] and index 1 does not exist.
    core.debug_move_monster(hunter, Direction::North);
    core.drain_events();
    let before = core.monster_hp(quarry).expect("quarry lives");
    core.debug_monster_consider(hunter);
    assert_eq!(core.monster_location(hunter), Some(MID), "no step to take");
    assert!(
        core.monster_hp(quarry).is_some_and(|hp| hp < before),
        "cold trail swings anyway, across rooms"
    );
}

#[test]
fn a_suppressed_hunter_on_a_cold_trail_does_nothing() {
    // 20453: the swing is gated on `+0x116 == 0`. A summoned hunter is
    // born unsuppressed (43929), so this needs the charm suppression —
    // drive it through the state directly.
    let (mut core, s) = setup();
    let quarry = core.spawn_monster(DUMMY, HALL).expect("fixture template");
    cast(&mut core, s, "cast hunt dummy");
    let hunter = instance_of(&core, STALKER);
    core.debug_suppress_monster(hunter, true);
    let before = core.monster_hp(quarry).expect("quarry lives");
    core.debug_monster_consider(hunter);
    assert_eq!(
        core.monster_hp(quarry),
        Some(before),
        "suppressed: no swing"
    );
}

#[test]
fn a_dead_victim_leaves_the_hunter_idle() {
    // §7's stale-link flag, closed by construction: the DLL never clears
    // `+0x88` and reuses monster ids, so a long-lived hunter can
    // redirect onto a recycled body. Our ids come from a monotonic u64
    // counter, so a dangling link is simply dead — and the hunter still
    // does NOT fall through to player acquisition, exactly as the DLL's
    // `+0x88 != 0` test decides the branch before any of it.
    let (mut core, s) = setup();
    let quarry = core.spawn_monster(DUMMY, HALL).expect("fixture template");
    cast(&mut core, s, "cast hunt dummy");
    let hunter = instance_of(&core, STALKER);
    core.debug_kill_monster(quarry);
    core.drain_events();
    let hp = core.current_hp(s);
    core.debug_monster_consider(hunter);
    assert_eq!(core.current_hp(s), hp, "no acquisition fall-through");
    assert_eq!(
        core.debug_monster_hunt(hunter),
        Some(Some(quarry)),
        "link never cleared"
    );
}

// --- the medium tick's travel arm (19376-19380) ---

#[test]
fn the_travel_arm_pre_empts_the_wander_arms() {
    // `medium_update_monster` is THREE levels, not two: 19339 gates on
    // the name link alone, 19340 splits on `+0x88`, and the charmed test
    // `(mon+0x128 & 1) == 0` lives ONLY inside the two wander arms
    // (19346/19364). STALKER is roam class 0, which the wander switch
    // refuses outright (19342-19344) — so the step below can only be the
    // travel arm.
    let (mut core, s) = setup();
    let quarry = core.spawn_monster(STALKER, HALL).expect("fixture template");
    cast(&mut core, s, "cast hunt shade");
    let hunter = core
        .monster_ids()
        .into_iter()
        .find(|id| *id != quarry && core.monster_template(*id) == Some(STALKER))
        .expect("hunter spawned");
    core.debug_move_monster(quarry, Direction::North);
    core.drain_events();
    for _ in 0..3 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(
        core.monster_location(hunter),
        Some(MID),
        "the medium tick walked the trail"
    );
}
