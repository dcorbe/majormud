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

/// A board that speaks the MBBSEmu login `dialect::login` drives.
///
/// It greets the first caller and drops the line 300 ms later, which is
/// the drop a reconnect answers. Interactive play never sends
/// credentials, so the first caller only ever sees the greeting. The
/// second caller gets the whole script and the line is then held open.
async fn relogin_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome to the Test Board\r\nUsername: ").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(sock);
        let (mut sock, _) = listener.accept().await.unwrap();
        login_script(&mut sock).await;
        let mut hold = [0u8; 256];
        while sock.read(&mut hold).await.unwrap_or(0) > 0 {}
    });
    addr
}

/// The prompts `dialect::login` waits for, each served only once the
/// client has answered the one before it.
async fn login_script(sock: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt;
    sock.write_all(b"Welcome to the Test Board\r\nUsername: ").await.unwrap();
    read_until(sock, "dan").await;
    sock.write_all(b"\r\nPassword: ").await.unwrap();
    read_until(sock, "secret").await;
    sock.write_all(
        b"\r\nPlease select one of the following:\r\n\r\n   A ... MajorMUD\r\n\r\n\
          Main Menu\r\nMake your selection (X to exit):  ",
    )
    .await
    .unwrap();
    read_until(sock, "A").await;
    sock.write_all(b"\r\n[E] . Enter the Realm\r\n\r\n[MAJORMUD]:").await.unwrap();
    read_until(sock, "E").await;
    sock.write_all(b"\r\nNewhaven, Narrow Road\r\nObvious exits: North\r\n[HP=33/MA=8]:")
        .await
        .unwrap();
}

/// Read until the client has sent `needle`, so the board answers what
/// was actually typed rather than a fixed number of bytes.
async fn read_until(sock: &mut tokio::net::TcpStream, needle: &str) {
    use tokio::io::AsyncReadExt;
    let mut seen = String::new();
    let mut buf = [0u8; 512];
    while !seen.contains(needle) {
        let n = sock.read(&mut buf).await.expect("the client hung up early");
        assert!(n > 0, "the client closed while the board waited for {needle:?}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
}

/// A board that greets the first caller and drops the line, then
/// accepts every later caller and says nothing at all. A login against
/// it runs its full timeout with the socket still open, which is the
/// case a leaked session hides in. Each of those connections reports
/// when the client closed it.
async fn stalling_board(closes: tokio::sync::mpsc::UnboundedSender<()>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"Welcome\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(sock);
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let closes = closes.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                // The board never writes, so this read ends only when
                // the client closes its side.
                while sock.read(&mut buf).await.unwrap_or(0) > 0 {}
                let _ = closes.send(());
            });
        }
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
    async fn until(&mut self, what: &str, pred: impl FnMut(&FrontMsg) -> bool) -> FrontMsg {
        self.until_within(what, Duration::from_secs(5), pred).await
    }

    /// `until` with the wait named. A reconnect spends its delay before
    /// it dials, so those tests need longer than the five seconds every
    /// other wait here gets.
    async fn until_within(
        &mut self,
        what: &str,
        wait: Duration,
        mut pred: impl FnMut(&FrontMsg) -> bool,
    ) -> FrontMsg {
        let deadline = tokio::time::Instant::now() + wait;
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

    /// Wait until the screen says `what`. One action can move the bar
    /// and print a note, and the front end hears about both, so counting
    /// `Changed` messages is not a contract a test may rely on. Which of
    /// the two moved is one: a bar that ticked says `screen: false`, and
    /// that is what saves the front end a snapshot.
    async fn until_text(&mut self, what: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !self.text().contains(what) {
            tokio::time::timeout_at(deadline, self.front.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what:?} in {}", self.text()))
                .expect("the window task ended");
        }
    }
}

#[tokio::test]
async fn a_disconnected_window_takes_settings_commands_on_its_own_screen() {
    let mut r = rig(settings_for(None), None);
    r.handle
        .msgs
        .send(WindowMsg::Outcome(KeyOutcome::SetList { pattern: "host".into() }))
        .unwrap();
    r.until_text("host = \"\"").await;
    assert!(!r.handle.info.lock().unwrap().connected);
    r.handle
        .msgs
        .send(WindowMsg::Outcome(KeyOutcome::Send("look".into())))
        .unwrap();
    r.until_text("not connected").await;
    assert_eq!(
        r.handle.info.lock().unwrap().bar,
        "not connected  :23",
        "a window with no session names where it would connect"
    );
}

#[tokio::test]
async fn a_window_with_a_host_connects_at_once_and_shows_the_banner() {
    let addr = banner_board().await;
    let mut r = rig(settings_for(Some(addr)), None);
    r.until("connected", |m| matches!(m, FrontMsg::Event { kind: EventKind::Connected, .. })).await;
    r.until("the banner", |m| matches!(m, FrontMsg::Changed { screen: true, .. })).await;
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
    r.until_text("-- disconnected.").await;
    assert!(!r.handle.info.lock().unwrap().connected);
    // The last thing a window does on its way to idle is set the bar,
    // so the last message it sends is the one that carries it. A bar
    // that moved is not a screen that moved, and that is what lets the
    // front end keep the snapshot it already has.
    let mut last = None;
    while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(500), r.front.recv()).await {
        if matches!(msg, FrontMsg::Changed { .. }) {
            last = Some(msg);
        }
    }
    assert!(
        r.handle.info.lock().unwrap().bar.starts_with("not connected"),
        "the bar kept its play text"
    );
    assert!(
        matches!(last, Some(FrontMsg::Changed { screen: false, .. })),
        "the idle bar moved no rows: {last:?}"
    );
    r.handle
        .msgs
        .send(WindowMsg::Outcome(KeyOutcome::SetList { pattern: "port".into() }))
        .unwrap();
    r.until_text("port = ").await;
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
    let farm_died = Phase::Done { why: format!("{DIED} (3 kills, 2 loops)"), at: None };
    assert_eq!(event_for(&farm_died), EventKind::Died);
    let done = Phase::Done { why: "loops done".into(), at: None };
    assert_eq!(event_for(&done), EventKind::JobEnded(done.label()));
}

/// A line that closes on its own comes back without the operator,
/// credentials and all.
#[tokio::test]
async fn a_dropped_line_redials_and_logs_back_in() {
    let addr = relogin_board().await;
    let mut s = settings_for(Some(addr));
    s.set("username", "\"dan\"").unwrap();
    s.set("password", "\"secret\"").unwrap();
    s.set("pace_ms", "0").unwrap();
    s.set("reconnect", "true").unwrap();
    s.set("reconnect_delay_seconds", "1").unwrap();
    let mut r = rig(s, None);
    r.until("the first connect", |m| matches!(m, FrontMsg::Event { kind: EventKind::Connected, .. })).await;
    r.until("the drop", |m| matches!(m, FrontMsg::Event { kind: EventKind::Disconnected, .. })).await;
    r.until_within("the redial", Duration::from_secs(10), |m| {
        matches!(m, FrontMsg::Event { kind: EventKind::Reconnecting { attempt: 1 }, .. })
    })
    .await;
    r.until_within("the second connect", Duration::from_secs(10), |m| {
        matches!(m, FrontMsg::Event { kind: EventKind::Connected, .. })
    })
    .await;
    // The realm is the proof the credentials went through. This board
    // serves it only to a caller that answered every prompt, and the
    // window paints what the login was sent.
    r.until_text("Newhaven, Narrow Road").await;
    assert!(r.handle.info.lock().unwrap().connected);
}

/// A redial that cannot log in is worse than none. It would sit at the
/// username prompt with the operator none the wiser.
#[tokio::test]
async fn reconnect_without_a_password_says_so_and_stays_put() {
    let addr = hangup_board().await;
    let mut s = settings_for(Some(addr));
    s.set("username", "\"dan\"").unwrap();
    s.set("reconnect", "true").unwrap();
    s.set("reconnect_delay_seconds", "1").unwrap();
    let mut r = rig(s, None);
    r.until("the drop", |m| matches!(m, FrontMsg::Event { kind: EventKind::Disconnected, .. })).await;
    r.until_text("reconnect is on but username or password is empty").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, r.front.recv()).await {
        assert!(
            !matches!(msg, FrontMsg::Event { kind: EventKind::Reconnecting { .. }, .. }),
            "it dialled with no password"
        );
    }
}

/// The operator gets the last word. `/disconnect` while the wait runs
/// stops the window dialling behind their back.
#[tokio::test]
async fn disconnect_pauses_the_wait() {
    let addr = hangup_board().await;
    let mut s = settings_for(Some(addr));
    s.set("username", "\"dan\"").unwrap();
    s.set("password", "\"secret\"").unwrap();
    s.set("reconnect", "true").unwrap();
    s.set("reconnect_delay_seconds", "3").unwrap();
    let mut r = rig(s, None);
    r.until("the drop", |m| matches!(m, FrontMsg::Event { kind: EventKind::Disconnected, .. })).await;
    r.until_text("-- disconnected.").await;
    r.handle.msgs.send(WindowMsg::Outcome(KeyOutcome::Disconnect)).unwrap();
    r.until_text("reconnect paused").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, r.front.recv()).await {
        assert!(
            !matches!(msg, FrontMsg::Event { kind: EventKind::Reconnecting { .. }, .. }),
            "the paused window dialled anyway"
        );
    }
}

/// A login that fails has to hand the socket back. `Session` has no
/// `Drop`, `play` is the only other place that closes one, and this
/// path never reaches `play`. The reader task holds a `cmd_tx` clone,
/// so nothing else ends the writer either, and a board that accepts and
/// stalls would cost a live socket and two tasks every redial.
///
/// The redial gives its login three delays per prompt, so a one second
/// delay puts the second attempt about five seconds out rather than the
/// half minute a headless login would wait.
#[tokio::test]
async fn a_failed_login_closes_its_session() {
    let (closes_tx, mut closes) = tokio::sync::mpsc::unbounded_channel();
    let addr = stalling_board(closes_tx).await;
    let mut s = settings_for(Some(addr));
    s.set("username", "\"dan\"").unwrap();
    s.set("password", "\"secret\"").unwrap();
    s.set("pace_ms", "0").unwrap();
    s.set("reconnect", "true").unwrap();
    s.set("reconnect_delay_seconds", "1").unwrap();
    let mut r = rig(s, None);
    r.until("the first connect", |m| matches!(m, FrontMsg::Event { kind: EventKind::Connected, .. })).await;
    r.until("the drop", |m| matches!(m, FrontMsg::Event { kind: EventKind::Disconnected, .. })).await;
    r.until_within("the first redial", Duration::from_secs(10), |m| {
        matches!(m, FrontMsg::Event { kind: EventKind::Reconnecting { attempt: 1 }, .. })
    })
    .await;
    r.until_within("the second redial", Duration::from_secs(15), |m| {
        matches!(m, FrontMsg::Event { kind: EventKind::Reconnecting { attempt: 2 }, .. })
    })
    .await;
    // The first redial's socket is shut, so the board's reader saw EOF
    // rather than a connection an abandoned task still holds open.
    tokio::time::timeout(Duration::from_secs(5), closes.recv())
        .await
        .expect("the failed login left its socket open")
        .expect("the board stopped reporting");
}

/// `/disconnect` is the operator saying stop. It must not be answered
/// with a redial, which is what a board hanging up gets.
#[tokio::test]
async fn a_typed_disconnect_does_not_redial() {
    let addr = banner_board().await;
    let mut s = settings_for(Some(addr));
    s.set("username", "\"dan\"").unwrap();
    s.set("password", "\"secret\"").unwrap();
    s.set("reconnect", "true").unwrap();
    s.set("reconnect_delay_seconds", "1").unwrap();
    let mut r = rig(s, None);
    r.until("the connect", |m| matches!(m, FrontMsg::Event { kind: EventKind::Connected, .. })).await;
    r.handle.msgs.send(WindowMsg::Outcome(KeyOutcome::Disconnect)).unwrap();
    r.until_text("-- disconnected.").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, r.front.recv()).await {
        assert!(
            !matches!(msg, FrontMsg::Event { kind: EventKind::Reconnecting { .. }, .. }),
            "the operator said stop and the window dialled anyway"
        );
    }
}

/// A `/connect` that fails is not a reason to stop redialling. The
/// schedule stays armed and the next attempt comes after the delay.
#[tokio::test]
async fn a_failed_connect_keeps_redialling() {
    // Bound and dropped, so nothing is listening on a port that was
    // free a moment ago.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = closed.local_addr().unwrap();
    drop(closed);
    let mut s = settings_for(None);
    s.set("username", "\"dan\"").unwrap();
    s.set("password", "\"secret\"").unwrap();
    s.set("reconnect", "true").unwrap();
    s.set("reconnect_delay_seconds", "1").unwrap();
    let target = format!("{}:{}", addr.ip(), addr.port());
    let mut r = rig(s, Some(KeyOutcome::Connect { target: Some(target) }));
    r.until_text("-- connect failed, redialling in 1 seconds --").await;
    r.until_within("the redial", Duration::from_secs(10), |m| {
        matches!(m, FrontMsg::Event { kind: EventKind::Reconnecting { attempt: 1 }, .. })
    })
    .await;
}

