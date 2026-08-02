//! Reading MegaMud path files.
//!
//! The format is specified in
//! `docs/mirrors/gitlab-beckersource-OmegaMUD/src/mega/OMUD_MEGA.java`,
//! and there is a corpus of 19 real ones under
//! `docs/mirrors/megamud.net/www.megamud.net/paths/` (the other 137 files
//! there are zips, not yet unpacked).
//!
//! **A MegaMud room id is a checksum, not an identity.** It is three hex
//! digits of a name hash and five nibbles of exit signature, so it says
//! what a room looks like, not which room it is: `A0700050` matches 38 of
//! our rooms, and 8,584 distinct ids cover 26,720. Importing by looking
//! rooms up one at a time therefore cannot work.
//!
//! What does work is that a path is fundamentally **"start here, then
//! walk these directions"**. Resolve the start — usually unique, because
//! a path starts somewhere distinctive — then replay the directions on
//! our own graph. The per-step ids become a *verification* signal: where
//! they stop agreeing is where the foreign path stops describing our
//! world, and saying so is more useful than pretending it does. Many of
//! the mirrored paths are for other realms' custom maps and diverge
//! immediately; that is a fact about them, not a bug here.

use std::collections::BTreeMap;

use mud_core::content::{Direction, RoomId};

use crate::graph::{ExitEdge, RoomGraph};
use crate::loops::{Loop, Stop};

/// One `roomid:flags:command` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpStep {
    /// The room the character is standing in when the command is sent.
    pub room: String,
    pub flags: u16,
    pub command: String,
}

/// A parsed `.mp` file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MpPath {
    pub name: String,
    pub author: Option<String>,
    /// Room id of the first step, from the summary line.
    pub start: String,
    pub end: String,
    pub steps: Vec<MpStep>,
}

/// MegaMud step bit flags (`OMUD_MEGA.java`). Only the ones mmc can act
/// on are carried into a [`Loop`]; the rest are reported as dropped, so
/// nothing disappears without being mentioned.
const DARK: u16 = 0x0001;
const REST_HERE: u16 = 0x0002;
const DONT_REST: u16 = 0x0004;
const PITCH_BLACK: u16 = 0x0008;
const DONT_ATTACK: u16 = 0x0040;
/// Flags with no counterpart in mmc, and what to call them in the report.
const UNUSED_FLAGS: [(u16, &str); 4] = [
    (0x0010, "stash point"),
    (0x0080, "re-learn room"),
    (0x0200, "disarm trap"),
    (0x0400, "pick lock"),
];

/// Parse a `.mp` file.
///
/// Tolerant by necessity: the mirrored corpus varies. `rocsloop.mp` has
/// both a `[Name][Author]` line and a `[PREFIX:Group:Node]` line and uses
/// CRLF; `slm2loop.mp` has only the name; `delfcity.mp` is LF-only and
/// has an unbalanced bracket in its title. Anything unrecognised is
/// skipped rather than refused — the steps are what matter.
pub fn parse_mp(text: &str) -> Result<MpPath, String> {
    let mut path = MpPath::default();
    for line in text.split('\n').map(|l| l.trim_end_matches('\r').trim()) {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if !path.name.is_empty() {
                continue;
            }
            match rest.split_once("][") {
                // `[Name][Author]`.
                Some((name, author)) => {
                    path.name = name.trim().to_string();
                    let author = author.trim_end_matches(']').trim();
                    path.author = (!author.is_empty()).then(|| author.to_string());
                }
                // One group. A goto-tree line is `[PREFIX:Group:Node]`
                // and is no use here; anything without colons is a bare
                // title, which `slm2loop.mp` has and which is the only
                // name that file carries.
                None if !rest.contains(':') => {
                    path.name = rest.trim_end_matches(']').trim().to_string();
                }
                None => {}
            }
            continue;
        }
        let fields: Vec<&str> = line.split(':').collect();
        match fields.as_slice() {
            // Summary: start, end, step count, -1, gold, fail, finish.
            [start, end, ..] if is_room_id(start) && is_room_id(end) => {
                path.start = (*start).to_string();
                path.end = (*end).to_string();
            }
            // A step: room, four hex digits of flags, the command.
            [room, flags, command, ..] if is_room_id(room) && flags.len() == 4 => {
                path.steps.push(MpStep {
                    room: (*room).to_string(),
                    flags: u16::from_str_radix(flags, 16).unwrap_or(0),
                    command: command.trim().to_lowercase(),
                });
            }
            _ => {}
        }
    }
    if path.steps.is_empty() {
        return Err("no steps in this file; is it a MegaMud .mp path?".into());
    }
    if path.start.is_empty() {
        path.start = path.steps[0].room.clone();
    }
    Ok(path)
}

fn is_room_id(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase())
}

/// A room's MegaMud id: three hex digits of name hash, then five nibbles
/// of exit signature (`u_d`, `se_sw`, `ne_nw`, `e_w`, `n_s`).
///
/// Within each pair the first direction is 1 and the second is 4, doubled
/// when the exit has a door. Door-ness is [`crate::nav::is_door`], the
/// same test the navigator uses to decide whether to try `open`.
pub fn room_id(name: &str, exits: &[Option<ExitEdge>; 10]) -> String {
    // Index order in the graph is N S E W NE NW SE SW U D; the value and
    // the nibble each contributes are fixed by the format.
    const SLOTS: [(usize, u32); 10] = [
        (4, 1), // N
        (4, 4), // S
        (3, 1), // E
        (3, 4), // W
        (2, 1), // NE
        (2, 4), // NW
        (1, 1), // SE
        (1, 4), // SW
        (0, 1), // U
        (0, 4), // D
    ];
    let mut nibbles = [0u32; 5];
    for (i, edge) in exits.iter().enumerate() {
        let Some(edge) = edge else { continue };
        let (slot, value) = SLOTS[i];
        nibbles[slot] += value * if crate::nav::is_door(edge.exit_type) { 2 } else { 1 };
    }
    let mut out = name_hash(name);
    for n in nibbles {
        out.push_str(&format!("{n:X}"));
    }
    out
}

/// The name half: sum of each character times its 1-based position, and
/// the last three hex digits of that.
pub fn name_hash(name: &str) -> String {
    let sum: u32 = name
        .chars()
        .enumerate()
        .map(|(i, c)| c as u32 * (i as u32 + 1))
        .sum();
    let hex = format!("{sum:X}");
    let tail: String = hex.chars().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{tail:0>3}")
}

/// Every room in the world, keyed by its MegaMud id.
pub struct Index(BTreeMap<String, Vec<RoomId>>);

impl Index {
    pub fn build(graph: &RoomGraph) -> Index {
        let mut map: BTreeMap<String, Vec<RoomId>> = BTreeMap::new();
        for (id, room) in graph.iter() {
            map.entry(room_id(&room.name, &room.exits)).or_default().push(id);
        }
        Index(map)
    }

    pub fn get(&self, id: &str) -> &[RoomId] {
        self.0.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What became of an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub name: String,
    /// Steps in the file.
    pub steps: usize,
    /// Steps that walked on our graph.
    pub walked: usize,
    /// Of those, how many landed in a room whose id matched the file's.
    pub hash_matches: usize,
    /// Where the walk stopped short: the step, and the room it was
    /// standing in when the next direction did not exist.
    pub stopped_at: Option<(usize, RoomId)>,
    /// Names of MegaMud flags that had no counterpart here.
    pub dropped_flags: Vec<&'static str>,
}

impl Report {
    /// One line each, for the caller to print.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "{}: walked {} of {} steps, {} of them verified by room id",
            self.name, self.walked, self.steps, self.hash_matches
        )];
        if let Some((step, at)) = self.stopped_at {
            out.push(format!(
                "  stopped at step {step}, standing in {}/{}: this path stops describing our world there",
                at.map, at.room
            ));
        }
        if self.hash_matches < self.walked {
            out.push(format!(
                "  {} steps walked into a room the file describes differently; check the route before running it",
                self.walked - self.hash_matches
            ));
        }
        if !self.dropped_flags.is_empty() {
            out.push(format!(
                "  dropped flags with no equivalent here: {}",
                self.dropped_flags.join(", ")
            ));
        }
        out
    }
}

/// Why a path could not be imported at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// The start room does not exist in this world. Almost always means
    /// the path was written for another realm's custom map.
    UnknownStart(String),
    /// Several rooms look identical to the start; the caller must pick.
    AmbiguousStart { id: String, rooms: Vec<RoomId> },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::UnknownStart(id) => write!(
                f,
                "start room {id} is not in this world; the path was probably written for another realm"
            ),
            ImportError::AmbiguousStart { id, rooms } => {
                write!(f, "start room {id} matches {} rooms:", rooms.len())?;
                for r in rooms.iter().take(5) {
                    write!(f, " {}/{}", r.map, r.room)?;
                }
                Ok(())
            }
        }
    }
}

const DIRECTIONS: [(&str, Direction); 10] = [
    ("n", Direction::North),
    ("s", Direction::South),
    ("e", Direction::East),
    ("w", Direction::West),
    ("ne", Direction::NorthEast),
    ("nw", Direction::NorthWest),
    ("se", Direction::SouthEast),
    ("sw", Direction::SouthWest),
    ("u", Direction::Up),
    ("d", Direction::Down),
];

fn direction_of(command: &str) -> Option<Direction> {
    DIRECTIONS
        .iter()
        .find(|(word, _)| *word == command)
        .map(|(_, d)| *d)
}

/// Turn a parsed path into a loop, by walking it.
///
/// `start` overrides the resolved start room, for the ambiguous case
/// where only the operator can say which of the candidates was meant.
pub fn import(
    graph: &RoomGraph,
    index: &Index,
    mp: &MpPath,
    start: Option<RoomId>,
) -> Result<(Loop, Report), ImportError> {
    let from = match start {
        Some(id) => id,
        None => match index.get(&mp.start) {
            [] => return Err(ImportError::UnknownStart(mp.start.clone())),
            [one] => *one,
            many => {
                return Err(ImportError::AmbiguousStart {
                    id: mp.start.clone(),
                    rooms: many.to_vec(),
                });
            }
        },
    };

    let mut dropped: Vec<&'static str> = Vec::new();
    let note_flags = |flags: u16, dropped: &mut Vec<&'static str>| {
        for (bit, name) in UNUSED_FLAGS {
            if flags & bit != 0 && !dropped.contains(&name) {
                dropped.push(name);
            }
        }
    };

    let stop_for = |id: RoomId, flags: u16, via: Option<Direction>| Stop {
        rest: match (flags & REST_HERE != 0, flags & DONT_REST != 0) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        },
        light: (flags & (DARK | PITCH_BLACK) != 0).then_some(true),
        fight: (flags & DONT_ATTACK != 0).then_some(false),
        via: via.map(|d| crate::nav::dir_word(d).to_string()),
        ..Stop::named(id, graph)
    };

    let first_flags = mp.steps.first().map(|s| s.flags).unwrap_or(0);
    note_flags(first_flags, &mut dropped);
    let mut stops = vec![stop_for(from, first_flags, None)];

    let mut at = from;
    let mut walked = 0usize;
    let mut hash_matches = 0usize;
    let mut stopped_at = None;
    for (i, step) in mp.steps.iter().enumerate() {
        // A command that is not a direction is not something this can
        // replay: the board may want a phrase, a door opened, a boat
        // borrowed. Stop rather than guess.
        let Some(dir) = direction_of(&step.command) else {
            stopped_at = Some((i, at));
            break;
        };
        let Some(edge) = graph
            .room(at)
            .and_then(|r| r.exits[dir as usize].as_ref())
            .filter(|e| graph.room(e.dest).is_some())
        else {
            stopped_at = Some((i, at));
            break;
        };
        at = edge.dest;
        walked += 1;

        // The file says what the NEXT room should look like; a loop wraps
        // to its first step.
        let expected = &mp.steps[(i + 1) % mp.steps.len()].room;
        if let Some(room) = graph.room(at)
            && &room_id(&room.name, &room.exits) == expected
        {
            hash_matches += 1;
        }

        // The last step closes the circuit, so its destination is the
        // first stop again and must not be added twice.
        if i + 1 < mp.steps.len() {
            let flags = mp.steps[i + 1].flags;
            note_flags(flags, &mut dropped);
            stops.push(stop_for(at, flags, Some(dir)));
        }
    }

    let name = if mp.name.is_empty() {
        "imported".to_string()
    } else {
        mp.name.clone()
    };
    let loop_ = Loop {
        note: mp.author.as_ref().map(|a| format!("by {a}")),
        origin: Some(format!("megamud:{name}")),
        stops,
        ..Loop::new(&slug(&name))
    };
    Ok((
        loop_,
        Report {
            name,
            steps: mp.steps.len(),
            walked,
            hash_matches,
            stopped_at,
            dropped_flags: dropped,
        },
    ))
}

/// A loop-library file name from a MegaMud path title.
///
/// Titles are free text — `Dark Elf City)`, `Slum loop with HO's in it` —
/// and a loop name has to be a file name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "imported".into()
    } else {
        trimmed
    }
}
