//! The leader's side of the wait handshake, against a scripted board.
//!
//! A follower that says `@wait` holds the leader where it stands. The
//! leg does not send its next step until the follower says `@ok`, or
//! until `[party].wait_secs` runs out. The harness is
//! `farm_scripted.rs`'s scripted board, extended with lines the board
//! volunteers, since a telepath arrives unasked.
//!
//! Test crates do not share modules, so the board is duplicated here,
//! which is this suite's existing pattern.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mud_client::bank::BankConfig;
use mud_client::bot::BotConfig;
use mud_client::farm::{FarmConfig, FarmEnd, FarmPlan, run_farm};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::live::Live;
use mud_client::party::PartyConfig;
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_client::tui::World;
use mud_core::content::{Content, Direction, Room, RoomId, Shop, ShopId, ShopStock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HOME: RoomId = RoomId { map: 1, room: 1 };
const FIELD: RoomId = RoomId { map: 1, room: 2 };
const MEADOW: RoomId = RoomId { map: 1, room: 3 };
const BANK: RoomId = RoomId { map: 1, room: 4 };

/// A notices sink that keeps every line, and the lines it kept.
fn collected() -> (mud_client::farm::Notices, Arc<Mutex<Vec<String>>>) {
    let said = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&said);
    (
        Arc::new(move |line: &str| into.lock().unwrap().push(line.to_string())),
        said,
    )
}

fn room_block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:")
}

fn edge(dest: RoomId) -> ExitEdge {
    ExitEdge {
        dest,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    }
}

/// A chain of rooms numbered 1/1 upward, each east to the next and
/// west back. Two of them is one leg of one step. Three is a leg long
/// enough for a hold to land between steps.
fn corridor(names: &[&str]) -> Arc<RoomGraph> {
    let ids = [HOME, FIELD, MEADOW];
    let mut rooms = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let mut room = GraphRoom {
            name: (*name).into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        if let Some(&east) = ids.get(i + 1).filter(|_| i + 1 < names.len()) {
            room.exits[Direction::East as usize] = Some(edge(east));
        }
        if i > 0 {
            room.exits[Direction::West as usize] = Some(edge(ids[i - 1]));
        }
        rooms.push((ids[i], room));
    }
    Arc::new(RoomGraph::from_rooms(rooms))
}

/// A line the board volunteers, unasked: the received line that arms
/// it, how long after that line it goes out, a label, and the text.
///
/// Armed by a received line rather than by the clock, so a scenario is
/// ordered against the run itself. A zero delay goes out ahead of that
/// line's own reply, which is how a board interleaves somebody else's
/// telepath with an answer it was already about to send. Each push
/// fires once.
type Push = (&'static str, Duration, &'static str, String);

/// What the board received, and when.
type Heard = Arc<Mutex<Vec<(Instant, String)>>>;
/// What the board volunteered, by label, and when.
type Pushed = Arc<Mutex<Vec<(String, Instant)>>>;

/// The `farm_scripted.rs` board: a per-line script of `(matcher,
/// reply)`, each entry used once, first unused match wins, unmatched
/// lines echo and say back. Every received line is logged with the
/// instant it arrived, and every push with the instant it went out, so
/// a test can order a sent step against a line the board volunteered.
async fn scripted_board(
    script: Vec<(&'static str, String)>,
    pushes: Vec<Push>,
) -> (std::net::SocketAddr, Heard, Pushed) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received: Heard = Arc::new(Mutex::new(Vec::new()));
    let pushed: Pushed = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    let sent = Arc::clone(&pushed);
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (mut rx, tx) = tokio::io::split(sock);
        // Shared because a delayed push and the reply loop both write.
        let tx = Arc::new(tokio::sync::Mutex::new(tx));
        write(&tx, &room_block("Home", "east")).await;
        // One-shot entries consumed in order. A re-ask replays the
        // most recently consumed matching entry, exactly as a real
        // board re-answers a look with the room it is still showing.
        let mut used: Vec<Option<u64>> = vec![None; script.len()];
        let mut fired = vec![false; pushes.len()];
        let mut clock: u64 = 0;
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        while let Ok(n) = rx.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim().to_lowercase();
                log.lock().unwrap().push((Instant::now(), line.clone()));
                for (i, (on, delay, label, text)) in pushes.iter().enumerate() {
                    if fired[i] || *on != line {
                        continue;
                    }
                    fired[i] = true;
                    if delay.is_zero() {
                        write(&tx, text).await;
                        sent.lock().unwrap().push((label.to_string(), Instant::now()));
                        continue;
                    }
                    let tx = Arc::clone(&tx);
                    let sent = Arc::clone(&sent);
                    let (delay, label, text) = (*delay, label.to_string(), text.clone());
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        write(&tx, &text).await;
                        sent.lock().unwrap().push((label, Instant::now()));
                    });
                }
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
                write(&tx, &reply).await;
            }
        }
    });
    (addr, received, pushed)
}

async fn write(tx: &tokio::sync::Mutex<tokio::io::WriteHalf<tokio::net::TcpStream>>, text: &str) {
    let mut out = tx.lock().await;
    out.write_all(text.as_bytes()).await.unwrap();
    out.flush().await.unwrap();
}

async fn session_for(addr: std::net::SocketAddr, party: PartyConfig) -> Session {
    session_with(
        addr,
        party,
        BankConfig {
            auto_deposit: false,
            ..BankConfig::default()
        },
    )
    .await
}

/// The same session with a bank policy of its own, for the tests where
/// the leader really walks to a bank.
async fn session_with(
    addr: std::net::SocketAddr,
    party: PartyConfig,
    bank: BankConfig,
) -> Session {
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
        bank,
        party,
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

/// The board's script for the one-lap circuit Home -> Field.
fn lap_script() -> Vec<(&'static str, String)> {
    vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        // verify_start's attributed look.
        ("look", format!("\r\nlook{}", room_block("Home", "east"))),
        ("e", format!("\r\ne{}", room_block("Field", "west"))),
        ("look", format!("\r\nlook{}", room_block("Field", "west"))),
    ]
}

fn lap_config() -> FarmConfig {
    FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        ..FarmConfig::default()
    }
}

/// Pootwaddle joins and says `@wait` on the same breath as the run's
/// opening look, so the hold is in place before the leg's first step.
/// `@ok` follows a second and a half later, and the step east must
/// come after it.
#[tokio::test]
async fn a_wait_holds_the_leg_until_ok() {
    let (addr, received, pushed) = scripted_board(
        lap_script(),
        vec![
            (
                "look",
                Duration::ZERO,
                "wait",
                "\r\nPootwaddle started to follow you.\r\nPootwaddle telepaths: @wait\r\n".into(),
            ),
            (
                "look",
                Duration::from_millis(1500),
                "ok",
                "\r\nPootwaddle telepaths: @ok\r\n".into(),
            ),
        ],
    )
    .await;
    let session = session_for(addr, PartyConfig::default()).await;
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor(&["Home", "Field"]);
    let cfg = lap_config();
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, _said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(20),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let heard: Vec<String> = log.iter().map(|(_, l)| l.clone()).collect();
    let e_sent_at = at(&log, "e").expect("the leg stepped east");
    let ok_seen = pushed_at(&pushed, "ok").unwrap_or_else(|| {
        panic!("the leg finished before the board could say @ok: {heard:?}")
    });
    assert!(
        e_sent_at >= ok_seen,
        "the step east waited for @ok: {heard:?}"
    );
}

/// The held leader stands in an empty room. Standing still is the
/// whole job, so the board hears one `look` when the hold begins and
/// at most one more per stop the hold is cut into. It used to hear one
/// per round trip: the hold ran two-second stops, an empty room ended
/// each stop the moment its look was answered, and the next stop opened
/// with a fresh look (carrot, cwgaming 2026-09-14: 234 looks in twenty
/// seconds).
#[tokio::test]
async fn a_held_leader_does_not_loop_on_look() {
    let (addr, received, pushed) = scripted_board(
        lap_script(),
        vec![
            (
                "look",
                Duration::ZERO,
                "wait",
                "\r\nPootwaddle started to follow you.\r\nPootwaddle telepaths: @wait\r\n".into(),
            ),
            (
                "look",
                Duration::from_millis(1500),
                "ok",
                "\r\nPootwaddle telepaths: @ok\r\n".into(),
            ),
        ],
    )
    .await;
    let session = session_for(addr, PartyConfig::default()).await;
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor(&["Home", "Field"]);
    let cfg = lap_config();
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, _said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(20),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let wait_seen = pushed_at(&pushed, "wait").expect("the board said @wait");
    let ok_seen = pushed_at(&pushed, "ok").expect("the board said @ok");
    let looks_held = log
        .iter()
        .filter(|(t, l)| l == "look" && *t >= wait_seen && *t <= ok_seen)
        .count();
    // The hold lasts a second and a half: the stop that opened on the
    // hold, its re-ask at `idle_poke_ms` (500ms here), and the next
    // two-second stop's opening look. A fourth is slack for the timing
    // of the push against the stop boundary.
    assert!(
        looks_held <= 4,
        "the board heard {looks_held} looks during a 1.5s hold: {:?}",
        log.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>()
    );
}

/// No `@ok` ever comes. With `[party].wait_secs = 1` the hold expires
/// on its own, the leg goes on, and the run says whose hold it dropped.
#[tokio::test]
async fn a_wait_with_no_ok_expires_and_the_leg_goes_on() {
    let (addr, received, pushed) = scripted_board(
        lap_script(),
        vec![(
            "look",
            Duration::ZERO,
            "wait",
            "\r\nPootwaddle started to follow you.\r\nPootwaddle telepaths: @wait\r\n".into(),
        )],
    )
    .await;
    let session = session_for(
        addr,
        PartyConfig {
            wait_secs: 1,
            ..PartyConfig::default()
        },
    )
    .await;
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor(&["Home", "Field"]);
    let cfg = lap_config();
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(20),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let heard: Vec<String> = log.iter().map(|(_, l)| l.clone()).collect();
    let e_sent_at = at(&log, "e").expect("the leg stepped east");
    let wait_seen = pushed_at(&pushed, "wait").expect("the board said @wait");
    let held_for = e_sent_at.saturating_duration_since(wait_seen);
    assert!(
        held_for >= Duration::from_secs(1),
        "the leg stood for the hold's second, not {held_for:?}: {heard:?}"
    );
    let said = said.lock().unwrap().clone();
    assert!(
        said.iter().any(|l| l == "party: hold on Pootwaddle expired"),
        "the expiry is named: {said:?}"
    );
}

/// The `@wait` lands while the leg is already walking: the board says
/// it on the same breath as the first step's own answer. The hold has
/// to stop the SECOND step, which is what `Interrupt::Held` is for.
#[tokio::test]
async fn a_wait_mid_leg_stops_the_next_step() {
    let (addr, received, pushed) = scripted_board(
        vec![
            (
                "inventory",
                "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                    .into(),
            ),
            ("look", format!("\r\nlook{}", room_block("Home", "east"))),
            // The telepath rides out behind the step's own answer, the
            // way a board interleaves one. Ahead of the answer it
            // would be an interrupt arriving instead of the arrival,
            // which is a different scenario and not this one.
            (
                "e",
                format!(
                    "\r\ne{}\r\nPootwaddle started to follow you.\r\nPootwaddle telepaths: @wait\r\n",
                    room_block("Field", "east west")
                ),
            ),
            // The hold's own stop looks where it stands. Without this
            // the fallback would replay Home's block at Field and the
            // stop would believe it had been moved.
            ("look", format!("\r\nlook{}", room_block("Field", "east west"))),
            ("e", format!("\r\ne{}", room_block("Meadow", "west"))),
            ("look", format!("\r\nlook{}", room_block("Meadow", "west"))),
        ],
        vec![(
            "e",
            Duration::from_millis(1500),
            "ok",
            "\r\nPootwaddle telepaths: @ok\r\n".into(),
        )],
    )
    .await;
    let session = session_for(addr, PartyConfig::default()).await;
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor(&["Home", "Field", "Meadow"]);
    let cfg = FarmConfig {
        circuit: vec!["1/3".into()],
        ..lap_config()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, _said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(20),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert_eq!(
        stats.interrupts, 0,
        "a hold is not danger and must never spend the budget: {stats:?}"
    );

    let log = received.lock().unwrap().clone();
    let heard: Vec<String> = log.iter().map(|(_, l)| l.clone()).collect();
    let steps: Vec<Instant> = log.iter().filter(|(_, l)| l == "e").map(|(t, _)| *t).collect();
    assert_eq!(steps.len(), 2, "the leg is two steps: {heard:?}");
    let ok_seen = pushed_at(&pushed, "ok").unwrap_or_else(|| {
        panic!("the leg finished before the board could say @ok: {heard:?}")
    });
    assert!(
        steps[1] >= ok_seen,
        "the second step waited for @ok: {heard:?}"
    );
}

/// Home, Field one step east, and the bank one step east of that. The
/// shop number matches the one `bank_content` gives the bank room.
fn bank_corridor() -> Arc<RoomGraph> {
    let mut home = GraphRoom {
        name: "Home".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    home.exits[Direction::East as usize] = Some(edge(FIELD));
    let mut field = GraphRoom {
        name: "Field".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    field.exits[Direction::East as usize] = Some(edge(BANK));
    field.exits[Direction::West as usize] = Some(edge(HOME));
    let mut bank = GraphRoom {
        name: "Bank of Godfrey".into(),
        exits: Default::default(),
        light: 0,
        shop: 8,
        ..Default::default()
    };
    bank.exits[Direction::West as usize] = Some(edge(FIELD));
    Arc::new(RoomGraph::from_rooms(vec![
        (HOME, home),
        (FIELD, field),
        (BANK, bank),
    ]))
}

/// The one bank the errand can find, in the shape `bank_rooms` reads.
fn bank_content() -> Content {
    let mut c = Content::default();
    c.add_shop(Shop {
        id: ShopId(8),
        name: "Bank of Godfrey".into(),
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

/// 15 gold on hand, which is 1500 copper farthings.
const CARRYING: &str = "\r\ni\r\nYou are carrying 15 gold crowns\r\nYou have no keys.\r\nWealth: 1500 copper farthings\r\nEncumbrance: 5/2400 - None [0%]\r\n[HP=30/MA=0]:";
/// An empty purse, which is nothing above a keep floor of zero.
const EMPTY: &str = "\r\ni\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:";

/// The board's answer to `par`: the header, one indented row per name,
/// then the blank line that ends the block.
fn roster(rows: &[&str]) -> String {
    let mut out = format!("\r\npar\r\n{}\r\n", mud_client::party::ROSTER_HEADER);
    for row in rows {
        out.push_str(&format!("  {row}\r\n"));
    }
    out.push_str("\r\n[HP=30/MA=0]:");
    out
}

/// The one-lap circuit Home -> Field with a detour east to the bank.
/// `Pootwaddle telepaths: @bank` rides out behind the step into Field,
/// so the request is in hand when the stop judges its gate. `tail` is
/// what the board answers at the bank and `rows` is who `par` names,
/// which are the only parts the detour tests disagree about.
fn detour_script(
    rows: &[&str],
    tail: Vec<(&'static str, String)>,
) -> Vec<(&'static str, String)> {
    let mut script = vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Home", "east"))),
        // The gate's seeding read at the run's start.
        ("i", CARRYING.into()),
        (
            "e",
            format!(
                "\r\ne{}\r\nPootwaddle telepaths: @bank\r\n",
                room_block("Field", "east west")
            ),
        ),
        ("e", format!("\r\ne{}", room_block("Bank of Godfrey", "west"))),
    ];
    script.extend(tail);
    script.push(("par", roster(rows)));
    // The bank wait stands at the bank, and its stop looks where it
    // stands.
    script.push((
        "look",
        format!("\r\nlook{}", room_block("Bank of Godfrey", "west")),
    ));
    script
}

/// The bank policy the detour tests run under: the gate is armed, but
/// its own mark is far above what the character carries, so any walk
/// to the bank is the follower's doing and not the leader's.
fn armed_gate() -> BankConfig {
    BankConfig {
        auto_deposit: true,
        deposit_at_coins: 1000,
        keep_gold: 0,
        ..BankConfig::default()
    }
}

/// Pootwaddle asks for the bank from Field. The leader walks the extra
/// step east, deposits, asks the board who is in the party, and stands
/// at the bank until Pootwaddle says `@ok`.
#[tokio::test]
async fn a_follower_s_bank_detours_the_leader_and_waits_for_ok() {
    let (addr, received, pushed) = scripted_board(
        detour_script(&["Pootwaddle   Mystic"], vec![
            ("i", CARRYING.into()),
            (
                "deposit 1500",
                "\r\ndeposit 1500\r\nYou deposit 15 gold crowns.\r\n[HP=30/MA=0]:".into(),
            ),
            ("i", EMPTY.into()),
        ]),
        vec![
            (
                "look",
                Duration::ZERO,
                "follow",
                "\r\nPootwaddle started to follow you.\r\n".into(),
            ),
            (
                "par",
                Duration::from_millis(500),
                "ok",
                "\r\nPootwaddle telepaths: @ok\r\n".into(),
            ),
        ],
    )
    .await;
    let session = session_with(addr, PartyConfig::default(), armed_gate()).await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(bank_content()));

    let graph = bank_corridor();
    let cfg = lap_config();
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    let ended = Instant::now();
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let heard: Vec<String> = log.iter().map(|(_, l)| l.clone()).collect();
    let steps: Vec<Instant> = log.iter().filter(|(_, l)| l == "e").map(|(t, _)| *t).collect();
    assert_eq!(steps.len(), 2, "Home to Field, then Field to the bank: {heard:?}");
    let at_bank: Vec<&str> = heard
        .iter()
        .skip_while(|l| *l != "e")
        .skip(1)
        .skip_while(|l| *l != "e")
        .map(String::as_str)
        .filter(|l| *l == "i" || *l == "deposit 1500" || *l == "par")
        .collect();
    assert_eq!(
        at_bank,
        vec!["i", "deposit 1500", "i", "par"],
        "read, deposit, read again, then ask who is in the party: {heard:?}"
    );
    let ok_seen = pushed_at(&pushed, "ok")
        .unwrap_or_else(|| panic!("the board answered par and said @ok: {heard:?}"));
    assert!(
        ended >= ok_seen,
        "the leader stood at the bank until @ok: {heard:?}"
    );
    let waited = ended.saturating_duration_since(steps[1]);
    assert!(
        waited < Duration::from_secs(3),
        "the @ok ended the wait, not bank_wait_secs: {waited:?} {heard:?}"
    );
    let said = said.lock().unwrap().clone();
    assert!(
        !said.iter().any(|l| l.contains("no reply from")),
        "every follower answered: {said:?}"
    );
}

/// No `@ok` ever comes. With `[party].bank_wait_secs = 1` the hold
/// expires, the leader walks on, and the run names who never answered.
#[tokio::test]
async fn a_bank_wait_with_no_ok_expires_and_names_the_silent() {
    let (addr, received, _pushed) = scripted_board(
        detour_script(&["Pootwaddle   Mystic"], vec![
            ("i", CARRYING.into()),
            (
                "deposit 1500",
                "\r\ndeposit 1500\r\nYou deposit 15 gold crowns.\r\n[HP=30/MA=0]:".into(),
            ),
            ("i", EMPTY.into()),
        ]),
        vec![(
            "look",
            Duration::ZERO,
            "follow",
            "\r\nPootwaddle started to follow you.\r\n".into(),
        )],
    )
    .await;
    let session = session_with(
        addr,
        PartyConfig {
            bank_wait_secs: 1,
            ..PartyConfig::default()
        },
        armed_gate(),
    )
    .await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(bank_content()));

    let graph = bank_corridor();
    let cfg = lap_config();
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let heard: Vec<String> = received.lock().unwrap().iter().map(|(_, l)| l.clone()).collect();
    let said = said.lock().unwrap().clone();
    assert!(
        said.iter().any(|l| l == "party: hold on Pootwaddle expired"),
        "the expiry is named: {said:?} {heard:?}"
    );
    assert!(
        said.iter()
            .any(|l| l == "party: bank wait: no reply from Pootwaddle"),
        "the silent follower is named: {said:?} {heard:?}"
    );
}

/// The leader carries nothing above its keep floor. The detour is the
/// followers' errand, so an empty purse is said out loud and deposits
/// stay on for the rest of the run.
#[tokio::test]
async fn an_asked_detour_with_an_empty_purse_leaves_deposits_on() {
    let (addr, received, _pushed) = scripted_board(
        detour_script(&["Pootwaddle   Mystic"], vec![("i", EMPTY.into())]),
        vec![
            (
                "look",
                Duration::ZERO,
                "follow",
                "\r\nPootwaddle started to follow you.\r\n".into(),
            ),
            (
                "par",
                Duration::from_millis(500),
                "ok",
                "\r\nPootwaddle telepaths: @ok\r\n".into(),
            ),
        ],
    )
    .await;
    let session = session_with(addr, PartyConfig::default(), armed_gate()).await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(bank_content()));

    let graph = bank_corridor();
    let cfg = lap_config();
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let heard: Vec<String> = received.lock().unwrap().iter().map(|(_, l)| l.clone()).collect();
    assert!(
        !heard.iter().any(|l| l.starts_with("deposit")),
        "an empty purse deposits nothing: {heard:?}"
    );
    assert!(
        heard.iter().any(|l| l == "par"),
        "the leader still waits for its followers: {heard:?}"
    );
    let said = said.lock().unwrap().clone();
    assert!(
        said.iter().any(|l| l.contains("nothing above the keep floor")),
        "the empty purse is said out loud: {said:?} {heard:?}"
    );
    assert!(
        !said.iter().any(|l| l.contains("Deposits are off")),
        "an asked errand that deposits nothing is not a failure: {said:?}"
    );
}

/// The follower answers while the leader is still depositing. The
/// board writes `@ok` ahead of the deposit's own reply, which puts it
/// behind the leader's first `i` at the bank and well before the `par`.
/// The hold has to be in place by then, or the release lands on
/// nothing and the leader stands the whole of `bank_wait_secs` for a
/// follower that answered.
#[tokio::test]
async fn an_ok_during_the_deposit_ends_the_bank_wait_at_once() {
    let (addr, received, pushed) = scripted_board(
        detour_script(
            &["Pootwaddle   Mystic"],
            vec![
                ("i", CARRYING.into()),
                (
                    "deposit 1500",
                    "\r\ndeposit 1500\r\nYou deposit 15 gold crowns.\r\n[HP=30/MA=0]:".into(),
                ),
                ("i", EMPTY.into()),
            ],
        ),
        vec![
            (
                "look",
                Duration::ZERO,
                "follow",
                "\r\nPootwaddle started to follow you.\r\n".into(),
            ),
            (
                "deposit 1500",
                Duration::ZERO,
                "ok",
                "\r\nPootwaddle telepaths: @ok\r\n".into(),
            ),
        ],
    )
    .await;
    let session = session_with(addr, PartyConfig::default(), armed_gate()).await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(bank_content()));

    let graph = bank_corridor();
    let cfg = lap_config();
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    let ended = Instant::now();
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let heard: Vec<String> = log.iter().map(|(_, l)| l.clone()).collect();
    let steps: Vec<Instant> = log.iter().filter(|(_, l)| l == "e").map(|(t, _)| *t).collect();
    assert_eq!(steps.len(), 2, "Home to Field, then Field to the bank: {heard:?}");
    pushed_at(&pushed, "ok")
        .unwrap_or_else(|| panic!("the board said @ok during the deposit: {heard:?}"));
    let waited = ended.saturating_duration_since(steps[1]);
    assert!(
        waited < Duration::from_secs(3),
        "the early @ok ended the wait, not bank_wait_secs: {waited:?} {heard:?}"
    );
    let said = said.lock().unwrap().clone();
    assert!(
        !said.iter().any(|l| l.contains("no reply from")),
        "the follower answered before the hold was even asked for: {said:?}"
    );
}

/// Two followers on the roster. The leader leaves as soon as both have
/// answered, which is well under `bank_wait_secs`.
#[tokio::test]
async fn a_bank_wait_ends_when_both_followers_answer() {
    let (addr, received, pushed) = scripted_board(
        detour_script(
            &["Pootwaddle   Mystic", "Grumbleweed  Warrior"],
            vec![("i", EMPTY.into())],
        ),
        vec![
            (
                "look",
                Duration::ZERO,
                "follow",
                "\r\nPootwaddle started to follow you.\r\nGrumbleweed started to follow you.\r\n"
                    .into(),
            ),
            (
                "par",
                Duration::from_millis(300),
                "ok one",
                "\r\nPootwaddle telepaths: @ok\r\n".into(),
            ),
            (
                "par",
                Duration::from_millis(900),
                "ok two",
                "\r\nGrumbleweed telepaths: @ok\r\n".into(),
            ),
        ],
    )
    .await;
    let session = session_with(addr, PartyConfig::default(), armed_gate()).await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(bank_content()));

    let graph = bank_corridor();
    let cfg = lap_config();
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let (notices, said) = collected();

    let (end, stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, World::over(graph.clone()), &plan, Live::fixed(bot, cfg), None, &notices),
    )
    .await
    .expect("run_farm should finish, not hang")
    .expect("the lap must finish");
    let ended = Instant::now();
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let heard: Vec<String> = log.iter().map(|(_, l)| l.clone()).collect();
    let par_at = at(&log, "par").expect("the leader asked who is in the party");
    let second = pushed_at(&pushed, "ok two")
        .unwrap_or_else(|| panic!("the leg finished before the second @ok: {heard:?}"));
    assert!(
        ended >= second,
        "the leader waited for both followers: {heard:?}"
    );
    let waited = ended.saturating_duration_since(par_at);
    assert!(
        waited < Duration::from_secs(5),
        "both answers ended the wait, not bank_wait_secs: {waited:?} {heard:?}"
    );
    let said = said.lock().unwrap().clone();
    assert!(
        said.iter()
            .any(|l| l == "party: waiting at the bank for Grumbleweed, Pootwaddle"),
        "both followers are held: {said:?}"
    );
    assert!(
        !said.iter().any(|l| l.contains("no reply from")),
        "both answered: {said:?}"
    );
}

/// When the board first received this line.
fn at(log: &[(Instant, String)], line: &str) -> Option<Instant> {
    log.iter().find(|(_, l)| l == line).map(|(t, _)| *t)
}

/// When the board volunteered the push with this label.
fn pushed_at(pushed: &Pushed, label: &str) -> Option<Instant> {
    pushed.lock().unwrap().iter().find(|(l, _)| l == label).map(|(_, t)| *t)
}
