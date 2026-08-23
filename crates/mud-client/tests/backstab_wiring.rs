//! Production wiring: `run_farm`'s own `Navigator` actually carries
//! `with_backstab` end to end, and `farm_stop` actually arms
//! `Bot::arm_backstab_opener` from the leg's own `Arrival` -- the two
//! halves `tests/backstab_opener.rs` (nav.rs's swap/sneak ordering,
//! bot.rs's opener consumption) already prove in isolation, but neither
//! side had a production caller before this branch.
//!
//! This is the test that fails without the wiring, in the
//! `tests/session_capabilities.rs` sense: it drives a REAL `run_farm`
//! (the production call site at `farm.rs`'s `farm_loop`, not a
//! hand-built `Navigator`/`Bot` pair) against the real WG3-NT item
//! database (`tests/backstab_opener.rs`'s own fixture: quarterstaff
//! lacks BSAccu, dagger carries it), and checks the wire sees `bs`
//! before `a`.
//!
//! The harness is `tests/farm_scripted.rs`'s shape, duplicated per this
//! suite's existing convention (test crates share no modules).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::bot::BotConfig;
use mud_client::farm::{FarmConfig, FarmEnd, FarmPlan, run_farm};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const START: RoomId = RoomId { map: 1, room: 1 };
const STOP: RoomId = RoomId { map: 1, room: 2 };

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn room_block(name: &str, also_here: Option<&str>, exits: &str) -> String {
    let also = match also_here {
        Some(names) => format!("Also here: {names}.\r\n"),
        None => String::new(),
    };
    format!("\r\n\x1b[1;36m{name}\r\n{also}Obvious exits: {exits}\r\n[HP=30/MA=0]:")
}

fn corridor() -> Arc<RoomGraph> {
    let mut start = GraphRoom {
        name: "Guard Post".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    start.exits[Direction::North as usize] = Some(ExitEdge {
        dest: STOP,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut stop = GraphRoom {
        name: "Keep".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    stop.exits[Direction::South as usize] = Some(ExitEdge {
        dest: START,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    Arc::new(RoomGraph::from_rooms(vec![(START, start), (STOP, stop)]))
}

/// `tests/farm_scripted.rs`'s own `scripted_board`: a per-line script,
/// each entry consumed once in order; unmatched lines echo + say back.
async fn scripted_board(
    script: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
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
    };
    Session::connect(&profile, None).await.unwrap()
}

/// A stealthy character wielding a non-BS-capable weapon with a
/// BS-capable one in the pack: walking into a room with a monster
/// listed in the arrival block must open with `bs`, swap in first
/// (`eq a dagger` before `sneak` before `n` -- already proven in
/// `tests/backstab_opener.rs`), and restore the primary weapon
/// (`eq quarterstaff`) right behind the backstab.
#[tokio::test]
async fn a_sneaking_farm_opens_the_arrival_with_a_backstab() {
    let stat_reply = "\r\nstat\r\n\
        Name: Beef                             Lives/CP:    9/100\r\n\
        Race: Dark-Elf    Exp: 0               Perception:     43\r\n\
        Class: Ninja      Level: 1             Stealth:        56\r\n\
        Hits:    30/30    Armour Class:   0/0  Thievery:        0\r\n\
        \x20                                       Traps:          29\r\n\
        \x20                                       Picklocks:       0\r\n\
        Strength:  40     Agility: 50          Tracking:       26\r\n\
        Intellect: 50     Health:  30          Martial Arts:   51\r\n\
        Willpower: 30     Charm:   40          MagicRes:       35\r\n\
        [HP=30/MA=0]:"
        .to_string();
    let (addr, received) = scripted_board(vec![
        ("stat", stat_reply),
        (
            "i",
            "\r\ni\r\nYou are carrying quarterstaff (Two handed), a dagger.\r\n\
             You have no keys.\r\nWealth: 0 copper farthings\r\n\
             Encumbrance: 100/2400 - Light [4%]\r\n[HP=30/MA=0]:"
                .to_string(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        (
            "sneak",
            "\r\nsneak\r\nAttempting to sneak...\r\n[HP=30/MA=0]:".to_string(),
        ),
        (
            "n",
            format!("\r\nn{}", room_block("Keep", Some("kobold thief"), "south")),
        ),
        (
            "bs thief",
            "\r\nbs thief\r\nYou sneak up on the kobold thief and drive your dagger home!\r\n\
             The kobold thief falls to the ground with a tortured squeak.\r\n\
             You gain 40 experience.\r\n*Combat Off*\r\n[HP=30/MA=0]:"
                .to_string(),
        ),
        (
            "eq quarterstaff",
            "\r\neq quarterstaff\r\nYou are now holding quarterstaff.\r\n[HP=30/MA=0]:".to_string(),
        ),
        ("look", format!("\r\nlook{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;

    // The realistic realm-entry priming a live TUI session would have
    // already done -- `tui::on_realm_entry` sends `stat`... actually it
    // does not; a player checking their own sheet once is what actually
    // populates `Session::stats()` in practice (nothing in mud-client
    // sends `stat` on its own account). `i` matches `on_realm_entry`'s
    // own send exactly.
    session.send("stat");
    session.send("i");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if session.stats().stealth == Some(56) && session.wielded().as_deref() == Some("quarterstaff")
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("stat/i priming never landed");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let graph = corridor();
    let cfg = FarmConfig {
        content: db_path(),
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: 0,
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };

    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the run must survive the entry: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };

    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert!(stats.kills >= 1, "the thief should have died: {stats:?}");

    let log = received.lock().unwrap();
    assert!(
        log.contains(&"bs thief".to_string()),
        "the opener must have fired a backstab: {log:?}"
    );
    assert!(
        !log.contains(&"a thief".to_string()),
        "an ordinary swing must never have been sent: {log:?}"
    );

    let swap = log.iter().position(|l| l == "eq a dagger").expect("swap before the move: {log:?}");
    let sneak = log.iter().position(|l| l == "sneak").expect("sneak before the move: {log:?}");
    let step = log.iter().position(|l| l == "n").expect("the leg walked: {log:?}");
    let bs = log.iter().position(|l| l == "bs thief").expect("backstab fired: {log:?}");
    let restore = log
        .iter()
        .position(|l| l == "eq quarterstaff")
        .expect("the primary weapon was restored: {log:?}");
    assert!(
        swap < sneak && sneak < step && step < bs && bs < restore,
        "decide -> swap -> sneak -> move -> backstab -> restore, in that exact order: {log:?}"
    );
}

/// The other half of the handoff: an arrival the walk did NOT believe
/// itself sneaking for (Stealth 0, so `Navigator::goto` never even sends
/// `sneak`) must open the SAME monster with an ordinary attack, never
/// `bs`. Mutation target for "the opener fires when not believed
/// sneaking" -- hardcode `farm_stop`'s `bot.arm_backstab_opener(sneaking,
/// ..)` call to `true` regardless of `sneaking` and this must fail.
#[tokio::test]
async fn a_non_stealthy_farm_never_opens_with_a_backstab() {
    let stat_reply = "\r\nstat\r\n\
        Name: Beef                             Lives/CP:    9/100\r\n\
        Race: Dark-Elf    Exp: 0               Perception:     43\r\n\
        Class: Ninja      Level: 1             Stealth:         0\r\n\
        Hits:    30/30    Armour Class:   0/0  Thievery:        0\r\n\
        \x20                                       Traps:          29\r\n\
        \x20                                       Picklocks:       0\r\n\
        Strength:  40     Agility: 50          Tracking:       26\r\n\
        Intellect: 50     Health:  30          Martial Arts:   51\r\n\
        Willpower: 30     Charm:   40          MagicRes:       35\r\n\
        [HP=30/MA=0]:"
        .to_string();
    let (addr, received) = scripted_board(vec![
        ("stat", stat_reply),
        (
            "i",
            "\r\ni\r\nYou are carrying quarterstaff (Two handed), a dagger.\r\n\
             You have no keys.\r\nWealth: 0 copper farthings\r\n\
             Encumbrance: 100/2400 - Light [4%]\r\n[HP=30/MA=0]:"
                .to_string(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        (
            "n",
            format!("\r\nn{}", room_block("Keep", Some("kobold thief"), "south")),
        ),
        (
            "a thief",
            "\r\na thief\r\nYou smack the kobold thief for 12 damage!\r\n\
             The kobold thief falls to the ground with a tortured squeak.\r\n\
             You gain 40 experience.\r\n*Combat Off*\r\n[HP=30/MA=0]:"
                .to_string(),
        ),
        ("look", format!("\r\nlook{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;

    session.send("stat");
    session.send("i");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if session.stats().stealth == Some(0) && session.wielded().as_deref() == Some("quarterstaff")
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("stat/i priming never landed");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let graph = corridor();
    let cfg = FarmConfig {
        content: db_path(),
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: 0,
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    };

    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the run must survive the entry: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };

    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert!(stats.kills >= 1, "the thief should have died: {stats:?}");

    let log = received.lock().unwrap();
    assert!(
        !log.contains(&"bs thief".to_string()),
        "an un-sneaking arrival must never open with a backstab: {log:?}"
    );
    assert!(
        log.contains(&"a thief".to_string()),
        "the ordinary attack must have fired instead: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l == "sneak"),
        "zero Stealth must never send `sneak` at all: {log:?}"
    );
}
