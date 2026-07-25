//! Golden end-to-end scenario (M5 slice 3, Task 13): one continuous seeded
//! session in which a mage with an empty book checks `spells`, buys a
//! LearnSp scroll from the shop, learns it with `use`, sees the one-row
//! listing, walks to the monster's lair, opens with `c mmis filthbug`, and
//! the combat driver fires magic missiles until the kill routes through
//! M3's death/exp path. Every oracle-pinned string is asserted verbatim and
//! the key lines are asserted IN ORDER over the accumulated transcript.
//!
//! Determinism: `CoreConfig::rng_seed` drives every genrdn stream (success
//! rolls + magnitude). Under SEED both 70%-chance rolls succeed and the
//! kill lands on combat round 2 (raw magnitudes 12 then 10, Damage(-MR)
//! amplified by +20% against the filthbug's mr 30 into fires of 14 then 12
//! against 20 HP); the round bound exists so a broken driver fails with a
//! message instead of spinning. The failed-roll path is covered in
//! tests/cast.rs.
//!
//! The slice-4 companion scenario (`mage_learns_blur_and_outlives_it`)
//! runs the duration lifecycle end to end: learn blur from its scroll,
//! cast it (slot entry, the `st` active line, Dodge into the defender),
//! idle through the 70 upkeep ticks, and outlive it (wear-off line, st
//! line gone, Dodge gone, slot empty).
//!
//! The slice-5 companions: `mage_blurs_a_second_player_who_outlives_it`
//! drives the §8.13 player-target lifecycle across sessions (caster/
//! target/room three-view lines, slot on the TARGET, the target's `st`
//! line, target-only wear-off), and
//! `area_cast_sweeps_monsters_and_spares_the_bystander` drives the area
//! surface (pre-charge no-effect refusal with a player present, the
//! caster + single room fan-out lines, per-monster damage with a kill,
//! the bystander untouched).
//!
//! The slice-6 companion (`caster_monster_fight_and_the_live_poison_
//! lifecycle`, MEASURED §8.14 where noted): a 65% caster monster lands
//! and fizzles casts at the player (victim line WITH the damage number,
//! room line WITHOUT — the record-8455 fan-out), then a venom caster
//! runs the live poison lifecycle end to end (slot entry, counter
//! hard-write, the "You feel ill." slow tick, termination reversal).

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Direction, Element, Exit, Item, ItemId, MatchType,
    Message, MessageId, Monster, MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair,
    Shop, ShopId, ShopStock, Spell, SpellId, StatBlock, TargetMode,
};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const MAGE: ClassId = ClassId(1);
const MAGIC_MISSILE: SpellId = SpellId(20);
const MMIS_SCROLL: ItemId = ItemId(200);
/// The real blur's spell number (129) — the slice-4 duration scenario.
const BLUR: SpellId = SpellId(129);
const BLUR_SCROLL: ItemId = ItemId(210);
const FILTHBUG: MonsterId = MonsterId(7);
/// Fixture offensive instant match-12 area — no damaging area spell is
/// learnable in shipped data (§8.13 measured only benign debuff areas),
/// so the damage sweep is fixture-shaped: Damage(0) 12..12, Magic
/// (unresistable), generic "on the room!" castmsgb frame.
const SHOCKWAVE: SpellId = SpellId(140);
/// The 5 HP area-kill probe (dies to any 12..=13 sweep roll).
const WISP: MonsterId = MonsterId(8);
/// The slice-6 caster monster (§8.14's moaning spirit 66): one kind-2
/// form, 65% (the dark cleric's sub-100 band — the fizzle probe), spell
/// DRAW_BREATH at cast level 8.
const SPIRIT: MonsterId = MonsterId(9);
/// The venom caster (§8.14's tentacled abomination 37 shape): kind-2
/// form at 101% (deterministic), spell SPIT_VENOM.
const HORROR: MonsterId = MonsterId(10);
/// The real `draws the breath` (82, MEASURED §8.14): instant Drain,
/// bounds 4..12 with zero scaling pairs (no visible cast-level scaling
/// observed live), castmsgb = the record-8455 mirror — victim line WITH
/// the damage number, room line WITHOUT one.
const DRAW_BREATH: SpellId = SpellId(150);
/// The real `spits a stream of venom` (79) shape: duration Poison(6) +
/// the record-8575 DescMsg ("You feel ill." entry line / "The effects of
/// the poison wear off!" expiry). Duration fixture-shortened 100 -> 12 so
/// the lifecycle (entry, one slow tick, expiry) fits one test window.
const SPIT_VENOM: SpellId = SpellId(151);

/// Newhaven-shaped shop room (protected, shop-active) with the lair north.
const SHOP_ROOM: RoomId = RoomId { map: 1, room: 1 };
const LAIR: RoomId = RoomId { map: 1, room: 2 };
/// The venom caster's den, east of the shop (LAIR keeps its single south
/// exit — the round-1 golden pins that exits line).
const PIT: RoomId = RoomId { map: 1, room: 3 };

/// The seed the whole transcript is pinned under ("MMUD_WG!", the
/// CoreConfig default — restated here so the golden numbers below cannot
/// drift if the default ever changes).
const SEED: u64 = 0x4d4d55445f574721;

/// Combat rounds allowed before the test declares the driver broken. Under
/// SEED the kill lands on round 2.
const ROUND_BOUND: usize = 20;

fn world() -> Content {
    let mut content = Content::default();
    let mut shop_room = Room {
        id: SHOP_ROOM,
        name: "Spell Shop".into(),
        description: vec![],
        room_type: 1, // shop-active
        attributes: 1, // protected, like the real Newhaven shops
        shop: Some(ShopId(48)),
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    shop_room.exits[Direction::North as usize] = Some(Exit {
        dest: LAIR,
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    shop_room.exits[Direction::East as usize] = Some(Exit {
        dest: PIT,
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    content.add_room(shop_room);
    let mut pit = Room {
        id: PIT,
        name: "Slimy Pit".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    pit.exits[Direction::West as usize] = Some(Exit {
        dest: SHOP_ROOM,
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    content.add_room(pit);
    let mut lair = Room {
        id: LAIR,
        name: "Dusty Cellar".into(),
        description: vec![],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    lair.exits[Direction::South as usize] = Some(Exit {
        dest: SHOP_ROOM,
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    content.add_room(lair);
    // A silent punching bag (no attack forms): 20 HP dies in 2-4 fires of
    // the MR-scaled mmis. mr 30 is the REAL filthbug's (monster 3) — with
    // no AntiMagic the Damage(-MR) reduction is 0, so every fire is
    // amplified by (50-30)% = +20%.
    content.add_monster(Monster {
        id: FILTHBUG,
        name: "nasty filthbug".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 20,
        experience: 12,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 0,
        magic_resist: 30,
        bs_defence: 0,
        energy: 0,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: Default::default(),
        ..Default::default()
    });
    // The area-kill probe: dies to any 12..=13 shockwave roll.
    content.add_monster(Monster {
        id: WISP,
        name: "sickly wisp".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 5,
        experience: 9,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 0,
        magic_resist: 0,
        bs_defence: 0,
        energy: 0,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: Default::default(),
        ..Default::default()
    });
    // §8.14's moaning spirit shape: cast-only (no melee form), 65% cast
    // success — the dark cleric/priest band the expedition could NOT
    // reach live (every reachable caster was 100%), so the fizzle line
    // here rests on the decompile strings (00481338/00481369). Energy
    // 400 of the 1000 pool bounds it to 2-4 attempts per round.
    content.add_monster(Monster {
        id: SPIRIT,
        name: "moaning spirit".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 400,
        experience: 30,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 0,
        magic_resist: 0,
        bs_defence: 0,
        energy: 1000,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [
            AttackForm {
                kind: 2,
                accuracy: DRAW_BREATH.0 as i16, // spell id
                weight: 100,
                min_damage: 65, // cast success %
                max_damage: 8,  // cast level (§8.14: spirit form lvl 8)
                hit_msg: None,
                dodge_msg: None,
                miss_msg: None,
                energy: 400,
            },
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
        ..Default::default()
    });
    // The venom caster: 101% keeps the seeded transcript free of fizzle
    // noise (the abomination's own % is unmeasured — it sits 71 rooms
    // deep, §8.14 survey).
    content.add_monster(Monster {
        id: HORROR,
        name: "tentacled horror".into(),
        move_msg: None,
        death_msg: None,
        abilities: vec![],
        hitpoints: 400,
        experience: 30,
        exp_multi: 1,
        armour_class: 0,
        damage_resist: 0,
        magic_resist: 0,
        bs_defence: 0,
        energy: 1000,
        coins: [0; 5],
        weapon: None,
        loot: vec![],
        attacks: [
            AttackForm {
                kind: 2,
                accuracy: SPIT_VENOM.0 as i16,
                weight: 100,
                min_damage: 101, // always passes genrdn(0,100) < pct
                max_damage: 6,   // cast level (duration is flat anyway)
                hit_msg: None,
                dodge_msg: None,
                miss_msg: None,
                energy: 400,
            },
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
            AttackForm::default(),
        ],
        ..Default::default()
    });
    // The mmis castmsgb shape (message 3242; line 3's damage is %s).
    content.add_message(Message {
        id: MessageId(900),
        lines: vec![
            "You fire a %s at %s for %d damage!".into(),
            "%s fires a %s at you for %d damage!".into(),
            "%s fires a %s at %s for %s damage!".into(),
        ],
    });
    // The blur castmsgb shape (message 7): a targeted benign template; a
    // self-cast delivers the caster line only (oracle §8.6).
    content.add_message(Message {
        id: MessageId(901),
        lines: vec![
            "You cast %s on %s!".into(),
            "%s casts %s upon you!".into(),
            "%s casts %s on %s!".into(),
        ],
    });
    // Blur's DescMsg record (msg 68), MEASURED §8.9/§8.11: line1 = the
    // expiry line, line2 = empty (room variant unmeasured), line3 = the
    // active line printed at cast and appended to `st`.
    content.add_message(Message {
        id: MessageId(904),
        lines: vec![
            "The effects of blur wear off.".into(),
            String::new(),
            "You are blurred!".into(),
        ],
    });
    // The generic area castmsgb frame (the message-89/75 room shape,
    // §8.13: stinking cloud's room view was "Zinvar casts stinking cloud
    // on the room!"). The target line never fires on the area path.
    content.add_message(Message {
        id: MessageId(905),
        lines: vec![
            "You cast %s on the room!".into(),
            "%s casts %s on you!".into(),
            "%s casts %s on the room!".into(),
        ],
    });
    // The `draws the breath` castmsgb — the REAL record 8455, verbatim
    // (MEASURED §8.14: "Moaning spirit draws the breath from your body
    // for 11 damage!" / room "Moaning spirit draws the breath from
    // Zinvar's body!"). Even-style target line binds (caster, spell,
    // damage); the room line binds (caster, spell, target) and carries
    // NO damage slot — the victim alone sees the number, the exact
    // inverse of the melee pair (§8.10 room lines DO print damage).
    content.add_message(Message {
        id: MessageId(906),
        lines: vec![
            "You %s from %s for %d damage!".into(),
            "%s %s from your body for %d damage!".into(),
            "%s %s from %s's body!".into(),
        ],
    });
    // The venom-family DescMsg — the REAL record 8575, verbatim: line1
    // the wear-off, line3 the entry active line (§8.14 pinned the tick
    // line "You feel ill." live; the DescMsg strings themselves are
    // DB-read).
    content.add_message(Message {
        id: MessageId(907),
        lines: vec![
            "The effects of the poison wear off!".into(),
            String::new(),
            "You feel ill.".into(),
        ],
    });
    // The `spits a stream of venom` castmsgb — the REAL record 8446,
    // verbatim: no damage slot on any line (the payload is pure poison).
    content.add_message(Message {
        id: MessageId(908),
        lines: vec![
            "You %s at %s!".into(),
            "The %s %s at you!".into(),
            "The %s %s at %s!".into(),
        ],
    });
    content.add_item(Item {
        id: MMIS_SCROLL,
        name: "scroll of magic missile".into(),
        abilities: vec![(Ability::from_id(42).unwrap(), MAGIC_MISSILE.0 as i16)],
        uses: 1,
        gettable: 1,
        description: vec![
            "This parchment is inscribed with runes of magic, but exactly".into(),
            "what is written can only be learned by reading it.".into(),
        ],
        ..Item::default()
    });
    content.add_item(Item {
        id: BLUR_SCROLL,
        name: "scroll of blur".into(),
        abilities: vec![(Ability::from_id(42).unwrap(), BLUR.0 as i16)],
        uses: 1,
        gettable: 1,
        description: vec![
            "This parchment is inscribed with runes of magic, but exactly".into(),
            "what is written can only be learned by reading it.".into(),
        ],
        ..Item::default()
    });
    let mut stock = [ShopStock::default(); 20];
    stock[0] = ShopStock {
        item: Some(MMIS_SCROLL),
        max: 5,
        now: 5,
        ..ShopStock::default()
    };
    stock[1] = ShopStock {
        item: Some(BLUR_SCROLL),
        max: 5,
        now: 5,
        ..ShopStock::default()
    };
    content.add_shop(Shop {
        id: ShopId(48),
        name: "Spell Shop".into(),
        shop_type: 2,
        min_level: 0,
        max_level: 0,
        markup: 0,
        class_limit: 0,
        stock,
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
    // The real mmis shape: rolled Damage(-MR) (17, value 0 — the ability
    // the real spell 1 carries, NOT plain Damage) over 4..=13 (bounds
    // 4..12, max_increase 1/0 = the zero-denominator guard), base_chance
    // 15, mana 1 (a failed roll deducts floor(1/2) = 0), Magic =
    // unresistable.
    let mmis = Spell {
        id: MAGIC_MISSILE,
        name: "magic missile".into(),
        short_name: "mmis".into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(900)),
        abilities: vec![(Ability::DamageMR, 0)],
        level_cap: 0,
        round_cost: 100,
        required_power: 1,
        min_base: 4,
        max_base: 12,
        target_mode: TargetMode::Offensive0,
        save_class: SaveClass::None,
        base_chance: 15,
        duration_per_level: 0,
        // The real spell 1 ships `target` 8 (mud-server's load_real_db
        // pins it): the cast dispatcher routes on the MATCH type, and 8
        // is the offensive single-target band.
        match_type: MatchType::Special8,
        duration: 0,
        element: Element::Magic,
        class_gate_group: 1,
        mana_cost: 1,
        max_increase: ScalePair { per: 1, levels: 0 },
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    };
    content.add_spell(mmis);
    // The real blur (129) shape: duration 70 flat (no scaling), magnitude
    // bounds 5..5 (rolled V 5..=6 stored as the slot value), mana 4,
    // abilities [(Dodge, 0), (DescMsg, 68 -> fixture 904)]. base_chance
    // 200 keeps the cast deterministic (the real record rolls); the real
    // (RemovesSpell, 157) anti-stacking row is omitted — spell 157 (the
    // amethyst pendant's effect) is not in this world, and the dispel
    // pre-pass is covered in tests/cast.rs.
    let blur = Spell {
        id: BLUR,
        name: "blur".into(),
        short_name: "blur".into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(901)),
        abilities: vec![(Ability::Dodge, 0), (Ability::DescMsg, 904)],
        level_cap: 0,
        round_cost: 100,
        required_power: 1,
        min_base: 5,
        max_base: 5,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 200,
        duration_per_level: 0,
        // The real blur is match 2: explicit-target benign — a bare
        // `c blur` self-casts, `c blur <player>` slots on the target.
        match_type: MatchType::Single2,
        duration: 70,
        element: Element::Magic,
        class_gate_group: 1,
        mana_cost: 4,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    };
    content.add_spell(blur);
    // The area-sweep fixture (no learnable damaging area ships): match 12
    // iterates the room's live monsters only, one magnitude roll shared
    // by every target (12 does not split), Magic = unresistable so both
    // hits land the raw roll. base_chance 200 keeps the cast
    // deterministic, mirroring the blur record above.
    let shockwave = Spell {
        id: SHOCKWAVE,
        name: "shockwave".into(),
        short_name: "shoc".into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(905)),
        abilities: vec![(Ability::Damage, 0)],
        level_cap: 0,
        round_cost: 100,
        required_power: 1,
        min_base: 12,
        max_base: 12,
        target_mode: TargetMode::Offensive0,
        save_class: SaveClass::None,
        base_chance: 200,
        duration_per_level: 0,
        match_type: MatchType::AreaC,
        duration: 0,
        element: Element::Magic,
        class_gate_group: 1,
        mana_cost: 6,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    };
    content.add_spell(shockwave);
    // The real `draws the breath` (82) shape: instant (Drain, 0) rolled
    // over the record band 4..12, every ScalePair NONE (§8.14: rolls
    // 4,5,8,11,12,12 all inside 4..12 at cast level 8 — no visible
    // scaling), msg_style 32 (even), MODE 3 like the mummy's breathes
    // (routing keys on MATCH 0 alone; mode only skips the elemental
    // scale). SaveClass::None: §8.14 never captured a resist (8 casts,
    // zero resist lines) and the resist family stays decompile-only.
    let draw_breath = Spell {
        id: DRAW_BREATH,
        name: "draws the breath".into(),
        short_name: "drbr".into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(906)),
        abilities: vec![(Ability::Drain, 0)],
        level_cap: 0,
        round_cost: 0,
        required_power: 5,
        min_base: 4,
        max_base: 12,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 0, // the monster path never reads difficulty
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
        msg_style: 32,
    };
    content.add_spell(draw_breath);
    // The real `spits a stream of venom` (79) shape: offensive duration
    // Poison(6) + DescMsg, duration 12 (real: 100 — shortened so entry,
    // one slow tick and expiry fit one window; §6.5 fixed duration, no
    // caster scaling either way).
    let spit_venom = Spell {
        id: SPIT_VENOM,
        name: "spits a stream of venom".into(),
        short_name: "spit".into(),
        cast_msg_a: None,
        cast_msg_b: Some(MessageId(908)),
        abilities: vec![(Ability::Poison, 6), (Ability::DescMsg, 907)],
        level_cap: 0,
        round_cost: 0,
        required_power: 5,
        min_base: 6,
        max_base: 10,
        target_mode: TargetMode::Offensive1,
        save_class: SaveClass::None,
        base_chance: 0,
        duration_per_level: 0,
        match_type: MatchType::Single0,
        duration: 12,
        element: Element::Magic,
        class_gate_group: 0,
        mana_cost: 0,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 32,
    };
    content.add_spell(spit_venom);
    content
}

/// The scenario mage: L1, empty book, in the shop. Int 60 / Wis 30 gives
/// SC = 2 + (60*3+30)/6 + 3*5 + 3 = 55, so the mmis chance is
/// min(55+15, 98) = 70%; health 50 keeps the derived max HP sane.
fn mage() -> Player {
    let stats = StatBlock {
        intellect: 60,
        wisdom: 30,
        strength: 30,
        health: 50,
        agility: 30,
        charm: 30,
    };
    Player {
        name: "Vexil".into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: MAGE,
        level: 1,
        stats,
        base_stats: stats,
        hp_base: 0,
        current_hp: 10,
        current_mana: 10,
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
        location: SHOP_ROOM,
        spellbook: BTreeMap::new(),
        poison: 0,
        active_spells: Default::default(),
        ..Default::default()
    }
}

/// A second body for the multi-session scenarios — the mage frame with a
/// different name and an empty book (the class never matters: the body
/// only receives casts and reads its own sheet).
fn body(name: &str) -> Player {
    Player {
        name: name.into(),
        ..mage()
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

/// Sends one line and appends the session's output to the transcript.
fn drive(core: &mut Core, s: SessionId, transcript: &mut String, line: &str) {
    core.input(s, line);
    transcript.push_str(&text_to(&core.drain_events(), s));
}

/// One combat round (the Job::Energy cadence is 5 ticks).
fn combat_round(core: &mut Core, s: SessionId) -> String {
    for _ in 0..5 {
        core.tick();
    }
    text_to(&core.drain_events(), s)
}

/// Sends one line as `s` and fans the drained output into EVERY view's
/// transcript — the multi-session drive.
fn drive_all(core: &mut Core, s: SessionId, views: &mut [(SessionId, String)], line: &str) {
    core.input(s, line);
    let events = core.drain_events();
    for (sid, transcript) in views.iter_mut() {
        transcript.push_str(&text_to(&events, *sid));
    }
}

/// Runs `n` raw game ticks, fanning every drained event into its view.
fn tick_all(core: &mut Core, views: &mut [(SessionId, String)], n: usize) {
    for _ in 0..n {
        core.tick();
        let events = core.drain_events();
        for (sid, transcript) in views.iter_mut() {
            transcript.push_str(&text_to(&events, *sid));
        }
    }
}

/// Asserts every `(what, line)` pair appears in `transcript`, verbatim
/// and in order.
fn assert_in_order(transcript: &str, pairs: &[(&str, &str)]) {
    let mut last = 0;
    for (what, line) in pairs {
        let at = transcript[last..]
            .find(line)
            .unwrap_or_else(|| panic!("{what} missing/out of order: {transcript:?}"));
        last += at + line.len();
    }
}

#[test]
fn mage_learns_scroll_casts_and_kills() {
    let config = CoreConfig {
        rng_seed: SEED,
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let s = core.attach_player(mage());
    let m = core.spawn_monster(FILTHBUG, LAIR).expect("fixture template");
    core.drain_events();

    let mut transcript = String::new();
    drive(&mut core, s, &mut transcript, "spells");
    drive(&mut core, s, &mut transcript, "buy scroll of magic missile");
    drive(&mut core, s, &mut transcript, "use scroll of magic missile");
    drive(&mut core, s, &mut transcript, "spells");
    drive(&mut core, s, &mut transcript, "n");
    drive(&mut core, s, &mut transcript, "c mmis filthbug");

    // The engagement zeroes the round pool; the driver fires from the next
    // combat round on, one cast per round, until the kill.
    let mut rounds = 0;
    while core.monster_hp(m).is_some() {
        rounds += 1;
        assert!(
            rounds <= ROUND_BOUND,
            "no kill within {ROUND_BOUND} rounds; transcript: {transcript:?}"
        );
        transcript.push_str(&combat_round(&mut core, s));
    }
    assert_eq!(rounds, 2, "the SEED transcript kills on round 2: {transcript:?}");

    // Key lines, verbatim (oracle-pinned) and in order.
    assert_in_order(&transcript, &[
        // §8.5 empty book: single line, no header.
        ("empty book", "You have no spells.\n"),
        // Shop purchase (economy.md oracle string, free item).
        ("bought", "You just bought scroll of magic missile for nothing.\n"),
        // §8.4 `use` learn line (a trailing blank line follows).
        (
            "learn",
            "You read scroll of magic missile and learn the spell magic missile.\n\n",
        ),
        // §8.5 golden listing: header + the one row, byte-for-byte
        // (trailing spaces and the closing blank line are part of it).
        (
            "spells row",
            concat!(
                "You have the following spells:\n",
                "Level Mana Short Spell Name\n",
                "  1   1    mmis  magic missile                 \n",
                "\n",
            ),
        ),
        // Walking into the lair: room name, the monster, the way back.
        ("lair", "Dusty Cellar\nAlso here: nasty filthbug.\nObvious exits: south\n"),
        // §8.6: the opening cast engages without firing.
        ("engaged", "*Combat Engaged*"),
        // Round 1 under SEED: raw magnitude 10 (of 4..=13), Damage(-MR)
        // amplified against mr 30: 10 + 10*20/100 = 12.
        // Slice-3 stream re-pin: the free-attack roll on each walk and
        // the retaliation-gate draws shifted the magnitudes (14 then 12).
        ("first fire", "You fire a magic missile at nasty filthbug for 14 damage!\n"),
        // Round 2 under SEED: raw 12 -> 12 + 12*20/100 = 14, the kill.
        ("killing fire", "You fire a magic missile at nasty filthbug for 12 damage!\n"),
        // M3 death path: death line, exp split, disengage — in order.
        ("death", "The nasty filthbug is dead.\n"),
        ("exp", "You gain 12 experience.\n"),
        ("combat off", "*Combat Off*"),
    ]);

    // Session end state: book learned, scroll consumed, exp banked, mana
    // charged only for the two successful fires (10 - 2).
    let after = core.player_snapshot(s);
    assert_eq!(after.spellbook.get(&MAGIC_MISSILE), Some(&false));
    assert!(after.inventory.is_empty(), "scroll consumed");
    assert_eq!(after.experience, 12);
    assert_eq!(core.current_mana(s), 8, "1 mana per successful fire");
    assert_eq!(core.monster_hp(m), None, "instance gone");
}

/// Slice-4 golden scenario: the blur lifecycle, seeded end to end. The
/// mage buys and reads the blur scroll, casts it (slot entry, the DescMsg
/// active line at cast AND on the `st` sheet, the rolled Dodge value into
/// the defender's recompute), then idles through the full duration —
/// `Job::Upkeep` fires every 3 game ticks (the §8.11-measured ~3 s tick),
/// so blur's 70 ticks expire on game-second 210 with the wear-off line,
/// the st line gone, the Dodge contribution gone, and the slot empty.
#[test]
fn mage_learns_blur_and_outlives_it() {
    let config = CoreConfig {
        rng_seed: SEED,
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let s = core.attach_player(mage());
    core.drain_events();
    let base_parry = core.defender_debug(s).parry;

    let mut transcript = String::new();
    drive(&mut core, s, &mut transcript, "buy scroll of blur");
    drive(&mut core, s, &mut transcript, "use scroll of blur");
    drive(&mut core, s, &mut transcript, "c blur");

    // Slot entry: the rolled magnitude (bounds 5..5 -> genrdn 5..=6; 6
    // under SEED) is stored and feeds the defender through the recompute.
    let p = core.player_snapshot(s);
    let idx = p.find_active(BLUR).expect("blur entered a slot");
    let v = i32::from(p.active_spells[idx].value);
    assert_eq!(v, 6, "the SEED magnitude roll");
    assert_eq!(p.active_spells[idx].remaining, 70, "flat duration");
    assert_eq!(
        core.defender_debug(s).parry,
        base_parry + v,
        "dodge feeds the defender"
    );
    assert_eq!(core.current_mana(s), 6, "full mana 4 paid");

    // The sheet appends the active line while the buff lives (§8.11).
    drive(&mut core, s, &mut transcript, "st");

    // 69 upkeep firings (game ticks 3, 6, .., 207): one shy of expiry.
    for _ in 0..209 {
        core.tick();
    }
    transcript.push_str(&text_to(&core.drain_events(), s));
    let p = core.player_snapshot(s);
    assert_eq!(p.active_spells[idx].remaining, 1, "one tick left");

    // Game tick 210 = the 70th upkeep: expiry.
    core.tick();
    transcript.push_str(&text_to(&core.drain_events(), s));
    let p = core.player_snapshot(s);
    assert!(
        p.active_spells.iter().all(|slot| slot.spell.is_none()),
        "slot empty after expiry"
    );
    assert_eq!(core.defender_debug(s).parry, base_parry, "dodge contribution gone");

    // The post-expiry sheet, captured separately for the absence check.
    core.input(s, "st");
    let after_sheet = text_to(&core.drain_events(), s);
    assert!(after_sheet.contains("Vexil"), "sheet rendered: {after_sheet:?}");
    assert!(
        !after_sheet.contains("You are blurred!"),
        "st line gone after expiry: {after_sheet:?}"
    );
    transcript.push_str(&after_sheet);

    // Key lines, verbatim (oracle-pinned) and in order.
    assert_in_order(
        &transcript,
        &[
            // Shop purchase (economy.md oracle string, free item).
            ("bought", "You just bought scroll of blur for nothing.\n"),
            // §8.4 `use` learn line (a trailing blank line follows).
            (
                "learn",
                "You read scroll of blur and learn the spell blur.\n\n",
            ),
            // §8.6/§8.11 cast order: castmsgb caster line, then DescMsg
            // line3.
            ("cast line", "You cast blur on Vexil!\n"),
            ("active line", "You are blurred!\n"),
            // §8.11: the sheet repeats the active line while the buff
            // lives.
            ("st line", "You are blurred!\n"),
            // §8.9/§8.11: expiry after the 70 measured ticks.
            ("wear-off", "The effects of blur wear off.\n"),
        ],
    );
}

/// Slice-5 golden scenario (MEASURED §8.13): the mage blurs a SECOND
/// player. Three sessions capture the three-view string table — caster
/// `You cast blur on Oracle!`, target `Vexil casts blur upon you!` + the
/// async DescMsg line, room `Vexil casts blur on Oracle!` — the slot
/// enters on the TARGET (value into Oracle's defender, the `st` active
/// line on Oracle's sheet), and after the 70 upkeep ticks the wear-off
/// line reaches the TARGET ONLY (§8.13: Kaimon, in-room, saw nothing at
/// expiry).
#[test]
fn mage_blurs_a_second_player_who_outlives_it() {
    let config = CoreConfig {
        rng_seed: SEED,
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let vex = core.attach_player(mage());
    let ora = core.attach_player(body("Oracle"));
    let kai = core.attach_player(body("Kaimon"));
    core.drain_events();
    let caster_parry = core.defender_debug(vex).parry;
    let target_parry = core.defender_debug(ora).parry;

    let mut views = vec![
        (vex, String::new()),
        (ora, String::new()),
        (kai, String::new()),
    ];
    drive_all(&mut core, vex, &mut views, "buy scroll of blur");
    drive_all(&mut core, vex, &mut views, "use scroll of blur");
    drive_all(&mut core, vex, &mut views, "c blur oracle");

    // The slot enters on the TARGET: the rolled magnitude (bounds 5..5 ->
    // genrdn 5..=6; 6 under SEED) lands in Oracle's slot and Oracle's
    // defender; the caster keeps neither.
    let target = core.player_snapshot(ora);
    let idx = target.find_active(BLUR).expect("blur entered the TARGET's slot");
    let v = i32::from(target.active_spells[idx].value);
    assert_eq!(v, 6, "the SEED magnitude roll");
    assert_eq!(target.active_spells[idx].remaining, 70, "flat duration");
    assert!(
        core.player_snapshot(vex).active_spells.iter().all(|slot| slot.spell.is_none()),
        "no slot on the caster"
    );
    assert_eq!(
        core.defender_debug(ora).parry,
        target_parry + v,
        "dodge feeds the TARGET's defender"
    );
    assert_eq!(core.defender_debug(vex).parry, caster_parry, "caster untouched");
    assert_eq!(core.current_mana(vex), 6, "full mana 4 paid by the caster");

    // The TARGET's sheet appends the active line while the buff lives.
    drive_all(&mut core, ora, &mut views, "st");

    // 70 upkeep firings (game ticks 3, 6, .., 210): expiry on the target.
    for _ in 0..210 {
        core.tick();
    }
    let events = core.drain_events();
    for (sid, transcript) in views.iter_mut() {
        transcript.push_str(&text_to(&events, *sid));
    }
    let target = core.player_snapshot(ora);
    assert!(
        target.active_spells.iter().all(|slot| slot.spell.is_none()),
        "target slot empty after expiry"
    );
    assert_eq!(core.defender_debug(ora).parry, target_parry, "dodge contribution gone");

    // The post-expiry sheet: the active line is gone from Oracle's st.
    core.input(ora, "st");
    let after_sheet = text_to(&core.drain_events(), ora);
    assert!(after_sheet.contains("Oracle"), "sheet rendered: {after_sheet:?}");
    assert!(
        !after_sheet.contains("You are blurred!"),
        "st line gone after expiry: {after_sheet:?}"
    );

    let [(_, caster_view), (_, target_view), (_, room_view)] = &views[..] else {
        unreachable!()
    };
    // Caster view (§8.13 table row 1): the success line; the DescMsg and
    // the wear-off belong to the target alone.
    assert_in_order(
        caster_view,
        &[
            ("bought", "You just bought scroll of blur for nothing.\n"),
            (
                "learn",
                "You read scroll of blur and learn the spell blur.\n\n",
            ),
            ("caster line", "You cast blur on Oracle!\n"),
        ],
    );
    assert!(!caster_view.contains("You are blurred!"), "caster: {caster_view:?}");
    assert!(!caster_view.contains("wear off"), "caster: {caster_view:?}");
    // Target view (row 2): success line, async active line, the st
    // repeat, then — 70 ticks later — the wear-off, all in order.
    assert_in_order(
        target_view,
        &[
            ("target line", "Vexil casts blur upon you!\n"),
            ("active line", "You are blurred!\n"),
            ("st line", "You are blurred!\n"),
            ("wear-off", "The effects of blur wear off.\n"),
        ],
    );
    // Room view (row 3): the third-person line only — no target-line
    // leak, no expiry line (§8.13: in-room Kaimon saw nothing).
    assert_in_order(room_view, &[("room line", "Vexil casts blur on Oracle!\n")]);
    assert!(!room_view.contains("upon you"), "room: {room_view:?}");
    assert!(!room_view.contains("You are blurred!"), "room: {room_view:?}");
    assert!(!room_view.contains("wear off"), "room: {room_view:?}");
}

/// Slice-5 golden scenario (MEASURED §8.13 for the refusal and fan-out;
/// fixture-shaped damage — no damaging area is learnable): the area
/// sweep. Empty room refuses pre-charge even with a player standing
/// there (players are NEVER area targets); with two monsters the cast
/// prints the caster line and ONE room line (no per-target lines, no
/// damage numbers, no engagement), damages each monster with the shared
/// roll, kills the frail one through the M3 death route, and leaves the
/// bystander untouched.
#[test]
fn area_cast_sweeps_monsters_and_spares_the_bystander() {
    let config = CoreConfig {
        rng_seed: SEED,
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let mut caster = mage();
    caster.location = LAIR; // unprotected: no guilt gate
    caster.spellbook.insert(SHOCKWAVE, false);
    let vex = core.attach_player(caster);
    let mut bystander = body("Oracle");
    bystander.location = LAIR;
    let ora = core.attach_player(bystander);
    core.drain_events();

    // No monsters (bystander present): the pre-charge no-effect refusal —
    // players never count as area targets, and nobody else hears it.
    core.input(vex, "c shoc");
    let events = core.drain_events();
    let refusal = text_to(&events, vex);
    assert!(
        refusal.contains("Your spell has no effect in this room!\n"),
        "got: {refusal:?}"
    );
    assert_eq!(core.current_mana(vex), 10, "pre-charge refusal");
    assert_eq!(text_to(&events, ora), "", "refusal is caster-only");

    // Two monsters: the 20 HP filthbug survives the sweep, the 5 HP wisp
    // dies through the M3 kill route.
    let bug = core.spawn_monster(FILTHBUG, LAIR).expect("fixture template");
    let wisp = core.spawn_monster(WISP, LAIR).expect("fixture template");
    core.drain_events();
    core.input(vex, "c shoc");
    let events = core.drain_events();
    let caster_view = text_to(&events, vex);
    let room_view = text_to(&events, ora);

    // Caster fan-out: the caster line, then the kill route — death line
    // and the full experience (the bystander is not engaged, no split).
    assert_in_order(
        &caster_view,
        &[
            ("caster line", "You cast shockwave on the room!\n"),
            ("death", "The sickly wisp is dead.\n"),
            ("exp", "You gain 9 experience.\n"),
        ],
    );
    assert!(!caster_view.contains("damage"), "no damage numbers: {caster_view:?}");
    assert!(!caster_view.contains("Combat"), "no engagement: {caster_view:?}");
    // Room fan-out: ONE generic frame line + the broadcast death line —
    // no per-target lines, no numbers, no experience.
    assert_in_order(
        &room_view,
        &[
            ("room line", "Vexil casts shockwave on the room!\n"),
            ("death", "The sickly wisp is dead.\n"),
        ],
    );
    assert!(!room_view.contains("on you"), "no target line: {room_view:?}");
    assert!(!room_view.contains("damage"), "no damage numbers: {room_view:?}");
    assert!(!room_view.contains("experience"), "no exp for the bystander: {room_view:?}");

    // Per-monster damage: one shared roll (12..12 -> genrdn 12..=13; 13
    // under SEED — match 12 does not split), Magic unresistable, so the
    // filthbug's mr 30 does not shield plain Damage.
    assert_eq!(core.monster_hp(bug), Some(7), "20 - the SEED roll 13");
    assert_eq!(core.monster_hp(wisp), None, "instance gone through the kill route");
    assert_eq!(core.current_mana(vex), 4, "full mana 6 charged");
    assert_eq!(core.player_snapshot(vex).experience, 9, "kill exp banked");

    // The bystander is untouched: HP, poison, slots, mana all pristine.
    let untouched = core.player_snapshot(ora);
    assert_eq!(untouched.current_hp, 10, "bystander HP untouched");
    assert_eq!(untouched.poison, 0, "bystander not poisoned");
    assert_eq!(untouched.experience, 0, "no exp share");
    assert!(
        untouched.active_spells.iter().all(|slot| slot.spell.is_none()),
        "no slot on the bystander"
    );
    assert_eq!(core.current_mana(ora), 10, "bystander mana untouched");
}

/// Slice-6 golden scenario (MEASURED §8.14 for the hit fan-out and the
/// poison lifecycle; the fizzle family is decompile-only — every §8.14
/// caster in reach carried a 100% form): Vexil engages the moaning
/// spirit (kind-2 form, 65%, `draws the breath` — instant Drain with
/// the record-8455 castmsgb), with Kaimon watching. Under SEED the 65%
/// band produces both outcomes: landed casts (victim line WITH the
/// damage number, room line WITHOUT — the exact inverse of §8.10's
/// melee pair) and the "attempted to cast ... but failed." fizzle pair.
/// Then the venom phase: the tentacled horror's duration cast enters
/// Vexil's slot (value 6, fixed duration — §6.5), hard-writes the
/// poison counter, the slow tick prints "You feel ill." and deals the
/// counter in HP, and the slot expiry reverses the counter to zero with
/// the DescMsg wear-off line (victim-private, like every expiry).
#[test]
fn caster_monster_fight_and_the_live_poison_lifecycle() {
    let config = CoreConfig {
        rng_seed: SEED,
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let vex = core.attach_player(mage());
    let kai = core.attach_player(body("Kaimon"));
    core.spawn_monster(SPIRIT, LAIR).expect("fixture template");
    core.spawn_monster(HORROR, PIT).expect("fixture template");
    // 200 HP soaks the whole scenario without regen interference (regen
    // only fires below the derived max).
    core.set_current_hp(vex, 200);
    core.drain_events();

    let mut views = vec![(vex, String::new()), (kai, String::new())];

    // --- Phase 1: the 65% caster (ticks 1..=15, rounds t5/t10/t15) ---
    drive_all(&mut core, vex, &mut views, "n");
    drive_all(&mut core, kai, &mut views, "n");
    drive_all(&mut core, vex, &mut views, "attack spirit");
    tick_all(&mut core, &mut views, 15);

    let (vex_p1, kai_p1) = (views[0].1.clone(), views[1].1.clone());
    let landed = vex_p1.matches("from your body for").count();
    let fizzled = vex_p1
        .matches("The moaning spirit attempted to cast draws the breath at you, but failed.")
        .count();
    assert_eq!(landed, 5, "SEED landed casts: {vex_p1:?}");
    assert_eq!(fizzled, 2, "SEED fizzles: {vex_p1:?}");
    // HP accounting: every landed drain shows the exact amount it dealt
    // (SEED rolls 13, 5, 11, 9, 11 — all inside the record band 4..13;
    // stream re-pinned for the slice-3 draws: retaliation-gate and
    // post-swing lock rolls now sit in every monster attack sequence.
    // Re-pinned again in slice 5: the ENGAGE-time lock roll moved off
    // the tail of the player swing loop and onto the ATTACK command,
    // where 26230 lives — one draw earlier in the stream, one fizzle
    // fewer in this window).
    assert_eq!(core.current_hp(vex), 200 - 49, "SEED drain total: {vex_p1:?}");
    // Victim view, in order: the engagement, a landed line (WITH the
    // damage number), and a fizzle line.
    assert_in_order(
        &vex_p1,
        &[
            ("engaged", "*Combat Engaged*"),
            (
                "landed cast",
                "Moaning spirit draws the breath from your body for 13 damage!\n",
            ),
            (
                "fizzle",
                "The moaning spirit attempted to cast draws the breath at you, but failed.\n",
            ),
        ],
    );
    // Room view: the landed room line has NO damage number (§8.14 — the
    // simultaneous victim/room pair), the fizzle pair names the victim.
    assert!(
        kai_p1.contains("Moaning spirit draws the breath from Vexil's body!\n"),
        "room hit line: {kai_p1:?}"
    );
    assert!(
        !kai_p1.contains("body for"),
        "no damage number in any room cast line: {kai_p1:?}"
    );
    assert!(
        kai_p1.contains(
            "The moaning spirit attempted to cast draws the breath at Vexil, but failed.\n"
        ),
        "room fizzle line: {kai_p1:?}"
    );
    // The victim's second-person lines stay private.
    assert!(!kai_p1.contains("your body"), "room: {kai_p1:?}");

    // --- Phase 2: the venom lifecycle (entry t20, slow tick t30, the
    // 12-tick duration expires on the t54 upkeep — one tick short of the
    // t55 round, where the live caster would legally re-poison) ---
    for (_, transcript) in views.iter_mut() {
        transcript.clear();
    }
    drive_all(&mut core, vex, &mut views, "s");
    drive_all(&mut core, vex, &mut views, "e");
    drive_all(&mut core, kai, &mut views, "s");
    drive_all(&mut core, kai, &mut views, "e");
    drive_all(&mut core, vex, &mut views, "attack horror");
    let hp_before_venom = core.current_hp(vex);
    tick_all(&mut core, &mut views, 5); // t20: the entry round

    // Slot entry (monster_add_cast_spell_to_user semantics): value 6,
    // the FIXED duration (no caster scaling), poison hard-written after
    // the successful entry.
    let p = core.player_snapshot(vex);
    let idx = p.find_active(SPIT_VENOM).expect("venom entered a slot");
    assert_eq!(p.active_spells[idx].value, 6);
    assert_eq!(p.active_spells[idx].remaining, 12, "fixed duration");
    assert_eq!(core.poison(vex), 6, "counter hard-written at entry");
    assert_eq!(core.current_hp(vex), hp_before_venom, "poison deals nothing at entry");

    // Through the t30 slow tick to the t54 expiry upkeep.
    tick_all(&mut core, &mut views, 34);
    assert_eq!(core.poison(vex), 0, "termination reversed the counter");
    let p = core.player_snapshot(vex);
    assert!(
        p.active_spells.iter().all(|slot| slot.spell.is_none()),
        "slot empty after expiry"
    );
    // Exactly one slow tick fired while poisoned: -6 HP, no more.
    assert_eq!(core.current_hp(vex), hp_before_venom - 6, "one poison tick");

    let [(_, vex_p2), (_, kai_p2)] = &views[..] else {
        unreachable!()
    };
    // Victim view, in order: the castmsgb entry line, the DescMsg active
    // line, the slow-tick poison line (the same "You feel ill." — §8.14
    // measured the tick line live), then the wear-off.
    assert_in_order(
        vex_p2,
        &[
            ("venom line", "The tentacled horror spits a stream of venom at you!\n"),
            ("entry active line", "You feel ill.\n"),
            ("slow-tick line", "You feel ill.\n"),
            ("wear-off", "The effects of the poison wear off!\n"),
        ],
    );
    assert_eq!(
        vex_p2.matches("You feel ill.").count(),
        2,
        "entry + exactly one slow tick: {vex_p2:?}"
    );
    // Repeat casts while the slot holds 6 abort silently (6 never
    // EXCEEDS 6): one venom line total.
    assert_eq!(
        vex_p2.matches("spits a stream of venom at you!").count(),
        1,
        "set-if-greater rejects the refresh silently: {vex_p2:?}"
    );
    // Room view: the third-person cast line only — the poison tick and
    // the wear-off are victim-private.
    assert!(
        kai_p2.contains("The tentacled horror spits a stream of venom at Vexil!\n"),
        "room venom line: {kai_p2:?}"
    );
    assert!(!kai_p2.contains("You feel ill."), "tick is private: {kai_p2:?}");
    assert!(!kai_p2.contains("wear off"), "expiry is private: {kai_p2:?}");
}
