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

/// The `monster_cast_area` effect-loop no-op set — ability rows whose
/// case is a plain `break` (or an unmirrored call) in the area sibling
/// (decompile 22236-22906): the explicit no-op cases 0/6/15/16/23/26/
/// 44-49/52 (22246-22258 — note Alterhunger/AlterThirst and the whole
/// stat family DO NOTHING here, unlike the single path), 52 (the
/// `!= 0x34` wrap), 73/86 and the subtract-ladder exclusions 80/81/84/
/// 97/98/101 (22807-22824), the metadata band 108-115 + 120 + 122
/// (22826-22832 — 122 is consumed by the pre-pass instead), 148/153
/// (22834-22843), and 143 = ClearItem whose FUN_0046c241 call is
/// unmirrored (no shipped area spell carries it). Every OTHER id falls
/// to the duration-only default arm.
fn area_ability_case_is_noop(ability: Ability) -> bool {
    matches!(
        ability.id(),
        0 | 6 | 15 | 16 | 23 | 26 | 44..=49 | 52 | 73 | 80 | 81 | 84 | 86 | 97 | 98 | 101
            | 108..=115 | 120 | 122 | 143 | 148 | 153
    )
}

/// One save-failed area victim (`monster_count_valid_targets`' 0x10
/// flag): a snapshot of every target-side term the `monster_cast_area`
/// arms read piecemeal — MR + AntiMagic for the DamageMR ladder,
/// ImmuPoison for the Poison/CurePoison gates, the per-victim elemental
/// resist (offensive casts only), and the caps.
struct AreaTarget {
    session: SessionId,
    mr: i32,
    anti_magic: bool,
    immune_poison: bool,
    resist_pct: i32,
    max_hp: i32,
    max_mana: i32,
}

/// The per-victim elemental scale the area arms repeat (`local_24 =
/// (100 - resist) * local_1c / 100` under `spelltype < 3`; benign-mode
/// casts pass the divided roll through untouched).
fn area_scaled(base: i32, resist_pct: i32, offensive: bool) -> i32 {
    if offensive { (100 - resist_pct) * base / 100 } else { base }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Gender {
    #[default]
    Male,
    Female,
}

/// The persisted subset of the 0x7ec-byte player record
/// (`re/docs/character_creation.md` §6). Grows with each milestone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    /// `+0x542` — fame/notoriety word. Gates monster targeting: behaviour
    /// mode 6 spares players at >= 0x28 (unless already fighting); roam
    /// class 5 "guardians" initiate ONLY at >= 0x28; the flee free-attack
    /// mode-6 bound is 0x50 (decompile 20386/20420/23882). Fed by the M7
    /// crime system — creation seeds 0.
    pub fame: i16,
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
    /// Restored population cooldowns: (template, seconds since its last
    /// kill at boot). Applied before the boot population walk.
    pub restored_population: Vec<(crate::content::MonsterId, i64)>,
    /// Restored room respawn stamps: (room, seconds since the kill at
    /// boot). Kills the restart-to-respawn exploit — the original
    /// persists both through the record dirty flags.
    pub restored_room_stamps: Vec<(RoomId, i64)>,
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
            restored_population: Vec::new(),
            restored_room_stamps: Vec::new(),
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

/// The monster cast-chance roll (decompile `monster_cast` 22983 entry roll,
/// compared at 23033-23040): a flat per-form percentage (`attackminhcastper`,
/// `template+0x13e+idx*2`; hard 100 for a forced `idx == -1` cast, 23009-
/// 23011) beats `genrdn(0,100)` on STRICT less-than — `roll < chance`. No
/// caster or target stat term (spec §6.3). A 0% form never fires; a 100%
/// form still loses to the inclusive top roll (genrdn spans 0..=100).
pub fn monster_cast_chance_passes(
    chance: i16,
    roll: &mut impl FnMut(i32, i32) -> i32,
) -> bool {
    roll(0, 100) < i32::from(chance)
}

/// The player saving throw against a monster cast (decompile `monster_cast`
/// 23044-23056; spec §6.4): rolled only when the chance roll passed and the
/// spell's save class grants one (`Always`, or `IfAntiMagic` on a player
/// carrying AntiMagic 51): `genrdn(1, 100) <= min(save_stat / 2, 97)`
/// resists. `save_stat` is the DLL's player `+0xc2` — the MAX magic-resist
/// word (`leveling.md`: `(Int + Wis*3)/4 + M.R.(36) modifiers`), i.e. our
/// `Derived::magic_resist` — the same stat DamageMR(17) scales by, NOT the
/// spellcasting skill. NOTE the cap: 97 here (23048-23052: `mr/2 < 0x62 ?
/// mr/2 : 0x61`) versus the player-cast path's 98 (43606-43612,
/// [`monster_save_resists`]) — a genuine one-off DLL asymmetry, mirrored
/// faithfully.
pub fn player_save_resists(save_stat: i32, roll: &mut impl FnMut(i32, i32) -> i32) -> bool {
    roll(1, 100) <= (save_stat / 2).min(97)
}

/// The per-target save of an AREA monster cast — rolled inside
/// `monster_count_valid_targets` (decompile 21850-21868), BEFORE the
/// energy charge and the fizzle roll: a saving player is silently
/// excluded from the valid-target set (never flagged 0x10) — there is no
/// resist line anywhere in the area path, and a room where EVERYONE
/// saves aborts the cast entirely (0 targets → return 0, nothing
/// charged, 22102-22105). The gate is the spell's save class (`+0xc6`):
/// class 2 always rolls, class 1 only when THIS target carries AntiMagic
/// (51), class 0 never — then the shared [`player_save_resists`] formula
/// (97 cap). NOTE: unlike the single-target path (23026-23029), the area
/// path never reads SpellImmu (139) — an auto-resisting target is swept
/// like anyone else. (The count function's other exclusion, the
/// FUN_0043e3db worn-item predicate, is unmirrored here — the same spec
/// §7 hedge as the single path.)
pub fn monster_area_target_saves(
    save_class: crate::content::SaveClass,
    anti_magic: bool,
    save_stat: i32,
    roll: &mut impl FnMut(i32, i32) -> i32,
) -> bool {
    use crate::content::SaveClass;
    let allowed = match save_class {
        SaveClass::None => false,
        SaveClass::Always => true,
        SaveClass::IfAntiMagic => anti_magic,
    };
    allowed && player_save_resists(save_stat, roll)
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
/// matches). The caster's AlterSpDmg(165) boost applies to `amount`
/// BEFORE this scale ([`alter_sp_dmg`], wired at every call site) — the
/// slice-4 divergence note is closed.
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

/// The caster's AlterSpDmg(165) percent boost on spell damage: `V += V *
/// pct / 100`. The DLL applies it at every PLAYER Damage(1)/Damage-MR(17)
/// computation — DamageMR inline (`cast_monster_target` 43940-43941,
/// `cast_user_target` 42139-42141, `cast_no_target` 40151-40152 and the
/// area/default-target legs 40304-40305), plain Damage through
/// `FUN_0043fef4` (39025-39030: `(pct+100)*V/100`, same value as this
/// form on every non-negative product; the sole shipped carrier is item
/// 504 "multicoloured sash", +10). Drain(8) is never boosted. The
/// monster-cast twin (`monster_cast`) reads NO 0xa5 at all — the monster
/// fold is wired at our monster damage sites anyway, observably
/// identical because zero shipped monsters carry 165.
pub fn alter_sp_dmg(amount: i32, pct: i32) -> i32 {
    amount + amount * pct / 100
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
    /// A limited-population template was killed — persist the wall-clock
    /// stamp (check_kill_monster's tmpl+0xb4/+0xb6 write).
    PersistMonsterKill { template: crate::content::MonsterId },
    /// A room's respawn stamp changed — persist it (the room dirty flag).
    PersistRoomStamp { room: RoomId },
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
        /// `+0x6f4` bit 6 — set on a successful move, cleared at the top
        /// of the energy round and in the medium tick (decompile 12501/
        /// 18619/19755). While set: acquisition skips the player and
        /// pursuit's follow gate fails.
        moved_this_round: bool,
        /// `+0x6f0` — monsters that attacked this player this MEDIUM tick
        /// (reset 19748). Tapers the anti-pile-on roll (50 - 5n) and hard
        /// gates the flee free-attack (> 0 = already attacked, no swing).
        attackers_this_tick: i32,
        /// `+0x550/+0x5a0` — the 20-deep movement breadcrumb (index 0 =
        /// current room); `dir_player_travelling_coord` walks it to chase.
        trail: Vec<RoomId>,
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
    /// `background_fast`, every 1 s: the pursuit tier (prone recovery +
    /// chase; idle monsters cost nothing — the has-target gate).
    Fast,
    /// `background_monster_create`, every 5 s (`DAT_00482ca8`): the
    /// density spawner FUN_004232d3.
    Spawn,
    /// The nightly-cleanup stand-in: Worldgroup restarted the module every
    /// night, re-running check_initiate_restocking (its run-once flag
    /// DAT_00482138 is never reset within a process). A standalone server
    /// re-runs the shelf reconciliation every 24 h instead.
    Cleanup,
}

const SLOW_INTERVAL: u64 = 30;
/// The pursuit tier (`background_fast`).
const FAST_INTERVAL: u64 = 1;
/// The spawner kick (`DAT_00482ca8` = 5, dumped).
const SPAWN_INTERVAL: u64 = 5;
/// Default respawn delay in minutes (`DAT_00482d0c` = 5, dumped) when the
/// room's `delay` word is zero.
const RESPAWN_DEFAULT_MINUTES: i64 = 5;
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
/// ORACLE: -200 in the stock config — died at -204, survived -196;
/// §8.14 re-confirmed the FLAT bound on a 56-maxhp character with four
/// kills at -200/-202/-209/-225 — §8.10's ~7x-maxhp fit was coincidence).
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
    /// The 5 monster active-spell slots (`mon+0x14a` id / `+0x154` value /
    /// `+0x15e` remaining; spec §6.5-6.6). NOT persisted: instances are
    /// ephemeral until M6, exactly like the original's in-memory monster
    /// records — a restart clears them with the monster itself.
    pub active_spells: [ActiveSpell; 5],
    /// The poison counter (`mon+0x14`): set-if-greater by Poison(19)
    /// casts, dealt as HP damage each slow pass (`slow_update_monster`
    /// 19276-19279), reduced by the CurePoison(20) upkeep handler and the
    /// Poison termination.
    pub poison: i16,
    /// The dirty byte (`mon+0x140`): set by every slot/effect write; keys
    /// [`Core::recompute_monster_effects`]. The DLL rebuilds lazily at the
    /// next record touch; we recompute eagerly at each write site —
    /// observably identical.
    pub needs_recompute: bool,
    /// Cached fold of the 5 slots' ability rows (the monster half of
    /// `get_monster_ability_value`'s slot walk, 37187-37215): value-0 rows
    /// substitute the stored slot value. Rebuilt when `needs_recompute`.
    pub slot_bag: AbilityBag,
    /// `mon+0x108` ← template `follow`: aggression 0-100. Wander chance is
    /// `(100 - aggression)/2`; pursuit follow-rolls use it in M6 slice 3.
    pub aggression: i16,
    /// `mon+0x106` ← template `alignment`: the §4 behaviour-mode taxonomy
    /// (consumed by the slice-3 aggression driver).
    #[allow(dead_code)] // read from M6 slice 3 (FUN_00423863)
    pub behaviour: i16,
    /// `mon+0x12c` ← template `group`: roam/zone class — the wander leash
    /// key (0/2 stationary, 5 water, 0x25/0x26 free roamers, else zone id).
    pub roam_class: i16,
    /// `mon+0x148` ← template `type`: herd mode (0 none, 1/2 pack, 3 lair).
    pub herd_mode: i16,
    /// `mon+0x08` ← template `expmulti` (dual-use): pack herd rank.
    pub herd_rank: i32,
    /// `mon+0x132`: last wander direction — the anti-backtrack memory,
    /// cleared by the 30 s slow tick (decompile 19274).
    pub last_move_dir: Option<crate::content::Direction>,
    /// `mon+0x124` — pursuit give-up counter: bumped once per fast tick
    /// the lock can't be prosecuted (target gone/hidden/other map/roll or
    /// move failed); > 15 drops the lock (class 0x25 despawns instead).
    /// Zeroed on every attack engage (decompile 26775).
    pub give_up: u8,
    /// `mon+0x116` — attack-suppress byte: a suppressed lock (guardian
    /// summons) never swings at its named target. Cleared whenever a lock
    /// is (re)written by combat.
    pub suppress: bool,
    /// `mon+0x120` — the spawn/home room: check_kill_monster stamps ITS
    /// respawn timer and spawn accounting, wherever the monster died.
    pub home: RoomId,
    /// `mon+0xf0..+0x100` — the five coin piles (low->high denominations),
    /// rolled `lngrnd(0, max+1)` at generate time (fixture spawns copy the
    /// template maxes verbatim to keep M3-era goldens byte-stable).
    pub coins: [u32; 5],
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
    /// `DAT_0047fb90`: wander attempts this medium tick (cap 3), reset
    /// once per pass in `upkeep_update` (monsters.md §3).
    wander_budget: u8,
    /// The spawner/generate RNG stream. The DLL shares one genrdn stream;
    /// a background job drawing from the main stream would reshuffle every
    /// seeded golden each 5 s kick, so the spawner runs its own
    /// deterministic stream (documented representation divergence).
    spawn_rng: Rng,
    /// Per-room runtime spawn state (`+0x606` live count, `+0x562` stamp,
    /// `+0x5c0` linked live, `+0x564` bit 8) — ephemeral, like room_coins.
    room_spawn: BTreeMap<RoomId, RoomSpawnState>,
    /// The mongen candidate table (`DAT_004790f0`): every template in id
    /// order as (id, region, level). The DLL reloads it every 60 s from
    /// the DB; our content is immutable, so it is built once.
    mongen: Vec<(crate::content::MonsterId, i16, i16)>,
    /// Per-template population state (`knmsr+0xa8` active count,
    /// `+0xb4/+0xb6` kill stamps). Only gamelimit templates get entries.
    population: BTreeMap<crate::content::MonsterId, PopulationState>,
    /// Spawner phase-B user cursor (`DAT_0047fba4`).
    spawn_user_cursor: usize,
    /// Spawner phase-A cache cursor (`DAT_0047fba8`).
    spawn_cache_cursor: usize,
}

/// Per-template population state (monsters.md §2 step 3).
#[derive(Debug, Clone, Copy, Default)]
struct PopulationState {
    /// `knmsr+0xa8` — live spawns charged against the gamelimit.
    active: u16,
    /// The tick of the last kill (negative = before boot). `Some` also
    /// drives the first-kill loot guarantee: `None` = never killed.
    last_kill: Option<i64>,
}

/// Runtime spawn bookkeeping for one room (monsters.md §1/§2).
#[derive(Debug, Clone, Copy, Default)]
struct RoomSpawnState {
    /// `room+0x606` — current spawn count (bosses excluded).
    live: u8,
    /// `room+0x562` — the kill stamp, stored as the absolute tick of the
    /// kill (the DLL stores minutes-since-midnight with a +1440 wrap; the
    /// refusal window `stamp <= now <= stamp + delay` is identical).
    /// Negative = restored from a previous run (that many seconds before
    /// boot).
    stamp: Option<i64>,
    /// `room+0x5c0` — live count charged against this room's linked cap.
    linked_live: i16,
    /// `room+0x564` bit 8 — the boss/permnpc is present.
    boss_present: bool,
}

impl Core {
    pub fn new(content: Content, config: CoreConfig) -> Core {
        let mut scheduler = TickScheduler::new();
        scheduler.schedule_in(SLOW_INTERVAL, Job::Slow);
        scheduler.schedule_in(ENERGY_INTERVAL, Job::Energy);
        scheduler.schedule_in(UPKEEP_INTERVAL, Job::Upkeep);
        scheduler.schedule_in(FAST_INTERVAL, Job::Fast);
        scheduler.schedule_in(SPAWN_INTERVAL, Job::Spawn);
        scheduler.schedule_in(CLEANUP_INTERVAL, Job::Cleanup);
        let rng = Rng(config.rng_seed | 1);
        let spawn_rng = Rng((config.rng_seed ^ 0x5350_4157_4e21_0000) | 1); // "SPAWN!"-ish
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
            wander_budget: 0,
            spawn_rng,
            room_spawn: BTreeMap::new(),
            population: BTreeMap::new(),
            mongen: Vec::new(),
            spawn_user_cursor: 0,
            spawn_cache_cursor: 0,
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
        // The mongen candidate table (load_monster_quickreferences
        // 58555-58585): every template in record order, (id, region,
        // level), no filtering. The DLL reloads it every 60 s; our
        // content is immutable.
        core.mongen = core
            .content
            .monsters
            .values()
            .map(|m| (m.id, m.roam_class, m.level))
            .collect();
        // Restored persistence: population cooldowns and room stamps land
        // BEFORE the boot walk so a pre-restart kill still gates it.
        let restored_pop = core.config.restored_population.clone();
        for (template, elapsed) in restored_pop {
            core.population.insert(
                template,
                PopulationState { active: 0, last_kill: Some(-elapsed) },
            );
        }
        let restored_stamps = core.config.restored_room_stamps.clone();
        for (room, elapsed) in restored_stamps {
            core.spawn_state(room).stamp = Some(-elapsed);
        }
        // Boot population (preload_and_generate_buffers 27594-27767):
        // every boss/permnpc room spawns its boss (forced, levels
        // 0..0x7fff), and spawn-type 3 and 1 rooms swarm-fill until
        // their own gates refuse (type 1 is boot-fill-only — the 5 s
        // spawner never touches it).
        core.populate_world();
        core
    }

    /// The boot room walk's spawning half. Runs inside `Core::new`; the
    /// arrival broadcasts reach nobody (no sessions yet), as at module
    /// boot.
    fn populate_world(&mut self) {
        let rooms: Vec<RoomId> = self.content.rooms.keys().copied().collect();
        for id in rooms {
            let (boss, spawn_type, zone, forced, min_l, max_l) = {
                let r = &self.content.rooms[&id];
                (r.boss_monster, r.room_type, r.spawn_zone, r.forced_monster, r.min_level, r.max_level)
            };
            if let Some(boss) = boss
                && !self.spawn_state(id).boss_present
            {
                self.generate_monster(id, 0, Some(boss), 0, 0x7fff, true);
            }
            if spawn_type == 3 || spawn_type == 1 {
                for _ in 0..16 {
                    if self.generate_monster(id, zone, forced, min_l, max_l, false).is_none() {
                        break;
                    }
                }
            }
        }
    }

    fn spawn_state(&mut self, room: RoomId) -> &mut RoomSpawnState {
        self.room_spawn.entry(room).or_default()
    }

    /// `generate_monster` (0x24361, monsters.md §2): template -> live
    /// instance with every pre-flight gate. `bypass` mirrors the boot
    /// path's disable-flag dance (boot bosses ignore nothing else — the
    /// DLL passes param_9 '\0' everywhere but sysop tools; our `bypass`
    /// is ONLY the boss-forced boot call, which needs no gate skipped in
    /// practice since fresh state has no caps/timers pending). Returns
    /// the new instance id, `None` on any refusal.
    fn generate_monster(
        &mut self,
        room: RoomId,
        zone: i16,
        forced: Option<crate::content::MonsterId>,
        min_level: i16,
        max_level: i16,
        boot_boss: bool,
    ) -> Option<MonsterInstanceId> {
        // L20886: max level 0 with no forced monster never spawns — the
        // shipped zoned-band-0 rooms are spawnless by design.
        if max_level == 0 && forced.is_none() {
            return None;
        }
        let room_data = self.content.rooms.get(&room)?.clone();
        let is_boss = forced.is_some() && forced == room_data.boss_monster;
        let state = *self.spawn_state(room);
        // L20890-20895: the +0x55c cap (bosses bypass it).
        if !is_boss && i32::from(state.live) >= i32::from(room_data.spawn_cap) {
            return None;
        }
        // L20897-20906: the 15-slot room list.
        let occupants = self.monsters.values().filter(|m| m.location == room).count();
        if occupants >= 15 {
            return None;
        }
        // L20909-20925: the respawn window `stamp <= now <= stamp+delay`
        // (absolute ticks stand in for minutes-since-midnight + wrap).
        // Type-2 rooms and the room's boss skip the timer.
        if room_data.room_type != 2
            && !is_boss
            && let Some(stamp) = state.stamp
        {
            let delay_min = if room_data.respawn_delay != 0 {
                i64::from(room_data.respawn_delay)
            } else {
                RESPAWN_DEFAULT_MINUTES
            };
            let now = self.scheduler.now() as i64;
            if now >= stamp && now <= stamp + delay_min * 60 {
                return None;
            }
        }
        // L20927-20940: the linked-room cap (room+0x5c4 -> +0x5be/+0x5c0).
        if !is_boss
            && let Some(linked) = room_data.linked_room
        {
            let linked_cap = self
                .content
                .rooms
                .get(&linked)
                .map(|r| r.linked_cap)
                .unwrap_or(0);
            let linked_live = self.spawn_state(linked).linked_live;
            if linked_cap <= linked_live {
                return None;
            }
        }
        // Template selection (L20943-20972).
        let template = match forced {
            Some(f) => {
                // Forced boss already present (room+0x564 bit 8) refuses.
                if Some(f) == room_data.boss_monster && state.boss_present {
                    return None;
                }
                if !self.content.monsters.contains_key(&f) {
                    return None;
                }
                f
            }
            None => {
                // The mongen walk: adopt the first match; each later match
                // draws genrdn(1,100) — <= 29 replaces, >= 99 stops the
                // scan keeping the incumbent.
                let mut cur: Option<crate::content::MonsterId> = None;
                for (id, region, level) in self.mongen.clone() {
                    let matches = region == zone
                        && (min_level == 0 || min_level <= level)
                        && (max_level == 0 || level <= max_level);
                    if !matches {
                        continue;
                    }
                    if cur.is_none() {
                        cur = Some(id);
                        continue;
                    }
                    let r = self.spawn_rng.roll(1, 100);
                    if r <= 29 {
                        cur = Some(id);
                    } else if r >= 99 {
                        break;
                    }
                }
                cur?
            }
        };
        // World-population throttle (L20981-21008): a gamelimit template
        // refuses at active >= limit; a gamelimit-1 template with a kill
        // stamp and nonzero regentime waits base + lngrnd(0, base/4) -
        // base/8 minutes (base = regentime*60), jitter drawn per attempt.
        let (game_limit, cooldown_factor) = {
            let t = &self.content.monsters[&template];
            (t.game_limit, t.unique_cooldown)
        };
        if game_limit != 0 {
            let pop = self.population.get(&template).copied().unwrap_or_default();
            if i32::from(pop.active) >= i32::from(game_limit) {
                return None;
            }
            if game_limit == 1
                && cooldown_factor != 0
                && let Some(last_kill) = pop.last_kill
            {
                let elapsed_min = (self.scheduler.now() as i64 - last_kill) / 60;
                let base = i64::from(cooldown_factor) * 60; // minutes
                // lngrnd(0, base/4): 0..=base/4-1 (upper-exclusive —
                // PLAUSIBLE, monsters.md §2).
                let jitter = i64::from(self.spawn_rng.roll(0, ((base / 4) as i32 - 1).max(0)));
                if elapsed_min < base + jitter - base / 8 {
                    return None;
                }
            }
            self.population.entry(template).or_default().active += 1;
        }
        let _ = boot_boss;
        let tpl = &self.content.monsters[&template];
        let (hitpoints, energy) = (tpl.hitpoints, tpl.energy);
        let (aggression, behaviour) = (tpl.aggression, tpl.behaviour);
        let (roam_class, herd_mode, herd_rank) = (tpl.roam_class, tpl.herd_mode, tpl.exp_multi);
        let coin_maxes = tpl.coins;
        let loot = tpl.loot.clone();
        let move_msg = tpl.move_msg;
        let name = tpl.name.clone();
        // Cash draws (L21042-21056): lngrnd(0, max+1) per NONZERO max.
        let mut coins = [0u32; 5];
        for (i, max) in coin_maxes.iter().enumerate() {
            if *max != 0 {
                coins[i] = self.spawn_rng.roll(0, *max as i32) as u32;
            }
        }
        // Item draws (L21072-21097): a limited template that has NEVER
        // been killed carries every slot with NO draws — the first-kill
        // loot guarantee.
        let ever_killed = self
            .population
            .get(&template)
            .is_some_and(|p| p.last_kill.is_some());
        let guaranteed = game_limit != 0 && !ever_killed;
        let mut items = Vec::new();
        for slot in loot {
            if !self.content.items.contains_key(&slot.item) {
                continue; // shipped dangling ref
            }
            if guaranteed || self.spawn_rng.roll(1, 100) <= i32::from(slot.dropper) {
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
                active_spells: Default::default(),
                poison: 0,
                needs_recompute: false,
                slot_bag: AbilityBag::default(),
                aggression,
                behaviour,
                roam_class,
                herd_mode,
                herd_rank,
                last_move_dir: None,
                give_up: 0,
                suppress: false,
                home: room,
                coins,
            },
        );
        // Boss flag vs spawn count + linked bump (L21149-21162).
        if Some(template) == room_data.boss_monster {
            self.spawn_state(room).boss_present = true;
        } else {
            let st = self.spawn_state(room);
            st.live = st.live.saturating_add(1);
            if let Some(linked) = room_data.linked_room {
                self.spawn_state(linked).linked_live += 1;
            }
        }
        // Entry direction (L21164-21176): compass exits 0..7, TYPE 0 only;
        // first qualifier held, later ones replace on genrdn(1,10) >= 6.
        let mut dir: Option<crate::content::Direction> = None;
        for d in &crate::content::Direction::ALL[..8] {
            let plain = room_data.exits[*d as usize]
                .as_ref()
                .is_some_and(|e| e.dest.room != 0 && e.exit_type == 0);
            if !plain {
                continue;
            }
            if dir.is_none() {
                dir = Some(*d);
                continue;
            }
            if self.spawn_rng.roll(1, 10) >= 6 {
                dir = Some(*d);
            }
        }
        // Arrival line (L21177-21221): movemsg 0 = the default "just
        // arrived" pair; a resolvable custom message prints its first
        // line (empty = silent), %s slots bound (dir-spec, name) —
        // ORACLE-VERIFY the surplus-arg binding.
        match move_msg.and_then(|m| self.content.messages.get(&m)) {
            None => {
                let line = text::spawn_arrived(&name, dir);
                self.broadcast_to_room(room, None, &line);
            }
            Some(msg) => {
                let template_line = msg.lines.first().cloned().unwrap_or_default();
                if !template_line.is_empty() {
                    let dirspec = match dir {
                        None => "nowhere".to_string(),
                        Some(d) => format!("the {}", text::direction_shown(d)),
                    };
                    let line = template_line.replacen("%s", &dirspec, 1);
                    let line = line.replacen("%s", &name, 1);
                    // A custom text with no %s prints verbatim.
                    let line = if template_line.contains("%s") {
                        line
                    } else {
                        template_line
                    };
                    self.broadcast_to_room(room, None, &line);
                }
            }
        }
        // display_entry_movement(0xb, ...) — the adjacent-room rumble:
        // every exit of type {0,3,4,7,9,0xb} tells the far room "You hear
        // movement ..." with the reverse direction.
        for d in crate::content::Direction::ALL {
            let Some(exit) = room_data.exits[d as usize].as_ref() else {
                continue;
            };
            if !matches!(exit.exit_type, 0 | 3 | 4 | 7 | 9 | 0xb) {
                continue;
            }
            let line = text::hear_movement(d.opposite());
            self.broadcast_to_room(exit.dest, None, &line);
        }
        Some(id)
    }

    /// `FUN_004232d3` (0x4232d3, monsters.md §1): the 5 s density pass —
    /// phase B (per-user own rooms, building the room cache) then phase A
    /// (the ~5%-per-exit neighbor pass consuming it). Soft cap ~9 spawns
    /// per pass with resumable cursors. Not reproduced: the stale-map
    /// neighbor lookup (Q1 — our exits carry true destinations), the
    /// stale-cache-bytes multi-hop quirk (Q3), and the stale room pointer
    /// (Q2) — all documented original bugs.
    fn spawn_pass(&mut self) {
        #[derive(Clone, Copy)]
        struct CacheEntry {
            room: RoomId,
            players: u8,
            monsters: u8,
        }
        let mut cache: Vec<CacheEntry> = Vec::new();
        let mut budget = 0u32;
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        let start = self.spawn_user_cursor.min(sessions.len());
        // --- Phase B: per logged-in user, own room ---
        for (idx, sid) in sessions.iter().enumerate().skip(start) {
            let Some(Session::InGame { player, .. }) = self.sessions.get(sid) else {
                continue;
            };
            let room = player.location;
            let entry = match cache.iter().find(|e| e.room == room) {
                Some(e) => *e,
                None => {
                    let monsters =
                        self.monsters.values().filter(|m| m.location == room).count() as u8;
                    let players = self
                        .sessions
                        .values()
                        .filter(|s| {
                            matches!(s, Session::InGame { player, .. } if player.location == room)
                        })
                        .count() as u8;
                    let e = CacheEntry { room, players, monsters };
                    cache.push(e);
                    e
                }
            };
            let Some(r) = self.content.rooms.get(&room) else {
                continue; // a session parked in a nonexistent room
            };
            let (spawn_type, zone, forced, min_l, max_l) =
                (r.room_type, r.spawn_zone, r.forced_monster, r.min_level, r.max_level);
            match spawn_type {
                3 => {
                    // Swarm: no gates, no roll; stops at the budget.
                    while budget < 9
                        && self
                            .generate_monster(room, zone, forced, min_l, max_l, false)
                            .is_some()
                    {
                        budget += 1;
                    }
                }
                0 | 2 => {
                    // The roll draws even when every gate below fails.
                    let roll = self.spawn_rng.roll(1, 100);
                    let spawn = if entry.monsters >= 15 {
                        false
                    } else if entry.monsters < entry.players {
                        let thresh = if spawn_type == 2 { 0x5a } else { 5 };
                        roll < thresh
                            || (roll >= 100 && i32::from(entry.players) * 2 > i32::from(entry.monsters))
                    } else {
                        // Natural-100 fallback while monsters < 2x players.
                        roll >= 100 && i32::from(entry.players) * 2 > i32::from(entry.monsters)
                    };
                    if spawn
                        && self
                            .generate_monster(room, zone, forced, min_l, max_l, false)
                            .is_some()
                    {
                        budget += 1;
                        if budget > 8 {
                            self.spawn_user_cursor = idx;
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
        self.spawn_user_cursor = 0;
        // --- Phase A: the neighbor pass over the cache ---
        let start = self.spawn_cache_cursor;
        let mut i = start;
        while i < cache.len() {
            let entry = cache[i];
            let Some(src) = self.content.rooms.get(&entry.room) else {
                i += 1;
                continue;
            };
            let exits = src.exits.clone();
            for exit in exits.iter().flatten() {
                if exit.dest.room == 0 {
                    continue;
                }
                // ~5% per nonzero exit, rolled before anything else.
                if self.spawn_rng.roll(1, 100) >= 6 {
                    continue;
                }
                if cache.iter().any(|e| e.room == exit.dest) {
                    continue; // already cached this pass
                }
                let (nb_type, zone, forced, min_l, max_l) = {
                    let Some(r) = self.content.rooms.get(&exit.dest) else {
                        continue;
                    };
                    (r.room_type, r.spawn_zone, r.forced_monster, r.min_level, r.max_level)
                };
                let dest = exit.dest;
                match nb_type {
                    3 => {
                        while budget < 9
                            && self
                                .generate_monster(dest, zone, forced, min_l, max_l, false)
                                .is_some()
                        {
                            budget += 1;
                        }
                    }
                    0 | 2 => {
                        // One type-0/2 neighbor attempt per source room:
                        // spawn iff the neighbor's live monsters < the
                        // SOURCE room's players. No further roll.
                        let nb_monsters =
                            self.monsters.values().filter(|m| m.location == dest).count();
                        if nb_monsters < usize::from(entry.players)
                            && self
                                .generate_monster(dest, zone, forced, min_l, max_l, false)
                                .is_some()
                        {
                            budget += 1;
                            if budget > 8 {
                                self.spawn_cache_cursor = i;
                                return;
                            }
                        }
                        cache.push(CacheEntry { room: dest, players: 0, monsters: 0 });
                        break; // local_c = 10 — done with this source room
                    }
                    _ => {
                        cache.push(CacheEntry { room: dest, players: 0, monsters: 0 });
                    }
                }
            }
            i += 1;
        }
        self.spawn_cache_cursor = 0;
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
        let tpl = &self.content.monsters[&template];
        let (aggression, behaviour) = (tpl.aggression, tpl.behaviour);
        let (roam_class, herd_mode, herd_rank) = (tpl.roam_class, tpl.herd_mode, tpl.exp_multi);
        let coins = tpl.coins;
        self.monsters.insert(
            id,
            MonsterInstance {
                template,
                location: room,
                current_hp: hitpoints,
                energy,
                target: None,
                items,
                active_spells: Default::default(),
                poison: 0,
                needs_recompute: false,
                slot_bag: AbilityBag::default(),
                aggression,
                behaviour,
                roam_class,
                herd_mode,
                herd_rank,
                last_move_dir: None,
                give_up: 0,
                suppress: false,
                home: room,
                coins,
            },
        );
        Some(id)
    }

    /// Test/inspection: a live monster's current room.
    pub fn monster_location(&self, id: MonsterInstanceId) -> Option<RoomId> {
        self.monsters.get(&id).map(|m| m.location)
    }

    /// Test/inspection: a live monster's target lock (`mon+0x1a`).
    pub fn monster_target(&self, id: MonsterInstanceId) -> Option<SessionId> {
        self.monsters.get(&id).and_then(|m| m.target)
    }

    /// Test/inspection: every live monster instance id.
    pub fn monster_ids(&self) -> Vec<MonsterInstanceId> {
        self.monsters.keys().copied().collect()
    }

    /// Test/inspection: an instance's template id.
    pub fn monster_template(&self, id: MonsterInstanceId) -> Option<crate::content::MonsterId> {
        self.monsters.get(&id).map(|m| m.template)
    }

    /// Test/inspection: the floor items of a room.
    pub fn debug_room_items(&self, room: RoomId) -> Vec<crate::content::ItemId> {
        self.room_items
            .get(&room)
            .map(|v| v.iter().map(|(id, _)| *id).collect())
            .unwrap_or_default()
    }

    /// Test hook: route a monster through the kill path (no killer).
    pub fn debug_kill_monster(&mut self, id: MonsterInstanceId) {
        self.monster_killed(id, None);
    }

    /// Test hook: one generate_monster attempt with the room's own spawn
    /// parameters — true if something spawned.
    pub fn debug_generate(&mut self, room: RoomId) -> bool {
        let Some(r) = self.content.rooms.get(&room) else {
            return false;
        };
        let (zone, forced, min_l, max_l) =
            (r.spawn_zone, r.forced_monster, r.min_level, r.max_level);
        self.generate_monster(room, zone, forced, min_l, max_l, false)
            .is_some()
    }

    /// Test hook: write a target lock directly — the summon pre-lock shape
    /// (generate_monster param_7; combat can never lock a class 0x25).
    pub fn debug_lock_monster(&mut self, id: MonsterInstanceId, session: SessionId) {
        if let Some(m) = self.monsters.get_mut(&id) {
            m.target = Some(session);
            m.suppress = false;
        }
    }

    /// Test hook: drive one `move_monster` step directly (the wander and
    /// pursuit paths call the same gate ladder).
    pub fn debug_move_monster(&mut self, id: MonsterInstanceId, dir: crate::content::Direction) -> bool {
        self.move_monster(id, dir, false)
    }

    /// Test/inspection: a live monster's current HP (`None` once dead/gone).
    pub fn monster_hp(&self, id: MonsterInstanceId) -> Option<i32> {
        self.monsters.get(&id).map(|m| m.current_hp)
    }

    /// Test/inspection: a live monster's current energy pool (`mon+0x16`).
    pub fn monster_energy(&self, id: MonsterInstanceId) -> Option<i32> {
        self.monsters.get(&id).map(|m| m.energy)
    }

    /// Test/inspection: a live monster's 5 active-spell slots.
    pub fn monster_active_spells(&self, id: MonsterInstanceId) -> Option<[ActiveSpell; 5]> {
        self.monsters.get(&id).map(|m| m.active_spells)
    }

    /// Test/inspection: a live monster's poison counter (`mon+0x14`).
    pub fn monster_poison(&self, id: MonsterInstanceId) -> Option<i16> {
        self.monsters.get(&id).map(|m| m.poison)
    }

    /// Test hook: the slot-fed combat mapping — (evasion, soak, save
    /// stat), i.e. `move_monster_to_fighter`'s AC/DR words plus
    /// [`Core::monster_save_stat`] (regression coverage for the monster
    /// ability fold).
    pub fn monster_defense_debug(&self, id: MonsterInstanceId) -> Option<(i32, i32, i32)> {
        self.monsters.get(&id)?;
        let f = self.build_monster_defender(id);
        Some((f.evasion_a, f.armor, self.monster_save_stat(id)))
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
                Job::Fast => {
                    self.fast_update();
                    self.scheduler.schedule_in(FAST_INTERVAL, Job::Fast);
                }
                Job::Spawn => {
                    self.spawn_pass();
                    self.scheduler.schedule_in(SPAWN_INTERVAL, Job::Spawn);
                }
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

            // Poison (`regeneration.md` §4; decompile 19518-19533;
            // MEASURED §8.14 at a patched counter 5 — the line, the
            // counter damage and the regen all in one slow tick): a
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
        // `slow_update_monster` (19270-19284), in order: clear the
        // anti-backtrack memory (`mon+0x132 = 0xffff`), deal the poison
        // counter as HP damage, then regen `mon+0x130` capped at max HP.
        // NO death check here — the DLL leaves the corpse for the next
        // medium pass's HP < 0 sweep, and so do we.
        let monsters: Vec<MonsterInstanceId> = self.monsters.keys().copied().collect();
        for id in monsters {
            let Some(m) = self.monsters.get_mut(&id) else {
                continue;
            };
            m.last_move_dir = None;
            if m.poison > 0 {
                m.current_hp -= i32::from(m.poison);
            }
            let (max_hp, regen) = self
                .content
                .monsters
                .get(&m.template)
                .map_or((0, 0), |t| (t.hitpoints, i32::from(t.hp_regen)));
            if m.current_hp < max_hp {
                m.current_hp = (m.current_hp + regen).min(max_hp);
            }
        }
    }

    /// The ~3 s routine pass: each in-game player's slot walk (spec §5;
    /// decompile 19807-19830), then each live monster's
    /// (`medium_update_monster`, spec §6.6).
    fn upkeep_update(&mut self) {
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in &sessions {
            // medium_update_character resets the anti-pile-on counter
            // (19748) and the moved-flag (19755).
            if let Some(Session::InGame { attackers_this_tick, moved_this_round, .. }) =
                self.sessions.get_mut(id)
            {
                *attackers_this_tick = 0;
                *moved_this_round = false;
            }
        }
        for id in sessions {
            self.upkeep_player(id);
        }
        // `medium_update_monsters` (0x21b31) resets the wander fairness
        // counter (DAT_0047fb90) once per pass, then each monster runs
        // spell upkeep -> env-death -> wander in one function.
        self.wander_budget = 0;
        let monsters: Vec<MonsterInstanceId> = self.monsters.keys().copied().collect();
        for id in monsters {
            self.upkeep_monster(id);
            self.wander_monster(id);
        }
    }

    /// `medium_update_monster`'s slot half (decompile 19308-19341), per
    /// occupied slot: set the dirty byte, decrement remaining,
    /// `perform_routine_spell_monster_upkeep` (0x4a263 — the REDUCED
    /// handler set: Damage/EnergyLevel/Heal/CurePoison/Fear only), then at
    /// 0 remaining clear the slot and
    /// `perform_spell_termination_monster_upkeep` (0x4a45d — reverses ONLY
    /// Enslave and Poison; no stat reversal, no wear-off lines, NO EndCast
    /// chains). An unknown spell id still decrements but idles past both
    /// (the get_spell_data gate at 19312-19319). After the walk: strictly
    /// negative HP routes through the killer-less death path (19327-19339:
    /// check_kill_monster(-1) + distribute_experience(-1, ...)).
    fn upkeep_monster(&mut self, id: MonsterInstanceId) {
        let max_energy = self
            .monsters
            .get(&id)
            .and_then(|m| self.content.monsters.get(&m.template))
            .map_or(0, |t| t.energy);
        for idx in 0..5 {
            let mut fear_flee = false;
            let (spell_id, stored, remaining) = {
                let Some(m) = self.monsters.get_mut(&id) else {
                    return;
                };
                let Some(spell_id) = m.active_spells[idx].spell else {
                    continue;
                };
                m.needs_recompute = true;
                m.active_spells[idx].remaining -= 1;
                (
                    spell_id,
                    i32::from(m.active_spells[idx].value),
                    m.active_spells[idx].remaining,
                )
            };
            let Some(spell) = self.content.spells.get(&spell_id).cloned() else {
                continue; // unknown id: decrement + dirty only (19312)
            };
            let Some(m) = self.monsters.get_mut(&id) else {
                return;
            };
            // Routine handlers (44905-44962): value = the STORED slot
            // value unless the ability row carries its own nonzero value
            // — the same override convention as the player tick.
            for (ability, row) in &spell.abilities {
                let v = if *row != 0 { i32::from(*row) } else { stored };
                match ability {
                    // Damage (1): HP -= v, dirty (44928-44930). No death
                    // check per slot — the post-walk HP sweep collects it.
                    Ability::Damage => m.current_hp -= v,
                    // EnergyLevel (11): += v, capped at the template max
                    // (`+0x114`; 44932-44936).
                    Ability::EnergyLevel => {
                        m.energy = (m.energy + v).min(max_energy);
                    }
                    // Heal (18): HP += v, UNCAPPED here (44941-44943 —
                    // the monster handler carries no max clamp).
                    Ability::Heal => m.current_hp += v,
                    // Cure Poison (20): the odd `poison - v < poison`
                    // guard just skips v <= 0 (44945-44951).
                    Ability::CurePoison => {
                        if v > 0 {
                            m.poison = clamp_poison(i32::from(m.poison) - v);
                        }
                    }
                    // Fear (60): genrdn(0,100) < v => flee a random exit
                    // via move_monster (44957-44960; the roll draws here,
                    // the step executes after the slot's ability walk —
                    // observably identical, no other arm emits output).
                    Ability::Fear => {
                        if self.rng.roll(0, 100) < v {
                            fear_flee = true;
                        }
                    }
                    _ => {}
                }
            }
            if remaining <= 0 {
                // Expiry (19318-19324): clear the slot FIRST, then
                // terminate with the stored value.
                m.active_spells[idx] = ActiveSpell::default();
                for (ability, row) in &spell.abilities {
                    let v = if *row != 0 { i32::from(*row) } else { stored };
                    match ability {
                        // Enslave (6): release the charm — owner name,
                        // follow flags (44991-44995). M6 PENDING
                        // (retagged at the slice-6 close-out): monster
                        // charm/ownership state ships with M6 pets/
                        // aggro (the Summon owner tag lands there too);
                        // until an Enslave cast can CREATE a charm
                        // there is nothing to release here.
                        Ability::Enslave => {}
                        // Poison (19): counter -= v, floored 0
                        // (45003-45008).
                        Ability::Poison => {
                            m.poison = clamp_poison(i32::from(m.poison) - v);
                        }
                        // NO other reversal and NO EndCast chain — the
                        // monster termination handles exactly these two.
                        _ => {}
                    }
                }
            }
            if fear_flee
                && let Some(from) = self.monsters.get(&id).map(|m| m.location)
                && let Some(dir) = self.pick_valid_random_direction(from)
            {
                self.move_monster(id, dir, false);
            }
        }
        self.recompute_monster_effects(id);
        // The post-walk death sweep (19327-19339): STRICTLY negative — a
        // monster sitting at exactly 0 survives the medium pass.
        if self.monsters.get(&id).is_some_and(|m| m.current_hp < 0) {
            self.monster_killed(id, None);
        }
    }

    /// The wander half of `medium_update_monster` (decompile 19339-19372;
    /// monsters.md §3). Gated to monsters with no target lock and no
    /// directed-travel order; switches on the roam class:
    /// 0/2 stationary; 5 water (no aggression roll, budget consumed before
    /// the confusion check); default rolls `genrdn(0,100) <
    /// (100-aggression)/2` then checks confusion, then consumes budget.
    /// The chosen direction is rejected (budget already spent) when it
    /// equals the last-move memory.
    fn wander_monster(&mut self, id: MonsterInstanceId) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        if m.target.is_some() {
            return; // locked on (`mon+0x1a`); pursuit owns movement
        }
        let (roam, aggression, from, last) =
            (m.roam_class, m.aggression, m.location, m.last_move_dir);
        match roam {
            0 | 2 => return,
            5 => {
                // Water path (19364-19371): cap first (the mon+0x140
                // dirty-byte bypass is unmodeled — PLAUSIBLE quirk),
                // budget consumed before the confusion check.
                if self.wander_budget >= 3 {
                    return;
                }
                self.wander_budget += 1;
                if self.monster_confusion_fumble(id) {
                    return;
                }
            }
            _ => {
                // Default path (19346-19360): cap, roll, confusion, budget.
                if self.wander_budget >= 3 {
                    return;
                }
                let roll = self.rng.roll(0, 100);
                if roll >= (100 - i32::from(aggression)) / 2 {
                    return;
                }
                if self.monster_confusion_fumble(id) {
                    return;
                }
                self.wander_budget += 1;
            }
        }
        let Some(dir) = self.pick_valid_random_direction(from) else {
            return;
        };
        if Some(dir) == last {
            return; // anti-backtrack (mon+0x132), budget already spent
        }
        self.move_monster(id, dir, false);
    }

    /// The 1 s pursuit tier (`fast_update_monster` 19393-19489). The DLL
    /// drains one table slot per user-poll and completes the pass within
    /// the tick interval; we run the whole sweep each second (documented
    /// idealization). Only monsters holding a lock do any work.
    fn fast_update(&mut self) {
        let ids: Vec<MonsterInstanceId> = self.monsters.keys().copied().collect();
        for id in ids {
            self.pursue_monster(id);
        }
    }

    fn pursue_monster(&mut self, id: MonsterInstanceId) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        // (Prone recovery, 19404-19410, joins with the mechanic that can
        // knock monsters prone.)
        let Some(victim) = m.target else {
            return;
        };
        let (mon_room, aggression, roam) = (m.location, m.aggression, m.roam_class);
        // One give-up bump per unprosecutable tick (19414/19426/19433/
        // 19440); same-room ticks never bump.
        let mut bump = false;
        match self.sessions.get(&victim) {
            Some(Session::InGame { player, moved_this_round, .. }) => {
                let (loc, moved) = (player.location, *moved_this_round);
                if loc == mon_room {
                    // nothing to do — acquisition owns the same-room case
                } else if loc.map != mon_room.map || moved {
                    bump = true; // can't chase across maps / a mid-flight runner
                } else if self.rng.roll(0, 100) >= i32::from(aggression) {
                    bump = true; // the follow roll failed
                } else {
                    match self.dir_toward_player(victim, mon_room) {
                        None => bump = true,
                        Some(dir) => {
                            if !self.monster_confusion_fumble(id)
                                && !self.move_monster(id, dir, false)
                            {
                                bump = true;
                            }
                        }
                    }
                }
            }
            _ => bump = true, // logged off — the stale name ages out
        }
        let Some(m) = self.monsters.get_mut(&id) else {
            return;
        };
        if bump {
            m.give_up = m.give_up.saturating_add(1);
        }
        // Give-up past 15 (19446): class 0x25 silently despawns
        // (FUN_004298ec — the spawn-accounting half joins with slice 4);
        // everyone else drops the lock and goes back to wandering.
        if m.give_up > 15 {
            if roam == 0x25 {
                self.monsters.remove(&id);
            } else {
                m.give_up = 0;
                m.target = None;
            }
        }
    }

    /// `dir_player_travelling_coord` (15657-15686): find the monster's
    /// room in the target's breadcrumb trail, then the exit of that room
    /// whose destination is the room the player entered NEXT.
    fn dir_toward_player(
        &self,
        victim: SessionId,
        mon_room: RoomId,
    ) -> Option<crate::content::Direction> {
        let Some(Session::InGame { trail, .. }) = self.sessions.get(&victim) else {
            return None;
        };
        let i = (1..trail.len()).find(|i| trail[*i] == mon_room)?;
        let next_room = trail[i - 1];
        let room = self.content.rooms.get(&mon_room)?;
        crate::content::Direction::ALL
            .into_iter()
            .find(|d| {
                room.exits[*d as usize]
                    .as_ref()
                    .is_some_and(|e| e.dest == next_room)
            })
    }

    /// `check_monster_confusion` (0x29812): Confusion (0x47) value beats
    /// `genrdn(0,100)` => the fumble line — ConfuseMsg (0x65) names a
    /// custom message (first line, %s = instance name), else the stock
    /// "looks around stupidly" text — and the move is forfeited.
    fn monster_confusion_fumble(&mut self, id: MonsterInstanceId) -> bool {
        let confusion = self.monster_ability_value(id, Ability::Confusion);
        if confusion <= 0 || self.rng.roll(0, 100) >= confusion {
            return false;
        }
        let name = self.monster_name(id);
        let room = self.monsters[&id].location;
        let custom = self.monster_ability_value(id, Ability::ConfuseMsg);
        let line = u16::try_from(custom)
            .ok()
            .and_then(|id| self.content.messages.get(&crate::content::MessageId(id)))
            .and_then(|msg| msg.lines.first())
            .map(|l| l.replace("%s", &name));
        let line = line.unwrap_or_else(|| text::monster_confused_fumble(&name));
        self.broadcast_to_room(room, None, &line);
        true
    }

    /// `move_monster` (0x252f3; monsters.md §3): one gated step. Returns
    /// true only when the monster changed rooms. `herd_flag` marks a pack
    /// drag — it skips the hold check and the recursive drag.
    fn move_monster(
        &mut self,
        id: MonsterInstanceId,
        dir: crate::content::Direction,
        herd_flag: bool,
    ) -> bool {
        let Some(m) = self.monsters.get(&id) else {
            return false;
        };
        let (from, roam, my_mode, my_rank, template) =
            (m.location, m.roam_class, m.herd_mode, m.herd_rank, m.template);
        // Gate 1: immobility — HoldPerson (0x4a) never moves; Slowness
        // (0x44) skips ~half of all calls. (The prone branch, mon+0x128
        // bit 8, lands with the mechanic that can knock monsters prone.)
        if self.monster_ability_value(id, Ability::HoldPerson) > 0 {
            return false;
        }
        if self.monster_ability_value(id, Ability::Slowness) > 0 && self.rng.roll(0, 100) <= 0x31 {
            return false;
        }
        // Gate 2: herd hold (herdFlag==0, own mode 1/2): refuse while a
        // same-herd mode-1 packmate holds the room — any mode-1 for a
        // mode-2 monster, a HIGHER-RANKED mode-1 for a mode-1.
        let my_herd = self.content.monsters[&template].herd_id;
        if !herd_flag && (my_mode == 1 || my_mode == 2) {
            let held = self.monsters.iter().any(|(oid, o)| {
                *oid != id
                    && o.location == from
                    && o.herd_mode == 1
                    && (my_mode != 1 || my_rank < o.herd_rank)
                    && self
                        .content
                        .monsters
                        .get(&o.template)
                        .is_some_and(|t| t.herd_id == my_herd)
            });
            if held {
                return false;
            }
        }
        // Gate 3: lair (mode 3) never takes an exit.
        if my_mode == 3 {
            return false;
        }
        let Some(exit) = self
            .content
            .rooms
            .get(&from)
            .and_then(|r| r.exits[dir as usize].clone())
        else {
            return false;
        };
        // Gate 4: exit-type switch (blocked types checked before the zone
        // gate — order swapped from the DLL, observably identical since
        // both merely refuse the step).
        match exit.exit_type {
            1 | 3 | 4 | 6 | 8 | 0xc => return false,
            2 if exit.door_closed && roam != 0x26 => return false,
            7 | 0xb if exit.param != 0 && roam != 5 && roam != 0x26 => return false,
            _ => {}
        }
        // Gate 5: zone leash — dest zone must match the roam class, or a
        // free-roam class applies (0x25: flagless non-type-5 rooms only;
        // 0x26: unconditional; 5: water rooms via non-0x13 exits).
        let Some(dest_room) = self.content.rooms.get(&exit.dest) else {
            return false;
        };
        let allowed = i32::from(dest_room.spawn_zone) == i32::from(roam)
            || (roam == 0x25 && dest_room.attributes == 0 && dest_room.room_type != 5)
            || roam == 0x26
            || (roam == 5 && dest_room.attributes & 2 != 0 && exit.exit_type != 0x13);
        if !allowed {
            return false;
        }
        // Room cap: add_monster_to_room fails on a full 15-slot list.
        let occupants = self
            .monsters
            .values()
            .filter(|o| o.location == exit.dest)
            .count();
        if occupants >= 15 {
            return false;
        }
        // Damage exits (9/0x18): crossing costs genrdn(dmg/2, dmg+1).
        // Type 9 may leave HP negative (the medium sweep collects the
        // corpse); 0x18 floors at 0.
        if matches!(exit.exit_type, 9 | 0x18) {
            let dmg = self.rng.roll(exit.param / 2, exit.param + 1);
            let m = self.monsters.get_mut(&id).expect("checked above");
            m.current_hp -= dmg;
            if exit.exit_type == 0x18 {
                m.current_hp = m.current_hp.max(0);
            }
        }
        // The step: anti-backtrack memory, relocation, both-room lines.
        let name = self.monster_name(id);
        let dest = exit.dest;
        {
            let m = self.monsters.get_mut(&id).expect("checked above");
            m.last_move_dir = Some(dir);
            m.location = dest;
        }
        self.broadcast_to_room(from, None, &text::left_via(&name, dir));
        self.broadcast_to_room(dest, None, &text::monster_moves_in_from(&name, dir.opposite()));
        // Drag (21611-21631): a mode-1 mover pulls same-herd packmates —
        // mode 2, or lower-ranked mode 1 — from the old room, up to the
        // template follower cap.
        if !herd_flag && my_mode == 1 {
            let cap = usize::from(
                u16::try_from(self.content.monsters[&template].follower_cap.max(0))
                    .unwrap_or(0),
            );
            let followers: Vec<MonsterInstanceId> = self
                .monsters
                .iter()
                .filter(|(oid, o)| {
                    **oid != id
                        && o.location == from
                        && (o.herd_mode == 2 || (o.herd_mode == 1 && o.herd_rank < my_rank))
                        && self
                            .content
                            .monsters
                            .get(&o.template)
                            .is_some_and(|t| t.herd_id == my_herd)
                })
                .map(|(oid, _)| *oid)
                .take(cap)
                .collect();
            for f in followers {
                self.move_monster(f, dir, true);
            }
        }
        true
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
            let mut fear_rows: Vec<i32> = Vec::new();
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
                        // Fear (60): genrdn(0,100) < v ⇒ flee out a
                        // random valid exit (44788-44793: roll, then
                        // pick_valid_random_direction, then move_user
                        // MODE 6 — which has no mode-6 branch anywhere in
                        // move_user, i.e. a plain forced move: NO fear-
                        // specific line, just the standard leave/arrive
                        // broadcasts and the destination render).
                        // Deferred past the borrow; the DLL rolls
                        // in-handler, but no shipped Fear carrier (397/
                        // 822/836) pairs Fear with another recurring row,
                        // so the order is unobservable.
                        Ability::Fear => fear_rows.push(v),
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
            for v in fear_rows {
                if self.rng.roll(0, 100) < v {
                    let room = self.player(session).location;
                    if let Some(dir) = self.pick_valid_random_direction(room) {
                        self.move_player(session, dir);
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
        let trail_seed = player.location;
        self.sessions
            .insert(id, Session::InGame {
                player: Box::new(player),
                derived,
                exiting: None,
                target: None,
                aided: false,
                energy: PLAYER_ENERGY_MAX,
                cast_this_round: false,
                casting: None,
                moved_this_round: false,
                attackers_this_tick: 0,
                trail: vec![trail_seed],
            });
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
        let mut active_lines: Vec<String> = player
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
        // MEASURED (§8.14): a bare positive poison counter — no slot
        // needed — appends "You are Poisoned!" to the sheet (seen live at
        // a patched counter with zero active spells; gone after the cure).
        // ORACLE-OPEN: ordering vs the DescMsg active lines is unmeasured
        // (the live capture had no simultaneous buff); appended last.
        if player.poison > 0 {
            active_lines.push("You are Poisoned!".into());
        }
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
        // The formula value is SILVER-denominated (MEASURED §8.7: markup-0
        // shop 38 charged "5 silver nobles" for L1->2, formula = 5; §8.12:
        // a copper-only purse paid 50/100 copper for the same 5/10) — the
        // same `* ratios[0]` conversion the healer services apply.
        let ratios = self.config.coin_ratios;
        let cost = ((i64::from(shop.markup) + 100).max(0) as u64)
            * u64::from(player.level)
            * 5
            / 100
            * ratios[0];
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
        // Target RESOLUTION is match-type-driven (the §8.13 refusal
        // matrix): area types leave the single-target paths entirely —
        // an explicit word refuses kind-keyed, a bare cast sweeps the
        // room. Everything below this dispatch is single-target.
        if spell.match_type.room_wide() {
            self.area_cast(session, &spell, &target);
            return;
        }
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
            // Player-target resolution (match 1/2 benign; MEASURED §8.13):
            // players in the caster's room match by the §8.9 word-prefix
            // rule (`c blur ora` -> Oracle). The refusal matrix is
            // match-type-keyed: only the single-target types carry a
            // target slot — match 0 benign has none, so its lookup always
            // falls to the do-not-see refusal (MEASURED §8.9: "cast blur
            // extra trailing words", no self-cast, no mana, pre-cost).
            let single_target = matches!(
                spell.match_type,
                crate::content::MatchType::Single1 | crate::content::MatchType::Single2
            );
            let room = self.player(session).location;
            let want = target.trim().to_ascii_lowercase();
            let found = if single_target {
                self.in_game_sessions()
                    .filter(|(_, p)| p.location == room)
                    .find(|(_, p)| word_prefix_match(&p.name, &want))
                    .map(|(id, _)| id)
            } else {
                None
            };
            match found {
                Some(target_id) if target_id != session => {
                    self.benign_target_cast(session, target_id, &spell);
                    return;
                }
                Some(_) => {
                    // Own name = a plain self-cast (MEASURED §8.13: the
                    // castmsgb frames keep the name — "You cast blur on
                    // Zinvar!" / "Zinvar casts blur on Zinvar!" — which
                    // is exactly what the self path renders). Fall
                    // through to the benign self tail below.
                }
                None => {
                    if single_target && self.find_monster(room, &target).is_some() {
                        // MEASURED (§8.13): `c blur cat` — benign single
                        // targets are players only, uncharged.
                        self.output_line(session, text::MAY_NOT_CAST_ON_MONSTER);
                        return;
                    }
                    self.output_line(session, &text::do_not_see_here(&target));
                    return;
                }
            }
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
            // The offensive-duration split (cast_monster_target: the
            // engage-only block below is CONDITIONED on duration == 0,
            // 43421-43481): a duration!=0 offensive cast resolves RIGHT
            // NOW — roll, costs, slot entry — with NO engagement and no
            // *Combat Engaged* (engage_autocombat appears only in the
            // duration==0 block and the autocombat-driver re-fire).
            // The command's triple gate messages first (43554-43580):
            // round energy prints the already-cast line, mana its
            // shortfall line. DATA: zero learnable spells reach this
            // (all 65 shipped offensive-duration spells are monster
            // payloads) — fixture-covered until content grows one.
            if spell.duration != 0 {
                let Some(Session::InGame { energy, player, .. }) = self.sessions.get(&session)
                else {
                    return;
                };
                if *energy < round_cost {
                    self.output_line(session, self.already_cast_line(session));
                    return;
                }
                if player.current_mana < mana_cost {
                    self.output_line(session, self.not_enough_mana_line(session));
                    return;
                }
                self.offensive_cast_attempt(session, spell_id, monster_id);
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
            // per-round attempt handles both silently.
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
            // the bare engagement, before any damage landed) — gated like
            // every damaging path since slice 3.
            self.retaliation_lock(monster_id, session);
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
        // at entry time — race/class/gear AND active slots via the same
        // bag the recompute uses).
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
        self.benign_success_effects(session, session, &spell, magnitude, duration);
    }

    /// `cast_user_target` for a benign spell at ANOTHER player (decompile
    /// save gate 41712-41733, resist block 42984-43009; MEASURED §8.13).
    /// The gate/cost/roll skeleton mirrors the benign self tail of
    /// `cast_resolved`; the effects land on the TARGET.
    fn benign_target_cast(
        &mut self,
        session: SessionId,
        target_id: SessionId,
        spell: &crate::content::Spell,
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
            return; // silent, like the self path (no measured message)
        }
        if player.current_mana < mana_cost {
            self.output_line(session, self.not_enough_mana_line(session));
            return;
        }
        // AlterSpLength reads through the TARGET's bag —
        // add_cast_spell_to_user runs on the target terminal (decompile
        // 38165), and the two bags coincide on every measured (self-)cast.
        // ORACLE-VERIFY: never separable live so far.
        let alter_sp_length = if spell.duration == 0 {
            0
        } else {
            self.ability_bag(self.player(target_id)).value(Ability::AlterSpLength)
        };
        if let Some(Session::InGame { cast_this_round, .. }) = self.sessions.get_mut(&session) {
            *cast_this_round = true;
        }
        let target_name = self.player(target_id).name.clone();
        let rng = &mut self.rng;
        let succeeded = cast_roll_succeeds(spellcasting, spell.base_chance, &mut |lo, hi| {
            rng.roll(lo, hi)
        });
        // Saving throw (success only; spec §3): Always, or IfAntiMagic
        // when the target's bag carries AntiMagic (51), rolled against
        // the player MR analog (`user+0xc2` = the MagicRes stat).
        // MEASURED §8.13: blur (typeofresists 1) never rolled against
        // the AntiMagic-less MagicRes-55 dwarf — three straight lands.
        let save_allowed = match spell.save_class {
            crate::content::SaveClass::None => false,
            crate::content::SaveClass::Always => true,
            crate::content::SaveClass::IfAntiMagic => {
                self.ability_bag(self.player(target_id)).value(Ability::AntiMagic) != 0
            }
        };
        let resisted = succeeded && save_allowed && {
            let stat = match self.sessions.get(&target_id) {
                Some(Session::InGame { derived, .. }) => derived.magic_resist,
                _ => 0,
            };
            let rng = &mut self.rng;
            monster_save_resists(stat, &mut |lo, hi| rng.roll(lo, hi))
        };
        let landed = succeeded && !resisted;
        let magnitude = if landed {
            let rng = &mut self.rng;
            spell_magnitude(spell, level, 0, &mut |lo, hi| rng.roll(lo, hi))
        } else {
            0
        };
        let duration = if landed && spell.duration != 0 {
            let rng = &mut self.rng;
            spell_duration(spell, level, alter_sp_length, &mut |lo, hi| rng.roll(lo, hi))
        } else {
            0
        };
        let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session)
        else {
            return;
        };
        *energy -= round_cost;
        if !succeeded {
            // MEASURED (§8.13): caster "You attempt to cast blur at
            // Oracle, but fail." + room "...attempted to cast blur at
            // Oracle, but failed."; the TARGET sees NOTHING. Half mana
            // rounded down, clamped non-negative like every fail path.
            player.current_mana -= (mana_cost / 2).max(0);
            self.output_line(session, &text::cast_fail_at(&spell.name, &target_name));
            self.broadcast_to_room_except(
                room,
                &[session, target_id],
                &text::cast_fail_at_room(&caster_name, &spell.name, &target_name),
            );
            return;
        }
        if resisted {
            // A resist pays like a failed roll — full round cost, half
            // mana (spec §3). ORACLE-VERIFY strings: the DLL resist
            // family; whether the target's second-person line is
            // delivered (unlike the suppressed fail line) is unmeasured
            // — the family ships one, so we deliver it.
            player.current_mana -= (mana_cost / 2).max(0);
            self.output_line(session, &text::cast_resisted(&spell.name, &target_name));
            self.output_line(target_id, &text::you_resisted(&caster_name, &spell.name));
            self.broadcast_to_room_except(
                room,
                &[session, target_id],
                &text::cast_resisted_room(&target_name, &caster_name, &spell.name),
            );
            return;
        }
        player.current_mana -= mana_cost;
        self.benign_success_effects(session, target_id, spell, magnitude, duration);
    }

    /// The room-wide arm of `cast_no_target` (match types 3/5/9/10/11/12/
    /// 13; MEASURED §8.13 for match 12 — flash 51 and stinking cloud 131).
    ///
    /// Target law (the §8.13 correction to spec §4's grouping): players
    /// are NEVER area targets — measured alone, with players present, and
    /// with an explicit player word. NOTE the decompile's player loop is
    /// gated on flag `+0x7c8 & 0x10` (in-room live-target sweep bit), so
    /// "never" is flag-driven, not categorical — §8.13 measured unpartied,
    /// out-of-combat players; ORACLE-OPEN whether the bit ever admits
    /// players (combat sweeps, parties). The sweep covers the decompile's
    /// monster set (3/5/9/11/12); 10/13 iterate players ONLY in the
    /// decompile, and with players excluded they collect nothing, so the
    /// no-effect refusal fires unconditionally. ORACLE-VERIFY: whether 13
    /// really excludes players like 12 is unsettled (the lowest learnable
    /// 13 is priest chant L6 — §8.13 left it open); revisit before bard/
    /// priest support.
    fn area_cast(&mut self, session: SessionId, spell: &crate::content::Spell, target: &str) {
        let room = self.player(session).location;
        // Explicit target words refuse KIND-KEYED before any cost
        // (MEASURED §8.13: `c flash oracle` -> "on a user!", `c stnk cat`
        // -> "on a monster!", both uncharged).
        let words = target.trim();
        if !words.is_empty() {
            let want = words.to_ascii_lowercase();
            let player_hit = self
                .in_game_sessions()
                .filter(|(_, p)| p.location == room)
                .any(|(_, p)| word_prefix_match(&p.name, &want));
            if player_hit {
                self.output_line(session, text::MAY_NOT_CAST_ON_USER);
                return;
            }
            if self.find_monster(room, words).is_some() {
                self.output_line(session, text::MAY_NOT_CAST_ON_MONSTER);
                return;
            }
            // ORACLE-VERIFY: an unmatched word was not measured on the
            // area path — the room-lookup refusal, like every other path.
            self.output_line(session, &text::do_not_see_here(words));
            return;
        }
        // Room protection (§3 step 2) precedes target counting for
        // offensive modes — the same guilt gate and round-cost-only
        // charging as the single-target paths. ORACLE-VERIFY: every
        // learnable area is benign-mode (spelltype 3), so this leg is
        // decompile-mirrored only.
        let round_cost = i32::from(spell.round_cost);
        if spell.target_mode.is_offensive()
            && self.content.rooms.get(&room).is_some_and(|r| r.protected())
        {
            if let Some(Session::InGame { energy, .. }) = self.sessions.get_mut(&session)
                && *energy >= round_cost
            {
                *energy -= round_cost;
            }
            self.output_line(session, text::CAST_GUILT);
            return;
        }
        // Target counting (§3 step 3): live monsters only.
        let targets: Vec<MonsterInstanceId> = if spell.match_type.hits_monsters() {
            self.monsters
                .iter()
                .filter(|(_, m)| m.location == room && m.current_hp > 0)
                .map(|(id, _)| *id)
                .collect()
        } else {
            Vec::new()
        };
        if targets.is_empty() {
            // MEASURED (§8.13): a real pre-charge gate — mana unchanged,
            // the round not spent.
            self.output_line(session, text::SPELL_NO_EFFECT_IN_ROOM);
            return;
        }
        // Costs and the roll at the command, like the benign self path
        // (MEASURED §8.13: flash/stinking cloud mana moved at the
        // prompt). Offensive-mode areas charge here too — the engage-only
        // convention belongs to the single-target monster path.
        let Some(Session::InGame { energy, player, derived, .. }) = self.sessions.get(&session)
        else {
            return;
        };
        let spellcasting = derived.spellcasting;
        let level = player.level;
        let caster_name = player.name.clone();
        let caster_max_hp = derived.max_hp;
        let mana_cost = i32::from(spell.mana_cost);
        if *energy < round_cost {
            return; // silent no-op within the round, like the self path
        }
        if player.current_mana < mana_cost {
            self.output_line(session, self.not_enough_mana_line(session));
            return;
        }
        if let Some(Session::InGame { cast_this_round, .. }) = self.sessions.get_mut(&session) {
            *cast_this_round = true;
        }
        let rng = &mut self.rng;
        let succeeded = cast_roll_succeeds(spellcasting, spell.base_chance, &mut |lo, hi| {
            rng.roll(lo, hi)
        });
        // ONE magnitude roll (elemental resist joins PER TARGET below);
        // match types 3/5/9/10 split it by the target count (spec §3) —
        // fixture-only, no learnable 3/5/9/10 spell ships.
        let magnitude = if succeeded {
            let rng = &mut self.rng;
            let v = spell_magnitude(spell, level, 0, &mut |lo, hi| rng.roll(lo, hi));
            if spell.match_type.splits_magnitude() {
                v / i32::try_from(targets.len()).unwrap_or(1).max(1)
            } else {
                v
            }
        } else {
            0
        };
        let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session)
        else {
            return;
        };
        *energy -= round_cost;
        if !succeeded {
            // ORACLE-VERIFY: no area fail was measured — the untargeted
            // cast_no_target fail pair, half mana rounded down.
            player.current_mana -= (mana_cost / 2).max(0);
            self.output_line(session, &text::cast_fail(&spell.name));
            self.broadcast_to_room(
                room,
                Some(session),
                &text::cast_fail_room(&caster_name, &spell.name),
            );
            return;
        }
        player.current_mana -= mana_cost;
        // Fan-out (MEASURED §8.13): the caster line and ONE room line —
        // NO per-target lines, NO damage numbers, NO combat engagement.
        // Both come from the spell's own castmsgb pair (flash's room line
        // mirrors the caster text, the clouds use the generic "on the
        // room!" frame); the record's target line fires for no one, and
        // the target/damage args stay unbound.
        if let Some(msg) = spell.cast_msg_b.and_then(|id| self.content.messages.get(&id)) {
            let args = text::CastMsgArgs {
                caster: &caster_name,
                target: None,
                spell: &spell.name,
                damage: None,
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
        if spell.duration != 0 {
            // Duration areas enter each monster's 5-slot table
            // (add_duration_spell_to_room 38701-38746, monster leg): value
            // = the magnitude with the per-monster elemental resist for
            // offensive modes (unscaled for benign, 38712/38742), duration
            // re-rolled PER MONSTER inside add_cast_spell_to_monster (the
            // spell_duration twin — an independent band roll each). Slot
            // exhaustion is silent here: the fan-out above already set the
            // caster told-flag, which suppresses the fail line (38277:
            // DAT_00485964 gate). No poison hard-write on the area leg.
            // MEASURED (§8.13): no per-monster lines — display_spell_
            // success's told-flags collapse the area fan-out to the one
            // caster/room pair already printed.
            let alter = self
                .ability_bag(self.player(session))
                .value(Ability::AlterSpLength);
            for monster_id in targets {
                let value = if spell.target_mode.is_offensive() {
                    let resist = spell
                        .element
                        .resist_ability()
                        .map_or(0, |a| self.monster_ability_value(monster_id, a));
                    (100 - resist) * magnitude / 100
                } else {
                    magnitude
                };
                let rng = &mut self.rng;
                let duration =
                    spell_duration(spell, level, alter, &mut |lo, hi| rng.roll(lo, hi));
                self.enter_monster_spell_slot(monster_id, spell.id, value, duration);
            }
            return;
        }
        // Instant apply, PER MONSTER (spec §4: each target gets its own
        // elemental modifier ((100-resist)*V)/100 keyed on the spell's
        // element — the same final scale spell_magnitude applies on the
        // single-target path; fixed non-zero rows bypass roll and resist
        // alike). No saving throw here: the §3 save gate lives in the
        // targeted entry points, not cast_no_target's area loop — whose
        // own resist branch is DEAD CODE (decompiled ~39726-39748:
        // genrdn(1,100) < 1000 always passes; "The %s resists your
        // spell!" is unreachable), even though flash/stinking cloud/
        // poison cloud all ship typeofresists 2.
        let mut kills: Vec<MonsterInstanceId> = Vec::new();
        // AlterSpDmg(165): the area apply loop reads the caster's bag per
        // damage row like the targeted paths (cast_no_target FUN_0043fef4
        // at 39626, DamageMR at 40304-40305 — re-read per target in the
        // DLL, one value here).
        let boost = self
            .ability_bag(self.player(session))
            .value(Ability::AlterSpDmg);
        for monster_id in targets {
            let resist = spell
                .element
                .resist_ability()
                .map_or(0, |a| self.monster_ability_value(monster_id, a));
            let mr = self.monster_save_stat(monster_id);
            let anti_magic = self
                .monsters
                .get(&monster_id)
                .and_then(|m| self.content.monsters.get(&m.template))
                .is_some_and(|t| t.abilities.iter().any(|(a, _)| *a == Ability::AntiMagic));
            let mut damage_total = 0i32;
            let mut drain_total = 0i32;
            let mut harms = false;
            for (ability, value) in &spell.abilities {
                let amount = match *value {
                    0 => (100 - resist) * magnitude / 100,
                    v => i32::from(v),
                };
                match ability {
                    Ability::Damage => {
                        damage_total += alter_sp_dmg(amount, boost);
                        harms = true;
                    }
                    Ability::DamageMR => {
                        damage_total += damage_mr(alter_sp_dmg(amount, boost), mr, anti_magic);
                        harms = true;
                    }
                    Ability::Drain => {
                        drain_total += amount;
                        harms = true;
                    }
                    // Summon (12) is silly_spell on every AREA match
                    // (cast_no_target 40058-40064: the 3/5/9/10-0xd arm)
                    // — a deliberate no-op, not a pending gap.
                    // M6 PENDING (retagged at the slice-6 close-out):
                    // the instant-area arms for the remaining
                    // monster-side abilities (Poison set-if-greater
                    // included — the counter and slots exist; the
                    // single-target twin already writes it). No shipped
                    // LEARNABLE area carries any of them — §8.13
                    // measured zero observable effect and every harm
                    // row is covered above — so the gap is fixture-only
                    // today; wire the arms with M6's monster content
                    // pass.
                    _ => {}
                }
            }
            if !harms {
                continue;
            }
            let dead = {
                let Some(m) = self.monsters.get_mut(&monster_id) else {
                    continue;
                };
                m.current_hp -= damage_total + drain_total;
                m.current_hp <= 0
            };
            // Retaliation lock like every damaging path (gated, slice 3)
            // — but NO caster-side engagement (no *Combat Engaged*
            // MEASURED §8.13 on debuff-only payloads; ORACLE-VERIFY for
            // damaging sweeps — fixture-only today; evil warnings/crime
            // = M7).
            self.retaliation_lock(monster_id, session);
            if drain_total != 0
                && let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
            {
                player.current_hp = (player.current_hp + drain_total).min(caster_max_hp);
            }
            if dead {
                kills.push(monster_id);
            }
        }
        for monster_id in kills {
            self.monster_killed(monster_id, Some(session));
        }
    }

    /// Everything a SUCCESSFUL benign cast does after its costs are paid
    /// — shared by the self-cast command path (`target == session`), the
    /// player-target path (`benign_target_cast`) and the mode-2 forced
    /// cast (`forced_cast`): the dispel pre-pass, the instant apply loop
    /// or duration slot entry, then the success lines. Every effect —
    /// dispel, hard-write, apply loop, slot entry — lands on the TARGET;
    /// only the fan-out geometry involves the caster.
    fn benign_success_effects(
        &mut self,
        session: SessionId,
        target_id: SessionId,
        spell: &crate::content::Spell,
        magnitude: i32,
        duration: i32,
    ) {
        let Some(Session::InGame { derived, .. }) = self.sessions.get(&target_id) else {
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
                && let Some(idx) = self.player(target_id).find_active(SpellId(id))
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
                self.emit_cast_success_lines(session, target_id, spell, magnitude, false);
                self.terminate_active_spell(target_id, idx, honor_endcast);
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
            && self.ability_bag(self.player(target_id)).value(Ability::ImmuPoison) != 0;
        // The driving slot: the first non-noop, non-gated ability. Its
        // fixed-or-rolled value is display_spell_success's param_6 — the
        // damage arg of the success lines (every case passes its own
        // local_9c: 40529-40531 Poison, 40075-40077 Alterhunger, ...; the
        // loop head 39575-39580 rewrites local_9c to the slot value when
        // non-zero, else leaves the rolled magnitude).
        let gated =
            |a: Ability| ability_case_is_noop(a) || (immune_poison && a == Ability::Poison);
        let driving = spell.abilities.iter().find(|(a, _)| !gated(*a));
        let display_damage =
            driving.map_or(magnitude, |(_, v)| if *v != 0 { i32::from(*v) } else { magnitude });
        // Summon (12) drives its own display call (cast_no_target case
        // 0xc, 40040-40042): the target string is the LITERAL "everyone"
        // — the caster/room lines read "... on everyone" and the
        // target-private line is skipped.
        let everyone_target = driving.is_some_and(|(a, _)| *a == Ability::Summon);
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
        // Summon(12) rows collected in the instant loop; spawned after
        // the session borrow drops.
        let mut summons: Vec<i32> = Vec::new();
        if spell.duration == 0 {
            // Instant apply loop (spec §4 table, on the resolved target):
            // iterate the ability slots; a non-zero slot value is a FIXED
            // amount, 0 means the rolled V — pinned on BOTH paths
            // (offensive decompile 43711-43717; benign loop ~39577).
            let Some(Session::InGame { energy, player, .. }) =
                self.sessions.get_mut(&target_id)
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
                    // Summon (12): generate_monster into the caster's
                    // room (cast_no_target case 0xc, 40035-40051) —
                    // instant matches 1/2/6 only; every AREA match is
                    // silly_spell. DATA (slice-5 Task 6 check): all 87
                    // Summon carriers are monster-attack payloads, ZERO
                    // learnable — fixture-reachable only. Spawned after
                    // the loop (the borrow); the caster-name/pet tag is
                    // M6 (see summon_spawn).
                    Ability::Summon => summons.push(amount),
                    // Remaining benign instants land with their systems.
                    _ => {}
                }
            }
        }
        // Duration spells apply NOTHING directly: add_cast_spell_to_user
        // enters them into the target's active-spell slots and the stat
        // recompute reads the slots from there (spec §4; ability_bag
        // folds the occupied slots on every recompute).
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
                        self.sessions.get_mut(&target_id)
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
                let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&target_id)
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
                let snapshot = Box::new(self.player(target_id).clone());
                self.events.push(Event::Persist(snapshot));
                // The slot now feeds the TARGET's ability bag: recompute
                // their cached derived stats (MEASURED §8.13: Oracle's
                // st/MA row moved while blurred).
                self.refresh_derived(target_id);
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
        if !summons.is_empty() {
            let room = self.player(session).location;
            for value in summons {
                self.summon_spawn(value, room, None); // pet links M7
            }
        }
        self.emit_cast_success_lines(session, target_id, spell, display_damage, everyone_target);
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

    /// `display_spell_success` for a benign cast (decompile 38004-38011):
    /// the castmsgb fan-out, then the DescMsg (115) line3 active line on
    /// duration casts — both keyed on the resolved TARGET (`target_id ==
    /// session` for a self-cast). `damage` is the function's param_6
    /// — the caller's fixed-or-rolled magnitude, bound into any `%d` slot
    /// of the message (even caster line 37988 `prf(local_60, spellName,
    /// target, param_6)`; odd caster line 38068 `prf(local_60, target,
    /// param_6)`) — annointed hands (744) and minor healing (13) both
    /// carry a `%d` that binds the heal roll.
    ///
    /// `everyone_target`: the Summon (12) display call passes the literal
    /// "everyone" as the target string (cast_no_target 40040-40042) and
    /// sends no target-private line.
    fn emit_cast_success_lines(
        &mut self,
        session: SessionId,
        target_id: SessionId,
        spell: &crate::content::Spell,
        damage: i32,
        everyone_target: bool,
    ) {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return;
        };
        let caster_name = player.name.clone();
        let room = player.location;
        let target_name = if everyone_target {
            "everyone".to_string()
        } else {
            self.player(target_id).name.clone()
        };
        // Cast messages: castmsgb only (castmsga is the empty message on
        // every sampled spell — the Task-10 renderer contract). Fan-out
        // (MEASURED §8.6 self / §8.13 player-target): the caster line
        // always prints; the TARGET line goes to the resolved target
        // only when it is another player (a self-cast delivers it to no
        // one — `c blur` printed the caster line only); the room line
        // goes to everyone else.
        if let Some(msg) = spell.cast_msg_b.and_then(|id| self.content.messages.get(&id)) {
            let args = text::CastMsgArgs {
                caster: &caster_name,
                target: Some(&target_name),
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
            let target_line = text::render_cast_line(msg, text::CastAudience::Target, &args, odd);
            let room_line = text::render_cast_line(msg, text::CastAudience::Room, &args, odd);
            if let Some(line) = caster_line {
                self.output_line(session, &line);
            }
            if !everyone_target
                && target_id != session
                && let Some(line) = target_line
            {
                self.output_line(target_id, &line);
            }
            if let Some(line) = room_line {
                self.broadcast_to_room_except(room, &[session, target_id], &line);
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
        // ... and it goes to the TARGET (MEASURED §8.13: "You are
        // blurred!" arrived async on Oracle's terminal, never the
        // caster's).
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
            self.output_line(target_id, &line);
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
                // named by no LearnSp scroll AND by no kind-2 attack
                // form — the DB census finds no caster), so the pair
                // stays unreachable even with slice-6 monster casting
                // live; wire add-at-entry and subtract-here together if
                // content ever names it.
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
    /// none is named by any LearnSp scroll). Monster slots exist since
    /// slice 6, but the offensive arm stays M6-pending for lack of any
    /// reachable trigger (the gate below).
    fn forced_cast(&mut self, session: SessionId, spell_id: SpellId) {
        let Some(spell) = self.content.spells.get(&spell_id).cloned() else {
            return;
        };
        if spell.target_mode.is_offensive() {
            // M6 PENDING (retagged at the slice-6 close-out): the
            // offensive forced-cast arm (an EndCast chain firing at a
            // monster). Monster slots exist since slice 6, but no
            // shipped trigger reaches this — all 48 EndCast carriers
            // are unlearnable (see the doc above) — so the arm stays a
            // silent refusal until a reachable trigger ships.
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
        self.benign_success_effects(session, session, &spell, magnitude, duration);
    }

    /// One Summon(12) row: the fixed-or-rolled value IS the template id,
    /// spawned into the given room (every apply loop passes it straight
    /// to `generate_monster`: monster single 23259, player self 40044,
    /// player-at-monster 43911). An unknown template spawns nothing, like
    /// generate_monster's 0 return. The MONSTER-cast path tags the spawn
    /// with the victim's name (23263 -> mon+0x1a, +0x116 = 0) — it wakes
    /// up already hunting. The player-path caster/pet links (40048-40050,
    /// 43915-43925: +0x116 = 1 guardian suppression, charm ownership) are
    /// M7 PENDING with the charm system — those summons stand idle.
    fn summon_spawn(&mut self, template: i32, room: RoomId, lock: Option<SessionId>) {
        if let Ok(id) = u16::try_from(template)
            && let Some(spawned) = self.spawn_monster(crate::content::MonsterId(id), room)
            && let Some(victim) = lock
            && let Some(m) = self.monsters.get_mut(&spawned)
        {
            m.target = Some(victim);
            m.suppress = false;
        }
    }

    /// `pick_valid_random_direction` (decompile 67383-67421): scan the
    /// ten exit slots in storage order; an exit qualifies when its TYPE is
    /// one of {0, 2, 5, 7, 11, 19, 24} (closed doors, action exits and
    /// the rest never). The FIRST qualifying exit is held and every LATER
    /// one replaces it on `genrdn(0,100) < 40` — a front-weighted
    /// reservoir, not a uniform pick. `None` when no exit qualifies.
    fn pick_valid_random_direction(&mut self, room: RoomId) -> Option<Direction> {
        let r = self.content.rooms.get(&room)?;
        let mut held = None;
        for d in Direction::ALL {
            let qualifies = r.exits[d as usize]
                .as_ref()
                .is_some_and(|e| matches!(e.exit_type, 0 | 2 | 5 | 7 | 11 | 19 | 24));
            if qualifies && (held.is_none() || self.rng.roll(0, 100) < 40) {
                held = Some(d);
            }
        }
        held
    }

    /// A live monster's template name (empty if the instance is gone).
    fn monster_name(&self, id: MonsterInstanceId) -> String {
        self.monsters
            .get(&id)
            .and_then(|m| self.content.monsters.get(&m.template))
            .map_or_else(String::new, |t| t.name.clone())
    }

    /// `get_monster_ability_value` (decompile 37150-37270): the template's
    /// ability rows PLUS the 5 active-spell slots (value-0 rows substitute
    /// the stored slot value — the cached [`MonsterInstance::slot_bag`]
    /// fold). KNOWN-DIVERGENCES, both shared with the player bag: the DLL
    /// max-not-sums a resist-ability id set (37173-37185) where we sum
    /// (shipped templates carry at most one row per resist), and a
    /// NegateAbility(124) row naming the queried id zeroes the DLL's whole
    /// answer where we skip the row in the fold (37209-37212; zero shipped
    /// monster payloads carry 124). Carried/wielded item terms join with
    /// M6 monster inventories.
    fn monster_ability_value(&self, id: MonsterInstanceId, ability: Ability) -> i32 {
        let Some(m) = self.monsters.get(&id) else {
            return 0;
        };
        let template: i32 = self
            .content
            .monsters
            .get(&m.template)
            .map_or(0, |t| {
                t.abilities
                    .iter()
                    .filter(|(a, _)| *a == ability)
                    .map(|(_, v)| i32::from(*v))
                    .sum()
            });
        template + m.slot_bag.value(ability)
    }

    /// Rebuilds the cached slot fold when the dirty byte (`mon+0x140`) is
    /// set — the monster ability-bag-lite. The DLL leaves the byte for the
    /// next record touch; every write site here recomputes immediately, so
    /// reads never see a stale fold. Skips NegateAbility rows like the
    /// player fold (see [`Core::monster_ability_value`]).
    fn recompute_monster_effects(&mut self, id: MonsterInstanceId) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        if !m.needs_recompute {
            return;
        }
        let slots = m.active_spells;
        let mut bag = AbilityBag::default();
        let negate = Ability::from_id(124).expect("NegateAbility in the enum");
        for slot in &slots {
            let Some(spell) = slot.spell.and_then(|sid| self.content.spells.get(&sid)) else {
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
                bag.add(*ability, v);
            }
        }
        if let Some(m) = self.monsters.get_mut(&id) {
            m.slot_bag = bag;
            m.needs_recompute = false;
        }
    }

    /// `add_cast_spell_to_monster`'s slot write (decompile 38258-38285):
    /// an active slot with the same id is overwritten UNCONDITIONALLY
    /// (value and duration both — no exceed check, unlike the monster→
    /// player entry); otherwise the first free of the 5; both set the
    /// dirty byte. Returns `false` when the table is full (the caller owns
    /// the fail line, 38277-38283). Value/duration scaling (38236-38256 —
    /// the [`spell_duration`] twin, AlterSpLength included) happens at the
    /// call sites, which own the caster context.
    fn enter_monster_spell_slot(
        &mut self,
        id: MonsterInstanceId,
        spell_id: SpellId,
        value: i32,
        duration: i32,
    ) -> bool {
        let entered = {
            let Some(m) = self.monsters.get_mut(&id) else {
                return false;
            };
            let idx = m
                .active_spells
                .iter()
                .position(|s| s.spell == Some(spell_id))
                .or_else(|| m.active_spells.iter().position(|s| s.spell.is_none()));
            match idx {
                Some(idx) => {
                    m.active_spells[idx] = ActiveSpell {
                        spell: Some(spell_id),
                        value: value as i16,
                        remaining: duration,
                    };
                    m.needs_recompute = true;
                    true
                }
                None => false,
            }
        };
        if entered {
            self.recompute_monster_effects(id);
        }
        entered
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
            // whiffed rounds too) — driver rounds only: the command-time
            // duration path never engaged, and the DLL's fail branch sets
            // no aggro there (44234-44265 prints and moves on). Gated
            // like every lock since slice 3.
            if spell.duration == 0
                && self.monsters.get(&monster_id).is_some_and(|m| m.target.is_none())
            {
                self.retaliation_lock(monster_id, session);
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

        // Offensive abilities (spec §4 table): Damage (1), Damage(-MR)
        // (17), Drain (8) and Summon (12) instant; the duration table
        // enters the monster's 5 slots below (the area twins live in
        // `area_cast`). M6 PENDING (retagged at the slice-6 close-out):
        // the instant Enslave (needs the M6 charm state) and the
        // benign-at-monster instant arms (Heal/EnergyLevel/CurePoison,
        // cast_monster_target 43824-43882/44131-44160) — the command
        // path refuses benign-at-monster outright (§8.13
        // MAY_NOT_CAST_ON_MONSTER), so only the M6-pending forced-cast
        // route could ever reach them. A non-zero
        // ability value is a FIXED amount that bypasses both the magnitude
        // roll and the resist scaling (but NOT the 17 MR scale, which the
        // DLL applies to the fixed-or-rolled amount alike); value 0 means
        // "use the rolled magnitude" (decompile 43711-43717: slot value
        // == 0 selects the rolled local_20). Combined totals assume at
        // most one harm slot per spell — true for ALL shipped data (zero
        // spells carry two of Damage/Drain/DamageMR). The DLL applies
        // per-slot, a kill STOPS its loop (skipping later slots' caster
        // heal), and the message prints the first slot's amount — the
        // slice-5 area loop shares this combined model, and neither copy
        // must survive if multi-slot content ever appears.
        let mr = self.monster_save_stat(monster_id);
        // AlterSpDmg(165), from the caster's bag (get_user_ability_value
        // 0xa5): boosts Damage via FUN_0043fef4 (43740) and DamageMR
        // inline BEFORE the MR scale (43940-43941). Never Drain.
        let boost = self
            .ability_bag(self.player(session))
            .value(Ability::AlterSpDmg);
        // Duration scaling for the slot entry (`add_cast_spell_to_monster`
        // 38238-38256 — the spell_duration twin: level-cap clamp, divide-
        // first increase, band roll, AlterSpLength from the caster's bag).
        let duration = if spell.duration != 0 {
            let alter = self
                .ability_bag(self.player(session))
                .value(Ability::AlterSpLength);
            let rng = &mut self.rng;
            spell_duration(&spell, level, alter, &mut |lo, hi| rng.roll(lo, hi))
        } else {
            0
        };
        let immune_poison = self.monster_ability_value(monster_id, Ability::ImmuPoison) != 0;
        let mut damage_total = 0i32;
        let mut drain_total = 0i32;
        let mut harms = false;
        // The slot-entry once-flag (the DLL's local_36): None = no slot
        // candidacy seen, Some(ok) = the one entry attempt's outcome.
        let mut entered: Option<bool> = None;
        for (ability, value) in &spell.abilities {
            let amount = match *value {
                0 => magnitude,
                v => i32::from(v),
            };
            match ability {
                // Damage (1) has NO duration gate (43738-43776): a
                // duration spell's damage row lands instantly at cast —
                // the recurring copy comes from the slot at upkeep.
                Ability::Damage => {
                    damage_total += alter_sp_dmg(amount, boost);
                    harms = true;
                }
                // Damage(-MR) (17): the dominant attack-spell damage
                // (magic missile included) — the boosted amount scaled by
                // the target's MR, the same stat the save reads
                // (damage_mr; decompile 43937-43993). No duration gate
                // either (44287).
                Ability::DamageMR => {
                    damage_total += damage_mr(alter_sp_dmg(amount, boost), mr, anti_magic);
                    harms = true;
                }
                // Drain (8): instant when duration 0 (target loses it,
                // the caster gains it capped); a duration cast slots it
                // instead (43884-43898).
                Ability::Drain if duration == 0 => {
                    drain_total += amount;
                    harms = true;
                }
                Ability::Drain => {
                    if entered.is_none() {
                        entered = Some(self.enter_monster_spell_slot(
                            monster_id, spell.id, amount, duration,
                        ));
                    }
                }
                // Poison (19), case 0x13 (44066-44131): ImmuPoison(21) on
                // the monster gates the whole case; the counter is
                // SET-IF-GREATER on both arms, and the duration arm slots
                // the spell too.
                Ability::Poison => {
                    if immune_poison {
                        continue;
                    }
                    if duration != 0 && entered.is_none() {
                        entered = Some(self.enter_monster_spell_slot(
                            monster_id, spell.id, amount, duration,
                        ));
                    }
                    if let Some(m) = self.monsters.get_mut(&monster_id) {
                        let v = clamp_poison(amount);
                        if m.poison < v {
                            m.poison = v;
                        }
                        m.needs_recompute = true;
                    }
                    self.recompute_monster_effects(monster_id);
                }
                // Summon (12): the instant arm spawns the named monster
                // into the caster's room (cast_monster_target 43903-
                // 43926); the duration arm is silly_spell — a no-op here
                // (43927-43929).
                Ability::Summon if duration == 0 => {
                    self.summon_spawn(amount, room, None); // hunt links M7
                }
                Ability::Summon => {}
                // Every other row in a DURATION cast drives the one slot
                // entry (the cast_monster_target default arm, 43778-43799
                // — Enslave's charm half is M6, marker below in the
                // termination). Instant casts leave them to their systems.
                _ => {
                    if duration != 0 && entered.is_none() {
                        entered = Some(self.enter_monster_spell_slot(
                            monster_id, spell.id, amount, duration,
                        ));
                    }
                }
            }
        }
        if entered == Some(false) {
            // Both slot scans exhausted: "You attempt to cast %s, but
            // fail." to the caster only (38277-38283) — the effect is
            // lost, the full costs stay paid, and display_spell_success
            // is never reached (no castmsgb, no room line).
            self.output_line(session, &text::cast_fail(&spell.name));
            return;
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
        // stays ORACLE-VERIFY (the §8.14 expedition ran a mage, not a
        // mystic — an M6+ expedition item).
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
            // Slot-only payload: entered above, castmsgb printed, no HP
            // touch and NO retaliation mark (the DLL's default duration
            // arm sets no aggro — 43778-43799 writes the slot and breaks).
            return;
        };
        let dead = {
            let Some(m) = self.monsters.get_mut(&monster_id) else {
                return;
            };
            m.current_hp -= damage;
            m.current_hp <= 0
        };
        self.retaliation_lock(monster_id, session);
        // Drain: the stolen HP heals the caster, capped at max (spec §4).
        if drain_total != 0
            && let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
        {
            player.current_hp = (player.current_hp + drain_total).min(caster_max_hp);
        }
        if dead {
            self.monster_killed(monster_id, Some(session));
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
    /// decompile-only — STILL ORACLE-VERIFY: §8.14's live poisoned cure
    /// ran through the Silvermere TEMPLE healer, a textblock service
    /// with its own menu and prices — Cure Poison 10 gold, "The healer
    /// casts cure poison on you!" — not this healer-shop path; the cure
    /// itself, counter to zero + zero further ticks, is measured).
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

        // The round clears every player's moved-flag before the drivers
        // (energy_update_character, decompile 18619).
        for id in &sessions {
            if let Some(Session::InGame { moved_this_round, .. }) = self.sessions.get_mut(id) {
                *moved_this_round = false;
            }
        }

        if self.rng.roll(0, 100) < 60 {
            self.player_combat_driver(&sessions);
            self.monster_combat_driver();
        } else {
            self.monster_combat_driver();
            self.player_combat_driver(&sessions);
        }
    }

    fn player_combat_driver(&mut self, sessions: &[SessionId]) {
        for id in sessions {
            self.player_attack_sequence(*id);
        }
    }

    /// `FUN_00423863` (decompile 20335-20525): the per-round monster AI —
    /// iterate players (session order; the DLL walks a per-slow-tick
    /// shuffled map, a documented divergence), then the monsters in each
    /// player's room. A monster co-located with N players is visited N
    /// times; the full-energy entry gate makes the extra visits no-ops.
    fn monster_combat_driver(&mut self) {
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for s in sessions {
            let Some(Session::InGame { player, .. }) = self.sessions.get(&s) else {
                continue;
            };
            let room = player.location;
            let here: Vec<MonsterInstanceId> = self
                .monsters
                .iter()
                .filter(|(_, m)| m.location == room)
                .map(|(id, _)| *id)
                .take(15)
                .collect();
            for mid in here {
                self.monster_consider(mid);
            }
        }
    }

    /// One monster's acquisition/locked-attack decision (the per-monster
    /// body of `FUN_00423863`).
    fn monster_consider(&mut self, id: MonsterInstanceId) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        if m.current_hp <= 0 {
            return;
        }
        let (room, behaviour, roam, suppress) =
            (m.location, m.behaviour, m.roam_class, m.suppress);
        if let Some(victim) = m.target {
            // A2 — locked (20465-20519): no roll, attacked every round the
            // lock is valid. (The suppressed class-5 ward-defence branch
            // needs PvP and the charmed pet-assist branch is M7 charm.)
            if !suppress {
                if self.acquisition_valid(victim, room) {
                    self.bump_attackers(victim);
                    self.monster_attack(id, victim);
                }
            } else if !matches!(behaviour, 4 | 0 | 3) {
                // Suppressed aggressive (20492-20509): attacks the first
                // OTHER valid player — never its named target.
                let other = self.sessions_in_room(room).into_iter().find(|s| {
                    *s != victim && self.acquisition_valid(*s, room)
                });
                if let Some(other) = other {
                    self.bump_attackers(other);
                    self.monster_attack(id, other);
                }
            }
            return;
        }
        // A1 — no target. (The directed-travel monster-hunt branch, +0x88,
        // is monster-vs-monster combat — M7.)
        if roam == 5 {
            // Class-5 guardians (20408-20448): base 100, fame-keyed.
            let candidates = self.sessions_in_room(room);
            for s in candidates {
                if !self.acquisition_valid(s, room) {
                    continue;
                }
                let fighting_me = matches!(
                    self.sessions.get(&s),
                    Some(Session::InGame { target: Some(t), .. }) if *t == id
                );
                let fame = self.player(s).fame;
                let attack = if fighting_me {
                    true
                } else {
                    let fame_ok = if behaviour == 6 { fame < 0x28 } else { fame >= 0x28 };
                    fame_ok && {
                        let taper = self.attackers_of(s) * 5;
                        self.rng.roll(0, 100) < 100 - taper
                    }
                };
                if attack {
                    self.bump_attackers(s);
                    self.monster_attack(id, s);
                    return;
                }
            }
            return;
        }
        // Default classes (20372-20406): passive modes never initiate.
        if matches!(behaviour, 4 | 0 | 3) {
            return;
        }
        let mut fallback = None;
        for s in self.sessions_in_room(room) {
            if !self.acquisition_valid(s, room) {
                continue;
            }
            let fighting_me = matches!(
                self.sessions.get(&s),
                Some(Session::InGame { target: Some(t), .. }) if *t == id
            );
            if behaviour == 6 && self.player(s).fame >= 0x28 && !fighting_me {
                continue; // mode 6 spares the famous (20386-20390)
            }
            let taper = self.attackers_of(s) * 5;
            let roll = self.rng.roll(0, 100);
            fallback = Some(s); // last ROLLED candidate (20392)
            if roll < 50 - taper {
                self.bump_attackers(s);
                self.monster_attack(id, s);
                return;
            }
        }
        // Fallback (20401-20404): an eligible monster always attacks.
        if let Some(s) = fallback {
            self.bump_attackers(s);
            self.monster_attack(id, s);
        }
    }

    /// `FUN_004237de` (20296-20330): same room + the sneak/moved gates.
    /// Hidden/sneak state is unmodeled; the moved-this-round flag is live.
    fn acquisition_valid(&self, session: SessionId, room: RoomId) -> bool {
        matches!(
            self.sessions.get(&session),
            Some(Session::InGame { player, moved_this_round: false, .. })
                if player.location == room
        )
    }

    fn sessions_in_room(&self, room: RoomId) -> Vec<SessionId> {
        self.sessions
            .iter()
            .filter(|(_, s)| {
                matches!(s, Session::InGame { player, .. } if player.location == room)
            })
            .map(|(id, _)| *id)
            .collect()
    }

    fn attackers_of(&self, session: SessionId) -> i32 {
        match self.sessions.get(&session) {
            Some(Session::InGame { attackers_this_tick, .. }) => *attackers_this_tick,
            _ => 0,
        }
    }

    /// `player+0x6f0 += 1` — done at every attack launch site.
    fn bump_attackers(&mut self, session: SessionId) {
        if let Some(Session::InGame { attackers_this_tick, .. }) =
            self.sessions.get_mut(&session)
        {
            *attackers_this_tick += 1;
        }
    }

    /// The retaliation lock (`attack_user_monster` 26230-26236/26514-26525,
    /// and the cast-path twins): a hit monster locks its attacker iff
    /// `genrdn(1,100) < aggression` OR it is a passive mode (3/0/4); the
    /// roll draws either way. Class 0x25 never locks; class 5 keeps an
    /// existing lock. (The charmed bit-0 exemption is M7.)
    fn retaliation_lock(&mut self, id: MonsterInstanceId, attacker: SessionId) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        if m.roam_class == 0x25 || (m.roam_class == 5 && m.target.is_some()) {
            return;
        }
        let (aggression, behaviour) = (m.aggression, m.behaviour);
        let roll = self.rng.roll(1, 100);
        if roll < i32::from(aggression) || matches!(behaviour, 3 | 0 | 4) {
            let m = self.monsters.get_mut(&id).expect("checked above");
            m.target = Some(attacker);
            m.suppress = false;
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
                        m.current_hp <= 0
                    };
                    if dead {
                        self.monster_killed(target, Some(session));
                        return;
                    }
                    // Retaliation: gated lock per hit (26514-26525).
                    self.retaliation_lock(target, session);
                }
            }
        }
        // The engage-time lock re-mark (26230-26236) — same gates.
        if self.monsters.get(&target).is_some_and(|m| m.target.is_none()) {
            self.retaliation_lock(target, session);
        }
    }

    /// `attack_monster_user` (decompile 26667-27208): entry gates, the
    /// engage effects, form selection by cumulative weight, the 6-swing
    /// energy loop, then the post-swing target-lock re-roll. Both the
    /// acquisition paths and the flee free-attack funnel here with an
    /// explicit victim; the lock (`mon+0x1a`) is an OUTPUT of the attack,
    /// not its driver.
    fn monster_attack(&mut self, id: MonsterInstanceId, victim: SessionId) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        if m.current_hp <= 0 {
            return;
        }
        let template = m.template;
        let location = m.location;
        let behaviour = m.behaviour;
        let roam = m.roam_class;
        let victim_here = matches!(
            self.sessions.get(&victim),
            Some(Session::InGame { player, .. }) if player.location == location
        );
        if !victim_here {
            return; // no drop — the fast tier's give-up owns stale locks
        }
        // Full-energy entry gate (26706): also what makes the driver's
        // repeat visits in multi-player rooms no-ops.
        let (max_energy, forms) = {
            let tpl = self.content.monsters.get(&template).expect("live instance");
            (tpl.energy, tpl.attacks)
        };
        if m.energy < max_energy {
            return;
        }
        // Safe room (26707): no combat where room+0x564 bit 1 is set.
        if self.content.rooms[&location].protected() {
            return;
        }
        // Downed victims (26751-26755): only modes {1,2,4,5,6} finish the
        // helpless, and roam-5 guardians only the notorious (fame >= 0x50).
        let (victim_hp, victim_fame) = {
            let p = self.player(victim);
            (p.current_hp, p.fame)
        };
        if victim_hp < 1
            && (!matches!(behaviour, 1 | 2 | 4 | 5 | 6) || (roam == 5 && victim_fame < 0x50))
        {
            return;
        }
        // Confusion fumbles the whole sequence (26765-26767).
        if self.monster_confusion_fumble(id) {
            return;
        }
        // Engage effects (26768-26777): exit guard, give-up reset.
        self.cancel_exit(victim);
        if let Some(m) = self.monsters.get_mut(&id) {
            m.give_up = 0;
        }

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
            if form.kind == 2 {
                // The driver's cast arm (decompile 26796-26806):
                // monster_cast return 2 = the victim died — the swing
                // loop stops; 1 = on to the next swing. The return-0
                // melee-form-0 fallback (26805-26813) is unreachable with
                // shipped data (every kind-2 form names a live spell) and
                // is not mirrored — a bad form skips to the next swing.
                // RESOLVED (M6 slice 3): the §8.14 "free retargeting" IS
                // the acquisition model — the post-swing lock re-roll
                // drops aggressive locks most rounds, and FUN_00423863
                // re-picks the victim (fallback = last rolled candidate)
                // each round; casts fire at whatever victim the driver
                // handed us, exactly like the moaning spirit's alternation.
                if self.monster_cast_at_player(id, template, location, &form, victim) {
                    return;
                }
                continue;
            }
            if form.kind != 1 {
                continue; // rob forms (kind 3) arrive with M6+ theft
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
                    // check_kill_user clears the killer's lock (27194).
                    if let Some(m) = self.monsters.get_mut(&id) {
                        m.target = None;
                    }
                    self.player_killed(victim);
                    return;
                }
            }
        }
        // Post-swing target-lock re-roll (26867-26885): genrdn(1,100) vs
        // the template's follow word decides whether the lock (re)lands;
        // aggressive modes DROP an unlanded lock — the §8.14 free
        // retargeting. Classes 0x25 never lock; class 5 keeps an existing
        // lock unrolled.
        let keep_rolling = {
            let m = &self.monsters[&id];
            m.roam_class != 0x25 && (m.roam_class != 5 || m.target.is_none())
        };
        if keep_rolling {
            let aggression = self.monsters[&id].aggression;
            let roll = self.rng.roll(1, 100);
            let m = self.monsters.get_mut(&id).expect("checked above");
            if roll < i32::from(aggression) {
                m.target = Some(victim);
                m.suppress = false;
            } else if !matches!(m.behaviour, 4 | 0 | 3) {
                m.target = None;
            }
            // Passive modes keep whatever lock they already hold.
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

    /// `monster_cast` (decompile 0x27cc3, 22949-23785; spec §6) — one
    /// kind-2 attack form fired at the engaged player. Returns `true` when
    /// the victim died (the DLL's return 2). Shape:
    ///
    /// 1. Spell from the form's `accuracy` word (`template+0x12e`, 23000);
    ///    an unresolvable id skips the swing.
    /// 2. Whole-cast match-type gate {0,2,6,8} (23015-23016): matches
    ///    OUTSIDE the set route to [`Self::monster_cast_area`]
    ///    (23777-23779) — the room-wide sibling. Target MODE never
    ///    routes: a mode-3 single like mummy's `breathes` (84, match 0)
    ///    resolves right here; mode only gates the elemental-resist
    ///    scale (23091).
    /// 3. Energy: the form's cost word (`template+0x190`) gates against
    ///    the monster pool (23041) — an unaffordable cast is SILENT and
    ///    moves to the next swing (23773-23775 return 1), unlike the melee
    ///    arm's loop break. Landing pays full (23123); a resist OR fizzle
    ///    pays half, floored at 1 for a nonzero cost (23065, 23075-23087,
    ///    23758-23770).
    /// 4. Chance/save: [`monster_cast_chance_passes`] then the resist
    ///    ladder — SpellImmu auto-resist (23026-23029: the ability read IS
    ///    the confirmed `required_power < value` comparison, like the
    ///    player command gate at 43630; the OR'd FUN_0043e3db call at
    ///    23042-23061 is a separate predicate whose body scans the 20
    ///    worn-item slots, decompiled 37879-37908 — possibly item-granted
    ///    spell immunity, unmirrored here; ORACLE-VERIFY, spec §7 hedge).
    ///    The resist is NOT gated on the chance roll (23066 tests bVar3
    ///    first — an immune target sees the resist family even for a
    ///    fizzled attempt); then [`player_save_resists`] only when the
    ///    chance roll passed and the save class grants one.
    /// 5. Magnitude (23124-23143): L = the form's cast level
    ///    (`attackmaxhcastlvl`, `template+0x148`) — the DLL applies NO
    ///    level_cap clamp here (no `+0xa2` read anywhere in monster_cast,
    ///    unlike the player path's 43668-43704), so the raw cast level
    ///    scales the bounds; the roll and elemental-resist scale otherwise
    ///    match [`spell_magnitude`]. Duration (23144-23148) is the
    ///    divide-first scaling with NO genrdn band and NO AlterSpLength
    ///    (spec §6.5 — fixed duration).
    /// 6. Instant slots apply per the §4 table at the PLAYER (23150-23740);
    ///    duration spells enter the victim's 10-slot table through
    ///    `monster_add_cast_spell_to_user` semantics (21777-21816):
    ///    refresh only if the new value EXCEEDS the stored one, fixed
    ///    duration, display only on a real write, FULL energy refund +
    ///    silent abort on -1/-2 (23183-23188), poison hard-write AFTER a
    ///    successful entry (23404-23407), victim recompute (23774) +
    ///    persist once entered.
    fn monster_cast_at_player(
        &mut self,
        id: MonsterInstanceId,
        template: crate::content::MonsterId,
        location: RoomId,
        form: &crate::content::AttackForm,
        victim: SessionId,
    ) -> bool {
        use crate::content::{MatchType, SaveClass};
        let Ok(raw_id) = u16::try_from(form.accuracy) else {
            return false;
        };
        let Some(spell) = self.content.spells.get(&SpellId(raw_id)).cloned() else {
            return false; // unknown id: skip (the DLL would return 0)
        };
        if !matches!(
            spell.match_type,
            MatchType::Single0 | MatchType::Single2 | MatchType::Item6 | MatchType::Special8
        ) {
            // The match-gate ELSE (23777-23779): every match ∉ {0,2,6,8}
            // routes to the area sibling — 101 shipped forms (match 1 x1
            // hooded man `blacknight`, match 11 x1 wererat `plague`,
            // match 12 x99: dragonfire, hellstorm, chaos storm, the
            // dragonfish steam §8.14 measured, ...). The sweep ignores
            // the engaged victim entirely — it re-collects the room.
            return self.monster_cast_area(id, template, location, form, &spell);
        }

        let (victim_name, mr, max_hp) = match self.sessions.get(&victim) {
            Some(Session::InGame { player, derived, .. }) => {
                (player.name.clone(), derived.magic_resist, derived.max_hp)
            }
            _ => return false,
        };
        // One bag read covers every target-side term the DLL fetches
        // piecemeal: SpellImmu 139 (23028), AntiMagic 51 (23045/23309),
        // ImmuPoison 21 (23388), the elemental resist (23093-23117 — the
        // case table maps +0xd0 exactly onto Element::resist_ability).
        let bag = self.ability_bag(self.player(victim));
        let immu = bag.value(Ability::SpellImmu);
        let anti_magic = bag.value(Ability::AntiMagic) != 0;
        let immune_poison = bag.value(Ability::ImmuPoison) != 0;
        // The elemental-resist scale is OFFENSIVE-mode only (23091-23117:
        // the case table sits under `puVar9[0x62] < 3`; a mode-3 single
        // like mummy's `breathes` lands unscaled).
        let resist_pct = if spell.target_mode.is_offensive() {
            spell.element.resist_ability().map_or(0, |a| bag.value(a))
        } else {
            0
        };
        let monster_name = self.monster_name(id);

        let cost = i32::from(form.energy);
        {
            let Some(mi) = self.monsters.get_mut(&id) else {
                return false;
            };
            if mi.energy < cost {
                return false; // silent, next swing (23041 / 23773-23775)
            }
        }

        let rng = &mut self.rng;
        let passed =
            monster_cast_chance_passes(form.min_damage, &mut |lo, hi| rng.roll(lo, hi));

        // SpellImmu (139) auto-resist — evaluated BEFORE and independent
        // of the chance roll (23026-23029; bVar4 never gates it).
        let mut resisted = immu > 0 && i32::from(spell.required_power) < immu;
        if !resisted && passed {
            let save_allowed = match spell.save_class {
                SaveClass::None => false,
                SaveClass::Always => true,
                SaveClass::IfAntiMagic => anti_magic,
            };
            if save_allowed {
                let rng = &mut self.rng;
                resisted = player_save_resists(mr, &mut |lo, hi| rng.roll(lo, hi));
            }
        }

        if resisted || !passed {
            // Half the energy cost, floored at 1 when nonzero (23075-23087
            // resist twin 23758-23770).
            if cost != 0
                && let Some(mi) = self.monsters.get_mut(&id)
            {
                mi.energy -= (cost / 2).max(1);
            }
            let (v, r) = if resisted {
                (
                    text::you_resisted_monster_cast(&monster_name, &spell.name),
                    text::resisted_monster_cast_room(&victim_name, &monster_name, &spell.name),
                )
            } else {
                (
                    text::monster_cast_fizzle(&monster_name, &spell.name),
                    text::monster_cast_fizzle_room(&monster_name, &spell.name, &victim_name),
                )
            };
            self.output_line(victim, &v);
            self.broadcast_to_room(location, Some(victim), &r);
            return false;
        }

        // Landed: full energy cost (23123).
        if let Some(mi) = self.monsters.get_mut(&id) {
            mi.energy -= cost;
        }

        // Magnitude (23124-23143): raw cast level, no level_cap clamp.
        let l = i32::from(form.max_damage);
        let hi = i32::from(spell.max_base) + spell.max_increase.scaled(l);
        let lo = (i32::from(spell.min_base) + spell.min_increase.scaled(l)).min(hi);
        let rolled = self.rng.roll(0, hi - lo + 1) + lo;
        let magnitude = (100 - resist_pct) * rolled / 100;
        // Duration (23144-23148): divide-first scaling, fixed (no band).
        let duration =
            i32::from(spell.duration) + spell.duration_increase.scaled_duration(l);

        // AlterSpDmg(165) from the monster fold — OUR wiring, not the
        // DLL's: monster_cast reads no 0xa5 anywhere (the boost is a
        // player-cast-path exclusive), but zero shipped monsters carry
        // 165, so folding it here is observably identical and closes the
        // slice-4 note symmetrically with the player sites.
        let boost = self.monster_ability_value(id, Ability::AlterSpDmg);
        let mut duration_entered = false;
        for (ability, value) in &spell.abilities {
            // Per-slot fixed value overrides the rolled-and-resisted
            // magnitude (23153-23155).
            let amount = if *value != 0 { i32::from(*value) } else { magnitude };
            if duration != 0 {
                // The duration-arm no-op set: the plain breaks {0,6,23,
                // 26,52} (23158-23161, 23432-23435) PLUS the instant-only
                // cases Drain(8) and Summon(12), whose arms are gated
                // `local_28 == 0` with NO else (23207-23229, 23251-23267)
                // — dead rows in a duration cast. Every OTHER slot tries
                // the 10-slot entry through the once-flag (bVar5);
                // Poison's case is ImmuPoison-gated wholesale
                // (23387-23388).
                if matches!(ability.id(), 6 | 23 | 26 | 52)
                    || matches!(ability, Ability::Drain | Ability::Summon)
                    || (*ability == Ability::Poison && immune_poison)
                {
                    continue;
                }
                // DamageMR (17) carries NO duration gate at all (23305-
                // 23356): it deals its instant MR-scaled damage even
                // mid-duration-cast and never drives the slot entry.
                if *ability == Ability::DamageMR {
                    let shown = alter_sp_dmg(amount, boost);
                    let dealt = damage_mr(shown, mr, anti_magic);
                    if self.monster_cast_damage(victim, dealt, shown, &spell, &monster_name, true) {
                        return true;
                    }
                    continue;
                }
                if !duration_entered {
                    // monster_add_cast_spell_to_user (21777-21816): an
                    // active slot refreshes ONLY when the entering row's
                    // flag allows it AND the new value EXCEEDS the stored
                    // one (21795-21801: `param_7 != 0 && stored < new`,
                    // strict); otherwise -2. No active slot → the first
                    // free one; none → -1. Value AND duration write
                    // together; the success display fires only on a real
                    // write. The stat-write family Intel..Charm (44-49,
                    // cases 0x2c-0x31) passes the '\0' NO-REFRESH flag
                    // (23436-23477 and the 0x2e-0x31 twins): a same-id
                    // recast ALWAYS aborts — live via spell 238 "spits"
                    // (serpentkin), whose rows are Agility/Strength/
                    // Intel. Every other row passes '\x01'. (The DLL also
                    // hard-writes the stat words +0xa2.. immediately +
                    // calculate_secondary_stats; our slot fold applies
                    // the same rows at the refresh_derived below — the
                    // documented mechanism divergence, same outcome.)
                    let refresh = !matches!(ability.id(), 44..=49);
                    let entered = {
                        let Some(Session::InGame { player, .. }) =
                            self.sessions.get_mut(&victim)
                        else {
                            return false;
                        };
                        let slot = ActiveSpell {
                            spell: Some(spell.id),
                            value: amount as i16,
                            remaining: duration,
                        };
                        if let Some(idx) = player.find_active(spell.id) {
                            if refresh && i32::from(player.active_spells[idx].value) < amount {
                                player.active_spells[idx] = slot;
                                true
                            } else {
                                false
                            }
                        } else if let Some(idx) = player.first_free_slot() {
                            player.active_spells[idx] = slot;
                            true
                        } else {
                            false
                        }
                    };
                    if !entered {
                        // Any failure (-1 full, -2 not-greater) refunds
                        // the FULL energy cost and aborts the whole cast
                        // silently (23183-23188: `+0x16 += local_24`,
                        // return 0) — no lines, no poison write.
                        if let Some(mi) = self.monsters.get_mut(&id) {
                            mi.energy += cost;
                        }
                        return false;
                    }
                    duration_entered = true;
                    self.monster_cast_display(&spell, &monster_name, victim, location, amount);
                }
                // Poison (19) hard-writes set-if-greater AFTER the entry
                // attempt (23404-23407) — a rejected entry never reaches
                // it, unlike the player-cast path's write-before-entry.
                if *ability == Ability::Poison
                    && let Some(Session::InGame { player, .. }) =
                        self.sessions.get_mut(&victim)
                {
                    let v = clamp_poison(amount);
                    if player.poison < v {
                        player.poison = v;
                    }
                }
                continue;
            }
            match ability {
                // Damage (1): HP -= v (23164-23179), AlterSpDmg-boosted
                // (our fold wiring — see `boost` above).
                Ability::Damage => {
                    let v = alter_sp_dmg(amount, boost);
                    if self.monster_cast_damage(victim, v, v, &spell, &monster_name, true) {
                        return true;
                    }
                }
                // Drain (8): the victim loses v, the monster gains it
                // capped at the template max (23207-23228) — heal first,
                // then display, then the kill check.
                Ability::Drain => {
                    let cap = self
                        .content
                        .monsters
                        .get(&template)
                        .map_or(0, |t| t.hitpoints);
                    if let Some(mi) = self.monsters.get_mut(&id) {
                        mi.current_hp = (mi.current_hp + amount).min(cap);
                    }
                    if self.monster_cast_damage(victim, amount, amount, &spell, &monster_name, true)
                    {
                        return true;
                    }
                }
                // EnergyLevel (11): round pool += v (23231-23238; the DLL
                // add is uncapped like the benign instant's — we keep the
                // same documented cap as that path).
                Ability::EnergyLevel => {
                    if let Some(Session::InGame { energy, .. }) =
                        self.sessions.get_mut(&victim)
                    {
                        *energy = (*energy + amount).min(PLAYER_ENERGY_MAX);
                    }
                    self.monster_cast_display(&spell, &monster_name, victim, location, amount);
                }
                // Alterhunger (15) / AlterThirst (16): counter adds with
                // NO success display (23269-23303 carry none).
                Ability::Alterhunger => {
                    if let Some(Session::InGame { player, .. }) =
                        self.sessions.get_mut(&victim)
                    {
                        player.hunger = clamp_counter(i32::from(player.hunger) + amount);
                    }
                }
                Ability::AlterThirst => {
                    if let Some(Session::InGame { player, .. }) =
                        self.sessions.get_mut(&victim)
                    {
                        player.thirst = clamp_counter(i32::from(player.thirst) + amount);
                    }
                }
                // Damage(-MR) (17): the boosted amount scaled by the
                // victim's MR — the same +0xc2 word the save halves
                // (23305-23341 is byte-for-byte the damage_mr ladder);
                // the display arg stays the PRE-scale amount (23343
                // passes local_8, not local_5c).
                Ability::DamageMR => {
                    let shown = alter_sp_dmg(amount, boost);
                    let dealt = damage_mr(shown, mr, anti_magic);
                    if self.monster_cast_damage(victim, dealt, shown, &spell, &monster_name, true) {
                        return true;
                    }
                }
                // Heal (18): capped at max HP; the display shows the
                // CAPPED amount (23358-23372).
                Ability::Heal => {
                    let healed = {
                        let Some(Session::InGame { player, .. }) =
                            self.sessions.get_mut(&victim)
                        else {
                            continue;
                        };
                        let healed = if max_hp < player.current_hp + amount {
                            max_hp - player.current_hp
                        } else {
                            amount
                        };
                        player.current_hp += healed;
                        healed
                    };
                    self.monster_cast_display(&spell, &monster_name, victim, location, healed);
                }
                // Poison (19): ImmuPoison gates the WHOLE case (23387-
                // 23388); the counter is SET-IF-GREATER (23390-23392).
                Ability::Poison => {
                    if immune_poison {
                        continue;
                    }
                    if let Some(Session::InGame { player, .. }) =
                        self.sessions.get_mut(&victim)
                    {
                        let v = clamp_poison(amount);
                        if player.poison < v {
                            player.poison = v;
                        }
                    }
                    self.monster_cast_display(&spell, &monster_name, victim, location, amount);
                }
                // Cure Poison (20): counter subtract (23411-23418; floored
                // at 0 like every player-side write — the DLL's raw
                // subtract here relies on FUN_0043fca7, unresolved).
                Ability::CurePoison => {
                    if let Some(Session::InGame { player, .. }) =
                        self.sessions.get_mut(&victim)
                    {
                        player.poison = clamp_poison(i32::from(player.poison) - amount);
                    }
                    self.monster_cast_display(&spell, &monster_name, victim, location, amount);
                }
                // Summon (12), case 0xc (23251-23267): ONE room-wide
                // line first — monster_display_spell_success(-1, ...,
                // "everyone", v): usernum -1 skips the victim line and
                // tell_room excludes nobody, so the whole room (victim
                // included) sees the castmsgb room line with "everyone"
                // in the target slot — then generate_monster with the
                // row value as the template id. The victim-name tag on
                // the spawn (23263) is M6 (see summon_spawn).
                Ability::Summon => {
                    self.monster_room_wide_cast_line(
                        &spell,
                        &monster_name,
                        location,
                        "everyone",
                        amount,
                    );
                    self.summon_spawn(amount, location, Some(victim));
                }
                // Every remaining case is duration-armed only (the
                // `local_28 != 0` guards) or a no-op break in the DLL.
                _ => {}
            }
        }
        if duration_entered {
            // The occupied slot feeds the victim's ability bag: recompute
            // the cached derived stats (the DLL's per-cast
            // calculate_secondary_stats at 23774) and persist the slot.
            self.refresh_derived(victim);
            let snapshot = Box::new(self.player(victim).clone());
            self.events.push(Event::Persist(snapshot));
        }
        false
    }

    /// One landed damage slot: HP subtract, success display, the kill
    /// check and the crossing "drops to the ground" announce — in the
    /// DLL's order (23164-23179: subtract, display, check_kill_user →
    /// return 2, then FUN_0043c91d only when the victim survived and
    /// crossed below 1). Returns `true` when the victim died. `dealt` is
    /// what leaves the HP pool; `shown` what the success lines print
    /// (they differ for DamageMR). `display` is the area path's once-latch
    /// (local_25): a suppressed row still damages and kill-checks but
    /// prints no success pair — the single path always passes `true`.
    fn monster_cast_damage(
        &mut self,
        victim: SessionId,
        dealt: i32,
        shown: i32,
        spell: &crate::content::Spell,
        monster_name: &str,
        display: bool,
    ) -> bool {
        let (was_up, now_hp, victim_name, room) = {
            let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&victim) else {
                return false;
            };
            let was_up = player.current_hp >= 1;
            player.current_hp -= dealt;
            (was_up, player.current_hp, player.name.clone(), player.location)
        };
        if display {
            self.monster_cast_display(spell, monster_name, victim, room, shown);
        }
        if now_hp <= DEATH_FLOOR {
            self.player_killed(victim);
            return true;
        }
        if was_up && now_hp < 1 {
            self.output_line(victim, &text::drops_to_ground(&victim_name));
            self.broadcast_to_room(room, Some(victim), &text::drops_to_ground(&victim_name));
        }
        false
    }

    /// `monster_display_spell_success` (decompile 21663-21772; the hit
    /// fan-out MEASURED §8.14): the victim gets castmsgb LINE 2 — the
    /// target-audience line — rendered with the monster's name as the
    /// caster arg, the room gets LINE 3; there is no caster line (the
    /// caster has no terminal). §8.14 pinned the shape live on a
    /// simultaneous victim/room pair (moaning spirit's `draws the
    /// breath` 82): the victim line carries the damage number, the room
    /// line does not — message data, the inverse of the melee pair
    /// (§8.10 room melee lines DO print damage) — and the grammar varies
    /// per record (bare `Moaning spirit draws ...` vs the dragonfish's
    /// `The fat dragonfish breathes ...`). The odd `msg_style`
    /// branch (21735-21769) binds the same reduced orders as the player
    /// renderer. A spell without a castmsgb record falls back to the
    /// default pair "%s cast %s on you." / "%s cast %s on %s." (21687-
    /// 21689, strings 00481277/0048128a). Every rendered line is
    /// first-letter capitalized (21710/21729). StartMsg(120) prelude and
    /// DescMsg(115) active lines (21692-21716) join with the Task-3
    /// duration entry — no shipped instant monster payload carries either.
    fn monster_cast_display(
        &mut self,
        spell: &crate::content::Spell,
        monster_name: &str,
        victim: SessionId,
        location: RoomId,
        damage: i32,
    ) {
        let victim_name = self.player(victim).name.clone();
        // StartMsg (120) prelude (21692-21700, 21721-21725): victim gets
        // its line2 with the monster name; the room gets line3 with
        // (monster, victim). Printed BEFORE the castmsgb pair, raw (no
        // capitalization pass — the DLL's toupper touches only the
        // sprintf'd castmsgb buffer).
        if let Some(msg_val) = spell
            .abilities
            .iter()
            .find_map(|(a, v)| (*a == Ability::StartMsg).then_some(*v))
            && let Ok(msg_id) = u16::try_from(msg_val)
            && let Some(msg) = self.content.messages.get(&crate::content::MessageId(msg_id))
        {
            let victim_start = msg
                .lines
                .get(1)
                .filter(|l| !l.is_empty())
                .map(|l| text::fill_message(l, &[monster_name]));
            let room_start = msg
                .lines
                .get(2)
                .filter(|l| !l.is_empty())
                .map(|l| text::fill_message(l, &[monster_name, &victim_name]));
            if let Some(line) = victim_start {
                self.output_line(victim, &line);
            }
            if let Some(line) = room_start {
                self.broadcast_to_room(location, Some(victim), &line);
            }
        }
        let (victim_line, room_line) = if let Some(msg) =
            spell.cast_msg_b.and_then(|id| self.content.messages.get(&id))
        {
            let args = text::CastMsgArgs {
                caster: monster_name,
                target: Some(&victim_name),
                spell: &spell.name,
                damage: Some(damage),
            };
            let odd = spell.msg_style & 1 == 1;
            (
                text::render_cast_line(msg, text::CastAudience::Target, &args, odd),
                text::render_cast_line(msg, text::CastAudience::Room, &args, odd),
            )
        } else {
            (
                Some(text::monster_cast_default(monster_name, &spell.name)),
                Some(text::monster_cast_default_room(
                    monster_name,
                    &spell.name,
                    &victim_name,
                )),
            )
        };
        if let Some(line) = victim_line {
            self.output_line(victim, &text::capitalize_first(line));
        }
        if let Some(line) = room_line {
            self.broadcast_to_room(location, Some(victim), &text::capitalize_first(line));
        }
        // DescMsg (115) active line3 to the VICTIM only (21713-21715) —
        // the "You feel ill." family, after the castmsgb pair, raw. Same
        // line3 convention as the player-cast emit_cast_success_lines.
        if let Some(msg_val) = spell
            .abilities
            .iter()
            .find_map(|(a, v)| (*a == Ability::DescMsg).then_some(*v))
            && let Ok(msg_id) = u16::try_from(msg_val)
            && let Some(msg) = self.content.messages.get(&crate::content::MessageId(msg_id))
            && let Some(line) = msg.lines.get(2).filter(|l| !l.is_empty())
        {
            let line = line.clone();
            self.output_line(victim, &line);
        }
    }

    /// The room-wide-only success line (`monster_display_spell_success`
    /// with usernum -1, 21738-21745 skip the victim block and tell_room
    /// excludes nobody): ONE castmsgb room line with `target` in the
    /// target slot, seen by everyone present. The Summon arm passes the
    /// literal "everyone" (23255-23258 / area 22511-22514); the area
    /// self-Heal and self-CurePoison arms pass the monster's OWN name
    /// (`param_1 + 0x8e`, 22659-22662 / 22770-22772).
    fn monster_room_wide_cast_line(
        &mut self,
        spell: &crate::content::Spell,
        monster_name: &str,
        location: RoomId,
        target: &str,
        amount: i32,
    ) {
        let line = if let Some(msg) =
            spell.cast_msg_b.and_then(|mid| self.content.messages.get(&mid))
        {
            let args = text::CastMsgArgs {
                caster: monster_name,
                target: Some(target),
                spell: &spell.name,
                damage: Some(amount),
            };
            text::render_cast_line(
                msg,
                text::CastAudience::Room,
                &args,
                spell.msg_style & 1 == 1,
            )
        } else {
            Some(text::monster_cast_default_room(monster_name, &spell.name, target))
        };
        if let Some(line) = line {
            self.broadcast_to_room(location, None, &text::capitalize_first(line));
        }
    }

    /// `monster_cast_area` (decompile 22037-22943) — the room-wide
    /// sibling every kind-2 form with match ∉ {0,2,6,8} routes into
    /// (23777-23779). Returns `true` when any victim died (the DLL's
    /// return 2). Shape, in DLL order:
    ///
    /// 1. **Entry roll** (22073): the fizzle roll is drawn before
    ///    anything else. **Energy gate** (22100): an unaffordable form is
    ///    a silent skip to the next swing, like the single path.
    /// 2. **Target collection + per-target save**
    ///    (`monster_count_valid_targets` 21827-21886, called 22102):
    ///    PLAYERS ONLY — the sweep walks the terminal list for players in
    ///    the monster's room (the count function's monster out-param is
    ///    never incremented; monsters are NEVER area victims, so no
    ///    monster-vs-monster arm exists to mirror). Each player rolls
    ///    [`monster_area_target_saves`] THERE: a saver is silently
    ///    dropped (no resist line exists in the area path), and ZERO
    ///    survivors abort the whole cast unpaid (22103-22105).
    /// 3. **Fizzle** (22106-22126): half the cost floored at 1, NO lines
    ///    — the single path's "attempted to cast" pair (23749-23757) has
    ///    no area counterpart. **Landing** charges the full cost ONCE per
    ///    cast (22128-22131), never per victim.
    /// 4. **Magnitude** (22133-22150): ONE roll for the whole cast —
    ///    bounds scaled by the form's cast level through the AlterSpDmg-
    ///    style min/max increase pairs, exactly the single path's
    ///    formula. The divide switch (22152-22171) splits the roll by the
    ///    survivor count for match 3/5/9/10 (+ the unreachable defaults)
    ///    and leaves 11/12/13 UNDIVIDED. Duration (22172-22178) is base +
    ///    divide-first scaling; the room entries below nonetheless store
    ///    the RAW duration word.
    /// 5. **Dispel pre-pass** (22180-22233): RemovesSpell(122) /
    ///    KillSpell(153) rows run before the effect loop — every
    ///    survivor's slots are scanned for the named spell; a removal
    ///    displays the cast's success pair once per row (first removal
    ///    only), terminates the slot, and clears the victim's 0x10 target
    ///    flag (22225): a dispelled victim is OUT of the rest of the
    ///    cast (solid fog 256/282 are the shipped carriers).
    /// 6. **Effect loop** (22236-22906) with the once-display latch
    ///    `local_25`: the first landing row displays per victim; later
    ///    rows apply silently. Per-victim scale: offensive-mode casts
    ///    reduce by THAT victim's elemental resist (the repeated local_24
    ///    ladder); Heal/Poison/CurePoison/HealMana skip the scale
    ///    (local_1c direct). Arms:
    ///    - Damage(1): instant only for match 5/10/13 (22212-22218 —
    ///      match 11/12 Damage rows are DEAD: flesh-eating gas 766, hail
    ///      of stones 772, icy breath 895 et al deal nothing, and the
    ///      latch still trips, 22268); duration rows room-enter; the
    ///      {1,2,4,6} self group self-slots on duration.
    ///    - Drain(8): instant only; victim loses, monster gains capped at
    ///      the template max (22357-22434).
    ///    - EnergyLevel(11) / Heal(18) / Poison(19) / CurePoison(20) /
    ///      HealMana(150): per-victim adds with the same clamp semantics
    ///      as the single path (Poison set-if-greater + ImmuPoison gate;
    ///      the duration-armed Poison hard-writes BEFORE the room entry,
    ///      22702-22748); Heal/CurePoison also carry {1,2,(4),6} SELF
    ///      arms on the monster (22639-22711 / 22758-22806).
    ///    - Summon(12): match {1,2,6} only — the "everyone" line + spawn,
    ///      and it never trips the latch (22505-22525).
    ///    - DamageMR(17): NO duration gate (22527-22637); the display
    ///      shows the POST-MR dealt amount (uVar11 at 22624-22628) — the
    ///      single path shows the PRE-scale amount (23343), a genuine
    ///      asymmetry (§8.14 dragonfish lines are post-scale).
    ///    - Everything else: duration-only via the default arm (22819-
    ///      22843) or a no-op ([`area_ability_case_is_noop`]) — note the
    ///      stat family 44-49 and Alterhunger/AlterThirst DO NOTHING
    ///      here, unlike the single path.
    /// 7. **Room duration entry** (`monster_add_duration_spell_to_room`
    ///    21892-21965, guarded by the latch at every arm, e.g.
    ///    22346-22352): fires at most once per cast — each survivor gets
    ///    one slot holding the per-victim scaled ROLLED value (row
    ///    overrides never apply) with refresh-only-if-greater; a failed
    ///    entry BEFORE any success aborts the remaining cast with NO
    ///    energy refund (the single path refunds, 23183-23188); failures
    ///    after a success are tolerated.
    /// 8. The {1,2,4,6} self group writes the MONSTER's own 5-slot table
    ///    (FUN_004262d6 21970-22005) — blacknight 1220 (match 1) is the
    ///    shipped carrier. The self-entry's value/duration arguments are
    ///    Ghidra-invisible (stack artifact); we pass the row-or-rolled
    ///    amount + the scaled duration — ORACLE-OPEN.
    ///
    /// §8.14 reconciliation: the dragonfish's `breathes burning steam`
    /// (359, match 12, DamageMR 10-30) went through THIS path live and
    /// produced a single-victim-looking line — that is the room sweep
    /// with exactly one occupant, per-victim display, undivided match-12
    /// magnitude, post-MR amount. AlterSpDmg folding at the damage arms
    /// is our documented wiring, same as the single path (zero shipped
    /// monsters carry 165). The end-of-cast monster_update_room_users_
    /// stats (22934) is covered by the per-entry refresh_derived calls.
    fn monster_cast_area(
        &mut self,
        id: MonsterInstanceId,
        template: crate::content::MonsterId,
        location: RoomId,
        form: &crate::content::AttackForm,
        spell: &crate::content::Spell,
    ) -> bool {
        use crate::content::MatchType;
        // 1. Entry roll (22073) + energy gate (22100).
        let chance_roll = self.rng.roll(0, 100);
        let cost = i32::from(form.energy);
        match self.monsters.get(&id) {
            Some(mi) if mi.energy >= cost => {}
            _ => return false, // silent skip, next swing
        }
        // 2. monster_count_valid_targets (22102): collect + save.
        let sids: Vec<SessionId> = self.sessions.keys().copied().collect();
        let mut victims: Vec<AreaTarget> = Vec::new();
        for sid in sids {
            let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&sid)
            else {
                continue;
            };
            if player.location != location {
                continue;
            }
            let bag = self.ability_bag(player);
            let mr = derived.magic_resist;
            let max_hp = derived.max_hp;
            let max_mana = derived.max_mana;
            let anti_magic = bag.value(Ability::AntiMagic) != 0;
            let immune_poison = bag.value(Ability::ImmuPoison) != 0;
            let resist_pct = if spell.target_mode.is_offensive() {
                spell.element.resist_ability().map_or(0, |a| bag.value(a))
            } else {
                0
            };
            let rng = &mut self.rng;
            if monster_area_target_saves(spell.save_class, anti_magic, mr, &mut |lo, hi| {
                rng.roll(lo, hi)
            }) {
                continue; // silently excluded — no resist line
            }
            victims.push(AreaTarget {
                session: sid,
                mr,
                anti_magic,
                immune_poison,
                resist_pct,
                max_hp,
                max_mana,
            });
        }
        if victims.is_empty() {
            return false; // 0 survivors: nothing charged (22103-22105)
        }
        // 3. Fizzle (strict roll < percent, like the single path).
        if chance_roll >= i32::from(form.min_damage) {
            if cost != 0
                && let Some(mi) = self.monsters.get_mut(&id)
            {
                mi.energy -= (cost / 2).max(1);
            }
            return false; // SILENT (22106-22126)
        }
        if let Some(mi) = self.monsters.get_mut(&id) {
            mi.energy -= cost; // full cost, once per cast (22128-22131)
        }
        let monster_name = self.monster_name(id);
        // 4. One magnitude roll for the whole cast (22133-22150).
        let l = i32::from(form.max_damage);
        let hi = i32::from(spell.max_base) + spell.max_increase.scaled(l);
        let lo = (i32::from(spell.min_base) + spell.min_increase.scaled(l)).min(hi);
        let total = self.rng.roll(0, hi - lo + 1) + lo;
        let per_base = if matches!(
            spell.match_type,
            MatchType::AreaB | MatchType::AreaC | MatchType::AreaD
        ) {
            total
        } else {
            total / victims.len() as i32 // the divide switch (22152-22171)
        };
        let dur_total = i32::from(spell.duration) + spell.duration_increase.scaled_duration(l);
        let raw_dur = i32::from(spell.duration);
        let boost = self.monster_ability_value(id, Ability::AlterSpDmg);
        let offensive = spell.target_mode.is_offensive();
        let area = spell.match_type.room_wide();
        let self_group = matches!(
            spell.match_type,
            MatchType::Single1 | MatchType::Single2 | MatchType::Special4 | MatchType::Item6
        );
        let mut any_died = false;
        // 5. Dispel pre-pass (22180-22233), area matches only.
        if area {
            for (ability, value) in &spell.abilities {
                let honor = match ability {
                    Ability::RemovesSpell => true,
                    Ability::KillSpell => false,
                    _ => continue,
                };
                let Ok(rid) = u16::try_from(*value) else {
                    continue;
                };
                if rid == 0 {
                    continue;
                }
                let mut row_displayed = false;
                let mut kept = Vec::with_capacity(victims.len());
                for t in std::mem::take(&mut victims) {
                    if !self.area_victim_present(t.session, location) {
                        kept.push(t);
                        continue;
                    }
                    let mut dispelled = false;
                    while let Some(idx) = self.player(t.session).find_active(SpellId(rid)) {
                        // One display per ROW — the first removal across
                        // all victims (the inner local_25, 22203-22212).
                        if !row_displayed {
                            self.monster_cast_display(
                                spell,
                                &monster_name,
                                t.session,
                                location,
                                total,
                            );
                            row_displayed = true;
                        }
                        self.terminate_active_spell(t.session, idx, honor);
                        dispelled = true;
                    }
                    if !dispelled {
                        kept.push(t); // flag kept (22225 clears it on removal)
                    }
                }
                victims = kept;
            }
        }
        // 6. The effect loop with the once-display latch (local_25).
        let mut displayed = false;
        for (ability, value) in &spell.abilities {
            // Row overrides feed only the SELF arms and Summon (local_18
            // at 22239-22242); the per-victim arms use the divided roll.
            let amount_self = if *value != 0 { i32::from(*value) } else { total };
            match ability {
                Ability::Damage => {
                    if self_group {
                        if dur_total != 0 {
                            self.monster_self_slot_entry(id, spell.id, amount_self, dur_total);
                        }
                    } else if area {
                        if raw_dur == 0 {
                            // Match 5/10/13 only (22212-22218): 11/12
                            // Damage rows iterate nobody — DEAD.
                            if matches!(
                                spell.match_type,
                                MatchType::Area5 | MatchType::Area10 | MatchType::AreaD
                            ) {
                                for t in &victims {
                                    if !self.area_victim_present(t.session, location) {
                                        continue;
                                    }
                                    let v = alter_sp_dmg(
                                        area_scaled(per_base, t.resist_pct, offensive),
                                        boost,
                                    );
                                    any_died |= self.monster_cast_damage(
                                        t.session,
                                        v,
                                        v,
                                        spell,
                                        &monster_name,
                                        !displayed,
                                    );
                                }
                            }
                            displayed = true; // trips even when dead (22268)
                        } else {
                            if !displayed
                                && !self.monster_area_room_entry(
                                    &victims,
                                    spell,
                                    per_base,
                                    &monster_name,
                                    location,
                                )
                            {
                                return any_died; // abort, energy kept (22347-22352)
                            }
                            displayed = true;
                        }
                    }
                }
                Ability::Drain => {
                    // Instant-only (no duration else-arm, 22357-22434).
                    if area && raw_dur == 0 {
                        let cap = self
                            .content
                            .monsters
                            .get(&template)
                            .map_or(0, |t| t.hitpoints);
                        for t in &victims {
                            if !self.area_victim_present(t.session, location) {
                                continue;
                            }
                            let v = area_scaled(per_base, t.resist_pct, offensive);
                            if let Some(mi) = self.monsters.get_mut(&id) {
                                mi.current_hp = (mi.current_hp + v).min(cap);
                            }
                            any_died |= self.monster_cast_damage(
                                t.session,
                                v,
                                v,
                                spell,
                                &monster_name,
                                !displayed,
                            );
                        }
                        displayed = true;
                    }
                }
                Ability::EnergyLevel => {
                    if area {
                        if raw_dur == 0 {
                            for t in &victims {
                                if !self.area_victim_present(t.session, location) {
                                    continue;
                                }
                                let v = area_scaled(per_base, t.resist_pct, offensive);
                                if let Some(Session::InGame { energy, .. }) =
                                    self.sessions.get_mut(&t.session)
                                {
                                    *energy = (*energy + v).min(PLAYER_ENERGY_MAX);
                                }
                                if !displayed {
                                    self.monster_cast_display(
                                        spell,
                                        &monster_name,
                                        t.session,
                                        location,
                                        v,
                                    );
                                }
                            }
                            displayed = true;
                        } else {
                            if !displayed
                                && !self.monster_area_room_entry(
                                    &victims,
                                    spell,
                                    per_base,
                                    &monster_name,
                                    location,
                                )
                            {
                                return any_died;
                            }
                            displayed = true;
                        }
                    }
                }
                Ability::Summon => {
                    // Match {1,2,6} only; never trips the latch (22505-
                    // 22525).
                    if matches!(
                        spell.match_type,
                        MatchType::Single1 | MatchType::Single2 | MatchType::Item6
                    ) && raw_dur == 0
                    {
                        if !displayed {
                            self.monster_room_wide_cast_line(
                                spell,
                                &monster_name,
                                location,
                                "everyone",
                                amount_self,
                            );
                        }
                        // PLAUSIBLE: the area self-slot summon's lock was
                        // not extracted; spawn idle.
                        self.summon_spawn(amount_self, location, None);
                    }
                }
                Ability::DamageMR => {
                    if self_group {
                        if dur_total != 0 {
                            self.monster_self_slot_entry(id, spell.id, amount_self, dur_total);
                        }
                    } else if area {
                        // NO duration gate (22527-22637); display = the
                        // POST-MR dealt amount (22624-22628).
                        for t in &victims {
                            if !self.area_victim_present(t.session, location) {
                                continue;
                            }
                            let shown = alter_sp_dmg(
                                area_scaled(per_base, t.resist_pct, offensive),
                                boost,
                            );
                            let dealt = damage_mr(shown, t.mr, t.anti_magic);
                            any_died |= self.monster_cast_damage(
                                t.session,
                                dealt,
                                dealt,
                                spell,
                                &monster_name,
                                !displayed,
                            );
                        }
                        displayed = true;
                    }
                }
                Ability::Heal => {
                    if self_group {
                        if dur_total == 0 {
                            // Self-heal capped at the template max, ONE
                            // room-wide line naming the monster
                            // (22655-22668).
                            let cap = self
                                .content
                                .monsters
                                .get(&template)
                                .map_or(0, |t| t.hitpoints);
                            let healed = {
                                let Some(mi) = self.monsters.get_mut(&id) else {
                                    continue;
                                };
                                let healed = amount_self.min(cap - mi.current_hp);
                                mi.current_hp += healed;
                                healed
                            };
                            if !displayed {
                                let name = monster_name.clone();
                                self.monster_room_wide_cast_line(
                                    spell,
                                    &monster_name,
                                    location,
                                    &name,
                                    healed,
                                );
                            }
                            displayed = true;
                        } else {
                            self.monster_self_slot_entry(id, spell.id, amount_self, dur_total);
                        }
                    } else if area {
                        if raw_dur == 0 {
                            // Unscaled (local_1c direct — no elemental
                            // reduction on heals, 22671-22694).
                            for t in &victims {
                                if !self.area_victim_present(t.session, location) {
                                    continue;
                                }
                                let healed = {
                                    let Some(Session::InGame { player, .. }) =
                                        self.sessions.get_mut(&t.session)
                                    else {
                                        continue;
                                    };
                                    let healed = per_base.min(t.max_hp - player.current_hp);
                                    player.current_hp += healed;
                                    healed
                                };
                                if !displayed {
                                    self.monster_cast_display(
                                        spell,
                                        &monster_name,
                                        t.session,
                                        location,
                                        healed,
                                    );
                                }
                            }
                            displayed = true;
                        } else {
                            if !displayed
                                && !self.monster_area_room_entry(
                                    &victims,
                                    spell,
                                    per_base,
                                    &monster_name,
                                    location,
                                )
                            {
                                return any_died;
                            }
                            displayed = true;
                        }
                    }
                }
                Ability::Poison => {
                    if self_group {
                        if dur_total != 0 {
                            self.monster_self_slot_entry(id, spell.id, amount_self, dur_total);
                        }
                    } else if area {
                        // ImmuPoison gates per victim; the counter is
                        // set-if-greater on the UNSCALED roll, and the
                        // duration arm hard-writes BEFORE the room entry
                        // (22702-22757).
                        for t in &victims {
                            if !self.area_victim_present(t.session, location)
                                || t.immune_poison
                            {
                                continue;
                            }
                            if let Some(Session::InGame { player, .. }) =
                                self.sessions.get_mut(&t.session)
                            {
                                let v = clamp_poison(per_base);
                                if player.poison < v {
                                    player.poison = v;
                                }
                            }
                            if raw_dur == 0 && !displayed {
                                self.monster_cast_display(
                                    spell,
                                    &monster_name,
                                    t.session,
                                    location,
                                    per_base,
                                );
                            }
                        }
                        if raw_dur != 0
                            && !displayed
                            && !self.monster_area_room_entry(
                                &victims,
                                spell,
                                per_base,
                                &monster_name,
                                location,
                            )
                        {
                            return any_died;
                        }
                        displayed = true;
                    }
                }
                Ability::CurePoison => {
                    if matches!(
                        spell.match_type,
                        MatchType::Single1 | MatchType::Single2 | MatchType::Item6
                    ) {
                        // Self-cure {1,2,6}, no duration gate; the room
                        // line shows local_1c (22758-22775).
                        if let Some(mi) = self.monsters.get_mut(&id) {
                            mi.poison = (i32::from(mi.poison) - amount_self).max(0) as i16;
                        }
                        if !displayed {
                            let name = monster_name.clone();
                            self.monster_room_wide_cast_line(
                                spell,
                                &monster_name,
                                location,
                                &name,
                                per_base,
                            );
                        }
                        displayed = true;
                    } else if area {
                        if raw_dur == 0 {
                            for t in &victims {
                                if !self.area_victim_present(t.session, location)
                                    || t.immune_poison
                                {
                                    continue;
                                }
                                if let Some(Session::InGame { player, .. }) =
                                    self.sessions.get_mut(&t.session)
                                {
                                    player.poison =
                                        clamp_poison(i32::from(player.poison) - per_base);
                                }
                                if !displayed {
                                    self.monster_cast_display(
                                        spell,
                                        &monster_name,
                                        t.session,
                                        location,
                                        per_base,
                                    );
                                }
                            }
                            displayed = true;
                        } else {
                            if !displayed
                                && !self.monster_area_room_entry(
                                    &victims,
                                    spell,
                                    per_base,
                                    &monster_name,
                                    location,
                                )
                            {
                                return any_died;
                            }
                            displayed = true;
                        }
                    }
                }
                Ability::HealMana => {
                    // Case 0x96 (22855-22906): area only, unscaled,
                    // clamped into [0, max mana].
                    if area {
                        if raw_dur == 0 {
                            for t in &victims {
                                if !self.area_victim_present(t.session, location) {
                                    continue;
                                }
                                let delta = {
                                    let Some(Session::InGame { player, .. }) =
                                        self.sessions.get_mut(&t.session)
                                    else {
                                        continue;
                                    };
                                    let cur = player.current_mana;
                                    let delta = if per_base < 1 {
                                        if cur + per_base < 0 { -cur } else { per_base }
                                    } else if t.max_mana < cur + per_base {
                                        t.max_mana - cur
                                    } else {
                                        per_base
                                    };
                                    player.current_mana += delta;
                                    delta
                                };
                                if !displayed {
                                    self.monster_cast_display(
                                        spell,
                                        &monster_name,
                                        t.session,
                                        location,
                                        delta,
                                    );
                                }
                            }
                            displayed = true;
                        } else {
                            if !displayed
                                && !self.monster_area_room_entry(
                                    &victims,
                                    spell,
                                    per_base,
                                    &monster_name,
                                    location,
                                )
                            {
                                return any_died;
                            }
                            displayed = true;
                        }
                    }
                }
                _ => {
                    // The duration-only default arm (22819-22843) or a
                    // plain no-op.
                    if area_ability_case_is_noop(*ability) {
                        continue;
                    }
                    if self_group {
                        if dur_total != 0 {
                            self.monster_self_slot_entry(id, spell.id, amount_self, dur_total);
                        }
                    } else if area && raw_dur != 0 {
                        if !displayed
                            && !self.monster_area_room_entry(
                                &victims,
                                spell,
                                per_base,
                                &monster_name,
                                location,
                            )
                        {
                            return any_died;
                        }
                        displayed = true;
                    }
                }
            }
        }
        any_died
    }

    /// `monster_add_duration_spell_to_room` (decompile 21892-21965):
    /// every surviving victim gets ONE slot entry — the per-victim
    /// elemental-scaled ROLLED value (row overrides never reach here),
    /// the RAW spell duration word as the remaining ticks, always with
    /// the refresh-if-greater flag (no stat-family no-refresh here). A
    /// real write displays through `monster_add_cast_spell_to_user`'s own
    /// success call and recomputes; a failed entry before ANY success
    /// returns `false` — the caller aborts the cast WITHOUT an energy
    /// refund (21952-21956 → 22347-22352). Failures after a success are
    /// tolerated. No poison hard-write lives here (the area Poison arm
    /// does its own, before this).
    fn monster_area_room_entry(
        &mut self,
        victims: &[AreaTarget],
        spell: &crate::content::Spell,
        base: i32,
        monster_name: &str,
        location: RoomId,
    ) -> bool {
        let offensive = spell.target_mode.is_offensive();
        let raw_dur = i32::from(spell.duration);
        let mut any = false;
        for t in victims {
            if !self.area_victim_present(t.session, location) {
                continue;
            }
            let v = area_scaled(base, t.resist_pct, offensive);
            let entered = {
                let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&t.session)
                else {
                    continue;
                };
                let slot = ActiveSpell {
                    spell: Some(spell.id),
                    value: v as i16,
                    remaining: raw_dur,
                };
                if let Some(idx) = player.find_active(spell.id) {
                    if i32::from(player.active_spells[idx].value) < v {
                        player.active_spells[idx] = slot;
                        true
                    } else {
                        false
                    }
                } else if let Some(idx) = player.first_free_slot() {
                    player.active_spells[idx] = slot;
                    true
                } else {
                    false
                }
            };
            if entered {
                any = true;
                self.monster_cast_display(spell, monster_name, t.session, location, v);
                self.refresh_derived(t.session);
                let snapshot = Box::new(self.player(t.session).clone());
                self.events.push(Event::Persist(snapshot));
            } else if !any {
                return false;
            }
        }
        any
    }

    /// `FUN_004262d6` (decompile 21970-22005) — the monster's OWN 5-slot
    /// duration table (`+0x14a/+0x154/+0x15e`): a same-id slot is
    /// overwritten unconditionally, else the first empty one; a full
    /// table silently loses the entry. No display. The value/duration
    /// arguments are stack artifacts in the decompile — we pass the
    /// row-or-rolled amount and the scaled duration (ORACLE-OPEN;
    /// blacknight 1220 is the only shipped carrier).
    fn monster_self_slot_entry(
        &mut self,
        id: MonsterInstanceId,
        spell: SpellId,
        value: i32,
        remaining: i32,
    ) {
        let Some(mi) = self.monsters.get_mut(&id) else {
            return;
        };
        let slot = ActiveSpell { spell: Some(spell), value: value as i16, remaining };
        if let Some(idx) = mi.active_spells.iter().position(|s| s.spell == Some(spell)) {
            mi.active_spells[idx] = slot;
        } else if let Some(idx) = mi.active_spells.iter().position(|s| s.spell.is_none()) {
            mi.active_spells[idx] = slot;
        }
    }

    /// The per-loop re-check every area arm carries (`get_player` + the
    /// room compare + the 0x10 flag): a victim who died or left between
    /// rows drops out of later applications.
    fn area_victim_present(&self, session: SessionId, location: RoomId) -> bool {
        matches!(
            self.sessions.get(&session),
            Some(Session::InGame { player, .. }) if player.location == location
        )
    }

    /// `check_kill_monster` + `distribute_experience` (`death.md` §4/§5).
    /// `killer: None` is the upkeep/DoT death (medium_update_monster
    /// 19327-19339: check_kill_monster with terminal -1 +
    /// distribute_experience(-1, ...)): the announce goes to the whole
    /// room and the split covers only the engaged sessions — nobody
    /// engaged means the experience evaporates (ORACLE-VERIFY: the -1
    /// split's exact recipients are decompile-inferred).
    fn monster_killed(&mut self, id: MonsterInstanceId, killer: Option<SessionId>) {
        let Some(instance) = self.monsters.remove(&id) else {
            return;
        };
        let tpl = self
            .content
            .monsters
            .get(&instance.template)
            .expect("live instance has a template");
        let name = tpl.name.clone();
        let (tpl_experience, tpl_exp_multi) = (tpl.experience, tpl.exp_multi);
        let template = instance.template;
        // check_kill_monster's spawn-room block (21266-21297): the boss
        // clears its present-flag ONLY; anyone else decrements the live
        // count, stamps the respawn timer, and pays back the linked cap.
        let home = instance.home;
        let now = self.scheduler.now() as i64;
        let is_boss = self
            .content
            .rooms
            .get(&home)
            .is_some_and(|r| r.boss_monster == Some(template));
        if is_boss {
            self.spawn_state(home).boss_present = false;
        } else {
            let st = self.spawn_state(home);
            st.live = st.live.saturating_sub(1);
            st.stamp = Some(now);
            let linked = self.content.rooms.get(&home).and_then(|r| r.linked_room);
            if let Some(linked) = linked {
                let st = self.spawn_state(linked);
                st.linked_live = (st.linked_live - 1).max(0);
            }
        }
        // The CURRENT room is stamped too (21300-21313).
        self.spawn_state(instance.location).stamp = Some(now);
        self.events.push(Event::PersistRoomStamp { room: home });
        if instance.location != home {
            self.events.push(Event::PersistRoomStamp { room: instance.location });
        }
        // Template population (check_kill_monster L21361-21390): only
        // templates with a charged active count (= gamelimit spawns) get
        // the decrement and the kill stamp.
        if let Some(pop) = self.population.get_mut(&template)
            && pop.active != 0
        {
            pop.active -= 1;
            pop.last_kill = Some(now);
            self.events.push(Event::PersistMonsterKill { template });
        }
        // Coins drop into the room piles — the INSTANCE piles rolled at
        // generate time (fixture spawns carry the template maxes;
        // template order is high->low, the room piles low->high). The
        // killer sees the "drop to the ground." lines (21322-21358) in
        // runic-first order.
        let piles = self.room_coins.entry(instance.location).or_insert([0; 5]);
        for (i, amount) in instance.coins.iter().enumerate() {
            piles[4 - i] += amount;
        }
        if let Some(killer) = killer {
            const DENOMS: [&str; 5] = ["runic", "platinum", "gold", "silver", "copper"];
            for (i, amount) in instance.coins.iter().enumerate() {
                if *amount > 0 {
                    self.output_line(killer, &text::coins_drop(*amount, DENOMS[i]));
                }
            }
        }
        // Carried loot drops silently (check_kill_monster: no message; the
        // wielded weapon is not in the drop loop and stays gone).
        self.room_items
            .entry(instance.location)
            .or_default()
            .extend(instance.items.iter().copied());
        let exp = u64::from(tpl_experience.max(0) as u32)
            * u64::from(tpl_exp_multi.max(1) as u32);
        let room = instance.location;

        match killer {
            Some(killer) => {
                self.output_line(killer, &text::monster_dead(&name));
                self.broadcast_to_room(room, Some(killer), &text::monster_dead(&name));
            }
            None => self.broadcast_to_room(room, None, &text::monster_dead(&name)),
        }

        // Equal split among the killer and everyone engaged on this target.
        let mut recipients: Vec<SessionId> = killer.into_iter().collect();
        for (sid, session) in self.sessions.iter() {
            if let Session::InGame { target: Some(t), .. } = session
                && *t == id
                && Some(*sid) != killer
            {
                recipients.push(*sid);
            }
        }
        if recipients.is_empty() {
            return; // a killer-less death with nobody engaged: no split
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
        // Only the KILLER's lock clears (check_kill_user 27194, done at
        // the kill site). Other monsters keep their name-locks and chase
        // or age them out through the fast tier — the DLL has no
        // death-of-player lock sweep (slice-3 extraction §5).
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
    /// player fighter. Weapon skill (M4), dynamic accumulators (M5), and
    /// encumbrance (M4) all feed in below.
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
        //          + (Agl-50)/6  + the dynamic accuracy accumulators
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
    /// The ability fold joins both words (25190-25200): evasion += AC(2)
    /// through `get_monster_ability_value` — template rows AND active-slot
    /// debuffs — and the soak += DR(7) RAW (the *10 scale applies only to
    /// the template word).
    fn build_monster_defender(&self, id: MonsterInstanceId) -> crate::combat::Fighter {
        let tpl = self
            .monsters
            .get(&id)
            .and_then(|m| self.content.monsters.get(&m.template))
            .expect("live instance has a template");
        crate::combat::Fighter {
            accuracy: 0,
            evasion_a: i32::from(tpl.armour_class) + self.monster_ability_value(id, Ability::AC),
            evasion_b: 0,
            armor: i32::from(tpl.damage_resist) * 10
                + self.monster_ability_value(id, Ability::DR),
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
        let trail_seed = player.location;
        self.sessions
            .insert(session, Session::InGame {
                player: Box::new(player),
                derived,
                exiting: None,
                target: None,
                aided: false,
                energy: PLAYER_ENERGY_MAX,
                cast_this_round: false,
                casting: None,
                moved_this_round: false,
                attackers_this_tick: 0,
                trail: vec![trail_seed],
            });
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
            fame: 0,
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
        // give_monsters_a_free_attack (23846-23905), before the move
        // commits: one room roll per departure — drawn even with nothing
        // to hit — then the first eligible monster (roll <= aggression;
        // passive modes and class 0x25 only at their locked runner; mode
        // 6 spares fame >= 0x50) takes a full swing sequence. Only a
        // DEATH aborts the move; the +0x6f0 gate caps it at one free
        // attack per medium tick.
        let roll = self.rng.roll(0, 100);
        if self.attackers_of(session) <= 0 {
            let here: Vec<MonsterInstanceId> = self
                .monsters
                .iter()
                .filter(|(_, m)| m.location == from && m.current_hp > 0)
                .map(|(id, _)| *id)
                .take(15)
                .collect();
            let fame = self.player(session).fame;
            for mid in here {
                let m = &self.monsters[&mid];
                if roll > i32::from(m.aggression) {
                    continue;
                }
                let locked_on_me = m.target == Some(session);
                let swing = if matches!(m.behaviour, 4 | 0 | 3) || m.roam_class == 0x25 {
                    locked_on_me && !m.suppress
                } else if m.behaviour == 6 && fame >= 0x50 {
                    false
                } else {
                    !locked_on_me || !m.suppress
                };
                if swing {
                    let lives_before = self.player(session).lives;
                    self.bump_attackers(session);
                    self.monster_attack(mid, session);
                    let died = !matches!(
                        self.sessions.get(&session),
                        Some(Session::InGame { player, .. }) if player.lives == lives_before
                    );
                    if died {
                        return; // the kill aborts the move (23900-23902)
                    }
                    break; // max one free attack per departure
                }
            }
        }
        let name = self.player(session).name.clone();
        self.broadcast_to_room(from, Some(session), &text::left_via(&name, direction));
        match self.sessions.get_mut(&session) {
            Some(Session::InGame { player, moved_this_round, trail, .. }) => {
                player.location = exit.dest;
                // +0x6f4 bit 6 (12501) + the pursuit breadcrumb push.
                *moved_this_round = true;
                trail.insert(0, exit.dest);
                trail.truncate(20);
            }
            _ => unreachable!("mover is in game"),
        }
        self.broadcast_to_room(
            exit.dest,
            Some(session),
            &text::walks_in_from(&name, direction.opposite()),
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
        match exclude {
            Some(s) => self.broadcast_to_room_except(room, &[s], text),
            None => self.broadcast_to_room_except(room, &[], text),
        }
    }

    /// Room broadcast excluding a SET of sessions — the targeted-cast
    /// fan-outs exclude both the caster and the target (§8.13: the
    /// target's view is its own line or, on a fail, nothing).
    fn broadcast_to_room_except(&mut self, room: RoomId, exclude: &[SessionId], text: &str) {
        let recipients: Vec<SessionId> = self
            .in_game_sessions()
            .filter(|(id, p)| !exclude.contains(id) && p.location == room)
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
