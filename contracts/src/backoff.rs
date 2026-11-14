use std::time::Duration;

const FIRST_RETRY_DELAY: Duration = Duration::from_millis(100);

/// How long to wait before retrying something that keeps failing: no wait for
/// the first retry, then a delay that doubles from 100 ms up to a cap.
#[derive(Debug, Clone)]
pub struct Backoff {
    cap: Duration,
    failures_in_a_row: u32,
}

impl Backoff {
    #[must_use]
    pub fn new(cap: Duration) -> Self {
        Self {
            cap,
            failures_in_a_row: 0,
        }
    }

    /// The wait after an attempt that failed having run for `attempt_lasted`.
    /// An attempt that ran at least as long as the cap counts as a fresh
    /// failure, so a long-lived connection that drops is retried at once.
    pub fn next_delay(&mut self, attempt_lasted: Duration) -> Duration {
        if attempt_lasted >= self.cap {
            self.failures_in_a_row = 0;
        }
        let failures_before = self.failures_in_a_row;
        self.failures_in_a_row = failures_before.saturating_add(1);
        match failures_before {
            0 => Duration::ZERO,
            earlier => FIRST_RETRY_DELAY
                .saturating_mul(2_u32.saturating_pow(earlier - 1))
                .min(self.cap),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_at_once_then_doubles_up_to_the_cap() {
        let mut backoff = Backoff::new(Duration::from_secs(1));
        let delays: Vec<_> = (0..6)
            .map(|_| backoff.next_delay(Duration::ZERO).as_millis())
            .collect();
        assert_eq!(delays, [0, 100, 200, 400, 800, 1000]);
    }

    #[test]
    fn a_long_attempt_starts_over() {
        let mut backoff = Backoff::new(Duration::from_secs(1));
        backoff.next_delay(Duration::ZERO);
        backoff.next_delay(Duration::ZERO);
        assert_eq!(backoff.next_delay(Duration::from_secs(5)), Duration::ZERO);
    }
}
