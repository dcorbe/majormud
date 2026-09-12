//! Bot policy tests: replay synthetic event sequences, assert the
//! commands the bot decides to send. The decision core is pure — no
//! sockets, no timing.

use mud_client::bot::{Bot, BotAction, BotConfig, picked_up};
use mud_client::events::{Actor, Event, RoomView, Status};

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
        also_here_sgr: Vec::new(),
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

/// The verb is the profile's: a swing for most, a punch or a kick for a
/// mystic, a spell for a caster who fights with magic. Sent as it is
/// with the noun after it, and the speech echo that says the target was
/// gone is read with the same verb.
#[test]
fn the_attack_verb_is_the_profiles() {
    let mut bot = Bot::new(BotConfig {
        auto_combat: true,
        attack_command: "cast lbol".into(),
        ..BotConfig::default()
    });
    assert_eq!(bot.on_event(&room(&["kobold thief"])), vec![BotAction::Send("cast lbol thief".into())]);
    assert_eq!(bot.engaged(), Some("kobold thief"));
    bot.on_event(&Event::Line("You say \"cast lbol thief\"".into()));
    assert_eq!(bot.engaged(), None, "the echo as speech means the target was gone");
    assert_eq!(BotConfig::default().attack_command, "a");
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
    let mut bot = Bot::new(BotConfig { auto_combat: false, ..BotConfig::default() });
    let actions = bot.on_event(&room(&["kobold thief"]));
    assert!(actions.is_empty());
}

/// Every switch a fresh profile carries is on: attack, heal, loot, flee,
/// rest and sneak. `take_keys` was already on and stays so.
#[test]
fn the_bot_switches_default_on() {
    let cfg = BotConfig::default();
    assert!(cfg.auto_combat, "auto_combat");
    assert!(cfg.auto_heal, "auto_heal");
    assert!(cfg.auto_get, "auto_get");
    assert!(cfg.auto_flee, "auto_flee");
    assert!(cfg.auto_rest, "auto_rest");
    assert!(cfg.auto_sneak, "auto_sneak");
}

/// `auto_rest` alone gates the rest and meditate sends. With it off the
/// character stands hurt forever, whatever `auto_heal` says about the
/// spells. Those are a different knob answering a different question.
#[test]
fn auto_rest_off_never_rests_while_auto_heal_stays_on() {
    let mut bot = Bot::new(BotConfig {
        auto_rest: false,
        auto_heal: true,
        rest_at_percent: 60,
        max_hp: 40,
        ..BotConfig::default()
    });
    bot.on_event(&room(&[]));
    let actions = bot.on_event(&Event::Prompt { hp: 12, mana: None, status: None });
    assert!(actions.is_empty(), "auto_rest off must send neither rest nor meditate");
}

/// The other side: `auto_heal` off does not touch resting at all.
#[test]
fn auto_heal_off_still_rests() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: false,
        auto_rest: true,
        rest_at_percent: 60,
        rest_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    bot.on_event(&room(&[]));
    let actions = bot.on_event(&Event::Prompt { hp: 12, mana: None, status: None });
    assert_eq!(actions, vec![BotAction::Send("rest".into())]);
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
        rest_at_percent: 50,
        rest_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    assert!(
        bot.on_event(&Event::Prompt { hp: 25, mana: None, status: None })
            .is_empty(),
        "62% hp: no heal"
    );
    let actions = bot.on_event(&Event::Prompt { hp: 19, mana: None, status: None });
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
        rest_at_percent: 50,
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
        bot.on_event(&Event::Prompt { hp: 21, mana: None, status: None }).is_empty(),
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
        bot.on_event(&Event::Prompt { hp: 21, mana: None, status: None }).is_empty(),
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
    assert!(bot.on_event(&Event::Prompt { hp: 21, mana: None, status: None }).is_empty());
    bot.on_event(&room(&[]));
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 21, mana: None, status: None }),
        vec![BotAction::Send("rest".into())]
    );
}

/// A walk-in makes resting wrong again, before any block re-lists it.
#[test]
fn a_walk_in_makes_resting_wrong_again() {
    let mut bot = healing_fighter();
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&Event::Prompt { hp: 21, mana: None, status: None }).len(), 1);
    // HP recovers past the threshold: the heal debounce releases.
    bot.on_event(&Event::Prompt { hp: 40, mana: None, status: None });
    // A rat walks in (and is engaged); dropping low again must not rest.
    bot.on_event(&Event::ActorEntered {
        name: "giant rat".into(),
        from: None,
    });
    assert!(
        bot.on_event(&Event::Prompt { hp: 21, mana: None, status: None }).is_empty(),
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
        rest_at_percent: 50,
        max_hp: 52,
        ignore: vec!["town guard".into()],
        ..BotConfig::default()
    });
    bot.on_event(&room(&["town guard"]));
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 21, mana: None, status: None }),
        vec![BotAction::Send("rest".into())]
    );
}

#[test]
fn heal_requires_known_max_hp() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        rest_at_percent: 50,
        max_hp: 0, // unknown: percent policies stay off
        ..BotConfig::default()
    });
    assert!(
        bot.on_event(&Event::Prompt { hp: 1, mana: None, status: None })
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
    let actions = bot.on_event(&Event::Prompt { hp: 8, mana: None, status: None });
    assert_eq!(actions, vec![BotAction::Send("north".into())]);
}

#[test]
fn flee_takes_priority_over_heal() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        rest_at_percent: 50,
        auto_flee: true,
        flee_at_percent: 25,
        max_hp: 40,
        ..BotConfig::default()
    });
    bot.on_event(&room(&[]));
    let actions = bot.on_event(&Event::Prompt { hp: 5, mana: None, status: None });
    assert_eq!(actions, vec![BotAction::Send("north".into())]);
}

#[test]
fn does_not_repeat_heal_while_still_hurt() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        rest_at_percent: 50,
        rest_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 19, mana: None, status: None }),
        vec![BotAction::Send("rest".into())]
    );
    // Prompts arrive in bursts — async output disturbs the dangling
    // prompt and the board re-prompts — and these are all still inside
    // the heal band. One "rest" covers them; re-sending on each prompt
    // trips flood control.
    assert!(
        bot.on_event(&Event::Prompt { hp: 18, mana: None, status: None })
            .is_empty()
    );
    assert!(
        bot.on_event(&Event::Prompt { hp: 17, mana: None, status: None })
            .is_empty()
    );
    // Back above the threshold, then hurt again: heal again.
    assert!(
        bot.on_event(&Event::Prompt { hp: 30, mana: None, status: None })
            .is_empty()
    );
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 15, mana: None, status: None }),
        vec![BotAction::Send("rest".into())]
    );
}

#[test]
fn heals_when_hurt_but_no_exit_is_known() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        rest_at_percent: 50,
        auto_flee: true,
        flee_at_percent: 25,
        rest_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    // No room block seen yet, so there is no exit to flee through;
    // the heal policy still applies.
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 5, mana: None, status: None }),
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

/// The board's acknowledgement of a `get`, verbatim from the oracle
/// corpus ("You picked up 11 silver nobles", oracle_bank.raw, no
/// trailing period). This is the ONLY positive proof a pile left the
/// floor — `get` is otherwise fire-and-forget, and the client has never
/// been able to tell a successful sweep from an encumbrance refusal.
#[test]
fn reads_the_pickup_acknowledgement() {
    assert_eq!(
        picked_up("You picked up 11 silver nobles"),
        Some((11, "silver".to_string()))
    );
}

/// A single coin drops the plural. The count is what `Here` reconciles
/// against, so the singular form must not read as "no pile taken".
#[test]
fn reads_a_singular_pickup() {
    assert_eq!(
        picked_up("You picked up 1 silver noble"),
        Some((1, "silver".to_string()))
    );
}

/// Items wear denomination words ("silver holy amulet", live in
/// oracle_charm_lifecycle) and the board announces taking them the same
/// way. Only the five minted denominations name a PILE; anything else
/// would retire a coin pile that is still sitting on the floor.
#[test]
fn does_not_read_an_item_pickup_as_coins() {
    assert_eq!(picked_up("You picked up a silver holy amulet"), None);
}

/// The drop line and the pickup line are opposite facts about the same
/// pile. Confusing them would have `Here` delete a pile at the moment it
/// appears.
#[test]
fn does_not_mistake_a_coin_drop_for_a_pickup() {
    assert_eq!(picked_up("12 silver drop to the ground."), None);
}

/// A single coin takes the singular verb — "1 silver drops to the
/// ground." — and the plural-only pattern read it as no drop at all.
/// Seven live drops missed across one Arena session (cwrun2.raw), which
/// is the whole of the `pile-missing` count the reconciler reported.
/// The LEADING COUNT is what tells loot from a downed actor; the verb
/// never was.
#[test]
fn grabs_a_single_dropped_coin() {
    let mut bot = Bot::new(BotConfig {
        auto_get: true,
        ..BotConfig::default()
    });
    let actions = bot.on_event(&Event::Line("1 silver drops to the ground.".into()));
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
    let mut bot = Bot::new(BotConfig { auto_get: false, ..BotConfig::default() });
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
        bot.on_event(&Event::Prompt { hp: 8, mana: None, status: None }),
        vec![BotAction::Send("north".into())]
    );
}

#[test]
fn flees_once_per_room_not_once_per_prompt() {
    let mut bot = Bot::new(BotConfig {
        auto_flee: true,
        flee_at_percent: 25,
        auto_rest: false,
        max_hp: 40,
        ..BotConfig::default()
    });
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&Event::Prompt { hp: 8, mana: None, status: None }).len(), 1);
    // Prompts arrive in bursts and can even double up on one physical
    // line; flooding movement while dying is the worst case.
    assert!(
        bot.on_event(&Event::Prompt { hp: 7, mana: None, status: None })
            .is_empty()
    );
    assert!(
        bot.on_event(&Event::Prompt { hp: 6, mana: None, status: None })
            .is_empty()
    );
    // Arriving somewhere new re-arms it: still hurt, so keep running.
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&Event::Prompt { hp: 6, mana: None, status: None }).len(), 1);
}

#[test]
fn rearm_releases_a_heal_that_never_landed() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        rest_at_percent: 50,
        rest_command: "rest".into(),
        max_hp: 40,
        ..BotConfig::default()
    });
    assert_eq!(bot.on_event(&Event::Prompt { hp: 19, mana: None, status: None }).len(), 1);
    assert!(
        bot.on_event(&Event::Prompt { hp: 19, mana: None, status: None })
            .is_empty()
    );
    // A heal that never lands leaves HP low forever, so the debounce
    // would latch and the character dies quietly. The runner re-arms on
    // evidence of refusal.
    bot.rearm();
    assert_eq!(
        bot.on_event(&Event::Prompt { hp: 19, mana: None, status: None }),
        vec![BotAction::Send("rest".into())]
    );
}

#[test]
fn stays_quiet_while_downed() {
    let mut bot = Bot::new(BotConfig {
        auto_heal: true,
        rest_at_percent: 50,
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
            mana: None, status: None
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
        bot.on_event(&Event::Prompt { hp: 30, mana: None, status: None });
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
        bot.on_event(&Event::Prompt { hp: 30, mana: None, status: None });
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
        bot.on_event(&Event::Prompt { hp: 30, mana: None, status: None });
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
            bot.on_event(&Event::Prompt { hp: 30, mana: None, status: None });
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
        !Bot::new(BotConfig { auto_combat: false, ..BotConfig::default() })
            .has_target(&view(&["kobold thief"])),
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

// ---------------------------------------------------------------------
// Floor cash. The room render's "You notice ... here." line is the only
// announcement money already on the ground ever gets — the drop line
// exists only for a kill the bot just watched. Real board wordings
// (oracle captures): "You notice 11 silver nobles, 49 copper farthings
// here.", "You notice 24 platinum pieces, 43 gold crowns, silver holy
// amulet here." — note the amulet: an ITEM wearing a denomination word.
// ---------------------------------------------------------------------

fn view_with_items(also_here: &[&str], items: &[&str]) -> RoomView {
    RoomView {
        items: items.iter().map(|s| s.to_string()).collect(),
        ..view(also_here)
    }
}

fn get_bot() -> Bot {
    Bot::new(BotConfig {
        auto_combat: true,
        auto_get: true,
        ..BotConfig::default()
    })
}

#[test]
fn sweeps_floor_coins_listed_by_the_room() {
    let mut bot = get_bot();
    let actions = bot.on_event(&Event::RoomSeen(view_with_items(
        &[],
        &["11 silver nobles", "49 copper farthings"],
    )));
    assert_eq!(
        actions,
        vec![
            BotAction::Send("get silver".into()),
            BotAction::Send("get copper".into()),
        ]
    );
}

/// Coins only. An item that merely starts with a denomination word
/// ("silver holy amulet") has no count; an item is not ours to sweep —
/// on somebody else's board that is somebody else's dropped gear.
#[test]
fn an_item_wearing_a_coin_name_is_not_cash() {
    let mut bot = get_bot();
    let actions = bot.on_event(&Event::RoomSeen(view_with_items(
        &[],
        &["silver holy amulet", "wooden hammer"],
    )));
    assert!(actions.is_empty(), "{actions:?}");
}

/// Sweep first, THEN engage — the same priority `StopState::verdict`
/// uses, so the assist and a `/farm` run behave identically.
///
/// This used to assert the opposite (fight first, sweep the post-kill
/// block). On a shared board that loses every contested pile: measured
/// live 2026-08-02 (cwrun6.raw), a block announcing 15 copper was
/// answered `a rat`, then `look`, and only then `get copper` — by which
/// time another player had taken it. Three uncontested piles in the
/// same session were swept immediately and kept.
#[test]
fn sweeps_before_engaging() {
    let mut bot = get_bot();
    let actions = bot.on_event(&Event::RoomSeen(view_with_items(
        &["giant rat"],
        &["2968 copper farthings"],
    )));
    assert_eq!(
        actions,
        vec![
            BotAction::Send("get copper".into()),
            BotAction::Send("a rat".into()),
        ],
        "the coins go out first, and the fight still starts"
    );
}

/// The floor never interrupts a swing already traded: `engaged` gates
/// the sweep, exactly as it gates `Verdict::Busy` in the runner.
#[test]
fn an_ongoing_fight_is_not_interrupted_to_loot() {
    let mut bot = get_bot();
    // First block engages the rat.
    bot.on_event(&Event::RoomSeen(view_with_items(&["giant rat"], &[])));
    assert_eq!(bot.engaged(), Some("giant rat"));
    // A pile appears mid-fight; the bot keeps fighting and ignores it.
    let actions = bot.on_event(&Event::RoomSeen(view_with_items(
        &["giant rat"],
        &["2968 copper farthings"],
    )));
    assert!(
        actions.is_empty(),
        "money must not interrupt a fight in progress: {actions:?}"
    );
}

#[test]
fn auto_get_off_leaves_the_floor_alone() {
    let mut bot = Bot::new(BotConfig {
        auto_combat: true,
        auto_get: false,
        ignore: vec!["town guard".into()],
        ..BotConfig::default()
    });
    let actions = bot.on_event(&Event::RoomSeen(view_with_items(
        &[],
        &["2000 gold crowns"],
    )));
    assert!(actions.is_empty(), "{actions:?}");
}

/// One sweep per denomination per visit. A pile the character cannot
/// carry (encumbrance refusal) stays listed in every subsequent block,
/// and a bot that re-swept per block would `get` at the pacer floor
/// forever. Leaving and coming back is a new visit and a new try; a
/// fresh pile mid-stay is the drop line's job, which already pays once.
#[test]
fn sweeps_a_pile_once_per_visit() {
    let mut bot = get_bot();
    let heavy = || {
        Event::RoomSeen(RoomView {
            name: "Vault".into(),
            items: vec!["2000 gold crowns".into()],
            ..RoomView::default()
        })
    };
    assert_eq!(
        bot.on_event(&heavy()),
        vec![BotAction::Send("get gold".into())]
    );
    assert!(bot.on_event(&heavy()).is_empty(), "re-swept a standing pile");
    // Somewhere else and back: the visit ended, try again.
    bot.on_event(&Event::RoomSeen(RoomView {
        name: "Corridor".into(),
        ..RoomView::default()
    }));
    assert_eq!(
        bot.on_event(&heavy()),
        vec![BotAction::Send("get gold".into())]
    );
}

/// A kill's drop line and the room's own "You notice ..." listing name
/// the SAME pile. Before this, the drop line had no memo consultation at
/// all — a kill sent `get gold`, then the very next block re-listed the
/// pile and the room-render sweep sent `get gold` again. One `get` per
/// denomination per visit, regardless of which of the two paths sees the
/// pile first.
#[test]
fn a_kill_drop_and_the_room_listing_claim_the_same_pile_once() {
    let mut bot = get_bot();
    // The visit starts and the room is keyed before the kill.
    bot.on_event(&Event::RoomSeen(view_with_items(&["giant rat"], &[])));
    // The kill's drop line claims "gold" first.
    let drop_actions = bot.on_event(&Event::Line("40 gold drops to the ground.".into()));
    assert_eq!(drop_actions, vec![BotAction::Send("get gold".into())]);
    // The room re-renders, still listing the same pile the kill just
    // reported dropping. The room-render path must find "gold" already
    // claimed and send nothing.
    let render_actions = bot.on_event(&Event::RoomSeen(view_with_items(
        &[],
        &["40 gold crowns"],
    )));
    assert!(
        render_actions.is_empty(),
        "denomination already claimed by the drop line: {render_actions:?}"
    );
}

/// Stage 2 needs "is there work here" answered against the MAINTAINED
/// occupant list rather than whatever the last room block happened to
/// say, so the question has to be askable of any names at all.
#[test]
fn work_can_be_judged_from_maintained_names_not_just_a_block() {
    let bot = combat_bot();
    assert!(bot.has_target_among(["cave bear"].into_iter()));
    // Players are company, not work.
    assert!(!bot.has_target_among(["Vexil"].into_iter()));
    // The ignore list still applies, whoever is asking.
    assert!(!bot.has_target_among(["town guard"].into_iter()));
    assert!(!bot.has_target_among(std::iter::empty()));
}

/// The block-shaped question must be exactly the name-shaped question,
/// or the two ways of asking could disagree mid-migration.
#[test]
fn the_block_form_and_the_name_form_agree() {
    let bot = combat_bot();
    let r = view(&["Vexil", "cave bear"]);
    assert_eq!(
        bot.has_target(&r),
        bot.has_target_among(r.also_here.iter().map(String::as_str))
    );
}

// --- the board's own aggression marker ---------------------------------
//
// Ranking by `exp * 1000 + hp` picks the fattest name in the room, and
// in a town that is a passive `drunken brawler`: 250 exp, 110 hp, more
// than twice a cave bear, and it would never have touched the character.
// It killed the live character on 2026-08-02. The board says which is
// which by how it paints the name; the bot now reads that.

fn painted(entries: &[(&str, &str)]) -> RoomView {
    RoomView {
        name: "Newhaven, Village Center".into(),
        exits: vec!["north".into()],
        also_here: entries.iter().map(|(_, n)| n.to_string()).collect(),
        also_here_sgr: entries
            .iter()
            .map(|(c, _)| (!c.is_empty()).then(|| c.to_string()))
            .collect(),
        items: vec![],
    }
}

fn fighter() -> Bot {
    Bot::new(BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    })
}

#[test]
fn a_passive_mob_is_never_attacked_when_the_board_painted_it() {
    let mut bot = fighter();
    let acts = bot.on_event(&Event::RoomSeen(painted(&[("0;36", "big drunken brawler")])));
    assert!(acts.is_empty(), "cyan is passive: {acts:?}");
}

#[test]
fn a_guard_in_white_is_never_attacked_even_without_an_ignore_entry() {
    let mut bot = fighter();
    let acts = bot.on_event(&Event::RoomSeen(painted(&[("0;37", "fierce guardsman")])));
    assert!(acts.is_empty(), "white is law: {acts:?}");
}

/// The one that matters: the brawler outranks the rat on the threat
/// table, so a colour-blind bot picks the brawler.
#[test]
fn the_aggressive_one_is_chosen_over_a_fatter_passive_one() {
    let mut bot = fighter();
    let acts = bot.on_event(&Event::RoomSeen(painted(&[
        ("0;36", "big drunken brawler"),
        ("1;35", "giant rat"),
    ])));
    assert_eq!(acts, vec![BotAction::Send("a rat".into())], "{acts:?}");
}

/// Colour does not replace the case rule — players are magenta too.
#[test]
fn a_player_in_magenta_is_still_not_a_target() {
    let mut bot = fighter();
    let acts = bot.on_event(&Event::RoomSeen(painted(&[("1;35", "Habuji")])));
    assert!(acts.is_empty(), "players are capitalised: {acts:?}");
}

/// Self-calibrating: an unpainted block has no opinion, so the old rule
/// decides and every board that does not paint keeps working.
#[test]
fn an_unpainted_block_falls_back_to_the_case_rule() {
    let mut bot = fighter();
    let acts = bot.on_event(&Event::RoomSeen(painted(&[("", "giant rat")])));
    assert_eq!(acts, vec![BotAction::Send("a rat".into())], "{acts:?}");
}

/// A passive occupant is not "work", so it must not hold a stop open or
/// suppress a rest — that would strand the runner beside a townsman.
#[test]
fn a_passive_mob_is_not_work() {
    let mut bot = fighter();
    bot.on_event(&Event::RoomSeen(painted(&[("0;36", "big drunken brawler")])));
    assert!(!bot.has_target(&painted(&[("0;36", "big drunken brawler")])));
    assert!(bot.has_target(&painted(&[("1;35", "giant rat")])));
}

// --- the recovery ladders -------------------------------------------

#[test]
fn the_defaults_are_mudplays() {
    let c = BotConfig::default();
    assert_eq!(c.rest_at_percent, 60);
    assert_eq!(c.mana_rest_at_percent, 30);
    assert_eq!(c.rest_until_percent, 95);
    assert_eq!(c.minor_heal_at_percent, 70);
    assert_eq!(c.major_heal_at_percent, 40);
    assert_eq!(c.flee_at_percent, 20);
    assert!(!c.meditate);
    assert!(c.validate().is_ok());
}

#[test]
fn the_heal_ladder_refuses_a_major_mark_above_the_minor() {
    let c = BotConfig { minor_heal_at_percent: 40, major_heal_at_percent: 70, ..BotConfig::default() };
    let err = c.validate().expect_err("upside down");
    assert!(err.contains("major_heal_at_percent"), "{err}");
}

#[test]
fn the_rest_ladder_refuses_a_rest_mark_above_the_until_mark() {
    let c = BotConfig { rest_at_percent: 96, rest_until_percent: 95, ..BotConfig::default() };
    let err = c.validate().expect_err("upside down");
    assert!(err.contains("rest_until_percent"), "{err}");
    let mana = BotConfig { mana_rest_at_percent: 96, ..BotConfig::default() };
    assert!(mana.validate().is_err());
}

#[test]
fn a_flee_mark_above_a_heal_mark_is_refused_and_zero_marks_are_skipped() {
    let upside_down = BotConfig { flee_at_percent: 50, major_heal_at_percent: 40, ..BotConfig::default() };
    assert!(upside_down.validate().is_err());
    let off = BotConfig { minor_heal_at_percent: 0, major_heal_at_percent: 0, ..BotConfig::default() };
    assert!(off.validate().is_ok());
}

#[test]
fn the_old_heal_list_folds_into_the_two_names() {
    let mut c = BotConfig {
        heal_spells: vec!["minor healing".into(), "major healing".into()],
        ..BotConfig::default()
    };
    c.normalise();
    assert_eq!(c.minor_heal_spell, "minor healing");
    assert_eq!(c.major_heal_spell, "major healing");
    let mut one = BotConfig { heal_spells: vec!["mend".into()], ..BotConfig::default() };
    one.normalise();
    assert_eq!(one.minor_heal_spell, "mend");
    assert_eq!(one.major_heal_spell, "");
    // Names already set win over the list.
    let mut named = BotConfig {
        heal_spells: vec!["mend".into()],
        minor_heal_spell: "minor healing".into(),
        ..BotConfig::default()
    };
    named.normalise();
    assert_eq!(named.minor_heal_spell, "minor healing");
}

#[test]
fn mana_percent_needs_a_pool() {
    let none = BotConfig::default();
    assert_eq!(none.mana_percent(Some(5)), None);
    let pool = BotConfig { max_mana: 20, ..BotConfig::default() };
    assert_eq!(pool.mana_percent(Some(5)), Some(25));
    assert_eq!(pool.mana_percent(None), None);
}

/// The lobby's rest log prints the pools the marks were compared against.
/// Mana only appears when the character has a pool and the prompt carried
/// a reading, the same two conditions under which `mana_percent` answers.
#[test]
fn pools_print_what_the_marks_read() {
    let hp_only = BotConfig { max_hp: 52, ..BotConfig::default() };
    assert_eq!(hp_only.pools(12, None), "hp 12/52 23%");
    assert_eq!(hp_only.pools(12, Some(5)), "hp 12/52 23%", "a reading without a pool");
    let caster = BotConfig { max_hp: 52, max_mana: 20, ..BotConfig::default() };
    assert_eq!(caster.pools(12, Some(5)), "hp 12/52 23%, mana 5/20 25%");
    assert_eq!(caster.pools(12, None), "hp 12/52 23%", "a pool without a reading");
    let unknown = BotConfig::default();
    assert_eq!(unknown.pools(12, None), "hp 12/0", "no percent without a max");
}

/// A rest the bot decided on is explained by the pools and the two marks
/// it read, whichever command carried it.
#[test]
fn rest_reason_names_the_command_the_pools_and_the_marks() {
    let cfg = BotConfig { max_hp: 52, max_mana: 20, rest_at_percent: 40, mana_rest_at_percent: 30, ..BotConfig::default() };
    assert!(cfg.is_rest("rest"));
    assert!(cfg.is_rest("meditate"));
    assert!(!cfg.is_rest("look"));
    assert_eq!(
        cfg.rest_reason("meditate", 50, Some(5)),
        "meditate, hp 50/52 96%, mana 5/20 25%, rest_at 40%, mana_rest_at 30%"
    );
}

/// `hp_percent` is the number the marks are compared against, exposed so
/// the runner's cast dispatch cannot drift from `on_vitals`'s arithmetic. It
/// refuses to answer in exactly the two cases `on_vitals` refuses to decide:
/// max unknown, and downed (HP reads negative).
#[test]
fn hp_percent_answers_only_when_the_marks_could() {
    let known = Bot::new(BotConfig {
        max_hp: 50,
        ..BotConfig::default()
    });
    assert_eq!(known.hp_percent(25), Some(50));
    assert_eq!(known.hp_percent(-3), None, "downed");

    let unknown = Bot::new(BotConfig::default());
    assert_eq!(unknown.hp_percent(25), None, "max_hp unknown");
}

// --- rest, meditate, and the end of a recovery ------------------------

fn resting_bot(max_mana: i32, meditate: bool) -> Bot {
    Bot::new(BotConfig {
        auto_heal: true,
        max_hp: 100,
        max_mana,
        meditate,
        ..BotConfig::default()
    })
}

fn vitals(hp: i32, mana: Option<i32>, status: Option<Status>) -> Event {
    Event::Prompt { hp, mana, status }
}

#[test]
fn rests_below_the_rest_mark_and_not_again_while_resting() {
    let mut bot = resting_bot(0, false);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&vitals(50, None, None)), vec![BotAction::Send("rest".into())]);
    // The board shows the rest. No second send.
    assert!(bot.on_event(&vitals(50, None, Some(Status::Resting))).is_empty());
    assert!(bot.on_event(&vitals(70, None, Some(Status::Resting))).is_empty());
}

#[test]
fn a_rest_is_over_at_the_until_mark_and_nothing_is_sent_to_end_it() {
    let mut bot = resting_bot(0, false);
    bot.on_event(&room(&[]));
    bot.on_event(&vitals(50, None, None));
    assert!(bot.on_event(&vitals(94, None, Some(Status::Resting))).is_empty());
    // At the mark the recovery is over. Without stealth that sends
    // nothing. The character stands on its next action.
    assert!(bot.on_event(&vitals(95, None, Some(Status::Resting))).is_empty());
    // Standing again below the rest mark rests again.
    assert_eq!(bot.on_event(&vitals(50, None, None)), vec![BotAction::Send("rest".into())]);
}

#[test]
fn a_rest_waits_for_mana_too_when_there_is_a_pool() {
    let mut bot = resting_bot(20, false);
    bot.on_event(&room(&[]));
    bot.on_event(&vitals(50, Some(5), None));
    // HP is over the mark, mana is not: still resting.
    assert!(bot.on_event(&vitals(96, Some(10), Some(Status::Resting))).is_empty());
    // A meditation only needs mana.
    let mut m = resting_bot(20, true);
    m.on_event(&room(&[]));
    assert_eq!(m.on_event(&vitals(80, Some(2), None)), vec![BotAction::Send("meditate".into())]);
    assert!(m.on_event(&vitals(80, Some(19), Some(Status::Meditating))).is_empty());
}

#[test]
fn low_mana_alone_rests_or_meditates_by_the_switch() {
    let mut rests = resting_bot(20, false);
    rests.on_event(&room(&[]));
    assert_eq!(rests.on_event(&vitals(80, Some(2), None)), vec![BotAction::Send("rest".into())]);
    let mut meditates = resting_bot(20, true);
    meditates.on_event(&room(&[]));
    assert_eq!(meditates.on_event(&vitals(80, Some(2), None)), vec![BotAction::Send("meditate".into())]);
    // Both pools low: rest, which restores both.
    let mut both = resting_bot(20, true);
    both.on_event(&room(&[]));
    assert_eq!(both.on_event(&vitals(50, Some(2), None)), vec![BotAction::Send("rest".into())]);
}

#[test]
fn a_recovery_is_never_sent_into_a_room_with_work() {
    // room_has_work is judged from what this bot would attack, same as
    // does_not_rest_while_the_room_lists_a_monster above: auto_combat
    // has to be on for a listed monster to count as work at all.
    let mut bot = Bot::new(BotConfig {
        auto_combat: true,
        auto_heal: true,
        max_hp: 100,
        max_mana: 20,
        meditate: true,
        ..BotConfig::default()
    });
    bot.on_event(&room(&["kobold thief"]));
    assert!(bot.on_event(&vitals(50, Some(2), None)).is_empty());
}

#[test]
fn the_rest_latch_clears_when_the_board_shows_the_rest_landed() {
    // A rest that never lands keeps the latch. One that lands and is
    // then broken by a blow re-arms on the next standing prompt.
    let mut bot = resting_bot(0, false);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&vitals(50, None, None)), vec![BotAction::Send("rest".into())]);
    assert!(bot.on_event(&vitals(50, None, None)).is_empty());
    assert!(bot.on_event(&vitals(51, None, Some(Status::Resting))).is_empty());
    assert_eq!(bot.on_event(&vitals(48, None, None)), vec![BotAction::Send("rest".into())]);
}

// --- stealth when idle ---------------------------------------------------
//
// A character with Stealth on the sheet keeps it up whenever the assist
// finds it standing idle: `hide` under `auto_hide`, otherwise a `sneak`
// so the next move, its own or a party leader's, goes unseen. Both are
// beliefs read off the board's echo and forgotten on any other send.

fn stealth_bot(auto_hide: bool) -> Bot {
    Bot::new(BotConfig { auto_heal: true, auto_hide, max_hp: 100, ..BotConfig::default() }).with_stealth(true)
}

fn line(l: &str) -> Event {
    Event::Line(l.into())
}

fn standing(hp: i32) -> Event {
    vitals(hp, None, None)
}

fn send(cmd: &str) -> Vec<BotAction> {
    vec![BotAction::Send(cmd.into())]
}

/// Hiding is the operator's choice and a policy `/bot` off must stop.
#[test]
fn auto_hide_is_off_by_default_and_off_under_bot_off() {
    assert!(!BotConfig::default().auto_hide);
    let cfg = BotConfig { auto_hide: true, ..BotConfig::default() };
    assert!(!cfg.switched_off().auto_hide);
}

#[test]
fn an_idle_prompt_hides_with_auto_hide_and_sneaks_without() {
    let mut bot = stealth_bot(true);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&standing(100)), send("hide"));
    let mut bot = stealth_bot(false);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
}

#[test]
fn a_finished_rest_hides_once_and_believes_the_attempt() {
    let mut bot = stealth_bot(true);
    bot.on_event(&room(&[]));
    bot.on_event(&standing(50));
    assert_eq!(bot.on_event(&vitals(95, None, Some(Status::Resting))), send("hide"));
    // The echo's prompt still says resting. No second hide.
    assert!(bot.on_event(&vitals(95, None, Some(Status::Resting))).is_empty());
    assert!(bot.on_event(&line("Attempting to hide...")).is_empty());
    assert!(bot.hidden());
    assert!(bot.on_event(&standing(95)).is_empty());
}

#[test]
fn a_finished_rest_sneaks_without_auto_hide() {
    let mut bot = stealth_bot(false);
    bot.on_event(&room(&[]));
    bot.on_event(&standing(50));
    assert_eq!(bot.on_event(&vitals(95, None, Some(Status::Resting))), send("sneak"));
    assert!(bot.on_event(&vitals(95, None, Some(Status::Resting))).is_empty());
    assert!(bot.on_event(&line("Attempting to sneak...")).is_empty());
    assert!(bot.sneaking());
    assert!(bot.on_event(&standing(95)).is_empty());
}

/// The board names a failed hide when a perception roll passes. The
/// retry waits out the same three prompts a silence does, since every
/// failure prints a prompt and retrying on it sent three in a second,
/// and gives up after three sends until something changes.
#[test]
fn a_noticed_failure_retries_after_a_wait_three_times_then_stops() {
    let mut bot = stealth_bot(true);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&standing(100)), send("hide"));
    for _ in 0..2 {
        assert!(bot.on_event(&line("Attempting to hide...")).is_empty());
        assert!(bot.on_event(&line(" You don't think you are hidden.")).is_empty());
        assert!(!bot.hidden());
        assert!(bot.on_event(&standing(100)).is_empty(), "not on the failure's own prompt");
        assert!(bot.on_event(&standing(100)).is_empty());
        assert_eq!(bot.on_event(&standing(100)), send("hide"));
    }
    bot.on_event(&line("Attempting to hide..."));
    bot.on_event(&line(" You don't think you are hidden."));
    for _ in 0..6 {
        assert!(bot.on_event(&standing(100)).is_empty());
    }
    // A new room is a new situation. This one, printed again, is not.
    bot.on_event(&room(&[]));
    assert!(bot.on_event(&standing(100)).is_empty());
    bot.on_event(&Event::RoomSeen(RoomView { name: "Arena, Sand".into(), ..view(&[]) }));
    assert_eq!(bot.on_event(&standing(100)), send("hide"));
}

/// Prompts arrive in bursts. One sneak per burst, and a sneak the board
/// swallowed without a word (sent too fast behind another command) is
/// tried again after three quiet prompts rather than never.
#[test]
fn a_burst_of_prompts_sends_one_sneak_and_a_silent_one_is_retried() {
    let mut bot = stealth_bot(false);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
    assert!(bot.on_event(&standing(100)).is_empty());
    assert!(bot.on_event(&standing(100)).is_empty());
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
    assert!(bot.on_event(&standing(100)).is_empty());
    assert!(bot.on_event(&standing(100)).is_empty());
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
    for _ in 0..6 {
        assert!(bot.on_event(&standing(100)).is_empty(), "three tries, then silence");
    }
}

/// The live board glues a seen failure onto the attempt line.
#[test]
fn the_sneak_echo_is_believed_and_a_seen_failure_is_not() {
    let mut bot = stealth_bot(false);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
    assert!(bot.on_event(&line("Attempting to sneak...You don't think you're sneaking.")).is_empty());
    assert!(!bot.sneaking());
    assert!(bot.on_event(&standing(100)).is_empty());
    assert!(bot.on_event(&standing(100)).is_empty());
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
    bot.on_event(&line("Attempting to sneak..."));
    assert!(bot.sneaking());
    assert!(bot.on_event(&standing(100)).is_empty());
}

/// "You may not sneak right now!" is not a failed roll, it is the board
/// saying no, and asking again in the same situation is what sent
/// twenty sneaks into the Bank of Godfrey in four seconds (live,
/// 2026-09-08). Nothing more until the situation changes: a new room,
/// or a send of the bot's own.
#[test]
fn a_hard_block_on_sneak_ends_the_tries_until_the_situation_changes() {
    let mut bot = Bot::new(BotConfig { auto_combat: true, auto_heal: true, max_hp: 100, ..BotConfig::default() })
        .with_stealth(true);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
    assert!(bot.on_event(&line("You may not sneak right now!")).is_empty());
    assert!(!bot.sneaking());
    for _ in 0..6 {
        assert!(bot.on_event(&standing(100)).is_empty());
    }
    // The same room printed again is the same situation.
    bot.on_event(&room(&[]));
    assert!(bot.on_event(&standing(100)).is_empty());
    // A fight is a new one: the attack is a send of the bot's own.
    assert_eq!(bot.on_event(&room(&["kobold thief"])), send("a thief"));
    bot.on_event(&line("*Combat Off*"));
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
}

#[test]
fn any_other_send_forgets_both_beliefs() {
    let mut bot = Bot::new(BotConfig { auto_combat: true, auto_heal: true, max_hp: 100, ..BotConfig::default() })
        .with_stealth(true);
    bot.on_event(&room(&[]));
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
    bot.on_event(&line("Attempting to sneak..."));
    assert!(bot.sneaking());
    assert_eq!(bot.on_event(&room(&["kobold thief"])), send("a thief"));
    assert!(!bot.sneaking());
    assert!(bot.on_event(&standing(100)).is_empty(), "no sneak mid-fight");
    let mut bot = Bot::new(BotConfig { auto_combat: true, auto_heal: true, auto_hide: true, max_hp: 100, ..BotConfig::default() })
        .with_stealth(true);
    bot.on_event(&room(&[]));
    bot.on_event(&standing(100));
    bot.on_event(&line("Attempting to hide..."));
    assert!(bot.hidden());
    assert_eq!(bot.on_event(&room(&["kobold thief"])), send("a thief"));
    assert!(!bot.hidden());
}

/// Once the fight is over the assist's own look answers with a clear
/// room, and the next prompt re-arms.
#[test]
fn the_sneak_is_re_armed_after_a_fight() {
    let mut bot = Bot::new(BotConfig { auto_combat: true, auto_heal: true, max_hp: 100, ..BotConfig::default() })
        .with_stealth(true);
    bot.on_event(&room(&["kobold thief"]));
    bot.on_event(&line("*Combat Off*"));
    assert!(bot.on_event(&room(&[])).is_empty());
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
}

/// A move made while sneaking opens with "Sneaking..." and nothing else
/// does. A block without it is a move that was not sneaked, or a look,
/// and either way the belief is not worth keeping: a look after a fight
/// already follows a send that forgot it, and a silent move means the
/// sneak did not hold.
#[test]
fn a_room_block_keeps_the_sneak_only_behind_the_sneaking_line() {
    let mut bot = stealth_bot(false);
    bot.on_event(&room(&[]));
    bot.on_event(&standing(100));
    bot.on_event(&line("Attempting to sneak..."));
    assert!(bot.sneaking());
    bot.on_event(&line("Sneaking..."));
    bot.on_event(&room(&[]));
    assert!(bot.sneaking());
    assert!(bot.on_event(&standing(100)).is_empty());
    bot.on_event(&room(&[]));
    assert!(!bot.sneaking());
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
}

#[test]
fn a_sound_on_entry_breaks_the_sneak() {
    let mut bot = stealth_bot(false);
    bot.on_event(&room(&[]));
    bot.on_event(&standing(100));
    bot.on_event(&line("Attempting to sneak..."));
    assert!(bot.on_event(&line("You make a sound as you enter the room!")).is_empty());
    assert!(!bot.sneaking());
}

/// Moving un-hides. A block is a move or a look, and a look only ever
/// follows a send that already forgot the belief.
#[test]
fn a_room_block_forgets_the_hide() {
    let mut bot = stealth_bot(true);
    bot.on_event(&room(&[]));
    bot.on_event(&standing(100));
    bot.on_event(&line("Attempting to hide..."));
    assert!(bot.hidden());
    bot.on_event(&room(&[]));
    assert!(!bot.hidden());
    assert_eq!(bot.on_event(&standing(100)), send("hide"));
}

/// The assist is built before the realm entry probe reads the stat
/// sheet, so Stealth reads 0 at the build and the switch has to be
/// settable afterwards. Without this the assist never hid all session.
#[test]
fn stealth_learned_after_the_build_still_hides() {
    let mut bot = Bot::new(BotConfig { auto_heal: true, auto_hide: true, max_hp: 100, ..BotConfig::default() });
    bot.on_event(&room(&[]));
    assert!(bot.on_event(&standing(100)).is_empty());
    bot.set_stealth(true);
    assert_eq!(bot.on_event(&standing(100)), send("hide"));
}

#[test]
fn without_stealth_or_without_a_switch_idle_sends_nothing() {
    let mut bot = resting_bot(0, false);
    bot.on_event(&room(&[]));
    assert!(bot.on_event(&standing(100)).is_empty());
    bot.on_event(&standing(50));
    assert!(bot.on_event(&vitals(95, None, Some(Status::Resting))).is_empty());
    let mut bot = Bot::new(BotConfig { auto_heal: true, auto_sneak: false, max_hp: 100, ..BotConfig::default() })
        .with_stealth(true);
    bot.on_event(&room(&[]));
    assert!(bot.on_event(&standing(100)).is_empty());
    bot.on_event(&standing(50));
    assert!(bot.on_event(&vitals(95, None, Some(Status::Resting))).is_empty());
}

/// A room with something to fight is not idle, whether the fight has
/// started or not. An ignored monster is not work.
#[test]
fn a_room_with_work_does_not_sneak() {
    let mut bot = Bot::new(BotConfig {
        auto_combat: true,
        auto_heal: true,
        max_hp: 100,
        ignore: vec!["town guard".into()],
        ..BotConfig::default()
    })
    .with_stealth(true);
    assert_eq!(bot.on_event(&room(&["kobold thief"])), send("a thief"));
    assert!(bot.on_event(&standing(100)).is_empty());
    bot.on_event(&line("*Combat Off*"));
    bot.on_event(&room(&["town guard"]));
    assert_eq!(bot.on_event(&standing(100)), send("sneak"));
}

#[test]
fn a_rest_with_a_low_pool_does_not_hide_yet() {
    let mut bot = Bot::new(BotConfig { auto_heal: true, auto_hide: true, max_hp: 100, max_mana: 20, ..BotConfig::default() })
        .with_stealth(true);
    bot.on_event(&room(&[]));
    bot.on_event(&vitals(50, Some(5), None));
    // HP is over the mark, mana is not: still resting, so no hide yet.
    assert!(bot.on_event(&vitals(96, Some(10), Some(Status::Resting))).is_empty());
}

/// A room the character sneaked into is opened with a backstab. The
/// move's own "Sneaking..." is the evidence, the same line a walk's
/// arrival reads, and a party leader's move prints it for the follower
/// too. Under `auto_hide` the fight opens plainly: hidden in place is
/// what was asked for, not a backstab on entry.
#[test]
fn a_sneaked_arrival_opens_with_a_backstab_unless_auto_hide() {
    let mut bot = Bot::new(BotConfig { auto_combat: true, auto_heal: true, max_hp: 100, ..BotConfig::default() })
        .with_stealth(true);
    bot.on_event(&line("Sneaking..."));
    assert_eq!(bot.on_event(&room(&["kobold thief"])), send("bs thief"));
    bot.on_event(&line("*Combat Off*"));
    assert_eq!(bot.on_event(&room(&["giant rat"])), send("a rat"), "no sneak on this move");
    let mut bot = Bot::new(BotConfig { auto_combat: true, auto_heal: true, auto_hide: true, max_hp: 100, ..BotConfig::default() })
        .with_stealth(true);
    bot.on_event(&line("Sneaking..."));
    assert_eq!(bot.on_event(&room(&["kobold thief"])), send("a thief"));
}

// --- which heal ----------------------------------------------------------

#[test]
fn the_heal_need_follows_the_bands() {
    use mud_client::bot::heal_need;
    use mud_client::sheet::HealNeed;
    let plain = BotConfig::default();
    assert_eq!(heal_need(&plain, 80), None);
    assert_eq!(heal_need(&plain, 69), Some(HealNeed::Minor));
    assert_eq!(heal_need(&plain, 39), Some(HealNeed::Major));
    let regen = BotConfig { hp_regen_spell: "regeneration".into(), ..BotConfig::default() };
    assert_eq!(heal_need(&regen, 69), Some(HealNeed::Regen));
    assert_eq!(heal_need(&regen, 39), Some(HealNeed::Major));
    let off = BotConfig { minor_heal_at_percent: 0, major_heal_at_percent: 0, ..BotConfig::default() };
    assert_eq!(heal_need(&off, 10), None);
}

// --- keys on the floor -------------------------------------------------

fn key_pack(on_ring: &[&str]) -> mud_client::pack::PackHandle {
    use mud_client::pack::PackHandle;
    use mud_client::sheet::Inventory;
    use mud_core::content::{Content, Item, ItemId};
    let mut content = Content::default();
    content.add_item(Item {
        id: ItemId(172),
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    content.add_item(Item {
        id: ItemId(5),
        name: "rusty dagger".into(),
        item_type: 1,
        ..Default::default()
    });
    let handle = PackHandle::new(std::sync::Arc::new(content));
    handle.refresh(&Inventory {
        items: Vec::new(),
        keys: on_ring.iter().map(|s| s.to_string()).collect(),
        encumbrance: None,
    });
    handle
}

fn floor(items: &[&str]) -> Event {
    Event::RoomSeen(RoomView {
        name: "Slum Street".into(),
        exits: vec!["north".into()],
        also_here: vec![],
        items: items.iter().map(|s| s.to_string()).collect(),
        also_here_sgr: Vec::new(),
    })
}

/// Default on, and independent of the coin sweep: `auto_get` is off
/// here and the key is still fetched.
#[test]
fn a_key_on_the_floor_is_taken() {
    let mut bot = Bot::new(BotConfig::default()).with_pack(Some(key_pack(&[])));
    assert_eq!(
        bot.on_event(&floor(&["black star key"])),
        vec![BotAction::Send("get black star key".into())]
    );
}

#[test]
fn a_key_already_on_the_ring_is_left() {
    let mut bot = Bot::new(BotConfig::default()).with_pack(Some(key_pack(&["black star key"])));
    assert!(bot.on_event(&floor(&["black star key"])).is_empty());
}

/// Somebody's dropped dagger is somebody's dagger.
#[test]
fn an_item_that_is_not_a_key_is_left() {
    let mut bot = Bot::new(BotConfig::default()).with_pack(Some(key_pack(&[])));
    assert!(bot.on_event(&floor(&["rusty dagger"])).is_empty());
}

#[test]
fn take_keys_off_leaves_keys() {
    let mut bot = Bot::new(BotConfig {
        take_keys: false,
        ..BotConfig::default()
    })
    .with_pack(Some(key_pack(&[])));
    assert!(bot.on_event(&floor(&["black star key"])).is_empty());
}

/// Without a pack the bot cannot tell a key from a dagger, so it takes
/// nothing rather than everything.
#[test]
fn without_a_pack_no_item_is_taken() {
    let mut bot = Bot::new(BotConfig::default());
    assert!(bot.on_event(&floor(&["black star key"])).is_empty());
}

/// The play assist builds its bot before realm entry hands the pack
/// over, so it is built with none and would otherwise never learn one.
/// `set_pack` is what lets it take a key it could not have taken at
/// the build.
#[test]
fn set_pack_after_the_build_lets_a_bot_take_a_key() {
    let mut bot = Bot::new(BotConfig::default());
    assert!(bot.on_event(&floor(&["black star key"])).is_empty());
    bot.set_pack(Some(key_pack(&[])));
    assert_eq!(
        bot.on_event(&floor(&["black star key"])),
        vec![BotAction::Send("get black star key".into())]
    );
}

/// One ask per visit: the board relists the floor on every block, and
/// a refused pickup would otherwise be asked for at the pacer floor
/// forever, the same rule the coin sweep already has.
#[test]
fn a_key_is_asked_for_once_per_visit() {
    let mut bot = Bot::new(BotConfig::default()).with_pack(Some(key_pack(&[])));
    assert_eq!(bot.on_event(&floor(&["black star key"])).len(), 1);
    assert!(bot.on_event(&floor(&["black star key"])).is_empty());
}

/// The board's acknowledgement of an item pickup, and the re-read that
/// follows it: the pack is refreshed from the board, never inferred.
#[test]
fn a_pickup_confirmation_asks_for_the_inventory() {
    let mut bot = Bot::new(BotConfig::default()).with_pack(Some(key_pack(&[])));
    assert_eq!(
        bot.on_event(&Event::Line("You took black star key.".into())),
        vec![BotAction::Send("i".into())]
    );
}

/// A coin pickup is the sweep's business and does not touch the pack.
#[test]
fn a_coin_pickup_does_not_ask_for_the_inventory() {
    let mut bot = Bot::new(BotConfig::default()).with_pack(Some(key_pack(&[])));
    assert!(
        bot.on_event(&Event::Line("You picked up 11 silver nobles".into()))
            .is_empty()
    );
}

#[test]
fn picked_up_item_reads_the_item_and_not_the_coins() {
    use mud_client::bot::picked_up_item;
    assert_eq!(
        picked_up_item("You took black star key."),
        Some("black star key".to_string())
    );
    // The reimplemented live board's uncaptured wording, kept pending a
    // capture.
    assert_eq!(
        picked_up_item("You picked up a silver holy amulet"),
        Some("silver holy amulet".to_string())
    );
    assert_eq!(picked_up_item("You picked up 11 silver nobles"), None);
    assert_eq!(picked_up_item("Mystic picked up some coins."), None);
    // Shares the "You took " prefix with a pickup, but it is a landed
    // blow, not an item.
    assert_eq!(picked_up_item("You took 12 damage."), None);
    assert_eq!(picked_up_item("You took 12 damage!"), None);
}

/// The ignore list names denominations as `get` takes them. Anything
/// else is a typo the profile loader should refuse.
#[test]
fn ignore_coins_accepts_only_the_five_denominations() {
    let ok = BotConfig {
        ignore_coins: vec!["copper".into(), "silver".into()],
        ..BotConfig::default()
    };
    assert!(ok.validate().is_ok());
    let bad = BotConfig {
        ignore_coins: vec!["coppers".into()],
        ..BotConfig::default()
    };
    let err = bad.validate().expect_err("not a denomination");
    assert!(err.contains("ignore_coins"), "{err}");
    assert!(err.contains("copper, silver, gold, platinum, runic"), "{err}");
}

fn loot_room(items: &[&str]) -> Event {
    Event::RoomSeen(RoomView {
        items: items.iter().map(|s| s.to_string()).collect(),
        ..view(&[])
    })
}

fn sweeping_bot(ignore: &[&str]) -> Bot {
    Bot::new(BotConfig {
        auto_get: true,
        ignore_coins: ignore.iter().map(|s| s.to_string()).collect(),
        ..BotConfig::default()
    })
}

/// A listed pile of an ignored denomination is left where it is, and
/// a pile of any other denomination in the same block is still taken.
#[test]
fn an_ignored_denomination_is_not_swept_from_the_render() {
    let mut bot = sweeping_bot(&["copper"]);
    let actions = bot.on_event(&loot_room(&["49 copper farthings", "11 silver nobles"]));
    assert_eq!(actions, vec![BotAction::Send("get silver".into())]);
}

/// The kill's drop line is the other sweep site, and it honours the
/// same list.
#[test]
fn an_ignored_denomination_is_not_swept_from_a_drop_line() {
    let mut bot = sweeping_bot(&["copper"]);
    bot.on_event(&room(&[]));
    assert!(bot
        .on_event(&Event::Line("49 copper drop to the ground.".into()))
        .is_empty());
    assert_eq!(
        bot.on_event(&Event::Line("11 silver drop to the ground.".into())),
        vec![BotAction::Send("get silver".into())]
    );
}

/// The travel guard asks whether a room is worth stopping for. A floor
/// that holds only ignored coins is not.
#[test]
fn a_floor_of_ignored_coins_is_not_worth_stopping_for() {
    let bot = sweeping_bot(&["copper"]);
    let only_copper = RoomView {
        items: vec!["49 copper farthings".into()],
        ..view(&[])
    };
    assert!(!bot.has_loot(&only_copper));
    let with_silver = RoomView {
        items: vec!["49 copper farthings".into(), "11 silver nobles".into()],
        ..view(&[])
    };
    assert!(bot.has_loot(&with_silver));
    assert!(bot.wants_coin("silver"));
    assert!(!bot.wants_coin("copper"));
    assert!(
        !Bot::new(BotConfig { auto_get: false, ..BotConfig::default() }).wants_coin("silver"),
        "with auto_get off nothing is wanted"
    );
    let ignoring_with_auto_get_off = Bot::new(BotConfig {
        auto_get: false,
        ignore_coins: vec!["copper".into()],
        ..BotConfig::default()
    });
    assert!(
        ignoring_with_auto_get_off.ignores_coin("copper"),
        "the list is read on its own, regardless of the toggle"
    );
    assert!(!ignoring_with_auto_get_off.ignores_coin("silver"));
}

/// A settings change mid stop must not forget the fight. The policy
/// moves, the latch stays.
#[test]
fn reconfigure_keeps_the_engaged_latch_and_changes_the_policy() {
    let mut bot = combat_bot();
    bot.on_event(&room(&["kobold thief"]));
    assert_eq!(bot.engaged(), Some("kobold thief"));
    assert!(!bot.ignores_coin("copper"));
    bot.reconfigure(BotConfig {
        auto_combat: true,
        ignore_coins: vec!["copper".into()],
        ..BotConfig::default()
    });
    assert_eq!(bot.engaged(), Some("kobold thief"), "the fight is still on");
    assert!(bot.ignores_coin("copper"), "the new policy applies");
}


/// What `/bot` off makes of a profile's table: every automatic policy
/// off, and everything that is a choice rather than a policy kept, so
/// the next press restores the table as it was.
#[test]
fn switched_off_keeps_the_marks_and_pools_but_runs_nothing() {
    let cfg = BotConfig {
        max_hp: 52,
        max_mana: 10,
        auto_hide: true,
        buffs: vec!["bless".into()],
        rest_at_percent: 60,
        ignore: vec!["beetle".into()],
        ..BotConfig::default()
    };
    let off = cfg.switched_off();
    assert!(!off.auto_combat && !off.auto_heal && !off.auto_rest, "{off:?}");
    assert!(!off.auto_get && !off.take_keys && !off.auto_sneak && !off.auto_flee, "{off:?}");
    assert!(!off.auto_hide, "{off:?}");
    assert!(off.buffs.is_empty(), "a buff is a cast nobody asked for: {off:?}");
    assert_eq!((off.max_hp, off.max_mana), (52, 10));
    assert_eq!(off.rest_at_percent, 60);
    assert_eq!(off.ignore, vec!["beetle".to_string()]);
    assert_eq!(off.switched_off(), off, "off twice is off");
}

/// Idle is nothing in hand: no fight running and no heal in flight.
/// What the follower's deposit gate asks before it spends the
/// character's turn on an inventory read.
#[test]
fn idle_is_no_fight_and_no_heal_in_flight() {
    let mut bot = combat_bot();
    assert!(bot.is_idle(), "a fresh bot has nothing in hand");
    bot.on_event(&room(&["kobold thief"]));
    assert_eq!(bot.engaged(), Some("kobold thief"));
    assert!(!bot.is_idle(), "a fight is running");
    bot.on_event(&Event::Line("You say \"a thief\"".into()));
    assert!(bot.is_idle(), "the target was gone");
}
