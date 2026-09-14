//! A window dragged into a bank while following deposits once, and
//! once only. One test in this binary, because it sets
//! `XDG_CONFIG_HOME` for the whole process and nothing else may run
//! beside it.
//!
//! The job hands over with a `look`, whose block names the same bank.
//! Without the window's `last_room` the decision reads that block as a
//! second arrival and starts the deposit again, for as long as the
//! character stands there. Nothing else pins that wiring: the decision
//! is told what the last room was, and the job is driven directly by
//! `party_follower.rs`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::settings::Settings;
use mud_client::spawn::SpawnTable;
use mud_client::tui::{ContentCache, World};
use mud_client::window::spawn;
use mud_core::content::{Content, Direction, Room, RoomId, Shop, ShopId, ShopStock};

const HOME: RoomId = RoomId { map: 1, room: 1 };
const BANK: RoomId = RoomId { map: 1, room: 297 };

const HOME_BLOCK: &str = "\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP=30/MA=0]:";
const BANK_BLOCK: &str = "\r\n\x1b[1;36mBank of Godfrey\r\nObvious exits: west\r\n[HP=30/MA=0]:";
const DRAG: &str = "\r\n -- Following your Party leader east --\r\n\r\n\x1b[1;36mBank of Godfrey\r\nObvious exits: west\r\n[HP=30/MA=0]:";

/// A board that puts the character in the realm, says it is now
/// following Beef, then drags it into the bank, and prompts every so
/// often after that. It answers the purse with 15 gold until the
/// deposit lands and with nothing after it, and answers `look` with the
/// bank's block, as the board does for the handover the job's end
/// sends. Every line the client sends is logged.
async fn bank_drag_board(sent: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(format!("Welcome to the Test Board{HOME_BLOCK}").as_bytes()).await.unwrap();
        let mut buf = [0u8; 512];
        let mut pending = String::new();
        let mut deposited = false;
        let mut step = 0;
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        // The first tick is immediate, and nothing is worth saying
        // until the session is reading.
        tick.tick().await;
        loop {
            tokio::select! {
                n = sock.read(&mut buf) => {
                    let n = n.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                    while let Some(at) = pending.find('\n') {
                        let line = pending[..at].trim().to_string();
                        pending = pending[at + 1..].to_string();
                        sent.lock().unwrap().push(line.clone());
                        let purse = match deposited {
                            false => "You are carrying 15 gold crowns\r\nYou have no keys.\r\nEncumbrance: 5/2400 - None [0%]",
                            true => "You are carrying nothing\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]",
                        };
                        let reply = match line.to_lowercase().as_str() {
                            "i" | "inventory" => format!("\r\n{line}\r\n{purse}\r\n[HP=30/MA=0]:"),
                            "deposit 1500" => {
                                deposited = true;
                                format!("\r\n{line}\r\nYou deposit 15 gold crowns.\r\n[HP=30/MA=0]:")
                            }
                            "look" => format!("\r\n{line}{BANK_BLOCK}"),
                            "health" => "\r\nhealth\r\nHealth:   30/30   [100%]\r\n[HP=30/MA=0]:".to_string(),
                            "stat" => "\r\nstat\r\nName: Beef   Lives/CP: 9/2\r\nClass: Warrior   Level: 15\r\nMagicRes: 0\r\n[HP=30/MA=0]:".to_string(),
                            _ => format!("\r\n{line}\r\n[HP=30/MA=0]:"),
                        };
                        sock.write_all(reply.as_bytes()).await.unwrap();
                    }
                }
                _ = tick.tick() => {
                    let say = match step {
                        0 => "\r\nYou are now following Beef\r\n[HP=30/MA=0]:".to_string(),
                        1 => DRAG.to_string(),
                        _ => "\r\n[HP=30/MA=0]:".to_string(),
                    };
                    step += 1;
                    sock.write_all(say.as_bytes()).await.unwrap();
                }
            }
        }
    });
    addr
}

fn edge(dest: RoomId) -> ExitEdge {
    ExitEdge { dest, exit_type: 0, command: None, requirement: ExitRequirement::None }
}

/// Home, and the bank one step east. The content names the bank room as
/// the board prints it, and its shop otherwise, as the shipped world
/// does for four banks of five.
fn world() -> Arc<World> {
    let mut home = GraphRoom { name: "Home".into(), ..Default::default() };
    home.exits[Direction::East as usize] = Some(edge(BANK));
    let mut bank = GraphRoom { name: "Bank of Godfrey".into(), shop: 8, ..Default::default() };
    bank.exits[Direction::West as usize] = Some(edge(HOME));
    let graph = RoomGraph::from_rooms(vec![(HOME, home), (BANK, bank)]);
    let mut content = Content::default();
    content.add_shop(Shop {
        id: ShopId(8),
        name: "Godfrey Savings".into(),
        shop_type: 7,
        min_level: 0,
        max_level: 0,
        markup: 0,
        class_limit: 0,
        stock: [ShopStock::default(); 20],
    });
    content.add_room(Room {
        id: BANK,
        name: "Bank of Godfrey".into(),
        room_type: 1,
        shop: Some(ShopId(8)),
        ..Default::default()
    });
    Arc::new(World::new(Arc::new(content), Arc::new(graph), Arc::new(SpawnTable::default())))
}

#[tokio::test]
async fn the_window_deposits_once_when_it_is_dragged_into_a_bank() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("party_window_deposit");
    let _ = std::fs::remove_dir_all(&base);
    // Before any other task exists in this process. The only test in
    // this binary, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let sent = Arc::new(Mutex::new(Vec::new()));
    let addr = bank_drag_board(sent.clone()).await;
    // Never opened. The window asks the cache for this path and the
    // cache already has an answer.
    let db = base.join("world.sqlite");
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beef\"").unwrap();
    s.set("farm.content", &format!("{:?}", db.to_string_lossy())).unwrap();
    // The deposit is the assist's business, so the assist must be on.
    s.set("bot.assist_play", "true").unwrap();
    s.set("bot.max_hp", "30").unwrap();
    let mut cache = ContentCache::default();
    cache.insert(db, world());
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn(4, s, 200, 80, Arc::new(Mutex::new(cache)), tx, None, None);

    // Long enough for the drag, the deposit, the handover `look` and
    // the blocks after it. A loop would have said `@ok` many times over
    // by now.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::timeout_at(deadline, front.recv()).await.is_ok() {}

    let log = sent.lock().unwrap().clone();
    let screen = handle.screen.lock().unwrap().text();
    assert_eq!(
        log.iter().filter(|l| *l == "deposit 1500").count(),
        1,
        "one deposit: {log:?}\nscreen:\n{screen}"
    );
    assert_eq!(
        log.iter().filter(|l| *l == "/Beef @ok").count(),
        1,
        "one ok: {log:?}\nscreen:\n{screen}"
    );
}
