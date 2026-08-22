//! Working out where a lost character is by walking.
//!
//! Room names repeat: 26,720 rooms carry 1,754 distinct names, and only
//! 5,002 of them (18.7%) are unique by name AND the exits the board
//! lists. So "look and read the name" — which is all
//! [`mud_client::nav::Navigator::localize_view`] can do — answers less
//! than a fifth of the world, and a run that logs back in inside a
//! generic room simply ends (live, 2026-08-02: "Dark Tunnel" is not the
//! stop or any neighbor of it).
//!
//! Walking fixes it, but not by hunting for a unique room — in the
//! Library (212 identical rooms), the Negative Power Plane (138) or
//! Crystal Lake (433) there is no unique room to find. What works is
//! carrying the whole CANDIDATE SET through the moves: step, look, and
//! keep only the candidates whose neighbour in that direction matches
//! what the board just showed. Simulated over every room in the world:
//! 52% resolve in one step, 82% within three, 93% within twelve. The
//! remaining 7% are true mazes and no walk distinguishes them.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mud_client::events::RoomView;
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::lost::{self, Lost};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn graph() -> &'static RoomGraph {
    use std::sync::OnceLock;
    static G: OnceLock<RoomGraph> = OnceLock::new();
    G.get_or_init(|| RoomGraph::load(&db_path()).expect("load room graph"))
}

fn view(name: &str, exits: &[&str]) -> RoomView {
    RoomView {
        name: name.to_string(),
        exits: exits.iter().map(|e| e.to_string()).collect(),
        ..Default::default()
    }
}

// --- the candidate set ------------------------------------------------

/// The live case. Standing in a "Dark Tunnel" showing south and west,
/// the graph can say it is one of exactly two rooms — which is a long
/// way from "I have no idea", and one step from certainty.
#[test]
fn a_generic_room_narrows_to_the_rooms_it_could_be() {
    assert_eq!(
        lost::candidates(graph(), &view("Dark Tunnel", &["south", "west"])),
        vec![
            RoomId { map: 1, room: 790 },
            RoomId { map: 1, room: 1455 }
        ]
    );
}

/// 35 rooms are called "Dark Tunnel". The exits are what cut it to two,
/// so a block with no exit line at all must not pretend to know more
/// than the name says.
#[test]
fn the_name_alone_admits_every_room_that_carries_it() {
    assert_eq!(lost::candidates(graph(), &view("Dark Tunnel", &[])).len(), 35);
}

/// A room the graph has never heard of admits nothing. Saying "no
/// candidates" is the honest answer; guessing the nearest name would put
/// a character somewhere it has never been.
#[test]
fn a_room_the_graph_does_not_know_admits_nothing() {
    assert!(lost::candidates(graph(), &view("Vestibule of Nod", &["north"])).is_empty());
}

/// The board never lists hidden (type 6) or command (type 10) exits, so
/// a room that has one still matches a block that does not mention it.
/// Demanding the full exit set would reject the right room — and the
/// Silvermere Small Alleyway, which has exactly this shape, is a room a
/// character can very easily be standing in.
#[test]
fn an_unlisted_exit_does_not_disqualify_a_room() {
    let alley = RoomId { map: 1, room: 405 };
    assert!(
        lost::candidates(graph(), &view("Small Alleyway", &["north"])).contains(&alley),
        "the hidden south exit is not on the board's exits line"
    );
}

// --- walking it out ---------------------------------------------------

const TWIN_A: RoomId = RoomId { map: 1, room: 10 };
const TWIN_B: RoomId = RoomId { map: 1, room: 20 };
const MOUTH: RoomId = RoomId { map: 1, room: 11 };
const CRYPT: RoomId = RoomId { map: 1, room: 21 };

fn room(name: &str, exits: &[(Direction, RoomId)]) -> GraphRoom {
    let mut r = GraphRoom {
        name: name.into(),
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
}

/// Two rooms the board describes identically, told apart only by what
/// lies north of each.
fn twins() -> RoomGraph {
    RoomGraph::from_rooms(vec![
        (TWIN_A, room("Twisty Passage", &[(Direction::North, MOUTH)])),
        (TWIN_B, room("Twisty Passage", &[(Direction::North, CRYPT)])),
        (MOUTH, room("Cave Mouth", &[(Direction::South, TWIN_A)])),
        (CRYPT, room("Crypt", &[(Direction::South, TWIN_B)])),
    ])
}

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=68/MA=18]:")
}

/// A board that answers every direction with the room the script says.
/// `moves` counts what the walk actually sent.
async fn board(reply: fn(&str) -> String) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let moves = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&moves);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Twisty Passage", "north").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            counter.fetch_add(1, Ordering::SeqCst);
            let echo = format!("\r\n{line}");
            sock.write_all(format!("{echo}{}", reply(&line)).as_bytes())
                .await
                .unwrap();
        }
    });
    (addr, moves)
}

async fn session_for(addr: std::net::SocketAddr) -> Session {
    let profile = Profile {
        target: mud_client::dialect::Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "testuser".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        disable_evil_warnings: false,
        bot: None,
        farm: None,
    };
    Session::connect(&profile, None).await.unwrap()
}

/// One step settles the ambiguity, and the answer is where the walk ENDS
/// — not where it began. The caller routes from there.
#[tokio::test]
async fn one_step_tells_the_twins_apart() {
    let (addr, moves) = board(|line| match line {
        "n" => room_block("Crypt", "south"),
        _ => "\r\n[HP=68/MA=18]:".to_string(),
    })
    .await;
    let session = session_for(addr).await;
    let g = twins();

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        lost::relocalize(&session, &g, &view("Twisty Passage", &["north"]), 12),
    )
    .await
    .expect("relocalize should not hang")
    .expect("one step is enough");

    assert_eq!(at.at, CRYPT, "it reports where it now stands");
    assert_eq!(at.steps, 1, "and what the answer cost");
    assert_eq!(moves.load(Ordering::SeqCst), 1, "one step, no rummaging");
}

/// A room that was never ambiguous costs nothing. Walking a character
/// that already knew where it was would be a bug with a cost.
#[tokio::test]
async fn a_room_it_can_already_place_moves_nothing() {
    let (addr, moves) = board(|_| "\r\n[HP=68/MA=18]:".to_string()).await;
    let session = session_for(addr).await;
    let g = twins();

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        lost::relocalize(&session, &g, &view("Crypt", &["south"]), 12),
    )
    .await
    .expect("relocalize should not hang")
    .expect("Crypt is unique in this world");

    assert_eq!(at.at, CRYPT);
    assert_eq!(at.steps, 0, "the look alone settled it");
    assert_eq!(moves.load(Ordering::SeqCst), 0, "no command was owed");
}

/// A maze of identical rooms cannot be walked out of, so the walk stops
/// at its budget and says how many rooms it is still choosing between —
/// which is the useful thing to print. 1,796 of the shipped rooms are
/// like this; the Library alone is 212 of them.
#[tokio::test]
async fn a_maze_gives_up_inside_its_budget_and_says_so() {
    // Four cells, all "Hall of Mirrors", each north-linked to the next.
    let ids: Vec<RoomId> = (0..4).map(|n| RoomId { map: 1, room: 100 + n }).collect();
    let g = RoomGraph::from_rooms(
        ids.iter()
            .enumerate()
            .map(|(n, id)| {
                (
                    *id,
                    room(
                        "Hall of Mirrors",
                        &[(Direction::North, ids[(n + 1) % ids.len()])],
                    ),
                )
            })
            .collect(),
    );
    let (addr, moves) = board(|line| match line {
        "n" => room_block("Hall of Mirrors", "north"),
        _ => "\r\n[HP=68/MA=18]:".to_string(),
    })
    .await;
    let session = session_for(addr).await;

    let err = tokio::time::timeout(
        Duration::from_secs(30),
        lost::relocalize(&session, &g, &view("Hall of Mirrors", &["north"]), 5),
    )
    .await
    .expect("relocalize should not hang")
    .expect_err("nothing here can be told apart");

    match err {
        Lost::Maze { candidates, .. } => assert_eq!(candidates, 4),
        other => panic!("expected a maze, got {other:?}"),
    }
    assert_eq!(moves.load(Ordering::SeqCst), 5, "the budget, and not a step more");
}

/// A block the graph cannot place at all is not something walking can
/// fix, so it must not spend a single command trying.
#[tokio::test]
async fn an_unknown_room_is_refused_without_walking() {
    let (addr, moves) = board(|_| "\r\n[HP=68/MA=18]:".to_string()).await;
    let session = session_for(addr).await;
    let g = twins();

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        lost::relocalize(&session, &g, &view("Vestibule of Nod", &["north"]), 12),
    )
    .await
    .expect("relocalize should not hang")
    .expect_err("the graph has never heard of it");

    assert!(matches!(err, Lost::Unknown { .. }), "{err:?}");
    assert_eq!(moves.load(Ordering::SeqCst), 0, "walked while it knew nothing");
}
