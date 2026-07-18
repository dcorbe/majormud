//! The game core: a single-threaded, deterministic state machine.
//!
//! Sessions attach with a loaded (or freshly created) player, feed text input
//! in, and consume `Event`s out. No I/O happens here — the caller owns
//! networking and persistence.

use std::collections::BTreeMap;

use crate::ability::Ability;
use crate::command::{parse, Command, Resolution};
use crate::content::{ClassId, Content, Direction, RaceId, RoomId, ShopStock, SpellId, StatBlock};
use crate::stats::{derive, AbilityBag, Derived, StatInputs};
use crate::text;
use crate::tick::TickScheduler;

/// Integer square root as the engine computes it (the accuracy formula's
/// level term: largest i with (i+1)^2 <= n gives isqrt semantics).
fn isqrt(n: i32) -> i32 {
    let mut i = 0;
    while (i + 1) * (i + 1) <= n {
        i += 1;
    }
    i
}

/// Multi-word prefix matching shared by items and monsters: each input word
/// must prefix consecutive words of the name, starting at any word.
fn word_prefix_match(name: &str, want: &str) -> bool {
    let want: Vec<&str> = want.split_whitespace().collect();
    if want.is_empty() {
        return false;
    }
    let words: Vec<String> = name
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect();
    (0..words.len()).any(|start| {
        want.len() <= words.len() - start
            && want
                .iter()
                .enumerate()
                .all(|(i, w)| words[start + i].starts_with(w))
    })
}

enum FloorMatch {
    Gettable(usize),
    Fixture,
    None,
}

/// Accuracy accumulators (0x16 Accuracy, 0x69, 0x6a) feeding fighter[0].
fn accuracy_ability(id: u16) -> Ability {
    Ability::from_id(id).expect("accuracy ability ids are in the enum")
}

/// Encum (96): percent modifier to carry capacity.
fn encum_ability() -> Ability {
    Ability::from_id(96).expect("Encum is in the enum")
}

/// Clamps an i32 into the u16 counter range for the hunger/thirst word
/// fields (`+0xce`/`+0xd0`). DIVERGENCE: the DLL does a raw wrapping
/// 16-bit add (decompiled 40073/40111); we saturate. Unreachable with
/// shipped data — no instant spell carries Alterhunger/AlterThirst, and
/// no shipped value approaches the bounds.
fn clamp_counter(v: i32) -> u16 {
    u16::try_from(v.clamp(0, i32::from(u16::MAX))).expect("clamped into range")
}

/// The poison counter (`+0xbe`, a DLL `short`): every write site floors at
/// 0 (there is no negative poison); the i16 ceiling is our saturation —
/// the DLL would wrap, unreachable with shipped values.
fn clamp_poison(v: i32) -> i16 {
    i16::try_from(v.clamp(0, i32::from(i16::MAX))).expect("clamped into range")
}

/// Ability slots whose `cast_no_target` apply-loop case is an empty
/// `break` (the shared case list right after the loop head, decompile
/// 39590-39611: 0x17/0x1a/0x34/0x50/0x56/0x61/0x62/0x65/0x6c-0x73/0x78/
/// 0x7a/0x90/0x99), plus EndCast (0x97) / CastOnEnd% (0xa4) whose cases
/// only stash the chain locals (41197-41216). None of these slots ever
/// reaches a `display_spell_success` or `add_cast_spell_to_user` call —
/// they are metadata for other phases (the dispel pre-pass, DescMsg,
/// targeting predicates, the chain). Every OTHER slot is a "driver": the
/// first one to run prints the success display with ITS fixed-or-rolled
/// value as the damage arg and flips the once-flag (`local_49`).
fn ability_case_is_noop(ability: Ability) -> bool {
    matches!(
        ability.id(),
        23 | 26 | 52 | 80 | 86 | 97 | 98 | 101 | 108..=115 | 120 | 122 | 144 | 151 | 153 | 164
    )
}

/// HPRegen (123): percent modifier to slow-tick HP regen.
fn hp_regen_ability() -> Ability {
    Ability::from_id(123).expect("HPRegen is in the enum")
}

/// ManaRgn (145): percent modifier to slow-tick mana regen.
fn mana_regen_ability() -> Ability {
    Ability::from_id(145).expect("ManaRgn is in the enum")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
}

/// The persisted subset of the 0x7ec-byte player record
/// (`re/docs/character_creation.md` §6). Grows with each milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub name: String,
    pub gender: Gender,
    pub race: RaceId,
    pub class: ClassId,
    pub level: u16,
    /// Current effective stats (`+0xa2..+0xac`) — buffs apply here.
    pub stats: StatBlock,
    /// The unmodified base copy (`+0x96..+0xa0`).
    pub base_stats: StatBlock,
    /// `+0x724` — HP-base addend, seeded from the class and grown by training.
    pub hp_base: u16,
    pub current_hp: i32,
    pub current_mana: i32,
    /// `+0xce`/`+0xd0` — vestigial counters in WG3-NT: seeded 1000, decay one
    /// per slow tick, nothing reads them (`regeneration.md` §6).
    pub hunger: u16,
    pub thirst: u16,
    /// `+0x610..+0x620` — the five coin denominations (`economy.md` §1).
    pub coins: Coins,
    /// The irrevocable Lawful (PvP opt-out) choice from creation.
    pub lawful: bool,
    /// Carried items with remaining uses (`+0xd8`/`+0x268`, cap 100).
    pub inventory: Vec<(crate::content::ItemId, i16)>,
    /// Wielded weapon (`+0x624`).
    pub weapon: Option<(crate::content::ItemId, i16)>,
    /// Bank balances (shop id, copper) — the BANKBOOK records.
    pub bankbooks: Vec<(u16, u64)>,
    /// Worn equipment (`+0x62c[20]`).
    pub worn: Vec<(crate::content::ItemId, i16)>,
    pub cp_unspent: u16,
    pub cp_lifetime: u16,
    pub lives: u16,
    pub experience: u64,
    pub location: RoomId,
    /// Learned spells; `true` = temporary (GiveTempSpell 160, purged when
    /// the granting effect terminates — see `terminate_active_spell`).
    /// Display order is computed at render (level, then name), not
    /// storage order.
    pub spellbook: BTreeMap<SpellId, bool>,
    /// `+0xbe` — the poison counter (a `short` in the DLL). Each slow tick
    /// with a positive counter prints "You feel ill." and deals that much
    /// HP damage (`regeneration.md` §4, decompile 19518-19533). Written
    /// set-if-greater by Poison(19) at cast application, reduced by
    /// CurePoison(20), the poison spell's termination, the healer's curing
    /// service, and zeroed by death (13066). Never negative.
    pub poison: i16,
    /// Active duration-spell slots (`spellcasting.md` §1: id `+0x40+i*2`,
    /// value `+0x54+i*2`, remaining ticks `+0x68+i*2` — 10 slots each).
    /// An array, not a Vec: slot exhaustion is observable (an 11th buff
    /// finds no free slot).
    pub active_spells: [ActiveSpell; 10],
}

/// One player active-spell slot (`spellcasting.md` §1). `spell` is `None`
/// for an empty slot (the DLL's id 0); `value` is the stored potency fed to
/// upkeep and the dynamic-stat recompute (`+0x54`); `remaining` counts down
/// in upkeep ticks (`+0x68`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActiveSpell {
    pub spell: Option<SpellId>,
    pub value: i16,
    pub remaining: i32,
}

impl Player {
    /// Index of the first empty active-spell slot (`spellcasting.md` §4
    /// step 3: "write into the first empty slot"), or `None` when all 10
    /// are occupied.
    pub fn first_free_slot(&self) -> Option<usize> {
        self.active_spells.iter().position(|s| s.spell.is_none())
    }

    /// Index of the slot holding `spell`, if it is currently active
    /// (`spellcasting.md` §4 step 2: recast refreshes in place).
    pub fn find_active(&self, spell: SpellId) -> Option<usize> {
        self.active_spells
            .iter()
            .position(|s| s.spell == Some(spell))
    }
}

/// Why a spell can('t) be learned/used by this character.
/// [`Core::spell_gate`] covers gates 1-2 (spellcasting.md §2); the
/// alignment lattice (gate 3) is deferred with M4's other alignment
/// gates — no starter scroll carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellGate {
    Ok,
    /// Wrong magery group, or the class can't ever cast this deep.
    WrongClass,
    /// Right class, character level below spell.required_power (+0xbe).
    /// Oracle-proven level gate (spellcasting.md §8.3).
    TooPowerful,
}

/// The five coin denominations, high to low (`+0x610..+0x620`). All prices
/// are computed in copper (index 0, the lowest) via `convert_currency`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Coins {
    pub runic: u32,
    pub platinum: u32,
    pub gold: u32,
    pub silver: u32,
    pub copper: u32,
}

impl Coins {
    /// `convert_currency`: collapse to a copper total using the inter-coin
    /// ratios (config globals `DAT_00482ce0..cec`; values ORACLE-VERIFY,
    /// default 10 per step).
    pub fn total_copper(&self, ratios: [u64; 4]) -> u64 {
        let mut total = u64::from(self.runic);
        total = total * ratios[3] + u64::from(self.platinum);
        total = total * ratios[2] + u64::from(self.gold);
        total = total * ratios[1] + u64::from(self.silver);
        total * ratios[0] + u64::from(self.copper)
    }

    /// `cleanup_currency`: roll loose copper upward into proper coins
    /// through the ratios (selling/withdrawing mint path).
    pub fn add_copper_and_mint(&mut self, amount: u64, ratios: [u64; 4]) {
        let mut copper = u64::from(self.copper) + amount;
        let silver_r = ratios[0];
        let gold_r = ratios[1];
        let plat_r = ratios[2];
        let runic_r = ratios[3];
        let mut silver = u64::from(self.silver) + copper / silver_r;
        copper %= silver_r;
        let mut gold = u64::from(self.gold) + silver / gold_r;
        silver %= gold_r;
        let mut platinum = u64::from(self.platinum) + gold / plat_r;
        gold %= plat_r;
        let runic = u64::from(self.runic) + platinum / runic_r;
        platinum %= runic_r;
        self.copper = copper as u32;
        self.silver = silver as u32;
        self.gold = gold as u32;
        self.platinum = platinum as u32;
        self.runic = runic as u32;
        let _ = (&mut silver, &mut gold, &mut platinum);
    }

    /// `deduct_currency` (0x1edca), exact: repeat { greedily spend each
    /// denomination high->low while one whole coin fits the remainder;
    /// then break ONE coin of the smallest non-empty denomination above
    /// copper } until paid. Returns the coins actually handed over — the
    /// original prints them ("You just bought sickle for 9 silver nobles,
    /// 14 copper farthings.", oracle_m4_verify.raw, change-making
    /// visible). Caller checks affordability.
    pub fn deduct_copper(&mut self, amount: u64, ratios: [u64; 4]) -> Coins {
        debug_assert!(self.total_copper(ratios) >= amount);
        let per_silver = ratios[0];
        let per_gold = per_silver * ratios[1];
        let per_platinum = per_gold * ratios[2];
        let per_runic = per_platinum * ratios[3];
        let mut due = amount;
        let mut spent = Coins::default();

        while due > 0 {
            let mut pay = |drawer: &mut u32, spent: &mut u32, per: u64| {
                while per <= due && *drawer > 0 {
                    *drawer -= 1;
                    *spent += 1;
                    due -= per;
                }
            };
            pay(&mut self.runic, &mut spent.runic, per_runic);
            pay(&mut self.platinum, &mut spent.platinum, per_platinum);
            pay(&mut self.gold, &mut spent.gold, per_gold);
            pay(&mut self.silver, &mut spent.silver, per_silver);
            pay(&mut self.copper, &mut spent.copper, 1);
            if due == 0 {
                break;
            }
            // Break one coin of the smallest non-empty higher denomination.
            if self.silver > 0 {
                self.silver -= 1;
                self.copper += ratios[0] as u32;
            } else if self.gold > 0 {
                self.gold -= 1;
                self.silver += ratios[1] as u32;
            } else if self.platinum > 0 {
                self.platinum -= 1;
                self.gold += ratios[2] as u32;
            } else if self.runic > 0 {
                self.runic -= 1;
                self.platinum += ratios[3] as u32;
            } else {
                break; // insufficient (guarded by check_currency upstream)
            }
        }
        spent
    }
}

/// The authenticated identity a session arrives with. In the original this
/// came from the Worldgroup account; here it comes from our auth layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProfile {
    pub name: String,
    pub gender: Gender,
}

/// Server-operator configuration (the original's sysop config globals).
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Where new characters start (`DAT_00482cf8`). VERIFIED (oracle): the
    /// stock game starts new characters at Newhaven, Village Entrance.
    pub start_location: RoomId,
    /// Inter-coin ratios low→high (`DAT_00482cec..ce0`; ORACLE-VERIFY).
    pub coin_ratios: [u64; 4],
    /// Global level cap (`DAT_00482d90`; ORACLE-VERIFY the stock value).
    pub level_cap: u16,
    /// Lives granted per level (`DAT_00482cd8`, capped at 9; ORACLE-VERIFY).
    pub lives_per_level: u16,
    /// Seed for the game RNG (`genrdn`). Fixed seed = reproducible session.
    pub rng_seed: u64,
    /// Seconds (= dots) of exit meditation (ORACLE-VERIFY: 10 observed).
    pub exit_meditation_seconds: u8,
    /// Death recall room (`DAT_00482cfc`/`d00` temples; alignment split and
    /// per-room DeathRoom overrides arrive with alignment/zones). ORACLE:
    /// Newhaven deaths recall to Newhaven, Healer.
    pub recall_location: RoomId,
}

impl Default for CoreConfig {
    fn default() -> Self {
        CoreConfig {
            start_location: RoomId { map: 1, room: 2140 },
            // ORACLE-VERIFIED (Bank of Godfrey lobby sign): 10c=1s, 10s=1g,
            // 100g=1p, 100p=1r.
            coin_ratios: [10, 10, 100, 100],
            level_cap: 3000,
            lives_per_level: 1,
            rng_seed: 0x4d4d55445f574721, // "MMUD_WG!"
            exit_meditation_seconds: 10,
            recall_location: RoomId { map: 1, room: 2190 },
        }
    }
}

/// `genrdn(lo, hi)`-style PRNG: xorshift64*, uniform in `[lo, hi]`.
/// Deterministic given the seed; exactness targets distributions, not the
/// original's roll stream (design decision).
struct Rng(u64);

impl Rng {
    fn roll(&mut self, lo: i32, hi: i32) -> i32 {
        debug_assert!(lo <= hi);
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545F4914F6CDD1D);
        let span = (hi - lo + 1) as u64;
        lo + (x % span) as i32
    }
}

/// The cast success roll (spec §3 step 5; decompiled 39300-39340):
/// `base_chance >= 200` auto-succeeds; otherwise
/// `chance = min(SC + base_chance, 98)` and the cast succeeds when
/// `genrdn(0,100) < chance`. DELIBERATE DIVERGENCE: the DLL rolls
/// `genrdn(0,100)` unconditionally and discards it on auto-success
/// (39300); we skip the roll — outcomes identical, and RNG-stream parity
/// with the DLL's generator is unattainable anyway. There is no floor: a
/// chance at or below 0 (possible only with negative SC, i.e. a
/// non-caster) never succeeds.
/// `roll(lo, hi)` must return a uniform value in `[lo, hi]` — the
/// `calculate_attack` injection seam, so tests can script rolls.
pub fn cast_roll_succeeds(
    spellcasting: i32,
    base_chance: i16,
    roll: &mut impl FnMut(i32, i32) -> i32,
) -> bool {
    if base_chance >= 200 {
        return true;
    }
    let chance = (spellcasting + i32::from(base_chance)).min(98);
    roll(0, 100) < chance
}

/// The offensive-cast magnitude roll (decompile `cast_monster_target`
/// 43668-43704, identical block in `cast_user_target` 41783-41811; spec §3):
///
/// - `L` = caster level, clamped to `level_cap` — the DLL clamp is
///   `cap < 1 || level <= cap ? level : cap`, so a cap at or below 0 means
///   UNCAPPED and a positive cap gives `min(level, cap)`.
/// - `hi = max_base + max_increase.scaled(L)`,
///   `lo = min_base + min_increase.scaled(L)`; inverted data does not swap —
///   the DLL lowers `lo` to `hi` and keeps `hi` (`lo = min(lo, hi)`).
/// - `V = genrdn(0, hi - lo + 1) + lo`. genrdn is inclusive of BOTH ends,
///   so V spans `lo ..= hi + 1` — one MORE than the printed bounds. This is
///   not a bug in our port: the oracle observed mmis damage 13 at L1 bounds
///   4..12 (§8.6), which is exactly `lo 4 + top roll 9`.
/// - Resist: `V = (100 - resist) * V / 100` (integer division; resist 0
///   passes V through). The caller supplies the target's elemental resist —
///   0 for Element::Magic, which has no resist ability (spec §4).
///
/// `roll(lo, hi)` must return a uniform value in `[lo, hi]` — the same
/// injectable seam as `cast_roll_succeeds`.
pub fn spell_magnitude(
    spell: &crate::content::Spell,
    caster_level: u16,
    resist: i32,
    roll: &mut impl FnMut(i32, i32) -> i32,
) -> i32 {
    let level = i32::from(caster_level);
    let cap = i32::from(spell.level_cap);
    let l = if cap < 1 || level <= cap { level } else { cap };
    let hi = i32::from(spell.max_base) + spell.max_increase.scaled(l);
    let lo = (i32::from(spell.min_base) + spell.min_increase.scaled(l)).min(hi);
    let v = roll(0, hi - lo + 1) + lo;
    (100 - resist) * v / 100
}

/// The duration roll for a successful duration-spell cast (decompile
/// `add_cast_spell_to_user` 38148-38166, twin `add_cast_spell_to_monster`
/// 38238-38256; spec §4 step 1):
///
/// - `L` = caster level with the SAME clamp as [`spell_magnitude`]:
///   `cap < 1 || level <= cap ? level : cap` (cap at or below 0 = uncapped).
/// - `base = duration + duration_increase.scaled_duration(L)` — the
///   divide-first `(L / levels) * per` scaling.
/// - `max = duration_per_level * L`; only when `base < max` (strict — the
///   DLL's `base <= max && base != max`) is the band rolled:
///   `duration = genrdn(base, max + 1)`. genrdn is inclusive of BOTH ends
///   (the §8.6 magnitude finding), so the result spans `base ..= max + 1`.
/// - AlterSpLength (166) — the CASTER's accumulated ability percentage —
///   applies last: `duration = (alter + 100) * duration / 100` (integer
///   division). ORACLE-VERIFY: this term's placement is inferred through a
///   Ghidra return-register artifact (the decompile reuses the register at
///   38165-38166), not a clean data-flow — probe with an AlterSpLength
///   race/item live.
///
/// Blur (129): all scaling zero → 70 flat. `roll(lo, hi)` is the same
/// injectable uniform-`[lo, hi]` seam as [`spell_magnitude`]. The DLL
/// stores the slot value AND duration as 16-bit words; our slots keep the
/// i32 result (values in range agree).
pub fn spell_duration(
    spell: &crate::content::Spell,
    caster_level: u16,
    alter_sp_length: i32,
    roll: &mut impl FnMut(i32, i32) -> i32,
) -> i32 {
    let level = i32::from(caster_level);
    let cap = i32::from(spell.level_cap);
    let l = if cap < 1 || level <= cap { level } else { cap };
    let base = i32::from(spell.duration) + spell.duration_increase.scaled_duration(l);
    let max = i32::from(spell.duration_per_level) * l;
    let duration = if base < max { roll(base, max + 1) } else { base };
    (alter_sp_length + 100) * duration / 100
}

/// The monster saving throw against a player's successful offensive cast
/// (decompile `cast_monster_target` 43594-43614; spec §3). Rolled only when
/// the spell's save class grants one (`Always`, or `IfAntiMagic` on a
/// monster with AntiMagic 51): `genrdn(1, 100) <= min(save_stat / 2, 98)`
/// resists. The halving truncates toward zero, so `save_stat` 1 (the
/// engine's floor) never resists.
pub fn monster_save_resists(save_stat: i32, roll: &mut impl FnMut(i32, i32) -> i32) -> bool {
    roll(1, 100) <= (save_stat / 2).min(98)
}

/// The Damage(-MR) (17) scale — the damage path the shipped attack spells
/// predominantly carry (magic missile included). `mr` is the SAME stat the
/// saving throw reads (M.R.(36) modifiers + the template `mr` word, floored
/// at 1); `anti_magic` = the target carries AntiMagic (51). Decompile
/// `cast_monster_target` 43937-43993 (player-target twin `cast_no_target`
/// 40137-40198):
///
/// - without AntiMagic: `reduction% = clamp((mr-50)/2, 0, 50)`; when that
///   is 0 the damage is instead AMPLIFIED by `(50-mr)%` — a floor-MR
///   target takes +49%, and MR 50 is the unchanged pivot;
/// - with AntiMagic: `reduction% = clamp(mr/2, 0, 75)`, no amplification.
///
/// Divisions truncate toward zero (the DLL's signed idiv; Rust `/`
/// matches). DIVERGENCE (slice 5): the DLL first boosts the amount by the
/// caster's AlterSpDmg(165) percent (43940-43941; plain Damage gets the
/// same boost via FUN_0043fef4 39025) — no user-ability aggregation feeds
/// spells yet, so both paths skip it alike.
pub fn damage_mr(amount: i32, mr: i32, anti_magic: bool) -> i32 {
    let reduction = if anti_magic {
        (mr / 2).clamp(0, 75)
    } else {
        ((mr - 50) / 2).clamp(0, 50)
    };
    if reduction == 0 {
        if anti_magic {
            amount
        } else {
            amount + amount * (50 - mr) / 100
        }
    } else {
        amount - amount * reduction / 100
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Output { session: SessionId, text: String },
    Persist(Box<Player>),
    /// A shop shelf changed — upsert its rows in state.sqlite (the
    /// original's shop dirty byte +0x1dc → Btrieve save).
    PersistShopStock {
        shop: crate::content::ShopId,
        counts: [i16; 20],
    },
    /// Permadeath: remove the character record entirely.
    DeleteCharacter(String),
    Disconnect(SessionId),
}

enum Session {
    /// Character creation: race, class, then the Lawful question
    /// (`character_creation.md` §1 + oracle addendum §6.5).
    ChoosingRace {
        profile: AccountProfile,
    },
    ChoosingClass {
        profile: AccountProfile,
        race: RaceId,
    },
    ChoosingLawful {
        profile: AccountProfile,
        race: RaceId,
        class: ClassId,
    },
    InGame {
        player: Box<Player>,
        derived: Derived,
        /// Dots left in the exit meditation; `Some` swallows all input.
        exiting: Option<u8>,
        /// Autocombat target (the `DAT_004877e8` record, PvE slice).
        target: Option<MonsterInstanceId>,
        /// The AID flag (`+0x6a8`): a downed player stabilizes instead of
        /// bleeding. Cleared when HP climbs above 0.
        aided: bool,
        /// Combat energy pool (`+0xba`; max/regen `+0xb8` = 1000 default).
        energy: i32,
        /// One cast per combat round, even for energy-0 spells (MEASURED
        /// §8.6); set whether the roll then succeeds or fails, cleared by
        /// `energy_round`.
        cast_this_round: bool,
        /// The spell an offensive-cast engagement re-fires each combat
        /// round in place of melee swings (the DLL threads the spell id
        /// through `engage_autocombat`, decompile 43469; the unprompted
        /// re-fire is MEASURED §8.9). `Some` only while `target` is
        /// `Some`; cleared with it, and replaced by a melee `attack`.
        casting: Option<SpellId>,
    },
}

/// Self-rescheduling background jobs (`combat_rounds.md` §1). Medium (3 s)
/// and energy (5 s) tiers join with their systems in later milestones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    /// `background_slow`, every 30 s: regen, hunger/thirst decay.
    Slow,
    /// `background_energy`, every 5 s: the combat round.
    Energy,
    /// The "medium/routine update" pass (spec §5): active-spell duration
    /// decay, recurring per-tick effects, expiry termination.
    Upkeep,
    /// One meditation dot for a pending exit.
    ExitStep(SessionId),
    /// The nightly-cleanup stand-in: Worldgroup restarted the module every
    /// night, re-running check_initiate_restocking (its run-once flag
    /// DAT_00482138 is never reset within a process). A standalone server
    /// re-runs the shelf reconciliation every 24 h instead.
    Cleanup,
}

const SLOW_INTERVAL: u64 = 30;
/// One emulated board day (the nightly cleanup cadence).
const CLEANUP_INTERVAL: u64 = 86_400;
/// The combat-round cadence (`background_energy`).
const ENERGY_INTERVAL: u64 = 5;
/// The duration-upkeep cadence. MEASURED (spellcasting.md §8.11:
/// 3.03-3.06 s/tick over three clean runs) — a ~3 s routine cycle, NOT
/// the 5 s energy round; blur's 70 ticks ≈ 3½ minutes.
const UPKEEP_INTERVAL: u64 = 3;
/// Player energy pool max/regen (`DAT_00482cd0` default).
const PLAYER_ENERGY_MAX: i32 = 1000;
/// The two-stage death gate: HP at/below this kills (`DAT_00482cf0`;
/// ORACLE: -200 in the stock config — died at -204, survived -196).
pub const DEATH_FLOOR: i32 = -200;

/// A live monster in the world (ephemeral — evaporates on restart, like the
/// original's instances).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonsterInstanceId(pub u64);

#[derive(Debug, Clone)]
pub(crate) struct MonsterInstance {
    pub template: crate::content::MonsterId,
    pub location: RoomId,
    pub current_hp: i32,
    /// Current energy pool (`mon+0x16`); regen/max = the template's `energy`.
    pub energy: i32,
    /// The player this monster is fighting (retaliation; aggression is M6).
    pub target: Option<SessionId>,
    /// Carried loot, rolled once at spawn (`generate_monster` step 5:
    /// carry iff genrdn(1,100) <= dropper). All of it drops at death.
    pub items: Vec<(crate::content::ItemId, i16)>,
}

/// A scheduled shop-slot restock, due at an absolute tick. Events live
/// forever and reschedule themselves, as the original's linked list does.
#[derive(Debug, Clone, Copy)]
struct RestockEvent {
    shop: crate::content::ShopId,
    slot: usize,
    due: u64,
}

pub struct Core {
    content: Content,
    config: CoreConfig,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    events: Vec<Event>,
    scheduler: TickScheduler<Job>,
    rng: Rng,
    monsters: BTreeMap<MonsterInstanceId, MonsterInstance>,
    next_monster: u64,
    /// Ephemeral floor coin piles per room (low->high denominations).
    room_coins: BTreeMap<RoomId, [u32; 5]>,
    /// Ephemeral floor items per room (item, remaining uses), seeded from
    /// the rooms' static placements at boot.
    room_items: BTreeMap<RoomId, Vec<(crate::content::ItemId, i16)>>,
    /// Live shop stock counts. Boot fills every shelf to max — the pristine
    /// distribution shipped full, and the `shopnow` values in an extracted
    /// .VIR are played-board runtime state. (Cross-restart persistence in
    /// state.sqlite is deferred; a restart is a first run.)
    shop_stock: BTreeMap<crate::content::ShopId, [i16; 20]>,
    /// Pending timed restock events (`check_initiate_restocking` 0x5b58c).
    restock_events: Vec<RestockEvent>,
    /// `DAT_0047fb80`: the sweep runs on every 21st slow tick (630 s).
    restock_counter: u8,
    /// Set while an action-exit trigger routes through move_player.
    action_exit_pass: bool,
}

impl Core {
    pub fn new(content: Content, config: CoreConfig) -> Core {
        let mut scheduler = TickScheduler::new();
        scheduler.schedule_in(SLOW_INTERVAL, Job::Slow);
        scheduler.schedule_in(ENERGY_INTERVAL, Job::Energy);
        scheduler.schedule_in(UPKEEP_INTERVAL, Job::Upkeep);
        scheduler.schedule_in(CLEANUP_INTERVAL, Job::Cleanup);
        let rng = Rng(config.rng_seed | 1);
        let mut core = Core {
            content,
            config,
            sessions: BTreeMap::new(),
            next_session: 1,
            events: Vec::new(),
            scheduler,
            rng,
            monsters: BTreeMap::new(),
            next_monster: 1,
            room_coins: BTreeMap::new(),
            room_items: BTreeMap::new(),
            shop_stock: BTreeMap::new(),
            restock_events: Vec::new(),
            restock_counter: 0,
            action_exit_pass: false,
        };
        // First run of the world: every shelf full, and each timed slot's
        // first event lands at genrdn(1, max(2, interval)) minutes so the
        // timers don't all fire together (check_initiate_restocking).
        let mut timed: Vec<(crate::content::ShopId, usize, i16)> = Vec::new();
        for shop in core.content.shops.values() {
            let mut counts = [0i16; 20];
            for (i, slot) in shop.stock.iter().enumerate() {
                counts[i] = slot.max;
                if shop.shop_type != 11 && slot.item.is_some() && slot.restock_time > 0 {
                    timed.push((shop.id, i, slot.restock_time));
                }
            }
            core.shop_stock.insert(shop.id, counts);
        }
        for (shop, slot, interval) in timed {
            let minutes = core.rng.roll(1, i32::from(interval).max(2));
            core.restock_events.push(RestockEvent {
                shop,
                slot,
                due: minutes as u64 * 60,
            });
        }
        // Seed floor items from static placements.
        let mut seeded: BTreeMap<RoomId, Vec<(crate::content::ItemId, i16)>> = BTreeMap::new();
        for room in core.content.rooms.values() {
            for placed in &room.placed_items {
                let uses = core
                    .content
                    .items
                    .get(&placed.item)
                    .map_or(-1, |i| i.uses);
                let entry = seeded.entry(room.id).or_default();
                for _ in 0..placed.quantity.max(1) {
                    entry.push((placed.item, uses));
                }
            }
        }
        core.room_items = seeded;
        core
    }

    /// Places a live monster from its template (fixture placement — the
    /// density-driven spawner arrives in M6). `None` for unknown templates
    /// or rooms.
    pub fn spawn_monster(
        &mut self,
        template: crate::content::MonsterId,
        room: RoomId,
    ) -> Option<MonsterInstanceId> {
        let tpl = self.content.monsters.get(&template)?;
        self.content.rooms.get(&room)?;
        let loot = tpl.loot.clone();
        let (hitpoints, energy) = (tpl.hitpoints, tpl.energy);
        let mut items = Vec::new();
        for slot in loot {
            if !self.content.items.contains_key(&slot.item) {
                continue; // shipped dangling ref (saracen commander)
            }
            if self.rng.roll(1, 100) <= i32::from(slot.dropper) {
                items.push((slot.item, slot.uses));
            }
        }
        let id = MonsterInstanceId(self.next_monster);
        self.next_monster += 1;
        self.monsters.insert(
            id,
            MonsterInstance {
                template,
                location: room,
                current_hp: hitpoints,
                energy,
                target: None,
                items,
            },
        );
        Some(id)
    }

    /// Test/inspection: a live monster's current HP (`None` once dead/gone).
    pub fn monster_hp(&self, id: MonsterInstanceId) -> Option<i32> {
        self.monsters.get(&id).map(|m| m.current_hp)
    }

    /// Test hook: mutable access to loaded content.
    pub fn content_mut(&mut self) -> &mut Content {
        &mut self.content
    }

    /// Test hook: a copy of the live player state.
    pub fn player_snapshot(&self, session: SessionId) -> Player {
        self.player(session).clone()
    }

    /// Awards experience (`add_experience` — the restructured-flag path is
    /// always satisfied here since our characters are created flagged).
    /// The over-level banking cap joins with combat exp in M3.
    pub fn add_experience(&mut self, session: SessionId, amount: u64) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.experience += amount;
        }
    }

    /// Test hook: the attacker fighter and per-swing EU for a session
    /// (regression coverage for the decompile-exact formulas).
    pub fn combat_debug(&self, session: SessionId) -> (crate::combat::Fighter, i32) {
        (
            self.build_player_attacker(session),
            self.player_energy_used(session),
        )
    }

    /// Test hook: set lives.
    pub fn set_lives(&mut self, session: SessionId, lives: u16) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.lives = lives;
        }
    }

    /// Test hook: grant coins.
    pub fn give_copper(&mut self, session: SessionId, amount: u32) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.coins.copper += amount;
        }
    }

    /// Test hook: set the purse.
    pub fn set_coins(&mut self, session: SessionId, coins: Coins) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.coins = coins;
        }
    }

    /// Test hook: put an item in a player's hands.
    pub fn give_item(&mut self, session: SessionId, item: crate::content::ItemId) {
        let uses = self.content.items.get(&item).map_or(-1, |i| i.uses);
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.inventory.push((item, uses));
        }
    }

    /// Test hook: drop coins on a room floor (low->high denominations).
    pub fn add_room_coins(&mut self, room: RoomId, coins: [u32; 5]) {
        let piles = self.room_coins.entry(room).or_insert([0; 5]);
        for (i, c) in coins.iter().enumerate() {
            piles[i] += c;
        }
    }

    /// Advances game time by one tick (= one second) and runs due jobs.
    pub fn tick(&mut self) {
        for job in self.scheduler.advance() {
            match job {
                Job::Slow => {
                    self.slow_update();
                    // background_slow runs the restock sweep on every 21st
                    // slow tick (DAT_0047fb80 counter, fires past 0x14).
                    self.restock_counter += 1;
                    if self.restock_counter > 20 {
                        self.restock_counter = 0;
                        self.restock_items();
                    }
                    self.scheduler.schedule_in(SLOW_INTERVAL, Job::Slow);
                }
                Job::Energy => {
                    self.energy_round();
                    self.scheduler.schedule_in(ENERGY_INTERVAL, Job::Energy);
                }
                Job::Upkeep => {
                    self.upkeep_update();
                    self.scheduler.schedule_in(UPKEEP_INTERVAL, Job::Upkeep);
                }
                Job::ExitStep(session) => self.exit_step(session),
                Job::Cleanup => {
                    self.reconcile_shelves();
                    self.scheduler.schedule_in(CLEANUP_INTERVAL, Job::Cleanup);
                }
            }
        }
    }

    /// Applies shelf counts saved in state.sqlite (call once, right after
    /// boot), then runs the reconciliation pass the original performed in
    /// `check_initiate_restocking` on every module load. Rows naming
    /// unknown shops or empty slots are ignored (content may have changed
    /// between runs).
    pub fn restore_shop_stock(&mut self, rows: &[(crate::content::ShopId, usize, i16)]) {
        for &(shop, slot, now) in rows {
            let known = self
                .content
                .shops
                .get(&shop)
                .is_some_and(|s| slot < s.stock.len() && s.stock[slot].item.is_some());
            if known {
                self.shop_stock.entry(shop).or_default()[slot] = now;
            }
        }
        self.reconcile_shelves();
    }

    /// The per-slot reconciliation from `check_initiate_restocking`
    /// (0x5b58c), minus the event scheduling done at boot: clamp overstock
    /// down to max, and give dented interval-0 slots their one
    /// probability-gated top-up. Runs at restore and on the daily cleanup.
    fn reconcile_shelves(&mut self) {
        let shop_ids: Vec<crate::content::ShopId> =
            self.content.shops.keys().copied().collect();
        for shop_id in shop_ids {
            let shop = &self.content.shops[&shop_id];
            if shop.shop_type == 11 {
                continue;
            }
            let slots: Vec<(usize, ShopStock)> = shop
                .stock
                .iter()
                .enumerate()
                .filter(|(_, s)| s.item.is_some())
                .map(|(i, s)| (i, *s))
                .collect();
            let mut changed = false;
            for (i, slot) in slots {
                let counts = self.shop_stock.entry(shop_id).or_default();
                if counts[i] > slot.max {
                    counts[i] = slot.max;
                    changed = true;
                } else if slot.restock_time == 0 && counts[i] < slot.max {
                    let roll = self.rng.roll(1, 100);
                    let counts = self.shop_stock.entry(shop_id).or_default();
                    if roll < i32::from(slot.restock_percent) {
                        counts[i] = (counts[i] + slot.restock_amount).min(slot.max);
                        changed = true;
                    }
                }
            }
            if changed {
                self.persist_shop_stock(shop_id);
            }
        }
    }

    /// Emits the shelf-changed event (the dirty byte +0x1dc).
    fn persist_shop_stock(&mut self, shop: crate::content::ShopId) {
        let counts = self.shop_stock.entry(shop).or_default();
        self.events.push(Event::PersistShopStock {
            shop,
            counts: *counts,
        });
    }

    /// `restock_items` (0x5b6f6): drains due restock events. A due event on
    /// a live non-gang shop rolls 1-100 < percent to add the slot's amount
    /// (clamped to max, and only while below max), then reschedules itself
    /// +interval minutes whether or not the roll succeeded.
    fn restock_items(&mut self) {
        let now = self.scheduler.now();
        let mut events = std::mem::take(&mut self.restock_events);
        for ev in &mut events {
            if ev.due > now {
                continue;
            }
            let Some(shop) = self.content.shops.get(&ev.shop) else {
                continue;
            };
            if shop.shop_type == 11 {
                continue;
            }
            let slot = &shop.stock[ev.slot];
            let counts = self.shop_stock.entry(ev.shop).or_default();
            if counts[ev.slot] < slot.max {
                let roll = self.rng.roll(1, 100);
                if roll < i32::from(slot.restock_percent) {
                    let counts = self.shop_stock.entry(ev.shop).or_default();
                    counts[ev.slot] = (counts[ev.slot] + slot.restock_amount).min(slot.max);
                    let snapshot = *counts;
                    self.events.push(Event::PersistShopStock {
                        shop: ev.shop,
                        counts: snapshot,
                    });
                }
            }
            ev.due = now + u64::from(slot.restock_time.max(1) as u16) * 60;
        }
        self.restock_events = events;
    }

    /// `slow_update_characters` (`regeneration.md`): hunger/thirst decay,
    /// poison damage, bleed/aid, HP regen, mana regen for every in-game
    /// player.
    fn slow_update(&mut self) {
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in sessions {
            let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&id) else {
                continue;
            };
            let (max_hp, max_mana) = (derived.max_hp, derived.max_mana);
            let bag = self.ability_bag(player);
            // Regen reads the EFFECTIVE stats (+0xa2..: slow_update's
            // formulas consume the buffed fields the DLL direct-writes;
            // we fold from the bag — see effective_stats).
            let stats = self.effective_stats(player, &bag);
            let caster = self
                .content
                .classes
                .get(&player.class)
                .map(|c| (c.caster_group, c.casting_factor));
            let Some(Session::InGame { player, aided, .. }) = self.sessions.get_mut(&id)
            else {
                unreachable!("checked above");
            };
            let aided = *aided;

            player.hunger = player.hunger.saturating_sub(1);
            player.thirst = player.thirst.saturating_sub(1);

            // Poison (`regeneration.md` §4; decompile 19518-19533): a
            // positive counter prints "You feel ill.", deals its value in
            // HP damage, announces the drop when HP crosses from above 0
            // to below 0 (19526-19528: FUN_0043c91d, the same announce as
            // the upkeep Damage crossing), then check_kill_user. The
            // counter itself does not decay here — only CurePoison, the
            // poison spell's termination, the healer, or death lower it.
            // A downed poisoned player still bleeds below (the DLL's
            // branches are chained the same way), and a living one still
            // regenerates in the same tick.
            if player.poison > 0 {
                let was_up = player.current_hp > 0;
                player.current_hp -= i32::from(player.poison);
                let dropped = was_up && player.current_hp < 0;
                let name = player.name.clone();
                let room = player.location;
                self.output_line(id, text::YOU_FEEL_ILL);
                if dropped {
                    self.output_line(id, &text::drops_to_ground(&name));
                    self.broadcast_to_room(room, Some(id), &text::drops_to_ground(&name));
                }
                if self.player(id).current_hp <= DEATH_FLOOR {
                    self.player_killed(id);
                    continue;
                }
            }
            let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&id) else {
                continue;
            };

            // Near-death band (HP < 1): bleed toward the floor, or recover
            // one per tick when aided (`regeneration.md` §5).
            if player.current_hp < 1 {
                if aided {
                    player.current_hp += 1;
                    if player.current_hp > 0 {
                        // Recovered: normal regen resumes, flag clears.
                        if let Some(Session::InGame { aided, .. }) =
                            self.sessions.get_mut(&id)
                        {
                            *aided = false;
                        }
                    }
                } else {
                    player.current_hp -= 1;
                    if player.current_hp <= DEATH_FLOOR {
                        self.player_killed(id);
                    }
                }
                continue;
            }

            // HP: only while alive and below max (0 < HP < max).
            if player.current_hp > 0 && player.current_hp < max_hp {
                let mut base =
                    (i32::from(player.level) + 20) * i32::from(stats.health) / 750;
                if base < 2 {
                    base = 1;
                }
                let pct = bag.value(hp_regen_ability());
                if pct != 0 {
                    base = (pct + 100) * base / 100;
                }
                player.current_hp = (player.current_hp + base).min(max_hp);
            }

            // Mana: gated on current < max (non-casters have max 0).
            if player.current_mana < max_mana {
                let (group, tier) = caster.unwrap_or((0, 0));
                let stat = match group {
                    1 => i32::from(stats.intellect),
                    2 => i32::from(stats.wisdom),
                    3 => (i32::from(stats.wisdom) + i32::from(stats.intellect)) / 2,
                    4 => i32::from(stats.charm),
                    _ => 0,
                };
                let mut regen = (i32::from(player.level) + 20) * stat * (i32::from(tier) + 2)
                    / 1650;
                if group == 5 {
                    regen = 1;
                }
                let pct = bag.value(mana_regen_ability());
                if pct != 0 {
                    regen = (pct + 100) * regen / 100;
                }
                player.current_mana = (player.current_mana + regen).clamp(0, max_mana);
            }
        }
    }

    /// The player half of the ~3 s routine pass (spec §5; decompile
    /// 19807-19830): walk each in-game player's active-spell slots.
    fn upkeep_update(&mut self) {
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in sessions {
            self.upkeep_player(id);
        }
    }

    /// One player's slot walk (decompile 19807-19830, per non-empty slot):
    /// 1. decrement remaining (`+0x68 += -1`);
    /// 2. `perform_routine_spell_player_upkeep` — the recurring per-tick
    ///    effect (runs even on the expiry tick);
    /// 3. at 0 remaining — clear the slot + terminate, chain HONORED
    ///    (19823: chainFlag `'\x01'`);
    /// 4. `check_kill_user` — recurring damage kills mid-loop, ending the
    ///    player's whole pass (19826-19828 returns).
    fn upkeep_player(&mut self, session: SessionId) {
        for idx in 0..10 {
            let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session)
            else {
                // Died out of the loop (permadeath removes the session).
                return;
            };
            let Some(spell_id) = player.active_spells[idx].spell else {
                continue;
            };
            // Unknown spell id (content changed under a save): the DLL
            // gates the slot's ENTIRE processing on get_spell_data
            // (19812-19813) — no decrement, no effect; the slot idles.
            let Some(spell) = self.content.spells.get(&spell_id).cloned() else {
                continue;
            };
            let (max_hp, max_mana) = (derived.max_hp, derived.max_mana);
            let stored = i32::from(player.active_spells[idx].value);
            let mut visible = false;
            let mut crossed_down = false;
            let remaining;
            {
                let Some(Session::InGame { player, energy, .. }) =
                    self.sessions.get_mut(&session)
                else {
                    return;
                };
                player.active_spells[idx].remaining -= 1;
                remaining = player.active_spells[idx].remaining;
                // `perform_routine_spell_player_upkeep` (decompile
                // 44712-44810): value = the STORED slot value unless the
                // ability row carries its own nonzero value (44727-44730
                // — the `+0xa8` per-ability override, spec §5). The DLL
                // refreshes the prompt inside the Damage/Drain/Heal/
                // HealMana handlers (prf_prompt per hit); we fold that
                // into one refresh per slot below — same final line.
                for (ability, row) in &spell.abilities {
                    let v = if *row != 0 { i32::from(*row) } else { stored };
                    match ability {
                        // Damage (1) and Drain (8) are IDENTICAL at the
                        // player tick (44739-44754): HP -= v, prompt,
                        // drop-line when crossing below 1 — spec §5
                        // lists no caster-side heal for Drain here (a
                        // slot has no caster), and neither does the
                        // decompile.
                        Ability::Damage | Ability::Drain => {
                            let was_up = player.current_hp >= 1;
                            player.current_hp -= v;
                            visible = true;
                            if was_up && player.current_hp < 1 {
                                crossed_down = true;
                            }
                        }
                        // EnergyLevel (11): round pool += v, capped at
                        // the pool max (44756-44760).
                        Ability::EnergyLevel => {
                            *energy = (*energy + v).min(PLAYER_ENERGY_MAX);
                        }
                        // Alterhunger (15) / AlterThirst (16): plain adds
                        // (44735-44737, 44767-44769; u16 clamp is ours).
                        Ability::Alterhunger => {
                            player.hunger = clamp_counter(i32::from(player.hunger) + v);
                        }
                        Ability::AlterThirst => {
                            player.thirst = clamp_counter(i32::from(player.thirst) + v);
                        }
                        // Heal (18): HP += v capped at max, prompt
                        // (44770-44776).
                        Ability::Heal => {
                            player.current_hp = (player.current_hp + v).min(max_hp);
                            visible = true;
                        }
                        // Cure Poison (20): poison -= v floor 0. The
                        // decompile's odd guard (44778: `poison - v <
                        // poison`) just skips v <= 0 — a zero/negative
                        // per-tick cure is a no-op here, unlike the
                        // instant path.
                        Ability::CurePoison => {
                            if v > 0 {
                                player.poison =
                                    clamp_poison(i32::from(player.poison) - v);
                            }
                        }
                        // Fear (60): genrdn(0,100) < v => flee a random
                        // exit (44788-44793) — SLICE 5 with the fear
                        // flag; move_user needs the flee plumbing.
                        Ability::Fear => {}
                        // HealMana (150): mana += v, floored at 0 then
                        // capped at max — the DLL's sequential pair
                        // (44795-44805), not a clamp.
                        Ability::HealMana => {
                            player.current_mana += v;
                            if player.current_mana < 0 {
                                player.current_mana = 0;
                            }
                            if player.current_mana > max_mana {
                                player.current_mana = max_mana;
                            }
                            visible = true;
                        }
                        _ => {}
                    }
                }
            }
            if visible {
                self.show_prompt(session);
            }
            if crossed_down {
                // FUN_0043c91d — the drop announce the Damage/Drain
                // handlers fire when HP crosses below 1 (44745-44747),
                // same lines as the monster-swing crossing.
                let name = self.player(session).name.clone();
                let room = self.player(session).location;
                self.output_line(session, &text::drops_to_ground(&name));
                self.broadcast_to_room(room, Some(session), &text::drops_to_ground(&name));
            }
            // Expiry: the DLL checks `== 0` (19819); `<=` also catches a
            // zero/negative entered duration (an AlterSpLength-crushed
            // roll), which the DLL would tick past into a 16-bit wrap.
            if remaining <= 0 {
                self.terminate_active_spell(session, idx, true);
            }
            // check_kill_user (19826-19828): the M3 death path; a kill
            // ends this player's whole pass.
            if self.player(session).current_hp <= DEATH_FLOOR {
                self.player_killed(session);
                return;
            }
        }
    }

    /// Test/inspection accessors for the live player state.
    pub fn current_hp(&self, session: SessionId) -> i32 {
        self.player(session).current_hp
    }

    pub fn current_mana(&self, session: SessionId) -> i32 {
        self.player(session).current_mana
    }

    pub fn set_current_hp(&mut self, session: SessionId, hp: i32) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.current_hp = hp;
        }
    }

    pub fn set_current_mana(&mut self, session: SessionId, mana: i32) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.current_mana = mana;
        }
    }

    /// The poison counter (`+0xbe`; 0 for sessions not in game).
    pub fn poison(&self, session: SessionId) -> i16 {
        match self.sessions.get(&session) {
            Some(Session::InGame { player, .. }) => player.poison,
            _ => 0,
        }
    }

    pub fn set_poison(&mut self, session: SessionId, poison: i16) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.poison = poison;
        }
    }

    /// Test hook: pre-seed an active-spell slot (and recompute, since
    /// slots feed the ability bag).
    pub fn set_active_spell(&mut self, session: SessionId, idx: usize, slot: ActiveSpell) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.active_spells[idx] = slot;
            self.refresh_derived(session);
        }
    }

    /// The derived max HP (0 for sessions not in game).
    pub fn max_hp(&self, session: SessionId) -> i32 {
        match self.sessions.get(&session) {
            Some(Session::InGame { derived, .. }) => derived.max_hp,
            _ => 0,
        }
    }

    /// The remaining round-energy pool (0 for sessions not in game).
    pub fn round_energy(&self, session: SessionId) -> i32 {
        match self.sessions.get(&session) {
            Some(Session::InGame { energy, .. }) => *energy,
            _ => 0,
        }
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    /// Attaches an authenticated session with its loaded player, announces
    /// the entry to everyone else in the game, and shows the player their
    /// room (returning-character entry; ORACLE-VERIFY the exact ordering).
    pub fn attach_player(&mut self, player: Player) -> SessionId {
        let id = self.next_session_id();
        self.broadcast_to_others(id, &text::entered_realm(&player.name));
        let derived = self.derive_for(&player);
        self.sessions
            .insert(id, Session::InGame { player: Box::new(player), derived, exiting: None, target: None, aided: false, energy: PLAYER_ENERGY_MAX, cast_this_round: false, casting: None });
        self.show_room(id);
        self.show_prompt(id);
        id
    }

    /// The player's accumulated ability modifiers: race + class permanents,
    /// worn/wielded gear, and active duration-spell slots — the
    /// `update_dynamic_stats` from-scratch fold.
    fn ability_bag(&self, player: &Player) -> AbilityBag {
        let mut abilities = AbilityBag::default();
        if let Some(race) = self.content.races.get(&player.race) {
            for (ability, value) in &race.abilities {
                abilities.add(*ability, i32::from(*value));
            }
        }
        if let Some(class) = self.content.classes.get(&player.class) {
            for (ability, value) in &class.abilities {
                abilities.add(*ability, i32::from(*value));
            }
        }
        // Worn equipment and the wielded weapon contribute their abilities
        // (update_dynamic_stats folding).
        for (id, _) in player.worn.iter().chain(player.weapon.iter()) {
            if let Some(item) = self.content.items.get(id) {
                for (ability, value) in &item.abilities {
                    abilities.add(*ability, i32::from(*value));
                }
            }
        }
        // Occupied active-spell slots re-apply their spell's ability table
        // on every recompute (spec §4: "The stored (id, value) in the
        // active slot is what makes the effect persist" — when a spell
        // leaves the slots its contribution simply vanishes on the next
        // recompute). A value-0 ability row means "the rolled magnitude":
        // the STORED slot value substitutes, mirroring
        // add_cast_spell_to_user's stored potency; nonzero rows are fixed
        // amounts (the same convention as the instant apply loop).
        // Metadata rows (DescMsg 115, StartMsg 120, RemovesSpell 122,
        // EndCast 151, KillSpell 153, GiveTempSpell 160, CastOnEnd% 164)
        // accumulate harmlessly and are NOT filtered: every bag consumer
        // queries specific ability ids, none of which are metadata — the
        // DLL's update_dynamic_with_ability switch likewise ignores them.
        // EXCEPTION: NegateAbility (124/0x7c) is special-cased by the DLL —
        // skipped in the main fold, then post-passed through
        // negate_dynamic_with_ability on the row's VALUE (decompiled
        // 37743-37745, 37837-37855; spec §4). Five shipped duration spells
        // carry it (497/972 card-angel, 745 sunstone bracelet, 1322 hold
        // immune — with value 0, which substitution would corrupt — and
        // 1323 fear immune). KNOWN-DIVERGENCE: the negation post-pass
        // lands when its targets (HoldPerson/Fear flags) are modeled.
        // Unknown spell ids (content changed under a save) contribute
        // nothing.
        let negate = Ability::from_id(124).expect("NegateAbility in the enum");
        for slot in &player.active_spells {
            let Some(spell) = slot.spell.and_then(|id| self.content.spells.get(&id)) else {
                continue;
            };
            for (ability, value) in &spell.abilities {
                if *ability == negate {
                    continue;
                }
                let v = match *value {
                    0 => i32::from(slot.value),
                    v => i32::from(v),
                };
                abilities.add(*ability, v);
            }
        }
        abilities
    }

    /// The effective stats the DLL keeps at `+0xa2..+0xac`: base stats
    /// plus every accumulated stat-buff ability (Intel 44 .. Charm 49)
    /// from the bag. The DLL direct-writes these six at cast
    /// (cast_no_target 40757-40773: `+0xa2 += v` right after
    /// add_cast_spell_to_user) and reverses them at termination
    /// (perform_spell_termination_player_upkeep 44858-44875); we fold
    /// them from the bag at read time instead, so a cleared slot simply
    /// vanishes on the next recompute with no reversal bookkeeping.
    /// KNOWN-DIVERGENCE (mechanism, not outcome): the DLL re-applies the
    /// direct write on every refresh recast WITHOUT reversing the
    /// previous one — its stat buffs stack across recasts until a single
    /// stored-value reversal at termination; the bag contributes exactly
    /// once per slot. Direct readers of `player.stats` not yet folded
    /// (combat fighter builders, player_energy_used, the CHA shop-price
    /// haggle) keep reading base stats — no stat-buff duration spell is
    /// castable before bard support (all 12 learnable carriers are bard
    /// songs), and the one shipped stat ITEM (331 "Indiana Jones hat",
    /// (Charm 49, 30) per the loader's abilitya/abilityb pairing —
    /// checked against the content DB, its other rows are
    /// LoyalItem/AC/Shadow) awaits the same sweep. Values clamp at 0:
    /// our fields are u16 where the DLL's signed shorts can go negative.
    fn effective_stats(&self, player: &Player, bag: &AbilityBag) -> StatBlock {
        let fold = |base: u16, ability: Ability| -> u16 {
            clamp_counter(i32::from(base) + bag.value(ability))
        };
        StatBlock {
            intellect: fold(player.stats.intellect, Ability::Intel),
            wisdom: fold(player.stats.wisdom, Ability::Wisdom),
            strength: fold(player.stats.strength, Ability::Strength),
            health: fold(player.stats.health, Ability::Health),
            agility: fold(player.stats.agility, Ability::Agility),
            charm: fold(player.stats.charm, Ability::Charm),
        }
    }

    /// `update_dynamic_stats` + `calculate_secondary_stats`.
    fn derive_for(&self, player: &Player) -> Derived {
        let abilities = self.ability_bag(player);
        let stats = self.effective_stats(player, &abilities);
        let class = self.content.classes.get(&player.class);
        let race_hp = self
            .content
            .races
            .get(&player.race)
            .map_or(0, |r| i32::from(r.hp_per_level));
        derive(&StatInputs {
            level: i32::from(player.level),
            stats,
            health_base: i32::from(player.base_stats.health),
            hp_base: i32::from(player.hp_base),
            class_hp_per_level: class.map_or(0, |c| i32::from(c.hp_per_level)),
            race_hp_per_level: race_hp,
            caster_group: class.map_or(0, |c| i32::from(c.caster_group)),
            casting_factor: class.map_or(0, |c| i32::from(c.casting_factor)),
            abilities,
        })
    }

    fn show_prompt(&mut self, session: SessionId) {
        let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session) else {
            return;
        };
        let caster_group = self
            .content
            .classes
            .get(&player.class)
            .map_or(0, |c| c.caster_group);
        let prompt = text::prompt(
            player.current_hp,
            player.current_mana,
            derived.max_mana,
            caster_group,
        );
        self.output(session, &prompt);
    }

    fn show_sheet(&mut self, session: SessionId) {
        let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session) else {
            return;
        };
        let race = self
            .content
            .races
            .get(&player.race)
            .map_or("", |r| r.name.as_str());
        let class = self
            .content
            .classes
            .get(&player.class)
            .map_or("", |c| c.name.as_str());
        // Each active duration spell's DescMsg (115) line3, appended after
        // the MagicRes row (MEASURED §8.11: "You are blurred!" while
        // active, gone after expiry). DescMsg-less spells add no line.
        // ORACLE-VERIFY: multi-buff ordering unmeasured live; slot order
        // chosen (the DLL iterates the slot array).
        let active_lines: Vec<String> = player
            .active_spells
            .iter()
            .filter_map(|s| s.spell)
            .filter_map(|id| self.content.spells.get(&id))
            .filter_map(|spell| {
                spell
                    .abilities
                    .iter()
                    .find_map(|(a, v)| (*a == Ability::DescMsg).then_some(*v))
            })
            .filter_map(|v| u16::try_from(v).ok())
            .filter_map(|id| self.content.messages.get(&crate::content::MessageId(id)))
            // ORACLE-VERIFY: skipping empty/missing DescMsg line3 is
            // inferred, not measured (spell 776 ships a (DescMsg, 0) row).
            .filter_map(|m| m.lines.get(2).filter(|l| !l.is_empty()).cloned())
            .collect();
        // The sheet's six stat rows show the EFFECTIVE stats (+0xa2..;
        // buffs included — the DLL direct-writes them, we fold from the
        // bag, see effective_stats).
        let bag = self.ability_bag(player);
        let stats = self.effective_stats(player, &bag);
        let sheet = text::stat_sheet(&text::SheetData {
            name: &player.name,
            race,
            class,
            level: player.level,
            lives: player.lives,
            cp: player.cp_unspent,
            experience: player.experience,
            hp_current: player.current_hp,
            hp_max: derived.max_hp,
            armour_class: 0, // get_armour_rating: no equipment until M4
            armour_max: 0,
            stats,
            derived,
            active_lines: &active_lines,
            mana_current: player.current_mana,
            mana_max: derived.max_mana,
            caster_group: self
                .content
                .classes
                .get(&player.class)
                .map_or(0, |c| c.caster_group),
        });
        self.output(session, &sheet);
    }

    /// Attaches an authenticated session that has no saved character yet and
    /// starts the creation flow (race choice first — the name is the account
    /// handle and gender is inherited from the account, per spec §1/§5).
    pub fn attach_account(&mut self, profile: AccountProfile) -> SessionId {
        let id = self.next_session_id();
        self.show_race_list(id);
        self.sessions.insert(id, Session::ChoosingRace { profile });
        id
    }

    /// Feeds one line of player input. Input from unknown (never attached or
    /// already disconnected) sessions is dropped.
    pub fn input(&mut self, session: SessionId, line: &str) {
        match self.sessions.get(&session) {
            None => {}
            Some(Session::ChoosingRace { .. }) => self.choose_race(session, line),
            Some(Session::ChoosingClass { .. }) => self.choose_class(session, line),
            Some(Session::ChoosingLawful { .. }) => self.choose_lawful(session, line),
            // Oracle: commands during exit meditation are refused.
            Some(Session::InGame { exiting: Some(_), .. }) => {
                self.output_line(session, text::MEDITATION_BLOCKED);
            }
            Some(Session::InGame { .. }) => self.game_command(session, line),
        }
    }

    fn game_command(&mut self, session: SessionId, line: &str) {
        match parse(line) {
            Command::Quit => self.quit(session),
            // Blank input re-shows the room without its description (oracle).
            Command::Blank => self.show_room_brief(session),
            Command::Look => self.show_room(session),
            Command::Exits => self.show_exits_line(session),
            Command::Help => self.output_line(session, text::HELP_BANNER),
            Command::Top => self.output_line(session, text::TOP_HEADER),
            Command::Get(target) => {
                if target.trim().is_empty() {
                    self.output_line(session, text::SYNTAX_GET);
                } else if self.get_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Drop(target) => {
                if self.drop_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Inventory => self.show_inventory(session),
            Command::List => {
                if self.list_command(session) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Buy(target) => {
                if self.buy_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Sell(target) => {
                if self.sell_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Deposit(amount) => self.deposit_command(session, &amount),
            Command::Withdraw(amount) => self.withdraw_command(session, &amount),
            Command::Balance => self.balance_command(session),
            Command::Arm(target) => {
                if self.arm_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Wear(target) => {
                if self.wear_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Remove(target) => {
                if self.remove_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Use(target) => {
                if self.use_command(session, &target, false) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Read(target) => {
                if self.use_command(session, &target, true) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Status => self.show_sheet(session),
            Command::Experience => self.show_experience(session),
            Command::Health => self.show_health(session),
            Command::Spells => self.spells_command(session),
            Command::Powers => self.powers_command(session),
            Command::Train => self.train_level(session),
            // Argument commands do best-effort resolution; when they cannot
            // intuit the target, the whole line is said aloud (the parser's
            // universal fallback — applies to every future argument command).
            Command::Attack(target) => {
                if self.attack_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            // Cast never falls through to say: an unresolvable spell prints
            // the do-not-know line (MEASURED §8.6/§8.9). Invoke is its kai
            // twin (§8.12).
            Command::Cast(args) => self.cast_command(session, &args),
            Command::Invoke(args) => self.invoke_command(session, &args),
            Command::Aid(target) => {
                if self.aid_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
            Command::Move(direction) => {
                // Oracle: the downed band blocks movement (look/health work).
                if self.player(session).current_hp < 1 {
                    self.output_line(session, text::MORTALLY_WOUNDED);
                } else {
                    self.move_player(session, direction);
                }
            }
            // Type-10 action exits trigger on their phrases; anything else
            // is said aloud (oracle) - there is no error reply.
            Command::Unknown(what) => {
                if self.try_action_exit(session, &what) == Resolution::FallThrough {
                    self.say(session, &what);
                }
            }
        }
        self.show_prompt(session);
    }

    /// The exp base for the curve: `class.exp_base + race.exp_chart`.
    fn exp_base(&self, player: &Player) -> u64 {
        let class = self
            .content
            .classes
            .get(&player.class)
            .map_or(0, |c| i64::from(c.exp_base));
        let race = self
            .content
            .races
            .get(&player.race)
            .map_or(0, |r| i64::from(r.exp_chart));
        (class + race).max(0) as u64
    }

    fn show_experience(&mut self, session: SessionId) {
        let player = self.player(session);
        let needed = crate::stats::exp_needed(player.level, self.exp_base(player));
        let line = text::exp_line(player.experience, player.level, needed);
        self.output_line(session, &line);
    }

    fn show_health(&mut self, session: SessionId) {
        let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session) else {
            return;
        };
        let caster_group = self
            .content
            .classes
            .get(&player.class)
            .map_or(0, |c| c.caster_group);
        let line = text::health_line(
            player.current_hp,
            derived.max_hp,
            player.current_mana,
            derived.max_mana,
            caster_group,
        );
        self.output_line(session, &line);
    }

    /// `spells` — the learned-book listing (spellcasting.md §8.5). Rows
    /// sort by required power ascending, then name; display order is
    /// computed here, not stored. Format VERIFIED oracle_spell_train.raw.
    fn spells_command(&mut self, session: SessionId) {
        // MEASURED (§8.12): mystics are redirected before any listing.
        if self.is_kai(session) {
            self.output_line(session, text::KAI_NO_SPELLS);
            return;
        }
        let player = self.player(session);
        let mut known: Vec<_> = player
            .spellbook
            .keys()
            .filter_map(|id| self.content.spells.get(id))
            .collect();
        if known.is_empty() {
            self.output_line(session, text::NO_SPELLS);
            return;
        }
        known.sort_by(|a, b| {
            (a.required_power, &a.name).cmp(&(b.required_power, &b.name))
        });
        let mut out = String::from(text::SPELLS_HEADER);
        out.push('\n');
        for spell in known {
            out.push_str(&text::spell_row(
                spell.required_power,
                spell.mana_cost,
                &spell.short_name,
                &spell.name,
            ));
            out.push('\n');
        }
        // The table ends with a blank line (oracle; unlike exp/health).
        out.push('\n');
        self.output(session, &out);
    }

    /// `powers` — the kai book listing (MEASURED §8.12): same shape as
    /// `spells` (sort, trailing blank line) with the Kai header and the
    /// right-aligned short column; empty book is a single line.
    fn powers_command(&mut self, session: SessionId) {
        if !self.is_kai(session) {
            // ORACLE-VERIFY: unmeasured parallel of the kai redirect.
            self.output_line(session, text::NON_KAI_NO_POWERS);
            return;
        }
        let player = self.player(session);
        let mut known: Vec<_> = player
            .spellbook
            .keys()
            .filter_map(|id| self.content.spells.get(id))
            .collect();
        if known.is_empty() {
            self.output_line(session, text::NO_POWERS);
            return;
        }
        known.sort_by(|a, b| {
            (a.required_power, &a.name).cmp(&(b.required_power, &b.name))
        });
        let mut out = String::from(text::POWERS_HEADER);
        out.push('\n');
        for spell in known {
            out.push_str(&text::power_row(
                spell.required_power,
                spell.mana_cost,
                &spell.short_name,
                &spell.name,
            ));
            out.push('\n');
        }
        out.push('\n');
        self.output(session, &out);
    }

    /// `train_level` (`leveling.md` §4). Gate order is oracle-confirmed:
    /// location, class, level band, cap, experience, money.
    fn train_level(&mut self, session: SessionId) {
        let player = self.player(session);
        let shop = self.content.rooms[&player.location]
            .shop
            .and_then(|id| self.content.shops.get(&id));
        let Some(shop) = shop.filter(|s| s.shop_type == 8) else {
            self.output_line(session, text::TRAIN_WRONG_ROOM);
            return;
        };
        if shop.class_limit != 0 && shop.class_limit as u16 != player.class.0 {
            self.output_line(session, text::TRAIN_WRONG_ROOM);
            return;
        }
        let next = player.level + 1;
        if i32::from(next) < i32::from(shop.min_level) {
            // ORACLE-VERIFY exact wording.
            self.output_line(session, "You have not progressed far enough to use this trainer!");
            return;
        }
        if i32::from(next) > i32::from(shop.max_level) {
            // ORACLE-VERIFY exact wording.
            self.output_line(session, "You have progressed too far to use this trainer!");
            return;
        }
        if player.level >= self.config.level_cap {
            // ORACLE-VERIFY exact wording (DAT_00482d90 gate).
            self.output_line(session, "You may not train any further!");
            return;
        }
        let needed = crate::stats::exp_needed(player.level, self.exp_base(player));
        if player.experience < needed {
            self.output_line(session, text::TRAIN_NO_EXP);
            return;
        }
        let cost = ((i64::from(shop.markup) + 100).max(0) as u64)
            * u64::from(player.level)
            * 5
            / 100;
        let ratios = self.config.coin_ratios;
        if self.player(session).coins.total_copper(ratios) < cost {
            self.output_line(session, text::TRAIN_NO_MONEY);
            return;
        }

        // All gates passed: pay, level, grant CP, roll HP-base, add lives.
        let hp_seed = self
            .content
            .classes
            .get(&self.player(session).class)
            .map_or(0, |c| i32::from(c.hp_seed));
        let roll = self.rng.roll(0, hp_seed);
        let lives_grant = self.config.lives_per_level;
        // Kai grant (MEASURED §8.12): training a caster_group-5 class
        // inserts every magery-group-5 power whose required_power equals
        // the NEW level into the ordinary spellbook (swan at L2, owl at
        // L3 — the shipped data has exactly one power per level). The
        // §8.1 mage control measured NOTHING, so the grant is keyed on
        // the class group. ORACLE-VERIFY: classes other than mage/mystic
        // are unmeasured; multi-grant ordering (no shipped case) is by
        // spell id.
        let next_level = self.player(session).level + 1;
        let grants: Vec<(SpellId, String)> = if self.caster_group(session) == 5 {
            let mut grants: Vec<(SpellId, String)> = self
                .content
                .spells
                .values()
                .filter(|s| {
                    s.class_gate_group == 5
                        && i32::from(s.required_power) == i32::from(next_level)
                })
                .map(|s| (s.id, s.name.clone()))
                .collect();
            grants.sort_by_key(|(id, _)| *id);
            grants
        } else {
            Vec::new()
        };
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            unreachable!("train dispatched from in-game session");
        };
        let spent = player.coins.deduct_copper(cost, ratios);
        player.level += 1;
        let new_level = player.level;
        let cp = match new_level {
            0..=10 => 10,
            11..=20 => 15,
            l => (l - 1) / 10 * 5 + 10,
        };
        player.cp_unspent += cp;
        player.cp_lifetime += cp;
        if roll > 0 {
            player.hp_base += roll as u16;
        }
        player.lives = (player.lives + lives_grant).min(9);
        for (id, _) in &grants {
            // Permanent book entry — the same insertion the scroll path
            // uses (§8.12: the on-disk +0x474 word array gained the id).
            player.spellbook.insert(*id, false);
        }

        // Mode-2 recompute: derived stats refresh, current HP/mana kept
        // (MEASURED §8.12: training does NOT refill the kai pool).
        let refreshed = self.derive_for(self.player(session));
        if let Some(Session::InGame { derived, .. }) = self.sessions.get_mut(&session) {
            *derived = refreshed;
        }
        let snapshot = Box::new(self.player(session).clone());
        self.events.push(Event::Persist(snapshot));
        // The receipt (MEASURED §8.7/§8.12): payment sentence with the
        // coins actually handed over, the CP line, then the kai grants —
        // one power name per line.
        let coins = text::coin_listing([
            spent.copper,
            spent.silver,
            spent.gold,
            spent.platinum,
            spent.runic,
        ])
        .unwrap_or_else(|| "nothing".into());
        let mut out = text::train_hand_over(&coins, new_level);
        out.push('\n');
        out.push_str(text::TRAIN_RECEIVE_HEADER);
        out.push('\n');
        out.push_str(&text::train_cp_line(cp));
        out.push('\n');
        if !grants.is_empty() {
            out.push_str(text::KAI_LEARN_HEADER);
            out.push('\n');
            for (_, name) in &grants {
                out.push_str(name);
                out.push('\n');
            }
        }
        self.output(session, &out);
    }


    // ------------------------------------------------------------------
    // Combat (`combat_rounds.md`; message formats from the oracle).
    // ------------------------------------------------------------------

    /// `engage_autocombat` + `restart_autocombat`: set the target and run
    /// the opening swing sequence immediately. Flexible syntax (player
    /// testimony): no argument auto-picks; an unresolvable target falls
    /// through to SAY (oracle-observed).
    fn attack_command(&mut self, session: SessionId, target_words: &str) -> Resolution {
        if self.player(session).current_hp < 1 {
            self.output_line(session, text::MORTALLY_WOUNDED);
            return Resolution::Handled;
        }
        let room = self.player(session).location;
        let monster = if target_words.trim().is_empty() {
            self.auto_pick_target(session, room)
        } else {
            self.find_monster(room, target_words)
        };
        let Some(monster) = monster else {
            return Resolution::FallThrough;
        };
        if let Some(Session::InGame { target, casting, .. }) = self.sessions.get_mut(&session) {
            // Oracle: attacking while already engaged prints *Combat Off*
            // before the new *Combat Engaged*.
            *casting = None; // a melee attack replaces any cast engagement
            if target.is_some() {
                *target = None;
                self.output_line(session, text::COMBAT_OFF);
            }
        }
        if let Some(Session::InGame { target, .. }) = self.sessions.get_mut(&session) {
            *target = Some(monster);
        }
        self.output_line(session, text::COMBAT_ENGAGED);
        self.player_attack_sequence(session);
        Resolution::Handled
    }

    /// Bare `a`: current combat target first, then whoever is attacking us,
    /// then the first live monster in the room (priority ORACLE-VERIFY).
    fn auto_pick_target(&self, session: SessionId, room: RoomId) -> Option<MonsterInstanceId> {
        if let Some(Session::InGame { target: Some(t), .. }) = self.sessions.get(&session)
            && self
                .monsters
                .get(t)
                .is_some_and(|m| m.current_hp > 0 && m.location == room)
        {
            return Some(*t);
        }
        if let Some((id, _)) = self
            .monsters
            .iter()
            .find(|(_, m)| m.location == room && m.current_hp > 0 && m.target == Some(session))
        {
            return Some(*id);
        }
        self.monsters
            .iter()
            .find(|(_, m)| m.location == room && m.current_hp > 0)
            .map(|(id, _)| *id)
    }

    /// `cmd_aid`: set the aided flag on a downed co-located player. An
    /// unresolvable (or empty) name falls through to say.
    fn aid_command(&mut self, session: SessionId, target_words: &str) -> Resolution {
        let room = self.player(session).location;
        let want = target_words.trim().to_ascii_lowercase();
        if want.is_empty() {
            // Oracle: bare "ai" prints the syntax line.
            self.output_line(session, text::SYNTAX_AID);
            return Resolution::Handled;
        }
        let target = self
            .in_game_sessions()
            .filter(|(id, p)| *id != session && p.location == room)
            .find(|(_, p)| p.name.to_ascii_lowercase().starts_with(&want))
            .map(|(id, p)| (id, p.name.clone(), p.current_hp));
        let Some((target_id, name, hp)) = target else {
            return Resolution::FallThrough;
        };
        if hp >= 1 {
            // DLL: "is in no need of assistance".
            self.output_line(session, &format!("{name} is in no need of assistance."));
            return Resolution::Handled;
        }
        if let Some(Session::InGame { aided, .. }) = self.sessions.get_mut(&target_id) {
            *aided = true;
        }
        // DLL fragment: "You have aided %s; %s's wounds are [bound]".
        self.output_line(
            session,
            &format!("You have aided {name}; {name}'s wounds are bound."),
        );
        Resolution::Handled
    }

    /// `arm`/`wield`/`equip`: hold a weapon from inventory; any current
    /// weapon returns to the pack.
    fn arm_command(&mut self, session: SessionId, target: &str) -> Resolution {
        let want = target.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Resolution::FallThrough;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::FallThrough;
        };
        let pos = player.inventory.iter().position(|(id, _)| {
            self.content
                .items
                .get(id)
                .is_some_and(|i| i.item_type == 1 && word_prefix_match(&i.name, &want))
        });
        let Some(pos) = pos else {
            self.output_line(session, &text::not_unequipped(target.trim()));
            return Resolution::Handled;
        };
        {
            let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
                return Resolution::FallThrough;
            };
            let item = &self.content.items[&player.inventory[pos].0];
            if !self.user_can_use(player, item) {
                self.output_line(session, text::MAY_NOT_USE_WEAPON);
                return Resolution::Handled;
            }
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::FallThrough;
        };
        let entry = player.inventory.remove(pos);
        if let Some(old) = player.weapon.take() {
            player.inventory.push(old);
        }
        player.weapon = Some(entry);
        let name = self.content.items[&entry.0].name.clone();
        self.refresh_derived(session);
        self.output_line(session, &text::now_holding(&name));
        Resolution::Handled
    }

    /// `wear`: move armor from inventory to a worn slot, displacing a
    /// same-location piece (dual-slot pairs 4/0xd and 0xe/0x11 allow two).
    fn wear_command(&mut self, session: SessionId, target: &str) -> Resolution {
        let want = target.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Resolution::FallThrough;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::FallThrough;
        };
        let pos = player.inventory.iter().position(|(id, _)| {
            self.content
                .items
                .get(id)
                .is_some_and(|i| i.worn_on != 0 && word_prefix_match(&i.name, &want))
        });
        let Some(pos) = pos else {
            self.output_line(session, &text::not_unequipped(target.trim()));
            return Resolution::Handled;
        };
        let location = {
            let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
                return Resolution::FallThrough;
            };
            let id = player.inventory[pos].0;
            let item = &self.content.items[&id];
            if !self.user_can_use(player, item) {
                self.output_line(session, text::MAY_NOT_WEAR);
                return Resolution::Handled;
            }
            item.worn_on
        };
        let dual = |a: i16, b: i16| (a == 4 || a == 0xd) && (b == 4 || b == 0xd)
            || (a == 0xe || a == 0x11) && (b == 0xe || b == 0x11);
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::FallThrough;
        };
        // Count/displace same-location items.
        let same: Vec<usize> = player
            .worn
            .iter()
            .enumerate()
            .filter(|(_, (id, _))| {
                self.content
                    .items
                    .get(id)
                    .is_some_and(|i| i.worn_on == location || dual(i.worn_on, location))
            })
            .map(|(i, _)| i)
            .collect();
        let allowed_two = dual(location, location);
        if (allowed_two && same.len() >= 2) || (!allowed_two && !same.is_empty()) {
            let displaced = player.worn.remove(same[0]);
            player.inventory.push(displaced);
        }
        let entry = player.inventory.remove(
            player
                .inventory
                .iter()
                .position(|(id, _)| {
                    self.content
                        .items
                        .get(id)
                        .is_some_and(|i| i.worn_on != 0 && word_prefix_match(&i.name, &want))
                })
                .expect("checked above"),
        );
        player.worn.push(entry);
        let name = self.content.items[&entry.0].name.clone();
        self.refresh_derived(session);
        self.output_line(session, &text::now_wearing(&name));
        Resolution::Handled
    }

    /// `remove`: worn armor back to the pack.
    fn remove_command(&mut self, session: SessionId, target: &str) -> Resolution {
        let want = target.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Resolution::FallThrough;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::FallThrough;
        };
        let pos = player.worn.iter().position(|(id, _)| {
            self.content
                .items
                .get(id)
                .is_some_and(|i| word_prefix_match(&i.name, &want))
        });
        let Some(pos) = pos else {
            self.output_line(session, &text::not_wearing(target.trim()));
            return Resolution::Handled;
        };
        let entry = player.worn.remove(pos);
        player.inventory.push(entry);
        let name = self.content.items[&entry.0].name.clone();
        self.refresh_derived(session);
        self.output_line(session, &text::removed_item(&name));
        Resolution::Handled
    }

    /// The 2nd-person verb pools for the wielded weapon (pipe-separated
    /// message line 1; fists default to punch / "swing at").
    fn weapon_verbs(&self, session: SessionId) -> (Vec<String>, Vec<String>) {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return (vec![], vec![]);
        };
        let pool = |msg: Option<crate::content::MessageId>| -> Vec<String> {
            msg.and_then(|m| self.content.messages.get(&m))
                .and_then(|m| m.lines.first())
                .map(|line| line.split('|').map(str::to_owned).collect())
                .unwrap_or_default()
        };
        match player
            .weapon
            .and_then(|(id, _)| self.content.items.get(&id))
        {
            Some(weapon) => (pool(weapon.hit_msg), pool(weapon.miss_msg)),
            None => (vec!["punch".into()], vec!["swing at".into()]),
        }
    }

    /// Recompute cached derived stats after equipment changes.
    fn refresh_derived(&mut self, session: SessionId) {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return;
        };
        let refreshed = self.derive_for(player);
        if let Some(Session::InGame { derived, .. }) = self.sessions.get_mut(&session) {
            *derived = refreshed;
        }
    }

    /// Test hook: the defender fighter view.
    pub fn defender_debug(&self, session: SessionId) -> crate::combat::Fighter {
        self.build_player_defender(session)
    }

    /// The bank in the player's room (shop type 7), if any.
    fn bank_here(&self, session: SessionId) -> Option<crate::content::ShopId> {
        let shop = self.shop_here(session)?;
        (self.content.shops.get(&shop)?.shop_type == 7).then_some(shop)
    }

    fn balance_command(&mut self, session: SessionId) {
        let Some(bank) = self.bank_here(session) else {
            // Oracle: balance outside a bank prints nothing.
            return;
        };
        let shop = &self.content.shops[&bank];
        let balance = self
            .player(session)
            .bankbooks
            .iter()
            .find(|(id, _)| *id == bank.0)
            .map_or(0, |(_, b)| *b);
        let ratios = self.config.coin_ratios;
        let line = text::balance_lines(&shop.name, bank.0, balance, ratios[0] * ratios[1]);
        self.output(session, &format!("{line}\n"));
    }

    fn deposit_command(&mut self, session: SessionId, amount: &str) {
        let Some(bank) = self.bank_here(session) else {
            self.output_line(session, text::NOT_IN_BANK_DEPOSIT);
            return;
        };
        let Ok(amount) = amount.trim().parse::<u64>() else {
            self.output_line(session, text::UNREASONABLE_AMOUNT);
            return;
        };
        if amount == 0 {
            self.output_line(session, text::UNREASONABLE_AMOUNT);
            return;
        }
        let ratios = self.config.coin_ratios;
        if self.player(session).coins.total_copper(ratios) < amount {
            // ORACLE-VERIFY: over-deposit wording (silent like withdraw?).
            return;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        let spent = player.coins.deduct_copper(amount, ratios);
        match player.bankbooks.iter_mut().find(|(id, _)| *id == bank.0) {
            Some((_, b)) => *b += amount,
            None => player.bankbooks.push((bank.0, amount)),
        }
        // Report the actual coins handed over (deduct_currency tell line).
        let coins = text::coin_listing([
            spent.copper,
            spent.silver,
            spent.gold,
            spent.platinum,
            spent.runic,
        ])
        .unwrap_or_else(|| "nothing".into());
        self.output_line(session, &text::deposited(&coins));
    }

    fn withdraw_command(&mut self, session: SessionId, amount: &str) {
        let Some(bank) = self.bank_here(session) else {
            self.output_line(session, text::NOT_IN_BANK_WITHDRAW);
            return;
        };
        let Ok(amount) = amount.trim().parse::<u64>() else {
            self.output_line(session, text::UNREASONABLE_AMOUNT);
            return;
        };
        if amount == 0 {
            self.output_line(session, text::UNREASONABLE_AMOUNT);
            return;
        }
        let ratios = self.config.coin_ratios;
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        let Some((_, balance)) = player.bankbooks.iter_mut().find(|(id, _)| *id == bank.0)
        else {
            // Oracle: over-withdrawal is silent.
            return;
        };
        if *balance < amount {
            return; // silent (oracle)
        }
        *balance -= amount;
        player.coins.add_copper_and_mint(amount, ratios);
        self.output_line(session, &text::withdrew(amount));
    }

    /// The shop in the player's room, if any.
    fn shop_here(&self, session: SessionId) -> Option<crate::content::ShopId> {
        let room = self.player(session).location;
        let room = self.content.rooms.get(&room)?;
        // Only rooms with type 1 are shop-active (room+0x43c == 1): the
        // Silvermere Temple Healer (type 3) carries an inert shopnum.
        if room.room_type != 1 {
            return None;
        }
        room.shop
    }

    /// `user_can_use` (0x1fced): class/race allowlists (a match bypasses
    /// the permission matrix), AntiMagic vs Magical, MinLevel/MaxLevel
    /// abilities, then the class weapon/armour matrix. The alignment
    /// ability gates (Good/Evil/Neutral vs legal level) await the crime
    /// system — no legal points exist yet.
    fn user_can_use(&self, player: &Player, item: &crate::content::Item) -> bool {
        // Item type 0xb requires a class from a config list; no type-11
        // items ship in the 1.11p data.
        if item.item_type == 11 {
            return false;
        }
        let Some(class) = self.content.classes.get(&player.class) else {
            return false;
        };
        let bag = self.ability_bag(player);
        if bag.value(Ability::AntiMagic) != 0
            && item.abilities.iter().any(|(a, _)| *a == Ability::Magical)
        {
            return false;
        }
        for (ability, value) in &item.abilities {
            match ability {
                Ability::MinLevel if i32::from(player.level) < i32::from(*value) => {
                    return false;
                }
                Ability::MaxLevel if i32::from(*value) < i32::from(player.level) => {
                    return false;
                }
                _ => {}
            }
        }
        let mut listed = false;
        let mut allowed = false;
        if !item.classes.is_empty() {
            listed = true;
            if item.classes.contains(&player.class) {
                allowed = true;
            } else {
                return false;
            }
        }
        if !item.races.is_empty() {
            listed = true;
            if item.races.contains(&player.race) {
                allowed = true;
            } else {
                return false;
            }
        }
        if listed && allowed {
            return true; // explicit allowlist match bypasses the matrix
        }
        match item.item_type {
            0 => {
                if class.armour_code < item.armour_req {
                    return false;
                }
                // Weapon codes 0/2/4 also forbid the Off-Hand slot.
                if matches!(class.weapon_code, 0 | 2 | 4) && item.worn_on == 12 {
                    return false;
                }
                true
            }
            1 => match class.weapon_code {
                8 => true,
                9 => {
                    // Config triple: quarterstaff (100), dagger (68), 0.
                    item.id == crate::content::ItemId(100)
                        || item.id == crate::content::ItemId(68)
                }
                7 => matches!(item.weapon_type, 0 | 1),
                6 => matches!(item.weapon_type, 2 | 3),
                5 => matches!(item.weapon_type, 1 | 3),
                4 => matches!(item.weapon_type, 0 | 2),
                code => i32::from(item.weapon_type) == i32::from(code),
            },
            _ => true,
        }
    }

    /// Spell learnability/usability gates 1-2 (spellcasting.md §2 + §8.3):
    /// a gated spell (group != 0) needs the class's magery group to match
    /// AND the class casting factor to reach the spell's required class
    /// level; then the character level must reach `required_power` — the
    /// oracle-proven level (not Spellcasting) gate. A player whose class
    /// id resolves to nothing can't cast, matching `user_can_use`.
    pub fn spell_gate(&self, player: &Player, spell: &crate::content::Spell) -> SpellGate {
        let Some(class) = self.content.classes.get(&player.class) else {
            return SpellGate::WrongClass;
        };
        if spell.class_gate_group != 0
            && (class.caster_group != spell.class_gate_group
                || class.casting_factor < spell.required_class_level)
        {
            return SpellGate::WrongClass;
        }
        if i32::from(player.level) < i32::from(spell.required_power) {
            return SpellGate::TooPowerful;
        }
        SpellGate::Ok
    }

    /// Resolves a cast argument against the learned book (MEASURED,
    /// spellcasting.md §8.9): candidates are the player's spellbook only.
    /// A spell matches on exact shortname (whole first word) OR per-word
    /// name prefix (each typed word prefixes a name word — the item/monster
    /// `word_prefix_match`, reused verbatim). The name match is greedy:
    /// the longest run of leading words that still matches is consumed
    /// (`c magic mi` consumes both words), and the ENTIRE remainder is
    /// returned as the target string, spacing preserved. Ambiguity resolves
    /// to the first match in book (spell-id) order — ORACLE-VERIFY: the
    /// original's tie-break is unmeasured. ORACLE-VERIFY: word_prefix_match
    /// anchors typed words at ANY starting name word (`c missile` resolves
    /// magic missile here); every §8.9 probe was leading-anchored, so
    /// mid-name anchoring is unmeasured for spells (probe: `c missile`,
    /// `c mi`).
    pub fn resolve_spell_from_book(
        &self,
        player: &Player,
        args: &str,
    ) -> Option<(SpellId, String)> {
        // Leading words with their end offsets, so the remainder keeps the
        // caller's exact spacing.
        let mut words: Vec<(&str, usize)> = Vec::new();
        let mut pos = 0;
        for w in args.split_whitespace() {
            let start = pos + args[pos..].find(w).expect("word came from args");
            pos = start + w.len();
            words.push((w, pos));
        }
        let spells: Vec<&crate::content::Spell> = player
            .spellbook
            .keys()
            .filter_map(|id| self.content.spells.get(id))
            .collect();
        for take in (1..=words.len()).rev() {
            let typed = words[..take]
                .iter()
                .map(|(w, _)| w.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            for spell in &spells {
                let hit = word_prefix_match(&spell.name, &typed)
                    || (take == 1 && spell.short_name.eq_ignore_ascii_case(&typed));
                if hit {
                    let target = args[words[take - 1].1..].trim().to_string();
                    return Some((spell.id, target));
                }
            }
        }
        None
    }

    /// `cast` — parsing, book resolution (spellcasting.md §8.9) and the
    /// slice-3 gates in spec §3 order. Missing spec-§3 gates that join with
    /// their systems later, in order: confusion, fear, MageBind.
    fn cast_command(&mut self, session: SessionId, args: &str) {
        // Gate 1: the downed band blocks casting like attack/movement.
        if self.player(session).current_hp < 1 {
            self.output_line(session, text::MORTALLY_WOUNDED);
            return;
        }
        // Kai-block (decompile cast_no_target 39147, after the downed gate
        // 39125): MEASURED §8.12 — bare, garbage, known-power and
        // full-name forms all refuse before any argument parsing.
        if self.is_kai(session) {
            self.output_line(session, text::KAI_NO_CAST);
            return;
        }
        let args = args.trim();
        if args.is_empty() {
            // MEASURED (§8.9): bare cast is a syntax line, not an error.
            self.output_line(session, text::SYNTAX_CAST);
            return;
        }
        self.cast_resolved(session, args);
    }

    /// `invoke` — the kai cast verb (§8.12): same pipeline, kai wording.
    /// The refusal/resolution strings inside are shared with cast (the
    /// unknown line still says "cast", MEASURED); only the mana and
    /// one-per-round refusals swap to the kai variants, via the keyed
    /// helpers.
    fn invoke_command(&mut self, session: SessionId, args: &str) {
        // ORACLE-VERIFY: the downed-band ordering is unmeasured for
        // invoke; mirrored from cast (the DLL gates sit in the shared
        // cast_no_target).
        if self.player(session).current_hp < 1 {
            self.output_line(session, text::MORTALLY_WOUNDED);
            return;
        }
        if !self.is_kai(session) {
            // ORACLE-VERIFY: unmeasured parallel of the kai cast refusal.
            self.output_line(session, text::NON_KAI_NO_INVOKE);
            return;
        }
        let args = args.trim();
        if args.is_empty() {
            // MEASURED (§8.12): bare invoke is a syntax line.
            self.output_line(session, text::SYNTAX_INVOKE);
            return;
        }
        self.cast_resolved(session, args);
    }

    /// The shared cast/invoke pipeline, after the verb-specific gates.
    fn cast_resolved(&mut self, session: SessionId, args: &str) {
        // Gate 2: resolve against the learned book. Resolution comes BEFORE
        // the already-cast check: the DLL dispatcher resolves the spell and
        // passes a pointer into cast_no_target, whose mode-2 flag "skips
        // confusion/fear/round checks" — so the round gate lives inside,
        // after resolution. ORACLE-VERIFY: second-cast-unknown ordering
        // unmeasured (probe: `c blur` then `c zzz` in one round).
        let resolved = self.resolve_spell_from_book(self.player(session), args);
        let Some((spell_id, target)) = resolved else {
            self.output_line(session, &text::dont_know_cast(args));
            return;
        };
        // Gate 3: one cast per round — even for energy-0 spells
        // (MEASURED §8.6). ORACLE-VERIFY: the DLL scopes this flag to
        // benign casts (offensive ones are gated by round energy instead,
        // and their manual form only engages, below); whether a benign
        // cast blocks a subsequent offensive ENGAGEMENT in the same round
        // is unmeasured — we currently block it here.
        if let Some(Session::InGame { cast_this_round: true, .. }) = self.sessions.get(&session)
        {
            // MEASURED §8.12: the kai variant fires with kai still in the
            // pool and charges nothing — same flag, same position; the
            // invoke round flag IS cast_this_round (`+0x700 & 4`).
            self.output_line(session, self.already_cast_line(session));
            return;
        }
        let spell = self.content.spells[&spell_id].clone();
        // Gate 4: level vs required_power (spec §2) — unreachable via
        // scroll-learned books, reachable via slice-4 temp spells.
        if i32::from(self.player(session).level) < i32::from(spell.required_power) {
            self.output_line(session, text::SPELL_TOO_POWERFUL);
            return;
        }
        // Offensive target resolution, BEFORE the cost gates and the roll
        // (MEASURED §8.9: the must-specify, guilt and unmatched-target
        // refusals all left the prompt mana unchanged; in the DLL they
        // live in the dispatcher / cast_no_target ahead of
        // cast_monster_target's cost gates).
        let round_cost = i32::from(spell.round_cost);
        let mana_cost = i32::from(spell.mana_cost);
        let base_chance = spell.base_chance;
        let spell_name = spell.name.clone();
        let offensive = spell.target_mode.is_offensive();
        let mut monster = None;
        if offensive {
            let room = self.player(session).location;
            if target.is_empty() {
                // MEASURED (§8.9 run 2): a bare offensive cast while
                // melee-engaged prints *Combat Off* (the engagement
                // breaks) and THEN its refusal.
                self.break_combat(session);
                if self.content.rooms.get(&room).is_some_and(|r| r.protected()) {
                    // Protected room (attributes & 1 — the Newhaven
                    // shops): the guilt line (MEASURED §8.6/§8.9). The
                    // DLL charges the round cost here when affordable but
                    // never the mana (decompile cast_no_target
                    // 39185-39195; §8.9: mana unchanged).
                    if let Some(Session::InGame { energy, .. }) =
                        self.sessions.get_mut(&session)
                        && *energy >= round_cost
                    {
                        *energy -= round_cost;
                    }
                    self.output_line(session, text::CAST_GUILT);
                    return;
                }
                // Empty room and monsters-only alike — an offensive cast
                // NEVER auto-picks a target, unlike attack (MEASURED §8.9).
                self.output_line(session, text::MUST_SPECIFY_TARGET);
                return;
            }
            match self.find_monster(room, &target) {
                Some(id) => monster = Some(id),
                None => {
                    // MEASURED (§8.9): the entire remainder is one target
                    // string, echoed verbatim.
                    self.output_line(session, &text::do_not_see_here(&target));
                    return;
                }
            }
            // The protected-room flag gates the TARGETED path too
            // (decompile cast_monster_target 43232, guilt refusal
            // 44290-44297 — the same room+0x564 & 1 check as the bare-cast
            // gate above, sitting ahead of the SpellImmu and cost gates):
            // guilt line, no engagement, and the same round-cost-only
            // charging as the bare-cast guilt path.
            if self.content.rooms.get(&room).is_some_and(|r| r.protected()) {
                if let Some(Session::InGame { energy, .. }) = self.sessions.get_mut(&session)
                    && *energy >= round_cost
                {
                    *energy -= round_cost;
                }
                self.output_line(session, text::CAST_GUILT);
                return;
            }
        } else if !target.is_empty() {
            // Item-target spells (match 6/7 -> cast_item_target, decompile
            // 0x49232; the dispatcher's find_action_target kind-8 arm at
            // 59314): resolve the target against the CARRIED inventory.
            // ORACLE-VERIFY: whether ground/worn items also match, and the
            // "You are not carrying %s!" kind-4 refusal, are unmeasured —
            // an unmatched name falls to the do-not-see refusal below.
            if spell.match_type.is_item() {
                let want = target.trim().to_ascii_lowercase();
                let found = self.player(session).inventory.iter().find_map(|(id, _)| {
                    self.content
                        .items
                        .get(id)
                        .filter(|i| word_prefix_match(&i.name, &want))
                        .map(|_| *id)
                });
                if let Some(item_id) = found {
                    self.fire_item_cast(session, &spell, item_id);
                    return;
                }
            }
            // MEASURED (§8.9): a non-empty target string on a benign spell
            // does a room-entity lookup that can fail — "cast blur extra
            // trailing words" -> the do-not-see refusal, no self-cast, no
            // mana, before any cost. Slice 3 benign casts are SELF-ONLY,
            // so every lookup here fails; explicit friendly targets
            // (match types with a target slot) land in slice 5.
            // ORACLE-VERIFY: targeting a real other player is unmeasured.
            self.output_line(session, &text::do_not_see_here(&target));
            return;
        }
        if let Some(monster_id) = monster {
            // SpellImmu (139): a monster immune to spells at or below this
            // level refuses the cast before any cost or engagement
            // (decompile cast_monster_target 43630-43638: spell level <
            // SpellImmu value => "no effect"; the autocombat re-fire skips
            // this check, so it lives on the command path only).
            let immu = self.monster_ability_value(monster_id, Ability::SpellImmu);
            if immu > 0 && i32::from(spell.required_power) < immu {
                let name = self.monster_name(monster_id);
                self.output_line(session, &text::spell_no_effect_on(&name));
                return;
            }
            // Engagement is the command's ENTIRE effect (MEASURED,
            // oracle_spell_cast.raw 567-637 + decompile cast_monster_target
            // 43439-43481): the manual offensive cast never rolls, charges
            // or fires directly — it prints the *Combat Off*/*Combat
            // Engaged* toggle, zeroes the round energy and arms `casting`;
            // the combat round driver performs every actual cast. (§8.6's
            // condensed example shows engage+fire together, but the raw
            // capture shows mana UNCHANGED at the engagement prompt and the
            // fire arriving a round later — which is also why a mid-combat
            // re-cast is never blocked by the one-cast-per-round gate: for
            // offensive spells the round energy IS that gate.) Mana and
            // energy shortages are therefore not checked here either; the
            // per-round attempt handles both silently. The DLL conditions
            // this engage-only block on duration == 0 (decompile:
            // `param_1[0x67] == 0`); offensive DURATION spells take the
            // add_cast_spell_to_monster path instead — SLICE 6, with
            // monster slots. DATA (slice-4 Task 3 check): ZERO of the 65
            // shipped offensive-duration spells are taught by any
            // LearnSp(42) scroll — all are monster-attack payloads — so no
            // player cast can reach that path before slice 6; engage-only
            // is correct for every learnable spell until then.
            if let Some(Session::InGame { target, .. }) = self.sessions.get_mut(&session)
                && target.is_some()
            {
                *target = None;
                self.output_line(session, text::COMBAT_OFF);
            }
            if let Some(Session::InGame { target, casting, energy, .. }) =
                self.sessions.get_mut(&session)
            {
                *target = Some(monster_id);
                *casting = Some(spell_id);
                // DLL 43468: engagement zeroes the pool — the first fire
                // waits for the next combat round's refill.
                *energy = 0;
            }
            self.output_line(session, text::COMBAT_ENGAGED);
            // Retaliation lock (transcript: the filthbug swiped back after
            // the bare engagement, before any damage landed).
            if let Some(m) = self.monsters.get_mut(&monster_id) {
                m.target = Some(session);
            }
            return;
        }
        // Benign spells: roll + costs at the command, unlike offensive
        // (MEASURED: blur's mana moved at the prompt, §8.6/§8.9).
        // Gate 5: round energy — exactly like an M3 attack without energy,
        // a silent no-op within the round (no measured message). The
        // deduction itself happens at roll time below.
        let Some(Session::InGame { energy, player, derived, .. }) = self.sessions.get(&session)
        else {
            return;
        };
        let spellcasting = derived.spellcasting;
        let level = player.level;
        let caster_name = player.name.clone();
        let room = player.location;
        // AlterSpLength (166): the caster's accumulated percentage
        // stretches duration-spell lengths (spec §4 step 1; decompile
        // add_cast_spell_to_user 38165 reads get_user_ability_value(0xa6)
        // at entry time — race/class/gear via the same bag the recompute
        // uses; active-slot contributions join the bag in Task 4).
        let alter_sp_length = if spell.duration == 0 {
            0
        } else {
            self.ability_bag(player).value(Ability::AlterSpLength)
        };
        if *energy < round_cost {
            return;
        }
        // Gate 6: mana (MEASURED §8.6; kai wording §8.12) — checked here,
        // deducted at roll time (full on success, half rounded down on a
        // failed roll).
        if player.current_mana < mana_cost {
            self.output_line(session, self.not_enough_mana_line(session));
            return;
        }
        // All gates passed: the round is spent whether the roll then
        // succeeds or fails (MEASURED §8.6).
        if let Some(Session::InGame { cast_this_round, .. }) = self.sessions.get_mut(&session) {
            *cast_this_round = true;
        }
        // Success roll (spec §3 step 5): the seeded game genrdn drives it;
        // `cast_roll_succeeds` is the injectable seam for tests.
        let rng = &mut self.rng;
        let succeeded = cast_roll_succeeds(spellcasting, base_chance, &mut |lo, hi| {
            rng.roll(lo, hi)
        });
        // Magnitude (success only): the SAME roll as the offensive side
        // (spec §3) — but benign target modes never apply elemental
        // resistance (spec §4: the modifier returns 0 unless target mode
        // < 3), so resist is 0 by construction.
        let magnitude = if succeeded {
            let rng = &mut self.rng;
            spell_magnitude(&spell, level, 0, &mut |lo, hi| rng.roll(lo, hi))
        } else {
            0
        };
        // Duration (success only, duration spells only): spec §4 step 1,
        // rolled from the same seeded game rng as the magnitude.
        let duration = if succeeded && spell.duration != 0 {
            let rng = &mut self.rng;
            spell_duration(&spell, level, alter_sp_length, &mut |lo, hi| rng.roll(lo, hi))
        } else {
            0
        };
        let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        // Both outcomes pay the full round cost (spec §3 steps 6-7).
        *energy -= round_cost;
        if !succeeded {
            // Half mana rounded down (mmis 1 -> 0 oracle-confirmed §8.6;
            // blur 4 -> 2 §8.9), no effects applied.
            // DLL clamps the halved cost at 0 (decompiled 39387) — a
            // negative mana_cost must not refund on failure.
            // ORACLE-VERIFY: the kai fail wording is unmeasured (§8.12
            // never rolled a failure — the mystic sc term is 500).
            player.current_mana -= (mana_cost / 2).max(0);
            self.output_line(session, &text::cast_fail(&spell_name));
            self.broadcast_to_room(room, Some(session), &text::cast_fail_room(&caster_name, &spell_name));
            return;
        }
        player.current_mana -= mana_cost;
        self.benign_success_effects(session, &spell, magnitude, duration);
    }

    /// Everything a SUCCESSFUL benign cast does after its costs are paid
    /// — shared verbatim by the command path and the mode-2 forced cast
    /// (`forced_cast`): the dispel pre-pass, the instant apply loop or
    /// duration slot entry, then the success lines.
    fn benign_success_effects(
        &mut self,
        session: SessionId,
        spell: &crate::content::Spell,
        magnitude: i32,
        duration: i32,
    ) {
        let Some(Session::InGame { derived, .. }) = self.sessions.get(&session) else {
            return;
        };
        let max_hp = derived.max_hp;
        // RemovesSpell (122) / KillSpell (153) pre-pass on the target's
        // own slots (spec §3): the named spell is dispelled — blur (129)
        // removes the amethyst pendant's effect 157 (anti-stacking). The
        // pre-pass loop (decompile cast_no_target 39463-39572) runs for
        // EVERY benign cast — it is NOT duration-gated; 17 shipped instant
        // spells (cure-poison family) carry dispel abilities. RemovesSpell
        // honors the victim's EndCast chain, KillSpell suppresses it
        // (39491: chainFlag = `ability == 0x7a`).
        for (ability, value) in &spell.abilities {
            let honor_endcast = match ability {
                Ability::RemovesSpell => true,
                Ability::KillSpell => false,
                _ => continue,
            };
            // First find wins: the DLL returns from inside the slot scan
            // (39494) without visiting later ability slots.
            if let Ok(id) = u16::try_from(*value)
                && id != 0
                && let Some(idx) = self.player(session).find_active(SpellId(id))
            {
                // Early return (decompile 39472-39494, match types
                // 1/2/6): a FOUND dispel prints display_spell_success
                // FIRST (39481-39483), then clears the slot and runs the
                // termination path (wear-off line + chain), recomputes,
                // and RETURNS — the cast ends. The return precedes the
                // apply loop (39573), so the caster's spell is neither
                // applied (instant effects included) nor entered into a
                // slot; only the success lines print.
                // ORACLE-VERIFY: blur-over-157 live probe (st should NOT
                // show blurred).
                // param_6 here is uVar1 — the RAW rolled magnitude, no
                // slot-value override (39482-39484: the pre-pass sits
                // before the apply loop's per-slot local_9c rewrite).
                self.emit_cast_success_lines(session, spell, magnitude);
                self.terminate_active_spell(session, idx, honor_endcast);
                return;
            }
        }
        // ImmuPoison (21) gates the ENTIRE Poison(19) case — counter
        // write, success display AND slot entry (decompile 40522-40546:
        // user_has_ability(0x15) wraps the whole case body, instant and
        // duration arms alike). The wrap scope is one SLOT-LOOP ITERATION,
        // not the whole spell: any other driving slot still displays and
        // enters through the once-flag (local_49). Race/class/gear/
        // active-slot sources all count (the same bag the recompute
        // uses). DATA: all six shipped Poison-carrying match-1/2/6 spells
        // (yellow potion 184, red fungus 250, mushroom poison 373,
        // redberry poison trap 628, dart poison 694, poison 704) carry
        // only Poison + no-op slots (DescMsg 115 / NonMagicalSpell 144),
        // so an immune target gets nothing at all from them.
        let immune_poison = spell.abilities.iter().any(|(a, _)| *a == Ability::Poison)
            && self.ability_bag(self.player(session)).value(Ability::ImmuPoison) != 0;
        // The driving slot: the first non-noop, non-gated ability. Its
        // fixed-or-rolled value is display_spell_success's param_6 — the
        // damage arg of the success lines (every case passes its own
        // local_9c: 40529-40531 Poison, 40075-40077 Alterhunger, ...; the
        // loop head 39575-39580 rewrites local_9c to the slot value when
        // non-zero, else leaves the rolled magnitude).
        let gated =
            |a: Ability| ability_case_is_noop(a) || (immune_poison && a == Ability::Poison);
        let display_damage = spell
            .abilities
            .iter()
            .find(|(a, _)| !gated(*a))
            .map_or(magnitude, |(_, v)| if *v != 0 { i32::from(*v) } else { magnitude });
        if immune_poison
            && !spell
                .abilities
                .iter()
                .any(|(a, _)| *a != Ability::Poison && !ability_case_is_noop(*a))
        {
            // No driving slot survives the gate: the DLL's apply loop
            // finishes without ever reaching a display_spell_success or
            // add_cast_spell_to_user call — no lines, no slot entry, and
            // the costs stay paid (the roll already succeeded).
            return;
        }
        if spell.duration == 0 {
            // Instant apply loop (spec §4 table, self-target): iterate the
            // ability slots; a non-zero slot value is a FIXED amount, 0
            // means the rolled V — pinned on BOTH paths (offensive
            // decompile 43711-43717; benign cast_no_target loop ~39577).
            let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session)
            else {
                return;
            };
            for (ability, value) in &spell.abilities {
                let amount = match *value {
                    0 => magnitude,
                    v => i32::from(v),
                };
                match ability {
                    // Heal (18): HP += V, capped at the derived max.
                    Ability::Heal => {
                        player.current_hp = (player.current_hp + amount).min(max_hp);
                    }
                    // EnergyLevel (11): round pool += V, capped at max.
                    Ability::EnergyLevel => {
                        // ORACLE-VERIFY: spec §4 caps at the pool max, but
                        // the decompiled benign branch (case 0xb, ~39960)
                        // is an uncapped add — the cap may belong only to
                        // the §5 per-tick handler.
                        *energy = (*energy + amount).min(PLAYER_ENERGY_MAX);
                    }
                    // Alterhunger (15) / AlterThirst (16): the +0xce/+0xd0
                    // counters (u16 fields; clamp instead of wrapping).
                    Ability::Alterhunger => {
                        player.hunger = clamp_counter(i32::from(player.hunger) + amount);
                    }
                    Ability::AlterThirst => {
                        player.thirst = clamp_counter(i32::from(player.thirst) + amount);
                    }
                    // Poison (19): the +0xbe counter is SET-IF-GREATER,
                    // not added (decompile 40525-40527: `if (poison < v)
                    // poison = v` — every apply site agrees, incl. the
                    // monster-cast paths 22726/23390), ImmuPoison-gated.
                    Ability::Poison => {
                        if !immune_poison {
                            let v = clamp_poison(amount);
                            if player.poison < v {
                                player.poison = v;
                            }
                        }
                    }
                    // Cure Poison (20): poison -= V, floored 0
                    // (40669-40673) — no ImmuPoison gate on the cure side.
                    Ability::CurePoison => {
                        player.poison = clamp_poison(i32::from(player.poison) - amount);
                    }
                    // Summon (12): SLICE 6 — DATA (slice-5 Task 6 check,
                    // re/mmud_wgnt.sqlite): 87 shipped spells carry
                    // Summon(12); ZERO are named by any LearnSp(42) item —
                    // all are monster-attack payloads (raptor summon,
                    // calls for aid, ...). No player cast can reach this
                    // arm before slice-6 monster casting; the spawn-side
                    // wiring (owned tag = aggression marker) lands there.
                    Ability::Summon => {}
                    // Remaining benign instants land with their systems.
                    _ => {}
                }
            }
        }
        // Duration spells apply NOTHING directly: add_cast_spell_to_user
        // enters them into the target's active-spell slots and the stat
        // recompute reads the slots from there (spec §4; the recompute
        // feed is Task 4).
        else {
            // Poison (19) hard-writes at ENTRY too (decompile 40536-40538:
            // the duration arm max-writes the counter BEFORE
            // add_cast_spell_to_user — even a slot-overflow cast leaves
            // the counter raised), same ImmuPoison gate and the same
            // per-ability value override convention.
            if !immune_poison {
                for (ability, row) in &spell.abilities {
                    if *ability != Ability::Poison {
                        continue;
                    }
                    let v = clamp_poison(match *row {
                        0 => magnitude,
                        v => i32::from(v),
                    });
                    if let Some(Session::InGame { player, .. }) =
                        self.sessions.get_mut(&session)
                        && player.poison < v
                    {
                        player.poison = v;
                    }
                }
            }
            // Entry (spec §4 steps 2-3): already active → refresh value
            // and duration in place; else the first free slot. The DLL
            // stores the 16-bit rolled magnitude as the slot value
            // (decompile 38180/38195: `(undefined2)param_4`).
            let entered = {
                let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
                else {
                    return;
                };
                let slot = ActiveSpell {
                    spell: Some(spell.id),
                    value: magnitude as i16,
                    remaining: duration,
                };
                if let Some(idx) = player.find_active(spell.id) {
                    // VERIFIED (§8.11): a mid-buff recast is a SILENT
                    // full refresh — byte-identical output to a first
                    // cast (castmsgb + the DescMsg active line below, no
                    // "already have" variant), full mana charged, and the
                    // timer resets to a full duration from the recast.
                    player.active_spells[idx] = slot;
                    true
                } else if let Some(idx) = player.first_free_slot() {
                    player.active_spells[idx] = slot;
                    true
                } else {
                    false
                }
            };
            if entered {
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                // The slot now feeds the ability bag: recompute cached
                // derived stats (update_dynamic_stats runs after apply).
                self.refresh_derived(session);
            } else {
                // ORACLE-VERIFY slot overflow (unmeasured live;
                // decompile-backed): both slot scans exhausted →
                // add_cast_spell_to_user 38205-38217 prints the cast-fail
                // line to the CASTER only and returns -1. The effect is
                // lost, the full costs stay paid (the success roll
                // passed), and display_spell_success is never reached —
                // no castmsgb, no active line, no room broadcast.
                let fail = text::cast_fail(&spell.name);
                self.output_line(session, &fail);
                return;
            }
        }
        self.emit_cast_success_lines(session, spell, display_damage);
    }

    /// `cast_item_target` (decompile 0x49232), scoped to the LEARNABLE
    /// surface — DATA (slice-5 Task 6 check, re/mmud_wgnt.sqlite): of the
    /// 53 shipped match-6/7 spells exactly two are named by a LearnSp(42)
    /// item — detect magic (24, mage L5; scroll 121 sold at the Newhaven
    /// Mage Spell Shop 9) and song of lore (41, bard) — and both carry
    /// only DetectMagic(26). The skeleton mirrors the benign command path
    /// (NOT_ENOUGH_MANA, one-cast-per-round set, roll, full/half costs) —
    /// EXCEPT the energy gate: cast_item_target's own triple gate
    /// (44397-44409) prints the already-cast line for a round-energy
    /// shortage, unlike the benign self-cast path's measured silent
    /// no-op. The fail lines are the same cast_no_target pair
    /// (44694-44700).
    fn fire_item_cast(
        &mut self,
        session: SessionId,
        spell: &crate::content::Spell,
        item_id: crate::content::ItemId,
    ) {
        let Some(Session::InGame { energy, player, derived, .. }) = self.sessions.get(&session)
        else {
            return;
        };
        let spellcasting = derived.spellcasting;
        let level = player.level;
        let caster_name = player.name.clone();
        let room = player.location;
        let round_cost = i32::from(spell.round_cost);
        let mana_cost = i32::from(spell.mana_cost);
        if *energy < round_cost {
            // 44397-44409: the energy leg of the triple gate prints the
            // already-cast line (kai wording keyed like every refusal;
            // unreachable for mystics — no group-5 item-target spell)
            // and charges nothing.
            self.output_line(session, self.already_cast_line(session));
            return;
        }
        if player.current_mana < mana_cost {
            self.output_line(session, self.not_enough_mana_line(session));
            return;
        }
        if let Some(Session::InGame { cast_this_round, .. }) = self.sessions.get_mut(&session) {
            *cast_this_round = true;
        }
        // Roll, then magnitude (44450 genrdn(0,100); 44540 the min/max
        // roll) — the same seeded order as every other cast path.
        let rng = &mut self.rng;
        let succeeded = cast_roll_succeeds(spellcasting, spell.base_chance, &mut |lo, hi| {
            rng.roll(lo, hi)
        });
        let magnitude = if succeeded {
            let rng = &mut self.rng;
            spell_magnitude(spell, level, 0, &mut |lo, hi| rng.roll(lo, hi))
        } else {
            0
        };
        let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session)
        else {
            return;
        };
        *energy -= round_cost;
        if !succeeded {
            // Half mana rounded toward zero, clamped non-negative
            // (44676-44689), and the cast_no_target fail lines.
            player.current_mana -= (mana_cost / 2).max(0);
            self.output_line(session, &text::cast_fail(&spell.name));
            self.broadcast_to_room(
                room,
                Some(session),
                &text::cast_fail_room(&caster_name, &spell.name),
            );
            return;
        }
        // Success charges the FULL mana, clamped non-negative (44521-44526
        // — unlike the benign self-cast path, the item path never grants
        // mana on a pathological negative cost).
        player.current_mana -= mana_cost.max(0);
        let Some(item) = self.content.items.get(&item_id).cloned() else {
            return;
        };
        for (ability, row) in &spell.abilities {
            // The per-ability override convention holds here too (44553-
            // 44556); DetectMagic ignores the amount (it reads the ITEM).
            let _amount = match *row {
                0 => magnitude,
                v => i32::from(v),
            };
            // DetectMagic (26, case 0x1a 44620-44667): band on the item's
            // Magical(28) value, then the room announce. The duration != 0
            // arm is silly_spell — unreachable, no learnable match-6/7
            // spell carries a duration. Every OTHER ability × match-6/7
            // pairing in the DLL is either silly_spell (a joke refusal) or
            // a deep item-mutation case (Lore 162, ...) — none is carried
            // by a learnable spell (data check above); they land if a
            // future data pass ever surfaces one.
            if *ability != Ability::DetectMagic || spell.duration != 0 {
                continue;
            }
            let magical = item
                .abilities
                .iter()
                .find_map(|(a, v)| (*a == Ability::Magical).then_some(i32::from(*v)))
                .unwrap_or(0);
            let line = match magical {
                1 => text::glows_faintly(&item.name),
                2..=3 => text::glows_softly(&item.name),
                4..=5 => text::glows_brightly(&item.name),
                v if v >= 6 => text::blinding_aura(&item.name),
                _ => text::NO_MAGIC_IN_ITEM.to_string(),
            };
            self.output_line(session, &line);
            self.broadcast_to_room(
                room,
                Some(session),
                &text::casts_spell_on(&caster_name, &spell.name, &item.name),
            );
        }
    }

    /// `display_spell_success` for a benign self-cast (decompile
    /// 38004-38011): the castmsgb fan-out, then the DescMsg (115) line3
    /// active line on duration casts. `damage` is the function's param_6
    /// — the caller's fixed-or-rolled magnitude, bound into any `%d` slot
    /// of the message (even caster line 37988 `prf(local_60, spellName,
    /// target, param_6)`; odd caster line 38068 `prf(local_60, target,
    /// param_6)`) — annointed hands (744) and minor healing (13) both
    /// carry a `%d` that binds the heal roll.
    fn emit_cast_success_lines(
        &mut self,
        session: SessionId,
        spell: &crate::content::Spell,
        damage: i32,
    ) {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return;
        };
        let caster_name = player.name.clone();
        let room = player.location;
        // Cast messages: castmsgb only (castmsga is the empty message on
        // every sampled spell — the Task-10 renderer contract). Slice-3
        // benign casts are SELF-ONLY: target = the caster, the caster line
        // always prints, the TARGET line goes to no one (oracle §8.6:
        // `c blur` printed the caster line only), and the room line goes
        // to everyone else in the room.
        if let Some(msg) = spell.cast_msg_b.and_then(|id| self.content.messages.get(&id)) {
            let args = text::CastMsgArgs {
                caster: &caster_name,
                target: Some(&caster_name),
                spell: &spell.name,
                damage: Some(damage),
            };
            // msgstyle-odd binds (target, damage) with no spell name —
            // the renderer's second order table. DATA: the lowest
            // learnable odd BENIGN spell is annointed hands (744, mage
            // L10, odd instant heal via scroll 1179 at shop 111) — its
            // caster line "%s is healed of %d damage!" binds the heal
            // roll as the damage arg. ORACLE-VERIFY: unmeasured live.
            let odd = spell.msg_style & 1 == 1;
            let caster_line = text::render_cast_line(msg, text::CastAudience::Caster, &args, odd);
            let room_line = text::render_cast_line(msg, text::CastAudience::Room, &args, odd);
            if let Some(line) = caster_line {
                self.output_line(session, &line);
            }
            if let Some(line) = room_line {
                self.broadcast_to_room(room, Some(session), &line);
            }
        }
        // Cast-time active line: message line3 of the spell's DescMsg (115)
        // record, to the caster on a duration cast (decompile
        // display_spell_success 38010-38011 prints message line 3 to the
        // target; oracle: `You are blurred!`). MEASURED (§8.11): the live
        // async path prints castmsgb, prompt, then erases the pending
        // prompt in place and prints line3 + a fresh prompt — the net
        // visible order (castmsgb, line3, prompt) is exactly what our
        // single end-of-command prompt produces.
        if spell.duration != 0
            && let Some(msg_val) = spell
                .abilities
                .iter()
                .find_map(|(a, v)| (*a == Ability::DescMsg).then_some(*v))
            && let Ok(id) = u16::try_from(msg_val)
            && let Some(msg) = self.content.messages.get(&crate::content::MessageId(id))
            && let Some(line) = msg.lines.get(2).filter(|l| !l.is_empty())
        {
            let line = line.clone();
            self.output_line(session, &line);
        }
    }

    /// `perform_spell_termination_player_upkeep` (decompile 44814-44900;
    /// spec §5) — runs once when a spell leaves a slot: upkeep expiry, a
    /// targeted dispel (RemovesSpell honors the EndCast chain, KillSpell
    /// suppresses it), or death (chain suppressed). Clears the slot FIRST
    /// and terminates with the stored value — every DLL call site zeroes
    /// `+0x40/+0x54/+0x68` before the call (13053-13060 death,
    /// 19819-19823 expiry, 39487-39492 dispel; spec §5).
    fn terminate_active_spell(&mut self, session: SessionId, idx: usize, honor_endcast: bool) {
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        let slot = player.active_spells[idx];
        let Some(spell_id) = slot.spell else {
            return;
        };
        player.active_spells[idx] = ActiveSpell::default();
        let stored = i32::from(slot.value);
        let Some(spell) = self.content.spells.get(&spell_id).cloned() else {
            // Unknown spell id (content changed under a save): the DLL
            // clears the slot but skips termination when get_spell_data
            // fails (13056-13060). Recompute + persist the bare clear.
            self.refresh_derived(session);
            let snapshot = Box::new(self.player(session).clone());
            self.events.push(Event::Persist(snapshot));
            return;
        };
        // 1. The wear-off line: DescMsg (115) message line1, to the OWNER
        //    only (44824-44844: get_spell_ability_value(0x73) — the RAW
        //    row value, no stored-value substitution — then prf(line1,
        //    spellName) + prompt on the owner's terminal; %s binds the
        //    spell name). MEASURED §8.11: `The effects of blur wear off.`
        //    arrives via the async redraw path — our output_line is the
        //    same net line. Room line2: the DLL emits NOTHING to the room
        //    (44840 prints line1 only) — ORACLE-VERIFY, no room-side
        //    wear-off was ever measured.
        if let Some(msg_val) = spell
            .abilities
            .iter()
            .find_map(|(a, v)| (*a == Ability::DescMsg).then_some(*v))
            && let Ok(id) = u16::try_from(msg_val)
            && let Some(msg) = self.content.messages.get(&crate::content::MessageId(id))
            && let Some(line) = msg.lines.first().filter(|l| !l.is_empty())
        {
            let line = line.replacen("%s", &spell.name, 1);
            self.output_line(session, &line);
        }
        // 2. Hard-write reversal + chain metadata (44846-44890). Value =
        //    the stored slot value unless the ability row carries its own
        //    nonzero value (44849-44852 — the same per-ability override
        //    convention as the recurring upkeep handlers, spec §5).
        let mut chain: i32 = 0;
        // CastOnEnd% default 100 when the row is absent (44830).
        let mut pct: i32 = 100;
        for (ability, row) in &spell.abilities {
            let v = if *row != 0 { i32::from(*row) } else { stored };
            match ability {
                // Poison (19): `+0xbe -= v`, floored 0 (44853-44857).
                // With the set-if-greater apply, a doubly-poisoned
                // player keeps the surplus and a partially-cured one
                // floors at 0 — both faithful.
                Ability::Poison => {
                    if let Some(Session::InGame { player, .. }) =
                        self.sessions.get_mut(&session)
                    {
                        player.poison = clamp_poison(i32::from(player.poison) - v);
                    }
                }
                // Stat buffs 44-49 (44858-44875 subtract from the
                // effective stats +0xa2..+0xac): NO explicit reversal
                // here — our cast entry never direct-writes; the buff
                // lives in the ability bag and effective_stats folds it
                // on every recompute, so clearing the slot above already
                // removed it (KNOWN-DIVERGENCE in mechanism, identical
                // observable — see effective_stats).
                Ability::Intel
                | Ability::Wisdom
                | Ability::Strength
                | Ability::Health
                | Ability::Agility
                | Ability::Charm => {}
                // AlterHP (88): the DLL subtracts BOTH max (+0xae) and
                // current (+0xb0) HP (44876-44879). The max half is
                // bag-fed here (derive adds AlterHP into max_hp) and
                // vanishes with the recompute below. The CURRENT-HP half
                // is a real hard write — but our cast entry never adds
                // it, so there is nothing to subtract: exactly ONE
                // shipped duration spell carries 88 (746 "increase HP",
                // named by no LearnSp scroll — a monster-cast payload),
                // unreachable until slice-6 monster casting brings the
                // add-at-entry/subtract-here pair together.
                Ability::AlterHP => {}
                // GiveTempSpell (160): purge_spell_from_spellbook
                // (44883-44885) — only a `temporary` book entry (the
                // slice-2 flag) is removed; a permanently learned copy
                // of the same spell survives.
                Ability::GiveTempSpell => {
                    if let Ok(id) = u16::try_from(v)
                        && let Some(Session::InGame { player, .. }) =
                            self.sessions.get_mut(&session)
                        && player.spellbook.get(&SpellId(id)) == Some(&true)
                    {
                        player.spellbook.remove(&SpellId(id));
                    }
                }
                // EndCast (151) / CastOnEnd% (164) go through the SAME
                // override convention (44880-44882, 44886-44888): a
                // zero-value row substitutes the stored slot value as
                // the chained spell id / percentage. One shipped
                // duration spell rides that quirk (935 "sysop jail
                // time", (EndCast, 0)).
                Ability::EndCast => chain = v,
                Ability::CastOnEnd => pct = v,
                _ => {}
            }
        }
        // 3. Recompute + persist the cleared slot / purged book. (The DLL
        //    runs calculate_secondary_stats AFTER the chain, 44896 — but
        //    the chained cast's own slot entry recomputes for itself, so
        //    refreshing first is observably identical.)
        self.refresh_derived(session);
        let snapshot = Box::new(self.player(session).clone());
        self.events.push(Event::Persist(snapshot));
        // 4. EndCast chain (44891-44895): only when this termination
        //    honors it, the id resolves, and genrdn(0,100) < pct.
        if honor_endcast
            && chain != 0
            && self.rng.roll(0, 100) < pct
            && let Ok(id) = u16::try_from(chain)
        {
            self.forced_cast(session, SpellId(id));
        }
    }

    /// `cast_no_target(..., mode 2)` — the forced follow-up cast an
    /// EndCast chain fires (spec §5 step 4). The decompile's mode-2
    /// branches pin what a forced cast skips and what it still pays:
    /// - confusion / downed / NoMagic / Kai-block gates are all
    ///   `param_3 == '\0'`-gated (39118, 39125, 39133, 39147) — skipped;
    /// - the class-school gate still applies, SILENTLY (39235-39239:
    ///   wrong magery group or casting factor below the spell's class
    ///   level → bare `return 0`);
    /// - round-energy, mana and level gates still apply WITH their
    ///   refusal lines (39253-39281: the triple check is unconditional;
    ///   energy prints the already-cast line, then mana, then level).
    ///   The refusal block sits under `DAT_004877f4 == 0` (39257) — the
    ///   autocombat-driver flag, set nonzero only inside
    ///   do_autocombat_for_user (46642) and cleared on every exit
    ///   (46674/46689), so on the command and upkeep paths the gate is
    ///   effectively constant-false and the refusal lines always print
    ///   (the silent energy-drain else-branch at 39283-39297 belongs to
    ///   autocombat, out of scope here);
    /// - NO success roll and NO one-cast-per-round flag: modes 1/2 set
    ///   the success local unconditionally (39240), and the `+0x700 & 4`
    ///   round flag is only consulted in the mode-0 benign branch
    ///   (39344-39355);
    /// - the always-success path deducts the FULL round cost and mana
    ///   (39400-39408; the COST is clamped non-negative — for a
    ///   pathological negative mana_cost the DLL would grant mana, we
    ///   refuse; unreachable with shipped data).
    ///
    /// Slice-4 scope: benign self-cast only — no shipped EndCast chain is
    /// reachable by a player cast (48 duration spells carry EndCast 151;
    /// none is named by any LearnSp scroll), and offensive chain targets
    /// need slice-6 monster slots.
    fn forced_cast(&mut self, session: SessionId, spell_id: SpellId) {
        let Some(spell) = self.content.spells.get(&spell_id).cloned() else {
            return;
        };
        if spell.target_mode.is_offensive() {
            // SLICE 6: an offensive forced cast needs a target monster
            // slot; nothing shipped can reach this today (see above).
            return;
        }
        let Some(Session::InGame { player, energy, .. }) = self.sessions.get(&session) else {
            return;
        };
        // Class-school gate (39235-39239): silent refusal.
        if self.spell_gate(player, &spell) == SpellGate::WrongClass {
            return;
        }
        let round_cost = i32::from(spell.round_cost);
        let mana_cost = i32::from(spell.mana_cost);
        // The unconditional triple gate (39253), refusal lines in the
        // decompile's order (39264-39281): round energy prints the
        // already-cast line, then mana, then level-vs-required-power.
        if *energy < round_cost {
            self.output_line(session, self.already_cast_line(session));
            return;
        }
        if player.current_mana < mana_cost {
            self.output_line(session, self.not_enough_mana_line(session));
            return;
        }
        if i32::from(player.level) < i32::from(spell.required_power) {
            self.output_line(session, text::SPELL_TOO_POWERFUL);
            return;
        }
        let level = player.level;
        let alter_sp_length = if spell.duration == 0 {
            0
        } else {
            self.ability_bag(player).value(Ability::AlterSpLength)
        };
        // No success roll (39247): magnitude and duration always land.
        let rng = &mut self.rng;
        let magnitude = spell_magnitude(&spell, level, 0, &mut |lo, hi| rng.roll(lo, hi));
        let duration = if spell.duration != 0 {
            let rng = &mut self.rng;
            spell_duration(&spell, level, alter_sp_length, &mut |lo, hi| rng.roll(lo, hi))
        } else {
            0
        };
        // Full costs on the always-success path (39400-39408).
        if let Some(Session::InGame { player, energy, .. }) = self.sessions.get_mut(&session) {
            *energy -= round_cost;
            player.current_mana -= mana_cost.max(0);
        }
        self.benign_success_effects(session, &spell, magnitude, duration);
    }

    /// A live monster's template name (empty if the instance is gone).
    fn monster_name(&self, id: MonsterInstanceId) -> String {
        self.monsters
            .get(&id)
            .and_then(|m| self.content.monsters.get(&m.template))
            .map_or_else(String::new, |t| t.name.clone())
    }

    /// Sum of the template's ability values for one ability — the
    /// `get_monster_ability_value` template term (live monster buff slots
    /// join in slice 6 with the 5-slot monster active-spell table).
    fn monster_ability_value(&self, id: MonsterInstanceId, ability: Ability) -> i32 {
        self.monsters
            .get(&id)
            .and_then(|m| self.content.monsters.get(&m.template))
            .map_or(0, |t| {
                t.abilities
                    .iter()
                    .filter(|(a, _)| *a == ability)
                    .map(|(_, v)| i32::from(*v))
                    .sum()
            })
    }

    /// The monster-side MR stat, `local_34` (decompile cast_monster_target
    /// 43387-43392): M.R.(36) ability modifiers plus the template's `mr`
    /// column, floored at 1. Read by BOTH the saving throw (43600-43614)
    /// and the Damage(-MR) scale (43946-43982). ORACLE-VERIFY: the `mr`
    /// column's identity as the template word the DLL reads rests on the
    /// Nightmare field-map ordering; the starter spells are all
    /// SaveClass::None, so no live save was measurable.
    fn monster_save_stat(&self, id: MonsterInstanceId) -> i32 {
        let mr = self
            .monsters
            .get(&id)
            .and_then(|m| self.content.monsters.get(&m.template))
            .map_or(0, |t| i32::from(t.magic_resist));
        (self.monster_ability_value(id, Ability::MR) + mr).max(1)
    }

    /// One offensive-cast execution against the engaged monster, invoked by
    /// the combat round driver every round — including the first fire (the
    /// command only engages; MEASURED oracle_spell_cast.raw + §8.9's
    /// unprompted re-fire, with a fresh success roll and a fresh mana
    /// charge every round). Deliberately does NOT touch `cast_this_round`:
    /// the DLL gates offensive casts on round energy alone, keeping the
    /// benign one-cast flag independent.
    fn offensive_cast_attempt(
        &mut self,
        session: SessionId,
        spell_id: SpellId,
        monster_id: MonsterInstanceId,
    ) {
        let Some(spell) = self.content.spells.get(&spell_id).cloned() else {
            return;
        };
        let round_cost = i32::from(spell.round_cost);
        let mana_cost = i32::from(spell.mana_cost);

        {
            let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session)
            else {
                return;
            };
            if *energy < round_cost {
                return; // wait for the pool, like a melee whiff round
            }
            if player.current_mana < mana_cost {
                // Out of mana mid-combat: decompile cast_monster_target
                // 43560-43575 (the autocombat-driver branch) — the round
                // cost is still paid, nothing is cast, no message, and the
                // engagement holds; casting resumes if mana regenerates.
                // ORACLE-VERIFY: unmeasured live (§8.9 note).
                *energy -= round_cost;
                return;
            }
        }

        let (caster_name, room, level) = {
            let p = self.player(session);
            (p.name.clone(), p.location, p.level)
        };
        let monster_name = self.monster_name(monster_id);
        let (spellcasting, caster_max_hp) = match self.sessions.get(&session) {
            Some(Session::InGame { derived, .. }) => (derived.spellcasting, derived.max_hp),
            _ => return,
        };

        // Success roll (spec §3 step 5) — re-rolled on every re-fire
        // (§8.9: a failed roll was followed by an unprompted success).
        let rng = &mut self.rng;
        let succeeded = cast_roll_succeeds(spellcasting, spell.base_chance, &mut |lo, hi| {
            rng.roll(lo, hi)
        });

        // Saving throw (spec §3; decompile cast_monster_target 43594-43614):
        // only on a successful roll, and only when the save class grants
        // one — Always, or IfAntiMagic against a monster carrying
        // AntiMagic (51).
        let anti_magic = self
            .monsters
            .get(&monster_id)
            .and_then(|m| self.content.monsters.get(&m.template))
            .is_some_and(|t| t.abilities.iter().any(|(a, _)| *a == Ability::AntiMagic));
        let save_allowed = match spell.save_class {
            crate::content::SaveClass::None => false,
            crate::content::SaveClass::Always => true,
            crate::content::SaveClass::IfAntiMagic => anti_magic,
        };
        let resisted = succeeded && save_allowed && {
            let stat = self.monster_save_stat(monster_id);
            let rng = &mut self.rng;
            monster_save_resists(stat, &mut |lo, hi| rng.roll(lo, hi))
        };

        if !succeeded || resisted {
            // Fail and resist pay alike: full round cost, half mana
            // rounded toward zero (decompile 43617-43629 / 44244-44265).
            let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session)
            else {
                return;
            };
            *energy -= round_cost;
            player.current_mana -= (mana_cost / 2).max(0);
            if resisted {
                self.output_line(session, &text::cast_resisted(&spell.name, &monster_name));
                self.broadcast_to_room(
                    room,
                    Some(session),
                    &text::cast_resisted_room(&monster_name, &caster_name, &spell.name),
                );
            } else {
                self.output_line(session, &text::cast_fail(&spell.name));
                self.broadcast_to_room(
                    room,
                    Some(session),
                    &text::cast_fail_room(&caster_name, &spell.name),
                );
            }
            // The victim still locks on (the melee path re-marks after
            // whiffed rounds too).
            if let Some(m) = self.monsters.get_mut(&monster_id)
                && m.target.is_none()
            {
                m.target = Some(session);
            }
            return;
        }

        // Success: full costs (spec §3 step 7).
        let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session)
        else {
            return;
        };
        *energy -= round_cost;
        player.current_mana -= mana_cost;

        // Magnitude (spec §3; decompile 43668-43704), scaled by the
        // monster's elemental resist — Element::Magic has no resist
        // ability, so mmis lands at full value (spec §4; the modifier only
        // applies to offensive target modes, which this path is by
        // construction).
        let resist = spell
            .element
            .resist_ability()
            .map_or(0, |a| self.monster_ability_value(monster_id, a));
        let rng = &mut self.rng;
        let magnitude = spell_magnitude(&spell, level, resist, &mut |lo, hi| rng.roll(lo, hi));

        // Offensive instant abilities (spec §4 table): Damage (1),
        // Damage(-MR) (17) and Drain (8) — the area match types and the
        // remaining offensive abilities land in slice 5. A non-zero
        // ability value is a FIXED amount that bypasses both the magnitude
        // roll and the resist scaling (but NOT the 17 MR scale, which the
        // DLL applies to the fixed-or-rolled amount alike); value 0 means
        // "use the rolled magnitude" (decompile 43711-43717: slot value
        // == 0 selects the rolled local_20). Combined totals assume at
        // most one harm slot per spell — true for ALL shipped data (zero
        // spells carry two of Damage/Drain/DamageMR). The DLL applies
        // per-slot, a kill STOPS its loop (skipping later slots' caster
        // heal), and the message prints the first slot's amount — slice 5
        // must not inherit this combined model if multi-slot content ever
        // appears.
        let mr = self.monster_save_stat(monster_id);
        let mut damage_total = 0i32;
        let mut drain_total = 0i32;
        let mut harms = false;
        for (ability, value) in &spell.abilities {
            let amount = match *value {
                0 => magnitude,
                v => i32::from(v),
            };
            match ability {
                Ability::Damage => {
                    damage_total += amount;
                    harms = true;
                }
                // Damage(-MR) (17): the dominant attack-spell damage
                // (magic missile included) — the amount scaled by the
                // target's MR, the same stat the save reads (damage_mr;
                // decompile 43937-43993).
                Ability::DamageMR => {
                    damage_total += damage_mr(amount, mr, anti_magic);
                    harms = true;
                }
                // Drain (8): the target loses it, the caster gains it
                // (capped at the caster's max HP), kill-checked like
                // Damage (spec §4).
                Ability::Drain => {
                    drain_total += amount;
                    harms = true;
                }
                _ => {} // slice 5: other offensive abilities
            }
        }
        let damage = harms.then_some(damage_total + drain_total);

        // Cast messages: castmsgb only (castmsga is the empty message on
        // every sampled spell — the Task-10 renderer contract). The target
        // line is skipped: the target is a monster, not a session.
        // msgstyle-odd (fireball 120, deathtouch 58, ...) binds (target,
        // damage) with no spell-name slot — the renderer's second order
        // table, keyed on msg_style & 1 (ORACLE-VERIFY: odd rendering is
        // decompile-only; the lowest learnable odd spells are annointed
        // hands L10 / dancing blades L11 / fireball L15, none measured
        // live).
        // Kai wording (§8.12, resolved): the refusal variants are keyed
        // on caster_group 5 (already_cast_line/not_enough_mana_line), the
        // success verb line lives in the MESSAGE DATA ("You invoke the
        // %s." is castmsgb line1 for every group-5 spell), and the
        // per-round invoke flag IS cast_this_round (the measured string
        // differs only in wording). Melee + invoke same-round interplay
        // stays ORACLE-VERIFY (slice 6).
        if let Some(msg) = spell.cast_msg_b.and_then(|id| self.content.messages.get(&id)) {
            let args = text::CastMsgArgs {
                caster: &caster_name,
                target: Some(&monster_name),
                spell: &spell.name,
                damage,
            };
            let odd = spell.msg_style & 1 == 1;
            let caster_line = text::render_cast_line(msg, text::CastAudience::Caster, &args, odd);
            let room_line = text::render_cast_line(msg, text::CastAudience::Room, &args, odd);
            if let Some(line) = caster_line {
                self.output_line(session, &line);
            }
            if let Some(line) = room_line {
                self.broadcast_to_room(room, Some(session), &line);
            }
        }

        // Damage + retaliation + the M3 kill path (death line, exp split,
        // *Combat Off* — monster_killed is check_kill_monster +
        // distribute_experience).
        let Some(damage) = damage else {
            return; // other offensive abilities: slice 5
        };
        let dead = {
            let Some(m) = self.monsters.get_mut(&monster_id) else {
                return;
            };
            m.current_hp -= damage;
            m.target = Some(session);
            m.current_hp <= 0
        };
        // Drain: the stolen HP heals the caster, capped at max (spec §4).
        if drain_total != 0
            && let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
        {
            player.current_hp = (player.current_hp + drain_total).min(caster_max_hp);
        }
        if dead {
            self.monster_killed(monster_id, session);
        }
    }

    /// The eligibility annotation for one shop row (spellcasting.md §8.3):
    /// a LearnSp(42) scroll gates by `spell_gate` on the spell it teaches
    /// (WrongClass → "(You can't use)", TooPowerful → "(Too powerful)");
    /// a scroll whose taught spell id resolves to nothing can never be
    /// learned, so it annotates like WrongClass. Everything else keeps the
    /// M4 `user_can_use` gate.
    fn list_row_suffix(
        &self,
        session: SessionId,
        item: &crate::content::Item,
    ) -> Option<&'static str> {
        let taught = item
            .abilities
            .iter()
            .find_map(|(a, v)| (*a == Ability::LearnSp).then_some(*v));
        let Some(taught) = taught else {
            return (!self.user_can_use(self.player(session), item))
                .then_some(text::CANT_USE_SUFFIX);
        };
        let spell = u16::try_from(taught)
            .ok()
            .map(SpellId)
            .and_then(|id| self.content.spells.get(&id));
        let Some(spell) = spell else {
            return Some(text::CANT_USE_SUFFIX);
        };
        match self.spell_gate(self.player(session), spell) {
            SpellGate::Ok => None,
            SpellGate::WrongClass => Some(text::CANT_USE_SUFFIX),
            SpellGate::TooPowerful => Some(text::TOO_POWERFUL_SUFFIX),
        }
    }

    /// `display_shop_items`: shelf price = cost x (markup+100)/100 — the
    /// Charm haggle applies only at purchase (economy.md §2.1).
    fn list_command(&mut self, session: SessionId) -> Resolution {
        let Some(shop_id) = self.shop_here(session) else {
            // Unlike buy/sell, bare LIST refuses instead of falling to say
            // (oracle_healer_gates.raw at the Temple Healer).
            self.output_line(session, text::NOT_IN_SHOP_LIST);
            return Resolution::Handled;
        };
        let shop = &self.content.shops[&shop_id];
        let counts = self.shop_stock.get(&shop_id).copied().unwrap_or_default();
        // The header prints lazily on the first stocked row, and
        // zero-stock rows are hidden — an empty shop lists nothing at all
        // (display_shop_items 0x487xx; healer shops are silent).
        let mut out = String::new();
        for (i, slot) in shop.stock.iter().enumerate() {
            let Some(item_id) = slot.item else { continue };
            if counts[i] == 0 {
                continue;
            }
            let Some(item) = self.content.items.get(&item_id) else {
                continue;
            };
            if out.is_empty() {
                out.push_str(text::SHOP_HEADER);
                out.push('\n');
            }
            // Shelf value in the item's OWN cost denomination (oracle:
            // "4 gold crowns" for a 2-gold lantern at markup 100).
            let shelf = i64::from(item.cost) * (i64::from(shop.markup) + 100) / 100;
            let row = if shelf == 0 {
                text::shop_row_free(&item.name, counts[i])
            } else {
                text::shop_row_priced(
                    &item.name,
                    counts[i],
                    shelf,
                    item.cost_denomination as usize,
                )
            };
            out.push_str(&row);
            if let Some(suffix) = self.list_row_suffix(session, item) {
                out.push_str(suffix);
            }
            out.push('\n');
        }
        if !out.is_empty() {
            self.output(session, &out);
        }
        Resolution::Handled
    }

    /// The item's base cost in copper.
    fn item_base_copper(&self, item: &crate::content::Item) -> i64 {
        let ratios = self.config.coin_ratios;
        let mut value = i64::from(item.cost.max(0));
        for r in ratios.iter().take(item.cost_denomination.max(0) as usize) {
            value *= *r as i64;
        }
        value
    }

    /// `buy_item` (economy.md §2.1): price = base x (markup+100)/100
    /// x (110 - Charm/5)/100 copper.
    fn buy_command(&mut self, session: SessionId, target: &str) -> Resolution {
        let want = target.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Resolution::FallThrough;
        }
        let Some(shop_id) = self.shop_here(session) else {
            return Resolution::FallThrough;
        };
        let shop = self.content.shops[&shop_id].clone();
        // Healer shops (type 5) sell services, not items (economy.md §2;
        // strings VERIFIED oracle_healer2.raw).
        if shop.shop_type == 5 {
            return self.buy_healer_service(session, &want);
        }
        let slot = shop.stock.iter().enumerate().find(|(_, s)| {
            s.item.is_some_and(|id| {
                self.content
                    .items
                    .get(&id)
                    .is_some_and(|i| word_prefix_match(&i.name, &want))
            })
        });
        let Some((idx, slot)) = slot else {
            self.output_line(session, &text::not_known_item(target.trim()));
            return Resolution::Handled;
        };
        let item_id = slot.item.expect("matched");
        let item = self.content.items[&item_id].clone();
        let counts = self.shop_stock.entry(shop_id).or_default();
        if counts[idx] < 1 {
            self.output_line(session, &text::cannot_buy_here(&item.name));
            return Resolution::Handled;
        }

        let base = self.item_base_copper(&item);
        let charm = i64::from(self.player(session).stats.charm);
        let price = (110 - charm / 5) * (base * (i64::from(shop.markup) + 100) / 100) / 100;
        let ratios = self.config.coin_ratios;
        if price > 0
            && self.player(session).coins.total_copper(ratios) < price as u64
        {
            self.output_line(session, &text::cannot_afford(&item.name));
            return Resolution::Handled;
        }
        let counts = self.shop_stock.entry(shop_id).or_default();
        counts[idx] -= 1;
        self.persist_shop_stock(shop_id);
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::FallThrough;
        };
        let spent = if price > 0 {
            player.coins.deduct_copper(price as u64, ratios)
        } else {
            Coins::default()
        };
        player.inventory.push((item_id, item.uses));
        let msg = if price == 0 {
            text::bought_free(&item.name)
        } else {
            // The coins actually handed over, high->low (deduct_currency
            // tell line: "for 2 gold crowns, 8 copper farthings").
            let listing = text::coin_listing([
                spent.copper,
                spent.silver,
                spent.gold,
                spent.platinum,
                spent.runic,
            ])
            .unwrap_or_else(|| text::copper_amount(price as u64));
            text::bought_for(&item.name, &listing)
        };
        self.output_line(session, &msg);
        Resolution::Handled
    }

    /// Healer services: `buy healing` = (max−cur)×2 copper, full heal;
    /// `buy curing`/`buy cure poison` = 25 SILVER if poisoned (cures +
    /// terminates poison-carrying slots) or 15 SILVER when not — the
    /// constants ride in the silver arg of check_currency (live: 150
    /// copper, oracle_healer2.raw; the 25-silver poisoned price is
    /// decompile-only — ORACLE-VERIFY, needs a poisoned live probe).
    fn buy_healer_service(&mut self, session: SessionId, want: &str) -> Resolution {
        let ratios = self.config.coin_ratios;
        if word_prefix_match("healing", want) {
            let missing = {
                let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session)
                else {
                    return Resolution::FallThrough;
                };
                i64::from(derived.max_hp) - i64::from(player.current_hp)
            };
            let cost = (missing.max(0) * 2) as u64;
            if self.player(session).coins.total_copper(ratios) < cost {
                // ORACLE-VERIFY the cannot-afford-healing wording.
                self.output_line(session, &text::cannot_afford("healing"));
                return Resolution::Handled;
            }
            let Some(Session::InGame { player, derived, .. }) = self.sessions.get_mut(&session)
            else {
                return Resolution::FallThrough;
            };
            let spent = player.coins.deduct_copper(cost, ratios);
            player.current_hp = derived.max_hp;
            let coins = text::coin_listing([
                spent.copper,
                spent.silver,
                spent.gold,
                spent.platinum,
                spent.runic,
            ])
            .unwrap_or_else(|| "nothing".into());
            self.output_line(session, &text::healed(&coins));
            return Resolution::Handled;
        }
        if word_prefix_match("curing", want) || word_prefix_match("cure poison", want) {
            // Poisoned = 25 silver, cures; not poisoned = 15 silver, the
            // wasted-purchase line. Both constants ride the SILVER arg of
            // check_currency (decompile buy_item 14297/14335; economy.md
            // §2 + addendum — the addendum's live capture pinned the
            // not-poisoned 15 at 150 copper, and 0x19=25 sits in the SAME
            // argument slot, so the poisoned price is 25 SILVER — the
            // plan's "25 gold" was a memory transcription error).
            let poisoned = self.player(session).poison > 0;
            let cost = if poisoned { 25 } else { 15 } * ratios[0];
            if self.player(session).coins.total_copper(ratios) < cost {
                self.output_line(session, &text::cannot_afford("curing"));
                return Resolution::Handled;
            }
            let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
                return Resolution::FallThrough;
            };
            let spent = player.coins.deduct_copper(cost, ratios);
            let coins = text::coin_listing([
                spent.copper,
                spent.silver,
                spent.gold,
                spent.platinum,
                spent.runic,
            ])
            .unwrap_or_else(|| "nothing".into());
            if !poisoned {
                self.output_line(session, &text::not_poisoned(&coins));
                return Resolution::Handled;
            }
            player.poison = 0;
            self.output_line(session, &text::poisoning_cured(&coins));
            // Slot sweep (14306-14328): every active spell CARRYING
            // Poison(19) is cleared and terminated with its stored value,
            // chain HONORED ('\x01' at 14322); unknown-spell slots are
            // bare-cleared (terminate_active_spell's unknown arm matches
            // the DLL's inline zeroing); other slots survive. The
            // terminations' own poison subtractions floor at the already
            // cleared 0.
            for idx in 0..10 {
                let Some(spell_id) = self.player(session).active_spells[idx].spell else {
                    continue;
                };
                let carries_poison = self
                    .content
                    .spells
                    .get(&spell_id)
                    .is_none_or(|sp| sp.abilities.iter().any(|(a, _)| *a == Ability::Poison));
                if carries_poison {
                    self.terminate_active_spell(session, idx, true);
                }
            }
            return Resolution::Handled;
        }
        self.output_line(session, &text::not_known_item(want));
        Resolution::Handled
    }

    /// `sell_item` (economy.md §3.1): sellback = base x (Charm/2 + 25)/100,
    /// minted upward; the shop must stock the item, and restocks below max.
    fn sell_command(&mut self, session: SessionId, target: &str) -> Resolution {
        let want = target.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Resolution::FallThrough;
        }
        let Some(shop_id) = self.shop_here(session) else {
            return Resolution::FallThrough;
        };
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::FallThrough;
        };
        let pos = player.inventory.iter().position(|(id, _)| {
            self.content
                .items
                .get(id)
                .is_some_and(|i| word_prefix_match(&i.name, &want))
        });
        let Some(pos) = pos else {
            self.output_line(session, &format!("You don't have {} to sell!", target.trim()));
            return Resolution::Handled;
        };
        let item_id = player.inventory[pos].0;
        let item = self.content.items[&item_id].clone();
        let shop = self.content.shops[&shop_id].clone();
        let slot_idx = shop
            .stock
            .iter()
            .position(|s| s.item == Some(item_id));
        let Some(slot_idx) = slot_idx else {
            self.output_line(session, &text::cannot_sell_here(&item.name));
            return Resolution::Handled;
        };

        let base = self.item_base_copper(&item);
        let charm = i64::from(self.player(session).stats.charm);
        let price = (charm / 2 + 25) * base / 100;
        let ratios = self.config.coin_ratios;
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::FallThrough;
        };
        player.inventory.remove(pos);
        player.coins.add_copper_and_mint(price.max(0) as u64, ratios);
        let counts = self.shop_stock.entry(shop_id).or_default();
        if counts[slot_idx] < shop.stock[slot_idx].max {
            counts[slot_idx] += 1;
            self.persist_shop_stock(shop_id);
        }
        self.output_line(
            session,
            &text::sold_for(&item.name, &text::copper_amount(price.max(0) as u64)),
        );
        Resolution::Handled
    }

    /// Coin denomination names (low->high) for pile pickup by name.\n    /// Coin denomination names (low->high) for pile pickup by name.
    const COIN_WORDS: [(&'static str, usize); 5] = [
        ("copper", 0),
        ("silver", 1),
        ("gold", 2),
        ("platinum", 3),
        ("runic", 4),
    ];

    /// `get <item|coins>`: floor coins by denomination name, else a
    /// gettable floor item by word-prefix. Fixture items respond with
    /// "You don't see X here." (oracle); nothing matched falls to say.
    fn get_command(&mut self, session: SessionId, target: &str) -> Resolution {
        let room = self.player(session).location;
        let want = target.trim().to_ascii_lowercase();

        // Coins first: "get silver" empties the silver pile.
        for (word, idx) in Self::COIN_WORDS {
            if word.starts_with(&want) || want.starts_with(word) {
                let count = self
                    .room_coins
                    .get(&room)
                    .map_or(0, |p| p[idx]);
                let (one, many) = text::COIN_NAMES[idx];
                if count == 0 {
                    self.output_line(session, &text::dont_see_coins(many));
                    return Resolution::Handled;
                }
                self.room_coins.get_mut(&room).expect("checked")[idx] = 0;
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    match idx {
                        0 => player.coins.copper += count,
                        1 => player.coins.silver += count,
                        2 => player.coins.gold += count,
                        3 => player.coins.platinum += count,
                        _ => player.coins.runic += count,
                    }
                }
                self.output_line(session, &text::took_coins(count, one, many));
                return Resolution::Handled;
            }
        }

        // Floor items by word-prefix (gettable only picks up; a matching
        // fixture answers "don't see").
        let found = self.find_floor_item(room, &want);
        match found {
            FloorMatch::Gettable(pos) => {
                let (item, uses) = self.room_items.get_mut(&room).expect("has items").remove(pos);
                let name = self.content.items[&item].name.clone();
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    player.inventory.push((item, uses));
                }
                self.output_line(session, &text::took_item(&name));
                Resolution::Handled
            }
            FloorMatch::Fixture => {
                self.output_line(session, &text::dont_see_here(target.trim()));
                Resolution::Handled
            }
            FloorMatch::None => Resolution::FallThrough,
        }
    }

    fn find_floor_item(&self, room: RoomId, want: &str) -> FloorMatch {
        let Some(items) = self.room_items.get(&room) else {
            return FloorMatch::None;
        };
        let mut fixture = false;
        for (pos, (id, _)) in items.iter().enumerate() {
            let Some(item) = self.content.items.get(id) else {
                continue;
            };
            if word_prefix_match(&item.name, want) {
                if item.gettable != 0 {
                    return FloorMatch::Gettable(pos);
                }
                fixture = true;
            }
        }
        if fixture {
            FloorMatch::Fixture
        } else {
            FloorMatch::None
        }
    }

    /// `drop <item>` — works even on the armed weapon (oracle).
    fn drop_command(&mut self, session: SessionId, target: &str) -> Resolution {
        let want = target.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Resolution::FallThrough;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::FallThrough;
        };
        let room = player.location;
        // The armed weapon can be dropped directly (oracle).
        if let Some((id, uses)) = player.weapon
            && self
                .content
                .items
                .get(&id)
                .is_some_and(|i| word_prefix_match(&i.name, &want))
        {
            let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
                return Resolution::FallThrough;
            };
            player.weapon = None;
            let name = self.content.items[&id].name.clone();
            self.room_items.entry(room).or_default().push((id, uses));
            self.refresh_derived(session);
            self.output_line(session, &text::dropped_item(&name));
            return Resolution::Handled;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::FallThrough;
        };
        let pos = player.inventory.iter().position(|(id, _)| {
            self.content
                .items
                .get(id)
                .is_some_and(|i| word_prefix_match(&i.name, &want))
        });
        let Some(pos) = pos else {
            // A named but absent item gets the oracle error; resolve the
            // display name from content when we can.
            let known = self
                .content
                .items
                .values()
                .any(|i| word_prefix_match(&i.name, &want));
            let _ = known;
            self.output_line(session, &text::dont_have_to_drop(target.trim()));
            return Resolution::Handled;
        };
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::FallThrough;
        };
        let (item, uses) = player.inventory.remove(pos);
        let name = self.content.items[&item].name.clone();
        self.room_items.entry(room).or_default().push((item, uses));
        self.output_line(session, &text::dropped_item(&name));
        Resolution::Handled
    }

    /// `use`/`read <item>` — the LearnSp(42) scroll path (spellcasting.md
    /// §8.4). Both verbs share the learn handler; only the epilogue
    /// differs (`use` prints a trailing blank line, `read` the
    /// disintegrate line). Every refusal keeps the item.
    fn use_command(&mut self, session: SessionId, target: &str, read_verb: bool) -> Resolution {
        let want = target.trim().to_ascii_lowercase();
        if want.is_empty() {
            // ORACLE-VERIFY: bare use/read behavior unmeasured — falling
            // to the say fallback is a guess.
            return Resolution::FallThrough;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::FallThrough;
        };
        let pos = player.inventory.iter().position(|(id, _)| {
            self.content
                .items
                .get(id)
                .is_some_and(|i| word_prefix_match(&i.name, &want))
        });
        let Some(pos) = pos else {
            return self.use_unowned(session, target.trim(), &want, read_verb);
        };
        let item_id = player.inventory[pos].0;
        let item = &self.content.items[&item_id];
        let item_name = item.name.clone();
        // LearnSp's value is the taught spell id. Other usable item kinds
        // (charged items, light sources — "You lit the torch.",
        // oracle_use_verbs.raw) arrive with the item-charges slice and
        // slot in ahead of the refusal below.
        let taught = item
            .abilities
            .iter()
            .find_map(|(a, v)| (*a == Ability::LearnSp).then_some(*v))
            .and_then(|v| u16::try_from(v).ok())
            .map(SpellId)
            .and_then(|id| self.content.spells.get(&id));
        let Some(spell) = taught else {
            // VERIFIED (oracle_use_verbs2.raw): both verbs refuse an
            // owned non-LearnSp item and keep it.
            self.output_line(session, text::MAY_NOT_USE_ITEM);
            return Resolution::Handled;
        };
        let (spell_id, spell_name) = (spell.id, spell.name.clone());
        if self.spell_gate(self.player(session), spell) != SpellGate::Ok {
            // VERIFIED (§8.4): too-high or wrong-class — not consumed, so
            // the book can never hold an uncastable-yet spell.
            self.output_line(session, text::MAY_NOT_USE_ITEM);
            return Resolution::Handled;
        }
        if self.player(session).spellbook.contains_key(&spell_id) {
            // VERIFIED (oracle_use_verbs.raw): identical for both verbs;
            // not consumed, book unchanged.
            self.output_line(session, text::ALREADY_KNOW_SCROLL);
            return Resolution::Handled;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            unreachable!("session verified in-game above");
        };
        player.inventory.remove(pos);
        player.spellbook.insert(spell_id, false);
        let snapshot: Box<Player> = player.clone();
        let mut out = text::learned_spell(&item_name, &spell_name);
        out.push('\n');
        if read_verb {
            out.push_str(text::SCROLL_DISINTEGRATES);
        }
        out.push('\n');
        self.output(session, &out);
        self.events.push(Event::Persist(snapshot));
        Resolution::Handled
    }

    /// The unowned arm of `use`/`read`: `use` never reads the shelf
    /// ("You don't have {arg}." — oracle_use_verbs.raw); `read` prints the
    /// description paragraph of a visible shop-shelf or floor item, else
    /// "You do not see {arg} here!".
    fn use_unowned(
        &mut self,
        session: SessionId,
        raw: &str,
        want: &str,
        read_verb: bool,
    ) -> Resolution {
        if !read_verb {
            self.output_line(session, &text::dont_have(raw));
            return Resolution::Handled;
        }
        match self.visible_item_description(session, want) {
            Some(lines) => {
                let paragraph = text::item_description(&lines);
                // ORACLE-VERIFY: empty-description item output unmeasured
                // (we print nothing at all).
                if !paragraph.is_empty() {
                    self.output_line(session, &paragraph);
                }
            }
            None => self.output_line(session, &text::do_not_see_here(raw)),
        }
        Resolution::Handled
    }

    /// The description of an item visible to the player without owning
    /// it: the shop shelf (stocked rows only, like LIST), then the floor.
    /// Only the shelf case is oracle-measured; the relative priority never
    /// co-occurred (ORACLE-VERIFY).
    fn visible_item_description(&self, session: SessionId, want: &str) -> Option<Vec<String>> {
        if let Some(shop_id) = self.shop_here(session) {
            let shop = &self.content.shops[&shop_id];
            let counts = self.shop_stock.get(&shop_id).copied().unwrap_or_default();
            for (i, slot) in shop.stock.iter().enumerate() {
                let Some(item_id) = slot.item else { continue };
                if counts[i] == 0 {
                    continue;
                }
                if let Some(item) = self.content.items.get(&item_id)
                    && word_prefix_match(&item.name, want)
                {
                    return Some(item.description.clone());
                }
            }
        }
        let room = self.player(session).location;
        for (id, _) in self.room_items.get(&room).into_iter().flatten() {
            if let Some(item) = self.content.items.get(id)
                && word_prefix_match(&item.name, want)
            {
                return Some(item.description.clone());
            }
        }
        None
    }

    /// The inventory display (oracle-exact four lines).
    fn show_inventory(&mut self, session: SessionId) {
        let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session) else {
            return;
        };
        let mut out = String::new();
        let mut names: Vec<String> = Vec::new();
        let c = &player.coins;
        if let Some(coins) =
            text::coin_listing([c.copper, c.silver, c.gold, c.platinum, c.runic])
        {
            names.push(coins);
        }
        // Worn items first, suffixed with their wear location; then the
        // armed weapon with its hand suffix; then loose inventory grouped
        // by item with a count prefix ("3 sickle") — the original's
        // aggregate-array walk (oracle_m4_verify.raw).
        for (id, _) in &player.worn {
            if let Some(item) = self.content.items.get(id) {
                names.push(format!(
                    "{} ({})",
                    item.name,
                    text::worn_location(item.worn_on)
                ));
            }
        }
        if let Some((id, _)) = player.weapon
            && let Some(item) = self.content.items.get(&id)
        {
            let hands = if item.item_type == 1
                && (item.weapon_type == 1 || item.weapon_type == 3)
            {
                "(Two handed)"
            } else {
                "(Weapon Hand)"
            };
            names.push(format!("{} {hands}", item.name));
        }
        let mut grouped: Vec<(crate::content::ItemId, i32)> = Vec::new();
        for (id, _) in &player.inventory {
            match grouped.iter_mut().find(|(gid, _)| gid == id) {
                Some((_, n)) => *n += 1,
                None => grouped.push((*id, 1)),
            }
        }
        for (id, n) in grouped {
            if let Some(item) = self.content.items.get(&id) {
                if n > 1 {
                    names.push(format!("{n} {}", item.name));
                } else {
                    names.push(item.name.clone());
                }
            }
        }
        if names.is_empty() {
            out.push_str(text::CARRYING_NOTHING);
            out.push('\n');
        } else {
            out.push_str(&format!("You are carrying {}\n", names.join(", ")));
        }
        out.push_str(text::NO_KEYS);
        out.push('\n');
        // ORACLE-VERIFY: mixed-denomination wealth display (broke chars show
        // "0 copper farthings"; we render the total copper value).
        let total = player.coins.total_copper(self.config.coin_ratios);
        out.push_str(&format!(
            "Wealth: {total} {}\n",
            if total == 1 { "copper farthing" } else { "copper farthings" }
        ));
        let weight = self.carried_weight(player);
        let capacity = self.carry_capacity(player, derived);
        let percent = if capacity > 0 { weight * 100 / capacity } else { 0 };
        out.push_str(&format!(
            "Encumbrance: {weight}/{capacity} - {} [{percent}%]\n",
            text::encumbrance_descriptor(percent)
        ));
        self.output(session, &out);
    }

    /// Item weight (carried + worn + wielded) + coin weight (1/3 each).
    fn carried_weight(&self, player: &Player) -> i64 {
        let items: i64 = player
            .inventory
            .iter()
            .chain(player.worn.iter())
            .chain(player.weapon.iter())
            .filter_map(|(id, _)| self.content.items.get(id))
            .map(|i| i64::from(i.weight))
            .sum();
        let c = &player.coins;
        let coins: i64 = [c.copper, c.silver, c.gold, c.platinum, c.runic]
            .iter()
            .map(|&n| i64::from(n) / 3)
            .sum();
        items + coins
    }

    /// `get_max_weight`: the Str-derived cap scaled by the Encum ability
    /// percent (oracle: Dwarf 2400 * 1.2 = 2880).
    fn carry_capacity(&self, player: &Player, derived: &Derived) -> i64 {
        let encum = self.ability_bag(player).value(encum_ability());
        i64::from(derived.carry_capacity) * (100 + i64::from(encum)) / 100
    }

    /// The player's encumbrance percent (`+0x708`), feeding combat.
    fn encumbrance_percent(&self, session: SessionId) -> i32 {
        let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session) else {
            return 0;
        };
        let capacity = self.carry_capacity(player, derived);
        if capacity <= 0 {
            return 100;
        }
        (self.carried_weight(player) * 100 / capacity) as i32
    }

    /// Name match among the room's live monsters. Each input word must
    /// prefix-match consecutive words of the name, starting at any word:
    /// "kobold thief", "kobold", "thief", and "kob th" all match
    /// "kobold thief".
    fn find_monster(&self, room: RoomId, words: &str) -> Option<MonsterInstanceId> {
        let want: Vec<String> = words
            .trim()
            .to_ascii_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if want.is_empty() {
            return None;
        }
        self.monsters
            .iter()
            .filter(|(_, m)| m.location == room && m.current_hp > 0)
            .find(|(_, m)| {
                self.content.monsters.get(&m.template).is_some_and(|t| {
                    let name: Vec<&str> = t.name.split_whitespace().collect();
                    (0..name.len()).any(|start| {
                        want.len() <= name.len() - start
                            && want.iter().enumerate().all(|(i, w)| {
                                name[start + i].to_ascii_lowercase().starts_with(w)
                            })
                    })
                })
            })
            .map(|(id, _)| *id)
    }

    /// `background_energy`: regenerate energy, then run the two combat
    /// drivers in a coin-flipped order (anti first-strike bias).
    fn energy_round(&mut self) {
        // energy_update_character: cur += max; clamp unless in autocombat.
        // The new round also re-arms the one-cast-per-round gate.
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in &sessions {
            if let Some(Session::InGame { energy, target, cast_this_round, .. }) =
                self.sessions.get_mut(id)
            {
                *energy += PLAYER_ENERGY_MAX;
                if target.is_none() && *energy > PLAYER_ENERGY_MAX {
                    *energy = PLAYER_ENERGY_MAX;
                }
                *cast_this_round = false;
            }
        }
        // energy_update_monster: always clamps.
        let monster_ids: Vec<MonsterInstanceId> = self.monsters.keys().copied().collect();
        for id in &monster_ids {
            let max = self
                .monsters
                .get(id)
                .and_then(|m| self.content.monsters.get(&m.template))
                .map_or(0, |t| t.energy);
            if let Some(m) = self.monsters.get_mut(id) {
                m.energy = (m.energy + max).min(max);
            }
        }

        if self.rng.roll(0, 100) < 60 {
            self.player_combat_driver(&sessions);
            self.monster_combat_driver(&monster_ids);
        } else {
            self.monster_combat_driver(&monster_ids);
            self.player_combat_driver(&sessions);
        }
    }

    fn player_combat_driver(&mut self, sessions: &[SessionId]) {
        for id in sessions {
            self.player_attack_sequence(*id);
        }
    }

    fn monster_combat_driver(&mut self, monsters: &[MonsterInstanceId]) {
        for id in monsters {
            self.monster_attack_sequence(*id);
        }
    }

    /// `validate_auto_combat` + `attack_user_monster`: up to 6 swings gated
    /// by the energy pool.
    fn player_attack_sequence(&mut self, session: SessionId) {
        let Some(Session::InGame { player, target: Some(target), casting, .. }) =
            self.sessions.get(&session)
        else {
            return;
        };
        let target = *target;
        let casting = *casting;
        // Helpless players don't swing.
        if player.current_hp < 1 {
            return;
        }
        // Target gone or moved: combat breaks.
        let valid = self
            .monsters
            .get(&target)
            .is_some_and(|m| m.current_hp > 0 && m.location == player.location);
        if !valid {
            self.break_combat(session);
            return;
        }
        // A cast engagement re-fires its spell instead of swinging
        // (MEASURED §8.9: the unprompted mmis line the round after a
        // failed roll, mana charged again).
        if let Some(spell_id) = casting {
            self.offensive_cast_attempt(session, spell_id, target);
            return;
        }

        let attacker = self.build_player_attacker(session);
        let eu = self.player_energy_used(session);
        let target_name = self
            .monsters
            .get(&target)
            .and_then(|m| self.content.monsters.get(&m.template))
            .map(|t| t.name.clone())
            .expect("validated above");
        let (hit_verbs, miss_verbs) = self.weapon_verbs(session);

        let mut swings = 0;
        while swings <= 5 {
            swings += 1;
            let Some(Session::InGame { energy, .. }) = self.sessions.get_mut(&session) else {
                return;
            };
            if *energy < eu {
                break;
            }
            *energy -= eu;

            let defender = self.build_monster_defender(target);
            let rng = &mut self.rng;
            let result = crate::combat::calculate_attack(
                &attacker,
                &defender,
                crate::combat::AttackType::Normal,
                &mut |lo, hi| rng.roll(lo, hi),
            );
            use crate::combat::Outcome;
            let hit_verb = {
                let n = hit_verbs.len().max(1) as i32;
                let pick = if hit_verbs.len() > 1 { self.rng.roll(0, n - 1) } else { 0 };
                hit_verbs.get(pick as usize).cloned().unwrap_or_else(|| "punch".into())
            };
            let miss_verb = {
                let n = miss_verbs.len().max(1) as i32;
                let pick = if miss_verbs.len() > 1 { self.rng.roll(0, n - 1) } else { 0 };
                miss_verbs.get(pick as usize).cloned().unwrap_or_else(|| "swing at".into())
            };
            match result.outcome {
                // combat_rounds.md §5: result 3 renders distinct
                // dodge/parry flavor for the player view too — this
                // conflation is a pre-existing M3 gap. ORACLE-VERIFY:
                // needs a player-view capture of a monster
                // dodging/parrying a swing.
                Outcome::Dodged | Outcome::Parried => {
                    self.output_line(session, &text::player_miss(&miss_verb, &target_name));
                }
                Outcome::NoDamage => {
                    self.output_line(session, &text::player_glance(&miss_verb, &target_name));
                }
                Outcome::Hit | Outcome::Critical => {
                    let msg = if result.outcome == Outcome::Critical {
                        text::player_crit(&hit_verb, &target_name, result.damage)
                    } else {
                        text::player_hit(&hit_verb, &target_name, result.damage)
                    };
                    self.output_line(session, &msg);
                    let dead = {
                        let m = self.monsters.get_mut(&target).expect("validated");
                        m.current_hp -= result.damage;
                        // Retaliation: the victim locks onto its attacker.
                        m.target = Some(session);
                        m.current_hp <= 0
                    };
                    if dead {
                        self.monster_killed(target, session);
                        return;
                    }
                }
            }
        }
        // Re-mark retaliation even on whiffed rounds.
        if let Some(m) = self.monsters.get_mut(&target)
            && m.target.is_none()
        {
            m.target = Some(session);
        }
    }

    /// `attack_monster_user`: form selection by cumulative weight, then the
    /// same 6-swing energy loop.
    fn monster_attack_sequence(&mut self, id: MonsterInstanceId) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        if m.current_hp <= 0 {
            return;
        }
        let Some(victim) = m.target else {
            return;
        };
        let template = m.template;
        let location = m.location;
        // Victim gone (moved/quit/died-and-respawned elsewhere): drop target.
        let victim_here = matches!(
            self.sessions.get(&victim),
            Some(Session::InGame { player, .. }) if player.location == location
        );
        if !victim_here {
            if let Some(m) = self.monsters.get_mut(&id) {
                m.target = None;
            }
            return;
        }

        let tpl = self.content.monsters.get(&template).expect("live instance");
        let forms = tpl.attacks;
        // Any incoming attack sequence cancels a pending exit (combat
        // logout guard; DLL: stop_users_exit fires before the swing loop).
        self.cancel_exit(victim);

        let mut swings = 0;
        while swings <= 5 {
            swings += 1;
            // Pick a form by the cumulative weight table.
            let pick = self.rng.roll(0, 100);
            let form = forms
                .iter()
                .find(|f| f.kind != 0 && pick < i32::from(f.weight))
                .copied();
            let Some(form) = form else {
                continue; // action 0: no attack this swing
            };
            if form.kind != 1 {
                continue; // cast/rob forms arrive in M5+
            }
            let Some(mi) = self.monsters.get_mut(&id) else {
                return;
            };
            if mi.energy < i32::from(form.energy) {
                break;
            }
            mi.energy -= i32::from(form.energy);

            let attacker = crate::combat::Fighter {
                accuracy: i32::from(form.accuracy),
                evasion_a: 0,
                evasion_b: 0,
                armor: 0,
                min_damage: i32::from(form.min_damage),
                max_damage: i32::from(form.max_damage),
                parry: 0,
                crit_rating: 0, // monsters never crit (hard-zeroed)
            };
            let defender = self.build_player_defender(victim);
            let rng = &mut self.rng;
            let result = crate::combat::calculate_attack(
                &attacker,
                &defender,
                crate::combat::AttackType::Normal,
                &mut |lo, hi| rng.roll(lo, hi),
            );
            use crate::combat::Outcome;
            let (victim_line, room_line) =
                self.monster_swing_lines(template, &form, victim, result.outcome, result.damage);
            self.output_line(victim, &victim_line);
            self.broadcast_to_room(location, Some(victim), &room_line);
            if matches!(result.outcome, Outcome::Hit | Outcome::Critical) {
                let (was_up, now_hp, victim_name, room) = {
                    let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&victim)
                    else {
                        return;
                    };
                    let was_up = player.current_hp >= 1;
                    player.current_hp -= result.damage;
                    (was_up, player.current_hp, player.name.clone(), player.location)
                };
                if was_up && now_hp < 1 {
                    self.output_line(victim, &text::drops_to_ground(&victim_name));
                    self.broadcast_to_room(room, Some(victim), &text::drops_to_ground(&victim_name));
                }
                if now_hp <= DEATH_FLOOR {
                    self.player_killed(victim);
                    return;
                }
            }
        }
    }

    /// The victim + room lines for one monster swing (`spellcasting.md`
    /// §8.10; decompile `attack_monster_user` 0x2e34b). Templates come from
    /// the form's three message records — hit: line 1 victim / line 2 room;
    /// dodge record: line 1/2 the glance pair, line 3 the victim's parry
    /// ("dodge") line; miss record: line 1 the room parry line, lines 2/3
    /// the plain-miss pair. Outcome mapping per `calculate_attack` 0x2b800:
    /// a failed to-hit roll is result 0 = PLAIN miss; a parry is result 3 =
    /// the ", but you dodge" framing; damage < 1 is result 1 = glance.
    /// The `%s` verb/weapon slots are filled from the wielded weapon's
    /// records (`move_monster_to_fighter` 1040:1739): the HIT record's
    /// lines 2/3 fill the hit-verb slots, the miss record's lines 2/3 the
    /// swing-verb slots (line 2 victim-view, line 3 room-view, chosen from
    /// the `|` pool like player weapon verbs), with the swing slots
    /// falling back to the hit verbs when the weapon has no miss record.
    /// All render empty for unarmed monsters — exactly the giant rat's
    /// shape. Record-less forms (healer 47, zombie 492, ju-ju zombie
    /// 493/772) compose whole lines from the generic seg-1140 templates
    /// with those same buffers.
    fn monster_swing_lines(
        &mut self,
        template: crate::content::MonsterId,
        form: &crate::content::AttackForm,
        victim: SessionId,
        outcome: crate::combat::Outcome,
        damage: i32,
    ) -> (String, String) {
        let tpl = &self.content.monsters[&template];
        let name = tpl.name.clone();
        let weapon = tpl.weapon.and_then(|id| self.content.items.get(&id));
        let weapon_name = weapon.map(|i| i.name.clone()).unwrap_or_default();
        let pools = |msg: Option<crate::content::MessageId>| -> Option<(Vec<String>, Vec<String>)> {
            msg.and_then(|m| self.content.messages.get(&m)).map(|m| {
                let split = |i: usize| -> Vec<String> {
                    m.lines
                        .get(i)
                        .map(|l| l.split('|').map(str::to_owned).collect())
                        .unwrap_or_default()
                };
                (split(1), split(2))
            })
        };
        let (hit_pool2, hit_pool3) =
            pools(weapon.and_then(|i| i.hit_msg)).unwrap_or_default();
        // Swing verbs default to the hit verbs when the weapon carries no
        // miss record (move_monster_to_fighter's buffer-copy fallback).
        let (pool2, pool3) = pools(weapon.and_then(|i| i.miss_msg))
            .unwrap_or_else(|| (hit_pool2.clone(), hit_pool3.clone()));
        let pick = |pool: &[String], rng: &mut Rng| -> String {
            match pool.len() {
                0 => String::new(),
                1 => pool[0].clone(),
                n => pool[rng.roll(0, n as i32 - 1) as usize].clone(),
            }
        };
        let verb2 = pick(&pool2, &mut self.rng);
        let verb3 = pick(&pool3, &mut self.rng);
        let hit_verb2 = pick(&hit_pool2, &mut self.rng);
        let hit_verb3 = pick(&hit_pool3, &mut self.rng);
        let (victim_name, gender) = match self.sessions.get(&victim) {
            Some(Session::InGame { player, .. }) => (player.name.clone(), player.gender),
            _ => (String::new(), Gender::Male),
        };
        // FUN_0041d89d / FUN_0041d91d: subject and possessive pronouns.
        let (subj, poss) = match gender {
            Gender::Male => ("he", "his"),
            Gender::Female => ("she", "her"),
        };
        let line = |id: Option<crate::content::MessageId>, i: usize| -> Option<&str> {
            id.and_then(|m| self.content.messages.get(&m))
                .and_then(|m| m.lines.get(i))
                .map(String::as_str)
        };
        use crate::combat::Outcome;
        let (v, r) = match outcome {
            Outcome::Hit | Outcome::Critical => {
                let dmg = damage.to_string();
                // Record-less: generic templates 1140:0x7b7 / 0x7d1 with
                // the weapon's hit verbs; a crit wraps the verb in the
                // "critically %s" template (1140:0xab2).
                let crit_wrap = |verb: &str| -> String {
                    if outcome == Outcome::Critical {
                        format!("critically {verb}")
                    } else {
                        verb.to_string()
                    }
                };
                let v = line(form.hit_msg, 0)
                    .map(|t| text::fill_message(t, &[&name, &dmg]))
                    .unwrap_or_else(|| {
                        text::fill_message(
                            text::MONSTER_HIT_TPL,
                            &[&name, &crit_wrap(&hit_verb2), &dmg],
                        )
                    });
                let r = line(form.hit_msg, 1)
                    .map(|t| text::fill_message(t, &[&name, &victim_name, &dmg]))
                    .unwrap_or_else(|| {
                        text::fill_message(
                            text::MONSTER_HIT_ROOM_TPL,
                            &[&name, &crit_wrap(&hit_verb3), &victim_name, &dmg],
                        )
                    });
                (v, r)
            }
            // DLL result 3: the parry branch needs BOTH records — the
            // victim line is dodge record line 3, the room line miss
            // record line 1 — else it composes 1140:0xfc5 / 0xfe8.
            Outcome::Parried => match (line(form.dodge_msg, 2), line(form.miss_msg, 0)) {
                (Some(tv), Some(tr)) => (
                    text::fill_message(tv, &[&name, &verb2, &weapon_name]),
                    text::fill_message(tr, &[&name, &verb3, &victim_name, &weapon_name, subj]),
                ),
                _ => (
                    text::fill_message(
                        text::MONSTER_DODGE_TPL,
                        &[&name, &verb2, &weapon_name],
                    ),
                    text::fill_message(
                        text::MONSTER_DODGE_ROOM_TPL,
                        &[&name, &verb3, &victim_name, &weapon_name, subj],
                    ),
                ),
            },
            // DLL result 1 (never oracle-observed — decompile-only,
            // ORACLE-VERIFY against an armoured victim). Record-less:
            // 1140:0xf6b / 0xf98, both on the victim-view swing verb.
            Outcome::NoDamage => {
                let v = line(form.dodge_msg, 0)
                    .map(|t| text::fill_message(t, &[&name, &verb2]))
                    .unwrap_or_else(|| {
                        text::fill_message(text::MONSTER_GLANCE_TPL, &[&name, &verb2])
                    });
                let r = line(form.dodge_msg, 1)
                    .map(|t| text::fill_message(t, &[&name, &verb2, &victim_name, poss]))
                    .unwrap_or_else(|| {
                        text::fill_message(
                            text::MONSTER_GLANCE_ROOM_TPL,
                            &[&name, &verb2, &victim_name, poss],
                        )
                    });
                (v, r)
            }
            // DLL result 0: the to-hit roll failed — the PLAIN miss pair;
            // record-less composes 1140:0x100e / 0x1022.
            Outcome::Dodged => {
                let v = line(form.miss_msg, 1)
                    .map(|t| text::fill_message(t, &[&name, &verb2, &weapon_name]))
                    .unwrap_or_else(|| {
                        text::fill_message(
                            text::MONSTER_MISS_TPL,
                            &[&name, &verb2, &weapon_name],
                        )
                    });
                let r = line(form.miss_msg, 2)
                    .map(|t| text::fill_message(t, &[&name, &verb3, &victim_name, &weapon_name]))
                    .unwrap_or_else(|| {
                        text::fill_message(
                            text::MONSTER_MISS_ROOM_TPL,
                            &[&name, &verb3, &victim_name, &weapon_name],
                        )
                    });
                (v, r)
            }
        };
        // The DLL touppers the first byte of every composed line.
        (text::capitalize_first(v), text::capitalize_first(r))
    }

    /// `check_kill_monster` + `distribute_experience` (`death.md` §4/§5).
    fn monster_killed(&mut self, id: MonsterInstanceId, killer: SessionId) {
        let Some(instance) = self.monsters.remove(&id) else {
            return;
        };
        let tpl = self
            .content
            .monsters
            .get(&instance.template)
            .expect("live instance has a template");
        let name = tpl.name.clone();
        // Coins drop into the room piles (template order is high->low).
        let piles = self.room_coins.entry(instance.location).or_insert([0; 5]);
        for (i, amount) in tpl.coins.iter().enumerate() {
            piles[4 - i] += amount;
        }
        // Carried loot drops silently (check_kill_monster: no message; the
        // wielded weapon is not in the drop loop and stays gone).
        self.room_items
            .entry(instance.location)
            .or_default()
            .extend(instance.items.iter().copied());
        let exp = u64::from(tpl.experience.max(0) as u32)
            * u64::from(tpl.exp_multi.max(1) as u32);
        let room = instance.location;

        self.output_line(killer, &text::monster_dead(&name));
        self.broadcast_to_room(room, Some(killer), &text::monster_dead(&name));

        // Equal split among the killer and everyone engaged on this target.
        let mut recipients: Vec<SessionId> = vec![killer];
        for (sid, session) in self.sessions.iter() {
            if let Session::InGame { target: Some(t), .. } = session
                && *t == id
                && *sid != killer
            {
                recipients.push(*sid);
            }
        }
        let share = (exp / recipients.len() as u64).max(1);
        for sid in recipients {
            if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&sid) {
                player.experience += share;
            }
            self.output_line(sid, &text::gain_experience(share));
            self.break_combat(sid);
        }
    }

    /// `check_kill_user`'s full-death branch (`death.md` §2/§3) — reached
    /// when HP hits the death floor.
    fn player_killed(&mut self, session: SessionId) {
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        let name = player.name.clone();
        let died_in = player.location;

        // Drop all money into the room piles.
        let coins = player.coins;
        player.coins = Coins::default();
        let piles = self.room_coins.entry(died_in).or_insert([0; 5]);
        piles[0] += coins.copper;
        piles[1] += coins.silver;
        piles[2] += coins.gold;
        piles[3] += coins.platinum;
        piles[4] += coins.runic;
        // (Inventory/key/worn drops join with items in M4.)

        self.output_line(session, "You have been killed!");
        self.broadcast_to_room(died_in, Some(session), &format!("{name} is dead."));
        self.break_combat_silent(session);
        self.release_monster_targets(session);
        // Death terminates every occupied slot, in slot order, with the
        // EndCast chain SUPPRESSED (decompile check_kill_user's death
        // branch 13053-13066 passes chainFlag '\0'; the stats-reset /
        // reroll path 10404-10419 is the one that passes '\x01').
        // Wear-off lines print to the dying player (44833-44844 prf to
        // the owner's terminal), and the recompute leaves the respawn
        // max HP buff-free.
        for idx in 0..10 {
            self.terminate_active_spell(session, idx, false);
        }
        // check_kill_user zeroes the poison counter AFTER the slot
        // terminations (decompile 13066) — the poison-spell reversals
        // subtract first, then the hard clear catches any remainder.
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.poison = 0;
        }

        let Some(Session::InGame { player, derived, aided, .. }) =
            self.sessions.get_mut(&session)
        else {
            return;
        };
        player.lives = player.lives.saturating_sub(1);
        if player.lives < 1 {
            // Permadeath (`death.md` §2c).
            let name = player.name.clone();
            self.output_line(session, "You have no lives remaining!");
            self.sessions.remove(&session);
            self.events.push(Event::DeleteCharacter(name));
            self.events.push(Event::Disconnect(session));
            return;
        }
        // Miracle respawn: full HP/mana at the recall room.
        player.current_hp = derived.max_hp;
        player.current_mana = derived.max_mana;
        player.location = self.config.recall_location;
        *aided = false;
        let lives = player.lives;
        let snapshot: Box<Player> = player.clone();
        self.output_line(session, "But, due to a miracle, you have been saved.");
        self.output_line(session, &format!("You have {lives} lives left."));
        self.broadcast_to_room(
            self.config.recall_location,
            Some(session),
            &format!("{name} appeared on the floor in the middle of the room."),
        );
        self.events.push(Event::Persist(snapshot));
    }

    /// Tear down combat without the *Combat Off* print (death has its own
    /// messaging).
    fn break_combat_silent(&mut self, session: SessionId) {
        if let Some(Session::InGame { target, energy, casting, .. }) =
            self.sessions.get_mut(&session)
        {
            *target = None;
            *casting = None;
            *energy = (*energy).min(PLAYER_ENERGY_MAX);
        }
    }

    /// Monsters lose their lock on a dead/removed player.
    fn release_monster_targets(&mut self, session: SessionId) {
        for m in self.monsters.values_mut() {
            if m.target == Some(session) {
                m.target = None;
            }
        }
    }

    /// `kill_autocombat` + `display_autocombat_broken`.
    fn break_combat(&mut self, session: SessionId) {
        if let Some(Session::InGame { target, energy, casting, .. }) =
            self.sessions.get_mut(&session)
            && target.is_some()
        {
            *target = None;
            *casting = None;
            *energy = (*energy).min(PLAYER_ENERGY_MAX);
            self.output_line(session, text::COMBAT_OFF);
        }
    }

    /// EXACT (decompile 0x2a19d `move_player_to_fighter`): the normal-attack
    /// player fighter. Weapon skill/dyn accumulators are 0 until items and
    /// spells land (M4/M5); encumbrance is 0 until weight exists (M4).
    fn build_player_attacker(&self, session: SessionId) -> crate::combat::Fighter {
        let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session) else {
            unreachable!("caller holds an in-game session");
        };
        let level = i32::from(player.level);
        let str_ = i32::from(player.stats.strength);
        let agl = i32::from(player.stats.agility);
        let combat = self
            .content
            .classes
            .get(&player.class)
            .map_or(0, |c| i32::from(c.combat_factor));

        // skill = weapon/worn to-hit ratings (0 naked, floored 1) plus the
        // low-encumbrance bonus (enc < 33: += 15 - enc/10).
        let encumbrance = self.encumbrance_percent(session);
        let weapon = player
            .weapon
            .and_then(|(id, _)| self.content.items.get(&id));
        let ratings: i32 = i32::from(weapon.map_or(0, |w| w.accuracy))
            + player
                .worn
                .iter()
                .filter_map(|(id, _)| self.content.items.get(id))
                .map(|i| i32::from(i.accuracy))
                .sum::<i32>();
        let mut skill = if ratings == 0 { 1 } else { ratings };
        if encumbrance < 33 && player.current_hp > 0 {
            skill += 15 - encumbrance / 10;
        }
        // accuracy = (Str-50)/3
        //          + 2*((combat-1)*isqrt(level) + 2*combat + level/2 + skill/2 - 2)
        //          + (Agl-50)/6  (+ dynamic accuracy accumulators, M5)
        let bag = self.ability_bag(player);
        let dyn_accuracy = bag.value(accuracy_ability(0x16))
            + bag.value(accuracy_ability(0x69))
            + bag.value(accuracy_ability(0x6a));
        let accuracy = (str_ - 50) / 3
            + 2 * ((combat - 1) * isqrt(level) + 2 * combat + level / 2 + skill / 2 - 2)
            + (agl - 50) / 6
            + dyn_accuracy;

        // Weapon damage (or the unarmed 1-4 defaults), plus the Strength
        // bonuses: max += (Str-50)/10; min += 2*(Str-100)/10 when positive.
        let (base_min, base_max) = weapon.map_or((1, 4), |w| {
            (i32::from(w.min_damage), i32::from(w.max_damage))
        });
        let mut min_damage = base_min;
        let mut max_damage = base_max + (str_ - 50) / 10;
        let min_bonus = (str_ - 100) / 10 * 2;
        if min_bonus > 0 {
            min_damage += min_bonus;
        }
        if max_damage < min_damage {
            min_damage = max_damage;
        }
        min_damage = min_damage.max(0);
        max_damage = max_damage.max(0);

        crate::combat::Fighter {
            accuracy,
            evasion_a: 0,
            evasion_b: 0,
            armor: 0,
            min_damage,
            max_damage,
            // crit rating = dodge base byte + crits accumulator, min 1.
            crit_rating: derived.dodge_base.max(1),
            parry: 0,
        }
    }

    /// EXACT (decompile): defender view of a player. Naked: evasion 0
    /// (item ratings/10; the dynamic AC accumulator +0x70c joins when
    /// content carries AC(2) buffs), armor 0; parry is the word[10]
    /// formula — `dodgeAbil(0x22) + (Chm-50)/5 + level/5 + (Agl-50)/3`
    /// (combat.md "Parry") — plus the low-encumbrance bonus
    /// (10 - enc/10), forced -1 when helpless. The Dodge term reads the
    /// accumulated bag, so items AND active spells (blur) feed it.
    fn build_player_defender(&self, session: SessionId) -> crate::combat::Fighter {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            unreachable!("caller checked the session");
        };
        let parry = if player.current_hp < 1 {
            -1
        } else {
            let encumbrance = self.encumbrance_percent(session);
            let mut p = self.ability_bag(player).value(Ability::Dodge)
                + (i32::from(player.stats.charm) - 50) / 5
                + i32::from(player.level) / 5
                + (i32::from(player.stats.agility) - 50) / 3;
            if encumbrance < 33 {
                p += 10 - encumbrance / 10;
            }
            p
        };
        let armor: i32 = player
            .worn
            .iter()
            .filter_map(|(id, _)| self.content.items.get(id))
            .map(|i| i32::from(i.ac))
            .sum();
        let defense: i32 = (i32::from(
            player
                .weapon
                .and_then(|(id, _)| self.content.items.get(&id))
                .map_or(0, |w| w.defense),
        ) + player
            .worn
            .iter()
            .filter_map(|(id, _)| self.content.items.get(id))
            .map(|i| i32::from(i.defense))
            .sum::<i32>())
            / 10;
        crate::combat::Fighter {
            accuracy: 0,
            evasion_a: defense,
            evasion_b: 0,
            armor,
            min_damage: 0,
            max_damage: 0,
            parry,
            crit_rating: 0,
        }
    }

    /// EXACT (decompile 0x2b43e `move_monster_to_fighter`): defender view of
    /// a monster — evasion [1] = AC, armor [3] = DR*10, crit hard-zeroed.
    fn build_monster_defender(&self, id: MonsterInstanceId) -> crate::combat::Fighter {
        let tpl = self
            .monsters
            .get(&id)
            .and_then(|m| self.content.monsters.get(&m.template))
            .expect("live instance has a template");
        crate::combat::Fighter {
            accuracy: 0,
            evasion_a: i32::from(tpl.armour_class),
            evasion_b: 0,
            armor: i32::from(tpl.damage_resist) * 10,
            min_damage: 0,
            max_damage: 0,
            parry: 0,
            crit_rating: 0,
        }
    }

    /// EXACT (decompile 0x2a0c8 `compute_energy_used`):
    /// EU = speed*1000 / ((combat*level + 45) * (Agl+150) * 1500/9000) + bonus,
    /// divide-by-zero guard = 50. Unarmed fists speed = 1200 (0x4b0; the
    /// 1800 variant fires when player flag +0x7c8 & 2 is set — semantics
    /// not yet traced). Weapon speeds join in M4.
    fn player_energy_used(&self, session: SessionId) -> i32 {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return PLAYER_ENERGY_MAX;
        };
        let combat = self
            .content
            .classes
            .get(&player.class)
            .map_or(0, |c| i32::from(c.combat_factor));
        let i = combat * i32::from(player.level) + 45;
        let den = i * (i32::from(player.stats.agility) + 150) * 1500 / 9000;
        if i == 0 || den == 0 {
            return 50;
        }
        1200 * 1000 / den
    }

    fn say(&mut self, session: SessionId, what: &str) {
        let player = self.player(session);
        let (room, name) = (player.location, player.name.clone());
        self.output_line(session, &text::you_say(what));
        self.broadcast_to_room(room, Some(session), &text::says(&name, what));
    }

    /// Emits a single message line (most outputs; the prompt is the
    /// exception — it stays on its own unterminated line).
    fn output_line(&mut self, session: SessionId, text: &str) {
        self.events.push(Event::Output {
            session,
            text: format!("{text}\n"),
        });
    }

    fn next_session_id(&mut self) -> SessionId {
        let id = SessionId(self.next_session);
        self.next_session += 1;
        id
    }

    fn show_race_list(&mut self, session: SessionId) {
        let mut out = String::from(text::CHOOSE_RACE);
        out.push('\n');
        for race in self.content.races.values() {
            out.push_str(&text::list_entry(race.id.0, &race.name));
        }
        out.push('\n');
        out.push_str(text::RACE_PROMPT);
        self.output(session, &out);
    }

    fn show_class_list(&mut self, session: SessionId) {
        let mut out = String::from(text::CHOOSE_CLASS);
        out.push('\n');
        for class in self.content.classes.values() {
            out.push_str(&text::list_entry(class.id.0, &class.name));
        }
        out.push('\n');
        out.push_str(text::CLASS_PROMPT);
        self.output(session, &out);
    }

    /// State 0x33: the input is `atol`'d and validated against the race data.
    fn choose_race(&mut self, session: SessionId, line: &str) {
        if line.trim().is_empty() {
            self.output_line(session, text::EMPTY_RACE);
            return;
        }
        let choice = line.trim().parse::<u16>().ok().map(RaceId);
        let valid = choice.is_some_and(|id| self.content.races.contains_key(&id));
        if !valid {
            self.output_line(session, text::INVALID_RACE);
            return;
        }
        let Some(Session::ChoosingRace { profile }) = self.sessions.remove(&session) else {
            unreachable!("dispatched from ChoosingRace");
        };
        self.sessions.insert(
            session,
            Session::ChoosingClass {
                profile,
                race: choice.expect("validated above"),
            },
        );
        self.show_class_list(session);
    }

    /// State 0x34, then `roll_stats` + realm entry.
    fn choose_class(&mut self, session: SessionId, line: &str) {
        if line.trim().is_empty() {
            self.output_line(session, text::EMPTY_CLASS);
            return;
        }
        let choice = line.trim().parse::<u16>().ok().map(ClassId);
        let valid = choice.is_some_and(|id| self.content.classes.contains_key(&id));
        if !valid {
            self.output_line(session, text::INVALID_CLASS);
            return;
        }
        let Some(Session::ChoosingClass { profile, race }) = self.sessions.remove(&session)
        else {
            unreachable!("dispatched from ChoosingClass");
        };
        self.sessions.insert(
            session,
            Session::ChoosingLawful {
                profile,
                race,
                class: choice.expect("validated above"),
            },
        );
        self.output(session, &format!("\n{}\n{}", text::LAWFUL_PARAGRAPH, text::LAWFUL_QUESTION));
    }

    /// The Lawful (PvP opt-out) question, then `roll_stats` + realm entry.
    fn choose_lawful(&mut self, session: SessionId, line: &str) {
        let answer = line.trim().to_ascii_lowercase();
        let lawful = if answer.starts_with('y') {
            true
        } else if answer.starts_with('n') {
            false
        } else {
            self.output_line(session, text::LAWFUL_QUESTION);
            return;
        };
        let Some(Session::ChoosingLawful { profile, race, class }) =
            self.sessions.remove(&session)
        else {
            unreachable!("dispatched from ChoosingLawful");
        };
        let player = self.roll_stats(profile, race, class, lawful);
        self.events.push(Event::Persist(Box::new(player.clone())));
        self.broadcast_to_others(session, &text::entered_realm(&player.name));
        let derived = self.derive_for(&player);
        self.sessions
            .insert(session, Session::InGame { player: Box::new(player), derived, exiting: None, target: None, aided: false, energy: PLAYER_ENERGY_MAX, cast_this_round: false, casting: None });
        // Oracle: first entry shows the stat sheet, not the room.
        self.show_sheet(session);
        self.show_prompt(session);
    }

    /// `roll_stats` (spec §2.2): no randomisation — the racial template, CP
    /// grant, and creation defaults are copied verbatim; mode-0
    /// `calculate_secondary_stats` fills HP/mana to max.
    fn roll_stats(
        &self,
        profile: AccountProfile,
        race: RaceId,
        class: ClassId,
        lawful: bool,
    ) -> Player {
        let template = &self.content.races[&race];
        let hp_base = self
            .content
            .classes
            .get(&class)
            .map_or(0, |c| c.hp_seed.max(0) as u16);
        let mut player = Player {
            name: profile.name,
            gender: profile.gender,
            race,
            class,
            level: 1,
            stats: template.base_stats,
            base_stats: template.base_stats,
            hp_base,
            current_hp: 0,
            current_mana: 0,
            hunger: 1000,
            thirst: 1000,
            coins: Coins::default(),
            lawful,
            inventory: Vec::new(),
            weapon: None,
            bankbooks: Vec::new(),
            worn: Vec::new(),
            cp_unspent: template.cp,
            cp_lifetime: template.cp,
            lives: 9,
            experience: 0,
            location: self.config.start_location,
            spellbook: BTreeMap::new(),
            poison: 0,
            active_spells: Default::default(),
        };
        let derived = self.derive_for(&player);
        player.current_hp = derived.max_hp;
        player.current_mana = derived.max_mana;
        player
    }

    /// Takes all events produced since the last drain.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Removes a session whose connection dropped: same as quitting (persist
    /// + departure broadcast). Sessions still in creation just vanish.
    pub fn detach(&mut self, session: SessionId) {
        match self.sessions.get(&session) {
            Some(Session::InGame { .. }) => self.complete_quit(session),
            Some(_) => {
                self.sessions.remove(&session);
                self.events.push(Event::Disconnect(session));
            }
            None => {}
        }
    }

    /// Starts the delayed exit (oracle: message, then one dot per second;
    /// input is swallowed; combat cancels via `cancel_exit`).
    fn quit(&mut self, session: SessionId) {
        let dots = self.config.exit_meditation_seconds;
        let Some(Session::InGame { exiting, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        if exiting.is_some() {
            return;
        }
        *exiting = Some(dots);
        self.output_line(session, text::EXIT_MEDITATION);
        self.scheduler.schedule_in(1, Job::ExitStep(session));
    }

    /// Cancels a pending exit (the combat-engagement hook: you cannot leave
    /// the Realm while being attacked).
    pub fn cancel_exit(&mut self, session: SessionId) {
        if let Some(Session::InGame { exiting, .. }) = self.sessions.get_mut(&session) {
            *exiting = None;
        }
    }

    fn exit_step(&mut self, session: SessionId) {
        let Some(Session::InGame { exiting, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        let Some(remaining) = *exiting else {
            return; // cancelled
        };
        self.output(session, ".");
        if remaining > 1 {
            if let Some(Session::InGame { exiting, .. }) = self.sessions.get_mut(&session) {
                *exiting = Some(remaining - 1);
            }
            self.scheduler.schedule_in(1, Job::ExitStep(session));
        } else {
            self.complete_quit(session);
        }
    }

    fn complete_quit(&mut self, session: SessionId) {
        let Some(Session::InGame { player, .. }) = self.sessions.remove(&session) else {
            return;
        };
        self.broadcast_to_others(session, &text::left_realm(&player.name));
        self.events.push(Event::Persist(player));
        self.events.push(Event::Disconnect(session));
    }

    fn player(&self, session: SessionId) -> &Player {
        match &self.sessions[&session] {
            Session::InGame { player, .. } => player,
            _ => unreachable!("caller guarantees an in-game session"),
        }
    }

    /// The player's class caster group (`class+0x40`; 5 = kai/mystic).
    fn caster_group(&self, session: SessionId) -> i16 {
        self.content
            .classes
            .get(&self.player(session).class)
            .map_or(0, |c| c.caster_group)
    }

    /// caster_group == 5 — the kai wording/gating key (§8.12).
    fn is_kai(&self, session: SessionId) -> bool {
        self.caster_group(session) == 5
    }

    /// The one-cast-per-round refusal, kai wording for mystics (§8.12).
    fn already_cast_line(&self, session: SessionId) -> &'static str {
        if self.is_kai(session) {
            text::ALREADY_INVOKED
        } else {
            text::ALREADY_CAST
        }
    }

    /// The mana-gate refusal, kai wording for mystics (§8.12).
    fn not_enough_mana_line(&self, session: SessionId) -> &'static str {
        if self.is_kai(session) {
            text::NOT_ENOUGH_KAI
        } else {
            text::NOT_ENOUGH_MANA
        }
    }

    /// Text-triggered exits (type 10): match the input against each exit's
    /// pipe-separated phrase pool ("row skiff") and move on a hit.
    fn try_action_exit(&mut self, session: SessionId, what: &str) -> Resolution {
        if self.player(session).current_hp < 1 {
            return Resolution::FallThrough;
        }
        let room = self.player(session).location;
        let want = what.trim().to_ascii_lowercase();
        for direction in Direction::ALL {
            let Some(exit) = &self.content.rooms[&room].exits[direction as usize] else {
                continue;
            };
            if exit.exit_type != 10 {
                continue;
            }
            let phrases = exit
                .trigger_msg
                .and_then(|m| self.content.messages.get(&m))
                .and_then(|m| m.lines.first())
                .map(|l| l.to_ascii_lowercase());
            let Some(phrases) = phrases else { continue };
            if phrases.split('|').any(|p| p.trim() == want) {
                // ORACLE: "You climb into one of the skiffs, and row to
                // Silvermere." — the flavor line is board data we don't
                // have per-exit; the movement itself is the mechanic.
                self.action_exit_pass = true;
                self.move_player(session, direction);
                self.action_exit_pass = false;
                return Resolution::Handled;
            }
        }
        Resolution::FallThrough
    }

    fn move_player(&mut self, session: SessionId, direction: Direction) {
        let from = self.player(session).location;
        let Some(exit) = self.content.rooms[&from].exits[direction as usize].clone() else {
            self.output_line(session, text::NO_EXIT);
            return;
        };
        // Type-10 exits only move via their trigger phrases — but when the
        // trigger routes here (try_action_exit), it passes. Plain walking
        // sees "no exit" (oracle: 's' at the docks).
        if exit.exit_type == 10 && !self.action_exit_pass {
            self.output_line(session, text::NO_EXIT);
            return;
        }
        let name = self.player(session).name.clone();
        self.broadcast_to_room(from, Some(session), &text::left_via(&name, direction));
        match self.sessions.get_mut(&session) {
            Some(Session::InGame { player, .. }) => player.location = exit.dest,
            _ => unreachable!("mover is in game"),
        }
        self.broadcast_to_room(
            exit.dest,
            Some(session),
            &text::arrived_from(&name, direction.opposite()),
        );
        self.show_room(session);
    }

    fn show_room(&mut self, session: SessionId) {
        self.render_room(session, true);
    }

    /// The `exits` command: just the obvious-exits line (oracle).
    fn show_exits_line(&mut self, session: SessionId) {
        let player = self.player(session);
        let room = &self.content.rooms[&player.location];
        let exits: Vec<&str> = Direction::ALL
            .into_iter()
            .filter(|d| {
                room.exits[*d as usize]
                    .as_ref()
                    .is_some_and(|e| e.exit_type != 10)
            })
            .map(|d| text::direction_shown(d))
            .collect();
        let line = if exits.is_empty() {
            format!("{}{}", text::OBVIOUS_EXITS, text::NO_EXITS)
        } else {
            format!("{}{}", text::OBVIOUS_EXITS, exits.join(", "))
        };
        self.output_line(session, &line);
    }

    fn show_room_brief(&mut self, session: SessionId) {
        self.render_room(session, false);
    }

    /// Renders the session's current room: name, description (full display
    /// only, first line indented four spaces per the oracle transcript),
    /// occupants, obvious exits.
    fn render_room(&mut self, session: SessionId, full: bool) {
        let player = self.player(session);
        let room = &self.content.rooms[&player.location];

        let mut out = String::new();
        out.push_str(&room.name);
        out.push('\n');
        if full {
            for (i, line) in room.description.iter().enumerate() {
                if i == 0 {
                    out.push_str("    ");
                }
                out.push_str(line);
                out.push('\n');
            }
        }

        // Floor items and coin piles share the notice line (oracle shows
        // each alone; combined ordering items-then-coins ORACLE-VERIFY).
        let mut notices: Vec<String> = self
            .room_items
            .get(&room.id)
            .into_iter()
            .flatten()
            .filter_map(|(id, _)| self.content.items.get(id).map(|i| i.name.clone()))
            .collect();
        if let Some(piles) = self.room_coins.get(&room.id)
            && let Some(names) = text::coin_pile_names(*piles)
        {
            notices.push(names);
        }
        if !notices.is_empty() {
            out.push_str(&format!("You notice {} here.\n", notices.join(", ")));
        }

        // Players first, then live monsters (oracle: NPCs share the line).
        let mut others: Vec<&str> = self
            .in_game_sessions()
            .filter(|(id, p)| *id != session && p.location == room.id)
            .map(|(_, p)| p.name.as_str())
            .collect();
        others.extend(
            self.monsters
                .values()
                .filter(|m| m.location == room.id)
                .filter_map(|m| self.content.monsters.get(&m.template))
                .map(|t| t.name.as_str()),
        );
        if !others.is_empty() {
            out.push_str(text::ALSO_HERE);
            out.push_str(&others.join(", "));
            out.push_str(".\n");
        }

        let exits: Vec<&str> = Direction::ALL
            .into_iter()
            .filter(|d| {
                room.exits[*d as usize]
                    .as_ref()
                    .is_some_and(|e| e.exit_type != 10)
            })
            .map(|d| text::direction_shown(d))
            .collect();
        out.push_str(text::OBVIOUS_EXITS);
        if exits.is_empty() {
            out.push_str(text::NO_EXITS);
        } else {
            out.push_str(&exits.join(", "));
        }
        out.push('\n');

        self.output(session, &out);
    }

    fn broadcast_to_room(&mut self, room: RoomId, exclude: Option<SessionId>, text: &str) {
        let recipients: Vec<SessionId> = self
            .in_game_sessions()
            .filter(|(id, p)| Some(*id) != exclude && p.location == room)
            .map(|(id, _)| id)
            .collect();
        for session in recipients {
            self.output_line(session, text);
        }
    }

    fn in_game_sessions(&self) -> impl Iterator<Item = (SessionId, &Player)> {
        self.sessions.iter().filter_map(|(id, s)| match s {
            Session::InGame { player, .. } => Some((*id, player.as_ref())),
            _ => None,
        })
    }

    fn output(&mut self, session: SessionId, text: &str) {
        self.events.push(Event::Output {
            session,
            text: text.to_string(),
        });
    }

    /// Realm-wide broadcast — only players in the game hear it, not sessions
    /// still in character creation.
    fn broadcast_to_others(&mut self, exclude: SessionId, text: &str) {
        let recipients: Vec<SessionId> = self
            .in_game_sessions()
            .filter(|(id, _)| *id != exclude)
            .map(|(id, _)| id)
            .collect();
        for session in recipients {
            self.output_line(session, text);
        }
    }
}
