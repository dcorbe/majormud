//! The character stat sheet (`stat` command output).
//!
//! Today the client parses NOTHING off this screen: whether a character
//! can pick a lock is a config flag an operator has to set by hand,
//! rather than a fact the client could just read off `Picklocks:`. This
//! module is the fix's foundation — a struct with every field the sheet
//! carries, and a parser that fills in whatever the text has.
//!
//! Deliberately pure and offline: no session, no polling, no live board.
//! Later work wires [`Stats::parse`] to a `stat` reply.
//!
//! # Layout
//!
//! The sheet is three ragged columns, not a table — rows don't line up:
//!
//! ```text
//! Name: Beef                             Lives/CP:    9/100
//! Race: Dark-Elf    Exp: 0               Perception:     43
//! Class: Ninja      Level: 1             Stealth:        56
//! Hits:    22/22    Armour Class:   0/0  Thievery:        0
//!                                        Traps:          29
//!                                        Picklocks:      31
//! Strength:  40     Agility: 50          Tracking:       26
//! Intellect: 50     Health:  30          Martial Arts:   51
//! Willpower: 30     Charm:   40          MagicRes:       35
//! ```
//!
//! `Traps` and `Picklocks` sit alone on their rows with the first two
//! columns blank, and numbers are right-aligned with variable padding.
//! Column position carries no meaning, so every field is found by
//! searching the whole text for its own `Label:`, not by reading a fixed
//! slice of each line.
//!
//! A text field (`Name`, `Race`, `Class`) ends at a run of 2+ spaces, at
//! the next `Label:`, or at end of line — whichever comes first. The
//! "next `Label:`" branch matters because a long class name can land
//! only a single space from `Level:` (the fixed column position doesn't
//! leave room for two): see
//! `class_survives_a_single_space_before_the_next_label` in
//! `crates/mud-client/tests/stats.rs`, and the same fix in the reference
//! client at `archive/FujiTerm/MudPlay/Game/StatParser.cs` (`ClassRx`),
//! whose `ClassRx` expresses this with a regex lookahead. Rust's `regex`
//! crate deliberately has no lookaround (it guarantees linear-time
//! matching), so [`take_text_value`] below runs the same rule as a plain
//! scan over the rest of the line instead.

use std::sync::LazyLock;

use regex::Regex;

/// A snapshot of the `stat` screen.
///
/// Every field is optional: a board can word things differently, or the
/// screen the caller captured can be partial (scrolled, truncated,
/// mid-redraw). [`Stats::parse`] fills in whatever it finds and leaves
/// the rest `None` rather than failing the whole parse — a half-read
/// sheet is more useful than no sheet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub name: Option<String>,
    pub race: Option<String>,
    pub class: Option<String>,

    pub level: Option<u32>,
    pub exp: Option<u32>,

    /// Lives remaining — the left half of `Lives/CP: N/M`.
    pub lives: Option<u32>,
    /// Character points available to spend — the right half of
    /// `Lives/CP: N/M`.
    pub cp: Option<u32>,

    pub hits: Option<u32>,
    pub max_hits: Option<u32>,
    pub armour_class: Option<u32>,
    pub max_armour_class: Option<u32>,

    pub perception: Option<u32>,
    pub stealth: Option<u32>,
    pub thievery: Option<u32>,
    pub traps: Option<u32>,
    pub picklocks: Option<u32>,
    pub tracking: Option<u32>,
    pub martial_arts: Option<u32>,
    pub magic_res: Option<u32>,

    pub strength: Option<u32>,
    pub agility: Option<u32>,
    pub intellect: Option<u32>,
    pub health: Option<u32>,
    pub willpower: Option<u32>,
    pub charm: Option<u32>,
}

type TextSetter = fn(&mut Stats, String);
type PairSetter = fn(&mut Stats, u32, u32);
type NumberSetter = fn(&mut Stats, u32);

/// Every string-valued field, labelled exactly as this board (and
/// `mud-core`'s formatter) spells it. This is the ONE table: a
/// differently-worded board is a one-line edit here, not a rewrite of
/// the parsing logic below.
///
/// These three all lead their row (see the layout diagram above), so
/// their regex is anchored to line-start. That anchor is what keeps
/// `Class:` from matching inside `Armour Class:`, which sits later in
/// the same row-4 text and would otherwise be an equally valid `\bClass:`
/// match.
const TEXT_FIELDS: &[(&str, TextSetter)] = &[
    ("Name", |s, v| s.name = Some(v)),
    ("Race", |s, v| s.race = Some(v)),
    ("Class", |s, v| s.class = Some(v)),
];

/// Every `current/max` field.
const PAIR_FIELDS: &[(&str, PairSetter)] = &[
    ("Lives/CP", |s, a, b| {
        s.lives = Some(a);
        s.cp = Some(b);
    }),
    ("Hits", |s, a, b| {
        s.hits = Some(a);
        s.max_hits = Some(b);
    }),
    ("Armour Class", |s, a, b| {
        s.armour_class = Some(a);
        s.max_armour_class = Some(b);
    }),
];

/// Every plain-number field: progression, skills, and the six stats.
const NUMBER_FIELDS: &[(&str, NumberSetter)] = &[
    ("Level", |s, v| s.level = Some(v)),
    ("Exp", |s, v| s.exp = Some(v)),
    ("Perception", |s, v| s.perception = Some(v)),
    ("Stealth", |s, v| s.stealth = Some(v)),
    ("Thievery", |s, v| s.thievery = Some(v)),
    ("Traps", |s, v| s.traps = Some(v)),
    ("Picklocks", |s, v| s.picklocks = Some(v)),
    ("Tracking", |s, v| s.tracking = Some(v)),
    ("Martial Arts", |s, v| s.martial_arts = Some(v)),
    ("MagicRes", |s, v| s.magic_res = Some(v)),
    ("Strength", |s, v| s.strength = Some(v)),
    ("Agility", |s, v| s.agility = Some(v)),
    ("Intellect", |s, v| s.intellect = Some(v)),
    ("Health", |s, v| s.health = Some(v)),
    ("Willpower", |s, v| s.willpower = Some(v)),
    ("Charm", |s, v| s.charm = Some(v)),
];

/// Locates a text field's label and captures everything after it to the
/// end of that line. [`take_text_value`] then trims that tail down to
/// the actual value. Anchored to line-start (see [`TEXT_FIELDS`]).
fn text_field_regex(label: &str) -> Regex {
    Regex::new(&format!(r"(?m)^\s*{}:(.*)$", regex::escape(label)))
        .expect("text field pattern is a fixed, valid template")
}

/// Trim a text field's raw line-tail (everything after `Label:`, spaces
/// and all) down to its value.
///
/// Requires at least one space before the value (a bare `Label:x` with
/// no separating space is not a valid field and yields `None`). From
/// there, consumes one word at a time, stopping — without consuming the
/// separator that triggered the stop — at whichever comes first:
///
/// - a run of 2+ spaces (the column gutter), or
/// - a single space followed by a word that itself ends in `:` (the
///   next field's label butted up against this one), or
/// - end of line.
///
/// A single space NOT followed by a label is kept as part of the value,
/// so a multi-word value survives.
fn take_text_value(line_tail: &str) -> Option<String> {
    let after_colon = line_tail;
    let trimmed = after_colon.trim_start_matches(' ');
    if trimmed.len() == after_colon.len() || trimmed.is_empty() {
        // No separating space, or nothing after it.
        return None;
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let n = chars.len();
    let mut end = 0;
    let mut i = 0;
    while i < n {
        if chars[i] != ' ' {
            i += 1;
            end = i;
            continue;
        }

        let gap_start = i;
        while i < n && chars[i] == ' ' {
            i += 1;
        }
        if i - gap_start >= 2 {
            break; // 2+ space gutter
        }

        let word_start = i;
        while i < n && chars[i] != ' ' {
            i += 1;
        }
        let word: String = chars[word_start..i].iter().collect();
        if word.ends_with(':') {
            break; // the next field's label
        }
        // A lone space mid-value: keep scanning, word included.
        end = i;
    }

    let value: String = chars[..end].iter().collect();
    if value.is_empty() { None } else { Some(value) }
}

/// A `current/max` field's value. Digits are self-terminating, so no
/// lookahead terminator is needed here.
fn pair_field_regex(label: &str) -> Regex {
    Regex::new(&format!(r"\b{}:\s+(\d+)/(\d+)", regex::escape(label)))
        .expect("pair field pattern is a fixed, valid template")
}

/// A plain-number field's value.
fn number_field_regex(label: &str) -> Regex {
    Regex::new(&format!(r"\b{}:\s+(\d+)", regex::escape(label)))
        .expect("number field pattern is a fixed, valid template")
}

static TEXT_FIELD_REGEXES: LazyLock<Vec<(Regex, TextSetter)>> = LazyLock::new(|| {
    TEXT_FIELDS
        .iter()
        .map(|(label, setter)| (text_field_regex(label), *setter))
        .collect()
});

static PAIR_FIELD_REGEXES: LazyLock<Vec<(Regex, PairSetter)>> = LazyLock::new(|| {
    PAIR_FIELDS
        .iter()
        .map(|(label, setter)| (pair_field_regex(label), *setter))
        .collect()
});

static NUMBER_FIELD_REGEXES: LazyLock<Vec<(Regex, NumberSetter)>> = LazyLock::new(|| {
    NUMBER_FIELDS
        .iter()
        .map(|(label, setter)| (number_field_regex(label), *setter))
        .collect()
});

impl Stats {
    /// Parse a `stat` screen. Fields absent from `text` stay `None`;
    /// this never fails.
    pub fn parse(text: &str) -> Stats {
        let mut stats = Stats::default();

        for (re, setter) in TEXT_FIELD_REGEXES.iter() {
            if let Some(caps) = re.captures(text)
                && let Some(value) = take_text_value(&caps[1])
            {
                setter(&mut stats, value);
            }
        }

        for (re, setter) in PAIR_FIELD_REGEXES.iter() {
            if let Some(caps) = re.captures(text)
                && let (Ok(a), Ok(b)) = (caps[1].parse(), caps[2].parse())
            {
                setter(&mut stats, a, b);
            }
        }

        for (re, setter) in NUMBER_FIELD_REGEXES.iter() {
            if let Some(caps) = re.captures(text)
                && let Ok(v) = caps[1].parse()
            {
                setter(&mut stats, v);
            }
        }

        stats
    }
}
