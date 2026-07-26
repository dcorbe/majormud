//! ROB <player> + the evil-pair timers + FORGIVE (theft.md §3-5,
//! crime.md §4-5). Rob is the quiet crime: only the BUMP outcome ever
//! notifies the victim, the room never sees anything, and every attempt
//! charges 1 evil point through the player-victim path (victim-quality
//! multipliers, innocence gate) while banking an 11-slow-tick pair
//! timer that powers retaliation-free responses and FORGIVE refunds.

use mud_core::ability::Ability;
use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::crime::victim_multiplier;
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const ALLEY: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: ALLEY,
        name: "Alley".into(),
        ..Default::default()
    });
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
    // Thievery gate + a big bonus: the skill roll nearly always passes.
    content.add_class(Class {
        id: ClassId(1),
        name: "Thief".into(),
        abilities: vec![(Ability::from_id(0x27).unwrap(), 60)],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    // A skill-less klutz: rolls above T+10 nearly always (bump).
    content.add_class(Class {
        id: ClassId(2),
        name: "Oaf".into(),
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

fn person(name: &str, class: u16) -> Player {
    let stats = StatBlock {
        intellect: 60,
        wisdom: 40,
        strength: 40,
        health: 40,
        agility: 60,
        charm: 40,
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(class),
        level: 5,
        stats,
        base_stats: stats,
        current_hp: 30,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: ALLEY,
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
fn victim_multiplier_matches_the_table() {
    // crime.md §2.3: committed Lawful x3, Saint (<= -201) x3,
    // Good (<= -51) x2, everyone else x1.
    assert_eq!(victim_multiplier(0, true), 3);
    assert_eq!(victim_multiplier(-250, false), 3);
    assert_eq!(victim_multiplier(-100, false), 2);
    assert_eq!(victim_multiplier(0, false), 1);
    assert_eq!(victim_multiplier(120, false), 1);
}

#[test]
fn rob_syntax_and_unresolved_target() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(person("Fingers", 1));
    core.drain_events();
    core.input(s, "rob");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Syntax: ROB {user/monster}"), "{out:?}");
    core.input(s, "rob nobody");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("You don't see that anywhere!"), "{out:?}");
}

#[test]
fn successful_coin_rob_is_silent_and_charges_one_evil() {
    let mut core = Core::new(world(), CoreConfig::default());
    let robber = core.attach_player(person("Fingers", 1));
    let mut mark = person("Mark", 2);
    mark.coins.copper = 500;
    let victim = core.attach_player(mark);
    let observer = core.attach_player(person("Bystander", 2));
    core.drain_events();

    let mut stole = false;
    for _ in 0..60 {
        core.input(robber, "rob mark");
        let events = core.drain_events();
        let own = texts(&events, robber);
        let seen_by_victim = texts(&events, victim);
        let seen_by_room = texts(&events, observer);
        assert!(seen_by_room.is_empty() || !seen_by_room.contains("rob"),
            "the room never sees a rob: {seen_by_room:?}");
        if own.contains("You stole") {
            stole = true;
            assert!(
                seen_by_victim.is_empty() || !seen_by_victim.contains("stole"),
                "the victim is never told of a success: {seen_by_victim:?}"
            );
            break;
        }
    }
    assert!(stole, "a coin rob eventually lands");
    let taken = 500 - core.player_snapshot(victim).coins.copper;
    assert!(taken > 0, "coins actually moved");
    assert_eq!(
        core.player_snapshot(robber).coins.copper,
        taken,
        "the robber holds exactly what the mark lost"
    );
    assert!(core.player_fame(robber) >= 1, "every attempt charges evil");
}

#[test]
fn bump_notifies_only_the_victim() {
    let mut core = Core::new(world(), CoreConfig::default());
    let oaf = core.attach_player(person("Clumsy", 2)); // Thievery 0
    let victim = core.attach_player(person("Mark", 2));
    let observer = core.attach_player(person("Bystander", 2));
    core.drain_events();

    let mut bumped = false;
    for _ in 0..40 {
        core.input(oaf, "rob mark");
        let events = core.drain_events();
        let own = texts(&events, oaf);
        let to_victim = texts(&events, victim);
        let to_room = texts(&events, observer);
        assert!(!to_room.contains("bump"), "room hears nothing: {to_room:?}");
        if own.contains("You bump Mark as you try to rob him.") {
            bumped = true;
            assert!(
                to_victim.contains("Clumsy bumps you as he tries to rob you!"),
                "victim notified on the bump: {to_victim:?}"
            );
            break;
        }
    }
    assert!(bumped, "a bump eventually happens at Thievery 0");
}

#[test]
fn forgive_refunds_the_banked_evil() {
    let mut core = Core::new(world(), CoreConfig::default());
    let robber = core.attach_player(person("Fingers", 1));
    let mut mark = person("Mark", 2);
    mark.coins.copper = 100;
    let victim = core.attach_player(mark);
    core.drain_events();

    core.input(robber, "rob mark");
    core.drain_events();
    let charged = core.player_fame(robber);
    assert!(charged >= 1, "the attempt charged evil");

    core.input(victim, "forgive fingers");
    let events = core.drain_events();
    let to_robber = texts(&events, robber);
    let to_victim = texts(&events, victim);
    assert!(
        to_robber.contains("The gods have forgiven you for your action."),
        "{to_robber:?}"
    );
    assert!(
        to_victim.contains("The gods have forgiven Fingers for his action."),
        "{to_victim:?}"
    );
    assert_eq!(core.player_fame(robber), 0, "exact refund");

    // A second forgive finds no node.
    core.input(victim, "forgive fingers");
    let to_victim = texts(&core.drain_events(), victim);
    assert!(
        to_victim.contains("The gods refuse to forgive Fingers for his actions."),
        "{to_victim:?}"
    );
}

#[test]
fn repeat_robs_recharge_and_replace_the_timer() {
    // crime.md §2.3: the rob path ALWAYS charges (the free-retaliation
    // window belongs to the ATTACK path); a repeat rob REPLACES the
    // pair timer, so forgiveness refunds only the last banked point.
    let mut core = Core::new(world(), CoreConfig::default());
    let robber = core.attach_player(person("Fingers", 1));
    let mut mark = person("Mark", 2);
    mark.coins.copper = 100;
    let victim = core.attach_player(mark);
    core.drain_events();

    core.input(robber, "rob mark");
    core.drain_events();
    core.input(robber, "rob mark");
    core.drain_events();
    assert_eq!(core.player_fame(robber), 2, "every attempt charges");

    core.input(victim, "forgive fingers");
    core.drain_events();
    assert_eq!(
        core.player_fame(robber),
        1,
        "forgive refunds the replaced node's single point"
    );
}

#[test]
fn lawful_and_warned_robbers_are_refused() {
    let mut core = Core::new(world(), CoreConfig::default());
    let mut saint = person("Saint", 1);
    saint.lawful = true;
    let s = core.attach_player(saint);
    let _mark = core.attach_player(person("Mark", 2));
    core.drain_events();
    core.input(s, "rob mark");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You have chosen a way of life which prevents this action."),
        "{out:?}"
    );

    let mut core = Core::new(world(), CoreConfig::default());
    let mut wary = person("Wary", 1);
    wary.warn_on_evil = true;
    let s = core.attach_player(wary);
    let _mark = core.attach_player(person("Mark", 2));
    core.drain_events();
    core.input(s, "rob mark");
    let out = texts(&core.drain_events(), s);
    assert!(
        out.contains("You have chosen a way of life which prevents this action."),
        "warn-on-evil robs share the lawful refusal (theft.md §4.1): {out:?}"
    );
}
