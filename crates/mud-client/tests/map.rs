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
/// honest answer.
#[test]
fn two_diagonals_through_one_cell_are_drawn_crossed() {
    for zoom in [Zoom::Detail, Zoom::Normal] {
        let frame = drawn(&spokes(&[Direction::SouthEast, Direction::East]), zoom);
        // The hub's SE and the east neighbour's SW meet in one character.
        assert!(
            frame.contains('\u{2573}') || frame.contains('\u{2572}'),
            "{zoom:?}:\n{frame}"
        );
        let all = drawn(
            &spokes(&[
                Direction::North,
                Direction::South,
                Direction::East,
                Direction::West,
                Direction::NorthEast,
                Direction::NorthWest,
                Direction::SouthEast,
                Direction::SouthWest,
            ]),
            zoom,
        );
        assert!(all.contains('\u{2573}'), "{zoom:?} never crossed:\n{all}");
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
