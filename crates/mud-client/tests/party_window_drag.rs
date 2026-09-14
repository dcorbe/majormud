//! A follower dragged out of its rest by the leader's in-flight step
//! sits down again and tells the leader `@ok` only when that second
//! rest is over. One test in this binary, because it sets
//! `XDG_CONFIG_HOME` for the whole process and nothing else may run
//! beside it.
//!
//! Live 2026-09-14: the leader's step is already on the wire when
//! `@wait` lands, the board takes a second to resolve it, and every
//! rest ended in a drag. The lost Resting status was read as the rest
//! ending, and the `@ok` released the leader before the hold was ever
//! judged.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::graph::{GraphRoom, RoomGraph};
use mud_client::settings::Settings;
use mud_client::spawn::SpawnTable;
use mud_client::tui::{ContentCache, World};
use mud_client::window::spawn;
use mud_core::content::{Class, ClassId, Content, RoomId};

const HOME: RoomId = RoomId { map: 1, room: 1 };

/// The board's own mark in the send log for the second rest ending.
const REST_OVER: &str = "-- board: rest over --";

const HOME_BLOCK: &str = "\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP=100/MA=0]:";

/// A board that puts the character in the realm at full health, says it
/// is now following Beef, and only then drops it to a tenth of its
/// hits, which is under the profile's rest mark. The first rest is
/// answered with one prompt carrying the Resting status and then the
/// drag: the follow line, a room block, and a prompt without the
/// status, at the same health. The second rest is answered with
/// resting prompts that heal, and the one after those carries none.
/// Every line the client sends is logged.
async fn drag_board(sent: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(format!("Welcome to the Test Board{HOME_BLOCK}").as_bytes()).await.unwrap();
        let mut buf = [0u8; 512];
        let mut pending = String::new();
        let mut step = 0;
        let mut rests = 0;
        let mut dragged = false;
        // One health for the whole board, echoes included. An echo that
        // answered with a stale number would tell the client the rest
        // it just sent had healed nothing, and a rest that shows no
        // gain is one the client gives up on.
        let mut hp = 100;
        // The prompts still painted with the status after the rest went
        // out. The one after them is the end of the rest.
        let mut resting_left = 2;
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
                            "rest" => {
                                rests += 1;
                                format!("\r\nrest\r\n[HP={hp}/MA=0]: (Resting) ")
                            }
                            "look" => format!("\r\n{line}\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP={hp}/MA=0]:"),
                            "health" => format!("\r\nhealth\r\nHealth:   {hp}/100   [{hp}%]\r\n[HP={hp}/MA=0]:"),
                            "stat" => format!("\r\nstat\r\nName: Beefy   Lives/CP: 9/2\r\nClass: Warrior   Level: 15\r\nMagicRes: 0\r\n[HP={hp}/MA=0]:"),
                            "inventory" | "i" => format!("\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP={hp}/MA=0]:"),
                            _ => format!("\r\n{line}\r\n[HP={hp}/MA=0]:"),
                        };
                        sock.write_all(reply.as_bytes()).await.unwrap();
                    }
                }
                _ = tick.tick() => {
                    let say = if step == 0 {
                        // Full health until the party is known, so the
                        // rest cannot come before the follow line.
                        let say = format!("\r\nYou are now following Beef\r\n[HP={hp}/MA=0]:");
                        hp = 10;
                        say
                    } else if rests == 0 {
                        format!("\r\n[HP={hp}/MA=0]:")
                    } else if !dragged {
                        // The leader's step resolves: the board moves
                        // the character and the status is gone, but
                        // nothing has healed.
                        dragged = true;
                        format!("\r\n -- Following your Party leader east --\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP={hp}/MA=0]:")
                    } else if rests < 2 {
                        format!("\r\n[HP={hp}/MA=0]:")
                    } else if resting_left > 0 {
                        resting_left -= 1;
                        // A rest that heals: the client watches the
                        // climb to tell a rest from a refusal.
                        hp += 10;
                        format!("\r\n[HP={hp}/MA=0]: (Resting) ")
                    } else {
                        // The log is the board's timeline: what the
                        // client sent, and when the rest was over.
                        sent.lock().unwrap().push(REST_OVER.to_string());
                        hp = 90;
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
async fn a_dragged_follower_rests_again_and_says_ok_after_that_rest() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("party_window_drag");
    let _ = std::fs::remove_dir_all(&base);
    // Before any other task exists in this process. The only test in
    // this binary, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let sent = Arc::new(Mutex::new(Vec::new()));
    let addr = drag_board(sent.clone()).await;
    // Never opened. The window asks the cache for this path and the
    // cache already has an answer.
    let db = base.join("world.sqlite");
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beefy\"").unwrap();
    s.set("farm.content", &format!("{:?}", db.to_string_lossy())).unwrap();
    // The handshake rides with the assist's own rest, so both must be on.
    s.set("bot.assist_play", "true").unwrap();
    s.set("bot.auto_rest", "true").unwrap();
    // A tenth of its hits is under the flee mark too, and the drag's
    // room block is the first exit the assist learns. Fleeing is not
    // what this test is about.
    s.set("bot.auto_flee", "false").unwrap();
    s.set("bot.rest_at_percent", "50").unwrap();
    s.set("bot.rest_until_percent", "80").unwrap();
    s.set("bot.max_hp", "100").unwrap();
    let mut cache = ContentCache::default();
    cache.insert(db, world());
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn(4, s, 200, 80, Arc::new(Mutex::new(cache)), tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if sent.lock().unwrap().iter().any(|l| l == "/Beef @ok") {
            break;
        }
        if tokio::time::timeout_at(deadline, front.recv()).await.is_err() {
            break;
        }
    }

    let log = sent.lock().unwrap().clone();
    let screen = handle.screen.lock().unwrap().text();
    let at = |want: &str| -> Vec<usize> {
        log.iter().enumerate().filter(|(_, l)| l.as_str() == want).map(|(i, _)| i).collect()
    };
    let waits = at("/Beef @wait");
    let rests = at("rest");
    let oks = at("/Beef @ok");
    let over = at(REST_OVER);
    assert_eq!(waits.len(), 1, "one warning for the whole hold: {log:?}\nscreen:\n{screen}");
    assert_eq!(rests.len(), 2, "the drag broke the first rest: {log:?}\nscreen:\n{screen}");
    assert_eq!(oks.len(), 1, "one release: {log:?}\nscreen:\n{screen}");
    assert!(waits[0] < rests[0], "the warning comes first: {log:?}\nscreen:\n{screen}");
    assert!(rests[1] < oks[0], "the release waits for the second rest: {log:?}\nscreen:\n{screen}");
    assert_eq!(over.len(), 1, "the board ended the second rest: {log:?}\nscreen:\n{screen}");
    assert!(over[0] < oks[0], "the release waits for the second rest to end: {log:?}\nscreen:\n{screen}");
}
