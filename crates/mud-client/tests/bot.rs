//! Bot policy tests: replay synthetic event sequences, assert the
//! commands the bot decides to send. The decision core is pure — no
//! sockets, no timing.

use mud_client::bot::{Bot, BotAction, BotConfig};
use mud_client::events::{Actor, Event, RoomView};

// The Blood Pit's exits are "closed door north, up" verbatim — see the
// same room asserted in tests/parse.rs. Do not sanitise them here: the
// display token is not a movement command, and that gap is the bug
// `flees_through_a_closed_door` pins.
fn room(also_here: &[&str]) -> Event {
    Event::RoomSeen(RoomView {
        name: "Arena, Blood Pit".into(),
        exits: vec!["closed door north".into(), "up".into()],
        also_here: also_here.iter().map(|s| s.to_string()).collect(),
        items: vec![],
    })
}

fn combat_bot() -> Bot {
    Bot::new(BotConfig {
        auto_combat: true,
        ignore: vec!["town guard".into()],
        ..BotConfig::default()
    })
}

#[test]
fn attacks_monster_on_sighting() {
    let mut bot = combat_bot();
    let actions = bot.on_event(&room(&["kobold thief"]));
    assert_eq!(actions, vec![BotAction::Send("a thief".into())]);
}

#[test]
fn does_not_attack_ignored_names() {
    let mut bot = combat_bot();
    let actions = bot.on_event(&room(&["town guard"]));
    assert!(actions.is_empty());
}

#[test]
fn does_not_attack_when_toggle_off() {
    let mut bot = Bot::new(BotConfig::default());
    let actions = bot.on_event(&room(&["kobold thief"]));
    assert!(actions.is_empty());
}

#[test]
fn attacks_monster_that_walks_in() {
    let mut bot = combat_bot();
    let actions = bot.on_event(&Event::ActorEntered {
        name: "kobold thief".into(),
        from: Some("east".into()),
    });
    assert_eq!(actions, vec![BotAction::Send("a thief".into())]);
}

#[test]
fn does_not_spam_attack_same_target() {
    let mut bot = combat_bot();
    assert_eq!(bot.on_event(&room(&["kobold thief"])).len(), 1);
    // Same room block again (e.g. a look): already engaged, no re-send.
    assert!(bot.on_event(&room(&["kobold thief"])).is_empty());
    // Target gone, then a new one appears: engage again.
    assert!(bot.on_event(&room(&[])).is_empty());
    assert_eq!(bot.on_event(&room(&["kobold thief"])).len(), 1);
}

#[test]
fn heals_below_threshold() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        heal_at_percent: 50,
        heal_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    assert!(
        bot.on_event(&Event::Prompt { hp: 25, mana: None })
            .is_empty(),
        "62% hp: no heal"
    );
    let actions = bot.on_event(&Event::Prompt { hp: 19, mana: None });
    assert_eq!(actions, vec![BotAction::Send("rest".into())]);
}

#[test]
fn heal_requires_known_max_hp() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        heal_at_percent: 50,
        max_hp: 0, // unknown: percent policies stay off
        ..BotConfig::default()
    });
    assert!(
        bot.on_event(&Event::Prompt { hp: 1, mana: None })
            .is_empty()
    );
}

#[test]
fn flees_below_flee_threshold_via_last_known_exit() {
    let mut bot = Bot::new(BotConfig {
        auto_flee: true,
        flee_at_percent: 25,
        max_hp: 40,
        ..BotConfig::default()
    });
    // Bot learns the room first.
    bot.on_event(&room(&[]));
    let actions = bot.on_event(&Event::Prompt { hp: 8, mana: None });
    assert_eq!(actions, vec![BotAction::Send("north".into())]);
}

#[test]
fn flee_takes_priority_over_heal() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        heal_at_percent: 50,
        auto_flee: true,
        flee_at_percent: 25,
        max_hp: 40,
        ..BotConfig::default()
    });
    bot.on_event(&room(&[]));
    let actions = bot.on_event(&Event::Prompt { hp: 5, mana: None });
    assert_eq!(actions, vec![BotAction::Send("north".into())]);
}

#[test]
fn does_not_repeat_heal_while_still_hurt() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        heal_at_percent: 50,
        heal_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 19, mana: None }),
        vec![BotAction::Send("rest".into())]
    );
    // Prompts arrive in bursts — async output disturbs the dangling
    // prompt and the board re-prompts — and these are all still inside
    // the heal band. One "rest" covers them; re-sending on each prompt
    // trips flood control.
    assert!(
        bot.on_event(&Event::Prompt { hp: 18, mana: None })
            .is_empty()
    );
    assert!(
        bot.on_event(&Event::Prompt { hp: 17, mana: None })
            .is_empty()
    );
    // Back above the threshold, then hurt again: heal again.
    assert!(
        bot.on_event(&Event::Prompt { hp: 30, mana: None })
            .is_empty()
    );
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 15, mana: None }),
        vec![BotAction::Send("rest".into())]
    );
}

#[test]
fn heals_when_hurt_but_no_exit_is_known() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        heal_at_percent: 50,
        auto_flee: true,
        flee_at_percent: 25,
        heal_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    // No room block seen yet, so there is no exit to flee through;
    // the heal policy still applies.
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 5, mana: None }),
        vec![BotAction::Send("rest".into())]
    );
}

#[test]
fn grabs_dropped_coins() {
    let mut bot = Bot::new(BotConfig {
        auto_get: true,
        ..BotConfig::default()
    });
    let actions = bot.on_event(&Event::Line("12 silver drop to the ground.".into()));
    assert_eq!(actions, vec![BotAction::Send("get silver".into())]);
}

#[test]
fn does_not_mistake_a_collapsing_actor_for_coins() {
    let mut bot = Bot::new(BotConfig {
        auto_get: true,
        ..BotConfig::default()
    });
    // Verbatim from the oracle corpus: a downed actor, not loot. The
    // leading count is what carries the exclusion (the verb and the "!"
    // are corroborating, not load-bearing — a mutant that relaxes either
    // alone still fails `grabs_dropped_coins`).
    assert!(
        bot.on_event(&Event::Line("Vexil drops to the ground!".into()))
            .is_empty()
    );
}

#[test]
fn ignores_coin_drops_when_toggle_off() {
    let mut bot = Bot::new(BotConfig::default());
    assert!(
        bot.on_event(&Event::Line("12 silver drop to the ground.".into()))
            .is_empty()
    );
}

#[test]
fn does_not_attack_players() {
    let mut bot = combat_bot();
    // "Also here:" lists players first, then monsters (game.rs builds it
    // in that order, and the corpus has "Also here: Aiken."). Attacking
    // the player is a PK attempt; the monster behind them is the target.
    let actions = bot.on_event(&room(&["Aiken", "kobold thief"]));
    assert_eq!(actions, vec![BotAction::Send("a thief".into())]);
}

#[test]
fn attacks_by_trailing_noun_not_rolled_adjective() {
    let mut bot = combat_bot();
    // Instance names carry a rolled adjective, but the board resolves
    // targets against the TEMPLATE name — "a fat kobold thief" is three
    // words against a two-word template and cannot match, so it would be
    // said aloud instead of swung.
    let actions = bot.on_event(&room(&["fat kobold thief"]));
    assert_eq!(actions, vec![BotAction::Send("a thief".into())]);
}

#[test]
fn ignore_list_survives_a_rolled_adjective() {
    let mut bot = combat_bot();
    // combat_bot ignores "town guard"; the board spawns "big town guard".
    assert!(bot.on_event(&room(&["big town guard"])).is_empty());
}

#[test]
fn re_engages_after_the_target_walks_out() {
    let mut bot = combat_bot();
    assert_eq!(bot.on_event(&room(&["kobold thief"])).len(), 1);
    bot.on_event(&Event::ActorLeft {
        name: "kobold thief".into(),
        to: Some("west".into()),
    });
    let actions = bot.on_event(&Event::ActorEntered {
        name: "giant rat".into(),
        from: Some("east".into()),
    });
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

#[test]
fn re_engages_after_the_target_dies() {
    let mut bot = combat_bot();
    assert_eq!(bot.on_event(&room(&["kobold thief"])).len(), 1);
    // Verbatim corpus death line. Without this the bot stays engaged on
    // a corpse and never attacks again — the normal end of every fight.
    bot.on_event(&Event::Line(
        "The kobold thief falls to the ground with a shrill cry.".into(),
    ));
    let actions = bot.on_event(&Event::ActorEntered {
        name: "giant rat".into(),
        from: Some("east".into()),
    });
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

#[test]
fn flees_through_a_closed_door() {
    let mut bot = Bot::new(BotConfig {
        auto_flee: true,
        flee_at_percent: 25,
        max_hp: 40,
        ..BotConfig::default()
    });
    // "closed door north" is a display token; the command is "north".
    bot.on_event(&room(&[]));
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 8, mana: None }),
        vec![BotAction::Send("north".into())]
    );
}

#[test]
fn flees_once_per_room_not_once_per_prompt() {
    let mut bot = Bot::new(BotConfig {
        auto_flee: true,
        flee_at_percent: 25,
        max_hp: 40,
        ..BotConfig::default()
    });
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&Event::Prompt { hp: 8, mana: None }).len(), 1);
    // Prompts arrive in bursts and can even double up on one physical
    // line; flooding movement while dying is the worst case.
    assert!(
        bot.on_event(&Event::Prompt { hp: 7, mana: None })
            .is_empty()
    );
    assert!(
        bot.on_event(&Event::Prompt { hp: 6, mana: None })
            .is_empty()
    );
    // Arriving somewhere new re-arms it: still hurt, so keep running.
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&Event::Prompt { hp: 6, mana: None }).len(), 1);
}

#[test]
fn rearm_releases_a_heal_that_never_landed() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        heal_at_percent: 50,
        heal_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    assert_eq!(bot.on_event(&Event::Prompt { hp: 19, mana: None }).len(), 1);
    assert!(
        bot.on_event(&Event::Prompt { hp: 19, mana: None })
            .is_empty()
    );
    // A heal that never lands leaves HP low forever, so the debounce
    // would latch and the character dies quietly. The runner re-arms on
    // evidence of refusal.
    bot.rearm();
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 19, mana: None }),
        vec![BotAction::Send("rest".into())]
    );
}

#[test]
fn stays_quiet_while_downed() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        heal_at_percent: 50,
        auto_flee: true,
        flee_at_percent: 25,
        max_hp: 40,
        ..BotConfig::default()
    });
    bot.on_event(&room(&[]));
    // Corpus has [HP=-161] while unconscious; commands do not land.
    assert!(
        bot.on_event(&Event::Prompt {
            hp: -161,
            mana: None
        })
        .is_empty()
    );
}

/// The runner needs to know whether a fight is still on: a stop is only
/// "idle" — and therefore finished — when nothing is engaged.
#[test]
fn engaged_reports_the_current_target() {
    let mut bot = combat_bot();
    assert_eq!(bot.engaged(), None);

    bot.on_event(&room(&["kobold thief"]));
    assert_eq!(bot.engaged(), Some("kobold thief"));

    // Death lines name the template, and end the fight.
    bot.on_event(&Event::Line(
        "The kobold thief falls to the ground with a shrill cry.".into(),
    ));
    assert_eq!(bot.engaged(), None);
}

#[test]
fn engaged_clears_when_the_target_walks_off() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["kobold thief"]));
    bot.on_event(&Event::ActorLeft {
        name: "kobold thief".into(),
        to: Some("west".into()),
    });
    assert_eq!(bot.engaged(), None);
}

// --- refused attacks -------------------------------------------------
//
// The board can refuse an attack outright instead of starting a fight.
// All three refusals below are real DLL strings (crime.md §3, verified
// present in re/WCCMMUD.DLL) and all three abort the swing, so none of
// them ever produces a death line, an ActorLeft, or a room block without
// the target. The engaged latch is set optimistically when the attack is
// sent, so a refusal that goes unnoticed latches it forever -- and
// `farm_stop` reads `engaged().is_some()` as "fight in progress", which
// both suppresses its idle poke and resets its dwell counter. The stop
// then never ends.

/// A refused attack is not an attack in progress. The latch must clear,
/// or the farm runner dwells at the stop forever.
#[test]
fn a_refused_attack_clears_the_engaged_latch() {
    let mut bot = combat_bot();
    assert_eq!(
        bot.on_event(&room(&["kobold thief"])),
        vec![BotAction::Send("a thief".into())]
    );
    assert_eq!(bot.engaged(), Some("kobold thief"));

    bot.on_event(&Event::Line(
        mud_core::crime::WARN_ON_EVIL_REFUSAL.to_string(),
    ));

    assert_eq!(bot.engaged(), None, "refusal left the bot latched on a fight that never started");
}

/// Clearing the latch is only half the fix. The monster is still stood
/// there, so the very next room block -- the runner's own idle `look` --
/// would re-attack it, be refused again, and spin at the pacer's floor
/// forever. A refusal must be remembered.
#[test]
fn a_refused_target_is_not_attacked_again() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["kobold thief"]));
    bot.on_event(&Event::Line(
        mud_core::crime::WARN_ON_EVIL_REFUSAL.to_string(),
    ));

    let actions = bot.on_event(&room(&["kobold thief"]));

    assert!(actions.is_empty(), "re-attacked a target the board already refused: {actions:?}");
}

/// Death lines are per-template prose and only 67 of the 1085 monsters
/// carrying a death record use the "falls to the ground" wording. The
/// other 1018 -- "The filthbug collapses, its legs curling tightly around
/// it." and friends -- must still end the fight, or the bot sits latched
/// on a corpse and the stop never ends.
#[test]
fn an_unrecognised_death_still_ends_the_fight() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["filthbug"]));
    assert_eq!(bot.engaged(), Some("filthbug"));

    // Verbatim from the shipped data: message 31, messageline3 -- and
    // then the line that always follows one of our kills.
    bot.on_event(&Event::Line(
        "The filthbug collapses, its legs curling tightly around it.".into(),
    ));
    bot.on_event(&Event::Line("You gain 12 experience.".into()));

    assert_eq!(bot.engaged(), None, "the kill went unnoticed");
}

/// The backstop, for a fight that ends with no wording we know at all:
/// an exp-less kill, a monster somebody else finished, a template whose
/// death record is missing entirely (14 of them ship that way). Quiet
/// prompts with no combat in them mean the fight is over.
#[test]
fn a_fight_that_goes_quiet_releases_the_latch() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["kobold thief"]));
    assert_eq!(bot.engaged(), Some("kobold thief"));

    for _ in 0..BotConfig::default().combat_idle_prompts {
        bot.on_event(&Event::Prompt { hp: 30, mana: None });
    }

    assert_eq!(bot.engaged(), None, "nothing has happened for several prompts");
}

/// The counterpart, and the one that matters for not breaking real
/// fights: while blows are still landing the latch must hold, however
/// many prompts go by. A fight can easily outlast the idle threshold.
#[test]
fn an_ongoing_fight_keeps_the_latch() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["kobold thief"]));

    for _ in 0..(BotConfig::default().combat_idle_prompts * 3) {
        bot.on_event(&Event::CombatHit {
            attacker: Actor::You,
            target: Actor::Other("The kobold thief".into()),
            damage: 4,
        });
        bot.on_event(&Event::Prompt { hp: 30, mana: None });
    }

    assert_eq!(
        bot.engaged(),
        Some("kobold thief"),
        "a fight in progress was abandoned"
    );
}

/// A swing that misses is still the fight happening.
#[test]
fn a_missed_swing_counts_as_the_fight_continuing() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["kobold thief"]));

    for _ in 0..(BotConfig::default().combat_idle_prompts * 2) {
        bot.on_event(&Event::CombatMiss {
            line: "You swing at the kobold thief and miss!".into(),
        });
        bot.on_event(&Event::Prompt { hp: 30, mana: None });
    }

    assert_eq!(bot.engaged(), Some("kobold thief"));
}
