//! Login dialect tests.
//!
//! `tests/live_server.rs` covers the Rust-server flow against the real
//! in-process server. The MBBSEmu flow has no in-process equivalent, so
//! it is replayed here by a fake board scripted from a real board
//! capture (2026-07-29): the BBS username/password exchange, the module
//! menu, the `[MAJORMUD]:` menu, and the room block that only arrives
//! once `E` has been sent. Credentials in the fixture are dummies.

use mud_client::dialect::{self, LoginOutcome, Target};
use mud_client::profile::Profile;
use mud_client::session::Session;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn profile_for(addr: std::net::SocketAddr) -> Profile {
    Profile {
        target: Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        // Credentials are dummies. The dialect only relays whatever the
        // profile holds, so the fixture must never carry a real one.
        username: "testuser".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        bot: None,
        farm: None,
    }
}

/// The board's reply to `E`, verbatim from the capture except for the
/// anti-bot backspace junk, which the wire layer resolves before the
/// dialect ever sees it.
const REALM: &[u8] = b"\r\nMajorMUD - ROL\r\n\r\n\
=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=-=\r\n\r\n\
\x1b[1;36mNewhaven, Narrow Road\r\n\
    This narrow road is quite plain save for the various lanterns hanging from\r\n\
the trees around, and a large stone stairwell leading downwards.\r\n\
Obvious exits: North, East, West, Down\r\n\
[HP=33/MA=8]:";

/// Read from `sock` until `needle` shows up, so the fake board reacts to
/// what the client actually sent rather than to a fixed number of bytes.
async fn read_until(sock: &mut tokio::net::TcpStream, needle: &str) -> String {
    let mut seen = String::new();
    let mut buf = [0u8; 512];
    while !seen.contains(needle) {
        let n = sock
            .read(&mut buf)
            .await
            .expect("client disconnected early");
        assert!(n > 0, "client closed while waiting for {needle:?}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    seen
}

/// A fake MBBSEmu board that will only serve the realm to a client that
/// asks to enter it.
async fn fake_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Enter Username or enter \"NEW\" to create a new Account\r\nUsername: ")
            .await
            .unwrap();
        read_until(&mut sock, "testuser").await;
        sock.write_all(b"\r\nPassword: ").await.unwrap();
        read_until(&mut sock, "testpass").await;
        sock.write_all(
            b"\r\nFantasy awaits you in MajorMUD v1.11p-WG!\r\n\r\n\
              Please select one of the following:\r\n\r\n   A ... MajorMUD\r\n\r\n\
              Main Menu\r\nMake your selection (X to exit):  ",
        )
        .await
        .unwrap();
        read_until(&mut sock, "A").await;
        sock.write_all(
            b"\r\n M A J O R  M U D v1.11p-WG\r\n{Realm Of Legends}\r\n\r\n\
              [E] . Enter the Realm\r\n[X] . Exit Game\r\n\r\n[MAJORMUD]:",
        )
        .await
        .unwrap();
        // The realm is served only in response to E. A client that never
        // sends it sits at the menu forever, which is the bug this file
        // exists to pin.
        read_until(&mut sock, "E").await;
        sock.write_all(REALM).await.unwrap();
        // Hold the socket open so the client sees no EOF.
        let mut sink = [0u8; 512];
        while let Ok(n) = sock.read(&mut sink).await {
            if n == 0 {
                break;
            }
        }
    });
    addr
}

/// Logging in to the live board must leave the character standing in the
/// realm at a game prompt -- not parked at the `[MAJORMUD]:` menu, where
/// every subsequent game command would be swallowed by the menu parser.
#[tokio::test]
async fn mbbs_login_enters_the_realm() {
    let addr = fake_board().await;
    let profile = profile_for(addr);
    let session = Session::connect(&profile, None).await.unwrap();

    let outcome = dialect::login(&session, &profile).await.unwrap();

    assert_eq!(outcome, LoginOutcome::InGame);
    // Asserted against the parsed game state rather than by expecting
    // "[HP=" again: expect() consumes as it matches, so a second call
    // would wait for a *second* prompt and time out even on a login that
    // worked perfectly.
    let state = session.state().borrow().clone();
    assert_eq!(state.hp, 33, "login did not reach the realm's game prompt");
    assert_eq!(
        state
            .room
            .expect("no room block parsed -- still at the menu")
            .name,
        "Newhaven, Narrow Road"
    );
}
