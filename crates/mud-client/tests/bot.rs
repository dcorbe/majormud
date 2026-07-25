//! Bot policy tests: replay synthetic event sequences, assert the
//! commands the bot decides to send. The decision core is pure — no
//! sockets, no timing.

use mud_client::bot::{Bot, BotAction, BotConfig};
use mud_client::events::{Event, RoomView};

fn room(also_here: &[&str]) -> Event {
    Event::RoomSeen(RoomView {
        name: "Arena, Blood Pit".into(),
        exits: vec!["north".into(), "up".into()],
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
    assert_eq!(actions, vec![BotAction::Send("a kobold thief".into())]);
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
    assert_eq!(actions, vec![BotAction::Send("a kobold thief".into())]);
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
    // count, the singular verb and the "!" all separate the two.
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
