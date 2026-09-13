//! The assist learns the pools from the board. One test in this binary,
//! because it sets `XDG_CONFIG_HOME` for the whole process and nothing
//! else may run beside it.
//!
//! A profile that leaves `bot.max_hp` at zero means "ask the board", and
//! every job does through `discover_vitals`. The assist never did: it
//! ran on the profile's zero, and `Bot::hp_percent` refuses to decide on
//! a zero max, so a hand-played character never rested, healed, fled or
//! kept stealth up, whatever `/bot` said (live, 2026-09-08, Beef at
//! 102/102 with Stealth and `sneak` never sent).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::settings::Settings;
use mud_client::tui::{ContentCache, KeyOutcome};
use mud_client::window::{FrontMsg, WindowMsg, spawn};

/// A board that puts the character in the realm at 10 of 100 hits,
/// answers the realm-entry probe and `health` the way the live board
/// does, and prompts every so
/// often afterwards so the assist has a prompt to decide on once the
/// answer has been applied. Every line the client sends is logged.
async fn low_board(sent: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\n\x1b[1;36mDark Cave\r\nObvious exits: west\r\n[HP=10/MA=0]:")
            .await
            .unwrap();
        let mut buf = [0u8; 256];
        let mut pending = String::new();
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                n = sock.read(&mut buf) => {
                    let n = n.unwrap_or(0);
                    if n == 0 { return; }
                    pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                    while let Some(at) = pending.find('\n') {
                        let line = pending[..at].trim().to_string();
                        pending = pending[at + 1..].to_string();
                        sent.lock().unwrap().push(line.clone());
                        // The realm-entry probe waits on each reply's
                        // last line, so the sheet is stored at once
                        // rather than after three deadlines.
                        let reply = match line.as_str() {
                            "health" => "\r\nhealth\r\nHealth:   10/100   [10%]\r\n[HP=10/MA=0]:".to_string(),
                            "stat" => "\r\nstat\r\nName: Beef   Lives/CP: 9/2\r\nClass: Ninja   Level: 15   Stealth: 114\r\nMagicRes: 0\r\n[HP=10/MA=0]:".to_string(),
                            "inventory" => "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/100 - None [0%]\r\n[HP=10/MA=0]:".to_string(),
                            other => format!("\r\n{other}\r\n[HP=10/MA=0]:"),
                        };
                        sock.write_all(reply.as_bytes()).await.unwrap();
                    }
                }
                _ = tick.tick() => {
                    sock.write_all(b"\r\n[HP=10/MA=0]:").await.unwrap();
                }
            }
        }
    });
    addr
}

#[tokio::test]
async fn the_assist_asks_the_board_for_its_pools_and_rests_by_them() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("assist_window");
    let _ = std::fs::remove_dir_all(&base);
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let sent = Arc::new(Mutex::new(Vec::new()));
    let addr = low_board(sent.clone()).await;
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beef\"").unwrap();
    s.set("bot.assist_play", "true").unwrap();
    let cache = ContentCache::default();
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    // Tall, so the note about the learned pools is still on the screen
    // after the prompts the board keeps sending.
    let handle = spawn(7, s, 200, 80, Arc::new(Mutex::new(cache)), tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if sent.lock().unwrap().iter().any(|l| l == "rest") {
            break;
        }
        match tokio::time::timeout_at(deadline, front.recv()).await {
            Ok(Some(FrontMsg::Event { .. })) | Ok(Some(_)) => {}
            Ok(None) => panic!("the window task ended"),
            Err(_) => panic!("no rest in 20s; sent {:?}\nscreen:\n{}", sent.lock().unwrap(), handle.screen.lock().unwrap().text()),
        }
    }
    let log = sent.lock().unwrap().clone();
    let health = log.iter().position(|l| l == "health").expect("asked the board: {log:?}");
    let rest = log.iter().position(|l| l == "rest").unwrap();
    assert!(health < rest, "the rest came off the learned max: {log:?}");
    assert!(handle.screen.lock().unwrap().text().contains("100 hits"), "{}", handle.screen.lock().unwrap().text());
}

// ---------------------------------------------------------------------
// The assist sweeps loot through the window's room model, not through
// the bot's own `has_loot`. `new_assist` builds the assist bot with
// `auto_get` off for exactly this reason, so a `get` seen in the tests
// below can only have come from `Here::unswept_wanted`.
//
// A full stat sheet at 100 of 100 hits keeps the tests free of the
// rest and heal traffic `the_assist_asks_the_board_for_its_pools_and_rests_by_them`
// exists to prove. `bot.max_hp` is set on the profile so the vitals
// probe never asks the board for `health` at all.
// ---------------------------------------------------------------------

/// A board driven by a fixed reply table: a line found in `replies`
/// gets that exact reply, verbatim. Anything else gets the generic
/// echo `low_board` above falls back to. Every line sent is logged,
/// the same way.
async fn scripted_board(sent: Arc<Mutex<Vec<String>>>, replies: HashMap<String, String>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\n\x1b[1;36mDark Cave\r\nObvious exits: west\r\n[HP=100/MA=0]:")
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        let mut pending = String::new();
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                n = sock.read(&mut buf) => {
                    let n = n.unwrap_or(0);
                    if n == 0 { return; }
                    pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                    while let Some(at) = pending.find('\n') {
                        let line = pending[..at].trim().to_string();
                        pending = pending[at + 1..].to_string();
                        sent.lock().unwrap().push(line.clone());
                        let reply = replies
                            .get(line.as_str())
                            .cloned()
                            .unwrap_or_else(|| format!("\r\n{line}\r\n[HP=100/MA=0]:"));
                        sock.write_all(reply.as_bytes()).await.unwrap();
                    }
                }
                _ = tick.tick() => {
                    sock.write_all(b"\r\n[HP=100/MA=0]:").await.unwrap();
                }
            }
        }
    });
    addr
}

/// The realm-entry replies every test below needs: a full sheet at
/// 100 of 100 hits and no keys or spells to complicate the trace.
fn entry_replies() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert(
        "stat".to_string(),
        "\r\nstat\r\nName: Beef   Lives/CP: 9/2\r\nClass: Ninja   Level: 15   Stealth: 0\r\nMagicRes: 0\r\n[HP=100/MA=0]:".to_string(),
    );
    m.insert(
        "inventory".to_string(),
        "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/100 - None [0%]\r\n[HP=100/MA=0]:".to_string(),
    );
    m
}

/// A room render with a floor listing, the same shape `look` and
/// `look <direction>` both answer with. Echoes `cmd` first, the way
/// every other reply table entry here does: the correlator only
/// retires a pending command once its own echo has come back, so a
/// room block with no echo ahead of it answers nothing at all.
fn room_with_items(cmd: &str, name: &str, items: &str, exits: &str) -> String {
    format!("\r\n{cmd}\r\n\x1b[1;36m{name}\r\nYou notice {items} here.\r\nObvious exits: {exits}\r\n[HP=100/MA=0]:")
}

/// The board's acknowledgement of a `get`, worded so `bot::picked_up`
/// reads it and retires the pile.
fn picked_up(denom: &str, count: u32, noun: &str) -> String {
    format!("\r\nget {denom}\r\nYou picked up {count} {denom} {noun}\r\n[HP=100/MA=0]:")
}

/// A window and its settings, common to the three tests below: a fresh
/// config directory (its own, so the three can run in the same binary
/// under `--test-threads=1` without treading on each other), the
/// profile the loot tests share, and the assist on.
fn spawn_loot_window(
    dir: &str,
    addr: std::net::SocketAddr,
    ignore_coins: Option<&str>,
) -> mud_client::window::WindowHandle {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(dir);
    let _ = std::fs::remove_dir_all(&base);
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beef\"").unwrap();
    s.set("bot.assist_play", "true").unwrap();
    // Learned, not asked: with a max in the profile the vitals probe
    // never sends `health`, which would otherwise be one more line to
    // account for in every reply table below.
    s.set("bot.max_hp", "100").unwrap();
    if let Some(list) = ignore_coins {
        s.set("bot.ignore_coins", list).unwrap();
    }
    let cache = ContentCache::default();
    let (tx, front) = tokio::sync::mpsc::unbounded_channel();
    // The front channel is never read back in these three tests: the
    // wait loop below polls `sent` directly on a short tick instead of
    // waiting on a `FrontMsg`, so nothing here needs to hold `front`
    // open. Dropped at once, on purpose.
    drop(front);
    spawn(7, s, 200, 80, Arc::new(Mutex::new(cache)), tx, None, None)
}

/// Waits until `sent` holds a line equal to `want`, or panics with the
/// log so far once `secs` have passed.
async fn wait_for_sent(sent: &Arc<Mutex<Vec<String>>>, want: &str, secs: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if sent.lock().unwrap().iter().any(|l| l == want) {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("no {want:?} in {secs}s; sent {:?}", sent.lock().unwrap());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A wanted pile on the floor is swept by one `get`, and it can only
/// have come from the room model: the assist bot `new_assist` builds
/// is given `auto_get: false`, so its own `has_loot` sweep never fires.
#[tokio::test]
async fn the_assist_sweeps_a_wanted_pile_through_the_room_model() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut replies = entry_replies();
    replies.insert("look".to_string(), room_with_items("look", "Dark Cave", "11 silver nobles", "west"));
    replies.insert("get silver".to_string(), picked_up("silver", 11, "nobles"));
    let addr = scripted_board(sent.clone(), replies).await;
    let handle = spawn_loot_window("assist_window_loot", addr, None);

    // Sent only once realm entry's own traffic is done, so the
    // correlator's queue holds nothing older that a stray reply could
    // misattribute this `look`'s answer to.
    wait_for_sent(&sent, "inventory", 20).await;
    handle.msgs.send(WindowMsg::Outcome(KeyOutcome::Send("look".into()))).unwrap();
    wait_for_sent(&sent, "get silver", 20).await;

    let count = sent.lock().unwrap().iter().filter(|l| *l == "get silver").count();
    assert_eq!(count, 1, "swept more than once: {:?}", sent.lock().unwrap());
}

/// A `look <direction>` block answers for the neighbour, and it must
/// not seed the model the way an ordinary `look` does. Proven the only
/// way this seam can be: a pile named ONLY in the elsewhere block is
/// never swept, though the assist is otherwise sweeping freely, right
/// up to the elsewhere block itself.
#[tokio::test]
async fn a_directional_look_does_not_seed_the_room_model() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut replies = entry_replies();
    replies.insert("look".to_string(), room_with_items("look", "Dark Cave", "9 copper farthings", "west"));
    replies.insert("get copper".to_string(), picked_up("copper", 9, "farthings"));
    replies.insert("look north".to_string(), room_with_items("look north", "Narrow Path", "7 silver nobles", "south"));
    let addr = scripted_board(sent.clone(), replies).await;
    let handle = spawn_loot_window("assist_window_elsewhere", addr, None);

    wait_for_sent(&sent, "inventory", 20).await;
    handle.msgs.send(WindowMsg::Outcome(KeyOutcome::Send("look".into()))).unwrap();
    wait_for_sent(&sent, "get copper", 20).await;

    handle.msgs.send(WindowMsg::Outcome(KeyOutcome::Send("look north".into()))).unwrap();
    wait_for_sent(&sent, "look north", 20).await;
    // No terminal wording says "the model was not seeded"; the sweep
    // never firing over a bounded wait is the only evidence there is,
    // the same shape every timeout-as-proof in this suite takes.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !sent.lock().unwrap().iter().any(|l| l == "get silver"),
        "the elsewhere block seeded the model: {:?}",
        sent.lock().unwrap()
    );
}

/// A denomination on `[bot].ignore_coins` is never swept, though a
/// wanted one on the SAME floor still is: the model's sweep skips the
/// ignored pile rather than refusing to sweep the room at all.
#[tokio::test]
async fn an_ignored_denomination_is_never_swept() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut replies = entry_replies();
    replies.insert(
        "look".to_string(),
        room_with_items("look", "Dark Cave", "9 copper farthings, 5 gold pieces", "west"),
    );
    replies.insert("get gold".to_string(), picked_up("gold", 5, "pieces"));
    let addr = scripted_board(sent.clone(), replies).await;
    let handle = spawn_loot_window("assist_window_ignore", addr, Some(r#"["copper"]"#));

    wait_for_sent(&sent, "inventory", 20).await;
    handle.msgs.send(WindowMsg::Outcome(KeyOutcome::Send("look".into()))).unwrap();
    wait_for_sent(&sent, "get gold", 20).await;

    assert!(
        !sent.lock().unwrap().iter().any(|l| l == "get copper"),
        "swept an ignored denomination: {:?}",
        sent.lock().unwrap()
    );
}
