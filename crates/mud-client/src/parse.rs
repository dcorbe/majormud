//! Line classifier and room-block accumulator.
//!
//! Input is decoded text (post-telnet, post-CP437) with ANSI intact —
//! the opening SGR disambiguates lines (room names are `1;36`, your
//! misses `0;31`). Classification itself runs on the stripped,
//! backspace-resolved text. Patterns are anchored to `mud_core::text`.

use std::sync::LazyLock;

use regex::Regex;

use crate::events::{Actor, Event, RoomView, Status};
use crate::wire::{resolve_backspaces, strip_ansi};
use mud_core::text::{self, color};

// The board has two prompt templates. The source is WCCMMUD.DLL.
//
//   [HP=%s%d%s%s]:             HP only, status INSIDE the frame
//   [HP=%s%d%s/%s=%s%d%s]:%s   with a pool, status AFTER the frame
//
// The status slot is ` (Resting) ` or ` (Meditating) `, spaces included.
// Unanchored: the board redraws the prompt mid-line (rest ticks, typing
// echo interleaved with async regen).
static PROMPT_FRAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[HP=(-?\d+)(?:/(?:MA|KAI)=(-?\d+))?(?: \(([A-Za-z]+)\) )?\]:").unwrap()
});
// The pool template's trailing status, matched at the start of whatever
// follows "]:". The trailing space is part of the DLL's slot, so a
// half-arrived word without it does not match and the prompt is held
// until the line completes. A single word, because the DLL's two
// statuses are single words and anything wider would swallow the line's
// own text.
static TRAILING_STATUS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ \(([A-Za-z]+)\) ").unwrap());

/// One prompt found in a stripped line: the byte range it occupies,
/// status included, and the event it becomes.
struct FoundPrompt {
    start: usize,
    end: usize,
    event: Event,
}

/// The first prompt in `text`, whichever template painted it.
fn find_prompt(text: &str) -> Option<FoundPrompt> {
    let c = PROMPT_FRAME_RE.captures(text)?;
    let m = c.get(0).unwrap();
    let hp = c[1].parse().unwrap();
    let mana = c.get(2).map(|m| m.as_str().parse().unwrap());
    let mut status = c.get(3).map(|m| Status::from_word(m.as_str()));
    let mut end = m.end();
    // Only the pool template paints after the frame.
    if mana.is_some()
        && status.is_none()
        && let Some(t) = TRAILING_STATUS_RE.captures(&text[end..])
    {
        status = Some(Status::from_word(&t[1]));
        end += t.get(0).unwrap().end();
    }
    Some(FoundPrompt {
        start: m.start(),
        end,
        event: Event::Prompt { hp, mana, status },
    })
}
static YOU_HIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^You (?:critically )?\w+ (.+) for (-?\d+) damage!$").unwrap());
static MONSTER_HIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+) \w+ you for (-?\d+) damage!$").unwrap());
static LEFT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+) just left (?:to the (\w+)|(upwards)|(downwards))\.$").unwrap()
});
static ENTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(.+?) (?:walks|moves) into the room from (?:the (\w+)|(above)|(below)|nowhere)\.$",
    )
    .unwrap()
});
static ARRIVED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+) just arrived from (?:(nowhere)|the (\w+))\.$").unwrap());
// Monster movement wordings are DATA, not grammar: each monster's
// movemsg record holds free-form enter/leave/follow templates ("A %s
// creeps into the room from %s.", "The %s slithers out to %s!") — the
// verb is per-monster, the article belongs to the template, and a
// spawn's origin renders as "nowhere". These patterns cover every
// grammatical family in the shipped template dump (re/mmud_wgnt.sqlite,
// message table); the pattern-less remainder ("A dark storm
// approaches!") names nobody and is left to the combat backstops. The
// lowercase-name anchor keeps player lines and prose out; "(?:the )*"
// absorbs the doubled article produced when a template hardcoding "the"
// has %s filled with "the west".
static MOB_ENTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:(?:A|An|The) )?([a-z][a-z' -]*?) [a-z]+(?: (?:into the room|in the room|into the area|the room|the area|down|in))? from (?:the )*([a-z]+)[.!]$",
    )
    .unwrap()
});
static MOB_ENTER_BARE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:A|An|The) )?([a-z][a-z' -]*?) [a-z]+ (?:into the (?:room|area)|in the room)[.!]$")
        .unwrap()
});
static MOB_FOLLOW_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:A|An|The) )?([a-z][a-z' -]*?) [a-z]+ (?:in|into the room) after you[.!]$")
        .unwrap()
});
static MOB_LEAVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:(?:A|An|The) )?([a-z][a-z' -]*?) [a-z]+(?: (?:out of the room|out|off))? to (?:the )*([a-z]+)[.!]$",
    )
    .unwrap()
});

/// Streaming parser. Push decoded chunks in any sizes; complete lines
/// and end-of-buffer prompts are classified as they appear.
pub struct Parser {
    buf: String,
    room: Option<RoomView>,
    /// A floor listing still being read. The board wraps "You notice
    /// ... here." at 79 columns, mid-item ("113 copper" / "farthings
    /// here."), so the prefix and the suffix land on different
    /// physical lines. Holds the text after the prefix until a line
    /// supplies the suffix; dropped when the block moves on to its
    /// occupants or exits, or a new block starts.
    notice: Option<String>,
    /// An occupant list still being read: the names so far and the
    /// colour each was painted. The board wraps "Also here:" the same
    /// way it wraps a floor listing, after a separator, and the list
    /// ends with a full stop. Dropped when a new block starts; closed
    /// with what it holds if the exits line arrives first.
    occupants: Option<(String, Vec<Option<String>>)>,
}

impl Parser {
    pub fn new() -> Self {
        Parser {
            buf: String::new(),
            room: None,
            notice: None,
            occupants: None,
        }
    }

    /// Feed a decoded chunk; returns events completed by this chunk.
    pub fn push(&mut self, decoded: &str) -> Vec<Event> {
        let mut events = Vec::new();
        // The game marks its prompts with a trailing \x01 for client
        // sync (seen live 2026-08-26: `[HP=27]:\x01look`). It is not
        // text: kept, it glues onto the next echo — which then never
        // matches its command — and holds an end-of-buffer prompt back
        // until the next line. Dropped here so no consumer ever sees it.
        if decoded.contains('\u{1}') {
            self.buf.push_str(&decoded.replace('\u{1}', ""));
        } else {
            self.buf.push_str(decoded);
        }
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
            while let Some(found) = find_prompt(rest) {
                if found.start != 0 {
                    break;
                }
                pending.push(found.event);
                rest = &rest[found.end..];
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
        let mut saw_prompt = false;
        // Prompts can appear anywhere in a physical line (mid-line
        // redraws); classify the segments between them in order.
        while let Some(found) = find_prompt(rest) {
            let before = &rest[..found.start];
            if !before.is_empty() {
                self.classify(before, opening, raw, events);
            }
            events.push(found.event);
            rest = &rest[found.end..];
            // The opening color applied to the first segment only.
            opening = None;
            saw_prompt = true;
        }
        if rest.is_empty() {
            return;
        }
        if saw_prompt {
            // A redrawn prompt and the text answering it share a physical
            // line — the board separates them with cursor-back and
            // erase-line, not a newline (stopstate-run6.raw). The colour
            // opening the trailing segment sits after the prompt's
            // literal "]:" in the raw bytes; without it a room name glued
            // to a prompt dissolves into description lines, which is how
            // busy-room blocks went missing.
            opening = raw
                .rfind("]:")
                .and_then(|p| opening_sgr(&raw[p + 2..]));
        }
        self.classify(rest, opening, raw, events);
    }

    /// `raw` is the whole physical line with its escapes: the colour
    /// is the only thing that says what an occupant IS, and `text_line`
    /// has already lost it.
    fn classify(
        &mut self,
        text_line: &str,
        opening: Option<&str>,
        raw: &str,
        events: &mut Vec<Event>,
    ) {
        // A bare carriage return is a redraw, not text: foreign boards
        // overwrite the dangling prompt with `\r` + erase-line where
        // stock uses a newline (cwrun2.raw, cwgaming 2026-08-01), and
        // the CR then rides at the head of the redrawn segment where it
        // breaks every ^-anchored rule. No legitimate line starts with
        // one.
        let text_line = text_line.trim_start_matches('\r');
        // Room name: a 1;36-opened line starts (or restarts) a block.
        // Banner art also paints 1;36; the last name line before the
        // exits line wins. A line addressed to the player is not a
        // name whatever it wears: the live board paints the party
        // listing's preface, "You are following Carrot.", in this
        // colour (cwgaming 2026-09-14), and read as a room name it
        // opened a block that swallowed the roster and everything
        // after it until the next real block. No room is named
        // "You ...".
        if opening == Some(color::ROOM_NAME) && !text_line.starts_with("You ") {
            self.room = Some(RoomView {
                name: text_line.to_string(),
                ..RoomView::default()
            });
            self.notice = None;
            self.occupants = None;
            return;
        }
        if self.room.is_some() {
            // A wrapped occupant list continues until a line ends it
            // with a full stop. The exits line closes it with whatever
            // it holds: some names beat none.
            if let Some((mut names, mut sgr)) = self.occupants.take() {
                if text_line.starts_with(text::OBVIOUS_EXITS) {
                    self.set_occupants(&names, sgr);
                } else {
                    names.push(' ');
                    names.push_str(text_line);
                    sgr.extend(name_sgr(raw, false));
                    if text_line.ends_with('.') {
                        self.set_occupants(&names, sgr);
                    } else {
                        self.occupants = Some((names, sgr));
                    }
                    return;
                }
            }
            // A wrapped listing continues until a line ends it. The
            // occupants and exits lines never belong to it: a listing
            // that reaches them was not one, and they close the block
            // as usual.
            if let Some(pending) = self.notice.take()
                && !text_line.starts_with(text::OBVIOUS_EXITS)
                && !text_line.starts_with(text::ALSO_HERE)
            {
                let joined = format!("{pending} {text_line}");
                match joined.strip_suffix(" here.") {
                    Some(items) => self.room.as_mut().unwrap().items = split_items(items),
                    None => self.notice = Some(joined),
                }
                return;
            }
            if let Some(exits) = text_line.strip_prefix(text::OBVIOUS_EXITS) {
                let mut room = self.room.take().unwrap();
                if exits != text::NO_EXITS {
                    room.exits = exits.split(", ").map(str::to_string).collect();
                }
                events.push(Event::RoomSeen(room));
                return;
            }
            if let Some(names) = text_line.strip_prefix(text::ALSO_HERE) {
                let sgr = name_sgr(raw, true);
                if names.ends_with('.') {
                    self.set_occupants(names, sgr);
                } else {
                    self.occupants = Some((names.to_string(), sgr));
                }
                return;
            }
            if let Some(listing) = text_line.strip_prefix("You notice ") {
                if let Some(items) = listing.strip_suffix(" here.") {
                    self.room.as_mut().unwrap().items = split_items(items);
                    return;
                }
                // "You notice Salad sneak in from the east." is a
                // notice, not a listing; only a line nothing else
                // claims is read as the head of a wrapped one.
                if let Some(ev) = classify_line(text_line, opening) {
                    events.push(ev);
                    return;
                }
                self.notice = Some(listing.to_string());
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

impl Parser {
    /// Record a complete "Also here:" list on the block being read.
    /// `names` is the text after the marker, `sgr` the colour of each
    /// name in order.
    fn set_occupants(&mut self, names: &str, sgr: Vec<Option<String>>) {
        let room = self.room.as_mut().unwrap();
        room.also_here = names
            .trim_end_matches('.')
            .split(", ")
            .map(str::to_string)
            .collect();
        // Only when the split agrees with what the raw line painted;
        // a mismatch means one of the two readings is wrong, and a
        // wrong colour is worse than none.
        room.also_here_sgr = if sgr.len() == room.also_here.len() {
            sgr
        } else {
            Vec::new()
        };
    }
}

/// The entries of a "You notice ... here." listing, comma separated.
fn split_items(items: &str) -> Vec<String> {
    items.split(", ").map(str::to_string).collect()
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

/// The SGR sequence in effect at the first visible character of the
/// line, in `\x1b[..m` form; None when the line starts unpainted.
/// The SGR each name on an "Also here:" line was painted in, in order.
///
/// The board renders the line as alternating runs — `0;35 "Also here: "`,
/// `1;35 "<name>"`, `0;35 ", "`, `1;35 "<name>"`, `0;35 "."` — so the
/// colour belongs to the run, and the separators are the line's own
/// colour rather than anybody's. Everything after the marker that is not
/// punctuation is a name, and its run's SGR is its colour.
///
/// Empty when the line carried no escape at all: callers must read that
/// as "this board does not paint occupants", never as "nothing here is
/// aggressive".
///
/// `marked` says the line opens with the "Also here:" marker; a
/// continuation line of a wrapped list has none, and every run on it
/// is a name or a separator.
fn name_sgr(raw: &str, marked: bool) -> Vec<Option<String>> {
    let mut runs: Vec<(Option<String>, String)> = Vec::new();
    let mut cur: Option<String> = None;
    let mut text = String::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut painted = false;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'm' {
                runs.push((cur.take(), std::mem::take(&mut text)));
                let code = &raw[i + 2..j];
                // A reset ends the run without naming a colour.
                cur = (!code.is_empty() && code != "0").then(|| code.to_string());
                painted = true;
                i = j + 1;
                continue;
            }
        }
        let ch = raw[i..].chars().next().unwrap_or('\0');
        text.push(ch);
        i += ch.len_utf8();
    }
    runs.push((cur, text));
    if !painted {
        return Vec::new();
    }
    let start = if marked {
        match runs
            .iter()
            .position(|(_, t)| t.contains(text::ALSO_HERE))
        {
            Some(i) => i,
            None => return Vec::new(),
        }
    } else {
        0
    };
    // The marker's own run may carry the first name behind it when the
    // board does not reset between them.
    let mut out = Vec::new();
    for (idx, (sgr, t)) in runs.iter().enumerate().skip(start) {
        let t = if marked && idx == start {
            match t.split_once(text::ALSO_HERE) {
                Some((_, after)) => after,
                None => continue,
            }
        } else {
            t.as_str()
        };
        let t = t.trim().trim_end_matches('.').trim_end_matches(',').trim();
        if t.is_empty() {
            continue;
        }
        out.push(sgr.clone());
    }
    out
}

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
    // Misses: your swings that never connected paint 0;36 (MEASURED
    // 2026-07-26 -- the 0;31 this once assumed is the GLANCE, caught by its
    // own clause below). Cyan is shared with notices and failed casts, so the
    // rule needs the swing's shape too: a terminal `!`. Across the 57 corpus
    // transcripts every cyan "You ...!" line is a miss or a parry (84, no
    // exceptions). Monster whiffs are recognized by their fixed tails
    // (mud_core::text MONSTER_*_TPL family).
    // Monster whiff wordings are per-monster data too (attackmissmsg /
    // attackdodgemsg templates), so the tails are matched loosely: "but
    // you dodge" covers "but you dodge!", "...out of the way!" and
    // "...out of its way!"; "your armour deflects" covers the bare and
    // "the blow!" endings; " at you" anchors the plain attack text
    // ("The thin giant rat lunges at you!"). The long tail ("reaches
    // out for you!", "slashes you with their scimitar!") shares one
    // signature across all 61 shipped miss templates: the whiff cyan
    // with "you" in it, on a line that is not ours. A bystander's fight
    // names no "you" and stays a plain line; a cyan notice aimed at us
    // misclassifying as a whiff costs one redundant look.
    if (opening == Some(color::YOUR_MISS) && t.starts_with("You") && t.ends_with('!'))
        || t.contains("but you dodge")
        || t.contains("your armour deflects")
        || t.contains("glances off")
        || (!t.starts_with("You")
            && (t.ends_with(" at you!")
                || t.contains(" at you,")
                || (opening == Some(color::YOUR_MISS) && crate::events::mentions_you(t))))
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
    // ARRIVED_RE runs before the free-verb movemsg families: "kobold
    // thief just arrived from nowhere." would otherwise parse with
    // "just" swallowed into the name and "arrived" taken as the verb.
    if let Some(c) = ARRIVED_RE.captures(t) {
        let from = c.get(3).map(|m| m.as_str().to_string());
        return Some(Event::ActorEntered {
            name: c[1].to_string(),
            from,
        });
    }
    // The movemsg families run before ENTER_RE: a custom template using
    // "walks" ("An %s walks into the room from %s.") would otherwise be
    // captured with the article glued onto the name, which defeats the
    // player/monster case rule downstream.
    if let Some(c) = MOB_FOLLOW_RE.captures(t) {
        return Some(Event::ActorEntered {
            name: c[1].to_string(),
            from: None,
        });
    }
    if let Some(c) = MOB_ENTER_RE.captures(t)
        && let Some(from) = dir_word(&c[2])
    {
        return Some(Event::ActorEntered {
            name: c[1].to_string(),
            from,
        });
    }
    if let Some(c) = MOB_ENTER_BARE_RE.captures(t) {
        return Some(Event::ActorEntered {
            name: c[1].to_string(),
            from: None,
        });
    }
    if let Some(c) = MOB_LEAVE_RE.captures(t)
        && let Some(to) = dir_word(&c[2])
    {
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
    None
}

/// Origin/destination word from a movemsg line.
///
/// `None` means **this is not a movement line at all**, and that outer
/// layer is the whole point. `Some(None)` is "nowhere", a spawn rather
/// than a place.
///
/// The movemsg wordings are free-form per-monster data, so the families
/// that match them are necessarily loose — loose enough that WRAPPED ROOM
/// DESCRIPTION reads as an arrival. Live, 2026-08-03, Sovereign Street:
///
/// ```text
/// ...the sounds of
/// laughter and merriment coming from within.
/// ```
///
/// matched as "laughter and merriment" entering from "within", and the
/// bot sent `a merriment` at the scenery once per look, forever. The
/// corpus has three more of the same shape ("...clanging steel from
/// inside.", "...leads east and west from here.", "...from the earth.")
/// and NOT ONE genuine arrival whose origin is not a direction — the
/// board fills the template's `%s` with a direction word, so a real
/// movement always has one.
///
/// So the origin is the discriminator: anything else is prose, and prose
/// is not an event.
fn dir_word(w: &str) -> Option<Option<String>> {
    match w {
        "nowhere" => Some(None),
        "above" | "up" => Some(Some("up".to_string())),
        "below" | "down" => Some(Some("down".to_string())),
        "north" | "south" | "east" | "west" | "northeast" | "northwest" | "southeast"
        | "southwest" => Some(Some(w.to_string())),
        _ => None,
    }
}
