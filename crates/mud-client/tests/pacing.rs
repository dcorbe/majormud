//! Pacer tests: flood-control send pacing (README: keep >=1.5s per
//! command on the live board; the Rust server needs none).

use std::time::{Duration, Instant};

use mud_client::session::Pacer;

#[test]
fn first_send_is_immediate() {
    let mut p = Pacer::new(Duration::from_millis(1500));
    let t0 = Instant::now();
    assert_eq!(p.delay_for(t0), Duration::ZERO);
}

#[test]
fn rapid_sends_are_spaced_by_min_interval() {
    let mut p = Pacer::new(Duration::from_millis(1500));
    let t0 = Instant::now();
    assert_eq!(p.delay_for(t0), Duration::ZERO);
    // Second send at the same instant must wait the full interval.
    assert_eq!(p.delay_for(t0), Duration::from_millis(1500));
    // Third: its slot is one interval after the second's slot.
    assert_eq!(p.delay_for(t0), Duration::from_millis(3000));
}

#[test]
fn partial_elapsed_time_reduces_delay() {
    let mut p = Pacer::new(Duration::from_millis(1500));
    let t0 = Instant::now();
    assert_eq!(p.delay_for(t0), Duration::ZERO);
    let t1 = t0 + Duration::from_millis(500);
    assert_eq!(p.delay_for(t1), Duration::from_millis(1000));
}

#[test]
fn slow_sends_never_wait() {
    let mut p = Pacer::new(Duration::from_millis(1500));
    let t0 = Instant::now();
    assert_eq!(p.delay_for(t0), Duration::ZERO);
    let t1 = t0 + Duration::from_secs(10);
    assert_eq!(p.delay_for(t1), Duration::ZERO);
}

#[test]
fn zero_interval_is_unpaced() {
    let mut p = Pacer::new(Duration::ZERO);
    let t0 = Instant::now();
    assert_eq!(p.delay_for(t0), Duration::ZERO);
    assert_eq!(p.delay_for(t0), Duration::ZERO);
}

// ---- Dynamic re-pacing (run4, 2026-08-01) ----
//
// One session serves both a person and a farm: `mmc play` types unpaced,
// then `/farm` hands the SAME connection to automation, which must go
// back under flood control. The pacer therefore changes its interval
// mid-stream instead of being fixed at connect — the old fixed pacer is
// why the TUI farm ran at loopback echo speed (~40 commands in 400ms).

#[test]
fn raising_the_interval_paces_subsequent_sends() {
    let mut p = Pacer::new(Duration::ZERO);
    let t0 = Instant::now();
    assert_eq!(p.delay_for(t0), Duration::ZERO);
    assert_eq!(p.delay_for(t0), Duration::ZERO);
    p.set_min(Duration::from_millis(1500));
    // The first paced send is immediate — pacing spaces sends, it does
    // not tax the transition.
    assert_eq!(p.delay_for(t0), Duration::ZERO);
    assert_eq!(p.delay_for(t0), Duration::from_millis(1500));
}

#[test]
fn dropping_the_interval_to_zero_unpaces_immediately() {
    let mut p = Pacer::new(Duration::from_millis(1500));
    let t0 = Instant::now();
    assert_eq!(p.delay_for(t0), Duration::ZERO);
    // A slot two intervals out is reserved...
    assert_eq!(p.delay_for(t0), Duration::from_millis(1500));
    p.set_min(Duration::ZERO);
    // ...and the operator taking the keyboard must not serve it out.
    assert_eq!(p.delay_for(t0), Duration::ZERO);
}
