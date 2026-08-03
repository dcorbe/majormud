//! The run5 mid-leg entry, mechanized: a full [`run_farm`] against a
//! scripted echoing board (the `nav_echo.rs` harness shape — test
//! crates do not share modules, so the ~40 lines are duplicated here,
//! which is this suite's existing pattern).
//!
//! Live incident (2026-08-01, ~/mmc-probe/run5.raw): monsters entered
//! the room during travel legs — "A giant rat creeps into the room from
//! nowhere.", "acid slime moves into the room from the north." — whiffed
//! at the character, and the runner kept sending nav steps. The defence
//! never started because nothing interrupted the leg.
//!
//! A `farm_live` (in-process server) version of this test is not
//! possible today: the core's `debug_move_monster` hook is not exposed
//! through `mud_server::Server`, and swarm rooms boot-fill before login
//! so anything on the path is already listed in the arrival block and
//! trips `Sighted` first. The scripted board is the deterministic
//! substitute.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mud_client::bot::BotConfig;
use mud_client::farm::{FarmConfig, FarmEnd, FarmPlan, run_farm};
use mud_client::graph::{ExitEdge, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const START: RoomId = RoomId { map: 1, room: 1 };
const MIDWAY: RoomId = RoomId { map: 1, room: 2 };
const STOP: RoomId = RoomId { map: 1, room: 3 };

fn room_block(name: &str, also_here: Option<&str>, exits: &str) -> String {
    room_block_hp(name, also_here, exits, 30)
}

/// The prompt after a block is how HP reaches the client, so scenarios
/// about the departure gate pick the number each block carries.
fn room_block_hp(name: &str, also_here: Option<&str>, exits: &str, hp: i32) -> String {
    let also = match also_here {
        Some(names) => format!("Also here: {names}.\r\n"),
        None => String::new(),
    };
    format!("\r\n\x1b[1;36m{name}\r\n{also}Obvious exits: {exits}\r\n[HP={hp}/MA=0]:")
}

/// A block with floor loot: the "You notice ... here." line rides
/// inside the render, before the exits, as the live captures show.
fn room_block_items(name: &str, items: &[&str], exits: &str) -> String {
    format!(
        "\r\n\x1b[1;36m{name}\r\nYou notice {} here.\r\nObvious exits: {exits}\r\n[HP=30/MA=0]:",
        items.join(", ")
    )
}

/// Guard Post -> Inner Ward -> Keep, one straight corridor. The entry
/// happens at Inner Ward — mid-leg, not at the stop — so the defence
/// has to fire from the walk, not from the stop pump.
fn corridor() -> Arc<RoomGraph> {
    let mut start = GraphRoom {
        name: "Guard Post".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    start.exits[Direction::North as usize] = Some(ExitEdge {
        dest: MIDWAY,
        exit_type: 0,
        command: None,
    });
    let mut midway = GraphRoom {
        name: "Inner Ward".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    midway.exits[Direction::North as usize] = Some(ExitEdge {
        dest: STOP,
        exit_type: 0,
        command: None,
    });
    midway.exits[Direction::South as usize] = Some(ExitEdge {
        dest: START,
        exit_type: 0,
        command: None,
    });
    let mut stop = GraphRoom {
        name: "Keep".into(),
        exits: Default::default(),
        light: 0,
        ..Default::default()
    };
    stop.exits[Direction::South as usize] = Some(ExitEdge {
        dest: MIDWAY,
        exit_type: 0,
        command: None,
    });
    Arc::new(RoomGraph::from_rooms(vec![
        (START, start),
        (MIDWAY, midway),
        (STOP, stop),
    ]))
}

/// A board driven by a per-line script: `(matcher, reply)`, each entry
/// used once, first unused match wins. Unmatched lines echo + say back.
/// Every line the board receives is logged for ordering assertions.
async fn scripted_board(
    script: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(room_block("Guard Post", None, "north").as_bytes())
            .await
            .unwrap();
        // One-shot entries consumed in order; a re-ask (the runner may
        // poke the same `look` more than once) replays the most
        // recently consumed matching entry, exactly as a real board
        // re-answers a look with the room it is still showing.
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
                // Strictly sequential: only the next unconsumed entry
                // is eligible, so a repeated ask can never steal a
                // block scripted for later in the walk.
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

/// The whole run5 shape: walk toward the stop, have a rat walk in
/// mid-leg, and demand the runner turn and kill it — look first, swing
/// off the look's block, never off the entry wording — then resume the
/// leg and finish the lap.
#[tokio::test]
async fn a_monster_entering_mid_leg_is_fought_where_it_stands() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        // verify_start's attributed look.
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        // The leg's first step: the rat walks in behind the echo,
        // before the arrival block — run5's exact interleaving.
        (
            "n",
            format!(
                "\r\nn\r\nA giant rat creeps into the room from nowhere.\r\n{}",
                room_block("Inner Ward", None, "north south")
            ),
        ),
        // The defence's opening ask. The block, not the entry wording,
        // names the target.
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block("Inner Ward", Some("giant rat"), "north south")
            ),
        ),
        (
            "a rat",
            "\r\na rat\r\nYou smack giant rat for 12 damage!\r\nThe giant rat falls to the ground with a tortured squeak.\r\nYou gain 25 experience.\r\n*Combat Off*\r\n[HP=30/MA=0]:"
                .into(),
        ),
        // No post-kill look: the model subtracted the corpse the death
        // line named, so the leg resumes on evidence it already had.
        ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
        // The stop's own look: empty, dwell 0, lap done.
        ("look", format!("\r\nlook{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: 0,
        // The hard pin: an entry is the farm noticing work, not an
        // emergency. If it touched the interrupt budget, this run
        // would end TooHurt instead of LoopsDone.
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
    assert!(stats.kills >= 1, "the rat should have died: {stats:?}");
    assert!(
        stats.sightings >= 1,
        "the entry counts as work noticed: {stats:?}"
    );
    assert_eq!(
        stats.interrupts, 0,
        "an entry must never spend the emergency budget: {stats:?}"
    );

    // The defence happened where it should: the attack went out after
    // the entry's step and before the leg's next step.
    let log = received.lock().unwrap();
    let first_n = log.iter().position(|l| l == "n").expect("first step");
    let attack = log
        .iter()
        .position(|l| l == "a rat")
        .expect("the rat was attacked: {log:?}");
    let second_n = log.iter().rposition(|l| l == "n").expect("the leg resumed");
    assert!(
        first_n < attack && attack < second_n,
        "defence out of order: {log:?}"
    );
}

/// The run6 departure-gate incident, mechanized: the character rests
/// below the depart threshold, a rat walks in mid-rest, and the runner
/// must notice — the pokes that carry HP also carry the room — defend
/// where it stands, and only then leave fit. The blind version rested
/// through the bites until `max_rest_seconds` expired (observed live at
/// 12 HP beside a giant rat, and 37→22 under a three-mob swarm).
#[tokio::test]
async fn a_rest_contested_by_an_arrival_defends_instead_of_dozing() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=20/MA=0]:"
                .into(),
        ),
        // verify_start: wounded, alone — resting here is correct.
        (
            "look",
            format!("\r\nlook{}", room_block_hp("Guard Post", None, "north", 20)),
        ),
        (
            "rest",
            "\r\nrest\r\nYou are now resting.\r\n[HP=20/MA=0]:".into(),
        ),
        // The gate's HP poke answers with the rat that walked in.
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block_hp("Guard Post", Some("giant rat"), "north", 20)
            ),
        ),
        // The defence's own opening ask sees it too.
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block_hp("Guard Post", Some("giant rat"), "north", 20)
            ),
        ),
        (
            "a rat",
            "\r\na rat\r\nYou smack giant rat for 12 damage!\r\nThe giant rat falls to the ground with a tortured squeak.\r\nYou gain 25 experience.\r\n*Combat Off*\r\n[HP=26/MA=0]:"
                .into(),
        ),
        // No post-kill look. The kill's own prompt carried HP back over
        // the gate, and the model knows the room is clear.
        ("n", format!("\r\nn{}", room_block_hp("Inner Ward", None, "north south", 26))),
        ("n", format!("\r\nn{}", room_block_hp("Keep", None, "south", 26))),
        ("look", format!("\r\nlook{}", room_block_hp("Keep", None, "south", 26))),
    ])
    .await;
    let session = session_for(addr).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        // The gate is ON: 80% of 30 = 24, and the character sits at 20.
        depart_at_percent: 80,
        // Short leash so the BLIND failure mode (rest to the deadline,
        // then depart wounded past the rat) fails fast instead of
        // hanging the suite.
        max_rest_seconds: 5,
        // The hard pin again: a contested rest is the farm noticing
        // work, never an emergency.
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
            "the run must survive the contested rest: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };

    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert!(
        stats.kills >= 1,
        "the rat interrupting the rest should have died: {stats:?}\nboard received: {:?}",
        received.lock().unwrap()
    );
    assert_eq!(
        stats.interrupts, 0,
        "a contested rest must never spend the emergency budget: {stats:?}"
    );

    // The story in order: rested, noticed, fought, and only then left.
    let log = received.lock().unwrap();
    let rest = log
        .iter()
        .position(|l| l == "rest")
        .expect("the gate rested");
    let attack = log
        .iter()
        .position(|l| l == "a rat")
        .expect("the rat was fought");
    let depart = log.iter().position(|l| l == "n").expect("the leg departed");
    assert!(
        rest < attack && attack < depart,
        "defence out of order: {log:?}"
    );
}

/// Money on a travel leg: the arrival block lists a pile, the walk
/// stops exactly as it would for a monster, the defence pump sweeps the
/// coins, and the lap finishes. Coins never spend the emergency budget.
#[tokio::test]
async fn a_pile_on_a_travel_leg_is_swept_without_losing_the_lap() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block("Guard Post", None, "north")),
        ),
        // The leg's first step arrives on a room with cash on the floor.
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_items("Inner Ward", &["49 copper farthings"], "north south")
            ),
        ),
        (
            "get copper",
            "\r\nget copper\r\nYou picked up 49 copper farthings\r\n[HP=30/MA=0]:".into(),
        ),
        // No re-look scripted: the seeded arrival block already proves
        // the room holds no work, so the pump ends the moment the sweep
        // is queued and the leg resumes directly.
        ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
        ("look", format!("\r\nlook{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: 0,
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        auto_get: true,
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
            "the run must survive the pile: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };

    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert_eq!(
        stats.interrupts, 0,
        "loot must never spend the emergency budget: {stats:?}"
    );

    let log = received.lock().unwrap();
    let first_n = log.iter().position(|l| l == "n").expect("first step");
    let get = log
        .iter()
        .position(|l| l == "get copper")
        .unwrap_or_else(|| panic!("the pile was swept: {log:?}"));
    let second_n = log.iter().rposition(|l| l == "n").expect("the leg resumed");
    assert!(
        first_n < get && get < second_n,
        "sweep out of order: {log:?}"
    );
}

/// The Arena standoff, mechanized: a delay-0 respawn room shared with
/// another player never proves empty — every look lists a fresh rat —
/// so the evidence rule holds the stop open forever. `stop_seconds`
/// caps it: the lap moves on, and the room gets its next chance when
/// the circuit comes round.
#[tokio::test]
async fn an_endless_stop_is_left_when_its_cap_expires() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block("Guard Post", None, "north")),
        ),
        // Arriving on the stop finds it occupied — and the sticky
        // replays below keep it occupied forever: every re-look lists
        // the rat again (the respawn), every attack kills one.
        (
            "n",
            format!(
                "\r\nn{}",
                room_block("Inner Ward", Some("giant rat"), "north south")
            ),
        ),
        (
            "a rat",
            "\r\na rat\r\nYou smack giant rat for 12 damage!\r\nThe giant rat falls to the ground with a tortured squeak.\r\nYou gain 25 experience.\r\n*Combat Off*\r\n[HP=30/MA=0]:"
                .into(),
        ),
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block("Inner Ward", Some("giant rat"), "north south")
            ),
        ),
    ])
    .await;
    let session = session_for(addr).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: 0,
        // The standoff needs a respawn budget to BE a standoff. Every
        // kill now empties the model outright, so with no budget the
        // stop would simply end -- correctly, on a farm that was told
        // not to wait for respawns. Told to wait, it can never satisfy
        // the budget here, and that is the deadlock under test.
        dwell_empty_seconds: 5,
        // The knob under test: without it this run never ends.
        stop_seconds: 2,
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
    .expect("the cap must end a stop that cannot go quiet")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the run must end by cap, not error: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };

    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert!(stats.kills >= 1, "the respawn loop ran: {stats:?}");
    let attacks = received.lock().unwrap().iter().filter(|l| *l == "a rat").count();
    assert!(
        attacks >= 2,
        "the stop should have fought the respawns until the cap: {:?}",
        received.lock().unwrap()
    );
}

/// The cwrun3 death spiral, mechanized: a flee must REST where it
/// landed before walking back into the fight it ran from.
///
/// Live incident (2026-08-02, cwrun3.raw ~line 4863): AutoFlee bolted at
/// 12 HP of 52, `recover` walked straight back, and the arrival block
/// re-engaged the cave bear at 12 HP — `south / n / look / a bear`, five
/// times over ~70 prompts, pinned at 12-15 HP, until it died.
///
/// Three correct rules closed the loop. A heal is suppressed while the
/// room holds work (resting beside a monster is its own spiral),
/// `recover` is deliberately not hp-guarded (it runs below
/// `interrupt_at_percent` by construction), and nothing gates an attack
/// on health. The room the flee landed in is the one place where
/// resting is both safe and possible, so that is where it happens.
#[tokio::test]
async fn a_flee_rests_before_it_walks_back() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        // verify_start: fit, alone.
        (
            "look",
            format!("\r\nlook{}", room_block_hp("Guard Post", None, "north", 30)),
        ),
        // The leg to the stop.
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_hp("Inner Ward", Some("cave bear"), "south", 30)
            ),
        ),
        // The stop pump's opening ask.
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block_hp("Inner Ward", Some("cave bear"), "south", 30)
            ),
        ),
        // The swing that goes badly: 10 of 30 is under flee_at_percent.
        (
            "a bear",
            "\r\na bear\r\nYou smack cave bear for 2 damage!\r\nThe cave bear bites you for 20 damage!\r\n[HP=10/MA=0]:"
                .into(),
        ),
        // AutoFlee bolts down the only exit.
        (
            "south",
            format!("\r\nsouth{}", room_block_hp("Guard Post", None, "north", 10)),
        ),
        // THE FIX: rest here, where it is safe, before going back.
        (
            "rest",
            "\r\nrest\r\nYou are now resting.\r\n[HP=26/MA=0]:".into(),
        ),
        // Fit again, walk back. The bear has wandered off, so the stop
        // proves empty and the lap finishes.
        (
            "n",
            format!("\r\nn{}", room_block_hp("Inner Ward", None, "south", 26)),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block_hp("Inner Ward", None, "south", 26)),
        ),
    ])
    .await;
    let session = session_for(addr).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        // 80% of 30 = 24: fit to depart at 30, not at 10.
        depart_at_percent: 80,
        max_rest_seconds: 5,
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        auto_flee: true,
        flee_at_percent: 50,
        auto_heal: true,
        rest_at_percent: 80,
        max_hp: 30,
        ..BotConfig::default()
    };

    let (_end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the run must survive the flee: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };

    assert!(stats.flees >= 1, "the bot should have fled: {stats:?}");

    let log = received.lock().unwrap();
    let flee = log
        .iter()
        .position(|l| l == "south")
        .unwrap_or_else(|| panic!("the bot should have fled: {log:?}"));
    let rest = log
        .iter()
        .skip(flee)
        .position(|l| l == "rest")
        .map(|i| i + flee)
        .unwrap_or_else(|| panic!("a flee must rest before walking back: {log:?}"));
    let back = log
        .iter()
        .skip(flee)
        .position(|l| l == "n")
        .map(|i| i + flee)
        .unwrap_or_else(|| panic!("it should have walked back: {log:?}"));
    assert!(
        rest < back,
        "the rest must come BEFORE the walk back, or the fight resumes wounded: {log:?}"
    );
    // And it must not have swung again at 10 HP on the way.
    let swings_after_flee = log.iter().skip(flee).take(back - flee).filter(|l| l.starts_with("a ")).count();
    assert_eq!(
        swings_after_flee, 0,
        "nothing may re-engage between the flee and the walk back: {log:?}"
    );
}
