//! Monster casting — the kind-2 attack-form dispatch, the player-side
//! saving throw and the instant-effect table (`re/docs/spellcasting.md` §6;
//! decompile `monster_cast` 22949-23785, driver cast branch 26796-26806),
//! plus the room-wide sibling `monster_cast_area` (22037-22943) that every
//! match ∉ {0,2,6,8} form routes into (23777-23779).
//! The hit fan-out (victim castmsgb line with damage, room line per the
//! record — typically without) is MEASURED §8.14 — the dragonfish steam
//! (359, match 12) line was the AREA path with one occupant; the resist
//! and fizzle families remain decompile-only (every §8.14-reachable caster
//! carried a 100% form and never rolled a resist).

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Direction, Element, Exit, MatchType, Message,
    MessageId, Monster, MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell,
    SpellId, StatBlock, TargetMode,
};
use mud_core::game::{
    monster_cast_chance_passes, player_save_resists, ActiveSpell, Core, CoreConfig, Event,
    Gender, Player, SessionId,
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
/// Duration 100 fixed (Poison, 6) + DescMsg — the slot-entry probe.
const LINGERING: SpellId = SpellId(906);
/// Duration 2 fixed (Poison, 6) + DescMsg — the expiry/wear-off probe.
const FLEETING: SpellId = SpellId(907);
/// Benign-MODE (spelltype 3) match-0 duration debuff — the mummy
/// `breathes` (84) shape: routing keys on MATCH, not mode (23015-23016).
const BREATH: SpellId = SpellId(908);
/// Match-12 (AreaC) offensive (DamageMR 30-30, castmsgb 960) — the live
/// `monster_cast_area` probe, the dragonfish 359 shape (§8.14): rolled
/// magnitude, undivided per target, post-MR amount in the display.
const GUST: SpellId = SpellId(909);
/// Instant fixed (Summon, 8 = RAPTOR) — the spawn + "everyone" line probe
/// (case 0xc, 23251-23267).
const SUMMONER: SpellId = SpellId(910);
/// Duration 40 fixed (Drain, 50) + (Summon, 8) — both cases are gated
/// `local_28 == 0` with NO else arm (23207-23229 / 23251-23267): dead rows
/// in a duration cast.
const DURDEAD: SpellId = SpellId(911);
/// Duration 40 (DamageMR, 100) + (Poison, 6) — DamageMR carries NO
/// duration gate (23305-23356): instant damage mid-duration-cast.
const MRVENOM: SpellId = SpellId(912);
/// Duration 50 fixed (Intel, 10) — the stat family (44-49, cases
/// 0x2c-0x31) enters with the NO-REFRESH flag (23436-23477).
const SAPMIND: SpellId = SpellId(913);
/// Duration 30 fixed (Fear, 200) — the recurring flee (44788-44793).
const PANIC: SpellId = SpellId(914);
/// Match-12 instant (Damage, 5) — the DEAD instant-Damage row probe:
/// monster_cast_area case 1 damages players only for match 5/10/13
/// (22212-22218), so a match-12 Damage row does nothing (the shipped
/// flesh-eating gas 766 / hail of stones 772 quirk).
const DUDGAS: SpellId = SpellId(915);
/// Match-5 instant (Damage, 0) 12-12 — the split-magnitude probe: the
/// whole-cast roll divides by the valid-target count (22151-22160).
const SPRAY: SpellId = SpellId(916);
/// Match-12 duration 20 (AC, 5) 7-7 — the room-table entry probe: the
/// entered value is the ROLLED magnitude, never the row value
/// (monster_add_duration_spell_to_room passes local_1c, 21947-21949).
const MIST: SpellId = SpellId(917);
/// MIST plus (RemovesSpell, BREATH) — the dispel pre-pass probe: the
/// dispelled victim loses the 0x10 target flag (22224) and drops out of
/// the rest of the sweep.
const FOG: SpellId = SpellId(918);
/// Match-1 (Single1) duration 15 (Picklocks, 200) — the hooded man
/// `blacknight` 1220 shape: the {1,2,4,6} group inside the area sibling
/// self-slots the MONSTER (FUN_004262d6, the 5-slot table at +0x14a).
const NIGHT: SpellId = SpellId(919);
/// Match-12 instant (Drain, 0) 20-20 — the shipped `drains` 389 shape.
const SIPHON: SpellId = SpellId(920);
/// GUST with SaveClass::Always — the per-target save-gate probe.
const SAVEGUST: SpellId = SpellId(921);
/// The Summon payload template.
const RAPTOR: MonsterId = MonsterId(8);
/// One valid exit north of ARENA — the fear-flee destination.
const LAIR: RoomId = RoomId { map: 1, room: 2 };

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
        ..Default::default()
    }
}

fn world(monster: Monster) -> Content {
    let mut content = Content::default();
    let mut arena = Room {
        id: ARENA,
        name: "Arena".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    arena.exits[Direction::North as usize] =
        Some(Exit { dest: LAIR, exit_type: 0, trigger_msg: None, ..Default::default() });
    content.add_room(arena);
    content.add_room(Room {
        id: LAIR,
        name: "Lair".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    content.add_monster(monster);
    // The Summon(12) payload: an inert template (no attack forms).
    content.add_monster(Monster {
        id: RAPTOR,
        name: "raptor".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 30,
        experience: 10,
        exp_multi: 1,
        armour_class: 5,
        damage_resist: 0,
        magic_resist: 0,
        bs_defence: 0,
        energy: 0,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [AttackForm::default(); 5],
        ..Default::default()
    });
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
    // The venom-family DescMsg record (the shipped 8575 model): line1 the
    // wear-off, line3 the active line the victim sees at entry.
    content.add_message(Message {
        id: MessageId(961),
        lines: vec![
            "The effects of the poison wear off!".into(),
            String::new(),
            "You feel ill.".into(),
        ],
    });
    // A StartMsg record: victim line2 binds the caster name, room line3
    // binds (caster, victim) — monster_display_spell_success 21692-21700.
    content.add_message(Message {
        id: MessageId(962),
        lines: vec![
            String::new(),
            "The %s exhales a rotting wind!".into(),
            "The %s exhales a rotting wind at %s!".into(),
        ],
    });
    let mut lingering = spell(LINGERING, "lingering venom");
    lingering.abilities = vec![(Ability::Poison, 6), (Ability::DescMsg, 961)];
    lingering.duration = 100;
    let mut fleeting = spell(FLEETING, "fleeting venom");
    fleeting.abilities = vec![(Ability::Poison, 6), (Ability::DescMsg, 961)];
    fleeting.duration = 2;
    let mut breath = spell(BREATH, "decay breath");
    // Mummy `breathes` (84) shape: MODE 3 (benign) but MATCH 0 — the DLL
    // single-target gate reads only the match type, and mode >= 3 merely
    // skips the elemental-resist scale (23091-23117).
    breath.target_mode = TargetMode::Benign;
    breath.abilities = vec![(Ability::AC, -5), (Ability::StartMsg, 962)];
    breath.duration = 20;
    let mut gust = spell(GUST, "choking gust");
    gust.abilities = vec![(Ability::DamageMR, 0)];
    gust.match_type = MatchType::AreaC;
    gust.min_base = 30;
    gust.max_base = 30;
    gust.cast_msg_b = Some(MessageId(960));
    let mut summoner = spell(SUMMONER, "summon pet");
    summoner.abilities = vec![(Ability::Summon, RAPTOR.0 as i16)];
    let mut durdead = spell(DURDEAD, "grasping shadows");
    durdead.abilities = vec![(Ability::Drain, 50), (Ability::Summon, RAPTOR.0 as i16)];
    durdead.duration = 40;
    let mut mrvenom = spell(MRVENOM, "searing venom");
    mrvenom.abilities = vec![(Ability::DamageMR, 100), (Ability::Poison, 6)];
    mrvenom.duration = 40;
    let mut sapmind = spell(SAPMIND, "sap mind");
    sapmind.abilities = vec![(Ability::Intel, 10)];
    sapmind.duration = 50;
    let mut panic = spell(PANIC, "panic");
    panic.abilities = vec![(Ability::Fear, 200)];
    panic.duration = 30;
    let mut dudgas = spell(DUDGAS, "dud gas");
    dudgas.abilities = vec![(Ability::Damage, 5)];
    dudgas.match_type = MatchType::AreaC;
    let mut spray = spell(SPRAY, "spray");
    spray.abilities = vec![(Ability::Damage, 0)];
    spray.match_type = MatchType::Area5;
    spray.min_base = 12;
    spray.max_base = 12;
    let mut mist = spell(MIST, "mist");
    mist.abilities = vec![(Ability::AC, 5)];
    mist.match_type = MatchType::AreaC;
    mist.duration = 20;
    mist.min_base = 7;
    mist.max_base = 7;
    let mut fog = spell(FOG, "fog");
    // The RemovesSpell row sits AFTER the AC row: the pre-pass hoists it
    // ahead of the whole effect loop (22180-22233), like solid fog 256.
    fog.abilities = vec![(Ability::AC, 5), (Ability::RemovesSpell, BREATH.0 as i16)];
    fog.match_type = MatchType::AreaC;
    fog.duration = 20;
    fog.min_base = 7;
    fog.max_base = 7;
    let mut night = spell(NIGHT, "blacknight");
    night.abilities = vec![(Ability::Picklocks, 200)];
    night.match_type = MatchType::Single1;
    night.duration = 15;
    let mut siphon = spell(SIPHON, "siphon");
    siphon.abilities = vec![(Ability::Drain, 0)];
    siphon.match_type = MatchType::AreaC;
    siphon.min_base = 20;
    siphon.max_base = 20;
    let mut savegust = spell(SAVEGUST, "savegust");
    savegust.abilities = vec![(Ability::DamageMR, 0)];
    savegust.match_type = MatchType::AreaC;
    savegust.min_base = 30;
    savegust.max_base = 30;
    savegust.save_class = SaveClass::Always;
    for s in [
        hammer, sledge, mrbolt, venom, savebolt, leech, lingering, fleeting, breath, gust,
        summoner, durdead, mrvenom, sapmind, panic, dudgas, spray, mist, fog, night, siphon,
        savegust,
    ] {
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

// --- duration payloads: the 10-slot entry (Task 3) ---
// monster_add_cast_spell_to_user (decompile 21777-21816): refresh only if
// the new value EXCEEDS the stored one, fixed duration, display only on a
// real write; the caller refunds the FULL energy cost and aborts the cast
// when the entry returns a failure (monster_cast 23183-23188).

#[test]
fn duration_casts_enter_the_slot_and_hard_write_poison() {
    let mut core = Core::new(world(shaman(LINGERING, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    // Display fires from inside the successful entry: castmsgb fallback
    // pair + the DescMsg active line3 to the victim (21692-21716).
    assert!(
        shown.contains("Kobold shaman cast lingering venom on you."),
        "got: {shown:?}"
    );
    assert!(shown.contains("You feel ill."), "got: {shown:?}");
    // Poison hard-writes at entry (23404-23407, set-if-greater).
    assert_eq!(core.poison(s), 6);
    let snapshot = core.player_snapshot(s);
    let slot = snapshot
        .active_spells
        .iter()
        .find(|slot| slot.spell == Some(LINGERING))
        .expect("lingering venom occupies a slot");
    assert_eq!(slot.value, 6);
    assert!(slot.remaining > 90, "fixed duration 100, got {}", slot.remaining);
}

#[test]
fn expiry_terminates_and_reverses_the_poison() {
    // FLEETING (duration 2): t5 energy round casts (slot enters, poison
    // 6), upkeep t6/t9 decrement to 0 → the player termination path
    // reverses the hard write and prints the DescMsg wear-off line1.
    let mut core = Core::new(world(shaman(FLEETING, 101, 1000)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let mut events = Vec::new();
    for _ in 0..9 {
        core.tick();
        events.extend(core.drain_events());
    }
    let shown = text_to(&events, s);
    assert!(shown.contains("You feel ill."), "entry line: {shown:?}");
    assert!(
        shown.contains("The effects of the poison wear off!"),
        "wear-off line: {shown:?}"
    );
    assert_eq!(core.poison(s), 0, "termination subtracts the stored value");
    let snapshot = core.player_snapshot(s);
    assert!(snapshot.active_spells.iter().all(|slot| slot.spell != Some(FLEETING)));
}

#[test]
fn refresh_is_rejected_when_the_stored_value_is_not_exceeded() {
    // Stored 9 >= new 6: monster_add_cast_spell_to_user returns -2
    // (21796-21802) — no write, no display, and the monster gets the FULL
    // energy cost back (23186-23188), aborting before the poison write.
    let mut core = Core::new(world(shaman(LINGERING, 101, 400)), config());
    let (s, m) = engage(&mut core, HUMAN);
    core.set_active_spell(
        s,
        0,
        mud_core::game::ActiveSpell { spell: Some(LINGERING), value: 9, remaining: 50 },
    );
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(!shown.contains("cast lingering venom on you."), "got: {shown:?}");
    assert!(!shown.contains("You feel ill."), "got: {shown:?}");
    assert_eq!(core.poison(s), 0, "the abort precedes the poison hard-write");
    let slot = core.player_snapshot(s).active_spells[0];
    assert_eq!(slot.value, 9, "stored value survives");
    assert_eq!(core.monster_energy(m), Some(1000), "full refund nets zero");
}

#[test]
fn refresh_overwrites_when_the_new_value_is_greater() {
    // Stored 3 < new 6: value AND duration refresh in place (21795-21800)
    // and the success display fires like a first entry.
    let mut core = Core::new(world(shaman(LINGERING, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    core.set_active_spell(
        s,
        0,
        mud_core::game::ActiveSpell { spell: Some(LINGERING), value: 3, remaining: 5 },
    );
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(shown.contains("cast lingering venom on you."), "got: {shown:?}");
    let slot = core.player_snapshot(s).active_spells[0];
    assert_eq!(slot.spell, Some(LINGERING));
    assert_eq!(slot.value, 6);
    assert!(slot.remaining > 50, "duration reset to the fixed 100, got {}", slot.remaining);
    assert_eq!(core.poison(s), 6, "the hard write follows the successful entry");
}

#[test]
fn a_full_slot_table_loses_the_cast_and_refunds_the_energy() {
    // Both scans exhausted → -1 (21813-21816): effect lost, nothing
    // prints anywhere, full refund (23183-23188). The ten fillers use
    // unknown spell ids: upkeep idles them (get_spell_data gate), so the
    // table stays saturated across the run.
    let mut core = Core::new(world(shaman(LINGERING, 101, 400)), config());
    let (s, m) = engage(&mut core, HUMAN);
    for idx in 0..10 {
        core.set_active_spell(
            s,
            idx,
            mud_core::game::ActiveSpell {
                spell: Some(SpellId(800 + idx as u16)),
                value: 1,
                remaining: 1000,
            },
        );
    }
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(!shown.contains("lingering"), "got: {shown:?}");
    assert_eq!(core.poison(s), 0);
    let snapshot = core.player_snapshot(s);
    assert!(snapshot.active_spells.iter().all(|slot| slot.spell != Some(LINGERING)));
    assert_eq!(core.monster_energy(m), Some(1000), "full refund nets zero");
}

#[test]
fn benign_mode_single_match_forms_take_the_single_target_path() {
    // Routing keys on MATCH {0,2,6,8} alone (23015-23016) — the mummy's
    // `breathes` (84) is match 0 with MODE 3, and still slots its debuff
    // at the victim. The StartMsg prelude prints around the castmsgb pair.
    let mut core = Core::new(world(shaman(BREATH, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("The kobold shaman exhales a rotting wind!"),
        "StartMsg victim line: {shown:?}"
    );
    assert!(shown.contains("cast decay breath on you."), "got: {shown:?}");
    let snapshot = core.player_snapshot(s);
    let slot = snapshot
        .active_spells
        .iter()
        .find(|slot| slot.spell == Some(BREATH))
        .expect("decay breath occupies a slot");
    assert_eq!(slot.value, -5, "the fixed row value is stored");
}

// --- monster_cast_area: the match ∉ {0,2,6,8} sibling (22037-22943) ---
// Iteration is PLAYERS ONLY: monster_count_valid_targets (21827-21886)
// sweeps the terminal list for players in the monster's room, rolls the
// per-target save THERE (before energy/fizzle), and its monster counter
// out-param is never incremented — monsters are never area victims.

#[test]
fn area_save_is_save_class_keyed_per_target() {
    // The save lives in monster_count_valid_targets (21850-21868), rolled
    // once per player: class None never rolls, Always always rolls,
    // IfAntiMagic rolls only when THAT target carries AntiMagic (51) —
    // the same player_save_resists formula with the 97 cap. A saved
    // player is excluded SILENTLY (no resist line anywhere in the area
    // path). SpellImmu (139) is never consulted — unlike the single path.
    use mud_core::game::monster_area_target_saves;
    let roll = |v: i32| move |_lo: i32, _hi: i32| v;
    assert!(!monster_area_target_saves(SaveClass::None, false, 500, &mut roll(1)));
    assert!(monster_area_target_saves(SaveClass::Always, false, 500, &mut roll(97)));
    assert!(!monster_area_target_saves(SaveClass::Always, false, 500, &mut roll(98)));
    assert!(!monster_area_target_saves(SaveClass::Always, false, 0, &mut roll(1)));
    assert!(monster_area_target_saves(SaveClass::IfAntiMagic, true, 200, &mut roll(50)));
    assert!(!monster_area_target_saves(SaveClass::IfAntiMagic, false, 200, &mut roll(50)));
}

#[test]
fn area_cast_fans_out_to_every_player_in_the_room() {
    // One cast, one magnitude roll (22148-22150), EVERY player in the
    // room hit with the undivided (match-12) amount, one victim line
    // each (the local_25 latch trips only after the row's player loop,
    // 22637) — and the energy cost charged ONCE per cast, not per
    // victim (22128-22131).
    let mut core = Core::new(world(shaman(GUST, 101, 400)), config());
    let (s, m) = engage(&mut core, HUMAN);
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let events = run_rounds(&mut core, 1);
    let dain = text_to(&events, s);
    let seen = text_to(&events, watcher);
    let c = dain.matches("Kobold shaman's choking gust hits you for ").count() as i32;
    assert!(c > 0, "got: {dain:?}");
    assert_eq!(
        seen.matches("Kobold shaman's choking gust hits you for ").count() as i32,
        c,
        "both players hit per cast: {seen:?}"
    );
    // Each also sees the OTHER's room line.
    assert!(dain.contains("choking gust hits Onlooker for "), "got: {dain:?}");
    assert!(seen.contains("choking gust hits Dain for "), "got: {seen:?}");
    // Rolled 30-30 spans {30,31} (genrdn inclusive), MR 0 amplifies +50%:
    // 45 or 46 dealt — the SAME roll for both victims (one roll per cast).
    let dain_loss = 200 - core.current_hp(s);
    assert!(45 * c <= dain_loss && dain_loss <= 46 * c, "got {dain_loss} over {c}");
    assert_eq!(200 - core.current_hp(watcher), dain_loss, "shared per-cast roll");
    assert_eq!(core.monster_energy(m), Some(1000 - c * 400), "charged once per cast");
}

#[test]
fn area_cast_single_occupant_reproduces_the_dragonfish_shape() {
    // §8.14 reconciliation: the dragonfish's `breathes burning steam`
    // (359, match 12, DamageMR) IS an area cast — the measured
    // single-victim line (`The fat dragonfish breathes burning steam on
    // you for 27 damage!`) is the room-wide sweep with exactly one
    // occupant. The display amount is the POST-MR-scale dealt value
    // (22624-22628 passes uVar11) — the single-target path shows the
    // PRE-scale amount instead (23343), a genuine asymmetry.
    let mut core = Core::new(world(shaman(GUST, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    let hits: Vec<i32> = shown
        .lines()
        .filter_map(|l| {
            l.trim_start_matches("\r\x1b[K") // the async prompt-erase prefix
                .strip_prefix("Kobold shaman's choking gust hits you for ")
                .and_then(|rest| rest.strip_suffix(" damage!"))
                .and_then(|n| n.parse().ok())
        })
        .collect();
    assert!(!hits.is_empty(), "got: {shown:?}");
    // Post-scale: 30-31 rolled, +50% at MR 0 → 45/46 shown AND dealt.
    assert!(hits.iter().all(|d| *d == 45 || *d == 46), "got: {hits:?}");
    assert_eq!(200 - core.current_hp(s), hits.iter().sum::<i32>());
}

#[test]
fn area_split_matches_divide_the_roll_by_the_target_count() {
    // Match 5 divides the whole-cast roll by the valid-target count
    // (22151-22160: default/3/5/9/10 divide, 11/12/13 do not) — and its
    // instant Damage(1) row is LIVE (match 5 is in the 22212 filter).
    // 12-12 rolls {12,13}; both divide to 6 across two targets.
    let mut core = Core::new(world(shaman(SPRAY, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let events = run_rounds(&mut core, 1);
    let dain = text_to(&events, s);
    let c = dain.matches("Kobold shaman cast spray on you.").count() as i32;
    assert!(c > 0, "got: {dain:?}");
    assert_eq!(200 - core.current_hp(s), 6 * c, "12-13 total / 2 targets = 6");
    assert_eq!(200 - core.current_hp(watcher), 6 * c);
}

#[test]
fn match_twelve_instant_damage_rows_are_dead() {
    // Case 1's instant player loop hits only match 5/10/13 (22212-22218):
    // a match-12 (Damage, dur 0) row iterates nobody, prints nothing —
    // but the cast itself LANDED, so the full energy cost stays charged
    // (22128-22131) and the display latch still trips (22268). This is
    // the shipped flesh-eating gas 766 / icy breath 895 / hail of stones
    // 772 quirk: those casters deal no damage at all in the DLL.
    let mut core = Core::new(world(shaman(DUDGAS, 101, 400)), config());
    let (s, m) = engage(&mut core, HUMAN);
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 2);
    let shown = text_to(&events, s);
    assert!(!shown.contains("dud"), "got: {shown:?}");
    assert_eq!(core.current_hp(s), before);
    let energy = core.monster_energy(m).unwrap();
    assert!(energy < 1000, "the cast fired and charged");
    assert_eq!((1000 - energy) % 400, 0, "full cost per landed cast, got {energy}");
}

#[test]
fn area_fizzle_is_silent_and_charges_half() {
    // The area fizzle branch (22910-22929) charges cost/2 floored at 1
    // and prints NOTHING — no "attempted to cast" pair, unlike the
    // single-target path (23749-23757).
    let mut core = Core::new(world(shaman(GUST, 0, 400)), config());
    let (s, m) = engage(&mut core, HUMAN);
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(!shown.contains("gust"), "silent fizzle: {shown:?}");
    assert!(!shown.contains("attempted to cast"), "no single-path pair: {shown:?}");
    assert_eq!(core.current_hp(s), before);
    let energy = core.monster_energy(m).unwrap();
    assert!(energy < 1000, "half charge per fizzle");
    assert_eq!((1000 - energy) % 200, 0, "cost 400 → 200 per fizzle, got {energy}");
}

#[test]
fn area_casts_ignore_spell_immu() {
    // monster_count_valid_targets consults ONLY the save class + MR roll
    // (21850-21868) — no SpellImmu (139) read anywhere in the area path.
    // WARDED (SpellImmu 50 > required_power 5) auto-resists every SINGLE
    // cast, yet the area sweep hits it like anyone else.
    let mut core = Core::new(world(shaman(GUST, 101, 400)), config());
    let (s, _m) = engage(&mut core, WARDED);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(!shown.contains("resisted"), "got: {shown:?}");
    assert!(shown.contains("choking gust hits you for "), "got: {shown:?}");
    assert!(core.current_hp(s) < 200);
}

#[test]
fn area_save_class_always_never_excludes_floor_mr() {
    // The integration twin of the pure-fn test: MR 0 can never make the
    // genrdn(1,100) <= min(0/2, 97) save, so every SAVEGUST cast still
    // sweeps the zero-stat human in.
    let mut core = Core::new(world(shaman(SAVEGUST, 101, 400)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    run_rounds(&mut core, 1);
    assert!(core.current_hp(s) < 200);
}

#[test]
fn area_duration_enters_each_victims_slots_with_the_rolled_value() {
    // Duration rows route through monster_add_duration_spell_to_room
    // (21892-21965): every flagged player gets ONE slot entry holding
    // the ROLLED magnitude — the (AC, 5) row value is ignored (the room
    // path passes local_1c, never the row override) — with the RAW
    // spell duration word as the remaining ticks, refresh-if-greater.
    let mut core = Core::new(world(shaman(MIST, 101, 1000)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let events = run_rounds(&mut core, 1);
    for who in [s, watcher] {
        let shown = text_to(&events, who);
        assert!(shown.contains("cast mist on you."), "entry display: {shown:?}");
        let snapshot = core.player_snapshot(who);
        let slot = snapshot
            .active_spells
            .iter()
            .find(|slot| slot.spell == Some(MIST))
            .expect("mist occupies a slot");
        assert!(slot.value == 7 || slot.value == 8, "rolled, not the row 5: {slot:?}");
        assert!(slot.remaining >= 17, "raw duration 20, got {}", slot.remaining);
    }
}

#[test]
fn area_duration_first_entry_failure_aborts_without_refund() {
    // monster_add_duration_spell_to_room returns 0 when an entry fails
    // before ANY success (21952-21956), and monster_cast_area then
    // aborts the whole cast (22347-22352) — with NO energy refund,
    // unlike the single-target path's full refund (23183-23188). Dain
    // (first terminal) has a full table: the room entry dies on him and
    // the clean Onlooker gets nothing.
    let mut core = Core::new(world(shaman(MIST, 101, 1000)), config());
    let (s, m) = engage(&mut core, HUMAN);
    for idx in 0..10 {
        core.set_active_spell(
            s,
            idx,
            ActiveSpell { spell: Some(SpellId(800 + idx as u16)), value: 1, remaining: 1000 },
        );
    }
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let events = run_rounds(&mut core, 1);
    assert!(!text_to(&events, s).contains("mist"), "silent abort");
    assert!(!text_to(&events, watcher).contains("mist"), "silent abort");
    let slots = core.player_snapshot(watcher).active_spells;
    assert!(slots.iter().all(|sl| sl.spell.is_none()), "abort precedes Onlooker: {slots:?}");
    assert_eq!(core.monster_energy(m), Some(0), "cost stays paid — no refund");
}

#[test]
fn area_duration_failure_after_a_success_is_tolerated() {
    // A failed entry AFTER a success is skipped, not fatal (21952-21959:
    // local_d already set). Dain (first) enters; the saturated Onlooker
    // silently loses out; the cast completes.
    let mut core = Core::new(world(shaman(MIST, 101, 1000)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    for idx in 0..10 {
        core.set_active_spell(
            watcher,
            idx,
            ActiveSpell { spell: Some(SpellId(800 + idx as u16)), value: 1, remaining: 1000 },
        );
    }
    core.drain_events();
    let events = run_rounds(&mut core, 1);
    assert!(text_to(&events, s).contains("cast mist on you."), "Dain entered");
    let snapshot = core.player_snapshot(s);
    assert!(snapshot.active_spells.iter().any(|sl| sl.spell == Some(MIST)));
    let slots = core.player_snapshot(watcher).active_spells;
    assert!(slots.iter().all(|sl| sl.spell != Some(MIST)), "Onlooker full: {slots:?}");
}

#[test]
fn area_dispel_removes_the_named_spell_and_drops_the_victim_from_the_sweep() {
    // The RemovesSpell/KillSpell pre-pass (22180-22233) runs BEFORE the
    // effect loop regardless of row order: every flagged player's slots
    // are scanned for the named spell; a removal terminates it AND
    // clears the player's 0x10 target flag (22224) — the dispelled
    // victim drops out of the rest of the cast (solid fog 256's shape:
    // a blurred player gets dispelled but never slowed).
    let mut core = Core::new(world(shaman(FOG, 101, 1000)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    core.set_active_spell(s, 0, ActiveSpell { spell: Some(BREATH), value: -5, remaining: 50 });
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let events = run_rounds(&mut core, 1);
    // Dain: BREATH dispelled (the cast spell's success line displays the
    // removal, 22203-22209), and NO fog entry follows.
    assert!(text_to(&events, s).contains("cast fog on you."), "dispel display");
    let dain_slots = core.player_snapshot(s).active_spells;
    assert!(dain_slots.iter().all(|sl| sl.spell != Some(BREATH)), "dispelled: {dain_slots:?}");
    assert!(dain_slots.iter().all(|sl| sl.spell != Some(FOG)), "dropped: {dain_slots:?}");
    // Onlooker keeps the flag and takes the fog entry.
    let snapshot = core.player_snapshot(watcher);
    assert!(snapshot.active_spells.iter().any(|sl| sl.spell == Some(FOG)));
}

#[test]
fn match_one_forms_self_slot_the_monster() {
    // Match {1,2,4,6} inside monster_cast_area is the SELF group: a
    // duration row enters the MONSTER's own 5-slot table (FUN_004262d6,
    // 21970-22005) silently — the hooded man's blacknight 1220
    // (Picklocks 200, dur 15) makes him a lockpicker, not an attacker.
    let mut core = Core::new(world(shaman(NIGHT, 101, 1000)), config());
    let (s, m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(!shown.contains("blacknight"), "silent self-slot: {shown:?}");
    let slots = core.monster_active_spells(m).expect("monster alive");
    let slot = slots
        .iter()
        .find(|sl| sl.spell == Some(NIGHT))
        .expect("blacknight in the monster table");
    assert_eq!(slot.value, 200, "the row value, not the roll");
    assert!(slot.remaining >= 13, "duration 15, got {}", slot.remaining);
    let player_slots = core.player_snapshot(s).active_spells;
    assert!(player_slots.iter().all(|sl| sl.spell.is_none()), "no player entry");
    assert_eq!(core.monster_energy(m), Some(0), "the cast still charges");
}

#[test]
fn area_drain_bleeds_the_room_into_the_monster() {
    // Case 8 (22357-22434): every flagged player loses the per-target
    // amount and the monster gains it, capped at the template max —
    // the shipped `drains` 389 shape (fisher hulk et al).
    let mut core = Core::new(world(shaman(SIPHON, 101, 400)), config());
    let (s, m) = engage(&mut core, HUMAN);
    core.set_current_hp(s, 500);
    let watcher = core.attach_player(player("Onlooker", HUMAN));
    core.drain_events();
    let events = run_rounds(&mut core, 1);
    let dain = text_to(&events, s);
    let c = dain.matches("Kobold shaman cast siphon on you.").count() as i32;
    assert!(c > 0, "got: {dain:?}");
    let dain_loss = 500 - core.current_hp(s);
    assert!(20 * c <= dain_loss && dain_loss <= 21 * c, "got {dain_loss} over {c}");
    assert_eq!(200 - core.current_hp(watcher), dain_loss, "same roll, both bled");
    assert!(core.monster_hp(m) >= Some(4990), "got: {:?}", core.monster_hp(m));
}

// --- energy accounting (the 23041 gate; 23123 full; 23758-23770 half) ---

#[test]
fn a_fizzled_cast_charges_half_the_form_energy_floored_at_one() {
    // A failed chance roll pays cost/2, floored at 1 for a nonzero cost
    // (23758-23770): {1,2,3} all charge 1, 100 charges 50. One energy
    // round refills to the 1000 template max, then the swings spend —
    // read after the round, before the next refill.
    for (cost, charge) in [(1i16, 1i32), (2, 1), (3, 1), (100, 50)] {
        let mut core = Core::new(world(shaman(HAMMER, 0, cost)), config());
        let (s, m) = engage(&mut core, HUMAN);
        let events = run_rounds(&mut core, 1);
        let shown = text_to(&events, s);
        let fizzles =
            shown.matches("attempted to cast hammer at you, but failed.").count() as i32;
        assert!(fizzles > 0, "cost {cost}: no fizzles: {shown:?}");
        assert_eq!(
            core.monster_energy(m),
            Some(1000 - fizzles * charge),
            "cost {cost} charges {charge} per fizzle ({fizzles} fizzles)"
        );
    }
}

#[test]
fn a_landed_cast_charges_the_full_form_energy() {
    // Landing pays the whole cost word (23123). 1000-point pool at cost
    // 400: after the second cast 200 < 400, and the remaining swings are
    // silent skips — never a partial or half charge.
    let mut core = Core::new(world(shaman(HAMMER, 101, 400)), config());
    let (s, m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    let casts = shown.matches("Kobold shaman cast hammer on you.").count() as i32;
    assert!(casts > 0, "got: {shown:?}");
    assert_eq!(core.monster_energy(m), Some(1000 - casts * 400));
}

#[test]
fn an_unaffordable_cast_is_a_silent_skip_before_the_resist_ladder() {
    // The ENTIRE cast — chance roll, SpellImmu resist, every line, all
    // charging — sits inside the energy gate (23041 wraps 23042-23770).
    // A WARDED (SpellImmu 50) victim proves the ordering: were the
    // resist block outside the gate, the resist family would print.
    let mut monster = shaman(HAMMER, 101, 400);
    monster.energy = 300; // pool max/regen 300: the 400 cast never affords
    let mut core = Core::new(world(monster), config());
    let (s, m) = engage(&mut core, WARDED);
    let events = run_rounds(&mut core, 3);
    let shown = text_to(&events, s);
    assert!(!shown.contains("hammer"), "got: {shown:?}");
    assert!(!shown.contains("resisted"), "got: {shown:?}");
    assert_eq!(core.monster_energy(m), Some(300), "nothing charged");
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

// --- Task 5: Summon / Fear / AlterSpDmg + the duration-arm exceptions ---

#[test]
fn summon_spawns_the_named_monster_with_the_everyone_line() {
    let mut core = Core::new(world(shaman(SUMMONER, 101, 600)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    // monster_display_spell_success(-1, ..., "everyone", v) at 23255-23258:
    // usernum -1 skips the victim line, tell_room excludes nobody — ONE
    // room-wide line with "everyone" in the target slot, victim included.
    assert!(
        shown.contains("Kobold shaman cast summon pet on everyone."),
        "got: {shown:?}"
    );
    assert!(!shown.contains("on you."), "no victim-private line: {shown:?}");
    core.input(s, "look");
    let events = core.drain_events();
    let look = text_to(&events, s);
    assert!(look.contains("raptor"), "the raptor stands in the room: {look:?}");
    // The monster-cast summon pre-locks the VICTIM (23263: mon+0x1a =
    // victim name, +0x116 = 0) — M6 slice 3.
    let raptor = core
        .monster_ids()
        .into_iter()
        .find(|id| core.monster_template(*id) == Some(RAPTOR))
        .expect("raptor instance");
    assert_eq!(core.monster_target(raptor), Some(s), "summon spawns locked on the victim");
}

#[test]
fn duration_drain_and_summon_rows_are_dead() {
    // Cases 8 and 0xc are gated `local_28 == 0` with NO else arm
    // (23207-23229 / 23251-23267): a duration cast's Drain/Summon rows do
    // nothing at all — no entry, no damage, no spawn, no lines. The
    // energy cost stays paid (23123 runs before the loop).
    let mut core = Core::new(world(shaman(DURDEAD, 101, 600)), config());
    let (s, m) = engage(&mut core, HUMAN);
    let before = core.current_hp(s);
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(!shown.contains("grasping shadows"), "silent: {shown:?}");
    assert_eq!(core.current_hp(s), before, "no drain landed");
    let slots = core.player_snapshot(s).active_spells;
    assert!(slots.iter().all(|sl| sl.spell.is_none()), "no entry: {slots:?}");
    assert_eq!(core.monster_energy(m), Some(400), "full cost stays paid");
    core.input(s, "look");
    let events = core.drain_events();
    let look = text_to(&events, s);
    assert!(!look.contains("raptor"), "no spawn: {look:?}");
}

#[test]
fn damage_mr_ignores_the_duration_gate_and_never_drives_the_entry() {
    // Case 0x11 (23305-23356) carries no local_28 gate: the (DamageMR,
    // 100) row lands instantly even at duration 40 — MR 0 amplifies by
    // (50-0)% to 150 — while the Poison row still drives the one slot
    // entry and the counter hard-write.
    let mut core = Core::new(world(shaman(MRVENOM, 101, 600)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    run_rounds(&mut core, 1);
    assert_eq!(core.current_hp(s), 200 - 150, "instant MR-scaled damage");
    let slots = core.player_snapshot(s).active_spells;
    assert_eq!(slots[0].spell, Some(MRVENOM), "Poison row entered: {slots:?}");
    assert_eq!(slots[0].value, 6, "slot stores the Poison row value");
    assert_eq!(core.poison(s), 6, "counter hard-written");
}

#[test]
fn stat_rows_use_the_no_refresh_entry_flag() {
    // The Intel..Charm family (44-49, cases 0x2c-0x31) calls
    // monster_add_cast_spell_to_user with the '\0' flag (23436-23477): a
    // same-id active slot ALWAYS aborts — even though 10 exceeds the
    // stored 1 (21795 only refreshes under a non-zero flag). The abort
    // refunds the full energy cost and prints nothing (23183-23188).
    let mut core = Core::new(world(shaman(SAPMIND, 101, 600)), config());
    let m = core.spawn_monster(MonsterId(7), ARENA).expect("shaman spawns");
    let mut dain = player("Dain", HUMAN);
    dain.active_spells[0] =
        ActiveSpell { spell: Some(SAPMIND), value: 1, remaining: 1000 };
    let s = core.attach_player(dain);
    core.input(s, "attack shaman");
    core.drain_events();
    let events = run_rounds(&mut core, 1);
    let shown = text_to(&events, s);
    assert!(!shown.contains("sap mind"), "aborted silently: {shown:?}");
    let slots = core.player_snapshot(s).active_spells;
    assert_eq!(slots[0].value, 1, "slot untouched: {slots:?}");
    assert_eq!(core.monster_energy(m), Some(1000), "full refund");
}

#[test]
fn monster_alter_sp_dmg_boosts_damage_casts() {
    // Wired through the monster ability fold: 5 + 5*50/100 = 7. (The
    // DLL's monster_cast reads no 0xa5 — zero shipped monsters carry
    // AlterSpDmg, so the fold read is observably identical.)
    let mut boosted = shaman(HAMMER, 101, 600);
    boosted.abilities = vec![(Ability::AlterSpDmg, 50)];
    let mut core = Core::new(world(boosted), config());
    let (s, _m) = engage(&mut core, HUMAN);
    run_rounds(&mut core, 1);
    assert_eq!(core.current_hp(s), 200 - 7);
}

#[test]
fn fear_slot_flees_the_player_out_a_valid_exit() {
    // PANIC enters the slot on the first round (t5); the next upkeep
    // (t6) rolls genrdn(0,100) < 200 — always — and forces a move out
    // the one valid exit (44788-44793: move_user mode 6, which prints
    // NOTHING fear-specific — the standard movement observables only).
    let mut core = Core::new(world(shaman(PANIC, 101, 600)), config());
    let (s, _m) = engage(&mut core, HUMAN);
    let events = run_rounds(&mut core, 2);
    assert_eq!(core.player_snapshot(s).location, LAIR, "fled north");
    let shown = text_to(&events, s);
    assert!(shown.contains("Lair"), "the flee renders the destination: {shown:?}");
}
