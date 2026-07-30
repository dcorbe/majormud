//! The game core: a single-threaded, deterministic state machine.
//!
//! Sessions attach with a loaded (or freshly created) player, feed text input
//! in, and consume `Event`s out. No I/O happens here — the caller owns
//! networking and persistence.

use std::collections::{BTreeMap, BTreeSet};

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

/// What a room-name lookup for a cast resolved — `find_action_target`'s
/// `*param_4` kind code (decompile 63726): `1` = user, `2` = monster,
/// `8` = carried item. The cast dispatcher (59278-59320) picks the entry
/// point off THIS, never off the spell's target mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CastTarget {
    User(SessionId),
    Monster(MonsterInstanceId),
    Item(crate::content::ItemId),
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
    /// mode-6 bound is 0x50 (decompile 20386/20420/23882). Fed by the
    /// crime system (`crime.md`, landed) — creation seeds 0.
    pub fame: i16,
    /// Per-user ANSI. OURS (documented divergence): the real board keys
    /// this on the MBBS account outside the DLL. Overrides the
    /// `CoreConfig.ansi` global at the output funnel; toggled by the
    /// `ansi` command; creation seeds the server global.
    pub ansi: bool,
    /// `+0x700 & 0x10` — the "Warn on Evil" setting: while set, any
    /// action that would grant evil points is REFUSED (crime.md §2.1).
    /// Creation sets it ON (create_player ~53795); `set evil` toggles.
    pub warn_on_evil: bool,
    /// `+0x5f6` — the HIDDEN byte (theft.md §11.2). Runtime only, never
    /// persisted; cleared by non-sneak movement and combat engagement.
    pub hidden: bool,
    /// `+0x6f4` bit 4 — sneak-armed: the next movement runs as
    /// `sneak()` (theft.md §11.1). Runtime only.
    pub sneak_armed: bool,
    /// The 30-slot innate/quest ability table (quests.md §1.1 — ids at
    /// `+0x73a[30]`, values at `+0x776[30]`). Quest-flag counters
    /// (MageBaneQuest 50, the 125-134 path quests) and the completion
    /// detector's permanent stat grants live here. An array, not a Vec:
    /// slot exhaustion is observable (`addability` on a full table
    /// fail-stops, decompile 69628-69634). Reads sum matching slots.
    pub innate: [(Option<Ability>, i16); 30],
    /// The 64 script flag bits driven by the quest VM's `flag` verb
    /// (`FUN_0046fdda` 68319-68413 — a verb quests.md missed; zero
    /// shipped uses). Bits 1-32 live at `+0x71c`, 33-64 at `+0x460`;
    /// modeled as one u64 with flag `n` at bit `n-1`.
    pub quest_flags: u64,
    /// `+0x6c8` — the gang DISPLAY name (gangs.md §0); empty = not in a
    /// gang. Membership is defined entirely by this string.
    pub gang: String,
    /// `+0x7d4` — the gang rank/notice word (`gang::GF_*` bits). Only the
    /// gang bits are modeled; the DLL's unrelated low-byte settings bits
    /// keep their distance in other fields.
    pub gang_flags: u16,
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

    /// Sum of every innate slot holding `ability`
    /// (`get_user_ability_value` 36850-36869).
    pub fn innate_value(&self, ability: Ability) -> i32 {
        self.innate
            .iter()
            .filter(|(id, _)| *id == Some(ability))
            .map(|(_, v)| i32::from(*v))
            .sum()
    }

    /// `FUN_0046c507` (65893-65928): accumulate `value` into every slot
    /// already holding `ability`, else claim the first empty slot.
    /// Refuses GiveTempSpell (0xa0) outright; returns `false` for it and
    /// for a full table with no matching slot (the `giveability` verb
    /// fail-stops on `false`).
    pub fn give_innate_ability(&mut self, ability: Ability, value: i16) -> bool {
        if ability.id() == 0xa0 {
            return false;
        }
        let mut found = false;
        for (id, v) in &mut self.innate {
            if *id == Some(ability) {
                found = true;
                *v += value;
            }
        }
        if !found {
            for (id, v) in &mut self.innate {
                if id.is_none() {
                    *id = Some(ability);
                    *v = value;
                    return true;
                }
            }
        }
        found
    }

    /// The `addability` arm (69590-69637): raise every matching slot
    /// below `value` to it (at-least semantics — the raise is skipped
    /// for 0xa0), else claim the first empty slot. Returns `false` when
    /// the table is full with no matching slot (the verb fail-stops).
    /// The 0xa0 spellbook grant on slot creation is the verb layer's job.
    pub fn raise_innate_ability(&mut self, ability: Ability, value: i16) -> bool {
        let mut found = false;
        for (id, v) in &mut self.innate {
            if *id == Some(ability) {
                found = true;
                if ability.id() != 0xa0 && *v < value {
                    *v = value;
                }
            }
        }
        if !found {
            for (id, v) in &mut self.innate {
                if id.is_none() {
                    *id = Some(ability);
                    *v = value;
                    return true;
                }
            }
        }
        found
    }

    /// The `removeability` arm (69523-69558): zero every slot holding
    /// `ability`. Returns `false` when no slot held it (the verb
    /// fail-stops). The 0xa0 spellbook purge is the verb layer's job.
    pub fn remove_innate_ability(&mut self, ability: Ability) -> bool {
        let mut found = false;
        for slot in &mut self.innate {
            if slot.0 == Some(ability) {
                found = true;
                *slot = (None, 0);
            }
        }
        found
    }
}

/// Why a spell can('t) be learned/used by this character.
/// [`Core::spell_gate`] covers gates 1-3 (spellcasting.md §2 + the
/// crime.md §6.1 alignment lattice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellGate {
    Ok,
    /// Wrong magery group, or the class can't ever cast this deep.
    WrongClass,
    /// Right class, character level below spell.required_power (+0xbe).
    /// Oracle-proven level gate (spellcasting.md §8.3).
    TooPowerful,
    /// Refused by the alignment lattice at the caster's legal level.
    Alignment,
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
    /// crime.md §8: evil banked by a previous permadeath on this account
    /// — a re-rolled character starts with it (crime follows the
    /// account). 0 for fresh accounts.
    pub saved_evil: i16,
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
    /// Death recall room (`DAT_00482cfc`/`d00` temples; per-room
    /// DeathRoom overrides arrive with zones). ORACLE: Newhaven deaths
    /// recall to Newhaven, Healer.
    pub recall_location: RoomId,
    /// The criminal temple (`DAT_00482d00`, crime.md §6.4): fame >= 0x28
    /// respawns here. DLL init default = room 142 (the "outlaw start");
    /// ORACLE-VERIFY against a live criminal death.
    pub criminal_recall_location: RoomId,
    /// PvP level-range limit (option 0x39, `DAT_00482d8c`; crime.md
    /// §6.5): -1 disables PvP interactions (incl. rob); fallback 100.
    pub pvp_level_range: i32,
    /// Restored population cooldowns: (template, seconds since its last
    /// kill at boot). Applied before the boot population walk.
    pub restored_population: Vec<(crate::content::MonsterId, i64)>,
    /// Restored room respawn stamps: (room, seconds since the kill at
    /// boot). Kills the restart-to-respawn exploit — the original
    /// persists both through the record dirty flags.
    pub restored_room_stamps: Vec<(RoomId, i64)>,
    /// Restored gang records from the state.sqlite `gang` table
    /// (gangs.md §0; disbanded rows ride along — their flags matter to
    /// the login sweep).
    pub restored_gangs: Vec<crate::gang::Gang>,
    /// Restored offline-roster mirror rows: (player name, gang display
    /// name, gang_flags word), scanned from the player table at boot.
    /// The player row stays the durable truth; Core needs the mirror
    /// because roster/rank checks must see offline members (gangs.md
    /// §1.4-§1.6) and the core is I/O-free.
    pub restored_gang_members: Vec<(String, String, u16)>,
    /// Wall-clock seconds at boot (server-stamped; tests default 0).
    /// `wall_base + scheduler.now()` dates gang creation (`+0x4a`).
    pub wall_base: i64,
    /// ANSI graphics (the MBBS per-user setting; the stock palette is
    /// hardcoded in the DLL strings — there is no palette config). The
    /// core always BUILDS colored text; false strips every escape at the
    /// output funnel. Tests default plain; the server enables it.
    pub ansi: bool,
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
            criminal_recall_location: RoomId { map: 1, room: 142 },
            pvp_level_range: 100,
            restored_population: Vec::new(),
            restored_room_stamps: Vec::new(),
            restored_gangs: Vec::new(),
            restored_gang_members: Vec::new(),
            wall_base: 0,
            ansi: false,
        }
    }
}

/// `genrdn(lo, hi)`-style PRNG: xorshift64*, uniform in `[lo, hi)` —
/// EXCLUSIVE upper bound, `lo` when the span is empty. That is genrdn's
/// real contract, not the "inclusive on both bounds" the RE docs long
/// claimed: MBBSEmu (the oracle board) implements it as .NET
/// `_random.Next(min, max)`, the DLL's own damage idiom
/// `genrdn(0,(max-min)+1)+min` only makes design sense with an exclusive
/// top, and the Nekojin Mystic capture measured punch damage 2..6 where
/// the formula max is exactly 6. Call sites transcribe the decompile's
/// genrdn arguments verbatim and rely on this contract.
/// Deterministic given the seed; exactness targets distributions, not the
/// original's roll stream (design decision).
///
/// `draws` counts every value ever taken. Nothing in the game reads it —
/// it exists so tests can pin WHERE a `genrdn` is spent, not just what it
/// decided. Several DLL predicates short-circuit ahead of their roll (the
/// pursuit follow gate at 19423, the charm-exempt retaliation twins), and
/// a test that only asserts the outcome cannot tell a skipped DRAW from a
/// skipped BRANCH — while every seeded golden in the suite depends on the
/// difference. See [`Core::debug_rng_draws`].
struct Rng {
    state: u64,
    draws: u64,
}

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng { state: seed, draws: 0 }
    }

    fn roll(&mut self, lo: i32, hi: i32) -> i32 {
        debug_assert!(lo <= hi);
        self.draws += 1;
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let x = self.state.wrapping_mul(0x2545F4914F6CDD1D);
        let span = (hi - lo).max(1) as u64;
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

/// First word and trimmed remainder of a command argument string (the
/// margv[1]/tail split several DLL commands use).
fn split_word(args: &str) -> (&str, &str) {
    match args.trim().split_once(char::is_whitespace) {
        Some((word, rest)) => (word, rest.trim()),
        None => (args.trim(), ""),
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
    /// Permadeath: remove the character record entirely. `fame` rides
    /// along so the server can bank the evil points to the account
    /// (crime.md §8 — crime follows the account).
    DeleteCharacter { name: String, fame: i16 },
    /// A limited-population template was killed — persist the wall-clock
    /// stamp (check_kill_monster's tmpl+0xb4/+0xb6 write).
    PersistMonsterKill { template: crate::content::MonsterId },
    /// A room's respawn stamp changed — persist it (the room dirty flag).
    PersistRoomStamp { room: RoomId },
    /// A gang record changed — upsert its row (the gang dirty byte
    /// `+0x54` → `save_gang_from_buffer`, gangs.md §0).
    PersistGang(Box<crate::gang::Gang>),
    /// An OFFLINE member's saved row needs a gang write (gangs.md
    /// §1.4-§1.5: offline uninvite / pending promote-demote). The word
    /// becomes `(old & and_mask) | or_mask`; `clear_gang` also empties
    /// the membership string (via `clear_player_gang`'s bit policy).
    PersistOfflineGangMember {
        name: String,
        or_mask: u16,
        and_mask: u16,
        clear_gang: bool,
    },
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
        /// A prompt is dangling on the player's current line. Async
        /// output erases it (`\r ESC[K` — the DLL's ESC[79D ESC[K
        /// discipline) and the end-of-entry sweep redraws it; input
        /// consumes it silently (the echoed Enter broke the line).
        at_prompt: bool,
        /// The attack mode (`DAT_004877e4`, kept per-fighter in the
        /// autocombat record +8 and restored each round): cmd_attack
        /// auto-picks MartialArts1 unarmed with Punch; punch/kick/
        /// jumpkick set modes 1/2/3 (combat.md "Unarmed attack modes").
        attack_mode: crate::combat::AttackType,
        /// `add_delay` units remaining (the thief-family command delay;
        /// aged one per fast tick — unit length ORACLE-VERIFY). Only
        /// SNEAK and HIDE gate on it per theft.md; every charging
        /// command extends it.
        delay: u8,
    },
}

/// Direction-word resolution for argument-taking commands (PICKLOCK
/// etc.): the two-letter aliases and full-name prefixes.
fn direction_from_word(word: &str) -> Option<crate::content::Direction> {
    use crate::content::Direction as D;
    match word {
        "n" => return Some(D::North),
        "s" => return Some(D::South),
        "e" => return Some(D::East),
        "w" => return Some(D::West),
        "ne" => return Some(D::NorthEast),
        "nw" => return Some(D::NorthWest),
        "se" => return Some(D::SouthEast),
        "sw" => return Some(D::SouthWest),
        "u" => return Some(D::Up),
        "d" => return Some(D::Down),
        _ => {}
    }
    if word.is_empty() {
        return None;
    }
    // Diagonals first so "north" cannot shadow "northeast" prefixes.
    for d in [
        D::NorthEast,
        D::NorthWest,
        D::SouthEast,
        D::SouthWest,
        D::North,
        D::South,
        D::East,
        D::West,
        D::Up,
        D::Down,
    ] {
        let name = crate::text::direction_shown(d);
        if name.starts_with(word) {
            return Some(d);
        }
    }
    None
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
    /// A picked lock's re-lock kick (theft.md §8.6; 300 s per delay
    /// unit, scheduled whole rather than the DLL's head-decrement walk —
    /// documented simplification).
    ExitRelock(RoomId, u8),
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
    /// The spawn-composed display name (`get_random_name` over the
    /// template's name block; the base template name when there is
    /// none). Death announcements re-read the TEMPLATE name instead
    /// (check_kill_monster; spellcasting.md §8.10).
    pub name: String,
    pub location: RoomId,
    pub current_hp: i32,
    /// Current energy pool (`mon+0x16`); regen/max = the template's `energy`.
    pub energy: i32,
    /// The target lock (`mon+0x1a`): retaliation, acquisition re-rolls,
    /// summon pre-locks — monsters.md §4.
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
    /// `mon+0x128` bit 0 (int-idx `0x4a`) — the CHARMED bit, the third
    /// leg of the charm.md §0 state triple (`target` = the owner name
    /// link, `suppress` = 1, this bit = 1). A full pet: assists its
    /// owner, skips the pursuit follow-roll, never wanders. Set by the
    /// Enslave apply (43806/43820) and the Summon-pet path (40050) —
    /// grudge locks, healed "friends" and summoned hunters carry the
    /// other two legs WITHOUT this one.
    pub charmed: bool,
    /// `mon+0x88` (int-idx `0x22`) — the directed-travel HUNT link: the
    /// instance id of the monster this one was summoned to kill
    /// (charm.md §6, written at `cast_monster_target` 43920). It is NOT
    /// a name link and never a pet bond: the two live on opposite sides
    /// of the driver's `+0x1a` test (20369/20450) and of the medium
    /// tick's (19339/19376), so a monster is at most one of "locked on a
    /// user" and "hunting a monster".
    ///
    /// DIVERGENCE, in our favour: the DLL clears this on nobody's death
    /// (charm.md §7) and reuses monster ids, so a long-lived hunter can
    /// silently redirect onto a recycled body. [`MonsterInstanceId`]
    /// comes from a monotonic `u64` counter, so a dangling link here is
    /// simply dead — the arm finds no instance and does nothing, which
    /// is what the DLL's own `get_monster_data` failure produces too.
    /// The link is still never cleared, so the hunter never falls
    /// through to player acquisition either.
    pub hunt: Option<MonsterInstanceId>,
    /// `mon+0x38..+0x60` — the 10-deep breadcrumb trail, newest first.
    /// `move_monster` shifts it down one slot and writes the NEW room
    /// into index 0 (21572-21574: `memmove(mon+0x3c, mon+0x38, 0x24)`
    /// then `mon+0x38 = mon+0x10`), so index 0 always equals `location`
    /// and index 1 is the predecessor. That is why
    /// [`Core::dir_toward_monster`] scans from index 1, exactly like the
    /// 20-deep player trail and [`Core::dir_toward_player`].
    ///
    /// Seeded with the spawn room. The DLL leaves the array zeroed at
    /// generate time, which its scan skips as "no such room"; a one-entry
    /// seed is the same thing (the scan starts at 1 and finds nothing).
    pub trail: Vec<RoomId>,
    /// `mon+0x120` — the spawn/home room: check_kill_monster stamps ITS
    /// respawn timer and spawn accounting, wherever the monster died.
    pub home: RoomId,
    /// `mon+0xf0..+0x100` — the five coin piles (low->high denominations),
    /// rolled `lngrnd(0, max+1)` at generate time (fixture spawns copy the
    /// template maxes verbatim to keep M3-era goldens byte-stable).
    pub coins: [u32; 5],
}

/// Whether a call site's retaliation-lock twin exempts charmed monsters.
/// This tag selects THAT ONE GATE and nothing else — the rest of
/// [`Core::retaliation_lock`]'s body is the melee twin's, whichever tag
/// is passed. See the divergence list below before adding a call site.
///
/// The DLL inlines the lock EIGHT times and they do not agree
/// (charm.md §2.4/§4.3). Five open with `(mon+0x128 & 1) == 0`:
///
/// * 26230 — the melee ENGAGE arm (the `DAT_004877f4 == '\0'` half of
///   `attack_user_monster`);
/// * 26514 — the melee round's post-damage survivor branch (whose `else`
///   at 26527 is the §4.3 owner release);
/// * 43260, 43335, 43470 — all three `cast_monster_target` ENTRY grudges:
///   the `spelltype < 3` autocombat re-fire, its ability-0x34
///   (EvilInCombat) sibling, and the duration-0 "%s moves to cast %s upon
///   %s" engage arm.
///
/// Those are [`CharmedExemption::Exempt`]: a pet is never locked and no
/// roll is drawn.
///
/// The other three carry no charmed check at all — 43752 (the
/// single-target cast-DAMAGE twin) and its area copies at 40371 and
/// 40600. Those are [`CharmedExemption::Ignored`]: a damage spell can
/// grudge-lock somebody's own pet, clearing `+0x116` and overwriting the
/// owner link, while LEAVING the charmed bit set — so the ex-pet stays a
/// non-wandering, roll-free follower that now attacks its owner.
/// Deliberate fidelity to a sloppy original, pinned by
/// `a_damage_cast_grudges_a_pet_without_releasing_it` and
/// `an_area_damage_cast_grudges_somebody_elses_pet`.
///
/// STILL UNMODELLED at the `Ignored` sites — pre-existing debt in the
/// shared body, not introduced by this tag:
///
/// 1. **The roam-class arm.** 43752 opens `template == NULL ||
///    template.group == 0x25`, and 40371/40601 open `instance roam ==
///    0x25` with no null clause; that arm does `sameas(mon+0x1a,
///    attacker)` and, on a match, clears `+0x116` — a same-attacker
///    re-hit unsuppresses an existing grudge. We return and do nothing.
/// 2. **`roam == 5` holding a lock.** The cast twins have no such
///    clause: they roll AND write. We roll and then decline the write.
/// 3. **`behaviour in {3, 0, 4}`.** The cast twins have no such clause;
///    ours forces the lock regardless of the roll, so a passive monster
///    is always grudged by a damage spell.
///
/// (43752 also reads the TEMPLATE's aggression, `knmsr+0x6e`, where the
/// melee and area twins read the instance's `mon+0x42`. Not a divergence
/// here: instance aggression is copied at spawn and never mutated.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharmedExemption {
    Exempt,
    Ignored,
}

/// What a Summon(12) spawn is bound to (charm.md §6). All four DLL
/// handlers call the same `generate_monster(map, room, -1, templateId,
/// 0, 65000, -1, 0, 1)` and then write DIFFERENT ownership state, so the
/// tag is the whole difference between the four routes:
///
/// | route | site | `+0x1a` | `+0x116` | charmed | `+0x88` |
/// |---|---|---|---|---|---|
/// | [`SummonLink::Pet`] — `cast_no_target` 0xc | 40035-40056 | caster | 1 | yes | — |
/// | [`SummonLink::HuntUser`] — `cast_user_target` 0xc | 42059-42086 | target user | 0 | no | — |
/// | [`SummonLink::HuntUser`] — `monster_cast` 0xc | 23251-23268 | victim user | 0 | no | — |
/// | [`SummonLink::HuntMonster`] — `cast_monster_target` 0xc | 43902-43931 | (empty) | 0 | no | victim id |
///
/// The two `HuntUser` rows are genuinely the same three writes; only the
/// spawn's population-cap arguments differ (the monster route passes
/// `knmsr+0x5c` for both caps instead of the `0/65000` pair), which is a
/// `generate_monster` concern and not an ownership one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SummonLink {
    /// A full, timerless pet — the §0 triple toward the caster. Released
    /// only by §4.2 (leash give-up / logout) or §4.3 (owner attacks it):
    /// there is no ability-6 slot behind it for a timer to expire.
    Pet(SessionId),
    /// A hunter against a PLAYER: a bare grudge lock, no suppression and
    /// no charm.
    HuntUser(SessionId),
    /// A hunter against a MONSTER: the `+0x88` link and nothing else.
    HuntMonster(MonsterInstanceId),
    /// No ownership at all — the monster AREA self-slot summon, whose
    /// link was not extracted (PLAUSIBLE, see its call site).
    None,
}

/// Which of `find_action_target`'s monster passes a name lookup is
/// running. The `0x800` mask bit splits the room's monsters into an
/// UNCHARMED sweep (63776, `(param_7 & 0x800) == 0 || (mon+0x128 & 1) ==
/// 0`) followed by a CHARMED-ONLY sweep (63820); without the bit there
/// is a single sweep over everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonsterPass {
    /// No `0x800` — pets and wild bodies in one pass, room order.
    All,
    /// `0x800` pass 1.
    Uncharmed,
    /// `0x800` pass 2.
    Charmed,
}

impl MonsterPass {
    fn admits(self, charmed: bool) -> bool {
        match self {
            MonsterPass::All => true,
            MonsterPass::Uncharmed => !charmed,
            MonsterPass::Charmed => charmed,
        }
    }
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
    /// The evil-pair timer list (crime.md §4; `DAT_00488180`). Nodes age
    /// on the ATTACKER's slow tick and power retaliation-free responses,
    /// FORGIVE refunds, and the room-list star.
    evil_timers: Vec<crate::crime::EvilNode>,
    /// Runtime exit lock-state overlay (theft.md §8.1 unions): keyed
    /// (room, direction), value = the state word (2 locked, 1 picked).
    /// Absent = the shipped disk state.
    exit_locks: BTreeMap<(RoomId, u8), i32>,
    /// Ephemeral floor coin piles per room (low->high denominations).
    room_coins: BTreeMap<RoomId, [u32; 5]>,
    /// Ephemeral floor items per room (item, remaining uses), seeded from
    /// the rooms' static placements at boot.
    room_items: BTreeMap<RoomId, Vec<(crate::content::ItemId, i16)>>,
    /// The thief stash (theft.md §11.2): items hidden in rooms — off the
    /// notice line, revealed by a bare SEARCH, retrievable by name.
    room_hidden_items: BTreeMap<RoomId, Vec<(crate::content::ItemId, i16)>>,
    /// Hidden coin pools per room (the §4.6 hidden-coin fields),
    /// low->high denominations like `room_coins`.
    room_hidden_coins: BTreeMap<RoomId, [u32; 5]>,
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
    /// Live gangs keyed by the uppercase name key (gangs.md §0; the
    /// WCCGANG2 buffer cache). Disbanded rows stay until every member has
    /// drained through the login sweep — matching the DLL, which keeps
    /// the record and lets cleanup delete it out-of-band.
    gangs: BTreeMap<String, crate::gang::Gang>,
    /// Pending invitations (§1.2): (invitee name UPPER, gang name key).
    /// Ephemeral by design — the DLL keeps these in an in-memory linked
    /// list that a board restart empties.
    gang_invites: BTreeSet<(String, String)>,
    /// The offline-roster mirror: gang name key → (member name,
    /// gang_flags word). Kept in lockstep by every membership mutation;
    /// loaded from the player table at boot. Player rows remain the
    /// durable truth (see CoreConfig.restored_gang_members).
    gang_members: BTreeMap<String, Vec<(String, u16)>>,
    /// Sessions holding the DISBAND yes/no continuation (input state
    /// 0x88, gangs.md §1.4) — the next line is the answer.
    pending_disband: BTreeSet<SessionId>,
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
        let rng = Rng::new(config.rng_seed | 1);
        let spawn_rng = Rng::new((config.rng_seed ^ 0x5350_4157_4e21_0000) | 1); // "SPAWN!"-ish
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
            evil_timers: Vec::new(),
            exit_locks: BTreeMap::new(),
            room_coins: BTreeMap::new(),
            room_items: BTreeMap::new(),
            room_hidden_items: BTreeMap::new(),
            room_hidden_coins: BTreeMap::new(),
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
            gangs: BTreeMap::new(),
            gang_invites: BTreeSet::new(),
            gang_members: BTreeMap::new(),
            pending_disband: BTreeSet::new(),
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
        // Restored gangs + the offline-roster mirror (gangs.md §0; the
        // player table is the durable membership truth, re-scanned by the
        // server at boot).
        for gang in core.config.restored_gangs.clone() {
            core.gangs.insert(gang.name_key.clone(), gang);
        }
        for (name, gang_display, flags) in core.config.restored_gang_members.clone() {
            let key = gang_display.to_uppercase();
            core.gang_members
                .entry(key)
                .or_default()
                .push((name, flags));
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
        // Name roll (L21102 get_random_name, AFTER the item draws and
        // BEFORE the entry-direction pick — the decompile draw order).
        let name = self.roll_spawn_name(template, &name, true);
        let id = MonsterInstanceId(self.next_monster);
        self.next_monster += 1;
        self.monsters.insert(
            id,
            MonsterInstance {
                template,
                name: name.clone(),
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
                charmed: false,
                hunt: None,
                trail: vec![room],
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
                    // %s binding is (NAME, dirspec) — CONTENT-VERIFIED
                    // against the shipped texts ("An %s walks into the
                    // room from %s." + the oracle's "An nasty orc rogue
                    // walks into the room from the west."). Texts with no
                    // %s ("appears right beside you!") print verbatim.
                    let line = template_line.replacen("%s", &name, 1);
                    let line = line.replacen("%s", &dirspec, 1);
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

    /// Places a live monster from its template (the dev/test fixture path;
    /// the density spawner uses `generate_monster`). `None` for unknown templates
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
        let base = tpl.name.clone();
        let name = self.roll_spawn_name(template, &base, false);
        self.monsters.insert(
            id,
            MonsterInstance {
                template,
                name,
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
                charmed: false,
                hunt: None,
                trail: vec![room],
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

    /// Test/inspection: a player's fame (`+0x542`).
    pub fn player_fame(&self, session: SessionId) -> i16 {
        match self.sessions.get(&session) {
            Some(Session::InGame { player, .. }) => player.fame,
            _ => 0,
        }
    }

    /// Test/staging: write a player's fame directly.
    pub fn set_player_fame(&mut self, session: SessionId, fame: i16) {
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.fame = fame;
        }
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

    /// Test hook: the charm.md §0 state triple of a live monster —
    /// (`charmed` bit `+0x128`, `suppress` byte `+0x116`, owner/grudge
    /// name link `+0x1a`). A pet is all three; a grudge-holder is the
    /// link alone; a healed "friend" is link + suppression.
    pub fn debug_monster_charm(
        &self,
        id: MonsterInstanceId,
    ) -> Option<(bool, bool, Option<SessionId>)> {
        self.monsters.get(&id).map(|m| (m.charmed, m.suppress, m.target))
    }

    /// Test hook: the `+0x88` directed-travel hunt link of a live
    /// monster (charm.md §6). `None` for every body that was not summoned
    /// by `cast_monster_target` case 0xc; the outer `Option` is
    /// "instance alive".
    pub fn debug_monster_hunt(&self, id: MonsterInstanceId) -> Option<Option<MonsterInstanceId>> {
        self.monsters.get(&id).map(|m| m.hunt)
    }

    /// Test hook: a live monster's breadcrumb trail (`mon+0x38..+0x60`),
    /// newest first — index 0 is the current room.
    pub fn debug_monster_trail(&self, id: MonsterInstanceId) -> Option<Vec<RoomId>> {
        self.monsters.get(&id).map(|m| m.trail.clone())
    }

    /// Test hook: force the `+0x116` attack-suppression byte. The state
    /// combinations the shipped write sites cannot produce (a SUPPRESSED
    /// hunter, say) are still branches of the ported code, and this is
    /// the only way to reach them without inventing a spell for it.
    pub fn debug_suppress_monster(&mut self, id: MonsterInstanceId, suppress: bool) {
        if let Some(m) = self.monsters.get_mut(&id) {
            m.suppress = suppress;
        }
    }

    /// Test hook: how many values the main `genrdn` stream has produced
    /// since the world was built. The SPAWN stream is separate and is not
    /// counted. Take a reading either side of an action and the delta is
    /// its exact draw cost — the only way to prove a short-circuited DLL
    /// predicate skips the ROLL and not merely the branch.
    pub fn debug_rng_draws(&self) -> u64 {
        self.rng.draws
    }

    /// Test hook: one monster-vs-monster swing
    /// ([`Core::attack_monster_monster`], charm.md §3). The driver arms
    /// that call it in anger land with the pet-assist and hunt branches.
    pub fn debug_monster_attack_monster(
        &mut self,
        attacker: MonsterInstanceId,
        defender: MonsterInstanceId,
    ) {
        self.attack_monster_monster(attacker, defender);
    }

    /// Test hook: one combat-driver pass over a SINGLE monster
    /// ([`Core::monster_consider`]) with no tick around it. The pet-assist
    /// branch (charm.md §2.2) is deterministic and draw-free in two of its
    /// three outcomes, and that is only measurable when the energy round's
    /// own rolls are out of the sample.
    pub fn debug_monster_consider(&mut self, id: MonsterInstanceId) {
        self.monster_consider(id);
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

    // --- M7 slice 6: the quest text-block VM (quests.md §2;
    // `perform_matched_action` 0x70209, decompile 68548-69689) ---

    /// Test hook: run one `:`-chain through the VM, returning the raw
    /// control code (0 nothing dispatched / 1 continue / 2 fail-stop).
    pub fn debug_perform_matched_action(&mut self, session: SessionId, line: &str) -> u8 {
        self.perform_matched_action(session, line) as u8
    }

    /// `perform_matched_action` (68548-69689): split the line on `:`,
    /// dispatch each token's first word, and return the LAST dispatched
    /// verb's control code (0 when nothing matched — unknown tokens are
    /// silently skipped). A FailStop ends the chain; the DLL also
    /// restores the un-run tail into the caller's buffer, which we don't
    /// need — callers get the code, not a mutated string.
    pub(crate) fn perform_matched_action(
        &mut self,
        session: SessionId,
        line: &str,
    ) -> crate::questvm::ActionCode {
        use crate::questvm::{parse_action_token, ActionCode};
        // The takeitem rollback buffer (`auStack_1b4[100]`, 68570):
        // items taken by THIS chain, restored when a later `takeitem` or
        // `price` fail-stops (69397-69404, 69666-69673) — gated on the
        // KID_GLOVES config (`DAT_004906dc`), modeled always-on.
        let mut taken: Vec<crate::content::ItemId> = Vec::new();
        let mut code = ActionCode::NoOp;
        // Some arms stop the chain WITHOUT a fail code: giveitem's
        // overflow drop (69363-69367), hideitem (69334-69338) — the
        // DLL's tail-restore-and-null pattern with `local_24` still 1.
        let mut stop = false;
        for token in line.split(':') {
            let Some((verb, args)) = parse_action_token(token) else {
                continue;
            };
            let result = self.quest_verb(session, verb, args, &mut taken, &mut stop);
            if result != ActionCode::NoOp {
                code = result;
            }
            if result == ActionCode::FailStop || stop {
                break;
            }
        }
        code
    }

    /// `perform_text_block_as_special_command` (0x71d55, 69889-69958):
    /// run the block's lines through the VM in order. Stops on the first
    /// line whose chain returns Continue(1); after a 0/2 line it keeps
    /// going only while a `:` remains anywhere in the text ahead
    /// (69944-69948). Returns the last control code. (The DLL's 50-record
    /// walk and id-mismatch stop are loader-time concerns for us — the
    /// loader pre-assembled continuation records into one body.)
    pub(crate) fn perform_text_block_as_special_command(
        &mut self,
        session: SessionId,
        block: crate::content::TextBlockId,
    ) -> u8 {
        let Some(block) = self.content.textblocks.get(&block) else {
            return 0;
        };
        let body = block.body.clone();
        let lines: Vec<&str> = body.lines().collect();
        let mut last = 0u8;
        for (i, l) in lines.iter().enumerate() {
            let code = self.perform_matched_action(session, l) as u8;
            last = code;
            if code == 1 {
                break;
            }
            if !lines[i + 1..].iter().any(|rest| rest.contains(':')) {
                break;
            }
        }
        last
    }

    /// `perform_special_command` (0x71a30, 69691-69789): the
    /// input-matched interpreter. Each line is `wildcard:action-tail`;
    /// the head is wildcard-matched against the player's RAW typed
    /// input, and a matching line's whole tail runs as a chain. A tail
    /// returning Continue(1) consumes the input and stops; any other
    /// code keeps scanning later lines. Returns the last executed
    /// tail's code — the execute_input funnel treats ANY nonzero
    /// return as consumed (49143-49145), so even a fail-stopped match
    /// swallows the input.
    pub(crate) fn perform_special_command(
        &mut self,
        session: SessionId,
        block: crate::content::TextBlockId,
        raw_input: &str,
    ) -> u8 {
        let Some(block) = self.content.textblocks.get(&block) else {
            return 0;
        };
        let body = block.body.clone();
        let mut last = 0u8;
        for line in body.lines() {
            // The DLL's colon-stream scan stops when no ':' remains
            // (69747-69751); every shipped line carries one.
            let Some((pattern, tail)) = line.split_once(':') else {
                break;
            };
            if !crate::questvm::wildcard_match(pattern, raw_input) {
                continue;
            }
            let code = self.perform_matched_action(session, tail) as u8;
            last = code;
            if code == 1 {
                break;
            }
        }
        last
    }

    /// Test hooks for the two interpreters.
    pub fn debug_run_text_block(&mut self, session: SessionId, block: u16) -> u8 {
        self.perform_text_block_as_special_command(session, crate::content::TextBlockId(block))
    }

    pub fn debug_special_command(&mut self, session: SessionId, block: u16, input: &str) -> u8 {
        self.perform_special_command(session, crate::content::TextBlockId(block), input)
    }

    /// execute_input's unconsumed-line funnel (49143-49160): action
    /// exits, then the room's cmdtext special command, then say — for
    /// EVERY line no command consumed, not just unknown verbs. The DLL
    /// runs the same funnel after every failed verb dispatch; ours
    /// previously funneled only Unknown input through action exits.
    /// (The DLL's bare-spell-name shorthand sits between cmdtext and
    /// say; unmodeled engine-wide.)
    fn fall_through(&mut self, session: SessionId, line: &str) {
        let line = line.trim();
        if self.try_action_exit(session, line) == Resolution::Handled {
            return;
        }
        if self.try_room_special(session, line) {
            return;
        }
        self.say(session, line);
    }

    /// `cmd_ask` (0x458306, 53682-53727): the monster is resolved from
    /// argv[1] — ONE word — and the question is the raw tail after it.
    /// Bare `ask` (argc < 2) and an unresolved monster return 0, which
    /// sends the whole line down the funnel. (The DLL's multi-match
    /// disambiguation flag is unmodeled — `find_monster` takes the
    /// first match, a documented divergence.)
    fn ask_command(&mut self, session: SessionId, args: &str) -> Resolution {
        let args = args.trim();
        if args.is_empty() {
            return Resolution::FallThrough;
        }
        let (monster_word, question) = match args.split_once(char::is_whitespace) {
            Some((m, q)) => (m, Some(q.trim())),
            None => (args, None),
        };
        let room = self.player(session).location;
        let Some(monster) = self.find_monster(room, monster_word) else {
            return Resolution::FallThrough;
        };
        self.ask_monster_a_question(session, monster, question);
        Resolution::Handled
    }

    /// `ask_monster_a_question` (0x20834, 18070-18200; quests.md §3):
    /// the conversation block's lines are `KEYWORD:spoken-block` pairs.
    /// No question shows the block's `next` long text; a question is
    /// substring-matched (case-insensitive, first line wins, an empty
    /// keyword matches everything); the matched SPOKEN block displays
    /// with the monster as speaker, and ITS `next` runs as a quest
    /// script (correction: the script is the spoken block's next link,
    /// 18185-18187 — not the spoken block itself).
    fn ask_monster_a_question(
        &mut self,
        session: SessionId,
        monster: MonsterInstanceId,
        question: Option<&str>,
    ) {
        let Some(instance) = self.monsters.get(&monster) else {
            return;
        };
        let name = instance.name.clone();
        let block = self
            .content
            .monsters
            .get(&instance.template)
            .and_then(|t| t.greet_block);
        let Some(block) = block.and_then(|b| self.content.textblocks.get(&b)) else {
            self.output_line(session, &text::nothing_to_tell(&name));
            return;
        };
        let next = block.next;
        let body = block.body.clone();
        let Some(question) = question.filter(|q| !q.is_empty()) else {
            // Bare ask: the default long text, or the shrug (18131-18142).
            match next {
                Some(next) => {
                    self.display_long_text(session, next, Some(&name));
                }
                None => self.output_line(session, &text::doesnt_understand(&name)),
            }
            return;
        };
        let question = question.to_ascii_uppercase();
        for line in body.lines() {
            let Some((keyword, spoken)) = line.split_once(':') else {
                continue;
            };
            // strstr, uppercased both sides; an empty keyword matches
            // everything (decompile-literal, 18144-18160).
            if !question.contains(&keyword.to_ascii_uppercase()) {
                continue;
            }
            if let Ok(spoken) = u16::try_from(crate::questvm::atol(spoken)) {
                let follow =
                    self.display_long_text(session, crate::content::TextBlockId(spoken), Some(&name));
                if let Some(follow) = follow {
                    self.perform_text_block_as_special_command(session, follow);
                }
            }
            return;
        }
        self.output_line(session, &text::nothing_to_tell(&name));
    }

    /// The room `cmdtext` hook: run the current room's special-command
    /// block against `line`, reporting whether it consumed the input.
    fn try_room_special(&mut self, session: SessionId, line: &str) -> bool {
        let Some(block) = self
            .content
            .rooms
            .get(&self.player(session).location)
            .and_then(|r| r.command_block)
        else {
            return false;
        };
        self.perform_special_command(session, block, line) != 0
    }

    /// One dispatched verb (the arm bodies of 68600-69689). Recognized
    /// verbs return Continue/FailStop; verbs whose arms land later in
    /// the slice return NoOp so the chain code is untouched.
    fn quest_verb(
        &mut self,
        session: SessionId,
        verb: crate::questvm::QuestVerb,
        args: &str,
        taken: &mut Vec<crate::content::ItemId>,
        stop: &mut bool,
    ) -> crate::questvm::ActionCode {
        use crate::questvm::{ActionCode, QuestVerb, SkillName};
        const CONTINUE: crate::questvm::ActionCode = ActionCode::Continue;
        const FAIL: crate::questvm::ActionCode = ActionCode::FailStop;
        let mut w = args.split_whitespace();
        match verb {
            QuestVerb::CheckAbility => {
                // 69280-69316: presence first (no message), then the
                // optional second arg as a minimum value.
                let Some(id) = w.next() else { return CONTINUE };
                let ability = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .and_then(Ability::from_id);
                let Some(ability) = ability.filter(|a| self.player_has_quest_ability(session, *a))
                else {
                    return FAIL;
                };
                if let Some(val) = w.next()
                    && i64::from(self.quest_ability_value(session, ability))
                        < crate::questvm::atol(val)
                {
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::TestAbility => {
                // 68855-68894: the UPPER gate. Requires both args; absent
                // ability fails for val >= 0; present fails when the
                // value exceeds val. Shipped `testability X v:
                // checkability X v` pairs pin exact-step progression.
                let (Some(id), Some(val)) = (w.next(), w.next()) else {
                    return CONTINUE;
                };
                let val = crate::questvm::atol(val);
                let ability = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .and_then(Ability::from_id)
                    .filter(|a| self.player_has_quest_ability(session, *a));
                match ability {
                    None if val >= 0 => FAIL,
                    None => CONTINUE,
                    Some(a) if val < i64::from(self.quest_ability_value(session, a)) => FAIL,
                    Some(_) => CONTINUE,
                }
            }
            QuestVerb::FailAbility => {
                // 68828-68853: the whole check nests inside the SECOND
                // arg's parse — bare `failability <id>` does nothing.
                let (Some(id), Some(msg)) = (w.next(), w.next()) else {
                    return CONTINUE;
                };
                let has = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .and_then(Ability::from_id)
                    .is_some_and(|a| self.player_has_quest_ability(session, a));
                if has {
                    self.quest_fail_message(session, crate::questvm::atol(msg));
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::Class => {
                // 69263-69277: exact match, no message arg.
                let Some(id) = w.next() else { return CONTINUE };
                if i64::from(self.player(session).class.0) != crate::questvm::atol(id) {
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::Race => {
                // 69245-69261: exact match, no message arg.
                let Some(id) = w.next() else { return CONTINUE };
                if i64::from(self.player(session).race.0) != crate::questvm::atol(id) {
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::MinLevel => {
                // 69173-69196: fail when level < n.
                let Some(n) = w.next() else { return CONTINUE };
                if i64::from(self.player(session).level) < crate::questvm::atol(n) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::MaxLevel => {
                // 69148-69171: fail when n < level.
                let Some(n) = w.next() else { return CONTINUE };
                if crate::questvm::atol(n) < i64::from(self.player(session).level) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::EvilAligned => {
                // 69198-69220: fail when the evil word (`+0x542`) is
                // BELOW n — requires at least n evil.
                let Some(n) = w.next() else { return CONTINUE };
                if i64::from(self.player(session).fame) < crate::questvm::atol(n) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::GoodAligned => {
                // 69222-69243: fail when n < the evil word — requires at
                // most n (the shipped committed-good gate is -51).
                let Some(n) = w.next() else { return CONTINUE };
                if crate::questvm::atol(n) < i64::from(self.player(session).fame) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::CheckItem => {
                // 69103-69146: scans the carried array (`+0xd8`) only —
                // never worn or wielded. (The DLL also scans `+0x334`,
                // the hangup-cleared secondary list, which has no model
                // here and no shipped writer in our engine.)
                let Some(id) = w.next() else { return CONTINUE };
                if !self.quest_carries_item(session, crate::questvm::atol(id)) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::FailItem => {
                // 69058-69101: fail when carried.
                let Some(id) = w.next() else { return CONTINUE };
                if self.quest_carries_item(session, crate::questvm::atol(id)) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::RoomItem => {
                // 68985-69029: a GATE (require the item on the floor),
                // not a spawn — quests.md §2 had this row wrong.
                let Some(id) = w.next() else { return CONTINUE };
                if !self.quest_room_has_item(session, crate::questvm::atol(id)) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::FailRoomItem => {
                // 68937-68981: fail when the item is on the floor
                // (visible `+0x470` or hidden `+0x4d8`).
                let Some(id) = w.next() else { return CONTINUE };
                if self.quest_room_has_item(session, crate::questvm::atol(id)) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::CheckSpell => {
                // FUN_0046f4a5 (67891-67936): scans the ACTIVE effect
                // slots (`+0x40`), not the spellbook; the optional
                // failure arg is a fail-BLOCK, not a message.
                let Some(id) = w.next() else { return CONTINUE };
                let id = crate::questvm::atol(id);
                let active = self.player(session).active_spells.iter().any(|s| {
                    s.spell
                        .is_some_and(|sp| i64::from(sp.0) == id)
                });
                if !active {
                    if let Some(block) = w.next()
                        && let Ok(block) = u16::try_from(crate::questvm::atol(block))
                    {
                        self.perform_text_block_as_special_command(
                            session,
                            crate::content::TextBlockId(block),
                        );
                    }
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::NeedMonster => {
                // FUN_00470116 (68491-68542): fail unless a monster of
                // the TEMPLATE is in the room.
                let Some(id) = w.next() else { return CONTINUE };
                let id = crate::questvm::atol(id);
                let room = self.player(session).location;
                let present = self
                    .monsters
                    .values()
                    .any(|m| m.location == room && i64::from(m.template.0) == id);
                if !present {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::Monsters => {
                // 69463-69493: fail when the room has no monster at all.
                let room = self.player(session).location;
                if !self.monsters.values().any(|m| m.location == room) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::NoMonsters => {
                // 69495-69521: fail when ANY monster is present.
                let room = self.player(session).location;
                if self.monsters.values().any(|m| m.location == room) {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::TestSkill => {
                // FUN_0046f53a, roll variant (68078-68095): one
                // genrdn(0, range) draw; fail when skill < roll +
                // modifier, running the fail-BLOCK. One arg = the block
                // (modifier 0); two args = modifier then block.
                let Some(skill) = w.next() else { return CONTINUE };
                let skill = SkillName::parse(skill);
                let (modifier, block) = match (w.next(), w.next()) {
                    (Some(a), Some(b)) => (crate::questvm::atol(a), crate::questvm::atol(b)),
                    (Some(a), None) => (0, crate::questvm::atol(a)),
                    _ => return CONTINUE,
                };
                let range = skill.map_or(100, SkillName::roll_range);
                let value = self.quest_skill_value(session, skill);
                let roll = self.rng.roll(0, range);
                if i64::from(value) < i64::from(roll) + modifier {
                    if let Ok(block) = u16::try_from(block) {
                        self.perform_text_block_as_special_command(
                            session,
                            crate::content::TextBlockId(block),
                        );
                    }
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::CheckSkill => {
                // FUN_0046f53a, threshold variant (68097-68107): no
                // roll; value < threshold → message + fail. The check
                // only runs when the message arg is present
                // (decompile-literal quirk).
                let Some(skill) = w.next() else { return CONTINUE };
                let skill = SkillName::parse(skill);
                let (Some(threshold), Some(msg)) = (w.next(), w.next()) else {
                    return CONTINUE;
                };
                let value = self.quest_skill_value(session, skill);
                if i64::from(value) < crate::questvm::atol(threshold) {
                    self.quest_fail_message(session, crate::questvm::atol(msg));
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::TestTournament => {
                // 68710-68720: passes only when the tournament config
                // byte (`DAT_004906c9`) is 2 — this board never is.
                FAIL
            }
            QuestVerb::Flag => {
                // FUN_0046fdda (68319-68413): 64 player flag bits with
                // set/check/fail/clear sub-ops; out-of-range fails.
                let Some(n) = w.next() else { return CONTINUE };
                let n = crate::questvm::atol(n);
                if !(1..=64).contains(&n) {
                    return FAIL;
                }
                let bit = 1u64 << (n - 1);
                match w.next().map(str::to_ascii_lowercase).as_deref() {
                    Some("set") => {
                        if let Some(Session::InGame { player, .. }) =
                            self.sessions.get_mut(&session)
                        {
                            player.quest_flags |= bit;
                        }
                        let snapshot = Box::new(self.player(session).clone());
                        self.events.push(Event::Persist(snapshot));
                        CONTINUE
                    }
                    Some("clear") => {
                        if let Some(Session::InGame { player, .. }) =
                            self.sessions.get_mut(&session)
                        {
                            player.quest_flags &= !bit;
                        }
                        let snapshot = Box::new(self.player(session).clone());
                        self.events.push(Event::Persist(snapshot));
                        CONTINUE
                    }
                    Some("check") => {
                        if self.player(session).quest_flags & bit == 0 {
                            self.quest_gate_message(session, w.next());
                            return FAIL;
                        }
                        CONTINUE
                    }
                    Some("fail") => {
                        if self.player(session).quest_flags & bit != 0 {
                            self.quest_gate_message(session, w.next());
                            return FAIL;
                        }
                        CONTINUE
                    }
                    _ => CONTINUE,
                }
            }
            QuestVerb::AddAbility => {
                // 69590-69637: raise-to-at-least; creating a fresh 0xa0
                // slot also grants the spell whose id is the VALUE
                // (69619-69624); no slot found → fail-stop.
                let (Some(id), Some(val)) = (w.next(), w.next()) else {
                    return CONTINUE;
                };
                let Some(ability) = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .and_then(Ability::from_id)
                else {
                    return FAIL;
                };
                let val = crate::questvm::atol(val);
                let Ok(val16) = i16::try_from(val) else {
                    return FAIL;
                };
                let (ok, fresh) = {
                    let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
                    else {
                        return FAIL;
                    };
                    let had = player.innate.iter().any(|(a, _)| *a == Some(ability));
                    (player.raise_innate_ability(ability, val16), !had)
                };
                if !ok {
                    return FAIL;
                }
                if fresh
                    && ability.id() == 0xa0
                    && let Ok(spell) = u16::try_from(val)
                    && self.content.spells.contains_key(&SpellId(spell))
                    && let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
                {
                    player.spellbook.entry(SpellId(spell)).or_insert(false);
                }
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::GiveAbility => {
                // 69565-69588 → FUN_0046c507 (accumulate); a 0 return
                // (0xa0, or a full table with no match) fail-stops.
                let (Some(id), Some(val)) = (w.next(), w.next()) else {
                    return CONTINUE;
                };
                let Some(ability) = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .and_then(Ability::from_id)
                else {
                    return FAIL;
                };
                let Ok(val) = i16::try_from(crate::questvm::atol(val)) else {
                    return FAIL;
                };
                let ok = match self.sessions.get_mut(&session) {
                    Some(Session::InGame { player, .. }) => {
                        player.give_innate_ability(ability, val)
                    }
                    _ => false,
                };
                if !ok {
                    return FAIL;
                }
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::RemoveAbility => {
                // 69523-69558: zero every matching slot — a 0xa0 slot
                // purges the spell whose id is the slot VALUE first
                // (69536-69540); an absent id fail-stops.
                let Some(id) = w.next() else { return CONTINUE };
                let Some(ability) = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .and_then(Ability::from_id)
                else {
                    return FAIL;
                };
                let found = {
                    let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
                    else {
                        return FAIL;
                    };
                    if ability.id() == 0xa0 {
                        let spells: Vec<u16> = player
                            .innate
                            .iter()
                            .filter(|(a, _)| *a == Some(ability))
                            .filter_map(|(_, v)| u16::try_from(*v).ok())
                            .collect();
                        for spell in spells {
                            player.spellbook.remove(&SpellId(spell));
                        }
                    }
                    player.remove_innate_ability(ability)
                };
                if !found {
                    return FAIL;
                }
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::AddExp => {
                // 69372-69381 → add_quest_exp (0x6f291, 67785-67815;
                // quests.md §4.1): a raw experience add — no over-level
                // cap, no party split, silent (tell=0). The restructured
                // gate (`+0x7d5 & 0x20`) is always-passing for our
                // characters (documented divergence).
                let Some(n) = w.next() else { return CONTINUE };
                let n = crate::questvm::atol(n);
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    player.experience = player.experience.saturating_add_signed(n);
                }
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::GiveCoins => {
                // 68723-68748: `givecoins <n> [letter]` — the letter
                // dispatch (toupper, jump table 0x47183d) adds to that
                // denomination; the bare form adds to `+0x620`, the
                // LAST field of the runic..copper block = copper. Every
                // shipped use carries a letter (almost always G) with
                // tokens after it — Ghidra's `return` at 68742 is a
                // mangled indirect JUMP; the chain continues.
                let Some(n) = w.next() else { return CONTINUE };
                let n = crate::questvm::atol(n);
                let add = |field: &mut u32| {
                    *field = field.saturating_add_signed(i32::try_from(n).unwrap_or(0));
                };
                let letter = w
                    .next()
                    .and_then(|l| l.chars().next())
                    .map(|c| c.to_ascii_uppercase());
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    match letter {
                        Some('C') => add(&mut player.coins.copper),
                        Some('S') => add(&mut player.coins.silver),
                        Some('G') => add(&mut player.coins.gold),
                        Some('P') => add(&mut player.coins.platinum),
                        Some('R') => add(&mut player.coins.runic),
                        _ => add(&mut player.coins.copper),
                    }
                }
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::AddEvil => {
                // 68896-68908: `+0x542 += n`, raw — NOT the crime.rs
                // funnel: no Warn-on-Evil refusal, no minimum-10 bump,
                // no 30000 cap; negative amounts pay evil down (the
                // good-path quests do exactly that).
                let Some(n) = w.next() else { return CONTINUE };
                let n = crate::questvm::atol(n);
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    player.fame = player.fame.saturating_add(i16::try_from(n).unwrap_or(0));
                }
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::LearnSpell => {
                // FUN_0046fff6 (68418-68486): unknown spell fail-stops;
                // already in the book is a silent no-op; the
                // user_can_use_spell gate (→ spell_gate) refuses with
                // "You don't know what to do with this!"; success prints
                // "You learn the spell %s." and adds it permanently.
                let Some(id) = w.next() else { return CONTINUE };
                let Some(spell_id) = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .map(SpellId)
                    .filter(|id| self.content.spells.contains_key(id))
                else {
                    return FAIL;
                };
                if self.player(session).spellbook.contains_key(&spell_id) {
                    return CONTINUE;
                }
                let spell = self.content.spells[&spell_id].clone();
                if self.spell_gate(self.player(session), &spell) != SpellGate::Ok {
                    self.output_line(session, text::LEARNSPELL_CANT);
                    return FAIL;
                }
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    player.spellbook.insert(spell_id, false);
                }
                self.output_line(session, &text::learn_spell(&spell.name));
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::Cast => {
                // 69639-69658: alloc → cast_no_target(spell, user,
                // forced) — failure fail-stops; an unresolvable spell id
                // leaves the code at 1 (decompile-literal).
                let Some(id) = w.next() else { return CONTINUE };
                let Some(spell_id) = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .map(SpellId)
                    .filter(|id| self.content.spells.contains_key(id))
                else {
                    return CONTINUE;
                };
                if !self.forced_cast(session, spell_id) {
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::GiveItem => {
                // 69342-69370: add_item_to_inventory(user, item, -2) —
                // -2 copies the template's uses (13952-13953). On
                // failure the item lands on the floor VISIBLE and the
                // chain stops with code 1 (69352-69368). Our failure
                // condition is the 100-slot cap; the DLL's weight and
                // add-logical gates are unmodeled engine-wide.
                let Some(id) = w.next() else { return CONTINUE };
                let Some((item, uses)) = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .map(crate::content::ItemId)
                    .and_then(|id| self.content.items.get(&id).map(|i| (id, i.uses)))
                else {
                    // Unknown item: the add fails and nothing can drop
                    // (69354-69356 gets NULL) — chain still stops.
                    *stop = true;
                    return CONTINUE;
                };
                let room = self.player(session).location;
                let full = self.player(session).inventory.len() >= 100;
                if full {
                    self.room_items.entry(room).or_default().push((item, uses));
                    *stop = true;
                    return CONTINUE;
                }
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    player.inventory.push((item, uses));
                }
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::TakeItem => {
                // 69383-69424: remove from the carried array; success
                // buffers the id (`auStack_1b4`), failure re-adds every
                // buffered item — with uses 0, the re-add's literal
                // third arg (69399) — then the optional message and a
                // fail-stop. The KID_GLOVES rollback gate is modeled
                // always-on.
                let Some(id) = w.next() else { return CONTINUE };
                let id = crate::questvm::atol(id);
                let removed = match self.sessions.get_mut(&session) {
                    Some(Session::InGame { player, .. }) => {
                        match player
                            .inventory
                            .iter()
                            .position(|(item, _)| i64::from(item.0) == id)
                        {
                            Some(pos) => Some(player.inventory.remove(pos).0),
                            None => None,
                        }
                    }
                    _ => None,
                };
                match removed {
                    Some(item) => {
                        if taken.len() < 100 {
                            taken.push(item);
                        }
                        let snapshot = Box::new(self.player(session).clone());
                        self.events.push(Event::Persist(snapshot));
                        CONTINUE
                    }
                    None => {
                        if let Some(Session::InGame { player, .. }) =
                            self.sessions.get_mut(&session)
                        {
                            for item in taken.drain(..) {
                                player.inventory.push((item, 0));
                            }
                        }
                        self.quest_gate_message(session, w.next());
                        let snapshot = Box::new(self.player(session).clone());
                        self.events.push(Event::Persist(snapshot));
                        FAIL
                    }
                }
            }
            QuestVerb::HideItem => {
                // 69318-69340: spawn the item HIDDEN on the floor with
                // template uses, then an UNCONDITIONAL
                // tail-restore-and-stop with code 1. Zero shipped uses;
                // decompile-literal.
                let Some(id) = w.next() else { return CONTINUE };
                if let Some((item, uses)) = u16::try_from(crate::questvm::atol(id))
                    .ok()
                    .map(crate::content::ItemId)
                    .and_then(|id| self.content.items.get(&id).map(|i| (id, i.uses)))
                {
                    let room = self.player(session).location;
                    self.room_hidden_items
                        .entry(room)
                        .or_default()
                        .push((item, uses));
                }
                *stop = true;
                CONTINUE
            }
            QuestVerb::ClearItem => {
                // FUN_0046c241 (65701-65773): remove EVERY matching slot,
                // visible and hidden; id 0 clears the whole floor; not
                // found → optional message + fail-stop.
                let Some(id) = w.next() else { return CONTINUE };
                let id = crate::questvm::atol(id);
                let room = self.player(session).location;
                let mut found = false;
                for list in [
                    self.room_items.entry(room).or_default(),
                    self.room_hidden_items.entry(room).or_default(),
                ] {
                    if id == 0 {
                        found = true;
                        list.clear();
                    } else {
                        let before = list.len();
                        list.retain(|(item, _)| i64::from(item.0) != id);
                        found |= list.len() != before;
                    }
                }
                if !found {
                    self.quest_gate_message(session, w.next());
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::Message => {
                // 69426-69435 → FUN_0046f360 (the workhorse — 515
                // shipped uses): line 1 to the actor, line 2 to the room
                // with the actor's name substituted.
                let Some(msg) = w.next() else { return CONTINUE };
                self.quest_fail_message(session, crate::questvm::atol(msg));
                CONTINUE
            }
            QuestVerb::Text => {
                // 69046-69056 → display_LONG_text(block, 0): the
                // assembled body prints raw to the actor (36055-36056).
                let Some(block) = w.next() else { return CONTINUE };
                if let Ok(block) = u16::try_from(crate::questvm::atol(block)) {
                    self.display_long_text(session, crate::content::TextBlockId(block), None);
                }
                CONTINUE
            }
            QuestVerb::RoomText => {
                // 69033-69044 → display_LONG_text_to_room: tell_room
                // with NO exclusion (36128) — the actor sees it too.
                // Zero shipped uses; decompile-literal.
                let Some(block) = w.next() else { return CONTINUE };
                let block = match u16::try_from(crate::questvm::atol(block)) {
                    Ok(b) => crate::content::TextBlockId(b),
                    Err(_) => return CONTINUE,
                };
                let Some(block) = self.content.textblocks.get(&block) else {
                    return CONTINUE;
                };
                let body = block.body.clone();
                let room = self.player(session).location;
                for line in body.lines() {
                    self.broadcast_to_room(room, None, line);
                }
                CONTINUE
            }
            QuestVerb::Summon => {
                // 69437-69456: generate_monster(player room, template,
                // …) — an ORDINARY spawn, no owner link (this is NOT the
                // spell Summon(12) path and writes none of the slice-5
                // tags); a failed spawn fail-stops.
                let Some(template) = w.next() else { return CONTINUE };
                let Ok(template) = u16::try_from(crate::questvm::atol(template)) else {
                    return FAIL;
                };
                let room = self.player(session).location;
                if self
                    .spawn_monster(crate::content::MonsterId(template), room)
                    .is_none()
                {
                    return FAIL;
                }
                CONTINUE
            }
            QuestVerb::Teleport => {
                // FUN_0046f887: numeric `teleport <room> <map>` (room
                // FIRST, 68159-68168 — all 240 shipped uses). A real
                // move relocates (FUN_00416ae6 writes location only; the
                // room-history trails and entry-cast hook there are
                // unmodeled engine-wide), shows the room (ORACLE-VERIFY:
                // the DLL leaves the display to the caller), and returns
                // code 2 — the script stops, you left (68299-68310).
                // The 12 named destinations (silvermere/sewers/… random
                // ranges, 68135-68297) have ZERO shipped uses — their
                // words atol to room 0, a no-op, and stay unported.
                let (Some(room), Some(map)) = (w.next(), w.next()) else {
                    return CONTINUE;
                };
                let (Ok(room), Ok(map)) = (
                    u16::try_from(crate::questvm::atol(room)),
                    u16::try_from(crate::questvm::atol(map)),
                ) else {
                    return CONTINUE;
                };
                let dest = RoomId { map, room };
                if !self.content.rooms.contains_key(&dest) || self.player(session).location == dest
                {
                    return CONTINUE;
                }
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    player.location = dest;
                }
                self.show_room(session);
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                FAIL
            }
            QuestVerb::Random => {
                // 68773-68790: the runner's code passes through — only
                // a 2 restores the tail and stops; 0 leaves the chain
                // code untouched.
                let Some(block) = w.next() else { return CONTINUE };
                let Ok(block) = u16::try_from(crate::questvm::atol(block)) else {
                    return CONTINUE;
                };
                match self.quest_random_block(session, crate::content::TextBlockId(block)) {
                    2 => FAIL,
                    1 => CONTINUE,
                    _ => ActionCode::NoOp,
                }
            }
            QuestVerb::Price => {
                // FUN_0046f3fa (67849-67886): total wealth vs n copper;
                // deduct on success. Failure shows the optional message,
                // fail-stops, and restores this chain's taken items —
                // the caller's rollback loop (69662-69674).
                let Some(n) = w.next() else { return CONTINUE };
                let n = u64::try_from(crate::questvm::atol(n)).unwrap_or(0);
                let ratios = self.config.coin_ratios;
                if self.player(session).coins.total_copper(ratios) < n {
                    self.quest_gate_message(session, w.next());
                    if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
                    {
                        for item in taken.drain(..) {
                            player.inventory.push((item, 0));
                        }
                    }
                    let snapshot = Box::new(self.player(session).clone());
                    self.events.push(Event::Persist(snapshot));
                    return FAIL;
                }
                if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                    player.coins.deduct_copper(n, ratios);
                }
                let snapshot = Box::new(self.player(session).clone());
                self.events.push(Event::Persist(snapshot));
                CONTINUE
            }
            QuestVerb::AddDelay => {
                // 68761-68771: `player+0x6bb += n` — the same command
                // delay the theft verbs charge (add_delay), consumed by
                // delay_blocked.
                let Some(n) = w.next() else { return CONTINUE };
                let n = u8::try_from(crate::questvm::atol(n)).unwrap_or(u8::MAX);
                self.add_delay(session, n);
                CONTINUE
            }
            QuestVerb::RemoteAction => {
                // FUN_0046c573 (65933-66161) via the arm at 68793-68824:
                // `remoteaction <room> <msg> <action> <exit 0-10>` — the
                // target room is on the ACTOR's map.
                let (Some(room), Some(msg), Some(action), Some(exitn)) =
                    (w.next(), w.next(), w.next(), w.next())
                else {
                    return CONTINUE;
                };
                let action = crate::questvm::atol(action);
                let exitn = crate::questvm::atol(exitn);
                if !(0..=10).contains(&exitn) {
                    return CONTINUE;
                }
                let Ok(room) = u16::try_from(crate::questvm::atol(room)) else {
                    return CONTINUE;
                };
                let target = RoomId {
                    map: self.player(session).location.map,
                    room,
                };
                // The optional message pair: room line first
                // (name-substituted), then the actor line (65955-65970).
                self.quest_remote_message(session, crate::questvm::atol(msg));
                let Some(exit) = self
                    .content
                    .rooms
                    .get(&target)
                    .and_then(|r| r.exits.get(exitn as usize).cloned().flatten())
                else {
                    return CONTINUE;
                };
                let d = exitn as u8;
                match exit.exit_type {
                    6 => self.remote_lever(target, d, &exit, action),
                    7 | 0xb => self.remote_gate_toggle(target, d, &exit),
                    // M7 PENDING(slice-6): the type-9/0x18 arm
                    // (66119-66150) force-moves every player and monster
                    // in the target room through the exit (move_user
                    // mode 7); at most two ambiguous shipped uses.
                    _ => {}
                }
                CONTINUE
            }
        }
    }

    /// The `random` verb's block runner (`FUN_00471c02`, 69795-69884):
    /// ONE genrdn(0,100) draw at entry, then the FIRST line whose
    /// numeric head exceeds the roll runs its whole tail as a chain and
    /// its code is returned; 0 when no line qualifies. (Malformed
    /// no-colon lines stop the scan — shipped random blocks are all
    /// clean `NN:actions` lines.)
    fn quest_random_block(
        &mut self,
        session: SessionId,
        block: crate::content::TextBlockId,
    ) -> u8 {
        let Some(block) = self.content.textblocks.get(&block) else {
            return 0;
        };
        let body = block.body.clone();
        let roll = self.rng.roll(0, 100);
        for line in body.lines() {
            let Some((head, tail)) = line.split_once(':') else {
                break;
            };
            if i64::from(roll) < crate::questvm::atol(head) {
                return self.perform_matched_action(session, tail) as u8;
            }
        }
        0
    }

    /// The remoteaction message pair (65955-65970): line 2 to the
    /// actor's room with the name substituted, THEN line 1 to the actor
    /// — the reverse of FUN_0046f360's order.
    fn quest_remote_message(&mut self, session: SessionId, msg: i64) {
        let Some(msg) = u16::try_from(msg)
            .ok()
            .filter(|m| *m != 0)
            .and_then(|id| self.content.messages.get(&crate::content::MessageId(id)))
        else {
            return;
        };
        let name = self.player(session).name.clone();
        let room = self.player(session).location;
        let room_line = msg
            .lines
            .get(1)
            .map(|l| l.replacen("%s", &name, 1))
            .unwrap_or_default();
        let user_line = msg.lines.first().cloned().unwrap_or_default();
        if !room_line.is_empty() {
            self.broadcast_to_room(room, Some(session), &room_line);
        }
        if !user_line.is_empty() {
            self.output_line(session, &user_line);
        }
    }

    /// remoteaction case 6 (65973-66079): the lever machinery over a
    /// hidden exit's concealment bit-word (bits 0x10..0x2000, kept in
    /// the exit_locks overlay; disk para1 seeds it). Action 0 clears
    /// them all; action n clears bit n+3, gated — unless para2 is
    /// negative — on bit n+4 being already clear (levers pull in
    /// descending order). All bits clear → state 8 (lever-revealed),
    /// the ~5 min re-hide kick, and the reveal line to the TARGET room
    /// (custom message = para3's line 1 with the direction substituted,
    /// else the stock concealed-passage line).
    fn remote_lever(&mut self, target: RoomId, d: u8, exit: &crate::content::Exit, action: i64) {
        let cur = *self.exit_locks.get(&(target, d)).unwrap_or(&exit.param);
        if matches!(cur, 4 | 8) {
            return;
        }
        let word = cur as u32;
        let unconditional = exit.param2 < 0;
        let new = match action {
            0 => word & 0xffff_c00f,
            n @ 1..=9 => {
                let bit = 1u32 << (n + 3);
                let gate = 1u32 << (n + 4);
                if unconditional || word & gate == 0 {
                    word & !bit
                } else {
                    word
                }
            }
            10 => word & !0x2000,
            _ => word,
        };
        if new & 0x3ff0 == 0 {
            self.exit_locks.insert((target, d), 8);
            self.scheduler.schedule_in(300, Job::ExitRelock(target, d));
            let dir = crate::content::Direction::ALL[d as usize];
            let custom = u16::try_from(exit.param3)
                .ok()
                .filter(|m| *m != 0)
                .and_then(|id| self.content.messages.get(&crate::content::MessageId(id)))
                .and_then(|m| m.lines.first())
                .filter(|l| !l.is_empty())
                .map(|l| l.replacen("%s", text::direction_shown(dir), 1));
            let line =
                custom.unwrap_or_else(|| text::concealed_passage_opens(text::direction_shown(dir)));
            self.broadcast_to_room(target, None, &line);
        } else {
            self.exit_locks.insert((target, d), new as i32);
        }
    }

    /// remoteaction case 7/0xb (66081-66117): toggle the gate's lock
    /// state 0 <-> 2, re-lock timer on open (300 s x max(para3, 1) —
    /// the picklock convention), and the PAIRED reverse exit in the
    /// destination room toggles on its own timer.
    fn remote_gate_toggle(&mut self, target: RoomId, d: u8, exit: &crate::content::Exit) {
        let toggle = |core: &mut Core, room: RoomId, d: u8, exit: &crate::content::Exit| {
            let cur = core.exit_lock_state(room, d, exit);
            let new = if cur == 0 { 2 } else { 0 };
            core.exit_locks.insert((room, d), new);
            if new == 0 {
                let secs = 300 * u64::from(u16::try_from(exit.param3.max(1)).unwrap_or(1));
                core.scheduler.schedule_in(secs, Job::ExitRelock(room, d));
            }
        };
        toggle(self, target, d, exit);
        let dir = crate::content::Direction::ALL[d as usize];
        let opposite = dir.opposite();
        if let Some(back) = self
            .content
            .rooms
            .get(&exit.dest)
            .and_then(|r| r.exits[opposite as usize].clone())
            .filter(|b| b.dest == target && matches!(b.exit_type, 7 | 0xb))
        {
            toggle(self, exit.dest, opposite as usize as u8, &back);
        }
    }

    /// `display_LONG_text` (0x3bcca, 36021-36078): print the block's
    /// assembled body to the actor — raw with no speaker, or as a
    /// format string with the speaker name substituted for `%s`
    /// (36055-36059) — and return the block's `next` link (word +10),
    /// which `ask` feeds to the unconditional runner.
    pub(crate) fn display_long_text(
        &mut self,
        session: SessionId,
        block: crate::content::TextBlockId,
        speaker: Option<&str>,
    ) -> Option<crate::content::TextBlockId> {
        let block = self.content.textblocks.get(&block)?;
        let next = block.next;
        let body = match speaker {
            Some(name) => block.body.replacen("%s", name, 1),
            None => block.body.clone(),
        };
        for line in body.lines() {
            self.output_line(session, line);
        }
        next
    }

    /// The gates' optional trailing message arg: present → show it.
    fn quest_gate_message(&mut self, session: SessionId, msg: Option<&str>) {
        if let Some(msg) = msg {
            self.quest_fail_message(session, crate::questvm::atol(msg));
        }
    }

    /// `FUN_0046f360` (67820-67844): message line 1 to the actor, line 2
    /// to the room with the actor's name substituted. (The `DAT_0048fcf4`
    /// prefix byte is ORACLE-VERIFY — pinned with the slice-8 strings.)
    fn quest_fail_message(&mut self, session: SessionId, msg: i64) {
        let Some(msg) = u16::try_from(msg)
            .ok()
            .and_then(|id| self.content.messages.get(&crate::content::MessageId(id)))
        else {
            return;
        };
        let user_line = msg.lines.first().cloned().unwrap_or_default();
        let name = self.player(session).name.clone();
        let room_line = msg
            .lines
            .get(1)
            .map(|l| l.replacen("%s", &name, 1))
            .unwrap_or_default();
        let room = self.player(session).location;
        if !user_line.is_empty() {
            self.output_line(session, &user_line);
        }
        if !room_line.is_empty() {
            self.broadcast_to_room(room, Some(session), &room_line);
        }
    }

    /// `user_has_ability` (0x3d570, 37059-37145): presence across ALL
    /// sources — active-effect spell rows, race, class, the innate
    /// table, worn items, the wielded weapon, and carried inventory
    /// (skipping weapons `+0x2f4 == 1` and wearable items
    /// `+0x398 != 0` — those only count when worn).
    fn player_has_quest_ability(&self, session: SessionId, ability: Ability) -> bool {
        let player = self.player(session);
        if player.innate.iter().any(|(id, _)| *id == Some(ability)) {
            return true;
        }
        for slot in &player.active_spells {
            if let Some(spell) = slot.spell.and_then(|id| self.content.spells.get(&id))
                && spell.abilities.iter().any(|(a, _)| *a == ability)
            {
                return true;
            }
        }
        if let Some(race) = self.content.races.get(&player.race)
            && race.abilities.iter().any(|(a, _)| *a == ability)
        {
            return true;
        }
        if let Some(class) = self.content.classes.get(&player.class)
            && class.abilities.iter().any(|(a, _)| *a == ability)
        {
            return true;
        }
        let worn_or_wielded = player.worn.iter().chain(player.weapon.iter());
        for (id, _) in worn_or_wielded {
            if let Some(item) = self.content.items.get(id)
                && item.abilities.iter().any(|(a, _)| *a == ability)
            {
                return true;
            }
        }
        for (id, _) in &player.inventory {
            if let Some(item) = self.content.items.get(id)
                && item.item_type != 1
                && item.worn_on == 0
                && item.abilities.iter().any(|(a, _)| *a == ability)
            {
                return true;
            }
        }
        false
    }

    /// `get_user_ability_value` (0x3d038, 36795-37052): the same source
    /// set as the presence scan, folded with SUM semantics — except ids
    /// {0x16, 0x47, 0x48, 0x57, 0x65, 0x69, 0x6a}, which keep the MAX
    /// single contribution (36818-36832). An active spell carrying a
    /// NegateAbility (0x7c) row naming this id zeroes the result
    /// (36892-36898, 37038-37041).
    fn quest_ability_value(&self, session: SessionId, ability: Ability) -> i32 {
        let max_mode = matches!(
            ability.id(),
            0x16 | 0x47 | 0x48 | 0x57 | 0x65 | 0x69 | 0x6a
        );
        let player = self.player(session);
        let mut contributions: Vec<i32> = Vec::new();
        for (id, v) in &player.innate {
            if *id == Some(ability) {
                contributions.push(i32::from(*v));
            }
        }
        let mut negated = false;
        for slot in &player.active_spells {
            let Some(spell) = slot.spell.and_then(|id| self.content.spells.get(&id)) else {
                continue;
            };
            for (row_ability, row_value) in &spell.abilities {
                if *row_ability == ability {
                    // A value-0 row means "the stored magnitude" — the
                    // slot's `+0x54` value (36877-36883).
                    contributions.push(match *row_value {
                        0 => i32::from(slot.value),
                        v => i32::from(v),
                    });
                }
                if row_ability.id() == 0x7c && i64::from(*row_value) == i64::from(ability.id()) {
                    negated = true;
                }
            }
        }
        if let Some(race) = self.content.races.get(&player.race) {
            for (a, v) in &race.abilities {
                if *a == ability {
                    contributions.push(i32::from(*v));
                }
            }
        }
        if let Some(class) = self.content.classes.get(&player.class) {
            for (a, v) in &class.abilities {
                if *a == ability {
                    contributions.push(i32::from(*v));
                }
            }
        }
        let worn_or_wielded = player.worn.iter().chain(player.weapon.iter());
        for (id, _) in worn_or_wielded {
            if let Some(item) = self.content.items.get(id) {
                let sum: i32 = item
                    .abilities
                    .iter()
                    .filter(|(a, _)| *a == ability)
                    .map(|(_, v)| i32::from(*v))
                    .sum();
                if sum != 0 {
                    contributions.push(sum);
                }
            }
        }
        for (id, _) in &player.inventory {
            if let Some(item) = self.content.items.get(id)
                && item.item_type != 1
                && item.worn_on == 0
            {
                let sum: i32 = item
                    .abilities
                    .iter()
                    .filter(|(a, _)| *a == ability)
                    .map(|(_, v)| i32::from(*v))
                    .sum();
                if sum != 0 {
                    contributions.push(sum);
                }
            }
        }
        if negated {
            return 0;
        }
        if max_mode {
            contributions.into_iter().max().unwrap_or(0)
        } else {
            contributions.into_iter().sum()
        }
    }

    /// The carried-inventory scan shared by checkitem/failitem
    /// (69066-69073): the `+0xd8` array only.
    fn quest_carries_item(&self, session: SessionId, item: i64) -> bool {
        self.player(session)
            .inventory
            .iter()
            .any(|(id, _)| i64::from(id.0) == item)
    }

    /// The floor scan shared by roomitem/failroomitem (68945-69013):
    /// visible (`+0x470`) and hidden (`+0x4d8`) slots.
    fn quest_room_has_item(&self, session: SessionId, item: i64) -> bool {
        let room = self.player(session).location;
        let in_list = |list: Option<&Vec<(crate::content::ItemId, i16)>>| {
            list.is_some_and(|items| items.iter().any(|(id, _)| i64::from(id.0) == item))
        };
        in_list(self.room_items.get(&room)) || in_list(self.room_hidden_items.get(&room))
    }

    /// The named-skill source table for testskill/checkskill
    /// (`FUN_0046f53a` 67995-68070): effective stats, the derived skill
    /// words, MR, and current HP. Unknown names read 0.
    fn quest_skill_value(&self, session: SessionId, skill: Option<crate::questvm::SkillName>) -> i32 {
        use crate::questvm::SkillName::*;
        let player = self.player(session);
        let Some(skill) = skill else { return 0 };
        match skill {
            Agility => i32::from(player.stats.agility),
            Strength => i32::from(player.stats.strength),
            Intellect => i32::from(player.stats.intellect),
            Wisdom => i32::from(player.stats.wisdom),
            Health => i32::from(player.stats.health),
            Charm => i32::from(player.stats.charm),
            CurrentHp => player.current_hp,
            _ => {
                let derived = self.derive_for(player);
                match skill {
                    Spellcasting => derived.spellcasting,
                    Perception => derived.perception,
                    Stealth => derived.stealth,
                    Thievery => derived.thievery,
                    Traps => derived.find_traps,
                    Picklocks => derived.picklocks,
                    Tracking => derived.tracking,
                    MagicResistance => derived.magic_resist,
                    _ => unreachable!("stat arms handled above"),
                }
            }
        }
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
                    // Age the thief-family command delays.
                    let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
                    for id in ids {
                        if let Some(Session::InGame { delay, .. }) = self.sessions.get_mut(&id) {
                            *delay = delay.saturating_sub(1);
                        }
                    }
                    self.fast_update();
                    self.scheduler.schedule_in(FAST_INTERVAL, Job::Fast);
                }
                Job::Spawn => {
                    self.spawn_pass();
                    self.scheduler.schedule_in(SPAWN_INTERVAL, Job::Spawn);
                }
                Job::ExitRelock(room, dir) => self.relock_exit(room, dir),
                Job::Cleanup => {
                    self.reconcile_shelves();
                    self.scheduler.schedule_in(CLEANUP_INTERVAL, Job::Cleanup);
                }
            }
        }
        self.reprompt_disturbed();
    }

    /// Redraw the prompt for every in-game session whose dangling prompt
    /// was erased by async output this entry (the DLL re-prompts after
    /// every prf burst). Meditating sessions keep accumulating dots.
    fn reprompt_disturbed(&mut self) {
        let disturbed: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                matches!(
                    s,
                    Session::InGame { at_prompt: false, exiting: None, .. }
                )
            })
            .map(|(id, _)| *id)
            .collect();
        for id in disturbed {
            self.show_prompt(id);
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
        // decrement_evil_timers (crime.md §4.2): each ONLINE attacker's
        // nodes lose one round per slow tick; expired nodes free.
        let online: Vec<String> = self
            .in_game_sessions()
            .map(|(_, p)| p.name.clone())
            .collect();
        for node in &mut self.evil_timers {
            if online.iter().any(|n| n.eq_ignore_ascii_case(&node.attacker)) {
                node.rounds = node.rounds.saturating_sub(1);
            }
        }
        self.evil_timers.retain(|n| n.rounds > 0);
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
            let mut terminate = false;
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
                terminate = true;
            }
            if terminate {
                self.terminate_monster_slot(id, &spell, stored);
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

    /// `perform_spell_termination_monster_upkeep` (`0x4a45d`, 44972) —
    /// the WHOLE monster-side termination handler, which is a two-case
    /// switch and nothing more: case 6 (Enslave, [`Core::release_charm`])
    /// and case 0x13 (Poison, 45003-45008). No stat reversal, no wear-off
    /// line, no EndCast chain — the player-side handler's other two dozen
    /// cases have no monster twin.
    ///
    /// `stored` is the slot's saved value; a non-zero ability ROW wins
    /// over it, the same precedence the routine handler uses.
    ///
    /// Every caller goes through here rather than reaching for case 6
    /// alone: on shipped data the two are equivalent (none of the four
    /// Enslave spells — 49 song of charming, 55 enslave, 88 control
    /// undead, 92 charm animal — carries an ability-19 row), but a
    /// fixture that pairs Enslave with Poison would otherwise leave the
    /// slot-sweep paths silently forgetting to drain the counter.
    fn terminate_monster_slot(
        &mut self,
        id: MonsterInstanceId,
        spell: &crate::content::Spell,
        stored: i32,
    ) {
        for (ability, row) in &spell.abilities {
            let v = if *row != 0 { i32::from(*row) } else { stored };
            match ability {
                Ability::Enslave => self.release_charm(id),
                Ability::Poison => {
                    if let Some(m) = self.monsters.get_mut(&id) {
                        m.poison = clamp_poison(i32::from(m.poison) - v);
                        m.needs_recompute = true;
                    }
                }
                _ => {}
            }
        }
    }

    /// `perform_spell_termination_monster_upkeep` case 6 (44988-44995) —
    /// the charm.md §4.1 reversal, and the ONLY place the whole §0 triple
    /// comes apart at once:
    ///
    /// ```text
    /// mon+0x140 = 1        ; dirty (SET, not cleared)
    /// mon+0x1a  = 0        ; owner name emptied
    /// mon+0x116 = 0        ; suppression off
    /// mon+0x128 &= ~1      ; charmed bit off
    /// ```
    ///
    /// No message to anyone, in any direction. The released monster is
    /// NEUTRAL — it holds no grudge against the ex-owner and rejoins
    /// ordinary wander/acquisition, so it may re-acquire them through the
    /// normal aggression rolls a moment later.
    fn release_charm(&mut self, id: MonsterInstanceId) {
        if let Some(m) = self.monsters.get_mut(&id) {
            m.needs_recompute = true;
            m.target = None;
            m.suppress = false;
            m.charmed = false;
        }
    }

    /// The ability-6 slot sweep, inlined verbatim at three DLL sites
    /// (19455-19487 give-up, 26527-26562 owner melee, 46929-46953
    /// autocombat): walk the 5 slots; a slot whose spell id no longer
    /// resolves is simply zeroed, and a slot whose spell carries
    /// Enslave(6) goes through the FULL termination handler
    /// ([`Core::terminate_monster_slot`] — 26548 passes the whole spell
    /// record, not just the charm case) and is then zeroed. Slots holding
    /// anything else are left alone.
    ///
    /// This is what makes the §4.3 asymmetry: a SLOTLESS pet (instant
    /// Enslave, or a Summon-born pet) has nothing for the sweep to find,
    /// so whichever legs of the triple the caller did not clear itself
    /// survive the release.
    ///
    /// DIVERGENCE, unobservable on shipped data: the DLL's termination
    /// call sits INSIDE the per-ability-row loop (26547), so a spell with
    /// two ability-6 rows would run the handler twice. We run it once per
    /// slot. No shipped spell has a repeated row.
    fn sweep_charm_slots(&mut self, id: MonsterInstanceId) {
        for idx in 0..5 {
            let Some(m) = self.monsters.get(&id) else {
                return;
            };
            let Some(spell_id) = m.active_spells[idx].spell else {
                continue;
            };
            let stored = i32::from(m.active_spells[idx].value);
            // Unknown id (19461-19465): the slot is zeroed but nothing is
            // terminated — the DLL cannot ask an absent record for its
            // ability rows.
            let charmer = match self.content.spells.get(&spell_id) {
                None => None, // unknown: zero the slot, terminate nothing
                Some(spell) => {
                    if spell.abilities.iter().any(|(a, _)| *a == Ability::Enslave) {
                        Some(spell.clone())
                    } else {
                        continue; // an unrelated slot survives untouched
                    }
                }
            };
            if let Some(spell) = charmer {
                self.terminate_monster_slot(id, &spell, stored);
            }
            if let Some(m) = self.monsters.get_mut(&id) {
                m.active_spells[idx] = ActiveSpell::default();
                m.needs_recompute = true;
            }
        }
        self.recompute_monster_effects(id);
    }

    /// `FUN_0044cc65`'s self-target arm (46929-46953, charm.md §2.2/§4.3):
    /// the owner's autocombat target IS its own pet, so the pet releases
    /// itself on the next combat pass. Note what is NOT here — unlike the
    /// melee twin (26527) this arm never touches `+0x116`, so a SLOTLESS
    /// pet keeps both its owner link and its suppression and degrades into
    /// a "friend" (a monster that attacks players OTHER than its owner)
    /// rather than into a grudge holder.
    ///
    /// The call site is [`Core::pet_assist`], the combat driver's
    /// pet-assist branch.
    fn autocombat_release_charm(&mut self, id: MonsterInstanceId) {
        if let Some(m) = self.monsters.get_mut(&id) {
            m.charmed = false;
        }
        self.sweep_charm_slots(id);
    }

    /// The movement half of `medium_update_monster` (decompile
    /// 19339-19381; monsters.md §3). THREE levels, in the DLL's own
    /// order — getting the nesting wrong freezes bodies the original
    /// moves:
    ///
    /// 1. 19339 — a monster holding a name link (`mon+0x1a`) does nothing
    ///    here at all; the pursuit tier owns its movement.
    /// 2. 19340 — otherwise the `+0x88` hunt link splits the branch:
    ///    non-zero takes the TRAVEL arm (19376-19380,
    ///    [`Core::dir_toward_monster`] then one gated step), zero falls
    ///    through to the wander arms. The travel arm is charm-blind and
    ///    roam-blind: no budget, no aggression roll, no roam-class
    ///    switch.
    /// 3. the wander arms themselves, by roam class: 0/2 stationary;
    ///    5 water (no aggression roll, budget consumed before the
    ///    confusion check); default rolls `genrdn(0,100) <
    ///    (100-aggression)/2` then checks confusion, then consumes
    ///    budget. The chosen direction is rejected (budget already spent)
    ///    when it equals the last-move memory.
    ///
    /// The charmed test `(mon+0x128 & 1) == 0` belongs to level 3 and
    /// ONLY to level 3 (19346 and 19364) — a pet never wanders,
    /// charm.md §2.1. It was hoisted to level 1 before the travel arm
    /// existed; that was a latent divergence, since a charmed body
    /// carrying a hunt link travels in the DLL and would have been
    /// frozen here.
    ///
    /// Both level-3 copies are load-bearing, and an earlier version of
    /// this comment was WRONG to say otherwise. It claimed the guards
    /// were unreachable because "every charm apply also writes the owner
    /// link, and every release that clears the link clears the bit with
    /// it". True of the charm paths — and irrelevant, because the link
    /// is also cleared by code that knows nothing about charm:
    /// `monster_attack`'s death branch (`check_kill_user`, 27194) and its
    /// post-swing lock re-roll (26867-26885) both empty `+0x1a` while
    /// leaving `+0x128` alone. A pet reaches the first of those through
    /// the departure free-attack, since a NON-owner walking out of the
    /// room is a valid victim for a suppressed monster. A charmed body
    /// with no owner link is therefore an ordinary reachable state, it
    /// falls straight through level 1, and only these guards stop it
    /// drifting away from where its owner left it. Pinned per arm by
    /// `charm.rs::a_charmed_pet_never_wanders_the_default_arm` and
    /// `..._the_water_arm`.
    fn wander_monster(&mut self, id: MonsterInstanceId) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        if m.target.is_some() {
            return; // 19339: locked on a user; pursuit owns movement
        }
        // 19376-19380: the directed-travel arm. Unlike the driver's twin
        // (20450-20463) it never swings on a cold trail — the medium tick
        // only ever walks.
        if let Some(quarry) = m.hunt {
            let from = m.location;
            if let Some(dir) = self.dir_toward_monster(quarry, from)
                && !self.monster_confusion_fumble(id)
            {
                self.move_monster(id, dir, false);
            }
            return;
        }
        let (roam, aggression, from, last, charmed) = (
            m.roam_class,
            m.aggression,
            m.location,
            m.last_move_dir,
            m.charmed,
        );
        match roam {
            0 | 2 => return,
            5 => {
                // Water path (19364-19371): charm, cap (the mon+0x140
                // dirty-byte bypass is unmodeled — PLAUSIBLE quirk),
                // budget consumed before the confusion check.
                if charmed || self.wander_budget >= 3 {
                    return;
                }
                self.wander_budget += 1;
                if self.monster_confusion_fumble(id) {
                    return;
                }
            }
            _ => {
                // Default path (19346-19360): charm, cap, roll,
                // confusion, budget. The charm test precedes the
                // `genrdn`, so a pet costs no draw.
                if charmed || self.wander_budget >= 3 {
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
        let charmed = m.charmed;
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
                } else if !charmed && self.rng.roll(0, 100) >= i32::from(aggression) {
                    // The follow roll failed. A PET never gets here:
                    // 19422-19423 spells the gate `(mon+0x128 & 1) == 0 &&
                    // genrdn(0,100) >= aggression`, so the charmed bit
                    // both skips the DRAW (short-circuit, exactly as the
                    // DLL's `&&` does — this is RNG-order load-bearing)
                    // and makes the refusal unreachable: a pet always
                    // follows, whatever its template aggression says
                    // (charm.md §2.1). Every OTHER refusal source below
                    // still bumps the counter.
                    bump = true;
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
        // Since the counter bumps once per fast tick on an OFFLINE owner
        // too (19412-19415), this doubles as the logout release: ~16 s
        // after the owner drops, the pet is free (charm.md §4.2).
        if m.give_up > 15 {
            if roam == 0x25 {
                self.monsters.remove(&id);
                return;
            }
            m.needs_recompute = true; // 19451: `+0x140` dirty
            m.give_up = 0;
            m.target = None;
            // 19452-19487: a charmed monster additionally loses the bit
            // and has its ability-6 slots terminated. LITERAL SHAPE — the
            // branch itself never writes `+0x116`, so suppression comes
            // off only through the slot termination: a slotless pet ages
            // out nameless but still SUPPRESSED.
            if m.charmed {
                m.charmed = false;
                self.sweep_charm_slots(id);
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

    /// `dir_monster_travelling_coord` (15790-15816) — the monster twin of
    /// [`Core::dir_toward_player`], reading the VICTIM MONSTER's
    /// breadcrumb trail instead of a player's.
    ///
    /// Three differences from the player version, all decompile-literal:
    ///
    /// * the trail is 10 deep, not 20 (15810: `iVar4 < 10`);
    /// * there is no MAP check on the trail entry (the player version
    ///   conjoins `player+0x550+i*4 == map` at 15668; this one compares
    ///   the room word alone). Single-map worlds cannot show it;
    /// * the co-location test is the VICTIM's current room against the
    ///   hunter's (15799), so a hunter standing on its quarry gets `None`
    ///   — which is the driver arm's cue to swing rather than step.
    ///
    /// The scan starts at index 1 because index 0 is the victim's CURRENT
    /// room; finding the hunter's room at index `i` means the victim
    /// stood there `i` steps ago and left toward `trail[i-1]`.
    fn dir_toward_monster(
        &self,
        victim: MonsterInstanceId,
        mon_room: RoomId,
    ) -> Option<crate::content::Direction> {
        let v = self.monsters.get(&victim)?;
        if v.location == mon_room {
            return None;
        }
        let i = (1..v.trail.len()).find(|i| v.trail[*i] == mon_room)?;
        let next_room = v.trail[i - 1];
        let room = self.content.rooms.get(&mon_room)?;
        crate::content::Direction::ALL.into_iter().find(|d| {
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
            // The breadcrumb push (21572-21574), the exact shape of the
            // player one at the `move_user` relocation: shift down, write
            // the NEW room into index 0, cap at ten.
            m.trail.insert(0, dest);
            m.trail.truncate(10);
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
        let mut player = player;
        // load_player's quest passes (M7 slice 6), in the DLL's order:
        // the class-skill strip (0x15084, 10164-10201) then the
        // completion detector (FUN_00414d23, called at 10632) — both
        // BEFORE the stat derivation so grants feed the bag.
        self.quest_login_strip(&mut player);
        let penalty_lines = self.run_quest_completion(&mut player);
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
                at_prompt: false,
                attack_mode: crate::combat::AttackType::Normal,
                delay: 0,
            });
        for line in penalty_lines {
            self.output_line(id, &line);
        }
        self.show_room(id);
        self.show_prompt(id);
        self.reprompt_disturbed();
        id
    }

    /// load_player's quest-earned class-skill strip (0x15084, decompile
    /// 10164-10201; quests.md §4.4): Smash (0x20), Perfect Stealth
    /// (0xba) and Meditate (0xbb) survive only for the listed class at
    /// or above its level gate — change class or fall below and the
    /// skill is gone at next login. Tables verbatim from the decompile.
    fn quest_login_strip(&self, player: &mut Player) {
        const SMASH: &[(u16, u16)] =
            &[(1, 22), (2, 20), (3, 25), (4, 27), (0xb, 27), (0xe, 22)];
        const PERFECT_STEALTH: &[(u16, u16)] = &[
            (6, 25),
            (7, 20),
            (8, 20),
            (9, 25),
            (10, 25),
            (0xe, 20),
            (0xf, 27),
        ];
        const MEDITATE: &[(u16, u16)] = &[
            (3, 27),
            (4, 23),
            (5, 20),
            (6, 23),
            (9, 27),
            (10, 23),
            (0xb, 23),
            (0xc, 20),
            (0xd, 20),
            (0xe, 27),
        ];
        let class = player.class.0;
        let level = player.level;
        let keeps = |table: &[(u16, u16)]| {
            table
                .iter()
                .any(|(c, l)| *c == class && level >= *l)
        };
        for slot in &mut player.innate {
            let stripped = match slot.0.map(|a| a.id()) {
                Some(0x20) => !keeps(SMASH),
                Some(0xba) => !keeps(PERFECT_STEALTH),
                Some(0xbb) => !keeps(MEDITATE),
                _ => false,
            };
            if stripped {
                *slot = (None, 0);
            }
        }
    }

    /// The quest completion detector (`FUN_00414d23`, asm export;
    /// quests.md §4.3): one pass captures the quest counters
    /// (0x7d-0x84, LAST matching slot wins) and zeroes every reward-id
    /// slot (LAB_00414dd0 — the idempotence pre-pass), then the
    /// threshold table re-grants fresh. Returns the SheDragon penalty
    /// lines to print once the session exists.
    fn run_quest_completion(&self, player: &mut Player) -> Vec<String> {
        const REWARD_IDS: [u16; 10] =
            [0x2, 0x4, 0x16, 0x1b, 0x22, 0x3a, 0x45, 0x46, 0x75, 0x76];
        let (mut ice, mut good, mut neutral, mut evil) = (0i32, 0i32, 0i32, 0i32);
        let (mut druid, mut champ, mut dragon, mut rat) = (0i32, 0i32, 0i32, 0i32);
        for slot in &mut player.innate {
            let Some(id) = slot.0.map(|a| a.id()) else { continue };
            match id {
                0x7d => ice = i32::from(slot.1),
                0x7e => good = i32::from(slot.1),
                0x7f => neutral = i32::from(slot.1),
                0x80 => evil = i32::from(slot.1),
                0x81 => druid = i32::from(slot.1),
                0x82 => champ = i32::from(slot.1),
                0x83 => dragon = i32::from(slot.1),
                0x84 => rat = i32::from(slot.1),
                _ if REWARD_IDS.contains(&id) => *slot = (None, 0),
                _ => {}
            }
        }
        let give = |player: &mut Player, id: u16, v: i16| {
            player.give_innate_ability(Ability::from_id(id).expect("reward id"), v);
        };
        // IceSorc == 2 -> AC +1 (asm 00414e4d).
        if ice == 2 {
            give(player, 0x2, 1);
        }
        // The alignment paths (00414e5f): Good >= 8, Neutral >= 8, or
        // Evil >= 4 -> the CURRENT class's package (jump table 0x414ea1).
        if good >= 8 || neutral >= 8 || evil >= 4 {
            match player.class.0 {
                1 | 2 | 3 | 0xf => give(player, 0x4, 1),
                4 | 0xb => {
                    give(player, 0x2, 1);
                    give(player, 0x45, 6);
                }
                5 | 0xc | 0xd => {
                    give(player, 0x46, 1);
                    give(player, 0x45, 10);
                }
                6 | 9 | 10 => {
                    give(player, 0x75, 6);
                    give(player, 0x76, 6);
                    give(player, 0x1b, 1);
                    give(player, 0x45, 4);
                }
                7 | 8 | 0xe => {
                    give(player, 0x75, 10);
                    give(player, 0x76, 10);
                    give(player, 0x1b, 2);
                }
                _ => {}
            }
        }
        // DarkDruid == 2 -> S.C. +1 (00414f60).
        if druid == 2 {
            give(player, 0x46, 1);
        }
        // BloodChamp == 2 -> Accuracy +3 (00414f73).
        if champ == 2 {
            give(player, 0x16, 3);
        }
        // SheDragon == 3 -> Crits +1, S.C. +2 (00414f86).
        if dragon == 3 {
            give(player, 0x3a, 1);
            give(player, 0x46, 2);
        }
        let mut lines = Vec::new();
        // SheDragon == 2 -> the cheater penalty (00414fa5-00415068):
        // above 35,000,000 exp the strip and de-level fire with the two
        // stripped lines; at or below, silence — but the 0x83 counter
        // slots clear either way.
        if dragon == 2 {
            if player.experience > 35_000_000 {
                player.experience -= 35_000_000;
                self.quest_delevel(player);
                lines.push(text::SHEDRAGON_STRIPPED.to_string());
                lines.push(text::SHEDRAGON_RETRAIN.to_string());
            }
            for slot in &mut player.innate {
                if slot.0.map(|a| a.id()) == Some(0x83) {
                    *slot = (None, 0);
                }
            }
        }
        // Wererat == 2 -> Dodge +1 (0041506a).
        if rat == 2 {
            give(player, 0x22, 1);
        }
        lines
    }

    /// `FUN_00414c39` (9905-9944): walk the level down while the exp
    /// counter sits below the curve for the current level. (The DLL
    /// also resets the CP sentinel and HP base per step — recompute
    /// machinery our model derives elsewhere; unported.)
    fn quest_delevel(&self, player: &mut Player) {
        let base = self.exp_base(player);
        while player.level > 1
            && player.experience < crate::stats::exp_needed(player.level - 1, base)
        {
            player.level -= 1;
        }
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
        // The 30-slot innate/quest table (`+0x73a`/`+0x776`) is folded by
        // the same reads (`get_user_ability_value` 36850-36869) — quest
        // completion grants (AC/Accuracy/Dodge/…) reach combat through
        // here.
        for (ability, value) in &player.innate {
            if let Some(ability) = ability {
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
            derived.max_hp,
            player.current_mana,
            derived.max_mana,
            caster_group,
        );
        self.output(session, &prompt);
        if let Some(Session::InGame { at_prompt, .. }) = self.sessions.get_mut(&session) {
            *at_prompt = true;
        }
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
        let (armour_ac, armour_dr) = self.get_armour_rating(session);
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
            // `get_armour_rating` returns tenths; the status line divides
            // both by 10 (31553-31556).
            armour_class: armour_ac / 10,
            armour_max: armour_dr / 10,
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
        // The player's echoed Enter already broke the prompt line — no
        // erase codes for their own command's responses.
        if let Some(Session::InGame { at_prompt, .. }) = self.sessions.get_mut(&session) {
            *at_prompt = false;
        }
        match self.sessions.get(&session) {
            None => {}
            Some(Session::ChoosingRace { .. }) => self.choose_race(session, line),
            Some(Session::ChoosingClass { .. }) => self.choose_class(session, line),
            Some(Session::ChoosingLawful { .. }) => self.choose_lawful(session, line),
            // Oracle: commands during exit meditation are refused.
            Some(Session::InGame { exiting: Some(_), .. }) => {
                self.output_line(session, text::MEDITATION_BLOCKED);
            }
            Some(Session::InGame { .. }) => {
                // A pending yes/no continuation consumes the line first
                // (the DLL's usrptr+0x1c input states; 0x88 = disband).
                if self.pending_disband.remove(&session) {
                    self.disband_confirm(session, line);
                } else {
                    self.game_command(session, line)
                }
            }
        }
        self.reprompt_disturbed();
    }

    fn game_command(&mut self, session: SessionId, line: &str) {
        match parse(line) {
            Command::Quit => self.quit(session),
            // Blank input re-shows the room without its description (oracle).
            Command::Blank => self.show_room_brief(session),
            Command::Look => self.show_room(session),
            Command::Exits => self.show_exits_line(session),
            Command::Help => self.output_line(session, text::HELP_BANNER),
            Command::Top(args) => self.top_command(session, &args),
            Command::Gang(message) => {
                if self.gang_command(session, &message) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Create(args) => {
                if self.create_command(session, &args) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Join(args) => {
                if self.join_command(session, &args) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Invite(name) => {
                if self.invite_command(session, &name) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Uninvite(name) => {
                if self.uninvite_command(session, &name) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Promote(name) => {
                if self.promote_command(session, &name, true) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Demote(name) => {
                if self.promote_command(session, &name, false) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Disband(args) => {
                if self.disband_command(session, &args) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Leave(args) => {
                if self.leave_command(session, &args) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Stock(args) => {
                if self.stock_command(session, &args) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Unstock(args) => {
                if self.unstock_command(session, &args) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Markup(args) => {
                if self.markup_command(session, &args) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Get(target) => {
                if target.trim().is_empty() {
                    self.output_line(session, text::SYNTAX_GET);
                } else if self.get_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Drop(target) => {
                if self.drop_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Inventory => self.show_inventory(session),
            Command::List => {
                if self.list_command(session) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Buy(target) => {
                if self.buy_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Sell(target) => {
                if self.sell_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Deposit(amount) => self.deposit_command(session, &amount),
            Command::Withdraw(amount) => self.withdraw_command(session, &amount),
            Command::Balance => self.balance_command(session),
            Command::Arm(target) => {
                if self.arm_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Wear(target) => {
                if self.wear_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Remove(target) => {
                if self.remove_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Use(target) => {
                if self.use_command(session, &target, false) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Read(target) => {
                if self.use_command(session, &target, true) == Resolution::FallThrough {
                    self.fall_through(session, line);
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
                    self.fall_through(session, line);
                }
            }
            Command::Ansi => self.ansi_command(session),
            Command::Picklock(args) => self.picklock_command(session, &args),
            Command::Search(args) => self.search_command(session, &args),
            Command::Disarm(args) => self.disarm_command(session, &args),
            Command::Rob(target) => self.rob_command(session, &target),
            Command::Forgive(target) => self.forgive_command(session, &target),
            Command::Ask(target) => {
                if self.ask_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Sneak => self.sneak_command(session),
            Command::Hide(args) => {
                if args.trim().is_empty() {
                    self.hide_command(session);
                } else {
                    self.hide_stash_command(session, &args);
                }
            }
            Command::Set(args) => {
                if self.set_command(session, &args) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Punch(target) => {
                // cmd_punch: hidden/sneaking AND unarmed diverts to the
                // backstab mode; armed (or visible) punches stay mode 1.
                let p = self.player(session);
                let mode = if (p.hidden || p.sneak_armed) && p.weapon.is_none() {
                    crate::combat::AttackType::Backstab
                } else {
                    crate::combat::AttackType::MartialArts1
                };
                if self.ma_command(session, &target, 0x1d, mode) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Backstab(target) => {
                if self.backstab_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::Kick(target) => {
                let mode = crate::combat::AttackType::MartialArts2;
                if self.ma_command(session, &target, 0x1e, mode) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            Command::JumpKick(target) => {
                let mode = crate::combat::AttackType::MartialArts3;
                if self.ma_command(session, &target, 0x23, mode) == Resolution::FallThrough {
                    self.fall_through(session, line);
                }
            }
            // Cast never falls through to say: an unresolvable spell prints
            // the do-not-know line (MEASURED §8.6/§8.9). Invoke is its kai
            // twin (§8.12).
            Command::Cast(args) => self.cast_command(session, &args),
            Command::Invoke(args) => self.invoke_command(session, &args),
            Command::Aid(target) => {
                if self.aid_command(session, &target) == Resolution::FallThrough {
                    self.fall_through(session, line);
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
            // Type-10 action exits trigger on their phrases, then the
            // room's cmdtext special command; anything else is said
            // aloud (oracle) - there is no error reply.
            Command::Unknown(what) => self.fall_through(session, &what),
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
        // cmd_attack 49712-49720: a bare attack auto-selects mode-1
        // fists of fury when unarmed with the Punch ability. (The
        // hidden/sneak divert to mode 4 landed with the theft slice and
        // is the inner branch below.)
        let player = self.player(session);
        let mode = if player.weapon.is_none()
            && self
                .ability_bag(player)
                .value(Ability::from_id(0x1d).expect("Punch in the enum"))
                > 0
        {
            // 49716-49724: hidden/sneak-armed diverts the unarmed
            // auto-pick to the backstab mode.
            if player.hidden || player.sneak_armed {
                crate::combat::AttackType::Backstab
            } else {
                crate::combat::AttackType::MartialArts1
            }
        } else {
            crate::combat::AttackType::Normal
        };
        self.attack_with_mode(session, target_words, mode)
    }

    /// `cmd_backstab` (0x51573): visible = a silent plain attack;
    /// hidden/sneaking = mode 4 when unarmed or the weapon carries
    /// BSAccu (0x74) — a non-backstab weapon prints the refusal and
    /// attacks normally (the DLL leaves its two mode globals
    /// DISAGREEING there; the effective fighter mode is normal).
    fn backstab_command(&mut self, session: SessionId, target_words: &str) -> Resolution {
        let player = self.player(session);
        let stealthy = player.hidden || player.sneak_armed;
        let mode = if !stealthy {
            crate::combat::AttackType::Normal
        } else {
            match player.weapon {
                None => crate::combat::AttackType::Backstab,
                Some((id, _)) => {
                    let bs_capable = self
                        .content
                        .items
                        .get(&id)
                        .is_some_and(|i| {
                            i.abilities.iter().any(|(a, _)| {
                                Ability::from_id(0x74).is_some_and(|b| *a == b)
                            })
                        });
                    if bs_capable {
                        crate::combat::AttackType::Backstab
                    } else {
                        self.output_line(session, text::CANNOT_BACKSTAB_WEAPON);
                        crate::combat::AttackType::Normal
                    }
                }
            }
        };
        self.attack_with_mode(session, target_words, mode)
    }

    /// The MA verb family (cmd_punch 0x51e37 / cmd_kick 0x51df2 /
    /// cmd_jumpkick 0x51dad): gate on the granting ability — without it
    /// the handler returns 0 and the input falls through to SAY — then
    /// set the attack mode and run the shared attack path.
    fn ma_command(
        &mut self,
        session: SessionId,
        target_words: &str,
        ability_id: u16,
        mode: crate::combat::AttackType,
    ) -> Resolution {
        let player = self.player(session);
        let granted = self
            .ability_bag(player)
            .value(Ability::from_id(ability_id).expect("MA ability in the enum"))
            > 0;
        if !granted {
            return Resolution::FallThrough;
        }
        self.attack_with_mode(session, target_words, mode)
    }

    fn attack_with_mode(
        &mut self,
        session: SessionId,
        target_words: &str,
        mode: crate::combat::AttackType,
    ) -> Resolution {
        if self.player(session).current_hp < 1 {
            self.output_line(session, text::MORTALLY_WOUNDED);
            return Resolution::Handled;
        }
        let room = self.player(session).location;
        let monster = if target_words.trim().is_empty() {
            self.auto_pick_target(session, room)
        } else {
            // `cmd_any_attack` 49590 passes mask `0x883` — the `0x800`
            // bit rides ATTACK too, so a named swing prefers a wild body
            // over your own pet and only reaches the pet when nothing
            // else in the room answers to the name (§2.3).
            self.find_monster_charmed_last(room, target_words)
        };
        let Some(monster) = monster else {
            return Resolution::FallThrough;
        };
        if self.charge_passive_monster_evil(session, monster) {
            return Resolution::Handled;
        }
        if let Some(Session::InGame { target, casting, .. }) = self.sessions.get_mut(&session) {
            // Oracle: attacking while already engaged prints *Combat Off*
            // before the new *Combat Engaged*.
            *casting = None; // a melee attack replaces any cast engagement
            if target.is_some() {
                *target = None;
                self.output_line(session, text::COMBAT_OFF);
            }
        }
        if let Some(Session::InGame { target, attack_mode, .. }) = self.sessions.get_mut(&session)
        {
            *target = Some(monster);
            *attack_mode = mode;
        }
        self.output_line(session, text::COMBAT_ENGAGED);
        // The ENGAGE-arm retaliation lock (26230-26237), fired right
        // after `engage_autocombat` and before any round runs.
        //
        // It belongs HERE and nowhere else. `attack_user_monster` splits
        // on `DAT_004877f4` at 26112: the `== '\0'` half is the ATTACK
        // command — messages, `engage_autocombat`, this lock, and NO
        // swings — and its `else` (26241) is the autocombat round, which
        // swings and carries its own post-damage lock/release pair
        // (26514-26563). The two are arms of one `if`, so 26230 can
        // never run after 26527 in the same call. Hanging this re-mark
        // off the tail of the swing loop let both run and immediately
        // re-grudged a just-released pet back onto its owner.
        //
        // Divergence, pre-existing and untouched: our ATTACK command
        // goes on to swing, which the DLL's engage arm does not. The
        // `target.is_none()` gate is likewise ours — 26230 has no such
        // clause and would re-roll over an existing lock (unobservable
        // against the same attacker, and a pet is charm-exempt either
        // way, but it does cost a draw the DLL spends and we do not).
        if self.monsters.get(&monster).is_some_and(|m| m.target.is_none()) {
            self.retaliation_lock(monster, session, CharmedExemption::Exempt);
        }
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

    /// `user_can_use` (0x1fced): the alignment lattice FIRST (crime.md
    /// §6.1 — ahead of every class/race check, and the allowlist bypass
    /// does not reach it), then class/race allowlists (a match bypasses
    /// the permission matrix), AntiMagic vs Magical, MinLevel/MaxLevel
    /// abilities, then the class weapon/armour matrix.
    fn user_can_use(&self, player: &Player, item: &crate::content::Item) -> bool {
        let level = crate::crime::legal_level(player.fame);
        let has = |id: u16| {
            Ability::from_id(id).is_some_and(|a| item.abilities.iter().any(|(ab, _)| *ab == a))
        };
        if crate::crime::alignment_refuses(level, has) {
            return false;
        }
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
        // Gate 3 — the crime.md §6.1 lattice (user_can_use_spell
        // 17811-17846, identical table to items).
        let level = crate::crime::legal_level(player.fame);
        let has = |id: u16| {
            Ability::from_id(id)
                .is_some_and(|a| spell.abilities.iter().any(|(ab, _)| *ab == a))
        };
        if crate::crime::alignment_refuses(level, has) {
            return SpellGate::Alignment;
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
        let room = self.player(session).location;
        let mut monster = None;
        if target.is_empty() {
            // No target word: the DLL never runs a find at all
            // (dispatcher 59247-59252 -> `cast_no_target`). Offensive
            // modes refuse; benign ones fall through to the self tail.
            if offensive {
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
                    if let Some(Session::InGame { energy, .. }) = self.sessions.get_mut(&session)
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
        } else {
            // The dispatcher's TWO-STAGE find (decompile 59256-59271):
            // search the match type's preferred scope first, and when it
            // comes back empty repeat with the universal mask 0xf037.
            // The preferred scope is therefore only an ORDERING
            // preference — every match type can resolve every kind, and
            // the ACCEPTANCE decision belongs to the entry point the
            // found kind selects (59278-59320). `spelltype` routes
            // nothing; it owns hostility only.
            let preferred = spell.match_type.preferred_find();
            let found = if preferred.is_empty() {
                None
            } else {
                self.find_cast_target(session, preferred, &target)
            };
            let fallback = crate::content::FindScope::UNIVERSAL;
            match found.or_else(|| self.find_cast_target(session, fallback, &target)) {
                None => {
                    // MEASURED (§8.9): the entire remainder is one target
                    // string, echoed verbatim.
                    self.output_line(session, &text::do_not_see_here(&target));
                    return;
                }
                Some(CastTarget::Monster(id)) => {
                    if !spell.match_type.accepts_monster() {
                        // `cast_monster_target` 43205 / 44311-44315,
                        // uncharged — MEASURED §8.13 for `c blur cat`
                        // (match 2) and `c stnk cat` (match 12).
                        self.output_line(session, text::MAY_NOT_CAST_ON_MONSTER);
                        return;
                    }
                    // The protected-room flag gates the TARGETED path too
                    // (decompile cast_monster_target 43232, guilt refusal
                    // 44290-44297 — the same room+0x564 & 1 check as the
                    // bare-cast gate above, sitting ahead of the SpellImmu
                    // and cost gates). NOT spelltype-gated there, unlike
                    // its `cast_user_target` twin: guilt line, no
                    // engagement, and the same round-cost-only charging as
                    // the bare-cast guilt path.
                    if self.content.rooms.get(&room).is_some_and(|r| r.protected()) {
                        if let Some(Session::InGame { energy, .. }) = self.sessions.get_mut(&session)
                            && *energy >= round_cost
                        {
                            *energy -= round_cost;
                        }
                        self.output_line(session, text::CAST_GUILT);
                        return;
                    }
                    monster = Some(id);
                }
                Some(CastTarget::User(target_id)) => {
                    // `cast_user_target`'s gate ORDER, reproduced (the
                    // acceptance test is LAST, not first):
                    //   41422  offensive + self -> attack-yourself refusal
                    //   41429  protected room
                    //   41434  self -> divert to `cast_no_target`
                    //   41438  hostility (param_4)
                    //   41460  acceptance `{0, 2, 6, 8}`
                    if offensive {
                        // DIVERGENCE (PvP unimplemented): the DLL hands an
                        // offensive user target to `cast_user_target`'s
                        // hostility gates — the attack-yourself line at
                        // 41422 when it IS the caster, else the no-PK-room
                        // refusal, evil points, then the roll/effect tail.
                        // We have no PvP melee either, so the cast simply
                        // fails to see the player, as it did before this
                        // router landed. Reachable shape: the 45 learnable
                        // offensive match-8 spells (magic missile and
                        // friends). Placed HERE, ahead of the self-divert,
                        // because 41422 fires ahead of 41434 — the seam
                        // stays in the DLL's order for the PvP slice.
                        self.output_line(session, &text::do_not_see_here(&target));
                        return;
                    }
                    if target_id == session
                        && spell.match_type != crate::content::MatchType::Item6
                    {
                        // 41434 `if ((param_2 == param_3) && (spell+0xcc
                        // != 6)) cast_no_target(...)`: naming YOURSELF is
                        // a plain self-cast, and it diverts BEFORE the
                        // acceptance gate — so the self-only buff band
                        // (match 1: barkskin, stoneskin, magic armour,
                        // shadowform — 25 learnable) is castable by name
                        // even though 41460 rejects it. Match 6 is the one
                        // exclusion: it stays on the user path below.
                        // MEASURED §8.13: the castmsgb frames keep the
                        // name — "You cast blur on Zinvar!" — which is
                        // exactly what the self tail renders.
                        // Fall through to the self tail.
                    } else {
                        if !spell.match_type.accepts_user() {
                            // `cast_user_target` 41460 / 43064-43066,
                            // uncharged — MEASURED §8.13 for
                            // `c flash oracle` (match 12).
                            self.output_line(session, text::MAY_NOT_CAST_ON_USER);
                            return;
                        }
                        // Players in the caster's room match by the §8.9
                        // word-prefix rule (`c blur ora` -> Oracle). A
                        // match-6 self-name lands here too, by 41434's
                        // exclusion.
                        self.benign_target_cast(session, target_id, &spell);
                        return;
                    }
                }
                Some(CastTarget::Item(item_id)) => {
                    if !spell.match_type.accepts_item() {
                        // `cast_item_target` 44367-44369, uncharged.
                        self.output_line(session, text::MAY_NOT_CAST_ON_ITEM);
                        return;
                    }
                    self.fire_item_cast(session, &spell, item_id);
                    return;
                }
            }
        }
        if let Some(monster_id) = monster {
            // The pre-application eligibility scan's three refusal arms
            // (charm.md §1.1). They sit AHEAD of the SpellImmu gate in the
            // DLL (scan 43295-43376, SpellImmu 43378-43384), so a target
            // that would trip both takes this refusal — indistinguishable
            // in output, since both print 00485de3, but the order is what
            // the decompile does.
            if self.cast_eligibility_refused(monster_id, &spell) {
                let name = self.monster_name(monster_id);
                self.output_line(session, &text::spell_no_effect_on(&name));
                return;
            }
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
            // Offensive casts at passive monsters charge like melee
            // (crime.md §2.5 cast_monster_target rows).
            //
            // M7 PENDING (`re/docs/crime.md` §2.5, the 43330 row): the
            // `is_offensive()` gate is OURS, not the DLL's. 43323-43347
            // charges the same 10 points off the SPELL'S ABILITY 0x34
            // (EvilInCombat) with no `spelltype` test at all, so 25 of the
            // 29 learnable benign match-4/6/8 spells (curse, blind, slow,
            // hold person, the songs) should charge here and do not — see
            // the long note in `offensive_cast_attempt`'s fail arm.
            if spell.target_mode.is_offensive()
                && self.charge_passive_monster_evil(session, monster_id)
            {
                return;
            }
            // The engage-and-stop split (cast_monster_target 43411-43421):
            // the block is reached only when the cast is OFFENSIVE
            // (`spelltype < 3`) *and* instant (`spell+0xce == 0`). A
            // benign cast takes the 43496 else instead — one-per-round
            // flag, roll, costs, effects, all inside this command — and so
            // does an offensive DURATION spell, which resolves RIGHT NOW
            // with no engagement and no *Combat Engaged* (engage_autocombat
            // appears only in that block and in the autocombat re-fire).
            if offensive && spell.duration == 0 {
                // Engagement is then the command's ENTIRE effect
                // (MEASURED, oracle_spell_cast.raw 567-637 + decompile
                // 43439-43481): the manual offensive cast never rolls,
                // charges or fires directly — it prints the *Combat
                // Off*/*Combat Engaged* toggle, zeroes the round energy
                // and arms `casting`; the combat round driver performs
                // every actual cast. (§8.6's condensed example shows
                // engage+fire together, but the raw capture shows mana
                // UNCHANGED at the engagement prompt and the fire arriving
                // a round later — which is also why a mid-combat re-cast
                // is never blocked by the one-cast-per-round gate: for
                // offensive spells the round energy IS that gate.) Mana
                // and energy shortages are therefore not checked here
                // either; the per-round attempt handles both silently.
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
                    // DLL 43468: engagement zeroes the pool — the first
                    // fire waits for the next combat round's refill.
                    *energy = 0;
                }
                self.output_line(session, text::COMBAT_ENGAGED);
                // Retaliation lock (transcript: the filthbug swiped back
                // after the bare engagement, before any damage landed) —
                // gated like every damaging path since slice 3. This IS
                // the 43470 twin: the duration-0 engage arm that prints
                // "%s moves to cast %s upon %s", zeroes `+0xba`, calls
                // `engage_autocombat` and then locks. Charm-exempt, like
                // its two `cast_monster_target` ENTRY siblings at
                // 43260/43335 and the melee engage at 26230.
                self.retaliation_lock(monster_id, session, CharmedExemption::Exempt);
                return;
            }
            // Resolve now. The command's triple gate messages first
            // (43550-43580): round energy prints the already-cast line,
            // mana its shortfall line. DATA: no learnable spell reaches
            // the offensive-duration leg (all 65 shipped offensive
            // duration spells are monster payloads); the benign leg is
            // the 208-spell match-4/6/8 band (charm family, curse, blind,
            // slow, fear, hold person).
            let Some(Session::InGame { energy, player, .. }) = self.sessions.get(&session) else {
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
            if !offensive {
                // 43498-43509: the benign leg CONSUMES the one-per-round
                // permission bit (`user+0x700 & 4`) the way every other
                // benign cast path does. The offensive legs never touch
                // it — their gate is the round energy pool.
                if let Some(Session::InGame { cast_this_round, .. }) =
                    self.sessions.get_mut(&session)
                {
                    *cast_this_round = true;
                }
            }
            self.offensive_cast_attempt(session, spell_id, monster_id);
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
    /// `is_valid_monster_target` (decompile 38430) — the per-monster gate
    /// on the AREA sweeps, and ONLY on those: `count_valid_targets`
    /// (38610), `add_duration_spell_to_room` (38707),
    /// `add_evil_warnings_to_room` (38803) and the eight `cast_no_target`
    /// effect arms (39705..40724) call it. `cast_monster_target` never
    /// does — the single-target path's only charm awareness is the
    /// `0x800` find ORDERING ([`Core::find_monster_charmed_last`]), which
    /// is a preference and not a veto. The two gates are independent and
    /// live on disjoint call paths.
    ///
    /// The switch is on `spell+0xcc`, so the MATCH TYPE decides how much
    /// of the function runs:
    ///
    /// * 0/1/2/7 -> invalid (38461-38465); 3/5/0xb -> valid outright
    ///   (38467-38470), as does the `default` arm that would catch the
    ///   single-target 4/6/8 if they ever arrived here;
    /// * 10/0xd -> valid ONLY for your own charmed pet (38502-38509) —
    ///   the pet-command band. Unreachable: [`MatchType::hits_monsters`]
    ///   excludes both, exactly as the DLL's `{3,5,9,0xb,0xc}` sweep
    ///   guards do;
    /// * **9 and 0xc** -> the real body (38477-38499). This is the ONLY
    ///   place charm touches player-side targeting:
    ///   - uncharmed AND `+0x116 == 0` AND `mon+0x1a` == your name ->
    ///     valid at once (your grudge-holder is always fair game);
    ///   - charmed OR suppressed, AND `mon+0x1a` == your name -> INVALID.
    ///     charm.md §2.3's "hostile spells can't target your own pet",
    ///     correctly scoped: area match 9/12 only, and it covers
    ///     "friends" (suppressed, uncharmed, §2.4) on the same terms;
    ///   - else the fall-through: instance roam class 5 or 0x25 with
    ///     caster fame `player+0x542 < 0x28` -> invalid; behaviour mode
    ///     4 -> invalid; otherwise valid.
    ///
    /// The fall-through is not charm, but it is three lines of the same
    /// arm and a half-ported predicate is worse than none. ORACLE-VERIFY:
    /// it has no measured surface. Shipped reachability is real, not
    /// fixture-only — stinking cloud (131) is a learnable match-12 area.
    ///
    /// The port is an EXHAUSTIVE match, one arm per switch label, so a new
    /// [`MatchType`] is a compile error rather than a silent `true`. The
    /// earlier fail-open (`!= Area9|AreaC -> true`) contradicted this doc
    /// comment on 0/1/2/7 and defaulted the wrong way; unreachable today
    /// on either shape, since only the `{3,5,9,0xb,0xc}` area sweeps call
    /// in.
    fn is_valid_monster_target(
        &self,
        session: SessionId,
        spell: &crate::content::Spell,
        id: MonsterInstanceId,
    ) -> bool {
        use crate::content::MatchType;
        let Some(m) = self.monsters.get(&id) else {
            // `get_monster_data == 0` (38443-38445): a dead id is never a
            // target, whatever the match type.
            return false;
        };
        match spell.match_type {
            // 38461-38465 — the single-target user classes and the
            // item class are hard-invalid here.
            MatchType::Single0 | MatchType::Single1 | MatchType::Single2 | MatchType::Item7 => {
                return false;
            }
            // 38467-38473 — valid outright, no charm awareness. The
            // `default` arm (4/6/8) lands here too; it is unreachable
            // because only the area sweeps call this.
            MatchType::Area3
            | MatchType::Area5
            | MatchType::AreaB
            | MatchType::Special4
            | MatchType::Item6
            | MatchType::Special8 => return true,
            // 38502-38509 — the pet-command band: valid ONLY for your own
            // charmed pet. Dead in the DLL too (`hits_monsters` and the
            // `{3,5,9,0xb,0xc}` sweep guards both exclude 10/0xd), and
            // written out rather than left to a fail-open default.
            MatchType::Area10 | MatchType::AreaD => {
                return m.charmed && m.target == Some(session);
            }
            // 38475-38501 — the charm arm, below.
            MatchType::Area9 | MatchType::AreaC => {}
        }
        // `sameas(mon+0x1a, user+0x1e)`: the owner/grudge link is a NAME
        // in the DLL and a SessionId here (see the plan's key mapping).
        if m.target == Some(session) {
            return !(m.charmed || m.suppress);
        }
        if matches!(m.roam_class, 5 | 0x25) && self.player(session).fame < 0x28 {
            return false;
        }
        m.behaviour != 4
    }

    fn area_cast(&mut self, session: SessionId, spell: &crate::content::Spell, target: &str) {
        let room = self.player(session).location;
        // Explicit target words refuse KIND-KEYED before any cost
        // (MEASURED §8.13: `c flash oracle` -> "on a user!", `c stnk cat`
        // -> "on a monster!", both uncharged). The DLL has no separate
        // area arm here at all: `get_spell_match_type` hands the area
        // types an EMPTY find mask, the dispatcher's universal retry
        // (59265-59271) resolves the word anyway, and the entry point the
        // found kind selects refuses it — no area match type is in any of
        // the three acceptance sets. Same two-stage resolver as the
        // single-target path, therefore, minus the preferred stage.
        let words = target.trim();
        if !words.is_empty() {
            match self.find_cast_target(session, crate::content::FindScope::UNIVERSAL, words) {
                Some(CastTarget::Monster(_)) => {
                    self.output_line(session, text::MAY_NOT_CAST_ON_MONSTER);
                }
                Some(CastTarget::User(_)) => {
                    self.output_line(session, text::MAY_NOT_CAST_ON_USER);
                }
                Some(CastTarget::Item(_)) => {
                    self.output_line(session, text::MAY_NOT_CAST_ON_ITEM);
                }
                // ORACLE-VERIFY: an unmatched word was not measured on the
                // area path — the room-lookup refusal, like every other path.
                None => self.output_line(session, &text::do_not_see_here(words)),
            }
            return;
        }
        // add_evil_warnings_to_room (crime.md §2.5 last row): an offensive
        // sweep over a room holding an innocent passive monster charges
        // ONE 10-point NPC-style hit before any cost; a refusal aborts the
        // whole cast. (The per-victim 0-point PAIR timers are the PvP
        // half — slice 4 with rob.)
        if spell.target_mode.is_offensive() && spell.match_type.hits_monsters() {
            // The monster loop is MATCH-GATED (38793-38795): it runs only
            // for `spell+0xcc` in {3, 5, 9, 0xb, 0xc} — exactly
            // [`MatchType::hits_monsters`]. An offensive room-wide cast of
            // any other area type (10/0xd) charges nothing here, so the
            // gate is a conjunct and not a doc note.
            //
            // Inside it, 38802-38805 conjoins the innocence out-param with
            // `is_valid_monster_target` itself, so a body the sweep will
            // not reach is not a body you can be charged for either —
            // which on match 9/0xc silently retires the `behaviour == 4`
            // half of the innocence test (38454-38456: innocent requires
            // `+0x106` in {0, 4} and an unnamed link, but 38495 makes
            // mode 4 invalid).
            let passive = self
                .monsters
                .iter()
                .find(|(id, m)| {
                    m.location == room
                        && m.current_hp > 0
                        && matches!(m.behaviour, 0 | 4)
                        && m.target != Some(session)
                        && self.is_valid_monster_target(session, spell, **id)
                })
                .map(|(id, _)| *id);
            if let Some(id) = passive
                && self.charge_passive_monster_evil(session, id)
            {
                return;
            }
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
        // Target counting (§3 step 3): live monsters that pass
        // `is_valid_monster_target` (`count_valid_targets` 38600-38620 —
        // the same predicate the effect arms re-run per row, so counting
        // and applying can never disagree).
        let targets: Vec<MonsterInstanceId> = if spell.match_type.hits_monsters() {
            self.monsters
                .iter()
                .filter(|(id, m)| {
                    m.location == room
                        && m.current_hp > 0
                        && self.is_valid_monster_target(session, spell, **id)
                })
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
                    // Poison (19): set-if-greater, like the single-target
                    // twin (wired at the M6 slice-6 close-out; no shipped
                    // LEARNABLE area carries it — fixture-reachable only).
                    Ability::Poison => {
                        if let Some(m) = self.monsters.get_mut(&monster_id) {
                            m.poison = clamp_poison(i32::from(m.poison).max(amount));
                            m.needs_recompute = true;
                            harms = true;
                        }
                    }
                    // The remaining monster-side arms (Heal/EnergyLevel/
                    // CurePoison at a monster) have NO reachable trigger:
                    // benign-at-monster is refused at the command
                    // (MAY_NOT_CAST_ON_MONSTER) and the forced-cast route
                    // is data-gated dead — documented, not pending.
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
            // damaging sweeps — fixture-only today; the area path's
            // ability-52 evil charge is the still-open gap logged at
            // `offensive_cast_attempt`'s fail arm, 16 learnable match-12
            // carriers). The AREA damage twins (40370-40385 and 40601-40614)
            // consult the charmed bit exactly as much as the
            // single-target one does — not at all. Unlike 43752 they gate
            // on the INSTANCE roam class with no null-template clause;
            // pinned by `an_area_damage_cast_grudges_somebody_elses_pet`.
            // Reachable for OTHER players' pets only: the caster's own is
            // dropped upstream by [`Core::is_valid_monster_target`].
            self.retaliation_lock(monster_id, session, CharmedExemption::Ignored);
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
        // Summon(12) and TextBlock(148) rows collected in the instant
        // loop; executed after the session borrow drops.
        let mut summons: Vec<i32> = Vec::new();
        let mut blocks: Vec<i32> = Vec::new();
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
                    // TextBlock (148): the quest VM hook — run the
                    // block on the TARGET (cast_no_target case 0x94
                    // 41113-41114 = the caster on a self-cast;
                    // cast_user_target 42843 = the target player). The
                    // 199 shipped carriers are all instant chest/box/
                    // portal spells fired by item use. (Casting one AT
                    // a monster is a DLL no-op — cast_monster_target
                    // groups 0x94 with the silent cases, 44210-44217.)
                    Ability::TextBlock => blocks.push(amount),
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
            // Both DLL handlers that share this body spawn into the
            // CASTER's room (40044 and 42069 read the caster player
            // record), but they tag the spawn differently — and which
            // one we are in is exactly `target_id == session`:
            //
            // * self-cast = `cast_no_target` case 0xc -> the full pet
            //   triple (40048-40050);
            // * another player = `cast_user_target` case 0xc -> a bare
            //   grudge toward the TARGET (42079-42081). A hunter, not a
            //   pet: it pursues and attacks the person it was cast at.
            //
            // Tagging both `Pet(session)` would hand the caster a pet for
            // a spell the DLL uses to sic a monster ON somebody.
            let room = self.player(session).location;
            let link = if target_id == session {
                SummonLink::Pet(session)
            } else {
                SummonLink::HuntUser(target_id)
            };
            for value in summons {
                self.summon_spawn(value, room, link);
            }
        }
        // TextBlock scripts run before the success display (the DLL's
        // 0x94 arm calls the runner ahead of display_spell_success,
        // 41113-41117).
        for block in blocks {
            if let Ok(block) = u16::try_from(block) {
                self.perform_text_block_as_special_command(
                    target_id,
                    crate::content::TextBlockId(block),
                );
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
    /// Scope: benign self-cast only — no shipped EndCast chain is
    /// reachable by a player cast (48 duration spells carry EndCast 151;
    /// none is named by any LearnSp scroll). The offensive arm gained a
    /// reachable trigger with M7 slice 6 (the quest VM's `cast` verb
    /// names 17 trap spells) and carries the PENDING marker below.
    /// Returns whether the cast landed — the quest VM's `cast` verb
    /// fail-stops on a 0 return from cast_no_target (69650-69656).
    fn forced_cast(&mut self, session: SessionId, spell_id: SpellId) -> bool {
        let Some(spell) = self.content.spells.get(&spell_id).cloned() else {
            return false;
        };
        if spell.target_mode.is_offensive() {
            // M7 PENDING(slice-6): the offensive forced-cast now HAS
            // shipped triggers — the quest VM's `cast` verb names 17
            // trap spells (spelltype 0: spear/venom/fire traps etc.,
            // fired by chest/search blocks) into cast_no_target's
            // offensive machinery (39200-39500). Unported; a silent
            // no-op that keeps the block chain alive. The EndCast-chain
            // trigger remains dead (all 48 carriers unlearnable).
            return true;
        }
        let Some(Session::InGame { player, energy, .. }) = self.sessions.get(&session) else {
            return false;
        };
        // Class-school gate (39235-39239): silent refusal.
        if self.spell_gate(player, &spell) == SpellGate::WrongClass {
            return false;
        }
        let round_cost = i32::from(spell.round_cost);
        let mana_cost = i32::from(spell.mana_cost);
        // The unconditional triple gate (39253), refusal lines in the
        // decompile's order (39264-39281): round energy prints the
        // already-cast line, then mana, then level-vs-required-power.
        if *energy < round_cost {
            self.output_line(session, self.already_cast_line(session));
            return false;
        }
        if player.current_mana < mana_cost {
            self.output_line(session, self.not_enough_mana_line(session));
            return false;
        }
        if i32::from(player.level) < i32::from(spell.required_power) {
            self.output_line(session, text::SPELL_TOO_POWERFUL);
            return false;
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
        true
    }

    /// One Summon(12) row: the fixed-or-rolled value IS the template id,
    /// spawned into the given room (every apply loop passes it straight
    /// to `generate_monster`: monster single 23259, player self 40044,
    /// player-at-monster 43911, player-at-user 42069 — all four into the
    /// CASTER's room). An unknown template spawns nothing, like
    /// generate_monster's 0 return.
    ///
    /// Ownership is entirely in the state written right after the spawn,
    /// and the four sites write four different things (charm.md §6's
    /// table) — hence [`SummonLink`] rather than a nullable session.
    ///
    /// The spawn itself must stay the FIRST thing this does: `generate_monster`
    /// owns a documented draw sequence (loot, name) that the spawner
    /// goldens pin, and none of the link writes below draws at all.
    fn summon_spawn(&mut self, template: i32, room: RoomId, link: SummonLink) {
        let Ok(id) = u16::try_from(template) else {
            return;
        };
        let Some(spawned) = self.spawn_monster(crate::content::MonsterId(id), room) else {
            return;
        };
        let Some(m) = self.monsters.get_mut(&spawned) else {
            return;
        };
        match link {
            // 40048-40050: name link = caster, +0x116 = 1, +0x128 |= 1.
            SummonLink::Pet(owner) => {
                m.target = Some(owner);
                m.suppress = true;
                m.charmed = true;
            }
            // 42079-42081 / 23263-23265: name link = the VICTIM PLAYER,
            // +0x116 = 0 — an ordinary grudge, prosecuted by the pursuit
            // tier and the driver's locked branch with no charm anywhere.
            SummonLink::HuntUser(victim) => {
                m.target = Some(victim);
                m.suppress = false;
            }
            // 43919-43929: +0x140 dirty, +0x88 = victim id, +0x116 = 0,
            // and NO name link. The victim-side 10-deep back-link array
            // (`victim+0x60+i*4`, 43922-43928) is deliberately NOT
            // ported: charm.md §7 records that no reader for it was ever
            // located, and a write-only array is state we would have to
            // keep correct for nothing.
            SummonLink::HuntMonster(victim) => {
                m.hunt = Some(victim);
                m.suppress = false;
                m.needs_recompute = true;
            }
            SummonLink::None => {}
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

    /// A live monster's display name (empty if the instance is gone) —
    /// the spawn-composed adjective name, not the template's.
    fn monster_name(&self, id: MonsterInstanceId) -> String {
        self.monsters
            .get(&id)
            .map_or_else(String::new, |m| m.name.clone())
    }

    /// `cmd_set` (0x458b60) — EVIL (slice 3) and GANG (slice 7) ship;
    /// the other sixteen (keep/style/gossip/...) fall through to say
    /// until their systems exist. Subcommand matching is exact-word
    /// (ORACLE-VERIFY: DLL abbreviation behavior unmeasured).
    fn set_command(&mut self, session: SessionId, args: &str) -> Resolution {
        let (subword, rest) = split_word(args);
        if subword.eq_ignore_ascii_case("gang") {
            return self.set_gang_command(session, rest);
        }
        if !args.trim().eq_ignore_ascii_case("evil") {
            return Resolution::FallThrough;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::Handled;
        };
        player.warn_on_evil = !player.warn_on_evil;
        let (line, snapshot) = (
            if player.warn_on_evil { text::SET_EVIL_WARN_ON } else { text::SET_EVIL_WARN_OFF },
            player.clone(),
        );
        self.output_line(session, line);
        self.events.push(Event::Persist(snapshot));
        Resolution::Handled
    }

    /// The roster-view toggle (cmd_set 54136 bare / 54556 explicit):
    /// bare SET GANG flips bit 0x8; ONLINE/ALL set it; anything else
    /// prints the options line.
    fn set_gang_command(&mut self, session: SessionId, arg: &str) -> Resolution {
        let arg = arg.trim();
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::Handled;
        };
        let online_only = player.gang_flags & crate::gang::GF_ROSTER_ONLINE_ONLY != 0;
        let set_to = if arg.is_empty() {
            !online_only
        } else if arg.eq_ignore_ascii_case("online") {
            true
        } else if arg.eq_ignore_ascii_case("all") {
            false
        } else {
            self.output_line(session, text::SET_GANG_VALID);
            return Resolution::Handled;
        };
        if set_to {
            player.gang_flags |= crate::gang::GF_ROSTER_ONLINE_ONLY;
        } else {
            player.gang_flags &= !crate::gang::GF_ROSTER_ONLINE_ONLY;
        }
        let snapshot = player.clone();
        self.output_line(
            session,
            if set_to { text::SET_GANG_ONLINE } else { text::SET_GANG_ALL },
        );
        self.events.push(Event::Persist(snapshot));
        Resolution::Handled
    }

    /// The `ansi` toggle (OURS — see the Player.ansi divergence note):
    /// flip, confirm, persist.
    fn ansi_command(&mut self, session: SessionId) {
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        player.ansi = !player.ansi;
        let (line, snapshot) = (
            if player.ansi { text::ANSI_NOW_ON } else { text::ANSI_NOW_OFF },
            player.clone(),
        );
        self.output_line(session, line);
        self.events.push(Event::Persist(snapshot));
    }

    // --- gang commands (gangs.md §1, §5; M7 slice 7) ---

    /// `top [n] [gangs]` (gangs.md §5.1). The player-ranking arm is the
    /// pre-slice-7 stub. M7 PENDING: the gangs arm lands with the
    /// economy task; the player arm needs board-wide account data (M8).
    fn top_command(&mut self, session: SessionId, _args: &str) {
        self.output_line(session, text::TOP_HEADER);
    }

    /// `gang`/`guild` (cmd_broadgang 0x585cc): bare = roster, args =
    /// gangpath broadcast — both refuse without a gang (§1.6/§5.2).
    fn gang_command(&mut self, session: SessionId, message: &str) -> Resolution {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::Handled;
        };
        if player.gang.is_empty() {
            self.output_line(session, text::NOT_IN_A_GANG);
            return Resolution::Handled;
        }
        if message.trim().is_empty() {
            self.gang_roster(session);
            return Resolution::Handled;
        }
        // Gangpath broadcast (§5.2) — tell_gang, sender included.
        let sender = player.name.clone();
        let gang_display = player.gang.clone();
        let line = text::gangpath(&sender, message);
        let members: Vec<SessionId> = self
            .in_game_sessions()
            .filter(|(_, p)| p.gang.eq_ignore_ascii_case(&gang_display))
            .map(|(id, _)| id)
            .collect();
        for id in members {
            self.output_line(id, &line);
        }
        Resolution::Handled
    }

    /// The roster (§1.6). View picked by `GF_ROSTER_ONLINE_ONLY`:
    /// display_online_gang_members runs rank-ordered passes over the
    /// live terminals; display_gang_members prints the leader then
    /// scans the user db — our analog walks the offline-roster mirror,
    /// whose order is boot-scan (alphabetical) + join order rather than
    /// WCCUSERS record order (documented divergence).
    fn gang_roster(&mut self, session: SessionId) {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return;
        };
        let online_only = player.gang_flags & crate::gang::GF_ROSTER_ONLINE_ONLY != 0;
        let gang_key = player.gang.to_uppercase();
        // The DLL display fns return silently when the gang is gone.
        let Some(gang) = self.gangs.get(&gang_key) else { return };
        let display = gang.display.clone();
        let leader = gang.leader.clone();
        let count = gang.member_count;
        let disbanded = gang.is_disbanded();
        let leader_online = self
            .in_game_sessions()
            .any(|(_, p)| p.name.eq_ignore_ascii_case(&leader));
        let mut lines: Vec<String> = Vec::new();
        if online_only {
            lines.push(text::gang_roster_online_header(&display));
            lines.push(text::gang_row_leader(&leader, leader_online));
            // Two rank passes over the live sessions (0x3bf6f).
            for want_lieutenant in [true, false] {
                for (_, p) in self.in_game_sessions() {
                    if !p.gang.eq_ignore_ascii_case(&display)
                        || p.name.eq_ignore_ascii_case(&leader)
                    {
                        continue;
                    }
                    let is_lt = p.gang_flags & crate::gang::GF_LIEUTENANT != 0;
                    if is_lt == want_lieutenant {
                        lines.push(if is_lt {
                            text::gang_row_lieutenant(&p.name)
                        } else {
                            text::gang_row_member(&p.name)
                        });
                    }
                }
            }
        } else {
            lines.push(text::gang_roster_all_header(&display, count));
            if disbanded {
                lines.push(text::GANG_DISBANDED_BANNER.to_string());
            }
            lines.push(text::gang_all_row_leader(&leader, leader_online));
            let members = self.gang_members.get(&gang_key).cloned().unwrap_or_default();
            for (name, flags) in members {
                if name.eq_ignore_ascii_case(&leader) {
                    continue;
                }
                let online = self
                    .in_game_sessions()
                    .any(|(_, p)| p.name.eq_ignore_ascii_case(&name));
                let lieutenant = flags & crate::gang::GF_LIEUTENANT != 0;
                lines.push(text::gang_all_row(&name, online, lieutenant));
            }
        }
        for line in lines {
            self.output_line(session, &line);
        }
    }

    /// `create` (cmd_create 0x57d4a). Structure per the decompile:
    /// fewer than two argument words → the SAY fall-through (margc < 3);
    /// `ROOM <arg>` → the stubbed house-build path, every arm of which
    /// prints the lease line (§3.2); GANG/GUILD → the §1.1 creation
    /// gate chain; any other keyword pair is consumed silently (the
    /// decompile's no-output fall-off arm).
    fn create_command(&mut self, session: SessionId, args: &str) -> Resolution {
        let (subword, name) = split_word(args);
        if name.is_empty() {
            return Resolution::FallThrough;
        }
        if subword.eq_ignore_ascii_case("room") {
            self.output_line(session, text::GANG_HOUSE_LEASE_STUB);
            return Resolution::Handled;
        }
        if !subword.eq_ignore_ascii_case("gang") && !subword.eq_ignore_ascii_case("guild") {
            return Resolution::Handled;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::Handled;
        };
        // Gate order is the compiled order (53520-53601): experience,
        // membership, length, 'None', character set, uniqueness.
        if player.experience < 100_000 {
            self.output_line(session, text::GANG_NOT_EXPERIENCED);
            return Resolution::Handled;
        }
        if !player.gang.is_empty() {
            self.output_line(session, text::GANG_ALREADY_IN_ONE);
            return Resolution::Handled;
        }
        if name.len() >= 20 {
            let line = text::gang_name_too_long(name);
            self.output_line(session, &line);
            return Resolution::Handled;
        }
        if name.eq_ignore_ascii_case("None") {
            self.output_line(session, text::GANG_NAME_NONE);
            return Resolution::Handled;
        }
        if name.bytes().any(|b| !(0x20..=0x7e).contains(&b)) {
            self.output_line(session, text::GANG_NAME_INVALID_CHAR);
            return Resolution::Handled;
        }
        if let Some(existing) = self.gangs.get(&name.to_uppercase()) {
            let leader_line = (!existing.is_disbanded())
                .then(|| text::gang_leader_of(&existing.leader, &existing.display));
            self.output_line(session, text::GANG_NAME_IN_USE);
            if let Some(line) = leader_line {
                self.output_line(session, &line);
            }
            return Resolution::Handled;
        }
        // The DLL's insert-collision arm ("Gang already exists?  Not
        // created.") is a Btrieve check-vs-insert race with no analog
        // under the single-threaded map — not ported.
        let created = self.config.wall_base + self.scheduler.now() as i64;
        let name = name.to_string();
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return Resolution::Handled;
        };
        player.gang = name.clone();
        let member = (player.name.clone(), player.gang_flags);
        let gang = crate::gang::Gang::new(&name, &member.0, created);
        let snapshot = player.clone();
        self.gangs.insert(gang.name_key.clone(), gang.clone());
        self.gang_members
            .entry(gang.name_key.clone())
            .or_default()
            .push(member);
        self.events.push(Event::PersistGang(Box::new(gang)));
        self.events.push(Event::Persist(snapshot));
        self.output_line(session, text::GANG_CREATED);
        Resolution::Handled
    }

    /// `join` (cmd_join 0x541fb): bare prints the syntax line; `JOIN
    /// GANG <name>` runs join_gang (§1.3). Everything else — channel
    /// numbers, cmd_follow (including `JOIN GANG` with no name), and
    /// note there is NO GUILD alias here — is the unported party/channel
    /// system (M8 PENDING) and falls through.
    fn join_command(&mut self, session: SessionId, args: &str) -> Resolution {
        if args.trim().is_empty() {
            self.output_line(session, text::SYNTAX_JOIN);
            return Resolution::Handled;
        }
        let (subword, name) = split_word(args);
        if !subword.eq_ignore_ascii_case("gang") || name.is_empty() {
            return Resolution::FallThrough;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::Handled;
        };
        if !player.gang.is_empty() {
            self.output_line(session, text::GANG_JOIN_ALREADY);
            return Resolution::Handled;
        }
        let joiner_name = player.name.clone();
        let joiner_flags = player.gang_flags;
        let key = name.to_uppercase();
        if !self.gangs.contains_key(&key) {
            self.output_line(session, text::GANG_DOESNT_EXIST);
            return Resolution::Handled;
        }
        let joiner_upper = joiner_name.to_uppercase();
        let invited = self.gang_invites.contains(&(joiner_upper.clone(), key.clone()));
        if invited {
            let gang = self.gangs.get_mut(&key).expect("checked above");
            gang.member_count += 1;
            let display = gang.display.clone();
            let snapshot_gang = gang.clone();
            self.gang_members
                .entry(key.clone())
                .or_default()
                .push((joiner_name.clone(), joiner_flags));
            let snapshot = {
                let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
                    return Resolution::Handled;
                };
                player.gang = display.clone();
                player.clone()
            };
            let joined_line = text::gang_joined(&display);
            self.output_line(session, &joined_line);
            // tell_gang (0x6ae71) has no sender exclusion — the joiner
            // hears the broadcast too.
            let broadcast = text::gang_join_broadcast(&joiner_name);
            let members: Vec<SessionId> = self
                .in_game_sessions()
                .filter(|(_, p)| p.gang.eq_ignore_ascii_case(&display))
                .map(|(id, _)| id)
                .collect();
            for id in members {
                self.output_line(id, &broadcast);
            }
            self.events.push(Event::PersistGang(Box::new(snapshot_gang)));
            self.events.push(Event::Persist(snapshot));
        } else {
            self.output_line(session, text::GANG_NOT_INVITED);
        }
        // clear_gang_invitations(user, NULL): any join attempt against
        // an EXISTING gang wipes every pending invite for the user.
        self.gang_invites.retain(|(user, _)| *user != joiner_upper);
        Resolution::Handled
    }

    /// `invite` (cmd_invite 0x56420). Only the gang arm is ported:
    /// `INVITE MEMBER <name>` when the inviter is in a live gang. Plain
    /// `INVITE <name>` is the party-follow invite — M8 PENDING (parties)
    /// — and falls through like the rest of that system.
    fn invite_command(&mut self, session: SessionId, args: &str) -> Resolution {
        if args.trim().is_empty() {
            self.output_line(session, text::SYNTAX_INVITE);
            return Resolution::Handled;
        }
        let (subword, name) = split_word(args);
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::Handled;
        };
        let gang_key = player.gang.to_uppercase();
        if !subword.eq_ignore_ascii_case("member")
            || name.is_empty()
            || player.gang.is_empty()
            || !self.gangs.contains_key(&gang_key)
        {
            return Resolution::FallThrough;
        }
        let inviter_name = player.name.clone();
        let inviter_flags = player.gang_flags;
        let room = player.location;
        let gang = &self.gangs[&gang_key];
        let is_leader = gang.is_leader(&inviter_name);
        let gang_display = gang.display.clone();
        if !is_leader && inviter_flags & crate::gang::GF_LIEUTENANT == 0 {
            self.output_line(session, text::GANG_INVITE_RANK);
            return Resolution::Handled;
        }
        // find_action_target in the room, then find_any_action_target
        // anywhere online (52745-52749).
        let want = name.to_ascii_lowercase();
        let target = self
            .in_game_sessions()
            .filter(|(_, p)| p.location == room)
            .chain(self.in_game_sessions())
            .find(|(_, p)| word_prefix_match(&p.name, &want))
            .map(|(id, p)| (id, p.name.clone(), p.hidden));
        let subword = subword.to_string();
        let Some((target_id, target_name, target_hidden)) = target else {
            let line = text::dont_see_here_bang(&subword);
            self.output_line(session, &line);
            return Resolution::Handled;
        };
        if target_id == session {
            self.output_line(session, text::WHY_INVITE_YOURSELF);
            return Resolution::Handled;
        }
        let sees_hidden = self
            .ability_bag(self.player(session))
            .value(Ability::from_id(57).expect("SeeHidden in the enum"))
            > 0;
        if target_hidden && !sees_hidden {
            let line = text::dont_see_here_bang(&subword);
            self.output_line(session, &line);
            return Resolution::Handled;
        }
        // invite_to_gang dedupes (user, gang) pairs silently; the
        // inviter confirmation prints either way (52770-52785).
        let fresh = self
            .gang_invites
            .insert((target_name.to_uppercase(), gang_key));
        if fresh {
            let line = text::gang_invite_target(is_leader, &inviter_name, &gang_display);
            self.output_line(target_id, &line);
        }
        let line = text::gang_invite_confirm(&target_name);
        self.output_line(session, &line);
        Resolution::Handled
    }

    fn uninvite_command(&mut self, _session: SessionId, _name: &str) -> Resolution {
        Resolution::FallThrough // membership task
    }

    fn promote_command(&mut self, _session: SessionId, _name: &str, _promote: bool) -> Resolution {
        Resolution::FallThrough // membership task
    }

    /// `remove_from_gang` (0x4fc2f): clears membership + the Lieutenant
    /// bit, decrements the count, and — when the departer IS the leader
    /// — marks the gang disbanded and silently sweeps every ONLINE
    /// member (their notices came from the caller's tell_gang; offline
    /// members drain at login, §0 step 5). Emits the persist events.
    fn remove_from_gang(&mut self, session: SessionId) -> bool {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return false;
        };
        let name = player.name.clone();
        let gang_display = player.gang.clone();
        let key = gang_display.to_uppercase();
        self.gang_invites
            .remove(&(name.to_uppercase(), key.clone()));
        if gang_display.is_empty() || !self.gangs.contains_key(&key) {
            return false;
        }
        let snapshot = {
            let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
                return false;
            };
            player.gang.clear();
            player.gang_flags &= !crate::gang::GF_LIEUTENANT;
            player.clone()
        };
        let gang = self.gangs.get_mut(&key).expect("checked above");
        gang.member_count = gang.member_count.saturating_sub(1);
        let is_leader = gang.is_leader(&name);
        if is_leader {
            gang.flags |= crate::gang::GANG_DISBANDED;
        }
        if let Some(members) = self.gang_members.get_mut(&key) {
            members.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        }
        if is_leader {
            let swept: Vec<SessionId> = self
                .in_game_sessions()
                .filter(|(_, p)| p.gang.eq_ignore_ascii_case(&gang_display))
                .map(|(id, _)| id)
                .collect();
            for id in swept {
                let member_snapshot = {
                    let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&id)
                    else {
                        continue;
                    };
                    player.gang.clear();
                    player.gang_flags &= !crate::gang::GF_LIEUTENANT;
                    player.clone()
                };
                let gang = self.gangs.get_mut(&key).expect("checked above");
                gang.member_count = gang.member_count.saturating_sub(1);
                if let Some(members) = self.gang_members.get_mut(&key) {
                    members.retain(|(n, _)| !n.eq_ignore_ascii_case(&member_snapshot.name));
                }
                self.events.push(Event::Persist(member_snapshot));
            }
        }
        let gang_row = self.gangs[&key].clone();
        self.events.push(Event::PersistGang(Box::new(gang_row)));
        self.events.push(Event::Persist(snapshot));
        true
    }

    /// `disband` (cmd_disband 0x58725): `DISBAND GANG` (exact word, no
    /// GUILD alias) runs the leader gates and arms the 0x88 yes/no
    /// continuation; `DISBAND PARTY` is the unported group system (M8
    /// PENDING — the DLL stop_following is consumed silently); anything
    /// else prints the syntax line.
    fn disband_command(&mut self, session: SessionId, args: &str) -> Resolution {
        let args = args.trim();
        if args.eq_ignore_ascii_case("party") {
            return Resolution::Handled;
        }
        if !args.eq_ignore_ascii_case("gang") {
            self.output_line(session, text::SYNTAX_DISBAND);
            return Resolution::Handled;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::Handled;
        };
        if player.gang.is_empty() {
            self.output_line(session, text::GANG_NOT_IN_BANG);
            return Resolution::Handled;
        }
        let name = player.name.clone();
        let key = player.gang.to_uppercase();
        let Some(gang) = self.gangs.get(&key) else {
            // The internal_error arm: clear the dangling membership.
            if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                player.gang.clear();
            }
            return Resolution::Handled;
        };
        if !gang.is_leader(&name) {
            self.output_line(session, text::GANG_NOT_THE_LEADER_DISBAND);
            return Resolution::Handled;
        }
        let line = text::gang_disband_confirm(&gang.display.clone());
        self.output_line(session, &line);
        self.pending_disband.insert(session);
        Resolution::Handled
    }

    /// The 0x88 continuation (decompile 3741): one word starting with Y
    /// disbands — two tell_gang lines to the still-intact membership,
    /// then the remove_from_gang sweep; anything else declines.
    fn disband_confirm(&mut self, session: SessionId, line: &str) {
        let mut words = line.split_whitespace();
        let yes = matches!(
            (words.next(), words.next()),
            (Some(w), None) if w.to_ascii_uppercase().starts_with('Y')
        );
        let gang_display = self.player(session).gang.clone();
        let key = gang_display.to_uppercase();
        if !yes || gang_display.is_empty() || !self.gangs.contains_key(&key) {
            self.output_line(session, text::GANG_NOT_DISBANDED);
            return;
        }
        let disbanded_line = text::gang_disbanded(&self.gangs[&key].display.clone());
        let members: Vec<SessionId> = self
            .in_game_sessions()
            .filter(|(_, p)| p.gang.eq_ignore_ascii_case(&gang_display))
            .map(|(id, _)| id)
            .collect();
        for id in members {
            self.output_line(id, &disbanded_line);
            self.output_line(id, text::GANG_NAME_LOCKED);
        }
        self.remove_from_gang(session);
    }

    /// `leave` (cmd_leave 0x53cba): only the exact `LEAVE GANG` form is
    /// the gang system; bare LEAVE and group forms are the unported
    /// party system (M8) and fall through.
    fn leave_command(&mut self, session: SessionId, args: &str) -> Resolution {
        if !args.trim().eq_ignore_ascii_case("gang") {
            return Resolution::FallThrough;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return Resolution::Handled;
        };
        if player.gang.is_empty() {
            self.output_line(session, text::GANG_NOT_CURRENTLY_IN);
            return Resolution::Handled;
        }
        let name = player.name.clone();
        let gang_display = player.gang.clone();
        let room = player.location;
        let key = gang_display.to_uppercase();
        let is_leader = self
            .gangs
            .get(&key)
            .is_none_or(|g| g.is_leader(&name));
        if is_leader {
            // A dangling gang record takes this arm too (0x53cba: the
            // null-gang check shares the leader refusal).
            self.output_line(session, text::GANG_LEADER_MAY_NOT_LEAVE);
            return Resolution::Handled;
        }
        let room_line = text::gang_left_room(&name, &gang_display);
        self.broadcast_to_room_except(room, &[session], &room_line);
        let self_line = text::gang_left(&gang_display);
        self.output_line(session, &self_line);
        self.remove_from_gang(session);
        Resolution::Handled
    }

    fn stock_command(&mut self, _session: SessionId, _args: &str) -> Resolution {
        Resolution::FallThrough // guild-house task
    }

    fn unstock_command(&mut self, _session: SessionId, _args: &str) -> Resolution {
        Resolution::FallThrough // guild-house task
    }

    fn markup_command(&mut self, _session: SessionId, _args: &str) -> Resolution {
        Resolution::FallThrough // guild-house task
    }

    /// Test/inspection: the runtime hidden byte (`+0x5f6`).
    pub fn player_hidden(&self, session: SessionId) -> bool {
        matches!(self.sessions.get(&session),
            Some(Session::InGame { player, .. }) if player.hidden)
    }

    /// The §11.3 chance inputs for the acting player.
    fn stealth_chance_for(&self, session: SessionId) -> i32 {
        let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&session) else {
            return 0;
        };
        let room = player.location;
        let others = self
            .in_game_sessions()
            .filter(|(id, p)| *id != session && p.location == room)
            .count() as i32;
        let monsters = self
            .monsters
            .values()
            .filter(|m| m.location == room && m.current_hp > 0)
            .count() as i32;
        crate::stats::stealth_chance(
            derived.stealth,
            self.encumbrance_percent(session),
            others,
            monsters,
            95,
        )
    }

    /// `cmd_sneak` (theft.md §11.1): gate on being fought, then
    /// PerStealth auto-success or the §11.3 roll. Success is SILENT —
    /// the sneak-armed bit simply waits for the next move. Failure
    /// self-doubt is perception-gated. (The add_delay gates join with
    /// the slice-wide delay system.)
    fn sneak_command(&mut self, session: SessionId) {
        // UNPORTED (not slice-scoped — it lands with its first consumer,
        // whichever slice that turns out to be): `can_sneak`'s third
        // gate, `monster_could_attack` (18209, called at 65462). Its pet
        // exemption (18237-18240: a threat is a monster that is NOT
        // charmed-and-named-yours and has `+0x116 == 0`) is charm.md §2.3
        // material and comes with it — M7 slice 5 deliberately built no
        // speculative plumbing for a predicate with no caller (YAGNI).
        // FOUR callers in the DLL, all still unported: `can_sneak` 65462,
        // `cmd_hide` 62023, `cmd_close` 52341 and `cmd_lock` 53290. The
        // last three carry the identical four-term guard
        // (`is_inside_autocombat` == 0, `is_being_attacked` == 0,
        // `+0x6f0 < 1`, `monster_could_attack` == 0); `can_sneak` is the
        // odd one out (its own attacker-type/same-room pre-test, then
        // `+0x6f0 < 1` and the call).
        let being_fought = self
            .monsters
            .values()
            .any(|m| m.target == Some(session) && m.current_hp > 0
                && m.location == self.player(session).location);
        let engaged = self.attackers_of(session) >= 1;
        if being_fought || engaged {
            self.output_line(session, text::MAY_NOT_SNEAK);
            self.add_delay(session, 1);
            return;
        }
        if self.delay_blocked(session) {
            return;
        }
        self.add_delay(session, 1);
        self.output_line(session, "Attempting to sneak...");
        let auto = self
            .ability_bag(self.player(session))
            .value(Ability::from_id(0xba).expect("PerStealth in the enum"))
            > 0;
        let success = auto || {
            let chance = self.stealth_chance_for(session);
            self.rng.roll(0, 100) < chance
        };
        if success {
            if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                player.sneak_armed = true;
            }
            return; // silent — the player is never told sneaking worked
        }
        let perception = match self.sessions.get(&session) {
            Some(Session::InGame { derived, .. }) => derived.perception,
            _ => 0,
        };
        if self.rng.roll(0, 100) < perception {
            self.output_line(session, "You don't think you're sneaking.");
        }
    }

    /// `cmd_hide` with no argument (theft.md §11.2): the self-hide.
    /// No PerStealth shortcut here, unlike SNEAK.
    fn hide_command(&mut self, session: SessionId) {
        // Same unported `monster_could_attack` gate as `sneak_command`
        // (the 62023 caller) — see the note there for the full
        // four-caller inventory and the pet exemption that rides along.
        let being_fought = self
            .monsters
            .values()
            .any(|m| m.target == Some(session) && m.current_hp > 0
                && m.location == self.player(session).location);
        let engaged = self.attackers_of(session) >= 1;
        if being_fought || engaged {
            // Unconditional fake failure while being fought (§11.2).
            self.output_line(session, "Attempting to hide...");
            self.output_line(session, " You don't think you are hidden.");
            self.add_delay(session, 1);
            return;
        }
        if self.delay_blocked(session) {
            return;
        }
        self.add_delay(session, 1);
        self.output_line(session, "Attempting to hide...");
        let chance = self.stealth_chance_for(session);
        if self.rng.roll(0, 100) < chance {
            if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                player.hidden = true;
            }
            return; // silent success
        }
        let perception = match self.sessions.get(&session) {
            Some(Session::InGame { derived, .. }) => derived.perception,
            _ => 0,
        };
        if self.rng.roll(0, 100) < perception {
            self.output_line(session, " You don't think you are hidden.");
        }
    }





    /// `cmd_disarm` (theft.md §10): DISARM TRAP <direction>. A missing
    /// or bad direction is a SILENT return (the DLL's `return 1`).
    /// Mechanical (type 9) traps only; 0x18 spell traps await the
    /// room-cast plumbing (PENDING).
    fn disarm_command(&mut self, session: SessionId, args: &str) {
        let words: Vec<&str> = args.split_whitespace().collect();
        let Some(dir) = words.get(1).and_then(|w| direction_from_word(&w.to_ascii_lowercase()))
        else {
            return; // silent, per the DLL
        };
        let room = self.player(session).location;
        let d = dir as usize as u8;
        let fail = format!(
            "You failed to disarm any trap to the {}.",
            text::direction_shown(dir)
        );
        let exit = self
            .content
            .rooms
            .get(&room)
            .and_then(|r| r.exits[dir as usize].clone());
        let Some(exit) = exit.filter(|e| e.exit_type == 9) else {
            self.output_line(session, &fail);
            return;
        };
        // Trap state shares the 0x39c word (the lock overlay): 0/3
        // armed, 1/4 disarmed.
        let state = *self.exit_locks.get(&(room, d)).unwrap_or(&exit.param2);
        if !matches!(state, 0 | 3) {
            self.output_line(session, &fail);
            return;
        }
        let skill = match self.sessions.get(&session) {
            Some(Session::InGame { derived, .. }) => derived.disarm_traps,
            _ => 0,
        };
        let roll = self.rng.roll(0, 100);
        if roll < skill {
            self.output_line(
                session,
                &format!(
                    "You successfully disarmed the trap to the {}.",
                    text::direction_shown(dir)
                ),
            );
            self.exit_locks.insert((room, d), if state == 3 { 4 } else { 1 });
            self.scheduler.schedule_in(300, Job::ExitRelock(room, d));
            return;
        }
        if roll < skill + 10 {
            self.output_line(session, &fail); // near miss — safe
            return;
        }
        // Triggered: the message record (user line 1, room line 2 with
        // the name bound), then the consequence.
        let name = self.player(session).name.clone();
        if exit.param4 > 0
            && let Ok(id) = u16::try_from(exit.param4)
            && let Some(msg) = self.content.messages.get(&crate::content::MessageId(id))
        {
            let user_line = msg.lines.first().cloned().unwrap_or_default();
            let room_line = msg
                .lines
                .get(1)
                .map(|l| l.replacen("%s", &name, 1))
                .unwrap_or_default();
            if !user_line.is_empty() {
                self.output_line(session, &user_line);
            }
            if !room_line.is_empty() {
                self.broadcast_to_room(room, Some(session), &room_line);
            }
        }
        if state == 3 {
            // Trapdoor: you fall through (move_user mode 7 — modeled as
            // a forced relocation; the mode-7 spell arm is PENDING).
            if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                player.location = exit.dest;
            }
            self.show_room(session);
            return;
        }
        let rating = exit.param.max(1);
        let dmg = self.rng.roll(rating / 2, rating + 1).max(0);
        let dropped;
        {
            let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
                return;
            };
            let was_up = player.current_hp > 0;
            player.current_hp -= dmg;
            dropped = was_up && player.current_hp < 0;
        }
        if dropped {
            let line = text::drops_to_ground(&name);
            self.output_line(session, &line);
            self.broadcast_to_room(room, Some(session), &line);
        }
        if self.player(session).current_hp <= DEATH_FLOOR {
            self.player_killed(session);
        }
    }

    /// `cmd_search` / `search_for_hidden_exits` (theft.md §9). Bare form
    /// re-lists the room and broadcasts "searching the area"; a
    /// directional search broadcasts "searching for exits" then reveals
    /// a trap (type 9, FindTraps roll — no state change) or reports
    /// nothing. Hidden type-6 exit reveal rides the DISARM/trap-state
    /// pass. Non-directions are refused.
    fn search_command(&mut self, session: SessionId, args: &str) {
        self.add_delay(session, 1);
        let word = args.trim().to_ascii_lowercase();
        if word.is_empty() {
            let room = self.player(session).location;
            let name = self.player(session).name.clone();
            self.broadcast_to_room(
                room,
                Some(session),
                &format!("{name} is searching the area."),
            );
            self.show_room_brief(session);
            // The hidden stash surfaces to a searcher (theft.md §11.2;
            // presentation ORACLE-VERIFY — the notice-line frame reused).
            let stash: Vec<String> = self
                .room_hidden_items
                .get(&room)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|(id, _)| self.content.items.get(id))
                        .map(|i| i.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let mut lines: Vec<String> = stash;
            if let Some(pool) = self.room_hidden_coins.get(&room) {
                for (idx, n) in pool.iter().enumerate() {
                    if *n > 0 {
                        lines.push(format!("{n} {}", text::currency_name(idx)));
                    }
                }
            }
            if !lines.is_empty() {
                self.output_line(
                    session,
                    &format!("You notice {} here.", lines.join(", ")),
                );
            }
            return;
        }
        let Some(dir) = direction_from_word(&word) else {
            self.output_line(session, text::SEARCH_WHY);
            return;
        };
        let room = self.player(session).location;
        let name = self.player(session).name.clone();
        self.broadcast_to_room(
            room,
            Some(session),
            &format!("{name} is searching for exits."),
        );
        let nothing = match dir {
            Direction::Up => "You notice nothing different above you.".to_string(),
            Direction::Down => "You notice nothing different below you.".to_string(),
            d => format!("You notice nothing different to the {}.", text::direction_shown(d)),
        };
        let exit = self
            .content
            .rooms
            .get(&room)
            .and_then(|r| r.exits[dir as usize].clone());
        let d = dir as usize as u8;
        // Hidden type-6 exits (state & 2): roll < max(Perception-15, 3)
        // reveals — state 4, plus the ~5 min re-hide kick.
        if let Some(hexit) = exit.clone().filter(|e| e.exit_type == 6) {
            let state = *self.exit_locks.get(&(room, d)).unwrap_or(&hexit.param);
            if state & 2 != 0 {
                let perception = match self.sessions.get(&session) {
                    Some(Session::InGame { derived, .. }) => derived.perception,
                    _ => 0,
                };
                if self.rng.roll(0, 100) < (perception - 15).max(3) {
                    self.exit_locks.insert((room, d), 4);
                    self.scheduler.schedule_in(300, Job::ExitRelock(room, d));
                    let line = match dir {
                        Direction::Up => "You found an exit upwards!".to_string(),
                        Direction::Down => "You found an exit downwards!".to_string(),
                        d => format!("You found an exit to the {}!", text::direction_shown(d)),
                    };
                    self.output_line(session, &line);
                    return;
                }
            }
            self.output_line(session, &nothing);
            return;
        }
        let Some(exit) = exit.filter(|e| e.exit_type == 9) else {
            self.output_line(session, &nothing);
            return;
        };
        let find_traps = match self.sessions.get(&session) {
            Some(Session::InGame { derived, .. }) => derived.find_traps,
            _ => 0,
        };
        if self.rng.roll(0, 100) < find_traps {
            let line = match dir {
                Direction::Up => "You found a trap above you!".to_string(),
                Direction::Down => "You found a trap below you!".to_string(),
                d => format!("You found a trap to the {}!", text::direction_shown(d)),
            };
            let _ = exit; // finding changes no state (§9)
            self.output_line(session, &line);
        } else {
            self.output_line(session, &nothing);
        }
    }

    /// `add_delay`: extend the session's command delay.
    fn add_delay(&mut self, session: SessionId, units: u8) {
        if let Some(Session::InGame { delay, .. }) = self.sessions.get_mut(&session) {
            *delay = delay.saturating_add(units);
        }
    }

    /// The SNEAK/HIDE delay gate (theft.md §11): refuse while units
    /// remain. Returns true when blocked.
    fn delay_blocked(&mut self, session: SessionId) -> bool {
        let waiting = matches!(self.sessions.get(&session),
            Some(Session::InGame { delay, .. }) if *delay > 0);
        if waiting {
            self.output_line(session, text::MUST_WAIT);
        }
        waiting
    }

    /// Hidden type-6 exit check (theft.md §9/§8.6): found state 4
    /// (search) or 8 (the remoteaction lever-reveal, 66059) in the
    /// overlay (else the disk para1) reveals it; everything else —
    /// state 2, partial lever bit-words, and the re-hidden ticker codes
    /// — stays concealed. Ticker semantics UNDETERMINED beyond
    /// concealment.
    fn exit_hidden6(&self, room: RoomId, d: u8, exit: &crate::content::Exit) -> bool {
        exit.exit_type == 6
            && !matches!(*self.exit_locks.get(&(room, d)).unwrap_or(&exit.param), 4 | 8)
    }

    /// The effective lock state for a pickable exit (theft.md §8.1): the
    /// runtime overlay, else the shipped disk word — type 2 keeps it in
    /// para2 (0x39c), types 7/0xb in para1 (0x374). 2 = locked.
    fn exit_lock_state(&self, room: RoomId, d: u8, exit: &crate::content::Exit) -> i32 {
        if let Some(state) = self.exit_locks.get(&(room, d)) {
            return *state;
        }
        match exit.exit_type {
            2 => exit.param2,
            7 | 0xb => exit.param,
            _ => 0,
        }
    }

    /// `cmd_picklock` (theft.md §8). One shared fail string masks every
    /// refusal ("no such exit", wrong type, already open, skill-less,
    /// and the failed roll alike).
    fn picklock_command(&mut self, session: SessionId, args: &str) {
        let word = args.trim().to_ascii_lowercase();
        let dir = direction_from_word(&word);
        let Some(dir) = dir else {
            self.output_line(session, text::SYNTAX_PICKLOCK);
            return;
        };
        self.break_combat(session);
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.sneak_armed = false;
            player.hidden = false;
        }
        let room = self.player(session).location;
        let d = dir as usize as u8;
        let exit = self
            .content
            .rooms
            .get(&room)
            .and_then(|r| r.exits[dir as usize].clone());
        let fail = text::PICK_FAILS;
        let Some(exit) = exit else {
            self.output_line(session, fail);
            return;
        };
        self.add_delay(session, 2);
        let pickable = matches!(exit.exit_type, 2 | 7 | 0xb);
        if !pickable || self.exit_lock_state(room, d, &exit) != 2 {
            self.output_line(session, fail);
            return;
        }
        let skill = match self.sessions.get(&session) {
            Some(Session::InGame { derived, .. }) => derived.picklocks,
            _ => 0,
        };
        // Modifier + roll (§8.2/8.3): type 2 keeps its modifier in
        // para3; 7/0xb in para2 (typically negative — hard locks).
        let modifier = if exit.exit_type == 2 { exit.param3 } else { exit.param2 };
        let success = skill >= 1 && self.rng.roll(0, 100) < modifier + skill;
        if !success {
            self.add_delay(session, 2); // the second charge (§8.2)
            // §8.4: a failed 7/0xb pick fires the room lock-trap spell
            // (room+0x5fa) — no sqlite column is pinned for it yet
            // (PENDING with the trap pass), so the fail line prints.
            self.output_line(session, fail);
            return;
        }
        self.exit_locks.insert((room, d), 1);
        // Re-lock (§8.6): 300 s per unit; type 7/0xb locks with a
        // POSITIVE pick modifier never re-lock.
        let delay_units = if exit.exit_type == 2 { exit.param4 } else { exit.param3 };
        let relocks = exit.exit_type == 2 || modifier < 1;
        if relocks {
            let secs = 300 * i64::from(delay_units.max(1));
            self.scheduler
                .schedule_in(secs as u64, Job::ExitRelock(room, d));
        }
        // Reciprocal exit (§8.2): the destination's opposite unlocks on
        // its own timer.
        let opposite = dir.opposite();
        if let Some(back) = self
            .content
            .rooms
            .get(&exit.dest)
            .and_then(|r| r.exits[opposite as usize].clone())
            .filter(|b| b.dest == room && matches!(b.exit_type, 2 | 7 | 0xb))
        {
            let bd = opposite as usize as u8;
            self.exit_locks.insert((exit.dest, bd), 1);
            let bmod = if back.exit_type == 2 { back.param3 } else { back.param2 };
            let bdelay = if back.exit_type == 2 { back.param4 } else { back.param3 };
            if back.exit_type == 2 || bmod < 1 {
                let secs = 300 * i64::from(bdelay.max(1));
                self.scheduler
                    .schedule_in(secs as u64, Job::ExitRelock(exit.dest, bd));
            }
        }
        // §8.5: room first, then the picker.
        let name = self.player(session).name.clone();
        let leaf = if exit.exit_type == 0xb { "gate" } else { "door" };
        let line = match dir {
            Direction::Up => format!("You see {name} pick the lock on the {leaf} above you."),
            Direction::Down => format!("You see {name} pick the lock on the {leaf} below you."),
            d => format!(
                "You see {name} pick the lock on the {leaf} to the {}.",
                text::direction_shown(d)
            ),
        };
        self.broadcast_to_room(room, Some(session), &line);
        self.output_line(session, &format!("You successfully unlocked the {leaf}."));
    }

    /// One re-lock kick (theft.md §8.6): back to locked with the room
    /// broadcast, unless someone already re-locked it.
    fn relock_exit(&mut self, room: RoomId, d: u8) {
        let Some(exit) = self
            .content
            .rooms
            .get(&room)
            .and_then(|r| r.exits[d as usize].clone())
        else {
            return;
        };
        // Hidden exits re-hide to their shipped state (§8.6 type 6).
        if exit.exit_type == 6 {
            self.exit_locks.insert((room, d), exit.param);
            return;
        }
        // Traps re-arm silently (§8.6: 0x39c 1->0, 4->3).
        if matches!(exit.exit_type, 9 | 0x18) {
            let state = *self.exit_locks.get(&(room, d)).unwrap_or(&exit.param2);
            let rearmed = match state {
                1 => 0,
                4 => 3,
                other => other,
            };
            self.exit_locks.insert((room, d), rearmed);
            return;
        }
        if self.exit_lock_state(room, d, &exit) == 2 {
            return; // already locked again
        }
        self.exit_locks.insert((room, d), 2);
        let leaf = if exit.exit_type == 0xb { "gate" } else { "door" };
        let dir = crate::content::Direction::ALL[d as usize];
        let line = format!(
            "The {leaf} to the {} just locked!",
            text::direction_shown(dir)
        );
        self.broadcast_to_room(room, None, &line);
    }

    /// `cmd_rob` (theft.md §3): parse, resolve player-or-monster, then
    /// `rob_user`/`rob_monster`. Bare form prints the syntax; unmatched
    /// targets the don't-see line. (find_action_target kinds 4/8/0x10 —
    /// items — have no reachable surface here yet.)
    fn rob_command(&mut self, session: SessionId, target_words: &str) {
        let want = target_words.trim().to_ascii_lowercase();
        if want.is_empty() {
            self.output_line(session, text::SYNTAX_ROB);
            return;
        }
        // §3 kind-1 entry clears the robber's own stealth state.
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.sneak_armed = false;
            player.hidden = false;
        }
        let room = self.player(session).location;
        let sees_hidden = self
            .ability_bag(self.player(session))
            .value(Ability::from_id(57).expect("SeeHidden in the enum"))
            > 0;
        let target = self
            .in_game_sessions()
            .filter(|(_, p)| p.location == room)
            .find(|(_, p)| word_prefix_match(&p.name, &want))
            .map(|(id, p)| (id, p.hidden));
        if let Some((victim, victim_hidden)) = target {
            if victim_hidden && !sees_hidden && victim != session {
                self.output_line(session, text::DONT_SEE_ANYWHERE);
                return;
            }
            self.add_delay(session, 1);
            self.rob_user(session, victim);
            return;
        }
        if self.find_monster(room, &want).is_some() {
            // rob_monster (theft.md §6): a no-op — its only crime check
            // is the committed-Lawful refusal; otherwise SILENT.
            let lawful = self.player(session).lawful;
            if lawful {
                self.output_line(session, text::ROB_WAY_OF_LIFE);
            }
            return;
        }
        self.output_line(session, text::DONT_SEE_ANYWHERE);
    }

    /// `FUN_0046c417` (crime.md §6.5) — the PvP-range gate.
    fn pvp_in_range(&self, a: SessionId, b: SessionId) -> bool {
        if self.config.pvp_level_range < 0 {
            return false;
        }
        let (al, bl) = (
            i32::from(self.player(a).level),
            i32::from(self.player(b).level),
        );
        if al < 4 || bl < 4 {
            return false;
        }
        let (an, bn) = (self.player(a).name.clone(), self.player(b).name.clone());
        if self
            .evil_timers
            .iter()
            .any(|n| n.attacker.eq_ignore_ascii_case(&an) && n.victim.eq_ignore_ascii_case(&bn))
        {
            return true; // a live pair node bypasses balance
        }
        if crate::crime::legal_level(self.player(b).fame) == crate::crime::LegalLevel::Fiend {
            return true;
        }
        (al - bl).abs() <= self.config.pvp_level_range
    }

    /// `rob_user` (theft.md §4): gates, the Thievery roll, the evil
    /// charge, then the quiet loot transfer. Only the BUMP outcome ever
    /// reaches the victim; the room hears nothing.
    fn rob_user(&mut self, robber: SessionId, victim: SessionId) {
        let p = self.player(robber);
        // §4.1 gate 1: Lawful OR evil-warnings — one shared refusal.
        if p.lawful || p.warn_on_evil {
            self.output_line(robber, text::ROB_WAY_OF_LIFE);
            return;
        }
        if robber == victim {
            self.output_line(robber, text::ROB_YOURSELF);
            return;
        }
        if !self.pvp_in_range(robber, victim) {
            self.output_line(robber, text::ROB_UNBALANCED);
            return;
        }
        let room = self.player(robber).location;
        let safe = self
            .content
            .rooms
            .get(&room)
            .is_some_and(|r| r.protected() || r.room_type == 5);
        if safe {
            self.output_line(robber, text::ROB_GUILT);
            return;
        }
        let thievery = match self.sessions.get(&robber) {
            Some(Session::InGame { derived, .. }) => derived.thievery,
            _ => 0,
        };
        let victim_name = self.player(victim).name.clone();
        let robber_name = self.player(robber).name.clone();
        let victim_gender = self.player(victim).gender;
        let robber_gender = self.player(robber).gender;
        // Draw 1: the skill d100.
        let roll = self.rng.roll(1, 100);
        if roll > thievery + 10 {
            // Detected — the only outcome the victim ever sees.
            if self.charge_player_evil(robber, victim, 1, 1) {
                return;
            }
            self.output_line(
                robber,
                &format!(
                    "You bump {victim_name} as you try to rob {}.",
                    text::pronoun_object(victim_gender)
                ),
            );
            self.output_line(
                victim,
                &format!(
                    "{robber_name} bumps you as {} tries to rob you!",
                    text::pronoun_subject(robber_gender)
                ),
            );
            return;
        }
        if roll > thievery {
            if self.charge_player_evil(robber, victim, 1, 2) {
                return;
            }
            self.output_line(
                robber,
                &format!("Your skills fail as you try to rob {victim_name}."),
            );
            return;
        }
        // Success path: the charge lands BEFORE the loot draws (§4.2).
        if self.charge_player_evil(robber, victim, 1, 2) {
            return;
        }
        // Draw 2: coins vs items.
        if self.rng.roll(1, 100) < 50 {
            // Draw 3: the currency index — even one the victim lacks.
            let idx = self.rng.roll(0, 4).clamp(0, 4) as usize;
            let held = {
                let v = &self.player(victim).coins;
                match idx {
                    0 => v.copper,
                    1 => v.silver,
                    2 => v.gold,
                    3 => v.platinum,
                    _ => v.runic,
                }
            };
            let amount = if held > 0 {
                self.rng.roll(0, held.min(i32::MAX as u32) as i32).max(0) as u32
            } else {
                0
            };
            if amount == 0 {
                self.output_line(
                    robber,
                    &format!("Your skills fail as you try to rob {victim_name}."),
                );
                return;
            }
            let take = |c: &mut Coins, idx: usize, n: u32, add: bool| {
                let slot = match idx {
                    0 => &mut c.copper,
                    1 => &mut c.silver,
                    2 => &mut c.gold,
                    3 => &mut c.platinum,
                    _ => &mut c.runic,
                };
                if add {
                    *slot += n;
                } else {
                    *slot -= n;
                }
            };
            if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&victim) {
                take(&mut player.coins, idx, amount, false);
            }
            if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&robber) {
                take(&mut player.coins, idx, amount, true);
            }
            self.output_line(
                robber,
                &format!(
                    "You stole {amount} {} from {victim_name}.",
                    text::currency_name(idx)
                ),
            );
            return;
        }
        // Item path (§4.3): one d100 per occupied inventory slot; the
        // LAST sub-50 hit is the candidate, selected only when the item
        // carries LoyalItem (100). (The 50-slot key ring has no model
        // here yet — keys live in the inventory; PENDING with the key
        // system.)
        let inventory = self.player(victim).inventory.clone();
        let mut candidate: Option<usize> = None;
        let mut selected = false;
        for (i, (item_id, _)) in inventory.iter().enumerate() {
            if self.rng.roll(1, 100) < 50 {
                candidate = Some(i);
                selected = self
                    .content
                    .items
                    .get(item_id)
                    .is_some_and(|it| {
                        it.abilities.iter().any(|(a, _)| {
                            Ability::from_id(100).is_some_and(|l| *a == l)
                        })
                    });
            }
        }
        let fail = format!("Your skills fail as you try to rob {victim_name}.");
        let Some(slot) = candidate.filter(|_| selected) else {
            self.output_line(robber, &fail);
            return;
        };
        let (item_id, uses) = inventory[slot];
        // §4.5: the Robable byte gates the transfer. (The DLL's
        // only-copy-equipped rule is unreachable here: our worn gear
        // lives outside the inventory vec.)
        let (robable, item_name) = self
            .content
            .items
            .get(&item_id)
            .map(|i| (i.robable != 0, i.name.clone()))
            .unwrap_or((false, String::new()));
        if !robable {
            self.output_line(robber, &fail);
            return;
        }
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&victim) {
            player.inventory.remove(slot);
        }
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&robber) {
            player.inventory.push((item_id, uses));
        }
        self.output_line(
            robber,
            &format!("You successfully stole {item_name} from {victim_name}."),
        );
    }

    /// The player-victim evil charge (crime.md §2.3-2.4): gate, the
    /// innocence gate, victim-quality multiplier, minimum-10 bump, and
    /// the pair-timer bank (rob mode replaces an existing node).
    /// Returns true when the action is REFUSED.
    fn charge_player_evil(
        &mut self,
        robber: SessionId,
        victim: SessionId,
        base_points: i32,
        rob_mode: u8,
    ) -> bool {
        let (victim_fame, victim_lawful, victim_name) = {
            let v = self.player(victim);
            (v.fame, v.lawful, v.name.clone())
        };
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&robber) else {
            return true;
        };
        if player.warn_on_evil {
            self.output_line(robber, crate::crime::WARN_ON_EVIL_REFUSAL);
            return true;
        }
        if player.fame > 300 {
            self.output_line(robber, crate::crime::TOO_EVIL_REFUSAL);
            return true;
        }
        if player.lawful {
            self.output_line(robber, crate::crime::LAWFUL_REFUSAL);
            return true;
        }
        // Innocence gate: only Neutral-band victims yield points; the
        // timer is banked either way.
        let mut points = if victim_fame < 0x1e {
            base_points * crate::crime::victim_multiplier(victim_fame, victim_lawful)
        } else {
            0
        };
        let fame_now = i32::from(player.fame);
        if fame_now < 0 && fame_now + points < 10 {
            points = 10 - fame_now;
        }
        let before = crate::crime::legal_level(player.fame);
        if points > 0 && fame_now < 30000 {
            player.fame = (fame_now + points)
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
        let robber_name = player.name.clone();
        let crossed = crate::crime::legal_level(player.fame) != before;
        let snapshot = player.clone();
        self.output_line(robber, crate::crime::DARK_CLOUD);
        if crossed {
            self.update_allowed_worn_items(robber);
        }
        self.events.push(Event::Persist(snapshot));
        // Bank/replace the pair timer (§2.3 rob path, §4.1 node shape).
        let flags = if rob_mode == 2 { 3 } else { 1 };
        self.evil_timers.retain(|n| {
            !(n.attacker.eq_ignore_ascii_case(&robber_name)
                && n.victim.eq_ignore_ascii_case(&victim_name))
        });
        self.evil_timers.push(crate::crime::EvilNode {
            attacker: robber_name,
            victim: victim_name,
            rounds: 11,
            points,
            rob_flags: flags,
        });
        false
    }

    /// `cmd_forgive` + `attempt_to_forgive` (theft.md §5): the wronged
    /// party refunds a present criminal's banked points. Our list walk
    /// unlinks the matched node — the DLL's frees the PREDECESSOR (a
    /// use-after-free) and is deliberately not cloned.
    fn forgive_command(&mut self, session: SessionId, target_words: &str) {
        let want = target_words.trim().to_ascii_lowercase();
        let room = self.player(session).location;
        let target = self
            .in_game_sessions()
            .filter(|(_, p)| p.location == room)
            .find(|(_, p)| word_prefix_match(&p.name, &want))
            .map(|(id, _)| id);
        let Some(criminal) = target else {
            self.output_line(
                session,
                &format!("You do not see {} here!", target_words.trim()),
            );
            return;
        };
        let criminal_name = self.player(criminal).name.clone();
        let criminal_gender = self.player(criminal).gender;
        let my_name = self.player(session).name.clone();
        let node_idx = self.evil_timers.iter().position(|n| {
            n.attacker.eq_ignore_ascii_case(&criminal_name)
                && n.victim.eq_ignore_ascii_case(&my_name)
        });
        let Some(idx) = node_idx else {
            self.output_line(
                session,
                &format!(
                    "The gods refuse to forgive {criminal_name} for {} actions.",
                    text::pronoun_possessive(criminal_gender)
                ),
            );
            return;
        };
        let node = self.evil_timers.remove(idx);
        let before = crate::crime::legal_level(self.player(criminal).fame);
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&criminal) {
            player.fame = (i32::from(player.fame) - node.points)
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
        if crate::crime::legal_level(self.player(criminal).fame) != before {
            self.update_allowed_worn_items(criminal);
        }
        let snapshot = match self.sessions.get(&criminal) {
            Some(Session::InGame { player, .. }) => player.clone(),
            _ => return,
        };
        self.events.push(Event::Persist(snapshot));
        self.output_line(criminal, "The gods have forgiven you for your action.");
        self.output_line(
            session,
            &format!(
                "The gods have forgiven {criminal_name} for {} action.",
                text::pronoun_possessive(criminal_gender)
            ),
        );
    }

    /// HIDE <item> / HIDE <n> <currency> (theft.md §11.2): the thief's
    /// stash. NotDroppable refuses; success is quiet ("You hid %s.").
    /// The DLL's worn-single-copy rule is unreachable here (our worn
    /// gear lives outside the inventory vec).
    fn hide_stash_command(&mut self, session: SessionId, args: &str) {
        if self.delay_blocked(session) {
            return;
        }
        self.add_delay(session, 1);
        let words: Vec<String> = args
            .split_whitespace()
            .map(|w| w.to_ascii_lowercase())
            .collect();
        let room = self.player(session).location;
        // HIDE <n> <currency>.
        if let Some(Ok(n)) = words.first().map(|w| w.parse::<u32>()) {
            let idx = words.get(1).and_then(|w| {
                (0..5).find(|&i| text::currency_name(i).starts_with(w.as_str()))
            });
            let Some(idx) = idx else {
                self.output_line(session, &format!("Syntax: HIDE {n} {{Currency}}"));
                return;
            };
            if n == 0 {
                self.output_line(session, &format!("Syntax: HIDE {n} {{Currency}}"));
                return;
            }
            let held = {
                let c = &self.player(session).coins;
                match idx {
                    0 => c.copper,
                    1 => c.silver,
                    2 => c.gold,
                    3 => c.platinum,
                    _ => c.runic,
                }
            };
            if held < n {
                self.output_line(
                    session,
                    &format!("You don't have {n} {} to hide!", text::currency_name(idx)),
                );
                return;
            }
            if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
                match idx {
                    0 => player.coins.copper -= n,
                    1 => player.coins.silver -= n,
                    2 => player.coins.gold -= n,
                    3 => player.coins.platinum -= n,
                    _ => player.coins.runic -= n,
                }
            }
            self.room_hidden_coins.entry(room).or_insert([0; 5])[idx] += n;
            self.output_line(
                session,
                &format!("You hid {n} {}.", text::currency_name(idx)),
            );
            return;
        }
        // HIDE <item>.
        let want = words.join(" ");
        let found = {
            let inv = &self.player(session).inventory;
            inv.iter().position(|(id, _)| {
                self.content
                    .items
                    .get(id)
                    .is_some_and(|i| word_prefix_match(&i.name, &want))
            })
        };
        let Some(pos) = found else {
            self.output_line(session, &text::dont_see_here(args.trim()));
            return;
        };
        let (item_id, uses) = self.player(session).inventory[pos];
        let (not_droppable, name) = self
            .content
            .items
            .get(&item_id)
            .map(|i| (i.not_droppable != 0, i.name.clone()))
            .unwrap_or((true, String::new()));
        if not_droppable {
            self.output_line(session, text::MAY_NOT_HIDE_ITEM);
            return;
        }
        if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) {
            player.inventory.remove(pos);
        }
        self.room_hidden_items
            .entry(room)
            .or_default()
            .push((item_id, uses));
        self.output_line(session, &format!("You hid {name}."));
    }

    /// crime.md §2.5 (attack_user_monster 26113-26116, cast_monster_target
    /// 43255/43330/43417): initiating violence against a passive (mode
    /// 0/4) monster that is not already fighting you charges 10 evil via
    /// the NPC path (`crime::charge_npc_evil` — gates, dark cloud,
    /// minimum-10 bump). Returns true when the action is REFUSED; the
    /// caller aborts before any engagement. The own-summon exemption
    /// (the DLL's `sameas(mon+0x1a, user+0x1e) == 0` term) IS ported —
    /// M7 slice 5 gave the name link its owner semantics, and the
    /// `m.target != Some(session)` clause below is that term. What stays
    /// ORACLE-VERIFY is the refusal-before-engagement ORDERING, which no
    /// live run has ever exercised.
    fn charge_passive_monster_evil(
        &mut self,
        session: SessionId,
        monster: MonsterInstanceId,
    ) -> bool {
        let eligible = self
            .monsters
            .get(&monster)
            .is_some_and(|m| matches!(m.behaviour, 0 | 4) && m.target != Some(session));
        if !eligible {
            return false;
        }
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            return false;
        };
        let mut fame = player.fame;
        let before = crate::crime::legal_level(fame);
        match crate::crime::charge_npc_evil(&mut fame, player.warn_on_evil, player.lawful, 10) {
            Err(refusal) => {
                self.output_line(session, refusal);
                true
            }
            Ok(cloud) => {
                player.fame = fame;
                let crossed = crate::crime::legal_level(fame) != before;
                let snapshot = player.clone();
                self.output_line(session, cloud);
                if crossed {
                    self.update_allowed_worn_items(session);
                }
                self.events.push(Event::Persist(snapshot));
                false
            }
        }
    }

    /// `update_allowed_worn_items` (crime.md §2.4/§6.1): when a fame
    /// writer crosses a tier boundary, every worn piece and the wielded
    /// weapon re-run `user_can_use`; anything now refused is forced back
    /// to the pack. Removal wording ORACLE-VERIFY (M4 deferral note).
    fn update_allowed_worn_items(&mut self, session: SessionId) {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return;
        };
        let mut evict: Vec<(bool, usize)> = Vec::new(); // (is_weapon, worn index)
        for (i, (id, _)) in player.worn.iter().enumerate() {
            if let Some(item) = self.content.items.get(id)
                && !self.user_can_use(player, item)
            {
                evict.push((false, i));
            }
        }
        if let Some((id, _)) = player.weapon
            && let Some(item) = self.content.items.get(&id)
            && !self.user_can_use(player, item)
        {
            evict.push((true, 0));
        }
        if evict.is_empty() {
            return;
        }
        let mut lines = Vec::new();
        {
            let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
                return;
            };
            for (is_weapon, idx) in evict.into_iter().rev() {
                let entry = if is_weapon {
                    player.weapon.take()
                } else {
                    Some(player.worn.remove(idx))
                };
                if let Some(entry) = entry {
                    player.inventory.push(entry);
                    lines.push(entry.0);
                }
            }
        }
        for id in lines {
            let name = self
                .content
                .items
                .get(&id)
                .map_or_else(String::new, |i| i.name.clone());
            self.output_line(session, &text::item_force_removed(&name));
        }
        self.refresh_derived(session);
    }

    /// The spawn name roll (`get_random_name`, text::generate_name):
    /// walks the template's name block when it has one. The density
    /// spawner draws from its own stream (`spawn_rng`, the M6 seeded-
    /// golden divergence); the `--spawn` fixture path draws from the
    /// main stream like its other rolls.
    fn roll_spawn_name(
        &mut self,
        template: crate::content::MonsterId,
        base: &str,
        spawner_stream: bool,
    ) -> String {
        let Some(body) = self
            .content
            .monsters
            .get(&template)
            .and_then(|t| t.name_block)
            .and_then(|b| self.content.textblocks.get(&b))
            .map(|b| b.body.clone())
        else {
            return base.to_string();
        };
        let rng = if spawner_stream { &mut self.spawn_rng } else { &mut self.rng };
        text::generate_name(base, &body, &mut |lo, hi| rng.roll(lo, hi))
    }

    /// `get_monster_ability_value` (decompile 37150-37270): the template's
    /// ability rows PLUS the 5 active-spell slots (value-0 rows substitute
    /// the stored slot value — the cached [`MonsterInstance::slot_bag`]
    /// fold). KNOWN-DIVERGENCES, both shared with the player bag: the DLL
    /// max-not-sums a resist-ability id set (37173-37185) where we sum
    /// (shipped templates carry at most one row per resist), and a
    /// NegateAbility(124) row naming the queried id zeroes the DLL's whole
    /// answer where we skip the row in the fold (37209-37212; zero shipped
    /// monster payloads carry 124). Item terms (carried slots, the wielded
    /// weapon, the worn `something2` item) fold below per the 0x3d71f tail
    /// — wired at the M6 close-out.
    fn monster_ability_value(&self, id: MonsterInstanceId, ability: Ability) -> i32 {
        let Some(m) = self.monsters.get(&id) else {
            return 0;
        };
        let tpl = self.content.monsters.get(&m.template);
        let template: i32 = tpl.map_or(0, |t| {
            t.abilities
                .iter()
                .filter(|(a, _)| *a == ability)
                .map(|(_, v)| i32::from(*v))
                .sum()
        });
        // Item terms (0x3d71f tail): the 10 carried slots, the wielded
        // weapon (mon+0xb0) and the worn item (mon+0xac <- `something2`),
        // each through get_item_ability_value. Same sum-vs-max resist
        // divergence note as the rows above.
        let item_rows = |item: crate::content::ItemId| -> i32 {
            self.content.items.get(&item).map_or(0, |i| {
                i.abilities
                    .iter()
                    .filter(|(a, _)| *a == ability)
                    .map(|(_, v)| i32::from(*v))
                    .sum()
            })
        };
        let carried: i32 = m.items.iter().map(|(item, _)| item_rows(*item)).sum();
        let equipped: i32 = tpl
            .map_or(0, |t| {
                t.weapon.map_or(0, &item_rows) + t.worn_item.map_or(0, &item_rows)
            });
        template + carried + equipped + m.slot_bag.value(ability)
    }

    /// `monster_has_ability` (decompile 0x3d969, 37276-37333): PRESENCE of
    /// an ability id on a live monster, value-blind — the active-spell
    /// slots' spell rows first, then the template rows, then the carried /
    /// wielded / worn items. A value-0 row counts here where
    /// [`Core::monster_ability_value`] would fold it to nothing, which is
    /// exactly why the gates that ask "does it have X" call this one.
    fn monster_has_ability(&self, id: MonsterInstanceId, ability: Ability) -> bool {
        let Some(m) = self.monsters.get(&id) else {
            return false;
        };
        let item_has = |item: crate::content::ItemId| -> bool {
            self.content
                .items
                .get(&item)
                .is_some_and(|i| i.abilities.iter().any(|(a, _)| *a == ability))
        };
        let tpl = self.content.monsters.get(&m.template);
        m.active_spells
            .iter()
            .filter_map(|s| s.spell.and_then(|sid| self.content.spells.get(&sid)))
            .any(|s| s.abilities.iter().any(|(a, _)| *a == ability))
            || tpl.is_some_and(|t| t.abilities.iter().any(|(a, _)| *a == ability))
            || m.items.iter().any(|(item, _)| item_has(*item))
            || tpl.is_some_and(|t| {
                t.weapon.is_some_and(item_has) || t.worn_item.is_some_and(item_has)
            })
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

    /// `local_34` as a whole cast run carries it (charm.md §1.1): the
    /// pre-application ability scan preloads the save stat from the
    /// TEMPLATE's `charmres` (`knmsr+0x1a0`) for any spell whose ability
    /// list carries Enslave(6) (decompile 43310-43312) — no `.max(1)`
    /// floor, so a charmres of 2 halves to a threshold of 1.
    ///
    /// EDGE (decompile-verified, and the reason this is not a plain
    /// "charmres if Enslave" swap): the M.R. default at 43387 keys on
    /// `local_34 == 0`, and `local_34` starts at 0 (43170) — so a
    /// charmres-**0** template (48 shipped) silently falls back to the
    /// ordinary M.R. stat, floored at 1, exactly like a non-Enslave
    /// spell. Charmres 0 is not a free charm.
    ///
    /// The stat is ONE variable in the DLL, read by both the saving throw
    /// (43600-43614) and the Damage(-MR) scale (43946-43982) — so an
    /// Enslave spell that also carried a DamageMR row would scale that
    /// damage by charmres too. No shipped spell pairs them (the four
    /// Enslave carriers are ability 6 plus targeting-gate rows only).
    fn monster_cast_save_stat(&self, id: MonsterInstanceId, spell: &crate::content::Spell) -> i32 {
        if spell.abilities.iter().any(|(a, _)| *a == Ability::Enslave) {
            let charm_resist = self
                .monsters
                .get(&id)
                .and_then(|m| self.content.monsters.get(&m.template))
                .map_or(0, |t| i32::from(t.charm_resist));
            if charm_resist != 0 {
                return charm_resist;
            }
        }
        self.monster_save_stat(id)
    }

    /// The TARGETING half of the pre-application ability scan (charm.md
    /// §1.1; decompile `cast_monster_target` 43295-43376): a walk over the
    /// SPELL's ten ability rows, three of whose arms are pure refusals —
    /// `prf(00485de3)`, `tell_user`, `return 0`. True here means "print
    /// `spell_no_effect_on` and abort", UNCHARGED: the scan is entered only
    /// once the caster is known to be able to afford the cast (the
    /// sufficiency test at 43278) and every mana/energy subtraction is
    /// downstream of it (43443+). First refusing row wins, so the walk
    /// short-circuits in list order like the DLL's `return`.
    ///
    /// The other arms of the same loop are elsewhere or unported: ability
    /// 6 preloads the charm save stat ([`Core::monster_cast_save_stat`]),
    /// 52 (EvilInCombat) charges evil points, 144 (NonMagicalSpell) sets
    /// the flag that SKIPS the SpellImmu gate below (43378), and 163
    /// (SpellComponent) runs the component confirmation. None of those is
    /// carried by a shipped Enslave spell. Evil(98) has no arm at all —
    /// 0x62 falls past both the `< 0x51` block and the 0x6c/0x90/0xa3
    /// chain — so the 88 control-undead row is inert in the scan; its
    /// only engine effect is the crime.md §6.1 alignment gate at
    /// learn/cast time.
    ///
    /// Shipped reach: all four Enslave carriers hold exactly one of the
    /// three — 49 song of charming and 55 enslave AffectsLiving, 88
    /// control undead AffectsUndead, 92 charm animal AffectsAnimals — so
    /// without this, `charm animal` was legal on all 1101 templates
    /// instead of 155, and `control undead` on 1101 instead of 115.
    fn cast_eligibility_refused(
        &self,
        id: MonsterInstanceId,
        spell: &crate::content::Spell,
    ) -> bool {
        spell.abilities.iter().any(|(ability, _)| match ability {
            // 43299-43307: AffectsAnimals(80) refuses a target that does
            // NOT carry Animal(78). Value-blind presence, hence
            // `monster_has_ability` — the shipped rows are all value 0.
            Ability::AffectsAnimals => !self.monster_has_ability(id, Ability::Animal),
            // 43317-43324: AffectsUndead(23) refuses on the TEMPLATE's
            // `undead` byte (`knmsr+0xad`) being zero. A column, not an
            // ability row, and the test is `!= 0` — the 8 shipped `-1`
            // templates are undead.
            Ability::AffectsUndead => {
                self.monsters
                    .get(&id)
                    .and_then(|m| self.content.monsters.get(&m.template))
                    .map_or(0, |t| t.undead)
                    == 0
            }
            // 43349-43357: AffectsLiving(108) refuses a target that DOES
            // carry NonLiving(109) — the inverted polarity of the animals
            // arm, and a different predicate from the one above (6 shipped
            // templates are `undead != 0` without a 109 row).
            Ability::AffectsLiving => self.monster_has_ability(id, Ability::NonLiving),
            _ => false,
        })
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
        // The DLL's `local_34`, preloaded ahead of the attempt loop
        // (43302-43316 / 43387-43392) and read by the save AND the
        // Damage(-MR) scale below: `charmres` for an Enslave spell,
        // M.R. otherwise — see [`Core::monster_cast_save_stat`].
        let save_stat = self.monster_cast_save_stat(monster_id, &spell);
        let resisted = succeeded && save_allowed && {
            let rng = &mut self.rng;
            monster_save_resists(save_stat, &mut |lo, hi| rng.roll(lo, hi))
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
            // like every lock since slice 3, and on the OFFENSIVE mode.
            //
            // `cast_monster_target` has TWO grudge writes, and only the
            // FIRST is spelltype-gated:
            //   43249-43273 — inside `if (param_4 != 0)` (autocombat
            //     re-fire) AND `spelltype < 3`, so a benign cast never
            //     reaches it. That is the one this gate mirrors.
            //   43323-43347 — inside the spell's ABILITY scan, gated on
            //     neither `spelltype` nor `param_4`: `ability == 0x34`
            //     (EvilInCombat) + non-arena + monster mode ∈ {0, 4} +
            //     `sameas(mon+0x1a, user+0x1e) == 0`. Same body:
            //     `add_evil_points(caster, -1, 10, 0xb, 0)`, refuse on
            //     non-zero, else `mon[0x50] = 1`, copy the caster's name
            //     into `mon+0x1a` and clear the suppression byte at
            //     `mon+0x116`.
            //
            // M7 PENDING (`re/docs/crime.md` §2.5, the 43330 row): the
            // 0x34 arm is NOT implemented. In the DLL, cursing a passive
            // monster costs 10 evil points and earns a grudge; here it is
            // free and the monster never retaliates. DATA
            // (`re/mmud_wgnt.sqlite`): 25 of the 29 learnable benign
            // match-4/6/8 spells carry ability 52 — curse, blind, slow,
            // hold person, confusion, sleep, entangle, mute, the seven
            // songs, creeping doom, wrathful curse. This is a WIDENING of
            // an existing gap, not a new one: 16 learnable AREA spells
            // (match 12) already carry ability 52 and are already
            // unhandled on the `area_cast` path. `charge_passive_monster_evil`
            // already implements the 43323 predicate exactly — it is only
            // gated at the CALL SITE on `is_offensive()` rather than on
            // the ability, so closing this is a call-site change plus the
            // grudge/suppression writes. HOME: the crime slice has already
            // shipped, so this carries to the M7 close-out (slice 8) — it
            // was logged during slice 5's Task-3b routing fix, which is
            // what put 25 more spells in front of this gate.
            if spell.duration == 0
                && spell.target_mode.is_offensive()
                && self.monsters.get(&monster_id).is_some_and(|m| m.target.is_none())
            {
                // 43260/43335 are both charm-exempt; the `target.is_none()`
                // gate already makes a pet unreachable here (a pet always
                // carries its owner link), so the tag is documentation.
                self.retaliation_lock(monster_id, session, CharmedExemption::Exempt);
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
        // `area_cast`). Enslave (6) charms on both arms (charm.md §1,
        // the arm below). The healing-at-monster INSTANT arms
        // (Heal(18)/EnergyLevel(11)/CurePoison(20), cast_monster_target
        // 43824-43882/43883-43900/44131-44160) stay unimplemented on an
        // ABILITY-SIDE data gate, not a routing one — benign spells do
        // reach a monster (the match-4/6/8 band above). Of the 1379
        // shipped spells only five carry one of those three abilities at
        // match 4/6/8: 943 `sys j`, 1114 `sabre`, 1146 `godheal` and 1252
        // `dead heal` are instant but UNLEARNABLE (no LearnSp carrier),
        // and the one learnable carrier — 853 `wrathful curse`, scroll
        // 1300 — has duration 7, so it takes the slot arm below, exactly
        // like the DLL's `param_1[0x67] != 0` else at 43872-43880. The
        // forced-cast route has no shipped trigger either. A non-zero
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
                // the target's MR, literally the same `local_34` the save
                // read (damage_mr; decompile 43937-43993), which is why an
                // Enslave spell's charmres would scale it too. No duration
                // gate either (44287).
                Ability::DamageMR => {
                    damage_total += damage_mr(alter_sp_dmg(amount, boost), save_stat, anti_magic);
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
                    // 43920: the spawn carries the VICTIM's instance id
                    // in `+0x88` and no name link at all — the driver's
                    // hunt arm (20450-20463) then walks it to the victim
                    // and swings.
                    self.summon_spawn(amount, room, SummonLink::HuntMonster(monster_id));
                }
                Ability::Summon => {}
                // Enslave (6), case 43796-43822 — the charm apply
                // (charm.md §1.2/§1.3/§1.4).
                Ability::Enslave => {
                    // Two gates, both plain compares, no roll: the
                    // spell's match type must be one of the monster
                    // classes (43797), and the template's `charmlvl`
                    // must be at or below the caster's level
                    // (43798-43800). FAILURE IS SILENT — the DLL skips
                    // the case body entirely: no message, no slot entry,
                    // and the mana stays paid. It must NOT fall through
                    // to the default duration-slot arm.
                    //
                    // `match_ok` is DEFENCE IN DEPTH and is not reachable
                    // from any command path: `cmd_cast` already refuses a
                    // non-monster match type at the target resolution
                    // above (`MAY_NOT_CAST_ON_MONSTER`, 43205/44311-44315)
                    // and returns, so nothing that fails this test can
                    // arrive here. It is kept because the DLL keeps it —
                    // the two tests are separate in the decompile and a
                    // future caller (a monster-cast path, an item proc)
                    // could enter the apply loop without the command
                    // gate. Do not expect a test to kill its removal;
                    // what the command DOES do on a match-0 spell is
                    // pinned by
                    // `charm.rs::a_non_monster_match_type_is_refused_before_the_charm_arm`.
                    // The `charmlvl` half below IS live and is pinned by
                    // `charm_level_above_the_caster_is_silent` and
                    // `charm_level_equal_to_the_caster_still_charms`.
                    let match_ok = spell.match_type.accepts_monster();
                    let charm_level = self
                        .monsters
                        .get(&monster_id)
                        .and_then(|m| self.content.monsters.get(&m.template))
                        .map_or(0, |t| i32::from(t.charm_level));
                    if !match_ok || charm_level > i32::from(level) {
                        continue;
                    }
                    // Duration arm (43809-43821): the slot entry first,
                    // through the shared once-flag. Instant (43801-43807)
                    // takes no slot and no timer at all — permanent until
                    // a release path fires.
                    if duration != 0 && entered.is_none() {
                        entered = Some(self.enter_monster_spell_slot(
                            monster_id, spell.id, amount, duration,
                        ));
                    }
                    // The §0 triple, written UNCONDITIONALLY — the caller
                    // never checks `add_cast_spell_to_monster`'s -1
                    // (43810-43820), so a monster whose 5 slots are full
                    // of other spells takes the plain cast-fail line AND
                    // a permanent, timerless charm (§1.4). No rename: the
                    // display name is untouched, only the internal owner
                    // link (§1.3).
                    if let Some(m) = self.monsters.get_mut(&monster_id) {
                        m.target = Some(session); // +0x1a <- caster name
                        m.suppress = true; // +0x116 = 1
                        m.charmed = true; // +0x128 |= 1
                        m.needs_recompute = true; // +0x140 dirty
                    }
                    self.recompute_monster_effects(monster_id);
                }
                // Targeting-gate rows carry no payload and never reach a
                // slot: AffectsUndead(23) is in the case-break list
                // (43731), AffectsAnimals(80) is excluded from the switch
                // outright (43723-43724), and Evil(98)/AffectsLiving(108)
                // fall in the two break ranges at 44205/44208. Three of
                // them have already had their say by the time the apply
                // loop runs — [`Core::cast_eligibility_refused`] is the
                // pre-application scan (43295-43376), and an ineligible
                // target never reaches here at all. Evil(98) is the odd
                // one out: the scan has no arm for it either, so it is
                // inert in the engine and only reads as documentation on
                // the 88 control-undead row. The charm family ships
                // exactly these as its companion rows, so a
                // charmlvl-gated-out cast must stay slotless.
                Ability::AffectsUndead
                | Ability::AffectsAnimals
                | Ability::Evil
                | Ability::AffectsLiving => {}
                // Every other row in a DURATION cast drives the one slot
                // entry (the cast_monster_target default arm, 43778-43799).
                // Instant casts leave them to their systems.
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
        // mystic — oracle-backlog, not milestone-gated).
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
        // The cast-DAMAGE twin (43750-43766) — one of the three lock
        // sites with no charmed check: spell damage grudges a pet without
        // releasing it (charm.md §2.4, and see [`CharmedExemption`]).
        self.retaliation_lock(monster_id, session, CharmedExemption::Ignored);
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
            // ORACLE-VERIFY: no alignment-gated scroll was measured on a
            // shop shelf; the can't-use suffix is the least-wrong frame.
            SpellGate::Alignment => Some(text::CANT_USE_SUFFIX),
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
            FloorMatch::None => {
                // The hidden stash answers to its name (theft.md §11.2 —
                // knowing what's hidden is enough to take it).
                let hidden_pos = self.room_hidden_items.get(&room).and_then(|items| {
                    items.iter().position(|(id, _)| {
                        self.content
                            .items
                            .get(id)
                            .is_some_and(|i| word_prefix_match(&i.name, &want))
                    })
                });
                if let Some(pos) = hidden_pos {
                    let (item, uses) =
                        self.room_hidden_items.get_mut(&room).expect("has items").remove(pos);
                    let name = self.content.items[&item].name.clone();
                    if let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session)
                    {
                        player.inventory.push((item, uses));
                    }
                    self.output_line(session, &text::took_item(&name));
                    return Resolution::Handled;
                }
                Resolution::FallThrough
            }
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
    ///
    /// This is `find_action_target`'s monster block WITHOUT the `0x800`
    /// mask bit — one pass over everything, pets included. Callers that
    /// carry the bit want [`Core::find_monster_charmed_last`].
    fn find_monster(&self, room: RoomId, words: &str) -> Option<MonsterInstanceId> {
        self.find_monster_pass(room, words, MonsterPass::All)
    }

    /// The `0x800` search (`cmd_any_attack`'s mask `0x883` at 49590, and
    /// `cmd_cast`'s preferred `0x801`/`0x803`/`0xf837`): the monster
    /// block runs twice — 63776 skips `mon+0x128 & 1`, then 63820 scans
    /// ONLY charmed monsters. Charmed bodies are searched LAST, never
    /// excluded: with no wild match in the room the second pass hands
    /// back the pet, which is what keeps a lone pet castable at and,
    /// crucially, ATTACK-able (§2.3's physical-attack release path).
    fn find_monster_charmed_last(&self, room: RoomId, words: &str) -> Option<MonsterInstanceId> {
        self.find_monster_pass(room, words, MonsterPass::Uncharmed)
            .or_else(|| self.find_monster_pass(room, words, MonsterPass::Charmed))
    }

    fn find_monster_pass(
        &self,
        room: RoomId,
        words: &str,
        pass: MonsterPass,
    ) -> Option<MonsterInstanceId> {
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
            .filter(|(_, m)| m.location == room && m.current_hp > 0 && pass.admits(m.charmed))
            .find(|(_, m)| {
                // Word-prefix match against the DISPLAY name — the
                // spawn adjective is targetable ("attack nasty").
                let name: Vec<&str> = m.name.split_whitespace().collect();
                (0..name.len()).any(|start| {
                    want.len() <= name.len() - start
                        && want.iter().enumerate().all(|(i, w)| {
                            name[start + i].to_ascii_lowercase().starts_with(w)
                        })
                })
            })
            .map(|(id, _)| *id)
    }

    /// `find_action_target` (decompile 63726), restricted to the kinds
    /// [`crate::content::FindScope`] models and searched in the DLL's own
    /// order: the room's monsters (mask bit `0x1`) first, then the room's
    /// players (`0x2`), then the caster's carried items (`0x4`). The
    /// first hit wins — the DLL's multiple-match prompt is not modelled.
    ///
    /// The monster leg runs as ONE pass or TWO depending on the scope's
    /// `0x800` bit; either way it finishes before the user scan, so the
    /// bit orders monsters against monsters and never against a player.
    fn find_cast_target(
        &self,
        session: SessionId,
        scope: crate::content::FindScope,
        words: &str,
    ) -> Option<CastTarget> {
        let room = self.player(session).location;
        let monster = if !scope.monsters {
            None
        } else if scope.charmed_last {
            self.find_monster_charmed_last(room, words)
        } else {
            self.find_monster(room, words)
        };
        if let Some(id) = monster {
            return Some(CastTarget::Monster(id));
        }
        let want = words.trim().to_ascii_lowercase();
        if scope.users
            && let Some(id) = self
                .in_game_sessions()
                .filter(|(_, p)| p.location == room)
                .find(|(_, p)| word_prefix_match(&p.name, &want))
                .map(|(id, _)| id)
        {
            return Some(CastTarget::User(id));
        }
        // ORACLE-VERIFY: the carried set only. Room items (found kind 4 ->
        // "You are not carrying %s!") and spellbook entries (kind 0x10 ->
        // "Why would you want to cast a spell on a spell?") are in the
        // DLL's universal mask but have no measured surface here.
        if scope.items
            && let Some(id) = self.player(session).inventory.iter().find_map(|(id, _)| {
                self.content
                    .items
                    .get(id)
                    .filter(|i| word_prefix_match(&i.name, &want))
                    .map(|_| *id)
            })
        {
            return Some(CastTarget::Item(id));
        }
        None
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
        let (room, behaviour, roam, suppress, charmed, hunt) = (
            m.location,
            m.behaviour,
            m.roam_class,
            m.suppress,
            m.charmed,
            m.hunt,
        );
        if let Some(victim) = m.target {
            // A2 — locked (20465-20519): no roll, attacked every round the
            // lock is valid. The four arms are the DLL's, in the DLL's
            // order — suppression, roam class 5, charmed bit, behaviour —
            // and that order decides two things: roam 5 pre-empts the pet
            // arm, and the pet arm pre-empts the "friends" arm.
            if !suppress {
                if self.acquisition_valid(victim, room) {
                    self.bump_attackers(victim);
                    self.monster_attack(id, victim);
                }
            } else if roam == 5 {
                // 20477-20493: the class-5 ward defence. It only ever
                // swings at players who are in autocombat AGAINST the
                // named user — PvP, which is M8 — so it is faithfully a
                // no-op here.
                //
                // DECOMPILE over charm.md: this arm sits AHEAD of the
                // charmed check, so a charmed roam-5 monster lands here
                // and never assists. §2.2's "charmed pets take the
                // FUN_0044cc65 branch instead" describes the friends arm
                // below (which IS charm-gated) and overstates this one.
            } else if charmed {
                // 20512-20517: suppressed + charmed = a pet. Deterministic,
                // no roll, every pass.
                self.pet_assist(id, victim);
            } else if !matches!(behaviour, 4 | 0 | 3) {
                // Suppressed aggressive (20494-20511): attacks the first
                // OTHER valid player — never its named target. The
                // `(mon+0x128 & 1) == 0` guard is why a PET can never
                // arrive here: this arm belongs to non-charmed "friends"
                // (charm.md §2.2 last paragraph, §2.4).
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
        // A1 — no target (`mon+0x1a` empty, 20369). The `+0x88` HUNT link
        // is tested FIRST (20370) and pre-empts every acquisition arm
        // below: a summoned hunter never picks up a player, ever.
        if let Some(quarry) = hunt {
            // charm.md §7's stale-link flag, closed by construction: the
            // DLL clears `+0x88` on nobody's death and recycles monster
            // ids, so its hunter can redirect onto an unrelated body.
            // Our ids never repeat, so a dangling link is inert — and the
            // DLL agrees on the observable, because `get_monster_data`
            // fails inside both `dir_monster_travelling_coord` (15797)
            // and `attack_monster_monster` (27226), leaving the arm a
            // no-op. What we must NOT do is fall through to acquisition:
            // the DLL's branch is decided by `+0x88 != 0` alone.
            if !self.monsters.contains_key(&quarry) {
                return;
            }
            match self.dir_toward_monster(quarry, room) {
                // 20457-20461: one gated step per driver pass, no roll.
                Some(dir) => {
                    if !self.monster_confusion_fumble(id) {
                        self.move_monster(id, dir, false);
                    }
                }
                // 20452-20455: a cold trail SWINGS — and the arm has no
                // room compare, nor does `attack_monster_monster`
                // (charm.md §3), so the hunter hits its quarry across a
                // room boundary. Decompile-literal and deliberately so;
                // reachability is fixture-only (no learnable Summon
                // carrier ships), and the arm needs the hunter to share a
                // room with SOME player for the driver to reach it at
                // all (20362-20368).
                None => {
                    if !suppress {
                        self.attack_monster_monster(id, quarry);
                    }
                }
            }
            return;
        }
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

    /// `FUN_0044cc65` (46917-46965, charm.md §2.2) — everything a pet ever
    /// does in combat. It reads its OWNER's autocombat record
    /// (`DAT_004877e8 + usernum*0x14`: `[+0]` user target or -1, `[+4]`
    /// monster target or 0xffff) and takes one of three outcomes:
    ///
    /// - owner fighting a MONSTER -> [`Core::attack_monster_monster`] at
    ///   it (46958). No roll, no room compare, no acquisition gate: the
    ///   pet swings every driver pass, at whatever the owner is on, even
    ///   from another room (§3's missing room check is deliberate).
    /// - owner fighting the PET ITSELF -> the self-release (46929-46953,
    ///   [`Core::autocombat_release_charm`]).
    /// - owner idle -> nothing at all, draw-free. The caller's
    ///   `is_inside_autocombat` gate (20514) and the two record tests here
    ///   collapse into one `Option` in our model.
    ///
    /// M8 SEAM — the `[+0]` arm at 46963 is `attack_monster_user(pet,
    /// thatUser)`: a pet joins its owner's PvP. Our autocombat record
    /// carries only the monster half ([`Session::InGame`]'s `target`), so
    /// `None` here means "idle" and "fighting a player" alike. When PvP
    /// lands, the user half of the record grows the second arm and it
    /// belongs HERE, ahead of the monster one.
    ///
    /// An OFFLINE owner never reaches this function (the caller's
    /// `get_user_number` fails at 20464) — the pursuit tier's give-up
    /// counter owns that case and releases the pet ~16 s later (§4.2).
    fn pet_assist(&mut self, id: MonsterInstanceId, owner: SessionId) {
        let Some(Session::InGame { target, .. }) = self.sessions.get(&owner) else {
            return;
        };
        let Some(quarry) = *target else {
            return;
        };
        if quarry == id {
            self.autocombat_release_charm(id);
        } else {
            // THE ENERGY TRAP, live from here on: the pay gate inside
            // `attack_monster_monster` sits AFTER `calculate_attack`
            // (27241 then 27242), so a pet whose form-0 EU exceeds its
            // whole pool burns draws on every single one of these passes
            // and never lands a hit. 21 shipped templates are shaped that
            // way; `bishop`, `priest` and `boatman` are pool 0 / cost 5
            // with `charmlvl` 0, i.e. charmable by anyone. Faithful, not
            // an oversight — see [`Core::build_monster_attacker_form0`]
            // and the pin in tests/charm.rs.
            self.attack_monster_monster(id, quarry);
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

    /// The retaliation lock, shaped after the MELEE twins
    /// (`attack_user_monster` 26230-26237 and 26515-26525): a hit monster
    /// locks its attacker iff `genrdn(1,100) < aggression` OR it is a
    /// passive mode (3/0/4); the roll draws either way. Class 0x25 never
    /// locks and never rolls; class 5 rolls but keeps an existing lock.
    ///
    /// `charm` decides the charmed-bit gate ONLY — the cast and area
    /// twins differ from this body in three further ways that it does not
    /// model. Read [`CharmedExemption`] before adding a call site.
    fn retaliation_lock(
        &mut self,
        id: MonsterInstanceId,
        attacker: SessionId,
        charm: CharmedExemption,
    ) {
        let Some(m) = self.monsters.get(&id) else {
            return;
        };
        // Ahead of the roll in every twin that has it (26230, 26514,
        // 43260, 43335 and 43470 all OPEN with `(mon+0x128 & 1) == 0`),
        // so an exempt site draws nothing at all on a pet.
        if charm == CharmedExemption::Exempt && m.charmed {
            return;
        }
        // Class 0x25 short-circuits AHEAD of the draw in every twin — it
        // is the second operand of the `&&` chain, before the `genrdn`.
        if m.roam_class == 0x25 {
            return;
        }
        let (aggression, behaviour, roam, locked) =
            (m.aggression, m.behaviour, m.roam_class, m.target.is_some());
        // The roll is unconditional from here: the DLL spends it and only
        // THEN asks whether a class-5 guardian already holds a lock
        // (`roam != 5 || mon+0x1a == 0` is the LAST operand of the chain,
        // 26234/26520/43265/43340/43474). Returning early on that case,
        // as this used to, skipped a draw the original always makes.
        let roll = self.rng.roll(1, 100);
        if roam == 5 && locked {
            return;
        }
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
        let mode_now = match self.sessions.get(&session) {
            Some(Session::InGame { attack_mode, .. }) => *attack_mode,
            _ => crate::combat::AttackType::Normal,
        };
        // Special attacks cost the FULL pool = one swing
        // (combat_rounds.md: backstab/bash/smash).
        let eu = if mode_now == crate::combat::AttackType::Backstab {
            match self.sessions.get(&session) {
                Some(Session::InGame { energy, .. }) => (*energy).max(1),
                _ => PLAYER_ENERGY_MAX,
            }
        } else {
            self.player_energy_used(session)
        };
        let target_name = self
            .monsters
            .get(&target)
            .map(|m| m.name.clone())
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
            let mode = match self.sessions.get(&session) {
                Some(Session::InGame { attack_mode, .. }) => *attack_mode,
                _ => crate::combat::AttackType::Normal,
            };
            let rng = &mut self.rng;
            let result = crate::combat::calculate_attack(
                &attacker,
                &defender,
                mode,
                &mut |lo, hi| rng.roll(lo, hi),
            );
            use crate::combat::Outcome;
            let mut hit_verb = {
                let n = hit_verbs.len().max(1) as i32;
                let pick = if hit_verbs.len() > 1 { self.rng.roll(0, n - 1) } else { 0 };
                hit_verbs.get(pick as usize).cloned().unwrap_or_else(|| "punch".into())
            };
            let mut miss_verb = {
                let n = miss_verbs.len().max(1) as i32;
                let pick = if miss_verbs.len() > 1 { self.rng.roll(0, n - 1) } else { 0 };
                miss_verbs.get(pick as usize).cloned().unwrap_or_else(|| "swing at".into())
            };
            if mode == crate::combat::AttackType::Backstab {
                // Mode 4 wraps the verb slots in "surprise %s"
                // (move_player_to_fighter 24751-24758); wording of the
                // rendered line ORACLE-VERIFY.
                hit_verb = format!("surprise {hit_verb}");
                miss_verb = format!("surprise {miss_verb}");
            }
            if mode == crate::combat::AttackType::Backstab
                && matches!(result.outcome, Outcome::Hit | Outcome::NoDamage | Outcome::Critical)
            {
                // The autocombat +8 revert: backstab drops to a normal
                // attack after the first landed hit (M3 extraction).
                if let Some(Session::InGame { attack_mode, .. }) =
                    self.sessions.get_mut(&session)
                {
                    *attack_mode = crate::combat::AttackType::Normal;
                }
            }
            match result.outcome {
                // combat_rounds.md §5: result 3 renders distinct dodge/parry
                // flavor for the player view too. MEASURED (charm.md §8.3):
                // it does — `You swing at giant bat who dodges your attack!`
                // — so the two outcomes are rendered apart. Result 0 is the
                // to-hit failure and keeps the plain miss line.
                Outcome::Dodged => {
                    self.output_line(session, &text::player_miss(&miss_verb, &target_name));
                }
                Outcome::Parried => {
                    self.output_line(session, &text::player_dodge(&miss_verb, &target_name));
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
                    // Retaliation: gated lock per hit (26515-26525) —
                    // except on a charmed target, where the ELSE half of
                    // the same branch (26527) runs the owner release
                    // instead (26527-26562, charm.md §4.3).
                    if self.monsters.get(&target).is_some_and(|m| m.charmed) {
                        self.owner_melee_release(target, session);
                    } else {
                        self.retaliation_lock(target, session, CharmedExemption::Exempt);
                    }
                }
            }
        }
        // NO engage re-mark here: 26230 lives in the other arm of the
        // 26112 split and cannot follow the round's post-damage branch.
        // It fires once, at engagement, from the ATTACK command.
    }

    /// The charmed half of `attack_user_monster`'s post-damage branch
    /// (26527-26562, charm.md §4.3). Only the OWNER's own swing does
    /// anything — `sameas(mon+0x1a, attacker)`; a different player hitting
    /// somebody's pet gets NOTHING at all: no release, no grudge, no name
    /// overwrite (the ordinary lock lives in the non-charmed half).
    ///
    /// The writes follow the DLL's order — suppression off, then the
    /// charmed bit, then the ability-6 slot sweep, whose termination also
    /// empties the owner link. That ORDER is not observable, and this
    /// comment used to over-claim that it was: [`Core::release_charm`]
    /// (the sweep's terminator) clears suppression too, so hoisting the
    /// `suppress = false` below the sweep would land on the same state
    /// from either arm. Kept in the DLL's order for readability against
    /// 26527-26562, not because anything can tell.
    ///
    /// What IS observable is that the write happens at all, and on the
    /// SLOTLESS pet specifically: the sweep finds no ability-6 slot,
    /// terminates nothing, and therefore clears neither suppression nor
    /// the owner link. So this line is the only thing that unsuppresses
    /// such a pet, and the link SURVIVES — the ex-pet is a full grudge
    /// monster hostile to its former owner. Pinned by
    /// `charm.rs::owner_melee_leaves_a_slotless_pet_as_a_grudge_holder`.
    fn owner_melee_release(&mut self, id: MonsterInstanceId, attacker: SessionId) {
        if self.monsters.get(&id).is_none_or(|m| m.target != Some(attacker)) {
            return;
        }
        if let Some(m) = self.monsters.get_mut(&id) {
            m.needs_recompute = true;
            m.suppress = false;
            m.charmed = false;
        }
        self.sweep_charm_slots(id);
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
            // Rob forms (kind 3): monster_rob_user (0x295bd) is a
            // `return 0` STUB — monster robbery never happens in WG3-NT
            // (theft.md). The caller then swings with form slot 0 iff
            // its kind byte is 1 (attack_monster_user 26808-26813),
            // else this swing re-rolls.
            let form = if form.kind == 1 {
                form
            } else if form.kind == 3 && forms[0].kind == 1 {
                forms[0]
            } else {
                continue; // unknown kinds, or a rob fallback with no melee slot 0
            };
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
            // Oracle palette: incoming hits 1;31 ("The kobold thief stabs
            // you for 4 damage!"), incoming miss/glance/dodge 0;36.
            let paint = if matches!(result.outcome, Outcome::Hit | Outcome::Critical) {
                text::color::DAMAGE
            } else {
                text::color::INCOMING
            };
            let victim_line = format!("{paint}{victim_line}{}", text::color::RESET);
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

    /// `move_monster_to_fighter` (decompile 0x2b43e, 25087-25230) in its
    /// ATTACKER shape, from attack-form slot 0 — the only slot
    /// `attack_monster_monster` ever loads (27229/27238: `param_3 = local_8 = 0`).
    /// Returns the fighter and its energy cost (word [5]).
    ///
    /// - accuracy [0] = the form's accuracy (`knmsr+0x12e`) + Accuracy(0x16)
    ///   + Accuracy2(0x69) + Accuracy3(0x6a) (25188-25193);
    /// - damage [6]/[7] = the form's bounds, both raised by MaxDamage(4)
    ///   (25196-25198 — one value, both bounds);
    /// - energy [5] = the form's energy (`knmsr+0x190`), scaled by
    ///   Speed(0x57) as `EU*val/100` and capped at the template's pool
    ///   `knmsr+0x7a` (25203-25211); no shipped template carries Speed, so
    ///   the scale only ever arrives through an active slot (`sphere of
    ///   isolation` is Speed 5000). The DLL does that multiply in
    ///   `longlong` and we match it — an i32 product would be tight if
    ///   several Speed slots ever stacked onto a 1000-EU form.
    ///
    /// The form's KIND is not consulted: `move_monster_to_fighter` fails
    /// only when the record or template is missing, which is the whole
    /// content of the `!= '\0'` guards at 27239-27240. A monster whose
    /// slot 0 is a kind-0 (unused) form therefore still swings — with
    /// whatever words that slot holds, which is NOT the same as swinging
    /// with zeroes: 125 of the 1101 shipped templates have
    /// `attacktype_1 = 0` and 28 of those carry nonzero accuracy/min/max
    /// there. `dark warlock` (acc 49, min 100, max 15 — min > max, so
    /// [`crate::combat`] raises max to min and it lands a flat 100) and
    /// `dying master assassin` (acc 120, 7-20) are real, dangerous
    /// form-0 fighters. 73 kind-0 templates sit under the 9999
    /// charm floor, so this is live for pets.
    ///
    /// The EU trap that comes with it: 21 templates carry
    /// `attackenergy_1 > energy`, so the 27242 pay gate can never open
    /// and they burn draws forever without swinging. `bishop`, `priest`
    /// and `boatman` are the sharp edge — pool 0, form cost 5, and
    /// `charmlvl 0`, i.e. charmable by anyone.
    ///
    /// The alignment-code 4th argument (`FUN_0042a15c`) is passed its own
    /// return value here, so its accuracy branch is dead on this path
    /// (25109-25118) and word [2] stays 0.
    fn build_monster_attacker_form0(
        &self,
        id: MonsterInstanceId,
    ) -> Option<(crate::combat::Fighter, i32)> {
        let m = self.monsters.get(&id)?;
        let tpl = self.content.monsters.get(&m.template)?;
        let form = tpl.attacks[0];
        let pool = tpl.energy;
        let damage = self.monster_ability_value(id, Ability::MaxDamage);
        let accuracy = i32::from(form.accuracy)
            + self.monster_ability_value(id, Ability::Accuracy)
            + self.monster_ability_value(id, Ability::Accuracy2)
            + self.monster_ability_value(id, Ability::Accuracy3);
        let mut energy = i32::from(form.energy);
        let speed = self.monster_ability_value(id, Ability::Speed);
        if speed != 0 {
            // 25205: `(longlong)speed * (longlong)EU / 100`, then capped
            // at the pool.
            let scaled = (i64::from(energy) * i64::from(speed) / 100).min(i64::from(pool));
            energy = i32::try_from(scaled).unwrap_or(pool);
        }
        Some((
            crate::combat::Fighter {
                accuracy,
                evasion_a: 0,
                evasion_b: 0,
                armor: 0,
                min_damage: i32::from(form.min_damage) + damage,
                max_damage: i32::from(form.max_damage) + damage,
                parry: 0,
                crit_rating: 0, // monsters never crit (hard-zeroed 25187)
            },
            energy,
        ))
    }

    /// `attack_monster_monster` (decompile 0x2f6ae, 27213-27340;
    /// `charm.md` §3) — the one monster-vs-monster swing, shared by pets
    /// (§2.2) and summoned hunters (§6). One swing per call: no form
    /// selection, no swing loop, and no retaliation from the defender.
    ///
    /// Entry gates (27231-27234), both silent and draw-free: the attacker
    /// must be at FULL energy (`mon+0x114 <= mon+0x16` — the same gate
    /// `attack_monster_user` opens with at 26706) and must not carry
    /// Fear(0x3c). There is deliberately NO room compare and no safe-room
    /// check: the hunt arm swings at a victim it has not caught up with
    /// (charm.md §6), so the room only ever decides who SEES the line.
    ///
    /// DIVERGENCES from the DLL, all documented in charm.md §3:
    /// - the defender's `+0x14` poison floor is raised from the result
    ///   block's word [3] (27248-27249), and word [3] (`DAT_00495fdc`) is
    ///   zeroed on entry to `calculate_attack` (25246) and never written
    ///   by it — the raise is dead code in WG3-NT, so nothing is ported;
    /// - the split covers the sessions engaged on the victim; the DLL's
    ///   `distribute_experience(-1, ...)` also pays idle-autocombat users
    ///   standing in the room.
    fn attack_monster_monster(&mut self, attacker: MonsterInstanceId, defender: MonsterInstanceId) {
        let (Some(a), Some(d)) = (self.monsters.get(&attacker), self.monsters.get(&defender))
        else {
            return;
        };
        let (attacker_room, defender_room) = (a.location, d.location);
        let Some(pool) = self.content.monsters.get(&a.template).map(|t| t.energy) else {
            return;
        };
        if a.energy < pool || self.monster_has_ability(attacker, Ability::Fear) {
            return;
        }
        let Some((fighter, cost)) = self.build_monster_attacker_form0(attacker) else {
            return;
        };
        let target = self.build_monster_defender(defender);
        let rng = &mut self.rng;
        let result = crate::combat::calculate_attack(
            &fighter,
            &target,
            // Mode 5 for both mode globals (27235-27236) — our plain
            // `Normal`: no damage seed, no accuracy modifier, and the
            // hard-zeroed monster crit rating keeps crits off anyway.
            crate::combat::AttackType::Normal,
            &mut |lo, hi| rng.roll(lo, hi),
        );
        // 27242: the pay gate is checked AFTER the draws — a form costing
        // more than the whole pool burns rolls and lands nothing. The DLL
        // compares as `uint`, so a cost that resolved NEGATIVE wraps huge
        // and aborts; an i32 compare would instead REFUND energy below.
        if cost < 0 || cost > self.monsters[&attacker].energy {
            return;
        }
        // Both display names are read before the defender's record can
        // die (the DLL's `strcpy` of `mon+0x8e` at 27253).
        let attacker_name = self.monster_name(attacker);
        let defender_name = self.monster_name(defender);
        let dead = {
            let m = self.monsters.get_mut(&attacker).expect("checked above");
            m.energy -= cost;
            let m = self.monsters.get_mut(&defender).expect("checked above");
            // 27244-27247: the DLL clamps the subtraction to what the
            // defender has left, so a kill lands the HP on exactly 0
            // rather than going negative. Transcribed literally, but be
            // clear that it is NOT observable here and no test can pin
            // it: the only reader of the value is the `<= 0` kill test on
            // the next line, which is unchanged by the clamp, and a kill
            // removes the instance in `monster_died` before anything else
            // can look. Kept for fidelity to the decompile, not for
            // behaviour.
            m.current_hp -= result.damage.min(m.current_hp);
            m.current_hp <= 0
        };
        // The DamageShield(0x48) bite: `genrdn(1, max(val+1,1))` off the
        // ATTACKER's HP, then CAPPED at the attacker's maximum. The
        // attacker is never checked for death here — the DLL leaves a
        // shield-drained monster standing at whatever HP it lands on.
        //
        // The two arms differ, deliberately: the survivor block sits
        // inside the `damage >= 1` else-arm (27283), but the KILL block at
        // 27307 has no damage guard at all — a swing that kills for zero
        // damage (only reachable against an instance already sitting at
        // 0 HP) still draws. Ported as written.
        //
        // The value is read up front because the DLL reads it off the
        // defender's record AFTER `check_kill_monster` has freed it
        // (27307 passes the stale `puVar2`); we cannot read a removed
        // instance, so we snapshot instead of reproducing the read of
        // freed memory.
        let shield_value = self.monster_ability_value(defender, Ability::DamageShield);
        if dead {
            let exp = self.monster_died(defender, None);
            self.apply_damage_shield(attacker, shield_value);
            self.broadcast_to_room(
                attacker_room,
                None,
                &text::capitalize_first(text::monster_killed_monster(
                    &attacker_name,
                    &defender_name,
                )),
            );
            // 27327: `distribute_experience` runs AFTER the kill line.
            if let Some(exp) = exp {
                self.split_kill_experience(defender, None, exp);
            }
            return;
        }
        if result.damage >= 1 {
            self.apply_damage_shield(attacker, shield_value);
        }
        use crate::combat::Outcome;
        let line = match result.outcome {
            Outcome::NoDamage => {
                text::monster_glanced_off_monster(&attacker_name, &defender_name)
            }
            Outcome::Parried => text::monster_dodged_monster(&defender_name, &attacker_name),
            Outcome::Dodged => text::monster_missed_monster(&attacker_name, &defender_name),
            Outcome::Hit | Outcome::Critical => {
                text::monster_attacked_monster(&attacker_name, &defender_name)
            }
        };
        self.broadcast_to_room(defender_room, None, &text::capitalize_first(line));
    }

    /// The DamageShield roll of `attack_monster_monster` (27283-27296),
    /// shared by its two arms. A zero value means the defender has no
    /// shield: no draw at all. The result is CAPPED at the attacker's
    /// template maximum (27293-27295 assigns the max down onto anything
    /// above it) — the bite itself only ever subtracts.
    fn apply_damage_shield(&mut self, attacker: MonsterInstanceId, value: i32) {
        if value == 0 {
            return;
        }
        let bite = self.rng.roll(1, (value + 1).max(1));
        let max_hp = self
            .monsters
            .get(&attacker)
            .and_then(|m| self.content.monsters.get(&m.template))
            .map_or(0, |t| t.hitpoints);
        if let Some(m) = self.monsters.get_mut(&attacker) {
            m.current_hp = (m.current_hp - bite).min(max_hp);
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
        use crate::content::SaveClass;
        let Ok(raw_id) = u16::try_from(form.accuracy) else {
            return false;
        };
        let Some(spell) = self.content.spells.get(&SpellId(raw_id)).cloned() else {
            return false; // unknown id: skip (the DLL would return 0)
        };
        // The monster path's match gate (23777) is the SAME `{0, 2, 6, 8}`
        // set `cast_user_target` 41460 tests, so it reuses the predicate.
        // No self case exists here — a monster is never its own victim —
        // so unlike the player command path there is no 41434 divert to
        // sequence ahead of it.
        if !spell.match_type.accepts_user() {
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
                // TextBlock (148): run the block's script on the VICTIM
                // (monster_cast case 0x94, 23700-23701) — the M7 slice-6
                // quest VM; no display of its own.
                Ability::TextBlock => {
                    if let Ok(block) = u16::try_from(amount) {
                        self.perform_text_block_as_special_command(
                            victim,
                            crate::content::TextBlockId(block),
                        );
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
                    self.summon_spawn(amount, location, SummonLink::HuntUser(victim));
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
                        self.summon_spawn(amount_self, location, SummonLink::None);
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
    ///
    /// The two halves are separable because `attack_monster_monster`
    /// interleaves its own kill line between them (27322-27327): see
    /// [`Core::monster_died`] and [`Core::split_kill_experience`].
    fn monster_killed(&mut self, id: MonsterInstanceId, killer: Option<SessionId>) {
        if let Some(exp) = self.monster_died(id, killer) {
            self.split_kill_experience(id, killer, exp);
        }
    }

    /// `check_kill_monster` alone (`death.md` §4) — removal, respawn
    /// bookkeeping, the coin/loot drop and the death announcement.
    /// Returns the experience pot for [`Core::split_kill_experience`], or
    /// `None` if the instance was already gone.
    fn monster_died(&mut self, id: MonsterInstanceId, killer: Option<SessionId>) -> Option<u64> {
        let instance = self.monsters.remove(&id)?;
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

        // ORACLE (oracle_m6_arena_fight.raw) + check_kill_monster
        // 21361-21390: the deathmsg record's LINE 3 prints verbatim ("The
        // giant rat falls to the ground with a tortured squeak."); the
        // generic "%s is dead." is the no-record fallback.
        let announcement = self
            .content
            .monsters
            .get(&template)
            .and_then(|t| t.death_msg)
            .and_then(|id| self.content.messages.get(&id))
            .and_then(|m| m.lines.get(2))
            .filter(|l| !l.is_empty())
            .cloned()
            .unwrap_or_else(|| text::monster_dead(&name));
        match killer {
            Some(killer) => {
                self.output_line(killer, &announcement);
                self.broadcast_to_room(room, Some(killer), &announcement);
            }
            None => self.broadcast_to_room(room, None, &announcement),
        }
        Some(exp)
    }

    /// `distribute_experience` (`death.md` §5) — the equal split among the
    /// killer and everyone engaged on the dead instance. Split out of
    /// [`Core::monster_killed`] so `attack_monster_monster` can print its
    /// kill line between the two, which is the DLL's order at
    /// 27322-27327.
    fn split_kill_experience(
        &mut self,
        id: MonsterInstanceId,
        killer: Option<SessionId>,
        exp: u64,
    ) {
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
            // Permadeath (`death.md` §2c). Fame rides the delete event
            // for the account evil bank (crime.md §8).
            let name = player.name.clone();
            let fame = player.fame;
            self.output_line(session, "You have no lives remaining!");
            self.sessions.remove(&session);
            self.events.push(Event::DeleteCharacter { name, fame });
            self.events.push(Event::Disconnect(session));
            return;
        }
        // Miracle respawn: full HP/mana at the recall room — the
        // criminal temple for fame >= 0x28 (crime.md §6.4).
        let recall = if player.fame >= 0x28 {
            self.config.criminal_recall_location
        } else {
            self.config.recall_location
        };
        player.current_hp = derived.max_hp;
        player.current_mana = derived.max_mana;
        player.location = recall;
        *aided = false;
        let lives = player.lives;
        let snapshot: Box<Player> = player.clone();
        self.output_line(session, "But, due to a miracle, you have been saved.");
        self.output_line(session, &format!("You have {lives} lives left."));
        self.broadcast_to_room(
            recall,
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
        // Worn loop skips `+0x2f4 != 0` items (24788-24790); the weapon
        // term is a seed outside the loop and stays ungated.
        let ratings: i32 = i32::from(weapon.map_or(0, |w| w.accuracy))
            + player
                .worn
                .iter()
                .filter_map(|(id, _)| self.content.items.get(id))
                .filter(|i| i.item_type == 0)
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
        // +0x70a is ONE shared accumulator over abilities 0x16/0x69/0x6a,
        // and `update_dynamic_with_ability` (37549-37559) keeps the MAX
        // contribution, not the sum (seeded -32000, reset 0 if untouched) —
        // so a lone negative value survives as negative.
        let dyn_accuracy = [0x16, 0x69, 0x6a]
            .iter()
            .filter_map(|&id| bag.max_value(accuracy_ability(id)))
            .max()
            .unwrap_or(0);
        // Unarmed MA modes (combat.md "Unarmed attack modes",
        // move_player_to_fighter 24520-24660 + add-ons 24890-24916):
        // the stored attack mode (autocombat +8) picks the style —
        // 1 fists of fury (Punch 0x1d): min L*V/8+2, max (L+3)*V/4+6;
        // 2 lightning feet (Kick 0x1e): max L*V/6+7;
        // 3 flying feet (JumpKick 0x23): max L*V/6+8 — L = level capped
        // at 20, V = the folded style ability, plus the per-style
        // ACY/Dmg add-on pair. ORACLE pin: Nekojin Mystic L1 V1 Str40
        // punched raw 2..6 (shown 1..5 through the rat's DR 1).
        use crate::combat::AttackType;
        let mode = match self.sessions.get(&session) {
            Some(Session::InGame { attack_mode, .. }) => *attack_mode,
            _ => AttackType::Normal,
        };
        // (style V ability, ACY add-on, Dmg add-on) per unarmed mode.
        let style = match mode {
            AttackType::MartialArts1 => Some((0x1d, 0x59, 0x5c)),
            AttackType::MartialArts2 => Some((0x1e, 0x5a, 0x5d)),
            AttackType::MartialArts3 => Some((0x23, 0x5b, 0x5e)),
            _ => None,
        };
        let fold = |id: u16| bag.value(Ability::from_id(id).expect("MA ability in the enum"));
        let (v, style_acy, style_dmg) = match style {
            Some((v_id, acy_id, dmg_id)) if weapon.is_none() => {
                let v = fold(v_id);
                if v > 0 {
                    (v, fold(acy_id), fold(dmg_id))
                } else {
                    (0, 0, 0)
                }
            }
            _ => (0, 0, 0),
        };
        let accuracy = if mode == AttackType::Backstab {
            // Mode-4 accuracy (move_player_to_fighter 24817-24841):
            // (Agl + Stealth)/2 + Agl/2 + BSAccu (0x74). The +0x7d4
            // flag mods (+5/-15) and the +0x6f5&0x80 -10 are untraced
            // runtime bits — omitted, ORACLE-VERIFY.
            let stealth = match self.sessions.get(&session) {
                Some(Session::InGame { derived, .. }) => derived.stealth,
                _ => 0,
            };
            (agl + stealth) / 2
                + agl / 2
                + bag.value(Ability::from_id(0x74).expect("BSAccu in the enum"))
                + dyn_accuracy
        } else {
            (str_ - 50) / 3
                + 2 * ((combat - 1) * isqrt(level) + 2 * combat + level / 2 + skill / 2 - 2)
                + (agl - 50) / 6
                + dyn_accuracy
                + style_acy
        };

        // Weapon damage (or the unarmed defaults), plus the Strength
        // bonuses: max += (Str-50)/10; min += 2*(Str-100)/10 when positive.
        let (base_min, base_max) = match weapon {
            Some(w) => (i32::from(w.min_damage), i32::from(w.max_damage)),
            None if v > 0 => {
                let l = level.min(20);
                let max = match mode {
                    AttackType::MartialArts2 => l * v / 6 + 7,
                    AttackType::MartialArts3 => l * v / 6 + 8,
                    _ => (l + 3) * v / 4 + 6,
                };
                (l * v / 8 + 2 + style_dmg, max + style_dmg)
            }
            None => (1, 4),
        };
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

    /// `get_armour_rating` (`0x1f1eb`, decompiled.c 16956) — the pair the
    /// status line prints as `Armour Class: A/B`, each ÷10 at the call
    /// site (31553-31556). Returns `(ac, dr)`, both in tenths.
    ///
    /// AC accumulates the WIELDED weapon's `+0x342` (16983-16987) plus
    /// every worn slot's; DR accumulates the same slots' `+0x39c`. Worn
    /// items whose `+0x2f4` (type) is non-zero are skipped (16995-16996) —
    /// the weapon is not, it is read before the loop. The AC(Blur)
    /// ability (10) is then divided by the heaviest worn armour code
    /// (`+0x396`: 9 → /4, 7 or 8 → /3, 3..=6 → /2), scaled ×10, and added
    /// to BOTH sides (17023-17044); being encumbrance-derived is exactly
    /// what separates it from AC(2). Finally AC(2) × 10 joins the AC side
    /// only, and AC — not DR — is clamped at 0 (17045-17048).
    ///
    /// Both ability reads come from ONE call each, via a convention that
    /// is easy to misread: `get_user_ability_value` totals `param_1` into
    /// its return AND `param_4` into the out-param (36832-36864). So
    /// `(7, -1, player, 2, &local_c)` yields DR(7) in the discarded return
    /// and **AC(2)** in `local_c` — DR(7) does not reach this display at
    /// all. Ours reads the accumulated [`Core::ability_bag`], the same
    /// analogue [`Core::build_player_defender`] uses for Dodge.
    ///
    /// MEASURED (`re/oracle/oracle_dodge_parry_acc-{high,mid}.raw`,
    /// charm.md §8.3): Σac 125 / Σdr 8 printed `12/0`; adding an AC(2)
    /// -20 item took it to `0/0`, not `-8/0`, which is what pins the
    /// clamp and the ×10.
    fn get_armour_rating(&self, session: SessionId) -> (i32, i32) {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            unreachable!("caller checked the session");
        };
        let mut ac: i32 = player
            .weapon
            .and_then(|(id, _)| self.content.items.get(&id))
            .map_or(0, |w| i32::from(w.evasion));
        let mut dr: i32 = 0;
        // The heaviest worn armour code seen, as the three flag bytes
        // (local_5..local_8) collapse at 17023-17037: 9 beats 7/8 beats
        // 3..=6.
        let mut blur_divisor = 1;
        for item in player
            .worn
            .iter()
            .filter_map(|(id, _)| self.content.items.get(id))
            .filter(|i| i.item_type == 0)
        {
            ac += i32::from(item.evasion);
            dr += i32::from(item.damage_resist);
            let divisor = match item.armour_req {
                9 => 4,
                7 | 8 => 3,
                3..=6 => 2,
                _ => 1,
            };
            blur_divisor = blur_divisor.max(divisor);
        }
        let bag = self.ability_bag(player);
        let blur = bag.value(Ability::ACBlur);
        let scaled = if blur == 0 { 0 } else { (blur / blur_divisor) * 10 };
        ac += scaled;
        dr += scaled;
        ac += bag.value(Ability::AC) * 10;
        (ac.max(0), dr)
    }

    /// EXACT (decompile): defender view of a player. Naked: evasion 0
    /// (Σ worn `+0x342` ÷ 10; the dynamic AC accumulator +0x70c joins when
    /// content carries AC(2) buffs), armor 0 (Σ worn `+0x39c`, raw — the
    /// ÷10 happens in `calculate_attack`); parry is the word[10]
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
        // The worn loop skips `+0x2f4 != 0` items (24788-24790) — the
        // type gate wraps the WHOLE accumulation block (evasion, DR, and
        // the attacker's ratings alike). The weapon seed is ungated.
        let worn = || {
            player
                .worn
                .iter()
                .filter_map(|(id, _)| self.content.items.get(id))
                .filter(|i| i.item_type == 0)
        };
        // `[3]` — Σ worn `+0x39c` (24789), RAW. The word is a tenths-scale
        // quantity: `calculate_attack` subtracts `[3]/10` from damage
        // (25335), and `move_monster_to_fighter` reaches the same scale from
        // the other side by multiplying the whole-unit monster `dr` by 10
        // (25180). Shipped item `dr` is already pre-multiplied.
        let bag = self.ability_bag(player);
        // `[3] += player+0x7b6` (24888), raw and in the same tenths as the
        // item sum. `update_dynamic_with_ability` case 7 fills it (37475).
        //
        // PENDING: the DLL then scales `[3]` by `(player+0x7b8 + 100)/100`
        // (24889) — a DR PERCENT we do not apply, because the ability that
        // writes `+0x7b8` is not readable here. Ghidra reaches that store
        // via `if (param_2 != 0xe)`, i.e. ability 14, but its own brace
        // nesting in that region is provably wrong (it emits `case 0x4b/
        // 0x4c/0x57` *after* the inner switch closes), and ability 14 is
        // `RoomIllu` — its shipped carriers hold 9999 on a portal and 100
        // on two rings, which is light, not a doubling of damage
        // resistance. `AlterDRpercent`(99) is what the id table describes
        // for this, and it has two shipped spell carriers, but no visible
        // writer in `update_dynamic_with_ability`. Wiring either one on
        // this evidence would be a guess with live consequences, so the
        // term is left unapplied and named. Settle it from the 16-bit
        // disassembly or a capture before porting.
        let armor: i32 =
            worn().map(|i| i32::from(i.damage_resist)).sum::<i32>() + bag.value(Ability::DR);
        // `[1]` — Σ `+0x342` (24788), ÷10 at 24866, then `+= player+0x70c`
        // (24885, `update_dynamic_with_ability` case 2 at 37469). Drives
        // the quadratic to-hit term at 25311, never the soak. The AC term
        // is added AFTER the ÷10, which is why it is unscaled here while
        // `get_armour_rating` multiplies it by 10 — that accumulator has
        // not been divided yet. Both agree in display units.
        let defense: i32 = (i32::from(
            player
                .weapon
                .and_then(|(id, _)| self.content.items.get(&id))
                .map_or(0, |w| w.evasion),
        ) + worn().map(|i| i32::from(i.evasion)).sum::<i32>())
            / 10
            + bag.value(Ability::AC);
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
    /// a monster — evasion [1] = AC, armor [3] = DR*10, parry [8] = the
    /// Dodge(0x22) ability (25185-25186), crit hard-zeroed (25187). The
    /// ability fold joins the words: evasion += AC(2) (25194-25195)
    /// through `get_monster_ability_value` — template rows AND
    /// active-slot debuffs — and the soak += DR(7) RAW (25199-25200; the
    /// *10 scale applies only to the template word). ONE build for every
    /// defender: the DLL runs this same function for the
    /// player-attacks-monster path and for both sides of
    /// `attack_monster_monster`, so the Dodge word is not m-v-m-specific.
    ///
    /// MEASURED (charm.md §8.3, 2026-07-26): the parry word is a LIVE
    /// gameplay change on the player-attacks-monster path — 167 of the 1101
    /// shipped templates carry Dodge(0x22) at values 10..200, and the chance
    /// `parry*10 / (accuracy/8)` (capped 95, `calculate_attack` 25336-25360)
    /// turns roughly 28-80% of connecting player swings into zero-damage
    /// parries. An expedition ground giant bats (Dodge 20) at two accuracies,
    /// changing nothing else:
    ///
    ///   accuracy 23 (denominator 2, predicted 0.95): 28/31 parried,
    ///     CI [0.743, 0.980]
    ///   accuracy 43 (denominator 5, predicted 0.40): 26/58 parried,
    ///     CI [0.317, 0.585]
    ///
    /// Both contain their prediction and the two intervals are DISJOINT, so
    /// the formula's division by accuracy is real and roughly the right size
    /// — a rule that ignored accuracy could not move the same target from
    /// ~90% to ~45%. The transcripts also favour the cap being 95, not 100.
    ///
    /// ORACLE-VERIFY, narrowed: the linear point is consistent but not
    /// PINNED — 0.50 (d=4) and 0.333 (d=6) both sit inside its interval, and
    /// separating them needs ~92 and ~207 connecting swings against the 58
    /// collected. The measured 0.448 leans toward d=4, i.e. toward our
    /// ACCURACY derivation reading a few points high, rather than toward the
    /// parry formula being wrong. Pinned for shape by
    /// `game_combat.rs::monster_dodge_ability_parries_player_swings`.
    ///
    /// UNPORTED, all inert on shipped data but NOT all dead code:
    /// - the alignment-accuracy block at 25107-25118 writes evasion word
    ///   [2] from AlignmentAccuracy(0x18). It is genuinely dead on the
    ///   m-v-m path — `attack_monster_monster` passes
    ///   `FUN_0042a15c`'s own return value, so the `!=` never fires — but
    ///   it IS live when a PLAYER attacks: 26248 passes the player's
    ///   alignment code (`FUN_0042a12e`). Zero shipped templates carry
    ///   0x18 so word [2] stays 0 anyway, and note the DLL immediately
    ///   overwrites the 0x19 read with the 0x18 one, which makes the
    ///   single template carrying 0x19 (`gravedigger`, value 15) inert
    ///   too;
    /// - Shadow(9) adds 10 to evasion word [2] (25120-25122); zero
    ///   templates carry it;
    /// - DefenseModifier(0x68) rides word [0x92] (25201-25202) into the
    ///   attacker's accuracy inside `calculate_attack` (25291) — our
    ///   [`crate::combat`] engine has no term for it; zero templates
    ///   carry it.
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
            parry: self.monster_ability_value(id, Ability::Dodge),
            crit_rating: 0,
        }
    }

    /// EXACT (decompile 0x2a0c8 `compute_energy_used`):
    /// EU = speed*1000 / ((combat*level + 45) * (Agl+150) * 1500/9000) + bonus,
    /// divide-by-zero guard = 50. Speeds (combat.md mode table): armed =
    /// the weapon's `+0x3de` speed (0-speed weapons like "flurry of
    /// blades" hit the 6-swing round cap — intentional data); unarmed by
    /// attack mode — 1150 fists of fury / 1400 kicks / 1900 jumpkick /
    /// 1200 plain fists. (The flagged +0x7c8&2 variants are untraced.)
    fn player_energy_used(&self, session: SessionId) -> i32 {
        let Some(Session::InGame { player, attack_mode, .. }) = self.sessions.get(&session)
        else {
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
        use crate::combat::AttackType;
        let speed = match player
            .weapon
            .and_then(|(id, _)| self.content.items.get(&id))
        {
            Some(w) => i32::from(w.speed),
            None => match attack_mode {
                AttackType::MartialArts1 => 1150,
                AttackType::MartialArts2 => 1400,
                AttackType::MartialArts3 => 1900,
                _ => 1200,
            },
        };
        speed * 1000 / den
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
        self.output(session, &format!("{text}\n"));
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
        let class = choice.expect("validated above");
        // crime.md §6.8: the Lawful question is only asked when fame < 1
        // — a rerolling criminal (account-banked evil, §8) skips it and
        // starts with the restored points.
        if profile.saved_evil >= 1 {
            self.finish_creation(session, profile, race, class, false);
            return;
        }
        self.sessions.insert(
            session,
            Session::ChoosingLawful { profile, race, class },
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
        self.finish_creation(session, profile, race, class, lawful);
    }

    /// roll_stats + realm entry (the tail both creation paths share).
    fn finish_creation(
        &mut self,
        session: SessionId,
        profile: AccountProfile,
        race: RaceId,
        class: ClassId,
        lawful: bool,
    ) {
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
                at_prompt: false,
                attack_mode: crate::combat::AttackType::Normal,
                delay: 0,
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
            // Committed Lawful starts at -51 (crime.md §2.6: the
            // creation good-path prompt writes 0x544=0xF6 AND fame -51);
            // otherwise the account-banked evil restores (§8 — crime
            // follows the account; negatives clamp to 0, §6.8).
            fame: if lawful { -51 } else { profile.saved_evil.max(0) },
            ansi: self.config.ansi,
            warn_on_evil: true,
            hidden: false,
            sneak_armed: false,
            innate: [(None, 0); 30],
            quest_flags: 0,
            gang: String::new(),
            gang_flags: 0,
        };
        let derived = self.derive_for(&player);
        player.current_hp = derived.max_hp;
        player.current_mana = derived.max_mana;
        player
    }

    /// The gang record for a display name or key (`get_gang_data`
    /// uppercases its input, gangs.md §0).
    pub fn gang(&self, name: &str) -> Option<&crate::gang::Gang> {
        self.gangs.get(&name.to_uppercase())
    }

    /// Takes all events produced since the last drain.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Removes a session whose connection dropped: same as quitting (persist
    /// + departure broadcast). Sessions still in creation just vanish.
    pub fn detach(&mut self, session: SessionId) {
        self.pending_disband.remove(&session);
        match self.sessions.get(&session) {
            Some(Session::InGame { .. }) => self.complete_quit(session),
            Some(_) => {
                self.sessions.remove(&session);
                self.events.push(Event::Disconnect(session));
            }
            None => {}
        }
        self.reprompt_disturbed();
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
        // Hidden type-6 exits are no-exits until found (theft.md §9).
        if self.exit_hidden6(from, direction as usize as u8, &exit) {
            self.output_line(session, text::NO_EXIT);
            return;
        }
        // Locked pickable exits block until picked (theft.md §8; the
        // door-open command family is still unmodeled — a locked type-2
        // door refuses with the closed-door line, secret types stay
        // masked as no-exit. Wordings ORACLE-VERIFY).
        if matches!(exit.exit_type, 2 | 7 | 0xb)
            && self.exit_lock_state(from, direction as usize as u8, &exit) == 2
        {
            if exit.exit_type == 2 {
                self.output_line(session, text::DOOR_CLOSED);
            } else {
                self.output_line(session, text::NO_EXIT);
            }
            return;
        }
        // Alignment-restricted exits (type 0x14, crime.md §6.3
        // move_user 12433-12448): fame below paramA = too good, above
        // paramB = too evil.
        if exit.exit_type == 0x14 {
            let fame = i32::from(self.player(session).fame);
            if fame < exit.param {
                self.output_line(session, text::EXIT_TOO_GOOD);
                return;
            }
            if fame > exit.param2 {
                self.output_line(session, text::EXIT_TOO_EVIL);
                return;
            }
        }
        // give_monsters_a_free_attack (23846-23905), before the move
        // commits: one room roll per departure — drawn even with nothing
        // to hit — then the first eligible monster (roll <= aggression;
        // passive modes and class 0x25 only at their locked runner; mode
        // 6 spares fame >= 0x50) takes a full swing sequence. Only a
        // DEATH aborts the move; the +0x6f0 gate caps it at one free
        // attack per medium tick.
        //
        // NO charm gate anywhere in it (23865-23895, re-read for M7
        // slice 5): the locked-runner arm is `sameas(mon+0x1a, fleer)`
        // plus `+0x116 == 0` and nothing else, so a pet is held off a
        // fleeing owner by its SUPPRESSION alone — charm.md §2.4's
        // "what it suppresses" list, third entry. Release the pet
        // without clearing `+0x116` (the §4.3 "friend" outcome) and it
        // still declines the free swing; clear `+0x116` and it takes it.
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
        // Sneak movement (theft.md §11.1): the armed bit is consumed by
        // this move; the normal leave/arrive broadcasts are replaced by
        // perception-FILTERED "You notice %s sneaking..." lines, and the
        // sneaker keeps the hidden byte. A NORMAL move clears it.
        let sneaking = matches!(self.sessions.get(&session),
            Some(Session::InGame { player, .. }) if player.sneak_armed);
        if sneaking {
            // Self-awareness roll vs own Perception (12574+): a low roll
            // warns the sneaker — no effect on concealment.
            let perception = match self.sessions.get(&session) {
                Some(Session::InGame { derived, .. }) => derived.perception,
                _ => 0,
            };
            if self.rng.roll(0, 100) < perception {
                self.output_line(session, "You make a sound as you enter the room!");
            }
            self.broadcast_sneak(from, session, &text::sneak_out(&name, direction));
        } else {
            self.broadcast_to_room(from, Some(session), &text::left_via(&name, direction));
        }
        match self.sessions.get_mut(&session) {
            Some(Session::InGame { player, moved_this_round, trail, .. }) => {
                player.location = exit.dest;
                player.sneak_armed = false;
                if !sneaking {
                    player.hidden = false;
                }
                // +0x6f4 bit 6 (12501) + the pursuit breadcrumb push.
                *moved_this_round = true;
                trail.insert(0, exit.dest);
                trail.truncate(20);
            }
            _ => unreachable!("mover is in game"),
        }
        if sneaking {
            self.broadcast_sneak(
                exit.dest,
                session,
                &text::sneak_in_from(&name, direction.opposite()),
            );
        } else {
            self.broadcast_to_room(
                exit.dest,
                Some(session),
                &text::walks_in_from(&name, direction.opposite()),
            );
        }
        self.show_room(session);
    }

    /// A perception-filtered room broadcast (the DLL's tell_room
    /// perception-filter flag): each other player rolls genrdn(0,100)
    /// against their own Perception and only sees the line on a pass.
    fn broadcast_sneak(&mut self, room: RoomId, mover: SessionId, line: &str) {
        let candidates: Vec<(SessionId, i32)> = self
            .sessions
            .iter()
            .filter_map(|(id, s)| match s {
                Session::InGame { player, derived, .. }
                    if *id != mover && player.location == room =>
                {
                    Some((*id, derived.perception))
                }
                _ => None,
            })
            .collect();
        for (id, perception) in candidates {
            if self.rng.roll(0, 100) < perception {
                self.output_line(id, line);
            }
        }
    }

    fn show_room(&mut self, session: SessionId) {
        self.render_room(session, true);
    }

    /// One exit's obvious-exits entry, or `None` when hidden. ORACLE
    /// (oracle_m6_arena_fight.raw): closed type-2 doors render as
    /// "closed door <dir>"; action exits (10) and unfound secrets
    /// (7/0xb — found-state is unmodeled runtime, unfound is the shipped
    /// default) stay hidden. Other typed variants ORACLE-VERIFY.
    fn exit_entry(
        &self,
        room: RoomId,
        d: Direction,
        exit: &crate::content::Exit,
    ) -> Option<String> {
        match exit.exit_type {
            10 | 7 | 0xb => None,
            6 if self.exit_hidden6(room, d as usize as u8, exit) => None,
            2 if exit.door_closed => {
                Some(format!("closed door {}", text::direction_shown(d)))
            }
            _ => Some(text::direction_shown(d).to_string()),
        }
    }

    /// The `exits` command: just the obvious-exits line (oracle).
    fn show_exits_line(&mut self, session: SessionId) {
        let player = self.player(session);
        let room_id = player.location;
        let room = &self.content.rooms[&room_id];
        let exits: Vec<String> = Direction::ALL
            .into_iter()
            .filter_map(|d| {
                room.exits[d as usize]
                    .as_ref()
                    .and_then(|e| self.exit_entry(room_id, d, e))
            })
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
        out.push_str(text::color::ROOM_NAME);
        out.push_str(&room.name);
        out.push_str(text::color::RESET);
        out.push('\n');
        if full {
            for (i, line) in room.description.iter().enumerate() {
                out.push_str(text::color::PLAIN);
                if i == 0 {
                    out.push_str("    ");
                }
                out.push_str(line);
                out.push_str(text::color::RESET);
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
            out.push_str(&format!(
                "{}You notice {} here.{}\n",
                text::color::NOTICE,
                notices.join(", "),
                text::color::RESET
            ));
        }

        // Players first, then live monsters (oracle: NPCs share the line).
        let mut others: Vec<&str> = self
            .in_game_sessions()
            .filter(|(id, p)| *id != session && p.location == room.id && !p.hidden)
            .map(|(_, p)| p.name.as_str())
            .collect();
        others.extend(
            self.monsters
                .values()
                .filter(|m| m.location == room.id)
                .map(|m| m.name.as_str()),
        );
        if !others.is_empty() {
            // Oracle palette: "0;35 Also here: 1;35 <name> 0m 0;35 ." —
            // each name bright magenta inside the magenta line.
            out.push_str(text::color::ALSO);
            out.push_str(text::ALSO_HERE);
            let painted: Vec<String> = others
                .iter()
                .map(|n| {
                    format!(
                        "{}{n}{}{}",
                        text::color::ALSO_NAME,
                        text::color::RESET,
                        text::color::ALSO
                    )
                })
                .collect();
            out.push_str(&painted.join(", "));
            out.push('.');
            out.push_str(text::color::RESET);
            out.push('\n');
        }

        let exits: Vec<String> = Direction::ALL
            .into_iter()
            .filter_map(|d| {
                room.exits[d as usize]
                    .as_ref()
                    .and_then(|e| self.exit_entry(room.id, d, e))
            })
            .collect();
        out.push_str(text::color::GREEN);
        out.push_str(text::OBVIOUS_EXITS);
        if exits.is_empty() {
            out.push_str(text::NO_EXITS);
        } else {
            out.push_str(&exits.join(", "));
        }
        out.push_str(text::color::RESET);
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
        // Erase a dangling prompt before async output lands on it
        // (the DLL prefixes every burst with ESC[79D ESC[K).
        let erase = match self.sessions.get_mut(&session) {
            Some(Session::InGame { at_prompt, .. }) if *at_prompt => {
                *at_prompt = false;
                true
            }
            _ => false,
        };
        // Per-user ANSI overrides the global once a character is
        // attached; login/creation sessions follow the server global.
        let ansi = match self.sessions.get(&session) {
            Some(Session::InGame { player, .. }) => player.ansi,
            _ => self.config.ansi,
        };
        let mut text = if ansi {
            text.to_string()
        } else {
            text::strip_ansi(text)
        };
        if erase {
            // ANSI: erase the dangling prompt in place (ESC[79D ESC[K);
            // plain terminals get a newline away from it instead.
            text = if ansi {
                format!("\r\x1b[K{text}")
            } else {
                format!("\r\n{text}")
            };
        }
        self.events.push(Event::Output { session, text });
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
