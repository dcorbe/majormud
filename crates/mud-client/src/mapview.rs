//! The interactive map: a full-screen view you scroll around to read the
//! world and pick places to go.
//!
//! The state machine is separated from the terminal loop deliberately.
//! Everything in [`MapView`] is a pure function of keystrokes — where the
//! cursor goes, when the viewport follows it, what a plane hop
//! re-anchors on — so the parts worth getting right are testable without
//! a terminal or a board. [`run`] is the thin async shell that feeds it
//! keys and paints what it says.
//!
//! **The view owns the screen while it is up.** That is the deliberate
//! choice recorded in `docs/plans/2026-08-02-mmc-map-and-loops-design.md`:
//! an alternate screen gives the map the whole terminal, at the cost of
//! being blind to the board. So [`run`] keeps draining the session while
//! it draws — the assist and the counters go on working — and bounces
//! back to the board on a death, which is the one thing you must not miss.

use std::collections::BTreeMap;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mud_core::content::RoomId;

use crate::graph::RoomGraph;
use crate::loops::{Loop, Stop, route_rooms};
use crate::map::{Cell, Marks, Paint, PaintCtx, Plane, Style, Zoom, layout, render, styles};
use crate::spawn::{Dossier, SpawnTable};

/// Width of the room panel, in columns. Wide enough for a monster name
/// and its numbers on one line.
const PANEL: usize = 34;
/// A terminal narrower than this gets the map and no panel: 40 columns of
/// map is already cramped, and splitting it further helps nobody.
const PANEL_NEEDS: usize = 60;
/// How close the cursor may come to the edge before the viewport follows.
const MARGIN: i32 = 2;

/// What a keystroke asked of the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewAction {
    /// Handled; repaint and keep going.
    Continue,
    /// Close the map and give the board its screen back.
    Leave,
    /// Close the client entirely.
    Quit,
    /// Leave the map and walk to this room.
    Go(RoomId),
    /// Write this loop to the library. Built here and written by the
    /// caller: the view stays free of the filesystem, which is what
    /// makes every key it handles testable.
    Save(Box<Loop>),
}

/// What the `/` prompt is collecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asking {
    /// A room to jump the cursor to.
    Room,
    /// A name to save the marked stops under.
    LoopName,
}

pub struct MapView {
    graph: Arc<RoomGraph>,
    spawns: Arc<SpawnTable>,
    /// Where the character stands, if the client knows.
    here: Option<RoomId>,
    ctx: PaintCtx,
    plane: Plane,
    styles: BTreeMap<RoomId, Style>,
    cursor: Cell,
    /// Top-left cell of the viewport.
    view: Cell,
    zoom: Zoom,
    paint: Paint,
    /// The whole terminal, in characters.
    size: (usize, usize),
    /// Planes hopped out of, most recent last. `<` walks back up it.
    back: Vec<(RoomId, Cell)>,
    /// The marked circuit, in the order it will be walked.
    stops: Vec<RoomId>,
    /// Every room the walk passes through, recomputed when the stops
    /// change rather than per frame: the map repaints on every keystroke
    /// and this is a BFS per leg.
    route: std::collections::BTreeSet<RoomId>,
    /// Set while a prompt is taking a line.
    prompt: Option<(Asking, String)>,
    /// One line of explanation, cleared by the next keystroke.
    message: Option<String>,
}

impl MapView {
    pub fn new(
        graph: Arc<RoomGraph>,
        spawns: Arc<SpawnTable>,
        anchor: RoomId,
        here: Option<RoomId>,
        ctx: PaintCtx,
        size: (usize, usize),
    ) -> MapView {
        let plane = layout(&graph, anchor);
        let mut view = MapView {
            styles: styles(&plane, &graph, &spawns, Paint::Terrain, &ctx),
            plane,
            graph,
            spawns,
            here,
            ctx,
            cursor: (0, 0),
            view: (0, 0),
            zoom: Zoom::Normal,
            paint: Paint::Terrain,
            size,
            back: Vec::new(),
            stops: Vec::new(),
            route: Default::default(),
            prompt: None,
            message: None,
        };
        view.cursor = view.plane.cell_of(anchor).unwrap_or((0, 0));
        view.centre();
        view
    }

    pub fn cursor(&self) -> Cell {
        self.cursor
    }

    pub fn cursor_room(&self) -> Option<RoomId> {
        self.plane.room_at(self.cursor)
    }

    pub fn viewport(&self) -> Cell {
        self.view
    }

    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    pub fn paint(&self) -> Paint {
        self.paint
    }

    pub fn plane(&self) -> &Plane {
        &self.plane
    }

    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_ref().map(|(_, text)| text.as_str())
    }

    /// The marked circuit, in walking order.
    pub fn stops(&self) -> &[RoomId] {
        &self.stops
    }

    pub fn route(&self) -> &std::collections::BTreeSet<RoomId> {
        &self.route
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn resize(&mut self, size: (usize, usize)) {
        self.size = size;
        self.centre();
    }

    /// The map area, in characters: the terminal less the panel and the
    /// status line.
    fn map_area(&self) -> (usize, usize) {
        let panel = if self.size.0 >= PANEL_NEEDS {
            PANEL + 1
        } else {
            0
        };
        (
            self.size.0.saturating_sub(panel).max(1),
            self.size.1.saturating_sub(1).max(1),
        )
    }

    /// How many cells fit in the map area at the current zoom.
    fn visible(&self) -> (i32, i32) {
        let (w, h) = self.map_area();
        let (cw, ch) = self.zoom.cell();
        ((w / cw).max(1) as i32, (h / ch).max(1) as i32)
    }

    pub fn cursor_visible(&self) -> bool {
        let (vw, vh) = self.visible();
        (self.view.0..self.view.0 + vw).contains(&self.cursor.0)
            && (self.view.1..self.view.1 + vh).contains(&self.cursor.1)
    }

    fn centre(&mut self) {
        let (vw, vh) = self.visible();
        self.view = (self.cursor.0 - vw / 2, self.cursor.1 - vh / 2);
    }

    /// Scroll only when the cursor comes within [`MARGIN`] of an edge.
    /// A viewport that recentred on every step would make the whole map
    /// slide under a cursor that never moves, which is unreadable.
    fn follow(&mut self) {
        let (vw, vh) = self.visible();
        // On a tiny viewport the margins would overlap and fight; half
        // the span is the most either can claim.
        let (mx, my) = (MARGIN.min(vw / 2), MARGIN.min(vh / 2));
        self.view.0 = self.view.0.min(self.cursor.0 - mx);
        self.view.0 = self.view.0.max(self.cursor.0 - vw + mx + 1);
        self.view.1 = self.view.1.min(self.cursor.1 - my);
        self.view.1 = self.view.1.max(self.cursor.1 - vh + my + 1);
    }

    fn restyle(&mut self) {
        self.styles = styles(
            &self.plane,
            &self.graph,
            &self.spawns,
            self.paint,
            &self.ctx,
        );
    }

    /// Draw a different plane, keeping the one we left on the back stack.
    fn hop_to(&mut self, room: RoomId, remember: bool) {
        if remember {
            self.back.push((self.plane.anchor(), self.cursor));
        }
        self.plane = layout(&self.graph, room);
        self.cursor = self.plane.cell_of(room).unwrap_or((0, 0));
        self.restyle();
        self.centre();
    }

    /// Put the cursor on a room, bringing the plane along when the room
    /// is not on the one being drawn. Without that the cursor would
    /// point at something the map is not showing.
    fn go_to_room(&mut self, room: RoomId) {
        match self.plane.cell_of(room) {
            Some(cell) => {
                self.cursor = cell;
                self.follow();
            }
            None => self.hop_to(room, true),
        }
    }

    pub fn on_key(&mut self, key: &KeyEvent) -> ViewAction {
        self.message = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('q') => ViewAction::Quit,
                _ => ViewAction::Continue,
            };
        }
        if self.prompt.is_some() {
            return self.prompt_key(key);
        }
        let shifted = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            // Panning: a screenful, cursor left where it was.
            KeyCode::Left | KeyCode::Char('H') if shifted => self.pan(-1, 0),
            KeyCode::Right | KeyCode::Char('L') if shifted => self.pan(1, 0),
            KeyCode::Up | KeyCode::Char('K') if shifted => self.pan(0, -1),
            KeyCode::Down | KeyCode::Char('J') if shifted => self.pan(0, 1),

            KeyCode::Left | KeyCode::Char('h') => self.step(-1, 0),
            KeyCode::Right | KeyCode::Char('l') => self.step(1, 0),
            KeyCode::Up | KeyCode::Char('k') => self.step(0, -1),
            KeyCode::Down | KeyCode::Char('j') => self.step(0, 1),
            // Diagonals on the roguelike keys, because MajorMUD streets
            // run diagonally all the time -- the slums are full of them --
            // and reaching one by zig-zagging two orthogonals lands
            // somewhere else entirely once movement snaps room to room.
            KeyCode::Char('y') => self.step(-1, -1),
            KeyCode::Char('u') => self.step(1, -1),
            KeyCode::Char('b') => self.step(-1, 1),
            KeyCode::Char('n') => self.step(1, 1),

            KeyCode::Home => {
                match self.here {
                    Some(here) => {
                        // Re-anchor rather than just moving the cursor: the
                        // character may be on another plane entirely.
                        if self.plane.cell_of(here).is_none() {
                            self.hop_to(here, true);
                        } else {
                            self.cursor = self.plane.cell_of(here).unwrap_or(self.cursor);
                        }
                        self.centre();
                    }
                    None => self.message = Some("nobody knows where you are standing".into()),
                }
            }

            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.zoom = self.zoom.zoom_in();
                self.centre();
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.zoom = self.zoom.zoom_out();
                self.centre();
            }
            KeyCode::Char('m') => {
                self.paint = match self.paint {
                    Paint::Terrain => Paint::Danger,
                    Paint::Danger => Paint::Worth,
                    Paint::Worth => Paint::Terrain,
                };
                self.restyle();
            }

            KeyCode::Char('>') => match self.cursor_room().and_then(|r| {
                self.plane
                    .links_from(r)
                    .next()
                    .map(|l| (l.dir, l.dest))
            }) {
                Some((_, dest)) => self.hop_to(dest, true),
                None => {
                    self.message = Some("no stairs or portal here (+ marks the ones there are)".into())
                }
            },
            KeyCode::Char('<') => match self.back.pop() {
                Some((anchor, cursor)) => {
                    self.hop_to(anchor, false);
                    self.cursor = cursor;
                    self.centre();
                }
                None => self.message = Some("nowhere to go back to".into()),
            },

            KeyCode::Char('/') => self.prompt = Some((Asking::Room, String::new())),

            KeyCode::Enter | KeyCode::Char(' ') => self.toggle_stop(),
            KeyCode::Char('c') => {
                self.stops.clear();
                self.route.clear();
            }
            KeyCode::Char('s') => {
                if self.stops.is_empty() {
                    self.message = Some("mark some stops first (enter or space)".into());
                } else {
                    self.prompt = Some((Asking::LoopName, String::new()));
                }
            }

            KeyCode::Char('g') => {
                return match self.cursor_room() {
                    Some(id) => ViewAction::Go(id),
                    None => {
                        self.message = Some("no room under the cursor".into());
                        ViewAction::Continue
                    }
                };
            }

            KeyCode::Char('q') | KeyCode::Esc => return ViewAction::Leave,
            _ => {}
        }
        ViewAction::Continue
    }

    /// Move the cursor to the nearest room that way, skipping the gaps.
    ///
    /// The map is mostly empty space — streets are thin and the plane is
    /// wide — so a cursor that stepped one cell per press spent most of
    /// its life on nothing, with a blank panel and no way to tell where
    /// it had got to. Snapping makes "where is the cursor" unanswerable
    /// by construction: it is always on a room.
    ///
    /// Candidates are ranked by how far OFF the axis they sit first and
    /// how far along it second, so a straight run east prefers the next
    /// room in the same row, and a street that jogs one row over is still
    /// found. Scanning only the exact row would strand the cursor the
    /// moment a corridor stopped being straight.
    ///
    /// Nothing that way means no move. The cursor visibly staying put is
    /// a clearer answer than drifting into the void.
    fn step(&mut self, dx: i32, dy: i32) {
        let (cx, cy) = self.cursor;
        let best = self
            .plane
            .rooms()
            .filter_map(|id| self.plane.cell_of(id))
            .filter_map(|(x, y)| {
                let (vx, vy) = (x - cx, y - cy);
                // Distance ALONG the direction and deviation OFF it, as a
                // dot and a cross product. One formula for all eight
                // directions: for east it reads as (dx, |dy|), for
                // south-east as (dx+dy, |dx-dy|), and a room exactly on
                // the diagonal deviates by zero as it should.
                let along = vx * dx + vy * dy;
                let off = (vx * dy - vy * dx).abs();
                (along > 0).then_some(((off, along), (x, y)))
            })
            .min_by_key(|(rank, _)| *rank);
        if let Some((_, cell)) = best {
            self.cursor = cell;
            self.follow();
        }
    }

    fn pan(&mut self, dx: i32, dy: i32) {
        let (vw, vh) = self.visible();
        self.view = (self.view.0 + dx * vw, self.view.1 + dy * vh);
    }

    /// Mark or unmark the room under the cursor.
    ///
    /// Order is walking order, so a stop taken off and put back goes to
    /// the end — which is what somebody rebuilding a leg means by it.
    fn toggle_stop(&mut self) {
        let Some(id) = self.cursor_room() else {
            self.message = Some("no room under the cursor".into());
            return;
        };
        match self.stops.iter().position(|&s| s == id) {
            Some(i) => {
                self.stops.remove(i);
            }
            None => self.stops.push(id),
        }
        self.route = route_rooms(&self.graph, &self.stops);
    }

    /// The marked circuit as a loop file, ready for the caller to write.
    ///
    /// Each stop carries its room name: that is what lets a later load
    /// notice the file was written against a different world, and the
    /// map is the one place that knows the name for free.
    fn to_loop(&self, name: &str) -> Loop {
        Loop {
            stops: self
                .stops
                .iter()
                .map(|&id| Stop::named(id, &self.graph))
                .collect(),
            ..Loop::new(name.trim())
        }
    }

    fn prompt_key(&mut self, key: &KeyEvent) -> ViewAction {
        let Some((asking, text)) = self.prompt.as_mut() else {
            return ViewAction::Continue;
        };
        let asking = *asking;
        match key.code {
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            // Abandoning a prompt must not also close the map: one Esc,
            // one thing.
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => {
                let typed = self.prompt.take().map(|(_, t)| t).unwrap_or_default();
                if typed.trim().is_empty() {
                    return ViewAction::Continue;
                }
                match asking {
                    // Same resolver as `/go`, so a name means the same
                    // thing in the map as on the command line.
                    Asking::Room => {
                        match crate::go::resolve(
                            &self.graph,
                            self.cursor_room().or(self.here),
                            &typed,
                        ) {
                            Ok(id) => self.go_to_room(id),
                            Err(refusal) => self.message = refusal.lines().first().cloned(),
                        }
                    }
                    Asking::LoopName => {
                        return ViewAction::Save(Box::new(self.to_loop(&typed)));
                    }
                }
            }
            _ => {}
        }
        ViewAction::Continue
    }

    /// The whole frame, one string per terminal row.
    pub fn lines(&self) -> Vec<String> {
        let (map_w, map_h) = self.map_area();
        let marks = Marks {
            here: self.here,
            cursor: Some(self.cursor),
            stops: self.stops.iter().copied().collect(),
            route: self.route.clone(),
        };
        let map = render(
            &self.plane,
            &self.styles,
            self.view,
            (map_w, map_h),
            self.zoom,
            &marks,
        );
        let panel = if self.size.0 >= PANEL_NEEDS {
            self.panel(map_h)
        } else {
            Vec::new()
        };

        let mut out = Vec::with_capacity(map_h + 1);
        for row in 0..map_h {
            let mut line = map.get(row).cloned().unwrap_or_default();
            if let Some(text) = panel.get(row) {
                // A ruled edge, because the map's own right margin is
                // ragged and the panel read as part of it without one.
                pad_to(&mut line, map_w);
                line.push('\u{2502}');
                line.push_str(text);
            }
            out.push(line);
        }
        out.push(self.status());
        out
    }

    /// The room under the cursor, described by the same renderer `/room`
    /// uses. Two renderers would be two things to keep in step.
    fn panel(&self, rows: usize) -> Vec<String> {
        let mut lines = match self.cursor_room() {
            Some(id) => Dossier::of(&self.graph, &self.spawns, id)
                .map(|d| d.lines())
                .unwrap_or_default(),
            None => vec![format!("({}, {}) nothing here", self.cursor.0, self.cursor.1)],
        };
        let e = self.plane.extent();
        lines.push(String::new());
        lines.push(format!(
            "plane {} rooms, {}x{}",
            self.plane.len(),
            e.width(),
            e.height()
        ));
        if !self.plane.conflicts().is_empty() {
            lines.push(format!("{} rooms not placed", self.plane.conflicts().len()));
        }
        lines.push(format!("view ({}, {})", self.view.0, self.view.1));
        lines.truncate(rows);
        lines
            .into_iter()
            .map(|l| l.chars().take(PANEL).collect())
            .collect()
    }

    fn status(&self) -> String {
        let text = match (&self.prompt, &self.message) {
            (Some((Asking::Room, typed)), _) => format!("find: {typed}"),
            (Some((Asking::LoopName, typed)), _) => format!(
                "save {} stops as: {typed}",
                self.stops.len()
            ),
            (None, Some(msg)) => format!("-- {msg} --"),
            (None, None) => format!(
                "{} | {} | {} stops | arrows/hjkl move  yubn diagonals  +/- zoom  m mode  / find  enter marks  s saves  g go  q leave",
                match self.paint {
                    Paint::Terrain => "terrain",
                    Paint::Danger => "danger",
                    Paint::Worth => "worth",
                },
                match self.zoom {
                    Zoom::Detail => "detail",
                    Zoom::Normal => "normal",
                    Zoom::Overview => "overview",
                },
                self.stops.len(),
            ),
        };
        let mut line: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(self.size.0)
            .collect();
        while line.chars().count() < self.size.0 {
            line.push(' ');
        }
        format!("\x1b[7m{line}\x1b[0m")
    }
}

/// How a map session ended, and what the board said while it was up.
pub struct ViewExit {
    pub action: ViewAction,
    /// Board output that arrived while the map owned the screen. The
    /// caller flushes it into the scroll region, so nothing said during
    /// planning is lost.
    pub buffered: Vec<u8>,
    /// Set when the map closed itself rather than being closed.
    pub interrupted: Option<String>,
}

/// Paint one frame into the alternate screen.
fn paint(out: &mut impl std::io::Write, view: &MapView) -> std::io::Result<()> {
    // Home, then one line at a time with an explicit clear-to-end: a
    // full clear per frame flickers, and a frame that only overwrote
    // would leave the last one's tails behind.
    out.write_all(b"\x1b[H")?;
    let lines = view.lines();
    for (i, line) in lines.iter().enumerate() {
        // No newline after the LAST row. Writing one in the bottom-right
        // cell scrolls the terminal, which silently ate the first row of
        // every frame.
        if i > 0 {
            out.write_all(b"\r\n")?;
        }
        out.write_all(line.as_bytes())?;
        out.write_all(b"\x1b[K")?;
    }
    out.write_all(b"\x1b[J")?;
    out.flush()
}

/// Enter the alternate screen, and leave it however this returns.
struct Screen;

impl Screen {
    fn enter() -> std::io::Result<Screen> {
        crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide
        )?;
        // Clear the scroll margins. The alternate screen INHERITS the
        // DECSTBM region `tui::play` set for the board's passthrough, so
        // without this every frame scrolls inside those margins and the
        // top row is eaten (live, 2026-08-02 — invisible offline, where
        // no margins are ever set). `play` re-establishes its own region
        // when the map hands the terminal back.
        let mut out = std::io::stdout();
        std::io::Write::write_all(&mut out, b"\x1b[r")?;
        std::io::Write::flush(&mut out)?;
        Ok(Screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // Best effort, and unconditional: a panic or an early return that
        // left the terminal on the alternate screen would take the whole
        // session with it.
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::cursor::Show,
            crossterm::terminal::LeaveAlternateScreen
        );
    }
}

/// Drive the map until the operator closes it or the board interrupts.
///
/// The session keeps running underneath. Board bytes are buffered rather
/// than dropped, and `on_event` is still called for every correlated
/// event — that is what keeps the assist fighting and the counters
/// counting while somebody plans a route.
///
/// The one thing worth going blind for is not worth dying for, so a death
/// line closes the map by itself.
pub async fn run(
    session: &crate::session::Session,
    view: &mut MapView,
    keys: &mut tokio::sync::mpsc::UnboundedReceiver<crossterm::event::Event>,
    raw: &mut tokio::sync::broadcast::Receiver<Vec<u8>>,
    events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
    on_event: &mut dyn FnMut(&crate::correlate::Correlated),
) -> std::io::Result<ViewExit> {
    use crossterm::event::{Event as TermEvent, KeyEventKind};

    let _screen = Screen::enter()?;
    let mut out = std::io::stdout();
    let mut buffered: Vec<u8> = Vec::new();
    let username = session.profile().username.clone();
    paint(&mut out, view)?;

    loop {
        tokio::select! {
            bytes = raw.recv() => match bytes {
                Ok(bytes) => buffered.extend_from_slice(&bytes),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return Ok(ViewExit {
                    action: ViewAction::Leave,
                    buffered,
                    interrupted: Some("disconnected".into()),
                }),
            },
            ev = events.recv() => {
                if let Ok(cor) = &ev {
                    on_event(cor);
                    if let crate::events::Event::Line(line) = &cor.event
                        && crate::farm::is_player_death(line, &username)
                    {
                        return Ok(ViewExit {
                            action: ViewAction::Leave,
                            buffered,
                            interrupted: Some("you died; the map is closed".into()),
                        });
                    }
                }
            }
            key = keys.recv() => match key {
                None => return Ok(ViewExit { action: ViewAction::Quit, buffered, interrupted: None }),
                Some(TermEvent::Resize(cols, rows)) => {
                    view.resize((cols as usize, rows as usize));
                    paint(&mut out, view)?;
                }
                Some(TermEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                    match view.on_key(&key) {
                        ViewAction::Continue => paint(&mut out, view)?,
                        action => return Ok(ViewExit { action, buffered, interrupted: None }),
                    }
                }
                Some(_) => {}
            },
        }
    }
}

/// Raw mode for the offline browser, given back however it returns.
///
/// Not needed by [`run`]: `tui::play` has already put the terminal in raw
/// mode and owns putting it back.
struct RawMode;

impl RawMode {
    fn enter() -> std::io::Result<RawMode> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Browse the map with no board behind it, for `mmc map`.
///
/// The view needs the graph, not a connection, so planning a route
/// between sessions costs nothing. Blocking reads rather than a select:
/// with no session there is nothing else to wait on.
pub fn run_offline(view: &mut MapView) -> std::io::Result<ViewAction> {
    use crossterm::event::{Event as TermEvent, KeyEventKind};

    let _raw = RawMode::enter()?;
    let _screen = Screen::enter()?;
    let mut out = std::io::stdout();
    paint(&mut out, view)?;
    loop {
        match crossterm::event::read()? {
            TermEvent::Resize(cols, rows) => {
                view.resize((cols as usize, rows as usize));
                paint(&mut out, view)?;
            }
            TermEvent::Key(key) if key.kind == KeyEventKind::Press => match view.on_key(&key) {
                ViewAction::Continue => paint(&mut out, view)?,
                action => return Ok(action),
            },
            _ => {}
        }
    }
}

/// Pad a rendered line out to `width` printable columns.
///
/// By visible width, not byte length: the line is full of SGR escapes
/// that cost bytes and no columns, and padding by the wrong one puts the
/// panel in a different place on every row.
fn pad_to(line: &mut String, width: usize) {
    let have = crate::map::visible_width(line);
    if have >= width {
        return;
    }
    if !line.is_empty() {
        line.push_str("\x1b[0m");
    }
    line.extend(std::iter::repeat_n(' ', width - have));
}
