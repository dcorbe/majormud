//! The farm runner against a live in-process server.
//!
//! `tests/farm.rs` covers the pure pieces (plan, gate, heal watch) and
//! `tests/farm_corpus.rs` replays real transcripts. This file is the
//! only place the whole thing runs: log in, walk a circuit, kill what
//! stands in it, and stop on purpose.
//!
//! The world is deliberately small but not tidy. The Training Yard
//! boot-fills a lowercase `giant rat` whose death line ends in "falls to
//! the ground" — the wording the bot's death detector actually keys on —
//! and its first-listed exit leads somewhere other than the way back, so
//! a flee lands the character off the circuit exactly as it would on the
//! board.

use std::sync::Arc;
use std::time::Duration;

use mud_client::bot::BotConfig;
use mud_client::dialect::{self, Target};
use mud_client::farm::{FarmConfig, FarmEnd, FarmError, FarmPlan, FarmStats, run_farm};
use mud_client::graph::{ExitEdge, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Message, MessageId, Monster, MonsterId, Race, RaceId,
    Room, RoomId, StatBlock,
};
use mud_core::game::CoreConfig;
use mud_server::{server::Server, state_db::StateDb};

const GATES: RoomId = RoomId { map: 1, room: 1 };
const YARD: RoomId = RoomId { map: 1, room: 2 };
const CELLAR: RoomId = RoomId { map: 1, room: 3 };
const ALLEY: RoomId = RoomId { map: 1, room: 4 };

const RAT: MonsterId = MonsterId(50);

/// The rat's death line. Index 2 is the line the server prints
/// (`game.rs` reads `lines.get(2)`), and "falls to the ground" is the
/// tail `bot.rs` matches on — a monster death, not a player's.
const RAT_DEATH: &str = "The giant rat falls to the ground with a tortured squeak.";

fn room(id: RoomId, name: &str, exits: &[(Direction, RoomId)]) -> Room {
    let mut r = Room {
        id,
        name: name.into(),
        ..Default::default()
    };
    for (d, dest) in exits {
        r.exits[*d as usize] = Some(Exit {
            dest: *dest,
            ..Default::default()
        });
    }
    r
}

fn world() -> Content {
    let mut content = Content::default();

    content.add_room(room(GATES, "Town Gates", &[(Direction::North, YARD)]));

    // North is listed first, so it is the exit AutoFlee takes — and it
    // leads to the Side Alley, off the circuit, not back the way we came.
    let mut yard = room(
        YARD,
        "Training Yard",
        &[
            (Direction::North, ALLEY),
            (Direction::South, GATES),
            (Direction::East, CELLAR),
        ],
    );
    // Type 3 boot-fills to the cap, so the rat is standing there before
    // the first player connects; the forced template keeps it a rat.
    yard.room_type = 3;
    yard.spawn_zone = 7;
    yard.spawn_cap = 1;
    yard.min_level = 1;
    yard.max_level = 5;
    yard.forced_monster = Some(RAT);
    // Long enough that nothing respawns mid-test.
    yard.respawn_delay = 9999;
    content.add_room(yard);

    content.add_room(room(CELLAR, "Rat Cellar", &[(Direction::West, YARD)]));
    content.add_room(room(ALLEY, "Side Alley", &[(Direction::South, YARD)]));

    content.add_message(Message {
        id: MessageId(1),
        lines: vec![String::new(), String::new(), RAT_DEATH.into()],
    });
    content.add_monster(Monster {
        id: RAT,
        name: "giant rat".into(),
        death_msg: Some(MessageId(1)),
        // One good punch. The point of the fixture is the farm loop, not
        // a long fight.
        hitpoints: 1,
        energy: 1000,
        roam_class: 7,
        level: 1,
        behaviour: 0, // passive: it will not chase or fight back
        herd_mode: 0,
        ..Default::default()
    });

    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 40,
            wisdom: 40,
            strength: 90,
            health: 40,
            agility: 90,
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

/// The client-side graph mirroring the server world.
fn client_graph() -> RoomGraph {
    let mk = |id: RoomId, name: &str, exits: &[(Direction, RoomId)]| {
        let mut r = GraphRoom {
            name: name.into(),
            exits: Default::default(),
        };
        for (d, dest) in exits {
            r.exits[*d as usize] = Some(ExitEdge {
                dest: *dest,
                exit_type: 0,
            });
        }
        (id, r)
    };
    RoomGraph::from_rooms(vec![
        mk(GATES, "Town Gates", &[(Direction::North, YARD)]),
        mk(
            YARD,
            "Training Yard",
            &[
                (Direction::North, ALLEY),
                (Direction::South, GATES),
                (Direction::East, CELLAR),
            ],
        ),
        mk(CELLAR, "Rat Cellar", &[(Direction::West, YARD)]),
        mk(ALLEY, "Side Alley", &[(Direction::South, YARD)]),
    ])
}

async fn start() -> Server {
    let config = CoreConfig {
        start_location: GATES,
        exit_meditation_seconds: 1,
        // The room classifier keys on the 1;36 name colour.
        ansi: true,
        ..CoreConfig::default()
    };
    Server::start(
        world(),
        config,
        StateDb::open_in_memory().unwrap(),
        "127.0.0.1:0",
    )
    .await
    .unwrap()
}

async fn logged_in(addr: std::net::SocketAddr, name: &str) -> Arc<Session> {
    let profile = Profile {
        target: Target::RustServer,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: name.into(),
        password: "pw".into(),
        pace_ms: Some(0),
        bot: None,
        farm: None,
    };
    let session = Arc::new(Session::connect(&profile, None).await.unwrap());
    dialect::login(&session, &profile).await.unwrap();
    dialect::finish_creation(&session).await.unwrap();
    session
}

/// Before anything is built on this world, prove it does what the runner
/// will assume: a lowercase rat is standing in the Training Yard, "a
/// rat" engages it, and killing it prints the line the bot's death
/// detector matches.
#[tokio::test]
async fn the_fixture_world_spawns_a_rat_that_can_be_killed() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Ratter").await;
    let t = Duration::from_secs(10);

    session.send("n");
    session
        .expect("Training Yard", t)
        .await
        .expect("walked north");
    session
        .expect("giant rat", t)
        .await
        .expect("a rat should be standing here at boot");

    session.send("a rat");
    session
        .expect("falls to the ground", t)
        .await
        .expect("the rat should die and print its death line");
}

/// The server answers input and then goes quiet — and so does the real
/// board, for minutes at a time (`tests/board_cadence.rs`). Neither
/// sends a prompt to an idle session, so a dwell rule that only counted
/// prompts would wait forever on both. That is why the runner pokes an
/// idle stop with a `look` and counts the answers.
///
/// The poke interval here is far below the shipped default: these tests
/// want the dwell to expire in milliseconds, not the fifteen seconds
/// that is right for a live board.
fn farm_config(circuit: &[&str], loops: u32) -> FarmConfig {
    FarmConfig {
        start: "1/1".into(),
        circuit: circuit.iter().map(|s| (*s).to_string()).collect(),
        loops,
        dwell_idle_prompts: 2,
        idle_poke_ms: 150,
        // HP gating off: these tests are about the circuit, and the
        // depart gate has nothing to do while nothing is hitting us.
        depart_at_percent: 0,
        ..FarmConfig::default()
    }
}

async fn farm(
    session: &Session,
    bot: BotConfig,
    cfg: FarmConfig,
) -> Result<(FarmEnd, FarmStats), FarmError> {
    let graph = Arc::new(client_graph());
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(session, graph.clone(), &plan, &bot, &cfg),
    )
    .await
    .expect("run_farm should finish, not hang")
}

/// The whole point of the slice: walk the circuit, kill what is standing
/// in it, and stop when the lap count is met.
#[tokio::test]
async fn walks_the_circuit_killing_what_it_finds() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Farmer").await;

    let bot = BotConfig {
        auto_combat: true,
        auto_get: true,
        // 0 means "ask the board" — the runner probes it at startup.
        max_hp: 0,
        ..BotConfig::default()
    };
    let (end, stats) = farm(&session, bot, farm_config(&["1/2", "1/3"], 1))
        .await
        .expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone);
    assert_eq!(stats.loops, 1);
    assert!(stats.kills >= 1, "the rat should have died: {stats:?}");
    assert_eq!(
        session
            .state()
            .borrow()
            .room
            .as_ref()
            .map(|r| r.name.clone()),
        Some("Rat Cellar".into()),
        "should finish standing in the last stop of the circuit"
    );
}

/// The circuit is a list of room ids, and the character's position is
/// only ever confirmed by room name. Starting somewhere other than the
/// configured start means every id afterwards refers to the wrong room,
/// so the runner refuses rather than walking a live character blind.
#[tokio::test]
async fn refuses_to_run_from_the_wrong_room() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Lost").await;
    session.send("n"); // now in the Training Yard, not the Town Gates
    session
        .expect("Training Yard", Duration::from_secs(10))
        .await
        .unwrap();

    let err = farm(&session, BotConfig::default(), farm_config(&["1/2"], 1))
        .await
        .expect_err("must refuse");

    match err {
        FarmError::NotAtStart { expected, saw } => {
            assert_eq!(expected, "Town Gates");
            assert_eq!(saw.as_deref(), Some("Training Yard"));
        }
        other => panic!("expected NotAtStart, got {other:?}"),
    }
}

/// AutoFlee moves the character with no navigator involved, and the
/// Training Yard's first exit leads off the circuit. The runner has to
/// notice, work out where it landed, and walk back — then give up on the
/// stop rather than flee-loop forever.
#[tokio::test]
async fn recovers_from_a_flee_and_moves_on() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Coward").await;

    let bot = BotConfig {
        auto_flee: true,
        // Above 100%: every prompt looks like an emergency, so the flee
        // fires without having to choreograph real damage.
        flee_at_percent: 101,
        max_hp: 0,
        ..BotConfig::default()
    };
    let (end, stats) = farm(&session, bot, farm_config(&["1/2"], 1))
        .await
        .expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone);
    assert!(
        stats.flees >= 1,
        "should have fled at least once: {stats:?}"
    );
    // The real property: it did not just notice the flee, it walked back.
    // Without the return trip the character is left in the Side Alley,
    // off the circuit, and the next leg would start from a lie.
    assert_eq!(
        session
            .state()
            .borrow()
            .room
            .as_ref()
            .map(|r| r.name.clone()),
        Some("Training Yard".into()),
        "should have navigated back to the stop it fled from"
    );
}
