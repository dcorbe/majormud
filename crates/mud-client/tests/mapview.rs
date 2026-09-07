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
