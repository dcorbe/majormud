//! The vitals probe against a board whose transcript already holds the
//! word it waits for.
//!
//! Live incident, 2026-09-04, test_timing.log: the stat sheet the
//! client asks for at login carries an ability line, `Intellect: 60
//! Health:  40`, that nothing ever consumed from the transcript. The
//! probe's wait for `Health:` matched that stale line at once, read
//! nothing new since its own send, and answered None. `max_hp` stayed
//! 0, and a character at 6 of 47 hits kept farming: every percent
//! policy divides by that number and 0 turns them all off.

use std::time::Duration;

use mud_client::farm::{Vitals, discover_vitals};
use mud_client::profile::Profile;
use mud_client::session::Session;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SHEET: &str = concat!(
    "\r\nstat\r\n",
    "Name: Beef                             Lives/CP:      9/2    \r\n",
    "Race: Elf         Exp: 25246           Perception:     56\r\n",
    "Class: Ninja      Level: 5             Stealth:        83\r\n",
    "Hits:    47/47    Armour Class:  12/0  Thievery:        0\r\n",
    "Strength:  50     Agility: 82          Tracking:       53\r\n",
    "Intellect: 60     Health:  40          Martial Arts:   61\r\n",
    "Willpower: 40     Charm:   70          MagicRes:       45\r\n",
    "[HP=47]:",
);

/// A board that prints the sheet unasked, then answers `health` after
/// a pause long enough that a probe reading stale text has already
/// given its answer.
async fn board_with_a_stale_sheet(health: &'static str) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(SHEET.as_bytes()).await.unwrap();
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
                let reply = if line == "health" {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    format!("\r\nhealth\r\n{health}\r\n[HP=47]:")
                } else {
                    format!("\r\n{line}\r\nYou say \"{line}\"\r\n[HP=47]:")
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
        bank: Default::default(),
    };
    Session::connect(&profile, None).await.unwrap()
}

/// Wait until the whole sheet is in the transcript WITHOUT consuming
/// any of it: the client's own sheet probe reads events, not the
/// transcript, so live the stale line is still ahead of the cursor.
async fn wait_for_sheet(session: &Session) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !session.since(0).contains("MagicRes:") {
        assert!(tokio::time::Instant::now() < deadline, "sheet never arrived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_sheets_health_score_is_not_the_health_report() {
    let addr = board_with_a_stale_sheet("Health:    39/47    [82%]").await;
    let session = session_for(addr).await;
    wait_for_sheet(&session).await;

    let vitals = discover_vitals(&session).await;

    assert_eq!(vitals, Some(Vitals { max_hp: 47, max_mana: 0 }));
}

/// The pool shares the health report's line, so reading that one line
/// is enough for both maxima.
#[tokio::test]
async fn a_casters_pool_rides_the_same_line() {
    let addr = board_with_a_stale_sheet("Health:    29/29    [100%]  Mana:   8/18  [44%]").await;
    let session = session_for(addr).await;
    wait_for_sheet(&session).await;

    let vitals = discover_vitals(&session).await;

    assert_eq!(vitals, Some(Vitals { max_hp: 29, max_mana: 18 }));
}
