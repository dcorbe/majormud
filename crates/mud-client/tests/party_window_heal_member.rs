//! Potato's live shape, 2026-09-14: a follower with the heals hears a
//! FELLOW FOLLOWER say `@heal`, glued to a prompt, mid-fight, after the
//! roster has been polled, and casts on it by name. The leader is
//! somebody else. The roster's preface is painted the way the live
//! board paints it, in the room-name colour, which is what kept every
//! follower's member list empty. One test in this binary, because it
//! sets `XDG_CONFIG_HOME`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::graph::{GraphRoom, RoomGraph};
use mud_client::settings::Settings;
use mud_client::spawn::SpawnTable;
use mud_client::tui::{ContentCache, World};
use mud_client::window::spawn;
use mud_core::content::{Class, ClassId, Content, RoomId};

const HOME: RoomId = RoomId { map: 1, room: 1 };

const HOME_BLOCK: &str = "\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP=127/MA=28]:";

/// A board that puts the character in the realm at full health and
/// following Carrot, answers `party` with the live roster, then has
/// Salad, a fellow follower, say `@heal 65` glued to a prompt. Every
/// line the client sends is logged.
async fn heal_board(sent: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(format!("Welcome to the Test Board{HOME_BLOCK}").as_bytes()).await.unwrap();
        let mut buf = [0u8; 512];
        let mut pending = String::new();
        let mut step = 0;
        // One health for the whole board, echoes included.
        let hp = 127;
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
                            "look" => format!("\r\n{line}\r\n\x1b[1;36mHome\r\nAlso here: slime beast.\r\nObvious exits: east\r\n[HP={hp}/MA=28]:"),
                            "health" => format!("\r\nhealth\r\nHealth:   {hp}/127   [100%]\r\n[HP={hp}/MA=28]:"),
                            "stat" => format!("\r\nstat\r\nName: Potato                           Lives/CP:      9/3\r\nRace: Dwarf       Exp: 473636          Perception:     43\r\nClass: Paladin    Level: 11            Stealth:         0\r\nMagicRes: 0\r\n[HP={hp}/MA=28]:"),
                            "inventory" | "i" => format!("\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP={hp}/MA=28]:"),
                            "spells" => format!(
                                "\r\nspells\r\nYou have the following spells:\r\nLevel Mana Short Spell Name\r\n  1   1    harm  harm\r\n  1   2    mihe  minor healing\r\n  2   4    bles  bless\r\n  7   8    cure  cure poison\r\n  8   6    mahe  major healing\r\n 10   5    rain  healing rain\r\n\r\n[HP={hp}/MA=28]:"
                            ),
                            "party" => format!(
                                "\r\nparty\r\n\x1b[1;36mYou are following Carrot.\x1b[0m\r\n\x1b[0mThe following people are in your travel party:\r\n\x1b[0m  Carrot                         (Paladin)    [M:100%] [H:100%]   - Frontrank\r\n  Potato                         (Paladin)    [M:100%] [H: 81%]   - Frontrank\r\n  Beef                           (Ninja)               [H:100%]   - Midrank\r\n  Blueberry                      (Mystic)     [K:100%] [H:100%]   - Midrank\r\n  Salad                          (Ranger)     [M: 21%] [H:100%] R - Midrank\r\n[HP={hp}/MA=28]:"
                            ),
                            "cast mihe salad" => format!("\r\ncast mihe salad\r\nYou cast minor healing on Salad!\r\n[HP={hp}/MA=28]:"),
                            "a beast" => format!("\r\na beast\r\n*Combat Engaged*\r\n[HP={hp}/MA=28]:"),
                            _ => format!("\r\n{line}\r\n[HP={hp}/MA=28]:"),
                        };
                        sock.write_all(reply.as_bytes()).await.unwrap();
                    }
                }
                _ = tick.tick() => {
                    let say = if step == 0 {
                        format!("\r\nYou are now following Carrot\r\n[HP={hp}/MA=28]:")
                    } else if step == 6 {
                        // The leader's kill ends a fight: the assist
                        // looks, the answer lists the next beast, and
                        // the fight it joins runs for the rest of the
                        // scenario.
                        format!("\r\n[HP={hp}/MA=28]:*Combat Off*\r\n")
                    } else if step == 10 {
                        // Glued to the prompt, the way the live board
                        // prints an async line, in the middle of a round.
                        format!("\r\n[HP={hp}/MA=28]:Carrot impales slime beast for 22 damage!\r\n[HP={hp}/MA=28]:The slime beast slaps Salad for 22 damage!\r\n[HP={hp}/MA=28]:Salad casts mend on Salad!\r\n[HP={hp}/MA=28]:Salad says \"@heal 65\"\r\n[HP={hp}/MA=28]:Salad is looking around the room.\r\n[HP={hp}/MA=28]:Salad moves to attack slime beast.\r\n")
                    } else if step > 6 {
                        format!("\r\n[HP={hp}/MA=28]:You impale slime beast for 10 damage!\r\n[HP={hp}/MA=28]:Blueberry jumpkicks slime beast for 6 damage!\r\n")
                    } else {
                        format!("\r\n[HP={hp}/MA=28]:")
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
/// probe the character casts, so it reads the spell listing before the
/// assist's first event.
fn world() -> Arc<World> {
    let home = GraphRoom { name: "Home".into(), ..Default::default() };
    let graph = RoomGraph::from_rooms(vec![(HOME, home)]);
    let mut content = Content::default();
    content.add_class(Class {
        id: ClassId(1),
        name: "Paladin".into(),
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
async fn a_healer_answers_a_fellow_follower_by_name() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("party_window_heal_member");
    let _ = std::fs::remove_dir_all(&base);
    // Before any other task exists in this process. The only test in
    // this binary, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let sent = Arc::new(Mutex::new(Vec::new()));
    let addr = heal_board(sent.clone()).await;
    // Never opened. The window asks the cache for this path and the
    // cache already has an answer.
    let db = base.join("world.sqlite");
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"potato\"").unwrap();
    s.set("farm.content", &format!("{:?}", db.to_string_lossy())).unwrap();
    s.set("bot.assist_play", "true").unwrap();
    s.set("bot.max_hp", "127").unwrap();
    s.set("bot.auto_heal", "true").unwrap();
    s.set("bot.auto_rest", "false").unwrap();
    s.set("bot.minor_heal_at_percent", "70").unwrap();
    s.set("bot.major_heal_at_percent", "40").unwrap();
    s.set("party.poll_secs", "1").unwrap();
    let mut cache = ContentCache::default();
    cache.insert(db, world());
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn(4, s, 200, 80, Arc::new(Mutex::new(cache)), tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        if sent.lock().unwrap().iter().any(|l| l.as_str() == "cast mihe salad") {
            break;
        }
        if tokio::time::timeout_at(deadline, front.recv()).await.is_err() {
            break;
        }
    }

    let log = sent.lock().unwrap().clone();
    let screen = handle.screen.lock().unwrap().text();
    assert!(log.iter().any(|l| l.as_str() == "a beast"), "the fight was joined: {log:?}");
    let casts = log.iter().filter(|l| l.as_str() == "cast mihe salad").count();
    assert_eq!(casts, 1, "one cast for the one request, mid-fight: {log:?}\nscreen:\n{screen}");
}

