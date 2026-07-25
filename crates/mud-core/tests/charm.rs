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
/// cast-damage retaliation twin (43752-43765). MUTT itself is aggression
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
/// twin (43752-43765), which carries no charmed check at all.
const SEAR: SpellId = SpellId(790);
/// SEAR as an AREA (match 12) — the 40371/40600 copies of that twin,
/// which are equally charm-blind.
const GALE: SpellId = SpellId(795);

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
    for s in [enslave, thrall, hold, snap, whisper, leash, bind, bindsave, sear, gale] {
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
        ENSLAVE, THRALL, HOLD, SNAP, WHISPER, LEASH, BIND, BINDSAVE, SEAR, GALE,
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
    // 26230/26514. Its post-DAMAGE twin (43752-43765, and the area copies
    // at 40371/40600) has NO charmed check at all: it rolls aggression,
    // overwrites the name link and clears `+0x116` — while LEAVING the
    // charmed bit set. So a damage spell from the owner does not release
    // the pet; it turns it hostile and leaves it charmed (never wanders,
    // never rolls to follow).
    //
    // KEEN, not MUTT: see the const's doc. Aggression 100 makes the
    // `genrdn(1,100) < aggression` leg true unconditionally and
    // behaviour 1 sits outside our shared body's `behaviour in {3,0,4}`
    // clause — which the cast twin does not have — so the final assertion
    // rides on the roll and nothing else.
    let (mut core, s, m) = setup(KEEN);
    cast(&mut core, s, "cast ensl keen");
    energy_round(&mut core);
    let shown = cast(&mut core, s, "cast sear keen");
    assert!(shown.contains("*Combat Engaged*"), "offensive casts engage: {shown:?}");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "the entry grudge skips a charmed monster"
    );
    let hp = core.monster_hp(m).expect("keen mutt lives");
    tick_until(&mut core, s, 40, |c| c.monster_hp(m).is_some_and(|h| h < hp));
    assert!(core.monster_hp(m).is_some_and(|h| h < hp), "the cast must land");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, false, Some(s))),
        "the damage twin locks without clearing the charmed bit"
    );
}

#[test]
fn an_area_damage_cast_grudges_the_casters_own_pet() {
    // The AREA copies of that twin (`cast_no_target` 40371-40384 and
    // 40600-40613) are the same shape: `check_kill_monster`, then a
    // roam-class gate and `genrdn(1,100) < mon+0x42`, with no charmed
    // check and — unlike the single-target 43752 arm — no null-template
    // clause and no `roam == 5` carve-out either. Your own pet is just
    // another body in the room: it takes the damage and the grudge and
    // stays charmed.
    //
    // This pins `CharmedLock::Ignored` at the area call site, which is
    // otherwise reachable but unasserted — flipping that tag to `Exempt`
    // leaves the pet at `(true, true, Some(s))` and fails here.
    let (mut core, s, m) = setup(KEEN);
    cast(&mut core, s, "cast ensl keen");
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
    energy_round(&mut core);
    let hp = core.monster_hp(m).expect("keen mutt lives");
    cast(&mut core, s, "cast gale");
    assert!(
        core.monster_hp(m).is_some_and(|h| h < hp),
        "the area sweep must hit the pet"
    );
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, false, Some(s))),
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
