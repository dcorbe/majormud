//! Replay a capture's board output through the parser and the bot, and
//! flag command loops.
//!
//! The combat core keeps being patched for a new way the board can end
//! or continue a fight, and each miss shows up live as the character
//! hammering the same command many times a second: the goblin/archer
//! ping-pong (cw-beef, 2026-09-13), the 88-copper look spam (cw-blueberry).
//! This is the regression net for that whole family. It feeds a recorded
//! board stream to a fresh [`Bot`] and reports the commands the bot would
//! send, so a checked-in fixture of a known-pathological sequence can
//! assert the bot does NOT loop on it, and `mmc replay` can point at a
//! live capture and say whether today's code would.
//!
//! Feeding a RECORDED stream is the whole trick. A transcript is not
//! interactive — once the bot decides differently the board's recorded
//! answers no longer match — so this does not simulate a fight. It tests
//! the bot's REACTION to a fixed, known-bad sequence, which is exactly
//! what a regression net needs and all the captures we have can supply.

use crate::bot::{is_kill_line, Bot, BotAction, BotConfig};
use crate::events::Event;
use crate::parse::Parser;

/// One command the bot emitted during a replay, tagged with the index of
/// the board event that provoked it. The index locates a finding back in
/// the event stream without carrying the events themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emitted {
    pub at_event: usize,
    pub command: String,
}

/// A run of commands drawn from a small set, emitted back to back — the
/// signature of a stuck bot. `commands` is the distinct set that
/// repeated (one for look spam, two for the attack ping-pong).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopFinding {
    /// Index into the emitted list where the run begins.
    pub start: usize,
    /// The board event that provoked the first command of the run.
    pub at_event: usize,
    /// How many commands the run spans.
    pub count: usize,
    /// The distinct commands that make it up, in first-seen order.
    pub commands: Vec<String>,
}

/// A replay's result: the commands the bot emitted, and the event
/// indices at which the game state ADVANCED. A loop is the bot repeating
/// itself while nothing advances, so the progress marks are what tell a
/// stuck bot from an ordinary lap that loots and kills its way down a
/// street. See [`find_loops`].
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub commands: Vec<Emitted>,
    /// Event indices where state moved on: a kill, or a move to a
    /// differently-named room. Sorted ascending.
    pub progress_at: Vec<usize>,
}

/// Replay board output `text` through a fresh bot, returning the commands
/// it would send and where the state advanced. `text` is the board's
/// side of a session: a `.raw` capture verbatim, or the RX lines of a
/// timing log (see [`board_output`]). The bot's commands are the
/// harness's own — the recorded TX is never consulted.
pub fn replay(text: &str, config: BotConfig) -> Trace {
    let mut parser = Parser::new();
    let mut bot = Bot::new(config);
    let mut events = parser.push(text);
    events.extend(parser.finish());
    let mut trace = Trace::default();
    let mut room: Option<String> = None;
    for (i, event) in events.iter().enumerate() {
        // A kill, or a step into a differently-named room, is the state
        // moving on. A repeated block of the same room is NOT progress —
        // that is exactly the still frame a stuck bot re-reads.
        let progressed = match event {
            Event::Line(line) => is_kill_line(line),
            Event::RoomSeen(view) => {
                let moved = room.as_deref().is_some_and(|n| n != view.name);
                room = Some(view.name.clone());
                moved
            }
            _ => false,
        };
        if progressed {
            trace.progress_at.push(i);
        }
        for action in bot.on_event(event) {
            if let BotAction::Send(command) = action {
                trace.commands.push(Emitted { at_event: i, command });
            }
        }
    }
    trace
}

/// The board's side of a capture, as text the parser can consume. A
/// timing log (`<ts> RX <text>` / `<ts> TX <text>` lines) yields its RX
/// lines rejoined with newlines; anything else — a `.raw` byte capture —
/// is returned unchanged. A timing log has had its colour stripped, so a
/// `.raw` is the fuller input; both carry the occupant names a listed
/// monster is engaged from.
pub fn board_output(text: &str) -> String {
    let is_timing_log = text
        .lines()
        .take(50)
        .any(|l| line_after(l, " RX ").is_some() || line_after(l, " TX ").is_some());
    if !is_timing_log {
        return text.to_string();
    }
    let mut out = String::new();
    for line in text.lines() {
        if let Some(rx) = line_after(line, " RX ") {
            out.push_str(rx);
            out.push_str("\r\n");
        }
    }
    out
}

/// The text after `marker` on a line that opens with a `<seconds>.<frac>`
/// timestamp and then `marker`. `None` when the line is not that shape,
/// so a body line that merely contains " RX " is not mistaken for a
/// timing-log entry.
fn line_after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let (stamp, rest) = line.split_once(marker)?;
    let stamp = stamp.trim();
    (!stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit() || b == b'.')).then_some(rest)
}

/// Flag runs of at least `min_run` back-to-back commands drawn from a set
/// of at most `max_distinct` distinct values — a bot sending the same one
/// or two commands over and over with nothing else between. Normal play
/// varies (`a rat`, then rounds with no command, then `a bat`), so it
/// never fills a long run from a tiny set; a stuck bot does nothing else.
///
/// Runs are maximal and non-overlapping: each reported finding is
/// extended as far as the distinct cap allows before the scan moves past
/// it, so one stuck stretch is one finding, not many.
pub fn detect_loops(emitted: &[Emitted], min_run: usize, max_distinct: usize) -> Vec<LoopFinding> {
    let mut findings = Vec::new();
    let mut start = 0;
    while start < emitted.len() {
        let mut distinct: Vec<&str> = Vec::new();
        let mut end = start;
        while end < emitted.len() {
            let cmd = emitted[end].command.as_str();
            if !distinct.contains(&cmd) {
                if distinct.len() == max_distinct {
                    break;
                }
                distinct.push(cmd);
            }
            end += 1;
        }
        let count = end - start;
        if count >= min_run {
            findings.push(LoopFinding {
                start,
                at_event: emitted[start].at_event,
                count,
                commands: distinct.iter().map(|s| s.to_string()).collect(),
            });
            start = end;
        } else {
            start += 1;
        }
    }
    findings
}

/// Loops in a whole replay, counting only runs the bot emits WITHOUT the
/// state advancing between them. The command stream is cut at every
/// progress mark (a kill, a room change) and [`detect_loops`] is run on
/// each piece, so an ordinary lap that alternates `get silver` and
/// `bs thug` — a kill between every pair — never reads as a loop, while
/// the Combat Off ping-pong, which kills nothing as it spins, does.
pub fn find_loops(trace: &Trace, min_run: usize, max_distinct: usize) -> Vec<LoopFinding> {
    let mut out = Vec::new();
    let mut seg_start = 0;
    for i in 0..trace.commands.len() {
        let cut = i + 1 == trace.commands.len() || {
            // Did the state advance between this command and the next?
            let after = trace.commands[i].at_event;
            let upto = trace.commands[i + 1].at_event;
            trace
                .progress_at
                .iter()
                .any(|&p| p > after && p <= upto)
        };
        if cut {
            let segment = &trace.commands[seg_start..=i];
            for mut finding in detect_loops(segment, min_run, max_distinct) {
                finding.start += seg_start;
                out.push(finding);
            }
            seg_start = i + 1;
        }
    }
    out
}

/// The default loop thresholds `mmc replay` uses: a run of six or more
/// commands from at most two distinct values, all with no kill or room
/// change between them. Six clears the longest honest no-progress streak
/// (a backstab opener's `bs` then `eq`, or a couple of retries) while
/// catching every live loop, which ran to the hundreds; two covers both
/// the single-command look spam and the two-command attack ping-pong.
pub const DEFAULT_MIN_RUN: usize = 6;
pub const DEFAULT_MAX_DISTINCT: usize = 2;

