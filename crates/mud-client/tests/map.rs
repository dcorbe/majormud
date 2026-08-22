//! Map layout and render tests.
//!
//! Geometry is derived from the exit graph, not drawn: `re/slum_map.py`
//! proved the approach by closing 160 slum rooms with zero coordinate
//! conflicts. These tests hold the rule to that standard across the whole
//! shipped world and pin the numbers the design was sized against.

use mud_client::graph::RoomGraph;
use mud_client::map::{Marks, Paint, PaintCtx, Plane, Zoom, layout, render, styles};
use mud_client::spawn::SpawnTable;
use mud_core::content::{Direction, RoomId};

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn graph() -> &'static RoomGraph {
    use std::sync::OnceLock;
    static G: OnceLock<RoomGraph> = OnceLock::new();
    G.get_or_init(|| RoomGraph::load(&db_path()).expect("load room graph"))
}

fn spawns() -> &'static SpawnTable {
    use std::sync::OnceLock;
    static S: OnceLock<SpawnTable> = OnceLock::new();
    S.get_or_init(|| SpawnTable::load(&db_path()).expect("load spawn table"))
}

const SLUM_ENTRANCE: RoomId = RoomId { map: 1, room: 1072 };
const SMALL_CAVERN: RoomId = RoomId { map: 1, room: 2156 };
const TOWN_GATES: RoomId = RoomId { map: 1, room: 1 };

fn slums() -> Plane {
    layout(graph(), SLUM_ENTRANCE)
}

#[test]
fn a_plane_places_its_anchor_at_the_origin() {
    let p = slums();
    assert_eq!(p.cell_of(SLUM_ENTRANCE), Some((0, 0)));
    assert_eq!(p.room_at((0, 0)), Some(SLUM_ENTRANCE));
}

#[test]
fn a_compass_exit_steps_exactly_one_cell() {
    let p = slums();
    let here = graph().room(SLUM_ENTRANCE).expect("anchor");
    for (dir, delta) in [
        (Direction::North, (0, -1)),
        (Direction::South, (0, 1)),
        (Direction::East, (1, 0)),
        (Direction::West, (-1, 0)),
    ] {
        let Some(edge) = here.exits[dir as usize].as_ref() else {
            continue;
        };
        let Some(cell) = p.cell_of(edge.dest) else {
            continue; // conflicted or off-plane; other tests cover those
        };
        assert_eq!(cell, delta, "{dir:?} from the anchor");
    }
}

/// The measurement the whole scrolling design rests on: the biggest thing
/// a viewport ever has to scroll over. If a decode change moves this, the
/// zoom levels were sized against the wrong number.
///
/// Deliberately not asserting a plane COUNT. Exits are directed, so
/// "reachable from here" is not an equivalence relation and the sweep
/// below is not a partition — the number of planes it visits depends on
/// which room it starts from. The extremes are stable and are what the
/// design was sized against; the tally is not, so it is not a fact worth
/// pinning.
#[test]
fn the_largest_plane_is_small_enough_to_lay_out_whole() {
    let g = graph();
    let mut seen: std::collections::BTreeSet<RoomId> = Default::default();
    let mut largest = 0usize;
    let mut extent_of_largest = (0i32, 0i32);
    let mut widest = 0i32;
    let mut tallest = 0i32;
    for (id, _) in g.iter() {
        if seen.contains(&id) {
            continue;
        }
        let p = layout(g, id);
        seen.extend(p.rooms());
        widest = widest.max(p.extent().width());
        tallest = tallest.max(p.extent().height());
        if p.len() > largest {
            largest = p.len();
            extent_of_largest = (p.extent().width(), p.extent().height());
        }
    }
    assert_eq!(largest, 2420, "rooms in the largest plane");
    assert_eq!(extent_of_largest, (65, 118), "its extent in cells");
    // The bound the zoom table was sized against: overview draws one
    // character per cell, so the widest plane anywhere has to fit a wide
    // terminal.
    assert_eq!((widest, tallest), (160, 157), "the widest plane anywhere");
    assert!(widest <= 200, "overview must fit a 200-column terminal");
}

/// Conflicts are uncommon but not rare, and the honest figure is worth
/// pinning: the largest plane clashes on 145 of its 2,420 rooms, about
/// 6%. A grid layout of a world that was never built on a grid cannot do
/// better, which is exactly why the unplaced rooms are reported rather
/// than drawn on top of their neighbours.
#[test]
fn the_conflict_rate_is_what_the_design_assumed() {
    let p = layout(graph(), RoomId { map: 17, room: 1642 });
    assert_eq!(p.len(), 2420);
    assert_eq!(p.conflicts().len(), 145);
}

/// A cell can only hold one room. When the walk wants to put a second one
/// there the room is left unplaced and the clash is REPORTED — a map that
/// silently overlapped would be a map that lies.
#[test]
fn conflicts_are_reported_not_swallowed() {
    let p = slums();
    assert!(
        !p.conflicts().is_empty(),
        "the slum plane is known to clash"
    );
    for c in p.conflicts() {
        assert!(
            p.cell_of(c.from).is_some(),
            "a conflict is reported from a room that WAS placed"
        );
    }
    // Every placed room owns its cell alone.
    let mut cells: std::collections::BTreeSet<(i32, i32)> = Default::default();
    for room in p.rooms() {
        let cell = p.cell_of(room).expect("placed");
        assert!(cells.insert(cell), "two rooms at {cell:?}");
    }
}

/// Up and down are not compass directions; they leave the plane. This is
/// why Small Cavern's region looks tiny from inside it.
#[test]
fn a_vertical_exit_consumes_no_grid_cell_but_is_offered_as_a_link() {
    let p = layout(graph(), SMALL_CAVERN);
    for link in p.links() {
        assert!(
            !matches!(
                link.dir,
                Direction::North
                    | Direction::South
                    | Direction::East
                    | Direction::West
                    | Direction::NorthEast
                    | Direction::NorthWest
                    | Direction::SouthEast
                    | Direction::SouthWest
            ) || link.dest.map != link.from.map,
            "a same-map compass exit belongs in the grid, not in links"
        );
        assert!(p.cell_of(link.from).is_some(), "links hang off placed rooms");
        assert_eq!(p.cell_of(link.dest), None, "and lead off the plane");
    }
}

#[test]
fn a_plane_never_crosses_into_another_map() {
    let p = layout(graph(), TOWN_GATES);
    for room in p.rooms() {
        assert_eq!(room.map, 1);
    }
}

// --- render ----------------------------------------------------------

fn painted(p: &Plane, paint: Paint) -> std::collections::BTreeMap<RoomId, mud_client::map::Style> {
    styles(p, graph(), spawns(), paint, &PaintCtx::default())
}

#[test]
fn a_render_never_exceeds_the_terminal_it_was_given() {
    let p = slums();
    let s = painted(&p, Paint::Terrain);
    for zoom in [Zoom::Detail, Zoom::Normal, Zoom::Overview] {
        for (cols, rows) in [(80usize, 24usize), (200, 50), (20, 5)] {
            let out = render(&p, &s, (0, 0), (cols, rows), zoom, &Marks::default());
            assert!(out.len() <= rows, "{zoom:?} painted {} rows", out.len());
            for line in &out {
                let printed = mud_client::map::visible_width(line);
                assert!(
                    printed <= cols,
                    "{zoom:?} painted {printed} columns into {cols}: {line:?}"
                );
            }
        }
    }
}

/// Scrolling past the edge is an ordinary thing to do with an arrow key.
#[test]
fn a_viewport_off_the_edge_clips_instead_of_panicking() {
    let p = slums();
    let s = painted(&p, Paint::Terrain);
    for view in [(-500, -500), (500, 500), (0, -3)] {
        let out = render(&p, &s, view, (80, 24), Zoom::Normal, &Marks::default());
        assert!(out.len() <= 24);
    }
}

#[test]
fn the_anchor_is_drawn_where_the_layout_put_it() {
    let p = slums();
    let s = painted(&p, Paint::Terrain);
    let marks = Marks {
        here: Some(SLUM_ENTRANCE),
        ..Default::default()
    };
    // Origin cell, overview zoom: one character per room, so the anchor
    // is the first character of the first row.
    let out = render(&p, &s, (0, 0), (10, 4), Zoom::Overview, &marks);
    let first = mud_client::map::strip_sgr(&out[0]);
    assert_eq!(first.chars().next(), Some('@'));
}

// --- paint -----------------------------------------------------------

/// The board paints an aggressive monster in bright magenta and the
/// client already reads that (`bot::AGGRESSIVE`). The map says the same
/// thing in the same colour.
#[test]
fn danger_paint_uses_the_boards_own_magenta() {
    let p = slums();
    let s = painted(&p, Paint::Danger);
    // Guardsmen are behaviour mode 4 and never initiate, so the slum
    // entrance is a target, not a threat.
    assert_eq!(s.get(&SLUM_ENTRANCE).expect("styled").sgr, "0;36");

    // A cave bear is mode 1 and comes for you.
    let cavern = layout(graph(), SMALL_CAVERN);
    let s = styles(&cavern, graph(), spawns(), Paint::Danger, &PaintCtx::default());
    assert_eq!(s.get(&SMALL_CAVERN).expect("styled").sgr, "1;35");

    let gates = layout(graph(), TOWN_GATES);
    let s = styles(&gates, graph(), spawns(), Paint::Danger, &PaintCtx::default());
    assert_ne!(
        s.get(&TOWN_GATES).expect("styled").sgr,
        "1;35",
        "nothing spawns at Town Gates"
    );
}

/// Red is reserved: it means "you will have trouble here" and nothing
/// else, in every paint mode.
///
/// The trigger is hitpoints against the character's own maximum, NOT the
/// spawn band. `minindex`/`maxindex` and the monster `index` they select
/// on are a within-region ordinal, not a difficulty scale — the values
/// run to 666, 999 and 9999, and experience at every index spans the
/// whole 0-65,000 range. A warning built on them would be decoration.
#[test]
fn the_warning_overlay_takes_the_foreground_in_every_mode() {
    let p = slums();
    // A guardsman has 200 hp. A character with 30 loses that fight.
    let ctx = PaintCtx {
        max_hp: 30,
        ..Default::default()
    };
    for paint in [Paint::Terrain, Paint::Danger, Paint::Worth] {
        let s = styles(&p, graph(), spawns(), paint, &ctx);
        assert_eq!(
            s.get(&SLUM_ENTRANCE).expect("styled").sgr,
            "1;31",
            "{paint:?} must not outrank the warning"
        );
    }
}

#[test]
fn a_character_who_outclasses_the_spawn_is_not_warned() {
    let p = slums();
    let ctx = PaintCtx {
        max_hp: 5000,
        ..Default::default()
    };
    let s = styles(&p, graph(), spawns(), Paint::Terrain, &ctx);
    assert_ne!(s.get(&SLUM_ENTRANCE).expect("styled").sgr, "1;31");
}

/// The exp threshold is the operator's own line in the sand, for the
/// case hitpoints do not catch: something soft that hits very hard.
#[test]
fn an_exp_ceiling_warns_independently_of_hitpoints() {
    let p = slums();
    let ctx = PaintCtx {
        max_hp: 5000,
        warn_above_exp: 50,
        ..Default::default()
    };
    let s = styles(&p, graph(), spawns(), Paint::Terrain, &ctx);
    assert_eq!(
        s.get(&SLUM_ENTRANCE).expect("styled").sgr,
        "1;31",
        "a 75-exp guardsman is over a 50 ceiling"
    );
}

/// An unknown maximum must not paint the whole world red.
#[test]
fn nothing_is_warned_about_when_the_character_is_unknown() {
    let p = slums();
    let s = styles(&p, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    for room in p.rooms() {
        assert_ne!(s.get(&room).expect("styled").sgr, "1;31");
    }
}

/// Without a light source in hand, a dark room is a place you cannot
/// fight — the same trouble a too-high band is.
#[test]
fn a_dark_room_warns_only_when_nothing_can_light_it() {
    let p = layout(graph(), SMALL_CAVERN);
    let unlit = PaintCtx {
        no_light_source: true,
        ..Default::default()
    };
    let lit = PaintCtx::default();
    let dark = styles(&p, graph(), spawns(), Paint::Terrain, &unlit);
    assert_eq!(dark.get(&SMALL_CAVERN).expect("styled").sgr, "1;31");
    let ok = styles(&p, graph(), spawns(), Paint::Terrain, &lit);
    assert_ne!(ok.get(&SMALL_CAVERN).expect("styled").sgr, "1;31");
}

// --- connectors must not lie, 2026-08-02 ------------------------------

/// A hub with a neighbour in each of the given directions.
fn spokes(dirs: &[Direction]) -> Plane {
    use mud_client::graph::{ExitEdge, GraphRoom};
    let centre = RoomId { map: 1, room: 100 };
    let mut hub = GraphRoom {
        name: "Hub".into(),
        ..Default::default()
    };
    let mut rooms = Vec::new();
    for (n, dir) in dirs.iter().enumerate() {
        let id = RoomId {
            map: 1,
            room: n as u16 + 1,
        };
        hub.exits[*dir as usize] = Some(ExitEdge {
            dest: id,
            exit_type: 0,
            command: None,
        });
        rooms.push((
            id,
            GraphRoom {
                name: format!("Spoke {n}"),
                ..Default::default()
            },
        ));
    }
    rooms.push((centre, hub));
    layout(&RoomGraph::from_rooms(rooms), centre)
}

fn drawn(plane: &Plane, zoom: Zoom) -> String {
    let styles = styles(plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    render(plane, &styles, (-3, -3), (40, 20), zoom, &Marks::default())
        .iter()
        .map(|l| mud_client::map::strip_sgr(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every compass link has to be visible at the zooms that draw
/// connectors.
///
/// Normal used to be 2x1 — one row per cell — which left no row BETWEEN
/// rows for a vertical to occupy, so north/south and all four diagonals
/// were silently undrawable and every room appeared joined east-west
/// only. A map that shows the wrong connections is worse than one that
/// shows none (reported live, 2026-08-02).
#[test]
fn every_compass_direction_is_drawn_at_the_connector_zooms() {
    for zoom in [Zoom::Detail, Zoom::Normal] {
        for (dirs, glyph, what) in [
            (vec![Direction::East], '\u{2500}', "east"),
            (vec![Direction::West], '\u{2500}', "west"),
            (vec![Direction::North], '\u{2502}', "north"),
            (vec![Direction::South], '\u{2502}', "south"),
            (vec![Direction::SouthEast], '\u{2572}', "south-east"),
            (vec![Direction::NorthWest], '\u{2572}', "north-west"),
            (vec![Direction::NorthEast], '\u{2571}', "north-east"),
            (vec![Direction::SouthWest], '\u{2571}', "south-west"),
        ] {
            let frame = drawn(&spokes(&dirs), zoom);
            assert!(
                frame.contains(glyph),
                "{zoom:?} drew no {what} connector:\n{frame}"
            );
        }
    }
}

/// Both diagonals of a square share its centre character, so one used to
/// overwrite the other and was invisible everywhere. Crossed is the
/// honest answer — for a crossing that is REAL.
///
/// This test used to build a hub whose neighbours declared no exits at
/// all and then require a crossing to appear. The only thing that could
/// produce one was the adjacency bug, so the test asserted the defect it
/// was meant to guard. Both diagonals are now declared outright.
#[test]
fn two_real_diagonals_through_one_cell_are_drawn_crossed() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    // A(0,0) B(1,0) C(0,1) D(1,1). A goes south-east to D; B goes
    // south-west to C. The two links genuinely cross at the centre.
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let c = RoomId { map: 1, room: 3 };
    let d = RoomId { map: 1, room: 4 };
    let mk = |name: &str, exits: Vec<(Direction, RoomId)>| {
        let mut r = GraphRoom {
            name: name.into(),
            ..Default::default()
        };
        for (dir, dest) in exits {
            r.exits[dir as usize] = Some(ExitEdge {
                dest,
                exit_type: 0,
                command: None,
            });
        }
        r
    };
    let rooms = vec![
        (
            a,
            mk(
                "A",
                vec![(Direction::East, b), (Direction::SouthEast, d)],
            ),
        ),
        (b, mk("B", vec![(Direction::SouthWest, c)])),
        (c, mk("C", vec![])),
        (d, mk("D", vec![])),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);
    for zoom in [Zoom::Detail, Zoom::Normal] {
        let frame = drawn(&plane, zoom);
        assert!(
            frame.contains('\u{2573}'),
            "{zoom:?} lost one of two real crossing diagonals:\n{frame}"
        );
    }
}

/// Overview is one character per room and draws no connectors at all.
/// That is honest — it says nothing rather than something false.
#[test]
fn overview_draws_no_connectors_rather_than_misleading_ones() {
    let frame = drawn(
        &spokes(&[
            Direction::North,
            Direction::East,
            Direction::SouthEast,
            Direction::NorthWest,
        ]),
        Zoom::Overview,
    );
    for glyph in ['\u{2500}', '\u{2502}', '\u{2572}', '\u{2571}', '\u{2573}'] {
        assert!(!frame.contains(glyph), "overview drew a connector:\n{frame}");
    }
}

/// The cursor has to be visible even standing on nothing.
///
/// It used to be painted only as part of drawing a ROOM, and empty cells
/// are skipped entirely — so cursoring off the rooms made the cursor
/// disappear and there was no way to tell where it had got to. Moving
/// across a gap between two streets is an ordinary thing to do
/// (reported live, 2026-08-02).
#[test]
fn the_cursor_is_visible_on_an_empty_cell() {
    let plane = spokes(&[Direction::East]);
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    // (0, -5) is well clear of the two rooms at (0,0) and (1,0).
    let empty = (0, -5);
    assert_eq!(plane.room_at(empty), None, "the test cell must be empty");
    let marks = Marks {
        cursor: Some(empty),
        ..Default::default()
    };
    let out = render(&plane, &styles, (-2, -7), (30, 16), Zoom::Normal, &marks);
    let reversed = out.iter().filter(|l| l.contains(";7m") || l.contains("[7m")).count();
    assert_eq!(reversed, 1, "exactly one row carries the cursor:\n{out:#?}");

    // And it is where the cursor actually is, not merely somewhere.
    let (cw, ch) = Zoom::Normal.cell();
    let row = ((empty.1 - -7) as usize) * ch;
    let col = ((empty.0 - -2) as usize) * cw;
    assert!(
        out[row].contains(";7m") || out[row].contains("[7m"),
        "row {row} should hold the cursor: {:?}",
        out[row]
    );
    assert_eq!(
        mud_client::map::strip_sgr(&out[row]).chars().count(),
        col + 1,
        "the cursor block is the last painted character on its row"
    );
}

/// On a room it still reverses the room, as it always did.
#[test]
fn the_cursor_still_marks_a_room_it_stands_on() {
    let plane = spokes(&[Direction::East]);
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    let marks = Marks {
        cursor: Some((0, 0)),
        ..Default::default()
    };
    let out = render(&plane, &styles, (-2, -2), (30, 12), Zoom::Normal, &marks);
    assert_eq!(
        out.iter().filter(|l| l.contains(";7m") || l.contains("[7m")).count(),
        1
    );
}

/// "You are here" is a bright foreground on the shell's own background,
/// not white-on-green. `@` is already a unique glyph, so it needs no
/// block of colour behind it to be found — and a filled background makes
/// the one cell you look at most the ugliest thing on the screen.
#[test]
fn the_you_are_here_marker_is_a_foreground_not_a_background() {
    let plane = spokes(&[Direction::East]);
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    let here = plane.room_at((0, 0)).expect("the hub");
    let marks = Marks {
        here: Some(here),
        ..Default::default()
    };
    let out = render(&plane, &styles, (-2, -2), (30, 12), Zoom::Normal, &marks);
    let row = out
        .iter()
        .find(|l| mud_client::map::strip_sgr(l).contains('@'))
        .expect("the marker is drawn");
    assert!(row.contains("1;32"), "bright green foreground: {row:?}");
    for bg in ["40", "41", "42", "43", "44", "45", "46", "47"] {
        assert!(
            !row.contains(&format!(";{bg}m")) && !row.contains(&format!("[{bg}m")),
            "no background colour behind the marker ({bg}): {row:?}"
        );
    }
}

/// Standing in a room the palette would otherwise warn about must not
/// hide you. You already know you are there; the red is for rooms you
/// might walk into.
#[test]
fn the_marker_outranks_even_the_warning() {
    let plane = layout(graph(), SMALL_CAVERN);
    let ctx = PaintCtx {
        no_light_source: true,
        ..Default::default()
    };
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &ctx);
    assert_eq!(
        styles.get(&SMALL_CAVERN).expect("styled").sgr,
        "1;31",
        "the room itself is a warning"
    );
    let marks = Marks {
        here: Some(SMALL_CAVERN),
        ..Default::default()
    };
    let out = render(&plane, &styles, (-2, -2), (30, 12), Zoom::Normal, &marks);
    let row = out
        .iter()
        .find(|l| mud_client::map::strip_sgr(l).contains('@'))
        .expect("the marker is drawn");
    assert!(row.contains("1;32"), "still green: {row:?}");
}

/// The three danger states must not be shades of one another. Passive is
/// good news — something to farm that leaves you alone — and a dimmer
/// magenta read as "slightly less dangerous" instead.
#[test]
fn the_danger_states_are_told_apart_by_hue_not_brightness() {
    let cavern = layout(graph(), SMALL_CAVERN);
    let s = styles(&cavern, graph(), spawns(), Paint::Danger, &PaintCtx::default());
    let aggressive = s.get(&SMALL_CAVERN).expect("cave bear").sgr;
    assert_eq!(aggressive, "1;35");

    // A shop with a harmless resident and no spawns is plain, not magenta.
    let shop = layout(graph(), RoomId { map: 1, room: 159 });
    let s = styles(&shop, graph(), spawns(), Paint::Danger, &PaintCtx::default());
    let quiet = s.get(&RoomId { map: 1, room: 159 }).expect("Jael's").sgr;
    assert_eq!(quiet, "0;37", "a shopkeeper is not a danger");

    // And passive, where it occurs, shares no hue with aggressive.
    let passive = mud_client::map::PASSIVE_SGR;
    assert_ne!(passive, aggressive);
    assert!(
        !passive.ends_with("35"),
        "passive must not be a shade of the aggressive hue: {passive}"
    );
}

/// The room you are standing in is the one you mark first, and it was
/// the one room that could not show it.
///
/// Fixed by removing backgrounds altogether: one colour channel, one
/// precedence order, and `@` simply turns gold when it is part of the
/// loop.
#[test]
fn the_marker_turns_gold_when_you_stand_on_a_stop() {
    let plane = spokes(&[Direction::East]);
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    let here = plane.room_at((0, 0)).expect("the hub");
    let marks = Marks {
        here: Some(here),
        stops: [here].into_iter().collect(),
        ..Default::default()
    };
    let out = render(&plane, &styles, (-2, -2), (30, 12), Zoom::Normal, &marks);
    let row = out
        .iter()
        .find(|l| mud_client::map::strip_sgr(l).contains('@'))
        .expect("the marker is drawn");
    assert!(row.contains("1;33"), "bright gold once marked: {row:?}");
    assert!(!row.contains("1;32"), "not green any more: {row:?}");
}

/// Same for a route leg running under your feet.
#[test]
fn the_marker_turns_gold_on_the_route_too() {
    let plane = spokes(&[Direction::East]);
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    let here = plane.room_at((0, 0)).expect("the hub");
    let marks = Marks {
        here: Some(here),
        route: [here].into_iter().collect(),
        ..Default::default()
    };
    let out = render(&plane, &styles, (-2, -2), (30, 12), Zoom::Normal, &marks);
    let row = out
        .iter()
        .find(|l| mud_client::map::strip_sgr(l).contains('@'))
        .expect("the marker is drawn");
    assert!(row.contains("0;33"), "dim gold on a pass-through room: {row:?}");
}

/// Reverse video paints the cell in its FOREGROUND colour, so a cursor
/// on an unlit room — bright black, which is the terminal's own
/// background wearing a different name — reversed into a black block on
/// a black screen and could not be seen at all.
///
/// Not a corner: 17,432 of the 26,720 shipped rooms are unlit, so this
/// was most of the map (reported live, 2026-08-02).
#[test]
fn the_cursor_is_visible_on_an_unlit_room() {
    let plane = layout(graph(), SMALL_CAVERN);
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    assert_eq!(
        styles.get(&SMALL_CAVERN).expect("styled").sgr,
        "1;30",
        "the test needs a room the palette greys out"
    );

    let (cw, ch) = Zoom::Normal.cell();
    let view = (-2, -2);
    let marks = Marks {
        cursor: Some((0, 0)),
        ..Default::default()
    };
    let out = render(&plane, &styles, view, (30, 12), Zoom::Normal, &marks);
    let at = sgr_at_column(&out[2 * ch], 2 * cw);
    assert!(at.contains("7"), "the cursor still reverses: {at:?}");
    assert!(
        !at.contains("1;30"),
        "reversed into the background and vanished: {at:?}"
    );
}

/// Only the cursor's own cell is lifted. An unlit room the cursor is not
/// standing on stays grey — the palette is how you read the map, and
/// brightening the whole terrain to suit one marker would throw away
/// what the grey says.
#[test]
fn an_unlit_room_without_the_cursor_stays_grey() {
    let plane = slums();
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    let mut grey = plane
        .rooms()
        .filter(|id| styles.get(id).is_some_and(|s| s.sgr == "1;30"))
        .filter_map(|id| plane.cell_of(id))
        .filter(|(x, y)| (0..10).contains(x) && (-6..0).contains(y));
    let under = grey.next().expect("an unlit room to stand the cursor on");
    let elsewhere = grey.next().expect("and another one to leave alone");

    let (cw, ch) = Zoom::Normal.cell();
    let view = (0, -6);
    let marks = Marks {
        cursor: Some(under),
        ..Default::default()
    };
    let out = render(&plane, &styles, view, (120, 60), Zoom::Normal, &marks);
    let row = ((elsewhere.1 - view.1) as usize) * ch;
    let col = ((elsewhere.0 - view.0) as usize) * cw;
    assert_eq!(sgr_at_column(&out[row], col), "0;1;30");
}

/// No background colours anywhere in the palette. They were the source
/// of the composition rules that kept going wrong; the cursor's reverse
/// is not one, since it inverts whatever colour is already there.
#[test]
fn the_palette_paints_no_backgrounds() {
    let plane = spokes(&[Direction::East, Direction::North]);
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    let here = plane.room_at((0, 0)).expect("hub");
    let marks = Marks {
        here: Some(here),
        cursor: Some((0, 0)),
        stops: [here].into_iter().collect(),
        route: plane.rooms().collect(),
        walls: Default::default(),
    };
    for line in render(&plane, &styles, (-2, -2), (30, 12), Zoom::Normal, &marks) {
        for code in line.split('\u{1b}').skip(1) {
            let params = code.trim_start_matches('[').split('m').next().unwrap_or("");
            for p in params.split(';') {
                let n: u32 = match p.parse() {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                assert!(
                    !(40..=47).contains(&n) && !(100..=107).contains(&n),
                    "background {n} in {line:?}"
                );
            }
        }
    }
}

/// The colour in effect at a given printable column of a rendered line.
fn sgr_at_column(line: &str, want: usize) -> String {
    let mut current = String::new();
    let mut col = 0usize;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() != Some('[') {
                continue;
            }
            let mut code = String::new();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
                code.push(c);
            }
            current = code;
            continue;
        }
        if col == want {
            return current;
        }
        col += 1;
    }
    current
}

/// A stop is somewhere the runner stands and fights; a pass-through room
/// is somewhere it merely walks. Same hue, because it is one loop and the
/// route should read as one shape — different brightness, because that
/// distinction is what you are looking for when you check a circuit.
#[test]
fn a_stop_and_a_pass_through_room_are_told_apart() {
    let plane = slums();
    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    let a = RoomId { map: 1, room: 1076 };
    let b = RoomId { map: 1, room: 1122 };
    let route = mud_client::loops::route_rooms(graph(), &[a, b]);
    let between = *route
        .iter()
        .find(|r| **r != a && **r != b)
        .expect("the legs pass through something");

    let marks = Marks {
        stops: [a, b].into_iter().collect(),
        route,
        ..Default::default()
    };
    let (cw, ch) = Zoom::Normal.cell();
    let view = (-4, -4);
    let out = render(&plane, &styles, view, (120, 60), Zoom::Normal, &marks);
    let colour = |room: RoomId| {
        let (x, y) = plane.cell_of(room).expect("placed");
        let row = ((y - view.1) as usize) * ch;
        let col = ((x - view.0) as usize) * cw;
        sgr_at_column(&out[row], col)
    };
    assert_eq!(colour(a), "0;1;33", "a stop is bright gold");
    assert_eq!(colour(b), "0;1;33", "so is the other one");
    assert_eq!(colour(between), "0;0;33", "a pass-through room is dim");
}

// --- the plane records real exits, not grid adjacency -----------------

/// `A --east--> B` and `A --southeast--> D`. That places B at (1,0) and D
/// at (1,1), which makes B and D vertically adjacent on the grid -- with
/// nothing at all joining them. The plane must know the difference.
#[test]
fn the_plane_records_exits_and_not_adjacency() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let d = RoomId { map: 1, room: 4 };
    let mut ra = GraphRoom {
        name: "A".into(),
        ..Default::default()
    };
    ra.exits[Direction::East as usize] = Some(ExitEdge {
        dest: b,
        exit_type: 0,
        command: None,
    });
    ra.exits[Direction::SouthEast as usize] = Some(ExitEdge {
        dest: d,
        exit_type: 0,
        command: None,
    });
    let rooms = vec![
        (a, ra),
        (
            b,
            GraphRoom {
                name: "B".into(),
                ..Default::default()
            },
        ),
        (
            d,
            GraphRoom {
                name: "D".into(),
                ..Default::default()
            },
        ),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);

    assert!(plane.has_edge(a, Direction::East), "A really does go east");
    assert!(
        plane.has_edge(a, Direction::SouthEast),
        "A really does go south-east"
    );

    // B is at (1,0) and D at (1,1) -- adjacent, and unconnected.
    assert_eq!(plane.cell_of(b), Some((1, 0)));
    assert_eq!(plane.cell_of(d), Some((1, 1)));
    assert!(
        !plane.has_edge(b, Direction::South),
        "nothing joins B to D; adjacency is not an exit"
    );
    assert!(
        !plane.has_edge(d, Direction::North),
        "and nothing joins D back to B"
    );
}

/// Four rooms in a square, joined ONLY by orthogonal exits. Not one
/// diagonal exists, so not one diagonal may be drawn — and the crossed
/// glyph, which is two diagonals, least of all.
#[test]
fn a_square_loop_with_no_diagonal_exits_draws_no_diagonal() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let c = RoomId { map: 1, room: 3 };
    let d = RoomId { map: 1, room: 4 };
    let mk = |name: &str, exits: Vec<(Direction, RoomId)>| {
        let mut r = GraphRoom {
            name: name.into(),
            ..Default::default()
        };
        for (dir, dest) in exits {
            r.exits[dir as usize] = Some(ExitEdge {
                dest,
                exit_type: 0,
                command: None,
            });
        }
        r
    };
    let rooms = vec![
        (a, mk("A", vec![(Direction::East, b), (Direction::South, c)])),
        (b, mk("B", vec![(Direction::West, a), (Direction::South, d)])),
        (c, mk("C", vec![(Direction::East, d), (Direction::North, a)])),
        (d, mk("D", vec![(Direction::West, c), (Direction::North, b)])),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);
    for zoom in [Zoom::Detail, Zoom::Normal] {
        let frame = drawn(&plane, zoom);
        for glyph in ['\u{2573}', '\u{2572}', '\u{2571}'] {
            assert!(
                !frame.contains(glyph),
                "{zoom:?} drew {glyph:?} for a square with no diagonal exits:\n{frame}"
            );
        }
    }
}

/// Two rooms the grid puts side by side with nothing joining them get no
/// line. `A --east--> B` and `A --southeast--> D` leave B and D
/// vertically adjacent and unconnected.
#[test]
fn adjacent_rooms_with_no_exit_between_them_are_not_joined() {
    use mud_client::graph::{ExitEdge, GraphRoom};
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let d = RoomId { map: 1, room: 4 };
    let mut ra = GraphRoom {
        name: "A".into(),
        ..Default::default()
    };
    ra.exits[Direction::East as usize] = Some(ExitEdge {
        dest: b,
        exit_type: 0,
        command: None,
    });
    ra.exits[Direction::SouthEast as usize] = Some(ExitEdge {
        dest: d,
        exit_type: 0,
        command: None,
    });
    let rooms = vec![
        (a, ra),
        (
            b,
            GraphRoom {
                name: "B".into(),
                ..Default::default()
            },
        ),
        (
            d,
            GraphRoom {
                name: "D".into(),
                ..Default::default()
            },
        ),
    ];
    let plane = layout(&RoomGraph::from_rooms(rooms), a);
    let frame = drawn(&plane, Zoom::Normal);
    // Exactly one horizontal (A-B) and one diagonal (A-D). The vertical
    // that used to appear between B and D was never an exit.
    assert_eq!(
        frame.matches('\u{2502}').count(),
        0,
        "a vertical was drawn where no exit exists:\n{frame}"
    );
    assert_eq!(frame.matches('\u{2500}').count(), 1, "A-B:\n{frame}");
    assert_eq!(frame.matches('\u{2572}').count(), 1, "A-D:\n{frame}");
}

/// The pin: over the shipped world, every edge the PLANE RECORDED is an
/// exit that exists.
///
/// This is a layout-layer census. It reads `Plane::has_edge` and never
/// renders, so it cannot see a `connectors()` regression -- the render
/// path is pinned separately by
/// `the_reported_room_draws_only_its_real_exits`. Do not read this test
/// as covering what is drawn; an earlier version of this comment said it
/// did, and that claim is what let the wrong-layer gap hide.
///
/// What it does cover is the case no example test reaches: a conflicting
/// exit recorded as an edge. `layout()` places rooms by BFS and a
/// disagreeing destination must NOT be recorded, or the line would point
/// at the wrong room.
#[test]
fn no_connector_in_the_shipped_world_is_fabricated() {
    // `graph()` is the file's existing OnceLock helper and already
    // expects the fixture to be present, as `loads_all_rooms` does.
    // Follow that convention rather than inventing a second one.
    let g = graph();
    // Anchored at Newhaven, Narrow Road -- the plane the client shows on
    // a stock login. layout() is a BFS from the anchor, so the placed
    // set depends on it; the anchor is named so the numbers are
    // reproducible.
    let plane = layout(g, RoomId { map: 1, room: 2146 });
    assert!(plane.len() > 1000, "expected a large plane, got {}", plane.len());

    let mut fabricated = Vec::new();
    let mut drawn = 0usize;
    for room in plane.rooms() {
        for dir in mud_client::graph::DIRECTIONS {
            let Some(step) = mud_client::map::step_of(dir) else {
                continue; // up/down leave the plane
            };
            if !plane.has_edge(room, dir) {
                continue;
            }
            drawn += 1;
            let cell = plane.cell_of(room).expect("placed");
            let neighbour = plane.room_at((cell.0 + step.0, cell.1 + step.1));
            let real = g
                .room(room)
                .and_then(|r| {
                    mud_client::graph::DIRECTIONS
                        .iter()
                        .position(|d| *d == dir)
                        .and_then(|i| r.exits[i].as_ref())
                })
                .map(|e| Some(e.dest) == neighbour)
                .unwrap_or(false);
            if !real {
                fabricated.push((room, dir));
            }
        }
    }
    assert!(drawn > 3000, "expected thousands of connectors, got {drawn}");
    assert!(
        fabricated.is_empty(),
        "{} of {drawn} connectors are not backed by an exit; first few: {:?}",
        fabricated.len(),
        &fabricated[..fabricated.len().min(5)]
    );
}

/// The reported case: `1/837 Graveyard, East of Tomb` has exactly three
/// real exits -- north to 1/838, south to 1/836, east to 1/823, all
/// ordinary (type 0) -- and no west exit and no diagonals. Every one of
/// its eight neighbouring cells is nonetheless occupied by a real room
/// (835 to the west; 839/824/834/822 on the diagonals), because the
/// graveyard packs its rooms edge to edge. That is exactly the trap the
/// old adjacency-based `connectors()` fell into: it drew a line to
/// every OCCUPIED neighbour regardless of whether 837 itself had an
/// exit there, so the room came out boxed in on all eight sides. This
/// pins the render path (`connectors()`/`render()`) against the actual
/// reported room, which the has_edge-only census in
/// `no_connector_in_the_shipped_world_is_fabricated` does not exercise.
#[test]
fn the_reported_room_draws_only_its_real_exits() {
    let anchor = RoomId { map: 1, room: 837 };
    let plane = layout(graph(), anchor);
    assert_eq!(plane.cell_of(anchor), Some((0, 0)));
    // The premise the bug needs: every neighbour cell is occupied, so a
    // pure-adjacency renderer has all eight to draw a line to.
    for cell in [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ] {
        assert!(
            plane.room_at(cell).is_some(),
            "expected a real room at {cell:?} beside 1/837 -- if this fails \
             the graveyard's layout changed and the test no longer proves \
             what it claims"
        );
    }

    let styles = styles(&plane, graph(), spawns(), Paint::Terrain, &PaintCtx::default());
    for zoom in [Zoom::Detail, Zoom::Normal] {
        let (cw, ch) = zoom.cell();
        let (cw, ch) = (cw as i32, ch as i32);
        // A 3x3 block of cells with 1/837 dead centre, so every glyph
        // this test checks lands inside the rendered buffer.
        let view = (-1, -1);
        let size = ((3 * cw) as usize, (3 * ch) as usize);
        let lines = render(&plane, &styles, view, size, zoom, &Marks::default());
        let frame: Vec<Vec<char>> = lines
            .iter()
            .map(|l| mud_client::map::strip_sgr(l).chars().collect())
            .collect();
        let at = |col: i32, row: i32| -> char {
            frame
                .get(row as usize)
                .and_then(|r| r.get(col as usize))
                .copied()
                .unwrap_or(' ')
        };
        // 1/837's own cell, centred in the view.
        let (col, row) = (cw, ch);
        let half = cw / 2;

        assert_eq!(
            at(col + 1, row),
            '\u{2500}',
            "{zoom:?}: missing the real east connector to 1/823"
        );
        assert_eq!(
            at(col, row - 1),
            '\u{2502}',
            "{zoom:?}: missing the real north connector to 1/838"
        );
        assert_eq!(
            at(col, row + 1),
            '\u{2502}',
            "{zoom:?}: missing the real south connector to 1/836"
        );

        assert_eq!(
            at(col - 1, row),
            ' ',
            "{zoom:?}: fabricated a west connector toward 1/835, which 1/837 has no exit to"
        );
        assert_eq!(
            at(col + half, row - 1),
            ' ',
            "{zoom:?}: fabricated a north-east diagonal"
        );
        assert_eq!(
            at(col - half, row - 1),
            ' ',
            "{zoom:?}: fabricated a north-west diagonal"
        );
        assert_eq!(
            at(col + half, row + 1),
            ' ',
            "{zoom:?}: fabricated a south-east diagonal"
        );
        assert_eq!(
            at(col - half, row + 1),
            ' ',
            "{zoom:?}: fabricated a south-west diagonal"
        );
    }
}

