//! Walk versus run, against a scripted echoing board.
//!
//! The harness is the `farm_scripted.rs` / `nav_echo.rs` shape — test
//! crates do not share modules, so it is duplicated here, which is this
//! suite's existing pattern.
//!
//! This is the only place the feature's central promise is actually
//! exercised. `tests/go.rs` pins that `go_config` sets the right
//! boolean; what that boolean *does* comes from an interaction between
//! two files: `nav.rs` classifies a board combat-refusal as
//! `Interrupt::Attacked` unconditionally, and `farm::travel`'s
//! `Attacked` branch is not gated on `fight_while_travelling` — only its
//! sighting branch is. Nobody would notice either of those changing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::bot::BotConfig;
use mud_client::farm::FarmConfig;
use mud_client::go::{GoEnd, go_config, run_go};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};

const START: RoomId = RoomId { map: 1, room: 1 };
const MIDWAY: RoomId = RoomId { map: 1, room: 2 };
const STOP: RoomId = RoomId { map: 1, room: 3 };

fn room_block(name: &str, also_here: Option<&str>, exits: &str) -> String {
    let also = match also_here {
        Some(names) => format!("Also here: {names}.\r\n"),
        None => String::new(),
    };
    format!("\r\n\x1b[1;36m{name}\r\n{also}Obvious exits: {exits}\r\n[HP=30/MA=0]:")
}

/// Guard Post -> Inner Ward -> Keep, one straight corridor.
fn corridor() -> Arc<RoomGraph> {
    let mut start = GraphRoom {
        name: "Guard Post".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    start.exits[Direction::North as usize] = Some(ExitEdge {
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut midway = GraphRoom {
        name: "Inner Ward".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    midway.exits[Direction::North as usize] = Some(ExitEdge {
        dest: STOP,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    midway.exits[Direction::South as usize] = Some(ExitEdge {
        dest: START,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut stop = GraphRoom {
        name: "Keep".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    stop.exits[Direction::South as usize] = Some(ExitEdge {
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    Arc::new(RoomGraph::from_rooms(vec![
        (START, start),
        (MIDWAY, midway),
        (STOP, stop),
    ]))
}

/// A board driven by a per-line script: `(matcher, reply)`, each entry
/// used once, strictly in order; a re-ask replays the most recently
/// consumed matching entry. Unmatched lines echo and say back.
async fn scripted_board(
    script: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Guard Post", None, "north").as_bytes())
            .await
            .unwrap();
        let mut used: Vec<Option<u64>> = vec![None; script.len()];
        let mut clock: u64 = 0;
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
                let next = used.iter().position(Option::is_none);
                let fresh = next.filter(|&i| script[i].0 == line);
                let reply = match fresh {
                    Some(i) => {
                        clock += 1;
                        used[i] = Some(clock);
                        script[i].1.clone()
                    }
                    None => match script
                        .iter()
                        .enumerate()
                        .filter(|(i, (m, _))| used[*i].is_some() && *m == line)
                        .max_by_key(|(i, _)| used[*i])
                    {
                        Some((_, (_, r))) => r.clone(),
                        None => format!("\r\n{line}\r\nYou say \"{line}\"\r\n[HP=30/MA=0]:"),
                    },
                };
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        }
    });
    (addr, received)
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

fn bot() -> BotConfig {
    BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    }
}

/// No room database behind the tiny corridor, which `run_go` must treat
/// as "no threat opinion" rather than as fatal.
fn cfg(walking: bool) -> FarmConfig {
    go_config(
        &FarmConfig {
            content: std::path::PathBuf::from("/nonexistent/rooms.sqlite"),
            ..Default::default()
        },
        walking,
    )
}

async fn walk(walking: bool, script: Vec<(&'static str, String)>) -> (GoEnd, Vec<String>) {
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    let graph = corridor();
    let end = match tokio::time::timeout(
        Duration::from_secs(30),
        run_go(&session, graph, Some(START), STOP, &bot(), &cfg(walking), None),
    )
    .await
    .expect("run_go should finish, not hang")
    {
        Ok(end) => end,
        Err(e) => panic!(
            "the walk must survive: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    let log = received.lock().unwrap().clone();
    (end, log)
}

/// run_go's opening "where am I", and a first step that lands beside a
/// rat which swings and misses. Where the two modes diverge.
fn up_to_the_whiff() -> Vec<(&'static str, String)> {
    vec![
        (
            "look",
            format!("\r\nlook{}", room_block("Guard Post", None, "north")),
        ),
        (
            "n",
            format!(
                "\r\nn\r\nThe giant rat swings at you but misses!\r\n{}",
                room_block("Inner Ward", Some("giant rat"), "north south")
            ),
        ),
    ]
}

/// Run mode's whole point: something took a swing, and the walk did not
/// care. A whiff is not a refusal, so nothing stops.
///
/// The script ends one step after the whiff because that is the claim —
/// the board is scripted strictly in order, so a run that paused to look
/// or swing would find no reply waiting and desync. The absence of those
/// entries IS the assertion; the log check below only says so out loud.
#[tokio::test]
async fn run_mode_walks_past_a_fight() {
    let mut script = up_to_the_whiff();
    script.push(("n", format!("\r\nn{}", room_block("Keep", None, "south"))));
    let (end, log) = walk(false, script).await;
    assert_eq!(end, GoEnd::Arrived(STOP), "log: {log:?}");
    assert!(
        !log.iter().any(|l| l.starts_with("a ")),
        "run mode must not attack anything: {log:?}"
    );
    assert!(
        !log.iter().skip(1).any(|l| l == "look"),
        "run mode must not stop to look either: {log:?}"
    );
}

/// Walk mode, same whiff: the sighting stops the leg, the room is
/// cleared, and only then does the walk resume.
#[tokio::test]
async fn walk_mode_stops_for_the_fight_then_resumes() {
    let mut script = up_to_the_whiff();
    script.extend([
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block("Inner Ward", Some("giant rat"), "north south")
            ),
        ),
        (
            "a rat",
            "\r\na rat\r\nYou smack giant rat for 12 damage!\r\nThe giant rat falls to the ground with a tortured squeak.\r\nYou gain 25 experience.\r\n*Combat Off*\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
    ]);
    let (end, log) = walk(true, script).await;
    assert_eq!(end, GoEnd::Arrived(STOP), "log: {log:?}");
    let first_n = log.iter().position(|l| l == "n").expect("first step");
    let attack = log
        .iter()
        .position(|l| l == "a rat")
        .unwrap_or_else(|| panic!("walk mode must fight: {log:?}"));
    let last_n = log.iter().rposition(|l| l == "n").expect("the walk resumed");
    assert!(
        first_n < attack && attack < last_n,
        "the fight belongs between the steps: {log:?}"
    );
}

/// The one that matters. The board refuses movement outright while in
/// combat, so "run" cannot mean "never fight" — a walk that would not
/// fight is a walk that stays stuck wherever something picked a fight.
/// Run mode must therefore still fight its way out of a refusal, even
/// though it ignored the whiff that started it.
#[tokio::test]
async fn run_mode_still_fights_out_of_a_combat_lock() {
    let (end, log) = walk(
        false,
        vec![
            (
                "look",
                format!("\r\nlook{}", room_block("Guard Post", None, "north")),
            ),
            (
                "n",
                format!(
                    "\r\nn\r\nThe giant rat swings at you but misses!\r\n{}",
                    room_block("Inner Ward", Some("giant rat"), "north south")
                ),
            ),
            // The lock: the board simply will not let us leave.
            (
                "n",
                "\r\nn\r\nYou may not enter that room while in combat.\r\n[HP=30/MA=0]:".into(),
            ),
            // The defence that the refusal forces, even in run mode.
            (
                "look",
                format!(
                    "\r\nlook{}",
                    room_block("Inner Ward", Some("giant rat"), "north south")
                ),
            ),
            (
                "a rat",
                "\r\na rat\r\nYou smack giant rat for 12 damage!\r\nThe giant rat falls to the ground with a tortured squeak.\r\nYou gain 25 experience.\r\n*Combat Off*\r\n[HP=30/MA=0]:"
                    .into(),
            ),
            ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
        ],
    )
    .await;

    assert_eq!(
        end,
        GoEnd::Arrived(STOP),
        "a locked walk must fight free and finish: {log:?}"
    );
    assert!(
        log.iter().any(|l| l == "a rat"),
        "the refusal must be answered with a fight, not a retry: {log:?}"
    );
}
