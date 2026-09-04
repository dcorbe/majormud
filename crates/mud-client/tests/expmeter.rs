//! Experience rate.
//!
//! The board announces every award as "You gain %s experience." (DLL
//! 0xbc65f) — the same line the bot uses to notice a kill it could not
//! name. Counting it is the only measure of whether a circuit and its
//! tuning are actually worth anything.

use std::time::Duration;

use mud_client::progress::ExpMeter;

#[test]
fn it_counts_awards() {
    let mut m = ExpMeter::default();
    m.observe("You gain 300 experience.");
    m.observe("You gain 13 experience.");
    assert_eq!(m.total(), 313);
}

#[test]
fn lines_that_are_not_awards_are_ignored() {
    let mut m = ExpMeter::default();
    m.observe("You do not have the required experience");
    m.observe("The cave bear falls to the ground with a grunt!");
    assert_eq!(m.total(), 0);
}

/// Per hour, from an elapsed time passed in rather than read from a
/// clock, so the arithmetic is testable.
#[test]
fn it_reports_a_rate_per_hour() {
    let mut m = ExpMeter::default();
    m.observe("You gain 300 experience.");
    m.observe("You gain 300 experience.");
    assert_eq!(m.per_hour(Duration::from_secs(120)), Some(18_000));
    assert_eq!(m.per_hour(Duration::from_secs(60)), Some(36_000));
}

/// Before any time has passed the rate is meaningless, and dividing by
/// it would be worse than saying nothing.
#[test]
fn too_early_to_say_is_not_zero() {
    let mut m = ExpMeter::default();
    m.observe("You gain 300 experience.");
    assert_eq!(m.per_hour(Duration::from_secs(0)), None);
}

/// The real award line, which carries a thousands separator once the
/// numbers get big.
#[test]
fn a_grouped_number_still_counts() {
    let mut m = ExpMeter::default();
    m.observe("You gain 1,250 experience.");
    assert_eq!(m.total(), 1250);
}

// ---------------------------------------------------------------------
// Time to level. The board does the hard part: `exp` reports the total,
// the level, and how much more is needed, so nothing here reimplements
// the experience curve (which is class- and race-seeded — records.md).
// ---------------------------------------------------------------------

use mud_client::progress::{eta_label, level_progress};

/// Verbatim from the live board (mbbs, 2026-08-01).
const LINE: &str = "Exp: 57209 Level: 3 Exp needed for next level: 0 (10083) [572%]";

#[test]
fn reads_the_boards_experience_report() {
    let p = level_progress(LINE).expect("should parse");
    assert_eq!(p.exp, 57209);
    assert_eq!(p.level, 3);
    assert_eq!(p.needed, 0);
}

#[test]
fn a_line_that_is_not_the_report_parses_to_nothing() {
    assert!(level_progress("You gain 6 experience.").is_none());
}

/// Big numbers wear thousands separators on some boards; the count is
/// the point, not the punctuation.
#[test]
fn thousands_separators_do_not_defeat_it() {
    let p = level_progress("Exp: 1,234,567 Level: 24 Exp needed for next level: 89,000 (1,300,000) [94%]")
        .expect("should parse");
    assert_eq!(p.exp, 1_234_567);
    assert_eq!(p.needed, 89_000);
}

/// Nothing left to earn: the character can train now, and a duration
/// would be a lie.
#[test]
fn no_experience_needed_reads_as_ready() {
    assert_eq!(eta_label(0, Some(30_000)), "ready");
}

/// A rate of nothing gives no estimate rather than infinity. This is the
/// common case for the first minute of a run and after every death.
#[test]
fn without_a_rate_there_is_no_estimate() {
    assert_eq!(eta_label(10_000, None), "?");
    assert_eq!(eta_label(10_000, Some(0)), "?");
}

#[test]
fn an_estimate_reads_in_hours_and_minutes() {
    assert_eq!(eta_label(6_000, Some(6_000)), "1h0m");
    assert_eq!(eta_label(4_500, Some(6_000)), "45m");
    assert_eq!(eta_label(7_200, Some(6_000)), "1h12m");
}

/// A crawl must not print a number that implies precision it has not
/// got, and must not overflow the bar.
#[test]
fn an_absurd_estimate_is_capped() {
    assert_eq!(eta_label(10_000_000, Some(60)), ">99h");
}
