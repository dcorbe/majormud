//! Navigator tests: verified step-by-step movement against the
//! in-process mud-server. Every step is confirmed by parsing the
//! destination room name — never blind (monsters get free attacks on
//! movement; desyncs must surface immediately).

use std::sync::Arc;

use mud_client::dialect::{self, Target};
use mud_client::graph::{ExitEdge, GraphRoom, RoomGraph};
use mud_client::events::Event;
use mud_client::nav::{Interrupt, NavConfig, NavErrorKind, Navigator, NoGuard, TravelGuard};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::CoreConfig;
use mud_server::{server::Server, state_db::StateDb};

fn tiny_world() -> Content {
    let mut content = Content::default();
    let names = [
        (1u16, "Town Gates"),
        (2u16, "Town Square"),
        (3u16, "Market Street"),
    ];
    let mut rooms: Vec<Room> = names
        .iter()
        .map(|(n, name)| Room {
            id: RoomId { map: 1, room: *n },
            name: (*name).into(),
            ..Default::default()
        })
        .collect();
    // gates -N-> square -E-> market (and back)
    rooms[0].exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        ..Default::default()
    });
    rooms[1].exits[Direction::South as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 1 },
        ..Default::default()
    });
    rooms[1].exits[Direction::East as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 3 },
        ..Default::default()
    });
    rooms[2].exits[Direction::West as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        ..Default::default()
    });
    for r in rooms {
        content.add_room(r);
    }
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 40,
            wisdom: 40,
            strength: 40,
            health: 40,
            agility: 40,
            charm: 40,
        },
        max_stats: StatBlock {
            intellect: 100,
            wisdom: 100,
            strength: 100,
            health: 100,
            agility: 100,
            charm: 100,
        },
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
        abilities: vec![],
        hp_per_level: 6,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 6,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

/// Client-side graph mirroring (or deliberately mismatching) the world.
fn client_graph(market_name: &str) -> RoomGraph {
    let mk = |n: u16, name: &str, exits: Vec<(Direction, u16)>| {
        let mut room = GraphRoom {
            name: name.into(),
            exits: Default::default(),
        };
        for (d, dest) in exits {
            room.exits[d as usize] = Some(ExitEdge {
                dest: RoomId { map: 1, room: dest },
                exit_type: 0,
            });
        }
        (RoomId { map: 1, room: n }, room)
    };
    RoomGraph::from_rooms(vec![
        mk(1, "Town Gates", vec![(Direction::North, 2)]),
        mk(
            2,
            "Town Square",
            vec![(Direction::South, 1), (Direction::East, 3)],
        ),
        mk(3, market_name, vec![(Direction::West, 2)]),
    ])
}

async fn logged_in_session(addr: std::net::SocketAddr) -> Arc<Session> {
    let profile = Profile {
        target: Target::RustServer,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "Nav".into(),
        password: "pw".into(),
        pace_ms: Some(0),
        disable_evil_warnings: false,
        bot: None,
        farm: None,
    };
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());
    let outcome = dialect::login(&session, &profile).await.unwrap();
    assert_eq!(outcome, mud_client::dialect::LoginOutcome::CharacterCreation);
    dialect::finish_creation(&session).await.unwrap();
    session
}

async fn start() -> Server {
    let config = CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        exit_meditation_seconds: 1,
        ansi: true,
        ..CoreConfig::default()
    };
    Server::start(tiny_world(), config, StateDb::open_in_memory().unwrap(), "127.0.0.1:0")
        .await
        .unwrap()
}

#[tokio::test]
async fn goto_walks_verified_route() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());

    let at = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut NoGuard,
        )
        .await
        .expect("navigate gates -> market");

    assert_eq!(at, RoomId { map: 1, room: 3 });
    // The session's parsed state confirms where we ended up.
    let state = session.state().borrow().clone();
    assert_eq!(
        state.room.as_ref().map(|r| r.name.as_str()),
        Some("Market Street")
    );
}

#[tokio::test]
async fn goto_detects_desync_on_name_mismatch() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    // Client graph believes room 3 is called "Crystal Cavern"; the
    // server will print "Market Street" -> desync error, no blind walk.
    let nav = Navigator::new(Arc::new(client_graph("Crystal Cavern")), NavConfig::default());

    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut NoGuard,
        )
        .await
        .expect_err("must detect desync");
    match &err.kind {
        NavErrorKind::Desync { expected, saw } => {
            assert_eq!(expected, "Crystal Cavern");
            assert_eq!(saw, "Market Street");
        }
        other => panic!("expected Desync, got {other:?}"),
    }
    // The whole point of reporting a position: the caller has to know
    // where the character is standing to recover. Town Square is the
    // last room the walk actually confirmed — not the room it set off
    // from, and not the one it was aiming at.
    assert_eq!(err.at, RoomId { map: 1, room: 2 });
}

#[tokio::test]
async fn goto_without_route_fails_fast() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());
    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 9, room: 9 },
            &mut NoGuard,
        )
        .await
        .expect_err("no route");
    assert!(matches!(err.kind, NavErrorKind::NoRoute));
    // Nothing was walked, so the character is still at the start.
    assert_eq!(err.at, RoomId { map: 1, room: 1 });
}

/// A graph that believes in an exit the board does not have. Walking it
/// earns "There is no exit in that direction!" and no room block at all,
/// which is the shape of every step that never lands.
fn graph_with_a_phantom_exit() -> RoomGraph {
    let mut gates = GraphRoom {
        name: "Town Gates".into(),
        exits: Default::default(),
    };
    gates.exits[Direction::East as usize] = Some(ExitEdge {
        dest: RoomId { map: 1, room: 3 },
        exit_type: 0,
    });
    RoomGraph::from_rooms(vec![
        (RoomId { map: 1, room: 1 }, gates),
        (
            RoomId { map: 1, room: 3 },
            GraphRoom {
                name: "Market Street".into(),
                exits: Default::default(),
            },
        ),
    ])
}

/// A step that never lands must give up on the clock, not hang. The
/// deadline is configuration precisely so this is provable in
/// milliseconds instead of the fifteen seconds that is right for a
/// laggy live board.
#[tokio::test]
async fn a_step_that_never_lands_times_out_on_the_configured_deadline() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(
        Arc::new(graph_with_a_phantom_exit()),
        NavConfig {
            step_timeout_ms: 200,
        },
    );

    let started = std::time::Instant::now();
    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut NoGuard,
        )
        .await
        .expect_err("the board has no east exit here");

    assert!(matches!(
        err.kind,
        NavErrorKind::Expect(mud_client::session::ExpectError::Timeout { .. })
    ));
    assert_eq!(err.at, RoomId { map: 1, room: 1 });
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "gave up on the hardcoded 15s deadline, not the configured 200ms"
    );
}

// ---------------------------------------------------------------------
// Travel guards: goto watches the events it would otherwise throw away,
// and hands the connection back when the walk has become the wrong thing
// to be doing. The guard only ever observes — nav keeps sole ownership
// of the socket, so the one-sender invariant survives.
// ---------------------------------------------------------------------

/// Trips when a named room block goes past.
struct TripsOnRoom(&'static str);

impl TravelGuard for TripsOnRoom {
    fn on_event(&mut self, ev: &Event) -> Option<Interrupt> {
        match ev {
            Event::RoomSeen(r) if r.name == self.0 => Some(Interrupt::Hurt { hp: 7 }),
            _ => None,
        }
    }
}

/// Trips on any line the board prints.
struct TripsOnLine;

impl TravelGuard for TripsOnLine {
    fn on_event(&mut self, ev: &Event) -> Option<Interrupt> {
        matches!(ev, Event::Line(_)).then_some(Interrupt::Died)
    }
}

/// Hurt arms the walk; it does not abandon it mid-step. The direction
/// word for this step is already down the wire when the guard trips, so
/// returning here and now would name a room the character is in the act
/// of leaving. Finish the step, verify it, *then* hand back — `at` is
/// worth having only if it is true.
#[tokio::test]
async fn a_hurt_guard_finishes_the_step_before_handing_back() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());

    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut TripsOnRoom("Town Square"),
        )
        .await
        .expect_err("the guard tripped");

    assert!(matches!(
        err.kind,
        NavErrorKind::Interrupted(Interrupt::Hurt { hp: 7 })
    ));
    // Town Square is where the tripping step landed, and it is verified.
    assert_eq!(err.at, RoomId { map: 1, room: 2 });
    let state = session.state().borrow().clone();
    assert_eq!(
        state.room.as_ref().map(|r| r.name.as_str()),
        Some("Town Square"),
        "the walk must stop, not carry on to the target"
    );
}

/// Arming on the *last* step of a route is the case with nowhere left to
/// notice it: there is no next step whose drain would catch the arming,
/// so a walk that only checked between steps would arrive, report
/// success, and lose the interrupt entirely.
#[tokio::test]
async fn a_hurt_guard_arming_on_the_last_step_still_hands_back() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());

    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut TripsOnRoom("Market Street"),
        )
        .await
        .expect_err("arriving is not the same as being fit to carry on");

    assert!(matches!(
        err.kind,
        NavErrorKind::Interrupted(Interrupt::Hurt { hp: 7 })
    ));
    // It did arrive — the step was finished and verified before the
    // walk handed back.
    assert_eq!(err.at, RoomId { map: 1, room: 3 });
}

/// Death is the exception: a downed character is not going to complete
/// the step, so waiting for a room block that will never come would cost
/// the whole step deadline on every death. Hand back immediately.
#[tokio::test]
async fn a_death_guard_does_not_wait_out_the_step() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(
        Arc::new(graph_with_a_phantom_exit()),
        NavConfig {
            step_timeout_ms: 10_000,
        },
    );

    let started = std::time::Instant::now();
    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut TripsOnLine,
        )
        .await
        .expect_err("the guard tripped");

    assert!(matches!(
        err.kind,
        NavErrorKind::Interrupted(Interrupt::Died)
    ));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "waited out the step deadline instead of handing back at once"
    );
}

/// A guard that never trips leaves the walk exactly as it was.
#[tokio::test]
async fn an_unarmed_guard_changes_nothing() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());

    let at = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut NoGuard,
        )
        .await
        .expect("navigate gates -> market");

    assert_eq!(at, RoomId { map: 1, room: 3 });
}

// ---------------------------------------------------------------------
// localize: which room is this, given where we were?
//
// goto has always needed this to recover from a step that landed
// somewhere unexpected. The farm runner needs the same answer for a
// different reason: AutoFlee moves the character with no navigator
// involved, so before it can walk back it has to work out where "back"
// is from. Same question, same one-hop assumption, one implementation.
// ---------------------------------------------------------------------

#[test]
fn localize_finds_a_neighbor_by_name() {
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());
    // From Town Square, east leads to Market Street.
    assert_eq!(
        nav.localize(RoomId { map: 1, room: 2 }, "Market Street"),
        Some(RoomId { map: 1, room: 3 })
    );
}

/// A step that did not take: the room name is still the one we were in.
/// That is a legitimate answer, not a desync.
#[test]
fn localize_accepts_not_having_moved() {
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());
    assert_eq!(
        nav.localize(RoomId { map: 1, room: 2 }, "Town Square"),
        Some(RoomId { map: 1, room: 2 })
    );
}

/// More than one hop away, or not in the graph at all. The caller must
/// stop rather than guess — walking blind is what verified navigation
/// exists to prevent.
#[test]
fn localize_gives_up_on_a_room_that_is_not_adjacent() {
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());
    assert_eq!(nav.localize(RoomId { map: 1, room: 1 }, "Market Street"), None);
    assert_eq!(nav.localize(RoomId { map: 1, room: 1 }, "Nowhere At All"), None);
}

#[test]
fn localize_gives_up_when_the_starting_room_is_unknown() {
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());
    assert_eq!(nav.localize(RoomId { map: 9, room: 9 }, "Town Square"), None);
}
