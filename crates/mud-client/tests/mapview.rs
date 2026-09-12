//! The interactive map's state machine, driven by keystrokes over a
//! hand-made world.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::lost::Fix;
use mud_client::map::PaintCtx;
use mud_client::mapview::MapView;
use mud_client::spawn::SpawnTable;
use mud_core::content::{Direction, RoomId};

fn room(name: &str, exits: &[(Direction, RoomId)]) -> GraphRoom {
    let mut r = GraphRoom {
        name: name.into(),
        ..Default::default()
    };
    for (dir, dest) in exits {
        r.exits[*dir as usize] = Some(ExitEdge {
            dest: *dest,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
    }
    r
}


const DOOR: RoomId = RoomId { map: 1, room: 1 };
const STREET: RoomId = RoomId { map: 1, room: 2 };
const ROAD: RoomId = RoomId { map: 2, room: 1 };
const LANDING: RoomId = RoomId { map: 1, room: 3 };

/// `1/2 Street --west--> 1/1 Door --west--> 2/1 Road`, the Door with a
/// stair up to `1/3 Landing`.
fn world() -> Arc<RoomGraph> {
    Arc::new(RoomGraph::from_rooms(vec![
        (
            DOOR,
            room(
                "Western Door",
                &[
                    (Direction::West, ROAD),
                    (Direction::East, STREET),
                    (Direction::Up, LANDING),
                ],
            ),
        ),
        (STREET, room("Slum Street", &[(Direction::West, DOOR)])),
        (ROAD, room("Western Road", &[(Direction::East, DOOR)])),
        (LANDING, room("Landing", &[(Direction::Down, DOOR)])),
    ]))
}

fn view(anchor: RoomId, here: Fix) -> MapView {
    MapView::new(
        world(),
        Arc::new(SpawnTable::default()),
        anchor,
        here,
        PaintCtx::default(),
        (100, 30),
    )
}

fn press(view: &mut MapView, code: KeyCode) {
    view.on_key(&KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn stepping_west_crosses_into_the_next_map() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('h'));
    assert_eq!(v.cursor_room(), Some(ROAD));
    assert_eq!(v.plane().anchor(), DOOR, "no hop was needed");
}

#[test]
fn home_re_anchors_once_the_character_has_walked_into_another_map() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    v.set_here(Fix::Confirmed(ROAD));
    press(&mut v, KeyCode::Home);
    assert_eq!(v.cursor_room(), Some(ROAD));
    assert_eq!(
        v.plane().anchor(),
        ROAD,
        "the character's map is the one drawn whole"
    );
}

#[test]
fn home_keeps_the_plane_while_the_character_stays_on_its_map() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    v.set_here(Fix::Confirmed(STREET));
    press(&mut v, KeyCode::Home);
    assert_eq!(v.cursor_room(), Some(STREET));
    assert_eq!(v.plane().anchor(), DOOR);
}

#[test]
fn the_panel_names_the_stair_and_counts_the_maps() {
    let v = view(DOOR, Fix::Confirmed(DOOR));
    let text = v.lines().join("\n");
    assert!(text.contains("up [<] 1/3 Landing"), "{text}");
    assert!(text.contains("plane 3 rooms in 2 maps, 3x1"), "{text}");
    assert!(!text.contains("that way"), "{text}");
}

use mud_client::mapview::ViewAction;

#[test]
fn shift_r_on_a_room_asks_to_recover_from_it() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('h'));
    assert_eq!(v.cursor_room(), Some(ROAD));
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
    assert_eq!(action, ViewAction::Recover(ROAD));
}

#[test]
fn shift_r_on_nothing_says_so_and_stays() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('k'));
    assert_eq!(v.cursor_room(), None, "the cell north of the door is empty");
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
    assert_eq!(action, ViewAction::Continue);
    assert_eq!(v.message(), Some("no room under the cursor"));
}

/// `S` and `D` mark the recovery's two rooms under the cursor. A second
/// press on the same room clears the mark, the marks read back for the
/// window to keep, and marks handed in paint on the map.
#[test]
fn shift_s_and_shift_d_mark_the_safe_and_death_rooms_and_again_clear_them() {
    use mud_client::recover::Marks;
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('h'));
    assert_eq!(v.cursor_room(), Some(ROAD));
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    assert_eq!(action, ViewAction::Continue);
    assert_eq!(v.recover_marks(), Marks { safe: Some(ROAD), death: None });
    let status = v.lines().pop().unwrap();
    assert!(status.contains("safe room: Western Road [2/1]"), "{status}");
    v.on_key(&KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
    assert_eq!(v.recover_marks(), Marks { safe: Some(ROAD), death: Some(ROAD) });
    v.on_key(&KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    assert_eq!(v.recover_marks(), Marks { safe: None, death: Some(ROAD) });
    let status = v.lines().pop().unwrap();
    assert!(status.contains("safe room cleared"), "{status}");

    let v = view(DOOR, Fix::Confirmed(DOOR)).with_recover_marks(Marks { safe: Some(STREET), death: Some(ROAD) });
    assert_eq!(v.recover_marks(), Marks { safe: Some(STREET), death: Some(ROAD) });
    let frame = v.lines().join("\n");
    assert!(frame.contains("safe room: Slum Street [1/2]"), "{frame}");
    assert!(frame.contains("death room: Western Road [2/1]"), "{frame}");
}

#[test]
fn shift_s_on_nothing_says_so_and_marks_nothing() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('k'));
    assert_eq!(v.cursor_room(), None);
    v.on_key(&KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    assert_eq!(v.recover_marks(), Default::default());
    let status = v.lines().pop().unwrap();
    assert!(status.contains("no room under the cursor"), "{status}");
}

#[test]
fn the_legend_names_the_recover_key() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    // The legend runs past the 100-column width `view` sets up, so the
    // status line clips before reaching this tail on that width. Widen
    // it here so the assertion looks at the whole legend.
    v.resize((200, 30));
    let status = v.lines().pop().unwrap();
    assert!(status.contains("S safe  D death  g go  R recover  q leave"), "{status}");
}


// ---------------------------------------------------------------------
// `g` with marks. The marks are waypoints: the walk visits them in
// order and finishes at the cursor room, so a route can be planned by
// hand around a place the router would otherwise choose.
// ---------------------------------------------------------------------

#[test]
fn g_without_marks_goes_straight_to_the_cursor_room() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('h'));
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(action, mud_client::mapview::ViewAction::Go(vec![ROAD]));
}

#[test]
fn g_with_marks_walks_them_in_order_then_the_cursor_room() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    // Mark the Landing, then the Street, then put the cursor on the Road.
    press(&mut v, KeyCode::Char('<'));
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('>'));
    press(&mut v, KeyCode::Char('l'));
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('h'));
    press(&mut v, KeyCode::Char('h'));
    assert_eq!(v.cursor_room(), Some(ROAD));
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(
        action,
        mud_client::mapview::ViewAction::Go(vec![LANDING, STREET, ROAD])
    );
}

#[test]
fn g_on_the_last_mark_does_not_walk_it_twice() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('<'));
    press(&mut v, KeyCode::Enter);
    press(&mut v, KeyCode::Char('>'));
    press(&mut v, KeyCode::Char('h'));
    press(&mut v, KeyCode::Enter);
    assert_eq!(v.cursor_room(), Some(ROAD));
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(action, mud_client::mapview::ViewAction::Go(vec![LANDING, ROAD]));
}

/// Wide enough a terminal to show the whole status line; the key list
/// is cut at the right edge on a narrow one.
#[test]
fn the_status_line_says_g_walks_via_the_marks() {
    let mut v = MapView::new(
        world(),
        Arc::new(SpawnTable::default()),
        DOOR,
        Fix::Confirmed(DOOR),
        PaintCtx::default(),
        (220, 30),
    );
    let status = |v: &MapView| v.lines().last().cloned().unwrap_or_default();
    assert!(status(&v).contains("g go "), "{}", status(&v));
    assert!(!status(&v).contains("g go via marks"), "{}", status(&v));
    press(&mut v, KeyCode::Enter);
    assert!(status(&v).contains("g go via marks"), "{}", status(&v));
}

// ---------------------------------------------------------------------
// Avoid marks: rooms no walk may enter, kept in the profile.
// ---------------------------------------------------------------------

#[test]
fn a_toggles_an_avoid_mark_under_the_cursor() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('a'));
    assert_eq!(v.avoid().iter().copied().collect::<Vec<_>>(), vec![DOOR]);
    press(&mut v, KeyCode::Char('a'));
    assert!(v.avoid().is_empty());
}

#[test]
fn avoid_marks_come_in_with_the_view_and_go_out_with_it() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR)).with_avoid([STREET].into_iter().collect());
    assert!(v.avoid().contains(&STREET));
    press(&mut v, KeyCode::Char('a'));
    let out: Vec<RoomId> = v.avoid().iter().copied().collect();
    assert_eq!(out, vec![DOOR, STREET]);
}

#[test]
fn g_refuses_a_destination_on_the_avoid_list() {
    let mut v = view(DOOR, Fix::Confirmed(DOOR));
    press(&mut v, KeyCode::Char('h'));
    press(&mut v, KeyCode::Char('a'));
    let action = v.on_key(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(action, mud_client::mapview::ViewAction::Continue);
    let status = v.lines().last().cloned().unwrap_or_default();
    assert!(status.contains("avoid"), "{status}");
}

#[test]
fn the_status_line_counts_avoid_marks() {
    let mut v = MapView::new(
        world(),
        Arc::new(SpawnTable::default()),
        DOOR,
        Fix::Confirmed(DOOR),
        PaintCtx::default(),
        (220, 30),
    );
    press(&mut v, KeyCode::Char('a'));
    let status = v.lines().last().cloned().unwrap_or_default();
    assert!(status.contains("1 avoid"), "{status}");
    assert!(status.contains("a avoid"), "{status}");
}
