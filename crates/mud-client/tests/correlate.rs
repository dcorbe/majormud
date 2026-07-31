//! The Correlator answers "which command does this event answer?" — by
//! echo and FIFO order, never by prompt or count.
//!
//! The rules it must satisfy were measured, not designed (see
//! docs/board-correlation.md and ~/mmc-probe/stopstate-run5_timing.log,
//! which is transcribed into fixtures here because the capture itself
//! carries a live password and stays out of the repo):
//!
//! - The board echoes every ACCEPTED command. On a busy board a command
//!   echoes TWICE: a receipt echo ~2ms after send, and an execution echo
//!   immediately before its reply when commands queue behind the round
//!   timer. Idle, the two coincide. Post-parse the two are
//!   indistinguishable — both are `Line(cmd)` — so a duplicate echo must
//!   re-confirm the entry, never advance the queue.
//! - Replies are strictly FIFO. Order attributes; echo text alone never
//!   does (four pipelined `n` produced five `n` echo occurrences).
//! - No echo means unsolicited: the login render, a summon, a respawn.
//!   `answers: None`, and it must never satisfy anyone's pending step.
//! - A wrong association is worse than none. Anything ambiguous falls to
//!   the deadline and the consumer re-asks.

use std::time::{Duration, Instant};

use mud_client::correlate::{is_echo, CmdId, Correlator};
use mud_client::events::{Actor, Event, RoomView};

const TTL: Duration = Duration::from_secs(10);

fn room(name: &str) -> Event {
    Event::RoomSeen(RoomView {
        name: name.to_string(),
        ..Default::default()
    })
}

fn prompt() -> Event {
    Event::Prompt { hp: 43, mana: Some(6) }
}

fn miss(line: &str) -> Event {
    Event::CombatMiss { line: line.to_string() }
}

/// Feed one event, return who it answered.
fn ans(c: &mut Correlator, ev: Event, now: Instant) -> Option<CmdId> {
    c.on_event(ev, now).answers
}

// ---------------------------------------------------------------- is_echo

#[test]
fn an_echo_is_the_exact_text_we_sent() {
    assert!(is_echo("n", "n"));
    assert!(is_echo("look", "look"));
    assert!(is_echo("open n", "open n"));
    assert!(!is_echo("s", "n"));
    assert!(!is_echo("looked", "look"));
    assert!(!is_echo("", "n"));
}

#[test]
fn a_status_decoration_on_the_echo_is_stripped() {
    // The board splices "(Resting) " into lines it emits while the
    // character rests (DLL 0xe06f6); other markers are uncaptured, so any
    // single leading parenthesized token strips.
    assert!(is_echo("(Resting) look", "look"));
    assert!(is_echo("(Hiding) n", "n"));
    assert!(!is_echo("(Resting) s", "n"));
}

#[test]
fn a_split_echo_matches_by_proper_suffix_with_length_floors() {
    // ~0.4% of echoes split when async output lands mid-echo: `look`
    // arrives as `l` + <async line> + `ook`. The suffix fragment matches;
    // the prefix fragment does not (the suffix will).
    assert!(is_echo("ook", "look"));
    assert!(!is_echo("l", "look")); // prefix, not suffix
    assert!(!is_echo("k", "look")); // fragment floor: len >= 2
    // A one-letter direction never suffix-matches a longer command: a
    // stray `n` line must not read as the tail of `open n`.
    assert!(!is_echo("n", "open n"));
    // Commands shorter than 3 chars only match exactly.
    assert!(!is_echo("w", "nw"));
}

// ------------------------------------------------------------- Correlator

#[test]
fn the_echo_line_answers_the_command_it_echoes() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    // The echo itself is attributed: it is the board saying "accepted",
    // and the Gate acks on exactly this.
    assert_eq!(ans(&mut c, Event::Line("look".into()), t), Some(CmdId(1)));
}

#[test]
fn a_room_block_answers_the_echoed_command_and_retires_it() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    ans(&mut c, Event::Line("look".into()), t);
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), Some(CmdId(1)));
    // Retired: the next block is nobody's answer.
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), None);
}

#[test]
fn a_block_with_no_echo_is_unsolicited() {
    // The login render, a summon, a respawn: nothing pending, or pending
    // but not yet echoed — either way `answers` must be None.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), None);
    c.sent(CmdId(1), "n", t);
    // Sent but NOT echoed: a block now is somebody else's render, not our
    // step landing. This is the desync bug, dead.
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), None);
}

#[test]
fn prompts_answer_nothing_and_retire_nothing() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    ans(&mut c, Event::Line("look".into()), t);
    for _ in 0..5 {
        assert_eq!(ans(&mut c, prompt(), t), None);
    }
    // Still pending: the block after the prompt burst is still ours.
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), Some(CmdId(1)));
}

#[test]
fn classified_async_events_answer_nothing_and_retire_nothing() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    ans(&mut c, Event::Line("look".into()), t);
    let hit = Event::CombatHit {
        attacker: Actor::Other("The cave bear".into()),
        target: Actor::You,
        damage: 3,
    };
    assert_eq!(ans(&mut c, hit, t), None);
    assert_eq!(ans(&mut c, miss("The cave bear swipes at you!"), t), None);
    let entered = Event::ActorEntered { name: "acid slime".into(), from: Some("nowhere".into()) };
    assert_eq!(ans(&mut c, entered, t), None);
    assert_eq!(ans(&mut c, room("Small Cavern"), t), Some(CmdId(1)));
}

#[test]
fn an_unknown_line_is_a_single_line_reply_and_retires_the_head() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "cast star", t);
    ans(&mut c, Event::Line("cast star".into()), t);
    assert_eq!(
        ans(&mut c, Event::Line("You attempt to cast starlight, but fail.".into()), t),
        Some(CmdId(1))
    );
    // Retired: the next command's answer is not stolen by the lingerer.
    c.sent(CmdId(2), "look", t);
    ans(&mut c, Event::Line("look".into()), t);
    assert_eq!(
        ans(&mut c, Event::Line("The room is very dark - you can't see anything".into()), t),
        Some(CmdId(2))
    );
}

#[test]
fn identical_texts_attribute_fifo_and_duplicates_confirm_not_advance() {
    // Transcribed from stopstate-run5_timing.log 516-528s: four pipelined
    // `n` against a shut door, then open, then bash. Five `n` echo
    // occurrences for four `n` sends — the execution echo re-echoes the
    // command the receipt echo already covered. Replies are FIFO.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);

    c.sent(CmdId(1), "n", t); // 517.390
    assert_eq!(ans(&mut c, prompt(), t), None);
    assert_eq!(ans(&mut c, Event::Line("n".into()), t), Some(CmdId(1))); // 517.757

    c.sent(CmdId(2), "n", t); // 518.589
    assert_eq!(
        ans(&mut c, Event::Line("The door is closed!".into()), t),
        Some(CmdId(1)) // 518.800 answers the FIRST n
    );
    assert_eq!(ans(&mut c, Event::Line("n".into()), t), Some(CmdId(2))); // 518.805

    c.sent(CmdId(3), "n", t); // 519.789
    assert_eq!(ans(&mut c, Event::Line("n".into()), t), Some(CmdId(3))); // 519.791 receipt
    assert_eq!(
        ans(&mut c, Event::Line("The door is closed!".into()), t),
        Some(CmdId(2)) // 520.291 answers the SECOND n, even though n3 already echoed
    );
    // 520.298: execution echo of n3 — a duplicate. It confirms n3, it
    // does not advance past it.
    assert_eq!(ans(&mut c, Event::Line("n".into()), t), Some(CmdId(3)));

    c.sent(CmdId(4), "open n", t); // 520.989
    assert_eq!(ans(&mut c, Event::Line("open n".into()), t), Some(CmdId(4))); // receipt
    c.sent(CmdId(5), "bash n", t); // 522.190
    assert_eq!(ans(&mut c, Event::Line("bash n".into()), t), Some(CmdId(5))); // receipt

    // 522.740, three lines in one burst:
    assert_eq!(
        ans(&mut c, Event::Line("The door is closed!".into()), t),
        Some(CmdId(3)) // the third n's reply
    );
    assert_eq!(ans(&mut c, Event::Line("open n".into()), t), Some(CmdId(4))); // execution echo
    assert_eq!(
        ans(&mut c, Event::Line("The door is locked.".into()), t),
        Some(CmdId(4)) // open's reply
    );

    c.sent(CmdId(6), "n", t); // 523.389
    assert_eq!(ans(&mut c, Event::Line("n".into()), t), Some(CmdId(6))); // 523.390 receipt
    assert_eq!(ans(&mut c, Event::Line("bash n".into()), t), Some(CmdId(5))); // 523.750 execution
    assert_eq!(
        ans(&mut c, Event::Line("You bashed the door open.".into()), t),
        Some(CmdId(5)) // 523.753
    );
    // 526.235 execution echo of the last n, 527.683 its block.
    assert_eq!(ans(&mut c, Event::Line("n".into()), t), Some(CmdId(6)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(6)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), None); // retired
}

#[test]
fn the_dark_line_answers_the_command_that_asked() {
    // Transcribed from stopstate-run7_timing.log 604-612s. The same
    // wording answers a MOVE (step landed, room is dark) and a LOOK
    // (position unchanged) — only attribution tells them apart, which is
    // what makes dead reckoning sound.
    const DARK: &str = "The room is very dark - you can't see anything";
    let t = Instant::now();
    let mut c = Correlator::new(TTL);

    c.sent(CmdId(1), "n", t); // 605.767 — step into the dark
    assert_eq!(ans(&mut c, Event::Line("n".into()), t), Some(CmdId(1)));
    assert_eq!(ans(&mut c, Event::Line(DARK.into()), t), Some(CmdId(1)));

    c.sent(CmdId(2), "look", t); // 607.047
    assert_eq!(ans(&mut c, Event::Line("look".into()), t), Some(CmdId(2)));
    assert_eq!(ans(&mut c, Event::Line(DARK.into()), t), Some(CmdId(2)));

    // 609.449 cast star: async combat lands BEFORE the echo and answers
    // nothing; the failure line lands after it and answers the cast.
    c.sent(CmdId(3), "cast star", t);
    assert_eq!(ans(&mut c, miss("The cave bear claws at you, but you dodge out of the way!"), t), None);
    assert_eq!(ans(&mut c, Event::Line("cast star".into()), t), Some(CmdId(3)));
    assert_eq!(
        ans(&mut c, Event::Line("You attempt to cast starlight, but fail.".into()), t),
        Some(CmdId(3))
    );

    c.sent(CmdId(4), "s", t); // 611.848 — step back out
    assert_eq!(ans(&mut c, Event::Line("s".into()), t), Some(CmdId(4)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(4)));
}

#[test]
fn a_split_echo_still_answers_its_command() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    // `look` reaches the parser as `l` + async + `ook` (~0.4% live). The
    // prefix fragment is an unknown line — with nothing echoed yet it
    // answers nothing and retires nothing.
    assert_eq!(ans(&mut c, Event::Line("l".into()), t), None);
    assert_eq!(ans(&mut c, miss("The cave bear swipes at you!"), t), None);
    assert_eq!(ans(&mut c, Event::Line("ook".into()), t), Some(CmdId(1)));
    assert_eq!(ans(&mut c, room("Small Cavern"), t), Some(CmdId(1)));
}

#[test]
fn an_echo_for_a_later_command_drops_earlier_unechoed_entries() {
    // Echoes arrive in send order. If the echo of a LATER send appears
    // while an earlier send was never echoed, the earlier one was eaten
    // (flood control, disconnect race) and will never be answered —
    // holding it would misdirect FIFO attribution forever.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "get copper", t);
    c.sent(CmdId(2), "look", t);
    assert_eq!(ans(&mut c, Event::Line("look".into()), t), Some(CmdId(2)));
    assert_eq!(ans(&mut c, room("Small Cavern"), t), Some(CmdId(2)));
    // The eaten command's echo never comes; a late matching line is now
    // an unknown line, not a resurrection.
    assert_eq!(ans(&mut c, room("Small Cavern"), t), None);
}

#[test]
fn an_entry_expires_on_its_deadline() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    let late = t + TTL + Duration::from_secs(1);
    // Expired before its echo: the echo text is now just a line.
    assert_eq!(ans(&mut c, Event::Line("look".into()), late), None);
    assert_eq!(ans(&mut c, room("Small Cavern"), late), None);
}

#[test]
fn the_echo_refreshes_the_deadline_while_the_reply_streams() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    // The board sat on the command behind the round timer, then echoed at
    // execution: acceptance restarts the clock, so the reply that follows
    // the late echo still attributes.
    let echo_at = t + TTL - Duration::from_secs(1);
    assert_eq!(ans(&mut c, Event::Line("n".into()), echo_at), Some(CmdId(1)));
    let reply_at = echo_at + TTL - Duration::from_secs(1);
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), reply_at), Some(CmdId(1)));
}

#[test]
fn slow_down_flushes_everything_pending() {
    // Flood control DROPPED input; whether dropped commands still echo is
    // unverified, so the only safe response is to forget everything
    // pending and let the senders re-ask.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, Event::Line("n".into()), t);
    c.sent(CmdId(2), "look", t);
    assert_eq!(ans(&mut c, Event::SlowDown, t), None);
    // Neither the echoed step nor the un-echoed look survives.
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), None);
    assert_eq!(ans(&mut c, Event::Line("look".into()), t), None);
}

#[test]
fn flush_forgets_everything_pending() {
    // Reconnect / phase-reset path.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    ans(&mut c, Event::Line("look".into()), t);
    c.flush();
    assert_eq!(ans(&mut c, room("Small Cavern"), t), None);
}

#[test]
fn a_decorated_echo_still_answers() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    assert_eq!(ans(&mut c, Event::Line("(Resting) look".into()), t), Some(CmdId(1)));
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), Some(CmdId(1)));
}
