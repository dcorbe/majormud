//! `Stats::parse` against the `stat` sheet, and the session's own read of
//! it off the wire.
//!
//! The fixture in [`parses_every_field_from_the_beef_sheet`] is a live
//! capture from MMud Reborn, 2026-08-22 (see
//! `.superpowers/sdd/2026-08-22-session-knows-character/task-1-brief.md`).
//! Its columns do not line up between rows — `Traps` and `Picklocks` sit
//! alone on theirs — so a column-position parser would either misread or
//! silently drop them; this only proves out if every field is asserted.

use std::time::{Duration, Instant};

use mud_client::correlate::{CmdId, Correlator};
use mud_client::events::Event;
use mud_client::stats::Stats;

const BEEF_SHEET: &str = "\
Name: Beef                             Lives/CP:    9/100
Race: Dark-Elf    Exp: 0               Perception:     43
Class: Ninja      Level: 1             Stealth:        56
Hits:    22/22    Armour Class:   0/0  Thievery:        0
                                       Traps:          29
                                       Picklocks:      31
Strength:  40     Agility: 50          Tracking:       26
Intellect: 50     Health:  30          Martial Arts:   51
Willpower: 30     Charm:   40          MagicRes:       35";

#[test]
fn parses_every_field_from_the_beef_sheet() {
    let stats = Stats::parse(BEEF_SHEET);

    assert_eq!(stats.name.as_deref(), Some("Beef"));
    assert_eq!(stats.race.as_deref(), Some("Dark-Elf"));
    assert_eq!(stats.class.as_deref(), Some("Ninja"));
    assert_eq!(stats.level, Some(1));
    assert_eq!(stats.exp, Some(0));
    assert_eq!(stats.lives, Some(9));
    assert_eq!(stats.cp, Some(100));
    assert_eq!(stats.hits, Some(22));
    assert_eq!(stats.max_hits, Some(22));
    assert_eq!(stats.armour_class, Some(0));
    assert_eq!(stats.max_armour_class, Some(0));
    assert_eq!(stats.perception, Some(43));
    assert_eq!(stats.stealth, Some(56));
    assert_eq!(stats.thievery, Some(0));
    assert_eq!(stats.traps, Some(29));
    assert_eq!(stats.picklocks, Some(31));
    assert_eq!(stats.tracking, Some(26));
    assert_eq!(stats.martial_arts, Some(51));
    assert_eq!(stats.magic_res, Some(35));
    assert_eq!(stats.strength, Some(40));
    assert_eq!(stats.agility, Some(50));
    assert_eq!(stats.intellect, Some(50));
    assert_eq!(stats.health, Some(30));
    assert_eq!(stats.willpower, Some(30));
    assert_eq!(stats.charm, Some(40));
}

/// A board can bring the class column right up against the next label:
/// with a 10-character class name the fixed column position leaves only
/// ONE space before `Level:`, unlike the multi-space gutter the "Beef"
/// sheet happens to have. `MudPlay`'s `StatParser` hit exactly this (see
/// `archive/FujiTerm/MudPlay/Game/StatParser.cs`'s `ClassRx` and
/// `MudPlay.Tests/StatParserTests.cs`'s
/// `Class_SingleSpaceBeforeNextLabel_StillCaptured`) — a two-space-only
/// terminator silently drops the class here instead of erroring.
#[test]
fn class_survives_a_single_space_before_the_next_label() {
    let stats =
        Stats::parse("Class: Missionary Level: 2             Stealth:        65");

    assert_eq!(stats.class.as_deref(), Some("Missionary"));
    assert_eq!(stats.level, Some(2));
    assert_eq!(stats.stealth, Some(65));
}

#[test]
fn a_partial_sheet_parses_to_what_it_has() {
    let stats = Stats::parse("Name: Beef\nPicklocks: 31\n");

    assert_eq!(stats.name.as_deref(), Some("Beef"));
    assert_eq!(stats.picklocks, Some(31));
    assert_eq!(stats.race, None);
    assert_eq!(stats.class, None);
    assert_eq!(stats.level, None);
}

#[test]
fn empty_text_parses_to_an_entirely_absent_sheet() {
    let stats = Stats::parse("");

    assert_eq!(stats, Stats::default());
}

// ---------------------------------------------------- correlator: Kind::Stat

/// `stat`'s reply has no fixed terminal wording (`Traps`/`Picklocks`
/// aren't always the last row — active buffs append more lines after
/// them), so nothing in the BODY can retire it. Left `Opaque`, its
/// `completes` would always answer `false` and a consumer waiting on
/// `Correlated::answers` would burn its whole deadline for nothing —
/// the same class of phantom timeout `b19f862d` fixed for the locked
/// door. The ordinary game prompt that follows every reply is what
/// closes it out instead.
#[test]
fn the_ordinary_prompt_retires_a_pending_stat() {
    let t = Instant::now();
    let mut c = Correlator::new(Duration::from_secs(10));

    c.sent(CmdId(1), "stat", t);
    // The echo attributes and accepts — this already worked for Opaque
    // commands (e.g. `i`), so it is not what this test is proving.
    assert_eq!(c.on_event(Event::Line("stat".into()), t).answers, Some(CmdId(1)));
    // Ordinary body lines answer nothing on their own...
    assert_eq!(c.on_event(Event::Line("Name: Beef".into()), t).answers, None);
    // ...but the prompt that follows the whole reply retires the entry.
    assert_eq!(
        c.on_event(Event::Prompt { hp: 22, mana: None, status: None }, t).answers,
        Some(CmdId(1))
    );
}

/// The same prompt that closes a `stat` reply answers NOTHING for every
/// other kind — this is the invariant `the_ordinary_prompt_retires_a_
/// pending_stat` is the one deliberate exception to, pinned down here so
/// a future edit that makes `Kind::Stat`'s `completes` too permissive
/// (e.g. answering any Prompt) gets caught immediately rather than by a
/// stray reply landing on the wrong command months later.
#[test]
fn the_ordinary_prompt_retires_nothing_for_a_move() {
    let t = Instant::now();
    let mut c = Correlator::new(Duration::from_secs(10));

    c.sent(CmdId(1), "n", t);
    assert_eq!(c.on_event(Event::Line("n".into()), t).answers, Some(CmdId(1)));
    assert_eq!(
        c.on_event(Event::Prompt { hp: 22, mana: None, status: None }, t).answers,
        None
    );
}

// -------------------------------------------------- Session::stats(), live

use mud_client::dialect::Target;
use mud_client::profile::Profile;
use mud_client::session::Session;

async fn session_to(addr: std::net::SocketAddr) -> Session {
    let profile = Profile {
        target: Target::MbbsEmu,
        host: addr.ip().to_string(),
        port: addr.port(),
        username: "testuser".into(),
        password: "testpass".into(),
        pace_ms: Some(0),
        disable_evil_warnings: false,
        bot: None,
        farm: None,
        bank: Default::default(),
    };
    Session::connect(&profile, None).await.unwrap()
}

/// A board that echoes whatever it is sent and answers with the full
/// Beef sheet followed by an ordinary prompt — enough to drive the
/// session's own multi-line accumulation end to end without the rest of
/// `play`. `inject`, when `Some`, splices one extra unrelated line into
/// the MIDDLE of the body (between `Thievery` and `Traps`) to stand in
/// for interleaved traffic (another player's shout, a queued command's
/// own receipt echo) landing in the same window.
async fn stat_board(inject: Option<&'static str>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut pending = String::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = pending.find('\n') {
                let line: String = pending.drain(..=nl).collect();
                let line = line.trim().to_string();
                let mut rows: Vec<&str> = BEEF_SHEET.lines().collect();
                if let Some(noise) = inject {
                    rows.insert(4, noise); // after "Thievery:", before "Traps:"
                }
                let body = rows.join("\r\n");
                let reply = format!("\r\n{line}\r\n{body}\r\n[HP=22]:");
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        }
    });
    addr
}

/// The realm-entry pipeline, measured live 2026-08-26 (test.raw): the
/// purse ping's `i` and the probe's `stat` are both receipt-echoed
/// BEFORE the first reply, then each reply arrives FIFO with its own
/// prompt. The i-reply's prompt lands inside the freshly armed stat
/// window; closing on it parses the inventory text as the sheet
/// (nothing recognised), discards the real sheet that follows, and the
/// session walks with picklocks/stealth 0 — a Ninja with Picklocks 28
/// stopped at a locked door with "can't pick".
async fn pipelined_board() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut pending = String::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = sock.read(&mut buf).await {
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            if !pending.contains("stat\n") && !pending.contains("stat\r\n") {
                continue;
            }
            let sheet = BEEF_SHEET.lines().collect::<Vec<_>>().join("\r\n");
            let reply = format!(
                "\r\ni\r\nstat\r\n\
                 You are carrying 80 runic coins\r\n\
                 You have no keys.\r\n\
                 Wealth: 80000000 copper farthings\r\n\
                 Encumbrance: 26/960 - None [2%]\r\n\
                 [HP=22]:{sheet}\r\n\
                 [HP=22]:DONE\r\n"
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
            pending.clear();
        }
    });
    addr
}

#[tokio::test]
async fn a_pipelined_reply_ahead_of_the_sheet_does_not_close_the_window() {
    let addr = pipelined_board().await;
    let session = session_to(addr).await;

    session.send("i");
    session.send("stat");
    session
        .expect("DONE", Duration::from_secs(5))
        .await
        .expect("full pipelined exchange");

    assert_eq!(
        session.stats(),
        Stats::parse(BEEF_SHEET),
        "the i-reply's own prompt must not close the stat window before the sheet arrives"
    );
}

/// The property Task 2 exists for: a caller sends `stat`, the session
/// reads the WHOLE multi-line reply off the wire on its own, and
/// [`Session::stats`] hands back exactly what [`Stats::parse`] would
/// give the raw sheet — proving the session's accumulation and Task 1's
/// parser agree, not just that each works in isolation.
#[tokio::test]
async fn session_stats_reads_the_whole_multiline_sheet_off_the_board() {
    let addr = stat_board(None).await;
    let session = session_to(addr).await;

    session.send("stat");
    session
        .expect("[HP=22]:", Duration::from_secs(5))
        .await
        .expect("stat reply");

    assert_eq!(session.stats(), Stats::parse(BEEF_SHEET));
}

/// A line that has nothing to do with the sheet, landing mid-reply, must
/// not truncate it: every field after the interloper still has to come
/// through. Without this the accumulator could plausibly stop (or
/// discard the rest) the moment it saw a line that didn't look like part
/// of the sheet, silently losing `Picklocks` and everything below it —
/// exactly the field this whole feature exists to make readable.
#[tokio::test]
async fn an_interleaved_line_does_not_truncate_the_multiline_sheet() {
    let addr = stat_board(Some("Another player shouts hello!")).await;
    let session = session_to(addr).await;

    session.send("stat");
    session
        .expect("[HP=22]:", Duration::from_secs(5))
        .await
        .expect("stat reply");

    assert_eq!(
        session.stats(),
        Stats::parse(BEEF_SHEET),
        "an unsolicited line mid-reply must not corrupt or truncate the parsed sheet"
    );
}
