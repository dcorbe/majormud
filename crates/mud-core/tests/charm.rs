//! M7 slice 5 — Enslave acquisition, the charm apply (`cast_monster_target`
//! case 6, decompile 43796-43822 + the save-stat preload 43302-43316;
//! `re/docs/charm.md` §0/§1). Facts under test:
//! - the save stat SWAP: an ability-6 spell saves against the template's
//!   `charmres` (43311), not M.R. — and a charmres of **0** falls back to
//!   M.R. anyway, because the 43387 default keys on `local_34 == 0`;
//! - the `charmlvl` application gate (43798-43800): a plain signed level
//!   compare whose failure is silent — no slot entry, no charm, the mana
//!   stays paid;
//! - the §0 state triple (owner link + suppression + charmed bit) written
//!   by both the instant and the duration arm, with no rename;
//! - the slot-full edge (§1.4): `add_cast_spell_to_monster`'s -1 is
//!   ignored by the caller, so a fully slotted monster takes the plain
//!   cast-fail line AND a permanent, timerless charm;
//! - the §1.5 draw order (success roll -> save -> magnitude -> duration).
//!
//! FIXTURE SHAPE — the real one: all four shipped Enslave spells (49 song
//! of charming, 55 enslave, 88 control undead, 92 charm animal) carry
//! `spelltype` 3 (`TargetMode::Benign`) with match type 4, and that is
//! what the fixtures below use. The dispatcher routes them to
//! `cast_monster_target` off the MATCH type (`get_spell_match_type` ->
//! 0x801), and the benign target mode then keeps them out of the
//! engage-and-stop block at 43411-43421 — so every charm here resolves
//! inside the command, costs and all, and none of them engages combat.

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Direction, Element, Exit, MatchType, Message, MessageId,
    Monster, MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId,
    StatBlock, TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, MonsterInstanceId, Player, SessionId};

const TOWER: RoomId = RoomId { map: 1, room: 1 };
/// The §4.2 walking pair, deliberately DISCONNECTED from the exitless
/// tower: a roam-class-0 fixture never wanders, so no test that ticks in
/// the tower spends a wander draw.
const CELL: RoomId = RoomId { map: 1, room: 2 };
const DEN: RoomId = RoomId { map: 1, room: 3 };
const MAGE: ClassId = ClassId(1);
const HUMAN: RaceId = RaceId(1);

/// The pin from the shipped data: giant rat, `charmlvl` 1 / `charmres` 40.
const RAT: MonsterId = MonsterId(1);
/// `charmlvl` 9999 — the shipped "never" sentinel (381 templates).
const ELDER: MonsterId = MonsterId(2);
/// M.R. 200 (a 98% resist) with `charmres` 1 (a 0% one): the swap probe.
const WARY: MonsterId = MonsterId(3);
/// M.R. 0 with `charmres` 196: the swap probe from the other side.
const STUBBORN: MonsterId = MonsterId(4);
/// M.R. 200 with `charmres` **0**: the 43387 fallback probe.
const HOLLOW: MonsterId = MonsterId(5);
/// A 500 HP charmable body — the release paths need a pet that survives
/// a full melee round (and a long walk).
const MUTT: MonsterId = MonsterId(6);
/// MUTT with roam class 0x25: the give-up branch DESPAWNS this one
/// instead of releasing it (19448-19450).
const STRAY: MonsterId = MonsterId(7);
/// The §4.4 executioner — kills the owner, releases nothing.
const EXEC: MonsterId = MonsterId(8);
/// MUTT with aggression **100** and behaviour **1**: the probe for the
/// cast-damage retaliation twin (43750-43766). MUTT itself is aggression
/// 0 / behaviour 0, and a monster like that locks in our shared lock body
/// only through the `behaviour in {3,0,4}` clause — which the cast twin
/// does NOT have. Asserting the grudge on MUTT would therefore prove
/// nothing about the roll. Here the roll alone carries it: aggression 100
/// beats every `genrdn(1,100)`, and behaviour 1 is outside the clause, so
/// the assertion survives a faithful (clause-free) twin.
const KEEN: MonsterId = MonsterId(9);
/// Built per-test with a parameterised roam class — the class-5
/// "guardian" probe for the shared lock body's draw order. Not in
/// [`world`]: each arm needs its own template.
const GUARD: MonsterId = MonsterId(10);
/// The §2.2 assist pet: a real form-0 fighter whose swing is FREE (form
/// EU 0 against a 1000 pool), so the 27232 full-energy gate never closes
/// it out. Aggression 100 / behaviour 1 is deliberate — an aggressive
/// body is what makes the suppressed-"friend" arm (20494-20511) fire on
/// a bystander if the charmed guard is missing — and its damage soak
/// keeps the owner's own swings off the §4.3 release path.
const HOUND: MonsterId = MonsterId(11);
/// The pet's quarry: 500 HP, behaviour 3 (lair) and no attack form, so
/// it never initiates and never swings back — every combat line it takes
/// is the pet's.
const BAG: MonsterId = MonsterId(12);
/// The shipped `bishop`/`priest`/`boatman` shape: energy pool **0** with
/// a form-0 cost of 5, and `charmlvl` 0 (charmable by anyone). It can
/// never pay for the swing it has already drawn for (27242).
///
/// `attacktype_1` is 0 on all three shipped templates, and so is this
/// fixture's `kind` — the fighter build never reads it, so the field is
/// irrelevant to what this probe measures either way. (`old man` #39 and
/// `healer` #47 are the genuinely-`kind`-1 members of the pool-0 pool.)
const BISHOP: MonsterId = MonsterId(13);
/// A second body sharing HOUND's `dog` word: the `0x800` two-pass
/// ordering probe (§2.3). Inert — 500 HP, behaviour 3, no attack form.
const CUR: MonsterId = MonsterId(14);
/// HOUND with **roam class 5**: the probe for the A2 ladder's ORDER.
/// Not a corner case in the shipped data — 13 roam-5 templates carry
/// `charmlvl` < 9999, among them `guardsman` (#14, `charmlvl` 30), which
/// is the `monstertype` of 487 rooms, and `storm giant king` (#637,
/// `charmlvl` 0). A charmed class-5 body is reachable in play.
const WARDEN: MonsterId = MonsterId(15);

/// (Enslave, 0), duration 60 flat, no save — the state/slot probe.
const ENSLAVE: SpellId = SpellId(700);
/// ENSLAVE with `SaveClass::Always` — the save-stat probes.
const THRALL: SpellId = SpellId(710);
/// Save-Always debuff with NO ability 6 — the "swap must not leak" guard.
const HOLD: SpellId = SpellId(720);
/// (Enslave, 0) with duration 0 — the instant arm (§1.4).
const SNAP: SpellId = SpellId(730);
/// The SHIPPED shape (song of charming / enslave): (Enslave, 0) +
/// (AffectsLiving, 0) — the gate-fall-through probe.
const WHISPER: SpellId = SpellId(740);
/// (Enslave, 0) on match type 0 — the 43797 match-type gate probe.
const LEASH: SpellId = SpellId(750);
/// Wide magnitude + duration bands, no save — the draw-order probe.
const BIND: SpellId = SpellId(760);
/// BIND with `SaveClass::Always` — its one-extra-draw twin.
const BINDSAVE: SpellId = SpellId(770);
/// Five slot fillers.
const FILLER_BASE: u16 = 780;
/// A fixed-damage OFFENSIVE match-4 spell — the cast-damage retaliation
/// twin (43750-43766), which carries no charmed check at all.
const SEAR: SpellId = SpellId(790);
/// SEAR as an AREA (match 12) — the 40371/40601 copies of that twin,
/// which are equally charm-blind.
const GALE: SpellId = SpellId(795);
/// ENSLAVE carrying (Poison, 5) alongside the charm — the probe for the
/// slot sweep running the WHOLE termination handler (44972) and not just
/// its case 6. No shipped Enslave pairs the two rows, so this is
/// fixture-only.
const VENOMBOND: SpellId = SpellId(800);
/// A benign match-4 SLOT spell with no save and no damage: the §2.3
/// two-pass find probe. The slot it leaves says which body the
/// `0x801` search resolved, with no roll in the way.
const MARK: SpellId = SpellId(810);
/// GALE at match **11**, the area type `is_valid_monster_target` waves
/// through unconditionally (38461) — the control that keeps the pet
/// exemption pinned to match 9/12 and not to "area casts".
const SQUALL: SpellId = SpellId(815);

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
        // The shipped Enslave shape: spelltype 3 + match 4.
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200, // auto-success: >= 200 skips the roll
        duration_per_level: 0,
        // 43797: the case-6 gate accepts match types 4/6/8 only — the
        // same set the dispatcher routes to `cast_monster_target`.
        match_type: MatchType::Special4,
        duration: 60,
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

fn monster(id: MonsterId, name: &str, charm_level: i16, charm_resist: i16) -> Monster {
    Monster {
        id,
        name: name.into(),
        hitpoints: 40,
        experience: 12,
        exp_multi: 1,
        energy: 0,
        charm_level,
        charm_resist,
        attacks: [AttackForm::default(); 5],
        ..Default::default()
    }
}

/// A plain two-way corridor exit.
fn plain_exit(dest: RoomId) -> Option<Exit> {
    Some(Exit { dest, exit_type: 0, ..Default::default() })
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room { id: TOWER, name: "Tower".into(), ..Default::default() });
    let mut cell = Room { id: CELL, name: "Cell".into(), ..Default::default() };
    cell.exits[Direction::North as usize] = plain_exit(DEN);
    let mut den = Room { id: DEN, name: "Den".into(), ..Default::default() };
    den.exits[Direction::South as usize] = plain_exit(CELL);
    content.add_room(cell);
    content.add_room(den);
    content.add_monster(monster(RAT, "giant rat", 1, 40));
    content.add_monster(monster(ELDER, "elder wyrm", 9999, 40));
    let mut mutt = monster(MUTT, "docile mutt", 1, 40);
    mutt.hitpoints = 500;
    content.add_monster(mutt);
    let mut stray = monster(STRAY, "stray cur", 1, 40);
    stray.hitpoints = 500;
    stray.roam_class = 0x25;
    content.add_monster(stray);
    let mut exec = monster(EXEC, "executioner", 1, 40);
    exec.hitpoints = 500;
    exec.energy = 1000;
    exec.aggression = 100;
    exec.behaviour = 2;
    exec.attacks[0] = AttackForm {
        kind: 1,
        accuracy: 500,
        weight: 100,
        min_damage: 90,
        max_damage: 120,
        energy: 200,
        ..Default::default()
    };
    content.add_monster(exec);
    let mut keen = monster(KEEN, "keen mutt", 1, 40);
    keen.hitpoints = 500;
    keen.aggression = 100;
    keen.behaviour = 1;
    content.add_monster(keen);
    let mut hound = monster(HOUND, "war dog", 1, 40);
    hound.hitpoints = 500;
    hound.energy = 1000;
    // 20 points of soak against the owner's unarmed (1, 9) band: the
    // owner can engage autocombat on its own pet and never land the
    // damage the §4.3 MELEE release is gated on (game.rs:9043), which is
    // what keeps the autocombat release below measurable on its own.
    hound.damage_resist = 20;
    hound.aggression = 100;
    hound.behaviour = 1;
    hound.attacks[0] = AttackForm {
        kind: 1,
        accuracy: 500,
        weight: 100,
        min_damage: 7,
        max_damage: 7,
        energy: 0,
        ..Default::default()
    };
    content.add_monster(hound);
    // HOUND's twin in everything but roam class: same free swing, same
    // aggressive body, so the ONLY thing that can keep it from assisting
    // is where the class-5 arm sits in the ladder.
    let mut warden = monster(WARDEN, "stone warden", 1, 40);
    warden.hitpoints = 500;
    warden.energy = 1000;
    warden.damage_resist = 20;
    warden.aggression = 100;
    warden.behaviour = 1;
    warden.roam_class = 5;
    warden.attacks[0] = AttackForm {
        kind: 1,
        accuracy: 500,
        weight: 100,
        min_damage: 7,
        max_damage: 7,
        energy: 0,
        ..Default::default()
    };
    content.add_monster(warden);
    let mut bag = monster(BAG, "straw dummy", 9999, 40);
    bag.hitpoints = 500;
    bag.behaviour = 3;
    content.add_monster(bag);
    let mut cur = monster(CUR, "wild dog", 9999, 40);
    cur.hitpoints = 500;
    cur.behaviour = 3;
    content.add_monster(cur);
    let mut bishop = monster(BISHOP, "bishop", 0, 40);
    bishop.hitpoints = 200;
    bishop.aggression = 100;
    bishop.behaviour = 1;
    bishop.attacks[0] = AttackForm {
        kind: 0,
        accuracy: 200,
        weight: 100,
        min_damage: 4,
        max_damage: 6,
        energy: 5, // > the pool: drawn for, never paid (27242)
        ..Default::default()
    };
    content.add_monster(bishop);
    let mut wary = monster(WARY, "wary hound", 1, 1);
    wary.magic_resist = 200;
    content.add_monster(wary);
    content.add_monster(monster(STUBBORN, "stubborn mule", 1, 196));
    let mut hollow = monster(HOLLOW, "hollow husk", 1, 0);
    hollow.magic_resist = 200;
    content.add_monster(hollow);
    // The blur castmsgb shape (fixture 901 across the cast suites).
    content.add_message(Message {
        id: MessageId(901),
        lines: vec![
            "You cast %s on %s!".into(),
            "%s casts %s upon you!".into(),
            "%s casts %s on %s!".into(),
        ],
    });
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

    let mut enslave = spell(ENSLAVE, "enslave", "ensl");
    enslave.abilities = vec![(Ability::Enslave, 0)];
    let mut thrall = spell(THRALL, "thrall", "thra");
    thrall.abilities = vec![(Ability::Enslave, 0)];
    thrall.save_class = SaveClass::Always;
    let mut hold = spell(HOLD, "hold", "hold");
    hold.abilities = vec![(Ability::AC, -5)];
    hold.save_class = SaveClass::Always;
    let mut snap = spell(SNAP, "snap", "snap");
    snap.abilities = vec![(Ability::Enslave, 0)];
    snap.duration = 0;
    let mut whisper = spell(WHISPER, "whisper", "whis");
    whisper.abilities = vec![(Ability::Enslave, 0), (Ability::AffectsLiving, 0)];
    let mut leash = spell(LEASH, "leash", "leas");
    leash.abilities = vec![(Ability::Enslave, 0)];
    leash.match_type = MatchType::Single0;
    let mut bind = spell(BIND, "bind", "bind");
    bind.abilities = vec![(Ability::Enslave, 0)];
    bind.max_base = 100;
    bind.duration_per_level = 40; // level 3 -> band roll genrdn(60, 121)
    let mut bindsave = spell(BINDSAVE, "bindsave", "bins");
    bindsave.abilities = vec![(Ability::Enslave, 0)];
    bindsave.max_base = 100;
    bindsave.duration_per_level = 40;
    bindsave.save_class = SaveClass::Always;
    let mut sear = spell(SEAR, "sear", "sear");
    sear.abilities = vec![(Ability::Damage, 3)];
    sear.duration = 0;
    sear.target_mode = TargetMode::Offensive0;
    let mut gale = spell(GALE, "gale", "gale");
    gale.abilities = vec![(Ability::Damage, 3)];
    gale.duration = 0;
    gale.target_mode = TargetMode::Offensive0;
    gale.match_type = MatchType::AreaC;
    let mut squall = spell(SQUALL, "squall", "squa");
    squall.abilities = vec![(Ability::Damage, 3)];
    squall.duration = 0;
    squall.target_mode = TargetMode::Offensive0;
    squall.match_type = MatchType::AreaB;
    let mut venombond = spell(VENOMBOND, "venombond", "veno");
    venombond.abilities = vec![(Ability::Enslave, 0), (Ability::Poison, 5)];
    let mut mark = spell(MARK, "mark", "mark");
    mark.abilities = vec![(Ability::AC, -5)];
    for s in [
        enslave, thrall, hold, snap, whisper, leash, bind, bindsave, sear, gale, squall, venombond,
        mark,
    ] {
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

/// Level 3: above the rat's `charmlvl` 1, below the wyrm's 9999.
fn caster() -> Player {
    caster_named("Zin", TOWER)
}

fn caster_named(name: &str, location: RoomId) -> Player {
    let book: BTreeMap<SpellId, bool> = [
        ENSLAVE, THRALL, HOLD, SNAP, WHISPER, LEASH, BIND, BINDSAVE, SEAR, GALE, SQUALL, VENOMBOND,
        MARK,
    ]
    .into_iter()
    .chain((0..5).map(|i| SpellId(FILLER_BASE + i)))
    .map(|s| (s, false))
    .collect();
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: HUMAN,
        class: MAGE,
        // Strength 100: the unarmed default band is (1, 4 + (Str-50)/10),
        // so a 0-Strength caster can only ever GLANCE — and the §4.3
        // release sits behind landed damage. It also moves the accuracy
        // term `(Str-50)/3` from -16 to +16 for every test in this file,
        // which nothing here depends on but which is why the pre-M7
        // fixture's swing outcomes are not comparable to these.
        stats: StatBlock { strength: 100, ..StatBlock::default() },
        level: 3,
        current_hp: 200,
        current_mana: 100,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location,
        spellbook: book,
        ..Default::default()
    }
}

fn config() -> CoreConfig {
    CoreConfig { start_location: TOWER, ..CoreConfig::default() }
}

fn seeded(seed: u64) -> CoreConfig {
    CoreConfig { rng_seed: seed, ..config() }
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

fn setup(template: MonsterId) -> (Core, SessionId, MonsterInstanceId) {
    setup_at(template, TOWER)
}

fn setup_at(template: MonsterId, room: RoomId) -> (Core, SessionId, MonsterInstanceId) {
    let mut core = Core::new(world(), config());
    let m = core.spawn_monster(template, room).expect("fixture template");
    let s = core.attach_player(caster_named("Zin", room));
    core.drain_events();
    (core, s, m)
}

fn cast(core: &mut Core, s: SessionId, line: &str) -> String {
    core.input(s, line);
    text_to(&core.drain_events(), s)
}

/// One combat round: refills the energy pool and clears the
/// one-cast-per-round permission bit.
fn energy_round(core: &mut Core) {
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
}

// --- §1.3 the state triple ---

#[test]
fn charm_writes_the_triple_and_enters_one_slot() {
    // 43809-43821 (the duration arm): add_cast_spell_to_monster first,
    // then owner link + suppression + charmed bit, all under the dirty
    // byte. The display name is NOT touched (§1.3 — the DLL writes only
    // the internal +0x1a link).
    let (mut core, s, m) = setup(RAT);
    let shown = cast(&mut core, s, "cast ensl rat");
    assert!(shown.contains("You cast enslave on giant rat!"), "got: {shown:?}");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "charmed + suppressed + owned by the caster"
    );
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert_eq!(slots[0].spell, Some(ENSLAVE), "the one slot entry");
    assert_eq!(slots[0].remaining, 60, "flat duration, no per-level band");
    assert!(slots[1].spell.is_none(), "exactly one entry");
    // No rename (§1.3): the DLL writes the internal +0x1a link only, and
    // the display name at +0x8e is untouched — the room still lists it.
    assert!(cast(&mut core, s, "look").contains("giant rat"), "no rename");
    assert_eq!(core.current_mana(s), 96, "full costs paid");
    assert_eq!(core.monster_hp(m), Some(40), "charm harms nothing");
}

#[test]
fn a_charm_that_lands_sets_no_retaliation_grudge() {
    // The triple's owner link is a FRIEND link: +0x116 = 1. The harm-path
    // retaliation lock never runs (Enslave sets no damage), so the pet
    // must not come out of the cast hostile.
    let (mut core, s, m) = setup(RAT);
    cast(&mut core, s, "cast ensl rat");
    let (charmed, suppress, owner) = core.debug_monster_charm(m).expect("rat lives");
    assert!(charmed && suppress, "friend, not grudge-holder");
    assert_eq!(owner, Some(s));
}

#[test]
fn instant_enslave_writes_the_triple_without_a_slot() {
    // 43801-43807: `spell+0xce == 0` -> the charm state ONLY — no slot, no
    // timer, permanent until a §4 release. ORACLE-VERIFY (§7): the case
    // body prints nothing at all in the DLL (display_spell_success lives
    // inside add_cast_spell_to_monster, which the instant arm never
    // calls); our castmsgb pair still renders from the shared tail, and no
    // shipped Enslave spell is instant, so the live surface is unmeasured.
    // The instant arm resolves AT THE COMMAND like every other benign
    // cast: the engage-and-stop block at 43411-43421 is gated on
    // `spelltype < 3`, which the charm family (spelltype 3) never
    // satisfies, so there is no engagement round to wait for and no
    // grudge lock to overwrite.
    let (mut core, s, m) = setup(RAT);
    let shown = cast(&mut core, s, "cast snap rat");
    assert!(!shown.contains("*Combat Engaged*"), "benign never engages: {shown:?}");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "the triple lands inside the command"
    );
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert!(slots.iter().all(|slot| slot.spell.is_none()), "no slot, no timer");
    assert_eq!(core.current_mana(s), 96, "full costs paid at the command");
}

// --- §1.2 the charmlvl gate ---

#[test]
fn charm_level_above_the_caster_is_silent() {
    // 43798-43800: `charmlvl <= caster level` or the case body is skipped
    // ENTIRELY — no message, no slot entry, and the mana is already paid.
    // The fixture carries the shipped companion row (AffectsLiving) to
    // prove the skipped case does not fall through to the slot-entry
    // default arm.
    let (mut core, s, m) = setup(ELDER);
    let shown = cast(&mut core, s, "cast whis elder");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((false, false, None)),
        "no charm, no owner link, no suppression"
    );
    let slots = core.monster_active_spells(m).expect("wyrm lives");
    assert!(slots.iter().all(|slot| slot.spell.is_none()), "no slot entry");
    assert_eq!(core.current_mana(s), 96, "full costs stay paid");
    // The only output is the generic cast pair the shared tail renders.
    // DIVERGENCE (noted, not fixed here): the DLL prints NOTHING in this
    // case — its castmsgb comes from display_spell_success inside
    // add_cast_spell_to_monster, which never runs.
    assert!(!shown.contains("attempt to cast"), "no fail line: {shown:?}");
    assert!(!shown.contains("resist"), "no resist line: {shown:?}");
}

#[test]
fn the_silent_gate_costs_exactly_the_same_draws() {
    // §1.5: the gate sits INSIDE the ability-apply loop, downstream of
    // every roll — success, save, magnitude and duration are all drawn
    // before it. A gated-out cast must therefore leave the shared stream
    // exactly where a landing one does; the probe is a second cast whose
    // rolled band values would shift if it did not. The probe cast needs
    // a fresh round — a benign monster cast spends the one-per-round bit
    // (43498-43509) — but the round is driven the same way in both worlds,
    // so the two RNG streams stay aligned.
    let probe_after = |template: MonsterId, first: &str| {
        let (mut core, s, _m) = setup(template);
        cast(&mut core, s, first);
        let probe = core.spawn_monster(WARY, TOWER).expect("hound");
        energy_round(&mut core);
        cast(&mut core, s, "cast bind hound");
        core.monster_active_spells(probe).expect("hound lives")[0]
    };
    let landed = probe_after(RAT, "cast bind rat");
    let gated = probe_after(ELDER, "cast bind elder");
    assert_eq!(landed.spell, Some(BIND), "the probe must actually land");
    assert_eq!(
        (landed.value, landed.remaining),
        (gated.value, gated.remaining),
        "the gate is downstream of every draw"
    );
}

#[test]
fn a_non_monster_match_type_never_charms() {
    // 43797: the case-6 body requires match type 4, 6 or 8. Match 0 (a
    // single-scope spell) reaches the same apply loop and does nothing.
    let (mut core, s, m) = setup(RAT);
    cast(&mut core, s, "cast leas rat");
    assert_eq!(core.debug_monster_charm(m), Some((false, false, None)));
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert!(slots.iter().all(|slot| slot.spell.is_none()), "no slot entry");
}

// --- §1.1 the save-stat swap ---

/// One cast of `line` at a freshly spawned `template`, on its own RNG
/// seed — the save roll is a live draw, so the sample is spread across
/// `seed` values rather than across rounds of one world.
fn one_cast(template: MonsterId, line: &str, seed: u64) -> (String, Core, SessionId, MonsterInstanceId) {
    let mut core = Core::new(world(), seeded(seed));
    let m = core.spawn_monster(template, TOWER).expect("fixture template");
    let s = core.attach_player(caster());
    core.drain_events();
    let shown = cast(&mut core, s, line);
    (shown, core, s, m)
}

/// How many of `n` seeds resisted.
fn resists(template: MonsterId, line: &str, n: u64) -> usize {
    (1..=n)
        .filter(|seed| one_cast(template, line, *seed).0.contains("resist"))
        .count()
}

#[test]
fn enslave_saves_against_charmres_not_mr() {
    // 43311: ability 6 preloads `local_34` from the template's charmres.
    // The wary hound's M.R. 200 would resist 98 times in 100; its
    // charmres 1 halves to a threshold of 0, which genrdn(1,100) can
    // never meet — so an ability-6 spell NEVER resists on it.
    assert_eq!(resists(WARY, "cast thra hound", 20), 0, "charmres 1 = no resist");
    // ... and the same monster still resists a NON-Enslave save-Always
    // spell, which keeps reading M.R. (43387-43392). The swap must not
    // leak.
    assert!(
        resists(WARY, "cast hold hound", 20) >= 15,
        "a spell without ability 6 keeps the M.R. stat"
    );
}

#[test]
fn charmres_resist_prints_the_resist_line_and_applies_nothing() {
    // The mule's M.R. is 0 (floored at 1 = never resists) and its
    // charmres is 196 -> min(98, 98): the ability-6 save resists all but
    // the two top rolls, and a resisted charm writes no state at all.
    assert!(
        resists(STUBBORN, "cast thra mule", 20) >= 15,
        "charmres 196 = the capped 98% resist"
    );
    let mut seen = false;
    for seed in 1..=20 {
        let (shown, core, _s, m) = one_cast(STUBBORN, "cast thra mule", seed);
        if !shown.contains("resist") {
            continue;
        }
        seen = true;
        assert_eq!(
            core.debug_monster_charm(m),
            Some((false, false, None)),
            "a resisted charm applies nothing: {shown:?}"
        );
        let slots = core.monster_active_spells(m).expect("mule lives");
        assert!(slots.iter().all(|slot| slot.spell.is_none()), "no slot");
    }
    assert!(seen, "the fixture must resist at least once");
}

#[test]
fn charmres_zero_falls_back_to_the_mr_stat() {
    // THE DECOMPILE'S OWN EDGE (43311 + 43387): the ability-6 preload
    // writes charmres into `local_34`, but the M.R. default fires on
    // `local_34 == 0` — so a charmres-0 template (48 shipped) saves with
    // M.R. after all. The hollow husk's M.R. 200 resists ~98%.
    assert!(
        resists(HOLLOW, "cast thra husk", 20) >= 15,
        "charmres 0 is not a free charm — the M.R. default fires"
    );
}

// --- §1.4 the slot-full edge ---

#[test]
fn a_full_slot_table_still_lands_a_permanent_charm() {
    // 38281-38288 returns -1 after printing the plain fail line; the
    // case-6 caller (43810-43820) ignores the return and writes the charm
    // triple anyway — a fully slotted monster keeps a TIMERLESS charm.
    //
    // A benign monster cast CONSUMES the one-per-round permission bit
    // (43498-43509), so filling the table takes one round per filler.
    let (mut core, s, m) = setup(RAT);
    for i in 0..5 {
        cast(&mut core, s, &format!("cast fil{i} rat"));
        energy_round(&mut core);
    }
    let before = core.monster_active_spells(m).expect("rat lives");
    let shown = cast(&mut core, s, "cast ensl rat");
    assert!(
        shown.contains("You attempt to cast enslave, but fail.\n"),
        "the slot-full fail line: {shown:?}"
    );
    assert!(!shown.contains("You cast enslave on"), "no success pair: {shown:?}");
    assert_eq!(
        core.monster_active_spells(m).expect("rat lives"),
        before,
        "no Enslave slot"
    );
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "the charm lands anyway — permanent and timerless"
    );
}

// --- §1.5 draw order ---

#[test]
fn the_save_draw_precedes_the_magnitude_and_duration_rolls() {
    // Order: success roll (43410) -> save (43600) -> magnitude (43696) ->
    // duration (38252). The two fixtures differ ONLY in save class, so
    // the save-Always twin consumes one extra draw AHEAD of the magnitude
    // and duration bands — both rolled results must shift. (Neither cast
    // can resist: the rat's charmres 40 halves to a threshold of 20 and
    // the wary hound is not in play here, so a shifted-but-equal result
    // would mean the magnitude was drawn before the save.)
    let slot = |line: &str| {
        let (mut core, s, m) = setup(WARY); // charmres 1: the save never resists
        cast(&mut core, s, line);
        let slots = core.monster_active_spells(m).expect("hound lives");
        (slots[0].value, slots[0].remaining)
    };
    let no_save = slot("cast bind hound");
    let with_save = slot("cast bins hound");
    assert_ne!(
        no_save, with_save,
        "the save draw sits between the success roll and the value/duration bands"
    );
}

// --- §4 release paths ---

/// Tick until `done` or `limit` ticks pass, accumulating everything the
/// owner sees. Returns (ticks spent, output).
fn tick_until(
    core: &mut Core,
    s: SessionId,
    limit: u32,
    mut done: impl FnMut(&Core) -> bool,
) -> (u32, String) {
    let mut shown = String::new();
    for t in 1..=limit {
        core.tick();
        shown.push_str(&text_to(&core.drain_events(), s));
        if done(core) {
            return (t, shown);
        }
    }
    (limit, shown)
}

/// Swing at `word` until one hit LANDS — the §4.3 release lives in the
/// post-damage survivor branch (26513), so a whiffed round does nothing.
fn melee_until_a_hit(core: &mut Core, s: SessionId, word: &str, m: MonsterInstanceId) {
    let before = core.monster_hp(m).expect("target lives");
    core.input(s, &format!("attack {word}"));
    core.drain_events();
    let (_, _) = tick_until(core, s, 60, |c| {
        c.monster_hp(m).is_none_or(|hp| hp < before)
    });
    assert!(
        core.monster_hp(m).is_none_or(|hp| hp < before),
        "the fixture must land a swing"
    );
}

// §4.1 — timer expiry

#[test]
fn charm_expiry_releases_the_triple_silently() {
    // perform_spell_termination_monster_upkeep case 6 (44988-44995): dirty
    // byte SET, owner link emptied, suppression off, charmed bit off — and
    // not one line of output to anybody. The ex-pet is neutral: it holds
    // no grudge against the caster who enslaved it.
    let (mut core, s, m) = setup(RAT);
    cast(&mut core, s, "cast ensl rat");
    let (_, shown) = tick_until(&mut core, s, 400, |c| {
        c.debug_monster_charm(m) == Some((false, false, None))
    });
    assert_eq!(
        core.debug_monster_charm(m),
        Some((false, false, None)),
        "the 60-tick timer must run out and reverse the triple"
    );
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert!(slots.iter().all(|slot| slot.spell.is_none()), "slot cleared at expiry");
    assert!(shown.is_empty(), "the reversal is silent: {shown:?}");
}

// §4.2 — leash give-up / owner logout

#[test]
fn owner_logout_releases_a_slotted_pet_completely() {
    // 19412-19415: an offline owner bumps the give-up counter every fast
    // tick, and past 15 (19446-19487) the non-0x25 branch clears the
    // counter and the owner link, then clears the charmed bit and
    // TERMINATES every ability-6 slot — which is what turns the
    // suppression byte off, since the give-up branch never writes it.
    let (mut core, s, m) = setup(MUTT);
    cast(&mut core, s, "cast ensl mutt");
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
    core.detach(s);
    core.drain_events();
    let (ticks, _) = tick_until(&mut core, s, 40, |c| {
        c.debug_monster_charm(m).is_some_and(|t| !t.0)
    });
    assert!((16..=20).contains(&ticks), "the ~16 s window (19446), got {ticks}");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((false, false, None)),
        "the slot sweep runs the full §4.1 reversal"
    );
    let slots = core.monster_active_spells(m).expect("mutt lives");
    assert!(slots.iter().all(|slot| slot.spell.is_none()), "the Enslave slot is swept");
}

#[test]
fn owner_logout_leaves_a_slotless_pets_suppression_set() {
    // THE DECOMPILE'S LITERAL SHAPE (19452-19487): the give-up branch
    // writes `+0x1a = 0` and clears the charmed bit itself, but `+0x116`
    // is only ever cleared by the slot TERMINATION. An instant-Enslave pet
    // has no ability-6 slot, so the sweep finds nothing and the ex-pet
    // ages out still suppressed — a nameless, unsuppressible loiterer.
    let (mut core, s, m) = setup(MUTT);
    cast(&mut core, s, "cast snap mutt");
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
    core.detach(s);
    core.drain_events();
    tick_until(&mut core, s, 40, |c| c.debug_monster_charm(m).is_some_and(|t| !t.0));
    assert_eq!(
        core.debug_monster_charm(m),
        Some((false, true, None)),
        "no slot to terminate: suppression survives the release"
    );
}

#[test]
fn a_roam_0x25_pet_despawns_instead_of_releasing() {
    // 19448-19450: the class-0x25 arm of the same give-up branch calls
    // FUN_004298ec and returns — no reversal, no monster.
    let (mut core, s, m) = setup(STRAY);
    cast(&mut core, s, "cast ensl cur");
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
    core.detach(s);
    core.drain_events();
    tick_until(&mut core, s, 40, |c| c.debug_monster_charm(m).is_none());
    assert_eq!(core.debug_monster_charm(m), None, "silently despawned");
}

// §2.1 — the follow roll is skipped for a pet

#[test]
fn a_charmed_pet_follows_without_the_aggression_roll() {
    // 19422-19423: `(mon+0x128 & 1) == 0 && genrdn(0,100) >= aggression`
    // — the roll is only EVALUATED for a non-charmed monster, so the
    // charmed bit both skips the draw and makes the refusal impossible.
    // The fixture's aggression is 0, which no `genrdn(0,100)` can ever
    // beat: an uncharmed monster would refuse (and give up) every single
    // tick, so arrival in DEN is only reachable through the skip.
    let (mut core, s, m) = setup_at(MUTT, CELL);
    cast(&mut core, s, "cast ensl mutt");
    core.input(s, "n");
    core.drain_events();
    let (_, _) = tick_until(&mut core, s, 20, |c| c.monster_location(m) == Some(DEN));
    assert_eq!(core.monster_location(m), Some(DEN), "the pet always follows");
    // ... and having followed, it never ages out: the counter only bumps
    // on a refusal.
    tick_until(&mut core, s, 30, |_| false);
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "a following pet never reaches the give-up window"
    );
}

#[test]
fn the_charmed_follow_skips_the_draw_and_not_merely_the_branch() {
    // `(mon+0x128 & 1) == 0 && genrdn(0,100) >= aggression` is a C `&&`:
    // on a charmed monster the left operand is false and `genrdn` is
    // never CALLED. The sibling test above proves only that the refusal
    // is unreachable — rewriting our gate to draw first and test after
    // would still pass it, while silently shifting every seeded golden
    // downstream of a pursuit tick.
    //
    // So measure the stream directly. Both arms are the same template in
    // the same rooms taking the same single pursuit step; they differ
    // only in the charmed bit, and the setup draws are excluded because
    // the counter is sampled after it. KEEN's aggression 100 makes the
    // control follow too — `genrdn(0,100) >= 100` is false for every
    // value the generator can produce — so the two arms run the same
    // path and the delta is the roll and nothing else.
    let follow_draws = |charm: bool| {
        let (mut core, s, m) = setup_at(KEEN, CELL);
        if charm {
            // The INSTANT arm: a pet with no slot, so the medium upkeep
            // has nothing to walk in either arm.
            cast(&mut core, s, "cast snap keen");
        } else {
            core.debug_lock_monster(m, s);
        }
        assert_eq!(core.debug_monster_charm(m).map(|t| t.0), Some(charm));
        core.input(s, "n");
        core.drain_events();
        let before = core.debug_rng_draws();
        let (ticks, _) = tick_until(&mut core, s, 20, |c| c.monster_location(m) == Some(DEN));
        assert_eq!(core.monster_location(m), Some(DEN), "both arms must follow");
        (ticks, core.debug_rng_draws() - before)
    };
    let (pet_ticks, pet_draws) = follow_draws(true);
    let (grudge_ticks, grudge_draws) = follow_draws(false);
    assert_eq!(pet_ticks, grudge_ticks, "the two arms must take the same path");
    assert_eq!(
        grudge_draws,
        pet_draws + 1,
        "the aggression follow-roll is DRAWN for a grudge holder and not \
         drawn at all for a pet (pet {pet_draws}, grudge {grudge_draws})"
    );
}

// §4.3 — the owner attacks its own pet

#[test]
fn owner_melee_releases_a_slotted_pet() {
    // 26527-26562: charmed + `sameas(mon+0x1a, attacker)` -> suppression
    // off FIRST, then the charmed bit, then the ability-6 slot sweep —
    // whose termination also empties the owner link.
    //
    // The whole triple is pinned, not just the charmed bit: the engage
    // lock (26230) lives in the OTHER arm of `if (DAT_004877f4 == '\0')`
    // (26112) and can never run in the same call, so nothing re-grudges
    // the ex-pet onto the owner afterwards. §4.1's "released monster is
    // NEUTRAL" is observable here, and only here, on the melee path.
    let (mut core, s, m) = setup(MUTT);
    cast(&mut core, s, "cast ensl mutt");
    energy_round(&mut core);
    melee_until_a_hit(&mut core, s, "mutt", m);
    assert_eq!(
        core.debug_monster_charm(m),
        Some((false, false, None)),
        "the slot sweep empties the owner link and nothing re-locks it"
    );
    let slots = core.monster_active_spells(m).expect("mutt lives");
    assert!(
        slots.iter().all(|slot| slot.spell != Some(ENSLAVE)),
        "the Enslave slot is terminated and cleared"
    );
}

#[test]
fn the_slot_sweep_runs_the_whole_termination_handler_not_just_case_6() {
    // 26548/19470 hand `perform_spell_termination_monster_upkeep` the
    // WHOLE spell record once they spot an ability-6 row, so its case
    // 0x13 (Poison, 45003-45008) fires too. Reaching for the charm
    // reversal alone is equivalent on shipped data — none of the four
    // Enslave spells carries an ability-19 row — but it is a real
    // divergence and a DRY break with the expiry path, which always went
    // through the full handler.
    let (mut core, s, m) = setup(MUTT);
    cast(&mut core, s, "cast veno mutt");
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
    assert_eq!(
        core.monster_poison(m),
        Some(5),
        "the apply sets the counter"
    );
    energy_round(&mut core);
    melee_until_a_hit(&mut core, s, "mutt", m);
    assert_eq!(
        core.debug_monster_charm(m),
        Some((false, false, None)),
        "the sweep still runs the charm reversal"
    );
    assert_eq!(
        core.monster_poison(m),
        Some(0),
        "and drains the counter the same slot put there"
    );
}

#[test]
fn owner_melee_leaves_a_slotless_pet_as_a_grudge_holder() {
    // §4.3's asymmetry: an instant-Enslave pet has no ability-6 slot, so
    // the sweep never empties `+0x1a` — and the branch cleared `+0x116`
    // on the way in. The ex-pet keeps its owner as a TARGET with
    // suppression off: a full grudge monster hostile to its former owner.
    let (mut core, s, m) = setup(MUTT);
    cast(&mut core, s, "cast snap mutt");
    energy_round(&mut core);
    melee_until_a_hit(&mut core, s, "mutt", m);
    assert_eq!(
        core.debug_monster_charm(m),
        Some((false, false, Some(s))),
        "released into a grudge, not into neutrality"
    );
}

#[test]
fn another_players_swing_at_a_pet_changes_nothing() {
    // The charmed arm at 26527 is guarded by `sameas` on the OWNER's
    // name: a different attacker takes neither the release nor the
    // ordinary retaliation lock (that lives in the non-charmed if-half,
    // 26514-26525) — the pet does not even turn on them.
    let (mut core, s, m) = setup(MUTT);
    cast(&mut core, s, "cast ensl mutt");
    let other = core.attach_player(caster_named("Vex", TOWER));
    core.drain_events();
    energy_round(&mut core);
    melee_until_a_hit(&mut core, other, "mutt", m);
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "somebody else's pet is untouchable state-wise"
    );
}

// §2.4 / 43752 — the cast-damage retaliation twin

#[test]
fn a_damage_cast_grudges_a_pet_without_releasing_it() {
    // THREE different twins inside one function. `cast_monster_target`'s
    // ENTRY grudges (43260-43271, its 43335 evil-points sibling and the
    // 43470 duration-0 engage arm) all open with `(mon+0x128 & 1) == 0`
    // — a pet is never locked there, exactly like the melee twins at
    // 26230/26514. Its post-DAMAGE twin (43750-43766, and the area copies
    // at 40371/40601) has NO charmed check at all: it rolls aggression,
    // overwrites the name link and clears `+0x116` — while LEAVING the
    // charmed bit set. So a damage spell does not release the pet; it
    // turns it hostile and leaves it charmed (never wanders, never rolls
    // to follow).
    //
    // MEASURED FROM A BYSTANDER'S CAST, and it has to be (this changed
    // when §2.2's assist branch landed): an offensive cast ENGAGES
    // autocombat on its target, so an owner searing its own pet leaves
    // the owner's autocombat record pointing AT the pet — and
    // `FUN_0044cc65` turns exactly that into the self-release (46929) on
    // the very next driver pass. The pet would come out released by a
    // completely different mechanism (see
    // `owner_autocombat_on_pet_releases_as_friend`) before the twin's
    // grudge could be read. Vex's cast leaves the owner idle, so only the
    // twin is in the sample — and it sharpens the entry-grudge assertion
    // too: a stranger's cast does not overwrite the OWNER's link either.
    //
    // KEEN, not MUTT: see the const's doc. Aggression 100 makes the
    // `genrdn(1,100) < aggression` leg true unconditionally and
    // behaviour 1 sits outside our shared body's `behaviour in {3,0,4}`
    // clause — which the cast twin does not have — so the final assertion
    // rides on the roll and nothing else.
    let (mut core, s, m) = setup(KEEN);
    cast(&mut core, s, "cast ensl keen");
    let other = core.attach_player(caster_named("Vex", TOWER));
    core.drain_events();
    energy_round(&mut core);
    let shown = cast(&mut core, other, "cast sear keen");
    assert!(shown.contains("*Combat Engaged*"), "offensive casts engage: {shown:?}");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "the entry grudge skips a charmed monster"
    );
    let hp = core.monster_hp(m).expect("keen mutt lives");
    tick_until(&mut core, other, 40, |c| c.monster_hp(m).is_some_and(|h| h < hp));
    assert!(core.monster_hp(m).is_some_and(|h| h < hp), "the cast must land");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, false, Some(other))),
        "the damage twin locks onto the attacker without clearing the charmed bit"
    );
}

#[test]
fn an_area_damage_cast_grudges_somebody_elses_pet() {
    // The AREA copies of that twin (`cast_no_target` 40371-40384 and
    // 40600-40613) are the same shape: `check_kill_monster`, then a
    // roam-class gate and `genrdn(1,100) < mon+0x42`, with no charmed
    // check and — unlike the single-target 43752 arm — no null-template
    // clause and no `roam == 5` carve-out either. A pet reaching this
    // twin is just another body: it takes the damage and the grudge and
    // stays charmed.
    //
    // MEASURED FROM A BYSTANDER'S CAST, and — like the single-target
    // sibling above — it has to be, for a second and completely separate
    // reason: `is_valid_monster_target` (38477-38482) drops YOUR pet out
    // of a match-9/12 area sweep before any of this runs, so the owner
    // can never reach the twin at all (`an_area_cast_skips_your_own_pet`
    // pins that). Vex's pet is not Vex's problem: the name compare
    // misses, the fall-through admits the body, and the twin fires.
    //
    // This pins `CharmedExemption::Ignored` at the area call site, which
    // is otherwise reachable but unasserted — flipping that tag to
    // `Exempt` leaves the pet at `(true, true, Some(s))` and fails here.
    let (mut core, s, m) = setup(KEEN);
    cast(&mut core, s, "cast ensl keen");
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
    let other = core.attach_player(caster_named("Vex", TOWER));
    core.drain_events();
    energy_round(&mut core);
    let hp = core.monster_hp(m).expect("keen mutt lives");
    cast(&mut core, other, "cast gale");
    assert!(
        core.monster_hp(m).is_some_and(|h| h < hp),
        "the area sweep must hit a pet that is not the caster's"
    );
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, false, Some(other))),
        "the area twin locks the pet without clearing the charmed bit"
    );
}

#[test]
fn a_class_5_guardian_holding_a_lock_still_spends_the_roll() {
    // Not charm, but the same shared lock body. `roam != 5 ||
    // mon+0x1a == 0` is the LAST operand of the `&&` chain in every twin
    // (26234/26520/43265/43340/43474), so the `genrdn(1,100)` in front of
    // it has already been spent by the time a class-5 guardian's existing
    // lock cancels the write. We used to return before the draw.
    //
    // Measured through the AREA cast because it is the one damaging path
    // with a fixed draw sequence: no to-hit, no dodge, no magnitude band
    // (GALE's damage row is a literal), so the retaliation roll is the
    // only thing that can move the counter.
    let draws = |roam: i16| {
        let mut content = world();
        let mut guard = monster(GUARD, "gate guardian", 1, 40);
        guard.hitpoints = 500;
        guard.aggression = 100;
        guard.behaviour = 1;
        guard.roam_class = roam;
        content.add_monster(guard);
        let mut core = Core::new(content, config());
        let m = core.spawn_monster(GUARD, TOWER).expect("guardian");
        let s = core.attach_player(caster_named("Zin", TOWER));
        core.drain_events();
        // A lock already in place: the class-5 clause only bites here.
        core.debug_lock_monster(m, s);
        let before = core.debug_rng_draws();
        cast(&mut core, s, "cast gale");
        core.debug_rng_draws() - before
    };
    let roamer = draws(0);
    assert!(roamer > 0, "the control must draw its retaliation roll");
    assert_eq!(
        draws(5),
        roamer,
        "a locked class-5 guardian declines the WRITE, not the ROLL"
    );
}

// §4.4 — what does NOT release

#[test]
fn owner_death_keeps_the_pet() {
    // check_kill_user (12992) contains no monster sweep: the pet outlives
    // its owner's death untouched. (The respawned owner is the same
    // session, so pursuit simply resumes.)
    let mut core = Core::new(world(), CoreConfig { recall_location: TOWER, ..config() });
    let m = core.spawn_monster(MUTT, TOWER).expect("mutt");
    let s = core.attach_player(caster());
    core.drain_events();
    cast(&mut core, s, "cast ensl mutt");
    core.spawn_monster(EXEC, TOWER).expect("executioner");
    let (_, shown) = tick_until(&mut core, s, 400, |c| {
        c.player_snapshot(s).lives < 9
    });
    assert!(shown.contains("You have been killed!"), "the owner must die: {shown:?}");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "death releases nothing"
    );
    let slots = core.monster_active_spells(m).expect("mutt lives");
    assert_eq!(slots[0].spell, Some(ENSLAVE), "the charm slot survives");
}

// §2.2 — the pet assist branch (`FUN_0044cc65`, 46917-46965), reached
// from the driver's named-link ladder (`FUN_00423863` 20512-20517).

/// Stage: a pet template and the straw dummy in the tower with the owner.
fn kennel(pet: MonsterId) -> (Core, SessionId, MonsterInstanceId, MonsterInstanceId) {
    let mut core = Core::new(world(), config());
    let p = core.spawn_monster(pet, TOWER).expect("pet template");
    let q = core.spawn_monster(BAG, TOWER).expect("straw dummy");
    let s = core.attach_player(caster_named("Zin", TOWER));
    core.drain_events();
    (core, s, p, q)
}

#[test]
fn pet_assists_owner_target() {
    // 46955-46960: the owner's autocombat record names a MONSTER, so the
    // pet swings at it — `attack_monster_monster`, deterministically,
    // every driver pass, with no roll in front of the decision.
    let (mut core, s, pet, bag) = kennel(HOUND);
    cast(&mut core, s, "cast ensl dog");
    assert_eq!(core.debug_monster_charm(pet), Some((true, true, Some(s))));
    energy_round(&mut core);
    core.input(s, "attack dummy");
    core.drain_events();
    // Driven by real ticks: this is the branch's only proof that the
    // combat driver actually reaches it.
    let (_, shown) = tick_until(&mut core, s, 20, |_| false);
    assert!(
        shown.contains("War dog just attacked straw dummy!"),
        "the pet must swing at the owner's target: {shown:?}"
    );
    assert!(
        core.monster_hp(bag).is_some_and(|hp| hp < 500),
        "and the damage must land on the quarry"
    );
    // The assist is not an acquisition: the pet keeps the whole triple.
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((true, true, Some(s))),
        "assisting changes no state"
    );
}

#[test]
fn pet_idle_when_owner_idle() {
    // 20514 (`is_inside_autocombat`) and 46925/46927: an owner with no
    // autocombat target leaves the pet doing NOTHING — and doing it
    // without touching the shared stream. A pet must never fall through
    // to the suppressed-aggressive arm, so an idle owner is idle hands.
    //
    // The BYSTANDER is what makes that clause load-bearing here. With
    // the owner as the room's only player the friends arm (20494-20511)
    // has no candidate — it excludes the named user — so the test would
    // pass with the charmed arm deleted, proving only that the pet does
    // not attack its own owner. Vex gives the friends arm something to
    // find, so deleting the charmed arm costs draws, lines and Vex's HP.
    let (mut core, s, pet, bag) = kennel(HOUND);
    cast(&mut core, s, "cast ensl dog");
    let other = core.attach_player(caster_named("Vex", TOWER));
    core.drain_events();
    energy_round(&mut core);
    let hp = core.monster_hp(bag);
    let player_hp = core.player_snapshot(s).current_hp;
    let vex_hp = core.player_snapshot(other).current_hp;
    let draws = core.debug_rng_draws();
    for _ in 0..4 {
        core.debug_monster_consider(pet);
    }
    assert_eq!(
        core.debug_rng_draws(),
        draws,
        "an idle owner costs the pet not one draw"
    );
    assert!(core.drain_events().is_empty(), "and not one line");
    assert_eq!(core.monster_hp(bag), hp, "nothing is swung at");
    assert_eq!(
        core.player_snapshot(s).current_hp,
        player_hp,
        "least of all the owner"
    );
    assert_eq!(
        core.player_snapshot(other).current_hp,
        vex_hp,
        "nor the bystander the friends arm would have found"
    );
}

#[test]
fn a_charmed_class_five_body_never_assists() {
    // ORDER, not membership. The A2 ladder in `FUN_00423863` tests
    // `mon+0x12c == 5` (20477) BEFORE the charmed bit (20512), so a
    // charmed class-5 monster lands in the ward-defence arm — which only
    // ever swings at a player in autocombat against its named user, i.e.
    // PvP, i.e. never here — and NEVER reaches `FUN_0044cc65`. charm.md
    // §2.2's "charmed pets take the FUN_0044cc65 branch instead"
    // describes the FRIENDS arm below it, not this one.
    //
    // WARDEN is HOUND with `roam_class = 5` and nothing else changed, so
    // this is a pure ordering probe: it fails if the arm order is
    // reverted (`else if roam == 5 && !charmed`) AND it fails if the
    // class-5 arm is removed — both hand the body to the charmed arm,
    // which assists loudly.
    let (mut core, s, pet, bag) = kennel(WARDEN);
    cast(&mut core, s, "cast ensl warden");
    assert_eq!(core.debug_monster_charm(pet), Some((true, true, Some(s))));
    energy_round(&mut core);
    core.input(s, "attack dummy");
    core.drain_events();
    let hp = core.monster_hp(bag);
    let draws = core.debug_rng_draws();
    for _ in 0..6 {
        core.debug_monster_consider(pet);
    }
    assert_eq!(
        core.debug_rng_draws(),
        draws,
        "the ward arm is a pure no-op: not one draw"
    );
    assert!(
        core.drain_events().is_empty(),
        "and not one monster-vs-monster line"
    );
    assert_eq!(
        core.monster_hp(bag),
        hp,
        "the owner's quarry is never swung at"
    );
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((true, true, Some(s))),
        "and the triple is left exactly as the charm wrote it"
    );
}

#[test]
fn an_owner_cast_at_its_own_pet_self_releases() {
    // The chain `a_damage_cast_grudges_a_pet_without_releasing_it`
    // reasons about, executed instead of narrated: an offensive cast
    // ENGAGES autocombat on its target (43411-43421), so an owner
    // searing its own pet leaves its autocombat record pointing AT the
    // pet — and 46929 turns exactly that into the self-release on the
    // very next driver pass. This is why that test has to measure the
    // damage twin from a BYSTANDER's cast.
    //
    // The pet here is SLOTTED, so the release runs the whole
    // termination handler and the triple goes with it.
    let (mut core, s, pet, _bag) = kennel(HOUND);
    cast(&mut core, s, "cast ensl dog");
    assert_eq!(core.debug_monster_charm(pet), Some((true, true, Some(s))));
    energy_round(&mut core);
    let shown = cast(&mut core, s, "cast sear dog");
    assert!(
        shown.contains("*Combat Engaged*"),
        "the offensive cast must engage, not resolve: {shown:?}"
    );
    core.debug_monster_consider(pet);
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((false, false, None)),
        "one driver pass later the pet has released itself"
    );
}

#[test]
fn pet_never_attacks_others() {
    // The suppressed-aggressive arm (20494-20511) is guarded by
    // `(mon+0x128 & 1) == 0`: it is for non-charmed "friends" (§2.2 last
    // paragraph), and a pet takes the `FUN_0044cc65` arm instead. A
    // bystander is safe from somebody else's pet whether the owner is
    // idle or engaged elsewhere.
    let (mut core, s, pet, _bag) = kennel(HOUND);
    cast(&mut core, s, "cast ensl dog");
    let other = core.attach_player(caster_named("Vex", TOWER));
    core.drain_events();
    let vex_hp = core.player_snapshot(other).current_hp;
    for _ in 0..4 {
        core.debug_monster_consider(pet);
    }
    assert_eq!(
        core.player_snapshot(other).current_hp,
        vex_hp,
        "an idle owner's pet does not take the friend arm"
    );
    core.input(s, "attack dummy");
    core.drain_events();
    for _ in 0..4 {
        core.debug_monster_consider(pet);
    }
    assert_eq!(
        core.player_snapshot(other).current_hp,
        vex_hp,
        "nor does an engaged owner's pet"
    );
    // The bystander is not blind — the assist's room line reaches them —
    // but nothing in that stream is aimed AT them.
    let seen = text_to(&core.drain_events(), other);
    assert!(
        seen.lines()
            .all(|l| l.trim().is_empty() || l.contains("straw dummy")),
        "the only lines a bystander gets are the pet's swings at the quarry: {seen:?}"
    );
}

#[test]
fn owner_autocombat_on_pet_releases_as_friend() {
    // 46929-46953: the owner's autocombat target IS the pet, so the pet
    // releases itself on the next driver pass — charmed bit off, the
    // ability-6 slots swept. What is NOT here is the melee twin's
    // `+0x116 = 0` (26527): suppression SURVIVES, so a slotless ex-pet
    // degrades into a "friend" (link + suppression) rather than into the
    // grudge holder the melee path leaves behind.
    let (mut core, s, pet, _bag) = kennel(HOUND);
    cast(&mut core, s, "cast snap dog"); // instant: no slot for the sweep
    assert_eq!(core.debug_monster_charm(pet), Some((true, true, Some(s))));
    energy_round(&mut core);
    core.input(s, "attack dog");
    core.drain_events();
    core.debug_monster_consider(pet);
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((false, true, Some(s))),
        "released as a FRIEND: suppression stays set and the link survives"
    );

    // The SLOTTED twin, for contrast: this arm still never writes
    // `+0x116` itself — but the slot it sweeps goes through
    // `perform_spell_termination_monster_upkeep`, whose case 6 IS the
    // full §4.1 reversal, so suppression and the link go with it. The
    // "friend" outcome above is the slotless asymmetry (§4.3) and
    // nothing else.
    let (mut core, s, pet, _bag) = kennel(HOUND);
    cast(&mut core, s, "cast ensl dog");
    energy_round(&mut core);
    core.input(s, "attack dog");
    core.drain_events();
    core.debug_monster_consider(pet);
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((false, false, None)),
        "a slotted pet's release runs the whole termination"
    );
}

#[test]
fn a_pool_0_pet_draws_every_pass_and_never_swings() {
    // THE ENERGY TRAP, now live: `attack_monster_monster`'s pay gate sits
    // AFTER `calculate_attack` (27241 then 27242), so a template whose
    // form-0 EU exceeds its whole pool resolves a swing it can never pay
    // for. 21 shipped templates are shaped that way and `bishop`,
    // `priest` and `boatman` (pool 0, cost 5, `charmlvl` 0) are charmable
    // by anyone — so a pet like this burns draws on EVERY driver pass,
    // forever, and never lands a hit. DLL-faithful; pinned, not fixed.
    let (mut core, s, pet, bag) = kennel(BISHOP);
    cast(&mut core, s, "cast ensl bishop");
    assert_eq!(core.debug_monster_charm(pet), Some((true, true, Some(s))));
    energy_round(&mut core);
    core.input(s, "attack dummy");
    core.drain_events();
    let hp = core.monster_hp(bag);
    let deltas: Vec<u64> = (0..4)
        .map(|_| {
            let before = core.debug_rng_draws();
            core.debug_monster_consider(pet);
            core.debug_rng_draws() - before
        })
        .collect();
    assert!(
        deltas.iter().all(|d| *d > 0),
        "every pass pays for a resolution it cannot use: {deltas:?}"
    );
    assert_eq!(core.monster_hp(bag), hp, "and the swing never lands");
    assert_eq!(core.monster_energy(pet), Some(0), "nothing is ever paid");
    assert!(core.drain_events().is_empty(), "silently, forever");
}

// §2.3 — targeting. TWO INDEPENDENT GATES, on DISJOINT call paths.
//
// 1. `find_action_target`'s `0x800` bit (63776 / 63820) — an ORDERING,
//    not an exclusion. The monster block runs TWICE when the bit is set:
//    pass 1 skips `mon+0x128 & 1`, pass 2 scans ONLY charmed monsters.
//    Both passes precede the `0x2` user scan, so the bit reorders
//    nothing but monster-against-monster. Carriers: `cmd_any_attack`
//    `0x883` (49590) and `cmd_cast`'s preferred masks `0x801` (match 4),
//    `0x803` (match 8) and `0xf837` (match 6). NOT carried by the
//    dispatcher's universal retry `0xf037` (59265-59271) — but that
//    asymmetry has no observable surface, because every match type
//    `cast_monster_target` ACCEPTS ({4,6,8}, 43205) already searches
//    monsters in its preferred mask, so the retry only ever lands on a
//    monster for match types that then refuse it by KIND. Modelled
//    literally all the same.
// 2. `is_valid_monster_target` (38430) — a genuine exclusion, and it
//    lives ONLY on the AREA sweeps (`count_valid_targets` 38610,
//    `add_duration_spell_to_room` 38707, `add_evil_warnings_to_room`
//    38803 and the eight `cast_no_target` effect arms). Its switch on
//    `spell+0xcc` gives match **9 and 12 only** the charm arm
//    (38477-38488): charmed-or-suppressed AND `mon+0x1a` == your name ->
//    invalid; the same compare on an unsuppressed body makes your
//    grudge-holder always-valid. Match 3/5/11 are waved through at 38461
//    and single-target 4/6/8 never reach the function at all.
//
// So charm.md §2.3's "hostile spells can't target your own pet" is true
// of AREA match 9/12 and of nothing else: a single-target `cast mmis
// rat` prefers a wild rat, and takes the pet when the pet is the only
// rat in the room.

/// The §2.3 staging: an owner, a charmable `war dog` and an inert
/// `wild dog`, in that spawn order — so the pet has the LOWER instance
/// id and wins a single unordered pass.
fn two_dogs() -> (Core, SessionId, MonsterInstanceId, MonsterInstanceId) {
    let mut core = Core::new(world(), config());
    let pet = core.spawn_monster(HOUND, TOWER).expect("war dog");
    let wild = core.spawn_monster(CUR, TOWER).expect("wild dog");
    let s = core.attach_player(caster_named("Zin", TOWER));
    core.drain_events();
    (core, s, pet, wild)
}

fn slotted(core: &Core, m: MonsterInstanceId, spell: SpellId) -> bool {
    core.monster_active_spells(m)
        .is_some_and(|slots| slots.iter().any(|s| s.spell == Some(spell)))
}

#[test]
fn a_monster_cast_prefers_a_wild_body_over_your_pet() {
    // Pass 1 of the `0x801` search skips the pet and finds the wild dog
    // even though the pet is first in the room list.
    let (mut core, s, pet, wild) = two_dogs();
    cast(&mut core, s, "cast ensl war");
    assert_eq!(core.debug_monster_charm(pet), Some((true, true, Some(s))));
    energy_round(&mut core);
    cast(&mut core, s, "cast mark dog");
    assert!(
        slotted(&core, wild, MARK),
        "pass 1 must land on the wild body"
    );
    assert!(!slotted(&core, pet, MARK), "and never on the pet");
}

#[test]
fn a_lone_pet_is_still_a_valid_cast_target() {
    // Pass 2 (63820): with no wild body left, the charmed-only sweep
    // finds the pet and the cast lands on it. `0x800` DEPRIORITISES; it
    // does not hide.
    let (mut core, s, pet) = setup(HOUND);
    cast(&mut core, s, "cast ensl war");
    energy_round(&mut core);
    let shown = cast(&mut core, s, "cast mark dog");
    assert!(
        !shown.contains("You do not see"),
        "a lone pet is findable: {shown:?}"
    );
    assert!(slotted(&core, pet, MARK), "pass 2 resolves the pet");
}

#[test]
fn melee_attack_prefers_a_wild_body_over_your_pet() {
    // `cmd_any_attack` carries `0x800` too (49590, mask `0x883`), so the
    // same ordering decides which dog ATTACK engages. Read through the
    // pet: an owner engaged on the WILD dog leaves the pet assisting and
    // charmed, an owner engaged on the PET releases it (46929).
    let (mut core, s, pet, wild) = two_dogs();
    cast(&mut core, s, "cast ensl war");
    energy_round(&mut core);
    core.input(s, "attack dog");
    core.drain_events();
    let wild_hp = core.monster_hp(wild).expect("wild dog lives");
    core.debug_monster_consider(pet);
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((true, true, Some(s))),
        "the owner engaged the WILD dog, so the pet is untouched"
    );
    assert!(
        core.monster_hp(wild).is_some_and(|hp| hp < wild_hp),
        "and assists against it"
    );
}

#[test]
fn melee_attack_still_finds_a_lone_pet() {
    // §2.3's "physical attacks are allowed" — and they are a release
    // path (§4.3), so pass 2 has to hand ATTACK the pet when the pet is
    // the only match. A find that HID pets would silently disarm the
    // whole melee release family.
    let (mut core, s, pet) = setup(HOUND);
    cast(&mut core, s, "cast ensl war");
    energy_round(&mut core);
    let shown = cast(&mut core, s, "attack dog");
    assert!(
        shown.contains("*Combat Engaged*"),
        "ATTACK must resolve the lone pet: {shown:?}"
    );
    core.debug_monster_consider(pet);
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((false, false, None)),
        "and the engagement releases it on the next driver pass"
    );
}

#[test]
fn an_area_cast_skips_your_own_pet() {
    // 38477-38482: match 12 + charmed + `mon+0x1a` == the caster ->
    // invalid. The rest of the room burns as usual.
    let (mut core, s, pet, bag) = kennel(HOUND);
    cast(&mut core, s, "cast ensl dog");
    energy_round(&mut core);
    let pet_hp = core.monster_hp(pet).expect("pet lives");
    let bag_hp = core.monster_hp(bag).expect("dummy lives");
    cast(&mut core, s, "cast gale");
    assert_eq!(
        core.monster_hp(pet),
        Some(pet_hp),
        "your own pet is not a valid match-12 area victim"
    );
    assert!(
        core.monster_hp(bag).is_some_and(|hp| hp < bag_hp),
        "everything else in the room still takes it"
    );
}

#[test]
fn an_area_cast_skips_a_friend_of_yours() {
    // The exemption is `charmed OR suppressed` (38477), so the "friend"
    // state §4.3 leaves behind — link + `+0x116`, charmed bit gone —
    // is exempt on exactly the same terms.
    let (mut core, s, pet, bag) = kennel(HOUND);
    cast(&mut core, s, "cast snap dog"); // instant: no slot to sweep
    energy_round(&mut core);
    core.input(s, "attack dog");
    core.drain_events();
    core.debug_monster_consider(pet);
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((false, true, Some(s))),
        "staged as a friend: suppressed, named, not charmed"
    );
    energy_round(&mut core); // the melee spent the round pool
    let pet_hp = core.monster_hp(pet).expect("friend lives");
    let bag_hp = core.monster_hp(bag).expect("dummy lives");
    cast(&mut core, s, "cast gale");
    assert_eq!(core.monster_hp(pet), Some(pet_hp), "a friend is exempt too");
    assert!(
        core.monster_hp(bag).is_some_and(|hp| hp < bag_hp),
        "the sweep still ran"
    );
}

#[test]
fn an_area_cast_on_another_match_type_still_hits_your_pet() {
    // The carve-out is the switch label, not "area casts": match 3, 5
    // and 11 take the unconditional `return 1` at 38461 and never reach
    // the charm compare.
    let (mut core, s, pet, bag) = kennel(HOUND);
    cast(&mut core, s, "cast ensl dog");
    energy_round(&mut core);
    let pet_hp = core.monster_hp(pet).expect("pet lives");
    let bag_hp = core.monster_hp(bag).expect("dummy lives");
    cast(&mut core, s, "cast squall");
    assert!(
        core.monster_hp(pet).is_some_and(|hp| hp < pet_hp),
        "match 11 has no charm arm: the pet burns with everything else"
    );
    assert!(core.monster_hp(bag).is_some_and(|hp| hp < bag_hp));
}

#[test]
fn an_area_cast_spares_a_ward_the_caster_is_not_famous_enough_for() {
    // The REST of the 9/12 arm, ported with it because it is the same
    // three-line fall-through (38489-38499): instance roam class 5 or
    // 0x25 spares the body from a caster with fame < 0x28, and
    // behaviour mode 4 spares it outright. Nothing here is charm; it is
    // what `is_valid_monster_target` does once the name compare misses.
    let probe = |roam: i16, behaviour: i16, fame: i16| {
        let mut content = world();
        let mut ward = monster(GUARD, "gate ward", 9999, 40);
        ward.hitpoints = 500;
        ward.roam_class = roam;
        ward.behaviour = behaviour;
        content.add_monster(ward);
        let mut core = Core::new(content, config());
        let m = core.spawn_monster(GUARD, TOWER).expect("ward");
        let s = core.attach_player(caster_named("Zin", TOWER));
        core.set_player_fame(s, fame);
        core.drain_events();
        let hp = core.monster_hp(m).expect("ward lives");
        cast(&mut core, s, "cast gale");
        core.monster_hp(m).is_some_and(|now| now < hp)
    };
    assert!(probe(0, 1, 0), "the control burns");
    assert!(!probe(5, 1, 0), "roam 5 + fame < 0x28 is spared");
    assert!(probe(5, 1, 40), "fame 0x28 lifts it");
    assert!(!probe(0x25, 1, 0), "roam 0x25 + fame < 0x28 is spared");
    assert!(probe(0x25, 1, 40), "and lifts the same way");
    assert!(!probe(0, 4, 400), "behaviour 4 is spared at any fame");
}
