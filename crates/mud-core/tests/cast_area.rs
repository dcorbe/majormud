//! Area casts (slice-5 Task 5; MEASURED spellcasting.md §8.13 for match
//! 12, decompile groupings for the rest): match 11/12 (and the fixture
//! types 3/5/9) iterate the room's MONSTERS — players are NEVER area
//! targets, measured alone and with players present. The empty-target
//! refusal is a real pre-charge gate; explicit target words refuse
//! kind-keyed before any cost; success prints the caster line and ONE
//! room line (no per-target lines, no damage numbers, no engagement);
//! duration areas enter the monsters' 5-slot tables silently (slice 6 —
//! no player view ever hears the monster leg). Magnitude splits by
//! target count for 3/5/9/10 only.

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Element, MatchType, Message, MessageId, Monster,
    MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId, StatBlock,
    TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const MAGE: ClassId = ClassId(1);
const WARRIOR: ClassId = ClassId(2);

/// The flash (51) model: benign match-12 DURATION area whose castmsgb
/// room line mirrors the caster line (message 8457 shape).
const GLARE: SpellId = SpellId(500);
/// The stinking cloud (131) model: benign match-12 duration area with
/// the GENERIC room frame (message 75 shape) and a DescMsg that must
/// never surface on a player.
const REEK: SpellId = SpellId(510);
/// The poison cloud (142) model: benign match-12 duration area carrying
/// Poison(0) — the Task-5 regression (it must NOT self-poison).
const MIASMA: SpellId = SpellId(520);
/// Offensive INSTANT match-12 fire damage 12..12 — the per-monster
/// elemental-resist probe (no split: 12 is not in 3/5/9/10).
const QUAKE: SpellId = SpellId(530);
/// Offensive instant match-5 magic damage 12..12 — the magnitude-SPLIT
/// probe (V / target count; fixture-only, no learnable 3/5/9/10 exists).
const CLEAVE: SpellId = SpellId(540);
/// Benign match-13 duration area: 13 iterates PLAYERS in the decompile,
/// and players are never valid area targets (§8.13) — targetless for
/// now, ORACLE-VERIFY.
const HYMN: SpellId = SpellId(550);
/// Rollable (base 15) benign match-12 area: the failed-roll probe.
const SMOG: SpellId = SpellId(560);

const RAT: MonsterId = MonsterId(7);
/// Rfir (5) 50: fire-element amounts scale by (100-50)/100 per target.
const EMBER: MonsterId = MonsterId(8);
/// 5 HP: the area kill probe.
const FRAIL: MonsterId = MonsterId(11);

const TOWER: RoomId = RoomId { map: 1, room: 1 };
/// `attributes & 1` — the protected-room (guilt line) fixture.
const SHOP: RoomId = RoomId { map: 1, room: 2 };

fn monster(id: MonsterId, name: &str, hitpoints: i32) -> Monster {
    Monster {
        id,
        name: name.into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints,
        experience: 12,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 0,
        magic_resist: 0,
        bs_defence: 0,
        energy: 0,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [AttackForm::default(); 5],
    }
}

fn spell(id: SpellId, name: &str, short: &str) -> Spell {
    Spell {
        id,
        name: name.into(),
        short_name: short.into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![],
        level_cap: 0,
        round_cost: 0,
        required_power: 1,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200,
        duration_per_level: 0,
        match_type: MatchType::AreaC,
        duration: 0,
        element: Element::Cold,
        class_gate_group: 1,
        mana_cost: 0,
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
    content.add_room(Room {
        id: SHOP,
        name: "Spell Shop".into(),
        description: vec![],
        room_type: 0,
        attributes: 1,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_monster(monster(RAT, "giant rat", 1000));
    let mut ember = monster(EMBER, "ember beast", 1000);
    ember.abilities = vec![(Ability::from_id(5).unwrap(), 50)];
    content.add_monster(ember);
    content.add_monster(monster(FRAIL, "frail bat", 5));
    // The flash castmsgb (real message 8457): the room line MIRRORS the
    // caster line — both bind (caster, spell) prefixes of their orders.
    content.add_message(Message {
        id: MessageId(950),
        lines: vec![
            "You cast %s, blinding everyone in the room!".into(),
            "%s casts %s, blinding everyone in the room!".into(),
            "%s casts %s, blinding everyone in the room!".into(),
        ],
    });
    // The stinking cloud castmsgb (real message 75): spell-specific
    // caster line, generic "on the room!" frame for everyone else. The
    // target line exists in the record but never fires (§8.13).
    content.add_message(Message {
        id: MessageId(952),
        lines: vec![
            "You cast %s, enveloping the room in a foul stench!".into(),
            "%s casts %s on you!".into(),
            "%s casts %s on the room!".into(),
        ],
    });
    // A DescMsg record that must NEVER print on the area path (the
    // entries land in MONSTER slots, which have no terminal; §8.13:
    // nothing on any player view).
    content.add_message(Message {
        id: MessageId(953),
        lines: vec![
            "The fumes disperse.".into(),
            String::new(),
            "You feel enveloped!".into(),
        ],
    });
    // The poison cloud castmsgb (real message 89): generic both ways.
    content.add_message(Message {
        id: MessageId(954),
        lines: vec![
            "You cast %s on the room!".into(),
            "%s casts %s on you!".into(),
            "%s casts %s on the room!".into(),
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

    let mut glare = spell(GLARE, "glare", "glar");
    glare.duration = 5;
    glare.mana_cost = 10;
    glare.cast_msg_b = Some(MessageId(950));
    glare.abilities = vec![(Ability::BlindUser, 0)];
    let mut reek = spell(REEK, "reek", "reek");
    reek.duration = 20;
    reek.mana_cost = 10;
    reek.cast_msg_b = Some(MessageId(952));
    reek.abilities = vec![(Ability::Confusion, -5), (Ability::DescMsg, 953)];
    let mut miasma = spell(MIASMA, "miasma", "mias");
    miasma.duration = 40;
    miasma.mana_cost = 14;
    miasma.min_base = 12;
    miasma.max_base = 12;
    miasma.cast_msg_b = Some(MessageId(954));
    miasma.abilities = vec![(Ability::Poison, 0), (Ability::DescMsg, 953)];
    let mut quake = spell(QUAKE, "quake", "quak");
    quake.target_mode = TargetMode::Offensive0;
    quake.element = Element::Fire;
    quake.mana_cost = 2;
    quake.round_cost = 100;
    quake.min_base = 12;
    quake.max_base = 12;
    quake.cast_msg_b = Some(MessageId(954));
    quake.abilities = vec![(Ability::Damage, 0)];
    let mut cleave = quake.clone();
    cleave.id = CLEAVE;
    cleave.name = "cleave".into();
    cleave.short_name = "clea".into();
    cleave.match_type = MatchType::Area5;
    cleave.element = Element::Magic;
    let mut hymn = spell(HYMN, "hymn", "hymn");
    hymn.match_type = MatchType::AreaD;
    hymn.duration = 30;
    hymn.mana_cost = 5;
    hymn.cast_msg_b = Some(MessageId(952));
    hymn.abilities = vec![(Ability::Dodge, 0), (Ability::DescMsg, 953)];
    let mut smog = spell(SMOG, "smog", "smog");
    smog.base_chance = 15;
    smog.duration = 5;
    smog.mana_cost = 4;
    smog.cast_msg_b = Some(MessageId(952));
    smog.abilities = vec![(Ability::Confusion, -5)];
    content.add_spell(glare);
    content.add_spell(reek);
    content.add_spell(miasma);
    content.add_spell(quake);
    content.add_spell(cleave);
    content.add_spell(hymn);
    content.add_spell(smog);
    content
}

fn book() -> BTreeMap<SpellId, bool> {
    let mut book = BTreeMap::new();
    for id in [GLARE, REEK, MIASMA, QUAKE, CLEAVE, HYMN, SMOG] {
        book.insert(id, false);
    }
    book
}

fn player(name: &str, class: ClassId, location: RoomId) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class,
        level: 1,
        stats: StatBlock::default(),
        base_stats: StatBlock::default(),
        hp_base: 0,
        current_hp: 10,
        current_mana: 30,
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
        location,
        spellbook: book(),
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

fn cast(core: &mut Core, s: SessionId, line: &str) -> String {
    core.input(s, line);
    text_to(&core.drain_events(), s)
}

/// Zinvar (mage caster), Oracle and Kaimon (bystanders), all in the Tower.
fn stage() -> (Core, SessionId, SessionId, SessionId) {
    let mut core = Core::new(world(), CoreConfig::default());
    let zin = core.attach_player(player("Zinvar", MAGE, TOWER));
    let ora = core.attach_player(player("Oracle", WARRIOR, TOWER));
    let kai = core.attach_player(player("Kaimon", WARRIOR, TOWER));
    core.drain_events();
    (core, zin, ora, kai)
}

const ALREADY_CAST: &str = "You have already cast a spell this round!";

#[test]
fn area_with_no_monsters_refuses_uncharged_even_with_players_present() {
    // MEASURED (§8.13): alone AND with players present — "Your spell has
    // no effect in this room!", mana unchanged. Players never count as
    // area targets (no PvP path through match-12 areas).
    let (mut core, zin, _, _) = stage();
    let shown = cast(&mut core, zin, "c glare");
    assert!(
        shown.contains("Your spell has no effect in this room!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(zin), 30, "pre-charge refusal");
    // The refusal does not spend the round: a valid cast still works.
    core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    let next = cast(&mut core, zin, "c glare");
    assert!(!next.contains(ALREADY_CAST), "round not spent: {next:?}");
    assert!(next.contains("You cast glare"), "got: {next:?}");
}

#[test]
fn area_with_explicit_player_word_refuses_uncharged() {
    // MEASURED (§8.13): `c flash oracle` -> "You may not cast that spell
    // on a user!", pre-charge, even with valid monsters present.
    let (mut core, zin, _, _) = stage();
    core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    let shown = cast(&mut core, zin, "c glare ora");
    assert!(
        shown.contains("You may not cast that spell on a user!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(zin), 30, "uncharged");
}

#[test]
fn area_with_explicit_monster_word_refuses_uncharged() {
    // MEASURED (§8.13): `c stnk cat` -> "You may not cast that spell on
    // a monster!", pre-charge.
    let (mut core, zin, _, _) = stage();
    core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    let shown = cast(&mut core, zin, "c glare rat");
    assert!(
        shown.contains("You may not cast that spell on a monster!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(zin), 30, "uncharged");
}

#[test]
fn area_with_unmatched_word_is_do_not_see() {
    // ORACLE-VERIFY: only player/monster words were measured — an
    // unmatched word falls to the room-lookup refusal.
    let (mut core, zin, _, _) = stage();
    let shown = cast(&mut core, zin, "c glare zzz");
    assert!(shown.contains("You do not see zzz here!\n"), "got: {shown:?}");
    assert_eq!(core.current_mana(zin), 30, "uncharged");
}

#[test]
fn area_success_mirrored_lines_no_per_target_output_no_engagement() {
    // MEASURED (§8.13, flash): caster line + the mirrored room line to
    // every other player; NO per-target lines, NO damage numbers, NO
    // combat engagement, nothing on any player's sheet.
    let (mut core, zin, ora, kai) = stage();
    let rat = core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    core.input(zin, "c glare");
    let events = core.drain_events();
    let caster = text_to(&events, zin);
    assert!(
        caster.contains("You cast glare, blinding everyone in the room!\n"),
        "caster: {caster:?}"
    );
    assert!(!caster.contains("Combat"), "no engagement: {caster:?}");
    for view in [text_to(&events, ora), text_to(&events, kai)] {
        assert!(
            view.contains("Zinvar casts glare, blinding everyone in the room!\n"),
            "room: {view:?}"
        );
        assert!(!view.contains("on you"), "no target line: {view:?}");
    }
    assert_eq!(core.current_mana(zin), 20, "full mana 10 at the prompt");
    // Players are not targets: no slots anywhere.
    for s in [zin, ora, kai] {
        assert!(
            core.player_snapshot(s).active_spells.iter().all(|a| a.spell.is_none()),
            "no player slot entries"
        );
    }
    // The duration debuff entered the RAT's 5-slot table silently
    // (slice 6); its HP is untouched.
    assert_eq!(core.monster_hp(rat), Some(1000), "monster HP untouched");
    let slots = core.monster_active_spells(rat).expect("rat lives");
    assert!(
        slots.iter().any(|s| s.spell == Some(GLARE)),
        "glare entered the monster slots: {slots:?}"
    );
}

#[test]
fn area_generic_room_frame_and_silent_monster_side() {
    // MEASURED (§8.13, stinking cloud): the room view is the GENERIC
    // frame "on the room!", the DescMsg never surfaces, and no expiry
    // line ever reaches a player (monster slots die silently).
    let (mut core, zin, ora, _) = stage();
    core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    core.input(zin, "c reek");
    let events = core.drain_events();
    let caster = text_to(&events, zin);
    assert!(
        caster.contains("You cast reek, enveloping the room in a foul stench!\n"),
        "caster: {caster:?}"
    );
    assert!(!caster.contains("You feel enveloped!"), "no DescMsg: {caster:?}");
    let room = text_to(&events, ora);
    assert!(room.contains("Zinvar casts reek on the room!\n"), "room: {room:?}");
    // Run past the 20-tick duration: silence on every player view.
    for _ in 0..70 {
        core.tick();
    }
    let later = core.drain_events();
    for s in [zin, ora] {
        let view = text_to(&later, s);
        assert!(!view.contains("disperse"), "silent expiry: {view:?}");
    }
}

#[test]
fn poison_cloud_family_no_longer_self_poisons() {
    // The Task-5 regression note: match-12 poison areas routed through
    // the benign SELF path and poisoned the caster. The area path must
    // leave the caster clean (the duration entry rides the MONSTERS'
    // slots since slice 6; the area leg never hard-writes poison —
    // game.rs area_cast, decompile 38701-38746).
    let (mut core, zin, _, _) = stage();
    core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    let shown = cast(&mut core, zin, "c miasma");
    assert!(shown.contains("You cast miasma on the room!\n"), "got: {shown:?}");
    let p = core.player_snapshot(zin);
    assert_eq!(p.poison, 0, "the caster is NOT poisoned");
    assert!(p.active_spells.iter().all(|a| a.spell.is_none()), "no self slot");
    assert_eq!(core.current_mana(zin), 16, "full mana 14 charged");
}

#[test]
fn instant_area_damage_hits_every_monster_with_per_target_resist() {
    // Spec §4: each target gets its own elemental modifier
    // ((100-resist)*V/100). Fire 12 -> rat 12, ember (Rfir 50) 6. No
    // damage numbers reach any view; match 12 does not split.
    let (mut core, zin, ora, _) = stage();
    let rat = core.spawn_monster(RAT, TOWER).expect("fixture template");
    let ember = core.spawn_monster(EMBER, TOWER).expect("fixture template");
    core.drain_events();
    core.input(zin, "c quake");
    let events = core.drain_events();
    // The 12..12 bounds roll 12..=13 (inclusive genrdn); Rfir 50 halves
    // either to exactly 6.
    let rat_hp = core.monster_hp(rat).expect("alive");
    assert!((987..=988).contains(&rat_hp), "unresisted 12..=13: {rat_hp}");
    assert_eq!(core.monster_hp(ember), Some(994), "Rfir 50 halves to 6");
    let caster = text_to(&events, zin);
    assert!(caster.contains("You cast quake on the room!\n"), "got: {caster:?}");
    assert!(!caster.contains("12"), "no damage numbers: {caster:?}");
    assert!(!caster.contains("Combat"), "no engagement: {caster:?}");
    let room = text_to(&events, ora);
    assert!(!room.contains("12"), "no damage numbers: {room:?}");
}

#[test]
fn area_types_three_five_nine_ten_split_the_magnitude() {
    // Spec §3: for match 3/5/9/10 the rolled V divides by the target
    // count. cleave (match 5, magic 12): two monsters -> 6 each.
    let (mut core, zin, _, _) = stage();
    let rat = core.spawn_monster(RAT, TOWER).expect("fixture template");
    let ember = core.spawn_monster(EMBER, TOWER).expect("fixture template");
    core.drain_events();
    cast(&mut core, zin, "c cleave");
    // The 12..=13 roll halves to exactly 6 under two targets.
    assert_eq!(core.monster_hp(rat), Some(994), "12..=13 / 2 targets = 6");
    assert_eq!(core.monster_hp(ember), Some(994), "magic is unresistable");

    // A single target takes the whole roll.
    let mut core = Core::new(world(), CoreConfig::default());
    let zin = core.attach_player(player("Zinvar", MAGE, TOWER));
    let rat = core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    cast(&mut core, zin, "c cleave");
    let rat_hp = core.monster_hp(rat).expect("alive");
    assert!((987..=988).contains(&rat_hp), "single target: full roll: {rat_hp}");
}

#[test]
fn area_kill_routes_through_the_kill_path() {
    // An area kill runs the M3 kill route: death line, experience.
    let (mut core, zin, _, _) = stage();
    let frail = core.spawn_monster(FRAIL, TOWER).expect("fixture template");
    core.drain_events();
    let shown = cast(&mut core, zin, "c quake");
    assert!(shown.contains("The frail bat is dead.\n"), "got: {shown:?}");
    assert!(shown.contains("You gain 12 experience.\n"), "got: {shown:?}");
    assert_eq!(core.monster_hp(frail), None, "instance gone");
}

#[test]
fn offensive_area_in_a_protected_room_is_guilt_refused() {
    // The cast_no_target room-protection gate (§3 step 2) precedes
    // target counting; offensive-mode areas guilt-refuse with the round
    // cost charged when affordable, mana untouched (mirrors the bare
    // offensive gate; ORACLE-VERIFY — no learnable offensive area).
    let mut core = Core::new(world(), CoreConfig::default());
    let zin = core.attach_player(player("Zinvar", MAGE, SHOP));
    core.spawn_monster(RAT, SHOP).expect("fixture template");
    core.drain_events();
    let energy = core.round_energy(zin);
    let shown = cast(&mut core, zin, "c quake");
    assert!(
        shown.contains("You are overcome with a feeling of guilt and break off your attack.\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(zin), 30, "mana untouched");
    assert_eq!(core.round_energy(zin), energy - 100, "round cost charged");
}

#[test]
fn benign_area_in_a_protected_room_still_casts() {
    // Benign casts are legal in protected rooms (§8.6: blur was cast in
    // the Newhaven Spell Shop) — the guilt gate is offensive-only.
    let mut core = Core::new(world(), CoreConfig::default());
    let zin = core.attach_player(player("Zinvar", MAGE, SHOP));
    core.spawn_monster(RAT, SHOP).expect("fixture template");
    core.drain_events();
    let shown = cast(&mut core, zin, "c glare");
    assert!(shown.contains("You cast glare"), "got: {shown:?}");
}

#[test]
fn area_failed_roll_prints_the_untargeted_fail_pair() {
    // ORACLE-VERIFY: no area fail was measured — the plain cast_no_target
    // fail pair with half mana (the §8.6 model). Grunt (caster_group 0)
    // can never pass a rollable spell.
    let mut core = Core::new(world(), CoreConfig::default());
    let grunt = core.attach_player(player("Grunt", WARRIOR, TOWER));
    let ora = core.attach_player(player("Oracle", WARRIOR, TOWER));
    core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    core.input(grunt, "c smog");
    let events = core.drain_events();
    let caster = text_to(&events, grunt);
    assert!(
        caster.contains("You attempt to cast smog, but fail.\n"),
        "caster: {caster:?}"
    );
    let room = text_to(&events, ora);
    assert!(
        room.contains("Grunt attempted to cast smog, but failed.\n"),
        "room: {room:?}"
    );
    assert_eq!(core.current_mana(grunt), 28, "half of mana 4 deducted");
}

#[test]
fn area_cast_spends_the_round() {
    // The one-cast-per-round flag covers area casts (they charge at the
    // command like every benign cast; ORACLE-VERIFY for offensive-mode
    // areas — none is learnable).
    let (mut core, zin, _, _) = stage();
    core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    let first = cast(&mut core, zin, "c glare");
    assert!(!first.contains(ALREADY_CAST), "first passes: {first:?}");
    let second = cast(&mut core, zin, "c reek");
    assert!(second.contains(ALREADY_CAST), "got: {second:?}");
}

#[test]
fn match_thirteen_is_targetless_until_measured() {
    // Match 13 iterates PLAYERS in the decompile (§4) and never monsters
    // — but §8.13 measured that players are excluded from area targeting
    // (match 12). Until an oracle run settles 13 (no learnable 13 below
    // priest chant L6), the room-wide player sweep stays empty: the
    // no-effect refusal fires even with monsters and players present.
    // ORACLE-VERIFY.
    let (mut core, zin, _, _) = stage();
    core.spawn_monster(RAT, TOWER).expect("fixture template");
    core.drain_events();
    let shown = cast(&mut core, zin, "c hymn");
    assert!(
        shown.contains("Your spell has no effect in this room!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(zin), 30, "uncharged");
}
