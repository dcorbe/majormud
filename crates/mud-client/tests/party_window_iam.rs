//! A member introduces itself with `@iam <race> <class>` when it joins
//! a party and again to a newcomer. One test in this binary, because
//! it sets `XDG_CONFIG_HOME`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::graph::{GraphRoom, RoomGraph};
use mud_client::settings::Settings;
use mud_client::spawn::SpawnTable;
use mud_client::tui::{ContentCache, World};
use mud_client::window::spawn;
use mud_core::content::{Class, ClassId, Content, RoomId};

const HOME: RoomId = RoomId { map: 1, room: 1 };

const HOME_BLOCK: &str = "\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP=100/MA=0]:";

/// A board that puts the character in the realm at full health and says
/// it is now following Beef. A `party` poll later names Carrot too.
/// Every line the client sends is logged.
async fn iam_board(sent: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(format!("Welcome to the Test Board{HOME_BLOCK}").as_bytes()).await.unwrap();
        let mut buf = [0u8; 512];
        let mut pending = String::new();
        let mut step = 0;
        let hp = 100;
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
                        let reply = match line.to_lowercase().as_str() {
                            "look" => format!("\r\n{line}\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP={hp}/MA=0]:"),
                            "health" => format!("\r\nhealth\r\nHealth:   {hp}/100   [{hp}%]\r\n[HP={hp}/MA=0]:"),
                            "stat" => format!("\r\nstat\r\nName: Beefy   Lives/CP: 9/2\r\nRace: Human      Exp: 0\r\nClass: Warrior   Level: 15\r\nMagicRes: 0\r\n[HP={hp}/MA=0]:"),
                            "inventory" | "i" => format!("\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP={hp}/MA=0]:"),
                            "party" => format!(
                                "\r\nparty\r\nThe following people are in your travel party:\r\n  Beef                           (Ninja)               [H:100%]   - Frontrank\r\n  Carrot                         (Paladin)    [M:100%] [H:100%]   - Midrank\r\n\r\n[HP={hp}/MA=0]:"
                            ),
                            _ => format!("\r\n{line}\r\n[HP={hp}/MA=0]:"),
                        };
                        sock.write_all(reply.as_bytes()).await.unwrap();
                    }
                }
                _ = tick.tick() => {
                    let say = if step == 0 {
                        format!("\r\nYou are now following Beef\r\n[HP={hp}/MA=0]:")
                    } else {
                        format!("\r\n[HP={hp}/MA=0]:")
                    };
                    step += 1;
                    sock.write_all(say.as_bytes()).await.unwrap();
                }
            }
        }
    });
    addr
}

/// One room, and the character's class. Nothing walks anywhere, so the
/// graph is only here to keep the window off the database file. The
/// class carries its weight: a caster group of 0 tells the realm entry
/// probe the character casts nothing, so it skips the spell listing,
/// which has no terminator and otherwise costs the probe three seconds
/// before the assist reads its first event.
fn world() -> Arc<World> {
    let home = GraphRoom { name: "Home".into(), ..Default::default() };
    let graph = RoomGraph::from_rooms(vec![(HOME, home)]);
    let mut content = Content::default();
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
    Arc::new(World::new(Arc::new(content), Arc::new(graph), Arc::new(SpawnTable::default())))
}

#[tokio::test]
async fn a_member_introduces_itself_on_joining_and_to_a_newcomer() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("party_window_iam");
    let _ = std::fs::remove_dir_all(&base);
    // Before any other task exists in this process. The only test in
    // this binary, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let sent = Arc::new(Mutex::new(Vec::new()));
    let addr = iam_board(sent.clone()).await;
    // Never opened. The window asks the cache for this path and the
    // cache already has an answer.
    let db = base.join("world.sqlite");
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beefy\"").unwrap();
    s.set("farm.content", &format!("{:?}", db.to_string_lossy())).unwrap();
    s.set("bot.assist_play", "true").unwrap();
    s.set("bot.max_hp", "100").unwrap();
    s.set("party.poll_secs", "1").unwrap();
    let mut cache = ContentCache::default();
    cache.insert(db, world());
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn(4, s, 200, 80, Arc::new(Mutex::new(cache)), tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if sent.lock().unwrap().iter().filter(|l| l.as_str() == "say @iam Human Warrior").count() >= 2 {
            break;
        }
        if tokio::time::timeout_at(deadline, front.recv()).await.is_err() {
            break;
        }
    }
    let log = sent.lock().unwrap().clone();
    let screen = handle.screen.lock().unwrap().text();
    let iams = log.iter().filter(|l| l.as_str() == "say @iam Human Warrior").count();
    assert_eq!(iams, 2, "once on joining, once for Carrot: {log:?}\nscreen:\n{screen}");
}

