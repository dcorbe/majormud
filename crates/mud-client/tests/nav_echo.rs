//! The walk under attribution: scripted echoing boards replaying the
//! failure shapes measured in the live captures (stopstate-run5/6, see
//! docs/board-correlation.md). Every script echoes accepted commands
//! like the real board — the reply follows the echo — and every test
//! here is a way the old first-block-wins walk went wrong.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, Navigator, NoGuard};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HERE: RoomId = RoomId { map: 1, room: 1 };
const THERE: RoomId = RoomId { map: 1, room: 2 };

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:")
}

fn graph(exit_type: i64) -> Arc<RoomGraph> {
    let mut here = GraphRoom {
        name: "Guard Post".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    here.exits[Direction::North as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type,
        command: None,
        requirement: ExitRequirement::from_exit_type(exit_type, 0),
    });
    let mut there = GraphRoom {
        name: "Inner Ward".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    there.exits[Direction::South as usize] = Some(ExitEdge {
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

fn nav(g: Arc<RoomGraph>) -> Navigator {
    Navigator::new(
        g,
        NavConfig {
            step_timeout_ms: 1500,
            ..NavConfig::default()
        },
    )
}

/// A board driven by a per-line script: `(matcher, reply)` where the
/// reply is raw bytes already containing whatever echo the scenario
/// wants. Unmatched lines echo + say back. Every line received is
/// logged, so a test can assert what was — and was NOT — sent.
async fn scripted_board(
    script: Vec<(&'static str, String)>,
) -> (
    std::net::SocketAddr,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Vec<String>>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let opens = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&opens);
    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Guard Post", "north").as_bytes())
            .await
            .unwrap();
        let mut used = vec![false; script.len()];
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
                log.lock().unwrap().push(line.clone());
                if line.starts_with("open") || line.starts_with("bash") {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                let hit = script
                    .iter()
                    .enumerate()
                    .find(|(i, (m, _))| !used[*i] && *m == line);
                let reply = match hit {
                    Some((i, (_, r))) => {
                        used[i] = true;
                        r.clone()
                    }
                    None => format!("\r\n{line}\r\nYou say \"{line}\"\r\n[HP=30/MA=0]:"),
                };
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        }
    });
    (addr, opens, received)
}

/// The desync that ended live runs, mechanized: the board front-runs the
/// step's answer with a stale render of the room being left (a prior
/// look's block landing late). The walk must ignore it and land on the
/// block that follows ITS echo.
#[tokio::test]
async fn a_stale_render_before_the_echo_does_not_satisfy_the_step() {
    let (addr, _, _) = scripted_board(vec![(
        "n",
        format!(
            "{}\r\nn{}",
            room_block("Guard Post", "north"), // stale: nobody asked
            room_block("Inner Ward", "south")  // the echoed answer
        ),
    )])
    .await;
    let session = session_for(addr).await;
    let n = nav(graph(0));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("the echoed block is the real arrival");
    assert_eq!(at.at, THERE);
}

/// The run5 double echo: a receipt echo at accept, the reply arriving
/// later behind a SECOND (execution) echo. The duplicate must confirm,
/// never advance — the walk sees one answer and takes one step.
#[tokio::test]
async fn a_double_echoed_step_lands_once() {
    let (addr, _, _) = scripted_board(vec![(
        "n",
        format!(
            "\r\nn\r\n[HP=30/MA=0]:n{}",
            room_block("Inner Ward", "south")
        ),
    )])
    .await;
    let session = session_for(addr).await;
    let n = nav(graph(0));

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect("double echo still lands");
    assert_eq!(at.at, THERE);
}

/// A block with NO echo is unsolicited — somebody else's render. It must
/// never satisfy the step; the deadline hands the failure to goto's
/// machinery instead of silently drifting position.
#[tokio::test]
async fn an_echoless_render_never_satisfies_a_step() {
    let (addr, _, _) = scripted_board(vec![(
        "n",
        room_block("Inner Ward", "south"), // no echo anywhere
    )])
    .await;
    let session = session_for(addr).await;
    let n = nav(graph(0));

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");
    assert!(
        result.is_err(),
        "an unsolicited render satisfied the step: {result:?}"
    );
}

/// _cmd_look's refusal wording ("The door is closed in that direction!")
/// is not a move refusal — _move_user's is "There is a closed door in
/// that direction!". The look wording arriving during a step must not
/// provoke door handling; the step falls to its deadline and the graph
/// (exit_type 0: no door) is believed over a wording that answers a
/// command the walk never sent.
#[tokio::test]
async fn a_look_refusal_wording_provokes_no_door_handling() {
    let (addr, opens, _) = scripted_board(vec![(
        "n",
        "\r\nn\r\nThe door is closed in that direction!\r\n[HP=30/MA=0]:".to_string(),
    )])
    .await;
    let session = session_for(addr).await;
    let n = nav(graph(0));

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang");
    assert!(result.is_err(), "{result:?}");
    assert_eq!(opens.load(Ordering::SeqCst), 0, "no open/bash for a look refusal");
}

/// The same-named-twin hazard: 1/2151 and 1/2146 are BOTH "Newhaven,
/// Narrow Road", adjacent (~51k such pairs world-wide). When a step is
/// REFUSED and the recovery look answers with the room's own name, the
/// walk knows it did not move — that answer must never read as an
/// arrival at the same-named destination, or `current` drifts a room
/// ahead while the character stands still.
#[tokio::test]
async fn a_refused_step_between_same_named_twins_does_not_drift() {
    let mut here = GraphRoom {
        name: "Newhaven, Narrow Road".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    here.exits[Direction::North as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut there = GraphRoom {
        name: "Newhaven, Narrow Road".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    there.exits[Direction::South as usize] = Some(ExitEdge {
        dest: HERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let twins = Arc::new(RoomGraph::from_rooms(vec![(HERE, here), (THERE, there)]));

    let (addr, _, _) = scripted_board(vec![
        (
            "n",
            "\r\nn\r\nThere is no exit in that direction!\r\n[HP=30/MA=0]:".to_string(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block("Newhaven, Narrow Road", "north")),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    let n = nav(twins);

    let err = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, THERE, &mut NoGuard),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the board refuses this exit; the walk cannot arrive");
    assert_eq!(
        err.at, HERE,
        "recorded an arrival that never happened: {err:?}"
    );
}

// ---------------------------------------------------------------------
// The run5 mid-leg entry (2026-08-01): monsters walked in during travel
// legs and whiffed, and the walk kept sending steps. These pin the veto
// at the nav layer: the guard arms mid-step, and goto hands back before
// the next step goes out.
// ---------------------------------------------------------------------

const FAR: RoomId = RoomId { map: 1, room: 3 };

/// Guard Post -> Inner Ward -> Keep, so there is a NEXT step to veto.
fn corridor() -> Arc<RoomGraph> {
    let mut here = GraphRoom {
        name: "Guard Post".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    here.exits[Direction::North as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut there = GraphRoom {
        name: "Inner Ward".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    there.exits[Direction::North as usize] = Some(ExitEdge {
        dest: FAR,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    there.exits[Direction::South as usize] = Some(ExitEdge {
        dest: HERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut far = GraphRoom {
        name: "Keep".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    far.exits[Direction::South as usize] = Some(ExitEdge {
        dest: THERE,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    Arc::new(RoomGraph::from_rooms(vec![
        (HERE, here),
        (THERE, there),
        (FAR, far),
    ]))
}

fn fighting_guard() -> mud_client::farm::FarmGuard {
    mud_client::farm::FarmGuard::new(30, 25, "Farmer").sighting(mud_client::bot::Bot::new(
        mud_client::bot::BotConfig {
            auto_combat: true,
            ..mud_client::bot::BotConfig::default()
        },
    ))
}

/// The entry lands between the step's echo and its arrival block. The
/// guard arms mid-step, the block completes the step honestly, and the
/// walk hands back at the room it verified — before the next step goes
/// out. The board must see exactly one "n".
#[tokio::test]
async fn a_mob_entering_behind_the_echo_interrupts_before_the_next_step() {
    let (addr, _, received) = scripted_board(vec![(
        "n",
        format!(
            "\r\nn\r\nA giant rat creeps into the room from nowhere.{}",
            room_block("Inner Ward", "north south")
        ),
    )])
    .await;
    let session = session_for(addr).await;
    let n = nav(corridor());
    let mut guard = fighting_guard();

    let err = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, FAR, &mut guard),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the entry must stop the walk");
    assert_eq!(err.at, THERE, "hand back where the walk verified");
    match err.kind {
        mud_client::nav::NavErrorKind::Interrupted(mud_client::nav::Interrupt::Entered {
            ref name,
        }) => assert_eq!(name, "giant rat"),
        ref other => panic!("expected Entered, got {other:?}"),
    }
    let steps = received.lock().unwrap().iter().filter(|l| *l == "n").count();
    assert_eq!(steps, 1, "the queued second step must never go out");
}

/// A whiff aimed at us mid-step stops a fighting walk exactly like a
/// landed blow — run5's kobold thief never connected once across three
/// rooms, and a guard waiting for CombatHit never fired. It hands back
/// as `Entered` (work, no budget), not `Attacked`: whiff-storms in a
/// shared swarm room were spending the whole interrupt budget on a
/// healthy character (cwgaming, 2026-08-01).
#[tokio::test]
async fn a_whiff_behind_the_echo_interrupts_a_fighting_walk() {
    let (addr, _, received) = scripted_board(vec![(
        "n",
        format!(
            "\r\nn\r\nThe thin giant rat lunges at you!{}",
            room_block("Inner Ward", "north south")
        ),
    )])
    .await;
    let session = session_for(addr).await;
    let n = nav(corridor());
    let mut guard = mud_client::farm::FarmGuard::new(30, 25, "Farmer");

    let err = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, FAR, &mut guard),
    )
    .await
    .expect("goto should not hang")
    .expect_err("the whiff must stop the walk");
    assert_eq!(err.at, THERE);
    assert!(
        matches!(
            err.kind,
            mud_client::nav::NavErrorKind::Interrupted(mud_client::nav::Interrupt::Entered {
                ..
            })
        ),
        "expected Entered, got {:?}",
        err.kind
    );
    let steps = received.lock().unwrap().iter().filter(|l| *l == "n").count();
    assert_eq!(steps, 1);
}

/// A player walking in is not work: the case rule filters the entry and
/// the walk completes. Pins the composed behaviour — the parser's
/// lowercase anchor and the guard's would-attack policy both protect it.
#[tokio::test]
async fn a_players_arrival_does_not_interrupt() {
    let (addr, _, _) = scripted_board(vec![
        (
            "n",
            format!(
                "\r\nn\r\nKaimon walks into the room from the east.{}",
                room_block("Inner Ward", "north south")
            ),
        ),
        ("n", format!("\r\nn{}", room_block("Keep", "south"))),
    ])
    .await;
    let session = session_for(addr).await;
    let n = nav(corridor());
    let mut guard = fighting_guard();

    let at = tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(&session, HERE, FAR, &mut guard),
    )
    .await
    .expect("goto should not hang")
    .expect("a player's arrival must not stop the walk");
    assert_eq!(at.at, FAR);
}
