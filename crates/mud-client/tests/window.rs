//! A window is a task: settings, a session when connected, a screen.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::settings::Settings;
use mud_client::tui::{ContentCache, KeyOutcome};
use mud_client::window::{EventKind, FrontMsg, WindowHandle, WindowMsg, spawn};

async fn banner_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\nUsername: ").await.unwrap();
        let mut hold = [0u8; 64];
        let _ = sock.read(&mut hold).await;
    });
    addr
}

/// A board that hangs up a moment after greeting.
async fn hangup_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(sock);
    });
    addr
}

fn settings_for(addr: Option<std::net::SocketAddr>) -> Settings {
    let mut s = Settings::default();
    if let Some(addr) = addr {
        s.set("host", &format!("{:?}", addr.ip().to_string())).unwrap();
        s.set("port", &addr.port().to_string()).unwrap();
    }
    s
}

struct Rig {
    handle: WindowHandle,
    front: tokio::sync::mpsc::UnboundedReceiver<FrontMsg>,
}

fn rig(settings: Settings, first: Option<KeyOutcome>) -> Rig {
    let (tx, front) = tokio::sync::mpsc::unbounded_channel();
    let cache = Arc::new(Mutex::new(ContentCache::default()));
    let handle = spawn(7, settings, 24, 80, cache, tx, None, first);
    Rig { handle, front }
}

impl Rig {
    async fn until(&mut self, what: &str, mut pred: impl FnMut(&FrontMsg) -> bool) -> FrontMsg {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let msg = tokio::time::timeout_at(deadline, self.front.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
                .expect("the window task ended");
            if pred(&msg) {
                return msg;
            }
        }
    }

    fn text(&self) -> String {
        self.handle.screen.lock().unwrap().text()
    }
}

#[tokio::test]
async fn a_disconnected_window_takes_settings_commands_on_its_own_screen() {
    let mut r = rig(settings_for(None), None);
    r.handle
        .msgs
        .send(WindowMsg::Outcome(KeyOutcome::SetList { pattern: "host".into() }))
        .unwrap();
    r.until("the listing", |m| matches!(m, FrontMsg::Changed { window: 7 })).await;
    assert!(r.text().contains("host = \"\""), "{}", r.text());
    assert!(!r.handle.info.lock().unwrap().connected);
    r.handle
        .msgs
        .send(WindowMsg::Outcome(KeyOutcome::Send("look".into())))
        .unwrap();
    r.until("the refusal", |m| matches!(m, FrontMsg::Changed { window: 7 })).await;
    assert!(r.text().contains("not connected"), "{}", r.text());
}

#[tokio::test]
async fn a_window_with_a_host_connects_at_once_and_shows_the_banner() {
    let addr = banner_board().await;
    let mut r = rig(settings_for(Some(addr)), None);
    r.until("connected", |m| matches!(m, FrontMsg::Event { kind: EventKind::Connected, .. })).await;
    r.until("the banner", |m| matches!(m, FrontMsg::Changed { .. })).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !r.text().contains("Welcome to the Test Board") {
        assert!(std::time::Instant::now() < deadline, "banner never painted: {}", r.text());
        r.until("more output", |m| matches!(m, FrontMsg::Changed { .. })).await;
    }
    let info = r.handle.info.lock().unwrap().clone();
    assert!(info.connected);
    assert_eq!(info.port, addr.port());
    assert!(!info.bar.is_empty(), "the window keeps its bar text current");
}

#[tokio::test]
async fn a_first_outcome_connects_a_fresh_window() {
    let addr = banner_board().await;
    let target = format!("{}:{}", addr.ip(), addr.port());
    let mut r = rig(settings_for(None), Some(KeyOutcome::Connect { target: Some(target) }));
    r.until("connected", |m| matches!(m, FrontMsg::Event { kind: EventKind::Connected, .. })).await;
    let info = r.handle.info.lock().unwrap().clone();
    assert_eq!(info.host, addr.ip().to_string());
    assert!(info.dirty, "the host and port were written as settings");
}

#[tokio::test]
async fn a_board_hanging_up_reports_disconnected_and_stays_a_window() {
    let addr = hangup_board().await;
    let mut r = rig(settings_for(Some(addr)), None);
    r.until("connected", |m| matches!(m, FrontMsg::Event { kind: EventKind::Connected, .. })).await;
    r.until("disconnected", |m| matches!(m, FrontMsg::Event { kind: EventKind::Disconnected, .. })).await;
    r.until("the note", |m| matches!(m, FrontMsg::Changed { .. })).await;
    assert!(r.text().contains("disconnected"), "{}", r.text());
    assert!(!r.handle.info.lock().unwrap().connected);
    r.handle
        .msgs
        .send(WindowMsg::Outcome(KeyOutcome::SetList { pattern: "port".into() }))
        .unwrap();
    r.until("still answering", |m| matches!(m, FrontMsg::Changed { .. })).await;
    assert!(r.text().contains("port = "), "{}", r.text());
}

#[tokio::test]
async fn dropping_the_handle_ends_the_task() {
    let r = rig(settings_for(None), None);
    let mut front = r.front;
    drop(r.handle);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, front.recv()).await.expect("ended in time") {
            Some(FrontMsg::Ended { window: 7 }) | None => break,
            Some(_) => continue,
        }
    }
}

#[tokio::test]
async fn resize_reaches_the_screen() {
    let r = rig(settings_for(None), None);
    r.handle.msgs.send(WindowMsg::Resize { rows: 30, cols: 100 }).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if r.handle.screen.lock().unwrap().size() == (28, 100) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the screen never resized");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn a_death_is_its_own_event() {
    use mud_client::farm::{DIED, Phase};
    use mud_client::window::event_for;
    let died = Phase::Done { why: DIED.to_string(), at: None };
    assert_eq!(event_for(&died), EventKind::Died);
    let done = Phase::Done { why: "loops done".into(), at: None };
    assert_eq!(event_for(&done), EventKind::JobEnded(done.label()));
}

