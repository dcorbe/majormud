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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::settings::Settings;
use mud_client::tui::ContentCache;
use mud_client::window::{FrontMsg, spawn};

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
