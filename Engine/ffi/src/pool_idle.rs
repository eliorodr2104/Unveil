//! One deadline on the engine thread. Work replaces it; no timer callback can outlive a photo.

use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_secs(3);

pub(crate) struct PoolIdle {
    deadline: Option<Instant>,
    enabled: bool,
}

impl Default for PoolIdle {
    fn default() -> Self {
        Self { deadline: None, enabled: true }
    }
}

impl PoolIdle {
    pub(crate) fn completed(&mut self, now: Instant) {
        self.deadline = self.enabled.then_some(now + QUIET);
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.cancel();
    }

    pub(crate) fn cancel(&mut self) {
        self.deadline = None;
    }

    pub(crate) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.deadline.map(|deadline| deadline.saturating_duration_since(now))
    }

    pub(crate) fn take_due(&mut self, now: Instant) -> bool {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_work_arms_one_deadline_and_idle_does_not_poll_again() {
        let now = Instant::now();
        let mut idle = PoolIdle::default();
        assert_eq!(idle.remaining(now), None);
        idle.completed(now);
        assert!(!idle.take_due(now + QUIET - Duration::from_nanos(1)));
        assert!(idle.take_due(now + QUIET));
        assert_eq!(idle.remaining(now + QUIET), None);
        assert!(!idle.take_due(now + QUIET * 10));
    }

    #[test]
    fn new_work_replacement_and_suspend_cancel_the_previous_deadline() {
        let now = Instant::now();
        let mut idle = PoolIdle::default();
        for offset in 0..3 {
            let start = now + QUIET * offset * 3;
            idle.completed(start);
            idle.cancel();
            assert!(!idle.take_due(start + QUIET));
            idle.completed(start + QUIET * 2);
            assert!(!idle.take_due(start + QUIET));
            assert_eq!(idle.remaining(start + QUIET), Some(QUIET * 2));
            idle.cancel();
        }
    }

    #[test]
    fn diagnostic_control_can_disable_idle_without_leaving_a_pending_trim() {
        let now = Instant::now();
        let mut idle = PoolIdle::default();
        idle.completed(now);
        idle.set_enabled(false);
        idle.completed(now);
        assert_eq!(idle.remaining(now), None);
        assert!(!idle.take_due(now + QUIET));
        idle.set_enabled(true);
        idle.completed(now);
        assert!(idle.take_due(now + QUIET));
    }
}
