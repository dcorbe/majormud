//! Navigator tests: verified step-by-step movement against the
//! in-process mud-server. Every step is confirmed by parsing the
//! destination room name — never blind (monsters get free attacks on
//! movement; desyncs must surface immediately).

use std::sync::Arc;

use mud_client::dialect::{self, Target};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::events::{Event, RoomView};
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
            false,
        )
        .await
        .expect("navigate gates -> market");

    assert_eq!(at.at, RoomId { map: 1, room: 3 });
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
            false,
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
            false,
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
        light: 0,
        ..Default::default()
    };
    gates.exits[Direction::East as usize] = Some(ExitEdge {
        dest: RoomId { map: 1, room: 3 },
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    RoomGraph::from_rooms(vec![
        (RoomId { map: 1, room: 1 }, gates),
        (
            RoomId { map: 1, room: 3 },
            GraphRoom {
                name: "Market Street".into(),
                exits: Default::default(),
                light: 0,
                ..Default::default()
            },
        ),
    ])
}

/// A phantom exit — the graph believes in one the board does not — is
/// answered "There is no exit in that direction!", which is INFORMATION,
/// not silence: the walk is not where it thinks it is. It used to be
/// treated as a step that never landed, so every one cost a full
/// deadline and the walk kept trying doors that were not there. Now the
/// room is asked and the answer re-localizes.
///
/// It still fails here, because the graph's room 3 is unreachable on
/// this board however you localize — but it fails as a DESYNC, naming
/// what it saw, and without waiting out the clock.
#[tokio::test]
async fn a_phantom_exit_relocalizes_instead_of_waiting_out_the_clock() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(
        Arc::new(graph_with_a_phantom_exit()),
        NavConfig {
            step_timeout_ms: 200,
            ..NavConfig::default()
        },
    );

    let started = std::time::Instant::now();
    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut NoGuard,
            false,
        )
        .await
        .expect_err("the board has no east exit here");

    assert!(
        matches!(err.kind, NavErrorKind::Desync { .. }),
        "a phantom exit is a disagreement, not a timeout: {:?}",
        err.kind
    );
    assert_eq!(err.at, RoomId { map: 1, room: 1 });
    // The point of reading the board's answer: no deadline was waited
    // out, and the step timeout here is only 200ms.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
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
            false,
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
            false,
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
            ..NavConfig::default()
        },
    );

    let started = std::time::Instant::now();
    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut TripsOnLine,
            false,
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
            false,
        )
        .await
        .expect("navigate gates -> market");

    assert_eq!(at.at, RoomId { map: 1, room: 3 });
}

/// Sights a named room, but ONLY through the attributed-arrival hook —
/// `on_event` stays quiet, which is the whole point of the split: the
/// unfiltered stream is full of stale and foreign renders.
struct SightsRoom(&'static str);

impl TravelGuard for SightsRoom {
    fn on_event(&mut self, _ev: &Event) -> Option<Interrupt> {
        None
    }
    fn on_room(&mut self, room: &RoomView) -> Option<Interrupt> {
        (room.name == self.0).then(|| Interrupt::Sighted { room: room.clone() })
    }
}

/// The live incident this pins: a leg walked through "Also here: angry
/// kobold thief, thin giant rat, large kobold thief." and kept sending
/// steps while they attacked. A sighting must hand the walk back AT the
/// room the block described, with the block carried in the interrupt so
/// the caller can act on the evidence it already has.
#[tokio::test]
async fn a_sighted_room_is_reported_at_the_room_the_block_described() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());

    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut SightsRoom("Town Square"),
            false,
        )
        .await
        .expect_err("the guard sighted something");

    match &err.kind {
        NavErrorKind::Interrupted(Interrupt::Sighted { room }) => {
            assert_eq!(room.name, "Town Square");
        }
        other => panic!("expected Sighted, got {other:?}"),
    }
    assert_eq!(err.at, RoomId { map: 1, room: 2 });
    let state = session.state().borrow().clone();
    assert_eq!(
        state.room.as_ref().map(|r| r.name.as_str()),
        Some("Town Square"),
        "the walk must stop where it sighted, not carry on"
    );
}

/// A sighting at the destination is the stop's own arrival: the walk
/// still hands back (with `at` = the target), so the caller can start
/// the stop from the block instead of re-asking.
#[tokio::test]
async fn a_sighting_on_the_last_step_still_hands_back() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());

    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut SightsRoom("Market Street"),
            false,
        )
        .await
        .expect_err("the guard sighted at the destination");

    assert!(matches!(
        err.kind,
        NavErrorKind::Interrupted(Interrupt::Sighted { .. })
    ));
    assert_eq!(err.at, RoomId { map: 1, room: 3 });
}

/// The attribution pin. An answer to a command the walk is NOT waiting
/// on — here a look sent before the walk began — must never reach the
/// sighting hook, however truthfully it names a room. Every desync this
/// family ever had came from believing somebody else's render.
#[tokio::test]
async fn a_stale_attributed_answer_is_not_a_sighting() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(Arc::new(client_graph("Market Street")), NavConfig::default());

    // The board will answer this look with a Town Gates block while the
    // first step is in flight; its answer carries the look's own id.
    session.send("look");

    let at = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut SightsRoom("Town Gates"),
            false,
        )
        .await
        .expect("a stale answer must not sight");
    assert_eq!(at.at, RoomId { map: 1, room: 3 });
}

/// A recovery look (phantom exit -> "no exit" -> re-ask) answers with
/// the room we are still standing in. If the guard sights there, the
/// hand-back must name where we STAND — re-localization matched the
/// block to `current` before the arming was honoured.
#[tokio::test]
async fn a_sighting_on_a_recovery_look_reports_where_we_stand() {
    let server = start().await;
    let session = logged_in_session(server.local_addr()).await;
    let nav = Navigator::new(
        Arc::new(graph_with_a_phantom_exit()),
        NavConfig {
            step_timeout_ms: 200,
            ..NavConfig::default()
        },
    );

    let err = nav
        .goto(
            &session,
            RoomId { map: 1, room: 1 },
            RoomId { map: 1, room: 3 },
            &mut SightsRoom("Town Gates"),
            false,
        )
        .await
        .expect_err("the recovery look names the sighted room");

    assert!(
        matches!(
            err.kind,
            NavErrorKind::Interrupted(Interrupt::Sighted { .. })
        ),
        "expected Sighted, got {:?}",
        err.kind
    );
    assert_eq!(err.at, RoomId { map: 1, room: 1 });
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

// ---------------------------------------------------------------------
// localize_view: the same question, answered from a whole room block.
//
// The one-hop rule above is right when all you have is a name, because
// names genuinely repeat: Newhaven alone has two rooms called "Newhaven,
// Narrow Road" (1/2146 and 1/2151). But a room block carries its exits
// too, and those separate the twins -- 1/2146 shows north/east/west/down
// where 1/2151 shows only north. With that in hand a global search is
// safe, which is what lets a character work out where it is after being
// moved further than one step.
// ---------------------------------------------------------------------

fn twin_graph() -> RoomGraph {
    // Two rooms sharing a name, told apart only by their exits, plus a
    // third far from both so "far away" is a real distance.
    let mk = |name: &str, exits: &[(Direction, RoomId)]| {
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
                requirement: ExitRequirement::None,
            });
        }
        r
    };
    let a = RoomId { map: 1, room: 1 };
    let b = RoomId { map: 1, room: 2 };
    let twin_wide = RoomId { map: 1, room: 10 };
    let twin_narrow = RoomId { map: 1, room: 11 };
    RoomGraph::from_rooms(vec![
        (a, mk("Town Gates", &[(Direction::North, b)])),
        (b, mk("Town Square", &[(Direction::South, a)])),
        (
            twin_wide,
            mk(
                "Narrow Road",
                &[(Direction::North, b), (Direction::East, a), (Direction::Down, a)],
            ),
        ),
        (twin_narrow, mk("Narrow Road", &[(Direction::North, b)])),
    ])
}

fn view(name: &str, exits: &[&str]) -> RoomView {
    RoomView {
        name: name.into(),
        exits: exits.iter().map(|s| (*s).to_string()).collect(),
        also_here: vec![],
        items: vec![],
        also_here_sgr: Vec::new(),
    }
}

/// Displaced further than one hop: the name alone is not enough, but the
/// exits pin it down.
#[test]
fn localize_view_finds_a_room_that_is_far_away() {
    let nav = Navigator::new(Arc::new(twin_graph()), NavConfig::default());
    assert_eq!(
        nav.localize_view(
            RoomId { map: 1, room: 1 },
            &view("Narrow Road", &["north", "east", "down"])
        ),
        Some(RoomId { map: 1, room: 10 })
    );
}

/// The twin with the same name and a different exit set is not confused
/// for it. This is the case the one-hop rule was protecting against.
#[test]
fn localize_view_tells_same_named_rooms_apart_by_their_exits() {
    let nav = Navigator::new(Arc::new(twin_graph()), NavConfig::default());
    assert_eq!(
        nav.localize_view(RoomId { map: 1, room: 1 }, &view("Narrow Road", &["north"])),
        Some(RoomId { map: 1, room: 11 })
    );
}

/// Exits render as display tokens, not commands -- "closed door north",
/// and for trapdoors "above"/"below" rather than up/down. Localization
/// has to read them the way the board writes them.
#[test]
fn localize_view_reads_display_tokens_not_commands() {
    let nav = Navigator::new(Arc::new(twin_graph()), NavConfig::default());
    assert_eq!(
        nav.localize_view(
            RoomId { map: 1, room: 1 },
            &view(
                "Narrow Road",
                &["closed door north", "open gate east", "trap door below"]
            )
        ),
        Some(RoomId { map: 1, room: 10 })
    );
}

/// When the exits cannot separate the candidates, refuse. Guessing is
/// exactly what verified navigation exists to prevent.
#[test]
fn localize_view_refuses_when_still_ambiguous() {
    let mk = |name: &str| GraphRoom {
        name: name.into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    // Start somewhere that is neither twin and adjacent to neither, so
    // the cheap one-hop answer cannot apply and the exits are genuinely
    // all there is to go on.
    let graph = RoomGraph::from_rooms(vec![
        (RoomId { map: 1, room: 1 }, mk("Somewhere Else")),
        (RoomId { map: 1, room: 2 }, mk("Twin")),
        (RoomId { map: 1, room: 3 }, mk("Twin")),
    ]);
    let nav = Navigator::new(Arc::new(graph), NavConfig::default());
    assert_eq!(
        nav.localize_view(RoomId { map: 1, room: 1 }, &view("Twin", &[])),
        None
    );
}

/// Position tracking has to work from a cold start, with no previous
/// room to hop from -- the first block after logging in. localize_view's
/// global search is what makes that possible, so it must not depend on
/// the hint being right.
#[test]
fn localize_view_works_from_a_meaningless_hint() {
    let nav = Navigator::new(Arc::new(twin_graph()), NavConfig::default());
    let nowhere = RoomId { map: 0, room: 0 };
    assert_eq!(
        nav.localize_view(nowhere, &view("Narrow Road", &["north", "east", "down"])),
        Some(RoomId { map: 1, room: 10 })
    );
    assert_eq!(
        nav.localize_view(nowhere, &view("Town Square", &["south"])),
        Some(RoomId { map: 1, room: 2 })
    );
}

/// "The room is very dark - you can't see anything" answers BOTH walking
/// into an unlit room and looking while stood in one. The navigator used
/// to read it as arrival wherever it appeared, including here:
///
/// ```text
/// There is no exit in that direction!
/// [HP=46/MA=11]:look
/// The room is very dark - you can't see anything
/// ```
///
/// The board says the move did not happen, the walk asks where it is,
/// cannot see — and the old code concluded it had ARRIVED at the room it
/// had just been told it could not reach. `current` then ran a room ahead
/// of the character and every later step compounded it, which is the
/// desync that ended live cave-bear runs with a stray `s` into a wall.
///
/// The dark line is identical either way, so the only thing that can tell
/// them apart is what was asked.
#[test]
fn a_dark_room_answers_a_look_and_a_move_with_the_same_line() {
    use mud_client::nav::BlindContext;

    // A direction was sent: dark is the destination reporting itself.
    assert_eq!(
        Navigator::blind_position(BlindContext::AfterMove, "Small Cavern", "Dungeon, Entrance"),
        "Small Cavern"
    );
    // A look was sent, which moves nothing.
    assert_eq!(
        Navigator::blind_position(BlindContext::AfterLook, "Small Cavern", "Dungeon, Entrance"),
        "Dungeon, Entrance",
        "a blind look was read as arrival at the destination"
    );
}

// --- the roam fence ---------------------------------------------------

/// A fence that only filtered DESTINATIONS would look right for a long
/// time and then quietly walk through a wall.
///
/// The roam's rotation already picks its targets using fenced distances,
/// so end to end the runner rarely asks for a route that wants to cross
/// one — which is exactly what makes this worth asserting directly
/// rather than through a run. Here the wall sits on the cheapest path
/// between two rooms the roam is entitled to visit:
///
/// ```text
///   A --east--> WALLED --south--> B          2 steps
///   A --north--> D --north--> E --east--> B  3 steps, the honest way
/// ```
#[test]
fn a_fenced_navigator_routes_around_a_wall() {
    use mud_client::roam::Walls;

    const A: RoomId = RoomId { map: 1, room: 1 };
    const B: RoomId = RoomId { map: 1, room: 2 };
    const WALLED: RoomId = RoomId { map: 1, room: 3 };
    const D: RoomId = RoomId { map: 1, room: 4 };
    const E: RoomId = RoomId { map: 1, room: 5 };

    let room = |name: &str, exits: &[(Direction, RoomId)]| {
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
                requirement: ExitRequirement::None,
            });
        }
        r
    };
    let graph = Arc::new(RoomGraph::from_rooms(vec![
        (A, room("Guard Post", &[(Direction::East, WALLED), (Direction::North, D)])),
        (B, room("Inner Ward", &[(Direction::North, WALLED), (Direction::West, E)])),
        (WALLED, room("Keep", &[(Direction::West, A), (Direction::South, B)])),
        (D, room("Watchtower", &[(Direction::South, A), (Direction::North, E)])),
        (E, room("Barbican", &[(Direction::South, D), (Direction::East, B)])),
    ]));

    let open = Navigator::new(graph.clone(), NavConfig::default());
    assert_eq!(
        open.route_from(A, B),
        Some(vec![Direction::East, Direction::South]),
        "the fixture only means anything if the wall is the SHORT way"
    );

    let fenced = Navigator::new(graph.clone(), NavConfig::default())
        .fenced(Walls::new([WALLED]), A.map);
    assert_eq!(
        fenced.route_from(A, B),
        Some(vec![Direction::North, Direction::North, Direction::East]),
        "a fenced walk takes the long way rather than through the Keep"
    );
}

/// Fencing the only way through answers "no route" — and that is the
/// answer the operator asked for, not a defect.
#[test]
fn a_fence_can_cut_a_room_off_entirely() {
    use mud_client::roam::Walls;

    const A: RoomId = RoomId { map: 1, room: 1 };
    const WALLED: RoomId = RoomId { map: 1, room: 2 };
    const BEYOND: RoomId = RoomId { map: 1, room: 3 };

    let room = |name: &str, exits: &[(Direction, RoomId)]| {
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
                requirement: ExitRequirement::None,
            });
        }
        r
    };
    let graph = Arc::new(RoomGraph::from_rooms(vec![
        (A, room("Guard Post", &[(Direction::North, WALLED)])),
        (WALLED, room("Keep", &[(Direction::South, A), (Direction::North, BEYOND)])),
        (BEYOND, room("Inner Ward", &[(Direction::South, WALLED)])),
    ]));

    let fenced = Navigator::new(graph.clone(), NavConfig::default())
        .fenced(Walls::new([WALLED]), A.map);
    assert_eq!(fenced.route_from(A, BEYOND), None);
    assert_eq!(
        Navigator::new(graph, NavConfig::default()).route_from(A, BEYOND),
        Some(vec![Direction::North, Direction::North]),
        "and without the fence it is an ordinary two-step walk"
    );
}
