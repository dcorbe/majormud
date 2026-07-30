//! M7 slice 6: the quest VM skeleton — `perform_matched_action` token
//! dispatch, control codes, and the gate verbs (quests.md §2;
//! decompile 68548-69689). Every test cites the decompile arm it pins.

use mud_core::ability::Ability;
use mud_core::content::{
    Class, ClassId, Content, Item, ItemId, Message, MessageId, Monster, MonsterId, Race, RaceId,
    Room, RoomId, Spell, SpellId, StatBlock, TextBlock, TextBlockId,
};
use mud_core::game::{ActiveSpell, Core, CoreConfig, Event, Gender, Player, SessionId};

const A: RoomId = RoomId { map: 1, room: 1 };
const DARK_DRUID: u16 = 129;

fn spell(id: SpellId, name: &str) -> Spell {
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, TargetMode};
    Spell {
        id,
        name: name.into(),
        short_name: name.into(),
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
        msg_style: 0,
    }
}

fn ab(id: u16) -> Ability {
    Ability::from_id(id).unwrap()
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: A,
        name: "Shrine".into(),
        ..Default::default()
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
    // Class 1 is a group-1 caster so learnspell/cast fixtures are
    // learnable (spell_gate 17811-17846); class 2 is a non-caster, the
    // wrong-class refusal fixture.
    for (id, name, group) in [(1, "Warrior", 1), (2, "Witchunter", 0)] {
        content.add_class(Class {
            id: ClassId(id),
            name: name.into(),
            abilities: vec![],
            hp_per_level: 6,
            hp_seed: 4,
            caster_group: group,
            casting_factor: group,
            exp_base: 0,
            combat_factor: 6,
            weapon_code: 8,
            armour_code: 9,
        });
    }
    content.add_item(Item {
        id: ItemId(400),
        name: "brass token".into(),
        weight: 1,
        item_type: 0,
        uses: -1,
        gettable: 1,
        ..Item::default()
    });
    content.add_item(Item {
        id: ItemId(200),
        name: "iron helmet".into(),
        weight: 60,
        item_type: 0,
        uses: -1,
        worn_on: 2,
        gettable: 1,
        ..Item::default()
    });
    content.add_message(Message {
        id: MessageId(801),
        lines: vec![
            "You are judged unworthy.".into(),
            "%s is judged unworthy.".into(),
            String::new(),
        ],
    });
    content.add_monster(Monster {
        id: MonsterId(7),
        name: "kobold thief".into(),
        hitpoints: 10,
        ..Monster::default()
    });
    content.add_monster(Monster {
        id: MonsterId(8),
        name: "giant rat".into(),
        hitpoints: 10,
        ..Monster::default()
    });
    let mut aura = spell(SpellId(300), "aura");
    aura.duration = 40;
    aura.abilities = vec![(ab(2), 5)]; // AC +5 while active
    content.add_spell(aura);
    let mut trap = spell(SpellId(625), "spear trap");
    trap.target_mode = mud_core::content::TargetMode::Offensive0;
    content.add_spell(trap);
    let mut costly = spell(SpellId(301), "greater aura");
    costly.mana_cost = 999;
    content.add_spell(costly);
    // The failure block for checkspell/testskill: an observable mutation
    // (flag 7) — `flag` lands with this slice, so the block's effect is
    // visible in the player snapshot.
    content.add_text_block(TextBlock {
        id: TextBlockId(900),
        next: None,
        body: "flag 7 set".into(),
    });
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: A,
        ..CoreConfig::default()
    }
}

fn player(name: &str) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 5,
        current_hp: 40,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: A,
        ..Default::default()
    }
}

fn boot() -> (Core, SessionId) {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(player("Quester"));
    core.drain_events();
    (core, s)
}

fn boot_with(p: Player) -> (Core, SessionId) {
    let mut core = Core::new(world(), config());
    let s = core.attach_player(p);
    core.drain_events();
    (core, s)
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

// --- dispatch skeleton ---

#[test]
fn unknown_tokens_are_skipped_and_leave_code_zero() {
    // Dispatch tail 68695-68708: an unmatched first word runs nothing and
    // leaves the control code untouched (0 when nothing else ran).
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "frobnicate 5"), 0);
    // An unknown token after a passing gate keeps the gate's code.
    assert_eq!(core.debug_perform_matched_action(s, "minlevel 1:frobnicate"), 1);
}

#[test]
fn failed_gate_stops_the_chain() {
    // The control-code contract (quests.md §2): a FailStop ends the
    // chain — the mutation after the failed gate must not run.
    let (mut core, s) = boot();
    assert_eq!(
        core.debug_perform_matched_action(s, "minlevel 99:flag 3 set"),
        2
    );
    assert_eq!(core.player_snapshot(s).quest_flags, 0, "flag must not set");
    assert_eq!(
        core.debug_perform_matched_action(s, "minlevel 1:flag 3 set"),
        1
    );
    assert_eq!(core.player_snapshot(s).quest_flags, 1 << 2);
}

// --- ability gates ---

#[test]
fn checkability_requires_presence_then_minimum() {
    // 69280-69316: no ability → fail (no message); with the ability the
    // second arg is a minimum; bare `checkability <id>` is presence-only.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "checkability 129 1"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "checkability 129"), 2);

    let mut p = player("Adept");
    p.raise_innate_ability(ab(DARK_DRUID), 2);
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "checkability 129"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "checkability 129 2"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "checkability 129 3"), 2);
}

#[test]
fn testability_is_the_upper_gate() {
    // 68855-68894: absent fails for val >= 0 (passes for a negative
    // val); present fails when the value EXCEEDS val. The shipped
    // `testability X v:checkability X v` pairs pin exact-step progression.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "testability 129 0"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "testability 129 -1"), 1);

    let mut p = player("Adept");
    p.raise_innate_ability(ab(DARK_DRUID), 2);
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "testability 129 2"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "testability 129 3"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "testability 129 1"), 2);
}

#[test]
fn failability_fails_only_when_present() {
    // 68828-68853: requires BOTH args (the arm nests the whole check
    // inside the second parse); fails when the player carries the id.
    let mut p = player("Marked");
    p.raise_innate_ability(ab(DARK_DRUID), 1);
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "failability 129 801"), 2);
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You are judged unworthy."), "got: {shown:?}");
    // Bare `failability <id>` does nothing — decompile-literal quirk.
    assert_eq!(core.debug_perform_matched_action(s, "failability 129"), 1);

    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "failability 129 801"), 1);
}

// --- identity gates ---

#[test]
fn class_and_race_gates_are_exact() {
    // class 69263-69277, race 69245-69261: equality, no message arg.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "class 1"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "class 2"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "race 1"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "race 2"), 2);
}

#[test]
fn level_gates_bracket_and_report() {
    // minlevel 69173-69196 (fail when level < n), maxlevel 69148-69171
    // (fail when n < level); both take an optional failure message.
    let (mut core, s) = boot(); // level 5
    assert_eq!(core.debug_perform_matched_action(s, "minlevel 5"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "minlevel 6 801"), 2);
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You are judged unworthy."), "got: {shown:?}");
    assert_eq!(core.debug_perform_matched_action(s, "maxlevel 5"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "maxlevel 4"), 2);
}

#[test]
fn alignment_gates_use_the_evil_word() {
    // evilaligned 69198-69220 (fail when fame < n — requires AT LEAST n
    // evil), goodaligned 69222-69243 (fail when n < fame — requires AT
    // MOST n). The shipped committed-good pattern is `goodaligned -51`.
    let mut p = player("Sinner");
    p.fame = 40;
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "evilaligned 40"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "evilaligned 41"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "goodaligned 40"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "goodaligned 39"), 2);

    let mut p = player("Saint");
    p.fame = -51;
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "goodaligned -51"), 1);
}

// --- item gates ---

#[test]
fn checkitem_and_failitem_scan_carried_inventory() {
    // checkitem 69103-69146, failitem 69058-69101: the scan covers the
    // 100-slot carried array (`+0xd8`) — NOT worn (`+0x62c`) or the
    // wielded weapon (`+0x624`).
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "checkitem 400 801"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "failitem 400"), 1);
    core.give_item(s, ItemId(400));
    assert_eq!(core.debug_perform_matched_action(s, "checkitem 400"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "failitem 400 801"), 2);
}

#[test]
fn worn_items_do_not_satisfy_checkitem() {
    // The DLL scan reads `+0xd8` (and the unmodeled `+0x334` hangup
    // list) only — a WORN helmet is invisible to checkitem.
    let (mut core, s) = boot();
    core.give_item(s, ItemId(200));
    core.input(s, "wear helmet");
    core.drain_events();
    assert_eq!(core.player_snapshot(s).worn.len(), 1, "helmet worn");
    assert_eq!(core.debug_perform_matched_action(s, "checkitem 200"), 2);
}

#[test]
fn room_item_gates_scan_the_floor() {
    // roomitem-as-gate 68985-69029 (fail when absent), failroomitem
    // 68937-68981 (fail when present) — both scan the visible floor
    // (`+0x470`) and hidden (`+0x4d8`) slots.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "roomitem 400 801"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "failroomitem 400"), 1);
    core.give_item(s, ItemId(400));
    core.input(s, "drop token");
    core.drain_events();
    assert_eq!(core.debug_perform_matched_action(s, "roomitem 400"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "failroomitem 400 801"), 2);
}

// --- spell / monster gates ---

#[test]
fn checkspell_scans_active_effects_not_the_spellbook() {
    // FUN_0046f4a5 67891-67936: the scan is over the ACTIVE effect slots
    // (`+0x40`), and the optional failure arg is a fail-BLOCK run through
    // the unconditional runner — not a message.
    let mut p = player("Mage");
    p.spellbook.insert(SpellId(300), false); // known but not active
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "checkspell 300 900"), 2);
    assert_eq!(
        core.player_snapshot(s).quest_flags,
        1 << 6,
        "fail-block 900 (`flag 7 set`) ran"
    );

    let mut p = player("Warded");
    p.active_spells[0] = ActiveSpell {
        spell: Some(SpellId(300)),
        value: 1,
        remaining: 100,
    };
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "checkspell 300 900"), 1);
    assert_eq!(core.player_snapshot(s).quest_flags, 0);
}

#[test]
fn monster_presence_gates() {
    // monsters 69463-69493 (fail when the room is empty), nomonsters
    // 69495-69521 (fail when any monster is present), needmonster
    // 68491-68542 (fail unless the TEMPLATE is present).
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "monsters 801"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "nomonsters"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "needmonster 7 801"), 2);

    core.spawn_monster(MonsterId(7), A).unwrap();
    core.drain_events();
    assert_eq!(core.debug_perform_matched_action(s, "monsters"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "nomonsters 801"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "needmonster 7"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "needmonster 8 801"), 2);
}

// --- skill gates ---

#[test]
fn testskill_rolls_once_against_the_stat() {
    // FUN_0046f53a, testskill variant (68078-68095): one genrdn(0,range)
    // draw; fail when stat < roll + modifier, running the fail-BLOCK.
    // With modifier -200 the roll can never win; with +999 it always
    // does. Draw count pins exactly one draw either way.
    let (mut core, s) = boot();
    let before = core.debug_rng_draws();
    assert_eq!(
        core.debug_perform_matched_action(s, "testskill strength -200 900"),
        1
    );
    assert_eq!(core.debug_rng_draws(), before + 1, "one draw on pass");
    assert_eq!(core.player_snapshot(s).quest_flags, 0);

    let before = core.debug_rng_draws();
    assert_eq!(
        core.debug_perform_matched_action(s, "testskill strength 999 900"),
        2
    );
    assert_eq!(core.debug_rng_draws(), before + 1, "one draw on fail");
    assert_eq!(core.player_snapshot(s).quest_flags, 1 << 6, "fail block ran");
}

#[test]
fn testskill_single_arg_is_the_fail_block() {
    // 68079-68082: with only one numeric arg the modifier defaults to 0
    // and the arg is the fail block.
    let (mut core, s) = boot();
    let code = core.debug_perform_matched_action(s, "testskill charm 900");
    // Fixture charm is 0, every roll ties or beats it except roll 0 —
    // accept either outcome but flag-state must match the code.
    let flagged = core.player_snapshot(s).quest_flags != 0;
    assert_eq!(code == 2, flagged, "fail block iff the roll failed");
}

#[test]
fn checkskill_is_a_deterministic_threshold() {
    // FUN_0046f53a, checkskill variant (68097-68107): value < threshold
    // → message + fail; NO RNG draw; the check runs only when the
    // message arg is present (decompile-literal quirk).
    let (mut core, s) = boot();
    let before = core.debug_rng_draws();
    assert_eq!(
        core.debug_perform_matched_action(s, "checkskill strength 999 801"),
        2
    );
    assert_eq!(core.debug_rng_draws(), before, "no draw");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You are judged unworthy."), "got: {shown:?}");
    assert_eq!(
        core.debug_perform_matched_action(s, "checkskill strength 0 801"),
        1
    );
    // Missing message arg: no check happens at all.
    assert_eq!(
        core.debug_perform_matched_action(s, "checkskill strength 999"),
        1
    );
}

#[test]
fn test_tournament_fails_off_tournament() {
    // 68710-68720: passes only when the tournament config byte
    // (`DAT_004906c9`) is 2; our board is never in tournament mode.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "test_tournament"), 2);
}

// --- the flag verb (quests.md §2 MISSED it; FUN_0046fdda 68319-68413) ---

#[test]
fn flag_set_check_fail_clear_round_trip() {
    // 64 bits: 1-32 in `+0x71c`, 33-64 in `+0x460`; modeled as one u64.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "flag 3 set"), 1);
    assert_eq!(core.player_snapshot(s).quest_flags, 1 << 2);
    assert_eq!(core.debug_perform_matched_action(s, "flag 3 check"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "flag 3 fail 801"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "flag 4 check 801"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "flag 3 clear"), 1);
    assert_eq!(core.player_snapshot(s).quest_flags, 0);
    // The high word (bits 33-64, `+0x460`).
    assert_eq!(core.debug_perform_matched_action(s, "flag 40 set"), 1);
    assert_eq!(core.player_snapshot(s).quest_flags, 1u64 << 39);
    assert_eq!(core.debug_perform_matched_action(s, "flag 40 check"), 1);
    // Out-of-range bit numbers fail-stop (68339-68345).
    assert_eq!(core.debug_perform_matched_action(s, "flag 65 set"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "flag 0 set"), 2);
}

// --- mutation verbs (Task 5) ---

#[test]
fn addability_raises_and_creates() {
    // The addability arm (69590-69637): at-least semantics via
    // raise_innate_ability; a full table fail-stops the chain.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "addability 129 2"), 1);
    assert_eq!(core.player_snapshot(s).innate[0], (Some(ab(DARK_DRUID)), 2));
    assert_eq!(core.debug_perform_matched_action(s, "addability 129 1"), 1);
    assert_eq!(core.player_snapshot(s).innate[0].1, 2, "at-least, not add");

    let mut full = player("Full");
    for i in 0..30 {
        full.innate[i] = (Some(ab(22)), 1);
    }
    let (mut core, s) = boot_with(full);
    assert_eq!(
        core.debug_perform_matched_action(s, "addability 129 2:flag 3 set"),
        2
    );
    assert_eq!(core.player_snapshot(s).quest_flags, 0, "chain stopped");
}

#[test]
fn addability_temp_spell_grants_the_spell() {
    // 69619-69624: creating a fresh 0xa0 slot also adds the spell whose
    // id is the VALUE to the spellbook.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "addability 160 300"), 1);
    let p = core.player_snapshot(s);
    assert_eq!(p.innate[0], (Some(ab(160)), 300));
    assert!(p.spellbook.contains_key(&SpellId(300)), "spell granted");
}

#[test]
fn giveability_accumulates_and_refuses_temp_spell() {
    // 69565-69588 → FUN_0046c507: accumulate; 0xa0 returns failure →
    // the verb fail-stops.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "giveability 129 2"), 1);
    assert_eq!(core.debug_perform_matched_action(s, "giveability 129 3"), 1);
    assert_eq!(core.player_snapshot(s).innate[0].1, 5);
    assert_eq!(core.debug_perform_matched_action(s, "giveability 160 300"), 2);
}

#[test]
fn removeability_zeroes_and_purges_the_temp_spell() {
    // 69523-69558: zero every matching slot (0xa0 purges the spell whose
    // id is the slot VALUE first); an absent id fail-stops.
    let mut p = player("Marked");
    p.raise_innate_ability(ab(DARK_DRUID), 2);
    p.raise_innate_ability(ab(160), 300);
    p.spellbook.insert(SpellId(300), false);
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "removeability 129"), 1);
    assert_eq!(core.player_snapshot(s).innate[0], (None, 0));
    assert_eq!(core.debug_perform_matched_action(s, "removeability 129"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "removeability 160"), 1);
    let p = core.player_snapshot(s);
    assert_eq!(p.innate[1], (None, 0));
    assert!(!p.spellbook.contains_key(&SpellId(300)), "spell purged");
}

#[test]
fn addexp_is_uncapped_and_exact() {
    // add_quest_exp (0x6f291, 67785-67815; quests.md §4.1): a raw
    // experience add — no over-level cap, no party split, silent from
    // the verb (tell=0). The restructured-flag gate (`+0x7d5 & 0x20`)
    // is always-passing for our characters (documented divergence).
    let mut p = player("Grinder");
    p.experience = 1_000_000; // far over-level for level 5
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "addexp 150000"), 1);
    assert_eq!(core.player_snapshot(s).experience, 1_150_000);
    let shown = text_to(&core.drain_events(), s);
    assert!(!shown.contains("experience"), "silent: got {shown:?}");
}

#[test]
fn givecoins_by_denomination_letter_continues_the_chain() {
    // 68723-68748: `givecoins <n> [letter]` — the letter jump table
    // (0x47183d) adds to that denomination; no letter adds to `+0x620`
    // (copper, the last field of the runic..copper block). Every
    // shipped use carries a letter (almost always G) with tokens after
    // it, so the chain continues.
    let (mut core, s) = boot();
    assert_eq!(
        core.debug_perform_matched_action(s, "givecoins 100 G:flag 3 set"),
        1
    );
    let p = core.player_snapshot(s);
    assert_eq!(p.coins.gold, 100);
    assert_eq!(p.quest_flags, 1 << 2, "chain continued past the letter");
    core.debug_perform_matched_action(s, "givecoins 7 c");
    core.debug_perform_matched_action(s, "givecoins 3 S");
    core.debug_perform_matched_action(s, "givecoins 2 P");
    core.debug_perform_matched_action(s, "givecoins 1 R");
    core.debug_perform_matched_action(s, "givecoins 50");
    let coins = core.player_snapshot(s).coins;
    assert_eq!(
        (coins.runic, coins.platinum, coins.gold, coins.silver, coins.copper),
        (1, 2, 100, 3, 57)
    );
}

#[test]
fn addevil_is_a_raw_signed_add() {
    // 68896-68908: `+0x542 += n` — NOT the crime.rs funnel: no Warn on
    // Evil refusal, no minimum-10 bump, no 30000 cap, and negative
    // amounts subtract (the good-path quests pay evil down).
    let mut p = player("Penitent");
    p.fame = 40;
    p.warn_on_evil = true;
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "addevil 24"), 1);
    assert_eq!(core.player_snapshot(s).fame, 64, "warn-on-evil ignored");
    assert_eq!(core.debug_perform_matched_action(s, "addevil -60"), 1);
    assert_eq!(core.player_snapshot(s).fame, 4);
}

#[test]
fn learnspell_gates_and_learns() {
    // FUN_0046fff6 (68418-68486): unknown spell fail-stops; already
    // known is a silent no-op; an unlearnable class prints the refusal
    // and fail-stops; success prints "You learn the spell %s."
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "learnspell 9999"), 2);
    assert_eq!(core.debug_perform_matched_action(s, "learnspell 300"), 1);
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You learn the spell aura."), "got: {shown:?}");
    assert!(core.player_snapshot(s).spellbook.contains_key(&SpellId(300)));
    // Already known: silent no-op, still Continue.
    assert_eq!(core.debug_perform_matched_action(s, "learnspell 300"), 1);
    let shown = text_to(&core.drain_events(), s);
    assert!(!shown.contains("learn"), "got: {shown:?}");
}

#[test]
fn learnspell_refuses_the_wrong_class() {
    // The user_can_use_spell gate (68457-68465) → spell_gate: the
    // non-caster class prints the refusal and fail-stops.
    let mut p = player("Mundane");
    p.class = ClassId(2);
    let (mut core, s) = boot_with(p);
    assert_eq!(core.debug_perform_matched_action(s, "learnspell 300"), 2);
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You don't know what to do with this!"),
        "got: {shown:?}"
    );
    assert!(!core.player_snapshot(s).spellbook.contains_key(&SpellId(300)));
}

#[test]
fn cast_applies_the_benign_spell_as_a_forced_effect() {
    // 69639-69658: alloc → cast_no_target(spell, user, forced=1) —
    // no confusion gate, no success roll; failure fail-stops. Reuses
    // the EndCast forced_cast path.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "cast 300"), 1);
    let p = core.player_snapshot(s);
    assert!(
        p.active_spells.iter().any(|sl| sl.spell == Some(SpellId(300))),
        "aura active: {:?}",
        p.active_spells
    );
    // Unknown spell id: alloc fails, code stays 1 (decompile-literal).
    assert_eq!(core.debug_perform_matched_action(s, "cast 9999"), 1);
}

#[test]
fn cast_refusal_fail_stops() {
    // The forced path still charges gates (mana here): failure returns
    // 0 → the verb fail-stops (69650-69656).
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "cast 301:flag 3 set"), 2);
    assert_eq!(core.player_snapshot(s).quest_flags, 0, "chain stopped");
}

#[test]
fn cast_offensive_trap_arm_is_pending() {
    // M7 PENDING(slice-6): the 17 shipped trap spells (spelltype 0,
    // e.g. 625 spear trap) reach cast_no_target's offensive machinery
    // (39200-39500) — unported; currently a silent no-op that keeps the
    // chain alive. This test pins the placeholder behavior.
    let (mut core, s) = boot();
    assert_eq!(core.debug_perform_matched_action(s, "cast 625:flag 3 set"), 1);
    assert_eq!(core.player_snapshot(s).quest_flags, 1 << 2);
}

// --- failure-message plumbing (FUN_0046f360, 67820-67844) ---

#[test]
fn gate_failure_message_reaches_user_and_room() {
    // Line 1 to the actor, line 2 to the room with the actor's name
    // substituted.
    let (mut core, s) = boot();
    let witness = core.attach_player(player("Witness"));
    core.drain_events();
    core.debug_perform_matched_action(s, "minlevel 99 801");
    let events = core.drain_events();
    let mine = text_to(&events, s);
    let theirs = text_to(&events, witness);
    assert!(mine.contains("You are judged unworthy."), "got: {mine:?}");
    assert!(
        theirs.contains("Quester is judged unworthy."),
        "got: {theirs:?}"
    );
}
