//! A session's profile is replaced by `/set`, and `/disconnect` closes it.

use std::time::Duration;

use mud_client::profile::Profile;
use mud_client::session::{ExpectError, Session};

async fn silent_board() -> (std::net::SocketAddr, tokio::sync::oneshot::Receiver<usize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 64];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let _ = tx.send(n);
    });
    (addr, rx)
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

#[tokio::test]
async fn set_profile_is_what_the_next_reader_sees() {
    let (addr, _) = silent_board().await;
    let session = Session::connect(&profile_for(addr), None).await.unwrap();
    assert_eq!(session.profile().username, "dan");
    let mut changed = session.profile();
    changed.username = "ann".into();
    session.set_profile(changed);
    assert_eq!(session.profile().username, "ann");
}

#[tokio::test]
async fn close_ends_the_streams_and_the_board_sees_the_line_drop() {
    let (addr, saw_close) = silent_board().await;
    let session = Session::connect(&profile_for(addr), None).await.unwrap();
    let mut raw = session.raw();
    session.close();
    assert!(
        matches!(raw.recv().await, Err(tokio::sync::broadcast::error::RecvError::Closed)),
        "the raw stream must end"
    );
    assert!(
        matches!(
            session.expect("anything", Duration::from_secs(2)).await,
            Err(ExpectError::Closed { .. })
        ),
        "a pending expect must fail at once, not wait out its timeout"
    );
    assert_eq!(saw_close.await.unwrap(), 0, "the board reads end of file");
}

/// A rest the client sends is explained on the session, and a window
/// subscribed to the session reads the reason. Closing ends the stream
/// the way it ends the others.
#[tokio::test]
async fn rest_reports_reach_a_subscriber_until_close() {
    let (addr, _) = silent_board().await;
    let session = Session::connect(&profile_for(addr), None).await.unwrap();
    let mut rests = session.rests();
    session.report_rest("assist: rest, hp 12/52 23%".into());
    assert_eq!(rests.recv().await.unwrap(), "assist: rest, hp 12/52 23%");
    session.close();
    assert!(matches!(rests.recv().await, Err(tokio::sync::broadcast::error::RecvError::Closed)));
    assert!(matches!(session.rests().recv().await, Err(tokio::sync::broadcast::error::RecvError::Closed)));
}
