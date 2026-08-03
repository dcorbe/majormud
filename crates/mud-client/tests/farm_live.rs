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
const BEETLE: MonsterId = MonsterId(51);
const BUG: MonsterId = MonsterId(52);

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
        // Aggressive (mode 2), for two reasons. Swinging at it is not a
        // crime -- M7 charges evil only for an unprovoked behaviour-0/4
        // monster, and a fresh character ships with evil warnings ON, so
        // a mode-0 fixture is refused outright. And the server paints
        // occupants by behaviour, so a passive mode (0/3) would render
        // cyan and the bot would correctly decline to fight it: the
        // fixture has to SAY the rat is something worth farming.
        behaviour: 2,
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

/// The fixture world plus a monster the board will not let a fresh
/// character attack: behaviour 0 is "unprovoked", M7's crime system
/// charges evil for swinging at it, and a new character ships with evil
/// warnings ON — so the attack is refused rather than started. This is
/// the shape most of the shipped bestiary has, which is why the runner
/// must survive it.
fn world_with_refused_monster() -> Content {
    let mut content = world();
    let mut cellar = room(CELLAR, "Rat Cellar", &[(Direction::West, YARD)]);
    cellar.room_type = 3;
    cellar.spawn_zone = 8;
    cellar.spawn_cap = 1;
    cellar.min_level = 1;
    cellar.max_level = 5;
    cellar.forced_monster = Some(BEETLE);
    cellar.respawn_delay = 9999;
    content.add_room(cellar);
    // Without a death record the server falls back to "<name> is dead."
    // — the PLAYER form, which the bot deliberately does not match — so
    // the fixture would latch on its own kill. 1087 of the 1101 shipped
    // monsters carry a record, so having one is the realistic case.
    content.add_message(Message {
        id: MessageId(2),
        lines: vec![
            String::new(),
            String::new(),
            "The giant beetle falls to the ground with a wet crunch.".into(),
        ],
    });
    content.add_monster(Monster {
        id: BEETLE,
        name: "giant beetle".into(),
        death_msg: Some(MessageId(2)),
        // One punch, like the rat. A beefier beetle would out-live the
        // toggle test's timeout once the swing actually lands; the
        // refusal test's `kills == 0` is what catches a crime gate that
        // stops refusing, so the padding bought nothing.
        hitpoints: 1,
        energy: 1000,
        roam_class: 8,
        level: 1,
        behaviour: 0,
        herd_mode: 0,
        ..Default::default()
    });
    content
}

/// The refused beetle standing ON THE WAY instead of at the stop: the
/// Training Yard spawns it, the cellar is plain. A leg passing through
/// must sight it, learn the refusal during the defence, and then walk
/// past — never loop on the room and never burn the interrupt budget.
fn world_with_a_refused_monster_on_the_way() -> Content {
    let mut content = world_with_refused_monster();
    let mut yard = room(
        YARD,
        "Training Yard",
        &[
            (Direction::North, ALLEY),
            (Direction::South, GATES),
            (Direction::East, CELLAR),
        ],
    );
    yard.room_type = 3;
    yard.spawn_zone = 7;
    yard.spawn_cap = 1;
    yard.min_level = 1;
    yard.max_level = 5;
    yard.forced_monster = Some(BEETLE);
    yard.respawn_delay = 9999;
    content.add_room(yard);
    content.add_room(room(CELLAR, "Rat Cellar", &[(Direction::West, YARD)]));
    content
}

/// The fixture world with a monster whose death line does NOT contain
/// "falls to the ground" — which is the normal case, not the exception:
/// of the 1085 shipped monsters carrying a death record, 1018 word it
/// some other way. The wording here is the filthbug's, verbatim from
/// message 31.
fn world_with_prose_death() -> Content {
    let mut content = world();
    let mut cellar = room(CELLAR, "Rat Cellar", &[(Direction::West, YARD)]);
    cellar.room_type = 3;
    cellar.spawn_zone = 9;
    cellar.spawn_cap = 1;
    cellar.min_level = 1;
    cellar.max_level = 5;
    cellar.forced_monster = Some(BUG);
    cellar.respawn_delay = 9999;
    content.add_room(cellar);
    content.add_message(Message {
        id: MessageId(3),
        lines: vec![
            String::new(),
            String::new(),
            "The filthbug collapses, its legs curling tightly around it.".into(),
        ],
    });
    content.add_monster(Monster {
        id: BUG,
        name: "filthbug".into(),
        death_msg: Some(MessageId(3)),
        hitpoints: 1,
        energy: 1000,
        roam_class: 9,
        level: 1,
        behaviour: 2,
        herd_mode: 0,
        ..Default::default()
    });
    content
}

/// A stop holding THREE monsters at once.
///
/// Every other fixture room holds exactly one, and that is precisely why
/// the runner shipped a bug that walked out of a room with three things
/// still standing in it: a kill was followed by a prompt, the prompt
/// count ran out, and the stop ended. With one monster per room the stop
/// was legitimately finished after the only kill, so no test could tell
/// the difference.
///
/// `room_type` 3 boot-fills to the cap before the first player connects,
/// and the long respawn delay keeps anything from refilling mid-test —
/// so exactly three deaths are available and no more.
fn world_with_three_monsters() -> Content {
    let mut content = world();
    let mut yard = room(
        YARD,
        "Training Yard",
        &[
            (Direction::North, ALLEY),
            (Direction::South, GATES),
            (Direction::East, CELLAR),
        ],
    );
    yard.room_type = 3;
    yard.spawn_zone = 7;
    yard.spawn_cap = 3;
    yard.min_level = 1;
    yard.max_level = 5;
    yard.forced_monster = Some(RAT);
    yard.respawn_delay = 9999;
    content.add_room(yard);
    content
}

/// The client-side graph mirroring the server world.
fn client_graph() -> RoomGraph {
    let mk = |id: RoomId, name: &str, exits: &[(Direction, RoomId)]| {
        let mut r = GraphRoom {
            name: name.into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        for (d, dest) in exits {
            r.exits[*d as usize] = Some(ExitEdge {
                dest: *dest,
                exit_type: 0,
                command: None,
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
    start_with(world()).await
}

async fn start_with(content: Content) -> Server {
    let config = CoreConfig {
        start_location: GATES,
        exit_meditation_seconds: 1,
        // The room classifier keys on the 1;36 name colour.
        ansi: true,
        ..CoreConfig::default()
    };
    Server::start(
        content,
        config,
        StateDb::open_in_memory().unwrap(),
        "127.0.0.1:0",
    )
    .await
    .unwrap()
}

async fn logged_in(addr: std::net::SocketAddr, name: &str) -> Arc<Session> {
    logged_in_with(addr, name, false).await
}

async fn logged_in_with(
    addr: std::net::SocketAddr,
    name: &str,
    disable_evil_warnings: bool,
) -> Arc<Session> {
    let profile = Profile {
        target: Target::RustServer,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: name.into(),
        password: "pw".into(),
        pace_ms: Some(0),
        disable_evil_warnings,
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
/// want a stale observation re-asked in milliseconds, not the five
/// seconds that is right for a live board. `dwell_empty_seconds` stays
/// at its default 0 — the suite wants the runner to move on the instant
/// the room block proves the room empty, with no respawn wait.
fn farm_config(circuit: &[&str], loops: u32) -> FarmConfig {
    FarmConfig {
        start: "1/1".into(),
        circuit: circuit.iter().map(|s| (*s).to_string()).collect(),
        loops,
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
        run_farm(session, graph.clone(), &plan, &bot, &cfg, None),
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

/// `[farm].start` is where the character is EXPECTED to be, not a
/// precondition. Standing somewhere else is the ordinary case — a run
/// that died, a walk that wandered, a login in the wrong room — and the
/// runner has a navigator, so it walks to the circuit rather than
/// refusing. Position is still confirmed before the first step; the
/// difference is that the answer is used instead of only being checked.
#[tokio::test]
async fn walks_to_the_circuit_from_the_wrong_room() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Lost").await;
    session.send("n"); // now in the Training Yard, not the Town Gates
    session
        .expect("Training Yard", Duration::from_secs(10))
        .await
        .unwrap();

    let (end, stats) = farm(&session, BotConfig::default(), farm_config(&["1/2"], 1))
        .await
        .expect("should walk to the circuit, not refuse");

    assert_eq!(end, FarmEnd::LoopsDone);
    assert_eq!(stats.loops, 1);
}

/// The one position failure left: a room block the graph cannot place at
/// all. There is no honest way to route from an unknown room, so that
/// still stops the run.
#[tokio::test]
async fn refuses_to_run_from_a_room_it_cannot_place() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Nowhere").await;
    // A graph that disagrees with the board about what these rooms are
    // called. The character is standing in the Town Gates and no room in
    // this navigator's world answers to that name, so there is nothing to
    // route from.
    let mut gates = GraphRoom {
        name: "Somewhere Else Entirely".into(),
        ..Default::default()
    };
    gates.exits[Direction::North as usize] = Some(ExitEdge {
        dest: YARD,
        exit_type: 0,
        command: None,
    });
    let graph = Arc::new(RoomGraph::from_rooms(vec![
        (GATES, gates),
        (
            YARD,
            GraphRoom {
                name: "Nor This One".into(),
                ..Default::default()
            },
        ),
    ]));
    let cfg = farm_config(&["1/2"], 1);
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let err = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &BotConfig::default(), &cfg, None),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect_err("must refuse");

    match err {
        // Unknown, not a maze: walking cannot teach the client about a
        // world it did not load, so no command should have gone out.
        FarmError::Lost(mud_client::lost::Lost::Unknown { saw }) => {
            assert_eq!(saw, "Town Gates")
        }
        other => panic!("expected an unplaceable room, got {other:?}"),
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
    // KNOWN FLAKE, roughly 1 full-suite run in 10, and only under the
    // load of the whole file running at once:
    //
    //   farm run: Nav(Desync { at: 1/4, expected: "Training Yard",
    //                          saw: "Town Gates" })
    //
    // The walk back out of the Side Alley is verified by room name with
    // no request/response correlation underneath it, so a room block the
    // runner asked for BEFORE the flee can land during the first step and
    // satisfy it. The character is then a room further on than the
    // navigator believes.
    //
    // Measured at 1/11 here and 0/4 on the pre-refactor runner, which is
    // not enough to attribute — this config flees on EVERY prompt (101%),
    // which no real profile does, and recovery has never had correlation
    // to lean on. Recorded rather than papered over: the fix belongs with
    // `recover`, not with another timing tweak here.
    // A dwell keeps the stop open long enough for a prompt to reach the
    // bot. The stop used to linger by ACCIDENT — the old runner stacked
    // redundant looks and could not leave until the gate drained; the
    // attributed runner leaves the instant a clean empty answer lands,
    // so the flee choreography needs an honest budget instead.
    let cfg = FarmConfig {
        dwell_empty_seconds: 2,
        ..farm_config(&["1/2"], 1)
    };
    let (end, stats) = farm(&session, bot, cfg).await.expect("farm run");

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

/// Travel used to be blind: goto walked, and every event that was not a
/// room block went in the bin. Now a wounded character stops walking,
/// defends where it stands, and picks the leg back up.
///
/// The threshold is set above 100% so that every prompt reads as an
/// emergency — the same trick `recovers_from_a_flee_and_moves_on` uses
/// with flee_at_percent, and for the same reason: the fixture rat is
/// passive and cannot actually hurt anyone, so real damage would have to
/// be choreographed. What is genuinely under test is the machinery — the
/// interrupt is noticed, the leg is not abandoned, and the patrol still
/// finishes standing where the plan says.
#[tokio::test]
async fn an_interrupted_leg_is_defended_and_resumed() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Wounded").await;

    let cfg = FarmConfig {
        interrupt_at_percent: 101,
        ..farm_config(&["1/3"], 1)
    };
    let (end, stats) = farm(&session, BotConfig::default(), cfg)
        .await
        .expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone);
    assert!(
        stats.interrupts >= 1,
        "the walk should have been interrupted: {stats:?}"
    );
    assert_eq!(
        session
            .state()
            .borrow()
            .room
            .as_ref()
            .map(|r| r.name.clone()),
        Some("Rat Cellar".into()),
        "the interrupted leg has to be finished, not abandoned"
    );
}

/// Defending is bounded. A character that keeps being interrupted is not
/// going to walk this leg, and carrying on regardless is how a farm run
/// ends in a corpse — so the budget runs out and the run stops, standing
/// somewhere known.
#[tokio::test]
async fn the_interrupt_budget_ends_the_run() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Doomed").await;

    let cfg = FarmConfig {
        interrupt_at_percent: 101,
        travel_interrupts: 0,
        ..farm_config(&["1/3"], 1)
    };
    let (end, stats) = farm(&session, BotConfig::default(), cfg)
        .await
        .expect("farm run");

    assert_eq!(end, FarmEnd::TooHurt);
    assert_eq!(stats.interrupts, 1);
    assert_eq!(stats.loops, 0, "it never finished a lap");
}

/// The live incident this whole feature pins: a leg walked through a
/// room whose arrival render listed three monsters and kept sending
/// steps while they attacked. The Yard rat stands on the way to the
/// cellar; the leg must stop, fight it, and still finish the lap.
/// `travel_interrupts: 0` in the same breath proves a sighting is not
/// an emergency — if it touched the budget, this run would end TooHurt.
#[tokio::test]
async fn a_monster_on_the_way_is_fought_not_walked_past() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Sighter").await;

    let bot = BotConfig {
        auto_combat: true,
        max_hp: 0,
        ..BotConfig::default()
    };
    let cfg = FarmConfig {
        travel_interrupts: 0,
        ..farm_config(&["1/3"], 1)
    };
    let (end, stats) = farm(&session, bot, cfg).await.expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone);
    assert!(
        stats.kills >= 1,
        "the rat on the way should have died: {stats:?}"
    );
    assert!(
        stats.sightings >= 1,
        "the fight should have been a sighting: {stats:?}"
    );
    assert_eq!(
        stats.interrupts, 0,
        "a sighting is work, not an emergency: {stats:?}"
    );
    assert_eq!(
        session
            .state()
            .borrow()
            .room
            .as_ref()
            .map(|r| r.name.clone()),
        Some("Rat Cellar".into()),
        "the leg has to be finished after the fight, not abandoned"
    );
}

/// A sighting the defence cannot clear — the crime gate refuses the
/// beetle — must be walked past, not looped on. Honest about scope: it
/// passes pre-change too (the leg was simply blind), so what it pins is
/// the new machinery's failure modes — a naive implementation that
/// re-trips forever hangs the 30s harness, and one that spends the
/// budget ends TooHurt. The refusal learned during the defence is
/// shared with the guard, which is what stops the re-trip.
#[tokio::test]
async fn an_unkillable_sighting_is_walked_past_on_the_retry() {
    let server = start_with(world_with_a_refused_monster_on_the_way()).await;
    let session = logged_in(server.local_addr(), "Passer").await;

    let bot = BotConfig {
        auto_combat: true,
        max_hp: 0,
        ..BotConfig::default()
    };
    let cfg = FarmConfig {
        travel_interrupts: 0,
        ..farm_config(&["1/3"], 1)
    };
    let (end, stats) = farm(&session, bot, cfg).await.expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert_eq!(
        stats.kills, 0,
        "nothing here is killable; a kill means the crime gate stopped refusing: {stats:?}"
    );
    assert_eq!(
        session
            .state()
            .borrow()
            .room
            .as_ref()
            .map(|r| r.name.clone()),
        Some("Rat Cellar".into()),
        "the leg must end at the stop with the beetle behind it"
    );
}

/// The gate: `fight_while_travelling = false` means get there without
/// swinging, and a listed monster is walked past exactly like before.
#[tokio::test]
async fn no_sighting_when_fight_while_travelling_is_off() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Runner").await;

    let bot = BotConfig {
        auto_combat: true,
        max_hp: 0,
        ..BotConfig::default()
    };
    let cfg = FarmConfig {
        fight_while_travelling: false,
        ..farm_config(&["1/3"], 1)
    };
    let (end, stats) = farm(&session, bot, cfg).await.expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone);
    assert_eq!(stats.sightings, 0, "{stats:?}");
    assert_eq!(
        stats.kills, 0,
        "running past means the rat is still alive: {stats:?}"
    );
}

/// Before relying on the refusal, prove the world produces it: a fresh
/// character swinging at the behaviour-0 beetle is turned down, and the
/// wording is the one `bot.rs` matches on.
#[tokio::test]
async fn the_fixture_world_has_a_monster_the_board_refuses_to_attack() {
    let server = start_with(world_with_refused_monster()).await;
    let session = logged_in(server.local_addr(), "Beetler").await;
    let t = Duration::from_secs(10);

    session.send("n");
    session.expect("Training Yard", t).await.expect("walked north");
    session.send("e");
    session.expect("Rat Cellar", t).await.expect("walked east");
    session
        .expect("giant beetle", t)
        .await
        .expect("a beetle should be standing here at boot");

    session.send("a beetle");
    session
        .expect(mud_core::crime::WARN_ON_EVIL_REFUSAL, t)
        .await
        .expect("the board should refuse the swing, not start a fight");
}

/// The regression this whole change exists for. A refused attack yields
/// no death line, no ActorLeft and no room block without the target, so
/// the engaged latch used to stay set forever -- which made `farm_stop`
/// suppress its idle poke and reset its dwell counter on every prompt.
/// The run did not fail; it HUNG. The timeout is what turns a
/// regression back into a test failure instead of a wedged suite.
///
/// RETARGETED 2026-08-02. The bot now reads the board's occupant colour,
/// and a crime-refused monster is ALWAYS painted passive — the crime
/// gate fires only for behaviour 0/4, and both render cyan. So the bot
/// never swings at one and this exact hang is unreachable through it.
/// What the fixture still proves is the risk that replaced it: a passive
/// occupant standing in the stop room is not "work", and the stop must
/// prove itself empty and LEAVE rather than sit there watching a
/// townsman. Same wedge, same timeout, new cause. The latch behaviour
/// itself is covered at unit level by
/// `bot.rs::a_refused_attack_clears_the_engaged_latch` and friends,
/// which drive an unpainted room and so are unaffected — refusals still
/// reach magenta monsters for reasons other than the crime gate.
#[tokio::test]
async fn a_passive_occupant_does_not_hang_the_stop() {
    let server = start_with(world_with_refused_monster()).await;
    let session = logged_in(server.local_addr(), "Persist").await;

    let bot = BotConfig {
        auto_combat: true,
        max_hp: 0,
        ..BotConfig::default()
    };
    // The subject is the refused beetle AT THE STOP; sighting off keeps
    // the Yard rat out of the kill count (the leg fights it otherwise).
    let cfg = FarmConfig {
        fight_while_travelling: false,
        ..farm_config(&["1/3"], 1)
    };
    let run = farm(&session, bot, cfg);
    let (end, stats) = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("the stop hung on a refused attack")
        .expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone);
    assert_eq!(stats.loops, 1);
    assert_eq!(
        stats.kills, 0,
        "the beetle is painted passive; a kill means the colour gate stopped working: {stats:?}"
    );
}

/// The profile toggle still works on the SERVER — turning warnings off
/// makes the crime gate stop refusing — but it is no longer observable
/// through a farm run, because the bot declines the passive beetle
/// before the gate is ever consulted. Driven by hand for that reason.
///
/// (Before the colour gate this was a farm run asserting `kills >= 1`.)
#[tokio::test]
async fn the_evil_warning_toggle_stops_the_board_refusing() {
    let server = start_with(world_with_refused_monster()).await;
    let session = logged_in_with(server.local_addr(), "Unwarned", true).await;
    let t = Duration::from_secs(10);

    session.send("n");
    session.expect("Training Yard", t).await.expect("walked north");
    session.send("e");
    session.expect("Rat Cellar", t).await.expect("walked east");
    session
        .expect("giant beetle", t)
        .await
        .expect("a beetle should be standing here at boot");

    session.send("a beetle");
    let refused = tokio::time::timeout(
        Duration::from_secs(3),
        session.expect(mud_core::crime::WARN_ON_EVIL_REFUSAL, Duration::from_secs(3)),
    )
    .await;
    assert!(
        matches!(refused, Ok(Err(_)) | Err(_)),
        "with warnings off the board must NOT refuse the swing"
    );
}

/// A finished run must not leave the character standing in the lair.
/// This is the linkdead hazard: nobody is driving once the runner stops,
/// and a monster room is the worst place to be left.
#[tokio::test]
async fn the_run_walks_home_when_it_finishes() {
    let server = start().await;
    let session = logged_in(server.local_addr(), "Homer").await;

    let bot = BotConfig {
        auto_combat: true,
        max_hp: 0,
        ..BotConfig::default()
    };
    let mut cfg = farm_config(&["1/2"], 1);
    // The Town Gates: off the circuit, and nothing spawns there.
    cfg.finish_at = Some("1/1".into());

    let graph = Arc::new(client_graph());
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let (end, _stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("farm run");
    assert_eq!(end, FarmEnd::LoopsDone);

    mud_client::farm::go_to_finish(&session, graph, &plan, &cfg)
        .await
        .expect("should have walked home");

    assert_eq!(
        session
            .state()
            .borrow()
            .room
            .as_ref()
            .map(|r| r.name.clone()),
        Some("Town Gates".to_string()),
        "the character should be standing in the finish room"
    );
}

/// The regression for the death-detection hang. Killing a monster whose
/// death line is ordinary prose used to leave the bot latched on the
/// corpse, and `farm_stop` reads a latched bot as a fight in progress —
/// so it stopped poking the room and stopped counting the stop as idle,
/// and the run never ended. The timeout is what turns that back into a
/// failure instead of a wedged suite.
#[tokio::test]
async fn a_prose_death_line_does_not_wedge_the_stop() {
    let server = start_with(world_with_prose_death()).await;
    let session = logged_in(server.local_addr(), "Bugger").await;

    let bot = BotConfig {
        auto_combat: true,
        max_hp: 0,
        ..BotConfig::default()
    };
    // The subject is the prose-death filthbug at the stop; sighting off
    // keeps the Yard rat's ordinary death out of the exact kill count.
    let cfg = FarmConfig {
        fight_while_travelling: false,
        ..farm_config(&["1/3"], 1)
    };
    let graph = Arc::new(client_graph());
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None),
    )
    .await
    .expect("the stop wedged on a kill it did not recognise")
    .expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone);
    // The filthbug dies with prose and never says "falls to the ground",
    // which is precisely what the kill counter used to look for — this
    // kill scored zero. Counting the experience award instead is what
    // makes it visible, and this is the case that distinguishes the two.
    assert_eq!(
        stats.kills, 1,
        "a prose death paid experience but was not counted: {stats:?}"
    );
}

/// The multi-monster stop: three in the room, respawn blocked, so the
/// runner has to stay until all three are dead.
///
/// Every other fixture room holds exactly ONE monster. That is why the
/// runner could ship a bug that walked out of a room with three things
/// still standing in it — with one per room the stop really was finished
/// after the only kill, and no test could tell a correct stop from one
/// that left early.
///
/// Honest about what this does and does not prove: it passes on the
/// pre-refactor runner too, verified by restoring it. That symptom had
/// already been patched, by the fourth patch in the series that led
/// here. So this is a guard against the class coming back, not a
/// demonstration of a live bug — it closes the coverage gap that let the
/// bug ship, and it is the reason the next such patch would be caught.
#[tokio::test]
async fn it_does_not_walk_out_on_a_room_it_never_saw_empty() {
    let server = start_with(world_with_three_monsters()).await;
    let session = logged_in(server.local_addr(), "Thorough").await;

    let bot = BotConfig {
        auto_combat: true,
        max_hp: 0,
        ..BotConfig::default()
    };
    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(
            &session,
            Arc::new(client_graph()),
            &FarmPlan::build(&farm_config(&["1/2"], 1), &Arc::new(client_graph())).expect("plan"),
            &bot,
            &farm_config(&["1/2"], 1),
            None,
        ),
    )
    .await
    .expect("the stop never ended")
    .expect("farm run");

    assert_eq!(end, FarmEnd::LoopsDone);
    assert_eq!(
        stats.kills, 3,
        "left the stop with monsters still listed under \"Also here:\": {stats:?}"
    );
}
