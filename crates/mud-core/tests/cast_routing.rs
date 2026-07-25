//! Single-target cast ROUTING — which of the three `cast_*_target` entry
//! points a `cast <spell> <name>` reaches, and which refusal an
//! unacceptable pairing prints.
//!
//! The law (verified against `re/wg_nt_ghidra/exports/WCCMMUD_decompiled.c`):
//!
//! 1. **Routing is the resolved target KIND, never `spelltype`.** The
//!    dispatcher (59253-59320) asks `get_spell_match_type` (45018-45053)
//!    for a preferred `find_action_target` mask keyed on the MATCH type
//!    (`spell+0xcc`): match 4 -> `0x801`, 6 -> `0xf837`, 7 -> `0x14`,
//!    8 -> `0x803`, 0/1/2 -> `0x02` (`0x82` when offensive — the
//!    exclude-self bit), the seven area types -> `0`. When that find
//!    comes back empty the dispatcher re-runs it with the UNIVERSAL mask
//!    `0xf037` (59265-59271), so the preferred mask is only an ORDERING
//!    preference — any spell can resolve any kind. Dispatch (59278-59320)
//!    then reads only what was FOUND: 1 -> `cast_user_target`,
//!    2 -> `cast_monster_target`, 8 -> `cast_item_target`.
//!    Mask bits (`find_action_target` 63726+): `0x1` monsters (found
//!    kind 2), `0x2` users (kind 1), `0x4` carried items (kind 8),
//!    `0x10` room items (kind 4), `0x20` spellbook (kind 0x10).
//! 2. **Acceptance is the match type**, gated inside each entry point:
//!    `cast_monster_target` 43205 accepts `{4, 6, 8}` and its else at
//!    44311-44315 prints "You may not cast that spell on a monster!";
//!    `cast_user_target` 41460 accepts `{0, 2, 6, 8}`, else 43064-43066
//!    "...on a user!"; `cast_item_target` 44367 accepts `{6, 7}`, else
//!    44369 "...on an item!". All three refuse UNCHARGED.
//! 3. **`spelltype` routes nothing.** Inside `cast_monster_target` it
//!    only drives hostility: the evil-points/grudge block (43248-43273),
//!    the engage-and-stop block (43411-43421, offensive + duration 0
//!    only) and the elemental-resist scale (43525).
//!
//! DATA (`re/mmud_wgnt.sqlite`, 1379 spells): 208 benign-mode spells
//! carry match 4/6/8 (the charm family, curse, blind, slow, fear, hold
//! person...) and reach a monster; the 69 offensive-mode spells on
//! match 0/1/2/7 do not — and NONE of those 69 is learnable (of the 207
//! LearnSp-taught spells, every offensive one is match 4, 8 or 12).
//! Match 1 is the self-only buff band (barkskin, stoneskin, magic
//! armour); match 2 is the castable-on-another-player band (bless,
//! blur, minor healing).

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Element, Item, ItemId, MatchType, Message, MessageId,
    Monster, MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId,
    StatBlock, TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, MonsterInstanceId, Player, SessionId};

const TOWER: RoomId = RoomId { map: 1, room: 1 };
const MAGE: ClassId = ClassId(1);
const HUMAN: RaceId = RaceId(1);

/// The plain probe monster.
const CAT: MonsterId = MonsterId(1);
/// Shares its first word with [`TRINKET`] — the find-ORDER probe.
const SERPENT: MonsterId = MonsterId(2);

/// A carried item nobody else's name collides with.
const AMULET: ItemId = ItemId(700);
/// "silver trinket" vs the "silver serpent" monster.
const TRINKET: ItemId = ItemId(710);

/// Benign + match 4 — the shipped Enslave shape (49/55/88/92).
const CHARM: SpellId = SpellId(600);
/// CHARM with duration 0 — the benign-never-engages probe.
const SNAP: SpellId = SpellId(601);
/// Benign + match 8 — the shipped curse/blind/slow/fear shape.
const HEX: SpellId = SpellId(610);
/// Benign + match 2 — the shipped blur shape (MEASURED §8.13).
const BLUR: SpellId = SpellId(620);
/// Offensive + match 2 — the band that LOSES the monster path.
const STAB: SpellId = SpellId(630);
/// Offensive + match 8 — the shipped magic missile shape.
const SPARK: SpellId = SpellId(640);
/// Benign + match 7 — the shipped detect magic shape.
const SCRY: SpellId = SpellId(650);
/// Benign + match 6 — accepted by ALL THREE entry points.
const SIGHT: SpellId = SpellId(660);

fn spell(id: SpellId, name: &str, short: &str, mode: TargetMode, mt: MatchType) -> Spell {
    Spell {
        id,
        name: name.into(),
        short_name: short.into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(901)),
        abilities: vec![(Ability::AC, -1)],
        level_cap: 0,
        round_cost: 100,
        required_power: 1,
        min_base: 0,
        max_base: 0,
        target_mode: mode,
        save_class: SaveClass::None,
        base_chance: 200, // auto-success: >= 200 skips the roll
        duration_per_level: 0,
        match_type: mt,
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

fn monster(id: MonsterId, name: &str) -> Monster {
    Monster {
        id,
        name: name.into(),
        hitpoints: 40,
        experience: 12,
        exp_multi: 1,
        energy: 0,
        charm_level: 1,
        charm_resist: 1,
        attacks: [AttackForm::default(); 5],
        ..Default::default()
    }
}

fn item(id: ItemId, name: &str) -> Item {
    Item {
        id,
        name: name.into(),
        abilities: vec![(Ability::Magical, 1)],
        weight: 1,
        uses: -1,
        gettable: 1,
        ..Item::default()
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room { id: TOWER, name: "Tower".into(), ..Default::default() });
    content.add_monster(monster(CAT, "black cat"));
    content.add_monster(monster(SERPENT, "silver serpent"));
    content.add_item(item(AMULET, "gold amulet"));
    content.add_item(item(TRINKET, "silver trinket"));
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
    let mut snap = spell(SNAP, "snap", "snap", TargetMode::Benign, MatchType::Special4);
    snap.duration = 0;
    let mut spark = spell(SPARK, "spark", "spar", TargetMode::Offensive0, MatchType::Special8);
    spark.duration = 0;
    // The detect magic (24) shape — the only payload `fire_item_cast`
    // renders a line for.
    let mut scry = spell(SCRY, "scry", "scry", TargetMode::Benign, MatchType::Item7);
    scry.abilities = vec![(Ability::DetectMagic, 1)];
    scry.duration = 0;
    for s in [
        spell(CHARM, "charm", "char", TargetMode::Benign, MatchType::Special4),
        snap,
        spell(HEX, "hex", "hex", TargetMode::Benign, MatchType::Special8),
        spell(BLUR, "blur", "blur", TargetMode::Benign, MatchType::Single2),
        spell(STAB, "stab", "stab", TargetMode::Offensive0, MatchType::Single2),
        spark,
        scry,
        spell(SIGHT, "sight", "sigh", TargetMode::Benign, MatchType::Item6),
    ] {
        content.add_spell(s);
    }
    content
}

fn caster(name: &str) -> Player {
    let book: BTreeMap<SpellId, bool> = [CHARM, SNAP, HEX, BLUR, STAB, SPARK, SCRY, SIGHT]
        .into_iter()
        .map(|s| (s, false))
        .collect();
    Player {
        name: name.into(),
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

/// One caster, a black cat, a gold amulet in the pack.
fn setup() -> (Core, SessionId, MonsterInstanceId) {
    let mut core = Core::new(world(), CoreConfig { start_location: TOWER, ..Default::default() });
    let m = core.spawn_monster(CAT, TOWER).expect("cat template");
    let s = core.attach_player(caster("Zin"));
    core.give_item(s, AMULET);
    core.drain_events();
    (core, s, m)
}

// --- match 4/6/8 reach the monster (the 208-spell band) ---

#[test]
fn a_benign_match_four_spell_reaches_the_monster_path() {
    // The shipped Enslave shape: spelltype 3 (Benign) + match 4. The DLL
    // routes it by the 0x801 monster-preferring find mask, NOT by the
    // benign target mode — `cast charm cat` must land, not print the
    // do-not-see refusal.
    let (mut core, s, m) = setup();
    let shown = cast(&mut core, s, "cast char cat");
    assert!(shown.contains("You cast charm on black cat!"), "got: {shown:?}");
    assert!(!shown.contains("You do not see"), "no refusal: {shown:?}");
    let slots = core.monster_active_spells(m).expect("cat lives");
    assert_eq!(slots[0].spell, Some(CHARM), "the slot entry landed on the monster");
    assert_eq!(core.current_mana(s), 96, "full costs paid");
}

#[test]
fn a_benign_match_eight_spell_reaches_the_monster_path() {
    // 152 shipped benign match-8 spells (curse, blind, slow, fear, hold
    // person): mask 0x803 searches monsters AND users, and
    // `cast_monster_target` 43205 accepts 8.
    let (mut core, s, m) = setup();
    let shown = cast(&mut core, s, "cast hex cat");
    assert!(shown.contains("You cast hex on black cat!"), "got: {shown:?}");
    let slots = core.monster_active_spells(m).expect("cat lives");
    assert_eq!(slots[0].spell, Some(HEX));
}

#[test]
fn a_benign_match_six_spell_reaches_the_monster_path() {
    // Match 6 is the only type all three entry points accept; its mask
    // 0xf837 searches monsters FIRST, so a monster name routes there.
    let (mut core, s, m) = setup();
    let shown = cast(&mut core, s, "cast sigh cat");
    assert!(shown.contains("You cast sight on black cat!"), "got: {shown:?}");
    assert_eq!(core.monster_active_spells(m).expect("cat lives")[0].spell, Some(SIGHT));
}

// --- the acceptance gates, all three uncharged ---

#[test]
fn an_offensive_match_two_spell_is_refused_at_a_monster() {
    // The 69-spell band that LOSES the monster path: offensive mode no
    // longer buys a monster route. 43205 accepts {4,6,8} only, so the
    // 44311 else fires — uncharged, and no engagement.
    let (mut core, s, m) = setup();
    let shown = cast(&mut core, s, "cast stab cat");
    assert!(
        shown.contains("You may not cast that spell on a monster!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 100, "uncharged");
    assert!(!shown.contains("*Combat Engaged*"), "no engagement: {shown:?}");
    assert!(core.monster_active_spells(m).expect("cat lives")[0].spell.is_none());
}

#[test]
fn a_match_four_spell_is_refused_at_a_user() {
    // `cast_user_target` 41460 accepts {0,2,6,8}; match 4 is not in it,
    // so the 43066 twin refusal fires. The preferred 0x801 find has no
    // monster by that name, so the universal fallback resolves the
    // player and the KIND gate does the rest.
    let mut core =
        Core::new(world(), CoreConfig { start_location: TOWER, ..Default::default() });
    let a = core.attach_player(caster("Zin"));
    let _b = core.attach_player(caster("Oracle"));
    core.drain_events();
    let shown = cast(&mut core, a, "cast char oracle");
    assert!(shown.contains("You may not cast that spell on a user!\n"), "got: {shown:?}");
    assert_eq!(core.current_mana(a), 100, "uncharged");
}

#[test]
fn a_match_four_spell_is_refused_at_a_carried_item() {
    // `cast_item_target` 44367 accepts {6,7} only; 44369 prints the
    // third refusal of the family (DLL string table offset 854240,
    // 362 bytes past the monster variant).
    let (mut core, s, _m) = setup();
    let shown = cast(&mut core, s, "cast char amulet");
    assert!(shown.contains("You may not cast that spell on an item!\n"), "got: {shown:?}");
    assert_eq!(core.current_mana(s), 100, "uncharged");
}

#[test]
fn a_match_seven_spell_is_refused_at_a_monster() {
    // Match 7's preferred mask 0x14 is items-only; a monster name finds
    // nothing, the universal fallback resolves the cat, and 43205
    // refuses 7.
    let (mut core, s, _m) = setup();
    let shown = cast(&mut core, s, "cast scry cat");
    assert!(
        shown.contains("You may not cast that spell on a monster!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 100, "uncharged");
}

// --- the preferred mask is an ORDERING preference ---

#[test]
fn the_preferred_mask_orders_a_name_two_kinds_share() {
    // "silver serpent" (monster) and "silver trinket" (carried) both
    // match the word "silver". Match 4 prefers monsters (0x801) and
    // match 7 prefers items (0x14) — same word, different entry point.
    let mut core =
        Core::new(world(), CoreConfig { start_location: TOWER, ..Default::default() });
    let snake = core.spawn_monster(SERPENT, TOWER).expect("serpent template");
    let s = core.attach_player(caster("Zin"));
    core.give_item(s, TRINKET);
    core.drain_events();
    let shown = cast(&mut core, s, "cast char silver");
    assert!(shown.contains("You cast charm on silver serpent!"), "got: {shown:?}");
    assert_eq!(core.monster_active_spells(snake).expect("serpent lives")[0].spell, Some(CHARM));
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
    let shown = cast(&mut core, s, "cast scry silver");
    assert!(shown.contains("silver trinket"), "the item entry point: {shown:?}");
    assert!(!shown.contains("may not cast"), "no refusal: {shown:?}");
}

#[test]
fn the_universal_fallback_searches_monsters_before_items() {
    // Match 2's preferred mask is users-only; with no player named
    // "silver" the fallback 0xf037 runs, and `find_action_target`'s own
    // order puts the monster loop (mask bit 0x1) ahead of the inventory
    // loop (0x4) — so the serpent wins and the monster refusal prints.
    // MEASURED §8.13 is the same shape: `c blur cat`.
    let mut core =
        Core::new(world(), CoreConfig { start_location: TOWER, ..Default::default() });
    core.spawn_monster(SERPENT, TOWER).expect("serpent template");
    let s = core.attach_player(caster("Zin"));
    core.give_item(s, TRINKET);
    core.drain_events();
    let shown = cast(&mut core, s, "cast blur silver");
    assert!(
        shown.contains("You may not cast that spell on a monster!\n"),
        "got: {shown:?}"
    );
}

#[test]
fn an_unmatched_name_still_prints_the_room_refusal() {
    // Both finds come back empty -> 59272-59277, unchanged by the
    // router (MEASURED §8.9).
    let (mut core, s, _m) = setup();
    let shown = cast(&mut core, s, "cast char zzz");
    assert!(shown.contains("You do not see zzz here!\n"), "got: {shown:?}");
    assert_eq!(core.current_mana(s), 100, "uncharged");
}

// --- spelltype still owns engagement ---

#[test]
fn a_benign_monster_cast_never_engages() {
    // 43411-43421: the engage-and-stop block is gated on `spelltype < 3`
    // AND duration 0. A benign instant resolves at the command instead —
    // no *Combat Engaged*, no target lock.
    let (mut core, s, m) = setup();
    let shown = cast(&mut core, s, "cast snap cat");
    assert!(!shown.contains("*Combat Engaged*"), "benign never engages: {shown:?}");
    assert!(shown.contains("You cast snap on black cat!"), "resolved now: {shown:?}");
    assert_eq!(core.current_mana(s), 96, "full costs paid at the command");
    assert!(core.monster_active_spells(m).expect("cat lives")[0].spell.is_none());
}

#[test]
fn an_offensive_instant_at_a_monster_still_engages() {
    // The other side of the same gate: offensive + duration 0 keeps the
    // engagement-is-the-whole-command behaviour (MEASURED §8.9).
    let (mut core, s, _m) = setup();
    let shown = cast(&mut core, s, "cast spar cat");
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}
