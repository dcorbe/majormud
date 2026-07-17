//! Tests for the `cast` command: parsing (§8.9 bare/abbreviation behavior),
//! book-only spell resolution (exact shortname OR per-word name prefix,
//! remainder-as-target), the slice-3 cast gates (spec §3 order, oracle
//! strings from spellcasting.md §8.6/§8.9), the success roll + costs
//! (spec §3 steps 5-7: full costs on success, half mana on a failed roll),
//! and the Task-11 offensive path: targeting refusals, magnitude, resist,
//! saves, engagement, re-fire and the M3 kill route. Task 12 adds the
//! benign instant handlers (spec §4: Heal/EnergyLevel/hunger/thirst, plus
//! offensive Drain), the benign cast-message fan-out, and the
//! duration-spells-apply-nothing divergence that slice 4 closes.

use std::collections::BTreeMap;

use mud_core::ability::Ability;
use mud_core::content::{
    AttackForm, Class, ClassId, Content, Element, MatchType, Message, MessageId, Monster,
    MonsterId, Race, RaceId, Room, RoomId, SaveClass, ScalePair, Spell, SpellId, StatBlock,
    TargetMode,
};
use mud_core::command::{parse, Command};
use mud_core::game::{
    cast_roll_succeeds, damage_mr, monster_save_resists, spell_duration, spell_magnitude,
    ActiveSpell, Core, CoreConfig, Event, Gender, Player, SessionId,
};

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
/// Rollable (base_chance 15, mana 4, round cost 100): the failed-roll probe.
const JINX: SpellId = SpellId(60);
// --- Task 11 offensive fixtures (all base_chance 200 = deterministic hit) ---
/// Fire-element rolled damage 10..=11 (min=max=10, no increases): the
/// resist-scaling probe — 50% Rfir turns both roll outcomes into exactly 5.
const ZAP: SpellId = SpellId(70);
/// Magic-element twin of zap: unresistable (spec §4).
const PURE: SpellId = SpellId(80);
/// IfAntiMagic save class, mana 4: the saving-throw probe.
const HEXBOLT: SpellId = SpellId(90);
/// Fixed Damage(9) (non-zero ability value bypasses magnitude+resist),
/// round cost 1000: the kill/re-fire probe — one fire per round.
const DOOM: SpellId = SpellId(100);
/// Fixed Damage(9), mana 2, round cost 500: the mid-combat no-mana probe.
const SIPHON: SpellId = SpellId(110);
/// msgstyle-odd (the fireball/deathtouch family): refused pending the
/// slice-5 odd-style castmsgb arg table (slice-4 data check: no duration
/// starter is odd — the lowest odd learnable duration spell is L19).
const ODDBALL: SpellId = SpellId(120);
// --- Task 12 benign instant fixtures (base_chance 200 = deterministic) ---
/// Fixed Heal(25): the max-HP cap probe.
const MEND: SpellId = SpellId(130);
/// Heal(0) with bounds 10..10 — the rolled-magnitude probe (V is 10..=11).
const CURE: SpellId = SpellId(140);
/// EnergyLevel(300), round cost 500: exact round-pool delta.
const SURGE: SpellId = SpellId(150);
/// EnergyLevel(300), round cost 100: round-pool cap at the 1000 max.
const CHARGE: SpellId = SpellId(160);
/// Alterhunger(40) + AlterThirst(30): the +0xce/+0xd0 counter deltas.
const FEAST: SpellId = SpellId(170);
/// Offensive fixed Drain(9), round cost 1000: HP steal, caster cap, kill.
const LEECH: SpellId = SpellId(180);
/// Fixed Damage(-MR)(100), round cost 1000: the MR-scaling probe — the
/// ability the shipped attack spells actually carry.
const MRBOLT: SpellId = SpellId(190);
/// Fixed Damage(100) twin of mrbolt: the "1 ignores MR entirely" contrast.
const RAWBOLT: SpellId = SpellId(200);
// --- slice-4 Task 3 duration fixtures (base_chance 200 = deterministic) ---
/// The blur (129) model: duration 70 flat, magnitude bounds 5..5, abilities
/// [(Dodge, 0), (DescMsg, 903), (RemovesSpell, WARD)] — slot entry, the
/// cast-time active line, and the anti-stacking dispel.
const VEIL: SpellId = SpellId(210);
/// The amethyst-pendant-effect stand-in (blur's RemovesSpell target 157):
/// only ever pre-seeded into a slot, never cast.
const WARD: SpellId = SpellId(220);
/// KillSpell(WARD) twin of veil — the suppress-EndCast dispel.
const REAP: SpellId = SpellId(230);
/// INSTANT dispel (cure-poison model): duration 0, [(Heal, 5),
/// (RemovesSpell, WARD)] — the pre-pass runs for instant casts too, and a
/// successful dispel's early return skips the Heal.
const PURGE: SpellId = SpellId(240);
// --- slice-4 Task 6 termination fixtures ---
/// Stat-buff duration (the bard-song model, e.g. 45 song of brilliance):
/// (Intel, 0) — the 44-49 hard-write family flows through
/// effective_stats, reversal = recompute.
const ANTHEM: SpellId = SpellId(250);
/// Instant RemovesSpell(ANTHEM): the stat-reversal probe.
const UNSING: SpellId = SpellId(260);
/// EndCast chain head: (EndCast, ECHO), NO CastOnEnd row — pct defaults
/// to 100 (decompile 44830).
const CHIME: SpellId = SpellId(270);
/// The chained spell: a cost-free benign duration buff with its own
/// DescMsg — entered by the mode-2 forced cast.
const ECHO: SpellId = SpellId(280);
/// Instant RemovesSpell(CHIME): a dispel that HONORS the chain.
const UNBIND: SpellId = SpellId(290);
/// Instant KillSpell(CHIME): a dispel that SUPPRESSES the chain.
const SEVER: SpellId = SpellId(300);
/// (GiveTempSpell, ILLUMINATE) duration spell: termination purges the
/// temporary book entry (the slice-2 flag).
const GIFT: SpellId = SpellId(310);

const RAT: MonsterId = MonsterId(7);
const EMBER: MonsterId = MonsterId(8);
const WARDED: MonsterId = MonsterId(9);
const IMMUNE: MonsterId = MonsterId(10);
const FRAIL: MonsterId = MonsterId(11);
/// mr 150, no AntiMagic: the Damage(-MR) reduction cap probe (50%).
const SHELLED: MonsterId = MonsterId(12);

const TOWER: RoomId = RoomId { map: 1, room: 1 };
/// `attributes & 1` — the protected-room (guilt line) fixture.
const SHOP: RoomId = RoomId { map: 1, room: 2 };

/// A silent punching bag (no attack forms) worth 12 exp.
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
        match_type: MatchType::Single0,
        duration: 0,
        element: Element::Cold,
        class_gate_group: 1,
        mana_cost: 0,
        max_increase: ScalePair::NONE,
        required_class_level: 1,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0, // even = the render_cast_line contract
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
        // Bit 1 = protected (room+0x564; the Newhaven shops carry it) —
        // the guilt-line trigger for bare offensive casts.
        attributes: 1,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
    });
    content.add_monster(monster(RAT, "giant rat", 1000));
    let mut ember = monster(EMBER, "ember beast", 1000);
    // Rfir (5) 50: fire-element magnitude scales by (100-50)/100.
    ember.abilities = vec![(Ability::from_id(5).unwrap(), 50)];
    content.add_monster(ember);
    let mut warded = monster(WARDED, "warded golem", 1000);
    // AntiMagic (51) grants the IfAntiMagic save; mr 500 halves+caps to a
    // 98% resist roll (deterministic under the fixture rng seed).
    warded.abilities = vec![(Ability::AntiMagic, 1)];
    warded.magic_resist = 500;
    content.add_monster(warded);
    let mut immune = monster(IMMUNE, "immune wisp", 1000);
    // SpellImmu (139) 5: refuses spells below level 5 pre-cost.
    immune.abilities = vec![(Ability::SpellImmu, 5)];
    content.add_monster(immune);
    content.add_monster(monster(FRAIL, "frail bat", 5));
    let mut shelled = monster(SHELLED, "shelled horror", 1000);
    // mr 150, no AntiMagic: Damage(-MR) reduction clamp((150-50)/2,0,50)
    // = the 50% cap.
    shelled.magic_resist = 150;
    content.add_monster(shelled);
    // The mmis castmsgb shape (message 3242; line 3's damage is %s).
    content.add_message(Message {
        id: MessageId(900),
        lines: vec![
            "You fire a %s at %s for %d damage!".into(),
            "%s fires a %s at you for %d damage!".into(),
            "%s fires a %s at %s for %s damage!".into(),
        ],
    });
    // The blur castmsgb shape (message 7): a targeted benign template. The
    // target line exists in the record but a self-cast delivers it to no
    // one (oracle §8.6: `c blur` prints the caster line only).
    content.add_message(Message {
        id: MessageId(901),
        lines: vec![
            "You cast %s on %s!".into(),
            "%s casts %s upon you!".into(),
            "%s casts %s on %s!".into(),
        ],
    });
    // The illuminate castmsgb shape (message 2): target-less, consumes a
    // prefix of the arg order.
    content.add_message(Message {
        id: MessageId(902),
        lines: vec![
            "You cast %s!".into(),
            "%s casts %s!".into(),
            "%s casts %s!".into(),
        ],
    });
    // The DescMsg record (the blur msg-68 model): line1 = expiry line,
    // line2 = empty (room variant unmeasured), line3 = the active line
    // printed at cast and appended to `st` (§8.6/§8.11). One record,
    // three roles.
    content.add_message(Message {
        id: MessageId(903),
        lines: vec![
            "The effects of veil wear off.".into(),
            String::new(),
            "You are veiled!".into(),
        ],
    });
    // Blur's real DescMsg (msg 68) strings, MEASURED §8.9/§8.11.
    content.add_message(Message {
        id: MessageId(904),
        lines: vec![
            "The effects of blur wear off.".into(),
            String::new(),
            "You are blurred!".into(),
        ],
    });
    // Ward's DescMsg — the multi-buff `st` ordering probe.
    content.add_message(Message {
        id: MessageId(905),
        lines: vec![
            "The shimmering ward fades.".into(),
            String::new(),
            "A shimmering ward surrounds you!".into(),
        ],
    });
    // Anthem/chime/echo DescMsgs (wear-off line1 + active line3).
    content.add_message(Message {
        id: MessageId(906),
        lines: vec![
            "The anthem fades.".into(),
            String::new(),
            "An anthem lifts you!".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(907),
        lines: vec![
            "The chime fades.".into(),
            String::new(),
            "A chime rings around you!".into(),
        ],
    });
    content.add_message(Message {
        id: MessageId(908),
        lines: vec![
            "The echo fades.".into(),
            String::new(),
            "An echo follows you!".into(),
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

    let mut mmis = spell(MAGIC_MISSILE, "magic missile", "mmis");
    mmis.mana_cost = 1;
    mmis.base_chance = 15; // the real mmis base: rollable, floor(1/2)=0 mana
    let mut blur = spell(BLUR, "blur", "blur");
    blur.mana_cost = 4;
    blur.round_cost = 100;
    // The real blur (129) record: duration 70 flat, magnitude bounds 5..5,
    // Dodge value 0 = "store the rolled V", DescMsg 68 (fixture 904),
    // RemovesSpell -> the amethyst pendant's effect 157 (fixture WARD).
    blur.duration = 70;
    blur.min_base = 5;
    blur.max_base = 5;
    blur.cast_msg_b = Some(MessageId(901));
    blur.abilities = vec![
        (Ability::Dodge, 0),
        (Ability::DescMsg, 904),
        (Ability::RemovesSpell, 220),
    ];
    let mut illu = spell(ILLUMINATE, "illuminate", "illu");
    illu.mana_cost = 4;
    illu.required_power = 2;
    let mut spark = spell(SPARK, "spark", "spar");
    spark.cast_msg_b = Some(MessageId(902)); // target-less (msg 2 model)
    let mut heavy = spell(HEAVY, "heavy bolt", "hbol");
    heavy.round_cost = 2000;
    let mut jinx = spell(JINX, "jinx", "jinx");
    jinx.mana_cost = 4;
    jinx.round_cost = 100;
    jinx.base_chance = 15;
    // Task 11 offensive fixtures. Damage value 0 = "use the rolled
    // magnitude" (a non-zero value is a fixed, unresisted amount).
    let mut zap = spell(ZAP, "zap", "zapp");
    zap.target_mode = TargetMode::Offensive0;
    zap.element = Element::Fire;
    zap.abilities = vec![(Ability::Damage, 0)];
    zap.min_base = 10;
    zap.max_base = 10;
    zap.mana_cost = 1;
    zap.round_cost = 100;
    zap.cast_msg_b = Some(MessageId(900));
    let mut pure = zap.clone();
    pure.id = PURE;
    pure.name = "pure bolt".into();
    pure.short_name = "pure".into();
    pure.element = Element::Magic;
    let mut hexbolt = zap.clone();
    hexbolt.id = HEXBOLT;
    hexbolt.name = "hex bolt".into();
    hexbolt.short_name = "hexb".into();
    hexbolt.element = Element::Magic;
    hexbolt.save_class = SaveClass::IfAntiMagic;
    hexbolt.mana_cost = 4;
    let mut doom = spell(DOOM, "doom", "doom");
    doom.target_mode = TargetMode::Offensive0;
    doom.element = Element::Magic;
    doom.abilities = vec![(Ability::Damage, 9)];
    doom.mana_cost = 1;
    doom.round_cost = 1000;
    doom.cast_msg_b = Some(MessageId(900));
    let mut siphon = doom.clone();
    siphon.id = SIPHON;
    siphon.name = "siphon".into();
    siphon.short_name = "siph".into();
    siphon.mana_cost = 2;
    siphon.round_cost = 500;
    let mut oddball = zap.clone();
    oddball.id = ODDBALL;
    oddball.name = "oddball".into();
    oddball.short_name = "oddb".into();
    // msgstyle & 1 == 1: the castmsgb args bind in a different order with
    // no spell-name slot — must refuse until the slice-5 arg table.
    oddball.msg_style = 1;
    // Task 12 benign instant fixtures (spec §4 table, single-target).
    let mut mend = spell(MEND, "mend", "mend");
    mend.abilities = vec![(Ability::Heal, 25)]; // fixed value bypasses the roll
    let mut cure = spell(CURE, "cure", "cure");
    cure.abilities = vec![(Ability::Heal, 0)]; // 0 = the rolled magnitude
    cure.min_base = 10;
    cure.max_base = 10;
    let mut surge = spell(SURGE, "surge", "surg");
    surge.abilities = vec![(Ability::EnergyLevel, 300)];
    surge.round_cost = 500;
    let mut charge = spell(CHARGE, "charge", "chrg");
    charge.abilities = vec![(Ability::EnergyLevel, 300)];
    charge.round_cost = 100;
    let mut feast = spell(FEAST, "feast", "feas");
    feast.abilities = vec![(Ability::Alterhunger, 40), (Ability::AlterThirst, 30)];
    let mut leech = spell(LEECH, "leech", "leec");
    leech.target_mode = TargetMode::Offensive0;
    leech.element = Element::Magic;
    leech.abilities = vec![(Ability::Drain, 9)];
    leech.mana_cost = 1;
    leech.round_cost = 1000; // one fire per combat round
    leech.cast_msg_b = Some(MessageId(900));
    // Slice-4 MR fixtures: fixed values so the MR scale is the only
    // variable (a fixed value bypasses the magnitude roll and elemental
    // resist, but NOT the Damage(-MR) scale — decompile 43941-43983
    // applies it to local_2c whichever way that was selected).
    let mut mrbolt = spell(MRBOLT, "mrbolt", "mrbo");
    mrbolt.target_mode = TargetMode::Offensive0;
    mrbolt.element = Element::Magic;
    mrbolt.abilities = vec![(Ability::DamageMR, 100)];
    mrbolt.mana_cost = 1;
    mrbolt.round_cost = 1000;
    mrbolt.cast_msg_b = Some(MessageId(900));
    let mut rawbolt = mrbolt.clone();
    rawbolt.id = RAWBOLT;
    rawbolt.name = "rawbolt".into();
    rawbolt.short_name = "rawb".into();
    rawbolt.abilities = vec![(Ability::Damage, 100)];
    // Slice-4 duration fixtures. veil mirrors real blur (mana 4, round
    // cost 100, duration 70 flat, magnitude 5..5, Dodge value 0 = "store
    // the rolled V", RemovesSpell -> the pendant effect).
    let mut veil = spell(VEIL, "veil", "veil");
    veil.mana_cost = 4;
    veil.round_cost = 100;
    veil.duration = 70;
    veil.min_base = 5;
    veil.max_base = 5;
    veil.cast_msg_b = Some(MessageId(901));
    veil.abilities = vec![
        (Ability::Dodge, 0),
        (Ability::DescMsg, 903),
        (Ability::RemovesSpell, 220),
    ];
    let mut ward = spell(WARD, "ward", "ward");
    ward.duration = 70;
    // Dodge like the pendant effect it stands in for, plus a DescMsg of
    // its own — the dispel-recompute and multi-buff `st` probes.
    ward.abilities = vec![(Ability::Dodge, 0), (Ability::DescMsg, 905)];
    let mut reap = spell(REAP, "reap", "reap");
    reap.duration = 70;
    reap.abilities = vec![(Ability::KillSpell, 220)];
    // Instant dispel (cure-poison model): duration 0 stays the default.
    let mut purge = spell(PURGE, "purge", "purg");
    purge.abilities = vec![(Ability::Heal, 5), (Ability::RemovesSpell, 220)];
    // Task 6 termination fixtures (all cost-free, base_chance 200).
    let mut anthem = spell(ANTHEM, "anthem", "anth");
    anthem.duration = 70;
    anthem.min_base = 5;
    anthem.max_base = 5;
    anthem.abilities = vec![(Ability::Intel, 0), (Ability::DescMsg, 906)];
    let mut unsing = spell(UNSING, "unsing", "unsi");
    unsing.abilities = vec![(Ability::RemovesSpell, 250)];
    let mut chime = spell(CHIME, "chime", "chim");
    chime.duration = 70;
    chime.abilities = vec![(Ability::EndCast, 280), (Ability::DescMsg, 907)];
    let mut echo = spell(ECHO, "echo", "echo");
    echo.duration = 70;
    echo.min_base = 5;
    echo.max_base = 5;
    echo.abilities = vec![(Ability::Dodge, 0), (Ability::DescMsg, 908)];
    let mut unbind = spell(UNBIND, "unbind", "unbi");
    unbind.abilities = vec![(Ability::RemovesSpell, 270)];
    let mut sever = spell(SEVER, "sever", "seve");
    sever.abilities = vec![(Ability::KillSpell, 270)];
    let mut gift = spell(GIFT, "gift", "gift");
    gift.duration = 70;
    gift.abilities = vec![(Ability::GiveTempSpell, 10)]; // -> ILLUMINATE
    content.add_spell(anthem);
    content.add_spell(unsing);
    content.add_spell(chime);
    content.add_spell(echo);
    content.add_spell(unbind);
    content.add_spell(sever);
    content.add_spell(gift);
    content.add_spell(veil);
    content.add_spell(ward);
    content.add_spell(reap);
    content.add_spell(purge);
    content.add_spell(mmis);
    content.add_spell(blur);
    content.add_spell(illu);
    content.add_spell(spark);
    content.add_spell(heavy);
    content.add_spell(jinx);
    content.add_spell(zap);
    content.add_spell(pure);
    content.add_spell(hexbolt);
    content.add_spell(doom);
    content.add_spell(siphon);
    content.add_spell(oddball);
    content.add_spell(mend);
    content.add_spell(cure);
    content.add_spell(surge);
    content.add_spell(charge);
    content.add_spell(feast);
    content.add_spell(leech);
    content.add_spell(mrbolt);
    content.add_spell(rawbolt);
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
        active_spells: Default::default(),
    }
}

/// The standard fixture book: every fixture spell, including the
/// too-powerful illuminate (reachable in slice 4+ via temp spells).
fn full_book() -> BTreeMap<SpellId, bool> {
    let mut book = BTreeMap::new();
    for id in [
        MAGIC_MISSILE,
        BLUR,
        ILLUMINATE,
        SPARK,
        HEAVY,
        JINX,
        ZAP,
        PURE,
        HEXBOLT,
        DOOM,
        SIPHON,
        ODDBALL,
        MEND,
        CURE,
        SURGE,
        CHARGE,
        FEAST,
        LEECH,
        MRBOLT,
        RAWBOLT,
        VEIL,
        WARD,
        REAP,
        PURGE,
        UNSING,
        UNBIND,
        SEVER,
    ] {
        book.insert(id, false);
    }
    book
}

/// A player with a meaningful max HP for the heal/drain cap probes: base
/// health 50 gives the L1 mage fixture max_hp = 50/2 + 2 = 27 (the default
/// zeroed StatBlock derives a nonsensical negative max).
fn hardy(name: &str, class: ClassId) -> Player {
    let mut p = player(name, class, full_book());
    p.stats.health = 50;
    p.base_stats.health = 50;
    p
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
fn refused_casts_deduct_nothing() {
    // A cast stopped by a gate never reaches the roll: no mana, no energy.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.set_current_mana(s, 3); // blur costs 4 -> mana-gate refusal
    core.drain_events();
    let energy = core.round_energy(s);
    cast(&mut core, s, "c blur");
    assert_eq!(core.current_mana(s), 3, "refusal deducts no mana");
    assert_eq!(core.round_energy(s), energy, "refusal deducts no energy");
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

// --- success roll + costs (Task 9; spec §3 steps 5-7) ---

/// Scripted roll source (the tests/combat.rs injection pattern): pops from
/// the front; panics if exhausted or out of the requested range.
fn rolls(values: &[i32]) -> impl FnMut(i32, i32) -> i32 + '_ {
    let mut it = values.iter().copied();
    move |lo, hi| {
        let v = it.next().expect("script exhausted");
        assert!(v >= lo && v <= hi, "scripted roll {v} outside [{lo},{hi}]");
        v
    }
}

#[test]
fn roll_below_chance_succeeds_at_chance_fails() {
    // chance = min(SC + base, 98) = 20 + 15 = 35; success is roll < chance.
    assert!(cast_roll_succeeds(20, 15, &mut rolls(&[34])));
    assert!(!cast_roll_succeeds(20, 15, &mut rolls(&[35])));
}

#[test]
fn chance_caps_at_98_so_top_rolls_still_fail() {
    // SC 90 + base 60 = 150 -> capped at 98: 97 succeeds, 98/99 fail.
    assert!(cast_roll_succeeds(90, 60, &mut rolls(&[97])));
    assert!(!cast_roll_succeeds(90, 60, &mut rolls(&[98])));
    assert!(!cast_roll_succeeds(90, 60, &mut rolls(&[99])));
}

#[test]
fn base_chance_200_skips_the_roll_entirely() {
    // Spec §3 step 5: >= 200 auto-succeeds; the roll source must never fire.
    let mut no_roll = |_: i32, _: i32| -> i32 { panic!("auto-succeed rolled") };
    assert!(cast_roll_succeeds(-500, 200, &mut no_roll));
}

#[test]
fn successful_cast_deducts_full_mana_and_round_energy() {
    // blur: mana 4, round cost 100, base_chance 200 -> deterministic
    // success. Spec §3 step 7: full costs (a duration spell still pays in
    // full; only its effects wait for slice 4).
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    let energy = core.round_energy(s);
    cast(&mut core, s, "c blur");
    assert_eq!(core.current_mana(s), 2, "full mana cost 4 deducted");
    assert_eq!(core.round_energy(s), energy - 100, "full round cost");
}

// The failed-roll tests cast through a non-caster (Grunt): caster_group 0
// makes the SC stat term -150, so chance = min(SC + 15, 98) is negative and
// a rollable spell can NEVER succeed — seed-proof determinism, same trick as
// the restock boundary rows. (cast_command has no wrong-class gate by
// design: book membership IS the class gate; Grunt's book is a fixture.)

#[test]
fn failed_roll_prints_fail_line_half_mana_full_energy() {
    // Spec §3 step 6: jinx mana 4 -> 2 deducted (§8.9: blur 4 -> 2), full
    // round cost 100, and the round is spent (MEASURED §8.6).
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Grunt", WARRIOR, full_book()));
    core.drain_events();
    let energy = core.round_energy(s);
    let shown = cast(&mut core, s, "c jinx");
    assert!(
        shown.contains("You attempt to cast jinx, but fail.\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 4, "half of mana 4 deducted");
    assert_eq!(core.round_energy(s), energy - 100, "full round cost");
    let next = cast(&mut core, s, "c spark");
    assert!(next.contains(ALREADY_CAST), "failure spends the round: {next:?}");
}

#[test]
fn failed_roll_half_mana_floors_to_zero() {
    // mmis mana 1 -> floor(1/2) = 0 deducted (oracle-confirmed §8.6: the
    // prompt mana was unchanged across a failed mmis).
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Grunt", WARRIOR, full_book()));
    core.drain_events();
    let shown = cast(&mut core, s, "c mmis");
    assert!(
        shown.contains("You attempt to cast magic missile, but fail.\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 6, "floor(1/2) = 0 mana deducted");
}

#[test]
fn failed_roll_broadcasts_the_room_line_to_others_only() {
    // DLL string 00485be0: "%s attempted to cast %s, but failed."
    // (ORACLE-VERIFY: single-session captures cannot show the observer side.)
    let mut core = Core::new(world(), CoreConfig::default());
    let caster = core.attach_player(player("Grunt", WARRIOR, full_book()));
    let watcher = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    core.input(caster, "c jinx");
    let events = core.drain_events();
    let seen = text_to(&events, watcher);
    assert!(
        seen.contains("Grunt attempted to cast jinx, but failed.\n"),
        "got: {seen:?}"
    );
    let own = text_to(&events, caster);
    assert!(!own.contains("attempted"), "caster-only line: {own:?}");
}

// --- offensive casts (Task 11; spellcasting.md §8.6/§8.9 + decompile) ---

/// Fixture core with a mage in the Tower and a spawned monster.
fn arena(template: MonsterId) -> (Core, SessionId, mud_core::game::MonsterInstanceId) {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    let m = core.spawn_monster(template, TOWER).expect("fixture template");
    core.drain_events();
    (core, s, m)
}

#[test]
fn bare_offensive_cast_in_empty_room_must_specify() {
    // MEASURED (§8.9): mana unchanged — target resolution runs BEFORE the
    // cost gates and the roll.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    let shown = cast(&mut core, s, "c zap");
    assert!(
        shown.contains("You must specify a target for that spell!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 6, "mana unchanged (§8.9)");
}

#[test]
fn bare_offensive_cast_never_auto_picks_a_live_monster() {
    // MEASURED (§8.9): unlike attack, even a room with a live monster
    // refuses the bare cast.
    let (mut core, s, m) = arena(RAT);
    let shown = cast(&mut core, s, "c zap");
    assert!(
        shown.contains("You must specify a target for that spell!\n"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("*Combat Engaged*"), "got: {shown:?}");
    assert_eq!(core.monster_hp(m), Some(1000), "no damage dealt");
    assert_eq!(core.current_mana(s), 6, "mana unchanged (§8.9)");
}

#[test]
fn bare_offensive_cast_in_protected_room_prints_guilt() {
    // MEASURED (§8.6/§8.9): the guilt line keys on the ROOM's protected
    // flag (attributes & 1; decompile cast_no_target 39168-39184), not on
    // who is standing in it. Mana unchanged; the DLL charges the round
    // cost when affordable (39185-39195).
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.location = SHOP;
    let s = core.attach_player(vexil);
    core.drain_events();
    let energy = core.round_energy(s);
    let shown = cast(&mut core, s, "c zap");
    assert!(
        shown.contains(
            "You are overcome with a feeling of guilt and break off your attack.\n"
        ),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 6, "mana unchanged (§8.9)");
    assert_eq!(core.round_energy(s), energy - 100, "round cost charged (DLL)");
}

#[test]
fn targeted_offensive_cast_in_protected_room_prints_guilt() {
    // The protected-room gate covers the TARGETED path too (decompile
    // cast_monster_target 43232, guilt refusal 44290-44297 — the same
    // room+0x564 & 1 flag as the bare-cast gate): guilt line, no
    // engagement, mana unchanged, round cost charged when affordable
    // (mirrors the bare-cast guilt charging).
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.location = SHOP;
    let s = core.attach_player(vexil);
    let m = core.spawn_monster(RAT, SHOP).expect("fixture template");
    core.drain_events();
    let energy = core.round_energy(s);
    let shown = cast(&mut core, s, "c zap rat");
    assert!(
        shown.contains(
            "You are overcome with a feeling of guilt and break off your attack.\n"
        ),
        "got: {shown:?}"
    );
    assert!(!shown.contains("*Combat Engaged*"), "no engagement: {shown:?}");
    assert_eq!(core.current_mana(s), 6, "mana unchanged");
    assert_eq!(core.round_energy(s), energy - 100, "round cost charged (DLL)");
    assert_eq!(core.monster_hp(m), Some(1000), "no damage dealt");
    // Nothing was armed: the next combat round fires nothing.
    let round = fire_round(&mut core, s);
    assert!(!round.contains("You fire"), "no armed cast: {round:?}");
}

#[test]
fn msgstyle_odd_spell_refuses_before_costs_and_engagement() {
    // TEMPORARY until the slice-5 odd-style arg table: msgstyle-odd
    // spells (~441 shipped, incl. fireball 120 / deathtouch 58) bind
    // castmsgb args in a different order with no spell-name slot;
    // render_cast_line would silently mis-bind them, so the cast refuses
    // loudly before any cost or engagement.
    let (mut core, s, m) = arena(RAT);
    let energy = core.round_energy(s);
    let shown = cast(&mut core, s, "c oddb rat");
    assert!(
        shown.contains("You cannot cast that yet.\n"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("*Combat Engaged*"), "no engagement: {shown:?}");
    assert_eq!(core.current_mana(s), 6, "no mana cost");
    assert_eq!(core.round_energy(s), energy, "no round cost");
    assert_eq!(core.monster_hp(m), Some(1000), "no damage dealt");
    // Nothing was armed: the next combat round fires nothing.
    let round = fire_round(&mut core, s);
    assert!(!round.contains("You fire"), "no armed cast: {round:?}");
}

#[test]
fn engaged_bare_offensive_cast_breaks_off_then_refuses() {
    // MEASURED (§8.9 run 2): *Combat Off* precedes the must-specify line;
    // the second bare cast finds no engagement left to break.
    let (mut core, s, _m) = arena(RAT);
    let attack = cast(&mut core, s, "attack rat");
    assert!(attack.contains("*Combat Engaged*"), "got: {attack:?}");
    let shown = cast(&mut core, s, "c zap");
    let off = shown.find("*Combat Off*").expect("breaks engagement");
    let refuse = shown
        .find("You must specify a target for that spell!")
        .expect("then refuses");
    assert!(off < refuse, "order (§8.9 run 2): {shown:?}");
    let again = cast(&mut core, s, "c zap");
    assert!(!again.contains("*Combat Off*"), "already broken: {again:?}");
    assert!(again.contains("You must specify a target"), "got: {again:?}");
}

#[test]
fn unmatched_explicit_target_is_not_seen_here() {
    // MEASURED (§8.9): the whole remainder echoes verbatim; no mana moves.
    let (mut core, s, _m) = arena(RAT);
    let shown = cast(&mut core, s, "c zap purple dragon");
    assert!(
        shown.contains("You do not see purple dragon here!\n"),
        "got: {shown:?}"
    );
    assert_eq!(core.current_mana(s), 6, "mana unchanged");
}

#[test]
fn magnitude_spans_lo_to_hi_plus_one() {
    // Decompile 43668-43704: V = genrdn(0, hi-lo+1) + lo with genrdn
    // inclusive of both ends — the mmis-like fixture (bounds 4..12,
    // max_increase 1/0 = the zero-denominator guard) spans 4..=13,
    // matching the oracle's observed 13 (§8.6).
    let mut mmis_like = spell(SpellId(1), "probe", "prob");
    mmis_like.min_base = 4;
    mmis_like.max_base = 12;
    mmis_like.max_increase = ScalePair { per: 1, levels: 0 };
    assert_eq!(spell_magnitude(&mmis_like, 1, 0, &mut rolls(&[0])), 4);
    assert_eq!(spell_magnitude(&mmis_like, 1, 0, &mut rolls(&[9])), 13);
}

#[test]
fn level_cap_clamps_scaling_and_zero_means_uncapped() {
    // Decompile 41783-41789: cap < 1 OR level <= cap => use the level;
    // otherwise the cap. So cap 0 (and negative) = uncapped.
    let mut probe = spell(SpellId(1), "probe", "prob");
    probe.max_base = 10;
    probe.max_increase = ScalePair { per: 2, levels: 1 }; // +2/level
    probe.level_cap = 6;
    // L = min(20, 6) = 6: hi = 10 + 12 = 22, lo = 0 -> roll(0, 23).
    assert_eq!(spell_magnitude(&probe, 20, 0, &mut rolls(&[22])), 22);
    probe.level_cap = 0;
    // Uncapped: hi = 10 + 40 = 50.
    assert_eq!(spell_magnitude(&probe, 20, 0, &mut rolls(&[50])), 50);
    probe.level_cap = -1;
    assert_eq!(spell_magnitude(&probe, 20, 0, &mut rolls(&[50])), 50);
}

#[test]
fn inverted_bounds_clamp_lo_to_hi_without_swapping() {
    // Decompile: lo = min(lo, hi); hi keeps its value — inverted data
    // rolls in hi ..= hi+1, it does not swap.
    let mut probe = spell(SpellId(1), "probe", "prob");
    probe.min_base = 10;
    probe.max_base = 5;
    assert_eq!(spell_magnitude(&probe, 1, 0, &mut rolls(&[0])), 5);
    assert_eq!(spell_magnitude(&probe, 1, 0, &mut rolls(&[1])), 6);
}

#[test]
fn resist_scales_the_rolled_magnitude() {
    // Decompile 43705: V = (100 - resist) * V / 100, integer division.
    let mut probe = spell(SpellId(1), "probe", "prob");
    probe.min_base = 10;
    probe.max_base = 10;
    assert_eq!(spell_magnitude(&probe, 1, 50, &mut rolls(&[0])), 5);
    assert_eq!(spell_magnitude(&probe, 1, 50, &mut rolls(&[1])), 5); // 11*50/100
    assert_eq!(spell_magnitude(&probe, 1, 100, &mut rolls(&[0])), 0);
    assert_eq!(spell_magnitude(&probe, 1, 0, &mut rolls(&[1])), 11);
}

/// One combat round (the Job::Energy cadence), returning the caster's text.
fn fire_round(core: &mut Core, s: SessionId) -> String {
    for _ in 0..5 {
        core.tick();
    }
    text_to(&core.drain_events(), s)
}

#[test]
fn manual_offensive_cast_engages_without_firing() {
    // MEASURED (oracle_spell_cast.raw 567-573): the command prints
    // *Combat Engaged* and nothing else — mana unchanged at the prompt,
    // energy zeroed; the first fire arrives with the next combat round.
    let (mut core, s, m) = arena(RAT);
    let shown = cast(&mut core, s, "c doom rat");
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
    assert!(!shown.contains("You fire"), "no immediate fire: {shown:?}");
    assert_eq!(core.current_mana(s), 6, "no mana at engagement");
    assert_eq!(core.round_energy(s), 0, "engagement zeroes the pool (DLL 43468)");
    assert_eq!(core.monster_hp(m), Some(1000));
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a doom at giant rat for 9 damage!\n"),
        "driver fires: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(991));
    assert_eq!(core.current_mana(s), 5, "mana charged at the fire");
}

#[test]
fn elemental_resist_reduces_live_damage() {
    // zap is Fire; the ember beast carries Rfir 50. Both roll outcomes
    // (10, 11) scale to exactly 5 — deterministic without touching rng.
    let (mut core, s, m) = arena(EMBER);
    let shown = cast(&mut core, s, "c zapp ember");
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a zap at ember beast for 5 damage!\n"),
        "got: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(995));
}

#[test]
fn magic_element_bypasses_resistance() {
    // pure bolt is Element::Magic — no resist ability exists for it
    // (spec §4), so the ember beast's Rfir is ignored: full 10..=11.
    let (mut core, s, m) = arena(EMBER);
    cast(&mut core, s, "c pure ember");
    fire_round(&mut core, s);
    let hp = core.monster_hp(m).unwrap();
    assert!(
        (989..=990).contains(&hp),
        "full magnitude 10..=11 applied: {hp}"
    );
}

#[test]
fn save_roll_is_half_stat_capped_98() {
    // Decompile 43594-43614: genrdn(1,100) <= min(stat/2, 98) resists.
    assert!(monster_save_resists(20, &mut rolls(&[10])));
    assert!(!monster_save_resists(20, &mut rolls(&[11])));
    assert!(monster_save_resists(400, &mut rolls(&[98]))); // capped at 98
    assert!(!monster_save_resists(400, &mut rolls(&[99])));
    assert!(!monster_save_resists(1, &mut rolls(&[1]))); // floor stat never saves
}

#[test]
fn if_antimagic_save_needs_the_monster_ability() {
    // hex bolt is IfAntiMagic; the rat has no AntiMagic — no save is
    // rolled and the damage lands with the full mana charge.
    let (mut core, s, m) = arena(RAT);
    cast(&mut core, s, "c hexb rat");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a hex bolt at giant rat for 1"),
        "10 or 11 damage: {round:?}"
    );
    assert!(core.monster_hp(m).unwrap() < 1000, "damage applied");
    assert_eq!(core.current_mana(s), 2, "full mana 4 on success");
}

#[test]
fn antimagic_monster_resists_and_caster_pays_half_mana() {
    // The warded golem: AntiMagic + save stat 500 -> a 98%-capped resist
    // roll. The fixture seed rolls under 98 (deterministic); the resisted
    // cast pays like a failed roll — full round cost, half mana — and
    // prints the DLL resist pair (ORACLE-VERIFY: SaveClass::None on all
    // starter spells makes this unreachable live).
    let (mut core, s, m) = arena(WARDED);
    let shown = cast(&mut core, s, "c hexb golem");
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains(
            "You attempt to cast hex bolt at warded golem, but the spell is resisted.\n"
        ),
        "got: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(1000), "no damage on resist");
    assert_eq!(core.current_mana(s), 4, "half of mana 4 charged");
}

#[test]
fn damage_mr_formula_matches_the_decompile() {
    // Decompile cast_monster_target 43941-43983. Without AntiMagic (51):
    // reduction% = clamp((MR-50)/2, 0, 50); a zero reduction instead
    // AMPLIFIES by (50-MR)% — low-MR targets take up to +49%.
    assert_eq!(damage_mr(100, 1, false), 149); // the engine's MR floor
    assert_eq!(damage_mr(100, 30, false), 120); // the giant rat / filthbug
    assert_eq!(damage_mr(10, 30, false), 12); // 10*20/100 = 2
    assert_eq!(damage_mr(4, 30, false), 4); // 4*20/100 truncates to 0
    assert_eq!(damage_mr(100, 50, false), 100); // the pivot: unchanged
    assert_eq!(damage_mr(100, 51, false), 99); // (51-50)/2=0 -> -1% amp
    assert_eq!(damage_mr(50, 51, false), 50); // 50*-1/100 truncates to 0
    assert_eq!(damage_mr(100, 52, false), 99); // reduction 1%
    assert_eq!(damage_mr(100, 100, false), 75); // reduction 25%
    assert_eq!(damage_mr(100, 150, false), 50); // reduction hits the cap
    assert_eq!(damage_mr(100, 500, false), 50); // capped at 50%
    // With AntiMagic: reduction% = clamp(MR/2, 0, 75), never amplifies.
    assert_eq!(damage_mr(100, 1, true), 100); // 1/2=0 -> unchanged, no amp
    assert_eq!(damage_mr(100, 30, true), 85);
    assert_eq!(damage_mr(100, 100, true), 50);
    assert_eq!(damage_mr(100, 150, true), 25); // reduction hits the cap
    assert_eq!(damage_mr(100, 500, true), 25); // capped at 75%
}

#[test]
fn damage_mr_scales_by_the_targets_mr() {
    // mrbolt: fixed Damage(-MR)(100). The shelled horror's mr 150 hits the
    // 50% reduction cap: exactly 50 lands, in the message and on the HP.
    let (mut core, s, m) = arena(SHELLED);
    cast(&mut core, s, "c mrbo horror");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a mrbolt at shelled horror for 50 damage!\n"),
        "got: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(950));
}

#[test]
fn damage_mr_amplifies_against_a_low_mr_monster() {
    // The rat template's mr 0 floors to the engine's 1 (decompile
    // 43387-43392), so the zero reduction AMPLIFIES: 100 + 100*49/100.
    let (mut core, s, m) = arena(RAT);
    cast(&mut core, s, "c mrbo rat");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a mrbolt at giant rat for 149 damage!\n"),
        "got: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(851));
}

#[test]
fn damage_mr_differs_from_plain_damage_exactly_by_the_mr_scale() {
    // Same monster, same fixed 100: Damage (1) ignores MR entirely (the
    // ability-table contract), Damage(-MR) (17) halves against mr 150.
    let (mut core, s, m) = arena(SHELLED);
    cast(&mut core, s, "c rawb horror");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a rawbolt at shelled horror for 100 damage!\n"),
        "got: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(900));
}

#[test]
fn damage_mr_antimagic_branch_caps_reduction_at_75() {
    // The warded golem: AntiMagic + mr 500 -> clamp(250, 0, 75) = 75%.
    // mrbolt is SaveClass::None, so no save intervenes: 25 damage.
    let (mut core, s, m) = arena(WARDED);
    cast(&mut core, s, "c mrbo golem");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a mrbolt at warded golem for 25 damage!\n"),
        "got: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(975));
}

#[test]
fn spellimmu_monster_refuses_before_any_cost() {
    // Decompile 43630-43638: spell level below the monster's SpellImmu
    // (139) value => "no effect", before costs or engagement.
    let (mut core, s, m) = arena(IMMUNE);
    let energy = core.round_energy(s);
    let shown = cast(&mut core, s, "c zapp wisp");
    assert!(
        shown.contains("Your spell has no effect on immune wisp.\n"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("*Combat Engaged*"), "no engagement: {shown:?}");
    assert_eq!(core.monster_hp(m), Some(1000));
    assert_eq!(core.current_mana(s), 6, "no mana charged");
    assert_eq!(core.round_energy(s), energy, "no round cost charged");
    // And the round is not consumed.
    let next = cast(&mut core, s, "c spark");
    assert!(!next.contains(ALREADY_CAST), "free refusal: {next:?}");
}

#[test]
fn kill_routes_through_death_exp_and_combat_off() {
    // §8.6 kill epilogue: death line, "You gain N experience.",
    // *Combat Off* — the M3 monster-death path, fired by a cast.
    let (mut core, s, m) = arena(FRAIL);
    let shown = cast(&mut core, s, "c doom bat");
    assert!(shown.contains("*Combat Engaged*"), "got: {shown:?}");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a doom at frail bat for 9 damage!\n"),
        "fixed Damage(9) bypasses magnitude: {round:?}"
    );
    let death = round.find("The frail bat is dead.").expect("death line");
    let exp = round.find("You gain 12 experience.").expect("exp line");
    let off = round.find("*Combat Off*").expect("combat off");
    assert!(death < exp && exp < off, "order: {round:?}");
    assert_eq!(core.monster_hp(m), None, "instance gone");
    assert_eq!(core.player_snapshot(s).experience, 12);
}

#[test]
fn engaged_cast_refires_each_round_with_fresh_costs() {
    // MEASURED (§8.9): an engaged offensive cast re-fires unprompted each
    // combat round, charging mana again. doom costs the full round pool,
    // so exactly one fire per round.
    let (mut core, s, m) = arena(RAT);
    cast(&mut core, s, "c doom rat");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a doom at giant rat for 9 damage!\n"),
        "got: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(991));
    assert_eq!(core.current_mana(s), 5);
    // Next combat round: the same spell fires again with no input.
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a doom at giant rat for 9 damage!\n"),
        "unprompted re-fire: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(982));
    assert_eq!(core.current_mana(s), 4, "mana charged per round");
}

#[test]
fn out_of_mana_refire_skips_silently_and_stays_engaged() {
    // Decompile cast_monster_target 43560-43575 (autocombat branch): a
    // mana-short re-fire pays the round cost, casts nothing, says nothing,
    // and keeps the engagement — casting resumes when mana returns.
    // ORACLE-VERIFY: unmeasured live.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.current_mana = 3; // siphon costs 2: one cast, then short
    let s = core.attach_player(vexil);
    let m = core.spawn_monster(RAT, TOWER).unwrap();
    core.drain_events();
    cast(&mut core, s, "c siph rat");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a siphon at giant rat for 9 damage!\n"),
        "got: {round:?}"
    );
    assert_eq!(core.current_mana(s), 1, "3 - 2");
    // Next round: mana 1 < 2 -> silent skip, engagement holds.
    let round = fire_round(&mut core, s);
    assert!(!round.contains("You fire"), "no cast: {round:?}");
    assert!(!round.contains("*Combat Off*"), "still engaged: {round:?}");
    assert!(!round.contains("mana"), "silent: {round:?}");
    assert_eq!(core.monster_hp(m), Some(991), "no damage in the dry round");
    // Mana returns -> the SAME engagement fires again unprompted.
    core.set_current_mana(s, 4);
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a siphon at giant rat for 9 damage!\n"),
        "casting resumed: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(982));
}

#[test]
fn recasting_mid_combat_toggles_off_then_engaged() {
    // MEASURED (oracle_spell_cast.raw 592-606): re-casting mid-combat
    // toggles *Combat Off* / *Combat Engaged* — and is NOT blocked by the
    // one-cast-per-round gate, because the manual offensive cast only
    // re-engages (the driver does the casting).
    let (mut core, s, _m) = arena(RAT);
    let first = cast(&mut core, s, "c zapp rat");
    assert!(first.contains("*Combat Engaged*"), "got: {first:?}");
    assert!(!first.contains("*Combat Off*"), "got: {first:?}");
    fire_round(&mut core, s); // a fire happens in between
    let second = cast(&mut core, s, "c zapp rat");
    let off = second.find("*Combat Off*").expect("toggles off");
    let on = second.find("*Combat Engaged*").expect("then engages");
    assert!(off < on, "order: {second:?}");
}

#[test]
fn room_sees_the_cast_line_with_string_damage() {
    // Message 900 line 3 renders damage through %s (message 3242's shape).
    let (mut core, s, _m) = arena(RAT);
    let watcher = core.attach_player(player("Grunt", WARRIOR, BTreeMap::new()));
    core.drain_events();
    core.input(s, "c doom rat");
    for _ in 0..5 {
        core.tick();
    }
    let seen = text_to(&core.drain_events(), watcher);
    assert!(
        seen.contains("Vexil fires a doom at giant rat for 9 damage!\n"),
        "got: {seen:?}"
    );
}

// --- benign instant effects (Task 12; spec §4 instant table) ---

#[test]
fn heal_caps_at_max_hp() {
    // Heal (18): HP += V, capped at the derived max (spec §4).
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(hardy("Vexil", MAGE));
    let max = core.max_hp(s);
    core.set_current_hp(s, max - 5);
    core.drain_events();
    cast(&mut core, s, "c mend"); // fixed Heal(25) > the missing 5
    assert_eq!(core.current_hp(s), max, "healing never exceeds max HP");
}

#[test]
fn heal_value_zero_uses_the_rolled_magnitude() {
    // Ability value 0 = the rolled V (the offensive convention, decompile
    // 43711-43717); cure's bounds 10..10 roll V in 10..=11.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(hardy("Vexil", MAGE));
    core.set_current_hp(s, 5);
    core.drain_events();
    cast(&mut core, s, "c cure");
    let hp = core.current_hp(s);
    assert!((15..=16).contains(&hp), "5 + rolled 10..=11: {hp}");
}

#[test]
fn energy_level_adds_to_the_round_pool() {
    // EnergyLevel (11): round pool += V — costs deduct first, then the
    // effect lands: 1000 - 500 + 300 = 800.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    cast(&mut core, s, "c surge");
    assert_eq!(core.round_energy(s), 800, "1000 - round 500 + V 300");
}

#[test]
fn energy_level_caps_at_the_pool_max() {
    // 1000 - 100 + 300 = 1200 -> capped at the 1000 pool max.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    cast(&mut core, s, "c charge");
    assert_eq!(core.round_energy(s), 1000, "pool capped at max");
}

#[test]
fn hunger_and_thirst_deltas_apply() {
    // Alterhunger (15) / AlterThirst (16): the +0xce/+0xd0 counters.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    cast(&mut core, s, "c feast");
    let p = core.player_snapshot(s);
    assert_eq!(p.hunger, 1040, "1000 + 40");
    assert_eq!(p.thirst, 1030, "1000 + 30");
}

#[test]
fn drain_steals_hp_and_caps_the_caster_at_max() {
    // Drain (8), offensive side: target HP -= V, caster HP += V capped at
    // the caster's max (spec §4).
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(hardy("Vexil", MAGE));
    let m = core.spawn_monster(RAT, TOWER).expect("fixture template");
    let max = core.max_hp(s);
    core.set_current_hp(s, 10);
    core.drain_events();
    cast(&mut core, s, "c leech rat");
    let round = fire_round(&mut core, s);
    assert!(
        round.contains("You fire a leech at giant rat for 9 damage!\n"),
        "got: {round:?}"
    );
    assert_eq!(core.monster_hp(m), Some(991), "target loses the drain");
    assert_eq!(core.current_hp(s), 19, "caster gains the drain");
    // A drain that would overshoot max is capped there.
    core.set_current_hp(s, max - 5);
    let round = fire_round(&mut core, s);
    assert!(round.contains("You fire"), "re-fires: {round:?}");
    assert_eq!(core.current_hp(s), max, "caster heal capped at max HP");
}

#[test]
fn drain_kill_routes_through_the_death_path() {
    // A killing drain takes the M3 death route (death line, exp,
    // *Combat Off*) exactly like Damage.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(hardy("Vexil", MAGE));
    let m = core.spawn_monster(FRAIL, TOWER).expect("fixture template");
    core.set_current_hp(s, 10);
    core.drain_events();
    cast(&mut core, s, "c leech bat");
    let round = fire_round(&mut core, s);
    let death = round.find("The frail bat is dead.").expect("death line");
    let exp = round.find("You gain 12 experience.").expect("exp line");
    let off = round.find("*Combat Off*").expect("combat off");
    assert!(death < exp && exp < off, "order: {round:?}");
    assert_eq!(core.monster_hp(m), None, "instance gone");
    assert_eq!(core.current_hp(s), 19, "the kill still heals the caster");
}

#[test]
fn self_cast_message_reaches_caster_and_room_but_no_target_line() {
    // Oracle §8.6 (msg 7 model): "You cast blur on Vexil!" to the caster;
    // the room line to others in the SAME room; the target line ("%s casts
    // %s upon you!") to NO ONE on a self-cast; nothing to other rooms.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    let watcher = core.attach_player(player("Grunt", WARRIOR, BTreeMap::new()));
    let mut far = player("Distant", WARRIOR, BTreeMap::new());
    far.location = SHOP;
    let elsewhere = core.attach_player(far);
    core.drain_events();
    core.input(s, "c blur");
    let events = core.drain_events();
    let own = text_to(&events, s);
    assert!(own.contains("You cast blur on Vexil!\n"), "got: {own:?}");
    let seen = text_to(&events, watcher);
    assert!(seen.contains("Vexil casts blur on Vexil!\n"), "got: {seen:?}");
    assert!(!seen.contains("upon you"), "no target line to anyone: {seen:?}");
    assert!(!own.contains("upon you"), "no target line to anyone: {own:?}");
    let far_sees = text_to(&events, elsewhere);
    assert!(!far_sees.contains("blur"), "other rooms hear nothing: {far_sees:?}");
}

#[test]
fn targetless_benign_cast_prints_caster_and_room_lines() {
    // Oracle §8.6 (msg 2 model): "You cast illuminate!" — target-less
    // templates consume a prefix of the arg order.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    let watcher = core.attach_player(player("Grunt", WARRIOR, BTreeMap::new()));
    core.drain_events();
    core.input(s, "c spark");
    let events = core.drain_events();
    let own = text_to(&events, s);
    assert!(own.contains("You cast spark!\n"), "got: {own:?}");
    let seen = text_to(&events, watcher);
    assert!(seen.contains("Vexil casts spark!\n"), "got: {seen:?}");
}

#[test]
fn benign_cast_with_target_string_is_refused() {
    // MEASURED (§8.9): "cast blur extra trailing words" -> "You do not see
    // extra trailing words here!" — a non-empty target string on a benign
    // spell does a room-entity lookup that can fail; no self-cast, no
    // mana, and the round is not consumed.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    let shown = cast(&mut core, s, "c blur extra trailing words");
    assert!(
        shown.contains("You do not see extra trailing words here!\n"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("You cast"), "no self-cast: {shown:?}");
    assert_eq!(core.current_mana(s), 6, "mana unchanged (§8.9)");
    let next = cast(&mut core, s, "c spark");
    assert!(!next.contains(ALREADY_CAST), "round not consumed: {next:?}");
}

#[test]
fn duration_cast_feeds_derived_stats_and_the_sheet() {
    // The slice-3 tripwire (duration_spell_cast_applies_no_stats) closed:
    // a blur cast enters a slot AND the recompute reads it. MEASURED
    // (§8.11): `st` appends DescMsg line3 ("You are blurred!") directly
    // after the MagicRes row while active, and the Martial Arts row rises
    // by exactly the stored magnitude (13→18 live) — the Dodge value adds
    // RAW to the derived dodge, not doubled.
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(hardy("Vexil", MAGE));
    core.set_current_hp(s, 10);
    core.drain_events();
    let base_parry = core.defender_debug(s).parry;
    let shown = cast(&mut core, s, "c blur");
    assert!(shown.contains("You cast blur on Vexil!\n"), "got: {shown:?}");
    assert!(shown.contains("You are blurred!\n"), "active line: {shown:?}");
    let p = core.player_snapshot(s);
    let idx = p.find_active(BLUR).expect("slot occupied");
    let v = i32::from(p.active_spells[idx].value);
    assert!((5..=6).contains(&v), "stored rolled V: {v}");
    // Dodge (34) flows into the defender's parry rating (combat.md:
    // parry = dodgeAbil(0x22) + (Chm-50)/5 + level/5 + (Agl-50)/3 ...).
    assert_eq!(core.defender_debug(s).parry, base_parry + v, "blur dodges");
    // Sheet: Martial Arts = 2*dodge_base(1) + Dodge = 2 + V; the active
    // line follows the MagicRes row.
    core.input(s, "st");
    let sheet = text_to(&core.drain_events(), s);
    let want = format!("Martial Arts:{:>5}", 2 + v);
    assert!(sheet.contains(&want), "MA reflects the slot: {sheet:?}");
    let mr = sheet.find("MagicRes:").expect("MagicRes row");
    let line = sheet.find("You are blurred!").expect("st active line");
    assert!(mr < line, "active line after MagicRes: {sheet:?}");
    // Costs are unchanged from slice 3; blur carries no instant handlers.
    assert_eq!(core.current_mana(s), 2, "full mana 4 paid");
    assert_eq!(core.round_energy(s), 900, "round cost paid");
    assert_eq!(core.current_hp(s), 10, "no instant effects");
}

#[test]
fn dispelling_a_slot_recomputes_derived_stats() {
    // The pre-pass dispel (veil removes WARD) must drop ward's Dodge
    // contribution on the same cast — the recompute runs on slot clear
    // (spec §4: contributions vanish on the next from-scratch recompute).
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(WARD), value: 3, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    let warded_parry = core.defender_debug(s).parry;
    cast(&mut core, s, "c veil");
    let p = core.player_snapshot(s);
    assert_eq!(p.find_active(WARD), None, "ward dispelled");
    assert_eq!(p.find_active(VEIL), None, "veil not entered — cast ended");
    assert_eq!(
        core.defender_debug(s).parry,
        warded_parry - 3,
        "ward's stored Dodge 3 gone from the recompute"
    );
    // ORACLE-VERIFY: blur-over-157 live probe (st should NOT show the
    // dispelling spell's line) — asserted from the decompile early return.
    core.input(s, "st");
    let sheet = text_to(&core.drain_events(), s);
    assert!(!sheet.contains("You are veiled!"), "no veil line: {sheet:?}");
    assert!(!sheet.contains("shimmering ward"), "no ward line: {sheet:?}");
}

#[test]
fn sheet_appends_active_lines_in_slot_order() {
    // ORACLE-VERIFY: multi-buff `st` ordering is unmeasured live; slot
    // order chosen (the DLL iterates the slot array). A DescMsg-less
    // active spell (reap) contributes no line.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(REAP), value: 1, remaining: 40 };
    vexil.active_spells[1] = ActiveSpell { spell: Some(WARD), value: 2, remaining: 40 };
    vexil.active_spells[2] = ActiveSpell { spell: Some(BLUR), value: 5, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "st");
    let sheet = text_to(&core.drain_events(), s);
    let mr = sheet.find("MagicRes:").expect("MagicRes row");
    let ward = sheet
        .find("A shimmering ward surrounds you!")
        .expect("ward line");
    let blur = sheet.find("You are blurred!").expect("blur line");
    assert!(mr < ward && ward < blur, "slot order after MagicRes: {sheet:?}");
    assert!(!sheet.contains("reap"), "DescMsg-less spell adds no line: {sheet:?}");
}

#[test]
fn attached_player_with_saved_slots_derives_them_immediately() {
    // The login path loads player_effect rows into active_spells BEFORE
    // attach_player's first derive (server load_player fills the slots,
    // then attach recomputes) — a restored blur keeps dodging with no
    // recast. Modeled here by attaching a pre-seeded player.
    let mut core = Core::new(world(), CoreConfig::default());
    let plain = core.attach_player(player("Grunt", MAGE, full_book()));
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(BLUR), value: 5, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    assert_eq!(
        core.defender_debug(s).parry,
        core.defender_debug(plain).parry + 5,
        "restored slot feeds the first derive"
    );
    core.input(s, "st");
    let sheet = text_to(&core.drain_events(), s);
    assert!(
        sheet.contains("Martial Arts:    7"),
        "MA = 2 + restored 5: {sheet:?}"
    );
    assert!(sheet.contains("You are blurred!"), "st line: {sheet:?}");
}

// --- duration formula (slice 4 Task 3; spec §4 step 1, decompile
// --- add_cast_spell_to_user 38148-38166) ---

/// A roll source that must never fire (un-banded duration paths).
fn no_roll() -> impl FnMut(i32, i32) -> i32 {
    |_, _| panic!("duration rolled without a band")
}

#[test]
fn duration_is_flat_when_all_scaling_is_zero() {
    // Blur (spell 129): duration 70, duration_per_level 0, increase (0,0),
    // level_cap 0 — 70 flat at any caster level, no band roll.
    let mut probe = spell(SpellId(1), "probe", "prob");
    probe.duration = 70;
    assert_eq!(spell_duration(&probe, 1, 0, &mut no_roll()), 70);
    assert_eq!(spell_duration(&probe, 999, 0, &mut no_roll()), 70);
}

#[test]
fn duration_increase_divides_the_level_first_and_clamps_to_the_cap() {
    // scaled_duration = (L / levels) * per — divide-first truncation
    // (decompile 38156-38158); the level cap clamps exactly like the
    // magnitude clamp: cap < 1 = uncapped (38148-38155).
    let mut probe = spell(SpellId(1), "probe", "prob");
    probe.duration = 10;
    probe.duration_increase = ScalePair { per: 5, levels: 2 };
    assert_eq!(spell_duration(&probe, 7, 0, &mut no_roll()), 25); // 10+(7/2)*5
    probe.level_cap = 4;
    assert_eq!(spell_duration(&probe, 7, 0, &mut no_roll()), 20); // L = 4
    probe.level_cap = -1;
    assert_eq!(spell_duration(&probe, 7, 0, &mut no_roll()), 25); // uncapped
}

#[test]
fn duration_band_rolls_base_to_max_plus_one_only_below_max() {
    // max = duration_per_level * L; base < max -> genrdn(base, max+1),
    // inclusive of BOTH ends like the magnitude roll — the result spans
    // base ..= max+1 (decompile 38160-38163).
    let mut probe = spell(SpellId(1), "probe", "prob");
    probe.duration = 10;
    probe.duration_per_level = 4; // L 5 -> max 20
    assert_eq!(spell_duration(&probe, 5, 0, &mut rolls(&[10])), 10);
    assert_eq!(spell_duration(&probe, 5, 0, &mut rolls(&[21])), 21);
    // base == max never rolls (38161 is a strict less-than).
    probe.duration = 20;
    assert_eq!(spell_duration(&probe, 5, 0, &mut no_roll()), 20);
    // base > max (shrinking per-level data) never rolls either.
    probe.duration = 30;
    assert_eq!(spell_duration(&probe, 5, 0, &mut no_roll()), 30);
}

#[test]
fn alter_sp_length_is_a_final_percentage() {
    // duration = (alter + 100) * duration / 100, integer division
    // (decompile 38165-38166) — applied AFTER the band roll.
    let mut probe = spell(SpellId(1), "probe", "prob");
    probe.duration = 70;
    assert_eq!(spell_duration(&probe, 1, 50, &mut no_roll()), 105);
    assert_eq!(spell_duration(&probe, 1, -50, &mut no_roll()), 35);
    assert_eq!(spell_duration(&probe, 1, 0, &mut no_roll()), 70);
    probe.duration = 25;
    assert_eq!(spell_duration(&probe, 1, 50, &mut no_roll()), 37); // truncates
}

// --- duration slot entry (slice 4 Task 3; spec §4 add_cast_spell_to_user) ---

#[test]
fn duration_cast_enters_the_first_free_slot() {
    let mut core = Core::new(world(), CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    core.input(s, "c veil");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    // MEASURED order (§8.11): castmsgb caster line, then the DescMsg(115)
    // line3 active line (the live async path erases the pending prompt
    // and re-prompts; net visible order matches — noted at the emit site).
    let msg = shown.find("You cast veil on Vexil!").expect("castmsgb line");
    let active = shown.find("You are veiled!").expect("DescMsg line3");
    assert!(msg < active, "order: {shown:?}");
    let p = core.player_snapshot(s);
    let slot = p.active_spells[0];
    assert_eq!(slot.spell, Some(VEIL), "first free slot taken");
    assert!((5..=6).contains(&slot.value), "stored rolled V: {}", slot.value);
    assert_eq!(slot.remaining, 70, "flat blur-model duration");
    assert!(p.active_spells[1..].iter().all(|s| s.spell.is_none()));
    // Slot changes persist.
    assert!(
        events.iter().any(
            |e| matches!(e, Event::Persist(p) if p.active_spells[0].spell == Some(VEIL))
        ),
        "Persist after slot entry"
    );
    // Full costs paid (the stat feed itself is covered by
    // duration_cast_feeds_derived_stats_and_the_sheet).
    assert_eq!(core.current_mana(s), 2, "full mana 4 paid");
    assert_eq!(core.round_energy(s), 900, "round cost only");
}

#[test]
fn duration_recast_refreshes_the_slot_in_place() {
    // Spec §4 step 2: already active -> refresh value + duration in the
    // SAME slot; no second slot. VERIFIED (§8.11): a mid-buff recast is a
    // SILENT full refresh — byte-identical cast lines, full mana charged,
    // timer reset to a full duration from the recast.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(VEIL), value: 1, remaining: 3 };
    let s = core.attach_player(vexil);
    core.drain_events();
    let shown = cast(&mut core, s, "c veil");
    assert!(shown.contains("You cast veil on Vexil!"), "got: {shown:?}");
    assert!(shown.contains("You are veiled!"), "active line on refresh: {shown:?}");
    let p = core.player_snapshot(s);
    assert_eq!(p.active_spells[0].spell, Some(VEIL), "same slot");
    assert!((5..=6).contains(&p.active_spells[0].value), "value refreshed");
    assert_eq!(p.active_spells[0].remaining, 70, "duration refreshed");
    assert_eq!(
        p.active_spells.iter().filter(|s| s.spell.is_some()).count(),
        1,
        "no second slot"
    );
    assert!(!shown.contains("already"), "silent refresh: {shown:?}");
}

#[test]
fn full_slots_print_the_fail_line_and_lose_the_effect() {
    // ORACLE-VERIFY slot overflow (unmeasured live; decompile-backed):
    // add_cast_spell_to_user 38205-38217 — both slot scans exhausted ->
    // "You attempt to cast %s, but fail." to the caster ONLY, return -1.
    // The success roll already passed, so the FULL costs stay paid (no
    // failure half-mana), and display_spell_success is never reached: no
    // castmsgb, no active line, no room broadcast, no slot written.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    for (i, slot) in vexil.active_spells.iter_mut().enumerate() {
        *slot = ActiveSpell { spell: Some(SpellId(1000 + i as u16)), value: 1, remaining: 50 };
    }
    let before = vexil.active_spells;
    let s = core.attach_player(vexil);
    let watcher = core.attach_player(player("Grunt", WARRIOR, BTreeMap::new()));
    core.drain_events();
    core.input(s, "c veil");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    assert!(
        shown.contains("You attempt to cast veil, but fail.\n"),
        "got: {shown:?}"
    );
    assert!(!shown.contains("You cast veil"), "no castmsgb: {shown:?}");
    assert!(!shown.contains("You are veiled!"), "no active line: {shown:?}");
    let seen = text_to(&events, watcher);
    assert!(!seen.contains("veil"), "caster-only, no room line: {seen:?}");
    assert_eq!(core.player_snapshot(s).active_spells, before, "slots untouched");
    assert_eq!(core.current_mana(s), 2, "FULL mana 4 paid, not the failure half");
    assert_eq!(core.round_energy(s), 900, "full round cost paid");
}

#[test]
fn removes_spell_pre_pass_dispels_and_ends_the_cast() {
    // Blur's model (spec §3 pre-pass): (RemovesSpell 122, 157) dispels the
    // amethyst pendant's effect on cast (anti-stacking). Decompile
    // cast_no_target 39472-39494: a FOUND dispel prints the success lines,
    // clears the slot, runs the termination, and RETURNS — the cast ends;
    // veil does NOT enter a slot. RemovesSpell honors the chain (ward
    // simply has no EndCast row).
    // ORACLE-VERIFY: blur-over-157 live probe (st should NOT show blurred).
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.current_mana = 20; // two veil casts at 4 each
    vexil.active_spells[0] = ActiveSpell { spell: Some(WARD), value: 2, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "c veil");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    // display_spell_success runs on the dispel path (39482), so the normal
    // success lines still print even though veil never lands.
    assert!(shown.contains("You cast veil on Vexil!"), "castmsgb: {shown:?}");
    assert!(shown.contains("You are veiled!"), "DescMsg line3: {shown:?}");
    let p = core.player_snapshot(s);
    assert_eq!(p.find_active(WARD), None, "ward slot cleared");
    assert_eq!(p.find_active(VEIL), None, "veil NOT entered — cast ended");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Persist(p) if p.find_active(WARD).is_none())),
        "Persist after dispel"
    );
    // Nothing left to dispel: the next round's cast enters normally.
    energy_round(&mut core);
    cast(&mut core, s, "c veil");
    let p = core.player_snapshot(s);
    let idx = p.find_active(VEIL).expect("second cast enters");
    assert_eq!(p.active_spells[idx].remaining, 70);
}

#[test]
fn kill_spell_pre_pass_also_dispels_and_ends_the_cast() {
    // KillSpell (153) dispels too; it differs from RemovesSpell only by
    // suppressing the victim's EndCast chain (see
    // kill_spell_dispel_suppresses_the_chain). The same 39494 early
    // return applies: reap does NOT enter a slot on the dispel cast.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(WARD), value: 2, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    cast(&mut core, s, "c reap");
    let p = core.player_snapshot(s);
    assert_eq!(p.find_active(WARD), None, "ward slot cleared");
    assert_eq!(p.find_active(REAP), None, "reap NOT entered — cast ended");
    energy_round(&mut core);
    cast(&mut core, s, "c reap");
    assert!(
        core.player_snapshot(s).find_active(REAP).is_some(),
        "second cast enters"
    );
}

#[test]
fn instant_cast_runs_the_dispel_pre_pass_and_skips_its_own_effects() {
    // The pre-pass loop (decompile 39463-39572) runs for EVERY benign cast
    // — it is NOT gated on duration; 17 shipped instant spells carry
    // dispel abilities (the cure-poison family). The 39494 early return
    // precedes the apply loop (39573), so a successful dispel skips the
    // caster's instant effects too: purge's Heal(5) must NOT land.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = hardy("Vexil", MAGE); // max_hp 27
    vexil.current_hp = 10;
    vexil.active_spells[0] = ActiveSpell { spell: Some(WARD), value: 2, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "c purge");
    let events = core.drain_events();
    let p = core.player_snapshot(s);
    assert_eq!(p.find_active(WARD), None, "ward slot cleared");
    assert_eq!(p.current_hp, 10, "Heal skipped — the dispel ended the cast");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Persist(p) if p.find_active(WARD).is_none())),
        "Persist after dispel"
    );
    // Nothing active: the next round's cast applies its instants normally.
    energy_round(&mut core);
    cast(&mut core, s, "c purge");
    assert_eq!(core.player_snapshot(s).current_hp, 15, "Heal lands");
}

#[test]
fn caster_alter_sp_length_stretches_the_entered_duration() {
    // AlterSpLength (166) reads the caster's accumulated ability bag
    // (race/class/gear — decompile 38165 get_user_ability_value(0xa6)):
    // +100% doubles veil's flat 70.
    let mut content = world();
    content
        .classes
        .get_mut(&MAGE)
        .expect("fixture class")
        .abilities
        .push((Ability::AlterSpLength, 100));
    let mut core = Core::new(content, CoreConfig::default());
    let s = core.attach_player(player("Vexil", MAGE, full_book()));
    core.drain_events();
    cast(&mut core, s, "c veil");
    let p = core.player_snapshot(s);
    let idx = p.find_active(VEIL).expect("entered");
    assert_eq!(p.active_spells[idx].remaining, 140, "(100+100)*70/100");
}

#[test]
fn melee_attack_replaces_a_cast_engagement() {
    // `attack` after a cast engagement swings instead of re-firing the
    // spell — the casting slot is cleared by the melee engagement.
    let (mut core, s, m) = arena(RAT);
    cast(&mut core, s, "c doom rat");
    let round = fire_round(&mut core, s);
    assert!(round.contains("You fire"), "cast engagement fires: {round:?}");
    assert_eq!(core.monster_hp(m), Some(991));
    let mana = core.current_mana(s);
    cast(&mut core, s, "attack rat");
    let round = fire_round(&mut core, s);
    assert!(!round.contains("You fire"), "no re-fire after attack: {round:?}");
    assert_eq!(core.current_mana(s), mana, "no further mana charges");
}

// --- termination engine (slice 4 Task 6; spec §5, decompile
// --- perform_spell_termination_player_upkeep 44814-44900) ---

#[test]
fn dispel_prints_the_wear_off_line_after_the_success_lines() {
    // DLL order (39481-39494): display_spell_success FIRST, then clear +
    // terminate — the dispelling cast's own lines precede the victim's
    // DescMsg line1 wear-off. The wear-off goes to the OWNER only
    // (44833-44844); the room hears nothing (ORACLE-VERIFY: no measured
    // room-side wear-off; blur's msg-68 line2 is empty).
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(WARD), value: 2, remaining: 40 };
    let s = core.attach_player(vexil);
    let watcher = core.attach_player(player("Grunt", WARRIOR, BTreeMap::new()));
    core.drain_events();
    core.input(s, "c veil");
    let events = core.drain_events();
    let shown = text_to(&events, s);
    let cast_line = shown.find("You cast veil on Vexil!").expect("castmsgb");
    let active = shown.find("You are veiled!").expect("DescMsg line3");
    let wear = shown
        .find("The shimmering ward fades.")
        .expect("ward's DescMsg line1");
    assert!(cast_line < active && active < wear, "order: {shown:?}");
    let seen = text_to(&events, watcher);
    assert!(!seen.contains("fades"), "owner-only wear-off: {seen:?}");
}

#[test]
fn stat_buff_termination_reverts_through_the_recompute() {
    // The 44-49 hard-write family (spec §5 step 2, reversal 44858-44875):
    // our entry never direct-writes — the buff rides the ability bag into
    // effective_stats, so the sheet shows it while the slot lives and the
    // termination's recompute drops it (KNOWN-DIVERGENCE in mechanism,
    // identical observable). Anthem: (Intel, 0) stored 5 on a base-0
    // fixture -> Intellect row 5, MagicRes (5 + 0*3)/4 = 1.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(ANTHEM), value: 5, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    core.input(s, "st");
    let sheet = text_to(&core.drain_events(), s);
    assert!(sheet.contains("Intellect:  5"), "buffed Intel: {sheet:?}");
    assert!(sheet.contains("An anthem lifts you!"), "st line: {sheet:?}");
    let shown = cast(&mut core, s, "c unsing");
    assert!(shown.contains("The anthem fades.\n"), "wear-off: {shown:?}");
    let p = core.player_snapshot(s);
    assert_eq!(p.find_active(ANTHEM), None, "slot cleared");
    assert_eq!(p.stats.intellect, 0, "base stats never direct-written");
    core.input(s, "st");
    let sheet = text_to(&core.drain_events(), s);
    assert!(sheet.contains("Intellect:  0"), "buff reverted: {sheet:?}");
    assert!(!sheet.contains("anthem"), "st line gone: {sheet:?}");
}

#[test]
fn endcast_chain_fires_on_removes_spell_dispel() {
    // RemovesSpell terminates WITH the chain honored (39491: chainFlag =
    // ability == 0x7a). Chime carries (EndCast, ECHO) and no CastOnEnd
    // row -> pct 100 (44830): the forced mode-2 cast enters echo's slot
    // with a fresh roll and prints its own success lines.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(CHIME), value: 5, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    let shown = cast(&mut core, s, "c unbind");
    assert!(shown.contains("The chime fades.\n"), "wear-off: {shown:?}");
    assert!(
        shown.contains("An echo follows you!\n"),
        "chained cast's active line: {shown:?}"
    );
    let p = core.player_snapshot(s);
    assert_eq!(p.find_active(CHIME), None, "chime slot cleared");
    let idx = p.find_active(ECHO).expect("chained spell entered");
    assert!((5..=6).contains(&p.active_spells[idx].value), "fresh roll");
    assert_eq!(p.active_spells[idx].remaining, 70, "fresh duration");
}

#[test]
fn kill_spell_dispel_suppresses_the_chain() {
    // KillSpell (153) is the chainFlag-0 dispel: the wear-off line and
    // reversal still run, the EndCast chain does not (44891: the flag
    // gates ONLY the chain).
    let mut core = Core::new(world(), CoreConfig::default());
    let mut vexil = player("Vexil", MAGE, full_book());
    vexil.active_spells[0] = ActiveSpell { spell: Some(CHIME), value: 5, remaining: 40 };
    let s = core.attach_player(vexil);
    core.drain_events();
    let shown = cast(&mut core, s, "c sever");
    assert!(shown.contains("The chime fades.\n"), "wear-off still prints: {shown:?}");
    assert!(!shown.contains("echo"), "no chained cast: {shown:?}");
    let p = core.player_snapshot(s);
    assert_eq!(p.find_active(CHIME), None, "chime slot cleared");
    assert_eq!(p.find_active(ECHO), None, "chain suppressed");
}

#[test]
fn death_terminates_all_slots_chain_suppressed_and_purges_temp() {
    // Player death terminates every occupied slot in slot order with the
    // chain SUPPRESSED (decompile check_kill_user death branch
    // 13053-13066: chainFlag '\0' — unlike the reroll path 10404-10419's
    // '\x01'), and GiveTempSpell purges the temporary book entry
    // (44883-44885) while a permanent entry survives. Driven through the
    // M3 bleed: HP -199 hits the -200 floor on the next slow tick.
    let mut core = Core::new(world(), CoreConfig::default());
    let mut book = full_book();
    book.insert(ILLUMINATE, true); // temporary (the slice-2 flag)
    let mut vexil = player("Vexil", MAGE, book);
    vexil.active_spells[0] = ActiveSpell { spell: Some(CHIME), value: 5, remaining: 40 };
    vexil.active_spells[1] = ActiveSpell { spell: Some(BLUR), value: 5, remaining: 40 };
    vexil.active_spells[2] = ActiveSpell { spell: Some(GIFT), value: 1, remaining: 40 };
    vexil.current_hp = -199;
    let s = core.attach_player(vexil);
    core.drain_events();
    for _ in 0..30 {
        core.tick();
    }
    let shown = text_to(&core.drain_events(), s);
    assert!(shown.contains("You have been killed!"), "died: {shown:?}");
    let chime = shown.find("The chime fades.").expect("chime wear-off");
    let blur = shown
        .find("The effects of blur wear off.")
        .expect("blur wear-off");
    assert!(chime < blur, "slot order: {shown:?}");
    assert!(!shown.contains("echo"), "death suppresses the chain: {shown:?}");
    let p = core.player_snapshot(s);
    assert!(p.active_spells.iter().all(|s| s.spell.is_none()), "all slots cleared");
    assert_eq!(p.lives, 8, "miracle respawn");
    assert!(!p.spellbook.contains_key(&ILLUMINATE), "temp entry purged");
    assert!(p.spellbook.contains_key(&BLUR), "permanent entries survive");
}
