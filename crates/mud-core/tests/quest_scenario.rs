//! M7 slice 6 milestone goldens (design doc testing bar): the takeitem
//! rollback transcript pair, and (Task 12) the synthetic multi-step
//! quest. Byte-pinned output — these move only with a deliberate
//! re-pin against the decompile.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Message, MessageId, Race, RaceId, Room, RoomId,
    StatBlock, TextBlock, TextBlockId,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: A,
        name: "Shrine".into(),
        command_block: Some(TextBlockId(997)),
        ..Default::default()
    });
    // The synthetic two-step quest (the shipped offering shape):
    // step 1 surrenders the token for DarkDruidQuest = 1; step 2's
    // oath needs exactly step 1 (testability+checkability pair) and
    // advances to the == 2 completion threshold.
    content.add_text_block(TextBlock {
        id: TextBlockId(997),
        next: None,
        body: "give ruby to priest:checkitem 400 801:takeitem 400:giveability 129 1:message 802\n\
               recite oath:testability 129 1:checkability 129 1:giveability 129 1:message 803"
            .into(),
    });
    content.add_message(Message {
        id: MessageId(802),
        lines: vec![
            "The priest accepts your ruby.".into(),
            "%s hands the priest a ruby.".into(),
            String::new(),
        ],
    });
    content.add_message(Message {
        id: MessageId(803),
        lines: vec![
            "The circle brands you a druid.".into(),
            String::new(),
            String::new(),
        ],
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
    content.add_item(Item {
        id: ItemId(400),
        name: "brass token".into(),
        weight: 1,
        item_type: 0,
        uses: -1,
        gettable: 1,
        ..Item::default()
    });
    content.add_message(Message {
        id: MessageId(801),
        lines: vec![
            "The priest shakes his head.".into(),
            "%s is refused by the priest.".into(),
            String::new(),
        ],
    });
    content
}

fn player(name: &str) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 5,
        current_hp: 40,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: A,
        ..Default::default()
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

/// The design-doc multi-step quest golden: trigger -> gates -> item
/// surrender -> counter to threshold -> completion grant at re-attach ->
/// class-skill strip at a later login. Byte-pinned transcripts.
#[test]
fn synthetic_two_step_quest_golden() {
    let mut core = Core::new(world(), CoreConfig { start_location: A, ..CoreConfig::default() });
    let mut ovate = player("Ovate");
    ovate.class = ClassId(1);
    ovate.level = 22; // holds Smash's class-1 gate for the strip half
    let s = core.attach_player(ovate);
    let witness = core.attach_player(player("Witness"));
    core.drain_events();
    core.give_item(s, ItemId(400));

    // Out of order: the oath's testability gate refuses an absent
    // counter — consumed silently (no say, no brand).
    core.input(s, "recite oath");
    assert_eq!(text_to(&core.drain_events(), s), "[HP=40]:", "silent refusal");
    assert_eq!(core.player_snapshot(s).innate_value(Ability::from_id(129).unwrap()), 0);

    // Step 1: the offering.
    core.input(s, "give ruby to priest");
    let events = core.drain_events();
    assert_eq!(
        text_to(&events, s),
        "The priest accepts your ruby.\n[HP=40]:",
        "step-1 transcript"
    );
    assert!(
        text_to(&events, witness).contains("Ovate hands the priest a ruby."),
        "step-1 room transcript: got {:?}",
        text_to(&events, witness)
    );
    let snap = core.player_snapshot(s);
    assert!(snap.inventory.is_empty(), "ruby surrendered");
    assert_eq!(snap.innate_value(Ability::from_id(129).unwrap()), 1);

    // Replaying step 1 without the ruby: the checkitem message, and no
    // double advance.
    core.input(s, "give ruby to priest");
    assert_eq!(
        text_to(&core.drain_events(), s),
        "The priest shakes his head.\n[HP=40]:"
    );
    assert_eq!(core.player_snapshot(s).innate_value(Ability::from_id(129).unwrap()), 1);

    // Step 2: the oath — exactly step 1 required (the shipped
    // testability/checkability pair), advancing to the threshold.
    core.input(s, "recite oath");
    assert_eq!(
        text_to(&core.drain_events(), s),
        "The circle brands you a druid.\n[HP=40]:",
        "step-2 transcript"
    );
    let mut done = core.player_snapshot(s);
    assert_eq!(done.innate_value(Ability::from_id(129).unwrap()), 2);
    assert_eq!(done.innate_value(Ability::from_id(0x46).unwrap()), 0, "no grant yet");

    // Relog: the completion detector grants S.C. +1 (DarkDruid == 2);
    // hand the character Smash too — level 22 class 1 keeps it.
    done.give_innate_ability(Ability::from_id(0x20).unwrap(), 1);
    let mut core = Core::new(world(), CoreConfig { start_location: A, ..CoreConfig::default() });
    let s = core.attach_player(done);
    let mut relogged = core.player_snapshot(s);
    assert_eq!(relogged.innate_value(Ability::from_id(0x46).unwrap()), 1, "S.C. granted");
    assert_eq!(relogged.innate_value(Ability::from_id(0x20).unwrap()), 1, "Smash kept");

    // A later login below the gate: Smash strips, the quest grant
    // re-derives identically (idempotence).
    relogged.level = 21;
    let mut core = Core::new(world(), CoreConfig { start_location: A, ..CoreConfig::default() });
    let s = core.attach_player(relogged);
    let final_snap = core.player_snapshot(s);
    assert_eq!(final_snap.innate_value(Ability::from_id(0x20).unwrap()), 0, "Smash stripped");
    assert_eq!(final_snap.innate_value(Ability::from_id(0x46).unwrap()), 1, "grant idempotent");
    assert_eq!(final_snap.innate_value(Ability::from_id(129).unwrap()), 2, "counter intact");
}

/// The design-doc takeitem rollback golden: the same offering chain run
/// against a player who fails the gate (keeps the token — restored by
/// the rollback) and one who passes (loses it). The chain mirrors the
/// shipped offering shape `takeitem <id>:<gate>:<advance>` with a
/// second takeitem as the failing gate.
#[test]
fn takeitem_rollback_golden() {
    // Chain: take the token, then demand an item the player lacks (999)
    // with refusal message 801 — the failed take restores the token.
    let chain = "takeitem 400:takeitem 999 801:giveability 129 2";

    // Failing run: the token survives (uses zeroed by the literal
    // rollback re-add, decompile 69399), the refusal prints to both
    // sides, the counter never advances.
    let mut core = Core::new(world(), CoreConfig { start_location: A, ..CoreConfig::default() });
    let s = core.attach_player(player("Pilgrim"));
    let witness = core.attach_player(player("Witness"));
    core.drain_events();
    core.give_item(s, ItemId(400));
    assert_eq!(core.debug_perform_matched_action(s, chain), 2);
    let events = core.drain_events();
    assert_eq!(
        text_to(&events, s),
        "\r\nThe priest shakes his head.\n",
        "actor transcript"
    );
    assert_eq!(
        text_to(&events, witness),
        "\r\nPilgrim is refused by the priest.\n",
        "room transcript"
    );
    let p = core.player_snapshot(s);
    assert_eq!(p.inventory, vec![(ItemId(400), 0)], "token restored");
    assert_eq!(p.innate_value(mud_core::ability::Ability::from_id(129).unwrap()), 0);

    // Passing run: drop the failing demand — the token leaves and the
    // counter advances, with no output at all.
    let mut core = Core::new(world(), CoreConfig { start_location: A, ..CoreConfig::default() });
    let s = core.attach_player(player("Pilgrim"));
    core.drain_events();
    core.give_item(s, ItemId(400));
    assert_eq!(
        core.debug_perform_matched_action(s, "takeitem 400:giveability 129 2"),
        1
    );
    assert_eq!(text_to(&core.drain_events(), s), "", "silent on success");
    let p = core.player_snapshot(s);
    assert!(p.inventory.is_empty(), "token surrendered");
    assert_eq!(p.innate_value(mud_core::ability::Ability::from_id(129).unwrap()), 2);
}
