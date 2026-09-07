//! Where the character's name comes from. The automation matches its own
//! death line against it, so it has to be right before a job starts.
//! The board prints it on the stat sheet, and the profile's `username`
//! only stands in until the first sheet is read.

use std::time::Duration;

use mud_client::dialect::Target;
use mud_client::profile::Profile;
use mud_client::session::Session;

/// A board that reads and then says nothing, so a session connects and
/// no sheet ever arrives.
async fn silent_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 64];
        let _ = sock.read(&mut buf).await;
    });
    addr
}

const BEEF_SHEET: &str = "\
Name: Beef                             Lives/CP:    9/100
Race: Dark-Elf    Exp: 0               Perception:     43
Class: Ninja      Level: 1             Stealth:        56
Hits:    22/22    Armour Class:   0/0  Thievery:        0
Strength:  40     Agility: 50          Tracking:       26";

/// A board that echoes what it is sent and answers with Beef's sheet and
/// an ordinary prompt.
async fn sheet_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut pending = String::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim().to_string();
                let body = BEEF_SHEET.lines().collect::<Vec<_>>().join("\r\n");
                let reply = format!("\r\n{line}\r\n{body}\r\n[HP=51/MA=9]:");
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        }
    });
    addr
}

fn profile_for(addr: std::net::SocketAddr, username: &str) -> Profile {
    Profile {
        target: Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: username.into(),
        pace_ms: Some(0),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_profile_names_the_character_until_a_sheet_arrives() {
    let addr = silent_board().await;
    let session = Session::connect(&profile_for(addr, "dan"), None).await.unwrap();
    assert_eq!(session.character_name().as_deref(), Some("dan"));
}

#[tokio::test]
async fn an_empty_profile_and_no_sheet_leave_the_character_unnamed() {
    let addr = silent_board().await;
    let session = Session::connect(&profile_for(addr, ""), None).await.unwrap();
    assert_eq!(session.character_name(), None);
}

/// The point of the whole change. The board names the character, so an
/// empty `username` is no longer a reason to refuse to run.
#[tokio::test]
async fn the_stat_sheet_names_the_character() {
    let addr = sheet_board().await;
    let session = Session::connect(&profile_for(addr, ""), None).await.unwrap();
    assert_eq!(session.character_name(), None, "no sheet has been read yet");

    session.send("stat");
    session
        .expect("[HP=51/MA=9]:", Duration::from_secs(5))
        .await
        .expect("stat reply");

    assert_eq!(session.character_name().as_deref(), Some("Beef"));
}

/// The sheet wins over the profile. The account name and the character
/// name are different things, and only the character dies.
#[tokio::test]
async fn the_sheet_outranks_the_profile() {
    let addr = sheet_board().await;
    let session = Session::connect(&profile_for(addr, "dan"), None).await.unwrap();

    session.send("stat");
    session
        .expect("[HP=51/MA=9]:", Duration::from_secs(5))
        .await
        .expect("stat reply");

    assert_eq!(session.character_name().as_deref(), Some("Beef"));
}

