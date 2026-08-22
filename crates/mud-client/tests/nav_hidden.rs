//! Hidden exits.
//!
//! Type-6 exits are in the room record but not on the board's "Obvious
//! exits" line, and walking the direction answers "There is no exit in
//! that direction!" until SEARCH reveals them (`re/docs/theft.md` §9):
//!
//! ```text
//! search_for_hidden_exits: roll genrdn(0,100), success iff
//!   roll < max(Perception - 15, 3)
//!   -> state = 4 (found), re-hides in ~5 minutes
//!      "You found an exit to the %s!"   / "...upwards!" / "...downwards!"
//!   -> otherwise
//!      "You notice nothing different to the %s."
//! ```
//!
//! Two consequences the walk has to respect. The roll's floor is 3%, so
//! one attempt is close to useless. And an ALREADY-found exit reports
//! "nothing different" as well — the found state falls through the same
//! else — so searching first and waiting for a find would hang on the
//! rooms where nothing needed doing.
//!
//! Live incident (2026-08-02, walking a ranger to its Silvermere
//! trainer): the router took the hidden south exit out of the Small
//! Alleyway (1/405), the walk sent `s` at a brick wall, and the leg ended
//! `desync: expected "Secret Passage", saw "Small Alleyway"`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, Navigator, NoGuard};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HERE: RoomId = RoomId { map: 1, room: 405 };
const THERE: RoomId = RoomId { map: 1, room: 452 };

#[derive(Default)]
struct SearchLog {
    searches: AtomicUsize,
    moves: AtomicUsize,
}

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=51/MA=9]:")
}

/// A board whose south exit is hidden until the `reveal_on`th search
/// finds it. `usize::MAX` is the exit that never gives.
///
/// The alleyway's own exits line shows only north, exactly as the live
/// room does: a hidden exit is never advertised.
async fn alley_board(reveal_on: usize) -> (std::net::SocketAddr, Arc<SearchLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(SearchLog::default());
    let counter = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut found = reveal_on == 0;
        sock.write_all(room_block("Small Alleyway", "north").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            let echo = format!("\r\n{line}");
            let reply = match line.as_str() {
                "s" => {
                    counter.moves.fetch_add(1, Ordering::SeqCst);
                    if found {
                        room_block("Secret Passage", "north down")
                    } else {
                        "\r\nThere is no exit in that direction!\r\n[HP=51/MA=9]:".to_string()
                    }
                }
                "search s" | "search south" => {
                    let attempt = counter.searches.fetch_add(1, Ordering::SeqCst) + 1;
                    // The found state falls through to the same else as a
                    // failed roll, so a revealed exit keeps saying this.
                    if !found && attempt >= reveal_on {
                        found = true;
                        "\r\nYou found an exit to the south!\r\n[HP=51/MA=9]:".to_string()
                    } else {
                        "\r\nYou notice nothing different to the south.\r\n[HP=51/MA=9]:".to_string()
                    }
                }
                "look" => room_block("Small Alleyway", "north"),
                other => format!("\r\nYou say \"{other}\"\r\n[HP=51/MA=9]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });
    (addr, log)
}

fn graph_with_exit(exit_type: i64) -> Arc<RoomGraph> {
    let mut here = GraphRoom {
        name: "Small Alleyway".into(),
        ..Default::default()
    };
    here.exits[Direction::South as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type,
        command: None,
        requirement: ExitRequirement::from_exit_type(exit_type, 0),
    });
    let mut there = GraphRoom {
        name: "Secret Passage".into(),
        ..Default::default()
    };
    there.exits[Direction::North as usize] = Some(ExitEdge {
        dest: HERE,
        exit_type,
        command: None,
        requirement: ExitRequirement::from_exit_type(exit_type, 0),
    });
    Arc::new(RoomGraph::from_rooms(vec![(HERE, here), (THERE, there)]))
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

fn nav(graph: Arc<RoomGraph>) -> Navigator {
    Navigator::new(
        graph,
        NavConfig {
            step_timeout_ms: 1500,
            ..NavConfig::default()
        },
    )
}

/// The bug, fixed: the walk searches the direction the graph says is
/// hidden instead of sending the direction again until it desyncs.
#[tokio::test]
async fn a_hidden_exit_is_searched_until_it_is_found() {
    let (addr, log) = alley_board(3).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(6));

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("the third roll reveals it");

    assert_eq!(at.at, THERE);
    assert_eq!(log.searches.load(Ordering::SeqCst), 3, "one roll is not a search");
}

/// A hidden exit that is ALREADY found — somebody searched it minutes ago
/// and the re-hide ticker has not fired — simply walks. This is why the
/// search runs on the board's refusal rather than ahead of the step:
/// searching a found exit reports "nothing different" forever, so a
/// search-first walk would burn its whole budget on a step that needed
/// nothing.
#[tokio::test]
async fn an_already_revealed_exit_is_walked_without_searching() {
    let (addr, log) = alley_board(0).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(6));

    let at = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("nothing was in the way");

    assert_eq!(at.at, THERE);
    assert_eq!(log.searches.load(Ordering::SeqCst), 0, "no search was owed");
    assert_eq!(log.moves.load(Ordering::SeqCst), 1, "one step, first time");
}

/// The graph decides what is hidden, not the board's refusal. A plain
/// exit the board denies means the walk is not where it believes, and the
/// answer to that is re-localizing — searching a wall the graph never
/// called hidden would spend a hundred commands hiding a desync.
#[tokio::test]
async fn a_refused_plain_exit_is_never_searched() {
    let (addr, log) = alley_board(usize::MAX).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(0));

    let result = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");

    assert!(result.is_err(), "a plain exit the board denies is a desync");
    assert_eq!(log.searches.load(Ordering::SeqCst), 0, "not hidden: must not search");
}

/// SEARCH breaks hide and sneak and costs a command each roll, so it stays
/// behind a switch for the same reason bashing does.
#[tokio::test]
async fn searching_can_be_switched_off() {
    let (addr, log) = alley_board(3).await;
    let session = session_for(addr).await;
    let n = Navigator::new(
        graph_with_exit(6),
        NavConfig {
            step_timeout_ms: 1500,
            search_hidden: false,
            ..NavConfig::default()
        },
    );

    let result = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");

    assert!(result.is_err(), "a hidden exit with searching off is a dead end");
    assert_eq!(log.searches.load(Ordering::SeqCst), 0, "must not search when switched off");
}

/// An exit that never reveals ends in a bounded, diagnosable error rather
/// than a hang or an unbounded command flood.
#[tokio::test]
async fn an_exit_that_never_reveals_fails_cleanly_within_the_roll_budget() {
    let (addr, log) = alley_board(usize::MAX).await;
    let session = session_for(addr).await;
    let n = nav(graph_with_exit(6));

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");

    assert!(result.is_err(), "{result:?}");
    let searches = log.searches.load(Ordering::SeqCst);
    assert!(
        (1..=100).contains(&searches),
        "unbounded searching: {searches} rolls"
    );
}

/// The guard is the health backstop, and a search loop is exactly as long
/// as a bash loop. A monster swinging while the character rummages at a
/// wall has to take the walk back.
#[tokio::test]
async fn a_guard_interrupt_breaks_the_search_loop() {
    struct ArmOnHit;
    impl mud_client::nav::TravelGuard for ArmOnHit {
        fn on_event(&mut self, ev: &mud_client::events::Event) -> Option<mud_client::nav::Interrupt> {
            match ev {
                mud_client::events::Event::CombatHit { .. } => Some(
                    mud_client::nav::Interrupt::Attacked { by: "kobold".into() },
                ),
                _ => None,
            }
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let searches = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&searches);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Small Alleyway", "north").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            let echo = format!("\r\n{line}");
            let reply = match line.as_str() {
                "s" => "\r\nThere is no exit in that direction!\r\n[HP=51/MA=9]:".to_string(),
                "search s" | "search south" => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    "\r\nThe kobold thief slashes you for 5 damage!\r\nYou notice nothing different to the south.\r\n[HP=46/MA=9]:"
                        .to_string()
                }
                other => format!("\r\nYou say \"{other}\"\r\n[HP=51/MA=9]:"),
            };
            sock.write_all(format!("{echo}{reply}").as_bytes()).await.unwrap();
        }
    });

    let session = session_for(addr).await;
    let n = nav(graph_with_exit(6));

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        n.goto(&session, HERE, THERE, &mut ArmOnHit),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the guard must take the walk back");
    assert!(
        matches!(
            err.kind,
            mud_client::nav::NavErrorKind::Interrupted(mud_client::nav::Interrupt::Attacked { .. })
        ),
        "{err:?}"
    );
    assert!(
        searches.load(Ordering::SeqCst) <= 2,
        "kept searching under fire: {} rolls",
        searches.load(Ordering::SeqCst)
    );
}
