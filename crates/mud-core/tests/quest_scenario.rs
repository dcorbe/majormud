//! M7 slice 6 milestone goldens (design doc testing bar): the takeitem
//! rollback transcript pair, and (Task 12) the synthetic multi-step
//! quest. Byte-pinned output — these move only with a deliberate
//! re-pin against the decompile.

use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Message, MessageId, Race, RaceId, Room, RoomId,
    StatBlock,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: A,
        name: "Shrine".into(),
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
