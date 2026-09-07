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
use mud_client::farm::{FarmConfig, FarmEnd, FarmError, FarmPlan, run_farm};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const START: RoomId = RoomId { map: 1, room: 1 };
const MIDWAY: RoomId = RoomId { map: 1, room: 2 };
const STOP: RoomId = RoomId { map: 1, room: 3 };

/// A notices sink that keeps nothing, for the runs whose notices are
/// not what is under test.
fn quiet() -> mud_client::farm::Notices {
    Arc::new(|_: &str| {})
}

/// A notices sink that keeps every line, and the lines it kept.
fn collected() -> (mud_client::farm::Notices, Arc<Mutex<Vec<String>>>) {
    let said = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&said);
    (
        Arc::new(move |line: &str| into.lock().unwrap().push(line.to_string())),
        said,
    )
}

fn room_block(name: &str, also_here: Option<&str>, exits: &str) -> String {
    room_block_hp(name, also_here, exits, 30)
}

/// The prompt after a block is how HP reaches the client, so scenarios
/// about the departure gate pick the number each block carries.
fn room_block_hp(name: &str, also_here: Option<&str>, exits: &str, hp: i32) -> String {
    room_block_vitals(name, also_here, exits, hp, 0, None)
}

/// As above, with a mana pool and an optional status word ("Meditating",
/// "Resting") the way the board tacks one onto the prompt itself. The
/// prompt is the only place mana or status ever reaches the client, so
/// any scenario about casting or the departure gate has to carry them.
fn room_block_vitals(
    name: &str,
    also_here: Option<&str>,
    exits: &str,
    hp: i32,
    mana: i32,
    status: Option<&str>,
) -> String {
    let also = match also_here {
        Some(names) => format!("Also here: {names}.\r\n"),
        None => String::new(),
    };
    let status = match status {
        Some(word) => format!(" ({word}) "),
        None => String::new(),
    };
    format!("\r\n\x1b[1;36m{name}\r\n{also}Obvious exits: {exits}\r\n[HP={hp}/MA={mana}]:{status}")
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
        requirement: ExitRequirement::None,
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
        requirement: ExitRequirement::None,
    });
    midway.exits[Direction::South as usize] = Some(ExitEdge {
        dest: START,
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
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
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

/// The shared session, with deposits off. A run with deposits on
/// reads the purse once before its first leg, and a script that does
/// not answer that `i` costs the run the whole ask deadline. Only the
/// tests that bank turn deposits on, and they answer it.
async fn session_for(addr: std::net::SocketAddr) -> Session {
    session_with_bank(
        addr,
        mud_client::bank::BankConfig {
            auto_deposit: false,
            ..Default::default()
        },
    )
    .await
}

/// A session whose profile carries a chosen `[bank]` table, for the
/// scenarios that turn on what the operator configured.
async fn session_with_bank(
    addr: std::net::SocketAddr,
    bank: mud_client::bank::BankConfig,
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
        ..Default::default()
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
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
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
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
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
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        // The gate is ON: 80% of 30 = 24, and the character sits at 20.
        depart_at_percent: Some(80),
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
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
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
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
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
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
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
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
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
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
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
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        // 80% of 30 = 24: fit to depart at 30, not at 10.
        depart_at_percent: Some(80),
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
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
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

/// The spiral's other half, and the reason spell healing exists.
///
/// `rest` is suppressed while the room holds a fight, and correctly: the
/// board disengages combat to rest, the re-engage breaks the rest, and
/// the pair alternate every round while the monster keeps swinging
/// (2026-08-01, HP 21/52 beside a cave bear). But that left a wounded
/// character in an occupied room with NO recovery at all — fight on, or
/// run.
///
/// A cast disengages nothing, so it is not the same decision. Below the
/// spell mark and still in the fight, the bot must cast; `rest` must
/// still not go out while the bear is standing there.
#[tokio::test]
async fn a_fight_below_the_spell_mark_is_healed_not_rested() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=20]:"
                .into(),
        ),
        // The book the heal is discovered from. Nothing is configured:
        // `cast heal` is the cheapest thing in here.
        (
            "spells",
            "\r\nspells\r\nYou have the following spells:\r\nLevel Mana Short Spell Name\r\n  1   3    heal  minor healing\r\n  8   9    maj   major healing\r\n[HP=30/MA=20]:"
                .into(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block_vitals("Guard Post", None, "north", 30, 20, None)),
        ),
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_vitals("Inner Ward", Some("cave bear"), "south", 30, 20, None)
            ),
        ),
        // The arrival block names the bear and is attributed to the
        // step, so the bot engages off it directly — there is no
        // intervening look.
        //
        // The swing lands the character on 18 of 30 — 60%, under the
        // spell mark of 80 and over the flee mark of 20. The bear is
        // still up, so resting is off the table and casting is not.
        (
            "a bear",
            "\r\na bear\r\nYou smack cave bear for 2 damage!\r\nThe cave bear bites you for 12 damage!\r\n[HP=18/MA=20]:"
                .into(),
        ),
        // The cast and the round it happens in arrive together, which is
        // what a real board does: the character never swings again by
        // its own decision, because once engaged the ROUNDS are the
        // board's. The bear dies in this one, so the stop proves empty
        // and the lap finishes.
        (
            "cast heal",
            "\r\ncast heal\r\nYou cast minor healing!\r\nYou feel better.\r\nYou smack cave bear for 30 damage!\r\nThe cave bear collapses in a heap.\r\nYou gain 300 experience.\r\n*Combat Off*\r\n[HP=27/MA=17]:"
                .into(),
        ),
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block_vitals("Inner Ward", None, "south", 27, 17, None)
            ),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        auto_heal: true,
        auto_flee: true,
        minor_heal_at_percent: 80,
        rest_at_percent: 60,
        flee_at_percent: 20,
        max_hp: 30,
        ..BotConfig::default()
    };

    // The run's startup notices, which is where the heal it found gets
    // named. Collected rather than printed, because the window paints
    // them and stderr under the TUI is the raw terminal.
    let (notices, said) = collected();
    let finished = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &notices),
    )
    .await;
    // The log is the diagnosis for BOTH endings here, and a hang is the
    // likelier one: with the cast suppressed this scenario deadlocks —
    // the bot will not swing again while engaged, and the board says
    // nothing unprompted — so "what did it send" is the whole question.
    let Ok(result) = finished else {
        panic!("run_farm hung; board received: {:?}", received.lock().unwrap());
    };
    let (_end, stats) = result.unwrap_or_else(|e| {
        panic!(
            "the run must survive the fight: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        )
    });

    let log = received.lock().unwrap();
    assert!(
        log.iter().any(|l| l == "cast heal"),
        "18 of 30 is under the spell mark and the bear is still up: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l == "rest"),
        "resting beside the bear is the spiral this replaced: {log:?}"
    );
    assert!(stats.kills >= 1, "the bear should still have died: {stats:?}");
    let said = said.lock().unwrap();
    assert!(
        said.iter().any(|l| l == "minor heal below 80% with `cast heal` (3 mana)"),
        "the run must say which spell it found and at which mark: {said:?}"
    );
}

/// The heal line the run above collects must not ALSO have gone to
/// stderr, which under the TUI is the terminal the front end paints.
///
/// The harness swallows a test's own stderr, so the check is a second
/// run of that one test in a child process with the capture off. What
/// the child writes to fd 2 is what a window would have had scribbled
/// over its screen.
#[test]
fn the_heal_notice_never_reaches_stderr() {
    let out = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "a_fight_below_the_spell_mark_is_healed_not_rested",
            "--nocapture",
        ])
        .output()
        .expect("re-run this binary");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "the child run must pass: {stdout}");
    // `--exact` with no match exits 0 and runs nothing, so a rename of
    // the test above would leave this passing on an empty stderr.
    assert!(
        stdout.contains("1 passed"),
        "the child must have run the test, not filtered it away: {stdout}"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("minor heal below 80%"),
        "the heal notice belongs to the sink, not to stderr: {err:?}"
    );
}

/// Flee outranks everything, and the spell mark does not change that.
/// Below `flee_at_percent` the bot is leaving; a cast would spend the
/// round it leaves in, and staying to heal is what gets a character
/// killed. The mark being the HIGHEST of the three makes this easy to
/// get wrong — under 20% the character is under all three at once.
#[tokio::test]
async fn below_the_flee_mark_it_runs_and_does_not_cast() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=20]:"
                .into(),
        ),
        (
            "spells",
            "\r\nspells\r\nYou have the following spells:\r\nLevel Mana Short Spell Name\r\n  1   3    heal  minor healing\r\n[HP=30/MA=20]:"
                .into(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block_vitals("Guard Post", None, "north", 30, 20, None)),
        ),
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_vitals("Inner Ward", Some("cave bear"), "south", 30, 20, None)
            ),
        ),
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block_vitals("Inner Ward", Some("cave bear"), "south", 30, 20, None)
            ),
        ),
        // 5 of 30 is 16% — under the flee mark, and under the spell mark
        // as well. Mana is untouched, so nothing but the policy stops a
        // cast going out.
        (
            "a bear",
            "\r\na bear\r\nYou smack cave bear for 2 damage!\r\nThe cave bear mauls you for 25 damage!\r\n[HP=5/MA=20]:"
                .into(),
        ),
        (
            "south",
            format!("\r\nsouth{}", room_block_vitals("Guard Post", None, "north", 5, 20, None)),
        ),
        (
            "rest",
            "\r\nrest\r\nYou are now resting.\r\n[HP=30/MA=20]:".into(),
        ),
        (
            "n",
            format!("\r\nn{}", room_block_vitals("Inner Ward", None, "south", 30, 20, None)),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block_vitals("Inner Ward", None, "south", 30, 20, None)),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(80),
        max_rest_seconds: 5,
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        auto_heal: true,
        auto_flee: true,
        minor_heal_at_percent: 80,
        rest_at_percent: 60,
        flee_at_percent: 50,
        max_hp: 30,
        ..BotConfig::default()
    };

    let (_end, stats) = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    .unwrap_or_else(|e| {
        panic!(
            "the run must survive the flee: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        )
    });

    assert!(stats.flees >= 1, "the bot should have fled: {stats:?}");
    let log = received.lock().unwrap();
    let flee = log
        .iter()
        .position(|l| l == "south")
        .unwrap_or_else(|| panic!("the bot should have fled: {log:?}"));
    assert!(
        !log[..=flee].iter().any(|l| l.starts_with("cast ")),
        "a cast must not delay the flee: {log:?}"
    );
}

/// The dead zone: with `auto_flee = false` the bot never runs, so the
/// flee mark is a number nothing acts on — and suppressing the cast
/// below it would leave a character that neither ran nor healed, which
/// is worse than either. Under every mark at once, the cast must still
/// go out.
#[tokio::test]
async fn with_fleeing_off_the_flee_mark_does_not_suppress_the_cast() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=20]:"
                .into(),
        ),
        (
            "spells",
            "\r\nspells\r\nYou have the following spells:\r\nLevel Mana Short Spell Name\r\n  1   3    heal  minor healing\r\n[HP=30/MA=20]:"
                .into(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block_vitals("Guard Post", None, "north", 30, 20, None)),
        ),
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_vitals("Inner Ward", Some("cave bear"), "south", 30, 20, None)
            ),
        ),
        // 5 of 30 is 16%: below the spell mark, below rest, below flee.
        (
            "a bear",
            "\r\na bear\r\nYou smack cave bear for 2 damage!\r\nThe cave bear mauls you for 25 damage!\r\n[HP=5/MA=20]:"
                .into(),
        ),
        (
            "cast heal",
            "\r\ncast heal\r\nYou cast minor healing!\r\nYou feel better.\r\nYou smack cave bear for 30 damage!\r\nThe cave bear collapses in a heap.\r\nYou gain 300 experience.\r\n*Combat Off*\r\n[HP=14/MA=17]:"
                .into(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block_vitals("Inner Ward", None, "south", 14, 17, None)),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        auto_heal: true,
        // The whole point of this test.
        auto_flee: false,
        minor_heal_at_percent: 80,
        rest_at_percent: 60,
        flee_at_percent: 30,
        max_hp: 30,
        ..BotConfig::default()
    };

    let finished = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await;
    let Ok(result) = finished else {
        panic!("run_farm hung; board received: {:?}", received.lock().unwrap());
    };
    result.unwrap_or_else(|e| {
        panic!(
            "the run must survive the fight: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        )
    });

    let log = received.lock().unwrap();
    assert!(
        log.iter().any(|l| l == "cast heal"),
        "nothing else was going to happen at 5 of 30: {log:?}"
    );
}

/// A roam runs, works the rooms it can reach, and never enters the
/// walled one. End to end against a board.
#[tokio::test]
async fn a_roam_never_steps_into_a_walled_room() {
    use mud_client::roam::Walls;

    // A(1/1) --north--> B(1/2), and --east--> C(1/3), which is walled.
    let graph = {
        let mut a = GraphRoom {
            name: "Guard Post".into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        a.exits[Direction::North as usize] = Some(ExitEdge {
            dest: MIDWAY,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
        a.exits[Direction::East as usize] = Some(ExitEdge {
            dest: STOP,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
        let mut b = GraphRoom {
            name: "Inner Ward".into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        b.exits[Direction::South as usize] = Some(ExitEdge {
            dest: START,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
        let mut c = GraphRoom {
            name: "Keep".into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        c.exits[Direction::West as usize] = Some(ExitEdge {
            dest: START,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
        Arc::new(RoomGraph::from_rooms(vec![
            (START, a),
            (MIDWAY, b),
            (STOP, c),
        ]))
    };

    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block("Guard Post", None, "north east")),
        ),
        // No `look` per stop: the step that lands on a room is answered
        // with that room's block, and the stop opens from it. A roam
        // spends one command per room worked, which is the whole point
        // of `Arrival::seen`.
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "south"))),
        ("s", format!("\r\ns{}", room_block("Guard Post", None, "north east"))),
    ])
    .await;
    let session = session_for(addr).await;
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let cfg = FarmConfig {
        loops: 0,
        max_seconds: 8,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        stop_seconds: 2,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::roaming(START, Walls::new([STOP]), &graph).expect("a roam of A and B");
    assert!(plan.circuit.is_empty(), "a roam has no circuit");

    let bot = BotConfig {
        max_hp: 30,
        ..BotConfig::default()
    };
    let finished = tokio::time::timeout(
        Duration::from_secs(40),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await;
    let Ok(result) = finished else {
        panic!("run_farm hung; board received: {:?}", received.lock().unwrap());
    };
    let (end, stats) = result.unwrap_or_else(|e| {
        panic!(
            "the roam must survive: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        )
    });

    assert_eq!(end, FarmEnd::TimeUp, "a roam ends on the clock: {stats:?}");
    let log = received.lock().unwrap();
    assert!(
        !log.iter().any(|l| l == "e"),
        "the walled Keep is one step east and was entered: {log:?}"
    );
    assert!(
        log.iter().any(|l| l == "n"),
        "the roam should have worked the room it CAN reach: {log:?}"
    );
    assert!(stats.roamed >= 1, "rooms worked should be counted: {stats:?}");
}

/// A roam whose region reaches a room behind a puzzle whose lever room
/// the roam itself walled off. The exit is priced with hops counted
/// over the whole graph, so the flood puts the vault in the region.
/// The fenced walk then cannot reach the lever at all. The room has to
/// leave the roam, not end the run.
#[tokio::test]
async fn a_roam_drops_a_room_whose_lever_it_cannot_reach() {
    use mud_client::puzzle::{Puzzle, PuzzleAction};
    use mud_client::roam::Walls;

    // A(1/1) --north--> the vault B(1/2), a type 6 exit opened by the
    // lever in the alcove C(1/3), one step east of A. C is walled.
    let graph = {
        let mut a = GraphRoom {
            name: "Guard Post".into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        a.exits[Direction::North as usize] = Some(ExitEdge {
            dest: MIDWAY,
            exit_type: 6,
            command: None,
            requirement: ExitRequirement::Puzzle(Puzzle {
                word: 16,
                actions: vec![PuzzleAction {
                    room: STOP,
                    number: 1,
                    phrases: vec!["pull lever".into()],
                    item: None,
                    reply: Some("You pull the lever.".into()),
                    hops: Some(1),
                }],
            }),
        });
        a.exits[Direction::East as usize] = Some(ExitEdge {
            dest: STOP,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
        let mut b = GraphRoom {
            name: "Vault".into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        b.exits[Direction::South as usize] = Some(ExitEdge {
            dest: START,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
        let mut c = GraphRoom {
            name: "Lever Alcove".into(),
            exits: Default::default(),
            light: 0,
            ..Default::default()
        };
        c.exits[Direction::West as usize] = Some(ExitEdge {
            dest: START,
            exit_type: 0,
            command: None,
            requirement: ExitRequirement::None,
        });
        Arc::new(RoomGraph::from_rooms(vec![
            (START, a),
            (MIDWAY, b),
            (STOP, c),
        ]))
    };

    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        (
            "look",
            format!("\r\nlook{}", room_block("Guard Post", None, "east")),
        ),
        // The vault's wall, refused the way a concealed exit is. The
        // board never opens it, because the lever is never pulled.
        (
            "n",
            "\r\nn\r\nThere is no exit in that direction!\r\n[HP=30/MA=0]:".into(),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;

    let cfg = FarmConfig {
        loops: 0,
        max_seconds: 8,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        stop_seconds: 2,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::roaming(START, Walls::new([STOP]), &graph).expect("a roam of A and B");

    let bot = BotConfig {
        max_hp: 30,
        ..BotConfig::default()
    };
    let finished = tokio::time::timeout(
        Duration::from_secs(40),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await;
    let Ok(result) = finished else {
        panic!("run_farm hung; board received: {:?}", received.lock().unwrap());
    };
    let (end, _stats) = result.unwrap_or_else(|e| {
        panic!(
            "an unopenable room must not end the run: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        )
    });

    assert_eq!(end, FarmEnd::TimeUp, "the roam ends on the clock");
    let log = received.lock().unwrap();
    assert_eq!(
        log.iter().filter(|l| l.as_str() == "n").count(),
        1,
        "the vault leaves the roam after one refusal: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l == "e"),
        "the walled alcove was entered: {log:?}"
    );
}

/// The step that lands on the stop already carried the stop's block out
/// with it, so the stop must not ask for it again.
///
/// This was a whole round-trip per room, and a roam is nothing but
/// rooms: it picks one stop per pass, walks a step or two, and works
/// it. Live, the `look` doubled the commands a roam spent per room and
/// the board answered it with the render it had just sent.
///
/// [`StopState::seed`] has always accepted an arrival block — but only
/// the one an INTERRUPTED leg handed up ([`Interrupt::Sighted`]). A leg
/// that simply arrived threw its block away and the stop opened blind.
#[tokio::test]
async fn a_clean_arrival_is_not_re_asked_at_the_stop() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        // verify_start's attributed look. The only one this run needs.
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south"))),
        // The stop's own block, riding out on the step that landed here.
        ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
        // Scripted only so the OLD behaviour fails on the assertion
        // below rather than on a desync: the fallback would otherwise
        // replay Guard Post's block and the runner would read the
        // redundant look as having been swept back to the start.
        ("look", format!("\r\nlook{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
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
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the lap must finish: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap();
    let last_step = log.iter().rposition(|l| l == "n").expect("the leg walked");
    assert!(
        !log[last_step..].iter().any(|l| l == "look"),
        "the arrival block already said what the look asks for: {log:?}"
    );
}

/// The gate reads both pools. HP is fit, mana is not, so the gate sends
/// `meditate`, and departs once the poke shows the pool over the mark.
#[tokio::test]
async fn the_gate_meditates_for_mana_and_leaves_when_both_pools_clear_the_mark() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=2]:"
                .into(),
        ),
        // verify_start: HP is fit at 30 of 30, mana is not at 2 of 10.
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block_vitals("Guard Post", None, "north", 30, 2, None)
            ),
        ),
        // HP needs nothing, so the gate meditates instead of resting.
        (
            "meditate",
            "\r\nmeditate\r\nYou are now meditating.\r\n[HP=30/MA=2]: (Meditating) ".into(),
        ),
        // The gate's poke shows mana over the mark: 9 of 10 is 90%.
        (
            "look",
            format!(
                "\r\nlook{}",
                room_block_vitals("Guard Post", None, "north", 30, 9, Some("Meditating"))
            ),
        ),
        ("n", format!("\r\nn{}", room_block_hp("Inner Ward", None, "north south", 30))),
        ("n", format!("\r\nn{}", room_block_hp("Keep", None, "south", 30))),
        ("look", format!("\r\nlook{}", room_block_hp("Keep", None, "south", 30))),
    ])
    .await;
    let session = session_for(addr).await;
    // run_farm no longer probes the inventory/spellbook itself
    // (Task 4: read once, at realm entry) -- these scripts still
    // open with those replies, so the probe has to run explicitly
    // here, exactly where `tui::on_realm_entry`/`mmc farm`'s own
    // startup would run it on a real connection.
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        // No farm mark set: the gate falls back to the bot's own
        // `rest_until_percent`.
        depart_at_percent: None,
        max_rest_seconds: 5,
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        max_mana: 10,
        meditate: true,
        rest_until_percent: 80,
        ..BotConfig::default()
    };

    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the run must survive the mana gate: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };

    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    // The story in order: meditated for mana, and only then departed.
    // HP was already fit, so `rest` must never be sent.
    let log = received.lock().unwrap();
    let meditate = log
        .iter()
        .position(|l| l == "meditate")
        .expect("the gate meditated for mana");
    let depart = log.iter().position(|l| l == "n").expect("the leg departed");
    assert!(
        meditate < depart,
        "meditate must precede departure: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l == "rest"),
        "hp was already fit; rest must never be sent: {log:?}"
    );
}

/// The plan cannot catch this pair. It never sees the bot's config, so
/// with no farm mark of its own it does not know the departure gate will
/// release at 80. Caught at run start instead, before a single step goes
/// out, rather than after the patrol has burned its interrupt budget.
#[tokio::test]
async fn an_interrupt_mark_above_the_bots_own_departure_mark_is_refused_at_run_start() {
    let (addr, received) = scripted_board(vec![(
        "inventory",
        "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
            .into(),
    )])
    .await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;
    let after_probe = received.lock().unwrap().len();

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        // No farm mark, so the gate falls back to the bot's 80.
        depart_at_percent: None,
        interrupt_at_percent: 96,
        ..FarmConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("the plan cannot see the bot's mark");
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        rest_until_percent: 80,
        ..BotConfig::default()
    };

    let out = tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should refuse at once, not hang");

    let why = match out {
        Err(FarmError::Config(why)) => why,
        other => panic!("the pair must be refused: {other:?}"),
    };
    assert!(why.contains("96"), "{why}");
    assert!(why.contains("80"), "{why}");

    let log = received.lock().unwrap();
    assert_eq!(
        log.len(),
        after_probe,
        "the refusal must come before anything is sent: {log:?}"
    );
}

/// Sneak persists on the board, so a lap of quiet stops is one `sneak`
/// long, not one per stop. Live 2026-09-04 (test_timing.log, a sewer
/// roam): every room was its own leg, every leg opened with `sneak`,
/// and the board answered "Attempting to sneak..." to a character it
/// was already sneaking. The belief has to outlive the leg that formed
/// it: through the stop that spent nothing, into the next leg.
/// Mutation target: reset the belief per leg and the second `sneak`
/// comes back.
/// The `stat` sheet a sneaking lap opens with: Stealth is what lets
/// the walker sneak at all (`Session::capabilities`).
const NINJA_SHEET: &str = "\r\nstat\r\n\
Name: Beef                             Lives/CP:    9/100\r\n\
Race: Dark-Elf    Exp: 0               Perception:     43\r\n\
Class: Ninja      Level: 1             Stealth:        56\r\n\
Hits:    30/30    Armour Class:   0/0  Thievery:        0\r\n\
                                       Traps:          29\r\n\
                                       Picklocks:      31\r\n\
Strength:  40     Agility: 50          Tracking:       26\r\n\
Intellect: 50     Health:  30          Martial Arts:   51\r\n\
Willpower: 30     Charm:   40          MagicRes:       35\r\n\
[HP=30/MA=0]:";

#[tokio::test]
async fn a_lap_of_quiet_stops_sneaks_once() {
    let (addr, received) = scripted_board(vec![
        ("stat", NINJA_SHEET.into()),
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        ("sneak", "\r\nsneak\r\nAttempting to sneak...\r\n[HP=30/MA=0]:".into()),
        ("n", format!("\r\nn\r\nSneaking...{}", room_block("Inner Ward", None, "north south"))),
        ("n", format!("\r\nn\r\nSneaking...{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;
    assert_eq!(session.capabilities().stealth, 56, "the sheet must have been read");

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into(), "1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
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
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the lap must finish: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");

    let log = received.lock().unwrap();
    assert_eq!(log.iter().filter(|l| *l == "n").count(), 2, "both legs walked: {log:?}");
    assert_eq!(
        log.iter().filter(|l| *l == "sneak").count(),
        1,
        "one arm covers a lap of quiet stops: {log:?}"
    );
}

/// The other half of carrying the belief: a stop that swings has
/// spent it. The character sneaks into the first stop, opens with a
/// backstab off that belief, and the leg out must arm again -- the
/// board never says that the attack broke the sneak, so the stop has
/// to. Mutation target: keep the belief through the stop's sends and
/// the second `sneak` never goes out.
#[tokio::test]
async fn a_stop_that_swings_rearms_the_next_leg() {
    let (addr, received) = scripted_board(vec![
        ("stat", NINJA_SHEET.into()),
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        ("sneak", "\r\nsneak\r\nAttempting to sneak...\r\n[HP=30/MA=0]:".into()),
        // The first stop, with work listed on arrival: the opener the
        // sneak bought is a backstab.
        (
            "n",
            format!("\r\nn\r\nSneaking...{}", room_block("Inner Ward", Some("giant rat"), "north south")),
        ),
        (
            "bs rat",
            "\r\nbs rat\r\nYou backstab giant rat for 20 damage!\r\nThe giant rat falls to the ground with a tortured squeak.\r\nYou gain 25 experience.\r\n*Combat Off*\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("sneak", "\r\nsneak\r\nAttempting to sneak...\r\n[HP=30/MA=0]:".into()),
        ("n", format!("\r\nn\r\nSneaking...{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/2".into(), "1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
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
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the lap must finish: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert!(stats.kills >= 1, "the rat should have died: {stats:?}");

    let log = received.lock().unwrap();
    let swing = log.iter().position(|l| l == "bs rat").expect("opened with a backstab: {log:?}");
    let sneaks: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, l)| *l == "sneak")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(sneaks.len(), 2, "armed once in, once out: {log:?}");
    assert!(sneaks[0] < swing && swing < sneaks[1], "the swing spent the first arm: {log:?}");
}

/// A key on the floor of a stop is fetched and the pack re-read, with
/// the coin sweep off. The table is handed to the session by the test
/// the way `mmc play` hands it over at realm entry, since the farm's
/// own load of `re/` is not available here.
#[tokio::test]
async fn a_key_at_a_stop_is_picked_up_and_the_pack_reread() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south"))),
        // The key rides the arrival block itself: with no monster there
        // and no dwell configured, the stop is quiet the instant it is
        // seeded, so a look asked only after that would never go out.
        // The pickup has to be driven off the same block the sighting
        // bot is seeded from, and the `get` goes out ahead of any look.
        (
            "n",
            format!("\r\nn{}", room_block_items("Keep", &["black star key"], "south")),
        ),
        (
            "get black star key",
            "\r\nget black star key\r\nYou took black star key.\r\n[HP=30/MA=0]:".into(),
        ),
        (
            "i",
            "\r\ni\r\nYou are carrying nothing.\r\nYou have the following keys:  black star key.\r\nWealth: 0 copper farthings\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Keep", None, "south"))),
    ])
    .await;
    let session = session_for(addr).await;
    mud_client::farm::probe_sheet(&session, None).await;

    let mut content = mud_core::content::Content::default();
    content.add_item(mud_core::content::Item {
        id: mud_core::content::ItemId(172),
        name: "black star key".into(),
        item_type: 7,
        ..Default::default()
    });
    session.set_content(Arc::new(content));

    let graph = corridor();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        travel_interrupts: 0,
        ..FarmConfig::default()
    };
    let bot = mud_client::bot::BotConfig {
        max_hp: 30,
        ..Default::default()
    };
    let plan = mud_client::farm::FarmPlan::build(&cfg, &graph).unwrap();
    let (end, _) = tokio::time::timeout(
        Duration::from_secs(20),
        mud_client::farm::run_farm(&session, graph, &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("the run should finish")
    .expect("the run should succeed");
    assert_eq!(end, mud_client::farm::FarmEnd::LoopsDone);

    let lines = received.lock().unwrap().clone();
    let get = lines.iter().position(|l| l == "get black star key");
    let reread = lines.iter().rposition(|l| l == "i");
    assert!(get.is_some(), "the key was never asked for: {lines:?}");
    assert!(
        reread.is_some_and(|r| r > get.unwrap()),
        "the pack was not re-read after the pickup: {lines:?}"
    );
    assert!(
        session.capabilities().has_item(mud_core::content::ItemId(172)),
        "the re-read did not reach the pack"
    );
}

const BANK: RoomId = RoomId { map: 1, room: 4 };

/// The corridor with a bank east of Inner Ward.
fn corridor_with_bank() -> Arc<RoomGraph> {
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
        requirement: ExitRequirement::None,
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
        requirement: ExitRequirement::None,
    });
    midway.exits[Direction::South as usize] = Some(ExitEdge {
        dest: START,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    midway.exits[Direction::East as usize] = Some(ExitEdge {
        dest: BANK,
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
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut bank = GraphRoom {
        name: "Bank of Godfrey".into(),
        exits: Default::default(),
        light: 0,
        shop: 8,
        ..Default::default()
    };
    bank.exits[Direction::West as usize] = Some(ExitEdge {
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    Arc::new(RoomGraph::from_rooms(vec![
        (START, start),
        (MIDWAY, midway),
        (STOP, stop),
        (BANK, bank),
    ]))
}

/// The world the bank search reads: the bank room is shop-active and
/// its shop is type 7.
fn content_with_bank() -> mud_core::content::Content {
    use mud_core::content::{Content, Room, Shop, ShopId, ShopStock};
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

/// A pile at the stop that lifts the weight class: the stop sweeps it,
/// the runner reads the purse, the gate sees Light become Medium, and
/// the run walks to the bank, reads the purse again, deposits, and
/// reads it once more. The lap ends from the bank.
///
/// The crossing is measured against the reading the run took at its
/// own start, so this only trips when that reading was taken. The
/// count gate is off here for the same reason: it would trip on the
/// purse alone and prove nothing about the run-start reading.
#[tokio::test]
async fn a_pile_that_lifts_the_weight_class_sends_the_run_to_the_bank() {
    let carrying_3900 = "\r\ni\r\nYou are carrying 3900 copper farthings\r\nYou have no keys.\r\nWealth: 3900 copper farthings\r\nEncumbrance: 1800/2400 - Medium [75%]\r\n[HP=30/MA=0]:";
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        // The gate seeds itself from a fresh `i` before the first leg:
        // the startup probe sends `inventory`, which the contents
        // tracker never sees, so the run reads the purse itself. The
        // character is already carrying coins, and is already Light.
        (
            "i",
            "\r\ni\r\nYou are carrying 900 copper farthings\r\nYou have no keys.\r\nWealth: 900 copper farthings\r\nEncumbrance: 800/2400 - Light [33%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south east"))),
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_items("Keep", &["3000 copper farthings"], "south")
            ),
        ),
        (
            "get copper",
            "\r\nget copper\r\nYou picked up 3000 copper farthings\r\n[HP=30/MA=0]:".into(),
        ),
        // No re-look scripted: the arrival block already proved the
        // stop holds no work, so the pump ends on the sweep. The board
        // answers strictly in order, so an entry nothing asks for would
        // wall off everything after it.
        // The stop is over. The runner reads the purse and the gate
        // sees the class it seeded with, Light, become Medium.
        ("i", carrying_3900.into()),
        ("s", format!("\r\ns{}", room_block("Inner Ward", None, "north south east"))),
        ("e", format!("\r\ne{}", room_block("Bank of Godfrey", None, "west"))),
        // At the bank: read, deposit, read again.
        ("i", carrying_3900.into()),
        (
            "deposit 3900",
            "\r\ndeposit 3900\r\nYou deposit 3900 copper farthings.\r\n[HP=30/MA=0]:".into(),
        ),
        (
            "i",
            "\r\ni\r\nYou are carrying nothing.\r\nYou have no keys.\r\nWealth: 0 copper farthings\r\nEncumbrance: 500/2400 - None [20%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
    ])
    .await;
    let session = session_with_bank(
        addr,
        mud_client::bank::BankConfig {
            deposit_at_coins: 0,
            ..Default::default()
        },
    )
    .await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(content_with_bank()));

    let graph = corridor_with_bank();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        travel_interrupts: 0,
        // No world database: the errand reads the table the test
        // handed the session, and this says so out loud.
        content: std::path::PathBuf::from("no-such-content.sqlite"),
        ..FarmConfig::default()
    };
    let bot = BotConfig {
        auto_get: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the run must survive the errand: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert_eq!(stats.coin_pickups, 1, "{stats:?}");
    assert_eq!(stats.deposits, 1, "{stats:?}");
    assert_eq!(stats.deposited_farthings, 3900, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let get = log.iter().position(|l| l == "get copper").expect("the pile was swept");
    let deposit = log
        .iter()
        .position(|l| l == "deposit 3900")
        .unwrap_or_else(|| panic!("no deposit went out: {log:?}"));
    assert!(get < deposit, "{log:?}");
    let reads: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, l)| *l == "i")
        .map(|(i, _)| i)
        .collect();
    assert!(
        reads.iter().any(|&r| get < r && r < deposit),
        "the purse must be read between the pickup and the deposit: {log:?}"
    );
    assert!(
        reads.iter().any(|&r| r > deposit),
        "the purse must be re-read after the deposit: {log:?}"
    );
    let east = log.iter().position(|l| l == "e").expect("the walk to the bank");
    assert!(east < deposit, "deposited before arriving: {log:?}");
}

/// The board refuses the deposit. The errand ends without banking
/// anything, and the gate goes off for the rest of the run: the second
/// lap sweeps another pile over the same mark and stays where it is.
#[tokio::test]
async fn a_refused_deposit_switches_deposits_off_for_the_run() {
    let carrying_1200 = "\r\ni\r\nYou are carrying 1200 copper farthings\r\nYou have no keys.\r\nWealth: 1200 copper farthings\r\nEncumbrance: 400/2400 - None [16%]\r\n[HP=30/MA=0]:";
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        // The gate seeds itself from a fresh `i` before the first leg:
        // the startup probe sends `inventory`, which the contents
        // tracker never sees, so the run reads the purse itself.
        (
            "i",
            "\r\ni\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south east"))),
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_items("Keep", &["1200 copper farthings"], "south")
            ),
        ),
        (
            "get copper",
            "\r\nget copper\r\nYou picked up 1200 copper farthings\r\n[HP=30/MA=0]:".into(),
        ),
        ("i", carrying_1200.into()),
        ("s", format!("\r\ns{}", room_block("Inner Ward", None, "north south east"))),
        ("e", format!("\r\ne{}", room_block("Bank of Godfrey", None, "west"))),
        ("i", carrying_1200.into()),
        // The refusal. Nothing leaves the purse.
        (
            "deposit 1200",
            "\r\ndeposit 1200\r\nPlease specify a more reasonable amount.\r\n[HP=30/MA=0]:".into(),
        ),
        ("i", carrying_1200.into()),
        // The second lap walks back from the bank and sweeps another
        // pile. The purse is still over the mark, so only the switch
        // keeps the character out of the bank.
        ("w", format!("\r\nw{}", room_block("Inner Ward", None, "north south east"))),
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_items("Keep", &["1200 copper farthings"], "south")
            ),
        ),
        (
            "get copper",
            "\r\nget copper\r\nYou picked up 1200 copper farthings\r\n[HP=30/MA=0]:".into(),
        ),
        // Scripted for a second judgement that must never happen. The
        // board is answered in order and nothing follows this, so an
        // entry the runner never asks for is the proof the gate is off.
        ("i", carrying_1200.into()),
    ])
    .await;
    let session = session_with_bank(addr, mud_client::bank::BankConfig::default()).await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(content_with_bank()));

    let graph = corridor_with_bank();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 2,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        travel_interrupts: 0,
        content: std::path::PathBuf::from("no-such-content.sqlite"),
        ..FarmConfig::default()
    };
    let bot = BotConfig {
        auto_get: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "a refused deposit must not end the run: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert_eq!(stats.deposits, 0, "{stats:?}");
    assert_eq!(stats.deposited_farthings, 0, "{stats:?}");
    assert_eq!(stats.coin_pickups, 2, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let deposits: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, l)| *l == "deposit 1200")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(deposits.len(), 1, "the deposit was retried: {log:?}");
    assert!(
        !log[deposits[0]..].contains(&"s".to_string()),
        "the errand ran a second time: {log:?}"
    );
}

/// The corridor with a bank room nothing leads into: the bank has its
/// way out and no way in, so a route to it does not exist.
fn corridor_with_a_stranded_bank() -> Arc<RoomGraph> {
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
        requirement: ExitRequirement::None,
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
        requirement: ExitRequirement::None,
    });
    midway.exits[Direction::South as usize] = Some(ExitEdge {
        dest: START,
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
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut bank = GraphRoom {
        name: "Bank of Godfrey".into(),
        exits: Default::default(),
        light: 0,
        shop: 8,
        ..Default::default()
    };
    bank.exits[Direction::West as usize] = Some(ExitEdge {
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    Arc::new(RoomGraph::from_rooms(vec![
        (START, start),
        (MIDWAY, midway),
        (STOP, stop),
        (BANK, bank),
    ]))
}

/// `[bank].at` names a real bank the walk cannot route to. The gate
/// trips, the errand chooses the configured room, the walk fails with a
/// navigation error, and that ends the errand rather than the run:
/// deposits go off, the lap finishes, and nothing else is sent.
#[tokio::test]
async fn an_unreachable_bank_switches_deposits_off_without_ending_the_run() {
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        (
            "i",
            "\r\ni\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south"))),
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_items("Keep", &["1200 copper farthings"], "south")
            ),
        ),
        (
            "get copper",
            "\r\nget copper\r\nYou picked up 1200 copper farthings\r\n[HP=30/MA=0]:".into(),
        ),
        // The gate's read. The errand starts here and gets no further:
        // the board is answered in order and nothing follows, so any
        // command after this one would be an echo the runner cannot use.
        (
            "i",
            "\r\ni\r\nYou are carrying 1200 copper farthings\r\nYou have no keys.\r\nWealth: 1200 copper farthings\r\nEncumbrance: 400/2400 - None [16%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
    ])
    .await;
    let session = session_with_bank(
        addr,
        mud_client::bank::BankConfig {
            at: Some("1/4".into()),
            ..Default::default()
        },
    )
    .await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(content_with_bank()));

    let graph = corridor_with_a_stranded_bank();
    let cfg = FarmConfig {
        start: "1/1".into(),
        circuit: vec!["1/3".into()],
        loops: 1,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        travel_interrupts: 0,
        content: std::path::PathBuf::from("no-such-content.sqlite"),
        ..FarmConfig::default()
    };
    let bot = BotConfig {
        auto_get: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    let plan = FarmPlan::build(&cfg, &graph).expect("plan");
    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(30),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "a bank no route reaches must not end the run: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    assert_eq!(end, FarmEnd::LoopsDone, "{stats:?}");
    assert_eq!(stats.coin_pickups, 1, "{stats:?}");
    assert_eq!(stats.deposits, 0, "{stats:?}");

    let log = received.lock().unwrap().clone();
    assert!(
        !log.iter().any(|l| l.starts_with("deposit")),
        "a deposit went out from outside the bank: {log:?}"
    );
    let gate = log
        .iter()
        .rposition(|l| l == "i")
        .expect("the gate read the purse");
    let walked = ["n", "s", "e", "w"];
    assert!(
        !log[gate + 1..].iter().any(|l| walked.contains(&l.as_str())),
        "the errand walked somewhere with no route: {log:?}"
    );
}

/// The corridor with the bank a level below Inner Ward. Down and up
/// leave the plane a roam is confined to, so the bank is outside every
/// roam of this corridor and there is no fenced way back from it: the
/// errand has to walk both ways itself.
fn corridor_with_a_bank_below() -> Arc<RoomGraph> {
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
        requirement: ExitRequirement::None,
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
        requirement: ExitRequirement::None,
    });
    midway.exits[Direction::South as usize] = Some(ExitEdge {
        dest: START,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    midway.exits[Direction::Down as usize] = Some(ExitEdge {
        dest: BANK,
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
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    let mut bank = GraphRoom {
        name: "Bank of Godfrey".into(),
        exits: Default::default(),
        light: 0,
        shop: 8,
        ..Default::default()
    };
    bank.exits[Direction::Up as usize] = Some(ExitEdge {
        dest: MIDWAY,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    });
    Arc::new(RoomGraph::from_rooms(vec![
        (START, start),
        (MIDWAY, midway),
        (STOP, stop),
        (BANK, bank),
    ]))
}

/// A roam whose fence leaves the bank outside it. The character still
/// has to put its coin down, so the errand crosses the fence, and it
/// walks back into the region afterwards: the roam picks its next room
/// from inside the fence, and from the bank there is no room inside the
/// fence it can reach at all.
#[tokio::test]
async fn a_roam_leaves_its_fence_to_bank_and_walks_back() {
    use mud_client::roam::Walls;

    let carrying_1200 = "\r\ni\r\nYou are carrying 1200 copper farthings\r\nYou have no keys.\r\nWealth: 1200 copper farthings\r\nEncumbrance: 400/2400 - None [16%]\r\n[HP=30/MA=0]:";
    let (addr, received) = scripted_board(vec![
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        (
            "i",
            "\r\ni\r\nYou are carrying nothing.\r\nYou have no keys.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        // The roam works Inner Ward first and finds the pile there.
        (
            "n",
            format!(
                "\r\nn{}",
                room_block_items("Inner Ward", &["1200 copper farthings"], "north south down")
            ),
        ),
        (
            "get copper",
            "\r\nget copper\r\nYou picked up 1200 copper farthings\r\n[HP=30/MA=0]:".into(),
        ),
        ("i", carrying_1200.into()),
        // Down leaves the plane the roam is fenced to, so only an
        // unfenced walk gets here.
        ("d", format!("\r\nd{}", room_block("Bank of Godfrey", None, "up"))),
        ("i", carrying_1200.into()),
        (
            "deposit 1200",
            "\r\ndeposit 1200\r\nYou deposit 1200 copper farthings.\r\n[HP=30/MA=0]:".into(),
        ),
        (
            "i",
            "\r\ni\r\nYou are carrying nothing.\r\nYou have no keys.\r\nWealth: 0 copper farthings\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        // Back inside the fence, to the room the errand left. Nothing
        // else can walk this: the rotation cannot see a region room
        // from the bank at all.
        ("u", format!("\r\nu{}", room_block("Inner Ward", None, "north south down"))),
        // And on with the roam: Guard Post is the next region room.
        ("s", format!("\r\ns{}", room_block("Guard Post", None, "north"))),
    ])
    .await;
    let session = session_with_bank(addr, mud_client::bank::BankConfig::default()).await;
    mud_client::farm::probe_sheet(&session, None).await;
    session.set_content(Arc::new(content_with_bank()));

    let graph = corridor_with_a_bank_below();
    let cfg = FarmConfig {
        loops: 0,
        max_seconds: 8,
        idle_poke_ms: 500,
        depart_at_percent: Some(0),
        travel_interrupts: 0,
        stop_seconds: 2,
        content: std::path::PathBuf::from("no-such-content.sqlite"),
        ..FarmConfig::default()
    };
    let bot = BotConfig {
        auto_get: true,
        max_hp: 30,
        ..BotConfig::default()
    };
    // Keep is walled off, so the region is Guard Post and Inner Ward
    // and the roam simply alternates between them once the errand is
    // done. The bank is outside the region whether it is walled or not.
    let plan = FarmPlan::roaming(START, Walls::new([BANK, STOP]), &graph)
        .expect("a roam of two rooms");
    let (end, stats) = match tokio::time::timeout(
        Duration::from_secs(40),
        run_farm(&session, graph.clone(), &plan, &bot, &cfg, None, &quiet()),
    )
    .await
    .expect("run_farm should finish, not hang")
    {
        Ok(out) => out,
        Err(e) => panic!(
            "the roam must survive the errand: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    assert_eq!(end, FarmEnd::TimeUp, "a roam ends on the clock: {stats:?}");
    assert_eq!(stats.deposits, 1, "{stats:?}");
    assert_eq!(stats.deposited_farthings, 1200, "{stats:?}");

    let log = received.lock().unwrap().clone();
    let down = log
        .iter()
        .position(|l| l == "d")
        .unwrap_or_else(|| panic!("the errand never left the fence: {log:?}"));
    let deposit = log
        .iter()
        .position(|l| l == "deposit 1200")
        .unwrap_or_else(|| panic!("no deposit went out: {log:?}"));
    let back = log
        .iter()
        .position(|l| l == "u")
        .unwrap_or_else(|| panic!("the roam stayed at the bank: {log:?}"));
    assert!(down < deposit && deposit < back, "{log:?}");
    assert!(
        !log[back..].iter().any(|l| l == "d"),
        "the roam went back out of the fence: {log:?}"
    );
    assert_eq!(
        log[back + 1..].iter().find(|l| l == &"n" || l == &"s"),
        Some(&"s".to_string()),
        "the room after the walk back must be a region room: {log:?}"
    );
}

