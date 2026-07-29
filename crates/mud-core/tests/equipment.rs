//! Tests for arm/wield/equip, wear/remove, and combat integration
//! (oracle_m4_round2.raw).

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, SessionId};

fn quarterstaff() -> Item {
    Item {
        id: ItemId(100),
        name: "quarterstaff".into(),
        weight: 100,
        item_type: 1,
        uses: -1,
        min_damage: 2,
        max_damage: 12,
        weapon_type: 1, // two-handed
        gettable: 1,
        speed: 1200,
        accuracy: 4,
        // A wielded weapon's `+0x342` counts toward the evasion word too —
        // see `wielded_weapon_evasion_counts_toward_the_fighter`.
        evasion: 30,
        ..Item::default()
    }
}

fn helmet() -> Item {
    Item {
        id: ItemId(200),
        name: "iron helmet".into(),
        weight: 60,
        item_type: 0,
        uses: -1,
        // Deliberately unequal, and neither a multiple of the other, so a
        // transposition of the two fighter words cannot pass silently.
        evasion: 20,
        damage_resist: 7,
        worn_on: 2,
        gettable: 1,
        // +5 accuracy while worn, to prove abilities flow to the bag.
        abilities: vec![(Ability::from_id(22).unwrap(), 5)],
        ..Item::default()
    }
}

/// Carries the two dynamic-accumulator abilities and no armour columns, so
/// each term is attributable: AC(2) -> `+0x70c` -> fighter `[1]`, and
/// DR(7) -> `+0x7b6` -> fighter `[3]`.
fn warded_amulet() -> Item {
    Item {
        id: ItemId(300),
        name: "warded amulet".into(),
        weight: 5,
        item_type: 0,
        uses: -1,
        worn_on: 8,
        gettable: 1,
        abilities: vec![(Ability::AC, 5), (Ability::DR, 30)],
        ..Item::default()
    }
}

/// A second Accuracy(22) carrier in a different slot, so the shared
/// dynamic-accuracy word's MAX-vs-SUM semantics are observable.
fn steel_bracer() -> Item {
    Item {
        id: ItemId(400),
        name: "steel bracer".into(),
        weight: 10,
        item_type: 0,
        uses: -1,
        worn_on: 10,
        gettable: 1,
        abilities: vec![(Ability::from_id(22).unwrap(), 3)],
        ..Item::default()
    }
}

/// Rides Accuracy(2) (105/0x69) — the same +0x70a word as ability 22.
fn opal_ring() -> Item {
    Item {
        id: ItemId(500),
        name: "opal ring".into(),
        weight: 4,
        item_type: 0,
        uses: -1,
        worn_on: 4,
        gettable: 1,
        abilities: vec![(Ability::from_id(105).unwrap(), 8)],
        ..Item::default()
    }
}

/// A worn item with `item_type != 0` — the DLL's fighter loop skips these
/// (`+0x2f4 != 0` guard at 24788-24790) for ALL three accumulations:
/// accuracy ratings, evasion, and DR. Carries all three columns so each
/// gate is observable.
fn glowing_idol() -> Item {
    Item {
        id: ItemId(600),
        name: "glowing idol".into(),
        weight: 10,
        item_type: 7,
        uses: -1,
        worn_on: 6,
        gettable: 1,
        accuracy: 5,
        evasion: 20,
        damage_resist: 10,
        ..Item::default()
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Weapons Shop".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    content.add_item(quarterstaff());
    content.add_item(helmet());
    content.add_item(warded_amulet());
    content.add_item(steel_bracer());
    content.add_item(opal_ring());
    content.add_item(glowing_idol());
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
        saved_evil: 0,
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

#[test]
fn arm_holds_the_weapon() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "arm quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are now holding quarterstaff."),
        "got: {shown:?}"
    );

    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are carrying quarterstaff (Two handed)\n"),
        "got: {shown:?}"
    );
}

#[test]
fn wield_and_eq_are_synonyms_with_partial_names() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "wield quar");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You are now holding quarterstaff."), "got: {shown:?}");

    let s2 = create(&mut core, "Bofur");
    core.give_item(s2, ItemId(100));
    core.input(s2, "eq quar");
    let shown = text_to(&core.drain_events(), s2);
    assert!(shown.contains("You are now holding quarterstaff."), "got: {shown:?}");
}

#[test]
fn arming_an_absent_item_matches_the_oracle() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "arm dagger");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You do not have dagger left unequipped."),
        "got: {shown:?}"
    );
}

#[test]
fn armed_weapon_feeds_the_fighter() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "arm quarterstaff");
    core.drain_events();

    let (fighter, eu) = core.combat_debug(s);
    // Damage from the staff (2-12), Str 50 adds nothing.
    assert_eq!(fighter.min_damage, 2);
    assert_eq!(fighter.max_damage, 12);
    // skill = staff accuracy 4 (floor bypassed) + 15 unencumbered = 19;
    // accuracy = 0 + 2*((6-1)*1 + 12 + 0 + 19/2 - 2) + (30-50)/6
    //          = 2*(5+12+0+9-2) - 3 = 48 - 3 = 45.
    assert_eq!(fighter.accuracy, 45);
    // EU with staff speed 1200 = same as fists at L1: 1200000/1530 = 784.
    assert_eq!(eu, 784);
}

#[test]
fn wear_and_remove_armor() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(200));
    core.input(s, "wear helmet");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are now wearing iron helmet."),
        "got: {shown:?}"
    );

    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    // oracle_m4_verify.raw: worn items carry their location name.
    assert!(
        shown.contains("You are carrying iron helmet (Head)\n"),
        "got: {shown:?}"
    );

    core.input(s, "remove helmet");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You have removed iron helmet."),
        "got: {shown:?}"
    );
}

#[test]
fn removing_unworn_matches_the_oracle() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "remove quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You are not wearing quarterstaff."),
        "got: {shown:?}"
    );
}

#[test]
fn worn_item_abilities_feed_derived_stats() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(200));
    core.input(s, "wear helmet");
    core.drain_events();

    // Helmet grants Accuracy +5 (ability 22) — visible via the fighter's
    // dynamic accuracy accumulator (+0x70a analogue).
    let (fighter, _) = core.combat_debug(s);
    let naked = {
        let s2 = create(&mut core, "Bofur");
        core.combat_debug(s2).0
    };
    assert_eq!(fighter.accuracy, naked.accuracy + 5);

    // The two worn armour columns feed DIFFERENT fighter words
    // (`move_player_to_fighter` 24788-24789): `+0x342` (`ac`) accumulates
    // into [1] and is divided by 10 at 24866, driving the quadratic to-hit
    // term; `+0x39c` (`dr`) accumulates into [3] raw, and it is
    // `calculate_attack` that divides by 10 when it subtracts the soak
    // (25335). The helmet is evasion 20 / DR 7.
    let defender = core.defender_debug(s);
    assert_eq!(defender.evasion_a, 2, "Σ worn +0x342 ÷ 10");
    assert_eq!(defender.armor, 7, "Σ worn +0x39c, raw");
}

#[test]
fn type_gated_worn_item_feeds_no_fighter_word() {
    // The worn loop's `+0x2f4 != 0` skip (24788-24790) wraps the WHOLE
    // accumulation block — accuracy ratings (+0x39a), evasion (+0x342),
    // and DR (+0x39c) alike. Shipped data has 33 type!=0 worn ac-carriers
    // (e.g. glowing red amulet #494), so the defender half is live; no
    // shipped type!=0 worn item carries accuracy, so that half is
    // fidelity-only. The WEAPON's seed contributions stay ungated —
    // `wielded_weapon_evasion_counts_toward_the_fighter` and
    // `armed_weapon_feeds_the_fighter` (staff type 1) pin that side.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let naked_acc = core.combat_debug(s).0.accuracy;

    core.give_item(s, ItemId(600));
    core.input(s, "wear idol");
    core.drain_events();

    assert_eq!(
        core.combat_debug(s).0.accuracy,
        naked_acc,
        "type-7 accuracy 5 must not reach the ratings sum"
    );
    let defender = core.defender_debug(s);
    assert_eq!(defender.evasion_a, 0, "type-7 evasion 20 skipped");
    assert_eq!(defender.armor, 0, "type-7 DR 10 skipped");
}

#[test]
fn dynamic_accuracy_takes_the_max_contribution() {
    // The three accuracy abilities (22/0x16, 105/0x69, 106/0x6a) all land
    // on the ONE +0x70a word, and `update_dynamic_with_ability`
    // (37549-37559) writes it only when the new value EXCEEDS the stored
    // one — a joint MAX (seeded -32000, reset to 0 if untouched), not a
    // sum. Two 0x16 carriers therefore contribute their larger value, and
    // a bigger 0x69 value displaces both.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let naked = core.combat_debug(s).0.accuracy;

    core.give_item(s, ItemId(200)); // helmet: Accuracy(22) +5
    core.give_item(s, ItemId(400)); // steel bracer: Accuracy(22) +3
    core.input(s, "wear helmet");
    core.input(s, "wear bracer");
    core.drain_events();
    assert_eq!(
        core.combat_debug(s).0.accuracy,
        naked + 5,
        "two 0x16 carriers keep the max, not the sum"
    );

    core.give_item(s, ItemId(500)); // opal ring: Accuracy(2)/0x69 +8
    core.input(s, "wear ring");
    core.drain_events();
    assert_eq!(
        core.combat_debug(s).0.accuracy,
        naked + 8,
        "0x69's 8 displaces 0x16's 5 on the shared word"
    );
}

/// The evasion accumulator is seeded by the WIELDED weapon and only then
/// summed over the worn slots. `move_player_to_fighter`: `local_c = 0`
/// (24466), `local_c = weapon+0x342` (24683, a plain assignment), then
/// `local_c += item+0x342` per worn slot (24797), and `[1] = local_c/10`
/// (24866). The soak word `[3]` gets no such seed — the weapon never
/// touches it, only the worn loop at 24789 does.
///
/// Pinned because the worn loop reads as the only contributor if you stop
/// at 24797, and a reviewer working from that alone would delete the
/// weapon term as a divergence. It is not one.
#[test]
fn wielded_weapon_evasion_counts_toward_the_fighter() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let bare = core.defender_debug(s).evasion_a;
    assert_eq!(bare, 0, "naked: no weapon, no worn armour");

    core.give_item(s, ItemId(100));
    core.input(s, "arm quarterstaff");
    core.drain_events();
    // Staff evasion 30 -> (30 + 0)/10.
    assert_eq!(core.defender_debug(s).evasion_a, 3);
    // ...and contributes nothing to the soak.
    assert_eq!(core.defender_debug(s).armor, 0);
}

#[test]
fn dropping_the_armed_weapon_unarms_it() {
    // Oracle: "drop quarterstaff" while holding it just dropped it.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(100));
    core.input(s, "arm quarterstaff");
    core.drain_events();
    core.input(s, "drop quarterstaff");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You dropped quarterstaff."), "got: {shown:?}");

    let (fighter, _) = core.combat_debug(s);
    assert_eq!(fighter.max_damage, 4, "back to fists");
}

#[test]
fn carrying_line_suffixes_and_grouping_match_the_oracle() {
    // oracle_m4_verify.raw: "chain coif (Head), dagger (Weapon Hand),
    // quarterstaff, 3 sickle" — worn first (location name), armed weapon
    // next (hand suffix), then loose items grouped with a count prefix.
    let mut content = world();
    content.add_item(Item {
        id: ItemId(74),
        name: "sickle".into(),
        weight: 30,
        item_type: 1,
        uses: -1,
        weapon_type: 0, // one-handed
        gettable: 1,
        ..Item::default()
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(200));
    core.give_item(s, ItemId(100));
    core.give_item(s, ItemId(74));
    core.give_item(s, ItemId(74));
    core.give_item(s, ItemId(74));
    core.input(s, "wear helmet");
    core.input(s, "arm sickle");
    core.drain_events();

    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains(
            "You are carrying iron helmet (Head), sickle (Weapon Hand), quarterstaff, 2 sickle\n"
        ),
        "worn -> armed -> grouped loose: {shown:?}"
    );

    // Two-handed weapons get the other suffix.
    core.input(s, "arm quarterstaff");
    core.drain_events();
    core.input(s, "i");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("quarterstaff (Two handed)"),
        "2H suffix: {shown:?}"
    );
    assert!(
        shown.contains("3 sickle"),
        "unarmed sickles regroup: {shown:?}"
    );
}

/// The two dynamic accumulators reach the fighter words on the DEFENDER
/// side, exactly as the accuracy accumulator (`+0x70a`) already reaches the
/// attacker's. `move_player_to_fighter`: `[1] += player+0x70c` (24885) and
/// `[3] += player+0x7b6` (24888), both raw. `update_dynamic_with_ability`
/// (0x3dba6, 37455) is what fills them — case 2 -> `+0x70c`, case 7 ->
/// `+0x7b6`.
///
/// Both are added in the SAME units as the item sums they join, which is
/// why AC(2) is not scaled here while `get_armour_rating` multiplies it by
/// 10: the fighter's `[1]` is already the ÷10 quantity, the rating's
/// accumulator is not. The two agree in display units, and the measured
/// talisman capture pins that agreement.
#[test]
fn ac_and_dr_abilities_reach_the_defender_words() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(300));
    core.input(s, "wear amulet");
    core.drain_events();

    let d = core.defender_debug(s);
    // No armour columns on the amulet, so each word is the ability alone.
    assert_eq!(d.evasion_a, 5, "AC(2) joins [1] raw (+0x70c, 24885)");
    assert_eq!(d.armor, 30, "DR(7) joins [3] raw (+0x7b6, 24888)");
}
