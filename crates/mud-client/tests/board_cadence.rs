//! What the board does when nothing is happening.
//!
//! These two facts decide how any unattended client must be paced, and
//! both were believed backwards until they were measured. They are
//! pinned here against the captured `*_timing.log` files — the only
//! corpus artefacts carrying wall-clock stamps, and so the only evidence
//! that can answer a question about cadence at all.
//!
//! The log format is `<epoch>.<ms> <RX|TX> <line>`.

use std::collections::BTreeMap;

use mud_client::wire::{cp437_to_string, strip_ansi};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/oracle");

/// A quiet stretch long enough that a per-tick reprint would have to
/// show up inside it. Regen ticks are seconds apart, not minutes.
const SILENCE: f64 = 4.0;

#[derive(Debug)]
struct Entry {
    at: f64,
    tx: bool,
    line: String,
}

fn timing_logs() -> Vec<(String, Vec<Entry>)> {
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(CORPUS)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with("_timing.log"))
        .collect();
    files.sort();
    for path in files {
        let text = cp437_to_string(&std::fs::read(&path).unwrap());
        let entries = text
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, ' ');
                let at: f64 = parts.next()?.parse().ok()?;
                let kind = parts.next()?;
                let body = parts.next().unwrap_or("");
                // `!!` marker lines are the capture driver talking, not
                // the board.
                let tx = match kind {
                    "TX" => true,
                    "RX" => false,
                    _ => return None,
                };
                Some(Entry {
                    at,
                    tx,
                    line: strip_ansi(body).trim().to_string(),
                })
            })
            .collect();
        // Not every *_timing.log is a wire log: the M6 arena expedition
        // wrote hand-annotated running commentary ("=== attacking
        // 'giant' ===") under the same suffix. Those carry no RX/TX and
        // say nothing about board cadence.
        let entries: Vec<Entry> = entries;
        if !entries.is_empty() {
            out.push((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                entries,
            ));
        }
    }
    out
}

/// A line that is nothing but one or more prompts.
fn is_bare_prompt(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    let mut rest = line;
    let mut seen = 0;
    while let Some(open) = rest.strip_prefix("[HP=") {
        let Some(close) = open.find("]:") else {
            return false;
        };
        rest = &open[close + 2..];
        seen += 1;
    }
    seen > 0 && rest.is_empty()
}

/// **The board does not reprint the prompt on a regen tick.** An idle
/// session gets silence, and it can be a very long silence.
///
/// This was believed the other way round for two slices, and the belief
/// shaped real design: it is why the runner assumed its idle poke would
/// rarely fire, and it nearly justified "fixing" mud-server to reprint,
/// which would have made the reimplementation *less* faithful.
///
/// What is true — and what the mistake came from — is that prompts
/// double up on one physical line, `[HP=31]:[HP=32]:`. That is the async
/// redraw: output disturbs the dangling prompt and the DLL re-prompts.
/// It happens *around output*, never on its own.
#[test]
fn the_board_says_nothing_at_all_to_an_idle_session() {
    let mut examined = 0;
    let mut longest: f64 = 0.0;
    for (name, entries) in timing_logs() {
        for pair in entries.windows(2) {
            let (prev, cur) = (&pair[0], &pair[1]);
            let gap = cur.at - prev.at;
            if cur.tx || gap <= SILENCE || cur.line.is_empty() {
                continue;
            }
            examined += 1;
            longest = longest.max(gap);
            assert!(
                !is_bare_prompt(&cur.line),
                "{name}: after {gap:.1}s of silence the board sent a bare prompt \
                 ({:?}) — if this is real, an idle session does get prompts and \
                 the dwell policy in farm.rs can stop poking for them",
                cur.line
            );
        }
    }
    // Without a long quiet stretch the assertion above proves nothing:
    // a reprint every few seconds would never be caught by it.
    assert!(
        examined > 100,
        "too few quiet stretches to conclude anything"
    );
    assert!(
        longest > 60.0,
        "longest silence was only {longest:.0}s; a per-tick reprint could hide in that"
    );
}

/// **Flood control, measured.** Eight sends spaced 1.3s apart earned
/// "Why don't you slow down for a few seconds?" — so a sustained
/// automatic cadence has to stay well clear of the pacer's 1500ms floor,
/// which is only 200ms above a rate the board demonstrably punishes.
#[test]
fn a_sustained_send_every_1_3s_trips_flood_control() {
    let mut worst: Option<(String, usize, f64)> = None;
    for (name, entries) in timing_logs() {
        for (i, e) in entries.iter().enumerate() {
            if e.tx || !e.line.to_lowercase().contains("slow down") {
                continue;
            }
            // Sends in the ten seconds leading up to the scolding.
            let recent: Vec<f64> = entries[..i]
                .iter()
                .filter(|p| p.tx && e.at - p.at <= 10.0)
                .map(|p| p.at)
                .collect();
            if recent.len() < 2 {
                continue;
            }
            let span = recent[recent.len() - 1] - recent[0];
            let mean = span / (recent.len() - 1) as f64;
            if worst.as_ref().is_none_or(|w| recent.len() > w.1) {
                worst = Some((name.clone(), recent.len(), mean));
            }
        }
    }
    let (name, sends, mean) = worst.expect("no flood-control scolding in the corpus");
    assert!(
        sends >= 8 && mean <= 1.5,
        "{name}: expected the scolding to follow a burst of >=8 sends at <=1.5s \
         spacing, saw {sends} at {mean:.2}s"
    );
}

/// The facts above were measured from these seven wire logs. If the set
/// changes, the numbers they pin deserve a fresh look rather than a
/// silent pass. (Nine files carry the `_timing.log` suffix; two are the
/// M6 arena's hand-written commentary, not wire captures.)
#[test]
fn the_timing_corpus_is_what_these_numbers_were_measured_from() {
    let logs = timing_logs();
    let sizes: BTreeMap<&str, usize> = logs.iter().map(|(n, e)| (n.as_str(), e.len())).collect();
    assert_eq!(logs.len(), 7, "wire timing-log corpus changed: {sizes:?}");
}
