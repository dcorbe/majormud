//! The window's side of the party: the board's party lines reach the
//! screen as notices, and beginning to follow sends `set follow normal`.
//!
//! One test in this binary, because it sets `XDG_CONFIG_HOME` for the
//! whole process and nothing else may run beside it.
//!
//! `set follow normal` is how the operator wants to follow, not
//! automation, so it is not behind `/bot`. The assist is left off here
//! to keep the log to the lines this test is about.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::settings::Settings;
use mud_client::tui::ContentCache;
use mud_client::window::spawn;

/// A board that puts the character in the realm, then says the
/// character is now following Beef. Every line the client sends is
/// logged, and anything else is echoed back with a prompt.
async fn following_board(sent: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\n\x1b[1;36mDark Cave\r\nObvious exits: west\r\n[HP=30/MA=0]:")
            .await
            .unwrap();
        let mut buf = [0u8; 256];
        let mut pending = String::new();
        let mut follow = tokio::time::interval(Duration::from_millis(300));
        // The first tick is immediate, and the follow line is worth
        // saying only once the session is reading.
        follow.tick().await;
        let mut said = false;
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
                        sock.write_all(format!("\r\n{line}\r\n[HP=30/MA=0]:").as_bytes()).await.unwrap();
                    }
                }
                _ = follow.tick() => {
                    if said {
                        sock.write_all(b"\r\n[HP=30/MA=0]:").await.unwrap();
                    } else {
                        said = true;
                        sock.write_all(b"\r\nYou are now following Beef\r\n[HP=30/MA=0]:").await.unwrap();
                    }
                }
            }
        }
    });
    addr
}

#[tokio::test]
async fn the_window_notes_the_party_and_sets_follow_normal() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("party_window");
    let _ = std::fs::remove_dir_all(&base);
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let sent = Arc::new(Mutex::new(Vec::new()));
    let addr = following_board(sent.clone()).await;
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beef\"").unwrap();
    let cache = ContentCache::default();
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn(3, s, 200, 80, Arc::new(Mutex::new(cache)), tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if sent.lock().unwrap().iter().any(|l| l == "set follow normal") {
            break;
        }
        match tokio::time::timeout_at(deadline, front.recv()).await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the window task ended"),
            Err(_) => panic!(
                "no follow setting in 20s; sent {:?}\nscreen:\n{}",
                sent.lock().unwrap(),
                handle.screen.lock().unwrap().text()
            ),
        }
    }
    let screen = handle.screen.lock().unwrap().text();
    assert!(screen.contains("party: following Beef"), "{screen}");

    // Once per began-following change, not once per prompt after it.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let log = sent.lock().unwrap().clone();
    assert_eq!(
        log.iter().filter(|l| *l == "set follow normal").count(),
        1,
        "{log:?}"
    );
}
