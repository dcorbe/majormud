//! Monster casting — the kind-2 attack-form dispatch, the player-side
//! saving throw and the instant-effect table (`re/docs/spellcasting.md` §6;
//! decompile `monster_cast` 22949-23785, driver cast branch 26796-26806).

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Element, MatchType, Message, MessageId, Monster,
    MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId, StatBlock,
    TargetMode,
};
use mud_core::game::{
    monster_cast_chance_passes, player_save_resists, Core, CoreConfig, Event, Gender, Player,
    SessionId,
};

const ARENA: RoomId = RoomId { map: 1, room: 1 };
const WARRIOR: ClassId = ClassId(1);
/// Plain zero-stat race: MR = (0 + 0*3)/4 = 0 — the save never resists and
/// DamageMR amplifies by the full (50-0)% band.
const HUMAN: RaceId = RaceId(1);
/// Carries (SpellImmu, 50): every fixture spell ships required_power 5 < 50,
/// so casts at this race auto-resist (decompile 23026-23029/23042).
const WARDED: RaceId = RaceId(2);
/// Carries (ImmuPoison, 1): gates the whole Poison(19) case (23387-23388).
const IRONGUT: RaceId = RaceId(3);

/// Instant fixed (Damage, 5).
const HAMMER: SpellId = SpellId(900);
/// Instant fixed (Damage, 500) — the kill probe (DEATH_FLOOR is -200).
const SLEDGE: SpellId = SpellId(901);
/// Instant fixed (DamageMR, 100) with a damage-slot castmsgb.
const MRBOLT: SpellId = SpellId(902);
/// Instant fixed (Poison, 6).
const VENOM: SpellId = SpellId(903);
/// (Damage, 5) with SaveClass::Always — the save-formula integration probe.
const SAVEBOLT: SpellId = SpellId(904);
/// Instant fixed (Drain, 50).
const LEECH: SpellId = SpellId(905);
/// Duration 100 fixed (Poison, 6) — the Task-3 marker state probe.
const LINGERING: SpellId = SpellId(906);

fn spell(id: SpellId, name: &str) -> Spell {
    Spell {
        id,
        name: name.into(),
        short_name: name[..4.min(name.len())].into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![],
        level_cap: 0,
        round_cost: 0,
        required_power: 5,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Offensive0,
        save_class: SaveClass::None,
        base_chance: 0, // the monster path never reads difficulty (+0xc8)
        duration_per_level: 0,
        match_type: MatchType::Single0,
        duration: 0,
        element: Element::Magic,
        class_gate_group: 0,
        mana_cost: 0,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

/// A kind-2 caster: one cast form, weight 100 (picked on every swing roll
/// below 100). `cost` throttles casts per round via the 1000-point pool.
fn shaman(spell_id: SpellId, cast_pct: i16, cost: i16) -> Monster {
    Monster {
        id: MonsterId(7),
        name: "kobold shaman".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 5000,
        experience: 40,
        exp_multi: 1,
        armour_class: 10,
        damage_resist: 1,
        magic_resist: 10,
        bs_defence: 0,
        energy: 1000,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [
            AttackForm {
                kind: 2,
                accuracy: spell_id.0 as i16, // spell id (template+0x12e)
                weight: 100,
                min_damage: cast_pct, // cast success % (template+0x13e)
                max_damage: 3,        // cast level (template+0x148)
                hit_msg: None,
                dodge_msg: None,
                miss_msg: None,
                energy: cost, // cast energy cost (template+0x190)
            },
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
    }
}

fn world(monster: Monster) -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: ARENA,
        name: "Arena".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_monster(monster);
    for (id, name, abilities) in [
        (HUMAN, "Human", vec![]),
        (WARDED, "Warded", vec![(Ability::SpellImmu, 50)]),
        (IRONGUT, "Irongut", vec![(Ability::ImmuPoison, 1)]),
    ] {
        content.add_race(Race {
            id,
            name: name.into(),
            abilities,
            base_stats: StatBlock::default(),
            max_stats: StatBlock::default(),
            cp: 100,
            hp_per_level: 0,
            exp_chart: 30,
        });
    }
    content.add_class(Class {
        id: WARRIOR,
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
    // MRBOLT's castmsgb: even-style target line binds (caster, spell,
    // damage), room line (caster, spell, target, damage).
    content.add_message(Message {
        id: MessageId(960),
        lines: vec![
            String::new(),
            "%s's %s hits you for %s damage!".into(),
            "%s's %s hits %s for %s damage!".into(),
        ],
    });
    let mut hammer = spell(HAMMER, "hammer");
    hammer.abilities = vec![(Ability::Damage, 5)];
    let mut sledge = spell(SLEDGE, "sledge");
    sledge.abilities = vec![(Ability::Damage, 500)];
    let mut mrbolt = spell(MRBOLT, "mrbolt");
    mrbolt.abilities = vec![(Ability::DamageMR, 100)];
    mrbolt.cast_msg_b = Some(MessageId(960));
    let mut venom = spell(VENOM, "venom");
    venom.abilities = vec![(Ability::Poison, 6)];
    let mut savebolt = spell(SAVEBOLT, "savebolt");
    savebolt.abilities = vec![(Ability::Damage, 5)];
    savebolt.save_class = SaveClass::Always;
    let mut leech = spell(LEECH, "leech");
    leech.abilities = vec![(Ability::Drain, 50)];
    let mut lingering = spell(LINGERING, "lingering venom");
    lingering.abilities = vec![(Ability::Poison, 6)];
    lingering.duration = 100;
    for s in [hammer, sledge, mrbolt, venom, savebolt, leech, lingering] {
        content.add_spell(s);
    }
    content
}

fn config() -> CoreConfig {
    CoreConfig { start_location: ARENA, ..CoreConfig::default() }
}

fn player(name: &str, race: RaceId) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race,
        class: WARRIOR,
        level: 1,
        stats: StatBlock::default(),
        base_stats: StatBlock::default(),
        hp_base: 0,
        current_hp: 200,
        current_mana: 0,
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
        location: ARENA,
        spellbook: Default::default(),
        poison: 0,
        active_spells: Default::default(),
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

fn run_rounds(core: &mut Core, n: u64) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..(n * 5) {
        core.tick();
        all.extend(core.drain_events());
    }
    all
}

/// Spawn the shaman, attach, engage (the monster locks on via the
/// retaliation mark), drain the noise.
fn engage(core: &mut Core, race: RaceId) -> (SessionId, mud_core::game::MonsterInstanceId) {
    let m = core.spawn_monster(MonsterId(7), ARENA).expect("shaman spawns");
    let s = core.attach_player(player("Dain", race));
    core.input(s, "attack shaman");
    core.drain_events();
    (s, m)
}

// --- the pure rolls ---

#[test]
fn cast_chance_is_a_strict_less_than() {
    // Decompile 22983 + 23034: genrdn(0,100) < the form's percent.
    let roll = |v: i32| move |_lo: i32, _hi: i32| v;
    assert!(monster_cast_chance_passes(65, &mut roll(64)));
    assert!(!monster_cast_chance_passes(65, &mut roll(65)));
    // A 0% form never fires, even on a floor roll.
    assert!(!monster_cast_chance_passes(0, &mut roll(0)));
    // A 100% form still loses to the inclusive top roll.
    assert!(!monster_cast_chance_passes(100, &mut roll(100)));
    assert!(monster_cast_chance_passes(100, &mut roll(99)));
}

#[test]
fn player_save_halves_mr_and_caps_at_97() {
    // Decompile 23047-23054: genrdn(1,100) <= min(mr/2, 97) resists — the
    // cap is 97 here (23048: `< 0x62 ? mr/2 : 0x61`), one BELOW the
    // player-cast path's 98 (43606-43612, `monster_save_resists`).
    let roll = |v: i32| move |_lo: i32, _hi: i32| v;
    assert!(player_save_resists(10, &mut roll(5)));
    assert!(!player_save_resists(10, &mut roll(6)));
    // Truncating halve: 11/2 = 5.
    assert!(!player_save_resists(11, &mut roll(6)));
    // The 97 cap: an astronomical MR still loses to rolls 98-100.
    assert!(player_save_resists(500, &mut roll(97)));
    assert!(!player_save_resists(500, &mut roll(98)));
    // Floor MR never resists (roll starts at 1).
    assert!(!player_save_resists(0, &mut roll(1)));
    assert!(!player_save_resists(1, &mut roll(1)));
}

// --- dispatch + lines ---

#[test]
fn a_65_percent_cast_fires_and_damages() {
    let mut core = Core::new(world(shaman(HAMMER, 65, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 10);
    let shown = text_to(&events, s);
    // No castmsgb record: the default pair (strings 00481277/0048128a),
    // first-letter capitalized like every monster_display line.
    assert!(shown.contains("Kobold shaman cast hammer on you."), "got: {shown:?}");
    let casts = shown.matches("Kobold shaman cast hammer on you.").count() as i32;
    assert!(casts > 0);
    assert_eq!(before - core.current_hp(s), casts * 5, "got: {shown:?}");
}

#[test]
fn observers_see_the_room_cast_line() {
    let mut core = Core::new(world(shaman(HAMMER, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let events = run_rounds(&mut core, 5);
    let shown = text_to(&events, watcher);
    assert!(shown.contains("Kobold shaman cast hammer on Dain."), "got: {shown:?}");
    // The victim's second-person line stays private.
    assert!(!shown.contains("on you."), "got: {shown:?}");
    let _ = s;
}

#[test]
fn a_zero_percent_cast_fizzles_with_the_attempted_line() {
    let mut core = Core::new(world(shaman(HAMMER, 0, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 5);
    let shown = text_to(&events, s);
    // Strings 00481338/00481369 — a failed chance roll is NOT silent.
    assert!(
        shown.contains("The kobold shaman attempted to cast hammer at you, but failed."),
        "got: {shown:?}"
    );
    assert!(!shown.contains("cast hammer on you."), "got: {shown:?}");
    assert_eq!(core.current_hp(s), before);
    let room = text_to(&events, watcher);
    assert!(
        room.contains("The kobold shaman attempted to cast hammer at Dain, but failed."),
        "got: {room:?}"
    );
}

// --- the saving throw ---

#[test]
fn spell_immu_auto_resists() {
    // WARDED's SpellImmu 50 beats required_power 5 (23026-23029): the
    // resist family prints, no damage lands.
    let mut core = Core::new(world(shaman(HAMMER, 101, 400)), config());
    let (s, _m) = engage(&mut core, WARDED);
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 5);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("You resisted kobold shaman's cast of hammer."),
        "got: {shown:?}"
    );
    assert!(!shown.contains("cast hammer on you."), "got: {shown:?}");
    assert_eq!(core.current_hp(s), before);
    let room = text_to(&events, watcher);
    assert!(
        room.contains("Dain resisted kobold shaman's cast of hammer."),
        "got: {room:?}"
    );
}

#[test]
fn spell_immu_resists_even_when_the_chance_roll_fails() {
    // The resist branch (23066) is NOT gated on the chance roll (bVar4):
    // an immune target sees the resist line on every attempt, never the
    // fizzle line — a faithful decompile quirk.
    let mut core = Core::new(world(shaman(HAMMER, 0, 400)), config());
    let (s, _m) = engage(&mut core, WARDED);
    let events = run_rounds(&mut core, 5);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("You resisted kobold shaman's cast of hammer."),
        "got: {shown:?}"
    );
    assert!(!shown.contains("attempted to cast"), "got: {shown:?}");
}

#[test]
fn save_class_always_never_resists_at_floor_mr() {
    // HUMAN MR is 0 → threshold min(0/2, 97) = 0; genrdn(1,100) can never
    // land at or below it, so every SAVEBOLT lands.
    let mut core = Core::new(world(shaman(SAVEBOLT, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 5);
    let shown = text_to(&events, s);
    assert!(!shown.contains("You resisted"), "got: {shown:?}");
    assert!(shown.contains("Kobold shaman cast savebolt on you."), "got: {shown:?}");
    assert!(core.current_hp(s) < before);
}

// --- instant effects ---

#[test]
fn damage_mr_scales_from_the_player_mr_but_displays_the_raw_amount() {
    // Zero-stat HUMAN: MR 0, no AntiMagic → damage_mr amplifies by
    // (50-0)% (23329-23341): 100 → 150 dealt. The display arg is the
    // PRE-scale amount (23343 passes local_8, not local_5c).
    let mut core = Core::new(world(shaman(MRBOLT, 101, 1000)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("Kobold shaman's mrbolt hits you for 100 damage!"),
        "got: {shown:?}"
    );
    let casts = shown.matches("hits you for 100 damage!").count() as i32;
    assert!(casts > 0);
    assert_eq!(before - core.current_hp(s), casts * 150, "got: {shown:?}");
}

#[test]
fn venom_sets_the_poison_counter_if_greater() {
    let mut core = Core::new(world(shaman(VENOM, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    run_rounds(&mut core, 5);
    // SET-IF-GREATER (23390-23392): repeated venom holds at 6, never adds.
    assert_eq!(core.poison(s), 6);
    // A higher existing counter survives further casts.
    core.set_poison(s, 9);
    run_rounds(&mut core, 5);
    assert_eq!(core.poison(s), 9);
}

#[test]
fn immu_poison_blocks_venom_wholesale() {
    // ImmuPoison wraps the ENTIRE case (23387-23388): no counter, no
    // success display — VENOM carries nothing else, so nothing prints.
    let mut core = Core::new(world(shaman(VENOM, 101, 400)), config());
    let (s, _m) = engage(&mut core, IRONGUT);
    let events = run_rounds(&mut core, 5);
    let shown = text_to(&events, s);
    assert_eq!(core.poison(s), 0);
    assert!(!shown.contains("cast venom on you."), "got: {shown:?}");
}

#[test]
fn drain_damages_the_player_and_heals_the_monster() {
    let mut core = Core::new(world(shaman(LEECH, 101, 400)), config());
    let (s, m) = engage(&mut core, HUMAN);
    core.set_current_hp(s, 500); // 2 casts/round x 2 rounds stays alive
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 2);
    let shown = text_to(&events, s);
    let casts = shown.matches("Kobold shaman cast leech on you.").count() as i32;
    assert!(casts > 0, "got: {shown:?}");
    assert_eq!(before - core.current_hp(s), casts * 50);
    // Drain adds the stolen HP back, capped at the template max (23212-
    // 23215): despite the player's melee chipping away, the shaman sits
    // at (or within one uncompensated swing of) full.
    assert!(core.monster_hp(m) >= Some(4990), "got: {:?}", core.monster_hp(m));
}

#[test]
fn a_big_hit_routes_through_the_death_path() {
    let mut core = Core::new(world(shaman(SLEDGE, 101, 1000)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    core.set_current_hp(s, 10); // 10 - 500 <= DEATH_FLOOR (-200)
    let events = run_rounds(&mut core, 3);
    let shown = text_to(&events, s);
    assert!(shown.contains("Kobold shaman cast sledge on you."), "got: {shown:?}");
    assert!(shown.contains("You have been killed!"), "got: {shown:?}");
}

// --- duration payloads (Task-3 marker state) ---

#[test]
fn duration_casts_print_but_enter_no_slot_yet() {
    // Task 3 lands monster_add_cast_spell_to_user (exceed-only refresh,
    // fixed duration, poison hard-write at entry). Until then the fan-out
    // prints and the slots/counter stay untouched — this test pins the
    // marker state and FLIPS when Task 3 lands.
    let mut core = Core::new(world(shaman(LINGERING, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 5);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("Kobold shaman cast lingering venom on you."),
        "got: {shown:?}"
    );
    assert_eq!(core.poison(s), 0);
    let snapshot = core.player_snapshot(s);
    assert!(snapshot.active_spells.iter().all(|slot| slot.spell.is_none()));
}

// --- form selection ---

#[test]
fn melee_and_cast_forms_share_the_weight_table() {
    // Form 0: melee, cumulative threshold 50; form 1: cast, threshold 100.
    // Both fire across rounds — the kind-2 arm does not disturb the
    // cumulative pick (26784-26794).
    let mut monster = shaman(HAMMER, 101, 100);
    monster.attacks[0] = AttackForm {
        kind: 1,
        accuracy: 200,
        weight: 50,
        min_damage: 1,
        max_damage: 2,
        hit_msg: Some(MessageId(970)),
        dodge_msg: None,
        miss_msg: None,
        energy: 10,
    };
    monster.attacks[1] = AttackForm {
        kind: 2,
        accuracy: HAMMER.0 as i16,
        weight: 100,
        min_damage: 101,
        max_damage: 3,
        hit_msg: None,
        dodge_msg: None,
        miss_msg: None,
        energy: 100,
    };
    let mut content = world(monster);
    content.add_message(Message {
        id: MessageId(970),
        lines: vec![
            "The %s claws you for %d damage!".into(),
            "The %s claws %s for %s damage!".into(),
            "The kobold shaman collapses.".into(),
        ],
    });
    let mut core = Core::new(content, config());
    let (s, _m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 10);
    let shown = text_to(&events, s);
    assert!(shown.contains("claws you"), "melee form starved: {shown:?}");
    assert!(
        shown.contains("Kobold shaman cast hammer on you."),
        "cast form starved: {shown:?}"
    );
}
