//! `/recover` against a scripted echoing board.
//!
//! The harness is the `go_scripted.rs` shape. Test crates do not share
//! modules, so it is duplicated here, which is this suite's pattern.
//!
//! A corridor of three rooms: Guard Post, the start room and the safe
//! room, Inner Ward, and Keep, where the character died. The board
//! echoes every line, logs it lowercased, and answers from a script
//! consumed strictly in order, each entry once. A re-ask replays the
//! most recently consumed matching entry. Anything unmatched is said
//! aloud.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::bot::BotConfig;
use mud_client::farm::{FarmConfig, FarmError, Live, probe_sheet};
use mud_client::go::go_config;
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::recover::{HomeWhy, RecoverEnd, run_recover};
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};

fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

const START: RoomId = RoomId { map: 1, room: 1 };
const MIDWAY: RoomId = RoomId { map: 1, room: 2 };
const DEATH: RoomId = RoomId { map: 1, room: 3 };

const PROMPT: &str = "[HP=30/MA=0]:";

fn block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n{PROMPT}")
}

/// A block whose floor lists `items`, as the board renders after a bare
/// `search`, with the prompt `prompt` so a test can show hitpoints
/// falling. Used by the pickup tests. The allow keeps the build clean
/// until they land.
#[allow(dead_code)]
fn block_with(name: &str, items: &str, exits: &str, prompt: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nYou notice {items} here.\r\nObvious exits: {exits}\r\n{prompt}")
}

fn reply(cmd: &str, body: &str) -> String {
    format!("\r\n{cmd}\r\n{body}\r\n{PROMPT}")
}

/// The stat sheet `probe_sheet` reads. Stealth is what lets the job
/// start at all.
const NINJA_SHEET: &str = "\r\nstat\r\n\
Name: Beef                             Lives/CP:    9/100\r\n\
Race: Dark-Elf    Exp: 0               Perception:     43\r\n\
Class: Ninja      Level: 1             Stealth:        56\r\n\
Hits:    30/30    Armour Class:   0/0  Thievery:        0\r\n\
                                       Traps:          29\r\n\
                                       Picklocks:      31\r\n\
Strength:  40     Agility: 50          Tracking:       26\r\n\
Intellect: 50     Health:  30          Martial Arts:   51\r\n\
Willpower: 30     Charm:   40          MagicRes:       35\r\n\
[HP=30/MA=0]:";

const EMPTY_HANDED: &str =
    "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:";

fn edge(dest: RoomId) -> Option<ExitEdge> {
    Some(ExitEdge {
        dest,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    })
}

/// Guard Post -> Inner Ward -> Keep, north each step, with the way back.
/// `keep_light` is the death room's light, so a test can make it dark.
fn corridor(keep_light: i64) -> Arc<RoomGraph> {
    let mut start = GraphRoom {
        name: "Guard Post".into(),
        ..Default::default()
    };
    start.exits[Direction::North as usize] = edge(MIDWAY);
    let mut midway = GraphRoom {
        name: "Inner Ward".into(),
        ..Default::default()
    };
    midway.exits[Direction::North as usize] = edge(DEATH);
    midway.exits[Direction::South as usize] = edge(START);
    let mut death = GraphRoom {
        name: "Keep".into(),
        light: keep_light,
        ..Default::default()
    };
    death.exits[Direction::South as usize] = edge(MIDWAY);
    Arc::new(RoomGraph::from_rooms(vec![
        (START, start),
        (MIDWAY, midway),
        (DEATH, death),
    ]))
}

async fn scripted_board(
    script: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(block("Guard Post", "north").as_bytes())
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
                        None => format!("\r\n{line}\r\nYou say \"{line}\"\r\n{PROMPT}"),
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
        username: "Beef".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        // Known maxima, so the job never asks `health` and every
        // percent mark is against 30.
        bot: Some(BotConfig {
            max_hp: 30,
            minor_heal_at_percent: 70,
            ..BotConfig::default()
        }),
        farm: None,
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

/// The settings a recovery runs under, with the job's own forced fields
/// already applied, the way the window derives them. No room database:
/// these tests read no content at all.
fn settings() -> Live {
    let mut farm = go_config(
        &FarmConfig {
            content: std::path::PathBuf::new(),
            ..FarmConfig::default()
        },
        false,
    );
    farm.interrupt_at_percent = 0;
    farm.nav.bash_doors = false;
    let bot = BotConfig {
        max_hp: 30,
        minor_heal_at_percent: 70,
        auto_combat: false,
        auto_flee: false,
        auto_get: false,
        ..BotConfig::default()
    };
    Live::fixed(bot, farm)
}

/// The sheet, the inventory, the opening look and the sneak: how every
/// run starts.
fn opening() -> Vec<(&'static str, String)> {
    vec![
        ("stat", NINJA_SHEET.into()),
        ("inventory", EMPTY_HANDED.into()),
        ("look", format!("\r\nlook{}", block("Guard Post", "north"))),
        ("sneak", reply("sneak", "Attempting to sneak...")),
    ]
}

fn sneaky_step(name: &str, exits: &str) -> (&'static str, String) {
    ("n", format!("\r\nn\r\nSneaking...{}", block(name, exits)))
}

fn home_steps() -> Vec<(&'static str, String)> {
    vec![
        ("s", format!("\r\ns{}", block("Inner Ward", "north south"))),
        ("s", format!("\r\ns{}", block("Guard Post", "north"))),
    ]
}

async fn recover_over(
    graph: Arc<RoomGraph>,
    script: Vec<(&'static str, String)>,
) -> (Result<RecoverEnd, FarmError>, Vec<String>) {
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    probe_sheet(&session, None).await;
    let out = tokio::time::timeout(
        Duration::from_secs(40),
        run_recover(&session, graph, START, DEATH, settings(), None, &quiet()),
    )
    .await
    .expect("run_recover should finish, not hang");
    let log = received.lock().unwrap().clone();
    (out, log)
}

fn count(log: &[String], line: &str) -> usize {
    log.iter().filter(|l| *l == line).count()
}

// ------------------------------------------------------------ refusals

/// Nothing is sent before the refusal: the character cannot sneak, so
/// there is no job to run.
#[tokio::test]
async fn stealth_zero_is_refused_before_anything_is_sent() {
    let (addr, received) = scripted_board(vec![]).await;
    let session = session_for(addr).await;
    let out = run_recover(
        &session,
        corridor(0),
        START,
        DEATH,
        settings(),
        None,
        &quiet(),
    )
    .await;
    let why = match out {
        Err(FarmError::Config(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert!(why.contains("Stealth is 0"), "{why}");
    assert!(received.lock().unwrap().is_empty(), "nothing may be sent");
}

// ---------------------------------------------------------- the walk in

/// The first hop lands without `Sneaking...`. The sneak broke, and the
/// job turns for home from Inner Ward without ever searching.
#[tokio::test]
async fn a_broken_sneak_turns_for_home_without_searching() {
    let mut script = opening();
    script.push(("n", format!("\r\nn{}", block("Inner Ward", "north south"))));
    script.push(("s", format!("\r\ns{}", block("Guard Post", "north"))));
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must survive a break");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Broke {
                at: MIDWAY,
                name: "Inner Ward".into()
            },
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "search"), 0, "no search after a break: {log:?}");
    assert_eq!(count(&log, "sneak"), 1, "the walk home does not arm: {log:?}");
}

/// The floor is bare. One search, then home.
#[tokio::test]
async fn an_empty_search_goes_home() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push(("search", reply("search", "Your search revealed nothing.")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Nothing,
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "search"), 1, "exactly one search: {log:?}");
    let sneak_at = log.iter().position(|l| l == "sneak").unwrap();
    let first_move = log.iter().position(|l| l == "n").unwrap();
    assert!(
        sneak_at < first_move,
        "the sneak is armed before the first move: {log:?}"
    );
}

/// The board refuses the search because something already has the
/// character. Home at once.
#[tokio::test]
async fn a_search_refused_for_combat_goes_home() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push((
        "search",
        reply("search", "You may not search while attacking!"),
    ));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Attacked {
                at: DEATH,
                name: "Keep".into()
            },
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l.starts_with("a ")),
        "never an attack: {log:?}"
    );
}

