//! When to repaint.
//!
//! A model streaming at speed delivers hundreds of deltas a second, and drawing
//! on each one spends the whole terminal's bandwidth redrawing four rows that a
//! human cannot read that fast anyway. Draws are therefore coalesced: a change
//! marks the surface dirty, and the dirt is painted at most once a frame.
//!
//! Time is a parameter rather than a call to the clock, so the pacing can be
//! tested without waiting for it.

use std::time::{Duration, Instant};

/// The shortest gap between two draws. Sixty a second is already more than a
/// terminal can usefully show, and half the budget the design allows.
pub const FRAME: Duration = Duration::from_millis(16);

#[derive(Debug)]
pub struct Clock {
    last: Instant,
    pending: bool,
}

impl Clock {
    pub fn new(now: Instant) -> Self {
        // Behind by a frame, so the first draw happens immediately.
        Self {
            last: now - FRAME,
            pending: true,
        }
    }

    /// Something changed that the viewport shows.
    pub fn mark(&mut self) {
        self.pending = true;
    }

    pub fn pending(&self) -> bool {
        self.pending
    }

    pub fn due(&self, now: Instant) -> bool {
        self.pending && now.duration_since(self.last) >= FRAME
    }

    /// When the next draw may happen. Only meaningful while something is
    /// pending; it is what the loop sleeps until rather than spinning.
    pub fn deadline(&self) -> Instant {
        self.last + FRAME
    }

    pub fn drew(&mut self, now: Instant) {
        self.last = now;
        self.pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_of_changes_costs_a_bounded_number_of_draws() {
        // 2,000 deltas in one second — a fast local model — must not become
        // 2,000 repaints.
        let start = Instant::now();
        let mut clock = Clock::new(start);
        let mut draws = 0;

        for i in 0..2_000u32 {
            let now = start + Duration::from_micros(u64::from(i) * 500);
            clock.mark();
            if clock.due(now) {
                clock.drew(now);
                draws += 1;
            }
        }
        assert!(draws <= 120, "{draws} draws in a second");
        assert!(
            draws > 30,
            "coalescing should not stop the display: {draws}"
        );
    }

    #[test]
    fn nothing_pending_is_never_due() {
        let start = Instant::now();
        let mut clock = Clock::new(start);
        clock.drew(start);
        assert!(!clock.due(start + FRAME * 10));
    }

    #[test]
    fn a_lone_change_draws_without_waiting_a_whole_frame() {
        // Typing must not feel like it lags a frame behind the keystroke.
        let start = Instant::now();
        let mut clock = Clock::new(start);
        clock.drew(start);
        clock.mark();
        assert!(!clock.due(start));
        assert!(clock.due(start + FRAME));
        assert_eq!(clock.deadline(), start + FRAME);
    }
}
