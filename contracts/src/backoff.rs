use backon::{BackoffBuilder, ExponentialBackoff, ExponentialBuilder};
use std::time::Duration;

/// Capped retry delays for long-running station clients and workers.
/// Callers reset the delay when their operation has recovered.
#[derive(Debug)]
pub struct Backoff {
    cap: Duration,
    builder: ExponentialBuilder,
    delays: ExponentialBackoff,
}

impl Backoff {
    #[must_use]
    pub fn new(cap: Duration) -> Self {
        let builder = ExponentialBuilder::default()
            .with_min_delay(Duration::from_millis(100).min(cap))
            .with_max_delay(cap)
            .with_jitter()
            .without_max_times();
        Self {
            cap,
            builder,
            delays: builder.build(),
        }
    }

    pub fn reset(&mut self) {
        self.delays = self.builder.build();
    }

    pub fn next_delay(&mut self) -> Duration {
        // CONSTRAINT: Backon adds jitter after applying the configured cap.
        self.delays.next().unwrap_or(self.cap).min(self.cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_retries_keep_their_cap_and_start_over_after_recovery() {
        let cap = Duration::from_secs(1);
        let mut backoff = Backoff::new(cap);
        let initial_retry = Duration::from_millis(100)..=Duration::from_millis(200);

        assert!(initial_retry.contains(&backoff.next_delay()));
        for _ in 0..16 {
            assert!(backoff.next_delay() <= cap);
        }
        assert!(backoff.next_delay() > *initial_retry.end());

        backoff.reset();
        assert!(initial_retry.contains(&backoff.next_delay()));
    }
}
