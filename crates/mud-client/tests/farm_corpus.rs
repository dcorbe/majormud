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
use mud_client::correlate::{CmdId, Correlator};
use mud_client::events::Event;
use mud_client::farm::{FarmConfig, Gate, StopState, Verdict};
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
    /// that can force a resend. One, since the gate started holding a
    /// command until its echo instead of until the next prompt.
    scoldings_with_a_command_in_flight: usize,
    /// The (event index, command) pairs the board scolded away while in
    /// flight; each must be re-emitted later.
    scolded: Vec<(usize, String)>,
    /// What the gate still owed when the transcript ended, drained with
    /// the backoff expired: a scolding near the end of a capture backs
    /// off past the replay's last tick, and the resend it still owes
    /// lands here instead of in `emitted`.
    tail: Vec<String>,
    /// Times the stop was called finished while the last room block for
    /// it still listed something the bot would have swung at. Must be
    /// zero: that is the bug this whole mechanism replaced.
    left_with_a_target_listed: usize,
    /// Times the stop was called finished with no room block ever
    /// accepted — i.e. decided on something other than evidence.
    left_without_ever_looking: usize,
    /// Room blocks accepted whose "Also here:" held something attackable.
    /// A tripwire: without it the assertions above pass vacuously on a
    /// corpus that never showed the bot a monster.
    blocks_with_a_target: usize,
}

/// Replay one transcript through Bot -> Gate, draining the gate between
/// events exactly as the runner's pump does.
fn drive(path: &std::path::Path) -> Run {
    let t0 = Instant::now();
    let mut bot = armed_bot();
    let mut gate = Gate::new(BACKOFF);
    // The replay mirrors the session wiring: the gate's emissions feed a
    // Correlator and are acknowledged by SYNTHESIZED receipt echoes one
    // tick later. The capture is one-way — it holds the operator's
    // echoes, not ours — but the board echoes every accepted command
    // ~2ms after it lands, so modelling that is fidelity, not charity.
    // Without it the first unanswered emission wedges the gate for the
    // rest of the transcript (ACK_TIMEOUT is 5000 ticks) and every
    // gate assertion below goes vacuous — measured at 1818 -> 86
    // emissions when this replay briefly leaned on the operator's own
    // echoes instead.
    let mut correlator = Correlator::new(Duration::from_secs(10));
    let mut next_id = 1u64;
    let mut pending_acks: Vec<Event> = Vec::new();
    let mut run = Run::default();

    let events = corpus_events(path);
    // The transcripts wander, so the stop is whichever room the capture
    // opened in; blocks naming anywhere else are somebody else's problem
    // (the runner treats those as a flee).
    let stop_name = events
        .iter()
        .find_map(|ev| match ev {
            Event::RoomSeen(r) => Some(r.name.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let mut stop = StopState::new(stop_name.clone(), &FarmConfig::default());
    // The last block the board rendered for this stop, tracked straight
    // off the event stream so the check does not lean on StopState's own
    // bookkeeping to audit StopState.
    let mut last_block: Option<mud_client::events::RoomView> = None;

    for (i, ev) in events.iter().enumerate() {
        let now = t0 + TICK * (i as u32);

        // The board's receipt echoes for last tick's emissions.
        for ack in pending_acks.drain(..) {
            let cor = correlator.on_event(ack, now);
            gate.on_event(&cor, now);
        }

        // Pump like farm_stop: the verdict drives the looks, and the
        // corpus supplies the answers the operator's own looks recorded.
        match stop.verdict(&bot, now) {
            // The runner's `look` is deliberately NOT pushed through
            // `gate`: that gate is what the assertions below audit, and
            // injecting commands the bot never decided would rewrite what
            // they measure. It IS registered with the correlator, so the
            // operator's own captured `look` echo accepts it and the
            // block that follows attributes — the request/response pair
            // the stream actually recorded.
            Verdict::Ask | Verdict::Blind => {
                let id = CmdId(next_id);
                next_id += 1;
                correlator.sent(id, "look", now);
                stop.on_sent("look", id);
            }
            Verdict::Empty => {
                match &last_block {
                    None => run.left_without_ever_looking += 1,
                    Some(room) if bot.has_target(room) => run.left_with_a_target_listed += 1,
                    Some(_) => {}
                }
                // A real runner would leave; keep replaying so one
                // transcript can exercise more than a single stop.
                stop = StopState::new(stop_name.clone(), &FarmConfig::default());
                last_block = None;
            }
            Verdict::Busy | Verdict::Waiting { .. } => {}
        }

        if let Event::RoomSeen(room) = ev
            && room.name == stop_name
        {
            if bot.has_target(room) {
                run.blocks_with_a_target += 1;
            }
            last_block = Some(room.clone());
        }

        if matches!(ev, Event::SlowDown) {
            run.slow_downs += 1;
            if let Some(cmd) = gate.in_flight() {
                run.scoldings_with_a_command_in_flight += 1;
                run.scolded.push((i, cmd.to_string()));
            }
        }

        let cor = correlator.on_event(ev.clone(), now);
        gate.on_event(&cor, now);
        for BotAction::Send(cmd) in bot.on_event(ev) {
            run.decided += 1;
            gate.push(cmd);
        }

        // Drain whatever the gate is willing to release. The backoff is
        // far longer than TICK, so a scolding genuinely stalls the run.
        while let Some(cmd) = gate.poll(now) {
            let id = CmdId(next_id);
            next_id += 1;
            correlator.sent(id, &cmd, now);
            gate.confirm(id);
            pending_acks.push(Event::Line(cmd.clone()));
            run.emitted.push((i, cmd));
        }
        stop.on_event(&cor, &bot, now);
    }
    let after = t0 + TICK * (events.len() as u32) + BACKOFF * 2;
    while let Some(cmd) = gate.poll(after) {
        run.tail.push(cmd);
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

/// Any scolding that catches a command in flight must see it resent —
/// and the gap pin rides along: with receipt echoes acknowledging in
/// one tick, the in-flight window is a single tick wide and no captured
/// scolding lands inside it. If a future capture (or a slower ack
/// model) ever does, the resend loop above the pin starts doing real
/// work; until then the resend contract lives in `tests/farm.rs`.
#[test]
fn a_scolded_command_in_flight_is_resent() {
    let mut total = 0;
    for (path, run) in runs() {
        for (at, cmd) in &run.scolded {
            total += 1;
            assert!(
                run.emitted.iter().any(|(i, c)| i > at && c == cmd)
                    || run.tail.contains(cmd),
                "{}: {cmd:?} scolded at event {at} and never resent",
                path.display()
            );
        }
    }
    assert_eq!(total, 0, "a capture now exercises the resend path in replay");
}

/// One command in flight at a time, and the queue must actually FLOW:
/// consecutive emissions sit at least one tick apart (the ack is the
/// previous tick's receipt echo, never the same instant), and the
/// corpus-wide throughput is pinned. The pin is the anti-wedge
/// tripwire: when this replay briefly waited on echoes the capture
/// could never supply, the gate wedged on its first emission and
/// throughput fell 1818 -> 86 while every per-file assertion still
/// passed — vacuously. The serialization discipline itself is pinned
/// against real acks in `tests/farm.rs` and against the echoing
/// fixture server in `tests/farm_live.rs`.
#[test]
fn emissions_flow_one_per_ack_and_never_wedge() {
    let mut total = 0usize;
    for (path, run) in runs() {
        total += run.emitted.len();
        for pair in run.emitted.windows(2) {
            assert!(
                pair[1].0 > pair[0].0,
                "{}: two emissions in one tick: {pair:?}",
                path.display()
            );
        }
    }
    // 1857 -> 1947 when the five acceptance runs joined the corpus;
    // -> 1962 when the merge brought slice-8's 13 expedition raws in.
    assert_eq!(total, 1962, "corpus gate throughput changed");
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

/// The stop must never be called finished while the board's own room
/// block still lists something the bot would swing at — the failure that
/// this whole mechanism replaced, asserted across every transcript
/// rather than on one hand-built fixture.
#[test]
fn never_leaves_a_stop_with_a_target_still_listed() {
    let offenders: Vec<_> = runs()
        .into_iter()
        .filter(|(_, r)| r.left_with_a_target_listed > 0)
        .map(|(p, r)| {
            format!(
                "{}: left {} time(s) with a target listed",
                p.file_name().unwrap().to_string_lossy(),
                r.left_with_a_target_listed
            )
        })
        .collect();
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// A stop is only ever finished on evidence. "No room block has been
/// accepted" is not evidence of an empty room, however long the board
/// has been quiet — which is exactly the inference this replaced.
#[test]
fn never_leaves_a_stop_it_never_looked_at() {
    let offenders: Vec<_> = runs()
        .into_iter()
        .filter(|(_, r)| r.left_without_ever_looking > 0)
        .map(|(p, _)| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// The tripwire. Both assertions above are satisfied trivially by a
/// corpus in which the bot is never shown a monster, so pin that the
/// replay really does exercise occupied rooms.
#[test]
fn the_corpus_shows_the_bot_occupied_rooms() {
    let total: usize = runs().iter().map(|(_, r)| r.blocks_with_a_target).sum();
    // Pinned rather than merely non-zero, in the same spirit as the
    // file count above: if a change to the bot's targeting quietly stops
    // it seeing monsters, the two assertions above would start passing
    // for the wrong reason and nothing else would say so.
    // 393 -> 394 when the parser learned to open a room block whose name
    // is glued to a redrawn prompt on one physical line (the busy-room
    // render); one such block in the corpus lists a target. -> 399 when
    // the merge brought slice-8's expedition raws in (five more occupied
    // blocks across the gang and slime-lair sessions).
    assert_eq!(
        total, 399,
        "room blocks listing an attackable target changed; if that is \
         intended, update the number — but check the stop assertions \
         above are still doing work"
    );
}
