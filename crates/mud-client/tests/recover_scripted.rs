//! `/recover` against a scripted echoing board.
//!
//! The harness is the `go_scripted.rs` shape. Test crates do not share
//! modules, so it is duplicated here, which is this suite's pattern.
//!
//! A corridor of three rooms: Guard Post, the start room and the safe
//! room, Inner Ward, and Keep, where the character died. The board
//! echoes every line, logs it lowercased, and answers from a script
//! consumed strictly in order, each entry once. A re-ask replays the
//! most recently consumed matching entry. Anything unmatched is said
//! aloud.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::bot::BotConfig;
use mud_client::farm::{FarmConfig, FarmError, Live, probe_sheet};
use mud_client::go::go_config;
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_client::profile::Profile;
use mud_client::recover::{HomeWhy, RecoverEnd, run_recover};
use mud_client::session::Session;
use mud_core::content::{Content, Direction, RoomId};

fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

const START: RoomId = RoomId { map: 1, room: 1 };
const MIDWAY: RoomId = RoomId { map: 1, room: 2 };
const DEATH: RoomId = RoomId { map: 1, room: 3 };

const PROMPT: &str = "[HP=30/MA=0]:";

fn block(name: &str, exits: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nObvious exits: {exits}\r\n{PROMPT}")
}

/// A block whose floor lists `items`, as the board renders after a bare
/// `search`, with the prompt `prompt` so a test can show hitpoints
/// falling.
fn block_with(name: &str, items: &str, exits: &str, prompt: &str) -> String {
    format!("\r\n\x1b[1;36m{name}\r\nYou notice {items} here.\r\nObvious exits: {exits}\r\n{prompt}")
}

fn reply(cmd: &str, body: &str) -> String {
    format!("\r\n{cmd}\r\n{body}\r\n{PROMPT}")
}

/// The stat sheet `probe_sheet` reads. Stealth is what lets the job
/// start at all.
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

const EMPTY_HANDED: &str =
    "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:";

fn edge(dest: RoomId) -> Option<ExitEdge> {
    Some(ExitEdge {
        dest,
        exit_type: 0,
        command: None,
        requirement: ExitRequirement::None,
    })
}

/// Guard Post -> Inner Ward -> Keep, north each step, with the way back.
/// `keep_light` is the death room's light, so a test can make it dark.
fn corridor(keep_light: i64) -> Arc<RoomGraph> {
    let mut start = GraphRoom {
        name: "Guard Post".into(),
        ..Default::default()
    };
    start.exits[Direction::North as usize] = edge(MIDWAY);
    let mut midway = GraphRoom {
        name: "Inner Ward".into(),
        ..Default::default()
    };
    midway.exits[Direction::North as usize] = edge(DEATH);
    midway.exits[Direction::South as usize] = edge(START);
    let mut death = GraphRoom {
        name: "Keep".into(),
        light: keep_light,
        ..Default::default()
    };
    death.exits[Direction::South as usize] = edge(MIDWAY);
    Arc::new(RoomGraph::from_rooms(vec![
        (START, start),
        (MIDWAY, midway),
        (DEATH, death),
    ]))
}

/// Splits a script entry's reply in two. The board writes the first
/// half at once and the second [`PAUSE_MS`] later, still reading while
/// it waits. What a board printing the next round's prompt a moment
/// after a refusal looks like.
const PAUSE: &str = "\x00";
/// Written into the board's log where the second half went out, so a
/// test can say what the client sent before it and what after.
const LATER: &str = "-- the second half --";
const PAUSE_MS: u64 = 200;

async fn scripted_board(
    script: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (mut rd, mut wr) = sock.into_split();
        wr.write_all(block("Guard Post", "north").as_bytes())
            .await
            .unwrap();
        // The halves of a paused reply come back through this, so the
        // read loop is never the thing that is asleep.
        let (say, mut said) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut used: Vec<Option<u64>> = vec![None; script.len()];
        let mut clock: u64 = 0;
        let mut pending = String::new();
        let mut buf = [0u8; 512];
        loop {
            let n = tokio::select! {
                read = rd.read(&mut buf) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                },
                Some(late) = said.recv() => {
                    log.lock().unwrap().push(LATER.to_string());
                    wr.write_all(late.as_bytes()).await.unwrap();
                    continue;
                }
            };
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
                        None => format!("\r\n{line}\r\nYou say \"{line}\"\r\n{PROMPT}"),
                    },
                };
                match reply.split_once(PAUSE) {
                    Some((now, later)) => {
                        wr.write_all(now.as_bytes()).await.unwrap();
                        let say = say.clone();
                        let later = later.to_string();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(PAUSE_MS)).await;
                            let _ = say.send(later);
                        });
                    }
                    None => wr.write_all(reply.as_bytes()).await.unwrap(),
                }
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
        username: "Beef".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        // Known maxima, so the job never asks `health` and every
        // percent mark is against 30.
        bot: Some(BotConfig {
            max_hp: 30,
            minor_heal_at_percent: 70,
            ..BotConfig::default()
        }),
        farm: None,
        ..Default::default()
    };
    Session::connect(&profile, None).await.unwrap()
}

/// The tables a recovery runs under, with the job's own forced fields
/// already applied, the way the window derives them. No room database:
/// these tests read no content at all.
fn tables() -> (BotConfig, FarmConfig) {
    let mut farm = go_config(
        &FarmConfig {
            content: std::path::PathBuf::new(),
            ..FarmConfig::default()
        },
        false,
    );
    farm.interrupt_at_percent = 0;
    farm.nav.bash_doors = false;
    // The scripted board answers at once, and this is what every wait
    // in the job is bounded by. The shipped 15 seconds would be spent
    // in real time by every test that makes the board say nothing.
    farm.nav.step_timeout_ms = 2_000;
    let bot = BotConfig {
        max_hp: 30,
        minor_heal_at_percent: 70,
        auto_combat: false,
        auto_flee: false,
        auto_get: false,
        ..BotConfig::default()
    };
    (bot, farm)
}

/// Settings that never change, which is every test but the one about a
/// change.
fn settings() -> Live {
    let (bot, farm) = tables();
    Live::fixed(bot, farm)
}

/// The same tables over a channel the test holds the sender of, with a
/// derive that reads the heal mark off the profile and nothing else.
/// What `/set bot.minor_heal_at_percent` looks like from inside a
/// running job.
fn live_settings(rx: tokio::sync::watch::Receiver<Profile>) -> Live {
    let (bot, farm) = tables();
    let base = bot.clone();
    let table = farm.clone();
    Live::over(
        rx,
        "recover",
        quiet(),
        bot,
        farm,
        std::sync::Arc::new(move |p: &Profile| {
            let mut bot = base.clone();
            if let Some(set) = &p.bot {
                bot.minor_heal_at_percent = set.minor_heal_at_percent;
            }
            (bot, table.clone())
        }),
    )
}

/// The sheet, the inventory, the opening look and the sneak: how every
/// run starts.
fn opening() -> Vec<(&'static str, String)> {
    vec![
        ("stat", NINJA_SHEET.into()),
        ("inventory", EMPTY_HANDED.into()),
        ("look", format!("\r\nlook{}", block("Guard Post", "north"))),
        ("sneak", reply("sneak", "Attempting to sneak...")),
    ]
}

fn sneaky_step(name: &str, exits: &str) -> (&'static str, String) {
    ("n", format!("\r\nn\r\nSneaking...{}", block(name, exits)))
}

fn home_steps() -> Vec<(&'static str, String)> {
    vec![
        ("s", format!("\r\ns{}", block("Inner Ward", "north south"))),
        ("s", format!("\r\ns{}", block("Guard Post", "north"))),
    ]
}

/// The board's reply to `spells` for a character who knows starlight.
const STARLIGHT_BOOK: &str = "\r\nspells\r\nYou have the following spells:\r\n\
Level Mana Short Spell Name\r\n\x20 1   4    star  starlight                     \r\n\
[HP=30/MA=0]:";

/// One shipped `starlight` record, with only the fields discovery reads
/// set to anything: the ability list, the `target` column decoded as
/// `match_type`, and the mana cost. Everything else is zero. The same
/// shape `farm_scripted.rs` builds, duplicated because test crates do
/// not share modules.
fn starlight() -> mud_core::content::Spell {
    use mud_core::ability::Ability;
    use mud_core::content::{Element, MatchType, SaveClass, ScalePair, Spell, SpellId, TargetMode};
    Spell {
        id: SpellId(26),
        name: "starlight".into(),
        short_name: "star".into(),
        cast_msg_a: None,
        cast_msg_b: None,
        abilities: vec![(Ability::RoomIllu, 0)],
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
        duration: 80,
        element: Element::Magic,
        class_gate_group: 0,
        mana_cost: 4,
        max_increase: ScalePair::NONE,
        required_class_level: 0,
        min_increase: ScalePair::NONE,
        duration_increase: ScalePair::NONE,
        msg_style: 0,
    }
}

/// The spell table the light discovery reads. Handed to the session by
/// the test, because the job's own content path is empty here.
fn spell_table() -> Content {
    let mut content = Content::default();
    content.add_spell(starlight());
    content
}

/// The whole job over a scripted board, with `content` on the session
/// before it starts.
async fn recover_with(
    graph: Arc<RoomGraph>,
    script: Vec<(&'static str, String)>,
    content: Option<Content>,
) -> (Result<RecoverEnd, FarmError>, Vec<String>) {
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    probe_sheet(&session, None).await;
    if let Some(content) = content {
        session.set_content(Arc::new(content));
    }
    let out = tokio::time::timeout(
        Duration::from_secs(60),
        run_recover(&session, graph, START, DEATH, settings(), None, &quiet()),
    )
    .await
    .expect("run_recover should finish, not hang");
    let log = received.lock().unwrap().clone();
    (out, log)
}

async fn recover_over(
    graph: Arc<RoomGraph>,
    script: Vec<(&'static str, String)>,
) -> (Result<RecoverEnd, FarmError>, Vec<String>) {
    recover_with(graph, script, None).await
}

fn count(log: &[String], line: &str) -> usize {
    log.iter().filter(|l| *l == line).count()
}

// ------------------------------------------------------------ refusals

/// Nothing is sent before the refusal: the character cannot sneak, so
/// there is no job to run.
#[tokio::test]
async fn stealth_zero_is_refused_before_anything_is_sent() {
    let (addr, received) = scripted_board(vec![]).await;
    let session = session_for(addr).await;
    let out = run_recover(
        &session,
        corridor(0),
        START,
        DEATH,
        settings(),
        None,
        &quiet(),
    )
    .await;
    let why = match out {
        Err(FarmError::Config(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert!(why.contains("Stealth is 0"), "{why}");
    assert!(received.lock().unwrap().is_empty(), "nothing may be sent");
}

// ---------------------------------------------------------- the walk in

/// The first hop lands without `Sneaking...`. The sneak broke, and the
/// job turns for home from Inner Ward without ever searching.
#[tokio::test]
async fn a_broken_sneak_turns_for_home_without_searching() {
    let mut script = opening();
    script.push(("n", format!("\r\nn{}", block("Inner Ward", "north south"))));
    script.push(("s", format!("\r\ns{}", block("Guard Post", "north"))));
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must survive a break");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Broke {
                at: MIDWAY,
                name: "Inner Ward".into()
            },
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "search"), 0, "no search after a break: {log:?}");
    assert_eq!(count(&log, "sneak"), 1, "the walk home does not arm: {log:?}");
}

/// The floor is bare. One search, then home.
#[tokio::test]
async fn an_empty_search_goes_home() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push(("search", reply("search", "Your search revealed nothing.")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Nothing,
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "search"), 1, "exactly one search: {log:?}");
    let sneak_at = log.iter().position(|l| l == "sneak").unwrap();
    let first_move = log.iter().position(|l| l == "n").unwrap();
    assert!(
        sneak_at < first_move,
        "the sneak is armed before the first move: {log:?}"
    );
}

/// The board refuses the search because something already has the
/// character. Home at once.
#[tokio::test]
async fn a_search_refused_for_combat_goes_home() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push((
        "search",
        reply("search", "You may not search while attacking!"),
    ));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Attacked {
                at: DEATH,
                name: "Keep".into()
            },
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l.starts_with("a ")),
        "never an attack: {log:?}"
    );
}

/// The board swallows the `search` and says nothing at all. There is
/// nothing to be learned by standing in the room the character died in,
/// so the job goes home reading it as an empty floor.
#[tokio::test]
async fn a_search_the_board_never_answers_goes_home() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    // The entry is consumed and answered with nothing, so the search
    // waits out its whole deadline.
    script.push(("search", String::new()));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("an unanswered search must not strand the character");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Nothing,
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "search"), 1, "exactly one search: {log:?}");
}

/// The second hop is refused because something already has the
/// character. The job never searches, and it names the room it was
/// standing in when the board said no.
#[tokio::test]
async fn a_move_refused_for_combat_turns_for_home() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push((
        "n",
        reply("n", "You may not enter that room while in combat!"),
    ));
    script.push(("s", format!("\r\ns{}", block("Guard Post", "north"))));
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("a refused move must not fail the job");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Attacked {
                at: MIDWAY,
                name: "Inner Ward".into()
            },
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "search"), 0, "no search after a refusal: {log:?}");
}

/// The walk home is refused once and sent again after the round. The
/// run completes, because a refused move on the way back is retried
/// rather than given up on.
#[tokio::test]
async fn the_walk_home_retries_a_move_refused_for_combat() {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push(("search", reply("search", "Your search revealed nothing.")));
    // The refusal ends the round it was refused in. The next round's
    // prompt comes a moment later, and that is the one the retry waits
    // for.
    script.push((
        "s",
        format!(
            "{}{PAUSE}\r\n{PROMPT}",
            reply("s", "You may not enter that room while in combat!")
        ),
    ));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the retry must carry the walk home");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Nothing,
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    assert_eq!(count(&log, "s"), 3, "the refused move is sent again: {log:?}");
    let later = log
        .iter()
        .position(|l| l == LATER)
        .unwrap_or_else(|| panic!("the second prompt must go out: {log:?}"));
    let retry = log
        .iter()
        .enumerate()
        .filter(|(_, l)| *l == "s")
        .map(|(i, _)| i)
        .nth(1)
        .unwrap();
    assert!(
        later < retry,
        "the retry waits for the next round's prompt: {log:?}"
    );
}

// ------------------------------------------------------------- the light

/// The death room is dark and the character knows starlight, so the
/// spell goes out standing still, before the sneak is armed.
#[tokio::test]
async fn a_dark_death_room_is_lit_before_the_sneak() {
    let mut script = opening();
    script.insert(2, ("spells", STARLIGHT_BOOK.into()));
    script.insert(
        4,
        ("cast star", reply("cast star", "You cast starlight!")),
    );
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push(("search", reply("search", "Your search revealed nothing.")));
    script.extend(home_steps());
    let (out, log) = recover_with(corridor(-1), script, Some(spell_table())).await;
    let end = out.expect("the job must finish");
    assert_eq!(
        end,
        RecoverEnd::Home {
            at: START,
            why: HomeWhy::Nothing,
            haul: Default::default(),
        },
        "log: {log:?}"
    );
    let cast_at = log
        .iter()
        .position(|l| l == "cast star")
        .unwrap_or_else(|| panic!("the light spell must go out: {log:?}"));
    let sneak_at = log.iter().position(|l| l == "sneak").unwrap();
    assert!(cast_at < sneak_at, "lit before armed: {log:?}");
}

/// The death room is dark and the character carries a torch instead of
/// knowing a spell. The torch is lit in the start room, before the
/// sneak, because lighting breaks one. The item arm shares every line
/// of `ensure_lit` after the command string with the spell arm above,
/// so this pins the inventory read and the item's own light command.
#[tokio::test]
async fn a_dark_death_room_is_lit_by_a_torch_before_the_sneak() {
    let script = vec![
        ("stat", NINJA_SHEET.into()),
        (
            "inventory",
            "\r\ninventory\r\nYou are carrying a torch.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:"
                .to_string(),
        ),
        ("look", format!("\r\nlook{}", block("Guard Post", "north"))),
        ("light torch", reply("light torch", "You lit the torch.")),
        ("sneak", reply("sneak", "Attempting to sneak...")),
        sneaky_step("Inner Ward", "north south"),
        sneaky_step("Keep", "south"),
        ("search", reply("search", "Your search revealed nothing.")),
        ("s", format!("\r\ns{}", block("Inner Ward", "north south"))),
        ("s", format!("\r\ns{}", block("Guard Post", "north"))),
    ];
    let (out, log) = recover_over(corridor(-150), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(end.haul().summary(), "0 of 0 items", "log: {log:?}");
    let lit = log.iter().position(|l| l == "light torch").expect("the torch is lit");
    let armed = log.iter().position(|l| l == "sneak").expect("the sneak is armed");
    assert!(lit < armed, "light before sneak: {log:?}");
}

// ------------------------------------------------------------ the sweep

fn up_to_the_search(items: &str, prompt: &str) -> Vec<(&'static str, String)> {
    let mut script = opening();
    script.push(sneaky_step("Inner Ward", "north south"));
    script.push(sneaky_step("Keep", "south"));
    script.push((
        "search",
        format!("\r\nsearch{}", block_with("Keep", items, "south", prompt)),
    ));
    script
}

/// Gear, then coins, then home, and the ending names the counts.
#[tokio::test]
async fn the_happy_path_takes_gear_then_coins_and_reports_the_haul() {
    let mut script = up_to_the_search("a rusty dagger, 5 copper farthings", PROMPT);
    script.push((
        "get rusty dagger",
        reply("get rusty dagger", "You took a rusty dagger."),
    ));
    script.push((
        "get copper",
        reply("get copper", "You picked up 5 copper farthings"),
    ));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    let RecoverEnd::Home { at, why, haul } = end else {
        panic!("expected home, got {end:?}, log {log:?}");
    };
    assert_eq!(at, START);
    assert_eq!(why, HomeWhy::Swept);
    assert_eq!(haul.taken, vec!["a rusty dagger", "5 copper farthings"]);
    assert_eq!(haul.summary(), "1 of 1 items and 1 coin piles");
    let dagger = log.iter().position(|l| l == "get rusty dagger").unwrap();
    let copper = log.iter().position(|l| l == "get copper").unwrap();
    assert!(dagger < copper, "gear before coins: {log:?}");
    assert_eq!(count(&log, "search"), 1);
}

/// Somebody else got the dagger. Dropped without a retry, and the cap
/// is still taken.
#[tokio::test]
async fn a_dont_see_drops_the_entry_without_a_retry() {
    let mut script = up_to_the_search("a rusty dagger, a leather cap", PROMPT);
    script.push((
        "get rusty dagger",
        reply("get rusty dagger", "You don't see a rusty dagger here."),
    ));
    script.push((
        "get leather cap",
        reply("get leather cap", "You took a leather cap."),
    ));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(end.haul().summary(), "1 of 2 items", "log: {log:?}");
    assert_eq!(count(&log, "get rusty dagger"), 1, "no retry: {log:?}");
}

/// The board says nothing to the `get`, three times. That is what an
/// item too heavy to lift looks like, and the entry is dropped after
/// the third silence.
#[tokio::test]
async fn three_silent_attempts_drop_the_entry() {
    let mut script = up_to_the_search("an anvil", PROMPT);
    script.push(("get anvil", format!("\r\nget anvil\r\n{PROMPT}")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(end.haul().summary(), "0 of 1 items", "log: {log:?}");
    assert_eq!(count(&log, "get anvil"), 3, "three attempts: {log:?}");
    assert_eq!(count(&log, "s"), 2, "and then home: {log:?}");
}

/// The prompt after the first pickup shows 15 of 30, under the 70 mark.
/// The sweep ends there with what it has, and the cap stays on the
/// floor.
#[tokio::test]
async fn hitpoints_under_the_mark_end_the_sweep_with_a_partial_haul() {
    let mut script = up_to_the_search("a rusty dagger, a leather cap", PROMPT);
    script.push((
        "get rusty dagger",
        "\r\nget rusty dagger\r\nYou took a rusty dagger.\r\n[HP=15/MA=0]:".to_string(),
    ));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    let RecoverEnd::Home { why, haul, .. } = end else {
        panic!("expected home, got {end:?}, log {log:?}");
    };
    assert_eq!(why, HomeWhy::Hurt { mark: 70 });
    assert_eq!(haul.summary(), "1 of 2 items");
    assert_eq!(count(&log, "get leather cap"), 0, "the sweep stopped: {log:?}");
}

/// The character arrived already too hurt to stay: the search's own
/// block ends on 15 of 30, under the 70 mark. Nothing is asked for.
#[tokio::test]
async fn a_search_block_under_the_mark_asks_for_nothing() {
    let mut script = up_to_the_search("a rusty dagger", "[HP=15/MA=0]:");
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    let RecoverEnd::Home { why, haul, .. } = end else {
        panic!("expected home, got {end:?}, log {log:?}");
    };
    assert_eq!(why, HomeWhy::Hurt { mark: 70 });
    assert_eq!(haul.summary(), "0 of 1 items");
    assert_eq!(
        count(&log, "get rusty dagger"),
        0,
        "nothing is asked for: {log:?}"
    );
}

/// The operator raises the heal mark to 101 while the sweep is waiting
/// out a `get` the board will not answer. The next prompt is read
/// against the new mark, so a character at full hitpoints is now too
/// hurt to go on, and the second entry is never asked for.
#[tokio::test]
async fn a_settings_change_lands_while_the_sweep_waits() {
    let mut script = up_to_the_search("an anvil, a leather cap", PROMPT);
    script.push(("get anvil", format!("\r\nget anvil\r\n{PROMPT}")));
    script.extend(home_steps());
    let (addr, received) = scripted_board(script).await;
    let session = session_for(addr).await;
    probe_sheet(&session, None).await;
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let log = Arc::clone(&received);
    let notices = quiet();
    let (out, ()) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(60),
            run_recover(
                &session,
                corridor(0),
                START,
                DEATH,
                live_settings(rx),
                None,
                &notices,
            ),
        ),
        async {
            // The change goes out once the first `get` has, so it can
            // only land in the wait the sweep is already sitting in.
            while !log.lock().unwrap().iter().any(|l| l == "get anvil") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tx.send(Profile {
                bot: Some(BotConfig {
                    minor_heal_at_percent: 101,
                    ..BotConfig::default()
                }),
                ..Default::default()
            })
            .unwrap();
        },
    );
    let end = out
        .expect("run_recover should finish, not hang")
        .expect("the job must finish");
    let log = received.lock().unwrap().clone();
    let RecoverEnd::Home { why, .. } = end else {
        panic!("expected home, got {end:?}, log {log:?}");
    };
    assert_eq!(why, HomeWhy::Hurt { mark: 101 }, "log: {log:?}");
    assert_eq!(count(&log, "get leather cap"), 0, "the sweep stopped: {log:?}");
}

/// A rat swings during the sweep. Above the mark the sweep carries on,
/// and nothing the job sends is an attack.
#[tokio::test]
async fn a_swing_mid_sweep_is_ignored_and_never_answered() {
    let mut script = up_to_the_search("a rusty dagger, a leather cap", PROMPT);
    script.push((
        "get rusty dagger",
        reply(
            "get rusty dagger",
            "The giant rat swings at you but misses!\r\nYou took a rusty dagger.",
        ),
    ));
    script.push(("get leather cap", reply("get leather cap", "You took a leather cap.")));
    script.extend(home_steps());
    let (out, log) = recover_over(corridor(0), script).await;
    let end = out.expect("the job must finish");
    assert_eq!(end.haul().summary(), "2 of 2 items", "log: {log:?}");
    assert!(!log.iter().any(|l| l.starts_with("a ")), "never an attack: {log:?}");
    assert_eq!(count(&log, "look"), 1, "no defence look either: {log:?}");
}

