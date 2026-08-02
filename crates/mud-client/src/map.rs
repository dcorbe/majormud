//! Drawing the world as a grid: layout, zoom, and the palette.
//!
//! Geometry is derived from the exit graph rather than authored. Walk the
//! compass exits from an anchor room, stepping one grid cell per
//! direction, and the game's own geometry falls out — `re/slum_map.py`
//! proved it by closing 160 slum rooms with zero coordinate conflicts.
//!
//! **A plane is the unit of drawing.** Up, down and map-change portals do
//! not move the cursor in the plane; they leave it. Splitting the world
//! that way puts the median plane at 8 rooms and the largest at 2,420, in
//! a 65 x 118 cell extent; no plane anywhere is wider than 160 cells or
//! taller than 157. That is small enough to lay out whole and scroll a
//! viewport over, which is why [`layout`] takes no radius: a radius would
//! put a wall in the middle of the thing the operator is trying to scroll
//! around.
//!
//! **A cell holds one room or none.** When the walk wants to put a second
//! room in an occupied cell the room is left unplaced and the clash is
//! recorded, because a map that silently overlapped would be a map that
//! lies. This is not a rare edge: the largest plane clashes on 145 of its
//! 2,420 rooms, about 6%. A grid drawing of a world that was never built
//! on a grid cannot do better, so the honest move is to say so.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mud_core::content::{Direction, RoomId};

use crate::graph::RoomGraph;
use crate::spawn::{Dossier, SpawnTable, Standing, Threat};

/// A position on the plane's grid. North is -y, east is +x.
pub type Cell = (i32, i32);

/// The eight directions that move within a plane, and their steps.
const COMPASS: [(Direction, Cell); 8] = [
    (Direction::North, (0, -1)),
    (Direction::South, (0, 1)),
    (Direction::East, (1, 0)),
    (Direction::West, (-1, 0)),
    (Direction::NorthEast, (1, -1)),
    (Direction::NorthWest, (-1, -1)),
    (Direction::SouthEast, (1, 1)),
    (Direction::SouthWest, (-1, 1)),
];

const ALL: [Direction; 10] = [
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

/// The grid step a compass direction makes, or `None` for up and down.
pub fn step_of(dir: Direction) -> Option<Cell> {
    COMPASS.iter().find(|(d, _)| *d == dir).map(|(_, s)| *s)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Extent {
    pub min: Cell,
    pub max: Cell,
}

impl Extent {
    pub fn width(&self) -> i32 {
        self.max.0 - self.min.0 + 1
    }

    pub fn height(&self) -> i32 {
        self.max.1 - self.min.1 + 1
    }
}

/// A room the walk could not place, because the cell it wanted was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub from: RoomId,
    pub dir: Direction,
    pub dest: RoomId,
    /// The occupied cell the walk refused to draw over.
    pub cell: Cell,
}

/// An exit that leaves the plane: up, down, or a map-change portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub from: RoomId,
    pub dir: Direction,
    pub dest: RoomId,
}

/// One drawable level of the world.
pub struct Plane {
    anchor: RoomId,
    cells: BTreeMap<Cell, RoomId>,
    at: BTreeMap<RoomId, Cell>,
    extent: Extent,
    conflicts: Vec<Conflict>,
    links: Vec<Link>,
}

impl Plane {
    pub fn anchor(&self) -> RoomId {
        self.anchor
    }

    pub fn len(&self) -> usize {
        self.at.len()
    }

    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }

    pub fn extent(&self) -> Extent {
        self.extent
    }

    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    /// Exits off this plane, in the order the walk met them. The
    /// interactive view offers these as plane hops.
    pub fn links(&self) -> &[Link] {
        &self.links
    }

    /// Links that leave this particular room, for the view's `<`/`>`.
    pub fn links_from(&self, room: RoomId) -> impl Iterator<Item = &Link> {
        self.links.iter().filter(move |l| l.from == room)
    }

    pub fn cell_of(&self, room: RoomId) -> Option<Cell> {
        self.at.get(&room).copied()
    }

    pub fn room_at(&self, cell: Cell) -> Option<RoomId> {
        self.cells.get(&cell).copied()
    }

    pub fn rooms(&self) -> impl Iterator<Item = RoomId> + '_ {
        self.at.keys().copied()
    }
}

/// Lay out the whole plane containing `anchor`, which lands at `(0, 0)`.
///
/// Breadth-first so that the shortest walk to a room decides its cell:
/// with conflicts possible, the nearest placement is the one least likely
/// to have accumulated distortion.
pub fn layout(graph: &RoomGraph, anchor: RoomId) -> Plane {
    let mut plane = Plane {
        anchor,
        cells: BTreeMap::new(),
        at: BTreeMap::new(),
        extent: Extent::default(),
        conflicts: Vec::new(),
        links: Vec::new(),
    };
    if graph.room(anchor).is_none() {
        return plane;
    }
    plane.cells.insert((0, 0), anchor);
    plane.at.insert(anchor, (0, 0));

    let mut queue = VecDeque::from([anchor]);
    while let Some(from) = queue.pop_front() {
        let cell = plane.at[&from];
        let Some(room) = graph.room(from) else {
            continue;
        };
        for (i, dir) in ALL.into_iter().enumerate() {
            let Some(edge) = room.exits[i].as_ref() else {
                continue;
            };
            if graph.room(edge.dest).is_none() {
                continue; // an edge into nothing; never drawn, as `route` never walks it
            }
            // Up, down and anything crossing to another map leave the
            // plane rather than taking a cell in it.
            let Some(step) = step_of(dir).filter(|_| edge.dest.map == from.map) else {
                plane.links.push(Link {
                    from,
                    dir,
                    dest: edge.dest,
                });
                continue;
            };
            let want = (cell.0 + step.0, cell.1 + step.1);
            match plane.at.get(&edge.dest) {
                // Already placed. Agreeing is the common case; disagreeing
                // is the world not being flat, and is worth reporting once.
                Some(&there) => {
                    if there != want {
                        plane.conflicts.push(Conflict {
                            from,
                            dir,
                            dest: edge.dest,
                            cell: want,
                        });
                    }
                }
                None if plane.cells.contains_key(&want) => {
                    plane.conflicts.push(Conflict {
                        from,
                        dir,
                        dest: edge.dest,
                        cell: want,
                    });
                }
                None => {
                    plane.cells.insert(want, edge.dest);
                    plane.at.insert(edge.dest, want);
                    queue.push_back(edge.dest);
                }
            }
        }
    }

    let (xs, ys): (Vec<i32>, Vec<i32>) = plane.cells.keys().copied().unzip();
    plane.extent = Extent {
        min: (
            xs.iter().copied().min().unwrap_or(0),
            ys.iter().copied().min().unwrap_or(0),
        ),
        max: (
            xs.iter().copied().max().unwrap_or(0),
            ys.iter().copied().max().unwrap_or(0),
        ),
    };
    plane
}

// --- paint -----------------------------------------------------------

/// What the foreground says about a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    /// What the room is: dark, a shop, or ordinary.
    Terrain,
    /// Whether what spawns here will start the fight.
    Danger,
    /// How much the best thing here is worth, which is what a farm spot
    /// is actually chosen on.
    ///
    /// Deliberately experience and not the spawn band: `minindex` /
    /// `maxindex` and the monster `index` they select on are a
    /// within-region ordinal, not a difficulty scale. The values run to
    /// 666, 999 and 9999, and experience at every index covers the whole
    /// 0-65,000 range, so a ramp built on them would be decoration.
    Worth,
}

/// What the palette needs to know about the character to warn them.
///
/// All-zero means "nothing known", and nothing known must never paint the
/// world red: a warning that is always on is a warning nobody reads.
#[derive(Debug, Clone, Default)]
pub struct PaintCtx {
    /// The character's maximum hitpoints. A spawn with more of them than
    /// this is the clearest "you lose this fight" the shipped data
    /// supports. 0 = unknown, so no warning.
    pub max_hp: i64,
    /// Warn at or above this experience value, for the case hitpoints
    /// miss: something soft that hits very hard. 0 = never.
    pub warn_above_exp: i64,
    /// Where the character stands with the law. Only 19 of the 1,100
    /// monster templates consult it — the fame-sparing and the
    /// criminal-hunting — so the default of Neutral is right for almost
    /// the whole world and wrong only for those.
    pub standing: Standing,
    /// Set when the character is KNOWN to carry nothing that can light a
    /// room — a [`crate::sheet::LightState`] built from an empty source
    /// list. Darkness is then a warning rather than a shade of grey.
    ///
    /// Phrased as the negative on purpose, so that the default is
    /// "nothing known" and agrees with the two fields above: an unknown
    /// character must never paint the world red.
    pub no_light_source: bool,
}

/// One room's appearance, before the operator's own marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// SGR parameters, without the escape or the `m`.
    pub sgr: &'static str,
    pub glyph: char,
}

/// The board's own bright magenta for an aggressive monster
/// (`crate::bot::AGGRESSIVE`, `crate::events`). The map says the same
/// thing in the same colour so the two read as one game.
const AGGRESSIVE: &str = "1;35";
/// Reserved for the warning overlay and nothing else, in every mode.
const WARNING: &str = "1;31";
/// Something here to fight that will not start it — a target, not a
/// threat.
///
/// Deliberately NOT a dimmer magenta. A shade of the danger hue reads as
/// "slightly less dangerous", when passive means very nearly the
/// opposite: farmable, and it leaves you alone until you swing. Cyan
/// shares no hue with the warning red or the aggressive magenta, so the
/// three cannot be confused at a glance.
pub const PASSIVE_SGR: &str = "0;36";
const DARK: &str = "1;30";
const SHOP: &str = "1;36";
const PLAIN: &str = "0;37";

/// An ordinary room. One constant, because it is both what `styles`
/// hands out and what the overview zoom tests against to swap in a solid
/// block — and a character duplicated across those two is a character
/// that gets changed in one of them.
pub const ROOM: char = '□';
/// A room drawn at overview zoom, where a single cell cannot carry a
/// connector and colour has to do the work.
const ROOM_SOLID: char = '\u{2588}';

/// Worth ramp, low to high. No red: see [`WARNING`].
const WORTH: [(i64, &str); 4] = [
    (0, "0;32"),
    (100, "1;36"),
    (1_000, "1;33"),
    (10_000, "1;35"),
];

/// Style every room on the plane.
///
/// Computed for the whole plane at once rather than per drawn cell: the
/// answer changes only when the mode or the character does, while the
/// viewport changes on every arrow key.
pub fn styles(
    plane: &Plane,
    graph: &RoomGraph,
    spawns: &SpawnTable,
    paint: Paint,
    ctx: &PaintCtx,
) -> BTreeMap<RoomId, Style> {
    let mut out = BTreeMap::new();
    for id in plane.rooms() {
        let Some(d) = Dossier::of(graph, spawns, id) else {
            continue;
        };
        let glyph = if plane.links_from(id).next().is_some() {
            // A stairwell: somewhere this map does not show.
            '+'
        } else if d.shop > 0 {
            '$'
        } else {
            ROOM
        };
        let sgr = if warns(&d, ctx) {
            WARNING
        } else {
            match paint {
                Paint::Terrain => {
                    if d.dark {
                        DARK
                    } else if d.shop > 0 {
                        SHOP
                    } else {
                        PLAIN
                    }
                }
                Paint::Danger => match d.threat(&ctx.standing) {
                    Threat::Aggressive => AGGRESSIVE,
                    Threat::Passive => PASSIVE_SGR,
                    Threat::Nothing => PLAIN,
                },
                Paint::Worth => {
                    let best = d.candidates.iter().map(|t| t.experience).max();
                    match best {
                        None => PLAIN,
                        Some(exp) => WORTH
                            .iter()
                            .rev()
                            .find(|(floor, _)| exp >= *floor)
                            .map(|(_, sgr)| *sgr)
                            .unwrap_or(PLAIN),
                    }
                }
            }
        };
        out.insert(id, Style { sgr, glyph });
    }
    out
}

/// Will the character have trouble here?
///
/// Only conditions the client can actually determine. There is no combat
/// model behind this and there must not appear to be one.
fn warns(d: &Dossier, ctx: &PaintCtx) -> bool {
    if d.dark && ctx.no_light_source {
        return true;
    }
    let toughest = d.candidates.iter().map(|t| t.hitpoints).max().unwrap_or(0);
    if ctx.max_hp > 0 && toughest > ctx.max_hp {
        return true;
    }
    let richest = d.candidates.iter().map(|t| t.experience).max().unwrap_or(0);
    ctx.warn_above_exp > 0 && richest >= ctx.warn_above_exp
}

// --- render ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    /// 4 x 2 characters per cell: glyphs, connectors and door marks.
    Detail,
    /// 2 x 2: glyphs and every connector, at half Detail's width. The
    /// default.
    ///
    /// Two rows per cell is not cosmetic. A one-row cell leaves no row
    /// BETWEEN rows for a vertical to occupy, so north/south and all four
    /// diagonals become undrawable and every room looks joined
    /// east-west — which is what this zoom did until somebody opened it
    /// on a real map and said so (2026-08-02).
    Normal,
    /// 1 x 1: one glyph per room. Fits the widest plane (166 cells) on
    /// any wide terminal.
    Overview,
}

impl Zoom {
    pub fn cell(self) -> (usize, usize) {
        match self {
            Zoom::Detail => (4, 2),
            Zoom::Normal => (2, 2),
            Zoom::Overview => (1, 1),
        }
    }

    pub fn zoom_in(self) -> Zoom {
        match self {
            Zoom::Overview => Zoom::Normal,
            _ => Zoom::Detail,
        }
    }

    pub fn zoom_out(self) -> Zoom {
        match self {
            Zoom::Detail => Zoom::Normal,
            _ => Zoom::Overview,
        }
    }
}

/// What the operator has marked, painted as BACKGROUND so it composes
/// with the foreground rather than replacing it. A dangerous room on the
/// route has to show both, which one colour per cell cannot do.
#[derive(Debug, Clone, Default)]
pub struct Marks {
    pub here: Option<RoomId>,
    pub cursor: Option<Cell>,
    pub stops: BTreeSet<RoomId>,
    pub route: BTreeSet<RoomId>,
}

/// Gold behind a room the route passes through.
const ON_ROUTE: &str = "43";
/// A loop stop: white behind, dark in front.
const STOP: &str = "47;30";
/// Where the character stands: a bright green FOREGROUND on whatever the
/// shell's background already is.
///
/// Not a filled background like the other marks. `@` is a unique glyph
/// and needs no block of colour to be found, and the one cell you look at
/// most should not be the ugliest thing on the screen.
const HERE: &str = "1;32";

/// Paint the viewport.
///
/// `view` is the top-left CELL, so scrolling is cell arithmetic and never
/// re-lays anything out. Cost tracks the terminal rather than the plane:
/// only the cells that can land inside `size` are considered.
pub fn render(
    plane: &Plane,
    styles: &BTreeMap<RoomId, Style>,
    view: Cell,
    size: (usize, usize),
    zoom: Zoom,
    marks: &Marks,
) -> Vec<String> {
    let (cols, rows) = size;
    let (cw, ch) = zoom.cell();
    if cols == 0 || rows == 0 {
        return Vec::new();
    }
    let mut buf = vec![vec![(' ', Ink::default()); cols]; rows];

    let across = cols.div_ceil(cw) as i32 + 1;
    let down = rows.div_ceil(ch) as i32 + 1;

    for gy in (view.1 - 1)..(view.1 + down) {
        for gx in (view.0 - 1)..(view.0 + across) {
            let Some(id) = plane.room_at((gx, gy)) else {
                continue;
            };
            let col = (gx - view.0) as i64 * cw as i64;
            let row = (gy - view.1) as i64 * ch as i64;
            let style = styles.get(&id).copied().unwrap_or(Style {
                sgr: PLAIN,
                glyph: ROOM,
            });

            if zoom != Zoom::Overview {
                connectors(plane, &mut buf, (gx, gy), (col, row), zoom, style.sgr);
            }

            let glyph = match marks.here {
                Some(here) if here == id => '@',
                _ => match zoom {
                    // A single cell cannot carry a connector, so the room
                    // itself is drawn solid and colour does the work.
                    Zoom::Overview if style.glyph == ROOM => ROOM_SOLID,
                    _ => style.glyph,
                },
            };
            put(&mut buf, col, row, glyph, ink(style.sgr, id, marks));
        }
    }

    // The cursor last, and unconditionally. It used to be painted only
    // as part of drawing a room, and empty cells are skipped — so
    // cursoring into the gap between two streets made it vanish with no
    // way to tell where it had gone. Standing on nothing is an ordinary
    // thing for a cursor to do, and it still has to be somewhere.
    if let Some(cell) = marks.cursor {
        let col = (cell.0 - view.0) as i64 * cw as i64;
        let row = (cell.1 - view.1) as i64 * ch as i64;
        let (glyph, mut ink) = read(&buf, col, row).unwrap_or((' ', Ink::default()));
        ink.cursor = true;
        put(&mut buf, col, row, glyph, ink);
    }

    buf.into_iter().map(emit).collect()
}

/// One cell's colour: foreground from the paint mode, background from
/// the operator's marks. Kept apart rather than pre-joined so that the
/// two compose — a dangerous room ON the route has to show both, which
/// one colour per cell cannot do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Ink {
    fg: &'static str,
    bg: &'static str,
    cursor: bool,
}

fn ink(fg: &'static str, id: RoomId, marks: &Marks) -> Ink {
    Ink {
        // Where you stand outranks the paint mode and the warning: you
        // already know you are there, and red is for rooms you might walk
        // into. It takes the FOREGROUND only — an early return that also
        // cleared the background made the room you are standing in the
        // one room that could not show it had been marked as a stop,
        // which is the first room anybody marks.
        fg: if marks.here == Some(id) { HERE } else { fg },
        bg: if marks.stops.contains(&id) {
            STOP
        } else if marks.route.contains(&id) {
            ON_ROUTE
        } else {
            ""
        },
        // Not the cursor: `render` paints that last, so that it lands on
        // empty cells too.
        cursor: false,
    }
}

/// Write one character, dropping anything outside the viewport. Clipping
/// here rather than at each call site is what makes scrolling past the
/// edge an ordinary thing to do with an arrow key.
fn put(buf: &mut [Vec<(char, Ink)>], x: i64, y: i64, c: char, ink: Ink) {
    let (rows, cols) = (buf.len() as i64, buf.first().map_or(0, Vec::len) as i64);
    if x < 0 || y < 0 || x >= cols || y >= rows {
        return;
    }
    buf[y as usize][x as usize] = (c, ink);
}

fn connectors(
    plane: &Plane,
    buf: &mut [Vec<(char, Ink)>],
    cell: Cell,
    at: (i64, i64),
    zoom: Zoom,
    fg: &'static str,
) {
    let (col, row) = at;
    let (cw, ch) = zoom.cell();
    let (cw, ch) = (cw as i64, ch as i64);
    // A connector belongs to the room it leaves, so it takes that room's
    // foreground and none of its marks: gold behind a stop must not bleed
    // down the street.
    let sgr = Ink {
        fg,
        ..Default::default()
    };
    // Geometry from the cell size rather than per-zoom arms: the rules
    // are the same shape at every scale, and writing them twice is how
    // one of them ends up missing a direction.
    let half = cw / 2;
    for (_dir, step) in COMPASS {
        if plane.room_at((cell.0 + step.0, cell.1 + step.1)).is_none() {
            continue;
        }
        let (dx, dy) = (step.0 as i64, step.1 as i64);
        match (dx, dy) {
            // Horizontal: fill the gap between the two glyphs.
            (_, 0) => {
                for n in 1..cw {
                    link(buf, col + dx * n, row, '\u{2500}', sgr);
                }
            }
            // Vertical and diagonal both need a row between rows.
            _ if ch < 2 => {}
            (0, _) => link(buf, col, row + dy, '\u{2502}', sgr),
            _ => {
                // `╲` runs NW-SE, `╱` runs NE-SW.
                let glyph = if dx == dy { '\u{2572}' } else { '\u{2571}' };
                link(buf, col + dx * half, row + dy, glyph, sgr);
            }
        }
    }
}

/// Write a connector, crossing it with whatever is already there.
///
/// On a square grid the SE link out of one cell and the SW link out of
/// its eastern neighbour land on the SAME character: both are the centre
/// of the same square. Plain overwriting meant one diagonal always won
/// and the other was invisible everywhere. `╳` says both are real.
fn link(buf: &mut [Vec<(char, Ink)>], x: i64, y: i64, glyph: char, ink: Ink) {
    let here = peek(buf, x, y);
    let glyph = match (here, glyph) {
        (Some('\u{2572}'), '\u{2571}') | (Some('\u{2571}'), '\u{2572}') => '\u{2573}',
        (Some('\u{2573}'), _) => '\u{2573}',
        _ => glyph,
    };
    put(buf, x, y, glyph, ink);
}

fn peek(buf: &[Vec<(char, Ink)>], x: i64, y: i64) -> Option<char> {
    read(buf, x, y).map(|(c, _)| c)
}

fn read(buf: &[Vec<(char, Ink)>], x: i64, y: i64) -> Option<(char, Ink)> {
    let (rows, cols) = (buf.len() as i64, buf.first().map_or(0, Vec::len) as i64);
    if x < 0 || y < 0 || x >= cols || y >= rows {
        return None;
    }
    Some(buf[y as usize][x as usize])
}

/// One buffer row as an escaped string, one SGR change per run.
///
/// Foreground, background and the cursor's reverse are joined here rather
/// than being pre-composed per cell: a run change is per-run, and the
/// alternative was a cache keyed on every combination the palette can
/// make.
fn emit(row: Vec<(char, Ink)>) -> String {
    let mut out = String::new();
    let mut current = Ink::default();
    for (c, ink) in row {
        if ink != current {
            out.push_str("\x1b[0");
            if !ink.fg.is_empty() {
                out.push(';');
                out.push_str(ink.fg);
            }
            if !ink.bg.is_empty() {
                out.push(';');
                out.push_str(ink.bg);
            }
            if ink.cursor {
                out.push_str(";7");
            }
            out.push('m');
            current = ink;
        }
        out.push(c);
    }
    if current != Ink::default() {
        out.push_str("\x1b[0m");
    }
    // Trailing blanks cost columns on a narrow terminal and say nothing.
    let trimmed = out.trim_end().to_string();
    if trimmed.ends_with("\x1b[0m") || !trimmed.contains('\x1b') {
        trimmed
    } else {
        trimmed + "\x1b[0m"
    }
}

/// Printable columns, ignoring SGR escapes. The bound every render is
/// held to: escapes are free, characters are not.
pub fn visible_width(line: &str) -> usize {
    strip_sgr(line).chars().count()
}

/// A line with its SGR escapes removed, for measuring and for tests.
pub fn strip_sgr(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // CSI ... final byte in @..~; anything else is not ours to skip.
        if chars.next() != Some('[') {
            continue;
        }
        for c in chars.by_ref() {
            if ('@'..='~').contains(&c) {
                break;
            }
        }
    }
    out
}
