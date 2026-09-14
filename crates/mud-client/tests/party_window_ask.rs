//! A member under its heal mark says `@heal` to the room once per round
//! while it also casts on itself, and stops once it is over the mark.
//! Live 2026-09-14: Blueberry could always afford its own small heal, so
//! under the old rule it never asked, and died at 13 percent with two
//! healers standing beside it. One test in this binary, because it sets
//! `XDG_CONFIG_HOME`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::graph::{GraphRoom, RoomGraph};
use mud_client::settings::Settings;
use mud_client::spawn::SpawnTable;
use mud_client::tui::{ContentCache, World};
use mud_client::window::spawn;
use mud_core::content::{Class, ClassId, Content, RoomId};

const HOME: RoomId = RoomId { map: 1, room: 1 };

const HOME_BLOCK: &str = "\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP=100/MA=20]:";

/// A board that puts the character in the realm at full health, says it
/// is now following Beef, and only then drops it to under the profile's
/// heal mark. Every line the client sends is logged.
async fn ask_board(sent: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(format!("Welcome to the Test Board{HOME_BLOCK}").as_bytes()).await.unwrap();
        let mut buf = [0u8; 512];
        let mut pending = String::new();
        let mut step = 0;
        // One health for the whole board, echoes included. An echo that
        // answered with a stale number would tell the client the ask
        // it just sent had healed nothing, and an ask that shows no
        // gain is one the client gives up on.
        let mut hp = 100;
        let mut tick = tokio::time::interval(Duration::from_millis(600));
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
                            "look" => format!("\r\n{line}\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP={hp}/MA=20]:"),
                            "health" => format!("\r\nhealth\r\nHealth:   {hp}/100   [{hp}%]\r\n[HP={hp}/MA=20]:"),
                            "stat" => format!("\r\nstat\r\nName: Beefy   Lives/CP: 9/2\r\nClass: Warrior   Level: 15\r\nMagicRes: 0\r\n[HP={hp}/MA=20]:"),
                            "inventory" | "i" => format!("\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP={hp}/MA=20]:"),
                            "spells" => format!(
                                "\r\nspells\r\nYou have the following spells:\r\nLevel Mana Short Spell Name\r\n  1   2    mihe  minor healing                 \r\n[HP={hp}/MA=20]:"
                            ),
                            // The self cast's wording, captured live 2026-09-14.
                            "cast mihe" => format!("\r\ncast mihe\r\nYou cast minor healing on yourself, healing 12 damage!\r\n[HP={hp}/MA=20]:"),
                            _ => format!("\r\n{line}\r\n[HP={hp}/MA=20]:"),
                        };
                        sock.write_all(reply.as_bytes()).await.unwrap();
                    }
                }
                _ = tick.tick() => {
                    let say = if step == 0 {
                        let say = format!("\r\nYou are now following Beef\r\n[HP={hp}/MA=20]:");
                        hp = 30;
                        say
                    } else if step < 8 {
                        format!("\r\n[HP={hp}/MA=20]:")
                    } else {
                        hp = 90;
                        format!("\r\n[HP={hp}/MA=20]:")
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
/// class carries its weight: a caster group of 1 tells the realm entry
/// probe the character casts, so it reads the spell listing and the
/// assist has a heal of its own to cast.
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
        caster_group: 1,
        casting_factor: 1,
        exp_base: 0,
        combat_factor: 6,
        weapon_code: 8,
        armour_code: 9,
    });
    Arc::new(World::new(Arc::new(content), Arc::new(graph), Arc::new(SpawnTable::default())))
}

#[tokio::test]
async fn a_hurt_member_asks_once_per_round_while_it_casts_on_itself() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("party_window_ask");
    let _ = std::fs::remove_dir_all(&base);
    // Before any other task exists in this process. The only test in
    // this binary, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let sent = Arc::new(Mutex::new(Vec::new()));
    let addr = ask_board(sent.clone()).await;
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
    s.set("bot.auto_heal", "true").unwrap();
    s.set("bot.auto_rest", "false").unwrap();
    s.set("bot.minor_heal_at_percent", "70").unwrap();
    s.set("bot.major_heal_at_percent", "40").unwrap();
    let mut cache = ContentCache::default();
    cache.insert(db, world());
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn(4, s, 200, 80, Arc::new(Mutex::new(cache)), tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        if sent.lock().unwrap().iter().any(|l| l == "say @heal 30")
            && tokio::time::Instant::now() > deadline - Duration::from_secs(4)
        {
            break;
        }
        if tokio::time::timeout_at(deadline, front.recv()).await.is_err() {
            break;
        }
    }
    let log = sent.lock().unwrap().clone();
    let screen = handle.screen.lock().unwrap().text();
    let asks = log.iter().filter(|l| l.as_str() == "say @heal 30").count();
    assert!(log.iter().any(|l| l.as_str() == "cast mihe"), "it casts on itself too: {log:?}\nscreen:\n{screen}");
    assert!(asks >= 1, "the ask went out: {log:?}\nscreen:\n{screen}");
    assert!(asks <= 2, "once per round, not per prompt: {log:?}\nscreen:\n{screen}");
    assert!(!log.iter().any(|l| l == "say @heal 90"), "over the mark there is nothing to ask: {log:?}");
}

