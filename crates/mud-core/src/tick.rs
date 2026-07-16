//! The tick scheduler — the reimplementation of `my_rtkick` /
//! `perform_kicks` (`re/docs/combat_rounds.md` §1).
//!
//! One tick = one second (the `background_fast` cadence). Jobs are enqueued
//! at `now + delay` and drained once per tick, in insertion order within a
//! tick. Jobs are plain data; callers match and re-enqueue to build the
//! original's self-rescheduling 1/3/5/30 s tiers. Tests (and a fast-forward
//! server mode) advance ticks manually — nothing here touches a wall clock.

use std::collections::BinaryHeap;
use std::cmp::Reverse;

pub struct TickScheduler<J> {
    now: u64,
    /// Reverse((due_tick, seq)) → min-heap; `seq` keeps same-tick FIFO order.
    queue: BinaryHeap<Reverse<(u64, u64, JobBox<J>)>>,
    next_seq: u64,
}

/// Wrapper so `J` needn't be `Ord`: ordering ignores the job itself.
struct JobBox<J>(J);

impl<J> PartialEq for JobBox<J> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl<J> Eq for JobBox<J> {}
impl<J> PartialOrd for JobBox<J> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<J> Ord for JobBox<J> {
    fn cmp(&self, _: &Self) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }
}

impl<J> TickScheduler<J> {
    pub fn new() -> Self {
        TickScheduler {
            now: 0,
            queue: BinaryHeap::new(),
            next_seq: 0,
        }
    }

    /// The current tick count (seconds since boot).
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Enqueues `job` to fire `delay` ticks from now. A delay of 0 fires on
    /// the next `advance` (matching `my_rtkick(0, …)` semantics).
    pub fn schedule_in(&mut self, delay: u64, job: J) {
        let due = self.now + delay;
        self.queue.push(Reverse((due, self.next_seq, JobBox(job))));
        self.next_seq += 1;
    }

    /// Advances one tick and returns the jobs due, in insertion order.
    pub fn advance(&mut self) -> Vec<J> {
        self.now += 1;
        let mut due = Vec::new();
        while let Some(Reverse((tick, _, _))) = self.queue.peek() {
            if *tick > self.now {
                break;
            }
            let Reverse((_, _, JobBox(job))) = self.queue.pop().expect("peeked");
            due.push(job);
        }
        due
    }
}

impl<J> Default for TickScheduler<J> {
    fn default() -> Self {
        Self::new()
    }
}
