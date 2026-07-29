//! Login dialect tests.
//!
//! `tests/live_server.rs` covers the Rust-server flow against the real
//! in-process server. The MBBSEmu flow has no in-process equivalent, so
//! it is replayed here by a fake board scripted from a real board
//! capture (2026-07-29): the BBS username/password exchange, the module
//! menu, the `[MAJORMUD]:` menu, and the room block that only arrives
//! once `E` has been sent. Credentials in the fixture are dummies.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mud_client::dialect::{self, LoginOutcome, Target};
use mud_client::profile::Profile;
use mud_client::session::Session;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn profile_for(addr: std::net::SocketAddr) -> Profile {
    profile_with(addr, false)
}

fn profile_with(addr: std::net::SocketAddr, disable_evil_warnings: bool) -> Profile {
    Profile {
        target: Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        // Credentials are dummies. The dialect only relays whatever the
        // profile holds, so the fixture must never carry a real one.
        username: "testuser".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        disable_evil_warnings,
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
    fake_board_with_evil(true).await.0
}

/// As [`fake_board`], but it also models `set evil` the way the real one
/// does: a TOGGLE that reports the state it landed in, not a setter. The
/// returned counter is how many `set evil` commands the client sent.
async fn fake_board_with_evil(
    mut warn_on_evil: bool,
) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sets = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&sets);
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
        // Serve `set evil` for as long as the client stays connected.
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            if line.contains("set evil") {
                counter.fetch_add(1, Ordering::SeqCst);
                warn_on_evil = !warn_on_evil;
                let confirm = if warn_on_evil {
                    mud_core::text::SET_EVIL_WARN_ON
                } else {
                    mud_core::text::SET_EVIL_WARN_OFF
                };
                sock.write_all(format!("\r\n{confirm}\r\n[HP=33/MA=8]:").as_bytes())
                    .await
                    .unwrap();
            }
        }
    });
    (addr, sets)
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

// --- the evil-warning preference -------------------------------------
//
// `set evil` is a TOGGLE that reports the state it landed in, not a
// setter (mud_core::game::set_command). Sending it blindly is therefore
// wrong in exactly the case the operator cares about: a character whose
// warnings are ALREADY off would have them switched back ON, and the
// next unprovoked swing would be refused again.

/// Warnings on: one `set evil` is enough, and the client must not send a
/// second one that would switch them straight back on.
#[tokio::test]
async fn evil_warnings_on_are_switched_off_with_one_command() {
    let (addr, sets) = fake_board_with_evil(true).await;
    let profile = profile_with(addr, true);
    let session = Session::connect(&profile, None).await.unwrap();

    dialect::login(&session, &profile).await.unwrap();

    assert_eq!(sets.load(Ordering::SeqCst), 1, "should settle in one toggle");
}

/// Warnings already off: the first `set evil` turns them ON, so the
/// client has to notice and send a second. This is the case a naive
/// fire-and-forget implementation gets backwards.
#[tokio::test]
async fn evil_warnings_already_off_are_left_off() {
    let (addr, sets) = fake_board_with_evil(false).await;
    let profile = profile_with(addr, true);
    let session = Session::connect(&profile, None).await.unwrap();

    dialect::login(&session, &profile).await.unwrap();

    assert_eq!(
        sets.load(Ordering::SeqCst),
        2,
        "first toggle turned warnings ON; a second is required to land OFF"
    );
}

/// Opt-in means opt-in: an unset profile must not touch the character's
/// persistent state at all.
#[tokio::test]
async fn evil_warnings_are_untouched_when_the_profile_says_nothing() {
    let (addr, sets) = fake_board_with_evil(true).await;
    let profile = profile_with(addr, false);
    let session = Session::connect(&profile, None).await.unwrap();

    dialect::login(&session, &profile).await.unwrap();

    assert_eq!(sets.load(Ordering::SeqCst), 0, "must not touch the character");
}
