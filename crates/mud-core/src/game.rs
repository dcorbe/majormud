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
    /// the granting effect ends — wiring lands in slice 4). Display order
    /// is computed at render (level, then name), not storage order.
    pub spellbook: BTreeMap<SpellId, bool>,
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

/// The cast success roll (spec §3 step 5; decompiled 38150+ family):
/// `base_chance >= 200` auto-succeeds without consuming a roll; otherwise
/// `chance = min(SC + base_chance, 98)` and the cast succeeds when
/// `genrdn(0,100) < chance`. There is no floor: a chance at or below 0
/// (possible only with negative SC, i.e. a non-caster) never succeeds.
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
    /// HP regen, mana regen for every in-game player. (Poison and bleed/aid
    /// join in M3 with the death system.)
    fn slow_update(&mut self) {
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in sessions {
            let Some(Session::InGame { player, derived, .. }) = self.sessions.get(&id) else {
                continue;
            };
            let (max_hp, max_mana) = (derived.max_hp, derived.max_mana);
            let bag = self.ability_bag(player);
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
                    (i32::from(player.level) + 20) * i32::from(player.stats.health) / 750;
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
                    1 => i32::from(player.stats.intellect),
                    2 => i32::from(player.stats.wisdom),
                    3 => (i32::from(player.stats.wisdom) + i32::from(player.stats.intellect)) / 2,
                    4 => i32::from(player.stats.charm),
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
            .insert(id, Session::InGame { player: Box::new(player), derived, exiting: None, target: None, aided: false, energy: PLAYER_ENERGY_MAX, cast_this_round: false });
        self.show_room(id);
        self.show_prompt(id);
        id
    }

    /// The player's accumulated ability modifiers: race + class permanents
    /// now; gear and active spells join via the same bag in later milestones.
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
        abilities
    }

    /// `update_dynamic_stats` + `calculate_secondary_stats`.
    fn derive_for(&self, player: &Player) -> Derived {
        let abilities = self.ability_bag(player);
        let class = self.content.classes.get(&player.class);
        let race_hp = self
            .content
            .races
            .get(&player.race)
            .map_or(0, |r| i32::from(r.hp_per_level));
        derive(&StatInputs {
            level: i32::from(player.level),
            stats: player.stats,
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
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            return;
        };
        let caster_group = self
            .content
            .classes
            .get(&player.class)
            .map_or(0, |c| c.caster_group);
        let prompt = text::prompt(player.current_hp, player.current_mana, caster_group);
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
            stats: player.stats,
            derived,
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
            // the do-not-know line (MEASURED §8.6/§8.9).
            Command::Cast(args) => self.cast_command(session, &args),
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
        let line = text::health_line(player.current_hp, derived.max_hp);
        self.output_line(session, &line);
    }

    /// `spells` — the learned-book listing (spellcasting.md §8.5). Rows
    /// sort by required power ascending, then name; display order is
    /// computed here, not stored. Format VERIFIED oracle_spell_train.raw.
    fn spells_command(&mut self, session: SessionId) {
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
        let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&session) else {
            unreachable!("train dispatched from in-game session");
        };
        player.coins.deduct_copper(cost, ratios);
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

        // Mode-2 recompute: derived stats refresh, current HP/mana kept.
        let refreshed = self.derive_for(self.player(session));
        if let Some(Session::InGame { derived, .. }) = self.sessions.get_mut(&session) {
            *derived = refreshed;
        }
        self.output_line(session, &text::train_success(new_level));
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
        if let Some(Session::InGame { target, .. }) = self.sessions.get_mut(&session) {
            // Oracle: attacking while already engaged prints *Combat Off*
            // before the new *Combat Engaged*.
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
        let args = args.trim();
        if args.is_empty() {
            // MEASURED (§8.9): bare cast is a syntax line, not an error.
            self.output_line(session, text::SYNTAX_CAST);
            return;
        }
        // Gate 2: resolve against the learned book. Resolution comes BEFORE
        // the already-cast check: the DLL dispatcher resolves the spell and
        // passes a pointer into cast_no_target, whose mode-2 flag "skips
        // confusion/fear/round checks" — so the round gate lives inside,
        // after resolution. ORACLE-VERIFY: second-cast-unknown ordering
        // unmeasured (probe: `c blur` then `c zzz` in one round).
        let resolved = self.resolve_spell_from_book(self.player(session), args);
        let Some((spell_id, _target)) = resolved else {
            self.output_line(session, &text::dont_know_cast(args));
            return;
        };
        // Gate 3: one cast per round — even for energy-0 spells
        // (MEASURED §8.6).
        if let Some(Session::InGame { cast_this_round: true, .. }) = self.sessions.get(&session)
        {
            self.output_line(session, text::ALREADY_CAST);
            return;
        }
        let spell = &self.content.spells[&spell_id];
        // Gate 4: level vs required_power (spec §2) — unreachable via
        // scroll-learned books, reachable via slice-4 temp spells.
        if i32::from(self.player(session).level) < i32::from(spell.required_power) {
            self.output_line(session, text::SPELL_TOO_POWERFUL);
            return;
        }
        // Gate 5: round energy — exactly like an M3 attack without energy,
        // a silent no-op within the round (no measured message). The
        // deduction itself happens at roll time below.
        let round_cost = i32::from(spell.round_cost);
        let mana_cost = i32::from(spell.mana_cost);
        let base_chance = spell.base_chance;
        let spell_name = spell.name.clone();
        let Some(Session::InGame { energy, player, derived, .. }) = self.sessions.get(&session)
        else {
            return;
        };
        let spellcasting = derived.spellcasting;
        if *energy < round_cost {
            return;
        }
        // Gate 6: mana (MEASURED §8.6) — checked here, deducted at roll
        // time (full on success, half rounded down on a failed roll).
        if player.current_mana < mana_cost {
            self.output_line(session, text::NOT_ENOUGH_MANA);
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
        let Some(Session::InGame { energy, player, .. }) = self.sessions.get_mut(&session) else {
            return;
        };
        // Both outcomes pay the full round cost (spec §3 steps 6-7).
        *energy -= round_cost;
        if succeeded {
            player.current_mana -= mana_cost;
            // Task 11: effects + messages — success is silent until then,
            // observable only via the deductions.
        } else {
            // Half mana rounded down (mmis 1 -> 0 oracle-confirmed §8.6;
            // blur 4 -> 2 §8.9), no effects applied.
            player.current_mana -= mana_cost / 2;
            let caster = player.name.clone();
            let room = player.location;
            self.output_line(session, &text::cast_fail(&spell_name));
            self.broadcast_to_room(room, Some(session), &text::cast_fail_room(&caster, &spell_name));
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
    /// `buy curing`/`buy cure poison` = 25 silver if poisoned (poison is
    /// M5) or 15 SILVER when not — the constants ride in the silver arg
    /// of check_currency (live: 150 copper, oracle_healer2.raw).
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
            // Not-poisoned path only until M5 brings poison: 15 silver.
            let cost = 15 * ratios[0];
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
            self.output_line(session, &text::not_poisoned(&coins));
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
        let Some(Session::InGame { player, target: Some(target), .. }) =
            self.sessions.get(&session)
        else {
            return;
        };
        let target = *target;
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
        let name = tpl.name.clone();
        let forms = tpl.attacks;

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
            if matches!(result.outcome, Outcome::Hit | Outcome::Critical) {
                // Any incoming swing cancels a pending exit (combat logout guard).
                self.cancel_exit(victim);
                self.output_line(victim, &text::monster_hit(&name, "hits", result.damage));
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
        if let Some(Session::InGame { target, energy, .. }) = self.sessions.get_mut(&session) {
            *target = None;
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
        if let Some(Session::InGame { target, energy, .. }) = self.sessions.get_mut(&session)
            && target.is_some()
        {
            *target = None;
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
    /// (item ratings/10 + dynamic AC join in M4/M5), armor 0; parry is the
    /// word[10] formula plus the low-encumbrance bonus (10 - enc/10),
    /// forced -1 when helpless.
    fn build_player_defender(&self, session: SessionId) -> crate::combat::Fighter {
        let Some(Session::InGame { player, .. }) = self.sessions.get(&session) else {
            unreachable!("caller checked the session");
        };
        let parry = if player.current_hp < 1 {
            -1
        } else {
            let encumbrance = self.encumbrance_percent(session);
            let mut p = (i32::from(player.stats.charm) - 50) / 5
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
            .insert(session, Session::InGame { player: Box::new(player), derived, exiting: None, target: None, aided: false, energy: PLAYER_ENERGY_MAX, cast_this_round: false });
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
