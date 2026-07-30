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

/// Per minute, from an elapsed time passed in rather than read from a
/// clock, so the arithmetic is testable.
#[test]
fn it_reports_a_rate_per_minute() {
    let mut m = ExpMeter::default();
    m.observe("You gain 300 experience.");
    m.observe("You gain 300 experience.");
    assert_eq!(m.per_minute(Duration::from_secs(120)), Some(300));
    assert_eq!(m.per_minute(Duration::from_secs(60)), Some(600));
}

/// Before any time has passed the rate is meaningless, and dividing by
/// it would be worse than saying nothing.
#[test]
fn too_early_to_say_is_not_zero() {
    let mut m = ExpMeter::default();
    m.observe("You gain 300 experience.");
    assert_eq!(m.per_minute(Duration::from_secs(0)), None);
}

/// The real award line, which carries a thousands separator once the
/// numbers get big.
#[test]
fn a_grouped_number_still_counts() {
    let mut m = ExpMeter::default();
    m.observe("You gain 1,250 experience.");
    assert_eq!(m.total(), 1250);
}
