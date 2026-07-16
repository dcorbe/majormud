//! The game core: a single-threaded, deterministic state machine.
//!
//! Sessions attach with a loaded (or freshly created) player, feed text input
//! in, and consume `Event`s out. No I/O happens here — the caller owns
//! networking and persistence.

use std::collections::BTreeMap;

use crate::command::{parse, Command};
use crate::content::{ClassId, Content, Direction, RaceId, RoomId};
use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
}

/// The six primary stats, in the game's storage order
/// (`re/docs/records.md`: Int, Wis, Str, Hea, Agl, Chm).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub intellect: u16,
    pub wisdom: u16,
    pub strength: u16,
    pub health: u16,
    pub agility: u16,
    pub charm: u16,
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
    pub stats: Stats,
    pub cp_unspent: u16,
    pub cp_lifetime: u16,
    pub lives: u16,
    pub experience: u64,
    pub location: RoomId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Output { session: SessionId, text: String },
    Persist(Box<Player>),
    Disconnect(SessionId),
}

struct Session {
    player: Player,
}

pub struct Core {
    content: Content,
    sessions: BTreeMap<SessionId, Session>,
    next_session: u64,
    events: Vec<Event>,
}

impl Core {
    pub fn new(content: Content) -> Core {
        Core {
            content,
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
        let id = SessionId(self.next_session);
        self.next_session += 1;
        self.broadcast_to_others(id, &text::entered_realm(&player.name));
        self.sessions.insert(id, Session { player });
        self.show_room(id);
        id
    }

    /// Feeds one line of player input. Input from unknown (never attached or
    /// already disconnected) sessions is dropped.
    pub fn input(&mut self, session: SessionId, line: &str) {
        if !self.sessions.contains_key(&session) {
            return;
        }
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

    /// Takes all events produced since the last drain.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn quit(&mut self, session: SessionId) {
        let Some(state) = self.sessions.remove(&session) else {
            return;
        };
        self.broadcast_to_others(session, &text::left_realm(&state.player.name));
        self.events.push(Event::Persist(Box::new(state.player)));
        self.events.push(Event::Disconnect(session));
    }

    fn move_player(&mut self, session: SessionId, direction: Direction) {
        let from = self.sessions[&session].player.location;
        let Some(exit) = self.content.rooms[&from].exits[direction as usize].clone() else {
            self.output(session, text::NO_EXIT);
            return;
        };
        let name = self.sessions[&session].player.name.clone();
        self.broadcast_to_room(from, Some(session), &text::left_via(&name, direction));
        self.sessions.get_mut(&session).unwrap().player.location = exit.dest;
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
        let player = &self.sessions[&session].player;
        let room = &self.content.rooms[&player.location];

        let mut out = String::new();
        out.push_str(&room.name);
        out.push('\n');
        for line in &room.description {
            out.push_str(line);
            out.push('\n');
        }

        let others: Vec<&str> = self
            .sessions
            .iter()
            .filter(|(id, s)| **id != session && s.player.location == room.id)
            .map(|(_, s)| s.player.name.as_str())
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
            .sessions
            .iter()
            .filter(|(id, s)| Some(**id) != exclude && s.player.location == room)
            .map(|(id, _)| *id)
            .collect();
        for session in recipients {
            self.output(session, text);
        }
    }

    fn output(&mut self, session: SessionId, text: &str) {
        self.events.push(Event::Output {
            session,
            text: text.to_string(),
        });
    }

    fn broadcast_to_others(&mut self, exclude: SessionId, text: &str) {
        let recipients: Vec<SessionId> = self
            .sessions
            .keys()
            .copied()
            .filter(|s| *s != exclude)
            .collect();
        for session in recipients {
            self.output(session, text);
        }
    }
}
