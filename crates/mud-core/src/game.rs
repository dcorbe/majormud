//! The game core: a single-threaded, deterministic state machine.
//!
//! Sessions attach with a loaded (or freshly created) player, feed text input
//! in, and consume `Event`s out. No I/O happens here — the caller owns
//! networking and persistence.

use std::collections::BTreeMap;

use crate::command::{parse, Command};
use crate::content::{ClassId, Content, Direction, RaceId, RoomId, StatBlock};
use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
}

/// The M1 subset of the 0x7ec-byte player record
/// (`re/docs/character_creation.md` §6). Grows with each milestone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub name: String,
    pub gender: Gender,
    pub race: RaceId,
    pub class: ClassId,
    pub level: u16,
    pub stats: StatBlock,
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
    /// Where new characters start (`DAT_00482cf8`; ORACLE-VERIFY the stock
    /// value — Town Gates (1,1) is the working default).
    pub start_location: RoomId,
}

impl Default for CoreConfig {
    fn default() -> Self {
        CoreConfig {
            start_location: RoomId { map: 1, room: 1 },
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
    InGame { player: Player },
}

pub struct Core {
    content: Content,
    config: CoreConfig,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    events: Vec<Event>,
}

impl Core {
    pub fn new(content: Content, config: CoreConfig) -> Core {
        Core {
            content,
            config,
            sessions: BTreeMap::new(),
            next_session: 1,
            events: Vec::new(),
        }
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    /// Attaches an authenticated session with its loaded player, announces
    /// the entry to everyone else in the game, and shows the player their room.
    pub fn attach_player(&mut self, player: Player) -> SessionId {
        let id = self.next_session_id();
        self.broadcast_to_others(id, &text::entered_realm(&player.name));
        self.sessions.insert(id, Session::InGame { player });
        self.show_room(id);
        id
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
            Command::Blank => {}
            Command::Look => self.show_room(session),
            Command::Move(direction) => self.move_player(session, direction),
            Command::Unknown(_) => {
                self.output(session, text::COMMAND_NOT_UNDERSTOOD);
            }
        }
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
            out.push_str(&format!("{:>4}) {}\n", race.id.0, race.name));
        }
        self.output(session, &out);
    }

    fn show_class_list(&mut self, session: SessionId) {
        let mut out = String::from(text::CHOOSE_CLASS);
        out.push('\n');
        for class in self.content.classes.values() {
            out.push_str(&format!("{:>4}) {}\n", class.id.0, class.name));
        }
        self.output(session, &out);
    }

    /// State 0x33: the input is `atol`'d and validated against the race data.
    fn choose_race(&mut self, session: SessionId, line: &str) {
        let choice = line.trim().parse::<u16>().ok().map(RaceId);
        let valid = choice.is_some_and(|id| self.content.races.contains_key(&id));
        if !valid {
            self.output(session, text::INVALID_RACE);
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
        let choice = line.trim().parse::<u16>().ok().map(ClassId);
        let valid = choice.is_some_and(|id| self.content.classes.contains_key(&id));
        if !valid {
            self.output(session, text::INVALID_CLASS);
            return;
        }
        let Some(Session::ChoosingClass { profile, race }) = self.sessions.remove(&session)
        else {
            unreachable!("dispatched from ChoosingClass");
        };
        let player = self.roll_stats(profile, race, choice.expect("validated above"));
        self.events.push(Event::Persist(Box::new(player.clone())));
        self.broadcast_to_others(session, &text::entered_realm(&player.name));
        self.sessions.insert(session, Session::InGame { player });
        self.show_room(session);
    }

    /// `roll_stats` (spec §2.2): no randomisation — the racial template, CP
    /// grant, and creation defaults are copied verbatim.
    fn roll_stats(&self, profile: AccountProfile, race: RaceId, class: ClassId) -> Player {
        let template = &self.content.races[&race];
        Player {
            name: profile.name,
            gender: profile.gender,
            race,
            class,
            level: 1,
            stats: template.base_stats,
            cp_unspent: template.cp,
            cp_lifetime: template.cp,
            lives: 9,
            experience: 0,
            location: self.config.start_location,
        }
    }

    /// Takes all events produced since the last drain.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn quit(&mut self, session: SessionId) {
        let Some(Session::InGame { player }) = self.sessions.remove(&session) else {
            return;
        };
        self.broadcast_to_others(session, &text::left_realm(&player.name));
        self.events.push(Event::Persist(Box::new(player)));
        self.events.push(Event::Disconnect(session));
    }

    fn player(&self, session: SessionId) -> &Player {
        match &self.sessions[&session] {
            Session::InGame { player } => player,
            _ => unreachable!("caller guarantees an in-game session"),
        }
    }

    fn move_player(&mut self, session: SessionId, direction: Direction) {
        let from = self.player(session).location;
        let Some(exit) = self.content.rooms[&from].exits[direction as usize].clone() else {
            self.output(session, text::NO_EXIT);
            return;
        };
        let name = self.player(session).name.clone();
        self.broadcast_to_room(from, Some(session), &text::left_via(&name, direction));
        match self.sessions.get_mut(&session) {
            Some(Session::InGame { player }) => player.location = exit.dest,
            _ => unreachable!("mover is in game"),
        }
        self.broadcast_to_room(
            exit.dest,
            Some(session),
            &text::arrived_from(&name, direction.opposite()),
        );
        self.show_room(session);
    }

    /// Renders the session's current room: name, description, occupants,
    /// obvious exits. Line-exact to the original where VERIFIED.
    fn show_room(&mut self, session: SessionId) {
        let player = self.player(session);
        let room = &self.content.rooms[&player.location];

        let mut out = String::new();
        out.push_str(&room.name);
        out.push('\n');
        for line in &room.description {
            out.push_str(line);
            out.push('\n');
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
            self.output(session, text);
        }
    }

    fn in_game_sessions(&self) -> impl Iterator<Item = (SessionId, &Player)> {
        self.sessions.iter().filter_map(|(id, s)| match s {
            Session::InGame { player } => Some((*id, player)),
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
            self.output(session, text);
        }
    }
}
