//! Tests for the tick scheduler (`re/docs/combat_rounds.md` §1).
//!
//! Mirrors `my_rtkick`/`perform_kicks`: a free-running tick counter (1 tick =
//! 1 s), jobs enqueued at `now + delay`, drained once per tick. Jobs are data;
//! the caller matches on them and reschedules (self-rescheduling pattern).

use mud_core::tick::TickScheduler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    Fast,
    Medium,
    Energy,
    Slow,
}

#[test]
fn starts_at_tick_zero_with_nothing_due() {
    let mut sched: TickScheduler<Job> = TickScheduler::new();
    assert_eq!(sched.now(), 0);
    assert_eq!(sched.advance(), vec![]);
    assert_eq!(sched.now(), 1);
}

#[test]
fn job_fires_exactly_after_its_delay() {
    let mut sched = TickScheduler::new();
    sched.schedule_in(3, Job::Medium);
    assert_eq!(sched.advance(), vec![]); // tick 1
    assert_eq!(sched.advance(), vec![]); // tick 2
    assert_eq!(sched.advance(), vec![Job::Medium]); // tick 3
    assert_eq!(sched.advance(), vec![]); // fired once, not again
}

#[test]
fn self_rescheduling_produces_a_steady_cadence() {
    let mut sched = TickScheduler::new();
    sched.schedule_in(5, Job::Energy);
    let mut fired_at = Vec::new();
    for _ in 0..20 {
        for job in sched.advance() {
            assert_eq!(job, Job::Energy);
            fired_at.push(sched.now());
            sched.schedule_in(5, Job::Energy);
        }
    }
    assert_eq!(fired_at, vec![5, 10, 15, 20]);
}

#[test]
fn same_tick_jobs_fire_in_insertion_order() {
    let mut sched = TickScheduler::new();
    sched.schedule_in(1, Job::Fast);
    sched.schedule_in(1, Job::Medium);
    sched.schedule_in(1, Job::Slow);
    assert_eq!(sched.advance(), vec![Job::Fast, Job::Medium, Job::Slow]);
}

#[test]
fn zero_delay_fires_on_the_next_tick() {
    // my_rtkick(0, fn) lands at DAT_0047fb70 + 0 = the current counter; the
    // next perform_kicks drain picks it up.
    let mut sched = TickScheduler::new();
    sched.schedule_in(0, Job::Fast);
    assert_eq!(sched.advance(), vec![Job::Fast]);
}

#[test]
fn interleaved_cadences_stay_independent() {
    let mut sched = TickScheduler::new();
    sched.schedule_in(3, Job::Medium);
    sched.schedule_in(5, Job::Energy);
    let mut log = Vec::new();
    for _ in 0..15 {
        for job in sched.advance() {
            log.push((sched.now(), job));
            match job {
                Job::Medium => sched.schedule_in(3, Job::Medium),
                Job::Energy => sched.schedule_in(5, Job::Energy),
                _ => {}
            }
        }
    }
    assert_eq!(
        log,
        vec![
            (3, Job::Medium),
            (5, Job::Energy),
            (6, Job::Medium),
            (9, Job::Medium),
            (10, Job::Energy),
            (12, Job::Medium),
            // Both due at 15: Energy was enqueued at tick 10, Medium at 12 —
            // insertion order puts Energy first.
            (15, Job::Energy),
            (15, Job::Medium),
        ]
    );
}
