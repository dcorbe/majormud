//! `/bank` against a scripted echoing board: from wherever the
//! character stands, walk to the nearest bank, read the purse, deposit
//! above the keep floor, read again. The harness is the
//! `farm_scripted.rs` shape, duplicated because test crates do not
//! share modules.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::bank::{ErrandEnd, run_bank};
use mud_client::bot::BotConfig;
use mud_client::farm::{FarmConfig, probe_sheet};
use mud_client::live::Live;
use mud_client::go::go_config;
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Content, Direction, Room, RoomId, Shop, ShopId, ShopStock};

/// A notices sink that keeps nothing. What a runner says at startup is
/// not what these tests are about.
fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

const HOME: RoomId = RoomId { map: 1, room: 1 };
const BANK: RoomId = RoomId { map: 1, room: 2 };

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:")
}

fn edge(dest: RoomId) -> ExitEdge {
    ExitEdge {
        dest,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    }
}

/// Home, and the bank one step east.
fn graph() -> Arc<RoomGraph> {
    let mut home = GraphRoom {
        name: "Home".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    home.exits[Direction::East as usize] = Some(edge(BANK));
    let mut bank = GraphRoom {
        name: "Bank of Godfrey".into(),
        exits: Default::default(),
        light: 0,
        shop: 8,
        ..Default::default()
    };
    bank.exits[Direction::West as usize] = Some(edge(HOME));
    Arc::new(RoomGraph::from_rooms(vec![(HOME, home), (BANK, bank)]))
}

fn content() -> Content {
    let mut c = Content::default();
    c.add_shop(Shop {
        id: ShopId(8),
        name: "Bank of Godfrey".into(),
        shop_type: 7,
        min_level: 0,
        max_level: 0,
        markup: 0,
        class_limit: 0,
        stock: [ShopStock::default(); 20],
    });
    c.add_room(Room {
        id: BANK,
        name: "Bank of Godfrey".into(),
        room_type: 1,
        shop: Some(ShopId(8)),
        ..Default::default()
    });
    c
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
        sock.write_all(room_block("Home", "east").as_bytes()).await.unwrap();
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

async fn session_for(addr: std::net::SocketAddr, keep_gold: u32) -> Session {
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
        bank: mud_client::bank::BankConfig {
            keep_gold,
            ..Default::default()
        },
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

/// The whole errand, with a keep floor: 15 gold on hand, 5 kept, 10
/// gold deposited as 1000 farthings.
#[tokio::test]
async fn bank_walks_to_the_nearest_bank_and_deposits_above_the_floor() {
    let carrying = "\r\ni\r\nYou are carrying 15 gold crowns\r\nYou have no keys.\r\nWealth: 1500 copper farthings\r\nEncumbrance: 5/2400 - None [0%]\r\n[HP=30/MA=0]:";
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying 15 gold crowns\r\nYou have no keys.\r\nEncumbrance: 5/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Home", "east"))),
        ("e", format!("\r\ne{}", room_block("Bank of Godfrey", "west"))),
        ("i", carrying.into()),
        (
            "deposit 1000",
            "\r\ndeposit 1000\r\nYou deposit 10 gold crowns.\r\n[HP=30/MA=0]:".into(),
        ),
        (
            "i",
            "\r\ni\r\nYou are carrying 5 gold crowns\r\nYou have no keys.\r\nEncumbrance: 1/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
    ])
    .await;
    let session = session_for(addr, 5).await;
    probe_sheet(&session, None).await;
    session.set_content(Arc::new(content()));
    let cfg = go_config(
        &FarmConfig {
            depart_at_percent: Some(0),
            ..FarmConfig::default()
        },
        false,
    );
    let bot = BotConfig {
        max_hp: 30,
        ..BotConfig::default()
    };
    let end = tokio::time::timeout(
        Duration::from_secs(20),
        run_bank(&session, graph(), Some(HOME), Live::fixed(bot, cfg), None, &quiet()),
    )
    .await
    .expect("run_bank should finish, not hang")
    .unwrap_or_else(|e| panic!("{e}\nboard received: {:?}", received.lock().unwrap()));
    assert_eq!(
        end,
        ErrandEnd::Deposited {
            farthings: 1000,
            at: BANK,
            bank: "Bank of Godfrey".into()
        },
        "{:?}",
        received.lock().unwrap()
    );
    let log = received.lock().unwrap().clone();
    let east = log.iter().position(|l| l == "e").expect("the walk");
    let deposit = log.iter().position(|l| l == "deposit 1000").expect("the deposit");
    assert!(east < deposit, "{log:?}");
    assert_eq!(
        session.capabilities().purse.farthings(),
        500,
        "the re-read after the deposit reached the purse meter"
    );
}

