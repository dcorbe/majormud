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
    fake_board_with_evil().await.0
}

/// What the fake board made of the commands it was sent after login.
#[derive(Default)]
struct BoardLog {
    /// Recognised `SET WARNING <on|off>` commands.
    set_warning: AtomicUsize,
    /// Commands the board did not recognise, which it says out loud.
    /// Any of these is a client speaking the wrong dialect.
    unrecognised: AtomicUsize,
}

/// As [`fake_board`], but it also serves the real board's Warn-on-Evil
/// setting: `SET WARNING ON|OFF` — an explicit setter, not a toggle
/// (DLL: subcommand keyword at 0xd76e9, "Valid warning options: ON, OFF"
/// at 0xd7d68, and the master list at 0xd8027). Crucially it also does
/// what the live board does with anything else, which is say it out
/// loud; that is how a wrong verb shows up as a test failure instead of
/// a silent no-op.
async fn fake_board_with_evil() -> (std::net::SocketAddr, Arc<BoardLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(BoardLog::default());
    let counter = Arc::clone(&log);
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
        // Serve settings commands for as long as the client stays on.
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf[..n]).trim().to_lowercase();
            let reply = if line == "set warning off" {
                counter.set_warning.fetch_add(1, Ordering::SeqCst);
                mud_core::text::SET_EVIL_WARN_OFF.to_string()
            } else if line == "set warning on" {
                counter.set_warning.fetch_add(1, Ordering::SeqCst);
                mud_core::text::SET_EVIL_WARN_ON.to_string()
            } else {
                // Verbatim from the live board when it was sent the Rust
                // server's "set evil": unknown input is spoken aloud.
                counter.unrecognised.fetch_add(1, Ordering::SeqCst);
                format!("You say \"{line}\"")
            };
            sock.write_all(format!("\r\n{reply}\r\n[HP=33/MA=8]:").as_bytes())
                .await
                .unwrap();
        }
    });
    (addr, log)
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
// The live board takes `SET WARNING ON|OFF` -- an explicit setter with a
// required argument, per the DLL's "Valid warning options: ON, OFF".
// An earlier pass here sent the Rust server's `set evil` toggle at the
// MBBSEmu target and the board simply SAID it out loud, which is why the
// fake board above answers unknown input the same way.

/// The MBBSEmu dialect must name the setting the board actually has, and
/// say which way to set it -- once, with no guessing.
#[tokio::test]
async fn evil_warnings_are_turned_off_on_the_live_board_dialect() {
    let (addr, log) = fake_board_with_evil().await;
    let profile = profile_with(addr, true);
    let session = Session::connect(&profile, None).await.unwrap();

    dialect::login(&session, &profile).await.unwrap();

    assert_eq!(
        log.set_warning.load(Ordering::SeqCst),
        1,
        "an explicit setter needs exactly one command"
    );
    assert_eq!(
        log.unrecognised.load(Ordering::SeqCst),
        0,
        "the board did not recognise what was sent -- wrong verb for this target"
    );
}

/// Opt-in means opt-in: an unset profile must not touch the character's
/// persistent state at all.
#[tokio::test]
async fn evil_warnings_are_untouched_when_the_profile_says_nothing() {
    let (addr, log) = fake_board_with_evil().await;
    let profile = profile_with(addr, false);
    let session = Session::connect(&profile, None).await.unwrap();

    dialect::login(&session, &profile).await.unwrap();

    assert_eq!(log.set_warning.load(Ordering::SeqCst), 0, "must not touch the character");
    assert_eq!(log.unrecognised.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------
// Realm presence: is the character standing in the game, or in front of
// a login prompt / the module menu?
//
// Anything the client sends on a timer has to know this. The level poll
// shipped without it and fired its first `exp` the instant the socket
// opened — into the username prompt (live, 2026-08-01).
// ---------------------------------------------------------------------

use mud_client::dialect::realm_presence;
use mud_client::events::Event;

/// A game prompt is the proof `dialect::login` itself waits for.
#[test]
fn a_game_prompt_means_we_are_in_the_realm() {
    assert_eq!(
        realm_presence(&Event::Prompt { hp: 43, mana: Some(10) }),
        Some(true)
    );
}

/// The module menu is NOT the game. A command typed here is read as a
/// menu key, which is how an `exp` becomes an unintended selection.
#[test]
fn the_module_menu_means_we_are_not() {
    assert_eq!(
        realm_presence(&Event::Line("[MAJORMUD]:".into())),
        Some(false)
    );
    assert_eq!(
        realm_presence(&Event::Line("Make your selection".into())),
        Some(false)
    );
}

/// Most traffic settles nothing, and must not be read as either — a
/// silent stretch of combat does not mean we left.
#[test]
fn ordinary_traffic_settles_nothing() {
    assert_eq!(
        realm_presence(&Event::Line("The cave bear bites you for 14 damage!".into())),
        None
    );
    assert_eq!(realm_presence(&Event::Line("Username:".into())), None);
}
