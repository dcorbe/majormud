//! Buttons and levers: exits a phrase opens from here or from another
//! room.
//!
//! Live incident, 2026-09-04: a roam walked to 1/506 Secret Passage and
//! sent `search s` two hundred times. The south wall is a type 6 exit
//! with state 16, which no search roll clears, and the room's west slot
//! is a type 12 remote action: `push button`, action 1 on exit 1. The
//! Crypt pair captured 2026-09-05 is the two room shape: `pull lever` in
//! 1/1044 and 1/1038 each answer "You pull the lever. Off in the
//! distance you hear a small click." and 1/1056 then lists "dark
//! passageway north".
//!
//! The board here is a small world with a position, so an inner walk to
//! a lever room and back is a real walk: a direction the current room
//! lacks is refused, and the concealed exit is refused until the pulls
//! are in. The live board says an unknown phrase out loud rather than
//! rejecting it, and so does this one.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{Interrupt, NavConfig, NavErrorKind, Navigator, NoGuard, TravelGuard};
use mud_client::profile::Profile;
use mud_client::puzzle::{Puzzle, PuzzleAction};
use mud_client::session::Session;
use mud_core::content::{Direction, ItemId, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PASSAGE: RoomId = RoomId { map: 1, room: 506 };
const HALLWAY: RoomId = RoomId { map: 1, room: 507 };
const HALL: RoomId = RoomId { map: 1, room: 1056 };
const LEVER_A: RoomId = RoomId { map: 1, room: 1044 };
const LEVER_B: RoomId = RoomId { map: 1, room: 1038 };
const DARK_PASSAGE: RoomId = RoomId { map: 1, room: 1063 };

const PROMPT: &str = "\r\n[HP=51/MA=9]:";
const CLICK: &str = "You pull the lever. Off in the distance you hear a small click.";
const PUSHED: &str = "You push the button.";

/// One room as the scripted board knows it.
struct BoardRoom {
    name: &'static str,
    /// Plain exits: the command word, the word the exits line shows,
    /// and the room it leads to.
    exits: Vec<(&'static str, &'static str, &'static str)>,
    /// The concealed exit, refused until the pulls are in.
    concealed: Option<(&'static str, &'static str, &'static str)>,
    /// Whether a lever phrase does anything here. Elsewhere it is said
    /// aloud.
    lever: bool,
    /// A line printed before the lever reply, for the guard tests.
    ambush: Option<&'static str>,
    /// A line printed after the lever reply, for the guard tests. The
    /// walk's `speak` has already returned by the time this lands, so
    /// only the drain after the plan can see it.
    ambush_after: Option<&'static str>,
}

struct World {
    rooms: BTreeMap<&'static str, BoardRoom>,
    /// Pulls needed before the concealed exit opens.
    pulls_needed: usize,
    /// What a lever phrase answers.
    reply: &'static str,
}

#[derive(Default)]
struct BoardLog {
    lines: Mutex<Vec<String>>,
    pulls: AtomicUsize,
}

impl BoardLog {
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    fn count(&self, line: &str) -> usize {
        self.lines().iter().filter(|l| l.as_str() == line).count()
    }
}

fn render(world: &World, at: &str, opened: bool) -> String {
    let room = &world.rooms[at];
    let mut shown: Vec<&str> = room.exits.iter().map(|(_, shown, _)| *shown).collect();
    if opened {
        if let Some((_, label, _)) = room.concealed {
            shown.push(label);
        }
    }
    format!(
        "\r\n\x1b[1;36m{}\r\nObvious exits: {}{PROMPT}",
        room.name,
        shown.join(" ")
    )
}

async fn world_board(world: World, start: &'static str) -> (std::net::SocketAddr, Arc<BoardLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(BoardLog::default());
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut at = start;
        let opened = |seen: &BoardLog| seen.pulls.load(Ordering::SeqCst) >= world.pulls_needed;
        sock.write_all(render(&world, at, opened(&seen)).as_bytes())
            .await
            .unwrap();
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim().to_lowercase();
                seen.lines.lock().unwrap().push(line.clone());
                let room = &world.rooms[at];
                let reply = if line == "look" {
                    render(&world, at, opened(&seen))
                } else if line == "pull lever" || line == "push button" {
                    if room.lever {
                        seen.pulls.fetch_add(1, Ordering::SeqCst);
                        let before = match room.ambush {
                            Some(hit) => format!("\r\n{hit}"),
                            None => String::new(),
                        };
                        match room.ambush_after {
                            Some(hit) => {
                                format!("{before}\r\n{}\r\n{hit}{PROMPT}", world.reply)
                            }
                            None => format!("{before}\r\n{}{PROMPT}", world.reply),
                        }
                    } else {
                        format!("\r\nYou say \"{line}\"{PROMPT}")
                    }
                } else if let Some(dest) = room
                    .exits
                    .iter()
                    .find(|(cmd, _, _)| *cmd == line)
                    .map(|(_, _, dest)| *dest)
                {
                    at = dest;
                    render(&world, at, opened(&seen))
                } else if let Some((_, _, dest)) = room.concealed.filter(|(cmd, _, _)| *cmd == line) {
                    if opened(&seen) {
                        at = dest;
                        render(&world, at, opened(&seen))
                    } else {
                        format!("\r\nThere is no exit in that direction!{PROMPT}")
                    }
                } else if line.starts_with("search ") {
                    format!("\r\nYou notice nothing different.{PROMPT}")
                } else if line.starts_with("open ") {
                    format!("\r\nThere is no door in that direction!{PROMPT}")
                } else if [
                    "n", "s", "e", "w", "ne", "nw", "se", "sw", "u", "d",
                ]
                .contains(&line.as_str())
                {
                    format!("\r\nThere is no exit in that direction!{PROMPT}")
                } else {
                    format!("\r\nYou say \"{line}\"{PROMPT}")
                };
                sock.write_all(format!("\r\n{line}{reply}").as_bytes())
                    .await
                    .unwrap();
            }
        }
    });
    (addr, log)
}

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

fn puzzle_edge(dest: RoomId, puzzle: Puzzle) -> ExitEdge {
    ExitEdge {
        dest,
        exit_type: 6,
        command: None,
        requirement: ExitRequirement::Puzzle(puzzle),
    }
}

fn button(item: Option<ItemId>) -> Puzzle {
    Puzzle {
        word: 16,
        actions: vec![PuzzleAction {
            room: PASSAGE,
            number: 1,
            phrases: vec!["push button".into()],
            item,
            reply: Some(PUSHED.into()),
            hops: Some(0),
        }],
    }
}

/// 1/506 and the hallway behind its south wall.
fn passage_graph(item: Option<ItemId>) -> Arc<RoomGraph> {
    let mut passage = room("Secret Passage", &[]);
    passage.exits[Direction::South as usize] = Some(puzzle_edge(HALLWAY, button(item)));
    Arc::new(RoomGraph::from_rooms(vec![
        (PASSAGE, passage),
        (HALLWAY, room("Wooden Hallway", &[(Direction::North, PASSAGE)])),
    ]))
}

fn passage_world(pulls_needed: usize, lever: bool) -> World {
    passage_world_with(pulls_needed, lever, None)
}

/// As above, with a line the Secret Passage prints AFTER the button's
/// reply.
fn passage_world_with(
    pulls_needed: usize,
    lever: bool,
    ambush_after: Option<&'static str>,
) -> World {
    let mut rooms = BTreeMap::new();
    rooms.insert(
        "Secret Passage",
        BoardRoom {
            name: "Secret Passage",
            exits: Vec::new(),
            concealed: Some(("s", "south", "Wooden Hallway")),
            lever,
            ambush: None,
            ambush_after,
        },
    );
    rooms.insert(
        "Wooden Hallway",
        BoardRoom {
            name: "Wooden Hallway",
            exits: vec![("n", "north", "Secret Passage")],
            concealed: None,
            lever: false,
            ambush: None,
            ambush_after: None,
        },
    );
    World {
        rooms,
        pulls_needed,
        reply: PUSHED,
    }
}

/// The Crypt: the hallway, an alcove each side holding a lever, and the
/// passage north that both levers open.
fn crypt_graph() -> Arc<RoomGraph> {
    let lever = |room, number| PuzzleAction {
        room,
        number,
        phrases: vec!["pull lever".into()],
        item: None,
        reply: Some(CLICK.into()),
        hops: Some(1),
    };
    let mut hall = room(
        "Crypt, Stone Hallway",
        &[(Direction::East, LEVER_A), (Direction::West, LEVER_B)],
    );
    hall.exits[Direction::North as usize] = Some(puzzle_edge(
        DARK_PASSAGE,
        Puzzle {
            word: 48,
            actions: vec![lever(LEVER_B, 2), lever(LEVER_A, 1)],
        },
    ));
    Arc::new(RoomGraph::from_rooms(vec![
        (HALL, hall),
        (LEVER_A, room("Crypt, East Alcove", &[(Direction::West, HALL)])),
        (LEVER_B, room("Crypt, West Alcove", &[(Direction::East, HALL)])),
        (DARK_PASSAGE, room("Crypt, Dark Passage", &[(Direction::South, HALL)])),
    ]))
}

fn crypt_world(ambush_in_west: Option<&'static str>) -> World {
    let mut rooms = BTreeMap::new();
    rooms.insert(
        "Crypt, Stone Hallway",
        BoardRoom {
            name: "Crypt, Stone Hallway",
            exits: vec![("e", "east", "Crypt, East Alcove"), ("w", "west", "Crypt, West Alcove")],
            concealed: Some(("n", "dark passageway north", "Crypt, Dark Passage")),
            lever: false,
            ambush: None,
            ambush_after: None,
        },
    );
    rooms.insert(
        "Crypt, East Alcove",
        BoardRoom {
            name: "Crypt, East Alcove",
            exits: vec![("w", "west", "Crypt, Stone Hallway")],
            concealed: None,
            lever: true,
            ambush: None,
            ambush_after: None,
        },
    );
    rooms.insert(
        "Crypt, West Alcove",
        BoardRoom {
            name: "Crypt, West Alcove",
            exits: vec![("e", "east", "Crypt, Stone Hallway")],
            concealed: None,
            lever: true,
            ambush: ambush_in_west,
            ambush_after: None,
        },
    );
    rooms.insert(
        "Crypt, Dark Passage",
        BoardRoom {
            name: "Crypt, Dark Passage",
            exits: vec![("s", "south", "Crypt, Stone Hallway")],
            concealed: None,
            lever: false,
            ambush: None,
            ambush_after: None,
        },
    );
    World {
        rooms,
        pulls_needed: 2,
        reply: CLICK,
    }
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
        bank: Default::default(),
    };
    Session::connect(&profile, None).await.unwrap()
}

fn nav(graph: Arc<RoomGraph>) -> Navigator {
    Navigator::new(
        graph,
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors: false,
            ..NavConfig::default()
        },
    )
}

/// The bug, fixed: the south wall of 1/506 is refused, the button in
/// the same room is pushed, and the direction then lands. Not one
/// search is spent.
#[tokio::test]
async fn a_same_room_button_is_pushed_when_the_wall_refuses() {
    let (addr, log) = world_board(passage_world(1, true), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("the button opens the wall");

    assert_eq!(at.at, HALLWAY);
    assert_eq!(log.lines(), vec!["s", "push button", "s"]);
}

/// A passage somebody opened minutes ago simply walks. Sending the
/// direction first is what makes that free.
#[tokio::test]
async fn an_already_open_passage_is_walked_without_a_word() {
    let (addr, log) = world_board(passage_world(0, true), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("nothing was in the way");

    assert_eq!(at.at, HALLWAY);
    assert_eq!(log.lines(), vec!["s"]);
    assert_eq!(log.pulls.load(Ordering::SeqCst), 0);
}

/// The Crypt pair: the plan pulls action 2 first, so the walk goes west,
/// pulls, crosses to the east alcove, pulls, comes back to the hallway
/// and only then sends north again.
#[tokio::test]
async fn a_lever_pair_is_pulled_higher_number_first_and_the_walk_comes_back() {
    let (addr, log) = world_board(crypt_world(None), "Crypt, Stone Hallway").await;
    let session = session_for(addr).await;
    let n = nav(crypt_graph());

    let at = tokio::time::timeout(
        Duration::from_secs(30),
        n.goto(&session, HALL, DARK_PASSAGE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("two pulls open the passage");

    assert_eq!(at.at, DARK_PASSAGE);
    assert_eq!(
        log.lines(),
        vec!["n", "w", "pull lever", "e", "e", "pull lever", "w", "n"]
    );
    assert_eq!(log.pulls.load(Ordering::SeqCst), 2);
}

/// A wall that stays shut after a whole plan is tried once more, since
/// the re-hide can beat a long detour, and then reported as a puzzle
/// failure from the room the walk still stands in. Never a desync, and
/// never a search.
#[tokio::test]
async fn a_passage_that_stays_shut_is_reported_after_two_plans() {
    let (addr, log) = world_board(passage_world(usize::MAX, true), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the wall never opens");

    assert!(matches!(err.kind, NavErrorKind::Puzzle { .. }), "{err:?}");
    assert_eq!(err.at, PASSAGE);
    assert_eq!(log.count("push button"), 2, "{:?}", log.lines());
    assert_eq!(log.lines().iter().filter(|l| l.starts_with("search")).count(), 0);
}

/// The live board says an unknown phrase out loud. That is the only sign
/// the button did nothing, and no retry can change it, so the step ends
/// at once.
#[tokio::test]
async fn a_phrase_said_aloud_ends_the_step_at_once() {
    let (addr, log) = world_board(passage_world(1, false), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("speech opens nothing");

    assert!(matches!(err.kind, NavErrorKind::Puzzle { .. }), "{err:?}");
    assert_eq!(err.at, PASSAGE);
    assert_eq!(log.count("push button"), 1, "{:?}", log.lines());
}

/// Routing refuses a puzzle whose action needs an item the pack lacks,
/// so the walk never sets out.
#[tokio::test]
async fn a_plan_the_pack_cannot_fill_is_no_route() {
    let (addr, log) = world_board(passage_world(1, true), "Secret Passage").await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(Some(ItemId(500))));

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("no fork, no plan");

    assert!(matches!(err.kind, NavErrorKind::NoRoute), "{err:?}");
    assert!(log.lines().is_empty(), "{:?}", log.lines());
}

/// An interrupt while the character stands in a lever room three rooms
/// from the step it set out on has to report the lever room. A caller
/// that resumes from the step's own room would be routing from a lie.
#[tokio::test]
async fn an_interrupt_in_the_lever_room_reports_the_lever_room() {
    struct ArmOnHit;
    impl TravelGuard for ArmOnHit {
        fn on_event(&mut self, ev: &mud_client::events::Event) -> Option<Interrupt> {
            match ev {
                mud_client::events::Event::CombatHit { .. } => {
                    Some(Interrupt::Attacked { by: "ghoul".into() })
                }
                _ => None,
            }
        }
    }

    let (addr, log) = world_board(
        crypt_world(Some("The crypt ghoul slashes you for 5 damage!")),
        "Crypt, Stone Hallway",
    )
    .await;
    let session = session_for(addr).await;
    let n = nav(crypt_graph());

    let err = tokio::time::timeout(
        Duration::from_secs(30),
        n.goto(&session, HALL, DARK_PASSAGE, &mut ArmOnHit, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the guard takes the walk back");

    assert!(
        matches!(err.kind, NavErrorKind::Interrupted(Interrupt::Attacked { .. })),
        "{err:?}"
    );
    assert_eq!(err.at, LEVER_B, "the walk stands in the west alcove");
    assert_eq!(log.lines(), vec!["n", "w", "pull lever"]);
}

/// A hit that lands AFTER the button's reply is the one nobody has
/// seen: `speak` returned the moment the reply matched, and the tail
/// arrives on the outer receiver alone. The drain that clears the
/// receiver before the step is resent has to show it to the guard
/// rather than throw it away.
#[tokio::test]
async fn a_hit_after_the_reply_is_shown_to_the_guard() {
    struct ArmOnHit;
    impl TravelGuard for ArmOnHit {
        fn on_event(&mut self, ev: &mud_client::events::Event) -> Option<Interrupt> {
            match ev {
                mud_client::events::Event::CombatHit { .. } => {
                    Some(Interrupt::Attacked { by: "ghoul".into() })
                }
                _ => None,
            }
        }
    }

    let (addr, log) = world_board(
        passage_world_with(1, true, Some("The crypt ghoul slashes you for 5 damage!")),
        "Secret Passage",
    )
    .await;
    let session = session_for(addr).await;
    let n = nav(passage_graph(None));

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, PASSAGE, HALLWAY, &mut ArmOnHit, false),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the guard takes the walk back");

    assert!(
        matches!(err.kind, NavErrorKind::Interrupted(Interrupt::Attacked { .. })),
        "{err:?}"
    );
    assert_eq!(err.at, PASSAGE);
    assert_eq!(log.lines(), vec!["s", "push button"]);
}

const GATE_ROOM: RoomId = RoomId { map: 1, room: 20 };
const COURTYARD: RoomId = RoomId { map: 1, room: 21 };

/// A lever on a gate toggles its lock, so the walk pulls it after `open`
/// says locked and before any pick or bash is spent, then opens and
/// walks. The board here is locked until the lever is pulled once.
#[tokio::test]
async fn a_lever_on_a_gate_unlocks_it_before_any_pick_or_bash() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(BoardLog::default());
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let block = |name: &str, exits: &str| {
            format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}{PROMPT}")
        };
        sock.write_all(block("Gate Room", "closed gate north").as_bytes())
            .await
            .unwrap();
        let mut open = false;
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim().to_lowercase();
                seen.lines.lock().unwrap().push(line.clone());
                let unlocked = seen.pulls.load(Ordering::SeqCst) >= 1;
                let reply = match line.as_str() {
                    "n" if open => block("Courtyard", "south"),
                    "n" => format!("\r\nThe gate is closed!{PROMPT}"),
                    "open n" | "open north" if unlocked => {
                        open = true;
                        format!("\r\nThe gate is now open.{PROMPT}")
                    }
                    "open n" | "open north" => format!("\r\nThe gate is locked.{PROMPT}"),
                    "pull lever" => {
                        seen.pulls.fetch_add(1, Ordering::SeqCst);
                        format!("\r\n{CLICK}{PROMPT}")
                    }
                    "look" => block("Gate Room", "closed gate north"),
                    other => format!("\r\nYou say \"{other}\"{PROMPT}"),
                };
                sock.write_all(format!("\r\n{line}{reply}").as_bytes())
                    .await
                    .unwrap();
            }
        }
    });

    let mut gate_room = room("Gate Room", &[]);
    gate_room.exits[Direction::North as usize] = Some(ExitEdge {
        dest: COURTYARD,
        exit_type: 0xb,
        command: None,
        requirement: ExitRequirement::Puzzle(Puzzle {
            word: 0,
            actions: vec![PuzzleAction {
                room: GATE_ROOM,
                number: 0,
                phrases: vec!["pull lever".into()],
                item: None,
                reply: Some(CLICK.into()),
                hops: Some(0),
            }],
        }),
    });
    let graph = Arc::new(RoomGraph::from_rooms(vec![
        (GATE_ROOM, gate_room),
        (COURTYARD, room("Courtyard", &[(Direction::South, GATE_ROOM)])),
    ]));
    let session = session_for(addr).await;
    let n = nav(graph);

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, GATE_ROOM, COURTYARD, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .expect("the lever unlocks the gate");

    assert_eq!(at.at, COURTYARD);
    assert_eq!(log.lines(), vec!["n", "open n", "pull lever", "open n", "n"]);
}
