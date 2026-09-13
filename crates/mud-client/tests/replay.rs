//! The replay harness: does a recorded board stream make the bot loop?
//!
//! Two jobs are tested apart. The detector must fire on a looping command
//! stream and stay quiet on an ordinary one, and the whole pipeline
//! (parse -> bot -> detect) must find no loop in the very sequence that
//! spun a character live. The second is the regression net: if the
//! combat state ever forgets that a target switch is not a fight ending,
//! this replay starts emitting the ping-pong and the test fails.

use mud_client::bot::BotConfig;
use mud_client::replay::{
    board_output, detect_loops, find_loops, replay, Emitted, Trace, DEFAULT_MAX_DISTINCT,
    DEFAULT_MIN_RUN,
};

fn emitted(commands: &[&str]) -> Vec<Emitted> {
    commands
        .iter()
        .enumerate()
        .map(|(i, c)| Emitted {
            at_event: i,
            command: c.to_string(),
        })
        .collect()
}

/// The detector fires on the two-command ping-pong: `a goblin` and
/// `a archer` alternating past the run threshold.
#[test]
fn the_detector_flags_an_alternating_ping_pong() {
    let stream = emitted(&[
        "a archer", "a goblin", "a archer", "a goblin", "a archer", "a goblin", "a archer",
    ]);
    let found = detect_loops(&stream, DEFAULT_MIN_RUN, DEFAULT_MAX_DISTINCT);
    assert_eq!(found.len(), 1, "one loop: {found:?}");
    assert_eq!(found[0].count, 7);
    assert_eq!(found[0].commands, vec!["a archer", "a goblin"]);
}

/// And on the single-command look spam.
#[test]
fn the_detector_flags_a_single_command_spam() {
    let stream = emitted(&["look"; 20]);
    let found = detect_loops(&stream, DEFAULT_MIN_RUN, DEFAULT_MAX_DISTINCT);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].count, 20);
    assert_eq!(found[0].commands, vec!["look"]);
}

/// Ordinary combat varies — a swing, then rounds with no command, then
/// the next monster's swing — so it never fills a long run from a tiny
/// set. Nothing is flagged.
#[test]
fn ordinary_combat_is_not_flagged() {
    let stream = emitted(&["a rat", "a bat", "a wolf", "look", "a shade", "a wight", "a bear"]);
    let found = detect_loops(&stream, DEFAULT_MIN_RUN, DEFAULT_MAX_DISTINCT);
    assert!(found.is_empty(), "false positive: {found:?}");
}

fn combat_config() -> BotConfig {
    BotConfig {
        auto_combat: true,
        ..BotConfig::default()
    }
}

/// The pipeline end to end on the sequence that spun cw-beef: a room
/// with two kinds of monster, then the board's target-switch answer
/// (`*Combat Off*` then `*Combat Engaged*`) and the same block again,
/// over and over. Today's bot engages once and holds; it must not emit
/// the ping-pong. Feed the same fixture to a bot that reads the Off as a
/// fight ending and it alternates `a archer`/`a goblin` — which is the
/// regression this test guards.
#[test]
fn the_beef_ping_pong_fixture_does_not_loop_today() {
    // The board's side only: a block, then repeated switch-pair-plus-block
    // cycles, exactly the shape the capture recorded. The room name opens
    // in 1;36 and the exits in 0;32, the colours the parser keys a block
    // on; without them no RoomSeen is produced and the bot never engages,
    // which would make this test pass on nothing.
    let block = "\r\n\x1b[1;36mDarkwood Forest\r\n\
        Also here: dark goblin archer, short dark goblin.\r\n\
        \x1b[0;32mObvious exits: north\r\n[HP=97]:";
    let mut raw = String::from(block);
    for _ in 0..8 {
        raw.push_str("a archer\r\n*Combat Off*\r\n*Combat Engaged*\r\n[HP=97]:look");
        raw.push_str(block);
    }

    let trace = replay(&raw, combat_config());
    let found = find_loops(&trace, DEFAULT_MIN_RUN, DEFAULT_MAX_DISTINCT);
    assert!(
        found.is_empty(),
        "the bot looped on the target-switch fixture: {:?} -> {found:?}",
        trace.commands,
    );
    // It should engage, once, and then hold the fight rather than re-ask.
    let attacks = trace.commands.iter().filter(|c| c.command.starts_with("a ")).count();
    assert!(
        attacks <= 1,
        "expected a single engage, got {attacks}: {:?}",
        trace.commands,
    );
}

/// A timing log's RX lines are rejoined for replay; a raw capture is left
/// as it is. A body line that merely contains " RX " is not mistaken for
/// a timing-log entry, because the split needs a timestamp before it.
#[test]
fn board_output_reads_a_timing_log_or_a_raw_capture() {
    let timing = "1789285820.260 RX Darkwood Forest\n\
        1789285820.262 TX look\n\
        1789285820.321 RX Also here: dark goblin archer.\n";
    let got = board_output(timing);
    assert!(got.contains("Darkwood Forest"));
    assert!(got.contains("Also here: dark goblin archer."));
    assert!(!got.contains("look"), "TX lines are dropped: {got:?}");

    let raw = "Darkwood Forest\r\nObvious exits: north\r\n[HP=97]:";
    assert_eq!(board_output(raw), raw, "a raw capture passes through");
}

/// find_loops does not flag an ordinary lap: `get silver` then `bs thug`
/// alternating, with a kill (a progress mark) between every pair. The
/// stream is cut at each mark, so no piece is long enough to be a loop.
#[test]
fn a_lap_that_loots_and_kills_is_not_a_loop() {
    let commands = emitted(&[
        "bs thug", "get silver", "bs thug", "get silver", "bs thug", "get silver", "bs thug",
        "get silver",
    ]);
    // A kill lands between each command (event indices climb by one per
    // command in this fixture, so a mark sits on every boundary).
    let progress_at = (0..commands.len()).collect();
    let trace = Trace {
        commands,
        progress_at,
    };
    let found = find_loops(&trace, DEFAULT_MIN_RUN, DEFAULT_MAX_DISTINCT);
    assert!(found.is_empty(), "flagged an ordinary lap: {found:?}");
}

/// But the same alternation with NO progress between is a loop: the bot
/// is repeating itself while nothing advances.
#[test]
fn the_same_alternation_with_no_progress_is_a_loop() {
    let commands = emitted(&[
        "bs thug", "get silver", "bs thug", "get silver", "bs thug", "get silver",
    ]);
    let trace = Trace {
        commands,
        progress_at: Vec::new(),
    };
    let found = find_loops(&trace, DEFAULT_MIN_RUN, DEFAULT_MAX_DISTINCT);
    assert_eq!(found.len(), 1, "missed a no-progress loop: {found:?}");
    assert_eq!(found[0].count, 6);
}

