//! SNEAK and HIDE (theft.md §11). Success is SILENT (the DLL prints a
//! bare `\r`); failure self-doubt lines are perception-gated on the
//! actor; sneak movement replaces the leave/arrive broadcasts with
//! perception-FILTERED "You notice %s sneaking..." lines. The delay
//! gates (`add_delay`) and the HIDE item/coin stash arrive with the
//! room-hidden-storage work later in the slice.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};
use mud_core::stats::stealth_chance;

const HERE: RoomId = RoomId { map: 1, room: 1 };
const THERE: RoomId = RoomId { map: 1, room: 2 };

fn world() -> Content {
    let mut content = Content::default();
    let mut here = Room {
        id: HERE,
        name: "Shadows".into(),
        ..Default::default()
    };
    here.exits[Direction::North as usize] = Some(Exit {
        dest: THERE,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(here);
    let mut there = Room {
        id: THERE,
        name: "Lamplight".into(),
        ..Default::default()
    };
    there.exits[Direction::South as usize] = Some(Exit {
        dest: HERE,
        exit_type: 0,
        ..Default::default()
    });
    content.add_room(there);
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
    // A thief-shaped class: Stealth granted, plus PerStealth for the
    // sneak auto-success path.
    content.add_class(Class {
        id: ClassId(1),
        name: "Thief".into(),
        abilities: vec![
            (Ability::from_id(0x67).unwrap(), 1),  // ClassStealth (the gate)
            (Ability::from_id(0x1b).unwrap(), 20), // Stealth bonus
            (Ability::from_id(0xba).unwrap(), 1),  // PerStealth
        ],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    // A plain observer class.
    content.add_class(Class {
        id: ClassId(2),
        name: "Watcher".into(),
        abilities: vec![],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

fn person(name: &str, class: u16, perception_stats: bool) -> Player {
    // High Int/Agl push the Perception derivation up; low ones bury it.
    let stats = if perception_stats {
        StatBlock {
            intellect: 90,
            wisdom: 70,
            strength: 40,
            health: 40,
            agility: 90,
            charm: 40,
        }
    } else {
        StatBlock {
            intellect: 5,
            wisdom: 5,
            strength: 40,
            health: 40,
            agility: 5,
            charm: 40,
        }
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(class),
        level: 10,
        stats,
        base_stats: stats,
        current_hp: 30,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HERE,
        ..Default::default()
    }
}

fn texts(events: &[Event], who: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session, text } if *session == who => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn stealth_chance_matches_the_helper_formula() {
    // theft.md §11.3: base Stealth, encumbrance bands −10/−5, −1 per
    // other player and live monster, cap then floor.
    assert_eq!(stealth_chance(60, 70, 2, 3, 95), 45);
    assert_eq!(stealth_chance(60, 40, 0, 0, 95), 55);
    assert_eq!(stealth_chance(60, 0, 1, 0, 95), 59);
    assert_eq!(stealth_chance(120, 0, 0, 0, 95), 95, "cap");
    assert_eq!(stealth_chance(3, 90, 4, 4, 95), 0, "floored at zero");
}

#[test]
fn sneak_with_perstealth_arms_silently_and_filters_the_move() {
    let mut core = Core::new(world(), CoreConfig::default());
    let sneak = core.attach_player(person("Shade", 1, false));
    let sharp = core.attach_player(person("Hawk", 2, true));
    let dull = core.attach_player(person("Mole", 2, false));
    core.drain_events();

    core.input(sneak, "sneak");
    let events = core.drain_events();
    let own = texts(&events, sneak);
    assert!(own.contains("Attempting to sneak..."), "{own:?}");
    assert!(
        !own.contains("don't think"),
        "PerStealth auto-success is silent: {own:?}"
    );

    core.input(sneak, "n");
    let events = core.drain_events();
    let to_sharp = texts(&events, sharp);
    let to_dull = texts(&events, dull);
    assert!(
        to_sharp.contains("You notice Shade sneaking out to the north"),
        "high perception sees the sneak: {to_sharp:?}"
    );
    assert!(
        !to_dull.contains("Shade"),
        "low perception sees nothing at all: {to_dull:?}"
    );
    assert!(
        !to_sharp.contains("just left") && !to_dull.contains("just left"),
        "the normal departure broadcast is suppressed"
    );
}

#[test]
fn hide_sets_the_hidden_flag_and_leaves_also_here() {
    let mut core = Core::new(world(), CoreConfig::default());
    let hider = core.attach_player(person("Wisp", 1, false));
    let watcher = core.attach_player(person("Guard", 2, true));
    core.drain_events();

    // Stealth ~high for a level-10 thief; the fixed test seed decides
    // the roll — assert on the resulting state, not the roll.
    core.input(hider, "hide");
    let own = texts(&core.drain_events(), hider);
    assert!(own.contains("Attempting to hide..."), "{own:?}");
    if core.player_hidden(hider) {
        core.input(watcher, "look");
        let seen = texts(&core.drain_events(), watcher);
        assert!(
            !seen.contains("Wisp"),
            "hidden players leave the Also-here line: {seen:?}"
        );
    } else {
        // Failure is either the perception-gated self-doubt line or
        // plain silence (the DLL's bare \r) — never a success claim.
        assert!(!own.contains("hidden!"), "no success claim: {own:?}");
    }
}

#[test]
fn moving_normally_clears_hidden() {
    let mut core = Core::new(world(), CoreConfig::default());
    let hider = core.attach_player(person("Wisp", 1, false));
    core.drain_events();
    for _ in 0..20 {
        core.input(hider, "hide");
        core.drain_events();
        if core.player_hidden(hider) {
            break;
        }
    }
    assert!(core.player_hidden(hider), "eventually hides");
    core.input(hider, "n"); // un-sneaked movement
    core.drain_events();
    assert!(
        !core.player_hidden(hider),
        "non-sneak movement clears the hidden byte"
    );
}

// --- backstab (cmd_backstab 0x51573, move_player_to_fighter mode 4) ---

use mud_core::content::{AttackForm, Monster, MonsterId};

fn world_with_dummy() -> Content {
    let mut content = world();
    content.add_monster(Monster {
        id: MonsterId(1),
        name: "practice dummy".into(),
        hitpoints: 100_000,
        energy: 0,
        behaviour: 3,
        attacks: [AttackForm::default(); 5],
        ..Default::default()
    });
    content
}

fn hide_until_hidden(core: &mut Core, s: SessionId) {
    for _ in 0..30 {
        core.input(s, "hide");
        core.drain_events();
        if core.player_hidden(s) {
            return;
        }
    }
    panic!("never managed to hide");
}

#[test]
fn backstab_while_visible_is_a_plain_attack() {
    // cmd_backstab: not hidden and not sneak-armed -> mode 5 silently.
    let mut core = Core::new(world_with_dummy(), CoreConfig::default());
    let s = core.attach_player(person("Knife", 1, false));
    core.spawn_monster(MonsterId(1), HERE).unwrap();
    core.drain_events();
    core.input(s, "backstab dummy");
    let own = texts(&core.drain_events(), s);
    assert!(own.contains("*Combat Engaged*"), "{own:?}");
    assert!(!own.contains("surprise"), "no surprise verb visible: {own:?}");
}

#[test]
fn hidden_backstab_surprises_then_reverts() {
    // High Agl/Int: mode-4 accuracy (Agl+Stealth)/2 + Agl/2 clears 100
    // — the surprise swing cannot miss, so the revert must fire.
    let mut core = Core::new(world_with_dummy(), CoreConfig::default());
    let s = core.attach_player(person("Knife", 1, true));
    core.spawn_monster(MonsterId(1), HERE).unwrap();
    core.drain_events();
    hide_until_hidden(&mut core, s);
    core.input(s, "backstab dummy");
    let mut shown = texts(&core.drain_events(), s);
    for _ in 0..10 {
        core.tick();
        shown.push_str(&texts(&core.drain_events(), s));
    }
    assert!(
        shown.contains("surprise"),
        "the mode-4 swing wraps its verb in surprise: {shown:?}"
    );
    // After the first landed hit the mode reverts to normal (M3: the
    // autocombat +8 revert) — later rounds stop surprising.
    let mut later = String::new();
    for _ in 0..15 {
        core.tick();
        later.push_str(&texts(&core.drain_events(), s));
    }
    assert!(
        !later.contains("surprise"),
        "post-hit rounds swing normally: {later:?}"
    );
}

// --- the add_delay command delay (theft.md §8/§9/§11) ---

#[test]
fn thief_actions_charge_a_delay_that_gates_sneak_and_hide() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Shade", 1, false));
    core.drain_events();

    // sneak charges 1 unit; an immediate hide is refused.
    core.input(s, "sneak");
    core.drain_events();
    core.input(s, "hide");
    let own = texts(&core.drain_events(), s);
    assert!(
        own.contains("You must wait before you may do that!"),
        "{own:?}"
    );
    // After the delay ages out (1 unit ~ 1s tick), hide proceeds.
    for _ in 0..3 {
        core.tick();
    }
    core.drain_events();
    core.input(s, "hide");
    let own = texts(&core.drain_events(), s);
    assert!(own.contains("Attempting to hide..."), "{own:?}");
}

// --- HIDE <item> / <n> <currency>: the thief's stash (theft.md §11.2) ---

use mud_core::content::{Item, ItemId};

fn stash_world() -> Content {
    let mut content = world();
    content.add_item(Item {
        id: ItemId(10),
        name: "ruby".into(),
        weight: 1,
        uses: -1,
        gettable: 1,
        ..Default::default()
    });
    content.add_item(Item {
        id: ItemId(11),
        name: "cursed idol".into(),
        weight: 1,
        uses: -1,
        gettable: 1,
        not_droppable: 1,
        ..Default::default()
    });
    content
}

#[test]
fn hidden_items_leave_the_floor_list_until_searched() {
    let mut core = Core::new(stash_world(), CoreConfig::default());
    let mut p = person("Stash", 1, false);
    p.inventory.push((ItemId(10), -1));
    let s = core.attach_player(p);
    let other = core.attach_player(person("Passerby", 2, true));
    core.drain_events();

    core.input(s, "hide ruby");
    let own = texts(&core.drain_events(), s);
    assert!(own.contains("You hid ruby."), "{own:?}");
    assert!(core.player_snapshot(s).inventory.is_empty());

    // A plain look shows nothing on the floor.
    core.input(other, "look");
    let seen = texts(&core.drain_events(), other);
    assert!(!seen.contains("ruby"), "hidden stash invisible: {seen:?}");

    // Waiting out the hide delay, a bare search lists it.
    for _ in 0..3 {
        core.tick();
    }
    core.drain_events();
    core.input(other, "search");
    let seen = texts(&core.drain_events(), other);
    assert!(seen.contains("ruby"), "search reveals the stash: {seen:?}");

    // And get retrieves it by name even without a search.
    core.input(s, "get ruby");
    core.drain_events();
    assert!(
        core.player_snapshot(s).inventory.contains(&(ItemId(10), -1)),
        "the stasher takes it back"
    );
}

#[test]
fn not_droppable_items_refuse_to_hide() {
    let mut core = Core::new(stash_world(), CoreConfig::default());
    let mut p = person("Stash", 1, false);
    p.inventory.push((ItemId(11), -1));
    let s = core.attach_player(p);
    core.drain_events();
    core.input(s, "hide idol");
    let own = texts(&core.drain_events(), s);
    assert!(own.contains("You may not hide that item!"), "{own:?}");
}

// --- move_user's independent transit reroll (theft.md §11.1,
// 11945-11960): being armed is not a guarantee. A second, independent
// draw against the same §11.3 helper decides the actual transit;
// PerStealth still bypasses it, everyone else can still fail visibly.

fn world_with_reroll_classes() -> Content {
    let mut content = world();
    // Stealth-capable, but NO PerStealth — both the arm roll (cmd_sneak)
    // and the new transit roll are genuine draws, not auto-passes.
    content.add_class(Class {
        id: ClassId(3),
        name: "Footpad".into(),
        abilities: vec![
            (Ability::from_id(0x67).unwrap(), 1),  // ClassStealth (the gate)
            (Ability::from_id(0x1b).unwrap(), 20), // Stealth bonus
        ],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    // A huge Perception bonus so a perception-filtered broadcast is
    // caught deterministically (roll(0,100) < perception is then
    // certain) — this class exists only to make the test's OBSERVATION
    // reliable, it plays no part in the mechanic under test.
    content.add_class(Class {
        id: ClassId(4),
        name: "Eagle-Eye".into(),
        abilities: vec![(Ability::from_id(0x4d).unwrap(), 500)], // Perception
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

/// Retries SNEAK on `core` until the armed bit is set (bounded — the
/// per-attempt chance is the same §11.3 helper, so this converges fast).
/// Each attempt charges the thief-family command delay (theft.md §11),
/// so age it off between attempts the same way
/// `thief_actions_charge_a_delay_that_gates_sneak_and_hide` does.
fn arm_sneak(core: &mut Core, sneaker: SessionId) {
    for _ in 0..60 {
        for _ in 0..3 {
            core.tick();
        }
        core.drain_events();
        core.input(sneaker, "sneak");
        core.drain_events();
        if core.player_snapshot(sneaker).sneak_armed {
            return;
        }
    }
    panic!("never managed to arm sneak");
}

#[test]
fn sneak_transit_reroll_can_fail_after_arming() {
    // Armed + roll fails -> normal, fully-broadcast move; sneak_armed
    // cleared, and — corrected 2026-08-22 against live-board play
    // (theft.md §11.1) — the failure is what causes "You make a sound
    // as you enter the room!", the board's own break announcement, NOT
    // a separate decorative self-awareness roll layered on the
    // surviving path (see `sneak_transit_reroll_passing_stays_sneaky`).
    let mut core = Core::new(world_with_reroll_classes(), CoreConfig::default());
    let sneaker = core.attach_player(person("Shade", 3, false));
    let watcher = core.attach_player(person("Guard", 4, false));
    core.drain_events();

    for _ in 0..100 {
        arm_sneak(&mut core, sneaker);
        assert!(
            core.player_snapshot(sneaker).sneak_armed,
            "arm_sneak's own postcondition"
        );
        core.input(sneaker, "n");
        let events = core.drain_events();
        let to_watcher = texts(&events, watcher);
        let to_sneaker = texts(&events, sneaker);
        if to_watcher.contains("just left to the north") {
            // Normal path taken: the transit roll failed.
            assert!(
                !to_watcher.contains("sneaking"),
                "no sneak notice on a failed transit: {to_watcher:?}"
            );
            assert!(
                to_sneaker.contains("You make a sound as you enter the room!"),
                "the break is announced to the sneaker: {to_sneaker:?}"
            );
            assert!(
                !core.player_snapshot(sneaker).sneak_armed,
                "the armed bit is cleared on failure too"
            );
            return;
        }
        // Roll passed this attempt (sneaky move) — the watcher walked
        // back in via the far room in the earlier iteration's setup, so
        // reset the sneaker's position for the next attempt.
        if core.player_snapshot(sneaker).location == THERE {
            core.input(sneaker, "s");
            core.drain_events();
        }
    }
    panic!("never observed a failed transit roll in 100 attempts");
}

#[test]
fn sneak_transit_reroll_passing_stays_sneaky() {
    // Armed + roll passes -> the sneaky path: perception-filtered
    // notices, normal broadcasts suppressed. Corrected 2026-08-22
    // against live-board play (theft.md §11.1): sneak PERSISTS, so a
    // passing roll must leave the armed bit SET, not clear it -- the
    // opposite of what this test asserted before. Player-observed on a
    // real board: "sneak persists until it wears off, and the board
    // tells you when it stops."
    let mut core = Core::new(world_with_reroll_classes(), CoreConfig::default());
    let sneaker = core.attach_player(person("Shade", 3, false));
    let watcher = core.attach_player(person("Guard", 4, false)); // Eagle-Eye: catches it for sure
    core.drain_events();

    for _ in 0..100 {
        arm_sneak(&mut core, sneaker);
        core.input(sneaker, "n");
        let events = core.drain_events();
        let to_watcher = texts(&events, watcher);
        let to_sneaker = texts(&events, sneaker);
        if to_watcher.contains("just left to the north") {
            // Roll failed this attempt — reset and retry.
            if core.player_snapshot(sneaker).location == THERE {
                core.input(sneaker, "s");
                core.drain_events();
            }
            continue;
        }
        assert!(
            to_watcher.contains("You notice Shade sneaking out to the north"),
            "the roll passed: the perception-filtered notice fires: {to_watcher:?}"
        );
        assert!(
            !to_watcher.contains("just left"),
            "the normal broadcast is suppressed on a passing roll: {to_watcher:?}"
        );
        assert!(
            !to_sneaker.contains("make a sound"),
            "a surviving sneak is not a break: no announcement: {to_sneaker:?}"
        );
        assert!(
            core.player_snapshot(sneaker).sneak_armed,
            "sneak persists: the armed bit survives a passing transit roll"
        );
        return;
    }
    panic!("never observed a passing transit roll in 100 attempts");
}

#[test]
fn sneak_transit_reroll_perstealth_skips_the_draw() {
    // PerStealth (ability 0xba) auto-passes BOTH the arm roll (cmd_sneak)
    // and this second, independent transit roll (11946-11947:
    // user_has_ability(0xba,...) short-circuits before FUN_0046cc43 /
    // genrdn are ever reached). Class 1 ("Thief" in world()) carries
    // PerStealth already.
    let mut core = Core::new(world(), CoreConfig::default());
    let sneaker = core.attach_player(person("Shade", 1, false));
    let watcher = core.attach_player(person("Hawk", 2, true));
    core.drain_events();

    core.input(sneaker, "sneak");
    core.drain_events();
    assert!(
        core.player_snapshot(sneaker).sneak_armed,
        "PerStealth auto-arms on the first attempt, no retry needed"
    );

    let draws_before = core.debug_rng_draws();
    core.input(sneaker, "n");
    let events = core.drain_events();
    let draws_after = core.debug_rng_draws();

    // Two draws, neither the transit stealth-chance roll:
    //   1. give_monsters_a_free_attack's unconditional per-departure
    //      roll (drawn even with nothing to hit — see move_player).
    //   2. broadcast_sneak's per-candidate perception check, one draw
    //      for the watcher present in the departure room.
    // No self-awareness "make a sound" roll any more (corrected
    // 2026-08-22, theft.md §11.1): that decorative roll only ever fired
    // on the SURVIVING path, and under the persistent model the line is
    // now the deterministic announcement of a BREAK -- printed straight
    // off `broke`, no roll of its own. PerStealth never breaks, so on
    // this path the line never prints and nothing is drawn for it.
    // PerStealth also means FUN_0046cc43 + its genrdn were never
    // reached — if they had been, this delta would be 3 (roll drawn and
    // passed) or a differently-composed 1 (roll drawn and failed, which
    // would also flip the assertions below to the normal-move wording).
    assert_eq!(
        draws_after - draws_before,
        2,
        "PerStealth skips the transit stealth-chance draw entirely"
    );

    let to_watcher = texts(&events, watcher);
    assert!(
        to_watcher.contains("You notice Shade sneaking out to the north"),
        "PerStealth is always sneaky: {to_watcher:?}"
    );
    assert!(!to_watcher.contains("just left"), "{to_watcher:?}");
}

#[test]
fn coins_stash_and_report() {
    let mut core = Core::new(stash_world(), CoreConfig::default());
    let mut p = person("Stash", 1, false);
    p.coins.copper = 100;
    let s = core.attach_player(p);
    core.drain_events();
    core.input(s, "hide 60 copper");
    let own = texts(&core.drain_events(), s);
    assert!(own.contains("You hid 60 copper farthings."), "{own:?}");
    assert_eq!(core.player_snapshot(s).coins.copper, 40);
    // More than held refuses.
    for _ in 0..3 {
        core.tick();
    }
    core.drain_events();
    core.input(s, "hide 500 copper");
    let own = texts(&core.drain_events(), s);
    assert!(
        own.contains("You don't have 500 copper farthings to hide!"),
        "{own:?}"
    );
}
