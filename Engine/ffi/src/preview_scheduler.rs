//! PreviewScheduler decides which preview request renders next, without touching the engine.

pub trait Request {
    fn generation(&self) -> u64;
}

/// PreviewScheduler keeps at most one render in flight and remembers only the newest request:
/// a slider fires faster than the engine renders, and every older value is already stale.
pub struct PreviewScheduler<R> {
    in_flight: Option<u64>,
    pending: Option<R>,
    last_delivered: u64,
}

impl<R> Default for PreviewScheduler<R> {
    // Hand-written: a derive would demand `R: Default`.
    fn default() -> Self {
        Self {
            in_flight: None,
            pending: None,
            last_delivered: 0,
        }
    }
}

impl<R: Request> PreviewScheduler<R> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the request to start now, or None when one is already rendering (it becomes pending).
    /// A request older than the pending one is dropped: callers on several threads can send their
    /// generations out of order, and the newest must win.
    pub fn submit(&mut self, request: R) -> Option<R> {
        if self.in_flight.is_some() {
            if self
                .pending
                .as_ref()
                .is_none_or(|p| p.generation() < request.generation())
            {
                self.pending = Some(request);
            }
            return None;
        }
        self.in_flight = Some(request.generation());
        Some(request)
    }

    /// Marks `generation` finished; returns the pending request to start next, if any.
    pub fn finished(&mut self, generation: u64) -> Option<R> {
        if self.in_flight == Some(generation) {
            self.in_flight = None;
        }
        let next = self.pending.take()?;
        self.in_flight = Some(next.generation());
        Some(next)
    }

    /// True when a finished render should reach the callback: newer than anything delivered, and
    /// still for the active photo.
    pub fn should_deliver(
        &mut self,
        generation: u64,
        rendered_photo: u64,
        active_photo: Option<u64>,
    ) -> bool {
        if generation <= self.last_delivered || active_photo != Some(rendered_photo) {
            return false;
        }
        self.last_delivered = generation;
        true
    }

    /// Drops the pending request (used by suspend and shutdown).
    pub fn clear_pending(&mut self) {
        self.pending = None;
    }

    pub fn is_idle(&self) -> bool {
        self.in_flight.is_none() && self.pending.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct R(u64);
    impl Request for R {
        fn generation(&self) -> u64 {
            self.0
        }
    }

    #[test]
    fn idle_scheduler_starts_immediately() {
        let mut s = PreviewScheduler::new();
        assert_eq!(s.submit(R(1)), Some(R(1)));
    }

    #[test]
    fn requests_during_a_render_keep_only_the_newest() {
        let mut s = PreviewScheduler::new();
        s.submit(R(1));
        for g in 2..=10 {
            assert_eq!(s.submit(R(g)), None);
        }
        assert_eq!(s.finished(1), Some(R(10)));
        assert_eq!(s.finished(10), None);
        assert!(s.is_idle());
    }

    #[test]
    fn older_or_foreign_results_are_not_delivered() {
        let mut s: PreviewScheduler<R> = PreviewScheduler::new();
        assert!(s.should_deliver(5, 7, Some(7)));
        assert!(!s.should_deliver(4, 7, Some(7)), "older than delivered");
        assert!(!s.should_deliver(6, 7, Some(8)), "photo no longer active");
        assert!(!s.should_deliver(7, 7, None), "no active photo");
    }

    #[test]
    fn an_older_request_arriving_late_does_not_replace_a_newer_pending_one() {
        let mut s = PreviewScheduler::new();
        s.submit(R(1));
        s.submit(R(5));
        assert_eq!(s.submit(R(4)), None);
        assert_eq!(s.finished(1), Some(R(5)));
    }

    #[test]
    fn clear_pending_drops_the_waiting_request() {
        let mut s = PreviewScheduler::new();
        s.submit(R(1));
        s.submit(R(2));
        s.clear_pending();
        assert_eq!(s.finished(1), None);
    }
}
