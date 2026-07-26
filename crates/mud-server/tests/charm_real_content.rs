//! Real-content charm smoke test (M7 slice 5): the whole Enslave chain
//! driven against `re/mmud_wgnt.sqlite` — shipped spells, shipped
//! templates, shipped class/level/alignment gates, no fixtures.
//!
//! The slice's defining risk is SHIPPED-DATA REACHABILITY: every rule in
//! `charm.md` is only worth anything if a real spell can reach a real
//! monster through it. The three targeting predicates of the
//! pre-application eligibility scan (`cast_monster_target` 43295-43376)
//! are the sharpest case, because each of the four shipped Enslave
//! spells carries exactly one of them:
//!
//! | spell | ability rows | predicate |
//! |---|---|---|
//! | 49 song of charming | 6, 108 | refuses `NonLiving` (109) |
//! | 55 enslave | 6, 108 | refuses `NonLiving` (109) |
//! | 88 control undead | 6, 23, 98 | requires `undead != 0` |
//! | 92 charm animal | 6, 80 | requires `Animal` (78) |
//!
//! The templates below are chosen so the two undead-ish predicates
//! DISAGREE on real data, which is the whole reason they are two
//! predicates: `skeleton` (#11) is `undead` 1 AND carries 109, so
//! control undead lands on it while song of charming bounces; `zombie
//! cat` (#769) is `undead` **-1** — the tri-valued column's negative
//! arm, 8 shipped templates — and carries both 78 and 109, so it is
//! simultaneously a legal charm-animal target, a legal control-undead
//! target, and an illegal song-of-charming target.

use std::collections::BTreeMap;

use mud_core::content::{ClassId, MonsterId, RaceId, RoomId, SpellId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, MonsterInstanceId, Player, SessionId};
use mud_server::content_db;

/// "Cavern, Dead End" (map 10) — `attributes` 0 (not a protected room, so
/// the guilt gate stays out of the way), `monstertype` 0 and `permnpc` 0,
/// so the boot fill leaves it empty and every body in it is one this test
/// put there.
const ARENA: RoomId = RoomId { map: 10, room: 42 };

/// `wild dog` — carries ability 78 (`Animal`), `charmlvl` 5, `charmres`
/// 45, 46 HP, a 3-10 attack form and a 1000 energy pool: a real animal
/// that can also serve as a pet.
const WILD_DOG: MonsterId = MonsterId(50);
/// `kobold thief` — no 78, no 109, `undead` 0, `charmlvl` 4. The negative
/// control for all three predicates at once, and the brief's example of
/// what `charm animal` was charming before the scan landed.
const KOBOLD_THIEF: MonsterId = MonsterId(7);
/// `skeleton` — `undead` 1 AND ability 109. Legal for control undead,
/// illegal for song of charming: the pair that separates the two.
const SKELETON: MonsterId = MonsterId(11);
/// `zombie cat` — `undead` **-1**, plus abilities 78 and 109. The proof
/// that the negative arm of the tri-valued column is undead.
const ZOMBIE_CAT: MonsterId = MonsterId(769);
/// `healer` — the shipped punching bag: 1000 HP, `alignment` 4 (passive,
/// never initiates), `type` 3 (lair, fully stationary) and `attacktype_1`
/// 0 (no attack form at all), so every combat line it takes is somebody
/// else's swing. The real-content twin of `charm.rs`'s straw dummy.
const HEALER: MonsterId = MonsterId(73);

const SONG_OF_CHARMING: SpellId = SpellId(49);
const CONTROL_UNDEAD: SpellId = SpellId(88);
const CHARM_ANIMAL: SpellId = SpellId(92);

/// 49 and 92 SHARE the short name `char`, so each caster below is given a
/// one-spell book: the resolution is unambiguous and the class gate that
/// keeps them apart in play (magery group 4 vs 3) is exercised too.
fn caster(name: &str, class: ClassId, level: u16, fame: i16, spell: SpellId) -> Player {
    let book: BTreeMap<SpellId, bool> = [(spell, false)].into_iter().collect();
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class,
        // The spellcasting term is stat-driven per magery group
        // (`stats.rs`): 100s across the board put every group's chance at
        // the 98 cap, so the success roll is not what these tests are
        // measuring. The saving throw still runs — all four shipped
        // Enslave spells are `typeofresists` 2 (always saves) — which is
        // why the landing tests retry.
        stats: StatBlock {
            strength: 100,
            intellect: 100,
            wisdom: 100,
            agility: 100,
            health: 100,
            charm: 100,
        },
        level,
        current_hp: 5000,
        current_mana: 5000,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: ARENA,
        spellbook: book,
        fame,
        ..Default::default()
    }
}

/// Druid (class 13, magery group 3) at level 20 — above `charm animal`'s
/// required level 5 and above every `charmlvl` used here.
fn druid() -> Player {
    caster("Sylva", ClassId(13), 20, 0, CHARM_ANIMAL)
}

/// Bard (class 9, magery group 4) at level 20 — `song of charming` needs
/// level 4.
fn bard() -> Player {
    caster("Lyric", ClassId(9), 20, 0, SONG_OF_CHARMING)
}

/// Priest (class 5, magery group 2, casting factor 3) at level 20:
/// `control undead` needs level 16 and casting factor 2. It also carries
/// Evil (98), which the crime.md §6.1 alignment lattice refuses to a
/// Neutral or Good caster — so this one is an OUTLAW (fame 0x28). That is
/// a shipped-data fact about the spell, not a fixture convenience.
fn priest() -> Player {
    caster("Mordak", ClassId(5), 20, 0x28, CONTROL_UNDEAD)
}

fn load() -> Core {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    let content = content_db::load(&db).expect("load");
    Core::new(content, CoreConfig::default())
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

/// One caster and one shipped template, alone in the dead end.
fn arena(who: Player, template: MonsterId) -> (Core, SessionId, MonsterInstanceId) {
    let mut core = load();
    let m = core
        .spawn_monster(template, ARENA)
        .expect("shipped template");
    let s = core.attach_player(who);
    core.drain_events();
    assert_eq!(
        core.monster_ids()
            .into_iter()
            .filter(|id| core.monster_location(*id) == Some(ARENA))
            .count(),
        1,
        "the dead end must hold nothing but the template under test"
    );
    (core, s, m)
}

fn cast(core: &mut Core, s: SessionId, line: &str) -> String {
    core.input(s, line);
    text_to(&core.drain_events(), s)
}

/// A benign cast is one-per-round; five ticks clear the flag and refill
/// the pool.
fn energy_round(core: &mut Core) {
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
}

/// Casts until the charm lands. All four shipped Enslave spells always
/// grant a save (`typeofresists` 2), so a single cast is a coin toss
/// against `charmres`; the CHARM is what is under test here, not the
/// roll, and the resist path has its own deterministic coverage in
/// `mud-core/tests/charm.rs`.
fn charm_until(core: &mut Core, s: SessionId, line: &str, m: MonsterInstanceId) -> String {
    let mut shown = String::new();
    for _ in 0..40 {
        shown = cast(core, s, line);
        assert!(
            !shown.contains("no effect"),
            "an eligible target must never be refused: {shown:?}"
        );
        if core.debug_monster_charm(m).is_some_and(|t| t.0) {
            return shown;
        }
        energy_round(core);
    }
    panic!("the charm never landed in 40 casts: {shown:?}");
}

/// The 00485de3 refusal, matched in two halves: a spawned monster's
/// DISPLAY name is rolled from its `desctxt` name block ("large kobold
/// thief", "small kobold thief", ...), so only the template's base name
/// is stable across runs.
fn assert_refusal(shown: &str, base_name: &str) {
    assert!(
        shown.contains("Your spell has no effect on "),
        "expected the 00485de3 refusal, got: {shown:?}"
    );
    assert!(
        shown.contains(&format!("{base_name}.\n")),
        "the refusal must name the target, got: {shown:?}"
    );
}

fn tick_until(
    core: &mut Core,
    s: SessionId,
    limit: u32,
    mut done: impl FnMut(&Core) -> bool,
) -> String {
    let mut shown = String::new();
    for _ in 1..=limit {
        core.tick();
        shown.push_str(&text_to(&core.drain_events(), s));
        if done(core) {
            break;
        }
    }
    shown
}

// --- 92 charm animal: the AffectsAnimals (80) predicate ---

#[test]
fn charm_animal_lands_on_a_shipped_animal() {
    let (mut core, s, m) = arena(druid(), WILD_DOG);
    charm_until(&mut core, s, "cast char dog", m);
    assert_eq!(
        core.debug_monster_charm(m),
        Some((true, true, Some(s))),
        "the §0 triple: owner link, suppression, charmed bit"
    );
    let slots = core.monster_active_spells(m).expect("the dog lives");
    assert!(
        slots.iter().any(|slot| slot.spell == Some(CHARM_ANIMAL)),
        "the duration arm takes a slot"
    );
}

#[test]
fn charm_animal_refuses_a_shipped_non_animal() {
    // The delta this whole fix is about: 155 of 1101 shipped templates
    // carry ability 78, and before the scan landed `charm animal` was
    // charming the other 946 — kobold thieves included.
    let (mut core, s, m) = arena(druid(), KOBOLD_THIEF);
    let mana = core.current_mana(s);
    let energy = core.round_energy(s);
    let shown = cast(&mut core, s, "cast char kobold");
    assert_refusal(&shown, "kobold thief");
    assert_eq!(core.debug_monster_charm(m), Some((false, false, None)));
    assert_eq!(core.current_mana(s), mana, "the scan precedes the cost");
    assert_eq!(core.round_energy(s), energy, "and the round cost");
    // Free of the one-cast-per-round flag too: the refusal returns ahead
    // of everything the cast would otherwise consume.
    let again = cast(&mut core, s, "cast char kobold");
    assert!(!again.contains("already cast"), "got: {again:?}");
}

// --- 88 control undead: the `undead` COLUMN predicate ---

#[test]
fn control_undead_lands_on_a_shipped_undead() {
    let (mut core, s, m) = arena(priest(), SKELETON);
    charm_until(&mut core, s, "cast cded skeleton", m);
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
}

#[test]
fn control_undead_accepts_the_negative_undead_byte() {
    // `zombie cat` ships `undead` **-1**. The DLL's test is `!= 0`, so
    // the 8 negative templates are undead — a bool column would have
    // silently locked all 8 out of the spell.
    let (mut core, s, m) = arena(priest(), ZOMBIE_CAT);
    charm_until(&mut core, s, "cast cded cat", m);
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
}

#[test]
fn control_undead_refuses_a_shipped_living_template() {
    let (mut core, s, m) = arena(priest(), KOBOLD_THIEF);
    let mana = core.current_mana(s);
    let shown = cast(&mut core, s, "cast cded kobold");
    assert_refusal(&shown, "kobold thief");
    assert_eq!(core.debug_monster_charm(m), Some((false, false, None)));
    assert_eq!(core.current_mana(s), mana, "uncharged");
}

// --- 49 song of charming: the NonLiving (109) predicate ---

#[test]
fn song_of_charming_lands_on_a_shipped_living_template() {
    let (mut core, s, m) = arena(bard(), KOBOLD_THIEF);
    charm_until(&mut core, s, "cast char kobold", m);
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));
}

#[test]
fn song_of_charming_refuses_a_shipped_nonliving_template() {
    let (mut core, s, m) = arena(bard(), SKELETON);
    let mana = core.current_mana(s);
    let shown = cast(&mut core, s, "cast char skeleton");
    assert_refusal(&shown, "skeleton");
    assert_eq!(core.debug_monster_charm(m), Some((false, false, None)));
    assert_eq!(core.current_mana(s), mana, "uncharged");
}

#[test]
fn the_two_undead_predicates_disagree_on_shipped_data() {
    // One template, two spells, opposite answers — the reason
    // AffectsUndead (a column read) and AffectsLiving (an ability read)
    // must not be collapsed. `skeleton` is `undead` 1 AND `NonLiving`;
    // 6 shipped templates go the other way (`undead != 0`, no 109) and
    // 76 carry 109 with `undead == 0`.
    let (mut core, s, m) = arena(priest(), SKELETON);
    charm_until(&mut core, s, "cast cded skeleton", m);
    assert_eq!(core.debug_monster_charm(m), Some((true, true, Some(s))));

    let (mut core, s, m) = arena(bard(), SKELETON);
    let shown = cast(&mut core, s, "cast char skeleton");
    assert_refusal(&shown, "skeleton");
    assert_eq!(core.debug_monster_charm(m), Some((false, false, None)));
}

// --- the pet, end to end ---

#[test]
fn a_shipped_pet_assists_its_owner_and_then_expires() {
    // §2.2 assist + §4.1 expiry over shipped data: a real druid charms a
    // real wild dog with a real spell, the dog swings at the owner's
    // autocombat target through the combat driver, and when the shipped
    // 60-unit duration runs out the slot sweep reverses the triple in
    // silence.
    let mut core = load();
    let pet = core.spawn_monster(WILD_DOG, ARENA).expect("wild dog");
    let quarry = core.spawn_monster(HEALER, ARENA).expect("healer");
    let s = core.attach_player(druid());
    core.drain_events();
    charm_until(&mut core, s, "cast char dog", pet);
    let hp = core.monster_hp(quarry).expect("the healer lives");

    energy_round(&mut core);
    core.input(s, "attack healer");
    core.drain_events();
    // Both names carry a rolled `desctxt` adjective, so the assertion
    // pins the two base names and the verb between them. Driven by real
    // ticks: this is the only proof that the combat driver reaches the
    // assist branch over shipped data.
    let shown = tick_until(&mut core, s, 60, |_| false).to_lowercase();
    assert!(
        shown.contains("wild dog just attacked") && shown.contains("healer!"),
        "the pet must swing at the owner's target: {shown:?}"
    );
    assert!(
        core.monster_hp(quarry).is_none_or(|now| now < hp),
        "and the damage must land"
    );
    assert!(
        core.debug_monster_charm(pet).is_some_and(|t| t.0),
        "assisting changes no charm state"
    );

    // Expiry. The dog may be in combat by now; the timer does not care.
    let after = tick_until(&mut core, s, 400, |c| {
        c.debug_monster_charm(pet) == Some((false, false, None))
    });
    assert_eq!(
        core.debug_monster_charm(pet),
        Some((false, false, None)),
        "the shipped duration must run out and reverse the triple: {after:?}"
    );
    let slots = core.monster_active_spells(pet).expect("the dog lives");
    assert!(
        slots.iter().all(|slot| slot.spell != Some(CHARM_ANIMAL)),
        "and clear the slot"
    );
}

#[test]
fn owner_logout_releases_a_shipped_pet() {
    // §4.2: the give-up counter runs on an offline owner and the release
    // path fires over shipped data exactly as it does on fixtures.
    let (mut core, s, pet) = arena(druid(), WILD_DOG);
    charm_until(&mut core, s, "cast char dog", pet);
    core.detach(s);
    core.drain_events();
    tick_until(&mut core, s, 60, |c| {
        c.debug_monster_charm(pet).is_none_or(|t| !t.0)
    });
    assert!(
        core.debug_monster_charm(pet)
            .is_none_or(|t| t == (false, false, None)),
        "the leash must let go of a shipped body too"
    );
}
