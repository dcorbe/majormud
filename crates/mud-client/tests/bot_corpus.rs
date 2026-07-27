//! Bot policy driven by the real board, not by fixtures.
//!
//! `tests/bot.rs` replays hand-written event sequences, and hand-written
//! fixtures are always tidier than the board: they carry no rolled
//! adjectives, no "closed door north", no players standing in the room.
//! Every one of those gaps hid a real defect. This suite pipes the 51
//! captured transcripts through the same wire and parser stack the live
//! client uses, feeds the events to a fully-armed `Bot`, and asserts
//! invariants over whatever it decides to send.

use mud_client::bot::{Bot, BotAction, BotConfig};
use mud_client::events::Event;
use mud_client::parse::Parser;
use mud_client::wire::{TelnetFilter, cp437_to_string};
use mud_core::content::Direction;
use mud_core::text::direction_shown;

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/oracle");

/// Every currency the board mints (`mud_core` DENOMS).
const DENOMS: [&str; 5] = ["runic", "platinum", "gold", "silver", "copper"];

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

/// Every toggle on, so one pass exercises all four policies.
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

/// Replay one transcript, returning what the bot decided to send.
fn drive(path: &std::path::Path) -> Vec<String> {
    let mut bot = armed_bot();
    corpus_events(path)
        .iter()
        .flat_map(|ev| bot.on_event(ev))
        .map(|BotAction::Send(cmd)| cmd)
        .collect()
}

fn directions() -> Vec<&'static str> {
    Direction::ALL.iter().map(|&d| direction_shown(d)).collect()
}

#[test]
fn corpus_is_present() {
    // `re/oracle/` is shared with the server track, so this grows whenever an
    // oracle expedition lands transcripts — the assertion is a tripwire to
    // re-check the goldens in parse.rs, not a claim that the corpus is fixed.
    assert_eq!(corpus_files().len(), 57, "corpus size changed");
}

#[test]
fn never_attacks_a_player() {
    // Names the board capitalises are players and named NPCs. Collect
    // them from the transcripts themselves rather than hardcoding, then
    // assert none was ever swung at.
    let mut people: Vec<String> = Vec::new();
    for f in corpus_files() {
        for ev in corpus_events(&f) {
            if let Event::RoomSeen(room) = ev {
                for name in room.also_here {
                    if name.chars().next().is_some_and(char::is_uppercase) {
                        people.push(name);
                    }
                }
            }
        }
    }
    people.sort();
    people.dedup();
    assert!(
        !people.is_empty(),
        "corpus should contain capitalised occupants; parser or corpus changed"
    );

    for f in corpus_files() {
        for cmd in drive(&f) {
            let Some(target) = cmd.strip_prefix("a ") else {
                continue;
            };
            for person in &people {
                // Targeting sends the trailing noun, so compare that way.
                let noun = person.split_whitespace().last().unwrap_or(person);
                assert!(
                    !target.eq_ignore_ascii_case(noun),
                    "{}: bot attacked player/NPC {person:?} via {cmd:?}",
                    f.display()
                );
            }
        }
    }
}

#[test]
fn every_movement_command_is_a_real_direction() {
    // Catches the "closed door north" class of bug generally: anything
    // the bot sends that is not an attack, a get or the heal command is
    // a flee, and a flee must be a direction the board accepts.
    let dirs = directions();
    for f in corpus_files() {
        for cmd in drive(&f) {
            if cmd.starts_with("a ") || cmd.starts_with("get ") || cmd == "rest" {
                continue;
            }
            assert!(
                dirs.contains(&cmd.as_str()),
                "{}: {cmd:?} is not a movement command",
                f.display()
            );
        }
    }
}

#[test]
fn every_get_names_a_currency() {
    for f in corpus_files() {
        for cmd in drive(&f) {
            if let Some(what) = cmd.strip_prefix("get ") {
                assert!(
                    DENOMS.contains(&what),
                    "{}: {cmd:?} does not name a currency",
                    f.display()
                );
            }
        }
    }
}

#[test]
fn does_not_go_quiet_over_a_long_fight() {
    // Liveness: an engagement latch that never releases makes the bot
    // attack once and then sit silent forever. Pick the most combat-heavy
    // transcript and prove it keeps swinging.
    //
    // Note this does NOT isolate the death path — on real transcripts a
    // room block usually arrives first and re-arms the latch, so removing
    // the death clearing alone leaves this test green (verified by
    // mutation). `re_engages_after_the_target_dies` in tests/bot.rs is
    // what pins that path.
    let combat = corpus_files()
        .into_iter()
        .map(|f| {
            let deaths = corpus_events(&f)
                .iter()
                .filter(|e| matches!(e, Event::Line(l) if l.contains("falls to the ground")))
                .count();
            (f, deaths)
        })
        .max_by_key(|(_, deaths)| *deaths)
        .expect("corpus is not empty");
    let (file, deaths) = combat;
    assert!(
        deaths >= 2,
        "expected a transcript with repeated kills, best had {deaths}"
    );

    let attacks: Vec<String> = drive(&file)
        .into_iter()
        .filter(|c| c.starts_with("a "))
        .collect();
    assert!(
        attacks.len() > 1,
        "{}: {deaths} kills but only {} attack(s) — the engagement latch is stuck",
        file.display(),
        attacks.len()
    );
}
