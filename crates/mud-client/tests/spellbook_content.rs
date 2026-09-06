//! `probe_sheet`'s use of `class.magictype` to skip or retarget the
//! spellbook probe (`2026-08-22-one-path-to-content` Task 5).
//!
//! The harness is the `farm_scripted.rs` / `go_scripted.rs` shape --
//! test crates do not share modules, so it is duplicated here, matching
//! this suite's existing pattern.
//!
//! `a_class_the_database_does_not_carry_still_gets_probed` is the one
//! that matters: it is the test the plan's mutation targets. Mutating
//! `crate::sheet::class_caster_group` to answer `Some(0)` on a miss
//! (instead of `None`) makes `probe_sheet` skip the spellbook probe for
//! ANY unrecognized class name -- silently telling a real caster on an
//! unfamiliar or customised board that it has no spells. That is
//! exactly the failure mode this test exists to catch, and it does: the
//! mutation makes it fail (verified by hand while writing this test;
//! see the Task 5 report).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use mud_client::farm::probe_sheet;
use mud_client::profile::Profile;
use mud_client::session::Session;
use mud_core::content::{Class, ClassId, Content};

fn content_with_classes(classes: &[(&str, i16)]) -> Content {
    let mut content = Content::default();
    for (i, (name, magictype)) in classes.iter().enumerate() {
        content.add_class(Class {
            id: ClassId(i as u16 + 1),
            name: (*name).to_string(),
            abilities: Vec::new(),
            hp_per_level: 0,
            hp_seed: 0,
            caster_group: *magictype,
            casting_factor: 0,
            exp_base: 0,
            combat_factor: 0,
            weapon_code: 0,
            armour_code: 0,
        });
    }
    content
}

/// A board driven by a per-line script: `(matcher, reply)`, each entry
/// used once, first unused match wins. Unmatched lines echo + say back.
async fn scripted_board(
    script: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&received);
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"\r\n\x1b[1;36mGuard Post\r\nObvious exits: north\r\n[HP=30/MA=0]:")
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
    };
    Session::connect(&profile, None).await.unwrap()
}

/// A minimal `stat` reply naming one class -- enough for
/// `Stats::parse`'s `Class:` field, nothing else populated.
fn stat_reply(class: &str) -> String {
    format!("\r\nstat\r\nClass: {class}\r\n[HP=30/MA=0]:")
}

const INVENTORY: &str =
    "\r\ninventory\r\nYou are carrying nothing.\r\nEncumbrance: 0/2400 - None [0%]\r\n[HP=30/MA=0]:";

/// Reads the class off the board before `probe_sheet` runs.
///
/// This helper's previous doc claimed it did what "production code
/// would" -- and asserted outright that `probe_sheet` never sends
/// `stat`. Both were true, and together they were the bug: NOTHING in
/// the client sent `stat`, so `Session::stats()` stayed empty in the
/// field, `capabilities().picklocks` was always 0, and a live `/go`
/// stopped at a locked door reporting "can't pick" against a thief who
/// could. Every test here passed the whole time because this helper
/// hand-seeded what production never fetched.
///
/// `probe_sheet` now sends `stat` itself, so this is belt-and-braces:
/// it pins the parse independently of the probe. See
/// `probe_sheet_asks_for_the_sheet_itself` for the guard that the send
/// actually happens.
async fn prime_class(session: &Session, class: &str) {
    session.send("stat");
    session
        .expect("[HP=30/MA=0]:", Duration::from_secs(5))
        .await
        .expect("stat reply");
    assert_eq!(
        session.stats().class.as_deref(),
        Some(class),
        "test setup: the stat reply must actually parse"
    );
}

/// The regression guard for the live "can't pick" bug: `probe_sheet`
/// must ask the board for the sheet ITSELF, with no operator command
/// and no other probe having primed it. Everything downstream --
/// `picklocks`, `stealth`, the casting dialect -- reads
/// `Session::stats()`, and for months nothing filled it.
#[tokio::test]
async fn probe_sheet_asks_for_the_sheet_itself() {
    let content = content_with_classes(&[("Warrior", 0)]);
    let (addr, received) = scripted_board(vec![
        ("stat", stat_reply("Warrior")),
        ("inventory", INVENTORY.into()),
    ])
    .await;
    let session = session_for(addr).await;
    // Deliberately NO prime_class: the probe is on its own.

    probe_sheet(&session, Some(&content)).await;

    let log = received.lock().unwrap().clone();
    assert!(
        log.contains(&"stat".to_string()),
        "probe_sheet must send `stat` unprompted; sent {log:?}"
    );
    assert_eq!(
        session.stats().class.as_deref(),
        Some("Warrior"),
        "the sheet the probe fetched must reach Session::stats()"
    );
}

/// magictype 0 (Warrior, Witchunter, Ninja, Thief): a confidently-known
/// non-caster gets no spellbook probe at all -- neither `spells` nor
/// `powers` is ever sent.
#[tokio::test]
async fn a_non_caster_class_sends_no_spellbook_probe() {
    let content = content_with_classes(&[("Warrior", 0)]);
    let (addr, received) = scripted_board(vec![
        ("stat", stat_reply("Warrior")),
        ("inventory", INVENTORY.into()),
    ])
    .await;
    let session = session_for(addr).await;
    prime_class(&session, "Warrior").await;

    probe_sheet(&session, Some(&content)).await;

    let log = received.lock().unwrap().clone();
    assert!(log.contains(&"inventory".to_string()), "{log:?}");
    assert!(!log.contains(&"spells".to_string()), "{log:?}");
    assert!(!log.contains(&"powers".to_string()), "{log:?}");
}

/// magictype 5 (Mystic): the probe goes straight to `powers`, skipping
/// the spells-then-redirect round trip entirely.
#[tokio::test]
async fn a_mystic_class_asks_for_powers_directly() {
    let content = content_with_classes(&[("Mystic", 5)]);
    let (addr, received) = scripted_board(vec![
        ("stat", stat_reply("Mystic")),
        ("inventory", INVENTORY.into()),
        (
            "powers",
            "\r\npowers\r\nYou have the following powers:\r\n[HP=30/MA=0]:".into(),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    prime_class(&session, "Mystic").await;

    probe_sheet(&session, Some(&content)).await;

    let log = received.lock().unwrap().clone();
    assert!(log.contains(&"powers".to_string()), "{log:?}");
    assert!(
        !log.contains(&"spells".to_string()),
        "a known Mystic must never pay the spells-then-redirect round trip: {log:?}"
    );
}

/// The test the plan's mutation targets. `Content` here carries classes
/// at all, but never one named "Zorblatt" -- a class name the database
/// does not know, standing in for a customised board or an unexpected
/// spelling. The lookup MUST answer `None`, and `probe_sheet` MUST fall
/// through to the ordinary spells-then-maybe-redirect probe, exactly as
/// if no `Content` had been passed at all.
#[tokio::test]
async fn a_class_the_database_does_not_carry_still_gets_probed() {
    let content = content_with_classes(&[("Warrior", 0), ("Mystic", 5)]);
    let (addr, received) = scripted_board(vec![
        ("stat", stat_reply("Zorblatt")),
        ("inventory", INVENTORY.into()),
        (
            "spells",
            "\r\nspells\r\nYou have the following spells:\r\n[HP=30/MA=0]:".into(),
        ),
    ])
    .await;
    let session = session_for(addr).await;
    prime_class(&session, "Zorblatt").await;

    probe_sheet(&session, Some(&content)).await;

    let log = received.lock().unwrap().clone();
    assert!(
        log.contains(&"spells".to_string()),
        "a class miss must fall through to the ordinary probe, not skip it: {log:?}"
    );
}
