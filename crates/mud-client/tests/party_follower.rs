//! A follower dragged into a bank deposits and tells the leader `@ok`.
//!
//! The board is the `bank_scripted.rs` harness with one addition: a
//! list of lines it prints unprompted after the opening room block, one
//! every 100 ms. That is how the follow line and the leader's drag
//! arrive, with nothing sent to ask for them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Content, Room, RoomId, Shop, ShopId, ShopStock};

/// The stock line that makes this character a follower.
const FOLLOW: &str = "You are now following Beef";

/// The leader's move, which drags the follower into the bank. One
/// block, ending in its own prompt.
const DRAG: &str = "\r\n -- Following your Party leader east --\r\n\r\n\x1b[1;36mBank of Godfrey\r\nObvious exits: west\r\n[HP=30/MA=0]:";

const BANK: RoomId = RoomId { map: 1, room: 297 };

/// The two unprompted lines, in the order the board prints them. The
/// follow line needs its own framing. The drag carries its own.
fn arrival() -> Vec<String> {
    vec![format!("\r\n{FOLLOW}\r\n[HP=30/MA=0]:"), DRAG.to_string()]
}

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:")
}

/// One bank, whose shop is named differently from the room it stands
/// in, as four of the five shipped banks are.
fn content() -> Content {
    let mut c = Content::default();
    c.add_shop(Shop {
        id: ShopId(8),
        name: "Godfrey Savings".into(),
        shop_type: 7,
        min_level: 0,
        max_level: 0,
        markup: 0,
        class_limit: 0,
        stock: [ShopStock::default(); 20],
    });
    c.add_room(Room {
        id: BANK,
        name: "Bank of Godfrey".into(),
        room_type: 1,
        shop: Some(ShopId(8)),
        ..Default::default()
    });
    c
}

/// The `bank_scripted.rs` board, plus `pushes`: lines printed
/// unprompted after the opening block, one every 100 ms, before any
/// reply is answered.
async fn scripted_board(
    pushes: Vec<String>,
    script: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Home", "east").as_bytes()).await.unwrap();
        for push in pushes {
            tokio::time::sleep(Duration::from_millis(100)).await;
            sock.write_all(push.as_bytes()).await.unwrap();
        }
        let mut used: Vec<Option<u64>> = vec![None; script.len()];
        let mut clock: u64 = 0;
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim().to_lowercase();
                log.lock().unwrap().push(line.clone());
                let next = used.iter().position(Option::is_none);
                let fresh = next.filter(|&i| script[i].0 == line);
                let reply = match fresh {
                    Some(i) => {
                        clock += 1;
                        used[i] = Some(clock);
                        script[i].1.clone()
                    }
                    None => match script
                        .iter()
                        .enumerate()
                        .filter(|(i, (m, _))| used[*i].is_some() && *m == line)
                        .max_by_key(|(i, _)| used[*i])
                    {
                        Some((_, (_, r))) => r.clone(),
                        None => format!("\r\n{line}\r\nYou say \"{line}\"\r\n[HP=30/MA=0]:"),
                    },
                };
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
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
        disable_evil_warnings: false,
        bot: None,
        farm: None,
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

/// Wait until the board has read `line`, and answer with everything it
/// had read by then. Sends are queued, so a job that has finished has
/// not necessarily reached the wire.
async fn read_by_the_board(log: &Arc<Mutex<Vec<String>>>, line: &str) -> Vec<String> {
    let wait = async {
        loop {
            let seen = log.lock().unwrap().clone();
            if seen.iter().any(|l| l == line) {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    match tokio::time::timeout(Duration::from_secs(10), wait).await {
        Ok(seen) => seen,
        Err(_) => panic!("{line} never reached the board: {:?}", log.lock().unwrap()),
    }
}

/// Wait for the character to be following, which is what the drag that
/// follows it is only meaningful after.
async fn until_following(session: &Session) {
    let wait = async {
        while !session.party().is_follower() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait).await.expect("the follow line");
}

#[test]
fn a_bank_room_block_while_following_starts_the_deposit() {
    use mud_client::correlate::Correlated;
    use mud_client::events::{Event, RoomView};
    use mud_client::party::PartyState;
    use mud_client::tui::follower_bank_arrival_decision;
    let mut party = PartyState::new();
    party.observe(FOLLOW);
    let names: std::collections::BTreeSet<String> = ["Bank of Godfrey".to_string()].into();
    let block = |name: &str, elsewhere: bool| Correlated {
        event: Event::RoomSeen(RoomView { name: name.into(), ..Default::default() }),
        answers: None,
        elsewhere,
    };
    let seen = block("Bank of Godfrey", false);
    let from = |last: Option<&str>, cor: &Correlated| {
        follower_bank_arrival_decision(&party, true, true, &names, last, cor)
    };
    assert_eq!(from(Some("Home"), &seen), Some("Bank of Godfrey".to_string()));
    assert_eq!(
        from(Some("Bank of Godfrey"), &seen),
        None,
        "the block for the room already stood in is a re-render, not an arrival"
    );
    assert_eq!(
        follower_bank_arrival_decision(&party, false, true, &names, None, &seen),
        None,
        "bot off"
    );
    assert_eq!(
        follower_bank_arrival_decision(&party, true, false, &names, None, &seen),
        None,
        "auto_deposit off"
    );
    assert_eq!(
        follower_bank_arrival_decision(&PartyState::new(), true, true, &names, None, &seen),
        None,
        "not following"
    );
    assert_eq!(from(None, &block("Home", false)), None);
    assert_eq!(
        from(None, &block("Bank of Godfrey", true)),
        None,
        "a peek into the bank next door is not an arrival"
    );
    let line = Correlated {
        event: Event::Line("Bank of Godfrey".into()),
        answers: None,
        elsewhere: false,
    };
    assert_eq!(
        from(None, &line),
        None,
        "a line that merely says the name is not an arrival"
    );
}

#[tokio::test]
async fn a_follower_dragged_into_the_bank_deposits_and_says_ok() {
    let (addr, received) = scripted_board(
        arrival(),
        vec![
            ("i", "\r\ni\r\nYou are carrying 15 gold crowns\r\nYou have no keys.\r\nEncumbrance: 5/2400 - None [0%]\r\n[HP=30/MA=0]:".into()),
            ("deposit 1500", "\r\ndeposit 1500\r\nYou deposit 15 gold crowns.\r\n[HP=30/MA=0]:".into()),
            ("i", "\r\ni\r\nYou are carrying nothing\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:".into()),
        ],
    )
    .await;
    let session = Arc::new(session_for(addr).await);
    session.set_content(Arc::new(content()));
    until_following(&session).await;
    // The window starts this off the room block. Drive it directly.
    let job = mud_client::tui::start_follower_deposit(
        session.clone(),
        Arc::new(content()),
        "Bank of Godfrey",
    );
    let phase = job.phase.clone();
    tokio::time::timeout(Duration::from_secs(20), job.handle).await.unwrap().unwrap();
    let log = read_by_the_board(&received, "/beef @ok").await;
    let deposit = log.iter().position(|l| l == "deposit 1500").expect("the deposit");
    let ok = log.iter().position(|l| l == "/beef @ok").expect("the ok");
    assert!(deposit < ok, "{log:?}");
    assert_eq!(
        phase.borrow().clone(),
        mud_client::farm::Phase::Done {
            why: "deposited 1500 copper farthings at Bank of Godfrey".into(),
            at: Some(BANK),
        },
        "the room block's name found the bank's id"
    );
}

#[tokio::test]
async fn a_follower_with_nothing_to_deposit_says_ok_at_once() {
    let (addr, received) = scripted_board(
        arrival(),
        vec![("i", "\r\ni\r\nYou are carrying nothing\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:".into())],
    )
    .await;
    let session = Arc::new(session_for(addr).await);
    session.set_content(Arc::new(content()));
    until_following(&session).await;
    let job = mud_client::tui::start_follower_deposit(
        session.clone(),
        Arc::new(content()),
        "Bank of Godfrey",
    );
    tokio::time::timeout(Duration::from_secs(20), job.handle).await.unwrap().unwrap();
    let log = read_by_the_board(&received, "/beef @ok").await;
    assert!(!log.iter().any(|l| l.starts_with("deposit")), "{log:?}");
}
