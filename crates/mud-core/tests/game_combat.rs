//! Tests for engagement and the 5-second combat round
//! (`re/docs/combat_rounds.md` + oracle transcripts).

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Element, Item, ItemId, MatchType, Message, MessageId,
    Monster, MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId,
    StatBlock, TargetMode,
};
use mud_core::game::{
    AccountProfile, ActiveSpell, Core, CoreConfig, Event, Gender, Player, SessionId,
};

/// A punching bag: huge HP, hits back for exactly 1-8 like the kobold.
fn kobold() -> Monster {
    Monster {
        id: MonsterId(7),
        name: "kobold thief".into(),
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
                kind: 1,
                accuracy: 15,
                weight: 100,
                min_damage: 1,
                max_damage: 8,
                hit_msg: None,
                dodge_msg: None,
                miss_msg: None,
                energy: 666,
            },
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
        // Lair mode: passive presence WITHOUT the mode-0/4 evil charge
        // (crime.md §2.5 — created characters ship Warn on Evil ON, and
        // a mode-0 punching bag would refuse every attack).
        behaviour: 3,
        ..Default::default()
    }
}

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: RoomId { map: 1, room: 1 },
        name: "Arena".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    content.add_monster(kobold());
    content.add_race(Race {
        id: RaceId(2),
        name: "Dwarf".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 30,
            wisdom: 50,
            strength: 50,
            health: 50,
            agility: 30,
            charm: 30,
        },
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
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
    content
}

fn config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        ..CoreConfig::default()
    }
}

fn create(core: &mut Core, name: &str) -> SessionId {
    let s = core.attach_account(AccountProfile {
        name: name.into(),
        gender: Gender::Male,
        saved_evil: 0,
    });
    core.input(s, "2");
    core.input(s, "1");
    core.input(s, "No");
    core.drain_events();
    s
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

#[test]
fn fighter_numbers_match_the_decompile() {
    // Dwarf Warrior L1 (combat 6, Str 50, Agl 30), naked, unencumbered:
    // skill = 1 + 15 = 16
    // accuracy = 0 + 2*((6-1)*1 + 12 + 0 + 8 - 2) + (30-50)/6 = 46 - 3 = 43
    // damage 1-4 (Str 50 adds nothing); crit = dodge_base = 1
    // EU = 1200*1000 / ((6*1+45)*(30+150)*1500/9000) = 1200000/1530 = 784
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let (fighter, eu) = core.combat_debug(s);
    assert_eq!(fighter.accuracy, 43);
    assert_eq!(fighter.min_damage, 1);
    assert_eq!(fighter.max_damage, 4);
    assert_eq!(fighter.crit_rating, 1);
    assert_eq!(eu, 784);
}

#[test]
fn attack_engages_with_first_strike() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });

    core.input(s, "attack kobold");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
    // restart_autocombat: the opening swing happens immediately.
    assert!(
        shown.contains("You punch kobold thief for")
            || shown.contains("You swing at kobold thief!")
            || shown.contains("Your swing at kobold thief hits, but glances off its armour."),
        "first strike message: {shown:?}"
    );
}

#[test]
fn attack_matches_name_by_word_prefix() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack thief");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}

#[test]
fn unresolved_attack_falls_through_to_say() {
    // Oracle: "a kobold" in a kobold-less room was spoken aloud.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "a kobold");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You say \"a kobold\""),
        "got: {shown:?}"
    );
    assert!(!shown.contains("*Combat Engaged*"));
}

#[test]
fn bare_a_auto_picks_a_target() {
    // Player testimony: "just a and it picks a target for me".
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "a");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}

#[test]
fn bare_a_with_no_monster_is_said() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.input(s, "a");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You say \"a\""), "got: {shown:?}");
}

#[test]
fn reengaging_prints_combat_off_first() {
    // Oracle: "at thief" while already fighting printed *Combat Off* then
    // *Combat Engaged*.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack kobold");
    core.drain_events();

    core.input(s, "at thief");
    let shown = text_to(&core.drain_events(), s);
    let off = shown.find("*Combat Off*");
    let on = shown.find("*Combat Engaged*");
    assert!(off.is_some() && on.is_some() && off < on, "got: {shown:?}");
}

#[test]
fn multi_word_target_names_resolve() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "a kobold thief");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}

#[test]
fn partial_multi_word_prefixes_resolve() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "a kob th");
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
}

#[test]
fn rounds_exchange_blows_every_five_seconds() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    let m = core
        .spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 })
        .unwrap();
    core.input(s, "attack kobold");
    core.drain_events();

    let events = run_rounds(&mut core, 4);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("You punch kobold thief for")
            || shown.contains("You swing at kobold thief!")
            || shown.contains("glances off its armour"),
        "player swings across rounds: {shown:?}"
    );
    // Unarmed record-less form: the DLL's generic hit template 1140:0x7b7
    // with an empty verb slot ("Kobold thief  you for N damage!").
    assert!(
        shown.contains("Kobold thief  you for"),
        "monster retaliates: {shown:?}"
    );
    assert!(
        core.monster_hp(m).unwrap() < 5000 || core.current_hp(s) < 35,
        "damage flowed somewhere"
    );
}

#[test]
fn no_swings_without_engagement() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    let events = run_rounds(&mut core, 3);
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("punch") && !shown.contains("hits you"),
        "passive-mode monsters never initiate (M6 acquisition): {shown:?}"
    );
}

#[test]
fn moving_away_breaks_combat() {
    let mut content = world();
    content
        .rooms
        .get_mut(&RoomId { map: 1, room: 1 })
        .unwrap()
        .exits[mud_core::content::Direction::North as usize] =
        Some(mud_core::content::Exit {
            dest: RoomId { map: 1, room: 2 },
            exit_type: 0,
            trigger_msg: None,
            ..Default::default()
        });
    content.add_room(Room {
        id: RoomId { map: 1, room: 2 },
        name: "Vestibule".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    });
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack kobold");
    core.drain_events();

    core.input(s, "n");
    core.drain_events();
    let events = run_rounds(&mut core, 3);
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("You punch") && !shown.contains("hits you for"),
        "combat torn down after leaving: {shown:?}"
    );
}

#[test]
fn being_attacked_cancels_a_pending_exit() {
    // The meditation-delay purpose (user-confirmed): you cannot exit the
    // Realm while being attacked.
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack kobold");
    core.drain_events();

    core.input(s, "x"); // meditation starts mid-fight
    core.drain_events();
    let events = run_rounds(&mut core, 4);
    let disconnected = events
        .iter()
        .any(|e| matches!(e, Event::Disconnect(d) if *d == s));
    assert!(
        !disconnected,
        "combat swings must cancel the pending exit"
    );
}

#[test]
fn downed_player_stops_swinging_and_is_blocked() {
    let mut core = Core::new(world(), config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    core.input(s, "attack kobold");
    core.drain_events();
    core.set_current_hp(s, -5); // downed (death floor is -200)

    core.input(s, "n");
    let shown = text_to(&core.drain_events(), s);
    assert!(
        shown.contains("You may not do that while you are mortally wounded!"),
        "got: {shown:?}"
    );

    let events = run_rounds(&mut core, 2);
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("You punch"),
        "helpless players do not swing: {shown:?}"
    );
}

// ---------------------------------------------------------------------------
// Monster attack lines (`re/docs/spellcasting.md` §8.10 + decompile
// `attack_monster_user` 0x2e34b). The fixtures carry the REAL 1.11p message
// records: rat hit 27 / dodge 8307 / miss 8294; kobold hit 41 / dodge 8297 /
// miss 8310, with the shortsword's verb record 8287 filling the %s slots.
//
// DLL result mapping (calculate_attack 0x2b800): a failed to-hit roll leaves
// result 0 = the PLAIN miss lines; result 3 (parry) renders the ", but you
// dodge" framing; result 1 (damage < 1) the armour-deflect glance lines.
// ---------------------------------------------------------------------------

/// The giant rat's real records: no weapon, so the verb/weapon `%s` slots
/// render empty and the verbs live in the templates themselves.
fn add_rat_messages(content: &mut Content) {
    content.add_message(Message {
        id: MessageId(27),
        lines: vec![
            "The %s bites you for %d damage!".into(),
            "The %s bites %s for %s damage!".into(),
            "The giant rat falls to the ground with a tortured squeak.".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(8307),
        lines: vec![
            "The %s %slunges at you, but your armour deflects the blow!".into(),
            "The %s %slunges at %s, but %s armour deflects the blow!".into(),
            "The %s %slunges at %syou, but you dodge out of the way!".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(8294),
        lines: vec![
            "The %s %slunges at %s, %sbut %s dodges out of the way!".into(),
            "The %s %slunges at %syou!".into(),
            "The %s %slunges at %s! %s".into(),
        ],
    });
}

/// The kobold thief's real records plus its wielded shortsword (item 67,
/// miss-verb record 8287) — the weapon fills the `%s` verb/weapon slots.
fn add_kobold_messages(content: &mut Content) {
    content.add_message(Message {
        id: MessageId(41),
        lines: vec![
            "The %s stabs you for %d damage!".into(),
            "The %s stabs %s for %s damage!".into(),
            "The kobold thief falls to the ground with a shrill cry.".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(8297),
        lines: vec![
            "The %s %s you, but your armour deflects the blow!".into(),
            "The %s %s %s, but %s armour deflects the blow!".into(),
            "The %s %s you with their %s, but you dodge!".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(8310),
        lines: vec![
            "The %s %s %s with their %s, but %s dodges!".into(),
            "The %s %s you with their %s!".into(),
            "The %s %s %s with their %s!".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(8287),
        lines: vec!["lunge at".into(), "lunges at".into(), "lunges at".into()],
    });
    content.add_item(Item {
        id: ItemId(67),
        name: "shortsword".into(),
        item_type: 1,
        miss_msg: Some(MessageId(8287)),
        ..Default::default()
    });
}

/// A message-carrying giant rat with a single melee form.
fn rat(accuracy: i16, min: i16, max: i16) -> Monster {
    Monster {
        id: MonsterId(1),
        name: "giant rat".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 5000,
        experience: 9,
        exp_multi: 1,
        armour_class: 10,
        damage_resist: 1,
        magic_resist: 30,
        bs_defence: 0,
        energy: 1000,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [
            AttackForm {
                kind: 1,
                accuracy,
                weight: 100,
                min_damage: min,
                max_damage: max,
                hit_msg: Some(MessageId(27)),
                dodge_msg: Some(MessageId(8307)),
                miss_msg: Some(MessageId(8294)),
                energy: 666,
            },
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
        // Lair mode: passive presence WITHOUT the mode-0/4 evil charge
        // (crime.md §2.5 — created characters ship Warn on Evil ON, and
        // a mode-0 punching bag would refuse every attack).
        behaviour: 3,
        ..Default::default()
    }
}

/// The kobold with its real message ids and shortsword.
fn armed_kobold(accuracy: i16, min: i16, max: i16) -> Monster {
    let mut m = kobold();
    m.weapon = Some(ItemId(67));
    m.attacks[0] = AttackForm {
        kind: 1,
        accuracy,
        weight: 100,
        min_damage: min,
        max_damage: max,
        hit_msg: Some(MessageId(41)),
        dodge_msg: Some(MessageId(8297)),
        miss_msg: Some(MessageId(8310)),
        energy: 666,
    };
    m
}

/// world() with the monster swapped and the fixture race's agility/charm
/// set. Dwarf 30/30 gives defender parry 0 (no parries); 80/80 gives 26,
/// i.e. plenty of result-3 "dodge"-framed swings.
fn arena(monster: Monster, agility: u16, charm: u16) -> Content {
    let mut content = world();
    content.monsters.clear();
    content.add_monster(monster);
    let race = content.races.get_mut(&RaceId(2)).unwrap();
    race.base_stats.agility = agility;
    race.base_stats.charm = charm;
    content
}

fn engage(core: &mut Core, target: &str) -> SessionId {
    let s = create(core, "Dain");
    core.input(s, &format!("attack {target}"));
    core.drain_events();
    s
}

#[test]
fn monster_hit_renders_the_form_hit_message() {
    let mut content = arena(rat(200, 3, 3), 30, 30);
    add_rat_messages(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "rat");
    let shown = text_to(&run_rounds(&mut core, 4), s);
    assert!(
        shown.contains("The giant rat bites you for 3 damage!"),
        "hit uses the form's own verb phrase: {shown:?}"
    );
    assert!(
        !shown.contains("hits you for"),
        "no generic fallback when the record exists: {shown:?}"
    );
}

/// A worn armour fixture with the two armour columns set independently.
fn worn_armour(evasion: i16, damage_resist: i16) -> Item {
    Item {
        id: ItemId(700),
        name: "test corselet".into(),
        weight: 10,
        item_type: 0,
        uses: -1,
        evasion,
        damage_resist,
        worn_on: 11,
        gettable: 1,
        ..Item::default()
    }
}

/// Wear the fixture corselet and take four rounds of a fixed 3-damage bite.
fn bitten_wearing(evasion: i16, damage_resist: i16) -> String {
    let mut content = arena(rat(200, 3, 3), 30, 30);
    add_rat_messages(&mut content);
    content.add_item(worn_armour(evasion, damage_resist));
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let s = create(&mut core, "Dain");
    core.give_item(s, ItemId(700));
    core.input(s, "wear corselet");
    core.input(s, "attack rat");
    core.drain_events();
    text_to(&run_rounds(&mut core, 4), s)
}

/// The two armour columns are NOT interchangeable: `+0x39c` (`dr`) is the
/// damage soak, `+0x342` (`ac`) is the to-hit term. Until 2026-07-26 the
/// port had them transposed, and nothing caught it because every fixture
/// in the suite fought naked — the whole point of this pair.
///
/// Measured corroboration (charm.md §8.3): Oracle Delver wore Σac 125 /
/// Σdr 8, the board printed `Armour Class:  12/0`, and giant bats (2..5)
/// bit for the full undiminished 2..5. A soak of Σac/10 = 12 would have
/// made that character immune.
#[test]
fn worn_dr_soaks_damage_but_worn_ac_does_not() {
    // DR 20 -> soak 20/10 = 2, so the fixed 3-damage bite lands 1.
    let soaked = bitten_wearing(0, 20);
    assert!(
        soaked.contains("bites you for 1 damage!") && !soaked.contains("bites you for 3 damage!"),
        "Σ worn +0x39c is the soak (calculate_attack 25335): {soaked:?}"
    );

    // The same magnitude in the OTHER column soaks nothing. Evasion 20
    // reaches [1] as 2, which against accuracy 200 leaves the to-hit
    // clamp untouched, so the bite still connects for its full 3.
    let unsoaked = bitten_wearing(20, 0);
    assert!(
        unsoaked.contains("bites you for 3 damage!"),
        "Σ worn +0x342 drives to-hit only (25311), never the soak: {unsoaked:?}"
    );
}

#[test]
fn monster_plain_miss_renders_the_miss_line() {
    // Accuracy 5 -> to-hit threshold 5 (sub-formula), so ~95% of swings
    // leave result 0: the PLAIN miss line, no dodge framing.
    let mut content = arena(rat(5, 1, 1), 30, 30);
    add_rat_messages(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "rat");
    let shown = text_to(&run_rounds(&mut core, 10), s);
    assert!(
        shown.contains("The giant rat lunges at you!"),
        "to-hit failure is the plain miss: {shown:?}"
    );
    assert!(
        !shown.contains("but you dodge"),
        "parry impossible at parry rating 0: {shown:?}"
    );
}

#[test]
fn monster_parry_renders_the_dodge_framing() {
    // Agility/charm 80 -> defender parry 26; accuracy 40 -> to-hit 99%
    // and parry chance 26*10/(40/8) = 52%: result 3 swings render the
    // ", but you dodge out of the way!" framing from dodge record line 3.
    let mut content = arena(rat(40, 0, 0), 80, 80);
    add_rat_messages(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "rat");
    let shown = text_to(&run_rounds(&mut core, 20), s);
    assert!(
        shown.contains("The giant rat lunges at you, but you dodge out of the way!"),
        "parry renders the dodge framing: {shown:?}"
    );
}

#[test]
fn monster_glance_renders_the_armour_deflect_line() {
    // min=max=0 damage with high accuracy: connects resolve to result 1
    // (damage < 1) -> dodge record line 1 (ORACLE-VERIFY: decompile-only,
    // never observed in ~300 oracle swings).
    let mut content = arena(rat(200, 0, 0), 30, 30);
    add_rat_messages(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "rat");
    let shown = text_to(&run_rounds(&mut core, 10), s);
    assert!(
        shown.contains("The giant rat lunges at you, but your armour deflects the blow!"),
        "glance uses dodge record line 1: {shown:?}"
    );
}

#[test]
fn observers_see_name_substituted_variants() {
    let mut content = arena(rat(200, 3, 3), 30, 30);
    add_rat_messages(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "rat");
    let o = create(&mut core, "Oracle");
    let events = run_rounds(&mut core, 4);
    let seen = text_to(&events, o);
    assert!(
        seen.contains("The giant rat bites Dain for 3 damage!"),
        "observer hit line substitutes the victim and shows damage: {seen:?}"
    );
    let victim = text_to(&events, s);
    assert!(
        !victim.contains("bites Dain"),
        "the victim's own line stays second-person: {victim:?}"
    );
    assert!(
        !seen.contains("bites you"),
        "observers never get the second-person line: {seen:?}"
    );
}

#[test]
fn observers_see_miss_and_dodge_variants() {
    // Parry-heavy setup: observers get miss record line 1 ("... but he
    // dodges out of the way!") for parries and line 3 for plain misses.
    let mut content = arena(rat(40, 0, 0), 80, 80);
    add_rat_messages(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "rat");
    let o = create(&mut core, "Oracle");
    let seen = text_to(&run_rounds(&mut core, 30), o);
    assert!(
        seen.contains("The giant rat lunges at Dain, but he dodges out of the way!"),
        "observer dodge line: {seen:?}"
    );
    assert!(
        seen.contains("The giant rat lunges at Dain, but his armour deflects the blow!"),
        "observer glance line: {seen:?}"
    );
    let _ = s;
}

#[test]
fn weapon_verbs_fill_the_message_slots() {
    // Kobold + shortsword: verb slots take the weapon's miss record line 2
    // ("lunges at") for the victim view, and the weapon name fills %s.
    let mut content = arena(armed_kobold(40, 0, 0), 80, 80);
    add_kobold_messages(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "kobold");
    let shown = text_to(&run_rounds(&mut core, 30), s);
    assert!(
        shown.contains("The kobold thief lunges at you with their shortsword, but you dodge!"),
        "kobold's shorter dodge framing comes from ITS record: {shown:?}"
    );
    assert!(
        shown.contains("The kobold thief lunges at you, but your armour deflects the blow!"),
        "glance verb slot filled from the weapon: {shown:?}"
    );
}

#[test]
fn weapon_verbs_fill_the_plain_miss_slots() {
    let mut content = arena(armed_kobold(5, 1, 1), 30, 30);
    add_kobold_messages(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "kobold");
    let shown = text_to(&run_rounds(&mut core, 10), s);
    assert!(
        shown.contains("The kobold thief lunges at you with their shortsword!"),
        "plain miss with weapon clause: {shown:?}"
    );
}

// ---------------------------------------------------------------------------
// Record-less melee forms (healer 47, zombie 492, ju-ju zombie 493/772):
// `attack_monster_user` composes their lines from the generic templates at
// seg 1140 (0x7b7/0x7d1 hit, 0xf6b/0xf98 glance, 0xfc5/0xfe8 dodge,
// 0x100e/0x1022 plain miss), with the verb `%s` slots filled from the
// WIELDED WEAPON's records by `move_monster_to_fighter` (1040:1739): hit
// verbs from the hit record lines 2/3, swing verbs from the miss record
// lines 2/3 (hit verbs when no miss record; all empty when unarmed).
// ---------------------------------------------------------------------------

/// The healer's real shape: a record-less melee form plus the wielded
/// longsword (item 64, hit record 8226, miss record 8286 — real strings).
fn healer(accuracy: i16, min: i16, max: i16) -> Monster {
    let mut m = kobold();
    m.id = MonsterId(47);
    m.name = "healer".into();
    m.weapon = Some(ItemId(64));
    m.attacks[0] = AttackForm {
        kind: 1,
        accuracy,
        weight: 100,
        min_damage: min,
        max_damage: max,
        hit_msg: None,
        dodge_msg: None,
        miss_msg: None,
        energy: 666,
    };
    m
}

fn add_healer_weapon(content: &mut Content) {
    content.add_message(Message {
        id: MessageId(8226),
        lines: vec![
            "slash|impale|hack".into(),
            "slashes|impales|hacks".into(),
            "slashes|impales|hacks".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(8286),
        lines: vec!["swing at".into(), "swings at".into(), "swings at".into()],
    });
    content.add_item(Item {
        id: ItemId(64),
        name: "longsword".into(),
        item_type: 1,
        hit_msg: Some(MessageId(8226)),
        miss_msg: Some(MessageId(8286)),
        ..Default::default()
    });
}

#[test]
fn record_less_hit_composes_from_weapon_hit_verbs() {
    // 1140:0x7b7 "%s %s you for %d damage!" + hit record line 2 verb pool,
    // first char uppercased (the DLL's toupper on the composed buffer).
    let mut content = arena(healer(200, 3, 3), 30, 30);
    add_healer_weapon(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(47), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "healer");
    let shown = text_to(&run_rounds(&mut core, 4), s);
    assert!(
        ["slashes", "impales", "hacks"]
            .iter()
            .any(|v| shown.contains(&format!("Healer {v} you for 3 damage!"))),
        "record-less hit composes 0x7b7 with a weapon hit verb: {shown:?}"
    );
    assert!(
        !shown.contains("The healer hits you for"),
        "the invented generic fallback must be gone: {shown:?}"
    );
}

#[test]
fn record_less_hit_room_view_composes_0x7d1() {
    // 1140:0x7d1 "%s %s %s for %s damage!" (name, room verb, victim, dmg).
    let mut content = arena(healer(200, 3, 3), 30, 30);
    add_healer_weapon(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(47), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "healer");
    let o = create(&mut core, "Oracle");
    let seen = text_to(&run_rounds(&mut core, 4), o);
    assert!(
        ["slashes", "impales", "hacks"]
            .iter()
            .any(|v| seen.contains(&format!("Healer {v} Dain for 3 damage!"))),
        "record-less room hit composes 0x7d1: {seen:?}"
    );
    let _ = s;
}

#[test]
fn record_less_dodge_and_glance_compose_the_weapon_clause() {
    // Parry-heavy arena: 1140:0xfc5 "%s %s you with %s, but you dodge!"
    // takes the miss-record swing verb + weapon name; the glance pair
    // (0xf6b/0xf98) uses the same victim-view swing verb.
    let mut content = arena(healer(40, 0, 0), 80, 80);
    add_healer_weapon(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(47), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "healer");
    let o = create(&mut core, "Oracle");
    let events = run_rounds(&mut core, 30);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("Healer swings at you with longsword, but you dodge!"),
        "record-less parry composes 0xfc5: {shown:?}"
    );
    assert!(
        shown.contains("Healer's swings at hits you, but your armour deflects."),
        "record-less glance composes 0xf6b: {shown:?}"
    );
    let seen = text_to(&events, o);
    assert!(
        seen.contains("Healer swings at Dain with its longsword, but he dodges."),
        "record-less room parry composes 0xfe8: {seen:?}"
    );
    assert!(
        seen.contains("Healer's swings at hits Dain, but glances off his armour."),
        "record-less room glance composes 0xf98: {seen:?}"
    );
}

#[test]
fn record_less_plain_miss_composes_0x100e() {
    // Accuracy 5: ~95% plain misses -> "%s %s you with %s." and the room
    // variant "%s %s %s with its %s."
    let mut content = arena(healer(5, 1, 1), 30, 30);
    add_healer_weapon(&mut content);
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(47), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "healer");
    let o = create(&mut core, "Oracle");
    let events = run_rounds(&mut core, 10);
    let shown = text_to(&events, s);
    assert!(
        shown.contains("Healer swings at you with longsword."),
        "record-less plain miss composes 0x100e: {shown:?}"
    );
    let seen = text_to(&events, o);
    assert!(
        seen.contains("Healer swings at Dain with its longsword."),
        "record-less room miss composes 0x1022: {seen:?}"
    );
}

#[test]
fn record_less_unarmed_renders_empty_verb_slots() {
    // The zombies' shape (no weapon, no records): move_monster_to_fighter
    // zeroes every verb buffer, so the same templates render with EMPTY
    // verb/weapon slots — double space and all. Verbatim DLL behavior.
    let mut content = world();
    content
        .monsters
        .get_mut(&MonsterId(7))
        .unwrap()
        .attacks[0]
        .accuracy = 5;
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(7), RoomId { map: 1, room: 1 });
    let s = engage(&mut core, "kobold");
    let shown = text_to(&run_rounds(&mut core, 10), s);
    assert!(
        shown.contains("Kobold thief  you with ."),
        "unarmed record-less plain miss keeps the empty slots: {shown:?}"
    );
    assert!(
        !shown.contains("The kobold thief swings at you!"),
        "the invented plain-miss fallback must be gone: {shown:?}"
    );
}

// --- active-spell Dodge feeds the defender (M5 slice 4) ---

/// The blur (129) record reduced to what the recompute reads: a Dodge
/// ability with value 0 = "use the stored slot value" (spec §4).
fn blur_spell() -> Spell {
    Spell {
        id: SpellId(129),
        name: "blur".into(),
        short_name: "blur".into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![(Ability::Dodge, 0)],
        level_cap: 0,
        round_cost: 0,
        required_power: 1,
        min_base: 5,
        max_base: 5,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200,
        duration_per_level: 0,
        match_type: MatchType::Single0,
        duration: 70,
        element: Element::Cold,
        class_gate_group: 1,
        mana_cost: 4,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

/// A hand-built Dwarf Warrior matching the arena(_, 30, 30) creation
/// output (stats copied verbatim from the racial template; hp_base =
/// class hp_seed) so slots can be pre-seeded before attach.
fn dwarf(name: &str) -> Player {
    let stats = StatBlock {
        intellect: 30,
        wisdom: 50,
        strength: 50,
        health: 50,
        agility: 30,
        charm: 30,
    };
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(2),
        class: ClassId(1),
        level: 1,
        stats,
        base_stats: stats,
        hp_base: 4,
        current_hp: 35,
        current_mana: 0,
        hunger: 1000,
        thirst: 1000,
        coins: Default::default(),
        lawful: false,
        inventory: vec![],
        weapon: None,
        bankbooks: vec![],
        worn: vec![],
        cp_unspent: 100,
        cp_lifetime: 100,
        lives: 9,
        experience: 0,
        location: RoomId { map: 1, room: 1 },
        spellbook: std::collections::BTreeMap::new(),
        poison: 0,
        active_spells: Default::default(),
        ..Default::default()
    }
}

/// Runs the identical seeded attack script and counts result-3 swings
/// (the ", but you dodge out of the way!" framing).
fn parry_framing_count(blur_value: Option<i16>) -> usize {
    // rat(16, 0, 0) vs the 30/30 dwarf: to-hit 99% (naked defense 0),
    // damage 0 (no death over the run), defender parry 0 — so WITHOUT a
    // blur slot a result-3 swing is structurally impossible (the parry
    // branch requires parry > 0). WITH the stored Dodge 5: parry 5,
    // parry chance 5*10/(16/8) = 25% per connecting swing.
    let mut content = arena(rat(16, 0, 0), 30, 30);
    add_rat_messages(&mut content);
    content.add_spell(blur_spell());
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let mut player = dwarf("Dain");
    if let Some(v) = blur_value {
        player.active_spells[0] = ActiveSpell {
            spell: Some(SpellId(129)),
            value: v,
            remaining: 70,
        };
    }
    let s = core.attach_player(player);
    core.drain_events();
    core.input(s, "attack rat");
    core.drain_events();
    let shown = text_to(&run_rounds(&mut core, 20), s);
    assert!(
        shown.contains("but your armour deflects the blow!"),
        "swings connected: {shown:?}"
    );
    shown.matches("but you dodge out of the way!").count()
}

#[test]
fn a_blur_slot_makes_the_defender_dodge() {
    // Identical monster-attack script under the same fixed seed: the only
    // difference is the pre-seeded blur slot, whose stored Dodge 5 feeds
    // the defender's parry rating (combat.md: parry = dodgeAbil(0x22) +
    // stat terms) and turns some hits into the dodge framing.
    assert_eq!(parry_framing_count(None), 0, "parry 0: no dodges possible");
    assert!(
        parry_framing_count(Some(5)) > 0,
        "stored Dodge 5: dodges appear under the same script"
    );
}

#[test]
fn a_worn_dodge_item_feeds_the_defender_parry() {
    // The dodgeAbil defender term was motivated by M4 items (it was
    // missing entirely — worn Dodge never reached parry). Pin the item
    // path, not just the spell-slot path: same script, the Dodge rides a
    // worn item instead of a slot.
    let mut content = arena(rat(16, 0, 0), 30, 30);
    add_rat_messages(&mut content);
    content.add_item(Item {
        id: ItemId(500),
        name: "cloak of shadows".into(),
        worn_on: 7,
        abilities: vec![(Ability::from_id(34).unwrap(), 5)],
        ..Item::default()
    });
    let mut core = Core::new(content, config());
    core.spawn_monster(MonsterId(1), RoomId { map: 1, room: 1 });
    let mut player = dwarf("Dain");
    player.worn = vec![(ItemId(500), -1)];
    let s = core.attach_player(player);
    core.drain_events();
    core.input(s, "attack rat");
    core.drain_events();
    let shown = text_to(&run_rounds(&mut core, 20), s);
    assert!(
        shown.matches("but you dodge out of the way!").count() > 0,
        "worn Dodge 5 produces dodges under the same script: {shown:?}"
    );
}

// --- M7 slice 4: the monster rob form (kind 3) stub port ---

#[test]
fn rob_forms_fall_back_to_the_melee_slot_zero() {
    // theft.md: monster_rob_user (0x295bd) is a `return 0` stub — a
    // kind-3 pick never robs; the caller swings with form slot 0 iff
    // its kind is 1 (attack_monster_user 26808-26813).
    let mut content = world();
    let mut robber = kobold();
    robber.id = MonsterId(8);
    robber.name = "cutpurse".into();
    // Slot 0 melee (weight 0 — never picked directly), slot 1 rob at
    // weight 100 — every pick lands on the rob form.
    robber.attacks[0].weight = 0;
    robber.attacks[1] = AttackForm {
        kind: 3,
        accuracy: 0,
        weight: 100,
        min_damage: 0,
        max_damage: 0,
        hit_msg: None,
        dodge_msg: None,
        miss_msg: None,
        energy: 666,
    };
    content.add_monster(robber);
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    let m = core
        .spawn_monster(MonsterId(8), RoomId { map: 1, room: 1 })
        .unwrap();
    core.input(s, "attack cutpurse");
    core.drain_events();
    let events = run_rounds(&mut core, 6);
    let shown = text_to(&events, s);
    let _ = m;
    assert!(
        shown.contains("you for") || shown.contains("misses you") || shown.contains("swings at you"),
        "the rob pick swings with the melee slot instead: {shown:?}"
    );
}

#[test]
fn rob_form_without_a_melee_slot_zero_never_swings() {
    let mut content = world();
    let mut robber = kobold();
    robber.id = MonsterId(9);
    robber.name = "pickpocket".into();
    robber.attacks[0] = AttackForm {
        kind: 3,
        accuracy: 0,
        weight: 100,
        min_damage: 0,
        max_damage: 0,
        hit_msg: None,
        dodge_msg: None,
        miss_msg: None,
        energy: 666,
    };
    for slot in 1..5 {
        robber.attacks[slot] = AttackForm::default();
    }
    content.add_monster(robber);
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(9), RoomId { map: 1, room: 1 });
    core.input(s, "attack pickpocket");
    core.drain_events();
    let events = run_rounds(&mut core, 6);
    let shown = text_to(&events, s);
    assert!(
        !shown.contains("you for") && !shown.contains("misses you"),
        "kind-3 with a kind-3 slot 0 stays inert: {shown:?}"
    );
}

/// A sandbag carrying `dodge` and `ac`, with DR 0 and a form that cannot
/// hurt back. At AC 0 the to-hit threshold clamps to 99, so the Dwarf L1
/// attacker (accuracy 43, damage 1-4) lands a damaging hit on every swing
/// the parry roll does not cancel -- which is why every parry pin below
/// passes AC 0. Pass a real AC to exercise the to-hit term instead.
fn sandbag(dodge: i16, ac: i16) -> Monster {
    let mut m = kobold();
    m.id = MonsterId(11);
    m.name = "sandbag".into();
    m.armour_class = ac;
    m.damage_resist = 0;
    m.abilities = if dodge == 0 {
        vec![]
    } else {
        vec![(Ability::Dodge, dodge)]
    };
    m.attacks[0] = AttackForm {
        kind: 1,
        accuracy: 0,
        weight: 100,
        min_damage: 0,
        max_damage: 0,
        hit_msg: None,
        dodge_msg: None,
        miss_msg: None,
        energy: 666,
    };
    m
}

/// Runs `rounds` player swings against a sandbag carrying `dodge` and `ac`,
/// and returns (landed hits, HP taken off the sandbag).
fn sandbag_run(dodge: i16, ac: i16, rounds: u64) -> (usize, i32) {
    let mut content = world();
    content.monsters.clear();
    content.add_monster(sandbag(dodge, ac));
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    let m = core
        .spawn_monster(MonsterId(11), RoomId { map: 1, room: 1 })
        .expect("the sandbag spawns");
    core.input(s, "attack sandbag");
    core.drain_events();
    let shown = text_to(&run_rounds(&mut core, rounds), s);
    let hits = shown.matches("You punch sandbag for").count();
    (hits, 5000 - core.monster_hp(m).expect("the sandbag survives"))
}

#[test]
fn monster_dodge_ability_parries_player_swings() {
    // GAMEPLAY DELTA (M7 slice 5): `build_monster_defender` now feeds
    // Dodge(0x22) into the fighter's parry word per
    // `move_monster_to_fighter` 25185-25186, and the DLL runs that ONE
    // build for the player-attacks-monster path too. 167 of the 1101
    // shipped templates carry ability 34 at values 10..200, so this turns
    // a large share of connecting player swings into zero-damage parries.
    //
    // Chance per `calculate_attack` 25336-25360: `parry*10 / (accuracy/8)`
    // capped at 95. Dain is accuracy 43 -> denominator 5, so Dodge 20
    // buys 40% and Dodge 50 buys the 95% cap.
    //
    // ORACLE-VERIFY: the parry formula itself was recovered from the
    // 16-bit disassembly and has never been checked against a live
    // capture. Wiring Dodge into the monster defender build amplifies any
    // error in it across a sixth of the bestiary, so a slice-8 expedition
    // should capture a player grinding a Dodge-carrying template (e.g.
    // giant bat, Dodge 20) and compare the observed miss rate.
    // 40 rounds is 49 swings (the energy pool buys a second swing in
    // some rounds); with no Dodge every one of them lands.
    // (The damage totals below moved by a couple of points in M7 slice 5
    // when the engage-time retaliation lock was pulled out of the swing
    // loop and back onto the ATTACK command, where 26230 actually lives:
    // its `genrdn(1,100)` now precedes the round's damage rolls instead
    // of trailing them. Swing COUNTS are unchanged — this is stream
    // position, not a behaviour change.)
    let (control_hits, control_damage) = sandbag_run(0, 0, 40);
    assert_eq!(control_hits, 49, "a 99%-to-hit swing lands on every swing");
    assert_eq!(control_damage, 119, "49 swings of 1-4 damage");

    let (dodge_hits, dodge_damage) = sandbag_run(20, 0, 40);
    assert_eq!(dodge_hits, 29, "Dodge 20 parries ~40% of the swings");
    assert_eq!(dodge_damage, 70, "only the unparried swings do damage");

    let (capped_hits, capped_damage) = sandbag_run(50, 0, 40);
    assert_eq!(capped_hits, 4, "Dodge 50 pins the parry chance at its 95 cap");
    assert_eq!(capped_damage, 10, "almost nothing gets through");
}

#[test]
fn a_monsters_armour_class_is_a_whole_unit_not_a_tenths_scale() {
    // MEASURED (`re/oracle/oracle_dodge_parry_control.raw`, 2026-07-26):
    // `build_monster_defender` feeds the RAW template `ac` column into the
    // evasion word, while the PLAYER's own side of the same word divides its
    // item column by ten (24866). The shipped `ac` column runs 0..9999 with a
    // mean of 101 against a geared player's ~12, which is enough of a mismatch
    // to be worth a capture: if monster `ac` were also in tenths, every
    // monster in the game would be far easier to hit than we model.
    //
    // Oracle Delver (accuracy 23) swung a wooden hammer at grey spiders
    // (#30, AC 20, DR 2, and -- unlike the giant bat -- NO Dodge(0x22), so
    // every connect is a glance and the parry channel cannot confound the
    // count):
    //
    //     swings 21: glance 2, miss 19  ->  connect 2/21 = 0.0952
    //                                       95% CI [0.0117, 0.3038]
    //
    // `threshold = 100 - defense^2/(accuracy^2/14/10)` clamped [10, 99] reads
    // the raw column as 400/3 = 133 over 100, i.e. the clamp FLOOR of 0.10 --
    // which is what the board did. A column needing its own /10 predicts
    // defense 2 and a threshold of 0.99, which the interval EXCLUDES outright.
    // So the port is right as written, and this is also the first live
    // measurement of the clamp floor itself.
    //
    // Every other pin in this file fights an AC 0 sandbag, where the
    // threshold clamps to 99 and the to-hit term never bites -- the same
    // blindness that let the player armour columns sit swapped since M4. This
    // one carries a real AC so the term is exercised.
    //
    // Dain is accuracy 43, so den = 43^2/14/10 = 13 and AC 20 predicts
    // 100 - 400/13 = 70% of 49 swings ~= 34 landing. Reading the column in
    // tenths instead gives defense 2, a threshold clamped to 99, and ~48.
    // The band below is +-3 binomial sd around 34 and excludes that.
    let (hits, _damage) = sandbag_run(0, 20, 40);
    assert!(
        (25..=43).contains(&hits),
        "AC 20 against accuracy 43 predicts ~34 of 49 swings landing, got {hits}"
    );
    assert!(
        hits < 47,
        "a tenths-scale reading of the ac column would clamp to 99% and land \
         ~48 of 49; got {hits}, which would mean the column was divided twice"
    );
}

#[test]
fn a_parried_swing_renders_the_dodge_wording_not_a_plain_miss() {
    // MEASURED (charm.md §8.3, `oracle_dodge_parry_*.raw`): the board words
    // result 3 on the PLAYER-attacks-monster path apart from a plain miss --
    //
    //     You swing at giant bat who dodges your attack!
    //
    // and WCCMMUD.DLL carries the template verbatim at file offset 0xca40d,
    // `You %s %s who dodges your attack!`, sitting one slot after the plain
    // miss `You %s %s!` (0xca3ea) and one before the monster-side result-3
    // pair (0xca430/0xca464). We rendered both outcomes as the plain miss,
    // which is what this pins.
    //
    // Dodge 50 against Dain's accuracy 43 pins the parry chance at its 95
    // cap, so a 40-round run is essentially all parries.
    let mut content = world();
    content.monsters.clear();
    content.add_monster(sandbag(50, 0));
    let mut core = Core::new(content, config());
    let s = create(&mut core, "Dain");
    core.spawn_monster(MonsterId(11), RoomId { map: 1, room: 1 })
        .expect("the sandbag spawns");
    core.input(s, "attack sandbag");
    core.drain_events();
    let shown = text_to(&run_rounds(&mut core, 40), s);
    assert!(
        shown.contains("You swing at sandbag who dodges your attack!"),
        "a parried swing must carry the dodge wording: {shown:?}"
    );
}
