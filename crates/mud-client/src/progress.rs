//! The live progress feed for a long-running command.
//!
//! `mmc farm` used to print nothing between "started" and "finished", so
//! a run that was working and a run that was wedged looked exactly the
//! same for as long as they ran — on a live board that can be an hour.
//!
//! This renders the parsed event stream into lines an operator can watch.
//! It reads the same [`crate::events::Event`] broadcast the bot does, so
//! it needs no cooperation from the runner and cannot perturb it: a
//! progress view that had to be threaded through [`crate::farm::run_farm`]
//! would be another thing to keep in step with every phase change.
//!
//! Two levels. The default is what an operator cares about — where the
//! character is, what it is fighting, what it killed, what its HP is
//! doing — and `watch` is the firehose, every line the board sent.

use std::sync::LazyLock;

use regex::Regex;

use crate::events::{Actor, Event};

/// Lines that are worth showing even in the quiet feed, because each one
/// is either the point of the run or a reason it might be going wrong.
const NOTABLE: [&str; 8] = [
    // A kill: the thing the run exists to produce.
    "falls to the ground",
    "you gain",
    // Something went wrong, or the character is in trouble.
    "is dead",
    "you are dead",
    // The board refused an action; the runner may be about to give up on
    // a target.
    "evil warnings",
    "you must",
    // Doors, so a walk that stalls at one is visible.
    "the door",
    "bashed the",
];

pub struct ProgressView {
    watch: bool,
    /// Suppresses the repeats: a prompt lands after nearly every line and
    /// almost always carries the same HP.
    last_hp: Option<i32>,
}

impl ProgressView {
    pub fn new(watch: bool) -> Self {
        ProgressView {
            watch,
            last_hp: None,
        }
    }

    /// One line to show the operator, or `None` when this event is noise.
    pub fn on_event(&mut self, ev: &Event) -> Option<String> {
        match ev {
            Event::RoomSeen(room) => {
                let mut s = format!("-> {}", room.name);
                if !room.also_here.is_empty() {
                    s.push_str(&format!("  [{}]", room.also_here.join(", ")));
                }
                Some(s)
            }
            Event::Prompt { hp, mana, .. } => {
                if self.last_hp == Some(*hp) {
                    return None;
                }
                self.last_hp = Some(*hp);
                Some(match mana {
                    Some(m) => format!("   HP={hp} MA={m}"),
                    None => format!("   HP={hp}"),
                })
            }
            Event::CombatHit {
                attacker,
                target,
                damage,
            } => Some(format!(
                "   {} -> {} ({damage})",
                actor(attacker),
                actor(target)
            )),
            Event::CombatMiss { line } => self.watch.then(|| format!("   {line}")),
            Event::ActorEntered { name, from } => Some(match from {
                Some(d) => format!("   + {name} (from {d})"),
                None => format!("   + {name}"),
            }),
            Event::ActorLeft { name, to } => Some(match to {
                Some(d) => format!("   - {name} ({d})"),
                None => format!("   - {name}"),
            }),
            // Input above this point was DROPPED by the board. Never noise.
            Event::SlowDown => Some("!! flood control: input dropped".to_string()),
            Event::Line(line) => {
                let lower = line.to_lowercase();
                if self.watch || NOTABLE.iter().any(|m| lower.contains(m)) {
                    Some(format!("   {line}"))
                } else {
                    None
                }
            }
        }
    }
}

fn actor(a: &Actor) -> String {
    match a {
        Actor::You => "you".to_string(),
        Actor::Other(name) => name.clone(),
    }
}

/// Experience earned since the run began, and the rate it is coming in.
///
/// Fed from "You gain %s experience." (DLL 0xbc65f) — the same line the
/// bot reads to notice a kill whose death wording it does not know. This
/// is the only honest measure of whether a circuit, or a change to its
/// tuning, is worth anything.
#[derive(Debug, Default, Clone)]
pub struct ExpMeter {
    total: i64,
}

/// The board's award line for a kill of ours — "You gain 16 experience."
/// (DLL 0xbc65f) — and the amount it paid.
///
/// **The** definition, because there were three: an unanchored
/// two-substring `contains` in `bot.rs`, a second copy of that same pair
/// in `farm.rs`, and this one. Three spellings of one rule drift apart,
/// and the drift is silent.
///
/// This line carries weight no death line can. Monster deaths are
/// per-template PROSE — of the 1085 monsters shipping a death record, 67
/// say "falls to the ground" and 1018 say something else entirely ("The
/// filthbug collapses, its legs curling tightly around it."). Matching
/// them all would mean carrying a thousand strings; the award is one, and
/// the board prints it every time a kill of ours pays out.
///
/// Anchored, which matters more than it looks: the award arrives glued to
/// the prompt on a single physical line — "[HP=29/MA=18]:You gain 6
/// experience." is verbatim from the corpus — and it is only the parser
/// splitting at every prompt that leaves a segment starting at "You".
/// `tests/bot_corpus.rs` asserts the anchored and unanchored rules agree
/// on every line the board actually sent.
pub fn exp_award(line: &str) -> Option<i64> {
    let lower = line.trim_start().to_lowercase();
    let rest = lower.strip_prefix("you gain ")?;
    if !rest.contains("experience") {
        return None;
    }
    // The board groups thousands once the awards get large.
    rest.split_whitespace()
        .next()?
        .replace(',', "")
        .parse()
        .ok()
}

/// Did this line award experience? See [`exp_award`].
pub fn is_exp_award(line: &str) -> bool {
    exp_award(line).is_some()
}

impl ExpMeter {
    /// Note a line; awards are counted, everything else ignored.
    pub fn observe(&mut self, line: &str) {
        self.total += exp_award(line).unwrap_or(0);
    }

    pub fn total(&self) -> i64 {
        self.total
    }

    /// Zero the running total. A long `/go` or `/farm` dilutes the
    /// lifetime-of-session rate with minutes that earned nothing before
    /// the job even started; resetting the total here is only half the
    /// fix — the caller must also restart the elapsed clock it feeds to
    /// [`Self::per_minute`], or the rate reads as a spike (old total
    /// over a near-zero elapsed) instead of the fresh figure this exists
    /// to produce.
    pub fn reset(&mut self) {
        self.total = 0;
    }

    /// Experience per minute over `elapsed`, or `None` when too little
    /// time has passed for the figure to mean anything — which is a
    /// better answer than a number produced by dividing by nearly zero.
    pub fn per_minute(&self, elapsed: std::time::Duration) -> Option<i64> {
        let secs = elapsed.as_secs_f64();
        if secs < 1.0 {
            return None;
        }
        Some((self.total as f64 * 60.0 / secs).round() as i64)
    }
}

/// The board's own answer to `exp`, which is where "how long to level"
/// comes from.
///
/// Nothing here reimplements the experience curve. It is class- and
/// race-seeded (`seed = 10 * (base + 100)` with a 26-entry ratio table,
/// `re/docs/records.md`), it needs the character's class, race and level
/// — none of which the client parses — and the board already computes
/// it exactly. Asking is cheaper and cannot drift from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelProgress {
    pub exp: i64,
    pub level: i32,
    /// Experience still owed before the next level can be trained. Zero
    /// means the character can train right now.
    pub needed: i64,
}

static EXP_REPORT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"Exp:\s*([\d,]+)\s+Level:\s*(\d+)\s+Exp needed for next level:\s*([\d,]+)",
    )
    .unwrap()
});

fn count(s: &str) -> Option<i64> {
    s.replace(',', "").parse().ok()
}

/// VERIFIED against the live board (mbbs, 2026-08-01):
/// `Exp: 57209 Level: 3 Exp needed for next level: 0 (10083) [572%]`
pub fn level_progress(line: &str) -> Option<LevelProgress> {
    let c = EXP_REPORT_RE.captures(line)?;
    Some(LevelProgress {
        exp: count(&c[1])?,
        level: c[2].parse().ok()?,
        needed: count(&c[3])?,
    })
}

/// How long at the current rate, short enough for the status bar.
///
/// Honest about what it does not know: no rate yet (the first minute of
/// any run, and after every death resets the meter) gives `?` rather
/// than a fabricated number, and a crawl is capped rather than printed
/// to false precision.
pub fn eta_label(needed: i64, per_minute: Option<i64>) -> String {
    if needed <= 0 {
        return "ready".to_string();
    }
    let Some(rate) = per_minute.filter(|r| *r > 0) else {
        return "?".to_string();
    };
    let mins = needed / rate;
    if mins >= 99 * 60 {
        return ">99h".to_string();
    }
    if mins >= 60 {
        format!("{}h{}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}
