//! The Correlator answers "which command does this event answer?" — by
//! echo, FIFO order, and a reply grammar. Never by prompt, count, or
//! "whatever line came next".
//!
//! The rules were measured, not designed (docs/board-correlation.md; the
//! stopstate-run5/6/7 live captures, transcribed here because the raw
//! captures carry a live password and stay out of the repo):
//!
//! - The board echoes every ACCEPTED command. On a busy board a command
//!   echoes TWICE: a receipt echo ~2ms after send, and an execution echo
//!   immediately before its reply when commands queue behind the round
//!   timer. Post-parse the two are indistinguishable — both `Line(cmd)`
//!   — so a duplicate echo must re-confirm, never advance.
//! - Replies are strictly FIFO. Echo text identifies nothing (four
//!   pipelined `n` produced five `n` echo occurrences); order does.
//! - The board talks constantly without being asked: swing announces
//!   ("The fierce kobold thief lunges at you...!"), spawn wordings the
//!   classifier doesn't cover, "You hear movement...", exp and loot
//!   lines. None of that may retire or attribute — run6 127-131s shows a
//!   room block landing on the WRONG movement command if it does.
//!   Retirement therefore needs positive evidence: an event the pending
//!   command's reply grammar expects. Unknown lines answer nothing.
//! - No echo means unsolicited (`answers: None`), and it must never
//!   satisfy anyone's pending step. A wrong association is worse than no
//!   association: everything ambiguous falls to the deadline.

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
    Event::Prompt { hp: 45, mana: Some(8) }
}

fn line(l: &str) -> Event {
    Event::Line(l.to_string())
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
    assert!(is_echo(" look ", "look")); // wire-trimmed
    assert!(!is_echo("s", "n"));
    assert!(!is_echo("looked", "look"));
    assert!(!is_echo("", "n"));
}

#[test]
fn a_status_decoration_on_the_echo_is_stripped() {
    // The board splices "(Resting) " into lines it emits while the
    // character rests (DLL 0xe06f6); other markers are uncaptured, so any
    // single leading parenthesized run strips.
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
    assert!(is_echo("(Resting) ook", "look")); // decorated fragment
    assert!(!is_echo("l", "look")); // prefix, not suffix
    assert!(!is_echo("k", "look")); // fragment floor: len >= 2
    // A one-letter direction never suffix-matches a longer command: a
    // stray `n` line must not read as the tail of `open n`.
    assert!(!is_echo("n", "open n"));
    // Commands shorter than 3 chars only match exactly.
    assert!(!is_echo("w", "nw"));
    assert!(is_echo("ook", "ook")); // equal length: the exact path
}

// ------------------------------------------------------------- acceptance

#[test]
fn the_echo_line_answers_the_command_it_echoes() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    // The echo itself is attributed: it is the board saying "accepted",
    // and the Gate acks on exactly this.
    assert_eq!(ans(&mut c, line("look"), t), Some(CmdId(1)));
}

#[test]
fn a_room_block_answers_the_echoed_command_and_retires_it() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    ans(&mut c, line("look"), t);
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
fn a_decorated_echo_still_answers() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    assert_eq!(ans(&mut c, line("(Resting) look"), t), Some(CmdId(1)));
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), Some(CmdId(1)));
}

#[test]
fn a_split_echo_still_answers_its_command() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    assert_eq!(ans(&mut c, line("l"), t), None); // prefix fragment: unknown line
    assert_eq!(ans(&mut c, line("The cave bear swipes at you!"), t), None);
    assert_eq!(ans(&mut c, line("ook"), t), Some(CmdId(1)));
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
    assert_eq!(ans(&mut c, line("look"), t), Some(CmdId(2)));
    assert_eq!(ans(&mut c, room("Small Cavern"), t), Some(CmdId(2)));
    assert_eq!(ans(&mut c, room("Small Cavern"), t), None);
}

// ---------------------------------------------------- the unsolicited din

#[test]
fn unknown_lines_answer_nothing_and_retire_nothing() {
    // The stray vocabulary is large and constant: swing announces, spawn
    // wordings the classifier misses, "You hear movement...", exp/loot
    // lines, death cries. If any of it retired the pending command, its
    // real reply would attribute one command late — run6 shows a room
    // block landing on the WRONG move exactly that way.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t);
    for stray in [
        "The fierce kobold thief lunges at you with their shortsword!",
        "You hear movement to the north.",
        "You gain 23 experience.",
        "7 silver drop to the ground.",
        "The giant rat falls to the ground with a shrill cry!",
        "The room is dimly lit",
        "(Resting)", // prompt-pileup residue: empty after decoration strip
    ] {
        assert_eq!(ans(&mut c, line(stray), t), None, "{stray:?} must not attribute");
    }
    // The step's real reply is still ours.
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(1)));
}

#[test]
fn classified_async_events_answer_nothing_and_retire_nothing() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    ans(&mut c, line("look"), t);
    let hit = Event::CombatHit {
        attacker: Actor::Other("The cave bear".into()),
        target: Actor::You,
        damage: 3,
    };
    assert_eq!(ans(&mut c, hit, t), None);
    let entered = Event::ActorEntered { name: "acid slime".into(), from: Some("east".into()) };
    assert_eq!(ans(&mut c, entered, t), None);
    for _ in 0..5 {
        assert_eq!(ans(&mut c, prompt(), t), None);
    }
    assert_eq!(ans(&mut c, room("Small Cavern"), t), Some(CmdId(1)));
}

// -------------------------------------------------------- reply grammar

#[test]
fn a_reply_retires_only_a_command_that_expects_it() {
    // `cast star` fails; `look` is answered by the dark line. The cast's
    // failure wording completes the cast, so the look's answer is not
    // stolen by a lingerer — and the dark line completes the look, not
    // the cast.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "cast star", t);
    ans(&mut c, line("cast star"), t);
    assert_eq!(
        ans(&mut c, line("You attempt to cast starlight, but fail."), t),
        Some(CmdId(1))
    );
    c.sent(CmdId(2), "look", t);
    ans(&mut c, line("look"), t);
    assert_eq!(
        ans(&mut c, line("The room is very dark - you can't see anything"), t),
        Some(CmdId(2))
    );
}

#[test]
fn a_lingerer_with_no_known_reply_is_skipped_not_credited() {
    // `health` has no reply grammar; it lingers until its deadline. The
    // block that answers the later step must skip over it — and its
    // retirement clears the lingerer too, because replies are FIFO: if
    // the step's answer arrived, everything older was answered or eaten.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "health", t);
    ans(&mut c, line("health"), t);
    c.sent(CmdId(2), "n", t);
    ans(&mut c, line("n"), t);
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(2)));
    // The lingerer died with the retirement; a late line cannot revive it.
    assert_eq!(ans(&mut c, line("Hits: 45/45 Mana: 8/8"), t), None);
}

#[test]
fn a_multi_line_reply_is_missed_never_wrong() {
    // `inventory` answers with two lines; neither is in any grammar. The
    // entry expires on its deadline — a missed association, which the
    // sender's own timeout absorbs. It must never poison a neighbor.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "inventory", t);
    ans(&mut c, line("inventory"), t);
    assert_eq!(ans(&mut c, line("You are carrying 600 copper farthings, gilded robes (Torso)"), t), None);
    assert_eq!(ans(&mut c, line("You have no keys."), t), None);
    c.sent(CmdId(2), "look", t);
    ans(&mut c, line("look"), t);
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), Some(CmdId(2)));
}

#[test]
fn the_look_door_refusal_answers_the_look_not_the_move() {
    // Two wordings, two meanings (OmegaMUD splits them; ours conflated
    // them): "The door is closed!" refuses a MOVE, "The door is closed in
    // that direction!" refuses a LOOK. The bang on the move wording is
    // load-bearing — the look wording must never read as a move refusal.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t);
    c.sent(CmdId(2), "look", t);
    ans(&mut c, line("look"), t);
    assert_eq!(
        ans(&mut c, line("The door is closed in that direction!"), t),
        Some(CmdId(2))
    );
    // FIFO cleanup took the older step with it: its reply already came or
    // never will.
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), None);
}

// ----------------------------------------------------- captured sequences

#[test]
fn run6_a_swing_announce_never_steals_a_room_block() {
    // stopstate-run6_timing.log 127-131s, fully faithful: two pipelined
    // `n`, a kobold swing announce between the first echo and its block,
    // then a third `n` answered by "no exit". Under a retire-on-any-line
    // rule the swing shifts attribution one command late and Small Cavern
    // lands on the WRONG move — the exact position poison this module
    // exists to kill.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);

    c.sent(CmdId(1), "n", t); // 127.665
    assert_eq!(ans(&mut c, prompt(), t), None);
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(1))); // 128.420
    c.sent(CmdId(2), "n", t); // 128.864
    assert_eq!(
        ans(&mut c, line("The fierce kobold thief lunges at you with their shortsword!"), t),
        None // 129.440 — nobody asked
    );
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(1))); // 129.446
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(2))); // 129.446, glued echo
    c.sent(CmdId(3), "n", t); // 130.065
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(3))); // 130.066 receipt
    assert_eq!(ans(&mut c, line("You hear movement to the north."), t), None); // 131.087
    assert_eq!(ans(&mut c, room("Small Cavern"), t), Some(CmdId(2))); // 131.088
    assert_eq!(ans(&mut c, line("The room is dimly lit"), t), None);
    assert_eq!(
        ans(&mut c, line("There is no exit in that direction!"), t),
        Some(CmdId(3)) // 131.094
    );
}

#[test]
fn run5_identical_texts_attribute_fifo_and_duplicates_confirm_not_advance() {
    // stopstate-run5_timing.log 516-528s, faithful including the async
    // slime lines: four pipelined `n` against a shut door, then open,
    // then bash. Five `n` echo occurrences for four `n` sends — the
    // execution echo re-echoes what the receipt echo already covered.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);

    c.sent(CmdId(1), "n", t); // 517.390
    assert_eq!(ans(&mut c, prompt(), t), None);
    assert_eq!(ans(&mut c, line("A acid slime oozes into the room from nowhere."), t), None);
    assert_eq!(ans(&mut c, line("The acid slime flails at you!"), t), None);
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(1))); // 517.757

    c.sent(CmdId(2), "n", t); // 518.589
    assert_eq!(ans(&mut c, line("The door is closed!"), t), Some(CmdId(1))); // 518.800
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(2))); // 518.805

    c.sent(CmdId(3), "n", t); // 519.789
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(3))); // 519.791 receipt
    assert_eq!(ans(&mut c, line("The door is closed!"), t), Some(CmdId(2))); // 520.291
    // 520.298: execution echo of n3 — a duplicate. Confirms, never
    // advances.
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(3)));

    c.sent(CmdId(4), "open n", t); // 520.989
    assert_eq!(ans(&mut c, line("open n"), t), Some(CmdId(4))); // receipt
    c.sent(CmdId(5), "bash n", t); // 522.190
    assert_eq!(ans(&mut c, line("bash n"), t), Some(CmdId(5))); // receipt

    // 522.740, three lines in one burst:
    assert_eq!(ans(&mut c, line("The door is closed!"), t), Some(CmdId(3)));
    assert_eq!(ans(&mut c, line("open n"), t), Some(CmdId(4))); // execution echo
    assert_eq!(ans(&mut c, line("The door is locked."), t), Some(CmdId(4)));

    c.sent(CmdId(6), "n", t); // 523.389
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(6))); // 523.390 receipt
    assert_eq!(ans(&mut c, line("bash n"), t), Some(CmdId(5))); // 523.750 execution
    assert_eq!(ans(&mut c, line("You bashed the door open."), t), Some(CmdId(5)));
    // 526.235 execution echo of the last n, 527.683 its block.
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(6)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(6)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), None); // retired
}

#[test]
fn run7_the_dark_line_answers_the_command_that_asked() {
    // stopstate-run7_timing.log 604-612s, faithful. The same wording
    // answers a MOVE (step landed, room is dark) and a LOOK (position
    // unchanged) — only attribution tells them apart, which is what makes
    // dead reckoning sound.
    const DARK: &str = "The room is very dark - you can't see anything";
    let t = Instant::now();
    let mut c = Correlator::new(TTL);

    c.sent(CmdId(1), "look", t); // 604.567
    assert_eq!(ans(&mut c, line("look"), t), Some(CmdId(1)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(1)));

    c.sent(CmdId(2), "n", t); // 605.767 — step into the dark
    assert_eq!(ans(&mut c, line("n"), t), Some(CmdId(2)));
    assert_eq!(ans(&mut c, line(DARK), t), Some(CmdId(2)));

    c.sent(CmdId(3), "look", t); // 607.047
    assert_eq!(ans(&mut c, line("look"), t), Some(CmdId(3)));
    assert_eq!(ans(&mut c, line(DARK), t), Some(CmdId(3)));

    c.sent(CmdId(4), "look", t); // 608.248
    assert_eq!(ans(&mut c, line("look"), t), Some(CmdId(4)));
    assert_eq!(ans(&mut c, line(DARK), t), Some(CmdId(4)));

    // 609.449 cast star: async combat lands BEFORE the echo and answers
    // nothing (live wordings — the parser classifies neither).
    c.sent(CmdId(5), "cast star", t);
    assert_eq!(
        ans(&mut c, line("The cave bear claws at you, but you dodge out of the way!"), t),
        None
    );
    assert_eq!(ans(&mut c, line("The giant rat lunges at you!"), t), None);
    assert_eq!(ans(&mut c, line("cast star"), t), Some(CmdId(5)));
    assert_eq!(
        ans(&mut c, line("You attempt to cast starlight, but fail."), t),
        Some(CmdId(5))
    );

    c.sent(CmdId(6), "look", t); // 610.648
    assert_eq!(ans(&mut c, line("look"), t), Some(CmdId(6)));
    assert_eq!(ans(&mut c, line(DARK), t), Some(CmdId(6)));

    c.sent(CmdId(7), "s", t); // 611.848 — step back out
    assert_eq!(ans(&mut c, line("s"), t), Some(CmdId(7)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(7)));
}

// ------------------------------------------------------ deadlines & flush

#[test]
fn an_entry_expires_on_its_deadline() {
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    let late = t + TTL + Duration::from_secs(1);
    // Expired before its echo: the echo text is now just a line.
    assert_eq!(ans(&mut c, line("look"), late), None);
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
    assert_eq!(ans(&mut c, line("n"), echo_at), Some(CmdId(1)));
    let reply_at = echo_at + TTL - Duration::from_secs(1);
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), reply_at), Some(CmdId(1)));
}

#[test]
fn queue_progress_keeps_a_waiting_command_alive() {
    // Between its receipt echo and its execution echo a queued command
    // hears nothing about itself — the board is grinding the round timer,
    // ~6s per queued round (run5: n6 receipt 523.390, execution 526.235).
    // Any acceptance or retirement is proof the queue is moving, so it
    // refreshes everyone still waiting.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t);
    c.sent(CmdId(2), "n", t + Duration::from_secs(1));
    // n2's receipt echo lands just before n1's TTL would lapse...
    let t9 = t + TTL - Duration::from_secs(1);
    assert_eq!(ans(&mut c, line("n"), t9), Some(CmdId(2)));
    // ...and n1's reply, arriving after n1's original deadline, is still
    // n1's: the acceptance refreshed it.
    let t12 = t + TTL + Duration::from_secs(2);
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t12), Some(CmdId(1)));
    // Retirement refreshed n2 the same way.
    let t20 = t12 + TTL - Duration::from_secs(1);
    assert_eq!(ans(&mut c, room("Small Cavern"), t20), Some(CmdId(2)));
}

#[test]
fn slow_down_flushes_everything_pending() {
    // Flood control DROPPED input; whether dropped commands still echo is
    // unverified, so the only safe response is to forget everything
    // pending and let the senders re-ask.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t);
    c.sent(CmdId(2), "look", t);
    assert_eq!(ans(&mut c, Event::SlowDown, t), None);
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), None);
    assert_eq!(ans(&mut c, line("look"), t), None);
}

#[test]
fn flush_forgets_everything_pending() {
    // Reconnect / phase-reset path.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look", t);
    ans(&mut c, line("look"), t);
    c.flush();
    assert_eq!(ans(&mut c, room("Small Cavern"), t), None);
}

// ------------------------------------------- grammar corners (review 2)

#[test]
fn a_monsters_failed_cast_does_not_answer_ours() {
    // DLL ships "%s attempted to cast %s at you, but failed." as routine
    // combat din — it must not retire OUR cast, whose failure wording is
    // "You attempt to cast %s, but fail." (both contain "but fail").
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "cast star", t);
    ans(&mut c, line("cast star"), t);
    assert_eq!(
        ans(&mut c, line("The acid slime attempted to cast starlight at you, but failed."), t),
        None
    );
    assert_eq!(
        ans(&mut c, line("You attempt to cast starlight, but fail."), t),
        Some(CmdId(1))
    );
}

#[test]
fn every_captured_cast_reply_completes_the_cast() {
    // stopstate-run3/4/5: success, already-cast, and no-mana all end a
    // cast; leaving any of them out makes two back-to-back casts
    // mis-attribute (oldest-matching would hand the second's failure to
    // the first).
    let t = Instant::now();
    for reply in [
        "You cast starlight!",
        "You have already cast a spell this round!",
        "You do not have enough mana to cast that spell.",
        "You attempt to cast starlight at the kobold, but the spell is resisted.",
    ] {
        let mut c = Correlator::new(TTL);
        c.sent(CmdId(1), "cast star", t);
        ans(&mut c, line("cast star"), t);
        assert_eq!(ans(&mut c, line(reply), t), Some(CmdId(1)), "{reply:?}");
        c.sent(CmdId(2), "cast star", t);
        ans(&mut c, line("cast star"), t);
        assert_eq!(
            ans(&mut c, line("You attempt to cast starlight, but fail."), t),
            Some(CmdId(2)),
            "lingerer stole the second cast's reply after {reply:?}"
        );
    }
}

#[test]
fn each_door_refusal_wording_answers_its_own_command() {
    // The ReMUD decompile pins the emitters: _cmd_look says "The door is
    // closed in that direction!"; _move_user says "There is a closed
    // door in that direction!" (exit-type case 2, beside "There is no
    // exit..."). Crossing them hands a post-move room to a look consumer
    // or leaves a refused move lingering to claim the next block.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t);
    c.sent(CmdId(2), "look", t);
    ans(&mut c, line("look"), t);
    assert_eq!(
        ans(&mut c, line("There is a closed door in that direction!"), t),
        Some(CmdId(1)) // refuses the MOVE
    );
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), Some(CmdId(2)));

    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look nw", t);
    ans(&mut c, line("look nw"), t);
    assert_eq!(
        ans(&mut c, line("The door is closed in that direction!"), t),
        Some(CmdId(1)) // refuses the LOOK
    );
}

#[test]
fn every_move_refusal_completes_the_move() {
    // _move_user's other refusals (DLL 0xbc745, 0xbc8c4): a refused move
    // that lingers claims the next look's block as a phantom arrival.
    let t = Instant::now();
    for refusal in [
        "You can't seem to move anywhere!",
        "You need to cast a spell to go that way!",
        "You are not permitted in that room!",
        "You are too evil to go through this exit!",
        "You are too good to go through this exit!",
        "You are too heavy to move!",
        "You are too stunned to move anywhere!",
        "You do not have the appropriate item to go that direction!",
        "You have progressed too far to go through this exit!",
        "You may not go through this exit!",
        "You may not go through this exit during tournament play!",
        "You may not pass through that exit at this point in time.",
        "You do not have enough to cover the toll of 100 copper farthings.",
        "You may not enter that room during a retaliation time-period.",
    ] {
        let mut c = Correlator::new(TTL);
        c.sent(CmdId(1), "n", t);
        ans(&mut c, line("n"), t);
        assert_eq!(ans(&mut c, line(refusal), t), Some(CmdId(1)), "{refusal:?}");
        c.sent(CmdId(2), "look", t);
        ans(&mut c, line("look"), t);
        assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), Some(CmdId(2)));
    }
}

#[test]
fn a_failed_bash_never_claims_the_next_room_block() {
    // stopstate-run1 170-174: "Your attempts to bash through fail!" — the
    // wording nav never knew, which is why doors "gave up". If it does
    // not retire the bash, the entry lingers and the next look's block
    // reads as the bash carrying us through a door that never opened:
    // position poison.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "bash n", t);
    ans(&mut c, line("bash n"), t);
    assert_eq!(
        ans(&mut c, line("Your attempts to bash through fail!"), t),
        Some(CmdId(1))
    );
    c.sent(CmdId(2), "look", t);
    ans(&mut c, line("look"), t);
    assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), Some(CmdId(2)));
}

#[test]
fn a_carried_through_bash_is_confirmed_by_the_line_and_retired_by_its_block() {
    // The other bash outcome (DLL 0xd538e) walks the character through:
    // the wording announces it, the block that follows IS the arrival.
    // The line must confirm (attribute, keep pending) so the block still
    // attributes to the bash — retiring on the line would make every
    // successful bash-through arrival read unsolicited.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "bash n", t);
    ans(&mut c, line("bash n"), t);
    assert_eq!(
        ans(&mut c, line("You bash the door open and walk through"), t),
        Some(CmdId(1))
    );
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(1)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), None); // retired
}

#[test]
fn open_light_get_and_heal_replies_complete_their_commands() {
    let t = Instant::now();
    for (cmd, reply) in [
        ("open n", "The door is now open."),
        ("open n", "The door was already open!"),
        ("open n", "The door is already open."),
        ("open n", "You successfully unlocked the gate."),
        ("light torch", "You lit the torch."),
        ("get silver", "You picked up 7 silver nobles."),
        ("buy healing", "You hand over nothing and all your wounds are healed."),
    ] {
        let mut c = Correlator::new(TTL);
        c.sent(CmdId(1), cmd, t);
        ans(&mut c, line(cmd), t);
        assert_eq!(ans(&mut c, line(reply), t), Some(CmdId(1)), "{cmd:?} <- {reply:?}");
        assert_eq!(ans(&mut c, room("Newhaven, Arena"), t), None, "{cmd:?} not retired");
    }
}

#[test]
fn constant_traffic_cannot_keep_a_dead_entry_alive_forever() {
    // Progress-refresh exists for round-timer waits, but it must not
    // defeat the deadline outright: a move whose block was eaten would
    // otherwise ride along under constant combat traffic and steal an
    // unsolicited block minutes later. A hard lifetime cap, never
    // refreshed, bounds the residual.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t); // accepted; its block will be eaten
    // Opaque traffic keeps the queue "progressing" every few seconds.
    let mut now = t;
    let mut id = 2;
    while now < t + TTL * 6 {
        now += Duration::from_secs(3);
        c.sent(CmdId(id), "a kobold", now);
        ans(&mut c, line("a kobold"), now);
        id += 1;
    }
    // Way past any honest wait: the stale step must be gone, so the
    // unsolicited block answers nobody.
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), now), None);
}

#[test]
fn a_confirmed_bash_owns_the_next_block_even_past_a_stale_move() {
    // The walk-through confirm is FIFO evidence too: the bash executing
    // means everything senior was answered or never will be. Without the
    // senior drain, a move whose block was eaten sits in front and
    // steals the confirmed bash's arrival — two wrong associations from
    // one eaten reply.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t); // accepted; its block will be eaten
    c.sent(CmdId(2), "bash n", t);
    ans(&mut c, line("bash n"), t);
    assert_eq!(
        ans(&mut c, line("You bash the door open and walk through"), t),
        Some(CmdId(2))
    );
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(2)));
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), None);
}

#[test]
fn a_locked_reply_belongs_to_the_open_even_past_a_stale_move() {
    // _move_user emits NO locked wording — a move at a locked door
    // answers "The door is closed!" (22+ corpus occurrences agree). "The
    // door is locked." is _cmd_open's reply. With the needle wrongly in
    // the Move arm, a move whose refusal was eaten would steal the
    // open's lock line and leave the open lingering: two wrong
    // associations from one eaten line, on the standard door workflow.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t); // refusal eaten
    c.sent(CmdId(2), "open n", t);
    ans(&mut c, line("open n"), t);
    assert_eq!(ans(&mut c, line("The door is locked."), t), Some(CmdId(2)));
}

#[test]
fn the_hide_refusal_does_not_complete_a_move() {
    // "You can't seem to move anywhere to hide!" (0xdb396) shares the
    // move refusal's prefix; the trailing bang on the needle keeps the
    // two apart.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t);
    assert_eq!(
        ans(&mut c, line("You can't seem to move anywhere to hide!"), t),
        None
    );
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(1)));
}

#[test]
fn a_resisting_target_completes_the_cast() {
    // Second-person resist replies (0xcffe5 / 0xd03e9): without them a
    // resisted cast lingers and the next cast's reply retires the stale
    // one — the shift-by-one chain, while casting fast.
    let t = Instant::now();
    for reply in ["The kobold resists your spell!", "kobold thief resists your spell!"] {
        let mut c = Correlator::new(TTL);
        c.sent(CmdId(1), "cast star", t);
        ans(&mut c, line("cast star"), t);
        assert_eq!(ans(&mut c, line(reply), t), Some(CmdId(1)), "{reply:?}");
    }
}

#[test]
fn light_refusal_and_cast_lit_success_complete_their_commands() {
    // "You already have something lit!" (0xdb396 cluster) ends a light
    // command; "You lit the %s." also answers a CAST of a light spell
    // (the cast routes through the light routine).
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "light torch", t);
    ans(&mut c, line("light torch"), t);
    assert_eq!(
        ans(&mut c, line("You already have something lit!"), t),
        Some(CmdId(1))
    );

    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "cast light", t);
    ans(&mut c, line("cast light"), t);
    assert_eq!(ans(&mut c, line("You lit the torch."), t), Some(CmdId(1)));
}

#[test]
fn the_bash_cooldown_refusal_completes_the_bash() {
    // "You must wait before you may do that!" (0xd53fd, bash routine).
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "bash n", t);
    ans(&mut c, line("bash n"), t);
    assert_eq!(
        ans(&mut c, line("You must wait before you may do that!"), t),
        Some(CmdId(1))
    );
}

#[test]
fn a_directed_look_with_no_exit_completes_the_look() {
    // _cmd_look: "There are no exits to the %s!" / upwards / downwards.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "look e", t);
    ans(&mut c, line("look e"), t);
    assert_eq!(
        ans(&mut c, line("There are no exits to the east!"), t),
        Some(CmdId(1))
    );
}

#[test]
fn cast_and_light_refusals_are_symmetric() {
    // A cast of a light spell routes through _cmd_light, so its refusal
    // branch answers the CAST too; and _cmd_light's other refusal ends a
    // light command. The stunned needle keeps its bang discipline: the
    // hide routine ships "You are too stunned to move anywhere to
    // hide!", which must not complete a move.
    let t = Instant::now();
    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "cast light", t);
    ans(&mut c, line("cast light"), t);
    assert_eq!(
        ans(&mut c, line("You already have something lit!"), t),
        Some(CmdId(1))
    );

    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "light torch", t);
    ans(&mut c, line("light torch"), t);
    assert_eq!(
        ans(&mut c, line("You may not light that item!"), t),
        Some(CmdId(1))
    );

    let mut c = Correlator::new(TTL);
    c.sent(CmdId(1), "n", t);
    ans(&mut c, line("n"), t);
    assert_eq!(
        ans(&mut c, line("You are too stunned to move anywhere to hide!"), t),
        None
    );
    assert_eq!(ans(&mut c, room("Dungeon, Entrance"), t), Some(CmdId(1)));
}
