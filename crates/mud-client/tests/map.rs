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
    let entrance = s.get(&SLUM_ENTRANCE).expect("styled");
    assert_eq!(
        entrance.sgr, "1;35",
        "the slum entrance spawns guardsmen, which initiate"
    );

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
