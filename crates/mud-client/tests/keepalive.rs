//! Staying connected while idle.
//!
//! An idle line at home drops well under a minute after the last
//! keystroke, while the board is still sending (Carrot and Blueberry,
//! 2026-09-12). Three things answer that: the socket keeps itself alive
//! with kernel probes, the timing log says who hung up, and a profile can
//! ask for a telnet no-op after a quiet spell, which the board's telnet
//! layer eats without printing anything.

use std::time::Duration;

use mud_client::profile::Profile;
use mud_client::session::{Capture, ExpectError, Session};

/// A board that reads what the caller sends and reports each read.
async fn listening_board() -> (
    std::net::SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    tokio::sync::oneshot::Receiver<std::net::SocketAddr>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let (peer_tx, peer_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut sock, peer) = listener.accept().await.unwrap();
        let _ = peer_tx.send(peer);
        let mut buf = [0u8; 256];
        loop {
            let n = match sock.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    (addr, rx, peer_rx)
}

fn profile_for(addr: std::net::SocketAddr) -> Profile {
    Profile {
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "dan".into(),
        pace_ms: Some(0),
        ..Default::default()
    }
}

fn scratch(name: &str) -> Capture {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let raw = dir.join(format!("{name}.raw"));
    let timing = dir.join(format!("{name}_timing.log"));
    let _ = std::fs::remove_file(&raw);
    let _ = std::fs::remove_file(&timing);
    Capture { raw, timing: Some(timing) }
}

fn timing_log(capture: &Capture) -> String {
    std::fs::read_to_string(capture.timing.as_ref().unwrap()).expect("timing log exists")
}

/// The kernel's keepalive timer for a local TCP socket, from
/// `/proc/net/tcp`: `Some(ticks until it fires)` when armed.
fn keepalive_timer(local: std::net::SocketAddr) -> Option<u64> {
    let want = format!("0100007F:{:04X}", local.port());
    let table = std::fs::read_to_string("/proc/net/tcp").expect("/proc/net/tcp");
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(1) != Some(&want.as_str()) {
            continue;
        }
        let (timer, when) = fields[5].split_once(':').unwrap();
        return (timer == "02").then(|| u64::from_str_radix(when, 16).unwrap());
    }
    panic!("no /proc/net/tcp row for {want}");
}

#[tokio::test]
async fn a_connected_socket_keeps_itself_alive() {
    let (addr, _reads, peer) = listening_board().await;
    let _session = Session::connect(&profile_for(addr), None).await.unwrap();
    let local = peer.await.unwrap();
    let when = keepalive_timer(local).expect("the keepalive timer is armed at connect");
    // USER_HZ ticks. The first probe goes out inside fifteen quiet
    // seconds, so a NAT that forgets a line inside a minute never gets
    // the chance.
    assert!(when <= 15 * 100, "first probe due in {when} ticks");
}

#[tokio::test]
async fn a_board_that_hangs_up_is_logged_as_end_of_stream() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(sock);
    });
    let capture = scratch("hangup");
    let session = Session::connect(&profile_for(addr), Some(capture.clone())).await.unwrap();
    assert!(
        matches!(session.expect("anything", Duration::from_secs(2)).await, Err(ExpectError::Closed { .. })),
        "the line ends"
    );
    let log = timing_log(&capture);
    assert!(log.contains(" !! connection closed: end of stream"), "{log}");
    assert_eq!(session.close_reason().as_deref(), Some("end of stream"));
}

#[tokio::test]
async fn a_reset_line_is_logged_with_the_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        // Linger of zero turns the close into a reset, which is what a
        // NAT box that forgot the line answers with.
        socket2::SockRef::from(&sock).set_linger(Some(Duration::ZERO)).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(sock);
    });
    let capture = scratch("reset");
    let session = Session::connect(&profile_for(addr), Some(capture.clone())).await.unwrap();
    assert!(
        matches!(session.expect("anything", Duration::from_secs(2)).await, Err(ExpectError::Closed { .. })),
        "the line ends"
    );
    let log = timing_log(&capture);
    assert!(log.contains(" !! connection closed: Connection reset by peer"), "{log}");
    assert!(session.close_reason().unwrap().starts_with("Connection reset by peer"));
}

#[tokio::test]
async fn keepalive_is_off_by_default() {
    assert_eq!(Profile::default().keepalive_seconds, 0);
    let p: Profile = toml::from_str("keepalive_seconds = 30\n").unwrap();
    assert_eq!(p.keepalive_seconds, 30);
}

#[tokio::test]
async fn a_quiet_line_gets_a_telnet_nop_and_nothing_else() {
    let (addr, mut reads, _) = listening_board().await;
    let mut profile = profile_for(addr);
    profile.keepalive_seconds = 1;
    let capture = scratch("nop");
    let _session = Session::connect(&profile, Some(capture.clone())).await.unwrap();
    let first = tokio::time::timeout(Duration::from_millis(1500), reads.recv())
        .await
        .expect("a no-op inside the quiet second and a half")
        .unwrap();
    assert_eq!(first, vec![255, 241], "IAC NOP");
    let second = tokio::time::timeout(Duration::from_millis(1500), reads.recv())
        .await
        .expect("it repeats while the line stays quiet")
        .unwrap();
    assert_eq!(second, vec![255, 241]);
    assert!(timing_log(&capture).contains(" !! keepalive"));
}

#[tokio::test]
async fn a_send_postpones_the_nop() {
    let (addr, mut reads, _) = listening_board().await;
    let mut profile = profile_for(addr);
    profile.keepalive_seconds = 1;
    let session = Session::connect(&profile, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    session.send("look");
    let first = reads.recv().await.unwrap();
    assert_eq!(first, b"look\r\n", "the line goes out as typed");
    // The quiet second restarts at the send, so nothing arrives before
    // 1.7s from connect: at 1.3s the board has seen only the look.
    let early = tokio::time::timeout(Duration::from_millis(600), reads.recv()).await;
    assert!(early.is_err(), "no-op arrived early: {early:?}");
    let late = tokio::time::timeout(Duration::from_millis(600), reads.recv()).await.unwrap().unwrap();
    assert_eq!(late, vec![255, 241]);
}

#[tokio::test]
async fn zero_never_pokes_and_a_live_setting_turns_it_on() {
    let (addr, mut reads, _) = listening_board().await;
    let session = Session::connect(&profile_for(addr), None).await.unwrap();
    let quiet = tokio::time::timeout(Duration::from_millis(1500), reads.recv()).await;
    assert!(quiet.is_err(), "off by default, yet the board read {quiet:?}");
    let mut profile = session.profile();
    profile.keepalive_seconds = 1;
    session.set_profile(profile);
    let poke = tokio::time::timeout(Duration::from_millis(1500), reads.recv())
        .await
        .expect("/set keepalive_seconds takes effect on the live line")
        .unwrap();
    assert_eq!(poke, vec![255, 241]);
}
