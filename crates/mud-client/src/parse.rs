//! Line classifier and room-block accumulator.
//!
//! Input is decoded text (post-telnet, post-CP437) with ANSI intact —
//! the opening SGR disambiguates lines (room names are `1;36`, your
//! misses `0;31`). Classification itself runs on the stripped,
//! backspace-resolved text. Patterns are anchored to `mud_core::text`.

use std::sync::LazyLock;

use regex::Regex;

use crate::events::{Actor, Event, RoomView};
use crate::wire::{resolve_backspaces, strip_ansi};
use mud_core::text::{self, color};

// Unanchored: the board redraws the prompt mid-line (rest ticks, typing
// echo interleaved with async regen).
static PROMPT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[HP=(-?\d+)(?:/(?:MA|KAI)=(-?\d+))?\]:").unwrap());
static YOU_HIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^You (?:critically )?\w+ (.+) for (-?\d+) damage!$").unwrap());
static MONSTER_HIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+) \w+ you for (-?\d+) damage!$").unwrap());
static LEFT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+) just left (?:to the (\w+)|(upwards)|(downwards))\.$").unwrap()
});
static ENTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?) (?:walks|moves) into the room from (?:the (\w+)|(above)|(below))\.$")
        .unwrap()
});
static ARRIVED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+) just arrived from (?:(nowhere)|the (\w+))\.$").unwrap());

/// Streaming parser. Push decoded chunks in any sizes; complete lines
/// and end-of-buffer prompts are classified as they appear.
pub struct Parser {
    buf: String,
    room: Option<RoomView>,
}

impl Parser {
    pub fn new() -> Self {
        Parser {
            buf: String::new(),
            room: None,
        }
    }

    /// Feed a decoded chunk; returns events completed by this chunk.
    pub fn push(&mut self, decoded: &str) -> Vec<Event> {
        let mut events = Vec::new();
        self.buf.push_str(decoded);
        while let Some(nl) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=nl).collect();
            self.handle_line(line.trim_end_matches(['\n', '\r']), &mut events);
        }
        // A prompt arrives with no newline; emit it the moment the
        // pending partial is exactly one-or-more complete prompts.
        if !self.buf.is_empty() {
            let cleaned = resolve_backspaces(&strip_ansi(&self.buf));
            let mut rest = cleaned.as_str();
            let mut pending = Vec::new();
            while let Some(c) = PROMPT_RE.captures(rest) {
                let m = c.get(0).unwrap();
                if m.start() != 0 {
                    break;
                }
                pending.push(prompt_event(&c));
                rest = &rest[m.end()..];
            }
            if rest.is_empty() && !pending.is_empty() {
                events.append(&mut pending);
                self.buf.clear();
            }
        }
        events
    }

    /// Flush the pending partial line (end of capture / disconnect).
    pub fn finish(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            self.handle_line(line.trim_end_matches(['\n', '\r']), &mut events);
        }
        events
    }

    fn handle_line(&mut self, raw: &str, events: &mut Vec<Event>) {
        let cleaned = resolve_backspaces(&strip_ansi(raw));
        let mut rest = cleaned.as_str();
        let mut opening = opening_sgr(raw);
        // Prompts can appear anywhere in a physical line (mid-line
        // redraws); classify the segments between them in order.
        while let Some(c) = PROMPT_RE.captures(rest) {
            let m = c.get(0).unwrap();
            let before = &rest[..m.start()];
            if !before.is_empty() {
                self.classify(before, opening, events);
            }
            events.push(prompt_event(&c));
            rest = &rest[m.end()..];
            // The opening color applied to the first segment only.
            opening = None;
        }
        if rest.is_empty() {
            return;
        }
        self.classify(rest, opening, events);
    }

    fn classify(&mut self, text_line: &str, opening: Option<&str>, events: &mut Vec<Event>) {
        // Room name: a 1;36-opened line starts (or restarts) a block.
        // Banner art also paints 1;36; the last name line before the
        // exits line wins.
        if opening == Some(color::ROOM_NAME) {
            self.room = Some(RoomView {
                name: text_line.to_string(),
                ..RoomView::default()
            });
            return;
        }
        if self.room.is_some() {
            if let Some(exits) = text_line.strip_prefix(text::OBVIOUS_EXITS) {
                let mut room = self.room.take().unwrap();
                if exits != text::NO_EXITS {
                    room.exits = exits.split(", ").map(str::to_string).collect();
                }
                events.push(Event::RoomSeen(room));
                return;
            }
            if let Some(names) = text_line.strip_prefix(text::ALSO_HERE) {
                let room = self.room.as_mut().unwrap();
                room.also_here = names
                    .trim_end_matches('.')
                    .split(", ")
                    .map(str::to_string)
                    .collect();
                return;
            }
            if let Some(items) = text_line
                .strip_prefix("You notice ")
                .and_then(|t| t.strip_suffix(" here."))
            {
                let room = self.room.as_mut().unwrap();
                room.items = items.split(", ").map(str::to_string).collect();
                return;
            }
            // Async events can interleave with a rendering room block;
            // anything unrecognized is description text.
            if let Some(ev) = classify_line(text_line, opening) {
                events.push(ev);
            }
            return;
        }
        events.push(
            classify_line(text_line, opening).unwrap_or_else(|| Event::Line(text_line.to_string())),
        );
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

fn prompt_event(c: &regex::Captures) -> Event {
    Event::Prompt {
        hp: c[1].parse().unwrap(),
        mana: c.get(2).map(|m| m.as_str().parse().unwrap()),
    }
}

/// The SGR sequence in effect at the first visible character of the
/// line, in `\x1b[..m` form; None when the line starts unpainted.
fn opening_sgr(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut last: Option<&str> = None;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len()
                && (bytes[j].is_ascii_digit() || bytes[j] == b';' || bytes[j] == b'?')
            {
                j += 1;
            }
            if j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                if bytes[j] == b'm' {
                    last = Some(&raw[i..=j]);
                }
                i = j + 1;
                continue;
            }
        }
        return last;
    }
    last
}

fn classify_line(t: &str, opening: Option<&str>) -> Option<Event> {
    if t.contains("Why don't you slow down") {
        return Some(Event::SlowDown);
    }
    if let Some(c) = YOU_HIT_RE.captures(t) {
        return Some(Event::CombatHit {
            attacker: Actor::You,
            target: Actor::Other(c[1].to_string()),
            damage: c[2].parse().unwrap(),
        });
    }
    if let Some(c) = MONSTER_HIT_RE.captures(t) {
        return Some(Event::CombatHit {
            attacker: Actor::Other(c[1].to_string()),
            target: Actor::You,
            damage: c[2].parse().unwrap(),
        });
    }
    // Misses: your swings paint 0;31; monster whiffs are recognized by
    // their fixed tails (mud_core::text MONSTER_*_TPL family).
    if (opening == Some(color::YOUR_MISS) && t.starts_with("You"))
        || t.ends_with("but you dodge!")
        || t.contains("your armour deflects.")
        || t.contains("glances off")
    {
        return Some(Event::CombatMiss {
            line: t.to_string(),
        });
    }
    if let Some(c) = LEFT_RE.captures(t) {
        let to = c
            .get(2)
            .map(|m| m.as_str().to_string())
            .or_else(|| c.get(3).map(|_| "up".to_string()))
            .or_else(|| c.get(4).map(|_| "down".to_string()));
        return Some(Event::ActorLeft {
            name: c[1].to_string(),
            to,
        });
    }
    if let Some(c) = ENTER_RE.captures(t) {
        let from = c
            .get(2)
            .map(|m| m.as_str().to_string())
            .or_else(|| c.get(3).map(|_| "up".to_string()))
            .or_else(|| c.get(4).map(|_| "down".to_string()));
        return Some(Event::ActorEntered {
            name: c[1].to_string(),
            from,
        });
    }
    if let Some(c) = ARRIVED_RE.captures(t) {
        let from = c.get(3).map(|m| m.as_str().to_string());
        return Some(Event::ActorEntered {
            name: c[1].to_string(),
            from,
        });
    }
    None
}
