//! The session learns the party from the board's lines and keeps the
//! holds and requests for whoever drains them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::party::{Remote, Role};
use mud_client::profile::Profile;
use mud_client::session::Session;

/// A board that prints `lines` after the prompt, one every 50ms, and
/// echoes anything sent.
async fn board(lines: Vec<&'static str>) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"\r\n\x1b[1;36mHome\r\nObvious exits: east\r\n[HP=30/MA=0]:").await.unwrap();
        for line in lines {
            tokio::time::sleep(Duration::from_millis(50)).await;
            sock.write_all(format!("\r\n{line}\r\n[HP=30/MA=0]:").as_bytes()).await.unwrap();
        }
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            for l in text.lines() {
                log.lock().unwrap().push(l.trim().to_string());
            }
            sock.write_all(format!("\r\n{}\r\n[HP=30/MA=0]:", text.trim()).as_bytes()).await.unwrap();
        }
    });
    (addr, received)
}

async fn session_for(addr: std::net::SocketAddr) -> Session {
    let profile = Profile {
        target: mud_client::dialect::Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "testuser".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

async fn wait_until(session: &Session, pred: impl Fn(&Session) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !pred(session) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the session should reach the state");
}

#[tokio::test]
async fn the_session_learns_it_is_following_and_queues_the_leaders_requests() {
    let (addr, _) = board(vec![
        "You are now following Beef",
        "Beef telepaths: @wait",
        "Stranger telepaths: @wait",
        "Beef telepaths: @bank",
    ])
    .await;
    let session = session_for(addr).await;
    wait_until(&session, |s| s.party().role == Role::Follower).await;
    assert_eq!(session.party().leader.as_deref(), Some("Beef"));
    wait_until(&session, |s| !s.party_holds_clear()).await;
    assert_eq!(session.party_holds().lock().unwrap().names(), vec!["Beef".to_string()]);
    wait_until(&session, |s| {
        s.party().role == Role::Follower && s.party_holds().lock().unwrap().names().len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        session.party_holds().lock().unwrap().names(),
        vec!["Beef".to_string()],
        "the stranger is not in the party, so the stranger's @wait holds nothing"
    );
    let requests = session.take_party_requests();
    assert_eq!(requests.len(), 1, "the stranger's @wait and the stranger are dropped");
    assert_eq!(requests[0].from, "Beef");
    assert_eq!(requests[0].command, Remote::Bank);
    assert!(session.take_party_requests().is_empty(), "drained");
}

#[tokio::test]
async fn ok_releases_the_hold_that_wait_took() {
    let (addr, _) = board(vec![
        "Pootwaddle started to follow you.",
        "Pootwaddle telepaths: @wait",
        "Pootwaddle telepaths: @ok",
    ])
    .await;
    let session = session_for(addr).await;
    wait_until(&session, |s| s.party().role == Role::Leader).await;
    wait_until(&session, |s| !s.party_holds_clear()).await;
    assert_eq!(session.party_holds().lock().unwrap().names(), vec!["Pootwaddle".to_string()]);
    wait_until(&session, |s| s.party_holds_clear()).await;
    assert_eq!(
        session.party().role,
        Role::Leader,
        "@ok releases the hold, it does not end the party"
    );
}

#[tokio::test]
async fn the_party_ending_clears_everything_and_publishes_the_new_state() {
    let (addr, _) = board(vec![
        "Pootwaddle started to follow you.",
        "Pootwaddle telepaths: @wait",
        "You are not in a party at the present time.",
    ])
    .await;
    let session = session_for(addr).await;
    let mut changes = session.party_changes();
    wait_until(&session, |s| s.party().role == Role::Leader).await;
    // Pinned here as well as at the end: the ending resets the party to
    // the watch's own initial value, so an equality check taken only
    // after it would pass against a tracker that published nothing but
    // blanks.
    let joined = changes.borrow_and_update().clone();
    assert_eq!(joined, session.party(), "the watch publishes the join");
    wait_until(&session, |s| !s.party_holds_clear()).await;
    wait_until(&session, |s| s.party().role == Role::None).await;
    assert!(session.party_holds_clear(), "the ending drops the hold the @wait took");
    assert!(changes.has_changed().unwrap());
    // Cloned out of the borrow rather than compared inside it. The
    // reader task takes the party lock and then sends on this watch, so
    // holding the watch borrow across a `party()` call inverts that
    // order.
    let published = changes.borrow_and_update().clone();
    assert_eq!(published, session.party(), "the watch publishes what party() reads");
}

/// A consumer parked on the notes must end when the line closes, the
/// way `rests()` does, rather than wait forever on a sender the session
/// still holds.
#[tokio::test]
async fn the_notes_end_when_the_session_closes() {
    let (addr, _) = board(vec![]).await;
    let session = session_for(addr).await;
    let mut parked = session.party_notes();
    session.close();
    let ended = tokio::time::timeout(Duration::from_secs(1), parked.recv()).await;
    assert!(ended.expect("the notes must end at close, not hang").is_err());
    let mut after = session.party_notes();
    let ended = tokio::time::timeout(Duration::from_secs(1), after.recv()).await;
    assert!(ended.expect("a receiver taken after close must end at once").is_err());
}

/// A regression test for the roster comparison: the character's own row
/// is dropped before comparing members. Two rosters whose members match
/// once that row is gone return Vitals and print nothing.
#[tokio::test]
async fn two_rosters_with_the_same_members_are_silent() {
    let (addr, _) = board(vec![
        "Pootwaddle started to follow you.",
        "The following people are in your travel party:\r\n  Testuser     Mystic\r\n  Pootwaddle   Warrior",
        "The following people are in your travel party:\r\n  Testuser     Mystic\r\n  Pootwaddle   Warrior",
    ])
    .await;
    let session = session_for(addr).await;
    let mut notes = session.party_notes();
    let joined = tokio::time::timeout(Duration::from_secs(5), notes.recv()).await.unwrap().unwrap();
    assert_eq!(joined, "party: Pootwaddle joined");
    wait_until(&session, |s| s.party().members.len() == 1).await;
    let no_note = tokio::time::timeout(Duration::from_millis(500), notes.recv()).await;
    assert!(no_note.is_err(), "two rosters with the same members print no note");
    assert_eq!(
        session.party().members.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
        vec!["Pootwaddle".to_string()]
    );
}

/// The board lists the character itself on its own roster. A row taken
/// at face value would put the leader on a hold of its own name at
/// every bank trip, and would report itself as the follower that never
/// answered. The name matched here is the profile's username, which is
/// what `character_name` reads before a stat sheet has been seen, and
/// it is matched whatever the case.
#[tokio::test]
async fn the_roster_drops_the_characters_own_row() {
    let (addr, _) = board(vec![
        "Pootwaddle started to follow you.",
        "The following people are in your travel party:\r\n  Testuser     Mystic\r\n  Pootwaddle   Warrior",
    ])
    .await;
    let session = session_for(addr).await;
    let mut notes = session.party_notes();
    let joined = tokio::time::timeout(Duration::from_secs(5), notes.recv()).await.unwrap().unwrap();
    assert_eq!(joined, "party: Pootwaddle joined");
    wait_until(&session, |s| s.party().members.len() == 1).await;
    assert_eq!(session.party().role, Role::Leader);
    let no_note = tokio::time::timeout(Duration::from_millis(500), notes.recv()).await;
    assert!(no_note.is_err(), "a roster with the same members is silent");
    let members: Vec<String> = session.party().members.iter().map(|m| m.name.clone()).collect();
    assert_eq!(members, vec!["Pootwaddle".to_string()]);
}

/// The roster's numbers and a said request both reach the table a
/// healer reads, and a stranger's word does not.
#[tokio::test]
async fn the_session_keeps_the_party_health() {
    let (addr, _received) = board(vec![
        "Beef started to follow you.",
        "The following people are in your travel party:\r\n  Beef                           (Ninja)               [H:100%]   - Midrank\r\n",
        "Beef says \"@heal 35\"",
        "Stranger says \"@heal 5\"",
        "Beef says \"@iam Human Witchunter\"",
    ])
    .await;
    let session = session_for(addr).await;
    wait_until(&session, |s| s.party_health().get("Beef").is_some_and(|v| v.race.is_some())).await;
    let health = session.party_health();
    let beef = health.get("Beef").unwrap();
    assert_eq!(beef.hp, Some(35), "the request overwrote the roster's number");
    assert_eq!(beef.class.as_deref(), Some("Witchunter"));
    assert!(beef.resists_magic());
    assert!(health.get("Stranger").is_none());
    session.close();
}

#[tokio::test]
async fn notes_name_every_change_and_accepted_request() {
    let (addr, _) =
        board(vec!["Pootwaddle started to follow you.", "Pootwaddle telepaths: @bank"]).await;
    let session = session_for(addr).await;
    let mut notes = session.party_notes();
    let first = tokio::time::timeout(Duration::from_secs(5), notes.recv()).await.unwrap().unwrap();
    assert_eq!(first, "party: Pootwaddle joined");
    let second = tokio::time::timeout(Duration::from_secs(5), notes.recv()).await.unwrap().unwrap();
    assert_eq!(second, "party: Pootwaddle asks @bank");
}

