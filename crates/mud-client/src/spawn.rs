//! What lives in a room, answered from the shipped world database rather
//! than from watching the board.
//!
//! MajorMUD does not store a room's monsters; it stores a *recipe*. The
//! room names a spawn region and a level window, and `generate_monster`
//! (`re/docs/monsters.md` §2) walks the monster-generation table for
//! entries matching both. That table is the `monster` rows themselves:
//! the `group` column is the region and `index` the level. Verified two
//! ways on `1/305 Skali's Fine Armour, Front Room`, where the region+band
//! draw and the room's forced-monster field independently name monster 27
//! `Gurbultis`.
//!
//! So "what spawns here" is a static query, and everything above it —
//! the `/room` dossier, the map's danger paint, picking a farm spot —
//! is a read of the same recipe.

use std::collections::BTreeMap;
use std::path::Path;

use mud_core::content::{Direction, RoomId};

use crate::graph::{GraphRoom, RoomGraph, Spawn, SpawnKind};

/// One monster the spawner may pick, as the template stores it.
///
/// Field names follow what the value *means* rather than the column it
/// came from; the decode from column to meaning is recorded here because
/// two of the columns are named `follow` and `something3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub number: i64,
    pub name: String,
    /// `group` — the spawn region this template belongs to.
    pub region: i64,
    /// `index` — the level the room's band is matched against.
    pub level: i64,
    pub experience: i64,
    pub hitpoints: i64,
    /// `gamelimit` — how many may exist in the world at once; 0 is
    /// unlimited. This is the mechanism behind rare monsters, and the
    /// reason a cave bear (limit 1) is worth planning a route around.
    pub gamelimit: i64,
    /// `follow` — the 0-100 aggression rating (`mon+0x108`).
    ///
    /// **Not what decides whether it attacks you.** It is the LOCK roll
    /// after a swing and the pursuit roll when chasing
    /// (`re/docs/monsters.md` §4), i.e. how doggedly it sticks to a
    /// target it already has. Newhaven's shopkeepers are rated 100.
    pub aggression: i64,
    /// The behaviour mode, `mon+0x106` — which decides initiation.
    ///
    /// Read from the sqlite column named **`alignment`**. That is a
    /// naming artifact of the rectype import and a documented trap
    /// (`re/docs/monsters.md` §4, "COLUMN-NAME TRAP"): the column called
    /// `type` is something else entirely and the one called `alignment`
    /// is the mode. A cave bear is `type 3, alignment 1`, and it
    /// initiates on sight exactly as mode 1 predicts.
    pub behaviour: i64,
    /// The ROAM class, `mon+0x12c` (sqlite `something3`). Class 5 is the
    /// criminal-hunter branch; see [`Template::initiates_against`].
    pub roam_class: i64,
    /// `attackper_1` — how often its first attack fires. 0 means no
    /// attack table at all, which is what shopkeepers, healers, trainers
    /// and props have. Reported by the dossier; initiation is decided by
    /// the mode alone, per the spec.
    pub attack_percent: i64,
}

/// Where you stand with the law, which is all a monster's initiation
/// decision knows about you.
///
/// Only 19 of the 1,100 templates consult it — 15 fame-sparing and 4
/// criminal-hunting — so the other 98% of the world is answerable with no
/// knowledge of the character at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Standing {
    /// Evil points, `player+0x542`: positive is evil, negative good
    /// (`re/docs/crime.md` §1).
    pub fame: i64,
}

impl Standing {
    /// Fame ≥ 0x28, the one boundary both fame-dependent branches test.
    /// It is the Outlaw tier and above.
    pub fn notorious(&self) -> bool {
        self.fame >= 0x28
    }

    /// Read from the legal-level word the board prints in a WHO listing.
    ///
    /// The exact number never appears in play, but the tier does, and the
    /// tier is enough: every branch tests one threshold that falls on a
    /// tier boundary. Unknown words are `None` rather than a guess.
    pub fn from_legal_level(word: &str) -> Option<Standing> {
        // Mid-range of each tier, since only the 0x28 boundary is tested
        // and every tier sits wholly on one side of it.
        let fame = match word.trim().to_ascii_lowercase().as_str() {
            "saint" => -300,
            "good" => -100,
            "lawful" | "neutral" => 0,
            "seedy" => 35,
            "outlaw" => 60,
            "criminal" => 100,
            "villain" => 160,
            "fiend" => 300,
            _ => return None,
        };
        Some(Standing { fame })
    }
}

impl Template {
    /// Will this monster start a fight with somebody of this standing?
    ///
    /// Computed from the behaviour-mode taxonomy in
    /// `re/docs/monsters.md` §4, reconstructed from the aggression driver
    /// `FUN_00423863`, rather than inferred from the board's colours:
    ///
    /// | mode | behaviour |
    /// |------|-----------|
    /// | 0, 3, 4 | never initiates; fights back only once attacked |
    /// | 6 | initiates EXCEPT against fame ≥ 0x28 — spares the famous |
    /// | 1, 2, 5, other | initiates against any valid target |
    ///
    /// ROAM class 5 overrides the mode entirely: it is the
    /// criminal-hunter branch and initiates ONLY against fame ≥ 0x28,
    /// inverting to fame < 0x28 when the mode is also 6.
    ///
    /// **Aggression is not part of this.** The per-round roll is
    /// anti-pile-on (`50 - 5 × attackers`) and only chooses WHICH
    /// eligible monster swings — "an eligible monster always attacks".
    /// `follow` is the lock and pursuit roll instead.
    ///
    /// What this deliberately does NOT model, being per-encounter rather
    /// than per-template: being hidden or sneaking (unless the monster
    /// has see-hidden `0x39`), and the moved-this-round flag. Both
    /// SUPPRESS an attack, so a positive here is an upper bound — "may
    /// attack you", never "will".
    pub fn initiates_against(&self, you: &Standing) -> bool {
        // The hunter branch is decided before the mode, and is the only
        // place a mode-6 reading flips.
        if self.roam_class == 5 {
            return if self.behaviour == 6 {
                !you.notorious()
            } else {
                you.notorious()
            };
        }
        match self.behaviour {
            0 | 3 | 4 => false,
            6 => !you.notorious(),
            _ => true,
        }
    }

    /// Whether it can fight at all. 124 templates have no attack table —
    /// the shopkeepers, healers, trainers and props.
    pub fn armed(&self) -> bool {
        self.attack_percent > 0
    }
}

/// Every monster template, indexed the way the spawner reads them.
///
/// `Default` is the empty table, for a view built over a hand-made graph.
#[derive(Default)]
pub struct SpawnTable {
    /// region -> templates, in level order.
    by_region: BTreeMap<i64, Vec<Template>>,
    by_number: BTreeMap<i64, Template>,
}

impl SpawnTable {
    pub fn load(db: &Path) -> Result<Self, String> {
        let conn = rusqlite::Connection::open_with_flags(
            db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("open {}: {e}", db.display()))?;
        let mut stmt = conn
            .prepare(
                "select number, name, \"group\", \"index\", experience, hitpoints, \
                 alignment, gamelimit, follow, something3, attackper_1 \
                 from monster where name != '' order by \"index\", number",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Template {
                    number: row.get(0)?,
                    name: row.get(1)?,
                    region: row.get(2)?,
                    level: row.get(3)?,
                    experience: row.get(4)?,
                    hitpoints: row.get(5)?,
                    // Column `alignment` IS the behaviour mode; see the
                    // field docs for the naming trap.
                    behaviour: row.get(6)?,
                    gamelimit: row.get(7)?,
                    aggression: row.get(8)?,
                    roam_class: row.get(9)?,
                    attack_percent: row.get(10)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut by_region: BTreeMap<i64, Vec<Template>> = BTreeMap::new();
        let mut by_number = BTreeMap::new();
        for row in rows {
            let t = row.map_err(|e| e.to_string())?;
            by_number.insert(t.number, t.clone());
            by_region.entry(t.region).or_default().push(t);
        }
        Ok(SpawnTable {
            by_region,
            by_number,
        })
    }

    pub fn by_number(&self, number: i64) -> Option<&Template> {
        self.by_number.get(&number)
    }

    /// What this room's recipe can produce.
    ///
    /// A forced monster answers alone: `generate_monster` takes the
    /// `param_4` path and never draws from the region
    /// (`re/docs/monsters.md` §2 step 2).
    ///
    /// Otherwise the region draw, filtered by the level band. **Region 0
    /// yields nothing** — see [`Spawn::region`] for why that sentinel is
    /// not optional.
    pub fn candidates(&self, room: &GraphRoom) -> Vec<&Template> {
        if let Some(number) = room.spawn.forced {
            return self.by_number(number).into_iter().collect();
        }
        if !room.spawn.draws_from_region() {
            return Vec::new();
        }
        let (lo, hi) = room.spawn.band;
        self.by_region
            .get(&room.spawn.region)
            .into_iter()
            .flatten()
            .filter(|t| (lo..=hi).contains(&t.level))
            .collect()
    }
}

/// How much a room wants to hurt you, at the resolution the map paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threat {
    /// Nothing spawns here.
    Nothing,
    /// Something spawns, but nothing that starts a fight.
    Passive,
    /// Something here will start it. This is the board's own bright
    /// magenta, in map form.
    Aggressive,
}

/// Everything the client knows about one room, in one place.
///
/// Rendered by exactly one function, [`Dossier::lines`], because `/room`,
/// the map's side panel and the map's danger paint all need the same
/// answer and a second renderer is a second thing to get wrong.
#[derive(Debug, Clone)]
pub struct Dossier {
    pub id: RoomId,
    pub name: String,
    pub light: i64,
    pub dark: bool,
    pub shop: i64,
    pub spawn: Spawn,
    /// The room's permanent occupant, if it has one.
    pub resident: Option<Template>,
    pub candidates: Vec<Template>,
    /// Direction, where it goes, and the phrase that walks it when the
    /// exit is a command exit rather than a step.
    pub exits: Vec<(Direction, RoomId, Option<String>)>,
}

const DIRECTIONS: [Direction; 10] = [
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
    Direction::NorthEast,
    Direction::NorthWest,
    Direction::SouthEast,
    Direction::SouthWest,
    Direction::Up,
    Direction::Down,
];

impl Dossier {
    pub fn of(graph: &RoomGraph, spawns: &SpawnTable, id: RoomId) -> Option<Dossier> {
        let room = graph.room(id)?;
        Some(Dossier {
            id,
            name: room.name.clone(),
            light: room.light,
            dark: graph.dark(id),
            shop: room.shop,
            spawn: room.spawn,
            resident: room
                .spawn
                .resident
                .and_then(|n| spawns.by_number(n))
                .cloned(),
            candidates: spawns.candidates(room).into_iter().cloned().collect(),
            exits: room
                .exits
                .iter()
                .enumerate()
                .filter_map(|(d, e)| {
                    e.as_ref().map(|e| (DIRECTIONS[d], e.dest, e.command.clone()))
                })
                .collect(),
        })
    }

    /// Everything standing in or spawning into this room.
    pub fn occupants(&self) -> impl Iterator<Item = &Template> {
        self.resident.iter().chain(self.candidates.iter())
    }

    /// How much this room wants to hurt you.
    ///
    /// Anything here that would start a fight makes it `Aggressive`, the
    /// permanent resident included — a tasloi chief or a night hag at
    /// aggression 100 is exactly what the paint is for.
    ///
    /// `Passive` is about SPAWNS, though, not occupancy: it means there
    /// is something here to farm that will not open hostilities. A
    /// shopkeeper standing in a shop is neither a danger nor a target,
    /// and counting them made every shop, healer and trainer light up on
    /// the danger map (live, 2026-08-02 — in Newhaven, where nothing
    /// spawns, the shops were the only colour on the screen).
    pub fn threat(&self, you: &Standing) -> Threat {
        if self.occupants().any(|t| t.initiates_against(you)) {
            Threat::Aggressive
        } else if self.candidates.is_empty() {
            Threat::Nothing
        } else {
            Threat::Passive
        }
    }

    /// The highest level this room can produce, for the map's warning
    /// overlay to compare against the character. `None` when nothing
    /// spawns.
    pub fn top_level(&self) -> Option<i64> {
        self.candidates.iter().map(|t| t.level).max()
    }

    /// One line each, for the caller to paint.
    ///
    /// Deliberately not [`std::fmt::Display`], and deliberately
    /// control-character free: these land in a fixed-width panel and in a
    /// raw-mode scroll region, where a stray newline scrolls the terminal
    /// (the lesson `tui::render_status` records).
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!("{}/{}  {}", self.id.map, self.id.room, self.name)];

        let mut where_ = Vec::new();
        if self.dark {
            where_.push(format!("dark ({})", self.light));
        }
        if self.shop > 0 {
            where_.push(format!("shop {}", self.shop));
        }
        if !where_.is_empty() {
            out.push(where_.join("  "));
        }

        out.push(match self.spawn.kind {
            SpawnKind::Never => "spawns: never".to_string(),
            SpawnKind::BootFill => "spawns: at boot only".to_string(),
            SpawnKind::Swarm => "spawns: swarm".to_string(),
            kind => format!(
                "spawns: timed ~{}%",
                kind.rate_percent().unwrap_or_default()
            ),
        });
        if self.spawn.forced.is_some() {
            out.push("  one fixed monster".to_string());
        } else if self.spawn.draws_from_region() {
            let (lo, hi) = self.spawn.band;
            out.push(format!("  region {} band {lo}-{hi}", self.spawn.region));
        }

        if let Some(t) = &self.resident {
            out.push(format!("lives here: {}", describe(t)));
        }
        if self.candidates.is_empty() && self.resident.is_none() {
            out.push("  nothing".to_string());
        }
        for t in &self.candidates {
            out.push(format!("  {}", describe(t)));
        }

        let mut exits: Vec<String> = Vec::new();
        for (dir, _, command) in &self.exits {
            match command {
                Some(cmd) => exits.push(format!("{} ({cmd})", short(*dir))),
                None => exits.push(short(*dir).to_string()),
            }
        }
        out.push(format!("exits: {}", exits.join(" ")));

        // Defence in depth: whatever built these strings, nothing painted
        // into a panel may carry a control character.
        for line in &mut out {
            *line = line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        }
        out
    }
}

/// One monster on one line: name, what it is worth, what it takes.
fn describe(t: &Template) -> String {
    let mut note = format!(
        "{} - {} exp, {} hp, agg {}",
        t.name, t.experience, t.hitpoints, t.aggression
    );
    if !t.armed() {
        note.push_str(", unarmed");
    }
    if t.gamelimit > 0 {
        note.push_str(&format!(", limit {}", t.gamelimit));
    }
    note
}

fn short(dir: Direction) -> &'static str {
    match dir {
        Direction::North => "n",
        Direction::South => "s",
        Direction::East => "e",
        Direction::West => "w",
        Direction::NorthEast => "ne",
        Direction::NorthWest => "nw",
        Direction::SouthEast => "se",
        Direction::SouthWest => "sw",
        Direction::Up => "u",
        Direction::Down => "d",
    }
}
