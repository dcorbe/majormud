//! The outbound gate driven by the real board.
//!
//! `tests/farm.rs` pins the gate against hand-written event sequences,
//! and hand-written sequences are always tidier than the board: they put
//! a prompt after every command and a scolding in a convenient place.
//! This suite replays the 51 captured transcripts through the same wire
//! and parser stack the live client uses, feeds them to an armed `Bot`
//! wired to a `Gate`, and asserts the gate's contract survives whatever
//! the board actually did.
//!
//! What the corpus cannot prove: the resend. Fourteen scoldings survive
//! in the captures, but every one of them lands with the gate empty —
//! the transcripts were recorded by the oracle tooling driving the board
//! by hand, so no *bot* command was ever in flight when flood control
//! tripped. A resend assertion here would iterate an empty list and
//! report coverage that does not exist. The resend contract is pinned
//! deterministically in `tests/farm.rs` instead; what this file adds is
//! proof that real board bytes still parse into `Event::SlowDown` at
//! all, and that the gate's ordering holds over 51 real transcripts.

use std::time::{Duration, Instant};

use mud_client::bot::{Bot, BotAction, BotConfig};
use mud_client::events::Event;
use mud_client::farm::Gate;
use mud_client::parse::Parser;
use mud_client::wire::{TelnetFilter, cp437_to_string};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/oracle");

const BACKOFF: Duration = Duration::from_millis(5000);

/// The synthetic clock advances a millisecond per event: fast enough to
/// walk a whole transcript, slow enough that the in-flight ack timeout
/// never fires. What is under test here is the gate's ordering contract,
/// not its timing — the timeouts have their own tests in `farm.rs`.
const TICK: Duration = Duration::from_millis(1);

fn corpus_files() -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(CORPUS)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "raw"))
        .collect();
    files.sort();
    files
}

fn corpus_events(path: &std::path::Path) -> Vec<Event> {
    let raw = std::fs::read(path).unwrap();
    let mut f = TelnetFilter::new();
    let data = f.push(&raw).data;
    let mut p = Parser::new();
    let mut ev = p.push(&cp437_to_string(&data));
    ev.extend(p.finish());
    ev
}

fn armed_bot() -> Bot {
    Bot::new(BotConfig {
        auto_combat: true,
        auto_heal: true,
        auto_get: true,
        auto_flee: true,
        max_hp: 35, // the Dwarf Warrior body most captures were taken on
        ..BotConfig::default()
    })
}

/// What one replay produced, in the order the gate released it.
#[derive(Debug, Default)]
struct Run {
    /// (event index, command) for everything the gate actually emitted.
    emitted: Vec<(usize, String)>,
    /// Everything the bot decided, whether or not it went out.
    decided: usize,
    slow_downs: usize,
    /// Scoldings that arrived with a command in flight — the only ones
    /// that can force a resend. Zero across the whole corpus; see the
    /// module docs.
    scoldings_with_a_command_in_flight: usize,
}

/// Replay one transcript through Bot -> Gate, draining the gate between
/// events exactly as the runner's pump does.
fn drive(path: &std::path::Path) -> Run {
    let t0 = Instant::now();
    let mut bot = armed_bot();
    let mut gate = Gate::new(BACKOFF);
    let mut run = Run::default();

    for (i, ev) in corpus_events(path).iter().enumerate() {
        let now = t0 + TICK * (i as u32);

        if matches!(ev, Event::SlowDown) {
            run.slow_downs += 1;
            if gate.in_flight().is_some() {
                run.scoldings_with_a_command_in_flight += 1;
            }
        }

        gate.on_event(ev, now);
        for BotAction::Send(cmd) in bot.on_event(ev) {
            run.decided += 1;
            gate.push(cmd);
        }

        // Drain whatever the gate is willing to release. The backoff is
        // far longer than TICK, so a scolding genuinely stalls the run.
        while let Some(cmd) = gate.poll(now) {
            run.emitted.push((i, cmd));
        }
    }
    run
}

fn runs() -> Vec<(std::path::PathBuf, Run)> {
    corpus_files()
        .into_iter()
        .map(|p| {
            let r = drive(&p);
            (p, r)
        })
        .collect()
}

/// Real board bytes still reach the gate as `Event::SlowDown`. Pinned by
/// count, like the corpus size itself: if the parser stops recognising
/// "Why don't you slow down for a few seconds?", the gate silently loses
/// its only defence against the board dropping commands, and no other
/// test in the suite would notice.
#[test]
fn the_corpus_still_contains_flood_control() {
    let total: usize = runs().iter().map(|(_, r)| r.slow_downs).sum();
    assert_eq!(total, 14, "flood-control lines in the corpus changed");
}

/// Pins the gap itself. If a future capture ever catches the board
/// scolding us with a bot command in flight, this fails — and that is
/// the signal to replace the unit-level resend test with a corpus-fed
/// one, because the real thing finally exists to test against.
#[test]
fn no_capture_yet_catches_a_scolding_with_a_command_in_flight() {
    let total: usize = runs()
        .iter()
        .map(|(_, r)| r.scoldings_with_a_command_in_flight)
        .sum();
    assert_eq!(
        total, 0,
        "a capture now exercises the resend path — write the corpus-fed resend test"
    );
}

/// One command in flight at a time: without an intervening prompt to
/// acknowledge the last one, nothing new goes out. This is what keeps a
/// burst of decisions from queueing minutes of stale commands at the
/// live board's 1500ms pace.
#[test]
fn never_sends_twice_without_a_prompt_in_between() {
    for (path, run) in runs() {
        let events = corpus_events(&path);
        let mut last: Option<(usize, String)> = None;
        for (i, cmd) in &run.emitted {
            if let Some((prev_i, prev_cmd)) = &last {
                let acked = events[*prev_i..=*i]
                    .iter()
                    .any(|e| matches!(e, Event::Prompt { .. }));
                assert!(
                    acked,
                    "{}: sent {cmd:?} after {prev_cmd:?} with no prompt between",
                    path.display()
                );
            }
            last = Some((*i, cmd.clone()));
        }
    }
}

/// The gate invents nothing. Everything it emits was either a bot
/// decision or a resend of one the board threw away.
#[test]
fn emits_no_more_than_the_bot_decided_plus_resends() {
    for (path, run) in runs() {
        assert!(
            run.emitted.len() <= run.decided + run.slow_downs,
            "{}: emitted {} for {} decisions and {} scoldings",
            path.display(),
            run.emitted.len(),
            run.decided,
            run.slow_downs
        );
    }
}
