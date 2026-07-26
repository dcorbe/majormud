//! Navigator tests: verified step-by-step movement against the
//! in-process mud-server. Every step is confirmed by parsing the
//! destination room name — never blind (monsters get free attacks on
//! movement; desyncs must surface immediately).

use std::sync::Arc;

use mud_client::dialect::{self, Target};
use mud_client::graph::{ExitEdge, GraphRoom, RoomGraph};
use mud_client::nav::{NavError, Navigator};
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
    let nav = Navigator::new(Arc::new(client_graph("Market Street")));

    nav.goto(
        &session,
        RoomId { map: 1, room: 1 },
        RoomId { map: 1, room: 3 },
    )
    .await
    .expect("navigate gates -> market");

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
    let nav = Navigator::new(Arc::new(client_graph("Crystal Cavern")));

    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
        )
        .await
        .expect_err("must detect desync");
    match err {
        NavError::Desync { expected, saw } => {
            assert_eq!(expected, "Crystal Cavern");
            assert_eq!(saw.as_deref(), Some("Market Street"));
        }
        other => panic!("expected Desync, got {other:?}"),
    }
}

#[tokio::test]
async fn goto_without_route_fails_fast() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")));
    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 9, room: 9 },
        )
        .await
        .expect_err("no route");
    assert!(matches!(err, NavError::NoRoute));
}
