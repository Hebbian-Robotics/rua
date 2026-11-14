//! Fixed-rate cycles against absolute deadlines, so a late cycle does not push
//! every later one back.

use std::time::{Duration, Instant};

/// How much of each wait is spent spinning instead of sleeping, to land on the
/// deadline despite the operating system's timer granularity.
const SPIN_MARGIN: Duration = Duration::from_micros(200);

/// The real-time priority asked of the loop thread.
#[cfg(target_os = "linux")]
const REALTIME_PRIORITY: libc::c_int = 50;

/// Best effort: run the calling thread at real-time priority and lock the
/// whole process's memory resident, so the scheduler and paging do not make cycles late. Not being
/// permitted only costs a warning.
#[cfg(target_os = "linux")]
pub(crate) fn request_realtime_scheduling() {
    let parameters = libc::sched_param {
        sched_priority: REALTIME_PRIORITY,
    };
    // SAFETY: both calls only read their plain arguments.
    let (scheduled, locked) = unsafe {
        (
            libc::sched_setscheduler(0, libc::SCHED_FIFO, &raw const parameters) == 0,
            libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) == 0,
        )
    };
    if !scheduled || !locked {
        eprintln!(
            "Warning: real-time scheduling not fully available (SCHED_FIFO {scheduled}, mlockall {locked}); cycles may run late"
        );
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn request_realtime_scheduling() {
    eprintln!("Warning: real-time scheduling is only supported on Linux; cycles may run late");
}

#[derive(Debug, PartialEq)]
pub(crate) struct DeadlineMissed {
    pub cycle: Duration,
    pub deadline: Duration,
}

pub(crate) struct CycleTimer {
    period: Duration,
    deadline: Duration,
    next_tick: Instant,
    previous_tick: Instant,
}

impl CycleTimer {
    /// Begin timing afresh, for after a pause that was not a late cycle (a
    /// preflight or an enable).
    pub(crate) fn restart(&mut self) {
        let now = Instant::now();
        self.next_tick = now;
        self.previous_tick = now;
    }

    #[must_use]
    pub fn start(frequency_hertz: f64, deadline: Duration) -> Self {
        let now = Instant::now();
        Self {
            period: Duration::from_secs_f64(1.0 / frequency_hertz),
            deadline,
            next_tick: now,
            previous_tick: now,
        }
    }

    /// Wait out the rest of the cycle; the time since the previous tick, or the
    /// deadline miss that the loop must treat as a trip.
    ///
    /// # Errors
    /// `DeadlineMissed` when the cycle took longer than the loop deadline.
    pub(crate) fn tick(&mut self) -> Result<Duration, DeadlineMissed> {
        self.next_tick += self.period;
        let now = Instant::now();
        let wait = self
            .next_tick
            .saturating_duration_since(now)
            .saturating_sub(SPIN_MARGIN);
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        while Instant::now() < self.next_tick {
            std::hint::spin_loop();
        }
        self.record_tick(Instant::now())
    }

    fn record_tick(&mut self, now: Instant) -> Result<Duration, DeadlineMissed> {
        let cycle = now - self.previous_tick;
        self.previous_tick = now;
        // A late cycle does not owe the missed ticks: restart from now.
        if now > self.next_tick + self.period {
            self.next_tick = now;
        }
        if cycle > self.deadline {
            return Err(DeadlineMissed {
                cycle,
                deadline: self.deadline,
            });
        }
        Ok(cycle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_track_their_period_and_reset_after_a_late_tick() {
        let mut timer = CycleTimer::start(200.0, Duration::from_millis(50));
        let start = timer.previous_tick;
        for index in 1..=40 {
            timer.next_tick += timer.period;
            let tick = start + timer.period * index;
            assert_eq!(timer.record_tick(tick).unwrap(), timer.period);
        }
        timer.next_tick += timer.period;
        let late = timer.next_tick + timer.period * 2;
        assert!(timer.record_tick(late).is_ok());
        assert_eq!(timer.next_tick, late);
    }

    #[test]
    fn a_cycle_past_the_deadline_is_reported() {
        let mut timer = CycleTimer::start(1_000.0, Duration::from_millis(2));
        let start = timer.previous_tick;
        timer.next_tick += timer.period;
        let missed = timer
            .record_tick(start + Duration::from_millis(10))
            .unwrap_err();
        assert_eq!(missed.cycle, Duration::from_millis(10));
    }
}
