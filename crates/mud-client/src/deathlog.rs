//! Where the character died, kept across sessions.
//!
//! A full death drops everything the character carries into the room it
//! happened in, and the client had no record of which room that was: a
//! job that ended in a death carried no room, and hand play recorded
//! nothing at all. One line per death in `deaths.log` under the config
//! directory is the record. `/recover` with no argument reads the newest
//! one back.
//!
//! Append only, never rewritten, so the file is the history.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mud_core::content::RoomId;

use crate::lost::Fix;

/// How much the logged room was worth when the death was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixWord {
    Confirmed,
    Stale,
    Unknown,
}

impl FixWord {
    fn word(self) -> &'static str {
        match self {
            FixWord::Confirmed => "confirmed",
            FixWord::Stale => "stale",
            FixWord::Unknown => "unknown",
        }
    }

    fn parse(word: &str) -> Option<FixWord> {
        Some(match word {
            "confirmed" => FixWord::Confirmed,
            "stale" => FixWord::Stale,
            "unknown" => FixWord::Unknown,
            _ => return None,
        })
    }
}

/// One logged death.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Death {
    /// UTC, to the second, as [`stamp`] writes it.
    pub stamp: String,
    pub character: String,
    /// `None` when nothing was known about where.
    pub room: Option<RoomId>,
    /// The room's name as the graph has it. Empty when `room` is `None`.
    pub name: String,
    pub fix: FixWord,
}

impl Death {
    /// The line as written: stamp, character, id or `-`, name or `-`,
    /// fix word. Single spaces between fields. The name may contain
    /// spaces, which is why it sits between two fields that cannot.
    pub fn line(&self) -> String {
        let (id, name) = match self.room {
            Some(r) => (format!("{}/{}", r.map, r.room), self.name.as_str()),
            None => ("-".to_string(), "-"),
        };
        format!("{} {} {} {} {}", self.stamp, self.character, id, name, self.fix.word())
    }

    pub fn parse(line: &str) -> Option<Death> {
        let mut words = line.split_whitespace();
        let stamp = words.next()?.to_string();
        let character = words.next()?.to_string();
        let id = words.next()?;
        let rest: Vec<&str> = words.collect();
        let (fix, name) = rest.split_last()?;
        let fix = FixWord::parse(fix)?;
        let room = if id == "-" {
            None
        } else {
            Some(crate::farm::parse_room_id(id)?)
        };
        let name = match room {
            Some(_) => name.join(" "),
            None => String::new(),
        };
        Some(Death {
            stamp,
            character,
            room,
            name,
            fix,
        })
    }
}

/// `2026-09-07T14:42:07Z`. UTC, because the client has no timezone table
/// and an honest UTC beats a wrong local time. Hand-rolled because the
/// crate carries no date dependency and this is the only date it writes.
pub fn stamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let t = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

/// Days since 1970-01-01 to a civil date. Howard Hinnant's algorithm,
/// which is exact for every day the proleptic Gregorian calendar has.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `~/.config/mmc/deaths.log`, beside the profiles and the loop library.
pub fn path() -> PathBuf {
    crate::profile::config_dir().join("deaths.log")
}

/// A death as it will be logged: the character's last fix, current or
/// not, because a stale room beats no room. `name_of` is asked only when
/// there is a room to name, so a caller with no graph can pass a closure
/// that never runs.
pub fn death_of(
    character: &str,
    fix: Fix,
    name_of: impl FnOnce(RoomId) -> String,
    now: SystemTime,
) -> Death {
    let (room, word) = match fix {
        Fix::Confirmed(at) => (Some(at), FixWord::Confirmed),
        Fix::Stale(at) => (Some(at), FixWord::Stale),
        Fix::Unknown => (None, FixWord::Unknown),
    };
    let name = room.map(name_of).unwrap_or_default();
    Death {
        stamp: stamp(now),
        character: character.to_string(),
        room,
        name,
        fix: word,
    }
}

/// Append one line. The directory is made if it is missing.
pub fn record_in(path: &Path, death: &Death) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", death.line())
}

/// The newest logged death for `character` that carries a room. A line
/// that does not parse is skipped rather than fatal: the file is
/// hand-editable and one bad line must not hide the good ones.
pub fn last_in(path: &Path, character: &str) -> Option<Death> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .filter_map(Death::parse)
        .find(|d| d.character == character && d.room.is_some())
}

/// Sees a death exactly once.
///
/// The board says it three ways: the prompt drops to zero hitpoints,
/// `You have been killed.` is printed to the dying player, and
/// `<name> is dead.` is printed to the room. All three usually arrive
/// for one death, so the first fires and the rest are held until a
/// prompt shows the character alive again.
#[derive(Debug, Default)]
pub struct DeathWatch {
    dead: bool,
}

impl DeathWatch {
    /// True exactly once per death.
    pub fn on_event(&mut self, ev: &crate::events::Event, character: &str) -> bool {
        use crate::events::Event;
        match ev {
            Event::Prompt { hp, .. } if *hp > 0 => {
                self.dead = false;
                false
            }
            Event::Prompt { .. } => self.fire(),
            Event::Line(line)
                if line.trim() == "You have been killed."
                    || crate::farm::is_player_death(line, character) =>
            {
                self.fire()
            }
            _ => false,
        }
    }

    /// A death seen by somebody else, the map closing on the death
    /// line, so the wordings still to arrive do not fire a second time.
    pub fn mark_dead(&mut self) {
        self.dead = true;
    }

    fn fire(&mut self) -> bool {
        if self.dead {
            return false;
        }
        self.dead = true;
        true
    }
}

/// Build a death and write it, in one call, because every caller wants
/// both and none wants one without the other. The recorded death comes
/// back so the caller can say what it wrote in its own voice: a window
/// notes it on its screen, the headless watcher prints it.
pub(crate) fn log_death(
    character: &str,
    fix: Fix,
    name_of: impl FnOnce(RoomId) -> String,
    path: &Path,
) -> std::io::Result<Death> {
    let death = death_of(character, fix, name_of, SystemTime::now());
    record_in(path, &death)?;
    Ok(death)
}

/// Log deaths on a session no window is watching, against a log file the
/// caller names.
///
/// The room is the last block the board rendered, resolved by its name
/// when the graph has exactly one room by that name, and unknown
/// otherwise: coarser than the window's locator on purpose, since a
/// session watched this way has no locator and a death is not the moment
/// to build one.
///
/// A session with no character name at all is watched but never logged.
/// An empty name matches no death line, and it would write a record
/// with an empty field that reads back as a different line.
pub fn watch_headless_in(
    session: std::sync::Arc<crate::session::Session>,
    graph: std::sync::Arc<crate::graph::RoomGraph>,
    path: PathBuf,
) -> tokio::task::JoinHandle<()> {
    // Subscribed here, not inside the task. A spawned task is not polled
    // until the runtime gets to it, and a board that dies in that window
    // would have its death logged by nobody.
    let mut events = session.events();
    tokio::spawn(async move {
        let mut watch = DeathWatch::default();
        let mut fix = Fix::Unknown;
        loop {
            let cor = match events.recv().await {
                Ok(cor) => cor,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            };
            if let crate::events::Event::RoomSeen(room) = &cor.event
                && !cor.elsewhere
            {
                let named = graph.rooms_named(&room.name);
                fix = match named.as_slice() {
                    [one] => Fix::Confirmed(*one),
                    _ => fix.demote(),
                };
            }
            let Some(name) = session.character_name() else {
                continue;
            };
            if watch.on_event(&cor.event, &name) {
                let namer = |id| graph.room(id).map(|r| r.name.clone()).unwrap_or_default();
                match log_death(&name, fix, namer, &path) {
                    Ok(death) => eprintln!("death logged: {}", death.line()),
                    Err(e) => eprintln!("death not logged: {e}"),
                }
            }
        }
    })
}

