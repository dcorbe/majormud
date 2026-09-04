//! Session-level attribution: the Correlator wired between the writer
//! task (registry, wire order) and the reader task (attribution, arrival
//! order), with `answers` carried inside every broadcast event.
//!
//! Driven against a scripted echoing board — the real board echoes every
//! accepted command (docs/board-correlation.md), and these scripts model
//! that, including the unsolicited stale block that used to satisfy the
//! next step and desync the walk.

use std::time::Duration;

use mud_client::correlate::Correlated;
use mud_client::events::Event;
use mud_client::events::Status;
use mud_client::profile::Profile;
use mud_client::session::Session;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn room_block(name: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: north, south\r\n[HP=30/MA=0]:")
}

/// A board that echoes accepted commands like the real one. On `n` it
/// front-runs the reply with an UNSOLICITED stale block — the desync
/// repro — before echoing and answering.
async fn echoing_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        // The login render: no command asked for it.
        sock.write_all(room_block("Guard Post").as_bytes()).await.unwrap();
        // Line-buffered: rapid sends coalesce into one read (Nagle), and
        // a board that trims the whole buffer would answer garbage.
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
                let reply = match line {
                    "look" => format!("\r\nlook{}", room_block("Guard Post")),
                    "n" => format!(
                        "{}\r\nn{}",
                        room_block("Guard Post"), // stale render, nobody asked
                        room_block("Inner Ward")  // the echo'd answer
                    ),
                    other => format!("\r\nYou say \"{other}\"\r\n[HP=30/MA=0]:"),
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

/// Collect events until `stop` says done or the deadline passes.
async fn collect(
    events: &mut tokio::sync::broadcast::Receiver<Correlated>,
    mut stop: impl FnMut(&[Correlated]) -> bool,
) -> Vec<Correlated> {
    let mut got = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !stop(&got) {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(ev)) => got.push(ev),
            _ => break,
        }
    }
    got
}

fn blocks(evs: &[Correlated]) -> Vec<&Correlated> {
    evs.iter()
        .filter(|c| matches!(c.event, Event::RoomSeen(_)))
        .collect()
}

#[tokio::test]
async fn the_block_after_our_echo_answers_our_send() {
    let addr = echoing_board().await;
    let session = session_for(addr).await;
    let mut events = session.events();
    let id = session.send("look");
    // Two blocks arrive: the unsolicited login render, then the look's
    // echoed answer.
    let evs = collect(&mut events, |got| blocks(got).len() >= 2).await;
    let bs = blocks(&evs);
    assert_eq!(bs.len(), 2, "{evs:?}");
    assert_eq!(bs[0].answers, None, "{evs:?}");
    assert_eq!(bs[1].answers, Some(id), "{evs:?}");
}

#[tokio::test]
async fn the_login_render_answers_nobody() {
    let addr = echoing_board().await;
    let session = session_for(addr).await;
    let mut events = session.events();
    // No send at all: the greeting block must flow, unattributed.
    let evs = collect(&mut events, |got| !blocks(got).is_empty()).await;
    let bs = blocks(&evs);
    assert_eq!(bs.len(), 1, "{evs:?}");
    assert_eq!(bs[0].answers, None, "{evs:?}");
}

#[tokio::test]
async fn a_stale_render_cannot_satisfy_the_step_that_did_not_ask() {
    // The desync repro, end to end: the board front-runs the step's
    // answer with a stale render of the room we are leaving. Attribution
    // must hand the step ONLY the block that follows its echo.
    let addr = echoing_board().await;
    let session = session_for(addr).await;
    let mut events = session.events();
    // Consume the login render first so the test sees only the step.
    let _ = collect(&mut events, |got| !blocks(got).is_empty()).await;
    let id = session.send("n");
    let evs = collect(&mut events, |got| blocks(got).len() >= 2).await;
    let bs = blocks(&evs);
    assert_eq!(bs.len(), 2, "{evs:?}");
    let stale = &bs[0];
    let answer = &bs[1];
    assert_eq!(stale.answers, None, "the stale render answered someone: {evs:?}");
    assert!(matches!(&stale.event, Event::RoomSeen(r) if r.name == "Guard Post"));
    assert_eq!(answer.answers, Some(id), "{evs:?}");
    assert!(matches!(&answer.event, Event::RoomSeen(r) if r.name == "Inner Ward"));
}

#[tokio::test]
async fn every_send_gets_a_distinct_id() {
    // Distinct is all that is promised. Numeric order equals wire order
    // only per sender — two tasks racing send() can enqueue opposite to
    // allocation — which is why CmdId does not implement Ord: equality
    // matching is the whole contract.
    let addr = echoing_board().await;
    let session = session_for(addr).await;
    let a = session.send("look");
    let b = session.send("look");
    let c = session.send("look");
    assert!(a != b && b != c && a != c, "{a:?} {b:?} {c:?}");
}

#[tokio::test]
async fn an_untrimmed_send_still_correlates() {
    // The TUI hands over raw editor lines and scripts hand over raw lua
    // strings; a trailing space must not defeat echo matching (or, in
    // debug builds, panic the writer task on sent()'s precondition).
    let addr = echoing_board().await;
    let session = session_for(addr).await;
    let mut events = session.events();
    let id = session.send("  look  ");
    let evs = collect(&mut events, |got| blocks(got).len() >= 2).await;
    let bs = blocks(&evs);
    assert_eq!(bs.len(), 2, "{evs:?}");
    assert_eq!(bs[1].answers, Some(id), "{evs:?}");
}

/// A board that paints a resting prompt on connect, then the same HP with
/// no status, then a changed HP.
async fn resting_board() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"\r\n[HP=30 (Resting) ]:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sock.write_all(b"\r\n[HP=30]:").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sock.write_all(b"\r\n[HP=31]:").await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    addr
}

/// Wait until the game state satisfies `done`, or fail after five seconds.
async fn state_reaches(
    state: &mut tokio::sync::watch::Receiver<mud_client::session::GameState>,
    what: &str,
    mut done: impl FnMut(&mud_client::session::GameState) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if done(&state.borrow()) {
            return;
        }
        tokio::time::timeout_at(deadline, state.changed())
            .await
            .unwrap_or_else(|_| panic!("state never reached: {what}"))
            .expect("session closed");
    }
}

#[tokio::test]
async fn the_game_state_follows_the_prompt_status() {
    let addr = resting_board().await;
    let session = session_for(addr).await;
    let mut state = session.state();
    state_reaches(&mut state, "resting at 30", |s| {
        s.hp == 30 && s.status == Some(Status::Resting)
    })
    .await;
    // Same HP, no status. A rest usually ends at unchanged vitals, and
    // only the status term in the state's change check tells a watcher
    // it ended.
    state_reaches(&mut state, "standing at 30", |s| s.hp == 30 && s.status.is_none()).await;
    state_reaches(&mut state, "standing at 31", |s| s.hp == 31).await;
}
