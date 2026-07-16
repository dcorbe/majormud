//! The game core: a single-threaded, deterministic state machine.
//!
//! Sessions attach with a loaded (or freshly created) player, feed text input
//! in, and consume `Event`s out. No I/O happens here — the caller owns
//! networking and persistence.

use std::collections::BTreeMap;

use crate::ability::Ability;
use crate::command::{parse, Command, Resolution};
use crate::content::{ClassId, Content, Direction, RaceId, RoomId, StatBlock};
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
    pub cp_unspent: u16,
    pub cp_lifetime: u16,
    pub lives: u16,
    pub experience: u64,
    pub location: RoomId,
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

    /// `deduct_currency`: remove a copper amount, breaking higher coins into
    /// change only when the lower drawers run dry (never consolidating change
    /// upward). Caller must have checked affordability. ORACLE-VERIFY the
    /// original's exact spend order for mixed purses.
    pub fn deduct_copper(&mut self, amount: u64, ratios: [u64; 4]) {
        debug_assert!(self.total_copper(ratios) >= amount);
        let mut due = amount;
        loop {
            let pay = due.min(u64::from(self.copper));
            self.copper -= pay as u32;
            due -= pay;
            if due == 0 {
                return;
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
            } else {
                self.runic -= 1;
                self.platinum += ratios[3] as u32;
            }
        }
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
            coin_ratios: [10, 10, 10, 10],
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

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Output { session: SessionId, text: String },
    Persist(Box<Player>),
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
        player: Player,
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
}

const SLOW_INTERVAL: u64 = 30;
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
}

impl Core {
    pub fn new(content: Content, config: CoreConfig) -> Core {
        let mut scheduler = TickScheduler::new();
        scheduler.schedule_in(SLOW_INTERVAL, Job::Slow);
        scheduler.schedule_in(ENERGY_INTERVAL, Job::Energy);
        let rng = Rng(config.rng_seed | 1);
        Core {
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
        }
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
        let id = MonsterInstanceId(self.next_monster);
        self.next_monster += 1;
        self.monsters.insert(
            id,
            MonsterInstance {
                template,
                location: room,
                current_hp: tpl.hitpoints,
                energy: tpl.energy,
                target: None,
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

    /// Advances game time by one tick (= one second) and runs due jobs.
    pub fn tick(&mut self) {
        for job in self.scheduler.advance() {
            match job {
                Job::Slow => {
                    self.slow_update();
                    self.scheduler.schedule_in(SLOW_INTERVAL, Job::Slow);
                }
                Job::Energy => {
                    self.energy_round();
                    self.scheduler.schedule_in(ENERGY_INTERVAL, Job::Energy);
                }
                Job::ExitStep(session) => self.exit_step(session),
            }
        }
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
            .insert(id, Session::InGame { player, derived, exiting: None, target: None, aided: false, energy: PLAYER_ENERGY_MAX });
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
            Command::Get(_) => {
                // Items land in M4; the oracle syntax line stands in.
                self.output_line(session, text::SYNTAX_GET);
            }
            Command::Status => self.show_sheet(session),
            Command::Experience => self.show_experience(session),
            Command::Health => self.show_health(session),
            Command::Train => self.train_level(session),
            // Argument commands do best-effort resolution; when they cannot
            // intuit the target, the whole line is said aloud (the parser's
            // universal fallback — applies to every future argument command).
            Command::Attack(target) => {
                if self.attack_command(session, &target) == Resolution::FallThrough {
                    self.say(session, line.trim());
                }
            }
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
            // Anything else is said aloud (oracle) - there is no error reply.
            Command::Unknown(what) => self.say(session, &what),
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
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in &sessions {
            if let Some(Session::InGame { energy, target, .. }) = self.sessions.get_mut(id) {
                *energy += PLAYER_ENERGY_MAX;
                if target.is_none() && *energy > PLAYER_ENERGY_MAX {
                    *energy = PLAYER_ENERGY_MAX;
                }
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
            match result.outcome {
                Outcome::Dodged | Outcome::Parried => {
                    self.output_line(session, &text::player_miss(&target_name));
                }
                Outcome::NoDamage => {
                    self.output_line(session, &text::player_glance(&target_name));
                }
                Outcome::Hit | Outcome::Critical => {
                    let msg = if result.outcome == Outcome::Critical {
                        text::player_crit("punch", &target_name, result.damage)
                    } else {
                        text::player_hit("punch", &target_name, result.damage)
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
        let snapshot = player.clone();
        self.output_line(session, "But, due to a miracle, you have been saved.");
        self.output_line(session, &format!("You have {lives} lives left."));
        self.broadcast_to_room(
            self.config.recall_location,
            Some(session),
            &format!("{name} appeared on the floor in the middle of the room."),
        );
        self.events.push(Event::Persist(Box::new(snapshot)));
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
        let encumbrance = 0; // M4: weight carried percent
        let mut skill = 1;
        if encumbrance < 33 && player.current_hp > 0 {
            skill += 15 - encumbrance / 10;
        }
        // accuracy = (Str-50)/3
        //          + 2*((combat-1)*isqrt(level) + 2*combat + level/2 + skill/2 - 2)
        //          + (Agl-50)/6  (+ dynamic accuracy accumulators, M5)
        let accuracy = (str_ - 50) / 3
            + 2 * ((combat - 1) * isqrt(level) + 2 * combat + level / 2 + skill / 2 - 2)
            + (agl - 50) / 6;

        // Unarmed damage defaults 1-4, plus the Strength bonuses:
        // max += (Str-50)/10; min += 2*(Str-100)/10 when positive; min <= max.
        let mut min_damage = 1;
        let mut max_damage = 4 + (str_ - 50) / 10;
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
            let encumbrance = 0; // M4: weight carried percent
            let mut p = (i32::from(player.stats.charm) - 50) / 5
                + i32::from(player.level) / 5
                + (i32::from(player.stats.agility) - 50) / 3;
            if encumbrance < 33 {
                p += 10 - encumbrance / 10;
            }
            p
        };
        crate::combat::Fighter {
            accuracy: 0,
            evasion_a: 0,
            evasion_b: 0,
            armor: 0,
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
            .insert(session, Session::InGame { player, derived, exiting: None, target: None, aided: false, energy: PLAYER_ENERGY_MAX });
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
            cp_unspent: template.cp,
            cp_lifetime: template.cp,
            lives: 9,
            experience: 0,
            location: self.config.start_location,
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
        self.events.push(Event::Persist(Box::new(player)));
        self.events.push(Event::Disconnect(session));
    }

    fn player(&self, session: SessionId) -> &Player {
        match &self.sessions[&session] {
            Session::InGame { player, .. } => player,
            _ => unreachable!("caller guarantees an in-game session"),
        }
    }

    fn move_player(&mut self, session: SessionId, direction: Direction) {
        let from = self.player(session).location;
        let Some(exit) = self.content.rooms[&from].exits[direction as usize].clone() else {
            self.output_line(session, text::NO_EXIT);
            return;
        };
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
            .filter(|d| room.exits[*d as usize].is_some())
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

        // Floor coin piles (oracle: the notice line precedes Also-here).
        if let Some(piles) = self.room_coins.get(&room.id)
            && let Some(names) = text::coin_pile_names(*piles)
        {
            out.push_str(&format!("You notice {names} here.\n"));
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
            .filter(|d| room.exits[*d as usize].is_some())
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
            Session::InGame { player, .. } => Some((*id, player)),
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
