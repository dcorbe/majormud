//! M7 slice 6: the dispatch funnel — room `cmdtext` special commands in
//! the unconsumed-input path (execute_input 49143-49160), including the
//! fidelity fix that funnels EVERY fallen-through verb, and the
//! look/buy/use hook points.

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
        name: "Gaming Hall".into(),
        command_block: Some(TextBlockId(997)),
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
            "The lever grinds into place.".into(),
            "%s pulls the great lever.".into(),
            String::new(),
        ],
    });
    // The room's special-command block: an Unknown-phrase trigger and a
    // verb-shaped trigger only reachable through the widened funnel.
    content.add_text_block(TextBlock {
        id: TextBlockId(997),
        next: None,
        body: "pull lever:message 801\nget token:giveitem 400:flag 3 set".into(),
    });
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: A,
        ..CoreConfig::default()
    }
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

#[test]
fn unknown_input_runs_the_room_wildcard_before_say() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Gambler"));
    core.drain_events();
    core.input(s, "pull lever");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("The lever grinds into place."),
        "got: {shown:?}"
    );
    assert!(!shown.contains("You say"), "consumed, not said: {shown:?}");
}

#[test]
fn unmatched_input_still_falls_to_say() {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Gambler"));
    core.drain_events();
    core.input(s, "dance wildly");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You say"), "got: {shown:?}");
}

#[test]
fn fallen_through_verb_reaches_the_room_wildcard() {
    // The fidelity FIX (execute_input 49143): "get token" with no token
    // on the floor falls through the GET handler and must reach the
    // cmdtext block — the DLL funnels every unconsumed line, our old
    // dispatch said it aloud.
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Gambler"));
    core.drain_events();
    core.input(s, "get token");
    let shown = text_to(&core.drain_events(), s);
    assert!(!shown.contains("You say"), "consumed by the block: {shown:?}");
    let p = core.player_snapshot(s);
    assert_eq!(p.inventory, vec![(ItemId(400), -1)], "block gave the token");
    assert_eq!(p.quest_flags, 1 << 2);
}

#[test]
fn room_wildcard_survives_a_failing_tail_without_saying() {
    // A matched line whose tail fail-stops still CONSUMES the input
    // (execute_input treats any nonzero return as handled, 49143-49145).
    let mut content = world();
    content.add_text_block(TextBlock {
        id: TextBlockId(998),
        next: None,
        body: "pray:minlevel 99 801:flag 3 set".into(),
    });
    content.add_room(Room {
        id: A,
        name: "Gaming Hall".into(),
        command_block: Some(TextBlockId(998)),
        ..Default::default()
    });
    let mut core = Core::new(content, config());
    let s = core.attach_player(player("Gambler"));
    core.drain_events();
    core.input(s, "pray");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("The lever grinds"), "failure message shown");
    assert!(!shown.contains("You say"), "consumed despite the fail: {shown:?}");
    assert_eq!(core.player_snapshot(s).quest_flags, 0);
}
