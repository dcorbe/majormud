//! The interactive map view's state machine.
//!
//! Split from the terminal loop on purpose: everything here is a pure
//! function of keystrokes, so the parts worth getting right — where the
//! cursor goes, when the viewport follows, what a plane hop re-anchors
//! on — are testable without a terminal or a board.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mud_client::graph::RoomGraph;
use mud_client::map::{Paint, PaintCtx, Zoom};
use mud_client::mapview::{MapView, ViewAction};
use mud_client::spawn::SpawnTable;
use mud_core::content::RoomId;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn graph() -> std::sync::Arc<RoomGraph> {
    use std::sync::{Arc, OnceLock};
    static G: OnceLock<Arc<RoomGraph>> = OnceLock::new();
    G.get_or_init(|| Arc::new(RoomGraph::load(&db_path()).expect("graph")))
        .clone()
}

fn spawns() -> std::sync::Arc<SpawnTable> {
    use std::sync::{Arc, OnceLock};
    static S: OnceLock<Arc<SpawnTable>> = OnceLock::new();
    S.get_or_init(|| Arc::new(SpawnTable::load(&db_path()).expect("spawns")))
        .clone()
}

const CROSSROADS: RoomId = RoomId { map: 1, room: 1076 };
const SMALL_CAVERN: RoomId = RoomId { map: 1, room: 2156 };
/// A slum street with a sewer grate: one of the plane's few links down.
const SLUM_BEND: RoomId = RoomId { map: 1, room: 1084 };

/// Walk the cursor off the drawn rooms entirely. Bounded by the plane's
/// own height, so it cannot loop forever if the layout changes.
fn walk_off_the_map(v: &mut MapView) {
    let limit = v.plane().extent().height() + 4;
    for _ in 0..limit {
        press(v, KeyCode::Up);
        if v.cursor_room().is_none() {
            return;
        }
    }
    panic!("still on a room after {limit} steps north");
}

fn view() -> MapView {
    MapView::new(
        graph(),
        spawns(),
        CROSSROADS,
        Some(CROSSROADS),
        PaintCtx::default(),
        (100, 30),
    )
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn press(v: &mut MapView, code: KeyCode) -> ViewAction {
    v.on_key(&key(code))
}

#[test]
fn it_opens_on_the_room_it_was_given() {
    let v = view();
    assert_eq!(v.cursor_room(), Some(CROSSROADS));
    assert_eq!(v.zoom(), Zoom::Normal);
    assert_eq!(v.paint(), Paint::Terrain);
}

#[test]
fn the_cursor_moves_one_cell_per_press() {
    let mut v = view();
    let (x, y) = v.cursor();
    press(&mut v, KeyCode::Right);
    assert_eq!(v.cursor(), (x + 1, y));
    press(&mut v, KeyCode::Down);
    assert_eq!(v.cursor(), (x + 1, y + 1));
    press(&mut v, KeyCode::Char('h'));
    assert_eq!(v.cursor(), (x, y + 1));
    press(&mut v, KeyCode::Char('k'));
    assert_eq!(v.cursor(), (x, y), "hjkl and the arrows are the same keys");
}

/// The cursor may leave the rooms: crossing a gap to reach the next
/// street is an ordinary thing to do, and refusing to move would make
/// the map feel stuck.
#[test]
fn the_cursor_may_stand_on_an_empty_cell() {
    let mut v = view();
    walk_off_the_map(&mut v);
    assert_eq!(v.cursor_room(), None);
    assert!(!v.lines().is_empty(), "and the view still paints");
}

#[test]
fn the_viewport_follows_the_cursor_only_at_the_margin() {
    let mut v = view();
    let start = v.viewport();
    press(&mut v, KeyCode::Right);
    assert_eq!(v.viewport(), start, "one step from the middle scrolls nothing");
    for _ in 0..60 {
        press(&mut v, KeyCode::Right);
    }
    assert!(v.viewport().0 > start.0, "running at the edge scrolls");
    // Whatever it scrolled to, the cursor is still on screen.
    assert!(v.cursor_visible(), "the cursor must never scroll away");
}

/// Panning is for looking somewhere far off without losing your place.
#[test]
fn panning_moves_the_viewport_and_leaves_the_cursor() {
    let mut v = view();
    let cursor = v.cursor();
    let start = v.viewport();
    v.on_key(&shift(KeyCode::Right));
    assert_eq!(v.cursor(), cursor, "the cursor stays put");
    assert!(v.viewport().0 > start.0, "by a screenful");
    assert!(
        v.viewport().0 - start.0 > 5,
        "a screenful, not a cell: {:?} -> {:?}",
        start,
        v.viewport()
    );
}

#[test]
fn home_recentres_on_the_character() {
    let mut v = view();
    let home = v.viewport();
    v.on_key(&shift(KeyCode::Right));
    v.on_key(&shift(KeyCode::Down));
    assert_ne!(v.viewport(), home);
    press(&mut v, KeyCode::Home);
    assert_eq!(v.cursor_room(), Some(CROSSROADS));
    assert_eq!(v.viewport(), home);
}

#[test]
fn zoom_walks_the_three_levels_and_stops() {
    let mut v = view();
    press(&mut v, KeyCode::Char('-'));
    assert_eq!(v.zoom(), Zoom::Overview);
    press(&mut v, KeyCode::Char('-'));
    assert_eq!(v.zoom(), Zoom::Overview, "no fourth level to fall off");
    press(&mut v, KeyCode::Char('+'));
    assert_eq!(v.zoom(), Zoom::Normal);
    press(&mut v, KeyCode::Char('+'));
    assert_eq!(v.zoom(), Zoom::Detail);
    press(&mut v, KeyCode::Char('+'));
    assert_eq!(v.zoom(), Zoom::Detail);
}

/// Zooming is for looking closer at what you are looking at, so the
/// thing under the cursor has to stay under it.
#[test]
fn zooming_keeps_the_cursor_room_on_screen() {
    let mut v = view();
    for code in [
        KeyCode::Char('-'),
        KeyCode::Char('+'),
        KeyCode::Char('+'),
        KeyCode::Char('-'),
    ] {
        press(&mut v, code);
        assert_eq!(v.cursor_room(), Some(CROSSROADS));
        assert!(v.cursor_visible(), "{code:?} scrolled the cursor away");
    }
}

#[test]
fn m_cycles_the_paint_modes() {
    let mut v = view();
    let mut seen = vec![v.paint()];
    for _ in 0..2 {
        press(&mut v, KeyCode::Char('m'));
        seen.push(v.paint());
    }
    assert_eq!(seen, [Paint::Terrain, Paint::Danger, Paint::Worth]);
    press(&mut v, KeyCode::Char('m'));
    assert_eq!(v.paint(), Paint::Terrain, "and wraps");
}

/// Up and down leave the plane, so the view has to re-lay-out around
/// where they land — that is the only way to see a cave's lower level.
#[test]
fn a_plane_hop_re_anchors_on_the_far_side() {
    let mut v = view();
    let cell = v.plane().cell_of(SLUM_BEND).expect("on the slum plane");
    let dest = v
        .plane()
        .links_from(SLUM_BEND)
        .next()
        .expect("the bend has a grate down")
        .dest;

    press(&mut v, KeyCode::Char('/'));
    for c in "1/1084".chars() {
        press(&mut v, KeyCode::Char(c));
    }
    press(&mut v, KeyCode::Enter);
    assert_eq!(v.cursor(), cell);

    press(&mut v, KeyCode::Char('>'));
    assert_eq!(v.cursor_room(), Some(dest));
    assert_eq!(v.plane().anchor(), dest);
    assert_eq!(v.cursor(), (0, 0), "the new plane's anchor is the origin");

    // And back the way we came, to the room we hopped from.
    press(&mut v, KeyCode::Char('<'));
    assert_eq!(v.cursor_room(), Some(SLUM_BEND));
}

#[test]
fn a_room_with_no_link_does_not_hop() {
    let mut v = view();
    let before = v.plane().anchor();
    press(&mut v, KeyCode::Char('>'));
    assert_eq!(v.plane().anchor(), before);
    assert!(v.message().is_some(), "and says why");
}

#[test]
fn search_jumps_the_cursor_to_a_named_room() {
    let mut v = view();
    press(&mut v, KeyCode::Char('/'));
    assert!(v.prompt().is_some(), "the view is taking a line now");
    for c in "1/1195".chars() {
        press(&mut v, KeyCode::Char(c));
    }
    press(&mut v, KeyCode::Enter);
    assert_eq!(v.prompt(), None);
    assert_eq!(v.cursor_room(), Some(RoomId { map: 1, room: 1195 }));
    assert!(v.cursor_visible());
}

/// A search that lands off the current plane has to bring the plane with
/// it, or the cursor would point at a room the map is not drawing.
#[test]
fn a_search_off_the_plane_re_anchors() {
    let mut v = view();
    let target = RoomId { map: 2, room: 1 };
    assert_eq!(v.plane().cell_of(target), None, "not on the slum plane");
    press(&mut v, KeyCode::Char('/'));
    for c in "2/1".chars() {
        press(&mut v, KeyCode::Char(c));
    }
    press(&mut v, KeyCode::Enter);
    assert_eq!(v.cursor_room(), Some(target));
    assert_eq!(v.plane().anchor(), target);
}

#[test]
fn a_search_that_matches_nothing_says_so_and_keeps_the_cursor() {
    let mut v = view();
    let before = v.cursor();
    press(&mut v, KeyCode::Char('/'));
    for c in "zzzznotaroom".chars() {
        press(&mut v, KeyCode::Char(c));
    }
    press(&mut v, KeyCode::Enter);
    assert_eq!(v.cursor(), before);
    assert!(v.message().is_some());
}

#[test]
fn escape_abandons_a_search_rather_than_leaving_the_map() {
    let mut v = view();
    press(&mut v, KeyCode::Char('/'));
    press(&mut v, KeyCode::Char('x'));
    assert!(matches!(press(&mut v, KeyCode::Esc), ViewAction::Continue));
    assert_eq!(v.prompt(), None);
    assert!(matches!(press(&mut v, KeyCode::Esc), ViewAction::Leave));
}

#[test]
fn g_asks_to_walk_to_the_room_under_the_cursor() {
    let mut v = view();
    press(&mut v, KeyCode::Right);
    let want = v.cursor_room().expect("a room east of the crossroads");
    assert!(matches!(press(&mut v, KeyCode::Char('g')), ViewAction::Go(id) if id == want));
}

#[test]
fn g_on_an_empty_cell_walks_nowhere() {
    let mut v = view();
    walk_off_the_map(&mut v);
    assert!(matches!(
        press(&mut v, KeyCode::Char('g')),
        ViewAction::Continue
    ));
    assert!(v.message().is_some());
}

#[test]
fn q_leaves_and_ctrl_q_quits() {
    let mut v = view();
    assert!(matches!(press(&mut v, KeyCode::Char('q')), ViewAction::Leave));
    assert!(matches!(
        v.on_key(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        ViewAction::Quit
    ));
}

// --- painting --------------------------------------------------------

#[test]
fn a_frame_fits_the_terminal_it_was_given() {
    for size in [(100usize, 30usize), (80, 24), (200, 60), (40, 10)] {
        let mut v = MapView::new(
            graph(),
            spawns(),
            CROSSROADS,
            Some(CROSSROADS),
            PaintCtx::default(),
            size,
        );
        for zoom in [KeyCode::Char('-'), KeyCode::Char('+'), KeyCode::Char('+')] {
            press(&mut v, zoom);
            let lines = v.lines();
            assert!(lines.len() <= size.1, "{size:?} painted {} rows", lines.len());
            for line in &lines {
                let w = mud_client::map::visible_width(line);
                assert!(w <= size.0, "{size:?} painted {w} columns: {line:?}");
            }
        }
    }
}

/// The panel is the same `Dossier::lines` `/room` prints. One renderer,
/// or the two drift.
#[test]
fn the_panel_describes_the_room_under_the_cursor() {
    let mut v = MapView::new(
        graph(),
        spawns(),
        SMALL_CAVERN,
        Some(SMALL_CAVERN),
        PaintCtx::default(),
        (120, 30),
    );
    let frame = v
        .lines()
        .iter()
        .map(|l| mud_client::map::strip_sgr(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(frame.contains("Small Cavern"), "{frame}");
    assert!(frame.contains("cave bear"), "{frame}");
    press(&mut v, KeyCode::Char('m'));
    assert!(
        v.lines()
            .iter()
            .any(|l| mud_client::map::strip_sgr(l).contains("danger")),
        "the frame names the mode it is painting in"
    );
}

#[test]
fn a_frame_carries_no_stray_control_characters() {
    let v = view();
    for line in v.lines() {
        for c in mud_client::map::strip_sgr(&line).chars() {
            assert!(!c.is_control(), "control character in {line:?}");
        }
    }
}
