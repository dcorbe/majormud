//! The board says which command each room block answers, and we can read
//! it. This pins that, because a permanent fix for position tracking
//! rests on it.
//!
//! MajorMUD echoes every line it is sent, immediately after the prompt
//! that accepted it, and the reply follows the echo:
//!
//! ```text
//! [HP=51/MA=10]:d          <- the echo of our command
//! Newhaven, Arena          <- the block that answers it
//! ...
//! Obvious exits: ...
//! ```
//!
//! The client has never used this. `Gate` acknowledges a command on the
//! next PROMPT, but prompts arrive in bursts and the board also sends
//! them unsolicited when async output disturbs a dangling one — so a
//! prompt is not evidence that OUR command was answered. `Navigator`
//! verifies a step by taking the next room block it sees, whoever asked
//! for it. Between them, a `look` sent before a walk begins can have its
//! block satisfy the walk's first step, and the walk then believes it is
//! a room further on than the character is. That is the desync that ends
//! live runs.
//!
//! OmegaMUD (docs/mirrors/github-RonPenton-OmegaMUD) does not have this
//! problem, and its `RoomParseState` says why: it parses the ANSI control
//! stream and tracks what it asked. The room render is introduced by
//! Reset -> CursorBackward(79) -> EraseLine -> RoomNameColour, verbatim
//! on our own board as `ESC[0;37;40m ESC[79D ESC[K ESC[1;36m`.

use mud_client::correlate::strip_decoration;
use mud_client::events::Event;
use mud_client::parse::Parser;
use mud_client::wire::{TelnetFilter, cp437_to_string};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/oracle");

/// Everything the captures were driven with.
const CMDS: [&str; 12] = [
    "n", "s", "e", "w", "u", "d", "look", "open n", "bash n", "rest", "stand", "get copper",
];

/// The command whose echo introduced this room block, if we can see one.
/// Prompts between the echo and the block are skipped: the board emits
/// them freely and they carry no bearing on which question was asked.
fn asking_command(evs: &[Event], block: usize) -> Option<String> {
    let mut i = block;
    while i > 0 {
        i -= 1;
        match &evs[i] {
            Event::Prompt { .. } => continue,
            Event::Line(l) => return Some(strip_decoration(l.trim()).to_string()),
            _ => return None,
        }
    }
    None
}

#[test]
fn a_room_block_is_introduced_by_the_echo_of_the_command_that_asked_for_it() {
    let mut files: Vec<_> = std::fs::read_dir(CORPUS)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "raw"))
        .collect();
    files.sort();

    let (mut total, mut correlated) = (0usize, 0usize);
    for path in files {
        let raw = std::fs::read(&path).unwrap();
        let mut f = TelnetFilter::new();
        let data = f.push(&raw).data;
        let mut parser = Parser::new();
        let mut evs = parser.push(&cp437_to_string(&data));
        evs.extend(parser.finish());
        for i in 0..evs.len() {
            if !matches!(evs[i], Event::RoomSeen(_)) {
                continue;
            }
            total += 1;
            if asking_command(&evs, i).is_some_and(|c| CMDS.contains(&c.as_str())) {
                correlated += 1;
            }
        }
    }

    assert!(total > 1000, "corpus shrank: only {total} room blocks");
    let pct = correlated as f64 / total as f64 * 100.0;
    // Measured at 94.1% over 6344 blocks. The shortfall is not noise and
    // is not a reason to distrust the signal: it is the login banner
    // ("=-=-=-=-...") introducing the entry render, which no command
    // asked for, plus the `>` menu prompt outside the realm, plus a
    // handful of echoes split by async output arriving mid-line ("ook").
    // A consumer needs to treat "no echo" as "unsolicited", not as
    // "assume it answers my last command" — which is exactly today's bug.
    assert!(
        pct > 90.0,
        "only {correlated}/{total} ({pct:.1}%) room blocks follow a command echo; \
         position tracking cannot be built on this"
    );
}
