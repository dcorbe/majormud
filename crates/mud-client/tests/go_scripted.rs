//! Walk versus run, against a scripted echoing board.
//!
//! The harness is the `farm_scripted.rs` / `nav_echo.rs` shape — test
//! crates do not share modules, so it is duplicated here, which is this
//! suite's existing pattern.
//!
//! This is the only place the feature's central promise is actually
//! exercised. `tests/go.rs` pins that `go_config` sets the right
//! boolean; what that boolean *does* comes from an interaction between
//! two files: `nav.rs` classifies a board combat-refusal as
//! `Interrupt::Attacked` unconditionally, and `farm::travel`'s
//! `Attacked` branch is not gated on `fight_while_travelling` — only its
//! sighting branch is. Nobody would notice either of those changing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::bot::BotConfig;
use mud_client::farm::{FarmConfig, FarmError, probe_sheet};
use mud_client::live::{Derive, Live};
use mud_client::go::{GoEnd, go_config, run_go};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Direction, RoomId};

/// A notices sink that keeps nothing. What a runner says at startup is
/// not what these tests are about.
fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

const START: RoomId = RoomId { map: 1, room: 1 };
const MIDWAY: RoomId = RoomId { map: 1, room: 2 };
const STOP: RoomId = RoomId { map: 1, room: 3 };

fn room_block(name: &str, also_here: Option<&str>, exits: &str) -> String {
    let also = match also_here {
        Some(names) => format!("Also here: {names}.\r\n"),
        None => String::new(),
    };
    format!("\r\n\x1b[1;36m{name}\r\n{also}Obvious exits: {exits}\r\n[HP=30/MA=0]:")
}

/// Guard Post -> Inner Ward -> Keep, one straight corridor.
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
/// used once, strictly in order; a re-ask replays the most recently
/// consumed matching entry. Unmatched lines echo and say back.
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
        bank: Default::default(),
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

fn bot() -> BotConfig {
    BotConfig {
        auto_combat: true,
        max_hp: 30,
        ..BotConfig::default()
    }
}

/// No room database behind the tiny corridor, which `run_go` must treat
/// as "no threat opinion" rather than as fatal.
fn cfg(walking: bool) -> FarmConfig {
    go_config(&FarmConfig {
        content: std::path::PathBuf::from("/nonexistent/rooms.sqlite"),
        fight_while_travelling: walking,
        ..Default::default()
    })
}

async fn walk(walking: bool, script: Vec<(&'static str, String)>) -> (GoEnd, Vec<String>) {
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    let graph = corridor();
    let end = match tokio::time::timeout(
        Duration::from_secs(30),
        run_go(&session, graph, Some(START), &[STOP], Live::fixed(bot(), cfg(walking)), None, &quiet()),
    )
    .await
    .expect("run_go should finish, not hang")
    {
        Ok(end) => end,
        Err(e) => panic!(
            "the walk must survive: {e:?}\nboard received: {:?}",
            received.lock().unwrap()
        ),
    };
    let log = received.lock().unwrap().clone();
    (end, log)
}

/// run_go's opening "where am I", and a first step that lands beside a
/// rat which swings and misses. Where the two modes diverge.
fn up_to_the_whiff() -> Vec<(&'static str, String)> {
    vec![
        (
            "look",
            format!("\r\nlook{}", room_block("Guard Post", None, "north")),
        ),
        (
            "n",
            format!(
                "\r\nn\r\nThe giant rat swings at you but misses!\r\n{}",
                room_block("Inner Ward", Some("giant rat"), "north south")
            ),
        ),
    ]
}

/// Run mode's whole point: something took a swing, and the walk did not
/// care. A whiff is not a refusal, so nothing stops.
///
/// The script ends one step after the whiff because that is the claim —
/// the board is scripted strictly in order, so a run that paused to look
/// or swing would find no reply waiting and desync. The absence of those
/// entries IS the assertion; the log check below only says so out loud.
#[tokio::test]
async fn run_mode_walks_past_a_fight() {
    let mut script = up_to_the_whiff();
    script.push(("n", format!("\r\nn{}", room_block("Keep", None, "south"))));
    let (end, log) = walk(false, script).await;
    assert_eq!(end, GoEnd::Arrived(STOP), "log: {log:?}");
    assert!(
        !log.iter().any(|l| l.starts_with("a ")),
        "run mode must not attack anything: {log:?}"
    );
    assert!(
        !log.iter().skip(1).any(|l| l == "look"),
        "run mode must not stop to look either: {log:?}"
    );
}

/// Walk mode, same whiff: the sighting stops the leg, the room is
/// cleared, and only then does the walk resume.
#[tokio::test]
async fn walk_mode_stops_for_the_fight_then_resumes() {
    let mut script = up_to_the_whiff();
    script.extend([
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
        ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
    ]);
    let (end, log) = walk(true, script).await;
    assert_eq!(end, GoEnd::Arrived(STOP), "log: {log:?}");
    let first_n = log.iter().position(|l| l == "n").expect("first step");
    let attack = log
        .iter()
        .position(|l| l == "a rat")
        .unwrap_or_else(|| panic!("walk mode must fight: {log:?}"));
    let last_n = log.iter().rposition(|l| l == "n").expect("the walk resumed");
    assert!(
        first_n < attack && attack < last_n,
        "the fight belongs between the steps: {log:?}"
    );
}

/// The one that matters. The board refuses movement outright while in
/// combat, so "run" cannot mean "never fight" — a walk that would not
/// fight is a walk that stays stuck wherever something picked a fight.
/// Run mode must therefore still fight its way out of a refusal, even
/// though it ignored the whiff that started it.
#[tokio::test]
async fn run_mode_still_fights_out_of_a_combat_lock() {
    let (end, log) = walk(
        false,
        vec![
            (
                "look",
                format!("\r\nlook{}", room_block("Guard Post", None, "north")),
            ),
            (
                "n",
                format!(
                    "\r\nn\r\nThe giant rat swings at you but misses!\r\n{}",
                    room_block("Inner Ward", Some("giant rat"), "north south")
                ),
            ),
            // The lock: the board simply will not let us leave.
            (
                "n",
                "\r\nn\r\nYou may not enter that room while in combat.\r\n[HP=30/MA=0]:".into(),
            ),
            // The defence that the refusal forces, even in run mode.
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
            ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
        ],
    )
    .await;

    assert_eq!(
        end,
        GoEnd::Arrived(STOP),
        "a locked walk must fight free and finish: {log:?}"
    );
    assert!(
        log.iter().any(|l| l == "a rat"),
        "the refusal must be answered with a fight, not a retry: {log:?}"
    );
}

/// The point of Task 4: the session's inventory and spellbook are read
/// once, not per walk. `run_go` used to pay a three-second spell
/// collection plus an inventory round trip on every call (`read_sheet`,
/// via `leg_needs_light`); now it reads `Session::raw_sheet`, filled
/// once by `probe_sheet` at realm entry, so a second `/go` over the same
/// session must not touch the board for either again.
#[tokio::test]
async fn a_second_go_in_one_session_sends_no_spells() {
    let (addr, received) = scripted_board(vec![
        // The realm-entry probe. ONE inventory, ONE spells -- never
        // repeated no matter how many walks follow.
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying a torch.\r\nEncumbrance: 1/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .into(),
        ),
        (
            "spells",
            "\r\nspells\r\nYou don't know any spells.\r\n[HP=30/MA=0]:".into(),
        ),
        // First /go: Guard Post -> Keep.
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south"))),
        ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
        // Second /go, same session: Keep -> Guard Post.
        ("look", format!("\r\nlook{}", room_block("Keep", None, "south"))),
        ("s", format!("\r\ns{}", room_block("Inner Ward", None, "north south"))),
        ("s", format!("\r\ns{}", room_block("Guard Post", None, "north"))),
    ])
    .await;
    let session = session_for(addr).await;
    let graph = corridor();

    // What tui::on_realm_entry does in the background on a real connection.
    probe_sheet(&session, None).await;

    let first = tokio::time::timeout(
        Duration::from_secs(10),
        run_go(&session, graph.clone(), Some(START), &[STOP], Live::fixed(bot(), cfg(false)), None, &quiet()),
    )
    .await
    .expect("first /go should not hang")
    .expect("first /go should arrive");
    assert_eq!(first, GoEnd::Arrived(STOP));

    let second = tokio::time::timeout(
        Duration::from_secs(10),
        run_go(&session, graph, Some(STOP), &[START], Live::fixed(bot(), cfg(false)), None, &quiet()),
    )
    .await
    .expect("second /go should not hang")
    .expect("second /go should arrive");
    assert_eq!(second, GoEnd::Arrived(START));

    let log = received.lock().unwrap();
    assert_eq!(
        log.iter().filter(|l| **l == "inventory").count(),
        1,
        "inventory must be asked once, at the probe -- not again by either /go: {log:?}"
    );
    assert_eq!(
        log.iter().filter(|l| **l == "spells").count(),
        1,
        "spells must be asked once, at the probe -- not again by either /go: {log:?}"
    );
}

/// `check_departure_mark` runs first in `run_go`, same as it does in
/// `run_farm`: a walk that will set off already interrupted is caught
/// before the opening look goes out, not after.
#[tokio::test]
async fn a_walk_refuses_an_interrupt_mark_above_the_bots_mark_before_sending_anything() {
    let (addr, received) = scripted_board(vec![]).await;
    let session = session_for(addr).await;
    let graph = corridor();
    let cfg = go_config(&FarmConfig {
        interrupt_at_percent: 96,
        ..Default::default()
    });
    let bot = BotConfig {
        auto_combat: true,
        max_hp: 30,
        rest_at_percent: 60,
        ..BotConfig::default()
    };

    let out = tokio::time::timeout(
        Duration::from_secs(10),
        run_go(&session, graph, Some(START), &[STOP], Live::fixed(bot, cfg), None, &quiet()),
    )
    .await
    .expect("run_go should refuse at once, not hang");

    let why = match out {
        Err(FarmError::Config(why)) => why,
        other => panic!("the pair must be refused: {other:?}"),
    };
    assert!(why.contains("96"), "{why}");
    assert!(why.contains("60"), "{why}");

    let log = received.lock().unwrap();
    assert!(
        log.is_empty(),
        "the refusal must come before anything is sent: {log:?}"
    );
}

/// A go's target is an argument, not a setting. A profile change mid
/// walk changes the walk's settings and nothing about where it goes.
#[tokio::test]
async fn a_profile_change_mid_walk_leaves_the_target_alone() {
    let mut script = up_to_the_whiff();
    script.push(("n", format!("\r\nn{}", room_block("Keep", None, "south"))));
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let derive: Derive = Arc::new(|p: &Profile| {
        let base = FarmConfig { fight_while_travelling: false, ..p.farm.clone().unwrap_or_default() };
        (
            BotConfig { auto_combat: false, ..p.bot.clone().unwrap_or_default() },
            go_config(&base),
        )
    });
    let live = Live::over(rx, "go", quiet(), bot(), cfg(false), derive);
    let log_for_change = Arc::clone(&received);
    tokio::spawn(async move {
        loop {
            if log_for_change.lock().unwrap().iter().any(|l| l == "n") {
                let _ = tx.send(Profile {
                    farm: Some(FarmConfig { start: "9/9".into(), finish_at: Some("9/9".into()), ..Default::default() }),
                    ..Profile::default()
                });
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let end = tokio::time::timeout(
        Duration::from_secs(30),
        run_go(&session, corridor(), Some(START), &[STOP], live, None, &quiet()),
    )
    .await
    .expect("run_go should finish, not hang")
    .unwrap();
    assert_eq!(end, GoEnd::Arrived(STOP), "log: {:?}", received.lock().unwrap());
}

// --- stealth on the go walk -------------------------------------------

/// The same block with a mana reading on the prompt. The corridor's
/// ordinary block says MA=0, and a character with no mana casts
/// nothing at all.
fn room_block_mana(name: &str, exits: &str, mana: i32) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP=30/MA={mana}]:")
}

/// The sheet a sneaking walk opens with. Stealth is what lets the
/// walker sneak at all, through `Session::capabilities`.
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
[HP=30/MA=20]:";

/// One shipped `camouflage` record, with only the fields discovery
/// reads set to anything. The same shape `tests/sheet.rs` builds,
/// duplicated here because test crates do not share modules.
fn camouflage() -> mud_core::content::Spell {
    use mud_core::ability::Ability;
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, Spell, SpellId, TargetMode};
    Spell {
        id: SpellId(1314),
        name: "camouflage".into(),
        short_name: "camo".into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![(Ability::Stealth, 0)],
        level_cap: 0,
        round_cost: 0,
        required_power: 0,
        min_base: 0,
        max_base: 0,
        target_mode: TargetMode::Benign,
        save_class: SaveClass::None,
        base_chance: 0,
        duration_per_level: 0,
        match_type: MatchType::Single1,
        duration: 30,
        element: Element::Magic,
        class_gate_group: 0,
        mana_cost: 10,
        max_increase: ScalePair::NONE,
        required_class_level: 0,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

/// The wiring, end to end: `run_go` builds its navigator with the
/// stealth spells the session's own book and spell map say the
/// character knows, and the walk casts one before it sneaks.
///
/// Nothing else in the suite would notice `.with_stealth(...)` being
/// dropped from `run_go`, because every other stealth test hands the
/// navigator its buffs directly.
///
/// Mutation target: drop `.with_stealth(...)` from `run_go` and no
/// `cast camo` goes out.
#[tokio::test]
async fn a_go_walk_casts_the_stealth_spell_it_discovered() {
    let (addr, received) = scripted_board(vec![
        ("stat", NINJA_SHEET.into()),
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=20]:"
                .into(),
        ),
        (
            "spells",
            "\r\nspells\r\nYou have the following spells:\r\nLevel Mana Short Spell Name\r\n\x20 8  10    camo  camouflage                    \r\n[HP=30/MA=20]:"
                .into(),
        ),
        ("look", format!("\r\nlook{}", room_block_mana("Guard Post", "north", 20))),
        (
            "cast camo",
            "\r\ncast camo\r\nYou cast camouflage!\r\n[HP=30/MA=10]:".into(),
        ),
        ("sneak", "\r\nsneak\r\nAttempting to sneak...\r\n[HP=30/MA=10]:".into()),
        (
            "n",
            format!("\r\nn\r\nSneaking...{}", room_block_mana("Inner Ward", "north south", 10)),
        ),
        (
            "n",
            format!("\r\nn\r\nSneaking...{}", room_block_mana("Keep", "south", 10)),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    probe_sheet(&session, None).await;
    assert_eq!(session.capabilities().stealth, 56, "the sheet must have been read");
    let mut content = mud_core::content::Content::default();
    content.add_spell(camouflage());
    session.set_content(Arc::new(content));

    let graph = corridor();
    // The board is scripted strictly in order, so a walk that skipped
    // the cast would desync and stall rather than arrive. Say what it
    // did send, so that failure names the missing line.
    let walk = tokio::time::timeout(
        Duration::from_secs(20),
        run_go(&session, graph, Some(START), &[STOP], Live::fixed(bot(), cfg(false)), None, &quiet()),
    )
    .await;
    let end = match walk {
        Ok(r) => r.unwrap_or_else(|e| panic!("{e:?}\nboard received: {:?}", received.lock().unwrap())),
        Err(_) => panic!("the walk stalled\nboard received: {:?}", received.lock().unwrap()),
    };
    assert_eq!(end, GoEnd::Arrived(STOP));

    let log = received.lock().unwrap();
    let walked: Vec<&String> = log
        .iter()
        .filter(|l| *l == "sneak" || *l == "n" || l.starts_with("cast "))
        .collect();
    assert_eq!(
        walked,
        vec!["cast camo", "sneak", "n", "n"],
        "the discovered spell is cast before the sneak: {log:?}"
    );
}


// --- the bot's switches on the walk ------------------------------------

/// The same block at a chosen HP.
fn room_block_hp(name: &str, exits: &str, hp: i32) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n[HP={hp}/MA=0]:")
}

/// `auto_rest` off means the walk neither rests nor waits on health: a
/// character at a third of its HP sets off at once, and the travel
/// guard's hurt mark does not stop it on the way. The board is scripted
/// strictly, so a rest or a stop would leave the walk waiting on a
/// reply that never comes.
#[tokio::test]
async fn a_walk_with_resting_off_neither_rests_nor_stops_for_the_hurt_mark() {
    let script = vec![
        ("look", format!("\r\nlook{}", room_block_hp("Guard Post", "north", 10))),
        ("n", format!("\r\nn{}", room_block_hp("Inner Ward", "north south", 10))),
        ("n", format!("\r\nn{}", room_block_hp("Keep", "south", 10))),
    ];
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    let bot = BotConfig { auto_rest: false, ..bot() };
    let cfg = FarmConfig { interrupt_at_percent: 50, max_rest_seconds: 2, ..cfg(true) };
    let end = tokio::time::timeout(
        Duration::from_secs(10),
        run_go(&session, corridor(), Some(START), &[STOP], Live::fixed(bot, cfg), None, &quiet()),
    )
    .await
    .expect("run_go should finish, not hang")
    .unwrap();
    let log = received.lock().unwrap();
    assert_eq!(end, GoEnd::Arrived(STOP), "log: {log:?}");
    assert!(
        !log.iter().any(|l| l == "rest" || l == "meditate"),
        "resting is off: {log:?}"
    );
}

/// `/bot` off is the master switch. The profile says to take the fights
/// on the way, and the walk would, but with the switch off it walks
/// past the whiff like a run.
#[tokio::test]
async fn the_bot_switch_off_walks_past_a_fight_the_profile_would_take() {
    let mut script = up_to_the_whiff();
    script.push(("n", format!("\r\nn{}", room_block("Keep", None, "south"))));
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    let (_profile, rx) = tokio::sync::watch::channel(Profile::default());
    let (_switch, off) = tokio::sync::watch::channel(false);
    let derive: Derive = Arc::new(|p: &Profile| {
        (
            BotConfig { auto_combat: true, max_hp: 30, ..p.bot.clone().unwrap_or_default() },
            cfg(true),
        )
    });
    let live = Live::over(rx, "go", quiet(), bot(), cfg(true), derive).switched(off);
    let end = tokio::time::timeout(
        Duration::from_secs(30),
        run_go(&session, corridor(), Some(START), &[STOP], live, None, &quiet()),
    )
    .await
    .expect("run_go should finish, not hang")
    .unwrap();
    let log = received.lock().unwrap();
    assert_eq!(end, GoEnd::Arrived(STOP), "log: {log:?}");
    assert!(!log.iter().any(|l| l.starts_with("a ")), "nothing was fought: {log:?}");
}

/// Waypoints are walked in order: out to the Keep, then back to the
/// Guard Post, in one `/go`. The final waypoint is the destination.
#[tokio::test]
async fn a_go_with_waypoints_walks_each_leg_in_order() {
    let (addr, received) = scripted_board(vec![
        ("look", format!("\r\nlook{}", room_block("Guard Post", None, "north"))),
        ("n", format!("\r\nn{}", room_block("Inner Ward", None, "north south"))),
        ("n", format!("\r\nn{}", room_block("Keep", None, "south"))),
        ("s", format!("\r\ns{}", room_block("Inner Ward", None, "north south"))),
        ("s", format!("\r\ns{}", room_block("Guard Post", None, "north"))),
    ])
    .await;
    let session = session_for(addr).await;
    probe_sheet(&session, None).await;

    let end = tokio::time::timeout(
        Duration::from_secs(10),
        run_go(
            &session,
            corridor(),
            Some(START),
            &[STOP, START],
            Live::fixed(bot(), cfg(false)),
            None,
            &quiet(),
        ),
    )
    .await
    .expect("the walk should not hang")
    .expect("the walk should arrive");
    assert_eq!(end, GoEnd::Arrived(START));

    let moves: Vec<String> = received
        .lock()
        .unwrap()
        .iter()
        .filter(|l| matches!(l.as_str(), "n" | "s"))
        .cloned()
        .collect();
    assert_eq!(moves, vec!["n", "n", "s", "s"], "{:?}", received.lock().unwrap());
}
