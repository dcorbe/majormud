//! A window writes the death log. One test in this binary, because it
//! sets `XDG_CONFIG_HOME` for the whole process and nothing else may
//! run beside it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::settings::Settings;
use mud_client::tui::ContentCache;
use mud_client::window::{EventKind, FrontMsg, spawn};

/// A board that greets, waits for the window to settle, then kills the
/// character with the wording the dying player sees, and holds the
/// line open.
async fn killing_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\n[HP=22/MA=0]:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sock.write_all(b"\r\nYou have been killed.\r\n[HP=0/MA=0]:").await.unwrap();
        let mut hold = [0u8; 256];
        while sock.read(&mut hold).await.unwrap_or(0) > 0 {}
    });
    addr
}

#[tokio::test]
async fn a_death_in_hand_play_is_logged_with_no_room_when_none_is_known() {
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("deathlog_window");
    let _ = std::fs::remove_dir_all(&base);
    // Before any other task exists in this process. The only test in
    // this binary, so nothing reads the environment concurrently.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };

    let addr = killing_board().await;
    let mut s = Settings::default();
    s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
    s.set("port", &addr.port().to_string()).unwrap();
    s.set("username", "\"beef\"").unwrap();
    let (tx, mut front) = tokio::sync::mpsc::unbounded_channel();
    let cache = Arc::new(Mutex::new(ContentCache::default()));
    let handle = spawn(7, s, 24, 80, cache, tx, None, None);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut connected = false;
    loop {
        let msg = tokio::time::timeout_at(deadline, front.recv())
            .await
            .expect("timed out waiting for the death to be logged")
            .expect("the window task ended");
        if matches!(msg, FrontMsg::Event { kind: EventKind::Connected, .. }) {
            connected = true;
        }
        if handle.screen.lock().unwrap().text().contains("death logged") {
            break;
        }
    }
    assert!(connected);
    let text = std::fs::read_to_string(base.join("mmc").join("deaths.log")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one death, one line: {text}");
    assert!(lines[0].ends_with(" beef - - unknown"), "{text}");
    // This board printed no stat sheet, so the only name the client
    // ever had was the profile's username, and that is what names the
    // dead character.
    let character = lines[0].split_whitespace().nth(1).expect("a character field");
    assert_eq!(character, "beef", "a death before any sheet is logged under the username: {text}");
}

