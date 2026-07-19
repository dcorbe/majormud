//! M6 slice 3 — target acquisition and the flee free-attack
//! (`FUN_00423863` decompile 20335-20525, `give_monsters_a_free_attack`
//! 23846-23905; monsters.md §4). Key extracted facts under test:
//! - an eligible aggressive monster ALWAYS attacks someone each round —
//!   the anti-pile-on roll only picks who (fallback 20401);
//! - behaviour 0/3/4 never initiate; 6 spares fame >= 0x28 (20386);
//! - roam-class 5 "guardians" initiate ONLY against fame >= 0x28 at base
//!   100 (20434-20438) — mode-6 class-5 inverts to fame < 0x28;
//! - the retaliation lock rolls genrdn(1,100) < aggression, bypassed for
//!   passive modes, never for class 0x25 (26230-26236);
//! - the flee free-attack: one room roll <= aggression, passive modes
//!   swing only at their locked target, and only a DEATH aborts the move.

use mud_core::content::{
    AttackForm, Class, ClassId, Content, Direction, Exit, Monster, MonsterId, Race, RaceId, Room,
    RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };
const B: RoomId = RoomId { map: 1, room: 2 };

fn melee_form() -> AttackForm {
    AttackForm {
        kind: 1,
        accuracy: 100,
        weight: 100,
        min_damage: 1,
        max_damage: 2,
        energy: 200,
        ..Default::default()
    }
}

/// An armed monster: `behaviour` (alignment@0xae) and `aggression`
/// (follow@0x6e) drive the slice-3 gates; roam 0 keeps it stationary.
fn monster(id: u16, behaviour: i16, aggression: i16) -> Monster {
    Monster {
        id: MonsterId(id),
        name: format!("beast {id}"),
        hitpoints: 500,
        energy: 1000,
        magic_resist: 0,
        behaviour,
        aggression,
        attacks: [
            melee_form(),
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
    let mut a = Room {
        id: A,
        name: "Arena".into(),
        ..Default::default()
    };
    a.exits[Direction::North as usize] = Some(Exit {
        dest: B,
        exit_type: 0,
        ..Default::default()
    });
    let mut b = Room {
        id: B,
        name: "Refuge".into(),
        ..Default::default()
    };
    b.exits[Direction::South as usize] = Some(Exit {
        dest: A,
        exit_type: 0,
        ..Default::default()
    });
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

fn player_at(name: &str, location: RoomId, fame: i16) -> Player {
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
        fame,
        ..Default::default()
    }
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: A,
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

/// Run one 5 s combat round, returning the victim-visible text.
fn one_round(core: &mut Core, victim: SessionId) -> String {
    let mut out = String::new();
    for _ in 0..5 {
        core.tick();
        out.push_str(&text_to(&core.drain_events(), victim));
    }
    out
}

#[test]
fn aggressive_monster_attacks_on_sight_every_round() {
    // Fallback 20401: an eligible monster always attacks someone; with a
    // single candidate the anti-pile-on roll cannot change the outcome.
    let mut content = world();
    content.add_monster(monster(1, 2, 50));
    let mut core = Core::new(content, config());
    let victim = core.attach_player(player_at("Prey", A, 0));
    core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    for round in 1..=4 {
        let seen = one_round(&mut core, victim);
        assert!(
            seen.to_lowercase().contains("beast 1"),
            "round {round}: unprovoked attack expected, got {seen:?}"
        );
    }
}

#[test]
fn passive_behaviours_never_initiate() {
    // Modes 0/3/4 are skipped outright (20374-20376).
    let mut content = world();
    content.add_monster(monster(1, 0, 100));
    content.add_monster(monster(2, 3, 100));
    content.add_monster(monster(3, 4, 100));
    let mut core = Core::new(content, config());
    let bystander = core.attach_player(player_at("Bystander", A, 0));
    for id in 1..=3 {
        core.spawn_monster(MonsterId(id), A).unwrap();
    }
    core.drain_events();
    for _ in 0..6 {
        let seen = one_round(&mut core, bystander);
        assert!(
            !seen.to_lowercase().contains("beast"),
            "passive monsters must not initiate: {seen:?}"
        );
    }
}

#[test]
fn mode6_spares_the_famous() {
    // 20386-20390: mode 6 skips candidates with fame >= 0x28.
    let mut content = world();
    content.add_monster(monster(1, 6, 50));
    let mut core = Core::new(content, config());
    let famous = core.attach_player(player_at("Famous", A, 0x28));
    core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    for _ in 0..6 {
        let seen = one_round(&mut core, famous);
        assert!(!seen.to_lowercase().contains("beast"), "fame 0x28 is spared: {seen:?}");
    }
    // A fame-0x27 player in the same room is fair game.
    let nobody = core.attach_player(player_at("Nobody", A, 0x27));
    core.drain_events();
    let mut attacked = false;
    for _ in 0..6 {
        attacked |= one_round(&mut core, nobody).to_lowercase().contains("beast");
    }
    assert!(attacked, "fame 0x27 is a valid mode-6 victim");
}

#[test]
fn class5_guardian_attacks_only_the_notorious() {
    // 20434-20438: non-mode-6 roam-class 5 initiates ONLY at fame >= 0x28.
    let mut content = world();
    let mut guardian = monster(1, 2, 50);
    guardian.roam_class = 5;
    content.add_monster(guardian);
    let mut core = Core::new(content, config());
    let citizen = core.attach_player(player_at("Citizen", A, 0));
    core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    for _ in 0..6 {
        let seen = one_round(&mut core, citizen);
        assert!(!seen.to_lowercase().contains("beast"), "fame 0 citizen is safe: {seen:?}");
    }
    let outlaw = core.attach_player(player_at("Outlaw", A, 0x30));
    core.drain_events();
    let mut attacked = false;
    for _ in 0..6 {
        attacked |= one_round(&mut core, outlaw).to_lowercase().contains("beast");
    }
    assert!(attacked, "fame 0x30 outlaw draws the guardian");
}

#[test]
fn passive_monster_locks_its_attacker_and_fights_back() {
    // Retaliation 26230-26236: passive modes bypass the aggression roll —
    // the lock always lands; next round the locked path attacks.
    let mut content = world();
    content.add_monster(monster(1, 3, 0)); // aggression 0: roll can never pass
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Prodder", A, 0));
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    core.input(s, "attack beast");
    one_round(&mut core, s);
    assert_eq!(core.monster_target(m), Some(s), "passive retaliation locks");
    let seen = one_round(&mut core, s);
    assert!(seen.to_lowercase().contains("beast 1"), "locked monster swings back: {seen:?}");
}

#[test]
fn aggressive_zero_aggression_monster_never_keeps_a_lock() {
    // The lock roll is genrdn(1,100) < aggression — at 0 it cannot land
    // for an aggressive mode (2), and the post-swing re-roll clears it
    // (26867-26885). The monster still fights every round via acquisition.
    let mut content = world();
    content.add_monster(monster(1, 2, 0));
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Prodder", A, 0));
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    core.input(s, "attack beast");
    for _ in 0..4 {
        let seen = one_round(&mut core, s);
        assert_eq!(core.monster_target(m), None, "no lock at aggression 0");
        assert!(seen.to_lowercase().contains("beast 1"), "still attacks via acquisition: {seen:?}");
    }
}

#[test]
fn flee_free_attack_clips_the_runner_but_the_move_succeeds() {
    // 23846-23905: one room roll <= aggression (100 always passes),
    // aggressive mode, not my locked target => opportunist swing; the
    // move aborts only on death.
    let mut content = world();
    content.add_monster(monster(1, 2, 100));
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Runner", A, 0));
    core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    core.input(s, "n");
    let events = core.drain_events();
    let seen = text_to(&events, s);
    assert!(seen.to_lowercase().contains("beast 1"), "parting swing: {seen:?}");
    assert!(seen.contains("Refuge"), "the move still lands: {seen:?}");
}

#[test]
fn unlocked_passive_monster_lets_you_walk() {
    // Passive modes swing at departure ONLY when locked on the runner.
    let mut content = world();
    content.add_monster(monster(1, 0, 100));
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Walker", A, 0));
    core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    core.input(s, "n");
    let seen = text_to(&core.drain_events(), s);
    assert!(!seen.to_lowercase().contains("beast"), "no parting swing from a stranger: {seen:?}");
    assert!(seen.contains("Refuge"), "clean exit: {seen:?}");
}

#[test]
fn locked_passive_monster_takes_the_parting_swing() {
    // The free attack funnels through attack_monster_user's full-energy
    // gate, and the +0x6f0 counter gates it for the whole medium tick —
    // so the swing only lands on a departure while the monster is rested
    // and the runner unbothered. A door keeps the monster from pursuing
    // into the refuge between rounds.
    let mut content = world();
    let door = |dest| {
        Some(Exit {
            dest,
            exit_type: 2,
            door_closed: true,
            ..Default::default()
        })
    };
    content.rooms.get_mut(&A).unwrap().exits[Direction::North as usize] = door(B);
    content.rooms.get_mut(&B).unwrap().exits[Direction::South as usize] = door(A);
    content.add_monster(monster(1, 3, 100)); // passive, but max aggression
    let mut core = Core::new(content, config());
    let s = core.attach_player(player_at("Fighter", A, 0));
    let m = core.spawn_monster(MonsterId(1), A).unwrap();
    core.drain_events();
    core.input(s, "attack beast");
    one_round(&mut core, s); // t5: retaliation locks (passive: no roll)
    assert_eq!(core.monster_target(m), Some(s));
    core.tick(); // t6: medium resets +0x6f0
    core.drain_events();
    core.input(s, "n"); // duck behind the door (monster's energy is spent)
    core.drain_events();
    for _ in 0..6 {
        core.tick(); // t7-t12: energy refills (t10), +0x6f0 resets (t12)
    }
    core.drain_events();
    assert_eq!(core.monster_target(m), Some(s), "the lock outlasts the trip");
    core.input(s, "s"); // back into the lair
    core.drain_events();
    core.input(s, "n"); // and out again — NOW the parting swing fires
    let seen = text_to(&core.drain_events(), s);
    assert!(seen.to_lowercase().contains("beast 1"), "locked passive swings at flee: {seen:?}");
    assert!(seen.contains("Refuge"), "the move still lands: {seen:?}");
}
