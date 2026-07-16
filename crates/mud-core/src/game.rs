//! The game core: a single-threaded, deterministic state machine.
//!
//! Sessions attach with a loaded (or freshly created) player, feed text input
//! in, and consume `Event`s out. No I/O happens here — the caller owns
//! networking and persistence.

use std::collections::BTreeMap;

use crate::ability::Ability;
use crate::command::{parse, Command};
use crate::content::{ClassId, Content, Direction, RaceId, RoomId, StatBlock};
use crate::stats::{derive, AbilityBag, Derived, StatInputs};
use crate::text;
use crate::tick::TickScheduler;

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
    pub cp_unspent: u16,
    pub cp_lifetime: u16,
    pub lives: u16,
    pub experience: u64,
    pub location: RoomId,
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
}

impl Default for CoreConfig {
    fn default() -> Self {
        CoreConfig {
            start_location: RoomId { map: 1, room: 2140 },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Output { session: SessionId, text: String },
    Persist(Box<Player>),
    Disconnect(SessionId),
}

enum Session {
    /// Character creation: race then class (`character_creation.md` §1).
    ChoosingRace { profile: AccountProfile },
    ChoosingClass { profile: AccountProfile, race: RaceId },
    InGame { player: Player, derived: Derived },
}

/// Self-rescheduling background jobs (`combat_rounds.md` §1). Medium (3 s)
/// and energy (5 s) tiers join with their systems in later milestones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    /// `background_slow`, every 30 s: regen, hunger/thirst decay.
    Slow,
}

const SLOW_INTERVAL: u64 = 30;

pub struct Core {
    content: Content,
    config: CoreConfig,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    events: Vec<Event>,
    scheduler: TickScheduler<Job>,
}

impl Core {
    pub fn new(content: Content, config: CoreConfig) -> Core {
        let mut scheduler = TickScheduler::new();
        scheduler.schedule_in(SLOW_INTERVAL, Job::Slow);
        Core {
            content,
            config,
            sessions: BTreeMap::new(),
            next_session: 1,
            events: Vec::new(),
            scheduler,
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
            }
        }
    }

    /// `slow_update_characters` (`regeneration.md`): hunger/thirst decay,
    /// HP regen, mana regen for every in-game player. (Poison and bleed/aid
    /// join in M3 with the death system.)
    fn slow_update(&mut self) {
        let sessions: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in sessions {
            let Some(Session::InGame { player, derived }) = self.sessions.get(&id) else {
                continue;
            };
            let (max_hp, max_mana) = (derived.max_hp, derived.max_mana);
            let bag = self.ability_bag(player);
            let caster = self
                .content
                .classes
                .get(&player.class)
                .map(|c| (c.caster_group, c.casting_factor));
            let Some(Session::InGame { player, .. }) = self.sessions.get_mut(&id) else {
                unreachable!("checked above");
            };

            player.hunger = player.hunger.saturating_sub(1);
            player.thirst = player.thirst.saturating_sub(1);

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
        self.sessions.insert(id, Session::InGame { player, derived });
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
        let Some(Session::InGame { player, derived: _ }) = self.sessions.get(&session) else {
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
        let Some(Session::InGame { player, derived }) = self.sessions.get(&session) else {
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
            Some(Session::InGame { .. }) => self.game_command(session, line),
        }
    }

    fn game_command(&mut self, session: SessionId, line: &str) {
        match parse(line) {
            Command::Quit => self.quit(session),
            // Blank input re-shows the room without its description (oracle).
            Command::Blank => self.show_room_brief(session),
            Command::Look => self.show_room(session),
            Command::Status => self.show_sheet(session),
            Command::Move(direction) => self.move_player(session, direction),
            // Anything else is said aloud (oracle) - there is no error reply.
            Command::Unknown(what) => self.say(session, &what),
        }
        self.show_prompt(session);
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
        let player = self.roll_stats(profile, race, choice.expect("validated above"));
        self.events.push(Event::Persist(Box::new(player.clone())));
        self.broadcast_to_others(session, &text::entered_realm(&player.name));
        let derived = self.derive_for(&player);
        self.sessions.insert(session, Session::InGame { player, derived });
        // Oracle: first entry shows the stat sheet, not the room.
        self.show_sheet(session);
        self.show_prompt(session);
    }

    /// `roll_stats` (spec §2.2): no randomisation — the racial template, CP
    /// grant, and creation defaults are copied verbatim; mode-0
    /// `calculate_secondary_stats` fills HP/mana to max.
    fn roll_stats(&self, profile: AccountProfile, race: RaceId, class: ClassId) -> Player {
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
            Some(Session::InGame { .. }) => self.quit(session),
            Some(_) => {
                self.sessions.remove(&session);
                self.events.push(Event::Disconnect(session));
            }
            None => {}
        }
    }

    fn quit(&mut self, session: SessionId) {
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

        let others: Vec<&str> = self
            .in_game_sessions()
            .filter(|(id, p)| *id != session && p.location == room.id)
            .map(|(_, p)| p.name.as_str())
            .collect();
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
