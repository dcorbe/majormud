//! Bot policy tests: replay synthetic event sequences, assert the
//! commands the bot decides to send. The decision core is pure — no
//! sockets, no timing.

use mud_client::bot::{Bot, BotAction, BotConfig};
use mud_client::events::{Actor, Event, RoomView};

// The Blood Pit's exits are "closed door north, up" verbatim — see the
// same room asserted in tests/parse.rs. Do not sanitise them here: the
// display token is not a movement command, and that gap is the bug
// `flees_through_a_closed_door` pins.
fn view(also_here: &[&str]) -> RoomView {
    RoomView {
        name: "Arena, Blood Pit".into(),
        exits: vec!["closed door north".into(), "up".into()],
        also_here: also_here.iter().map(|s| s.to_string()).collect(),
        items: vec![],
    }
}

fn room(also_here: &[&str]) -> Event {
    Event::RoomSeen(view(also_here))
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

/// A hit on us is deliberately NOT blind-countered. The attacker slot of
/// a hit line cannot be split out reliably — the attack verb is
/// per-monster data and can be multi-word: "The fierce orc trainee
/// all-out slashes you for 37 damage!" (live, oracle_charm_lifecycle5)
/// parses its attacker as "...trainee all-out", and a counter would have
/// sent "a all-out". Being hit by something unlisted is answered by the
/// farm's re-look instead (see tests/farm.rs,
/// `being_hit_invalidates_the_room_block_but_swinging_does_not`), where
/// the room block names the attacker properly and the bot engages from
/// it.
#[test]
fn a_hit_on_us_is_not_blindly_countered() {
    let mut bot = combat_bot();
    let actions = bot.on_event(&Event::CombatHit {
        attacker: Actor::Other("The fierce orc trainee all-out".into()),
        target: Actor::You,
        damage: 37,
    });
    assert!(actions.is_empty());
}

/// The general un-latch this family kept asking for. A kill can hide
/// BOTH known end signals at once — a prose death line ("The acid slime
/// dissolves into a puddle of bluish goo.") while the untrained-XP cap
/// suppresses the experience award — and the latched bot then ignored a
/// fresh spawn for 29 seconds live (2026-08-01 arena run) until the
/// quiet-prompt backstop expired. The board announces the end itself:
/// "*Combat Off*". Believe it.
#[test]
fn combat_off_clears_the_latch() {
    let mut bot = combat_bot();
    assert_eq!(bot.on_event(&room(&["acid slime"])).len(), 1);
    // Prose death + XP cap: neither a death mark nor an award arrives.
    assert!(
        bot.on_event(&Event::Line(
            "The acid slime dissolves into a puddle of bluish goo.".into()
        ))
        .is_empty()
    );
    let actions = bot.on_event(&Event::Line("*Combat Off*".into()));
    assert!(actions.is_empty());
    // The next spawn must be engaged, not ignored by a corpse latch.
    let actions = bot.on_event(&Event::ActorEntered {
        name: "thin giant rat".into(),
        from: None,
    });
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

/// An attack that resolves NO target falls through to SAY — the board
/// answers `You say "a beast"` instead of `*Combat Engaged*`. Live
/// (2026-08-01): the carrion beast left south in the same instant the
/// sighting attack went out, its departure line arrived corrupted by
/// our own command echo ("a becarrion beast just left...") so ActorLeft
/// could not match, and the bot sat latched on a phantom for 20 seconds
/// while two thieves whiffed at it. The say echo of our own attack
/// command IS the board saying the swing never started.
#[test]
fn a_say_fallthrough_clears_the_latch() {
    let mut bot = combat_bot();
    assert_eq!(
        bot.on_event(&room(&["carrion beast"])),
        vec![BotAction::Send("a beast".into())]
    );
    assert!(
        bot.on_event(&Event::Line("You say \"a beast\"".into()))
            .is_empty()
    );
    let actions = bot.on_event(&Event::ActorEntered {
        name: "kobold thief".into(),
        from: None,
    });
    assert_eq!(actions, vec![BotAction::Send("a thief".into())]);
}

/// Somebody ELSE's speech — or our own words that are not the attack we
/// have in flight — proves nothing about the fight.
#[test]
fn unrelated_speech_does_not_clear_the_latch() {
    let mut bot = combat_bot();
    assert_eq!(bot.on_event(&room(&["carrion beast"])).len(), 1);
    bot.on_event(&Event::Line("You say \"hello there\"".into()));
    // Still latched: the same room block must not re-engage.
    assert!(bot.on_event(&room(&["carrion beast"])).is_empty());
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

/// Combat bot with healing armed — the shape that produced tonight's
/// death spiral (2026-08-01 run3): HP 21/52 beside a cave bear, `rest`
/// disengages combat, the Combat Off un-latch frees the bot, the next
/// block re-engages, the engage breaks the rest — forever, taking bear
/// swings every ~5s round while neither resting nor fighting. In an
/// occupied room the coherent choices are fight or flee; rest is for
/// cleared rooms.
fn healing_fighter() -> Bot {
    Bot::new(BotConfig {
        auto_combat: true,
        auto_heal: true,
        heal_at_percent: 50,
        max_hp: 52,
        ..BotConfig::default()
    })
}

#[test]
fn does_not_rest_while_the_room_lists_a_monster() {
    let mut bot = healing_fighter();
    // The block engages the bear; the low prompt must fight on, not rest.
    assert_eq!(bot.on_event(&room(&["cave bear"])).len(), 1);
    assert!(
        bot.on_event(&Event::Prompt { hp: 21, mana: None }).is_empty(),
        "resting mid-fight is the spiral"
    );
}

/// The exact spiral: the fight ends (rest disengaged it, or the bear
/// died in prose under the XP cap), the latch clears — but the room
/// STILL lists the bear. Resting now just gets broken by the re-engage.
#[test]
fn does_not_rest_after_combat_off_while_the_room_still_has_work() {
    let mut bot = healing_fighter();
    bot.on_event(&room(&["cave bear"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    assert!(
        bot.on_event(&Event::Prompt { hp: 21, mana: None }).is_empty(),
        "the room was never proven clear"
    );
}

/// The other side: suppression must not latch. The moment a block
/// proves the room clear, the very next low prompt rests.
#[test]
fn rests_once_the_room_is_proven_clear() {
    let mut bot = healing_fighter();
    bot.on_event(&room(&["cave bear"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    assert!(bot.on_event(&Event::Prompt { hp: 21, mana: None }).is_empty());
    bot.on_event(&room(&[]));
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 21, mana: None }),
        vec![BotAction::Send("rest".into())]
    );
}

/// A walk-in makes resting wrong again, before any block re-lists it.
#[test]
fn a_walk_in_makes_resting_wrong_again() {
    let mut bot = healing_fighter();
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&Event::Prompt { hp: 21, mana: None }).len(), 1);
    // HP recovers past the threshold: the heal debounce releases.
    bot.on_event(&Event::Prompt { hp: 40, mana: None });
    // A rat walks in (and is engaged); dropping low again must not rest.
    bot.on_event(&Event::ActorEntered {
        name: "giant rat".into(),
        from: None,
    });
    assert!(
        bot.on_event(&Event::Prompt { hp: 21, mana: None }).is_empty(),
        "an arrival is work; rest would be broken by the fight"
    );
}

/// Occupants the bot would never swing at do not block resting — the
/// bit follows would_attack (ignore list, case rule, refusals), not raw
/// occupancy.
#[test]
fn an_ignored_occupant_does_not_block_resting() {
    let mut bot = Bot::new(BotConfig {
        auto_combat: true,
        auto_heal: true,
        heal_at_percent: 50,
        max_hp: 52,
        ignore: vec!["town guard".into()],
        ..BotConfig::default()
    });
    bot.on_event(&room(&["town guard"]));
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 21, mana: None }),
        vec![BotAction::Send("rest".into())]
    );
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

// --- threat-ordered targeting ----------------------------------------
//
// "Also here:" is listed in the board's own order, which has nothing to
// do with danger. Taking the first attackable name meant punching a
// giant rat while a cave bear hit for 17 -- observed live on 2026-07-30.

fn threat() -> std::collections::HashMap<String, i64> {
    // Scores as the shipped data ranks them: experience, which is the
    // board's own valuation of how hard a thing is.
    [("cave bear", 100i64), ("filthbug", 12), ("giant rat", 9)]
        .into_iter()
        .map(|(n, s)| (n.to_string(), s))
        .collect()
}

fn threat_bot() -> Bot {
    Bot::with_threat(
        BotConfig {
            auto_combat: true,
            ..BotConfig::default()
        },
        std::sync::Arc::new(threat()),
    )
}

#[test]
fn the_most_dangerous_thing_in_the_room_is_attacked_first() {
    let mut bot = threat_bot();
    // Board order puts the rat first; the bear is what matters.
    let actions = bot.on_event(&room(&["giant rat", "cave bear", "filthbug"]));
    assert_eq!(actions, vec![BotAction::Send("a bear".into())]);
}

/// Instances carry a rolled adjective but the threat table is keyed by
/// template, so "fierce filthbug" has to score as a filthbug.
#[test]
fn a_rolled_adjective_still_scores_as_its_template() {
    let mut bot = threat_bot();
    let actions = bot.on_event(&room(&["giant rat", "fierce filthbug"]));
    assert_eq!(actions, vec![BotAction::Send("a filthbug".into())]);
}

/// Nothing known about any of them: fall back to the board's order
/// rather than inventing a ranking.
#[test]
fn unknown_monsters_keep_the_boards_order() {
    let mut bot = threat_bot();
    let actions = bot.on_event(&room(&["grue", "wumpus"]));
    assert_eq!(actions, vec![BotAction::Send("a grue".into())]);
}

/// Ranking must not override the ignore list -- a high-threat town
/// guard is exactly the thing never to swing at.
#[test]
fn the_ignore_list_still_wins_over_threat() {
    let mut bot = Bot::with_threat(
        BotConfig {
            auto_combat: true,
            ignore: vec!["bear".into()],
            ..BotConfig::default()
        },
        std::sync::Arc::new(threat()),
    );
    let actions = bot.on_event(&room(&["giant rat", "cave bear"]));
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

/// Prompts arrive in BURSTS, not one per combat round: async output
/// disturbs the dangling prompt and the board re-prompts, so
/// "[HP=31]:[HP=32]:" lands on one physical line. A slow fight — a cave
/// bear is 50hp against single-digit hits — therefore racks up quiet
/// prompts between rounds without the fight being over at all.
///
/// Abandoning the target there is worse than a wasted swing: `farm_stop`
/// reads a cleared latch as an idle room, dwells out, and walks off
/// mid-fight.
#[test]
fn a_burst_of_prompts_between_rounds_does_not_abandon_the_fight() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["cave bear"]));
    assert_eq!(bot.engaged(), Some("cave bear"));

    // Three rounds, each followed by a burst of prompts and some
    // unrelated chatter — the shape of a real fight.
    for _ in 0..3 {
        bot.on_event(&Event::CombatHit {
            attacker: Actor::You,
            target: Actor::Other("The cave bear".into()),
            damage: 4,
        });
        for _ in 0..5 {
            bot.on_event(&Event::Prompt { hp: 30, mana: None });
        }
    }

    assert_eq!(
        bot.engaged(),
        Some("cave bear"),
        "walked away from a fight that was still going"
    );
}

/// Busy monsters render with a status prefix — "(Resting) fierce
/// filthbug" is verbatim from the board. `is_attackable` looked at the
/// FIRST character to tell a lowercase monster from a capitalised
/// player, so "(" made every one of them invisible to the bot: it stood
/// in a room full of things and swung at none of them.
#[test]
fn a_monster_with_a_status_prefix_is_still_a_monster() {
    let mut bot = combat_bot();
    let actions = bot.on_event(&room(&["(Resting) fierce filthbug"]));
    assert_eq!(actions, vec![BotAction::Send("a filthbug".into())]);
}

/// The prefix must not become a way to smuggle a PLAYER past the check —
/// players are capitalised, and attacking one is a PK attempt.
#[test]
fn a_prefixed_player_is_still_not_attacked() {
    let mut bot = combat_bot();
    assert!(bot.on_event(&room(&["(Resting) Vexil"])).is_empty());
}

// --- has_target: "is this room worth staying in" ---------------------
//
// The farm runner used to answer that by counting prompts since it last
// did something, and shipped three bugs doing it. The room block states
// the answer outright, and these pin the predicate it reads.

/// The question is about the ROOM, not about what we happen to be doing.
/// A monster we are mid-fight with is still listed under "Also here:"
/// (verified in re/oracle/oracle_attack_syntax.raw), and a caller asking
/// "is there anything here" must get `true` for it — otherwise the runner
/// reads a live fight as an empty room and walks out of it.
#[test]
fn has_target_sees_past_the_current_fight() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["giant rat", "cave bear"]));
    // No threat table here, so both score 0 and the board's own order
    // stands — which of the two it picked does not matter, only that a
    // fight is now outstanding.
    assert_eq!(bot.engaged(), Some("giant rat"), "test needs a live fight");
    assert!(
        bot.has_target(&view(&["giant rat", "cave bear"])),
        "a room we are fighting in read as having nothing to fight"
    );
}

/// Everything the bot would not swing at leaves the room effectively
/// empty: the ignore list, the case rule that tells a player from a
/// monster, and the toggle itself.
#[test]
fn has_target_is_false_for_names_we_would_never_attack() {
    let bot = combat_bot();
    assert!(!bot.has_target(&view(&[])), "empty room");
    assert!(!bot.has_target(&view(&["town guard"])), "ignore list");
    assert!(
        !bot.has_target(&view(&["Vexil"])),
        "players are not targets"
    );
    assert!(
        !Bot::new(BotConfig::default()).has_target(&view(&["kobold thief"])),
        "auto_combat off"
    );
}

/// A refusal is the board saying we may not attack this at all. The stop
/// must not be held open waiting for a swing that will never land — this
/// is the hang that shipped.
#[test]
fn has_target_is_false_for_a_refused_name() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["kobold thief"]));
    bot.on_event(&Event::Line(
        mud_core::crime::WARN_ON_EVIL_REFUSAL.to_string(),
    ));
    assert!(
        !bot.has_target(&view(&["kobold thief"])),
        "a refused monster held the room open"
    );
}

/// One name we would fight is enough, wherever it sits in the listing.
#[test]
fn has_target_is_true_when_anything_in_the_listing_qualifies() {
    let bot = combat_bot();
    assert!(bot.has_target(&view(&["town guard", "Vexil", "giant rat"])));
}

/// A monster that sits down mid-fight re-renders with a "(Resting) "
/// decoration spliced in (DLL 0xe06f6). The engaged latch was compared
/// against "Also here:" by EXACT string, so the decorated name did not
/// match the plain one we latched onto — the bot concluded its target
/// had left, cleared the latch, and immediately re-attacked the very
/// same monster. That is a duplicate command per room block, straight
/// into flood control.
#[test]
fn a_target_that_sits_down_is_not_treated_as_a_new_monster() {
    let mut bot = combat_bot();
    assert_eq!(
        bot.on_event(&room(&["fierce filthbug"])),
        vec![BotAction::Send("a filthbug".into())]
    );
    let actions = bot.on_event(&room(&["(Resting) fierce filthbug"]));
    assert!(
        actions.is_empty(),
        "re-attacked a monster we were already fighting: {actions:?}"
    );
    assert_eq!(
        bot.engaged(),
        Some("fierce filthbug"),
        "dropped a fight that was still going"
    );
}

/// The mirror case: we latched on through an `ActorEntered` that carried
/// the decoration, and the room block prints the plain name.
#[test]
fn a_decorated_latch_matches_the_plain_name_in_the_room_block() {
    let mut bot = combat_bot();
    bot.on_event(&Event::ActorEntered {
        name: "(Resting) fierce filthbug".into(),
        from: None,
    });
    assert!(bot.engaged().is_some(), "test needs a live fight");
    let actions = bot.on_event(&room(&["fierce filthbug"]));
    assert!(
        actions.is_empty(),
        "re-attacked a monster we were already fighting: {actions:?}"
    );
}

/// Normalising must not blind the latch to the target actually leaving.
#[test]
fn a_decorated_latch_still_clears_when_the_room_empties() {
    let mut bot = combat_bot();
    bot.on_event(&Event::ActorEntered {
        name: "(Resting) fierce filthbug".into(),
        from: None,
    });
    bot.on_event(&room(&[]));
    assert_eq!(bot.engaged(), None, "latched on a monster that is gone");
}

/// The farm runner builds a fresh bot for every stop, and again on every
/// lagged broadcast and every flee recovery. A refusal learned in one of
/// those lifetimes has to outlive it: otherwise the next bot re-engages
/// the same monster, is refused again, and the swing is repeated once per
/// monster per stop per lap. On the live board that is a crime-system
/// interaction, not a free no-op.
#[test]
fn a_refusal_outlives_the_bot_that_learned_it() {
    let refusals = mud_client::bot::Refusals::default();
    let threat = std::sync::Arc::new(mud_client::bot::ThreatTable::new());

    let mut first = Bot::with_refusals(
        BotConfig {
            auto_combat: true,
            ..BotConfig::default()
        },
        threat.clone(),
        refusals.clone(),
    );
    first.on_event(&room(&["kobold thief"]));
    first.on_event(&Event::Line(
        mud_core::crime::WARN_ON_EVIL_REFUSAL.to_string(),
    ));

    // The runner throws that bot away and builds another.
    let mut next = Bot::with_refusals(
        BotConfig {
            auto_combat: true,
            ..BotConfig::default()
        },
        threat,
        refusals,
    );
    let actions = next.on_event(&room(&["kobold thief"]));
    assert!(
        actions.is_empty(),
        "a rebuilt bot re-attacked a target the board had already refused: {actions:?}"
    );
    assert!(!next.has_target(&view(&["kobold thief"])));
}

// ---- The wander-out re-engage debounce (run4, 2026-08-01) ----
//
// A monster wandering out mid-fight prints *Combat Off* while the room
// block still lists it for the length of the leave transition. The
// un-latch freed the bot, the re-look's block re-engaged the leaver, the
// board flipped Engaged/Off again — ~40 look+attack cycles in 400ms
// live. A targetless Combat Off (the un-latch fired while we still
// believed we were engaged) must therefore start a same-noun cooldown:
// blocks stop re-engaging that noun until the board settles the question.

/// The run4 shape itself: Combat Off mid-fight, then a block still
/// listing the leaver. Re-attacking it is the spin.
#[test]
fn combat_off_mid_fight_does_not_reengage_the_leaver_from_the_next_block() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["thin giant rat"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    let actions = bot.on_event(&room(&["thin giant rat"]));
    assert!(
        actions.is_empty(),
        "re-engaged the leaver during its transition: {actions:?}"
    );
}

/// The cooldown must not outlive the question it answers: a monster
/// still listed two blocks after the Combat Off is not leaving — it is
/// standing there, and standing monsters get fought.
#[test]
fn a_monster_still_present_two_blocks_later_is_reengaged() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["thin giant rat"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    assert!(bot.on_event(&room(&["thin giant rat"])).is_empty());
    let actions = bot.on_event(&room(&["thin giant rat"]));
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

/// Absence settles it: once a block omits the leaver, the transition is
/// over, and the next same-noun sighting is a new instance.
#[test]
fn absence_ends_the_cooldown_and_the_next_sighting_engages() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["thin giant rat"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    assert!(bot.on_event(&room(&[])).is_empty());
    let actions = bot.on_event(&room(&["giant rat"]));
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

/// An arrival is affirmative evidence — a new instance walked in, and
/// waiting out a cooldown against it would let it hit first.
#[test]
fn an_arrival_event_overrides_the_cooldown() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["thin giant rat"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    let actions = bot.on_event(&Event::ActorEntered {
        name: "giant rat".into(),
        from: None,
    });
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

/// The leave line is the transition completing. After it, a same-noun
/// listing is a different rat.
#[test]
fn the_leave_line_ends_the_cooldown() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["thin giant rat"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    bot.on_event(&Event::ActorLeft {
        name: "thin giant rat".into(),
        to: Some("west".into()),
    });
    let actions = bot.on_event(&room(&["giant rat"]));
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

/// The cooldown is per-noun, not a combat holiday: everything else in
/// the block still gets engaged.
#[test]
fn other_names_still_engage_during_the_cooldown() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["thin giant rat"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    let actions = bot.on_event(&room(&["cave bear", "thin giant rat"]));
    assert_eq!(actions, vec![BotAction::Send("a bear".into())]);
}

/// A normal kill is NOT targetless: the death line un-latched before the
/// Combat Off arrived, so no cooldown starts and a silent respawn of the
/// same template is engaged without delay.
#[test]
fn a_kill_then_combat_off_starts_no_cooldown() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["thin giant rat"]));
    bot.on_event(&Event::Line("The thin giant rat falls to the ground.".into()));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    let actions = bot.on_event(&room(&["giant rat"]));
    assert_eq!(actions, vec![BotAction::Send("a rat".into())]);
}

/// The cooldown must not make the stop look finished: a leaver mid-
/// transition is still work-in-question, and declaring the room clear
/// would re-enable resting beside it.
#[test]
fn the_cooldown_does_not_hide_the_room_s_work() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["thin giant rat"]));
    bot.on_event(&Event::Line("*Combat Off*".into()));
    bot.on_event(&room(&["thin giant rat"]));
    assert!(bot.has_target(&view(&["thin giant rat"])));
}
