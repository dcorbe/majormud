//! Bot policy tests: replay synthetic event sequences, assert the
//! commands the bot decides to send. The decision core is pure — no
//! sockets, no timing.

use mud_client::bot::{Bot, BotAction, BotConfig};
use mud_client::events::{Event, RoomView};

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
    // The board reprints the prompt on every regen tick, and these are
    // all still inside the heal band. One "rest" covers them; re-sending
    // on each prompt trips flood control.
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
    // Prompts arrive per regen tick and can even double up on one
    // physical line; flooding movement while dying is the worst case.
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
