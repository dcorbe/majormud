//! `/go` end to end against the in-process mud-server.
//!
//! The harness is the `nav.rs` shape — test crates do not share modules,
//! so it is duplicated here, which is this suite's existing pattern (see
//! the same note in `farm_scripted.rs`).
//!
//! What this file is really testing is the preamble: `run_go` is handed
//! no starting room, so it has to ask the board where it is standing and
//! turn the answer into a room id before it can route anywhere. The walk
//! itself belongs to `Navigator::goto` and is covered in `nav.rs`.

use std::sync::Arc;

use mud_client::dialect::{self, Target};
use mud_client::farm::FarmConfig;
use mud_client::go::{GoEnd, resolve, run_go};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Race, RaceId, Room, RoomId, StatBlock,
};
use mud_core::game::CoreConfig;
use mud_server::{server::Server, state_db::StateDb};

/// A notices sink that keeps nothing. What a runner says at startup is
/// not what these tests are about.
fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

const GATES: RoomId = RoomId { map: 1, room: 1 };
const SQUARE: RoomId = RoomId { map: 1, room: 2 };
const MARKET: RoomId = RoomId { map: 1, room: 3 };

fn tiny_world() -> Content {
    let mut content = Content::default();
    let names = [(1u16, "Town Gates"), (2u16, "Town Square"), (3u16, "Market Street")];
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
        dest: SQUARE,
        ..Default::default()
    });
    rooms[1].exits[Direction::South as usize] = Some(Exit {
        dest: GATES,
        ..Default::default()
    });
    rooms[1].exits[Direction::East as usize] = Some(Exit {
        dest: MARKET,
        ..Default::default()
    });
    rooms[2].exits[Direction::West as usize] = Some(Exit {
        dest: SQUARE,
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

fn client_graph() -> Arc<RoomGraph> {
    let mk = |n: u16, name: &str, exits: Vec<(Direction, u16)>| {
        let mut room = GraphRoom {
            name: name.into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        for (d, dest) in exits {
            room.exits[d as usize] = Some(ExitEdge {
                dest: RoomId { map: 1, room: dest },
                exit_type: 0,
                command: None,
                requirement: ExitRequirement::None,
            });
        }
        (RoomId { map: 1, room: n }, room)
    };
    Arc::new(RoomGraph::from_rooms(vec![
        mk(1, "Town Gates", vec![(Direction::North, 2)]),
        mk(2, "Town Square", vec![(Direction::South, 1), (Direction::East, 3)]),
        mk(3, "Market Street", vec![(Direction::West, 2)]),
    ]))
}

async fn logged_in_session(addr: std::net::SocketAddr) -> Arc<Session> {
    let profile = Profile {
        target: Target::RustServer,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "Goer".into(),
        password: "pw".into(),
        pace_ms: Some(0),
        disable_evil_warnings: false,
        bot: None,
        farm: None,
        bank: Default::default(),
        ..Default::default()
    };
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());
    let outcome = dialect::login(&session, &profile).await.unwrap();
    assert_eq!(outcome, mud_client::dialect::LoginOutcome::CharacterCreation);
    dialect::finish_creation(&session).await.unwrap();
    session
}

async fn start() -> Server {
    let config = CoreConfig {
        start_location: GATES,
        exit_meditation_seconds: 1,
        ansi: true,
        ..CoreConfig::default()
    };
    Server::start(
        tiny_world(),
        config,
        StateDb::open_in_memory().unwrap(),
        "127.0.0.1:0",
    )
    .await
    .unwrap()
}

/// A config with no room database behind it: the tiny world has no
/// sqlite file, so threat ranking and death wordings are simply absent,
/// which `run_go` must treat as "no opinion" rather than as fatal.
fn cfg() -> FarmConfig {
    mud_client::go::go_config(
        &FarmConfig {
            content: std::path::PathBuf::from("/nonexistent/rooms.sqlite"),
            ..Default::default()
        },
        false,
    )
}

#[tokio::test]
async fn go_walks_to_a_named_room() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let graph = client_graph();

    let to = resolve(&graph, Some(GATES), "Market Street").expect("one match");
    assert_eq!(to, MARKET);

    let end = run_go(
        &session,
        graph,
        Some(GATES),
        to,
        &mud_client::bot::BotConfig::default(),
        &cfg(),
        None,
        &quiet(),
    )
    .await
    .expect("walk gates -> market");

    assert_eq!(end, GoEnd::Arrived(MARKET));
    let state = session.state().borrow().clone();
    assert_eq!(
        state.room.as_ref().map(|r| r.name.as_str()),
        Some("Market Street")
    );
}

/// The part most likely to be wrong: no hint at all, so `run_go` must
/// `look`, parse the block, and find itself in the graph by name and
/// exits before it can route. Walked one room by hand first so the
/// answer is not simply the start room.
#[tokio::test]
async fn go_finds_its_own_starting_room() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let graph = client_graph();

    session.send("n");
    session
        .expect("Town Square", std::time::Duration::from_secs(15))
        .await
        .expect("walked north by hand");

    let end = run_go(
        &session,
        graph,
        None,
        MARKET,
        &mud_client::bot::BotConfig::default(),
        &cfg(),
        None,
        &quiet(),
    )
    .await
    .expect("walk square -> market with no hint");

    assert_eq!(end, GoEnd::Arrived(MARKET));
}

#[tokio::test]
async fn going_where_you_already_stand_is_a_no_op() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;

    let end = run_go(
        &session,
        client_graph(),
        Some(GATES),
        GATES,
        &mud_client::bot::BotConfig::default(),
        &cfg(),
        None,
        &quiet(),
    )
    .await
    .expect("already there");

    assert_eq!(end, GoEnd::Arrived(GATES));
}

