//! A library of named routes: hand-editable TOML, one loop per file.
//!
//! The model is **stops**, not steps. The navigator already knows how to
//! get from one room to another, so a loop says where to be and lets
//! [`crate::graph::RoomGraph::route`] work out the walking. That is what
//! makes a file short enough to edit by hand and robust enough to survive
//! inserting a stop in the middle. A leg that must not be routed — around
//! something nasty, or through a door the router would not pick — says so
//! with [`Stop::via`].
//!
//! A directory rather than one library file, because importing somebody
//! else's path pack means dropping files into it.
//!
//! The profile is never rewritten by any of this. It holds the account
//! password, and a library of routes has no business anywhere near it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mud_core::content::RoomId;
use serde::{Deserialize, Serialize};

use crate::farm::{FarmConfig, FarmPlan, parse_room_id};
use crate::graph::RoomGraph;

/// One place the runner should stand, and what is true about standing
/// there.
///
/// Every field but `at` is optional, so the terse form — an id and
/// nothing else — is a legal stop.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Stop {
    /// `map/room`, e.g. `1/2156`.
    pub at: String,
    /// The room's name, checked against the graph on load and never
    /// trusted. See [`Loop::check_names`] for why it earns its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether to rest here. MegaMud's `0002` / `0004` step flags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest: Option<bool>,
    /// This room needs a light source (MegaMud's `0001` / `0009`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<bool>,
    /// Whether to fight here (MegaMud's `0040` don't-attack, inverted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fight: Option<bool>,
    /// Force the leg INTO this stop, as space-separated directions
    /// (`"w w s"`), instead of letting the router choose it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

impl Stop {
    /// The terse stop: an id and nothing else.
    pub fn at(id: RoomId) -> Stop {
        Stop {
            at: format!("{}/{}", id.map, id.room),
            ..Default::default()
        }
    }

    /// The same, carrying the room's name for a human to read and the
    /// loader to check.
    pub fn named(id: RoomId, graph: &RoomGraph) -> Stop {
        Stop {
            name: graph.room(id).map(|r| r.name.clone()),
            ..Stop::at(id)
        }
    }

    pub fn room(&self) -> Option<RoomId> {
        parse_room_id(&self.at)
    }
}

/// A named route.
///
/// **Field order is load-bearing.** TOML puts every bare key before the
/// first table header, so a scalar declared after `stops` would serialise
/// INTO the last stop table. `finish` therefore sits above it, exactly as
/// `crate::profile::Profile` documents for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Loop {
    pub name: String,
    /// Whatever the author wanted to remember about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Which world the ids belong to, e.g. `mmud-1.11p`. Advisory: room
    /// ids are only meaningful against the world they were written for,
    /// and this is what says which that was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
    /// Where the loop came from — `mmc`, or `megamud:rocsloop.mp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Where to leave the character when a run stops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<String>,
    #[serde(rename = "stop", default, skip_serializing_if = "Vec::is_empty")]
    pub stops: Vec<Stop>,
}

/// What the `world` field says for the shipped 1.11p data.
pub const WORLD: &str = "mmud-1.11p";

impl Loop {
    pub fn new(name: &str) -> Loop {
        Loop {
            name: name.to_string(),
            world: Some(WORLD.to_string()),
            origin: Some("mmc".into()),
            ..Default::default()
        }
    }

    pub fn from_toml(text: &str) -> Result<Loop, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Every stop as a room id, in order.
    pub fn rooms(&self) -> Result<Vec<RoomId>, String> {
        self.stops
            .iter()
            .map(|s| {
                s.room()
                    .ok_or_else(|| format!("bad room id {:?}; want map/room, e.g. 1/1", s.at))
            })
            .collect()
    }

    /// Stops whose recorded name disagrees with the graph's, one line
    /// each.
    ///
    /// A warning rather than a refusal: the ids are what the runner
    /// walks, and they are valid ids. But a loop written against another
    /// realm's map resolves to rooms that exist here and are somewhere
    /// else entirely, and nothing but the name would ever notice.
    pub fn check_names(&self, graph: &RoomGraph) -> Vec<String> {
        let mut out = Vec::new();
        for stop in &self.stops {
            let (Some(want), Some(id)) = (stop.name.as_deref(), stop.room()) else {
                continue;
            };
            let actual = graph.room(id).map(|r| r.name.as_str()).unwrap_or("(no such room)");
            if actual != want {
                out.push(format!(
                    "{}: loop says {want:?}, the world says {actual:?}",
                    stop.at
                ));
            }
        }
        out
    }

    /// A [`FarmConfig`] the runner can walk, validated by the planner
    /// itself.
    ///
    /// Deliberately not a second validator: [`FarmPlan::build`] already
    /// knows every leg the runner will ever take, including the wrap and
    /// the walk home, and duplicating that here would be one more thing
    /// to keep in step.
    pub fn to_farm(&self, base: &FarmConfig, graph: &RoomGraph) -> Result<FarmConfig, String> {
        let rooms = self.rooms()?;
        let Some(first) = rooms.first() else {
            return Err("loop has no stops; it needs at least one".into());
        };
        let cfg = FarmConfig {
            // Only a hint since a run localizes itself and walks to the
            // circuit (`farm::locate_start`), so the first stop is the
            // honest thing to hint with.
            start: format!("{}/{}", first.map, first.room),
            circuit: self.stops.iter().map(|s| s.at.clone()).collect(),
            finish_at: self.finish.clone(),
            ..base.clone()
        };
        FarmPlan::build(&cfg, graph)?;
        Ok(cfg)
    }

    /// Write to `dir/<name>.toml`, creating the directory if needed.
    pub fn save(&self, dir: &Path) -> Result<PathBuf, String> {
        let file = loop_file(dir, &self.name)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let text = self.to_toml()?;
        std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
        Ok(file)
    }
}

/// Where the library lives: `loops` under the client's config
/// directory, beside the character profiles and never inside one.
pub fn dir() -> PathBuf {
    crate::profile::config_dir().join("loops")
}

/// A loop name is a file name.
///
/// Refused rather than sanitised: `../escape` quietly becoming `escape`
/// would write somewhere the operator did not ask for, and a loop saved
/// under a name nobody typed is worse than one not saved at all.
fn loop_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("a loop needs a name".into());
    }
    if Path::new(trimmed).components().count() != 1
        || trimmed.contains(['/', '\\'])
        || trimmed.starts_with('.')
    {
        return Err(format!(
            "{name:?} is not a usable file name; letters, digits and dashes"
        ));
    }
    Ok(dir.join(format!("{trimmed}.toml")))
}

/// Every loop name in the library, sorted. An absent directory is an
/// empty library, not an error: nobody has saved one yet.
pub fn list(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            (path.extension()? == "toml")
                .then(|| path.file_stem()?.to_str().map(str::to_string))
                .flatten()
        })
        .collect();
    names.sort();
    names
}

pub fn load(dir: &Path, name: &str) -> Result<Loop, String> {
    let file = loop_file(dir, name)?;
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("no loop {name:?} in {}: {e}", dir.display()))?;
    Loop::from_toml(&text).map_err(|e| format!("{}: {e}", file.display()))
}

/// Every room the walk passes through, stops included.
///
/// This is what the map paints gold, and it is the whole route rather
/// than the stops alone — seeing where a route actually goes is the point
/// of drawing it. The circuit closes, so the leg from the last stop back
/// to the first is included too: the runner walks it, so the map shows it.
///
/// A leg that does not route is skipped rather than fatal. `to_farm`
/// refuses to save such a loop; the map still has to draw the legs that
/// do exist while somebody is halfway through building one.
pub fn route_rooms(graph: &RoomGraph, stops: &[RoomId]) -> BTreeSet<RoomId> {
    let mut out: BTreeSet<RoomId> = stops.iter().copied().collect();
    if stops.len() < 2 {
        return out;
    }
    let legs = stops
        .windows(2)
        .map(|p| (p[0], p[1]))
        .chain(std::iter::once((stops[stops.len() - 1], stops[0])));
    for (from, to) in legs {
        let Some(steps) = graph.route(from, to) else {
            continue;
        };
        let mut at = from;
        for dir in steps {
            let Some(edge) = graph.room(at).and_then(|r| r.exits[dir as usize].as_ref()) else {
                break;
            };
            at = edge.dest;
            out.insert(at);
        }
    }
    out
}
