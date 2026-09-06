//! Key doors.
//!
//! The Black House door on Slum Street, captured 2026-09-05: `unlock e`
//! is not a command, `use black star key east` answers "You
//! successfully unlocked the door.", the same line a pick gives, and
//! the door still needs `open e`. The board here says exactly that.
//! Its `i` reply is the four line inventory, so the walk's re-read of
//! the pack after a key use has something to read.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mud_client::graph::{Capabilities, ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::nav::{NavConfig, NavErrorKind, Navigator, NoGuard};
use mud_client::pack::PackHandle;
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_client::sheet::Inventory;
use mud_core::content::{Content, Direction, Item, ItemId, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const STREET: RoomId = RoomId { map: 1, room: 1224 };
const HOUSE: RoomId = RoomId { map: 1, room: 1225 };
const KEY: ItemId = ItemId(172);
const PROMPT: &str = "\r\n[HP=30/MA=0]:";

#[derive(Default)]
struct DoorLog {
    lines: std::sync::Mutex<Vec<String>>,
    opens: AtomicUsize,
    uses: AtomicUsize,
    picks: AtomicUsize,
    bashes: AtomicUsize,
    inventories: AtomicUsize,
}

impl DoorLog {
    fn count(&self, line: &str) -> usize {
        self.lines.lock().unwrap().iter().filter(|l| l.as_str() == line).count()
    }
}

fn block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}{PROMPT}")
}

/// The door east of the street. `key_works` says whether the key is
/// the right one. `ring_after` is the key ring line the board prints
/// once asked, so a test can say the key was spent.
async fn house_board(
    key_works: bool,
    ring_after: &'static str,
) -> (std::net::SocketAddr, Arc<DoorLog>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(DoorLog::default());
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(block("Slum Street", "closed door east").as_bytes())
            .await
            .unwrap();
        let mut locked = true;
        let mut open = false;
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
                seen.lines.lock().unwrap().push(line.clone());
                let reply = match line.as_str() {
                    "e" | "east" if open => block("Black House", "open door west"),
                    "e" | "east" => format!("\r\nThe door is closed.{PROMPT}"),
                    "open e" | "open east" => {
                        seen.opens.fetch_add(1, Ordering::SeqCst);
                        if locked {
                            format!("\r\nThe door is locked.{PROMPT}")
                        } else {
                            open = true;
                            format!("\r\nThe door is now open.{PROMPT}")
                        }
                    }
                    "use black star key east" => {
                        seen.uses.fetch_add(1, Ordering::SeqCst);
                        if key_works {
                            locked = false;
                            format!("\r\nYou successfully unlocked the door.{PROMPT}")
                        } else {
                            format!("\r\nNothing happens.{PROMPT}")
                        }
                    }
                    "picklock e" | "picklock east" => {
                        seen.picks.fetch_add(1, Ordering::SeqCst);
                        locked = false;
                        format!("\r\nYou successfully unlocked the door.{PROMPT}")
                    }
                    "bash e" | "bash east" => {
                        seen.bashes.fetch_add(1, Ordering::SeqCst);
                        open = true;
                        format!("\r\nYou bashed the door open.{PROMPT}")
                    }
                    "i" => {
                        seen.inventories.fetch_add(1, Ordering::SeqCst);
                        format!(
                            "\r\nYou are carrying nothing.\r\n{ring_after}\r\nWealth: 0 copper farthings\r\nEncumbrance: 0/2400 - None [0%]{PROMPT}"
                        )
                    }
                    "look" => block("Slum Street", "closed door east"),
                    other => format!("\r\nYou say \"{other}\"{PROMPT}"),
                };
                sock.write_all(format!("\r\n{line}{reply}").as_bytes())
                    .await
                    .unwrap();
            }
        }
    });
    (addr, log)
}

/// The street and the house, joined by a door with `requirement`. A
/// key door is type 2, anything else here is a plain type 7 door.
fn house_graph(requirement: ExitRequirement) -> Arc<RoomGraph> {
    let exit_type = match requirement {
        ExitRequirement::KeyDoor { .. } => 2,
        _ => 7,
    };
    let door = |dest| ExitEdge {
        dest,
        exit_type,
        command: None,
        requirement: requirement.clone(),
    };
    let mut street = GraphRoom {
        name: "Slum Street".into(),
        ..Default::default()
    };
    street.exits[Direction::East as usize] = Some(door(HOUSE));
    let mut house = GraphRoom {
        name: "Black House".into(),
        ..Default::default()
    };
    house.exits[Direction::West as usize] = Some(door(STREET));
    Arc::new(RoomGraph::from_rooms(vec![(STREET, street), (HOUSE, house)]))
}

fn table() -> Arc<Content> {
    let mut content = Content::default();
    content.add_item(Item {
        id: KEY,
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    Arc::new(content)
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
        bank: Default::default(),
        ..Default::default()
    };
    let session = Session::connect(&profile, None).await.unwrap();
    session.set_content(table());
    session
}

/// The session's own pack, holding the key or not.
fn ring(session: &Session, with_key: bool) -> PackHandle {
    let pack = session.pack_handle().expect("set_content gave the session a pack");
    pack.refresh(&Inventory {
        items: Vec::new(),
        keys: if with_key { vec!["black star key".into()] } else { Vec::new() },
        encumbrance: None,
    });
    pack
}

fn key_door() -> ExitRequirement {
    ExitRequirement::KeyDoor { key: KEY, pick: -99 }
}

fn nav(
    session: &Session,
    requirement: ExitRequirement,
    with_key: bool,
    picklocks: u32,
    bash_doors: bool,
) -> Navigator {
    Navigator::new(
        house_graph(requirement),
        NavConfig {
            step_timeout_ms: 1500,
            bash_doors,
            ..NavConfig::default()
        },
    )
    .with_capabilities(Capabilities {
        picklocks,
        pack: Some(ring(session, with_key)),
        ..Capabilities::unrestricted()
    })
}

async fn walk(n: &Navigator, session: &Session) -> Result<RoomId, NavErrorKind> {
    tokio::time::timeout(
        Duration::from_secs(10),
        n.goto(session, STREET, HOUSE, &mut NoGuard, false),
    )
    .await
    .expect("goto should not hang")
    .map(|at| at.at)
    .map_err(|e| e.kind)
}

/// The captured flow: `open e` says locked, the key is used with its
/// full name and the full direction word, the unlocked line arrives,
/// `open e` now opens, and the step lands. No roll is spent.
#[tokio::test]
async fn a_key_door_is_unlocked_with_the_key_then_opened_and_walked() {
    let (addr, log) = house_board(true, "You have the following keys:  black star key.").await;
    let session = session_for(addr).await;
    let n = nav(&session, key_door(), true, 0, false);

    let at = walk(&n, &session).await.expect("the key opens it");

    assert_eq!(at, HOUSE);
    assert_eq!(log.count("use black star key east"), 1, "{:?}", log.lines.lock().unwrap());
    assert_eq!(log.opens.load(Ordering::SeqCst), 2, "one refused, one after the key");
    assert_eq!(log.picks.load(Ordering::SeqCst), 0);
    assert_eq!(log.bashes.load(Ordering::SeqCst), 0);
}

/// 37 shipped keys have one use. After the key is used the pack is
/// re-read from the board, never updated by inference, so a spent key
/// leaves the ring before routing trusts it again.
#[tokio::test]
async fn the_pack_is_re_read_after_the_key_is_used() {
    let (addr, log) = house_board(true, "You have no keys.").await;
    let session = session_for(addr).await;
    let n = nav(&session, key_door(), true, 0, false);
    let pack = session.pack_handle().unwrap();
    assert!(pack.has(KEY), "the walk starts with the key");

    walk(&n, &session).await.expect("the key opens it");

    assert!(log.inventories.load(Ordering::SeqCst) >= 1, "the pack was asked for");
    // The reader task refreshes the pack when the reply's last line
    // lands, which can be a moment after the walk returns.
    for _ in 0..20 {
        if !pack.has(KEY) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the spent key is still on the ring");
}

/// A lock the formula says cannot give is never picked. -99 against 10
/// Picklocks fails every roll, so the walk goes straight to force
/// rather than spending its whole pick budget first. A plain locked
/// door, because routing refuses a key door nobody can pick even with
/// bashing on, and the point here is the walk.
#[tokio::test]
async fn an_unpickable_lock_spends_no_pick() {
    let (addr, log) = house_board(true, "You have no keys.").await;
    let session = session_for(addr).await;
    let locked = ExitRequirement::Door { locked: true, pick: -99 };
    let n = nav(&session, locked, false, 10, true);

    let at = walk(&n, &session).await.expect("bashing is on");

    assert_eq!(at, HOUSE);
    assert_eq!(log.uses.load(Ordering::SeqCst), 0, "no key door, nothing to use");
    assert_eq!(log.picks.load(Ordering::SeqCst), 0, "-99 + 10 never passes");
    assert!(log.bashes.load(Ordering::SeqCst) >= 1);
}

/// Without the key, the skill or bashing there is nothing to try, and
/// routing knows it before a command is sent.
#[tokio::test]
async fn a_key_door_nobody_can_open_is_no_route() {
    let (addr, log) = house_board(true, "You have no keys.").await;
    let session = session_for(addr).await;
    let n = nav(&session, key_door(), false, 10, false);

    let err = walk(&n, &session).await.expect_err("nothing opens it");

    assert!(matches!(err, NavErrorKind::NoRoute), "{err:?}");
    assert_eq!(log.opens.load(Ordering::SeqCst), 0, "routing refused it first");
}

/// A key the board does not accept at this door is answered with some
/// other line and the next prompt. The walk reads that as the key
/// doing nothing and the lock is the story again: picking and bashing
/// get their turn.
#[tokio::test]
async fn a_key_that_does_nothing_falls_through_to_the_lock() {
    let (addr, log) = house_board(false, "You have the following keys:  black star key.").await;
    let session = session_for(addr).await;
    let n = nav(&session, key_door(), true, 100, false);

    let at = walk(&n, &session).await.expect("the pick opens it");

    assert_eq!(at, HOUSE);
    assert_eq!(log.uses.load(Ordering::SeqCst), 1);
    assert_eq!(log.picks.load(Ordering::SeqCst), 1, "-99 + 100 is worth a roll");
}
