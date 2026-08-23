//! `Session::wielded()` -- the session-level wielded-weapon model,
//! seeded from an `i`/`inventory` listing and kept current by any
//! `"You are now holding <new>."` confirmation, wherever in the stream
//! it lands -- `session.rs`'s `feed_contents`/`feed_equipment`.
//!
//! `inventory_board` is `tests/contents.rs`'s own helper, duplicated
//! rather than shared: test crates do not share modules, and this
//! suite's existing pattern (`tests/farm_scripted.rs`'s header comment)
//! already accepts the repetition.

use std::time::Duration;

use mud_client::profile::Profile;
use mud_client::session::Session;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A board that echoes `i` with a realistic reply -- wrapped mid-item at
/// the terminal width, the way the real board does -- and a `wield`
/// that confirms a swap; otherwise just echoes whatever it is sent.
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
                } else if line.eq_ignore_ascii_case("inventory") {
                    "inventory\r\n\
                     You are carrying a dagger (Weapon Hand), a torch.\r\n\
                     Encumbrance: 12/2400 - None [0%]\r\n\
                     [HP=30/MA=0]:"
                        .to_string()
                } else if line.eq_ignore_ascii_case("eq dagger") {
                    "eq dagger\r\nYou are now holding a dagger.\r\n[HP=30/MA=0]:".to_string()
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

async fn wait_for<F: Fn() -> Option<T>, T>(f: F) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(v) = f() {
            return v;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("condition never became true");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn wielded_is_none_before_any_listing_is_read() {
    let addr = inventory_board().await;
    let session = session_for(addr).await;
    assert_eq!(session.wielded(), None);
}

/// The `i` path (`feed_contents`'s own seed call): a real `i` reply,
/// wrap-rejoined, marks the two-handed weapon `"(Two handed)"` -- that
/// alone must seed `Session::wielded()`.
#[tokio::test]
async fn an_i_reply_seeds_the_wielded_weapon() {
    let addr = inventory_board().await;
    let session = session_for(addr).await;
    session.send("i");
    let wielded = wait_for(|| session.wielded()).await;
    assert_eq!(wielded, "quarterstaff");
}

/// The `inventory` path (`Session::set_sheet`, fed by
/// `crate::farm::probe_sheet` -- the realm-entry primer every caller,
/// TUI or headless CLI, actually runs) seeds it too, off the OTHER
/// marker spelling.
#[tokio::test]
async fn probe_sheets_inventory_reply_also_seeds_the_wielded_weapon() {
    let addr = inventory_board().await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;
    assert_eq!(session.wielded().as_deref(), Some("a dagger"));
}

/// Once seeded, a genuine equip confirmation still overrides it --
/// `Equipment::observe` outranks a stale seed, exactly as it outranks
/// itself.
#[tokio::test]
async fn a_swap_after_seeding_still_updates_the_model() {
    let addr = inventory_board().await;
    let session = session_for(addr).await;
    session.send("i");
    let seeded = wait_for(|| session.wielded()).await;
    assert_eq!(seeded, "quarterstaff");

    session.send("eq dagger");
    let swapped = wait_for(|| session.wielded().filter(|w| w == "a dagger")).await;
    assert_eq!(swapped, "a dagger");
}
