//! M7 slice 6: quest progress survives a board restart — the mid-quest
//! player rides through a real StateDb between the two steps of the
//! synthetic quest, and the completion grant fires on the post-restart
//! attach.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Message, MessageId, Race, RaceId, Room, RoomId,
    StatBlock, TextBlock, TextBlockId,
};
use mud_core::game::{Core, CoreConfig, Gender, Player};
use mud_server::state_db::StateDb;

const A: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: A,
        name: "Shrine".into(),
        command_block: Some(TextBlockId(997)),
        ..Default::default()
    });
    content.add_text_block(TextBlock {
        id: TextBlockId(997),
        next: None,
        body: "give ruby to priest:checkitem 400:takeitem 400:giveability 129 1:message 802\n\
               recite oath:testability 129 1:checkability 129 1:giveability 129 1:message 803"
            .into(),
    });
    for (id, line) in [(802u16, "The priest accepts your ruby."), (803, "The circle brands you a druid.")] {
        content.add_message(Message {
            id: MessageId(id),
            lines: vec![line.into(), String::new(), String::new()],
        });
    }
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
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: A,
        ..CoreConfig::default()
    }
}

#[test]
fn quest_progress_survives_a_restart() {
    let db = StateDb::open_in_memory().expect("state db");
    let druid_counter = Ability::from_id(129).unwrap();

    // Session 1: step 1 of the quest, then the board "goes down" — the
    // last Persist snapshot is what the server would have saved.
    let mut core = Core::new(world(), config());
    let s = core.attach_player(Player {
        name: "Ovate".into(),
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
    });
    core.give_item(s, ItemId(400));
    core.input(s, "give ruby to priest");
    core.drain_events();
    let mid = core.player_snapshot(s);
    assert_eq!(mid.innate_value(druid_counter), 1, "step 1 done");
    db.save_player(&mid).expect("save mid-quest");

    // Restart: a fresh Core, the player loaded from disk.
    let loaded = db.load_player("Ovate").expect("query").expect("found");
    assert_eq!(loaded.innate_value(druid_counter), 1, "counter survived");
    let mut core = Core::new(world(), config());
    let s = core.attach_player(loaded);
    core.input(s, "recite oath");
    core.drain_events();
    let done = core.player_snapshot(s);
    assert_eq!(done.innate_value(druid_counter), 2, "step 2 done post-restart");
    db.save_player(&done).expect("save finished");

    // The next login (load + attach) fires the completion grant.
    let finished = db.load_player("Ovate").expect("query").expect("found");
    let mut core = Core::new(world(), config());
    let s = core.attach_player(finished);
    assert_eq!(
        core.player_snapshot(s).innate_value(Ability::from_id(0x46).unwrap()),
        1,
        "S.C. granted at the post-completion login"
    );
}
