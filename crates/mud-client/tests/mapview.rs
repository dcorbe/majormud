//! The interactive map view's state machine.
//!
//! Split from the terminal loop on purpose: everything here is a pure
//! function of keystrokes, so the parts worth getting right — where the
//! cursor goes, when the viewport follows, what a plane hop re-anchors
//! on — are testable without a terminal or a board.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mud_client::graph::RoomGraph;
use mud_client::lost::Fix;
use mud_client::map::{Paint, PaintCtx, Zoom};
use mud_client::mapview::{MapView, ViewAction};
use mud_client::spawn::SpawnTable;
use mud_core::content::{Direction, RoomId};

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

/// A view anchored on a room the graph has never heard of: the plane is
/// empty, so the cursor genuinely has no room under it.
fn view_of_nowhere() -> MapView {
    MapView::new(
        graph(),
        spawns(),
        RoomId { map: 999, room: 999 },
        Fix::Unknown,
        PaintCtx::default(),
        (100, 30),
    )
}

fn view() -> MapView {
    MapView::new(
        graph(),
        spawns(),
        CROSSROADS,
        Fix::Confirmed(CROSSROADS),
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

/// One press, one cell. The cursor moves the canvas rather than hopping
/// between rooms.
///
/// It used to snap to the nearest room in the pressed direction, ranking
/// candidates by how far OFF the axis they sat. That made a press land
/// an unpredictable distance away, and often somewhere the operator was
/// not aiming — hard to drive in practice, which is why it was reverted.
///
/// The original objection to a cell cursor was that it "spent most of
/// its life on nothing" and vanished. It cannot vanish: `render` paints
/// the cursor last and unconditionally, over empty cells included, and
/// `under_cursor` puts a floor under the reversed colour. The panel
/// being blank over a gap is the accepted cost of aiming precisely.
#[test]
fn one_press_moves_the_cursor_exactly_one_cell() {
    let mut v = view();
    for (code, (dx, dy)) in [
        (KeyCode::Right, (1, 0)),
        (KeyCode::Left, (-1, 0)),
        (KeyCode::Down, (0, 1)),
        (KeyCode::Up, (0, -1)),
        (KeyCode::Char('l'), (1, 0)),
        (KeyCode::Char('h'), (-1, 0)),
        (KeyCode::Char('j'), (0, 1)),
        (KeyCode::Char('k'), (0, -1)),
        (KeyCode::Char('y'), (-1, -1)),
        (KeyCode::Char('u'), (1, -1)),
        (KeyCode::Char('b'), (-1, 1)),
        (KeyCode::Char('n'), (1, 1)),
    ] {
        let from = v.cursor();
        press(&mut v, code);
        assert_eq!(
            v.cursor(),
            (from.0 + dx, from.1 + dy),
            "{code:?} should move exactly one cell"
        );
    }
}

/// A cell with no room in it is a place the cursor may stand. The snap
/// made this impossible by construction; it is now ordinary.
#[test]
fn the_cursor_may_stand_on_empty_space() {
    use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom};
    // Two rooms two cells apart on the same row: A --east--> C is not
    // possible in one step, so build A -e-> B -e-> C and walk past C.
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let mut ra = GraphRoom {
        name: "A".into(),
        ..Default::default()
    };
    ra.exits[Direction::East as usize] = Some(ExitEdge {
        dest: b,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let g = std::sync::Arc::new(RoomGraph::from_rooms(vec![
        (a, ra),
        (
            b,
            GraphRoom {
                name: "B".into(),
                ..Default::default()
            },
        ),
    ]));
    let mut v = MapView::new(g, spawns(), a, Fix::Unknown, PaintCtx::default(), (100, 30));
    // A is at (0,0) and B at (1,0); (2,0) is empty.
    press(&mut v, KeyCode::Right);
    press(&mut v, KeyCode::Right);
    assert_eq!(v.cursor(), (2, 0));
    assert_eq!(v.cursor_room(), None, "standing on nothing is allowed");
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
    press(&mut v, KeyCode::Backspace);
    assert_eq!(v.cursor_room(), Some(SLUM_BEND));
}

#[test]
fn a_room_with_no_link_does_not_hop() {
    let mut v = view();
    let before = v.plane().anchor();
    press(&mut v, KeyCode::Char('>'));
    let _ = before;
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
    let mut v = view_of_nowhere();
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
            Fix::Confirmed(CROSSROADS),
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
        Fix::Confirmed(SMALL_CAVERN),
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

// --- stops and the drawn route ---------------------------------------

#[test]
fn enter_marks_the_room_under_the_cursor_as_a_stop() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    assert_eq!(v.stops(), [CROSSROADS]);
    press(&mut v, KeyCode::Right);
    press(&mut v, KeyCode::Char(' '));
    assert_eq!(v.stops().len(), 2, "space marks too");
    // Toggling: the same key takes it off again, in place.
    press(&mut v, KeyCode::Char(' '));
    assert_eq!(v.stops(), [CROSSROADS]);
}

#[test]
fn marking_an_empty_cell_marks_nothing() {
    let mut v = view_of_nowhere();
    press(&mut v, KeyCode::Enter);
    assert!(v.stops().is_empty());
    assert!(v.message().is_some());
}

/// The gold is the whole walk, not the stops: the point of drawing a
/// route is seeing where it goes.
#[test]
fn two_stops_draw_a_route_between_them() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    assert_eq!(v.route().len(), 1, "one stop is a route of one room");
    for _ in 0..4 {
        press(&mut v, KeyCode::Right);
    }
    press(&mut v, KeyCode::Enter);
    assert!(
        v.route().len() > v.stops().len(),
        "route {:?} should cover the rooms between {:?}",
        v.route().len(),
        v.stops()
    );
    for stop in v.stops() {
        assert!(v.route().contains(stop));
    }
}

#[test]
fn c_clears_the_stops_and_the_route_with_them() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Right);
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('c'));
    assert!(v.stops().is_empty());
    assert!(v.route().is_empty());
}

/// Stops survive a plane hop: a circuit that goes down a staircase and
/// back is exactly the kind nobody can build by hand.
#[test]
fn stops_are_kept_across_a_plane_hop() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('/'));
    for c in "1/1084".chars() {
        press(&mut v, KeyCode::Char(c));
    }
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('>'));
    assert_eq!(v.stops(), [CROSSROADS], "still marked on the plane above");
    press(&mut v, KeyCode::Enter);
    assert_eq!(v.stops().len(), 2);
}

// --- saving ----------------------------------------------------------

#[test]
fn s_asks_for_a_name_and_hands_back_a_loop() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Right);
    press(&mut v, KeyCode::Enter);

    press(&mut v, KeyCode::Char('s'));
    assert!(v.prompt().is_some());
    for c in "slum-sweep".chars() {
        press(&mut v, KeyCode::Char(c));
    }
    let action = press(&mut v, KeyCode::Enter);
    let ViewAction::Save(saved) = action else {
        panic!("expected a loop, got {action:?}");
    };
    assert_eq!(saved.name, "slum-sweep");
    assert_eq!(saved.stops.len(), 2);
    assert_eq!(saved.stops[0].at, "1/1076");
    assert_eq!(
        saved.stops[0].name.as_deref(),
        Some("Slum Street, Crossroads"),
        "the name is written so a foreign realm can be caught on load"
    );
    assert_eq!(saved.world.as_deref(), Some(mud_client::loops::WORLD));
    assert!(saved.check_names(&graph()).is_empty());
}

#[test]
fn saving_with_no_stops_is_refused_before_it_asks_for_a_name() {
    let mut v = view();
    assert!(matches!(
        press(&mut v, KeyCode::Char('s')),
        ViewAction::Continue
    ));
    assert!(v.prompt().is_none());
    assert!(v.message().is_some());
}

#[test]
fn escape_abandons_a_save_the_way_it_abandons_a_search() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('s'));
    assert!(matches!(press(&mut v, KeyCode::Esc), ViewAction::Continue));
    assert!(v.prompt().is_none());
    assert_eq!(v.stops(), [CROSSROADS], "and keeps the stops");
}

/// A saved loop has to be walkable by the runner that will walk it.
#[test]
fn a_saved_loop_validates_as_a_farm_circuit() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    for _ in 0..3 {
        press(&mut v, KeyCode::Right);
    }
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('s'));
    for c in "walkable".chars() {
        press(&mut v, KeyCode::Char(c));
    }
    let ViewAction::Save(saved) = press(&mut v, KeyCode::Enter) else {
        panic!("expected a loop");
    };
    saved
        .to_farm(&mud_client::farm::FarmConfig::default(), &graph())
        .expect("every leg walkable");
}

/// MajorMUD streets run diagonally constantly. Two orthogonal presses
/// can reach the same cell as one diagonal press -- `h` then `j` lands
/// where `b` would have in one -- so the diagonal keys are a shortcut
/// rather than the only route now that movement is one cell at a time.
/// They stay bound because that shortcut is worth having: diagonal
/// streets are common enough that spending two keystrokes on every one
/// of them would be a constant irritation.
#[test]
fn the_diagonal_keys_reach_diagonal_neighbours() {
    use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom};
    let here = RoomId { map: 1, room: 1 };
    let corners = [
        (Direction::NorthWest, 'y', RoomId { map: 1, room: 2 }),
        (Direction::NorthEast, 'u', RoomId { map: 1, room: 3 }),
        (Direction::SouthWest, 'b', RoomId { map: 1, room: 4 }),
        (Direction::SouthEast, 'n', RoomId { map: 1, room: 5 }),
    ];
    let mut hub = GraphRoom {
        name: "Hub".into(),
        ..Default::default()
    };
    let mut rooms = Vec::new();
    for (dir, _, id) in corners {
        hub.exits[dir as usize] = Some(ExitEdge {
            dest: id,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
        rooms.push((
            id,
            GraphRoom {
                name: format!("{dir:?}"),
                ..Default::default()
            },
        ));
    }
    rooms.push((here, hub));
    let g = std::sync::Arc::new(RoomGraph::from_rooms(rooms));

    for (_, key, want) in corners {
        let mut v = MapView::new(
            g.clone(),
            spawns(),
            here,
            Fix::Unknown,
            PaintCtx::default(),
            (100, 30),
        );
        press(&mut v, KeyCode::Char(key));
        assert_eq!(v.cursor_room(), Some(want), "key {key:?}");
    }
}

// --- getting off the plane, 2026-08-02 --------------------------------

const NARROW_ROAD: RoomId = RoomId { map: 1, room: 2146 };
const ARENA: RoomId = RoomId { map: 1, room: 2150 };

/// "exits: n e w d" said a `d` existed and nothing else — not that it
/// left the map, not where it went, not how to follow it. Standing at
/// 1/2146 knowing the Arena is below and being unable to find it on the
/// map is what this fixes.
#[test]
fn the_panel_says_where_a_plane_exit_goes_and_which_key_takes_it() {
    let v = MapView::new(
        graph(),
        spawns(),
        NARROW_ROAD,
        Fix::Confirmed(NARROW_ROAD),
        PaintCtx::default(),
        (120, 30),
    );
    let frame = v
        .lines()
        .iter()
        .map(|l| mud_client::map::strip_sgr(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(frame.contains("down [>]"), "names the direction and key:\n{frame}");
    assert!(frame.contains("1/2150"), "names where it goes:\n{frame}");
    assert!(frame.contains("Arena"), "by name:\n{frame}");
}

/// Rogue's convention, and the only scheme that reaches both halves of a
/// room with an up AND a down exit — nine rooms on this plane have one.
#[test]
fn up_and_down_are_separate_keys() {
    let mut v = MapView::new(
        graph(),
        spawns(),
        NARROW_ROAD,
        Fix::Confirmed(NARROW_ROAD),
        PaintCtx::default(),
        (120, 30),
    );
    press(&mut v, KeyCode::Char('<'));
    assert_eq!(v.plane().anchor(), NARROW_ROAD, "nothing leads up from here");
    assert!(v.message().is_some(), "and it says so");

    press(&mut v, KeyCode::Char('>'));
    assert_eq!(v.cursor_room(), Some(ARENA), "down reaches the Arena");
    assert_eq!(v.plane().anchor(), ARENA);

    press(&mut v, KeyCode::Char('<'));
    assert_eq!(v.cursor_room(), Some(NARROW_ROAD), "and up comes back");
}

/// A lone cross-map portal is reachable with `>`, which falls through to
/// any non-vertical link when the room has no stairs. The seven
/// multi-portal map-edge rooms take the first; the panel lists the rest.
#[test]
fn a_portal_is_reachable_without_a_key_of_its_own() {
    let g = graph();
    let plane = mud_client::map::layout(&g, NARROW_ROAD);
    let portal = plane
        .links()
        .iter()
        .find(|l| mud_client::map::step_of(l.dir).is_some()
            && plane.links_from(l.from).all(|o| mud_client::map::step_of(o.dir).is_some()))
        .expect("a room whose only links are portals");
    let mut v = MapView::new(g, spawns(), portal.from, Fix::Unknown, PaintCtx::default(), (120, 30));
    press(&mut v, KeyCode::Char('>'));
    assert_eq!(v.plane().anchor(), portal.dest, "> took the portal");
}

// --- roam walls -------------------------------------------------------
//
// A wall is the opposite of a stop: a stop says "stand here and fight",
// a wall says "never come here at all". They share the map and the
// cursor and nothing else, which is what these pin.

#[test]
fn x_walls_the_room_under_the_cursor() {
    let mut v = view();
    press(&mut v, KeyCode::Char('x'));
    assert_eq!(v.walls().iter().copied().collect::<Vec<_>>(), [CROSSROADS]);
    // Toggling: the same key takes it off again.
    press(&mut v, KeyCode::Char('x'));
    assert!(v.walls().is_empty());
}

#[test]
fn walling_an_empty_cell_walls_nothing() {
    let mut v = view_of_nowhere();
    press(&mut v, KeyCode::Char('x'));
    assert!(v.walls().is_empty());
    assert!(v.message().is_some(), "and it says why");
}

/// Both selections coexist. An operator may well mark a circuit and a
/// fence in one sitting, and neither key may quietly clobber the other's
/// work.
#[test]
fn walls_and_stops_do_not_clobber_each_other() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Right);
    press(&mut v, KeyCode::Char('x'));

    assert_eq!(v.stops().len(), 1);
    assert_eq!(v.walls().len(), 1);
    assert_ne!(
        v.stops()[0],
        v.walls().iter().copied().next().unwrap(),
        "the cursor moved between them, so they are different rooms"
    );
}

/// `c` clears the lot. Leaving walls behind after clearing the stops
/// would fence a run the operator thought they had reset.
#[test]
fn c_clears_walls_as_well_as_stops() {
    let mut v = view();
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('x'));
    press(&mut v, KeyCode::Char('c'));
    assert!(v.stops().is_empty());
    assert!(v.walls().is_empty());
}

/// A walled room paints red, and it outranks the gold stop marker on a
/// room that is somehow both — the fence is the half that must win,
/// because gold would read as "the run stands here" about a room the run
/// refuses to enter.
#[test]
fn a_wall_paints_red_over_everything() {
    // The cursor is drawn last and in reverse video, which merges into
    // the cell's SGR, so both cases step off the marked room first and
    // read the marker's own colour.
    let mut stopped = view();
    press(&mut stopped, KeyCode::Enter);
    press(&mut stopped, KeyCode::Right);
    let gold = stopped.lines().join("\n");
    assert!(gold.contains("1;33"), "a stop is gold");
    assert!(
        !gold.contains("1;31"),
        "nothing on this fixture is red until something is walled"
    );

    let mut walled = view();
    press(&mut walled, KeyCode::Enter);
    press(&mut walled, KeyCode::Char('x'));
    press(&mut walled, KeyCode::Right);
    let red = walled.lines().join("\n");
    assert!(
        red.contains("1;31"),
        "the same room, now walled as well, must be red"
    );
}

/// `r` hands the walls to the caller and leaves the map. Nothing is
/// written anywhere: a roam is a once-off and the walls die with it.
#[test]
fn r_hands_back_the_walls_and_leaves() {
    let mut v = view();
    press(&mut v, KeyCode::Char('x'));
    press(&mut v, KeyCode::Right);
    press(&mut v, KeyCode::Char('x'));

    let action = press(&mut v, KeyCode::Char('r'));
    let ViewAction::Roam(walls) = action else {
        panic!("expected a roam, got {action:?}");
    };
    assert_eq!(walls.len(), 2);
    assert!(walls.contains(CROSSROADS));
}

/// An unfenced roam is legitimate — it means "this whole plane" — so
/// unlike `s`, which refuses an empty stop list, `r` must not.
#[test]
fn r_with_no_walls_is_allowed() {
    let mut v = view();
    let action = press(&mut v, KeyCode::Char('r'));
    let ViewAction::Roam(walls) = action else {
        panic!("expected a roam, got {action:?}");
    };
    assert!(walls.is_empty());
}
