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
//! FIXTURE NOTE — all four shipped Enslave spells (49 song of charming,
//! 55 enslave, 88 control undead, 92 charm animal) carry `spelltype` 3
//! (`TargetMode::Benign`) with match type 4. Our command path picks the
//! monster-target branch off the TARGET MODE, while the DLL picks the cast
//! entry point off the MATCH type (`get_spell_match_type` -> 0x801 for 4),
//! so a benign-mode match-4 spell never reaches `offensive_cast_attempt`
//! here — 158 shipped spells sit in that gap (curse, blind, slow, fear,
//! the charm family). That routing gap is NOT this task's; the fixtures
//! below therefore keep match type 4 (the gate the case-6 arm reads) and
//! use an offensive target mode so the cast routes.

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Element, MatchType, Message, MessageId, Monster,
    MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId, StatBlock,
    TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, MonsterInstanceId, Player, SessionId};

const TOWER: RoomId = RoomId { map: 1, room: 1 };
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
        // Offensive so the cast ROUTES (see the fixture note above); the
        // charm family ships benign-mode.
        target_mode: TargetMode::Offensive0,
        save_class: SaveClass::None,
        base_chance: 200, // auto-success: >= 200 skips the roll
        duration_per_level: 0,
        // 43797: the case-6 gate accepts match types 4/6/8 only.
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

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room { id: TOWER, name: "Tower".into(), ..Default::default() });
    content.add_monster(monster(RAT, "giant rat", 1, 40));
    content.add_monster(monster(ELDER, "elder wyrm", 9999, 40));
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
    for s in [enslave, thrall, hold, snap, whisper, leash, bind, bindsave] {
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
    let book: BTreeMap<SpellId, bool> = [
        ENSLAVE, THRALL, HOLD, SNAP, WHISPER, LEASH, BIND, BINDSAVE,
    ]
    .into_iter()
    .chain((0..5).map(|i| SpellId(FILLER_BASE + i)))
    .map(|s| (s, false))
    .collect();
    Player {
        name: "Zin".into(),
        gender: Gender::Male,
        race: HUMAN,
        class: MAGE,
        level: 3,
        current_hp: 200,
        current_mana: 100,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: TOWER,
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
    let mut core = Core::new(world(), config());
    let m = core.spawn_monster(template, TOWER).expect("fixture template");
    let s = core.attach_player(caster());
    core.drain_events();
    (core, s, m)
}

fn cast(core: &mut Core, s: SessionId, line: &str) -> String {
    core.input(s, line);
    text_to(&core.drain_events(), s)
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
    // An INSTANT offensive cast only ENGAGES at the command (the
    // duration==0 block, 43421-43481); the combat round driver fires it —
    // so the engagement's own grudge lock (target set, suppression
    // cleared) lands FIRST and the charm write overwrites it.
    let (mut core, s, m) = setup(RAT);
    cast(&mut core, s, "cast snap rat");
    assert_eq!(
        core.debug_monster_charm(m),
        Some((false, false, Some(s))),
        "engagement only: the grudge lock, no charm yet"
    );
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
    let slots = core.monster_active_spells(m).expect("rat lives");
    assert!(slots.iter().all(|slot| slot.spell.is_none()), "no slot, no timer");
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
    // rolled band values would shift if it did not.
    let landed = {
        let (mut core, s, _m) = setup(RAT);
        cast(&mut core, s, "cast bind rat");
        let probe = core.spawn_monster(WARY, TOWER).expect("hound");
        core.drain_events();
        cast(&mut core, s, "cast bind hound");
        core.monster_active_spells(probe).expect("hound lives")[0]
    };
    let gated = {
        let (mut core, s, _m) = setup(ELDER);
        cast(&mut core, s, "cast bind elder");
        let probe = core.spawn_monster(WARY, TOWER).expect("hound");
        core.drain_events();
        cast(&mut core, s, "cast bind hound");
        core.monster_active_spells(probe).expect("hound lives")[0]
    };
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
    let (mut core, s, m) = setup(RAT);
    for i in 0..5 {
        cast(&mut core, s, &format!("cast fil{i} rat"));
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
