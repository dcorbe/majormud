//! World-state tests: the board's round clock (and, later, `Here`).
//! Pure and clock-injected like the bot and StopState suites.

use std::time::{Duration, Instant};

use mud_client::world::{ROUND, RoundClock};

#[test]
fn an_unlocked_clock_paces_by_one_period() {
    let clock = RoundClock::new();
    let t = Instant::now();
    assert_eq!(clock.next_round_after(t), t + ROUND);
}

#[test]
fn a_burst_locks_the_phase() {
    let mut clock = RoundClock::new();
    let t0 = Instant::now();
    clock.observe(t0);
    // Asked mid-round, the next round is the burst's phase plus one
    // period — not "one period from now".
    assert_eq!(
        clock.next_round_after(t0 + Duration::from_secs(1)),
        t0 + ROUND
    );
}

#[test]
fn lines_within_one_burst_do_not_slide_the_phase() {
    let mut clock = RoundClock::new();
    let t0 = Instant::now();
    clock.observe(t0);
    clock.observe(t0 + Duration::from_millis(40));
    clock.observe(t0 + Duration::from_millis(80));
    assert_eq!(
        clock.next_round_after(t0 + Duration::from_secs(1)),
        t0 + ROUND
    );
}

#[test]
fn a_later_burst_relocks_the_phase() {
    let mut clock = RoundClock::new();
    let t0 = Instant::now();
    clock.observe(t0);
    // Lag drifts the observed cadence; a burst clearly past the old
    // phase re-locks to what the board actually did.
    let t1 = t0 + 3 * ROUND + Duration::from_millis(200);
    clock.observe(t1);
    assert_eq!(
        clock.next_round_after(t1 + Duration::from_secs(1)),
        t1 + ROUND
    );
}
