//! M7 slice 5 — monster-vs-monster combat (`attack_monster_monster`
//! decompile 27213-27340, `move_monster_to_fighter` 25085-25230;
//! `re/docs/charm.md` §3). Facts under test:
//! - the entry gates: full energy (`mon+0x114 <= mon+0x16`) and no
//!   Fear(0x3c) on the attacker — both silent, no draws;
//! - the post-resolution EU abort (27242): the draws happen, the damage
//!   does not;
//! - both fighters come from attack-form slot 0 and run the ordinary
//!   mode-5 pipeline, so damage lands and the defender's Dodge(0x22)
//!   ability parries;
//! - ONE room line per swing, to the DEFENDER's room, first letter
//!   upcased — including the two-`%s` glance line (the shipped `.rdata`
//!   strings, not the decompiler's mangled symbol names);
//! - the kill path: killer-less experience split among the engaged
//!   sessions, the kill line to the ATTACKER's room, and the defender's
//!   DamageShield(0x48) biting back after the kill.

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Direction, Exit, Monster, MonsterId, Race, RaceId, Room,
    RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, MonsterInstanceId, Player, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };
const B: RoomId = RoomId { map: 1, room: 2 };

/// Attack-form slot 0 — the only slot `attack_monster_monster` reads
/// (27238: `move_monster_to_fighter(..., param_3 = 0)`).
fn form(accuracy: i16, min: i16, max: i16, energy: i16) -> AttackForm {
    AttackForm {
        kind: 1,
        accuracy,
        weight: 100,
        min_damage: min,
        max_damage: max,
        energy,
        ..Default::default()
    }
}

/// A brawler with a 1000-point energy pool and 200 HP; behaviour 3 (lair)
/// keeps the driver out of these tests — every swing here is driven by the
/// hook, never by a tick.
fn brawler(id: u16, name: &str, accuracy: i16, min: i16, max: i16) -> Monster {
    Monster {
        id: MonsterId(id),
        name: name.into(),
        hitpoints: 200,
        energy: 1000,
        experience: 40,
        exp_multi: 1,
        behaviour: 3,
        attacks: [
            form(accuracy, min, max, 0),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
        ..Default::default()
    }
}

fn world() -> Content {
    let mut content = Content::default();
    let mut a = Room { id: A, name: "Arena".into(), ..Default::default() };
    a.exits[Direction::North as usize] =
        Some(Exit { dest: B, exit_type: 0, ..Default::default() });
    let b = Room { id: B, name: "Pit".into(), ..Default::default() };
    content.add_room(a);
    content.add_room(b);
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
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

fn player_at(name: &str, location: RoomId) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 1,
        current_hp: 400,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location,
        ..Default::default()
    }
}

fn config() -> CoreConfig {
    CoreConfig { start_location: A, ..CoreConfig::default() }
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

/// Stage: hunter (template 1) and quarry (template 2) in the same room,
/// plus a watcher standing in `watch`.
fn arena(
    hunter: Monster,
    quarry: Monster,
    quarry_room: RoomId,
    watch: RoomId,
) -> (Core, SessionId, MonsterInstanceId, MonsterInstanceId) {
    let mut content = world();
    content.add_monster(hunter);
    content.add_monster(quarry);
    let mut core = Core::new(content, config());
    let watcher = core.attach_player(player_at("Watcher", watch));
    let a = core.spawn_monster(MonsterId(1), A).expect("hunter spawns");
    let d = core.spawn_monster(MonsterId(2), quarry_room).expect("quarry spawns");
    core.drain_events();
    (core, watcher, a, d)
}

/// `n` hook swings, returning everything the watcher saw.
fn swings(
    core: &mut Core,
    a: MonsterInstanceId,
    d: MonsterInstanceId,
    n: usize,
    watcher: SessionId,
) -> String {
    let mut out = String::new();
    for _ in 0..n {
        core.debug_monster_attack_monster(a, d);
        out.push_str(&text_to(&core.drain_events(), watcher));
    }
    out
}

#[test]
fn full_energy_gate_blocks_the_second_swing() {
    // 27232: `mon+0x114 <= mon+0x16` — the attacker must be at FULL
    // energy, so a form that actually costs energy swings once and then
    // waits for the pool to refill (same gate as attack_monster_user).
    let mut hunter = brawler(1, "hunter", 200, 5, 5);
    hunter.attacks[0] = form(200, 5, 5, 200);
    let (mut core, watcher, a, d) = arena(hunter, brawler(2, "quarry", 200, 5, 5), A, A);
    let seen = swings(&mut core, a, d, 4, watcher);
    assert_eq!(
        seen.matches("just attacked").count(),
        1,
        "one swing, then the energy gate: {seen:?}"
    );
    assert_eq!(core.monster_energy(a), Some(800), "one form-0 EU paid: {seen:?}");
    assert_eq!(core.monster_hp(d), Some(195), "exactly one 5-point hit");
}

#[test]
fn fear_blocks_the_swing() {
    // 27234: `monster_has_ability(0x3c)` on the ATTACKER — a feared
    // monster never swings, silently and without any draw.
    let mut hunter = brawler(1, "hunter", 200, 5, 5);
    hunter.abilities = vec![(Ability::Fear, 1)];
    let (mut core, watcher, a, d) = arena(hunter, brawler(2, "quarry", 200, 5, 5), A, A);
    let seen = swings(&mut core, a, d, 5, watcher);
    assert_eq!(seen, "", "a feared monster is silent: {seen:?}");
    assert_eq!(core.monster_hp(d), Some(200), "no damage");
    assert_eq!(core.monster_energy(a), Some(1000), "no energy paid");
}

#[test]
fn energy_cost_over_the_pool_aborts_after_the_draws() {
    // 27242: the abort compares the RESOLVED energy cost against the pool,
    // i.e. AFTER calculate_attack has drawn. A form costing more than the
    // whole pool therefore burns rolls and lands nothing, forever.
    let mut hunter = brawler(1, "hunter", 200, 5, 5);
    hunter.attacks[0] = form(200, 5, 5, 2000);
    let (mut core, watcher, a, d) = arena(hunter, brawler(2, "quarry", 200, 5, 5), A, A);
    let seen = swings(&mut core, a, d, 5, watcher);
    assert_eq!(seen, "", "no line on the abort: {seen:?}");
    assert_eq!(core.monster_hp(d), Some(200), "no damage");
    assert_eq!(core.monster_energy(a), Some(1000), "no energy paid");
}

#[test]
fn hit_applies_damage_and_room_line() {
    // Accuracy 200 -> to-hit 99, min=max=5 against DR 0: every landed
    // swing is exactly 5 points and one upcased room line (27298-27304).
    let (mut core, watcher, a, d) = arena(
        brawler(1, "hunter", 200, 5, 5),
        brawler(2, "quarry", 200, 5, 5),
        A,
        A,
    );
    let seen = swings(&mut core, a, d, 10, watcher);
    let hits = seen.matches("Hunter just attacked quarry!").count();
    assert!(hits >= 8, "99%-to-hit swings should nearly all land: {seen:?}");
    assert_eq!(
        core.monster_hp(d),
        Some(200 - 5 * hits as i32),
        "HP delta is exactly 5 per hit line: {seen:?}"
    );
}

#[test]
fn glance_line_is_the_two_slot_form() {
    // Result 1 (damage < 1). The shipped string carries only TWO %s
    // ("%s's just glanced off of %s's armour.") — the decompiler's mangled
    // symbol name reads like a three-slot weapon line, but the sprintf at
    // 27259 passes exactly the attacker and the defender.
    let (mut core, watcher, a, d) = arena(
        brawler(1, "hunter", 200, 0, 0),
        brawler(2, "quarry", 200, 5, 5),
        A,
        A,
    );
    let seen = swings(&mut core, a, d, 10, watcher);
    assert!(
        seen.contains("Hunter's just glanced off of quarry's armour."),
        "zero-damage connect is the glance line: {seen:?}"
    );
    assert_eq!(core.monster_hp(d), Some(200), "a glance does no damage");
}

#[test]
fn miss_line() {
    // Accuracy 5 -> the sub-formula 5% to-hit: result 0, the plain miss.
    let (mut core, watcher, a, d) = arena(
        brawler(1, "hunter", 5, 5, 5),
        brawler(2, "quarry", 200, 5, 5),
        A,
        A,
    );
    let seen = swings(&mut core, a, d, 12, watcher);
    assert!(
        seen.contains("Hunter just missed an attack against quarry."),
        "a failed to-hit roll is the miss line: {seen:?}"
    );
}

#[test]
fn dodge_line_uses_the_defenders_dodge_ability() {
    // `move_monster_to_fighter` 25185-25186 feeds Dodge(0x22) into the
    // fighter's parry word for BOTH sides of every build. Dodge 30 against
    // accuracy 40 = a 60% parry chance; the line names the DEFENDER first.
    let mut quarry = brawler(2, "quarry", 200, 5, 5);
    quarry.abilities = vec![(Ability::Dodge, 30)];
    let (mut core, watcher, a, d) = arena(brawler(1, "hunter", 40, 5, 5), quarry, A, A);
    let seen = swings(&mut core, a, d, 20, watcher);
    assert!(
        seen.contains("Quarry just dodged an attack from hunter."),
        "the defender's Dodge ability parries: {seen:?}"
    );
}

#[test]
fn swing_lines_go_to_the_defenders_room() {
    // 27304: `tell_room(defender room)` — and there is no same-room gate
    // anywhere in the function, which is what lets the hunt arm swing
    // across a room boundary (charm.md §6).
    let (mut core, in_pit, a, d) = arena(
        brawler(1, "hunter", 200, 5, 5),
        brawler(2, "quarry", 200, 5, 5),
        B,
        B,
    );
    let bystander = core.attach_player(player_at("Bystander", A));
    core.drain_events();
    core.debug_monster_attack_monster(a, d);
    let events = core.drain_events();
    assert!(
        text_to(&events, in_pit).contains("Hunter just attacked quarry!"),
        "the swing line lands in the defender's room"
    );
    assert_eq!(
        text_to(&events, bystander),
        "",
        "the attacker's room sees nothing until the kill"
    );
    assert_eq!(core.monster_hp(d), Some(195), "a cross-room swing connects");
}

#[test]
fn kill_splits_exp_to_engaged_users() {
    // 27322-27328: the name is captured BEFORE the kill, the kill line
    // goes to the ATTACKER's room, and `distribute_experience(-1, ...)`
    // splits worth x multi among the sessions engaged on the victim.
    let (mut core, watcher, a, d) = arena(
        brawler(1, "hunter", 200, 5, 5),
        brawler(2, "quarry", 200, 5, 5),
        A,
        A,
    );
    core.input(watcher, "attack quarry");
    core.drain_events();
    let before = core.player_snapshot(watcher).experience;
    let mut seen = String::new();
    for _ in 0..80 {
        if core.monster_hp(d).is_none() {
            break;
        }
        core.debug_monster_attack_monster(a, d);
        seen.push_str(&text_to(&core.drain_events(), watcher));
    }
    assert!(core.monster_hp(d).is_none(), "the quarry dies: {seen:?}");
    assert!(
        seen.contains("Hunter just killed quarry."),
        "kill line to the attacker's room: {seen:?}"
    );
    assert!(seen.contains("The quarry is dead."), "check_kill announcement: {seen:?}");
    assert_eq!(
        core.player_snapshot(watcher).experience - before,
        40,
        "the sole engaged session takes the whole worth x multi: {seen:?}"
    );
    assert!(seen.contains("You gain 40 experience."), "split line: {seen:?}");
    assert!(seen.contains("*Combat Off*"), "kill_autocombat breaks combat: {seen:?}");
}

#[test]
fn damage_shield_bites_the_attacker() {
    // 27283-27296: on a DAMAGING swing only, the defender's
    // DamageShield(0x48) draws `genrdn(1, max(val+1,1))` off the
    // attacker's HP. The attacker is never checked for death there.
    let mut quarry = brawler(2, "quarry", 200, 5, 5);
    quarry.abilities = vec![(Ability::DamageShield, 5)];
    let (mut core, watcher, a, d) = arena(brawler(1, "hunter", 200, 5, 5), quarry, A, A);
    let seen = swings(&mut core, a, d, 1, watcher);
    assert!(seen.contains("Hunter just attacked quarry!"), "the swing lands: {seen:?}");
    let hp = core.monster_hp(a).expect("the attacker survives 1-6 shield damage");
    assert!((194..=199).contains(&hp), "shield damage is genrdn(1,6): {hp}");
}

#[test]
fn glancing_swings_do_not_draw_the_damage_shield() {
    // The shield block sits in the `damage >= 1` arm only (27283): a
    // zero-damage connect leaves the attacker untouched.
    let mut quarry = brawler(2, "quarry", 200, 5, 5);
    quarry.abilities = vec![(Ability::DamageShield, 5)];
    let (mut core, watcher, a, d) = arena(brawler(1, "hunter", 200, 0, 0), quarry, A, A);
    let seen = swings(&mut core, a, d, 6, watcher);
    assert!(seen.contains("just glanced off of"), "zero-damage swings: {seen:?}");
    assert_eq!(core.monster_hp(a), Some(200), "no shield draw on a glance");
}
