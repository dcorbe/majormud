//! Tests for monster loot: the spawn-time carry roll and the silent death
//! drop (`generate_monster` 0x24361 step 5, `check_kill_monster` 0x24eb7).
//!
//! `itemdropper` is a CARRY chance rolled once per slot at spawn
//! (genrdn(1,100) <= dropper); death then drops everything actually
//! carried, with no message — the items just join the room's notice line.
//! The first-kill-guaranteed rule for limited-population monsters arrives
//! with M6's respawn stamps.

use mud_core::content::{
    AttackForm, Class, ClassId, Content, Item, ItemId, LootSlot, Monster, MonsterId, Race, RaceId,
    Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

/// A 1-HP thug carrying loot per the given slots.
fn thug(loot: Vec<LootSlot>) -> Monster {
    Monster {
        id: MonsterId(10),
        name: "thug".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 1,
        experience: 9,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 0,
        magic_resist: 0,
        bs_defence: 0,
        energy: 1000,
        coins: [0, 0, 0, 0, 0],
        weapon: Some(ItemId(92)),
        loot,
        attacks: [AttackForm::default(); 5],
    }
}

fn world(loot: Vec<LootSlot>) -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Alley".into(),
        description: vec![],
        room_type: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_item(Item {
        id: ItemId(1356),
        name: "severed arm".into(),
        weight: 50,
        uses: -1,
        gettable: 1,
        ..Item::default()
    });
    content.add_item(Item {
        id: ItemId(172),
        name: "black robes".into(),
        weight: 30,
        uses: 5,
        gettable: 1,
        ..Item::default()
    });
    content.add_item(Item {
        id: ItemId(92),
        name: "spiked club".into(),
        weight: 100,
        item_type: 1,
        uses: -1,
        gettable: 1,
        ..Item::default()
    });
    content.add_monster(thug(loot));
    content.add_race(Race {
        id: RaceId(2),
        name: "Dwarf".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 30,
        },
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
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        ..CoreConfig::default()
    }
}

fn create(core: &mut Core, name: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.input(s, "No");
    core.drain_events();
    s
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

fn kill_thug(core: &mut Core, s: SessionId) -> Vec<Event> {
    let mut all = Vec::new();
    core.input(s, "attack thug");
    all.extend(core.drain_events());
    for _ in 0..100 {
        if all.iter().any(|e| matches!(e, Event::Output { text, .. } if text.contains("is dead"))) {
            break;
        }
        core.tick();
        all.extend(core.drain_events());
    }
    all
}

#[test]
fn certain_loot_drops_to_the_floor_silently() {
    let mut core = Core::new(
        world(vec![LootSlot {
            item: ItemId(1356),
            uses: -1,
            dropper: 100, // genrdn(1,100) <= 100: always carried
        }]),
        config(),
    );
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(10), RoomId { map: 1, room: 1 });
    let events = kill_thug(&mut core, s);
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("severed arm"),
        "the drop itself is silent: {shown:?}"
    );

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You notice severed arm here."),
        "loot on the floor: {shown:?}"
    );
}

#[test]
fn zero_dropper_never_carries() {
    let mut core = Core::new(
        world(vec![LootSlot {
            item: ItemId(1356),
            uses: -1,
            dropper: 0, // genrdn(1,100) <= 0: never
        }]),
        config(),
    );
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(10), RoomId { map: 1, room: 1 });
    kill_thug(&mut core, s);

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        !shown.contains("severed arm"),
        "no loot carried: {shown:?}"
    );
}

#[test]
fn all_certain_slots_drop_and_the_weapon_does_not() {
    let mut core = Core::new(
        world(vec![
            LootSlot {
                item: ItemId(1356),
                uses: -1,
                dropper: 100,
            },
            LootSlot {
                item: ItemId(172),
                uses: 3, // template uses override the item default of 5
                dropper: 100,
            },
        ]),
        config(),
    );
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(10), RoomId { map: 1, room: 1 });
    kill_thug(&mut core, s);

    core.input(s, "look");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You notice severed arm, black robes here."),
        "both slots dropped: {shown:?}"
    );
    assert!(
        !shown.contains("spiked club"),
        "check_kill_monster never drops the wielded weapon: {shown:?}"
    );

    // The template's uses value rides along to the floor item.
    core.input(s, "get robes");
    core.drain_events();
    let inv = core.player_snapshot(s).inventory.clone();
    assert!(
        inv.contains(&(ItemId(172), 3)),
        "uses copied from the loot slot: {inv:?}"
    );
}
