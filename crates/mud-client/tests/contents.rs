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

/// The same board with one key on the ring, the live reply of
/// 2026-09-05.
async fn keyed_inventory_board() -> std::net::SocketAddr {
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
                     You are carrying 38 runic coins, ninjato (Weapon Hand)\r\n\
                     You have the following keys:  black star key.\r\n\
                     Wealth: 38982274 copper farthings\r\n\
                     Encumbrance: 430/1680 - Light [25%]\r\n\
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

/// The same board with two keys on the ring, for the second-table
/// refresh test: the reply never changes, only what the session knows
/// how to resolve does.
async fn two_keys_inventory_board() -> std::net::SocketAddr {
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
                     You are carrying 38 runic coins, ninjato (Weapon Hand)\r\n\
                     You have the following keys:  black star key, second key.\r\n\
                     Wealth: 38982274 copper farthings\r\n\
                     Encumbrance: 430/1680 - Light [25%]\r\n\
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

fn key_table() -> std::sync::Arc<mud_core::content::Content> {
    use mud_core::content::{Content, Item, ItemId};
    let mut content = Content::default();
    content.add_item(Item {
        id: ItemId(172),
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    content.add_item(Item {
        id: ItemId(500),
        name: "ninjato".into(),
        item_type: 1,
        ..Default::default()
    });
    std::sync::Arc::new(content)
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

/// The pack rides the same reply as the contents: once the session has
/// the item table, every `i` refreshes both, and `capabilities()` hands
/// out the shared handle.
#[tokio::test]
async fn an_i_reply_refreshes_the_capabilities_pack() {
    use mud_core::content::ItemId;
    let addr = keyed_inventory_board().await;
    let session = session_for(addr).await;
    session.set_content(key_table());
    assert!(!session.capabilities().has_item(ItemId(172)));
    session.send("i");
    wait_for_contents(&session).await;
    let caps = session.capabilities();
    assert!(caps.has_item(ItemId(172)), "the ring key");
    assert!(caps.has_item(ItemId(500)), "the wielded weapon");
    assert!(!caps.has_item(ItemId(501)));
}

/// Without the table there is no pack to refresh, and capabilities say
/// so rather than pretending.
#[tokio::test]
async fn without_the_table_there_is_no_pack() {
    use mud_core::content::ItemId;
    let addr = keyed_inventory_board().await;
    let session = session_for(addr).await;
    session.send("i");
    wait_for_contents(&session).await;
    assert!(session.pack_handle().is_none());
    assert!(!session.capabilities().has_item(ItemId(172)));
}

/// Handing the table over after a reply has already landed resolves
/// the reading the session already holds.
#[tokio::test]
async fn set_content_resolves_the_reading_already_held() {
    use mud_core::content::ItemId;
    let addr = keyed_inventory_board().await;
    let session = session_for(addr).await;
    session.send("i");
    wait_for_contents(&session).await;
    session.set_content(key_table());
    assert!(session.capabilities().has_item(ItemId(172)));
}

/// A second `set_content` swaps the table but must stay the SAME
/// shared pack: a handle taken out after the first call is still the
/// pack every later reply refreshes, or it goes stale forever the
/// moment a second table (a farm's own load, after `mmc play`'s realm
/// entry) is handed over.
#[tokio::test]
async fn a_second_set_content_keeps_the_pack_every_holder_shares() {
    use mud_core::content::{Content, Item, ItemId};
    let addr = two_keys_inventory_board().await;
    let session = session_for(addr).await;

    session.set_content(key_table());
    let first_handle = session.pack_handle().expect("a pack handle after the first set_content");

    let mut richer = Content::default();
    richer.add_item(Item {
        id: ItemId(172),
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    richer.add_item(Item {
        id: ItemId(173),
        name: "second key".into(),
        item_type: 7,
        ..Default::default()
    });
    session.set_content(std::sync::Arc::new(richer));

    session.send("i");
    wait_for_contents(&session).await;

    assert!(
        first_handle.has(ItemId(173)),
        "the handle taken before the second set_content never saw the new table's key"
    );
}
