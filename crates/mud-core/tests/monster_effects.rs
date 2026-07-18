//! Monster 5-slot active-spell effects — entry (`add_cast_spell_to_monster`
//! 0x3ed64), the offensive-duration engagement split (`cast_monster_target`
//! duration!=0 resolves immediately, engage-only is the duration==0 path),
//! upkeep (`medium_update_monster` 0x21cbc + the reduced handler set
//! 0x4a263), and termination (`perform_spell_termination_monster_upkeep`
//! 0x4a45d: Enslave + Poison only, no chains). `re/docs/spellcasting.md` §6.

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Element, MatchType, Message, MessageId, Monster,
    MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId, StatBlock,
    TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const TOWER: RoomId = RoomId { map: 1, room: 1 };
const MAGE: ClassId = ClassId(1);
const HUMAN: RaceId = RaceId(1);
const RAT: MonsterId = MonsterId(1);

/// Offensive-duration debuff: AC -5 / DR -3 / MR -10, duration 30.
const HEX: SpellId = SpellId(920);
/// HEX with duration 2 — the expiry probe.
const HEXSHORT: SpellId = SpellId(921);
/// Duration 2 + (EndCast -> BLIGHT) — the no-chain probe.
const HEXCHAIN: SpellId = SpellId(922);
/// (Damage, 7) + DescMsg driver, duration 10 — the upkeep DoT.
const BLIGHT: SpellId = SpellId(923);
/// Value-0 AC row, bounds 3..9 — the unconditional-refresh probe.
const REHEX: SpellId = SpellId(924);
/// (Poison, 6), duration 20 — the monster poison counter probe.
const VENOMOUS: SpellId = SpellId(925);
/// AreaC duration debuff — the area slot-entry probe.
const AREAHEX: SpellId = SpellId(926);
/// Five fillers for the slot-full probe.
const FILLER_BASE: u16 = 930;

fn spell(id: SpellId, name: &str, short: &str) -> Spell {
    Spell {
        id,
        name: name.into(),
        short_name: short.into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(901)),
        abilities: vec![],
        level_cap: 0,
        round_cost: 100,
        required_power: 1,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Offensive0,
        save_class: SaveClass::None,
        base_chance: 200, // auto-success: >= 200 skips the roll
        duration_per_level: 0,
        match_type: MatchType::Single0,
        duration: 30,
        element: Element::Magic,
        class_gate_group: 1,
        mana_cost: 4,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: TOWER,
        name: "Tower".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    let mut rat = Monster {
        id: RAT,
        name: "giant rat".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 40,
        experience: 12,
        exp_multi: 1,
        armour_class: 6,
        damage_resist: 2,
        magic_resist: 20,
        bs_defence: 0,
        energy: 0,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [AttackForm::default(); 5],
    };
    rat.abilities = vec![];
    content.add_monster(rat);
    content.add_race(Race {
        id: HUMAN,
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: MAGE,
        name: "Mage".into(),
        abilities: vec![],
        hp_per_level: 2,
        hp_seed: 4,
        caster_group: 1,
        casting_factor: 3,
        exp_base: 0,
        combat_factor: 2,
        weapon_code: 8,
        armour_code: 9,
    });
    content.add_message(Message {
        id: MessageId(901),
        lines: vec![
            "You cast %s on %s!".into(),
            "%s casts %s upon you!".into(),
            "%s casts %s on %s!".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(903),
        lines: vec![
            "The blight lifts.".into(),
            String::new(),
            "You are blighted!".into(),
        ],
    });

    let mut hex = spell(HEX, "hex", "hexx");
    hex.abilities = vec![(Ability::AC, -5), (Ability::DR, -3), (Ability::MR, -10)];
    let mut hexshort = spell(HEXSHORT, "brief hex", "brie");
    hexshort.abilities = vec![(Ability::AC, -5)];
    hexshort.duration = 2;
    let mut hexchain = spell(HEXCHAIN, "chained hex", "chai");
    hexchain.abilities = vec![(Ability::AC, -5), (Ability::EndCast, BLIGHT.0 as i16)];
    hexchain.duration = 2;
    let mut blight = spell(BLIGHT, "blight", "blig");
    blight.abilities = vec![(Ability::Damage, 7), (Ability::DescMsg, 903)];
    blight.duration = 10;
    let mut rehex = spell(REHEX, "rehex", "rehe");
    rehex.abilities = vec![(Ability::AC, 0)];
    rehex.min_base = 3;
    rehex.max_base = 9;
    let mut venomous = spell(VENOMOUS, "venomous touch", "veno");
    venomous.abilities = vec![(Ability::Poison, 6)];
    venomous.duration = 20;
    let mut areahex = spell(AREAHEX, "hex cloud", "hexc");
    areahex.abilities = vec![(Ability::AC, -5)];
    areahex.match_type = MatchType::AreaC;
    areahex.duration = 20;
    for s in [hex, hexshort, hexchain, blight, rehex, venomous, areahex] {
        content.add_spell(s);
    }
    for i in 0..5u16 {
        let id = SpellId(FILLER_BASE + i);
        let mut filler = spell(id, &format!("filler{i}"), &format!("fil{i}"));
        filler.abilities = vec![(Ability::AC, -1)];
        content.add_spell(filler);
    }
    content
}

fn caster(spells: &[SpellId]) -> Player {
    Player {
        name: "Zin".into(),
        gender: Gender::Male,
        race: HUMAN,
        class: MAGE,
        level: 5,
        stats: StatBlock::default(),
        base_stats: StatBlock::default(),
        hp_base: 0,
        current_hp: 200,
        current_mana: 100,
        hunger: 1000,
        thirst: 1000,
        coins: Default::default(),
        lawful: false,
        inventory: vec![],
        weapon: None,
        bankbooks: vec![],
        worn: vec![],
        cp_unspent: 0,
        cp_lifetime: 0,
        lives: 9,
        experience: 0,
        location: TOWER,
        spellbook: spells.iter().map(|s| (*s, false)).collect::<BTreeMap<_, _>>(),
        poison: 0,
        active_spells: Default::default(),
    }
}

fn setup(spells: &[SpellId]) -> (Core, SessionId, mud_core::game::MonsterInstanceId) {
    let mut core = Core::new(
        world(),
        CoreConfig { start_location: TOWER, ..CoreConfig::default() },
    );
    let m = core.spawn_monster(RAT, TOWER).expect("rat spawns");
    let s = core.attach_player(caster(spells));
    core.drain_events();
    (core, s, m)
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

// --- entry + the offensive-duration split ---

#[test]
fn offensive_duration_cast_resolves_immediately_without_engaging() {
    // cast_monster_target: engage-only is CONDITIONED on duration == 0
    // (43421-43481); the duration!=0 path rolls, pays and applies at the
    // command — no engage_autocombat, no *Combat Engaged*.
    let (mut core, s, m) = setup(&[HEX]);
    core.input(s, "cast hex rat");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    assert!(!shown.contains("Combat Engaged"), "got: {shown:?}");
    assert!(shown.contains("You cast hex on giant rat!"), "got: {shown:?}");
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert_eq!(slots[0].spell, Some(HEX));
    assert_eq!(slots[0].value, -5, "the triggering row's fixed value is stored");
    assert_eq!(slots[0].remaining, 30, "flat duration, no scaling rows");
    // Full costs paid at the command (mana 4 of 100).
    assert_eq!(core.current_mana(s), 96);
}

#[test]
fn slot_contributions_fold_into_the_combat_mapping() {
    // move_monster_to_fighter (25169-25200): evasion += slot AC(2), soak
    // += slot DR(7) raw on top of the template dr*10; monster_save_stat
    // reads MR(36) through the same fold.
    let (mut core, s, m) = setup(&[HEX]);
    let (ev0, ar0, sv0) = core.monster_defense_debug(m).expect("baseline");
    assert_eq!((ev0, ar0, sv0), (6, 20, 20));
    core.input(s, "cast hex rat");
    core.drain_events();
    let (ev, ar, sv) = core.monster_defense_debug(m).expect("hexed");
    assert_eq!(ev, 1, "AC -5 folds into evasion");
    assert_eq!(ar, 17, "DR -3 folds raw into the soak");
    assert_eq!(sv, 10, "MR -10 folds into the save stat");
}

#[test]
fn expiry_clears_the_slot_and_the_debuff() {
    let (mut core, s, m) = setup(&[HEXSHORT]);
    core.input(s, "cast brief hex rat");
    core.drain_events();
    assert_eq!(core.monster_defense_debug(m).map(|(ev, _, _)| ev), Some(1));
    // Upkeep every 3 ticks: t3 (rem 1), t6 (rem 0 -> terminate).
    for _ in 0..6 {
        core.tick();
    }
    core.drain_events();
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert!(slots.iter().all(|slot| slot.spell.is_none()), "slot cleared at 0");
    assert_eq!(
        core.monster_defense_debug(m).map(|(ev, _, _)| ev),
        Some(6),
        "the debuff vanishes with the recompute"
    );
}

#[test]
fn refresh_is_unconditional_for_monster_slots() {
    // add_cast_spell_to_monster (38260-38268): an active id overwrites
    // value AND duration with no exceed check — unlike the player entry.
    let (mut core, s, m) = setup(&[REHEX]);
    core.input(s, "cast rehex rat");
    core.drain_events();
    for _ in 0..3 {
        core.tick(); // one upkeep pass: remaining 29
    }
    core.drain_events();
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert_eq!(slots[0].spell, Some(REHEX));
    assert_eq!(slots[0].remaining, 29);
    core.input(s, "cast rehex rat");
    core.drain_events();
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert_eq!(slots[0].spell, Some(REHEX), "same slot, no duplicate");
    assert_eq!(slots[0].remaining, 30, "duration overwritten in place");
    assert!(slots[1].spell.is_none());
}

#[test]
fn a_sixth_spell_finds_no_slot_and_fails_loud() {
    // Both 5-slot scans exhausted (38270-38285): "You attempt to cast %s,
    // but fail." to the caster, effect lost, costs stay paid, no castmsgb.
    let mut spells: Vec<SpellId> = (0..5).map(|i| SpellId(FILLER_BASE + i)).collect();
    spells.push(HEX);
    let (mut core, s, m) = setup(&spells);
    for i in 0..5 {
        core.input(s, &format!("cast filler{i} rat"));
    }
    core.drain_events();
    let mana_before = core.current_mana(s);
    core.input(s, "cast hex rat");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    assert!(shown.contains("You attempt to cast hex, but fail"), "got: {shown:?}");
    assert!(!shown.contains("You cast hex on giant rat!"), "got: {shown:?}");
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert!(slots.iter().all(|slot| slot.spell != Some(HEX)));
    assert_eq!(core.current_mana(s), mana_before - 4, "full costs stay paid");
}

// --- area duration entry ---

#[test]
fn area_duration_casts_enter_every_monster_slot_table() {
    let (mut core, s, m1) = setup(&[AREAHEX]);
    let m2 = core.spawn_monster(RAT, TOWER).expect("second rat");
    core.input(s, "cast hex cloud");
    core.drain_events();
    for m in [m1, m2] {
        let slots = core.monster_active_spells(m).expect("rat lives");
        assert_eq!(slots[0].spell, Some(AREAHEX), "slot entered on {m:?}");
        assert_eq!(
            core.monster_defense_debug(m).map(|(ev, _, _)| ev),
            Some(1),
            "debuff live on {m:?}"
        );
    }
}

// --- upkeep ---

#[test]
fn upkeep_dot_kills_through_the_death_path() {
    // BLIGHT: instant Damage 7 at cast (case 1 has no duration gate),
    // then 7 per upkeep tick from the slot. Rat 40 hp: 33 at entry, then
    // t3 26, t6 19, t9 12, t12 5, t15 -2 -> HP < 0 after the slot walk
    // (19327-19339): killer-less check_kill_monster + the death announce.
    let (mut core, s, m) = setup(&[BLIGHT]);
    core.input(s, "cast blight rat");
    core.drain_events();
    assert_eq!(core.monster_hp(m), Some(33));
    let mut events = Vec::new();
    for _ in 0..15 {
        core.tick();
        events.extend(core.drain_events());
    }
    assert_eq!(core.monster_hp(m), None, "dead and gone");
    let shown = text_to(&events, s);
    assert!(shown.contains("giant rat"), "death announce reaches the room: {shown:?}");
    assert!(!shown.contains("experience"), "nobody engaged: no exp split: {shown:?}");
}

#[test]
fn monster_expiry_never_chains() {
    // perform_spell_termination_monster_upkeep reverses ONLY Enslave and
    // Poison — no EndCast chain (spec §6.6). HEXCHAIN carries
    // (EndCast -> BLIGHT); at expiry nothing enters and no damage ticks.
    let (mut core, s, m) = setup(&[HEXCHAIN]);
    core.input(s, "cast chained hex rat");
    core.drain_events();
    let hp_after_cast = core.monster_hp(m).expect("rat lives");
    for _ in 0..12 {
        core.tick();
    }
    core.drain_events();
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert!(
        slots.iter().all(|slot| slot.spell.is_none()),
        "no chained entry: {slots:?}"
    );
    assert_eq!(core.monster_hp(m), Some(hp_after_cast), "no blight ever ticked");
}

// --- the poison counter (mon+0x14) ---

#[test]
fn monster_poison_hard_writes_ticks_on_the_slow_pass_and_reverses() {
    let (mut core, s, m) = setup(&[VENOMOUS]);
    core.input(s, "cast venomous touch rat");
    core.drain_events();
    // Entry hard-writes the counter set-if-greater (44072-44076).
    assert_eq!(core.monster_poison(m), Some(6));
    assert_eq!(core.monster_hp(m), Some(40), "no damage at entry");
    // The slow pass (t30) deals the counter in HP (slow_update_monster
    // 19276-19279); the duration-20 slot expires at t60 and the Poison
    // termination subtracts the stored value (floored 0).
    for _ in 0..30 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.monster_hp(m), Some(34), "poison dealt once on the slow pass");
    for _ in 0..30 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.monster_poison(m), Some(0), "termination reverses the counter");
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert!(slots.iter().all(|slot| slot.spell.is_none()));
    let _ = s;
}
