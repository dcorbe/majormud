//! Tests for the `cast` command: parsing (§8.9 bare/abbreviation behavior),
//! book-only spell resolution (exact shortname OR per-word name prefix,
//! remainder-as-target), and the slice-3 cast gates (spec §3 order,
//! oracle strings from spellcasting.md §8.6/§8.9).

use std::collections::BTreeMap;

use mud_core::command::{parse, Command};
use mud_core::content::{
    Class, ClassId, Content, Element, MatchType, Race, RaceId, Room, RoomId, SaveClass,
    ScalePair, Spell, SpellId, StatBlock, TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const MAGE: ClassId = ClassId(1);
const WARRIOR: ClassId = ClassId(2);

const MAGIC_MISSILE: SpellId = SpellId(20);
const BLUR: SpellId = SpellId(30);
const ILLUMINATE: SpellId = SpellId(10);
/// Free in every dimension (mana 0, round cost 0, level 1) — the gate
/// tests' "this cast passes" probe.
const SPARK: SpellId = SpellId(40);
/// Round cost above the full player pool (1000): the energy-gate probe.
const HEAVY: SpellId = SpellId(50);

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
        match_type: MatchType::Single0,
        duration: 0,
        element: Element::Cold,
        class_gate_group: 1,
        mana_cost: 0,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Tower".into(),
        description: vec![],
        room_type: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
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

    let mut mmis = spell(MAGIC_MISSILE, "magic missile", "mmis");
    mmis.mana_cost = 1;
    let mut blur = spell(BLUR, "blur", "blur");
    blur.mana_cost = 4;
    let mut illu = spell(ILLUMINATE, "illuminate", "illu");
    illu.mana_cost = 4;
    illu.required_power = 2;
    let spark = spell(SPARK, "spark", "spar");
    let mut heavy = spell(HEAVY, "heavy bolt", "hbol");
    heavy.round_cost = 2000;
    content.add_spell(mmis);
    content.add_spell(blur);
    content.add_spell(illu);
    content.add_spell(spark);
    content.add_spell(heavy);
    content
}

fn player(name: &str, class: ClassId, spellbook: BTreeMap<SpellId, bool>) -> Player {
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
        current_mana: 6,
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
        location: RoomId { map: 1, room: 1 },
        spellbook,
    }
}

/// The standard fixture book: every fixture spell, including the
/// too-powerful illuminate (reachable in slice 4+ via temp spells).
fn full_book() -> BTreeMap<SpellId, bool> {
    let mut book = BTreeMap::new();
    book.insert(MAGIC_MISSILE, false);
    book.insert(BLUR, false);
    book.insert(ILLUMINATE, false);
    book.insert(SPARK, false);
    book.insert(HEAVY, false);
    book
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

// --- parsing (Task 7) ---

#[test]
fn cast_verb_and_c_shortcut_parse_with_args() {
    // MEASURED (§8.9): bare `c` and `c args` both route to cast, exactly
    // like `a`=attack's min-1 verb row.
    assert_eq!(parse("c"), Command::Cast(String::new()));
    assert_eq!(parse("cast"), Command::Cast(String::new()));
    assert_eq!(parse("c mmis rat"), Command::Cast("mmis rat".into()));
    assert_eq!(
        parse("cast magic missile"),
        Command::Cast("magic missile".into())
    );
    // Prefix-model intermediates (ORACLE-VERIFY: only c/cast measured).
    assert_eq!(parse("ca blur"), Command::Cast("blur".into()));
}

#[test]
fn bare_cast_prints_syntax_line() {
    // MEASURED (§8.9): a syntax line, not an error, not speech.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    for input in ["cast", "c"] {
        core.input(s, input);
        let shown = text_to(&core.drain_events(), s);
        assert!(
            shown.contains("Syntax: CAST {spell} [{target}]\n"),
            "{input}: {shown:?}"
        );
    }
}

// --- book resolution (Task 7) ---

#[test]
fn name_word_prefixes_resolve() {
    // MEASURED (§8.9): `c m` / `c magic` / `c magic mi` all hit magic
    // missile; the words consumed by the match never become the target.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    let vexil = core.player_snapshot(s);
    for probe in ["m", "magic", "magic mi"] {
        assert_eq!(
            core.resolve_spell_from_book(&vexil, probe),
            Some((MAGIC_MISSILE, String::new())),
            "probe: {probe}"
        );
    }
    assert_eq!(
        core.resolve_spell_from_book(&vexil, "b"),
        Some((BLUR, String::new()))
    );
}

#[test]
fn exact_shortname_resolves_but_shortname_prefixes_miss() {
    // MEASURED (§8.9): shortname matching is exact-only — `c mm`/`c mmi`
    // miss, `c mmis` hits. `mami` (§8.6) is no word-prefix either.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    let vexil = core.player_snapshot(s);
    assert_eq!(
        core.resolve_spell_from_book(&vexil, "mmis"),
        Some((MAGIC_MISSILE, String::new()))
    );
    for probe in ["mm", "mmi", "mami"] {
        assert_eq!(
            core.resolve_spell_from_book(&vexil, probe),
            None,
            "probe: {probe}"
        );
    }
}

#[test]
fn remainder_after_the_name_match_is_the_target_verbatim() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    let vexil = core.player_snapshot(s);
    // MEASURED (§8.9): the greedy name match consumes `magic mi`; only
    // the rest is the target.
    assert_eq!(
        core.resolve_spell_from_book(&vexil, "magic mi rat"),
        Some((MAGIC_MISSILE, "rat".into()))
    );
    assert_eq!(
        core.resolve_spell_from_book(&vexil, "mmis big rat"),
        Some((MAGIC_MISSILE, "big rat".into()))
    );
    // MEASURED (§8.9): trailing words are one target string, preserved for
    // room-entity lookup ("You do not see extra trailing words here!").
    assert_eq!(
        core.resolve_spell_from_book(&vexil, "blur extra trailing words"),
        Some((BLUR, "extra trailing words".into()))
    );
}

#[test]
fn candidates_are_the_learned_book_only() {
    // A spell in content but not in the book never resolves.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Grunt", WARRIOR, BTreeMap::new()));
    let grunt = core.player_snapshot(s);
    assert_eq!(core.resolve_spell_from_book(&grunt, "mmis"), None);
    assert_eq!(core.resolve_spell_from_book(&grunt, "magic missile"), None);
}

#[test]
fn unresolved_cast_prints_do_not_know_never_says() {
    // MEASURED (§8.6/§8.9): the argument is echoed verbatim, and the miss
    // is handled — it never falls through to the say fallback.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    core.input(s, "c mm");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You do not know how to cast mm.\n"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("You say"), "never speech: {shown:?}");

    core.input(s, "cast fireball at rat");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You do not know how to cast fireball at rat.\n"),
        "got: {shown:?}"
    );
}

// --- cast gates (Task 8; spec §3 order, strings §8.6) ---

const ALREADY_CAST: &str = "You have already cast a spell this round!";

/// Drives one energy round (the Job::Energy cadence is 5 ticks).
fn energy_round(core: &mut Core) {
    for _ in 0..5 {
        core.tick();
    }
    core.drain_events();
}

fn cast(core: &mut Core, s: SessionId, line: &str) -> String {
    core.input(s, line);
    text_to(&core.drain_events(), s)
}

#[test]
fn mortally_wounded_blocks_cast() {
    // Gate 1: M3's downed-band command gating, reused.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.current_hp = 0;
    let s = core.attach_player(vexil);
    core.drain_events();
    let shown = cast(&mut core, s, "c spark");
    assert!(
        shown.contains("You may not do that while you are mortally wounded!\n"),
        "got: {shown:?}"
    );
}

#[test]
fn second_cast_in_the_same_round_is_blocked() {
    // MEASURED (§8.6): one cast per round, even for energy-0 spells.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    let first = cast(&mut core, s, "c spark");
    assert!(!first.contains(ALREADY_CAST), "first cast passes: {first:?}");
    let second = cast(&mut core, s, "c spark");
    assert!(second.contains(ALREADY_CAST), "got: {second:?}");
}

#[test]
fn already_cast_beats_no_mana_but_not_resolution() {
    // Resolution precedes the per-round gate (DLL structure: the dispatcher
    // resolves and passes a spell pointer into cast_no_target, where the
    // round gate lives). ORACLE-VERIFY: second-cast-unknown unmeasured —
    // probe `c blur` then `c zzz` in one round.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.set_current_mana(s, 0);
    core.drain_events();
    cast(&mut core, s, "c spark");
    let no_mana = cast(&mut core, s, "c blur");
    assert!(no_mana.contains(ALREADY_CAST), "got: {no_mana:?}");
    assert!(!no_mana.contains("enough mana"), "got: {no_mana:?}");
    let unknown = cast(&mut core, s, "c zzz");
    assert!(unknown.contains("do not know"), "got: {unknown:?}");
    assert!(!unknown.contains(ALREADY_CAST), "got: {unknown:?}");
}

#[test]
fn too_powerful_spell_is_refused_without_blocking_the_round() {
    // Gate 4 (spec §2): level < required_power. Unreachable via
    // scroll-learned books; reachable via slice-4 temp spells.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    let shown = cast(&mut core, s, "c illu");
    assert!(
        shown.contains("This spell is too powerful for you.\n"),
        "got: {shown:?}"
    );
    // A refused cast does not set the per-round flag.
    let next = cast(&mut core, s, "c spark");
    assert!(!next.contains(ALREADY_CAST), "flag not set: {next:?}");
}

#[test]
fn insufficient_round_energy_is_a_silent_noop() {
    // Gate 5: exactly like an M3 attack without energy — no message was
    // ever measured; the cast is a no-op within the round.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    let shown = cast(&mut core, s, "c heavy");
    assert!(
        !shown.contains("You"),
        "no message, just the prompt: {shown:?}"
    );
    // And it does not consume the round.
    let next = cast(&mut core, s, "c spark");
    assert!(!next.contains(ALREADY_CAST), "flag not set: {next:?}");
}

#[test]
fn insufficient_mana_is_refused_with_the_oracle_string() {
    // Gate 6 (MEASURED §8.6). Blur costs 4; give the mage 3.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.set_current_mana(s, 3);
    core.drain_events();
    let shown = cast(&mut core, s, "c blur");
    assert!(
        shown.contains("You do not have enough mana to cast that spell.\n"),
        "got: {shown:?}"
    );
    let next = cast(&mut core, s, "c spark");
    assert!(!next.contains(ALREADY_CAST), "flag not set: {next:?}");
}

#[test]
fn gates_check_but_never_deduct() {
    // Mana and energy deduction happen at roll time (Task 9), not here.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    cast(&mut core, s, "c blur");
    assert_eq!(core.current_mana(s), 6, "mana untouched until the roll");
}

#[test]
fn cast_flag_resets_on_the_energy_round() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    cast(&mut core, s, "c spark");
    let blocked = cast(&mut core, s, "c spark");
    assert!(blocked.contains(ALREADY_CAST), "got: {blocked:?}");
    energy_round(&mut core);
    let after = cast(&mut core, s, "c spark");
    assert!(!after.contains(ALREADY_CAST), "new round: {after:?}");
}
