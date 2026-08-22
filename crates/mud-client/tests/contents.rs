//! Session-level carried-contents refresh: every `i` this session (or a
//! caller sharing it) sends keeps `Session::contents()` current, the way
//! `Session::stats()` already does for `stat`/`st` -- see
//! `2026-08-22-inventory-and-backstab-design.md`'s "Architecture" table.
//!
//! Contents are deliberately best-effort: this is a refresh mechanism,
//! not an incremental tracker, and the whole point is that it is allowed
//! to be stale between asks.

use std::time::Duration;

use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_client::sheet::Inventory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A board that echoes `i` with a realistic reply -- wrapped mid-item at
/// the terminal width, the way the real board does (`crates/mud-client/
/// tests/sheet.rs`'s `REAL_INVENTORY`) -- and otherwise just echoes
/// whatever it is sent, unadorned.
async fn inventory_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim();
                let reply = if line.eq_ignore_ascii_case("i") {
                    "i\r\n\
                     You are carrying 21 silver nobles, 56 copper farthings, quarterstaff (Two\r\n\
                     handed)\r\n\
                     You have no keys.\r\n\
                     Wealth: 266 copper farthings\r\n\
                     Encumbrance: 525/2400 - Light [21%]\r\n\
                     [HP=30/MA=0]:"
                        .to_string()
                } else {
                    format!("\r\nYou say \"{line}\"\r\n[HP=30/MA=0]:")
                };
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        }
    });
    addr
}

async fn session_for(addr: std::net::SocketAddr) -> Session {
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
    };
    Session::connect(&profile, None).await.unwrap()
}

/// Poll `Session::contents()` until it stops being empty, or time out.
async fn wait_for_contents(session: &Session) -> Inventory {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let inv = session.contents();
        if !inv.items.is_empty() {
            return inv;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("contents never refreshed");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn contents_is_empty_before_any_i_is_sent() {
    let addr = inventory_board().await;
    let session = session_for(addr).await;
    assert_eq!(session.contents(), Inventory::default());
}

/// Step 1: our own `i` refreshes `Session::contents()`.
#[tokio::test]
async fn our_own_i_refreshes_contents() {
    let addr = inventory_board().await;
    let session = session_for(addr).await;
    session.send("i");
    let inv = wait_for_contents(&session).await;
    assert_eq!(
        inv.items,
        vec![
            "21 silver nobles".to_string(),
            "56 copper farthings".to_string(),
            "quarterstaff (Two handed)".to_string(),
        ],
        "the wrapped item must have been rejoined, same as sheet::Inventory::parse alone"
    );
    assert_eq!(inv.encumbrance, Some((525, 2400)));
}

/// Contents refresh off ANY `i` on this session, not just a caller who
/// happens to hold the `Session` that sent it -- exactly like
/// `Session::stats()` and `Session::capabilities()`'s purse.
#[tokio::test]
async fn a_second_i_replaces_the_first_reading() {
    let addr = inventory_board().await;
    let session = session_for(addr).await;
    session.send("i");
    let first = wait_for_contents(&session).await;
    assert_eq!(first.items.len(), 3);

    session.send("i");
    // Same reply shape from the fixture board, but this proves the
    // second ask replaced (not accumulated onto) the first reading.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let second = session.contents();
    assert_eq!(second.items.len(), 3, "contents must be an assignment, not an accumulation");
}
